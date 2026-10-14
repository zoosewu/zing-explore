//! 縮圖：向 Windows 要縮圖（`IShellItemImageFactory`，與檔案總管相同的處理常式與 thumbcache），
//! 資料夾有封面圖（folder.jpg 等）時用封面，設定了外部命令產生器的檔案改用命令產生的 PNG。
//!
//! - 只要求畫面上看得到的格子：繪製時登記，繪製結束時把已不可見、還沒開始的請求從佇列移除。
//! - 本機檔案由兩條背景執行緒處理；網路磁碟、光碟另有一條，一次只取一張，不拖慢本機。
//! - 結果放在記憶體（上限 64 MB，最久沒用的先丟）；外部命令產生的 PNG 另存在磁碟快取，
//!   以「路徑、修改時間、大小、邊長、命令」判斷是否需要重做（啟動程序太慢，不能每次重跑）。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use windows::Win32::Foundation::{CloseHandle, SIZE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CreateProcessW, GetCurrentThread, GetExitCodeProcess,
    PROCESS_INFORMATION, STARTUPINFOW, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, TerminateProcess,
    WaitForSingleObject,
};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
use windows::core::{PCWSTR, PWSTR};
use zing_core::config::Thumbnailer;
use zing_core::path;
use zing_core::text::wide_z;

use crate::icons::IconPixels;
use crate::services::{Msg, Notifier};

/// 記憶體快取上限
const MEMORY_LIMIT: usize = 64 << 20;
/// 磁碟快取（外部命令產生的 PNG）超過這個大小時刪掉最舊的，直到低於 3/4
const DISK_LIMIT: u64 = 256 << 20;
/// 外部命令的時間上限
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// 要求的像素邊長，取不小於需要的一級（同一張圖在相近大小時共用）
const BUCKETS: [u32; 7] = [64, 96, 128, 192, 256, 384, 512];
/// 資料夾封面的檔名（依序）
const COVERS: [&str; 4] = ["folder.jpg", "folder.png", "cover.jpg", "cover.png"];

/// 縮圖的來源（冒煙測試用來確認封面與外部命令有作用）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Shell,
    Cover,
    Command,
    /// Lua 腳本的 thumbnail() 指定的圖片
    Script,
}

pub fn bucket(px: f32) -> u32 {
    BUCKETS.iter().copied().find(|&b| b as f32 >= px).unwrap_or(*BUCKETS.last().unwrap())
}

enum Slot {
    Pending,
    Ready {
        pixels: IconPixels,
        #[cfg_attr(not(feature = "smoke"), allow(dead_code))]
        source: Source,
        used: u64,
    },
    Failed,
}

struct Job {
    id: u32,
    path: String,
    is_dir: bool,
    px: u32,
}

struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    cv: Condvar,
}

impl Queue {
    fn new() -> Self {
        Queue { jobs: Mutex::new(VecDeque::new()), cv: Condvar::new() }
    }

    fn push(&self, j: Job) {
        self.jobs.lock().unwrap().push_back(j);
        self.cv.notify_one();
    }

    fn pop(&self) -> Job {
        let mut q = self.jobs.lock().unwrap();
        loop {
            if let Some(j) = q.pop_front() {
                return j;
            }
            q = self.cv.wait(q).unwrap();
        }
    }
}

struct Shared {
    local: Queue,
    slow: Queue,
    thumbnailers: Mutex<Arc<Vec<Thumbnailer>>>,
    cache_dir: PathBuf,
    pruned: std::sync::Once,
}

#[derive(Default)]
struct Inner {
    /// (路徑小寫, 像素邊長) → id
    ids: HashMap<(String, u32), u32>,
    slots: HashMap<u32, (Slot, (String, u32))>,
    bytes: usize,
    tick: u64,
    /// 這一幀要求過的 id
    wanted: HashSet<u32>,
    /// 被丟掉的 id：繪圖端要一併釋放點陣圖
    evicted: Vec<u32>,
}

