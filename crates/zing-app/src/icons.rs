//! 系統檔案圖示。
//!
//! `SHGetFileInfoW` 可能讀取磁碟或載入第三方圖示處理常式，所以在專用的 STA 執行緒呼叫，
//! 轉成預乘 alpha 的 BGRA 像素後送回 UI；UI 再建立 D2D 點陣圖（裝置切換時可由像素重建）。
//! 一般檔案以副檔名共用圖示；只有 exe、lnk 等每個檔案圖示不同的類型才逐檔讀取。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use zing_core::listing::Listing;
use zing_core::path;
use zing_core::text::wide_z;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::PCWSTR;

use crate::services::{Msg, Notifier};

/// 每個檔案都有自己圖示的副檔名
const PER_FILE_EXTS: [&str; 8] = ["exe", "lnk", "ico", "url", "cpl", "scr", "msc", "appref-ms"];
const THREADS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconKey {
    Folder,
    /// 依副檔名（小寫）共用
    Ext(String),
    /// 逐檔讀取（執行檔、捷徑、磁碟機）
    Path(String),
}

impl IconKey {
    pub fn for_entry(dir: &str, l: &Listing, i: usize) -> Self {
        if dir.is_empty() {
            return Self::Path(l.name_string(i));
        }
        if l.entries[i].is_dir() {
            return Self::Folder;
        }
        let ext = String::from_utf16_lossy(l.ext(i)).to_lowercase();
        if PER_FILE_EXTS.contains(&ext.as_str()) {
            Self::Path(path::join(dir, &l.name_string(i)))
        } else {
            Self::Ext(ext)
        }
    }
}

/// 預乘 alpha 的 BGRA 像素
pub struct IconPixels {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// UI 端的圖示快取。以 RefCell 包住，讓繪製（&self）時也能登記新的請求。
pub struct Icons {
    inner: RefCell<Inner>,
    notifier: Notifier,
}

#[derive(Default)]
struct Inner {
    ids: HashMap<IconKey, u32>,
    pixels: HashMap<u32, IconPixels>,
    failed: HashSet<u32>,
    tx: Option<Sender<(u32, IconKey)>>,
    /// 清過逐檔圖示快取；繪圖端要一併清掉點陣圖
    evicted: bool,
}

/// 逐檔圖示的快取上限，避免瀏覽大量執行檔時無限成長
const PER_FILE_LIMIT: usize = 5_000;

impl Icons {
    pub fn new(notifier: Notifier) -> Self {
        Self { inner: RefCell::new(Inner::default()), notifier }
    }

    /// 取得已就緒的圖示 id；尚未載入時送出請求並回傳 None（呼叫端先畫替代圖示）
    pub fn get(&self, key: IconKey) -> Option<u32> {
        let mut inner = self.inner.borrow_mut();
        if let Some(&id) = inner.ids.get(&key) {
            return inner.pixels.contains_key(&id).then_some(id);
        }
        if matches!(key, IconKey::Path(_)) && inner.ids.len() > PER_FILE_LIMIT {
            let Inner { ids, pixels, failed, .. } = &mut *inner;
            ids.retain(|k, id| {
                let keep = !matches!(k, IconKey::Path(_));
                if !keep {
                    pixels.remove(id);
                    failed.remove(id);
                }
                keep
            });
            inner.evicted = true;
        }
        let id = next_id();
        inner.ids.insert(key.clone(), id);
        if inner.tx.is_none() {
            inner.tx = Some(spawn_workers(self.notifier.clone()));
        }
        let _ = inner.tx.as_ref().unwrap().send((id, key));
        None
    }

    pub fn store(&self, id: u32, pixels: Option<IconPixels>) {
        let mut inner = self.inner.borrow_mut();
        match pixels {
            Some(p) => {
                inner.pixels.insert(id, p);
            }
            None => {
                inner.failed.insert(id);
            }
        }
    }

    pub fn with_pixels<R>(&self, id: u32, f: impl FnOnce(&IconPixels) -> R) -> Option<R> {
        self.inner.borrow().pixels.get(&id).map(f)
    }

    /// 回傳並清除「曾清除快取」旗標
    pub fn take_evicted(&self) -> bool {
        std::mem::take(&mut self.inner.borrow_mut().evicted)
    }
}

fn next_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn spawn_workers(n: Notifier) -> Sender<(u32, IconKey)> {
    let (tx, rx) = channel::<(u32, IconKey)>();
    let rx = Arc::new(Mutex::new(rx));
    for i in 0..THREADS {
        let rx = rx.clone();
        let n = n.clone();
        let _ = std::thread::Builder::new().name(format!("zing-icons-{i}")).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            loop {
                let job = match rx.lock() {
                    Ok(guard) => guard.recv(),
                    Err(_) => return,
                };
                let Ok((id, key)) = job else { return };
                n.send(Msg::Icon { id, pixels: load(&key) });
            }
        });
    }
    tx
}

fn load(key: &IconKey) -> Option<IconPixels> {
    zing_core::guard::io("讀取檔案圖示");
    let (p, attrs, extra) = match key {
        IconKey::Folder => ("folder".to_string(), FILE_ATTRIBUTE_DIRECTORY, SHGFI_USEFILEATTRIBUTES),
        IconKey::Ext(e) => (format!("x.{e}"), FILE_ATTRIBUTE_NORMAL, SHGFI_USEFILEATTRIBUTES),
        IconKey::Path(p) => (p.clone(), FILE_FLAGS_AND_ATTRIBUTES(0), Default::default()),
    };
    let w = wide_z(&p);
    let mut info = SHFILEINFOW::default();
    // Shell 的圖示快取剛初始化時，SHGetFileInfoW 可能回報成功卻沒有圖示，稍等後重試
    for attempt in 0..4 {
        let ok = unsafe {
            SHGetFileInfoW(PCWSTR(w.as_ptr()), attrs, Some(&mut info), size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_SMALLICON | extra)
        };
        if ok == 0 {
            return None;
        }
        if !info.hIcon.is_invalid() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30 << attempt));
    }
    if info.hIcon.is_invalid() {
        return None;
    }
    let pixels = hicon_to_bgra(info.hIcon);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    pixels
}

fn read_bitmap(hbm: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(hbm.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight.unsigned_abs());
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // 由上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbm, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        (lines > 0).then_some((w, h, buf))
    }
}

fn hicon_to_bgra(icon: HICON) -> Option<IconPixels> {
    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut ii).ok()? };
    let color = if ii.hbmColor.is_invalid() { None } else { read_bitmap(ii.hbmColor) };
    let mask = if ii.hbmMask.is_invalid() { None } else { read_bitmap(ii.hbmMask) };
    unsafe {
        let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
    }
    let (w, h, mut bgra) = color?;
    let has_alpha = bgra.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    if !has_alpha {
        // 舊式圖示沒有 alpha，改用遮罩：遮罩為白色（非零）的像素透明
        let (_, _, m) = mask?;
        for (px, mp) in bgra.as_chunks_mut::<4>().0.iter_mut().zip(m.as_chunks::<4>().0) {
            px[3] = if mp[0] != 0 { 0 } else { 255 };
        }
    }
    // D2D 需要預乘 alpha
    for px in bgra.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        for c in &mut px[..3] {
            *c = (*c as u32 * a / 255) as u8;
        }
    }
    Some(IconPixels { w, h, bgra })
}