pub struct Thumbs {
    inner: RefCell<Inner>,
    shared: Arc<Shared>,
    notifier: Notifier,
    started: std::cell::Cell<bool>,
}

impl Thumbs {
    /// `cache_dir`：外部命令產生的 PNG 放在哪裡（隔離模式用暫存資料夾）
    pub fn new(notifier: Notifier, cache_dir: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            local: Queue::new(),
            slow: Queue::new(),
            thumbnailers: Mutex::new(Arc::new(Vec::new())),
            cache_dir,
            pruned: std::sync::Once::new(),
        });
        Thumbs { inner: RefCell::default(), shared, notifier, started: std::cell::Cell::new(false) }
    }

    pub fn set_thumbnailers(&self, t: Vec<Thumbnailer>) {
        *self.shared.thumbnailers.lock().unwrap() = Arc::new(t);
        // 設定改變：已有的結果可能來自舊的命令，全部重取
        let mut inner = self.inner.borrow_mut();
        let ids: Vec<u32> = inner.slots.keys().copied().collect();
        inner.evicted.extend(ids);
        inner.slots.clear();
        inner.ids.clear();
        inner.bytes = 0;
    }

    /// 繪製時呼叫：已就緒回傳 id；否則送出請求（只送一次）並回傳 None
    pub fn get(&self, path: &str, is_dir: bool, px: u32) -> Option<u32> {
        let mut inner = self.inner.borrow_mut();
        inner.tick += 1;
        let tick = inner.tick;
        let key = (path.to_lowercase(), px);
        if let Some(&id) = inner.ids.get(&key) {
            inner.wanted.insert(id);
            return match &mut inner.slots.get_mut(&id)?.0 {
                Slot::Ready { used, .. } => {
                    *used = tick;
                    Some(id)
                }
                _ => None,
            };
        }
        let id = crate::icons::next_id();
        inner.ids.insert(key.clone(), id);
        inner.slots.insert(id, (Slot::Pending, key));
        inner.wanted.insert(id);
        drop(inner);
        self.start();
        self.shared.local.push(Job { id, path: path.to_string(), is_dir, px });
        None
    }

    fn start(&self) {
        if self.started.replace(true) {
            return;
        }
        for i in 0..2 {
            spawn_worker(format!("zing-thumbs-{i}"), self.shared.clone(), self.notifier.clone(), false);
        }
        spawn_worker("zing-thumbs-slow".into(), self.shared.clone(), self.notifier.clone(), true);
    }

    /// 繪製結束時呼叫：還在佇列裡、這一幀沒有要求的工作取消（之後再看到時重新要求）
    pub fn end_frame(&self) {
        let mut inner = self.inner.borrow_mut();
        let wanted = std::mem::take(&mut inner.wanted);
        let mut dropped = Vec::new();
        for q in [&self.shared.local, &self.shared.slow] {
            q.jobs.lock().unwrap().retain(|j| {
                let keep = wanted.contains(&j.id);
                if !keep {
                    dropped.push(j.id);
                }
                keep
            });
        }
        for id in dropped {
            if let Some((_, key)) = inner.slots.remove(&id) {
                inner.ids.remove(&key);
            }
        }
    }

    pub fn store(&self, id: u32, pixels: Option<IconPixels>, source: Source) {
        let mut inner = self.inner.borrow_mut();
        let tick = inner.tick;
        let Some((slot, _)) = inner.slots.get_mut(&id) else { return };
        let size = pixels.as_ref().map_or(0, |p| p.bgra.len());
        *slot = match pixels {
            Some(pixels) => Slot::Ready { pixels, source, used: tick },
            None => Slot::Failed,
        };
        inner.bytes += size;
        if inner.bytes > MEMORY_LIMIT {
            evict(&mut inner);
        }
    }

    pub fn with_pixels<R>(&self, id: u32, f: impl FnOnce(&IconPixels) -> R) -> Option<R> {
        match &self.inner.borrow().slots.get(&id)?.0 {
            Slot::Ready { pixels, .. } => Some(f(pixels)),
            _ => None,
        }
    }

    #[cfg(feature = "smoke")]
    pub fn source(&self, path: &str, px: u32) -> Option<Source> {
        let inner = self.inner.borrow();
        let id = inner.ids.get(&(path.to_lowercase(), px))?;
        match &inner.slots.get(id)?.0 {
            Slot::Ready { source, .. } => Some(*source),
            _ => None,
        }
    }

    pub fn take_evicted(&self) -> Vec<u32> {
        std::mem::take(&mut self.inner.borrow_mut().evicted)
    }

    /// 已就緒的數量（冒煙測試用）
    #[cfg(feature = "smoke")]
    pub fn ready_count(&self) -> usize {
        self.inner.borrow().slots.values().filter(|(s, _)| matches!(s, Slot::Ready { .. })).count()
    }
}

/// 丟掉最久沒用的，直到低於上限的 3/4
fn evict(inner: &mut Inner) {
    let mut ready: Vec<(u64, u32, usize)> = inner
        .slots
        .iter()
        .filter_map(|(&id, (s, _))| match s {
            Slot::Ready { pixels, used, .. } => Some((*used, id, pixels.bgra.len())),
            _ => None,
        })
        .collect();
    ready.sort_unstable();
    for (_, id, size) in ready {
        if inner.bytes <= MEMORY_LIMIT / 4 * 3 {
            break;
        }
        if let Some((_, key)) = inner.slots.remove(&id) {
            inner.ids.remove(&key);
        }
        inner.bytes -= size;
        inner.evicted.push(id);
    }
}

fn spawn_worker(name: String, shared: Arc<Shared>, n: Notifier, slow: bool) {
    let _ = std::thread::Builder::new().name(name).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
        }
        loop {
            let job = if slow { shared.slow.pop() } else { shared.local.pop() };
            if !slow && is_slow_path(&job.path) {
                shared.slow.push(job);
                continue;
            }
            let (pixels, source) = produce(&shared, &job);
            n.send(Msg::Thumb { id: job.id, pixels, source });
        }
    });
}

/// 網路磁碟、光碟：交給一次只處理一張的執行緒
fn is_slow_path(p: &str) -> bool {
    if p.starts_with(r"\\") {
        return true;
    }
    static TYPES: Mutex<Option<HashMap<u8, u32>>> = Mutex::new(None);
    let Some(&letter) = p.as_bytes().first() else { return false };
    let letter = letter.to_ascii_uppercase();
    let mut guard = TYPES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let t = *map.entry(letter).or_insert_with(|| {
        let root = wide_z(&format!("{}:\\", letter as char));
        unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }
    });
    // DRIVE_REMOTE = 4、DRIVE_CDROM = 5
    t == 4 || t == 5
}

fn produce(shared: &Shared, job: &Job) -> (Option<IconPixels>, Source) {
    zing_core::guard::io("讀取縮圖");
    if let Some(img) = crate::script::thumbnail_rule(&job.path)
        && let Some(p) = shell_image(&img, job.px)
    {
        return (Some(p), Source::Script);
    }
    if !job.is_dir {
        let name: Vec<u16> = path::file_name(&job.path).encode_utf16().collect();
        let list = shared.thumbnailers.lock().unwrap().clone();
        if let Some(t) = list.iter().find(|t| t.matches(&name))
            && let Some(png) = command_thumbnail(shared, t, &job.path, job.px)
            && let Some(p) = shell_image(&png.to_string_lossy(), job.px)
        {
            return (Some(p), Source::Command);
        }
    } else {
        for c in COVERS {
            let cover = path::join(&job.path, c);
            if std::fs::metadata(path::long_form(&cover)).is_ok_and(|m| m.is_file())
                && let Some(p) = shell_image(&cover, job.px)
            {
                return (Some(p), Source::Cover);
            }
        }
    }
    (shell_image(&job.path, job.px), Source::Shell)
}

/// Windows 的縮圖（沒有縮圖處理常式時是大圖示），轉成預乘 alpha 的 BGRA
pub(crate) fn shell_image(p: &str, px: u32) -> Option<IconPixels> {
    let w = wide_z(&path::long_form(p));
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let hbm = factory.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_RESIZETOFIT).ok()?;
        let r = crate::icons::read_bitmap(hbm);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        let (w, h, mut bgra) = r?;
        // 相片類的縮圖不帶 alpha（全為 0）：當成不透明；圖示類的已是預乘 alpha
        if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            for p in bgra.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
        }
        Some(IconPixels { w, h, bgra })
    }
}

/// 64 位元 FNV-1a
fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h ^= 0xff;
    }
    h
}

/// 外部命令產生（或從磁碟快取取得）的 PNG
fn command_thumbnail(shared: &Shared, t: &Thumbnailer, file: &str, px: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(path::long_form(file)).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let key = fnv(&[
        file.to_lowercase().as_bytes(),
        &mtime.to_le_bytes(),
        &meta.len().to_le_bytes(),
        &px.to_le_bytes(),
        t.cmd.as_bytes(),
    ]);
    let dir = &shared.cache_dir;
    let png = dir.join(format!("{key:016x}.png"));
    if png.is_file() {
        return Some(png);
    }
    let _ = std::fs::create_dir_all(dir);
    shared.pruned.call_once(|| prune(dir));
    let tmp = dir.join(format!("{key:016x}.tmp.png"));
    let _ = std::fs::remove_file(&tmp);
    let line = t.command_line(file, &tmp.to_string_lossy(), px);
    if !run(&line) || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    std::fs::rename(&tmp, &png).ok()?;
    Some(png)
}

/// 不顯示視窗、以較低優先權執行命令列，等待結束（逾時就終止）；成功（結束代碼 0）回傳 true
fn run(line: &str) -> bool {
    let mut cmd = wide_z(line);
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        if CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
        .is_err()
        {
            return false;
        }
        let done = WaitForSingleObject(pi.hProcess, COMMAND_TIMEOUT.as_millis() as u32) == WAIT_OBJECT_0;
        let mut code = 1u32;
        if done {
            let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        } else {
            let _ = TerminateProcess(pi.hProcess, 1);
        }
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        done && code == 0
    }
}

/// 磁碟快取超過上限時刪掉最舊的（用程式自己的列舉，不用 std::fs::read_dir：執行檔小約 30 KB）
fn prune(dir: &std::path::Path) {
    let dir = dir.to_string_lossy();
    let never = std::sync::atomic::AtomicBool::new(false);
    let Ok(l) = zing_core::enumerate::enumerate(&dir, false, &never, |_| {}) else { return };
    let total: u64 = l.entries.iter().map(|e| e.size).sum();
    if total <= DISK_LIMIT {
        return;
    }
    let mut order: Vec<usize> = (0..l.len()).collect();
    order.sort_unstable_by_key(|&i| l.entries[i].modified);
    let mut left = total;
    for i in order {
        if left <= DISK_LIMIT / 4 * 3 {
            break;
        }
        if std::fs::remove_file(path::join(&dir, &l.name_string(i))).is_ok() {
            left -= l.entries[i].size;
        }
    }
}

/// 外部命令產生的縮圖放在哪裡：%LOCALAPPDATA%\zing-explore\thumbs；隔離模式用暫存資料夾
pub fn cache_dir(isolated: bool) -> PathBuf {
    if isolated {
        return std::env::temp_dir().join(format!("zing-thumbs-{}", std::process::id()));
    }
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("zing-explore")
        .join("thumbs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        assert_eq!(bucket(10.0), 64);
        assert_eq!(bucket(96.0), 96);
        assert_eq!(bucket(97.0), 128);
        assert_eq!(bucket(320.0), 384);
        assert_eq!(bucket(4000.0), 512);
    }

    #[test]
    fn fnv_differs_by_part_boundary() {
        assert_ne!(fnv(&[b"ab", b"c"]), fnv(&[b"a", b"bc"]));
        assert_eq!(fnv(&[b"x"]), fnv(&[b"x"]));
    }
}
