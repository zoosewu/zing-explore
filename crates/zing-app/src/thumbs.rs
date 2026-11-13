//! 縮圖：向 Windows 要縮圖（`IShellItemImageFactory`，與檔案總管相同的處理常式與 thumbcache），
//! 資料夾有封面圖（folder.jpg 等）時用封面，設定了外部命令產生器的檔案改用命令產生的 PNG。
//!
//! - 只要求畫面上看得到的格子：繪製時登記，繪製結束時把已不可見、還沒開始的請求從佇列移除。
//! - 本機檔案由兩條背景執行緒處理；網路磁碟、光碟另有一條，一次只取一張，不拖慢本機。
//! - 產生縮圖的執行緒與記憶體中的結果是整個程序共用的（多視窗）：同一張縮圖只解碼一次，所有視窗合計上限 64 MB，
//!   最久沒用的先丟；視窗只持有 `Weak`，丟掉時記憶體真的釋放，視窗在繪製結束時清掉自己的點陣圖。
//!   以清單中的修改時間與大小（`stamp`）判斷檔案是否改寫過。
//!   資料夾的封面不能這樣判斷（上層列舉到的子資料夾修改時間不會因為裡面的變更而更新）：縮圖檢視的分頁監看子資料夾的
//!   內容（`services::watch` 的 `covers`），變更時 `forget`。外部命令產生的 PNG 另存在磁碟快取，
//!   以「路徑、修改時間、大小、邊長、命令」判斷是否需要重做（啟動程序太慢，不能每次重跑）。
//! - 手動指定的縮圖（右鍵「設定縮圖…」）優先於其他來源：圖片複製到 `custom_dir()`，檔名是項目的
//!   「磁碟區序號-NTFS 檔案 ID」，改名或在同一磁碟區內移動後仍有效。沒有任何手動縮圖時不必開檔取得 ID。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
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

/// 記憶體快取上限（所有視窗合計）
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
    /// 使用者手動指定（thumb.set）
    Custom,
}

pub fn bucket(px: f32) -> u32 {
    BUCKETS.iter().copied().find(|&b| b as f32 >= px).unwrap_or(*BUCKETS.last().unwrap())
}

/// 一張解碼好的縮圖。整個程序共用：`CACHE` 持有，視窗只持有 `Weak`，所以快取丟掉它時記憶體真的釋放
pub struct Thumb {
    pixels: IconPixels,
    #[cfg_attr(not(feature = "smoke"), allow(dead_code))]
    source: Source,
    /// 取得時項目的 stamp
    stamp: u64,
    /// 最後一次繪製的時間（`TICK`）：所有視窗共用一個「最久沒用」的順序
    used: AtomicU64,
}

/// 所有視窗共用的記憶體快取（多視窗，hub.rs）：同一張縮圖只解碼一次，合計不超過 `MEMORY_LIMIT`。
/// 鎖只在查表、放入、丟掉時持有一下（繪製時看的是視窗自己的 `Inner`，不碰這個鎖）
#[derive(Default)]
struct Cache {
    /// (路徑小寫, 像素邊長) → 縮圖
    map: HashMap<(String, u32), Arc<Thumb>>,
    bytes: usize,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
static TICK: AtomicU64 = AtomicU64::new(0);
/// 共用的快取丟掉過縮圖的次數：視窗看到它改變就清掉自己指向已釋放縮圖的項目
static DROPS: AtomicU64 = AtomicU64::new(0);

fn cache() -> std::sync::MutexGuard<'static, Option<Cache>> {
    CACHE.lock().unwrap()
}

/// 放進共用的快取，超過上限時丟掉最久沒用的，直到低於上限的 3/4
fn cache_insert(key: (String, u32), thumb: Arc<Thumb>) {
    let mut g = cache();
    let c = g.get_or_insert_with(Cache::default);
    c.bytes += thumb.pixels.bgra.len();
    if let Some(old) = c.map.insert(key, thumb) {
        c.bytes -= old.pixels.bgra.len();
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
    if c.bytes <= MEMORY_LIMIT {
        return;
    }
    let mut by_age: Vec<(u64, (String, u32))> = c.map.iter().map(|(k, t)| (t.used.load(Ordering::Relaxed), k.clone())).collect();
    by_age.sort_unstable();
    for (_, k) in by_age {
        if c.bytes <= MEMORY_LIMIT / 4 * 3 {
            break;
        }
        if let Some(t) = c.map.remove(&k) {
            c.bytes -= t.pixels.bgra.len();
        }
    }
    DROPS.fetch_add(1, Ordering::Relaxed);
}

/// 共用的快取中、stamp 相同的縮圖
fn cache_get(key: &(String, u32), stamp: u64) -> Option<Arc<Thumb>> {
    cache().as_ref()?.map.get(key).filter(|t| t.stamp == stamp).cloned()
}

/// 丟掉共用的快取中符合條件的縮圖
fn cache_remove(mut f: impl FnMut(&(String, u32)) -> bool) {
    let mut g = cache();
    let Some(c) = g.as_mut() else { return };
    let before = c.map.len();
    let mut freed = 0;
    c.map.retain(|k, t| {
        let keep = !f(k);
        if !keep {
            freed += t.pixels.bgra.len();
        }
        keep
    });
    c.bytes -= freed;
    if c.map.len() != before {
        DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

enum Slot {
    Pending,
    Ready(Weak<Thumb>),
    Failed,
}

struct Job {
    id: u32,
    /// 要求的視窗（`Thumbs::owner`）：繪製結束時只取消自己的
    owner: u32,
    n: Notifier,
    path: String,
    is_dir: bool,
    px: u32,
    stamp: u64,
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

/// 整個程序一份：工作佇列與產生縮圖的執行緒（所有視窗共用）、外部命令、手動縮圖
struct Shared {
    local: Queue,
    slow: Queue,
    thumbnailers: Mutex<Arc<Vec<Thumbnailer>>>,
    cache_dir: PathBuf,
    pruned: std::sync::Once,
    custom_dir: PathBuf,
    /// 手動指定的縮圖：檔案鍵 → 圖片；第一次用到時列舉 `custom_dir` 載入
    custom: Mutex<Option<HashMap<String, PathBuf>>>,
    started: std::sync::Once,
}

static SHARED: std::sync::OnceLock<Arc<Shared>> = std::sync::OnceLock::new();

/// 視窗自己的：這個視窗要過的縮圖（id 給繪圖端對應點陣圖）
#[derive(Default)]
struct Inner {
    /// (路徑小寫, 像素邊長) → id
    ids: HashMap<(String, u32), u32>,
    /// id → (結果, 鍵, 要求時項目的 stamp)
    slots: HashMap<u32, (Slot, (String, u32), u64)>,
    /// 這一幀要求過的 id
    wanted: HashSet<u32>,
    /// 被丟掉的 id：繪圖端要一併釋放點陣圖
    evicted: Vec<u32>,
    /// 上次看到的 `DROPS`
    drops_seen: u64,
}

pub struct Thumbs {
    inner: RefCell<Inner>,
    shared: Arc<Shared>,
    notifier: Notifier,
    owner: u32,
}

impl Thumbs {
    /// `cache_dir`：外部命令產生的 PNG 放在哪裡（隔離模式用暫存資料夾）。同一個程序的視窗共用第一個視窗給的資料夾
    pub fn new(notifier: Notifier, cache_dir: PathBuf, custom_dir: PathBuf) -> Self {
        let shared = SHARED
            .get_or_init(|| {
                Arc::new(Shared {
                    local: Queue::new(),
                    slow: Queue::new(),
                    thumbnailers: Mutex::new(Arc::new(Vec::new())),
                    cache_dir,
                    pruned: std::sync::Once::new(),
                    custom_dir,
                    custom: Mutex::new(None),
                    started: std::sync::Once::new(),
                })
            })
            .clone();
        let owner = crate::icons::next_id();
        Thumbs { inner: RefCell::default(), shared, notifier, owner }
    }

    pub fn set_thumbnailers(&self, t: Vec<Thumbnailer>) {
        {
            let mut cur = self.shared.thumbnailers.lock().unwrap();
            // 設定改變（每個視窗都會收到，只有第一個需要清）：已有的結果可能來自舊的命令，全部重取
            if **cur != t {
                *cur = Arc::new(t);
                drop(cur);
                cache_remove(|_| true);
            }
        }
        let mut inner = self.inner.borrow_mut();
        let ids: Vec<u32> = inner.slots.keys().copied().collect();
        inner.evicted.extend(ids);
        inner.slots.clear();
        inner.ids.clear();
    }

    /// 繪製時呼叫：已就緒回傳 id；否則送出請求（只送一次）並回傳 None。
    /// `stamp` 是項目在清單中的修改時間與大小（`stamp()`），與取得時不同就丟掉重新取得。
    /// 其他視窗已取得的縮圖直接用（共用的快取）
    pub fn get(&self, path: &str, is_dir: bool, px: u32, stamp: u64) -> Option<u32> {
        let mut inner = self.inner.borrow_mut();
        let key = (path.to_lowercase(), px);
        if let Some(&id) = inner.ids.get(&key) {
            match inner.slots.get(&id) {
                Some((Slot::Ready(w), _, s)) if *s == stamp => {
                    if let Some(t) = w.upgrade() {
                        t.used.store(TICK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
                        inner.wanted.insert(id);
                        return Some(id);
                    }
                }
                Some((_, _, s)) if *s == stamp => {
                    inner.wanted.insert(id);
                    return None;
                }
                _ => {}
            }
            // 項目改變了，或共用的快取已丟掉這張縮圖
            drop_id(&mut inner, id);
        }
        let id = crate::icons::next_id();
        inner.ids.insert(key.clone(), id);
        inner.wanted.insert(id);
        if let Some(t) = cache_get(&key, stamp) {
            t.used.store(TICK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
            inner.slots.insert(id, (Slot::Ready(Arc::downgrade(&t)), key, stamp));
            return Some(id);
        }
        inner.slots.insert(id, (Slot::Pending, key, stamp));
        drop(inner);
        self.start();
        self.shared.local.push(Job { id, owner: self.owner, n: self.notifier.clone(), path: path.to_string(), is_dir, px, stamp });
        None
    }

    fn start(&self) {
        self.shared.started.call_once(|| {
            for i in 0..2 {
                spawn_worker(format!("zing-thumbs-{i}"), self.shared.clone(), false);
            }
            spawn_worker("zing-thumbs-slow".into(), self.shared.clone(), true);
        });
    }

    /// 繪製結束時呼叫：這個視窗還在佇列裡、這一幀沒有要求的工作取消（之後再看到時重新要求）；
    /// 共用的快取丟掉過縮圖時，清掉指向已釋放縮圖的項目（點陣圖一併釋放）
    pub fn end_frame(&self) {
        let mut inner = self.inner.borrow_mut();
        let wanted = std::mem::take(&mut inner.wanted);
        let mut dropped = Vec::new();
        for q in [&self.shared.local, &self.shared.slow] {
            q.jobs.lock().unwrap().retain(|j| {
                let keep = j.owner != self.owner || wanted.contains(&j.id);
                if !keep {
                    dropped.push(j.id);
                }
                keep
            });
        }
        for id in dropped {
            if let Some((_, key, _)) = inner.slots.remove(&id) {
                inner.ids.remove(&key);
            }
        }
        let drops = DROPS.load(Ordering::Relaxed);
        if drops != inner.drops_seen {
            inner.drops_seen = drops;
            let dead: Vec<u32> =
                inner.slots.iter().filter(|(_, (s, _, _))| matches!(s, Slot::Ready(w) if w.strong_count() == 0)).map(|(&id, _)| id).collect();
            for id in dead {
                drop_id(&mut inner, id);
            }
        }
    }

    /// 產生縮圖的執行緒的結果（已放進共用的快取）
    pub fn store(&self, id: u32, thumb: Option<Arc<Thumb>>) {
        let mut inner = self.inner.borrow_mut();
        let Some((slot, _, _)) = inner.slots.get_mut(&id) else { return };
        *slot = match thumb {
            Some(t) => Slot::Ready(Arc::downgrade(&t)),
            None => Slot::Failed,
        };
    }

    pub fn with_pixels<R>(&self, id: u32, f: impl FnOnce(&IconPixels) -> R) -> Option<R> {
        match &self.inner.borrow().slots.get(&id)?.0 {
            Slot::Ready(w) => Some(f(&w.upgrade()?.pixels)),
            _ => None,
        }
    }

    #[cfg(feature = "smoke")]
    pub fn source(&self, path: &str, px: u32) -> Option<Source> {
        let inner = self.inner.borrow();
        let id = inner.ids.get(&(path.to_lowercase(), px))?;
        match &inner.slots.get(id)?.0 {
            Slot::Ready(w) => Some(w.upgrade()?.source),
            _ => None,
        }
    }

    /// 手動指定 `path` 的縮圖（`image` 為 None 時移除）。在背景複製圖片，完成後送出 `Msg::ThumbCustom`
    pub fn set_custom(&self, path: String, image: Option<String>) {
        let (shared, n) = (self.shared.clone(), self.notifier.clone());
        let _ = std::thread::Builder::new().name("zing-thumb-custom".into()).spawn(move || {
            let result = write_custom(&shared, &path, image.as_deref());
            n.send(Msg::ThumbCustom { path, set: image.is_some(), result });
        });
    }

    /// 丟掉 `path` 已取得的縮圖（所有大小、所有視窗），下次繪製時重新取得
    pub fn forget(&self, path: &str) {
        let lower = path.to_lowercase();
        cache_remove(|(p, _)| *p == lower);
        let mut inner = self.inner.borrow_mut();
        let ids: Vec<u32> = inner.ids.iter().filter(|((p, _), _)| *p == lower).map(|(_, &id)| id).collect();
        for id in ids {
            drop_id(&mut inner, id);
        }
    }

    /// 目前顯示的是手動指定的縮圖（右鍵選單決定是否提供「移除自訂縮圖」）
    pub fn is_custom(&self, path: &str) -> bool {
        let inner = self.inner.borrow();
        let lower = path.to_lowercase();
        inner.ids.iter().any(|((p, _), id)| {
            matches!(inner.slots.get(id), Some((Slot::Ready(w), _, _)) if *p == lower && w.upgrade().is_some_and(|t| t.source == Source::Custom))
        })
    }

    pub fn take_evicted(&self) -> Vec<u32> {
        std::mem::take(&mut self.inner.borrow_mut().evicted)
    }

    /// 已就緒的數量（冒煙測試用）
    #[cfg(feature = "smoke")]
    pub fn ready_count(&self) -> usize {
        self.inner.borrow().slots.values().filter(|(s, _, _)| matches!(s, Slot::Ready(w) if w.strong_count() > 0)).count()
    }

    /// 已經產生過的縮圖數（冒煙測試：另一個視窗用共用的快取，不再產生）
    #[cfg(feature = "smoke")]
    pub fn produced() -> usize {
        PRODUCED.load(Ordering::Relaxed)
    }
}

#[cfg(feature = "smoke")]
static PRODUCED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 項目的 stamp：修改時間與大小（資料夾的大小是 0，修改時間在裡面新增、刪除、改名項目時改變）
pub fn stamp(e: &zing_core::listing::Entry) -> u64 {
    e.modified ^ e.size.rotate_left(32)
}

/// 這個視窗不再用這個結果（還在佇列中的工作完成時找不到 id，結果直接丟棄）
fn drop_id(inner: &mut Inner, id: u32) {
    if let Some((_, key, _)) = inner.slots.remove(&id) {
        inner.ids.remove(&key);
        inner.evicted.push(id);
    }
}

fn spawn_worker(name: String, shared: Arc<Shared>, slow: bool) {
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
            let key = (job.path.to_lowercase(), job.px);
            // 另一個視窗同時要了同一張（排在前面的已做好）
            let thumb = cache_get(&key, job.stamp).or_else(|| {
                #[cfg(feature = "smoke")]
                PRODUCED.fetch_add(1, Ordering::Relaxed);
                let (pixels, source) = produce(&shared, &job);
                let t = Arc::new(Thumb { pixels: pixels?, source, stamp: job.stamp, used: AtomicU64::new(TICK.fetch_add(1, Ordering::Relaxed)) });
                cache_insert(key, t.clone());
                Some(t)
            });
            job.n.send(Msg::Thumb { id: job.id, thumb });
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
    if let Some(img) = custom_image(shared, &job.path)
        && let Some(p) = shell_image(&img.to_string_lossy(), job.px)
    {
        return (Some(p), Source::Custom);
    }
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

/// 項目的鍵：「磁碟區序號-NTFS 檔案 ID」（改名、同一磁碟區內移動後不變）；不支援檔案 ID 的檔案系統為 None
fn file_key(p: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx, OPEN_EXISTING,
    };
    let w = wide_z(&path::long_form(p));
    unsafe {
        let h = CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .ok()?;
        let mut info = FILE_ID_INFO::default();
        let ok = GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut _ as *mut _, size_of::<FILE_ID_INFO>() as u32);
        let _ = CloseHandle(h);
        ok.ok()?;
        let id: String = info.FileId.Identifier.iter().rev().map(|b| format!("{b:02x}")).collect();
        Some(format!("{:016x}-{id}", info.VolumeSerialNumber))
    }
}

/// 載入（第一次）並鎖住手動縮圖的對照表
fn custom_map(shared: &Shared) -> std::sync::MutexGuard<'_, Option<HashMap<String, PathBuf>>> {
    let mut g = shared.custom.lock().unwrap();
    if g.is_none() {
        let mut map = HashMap::new();
        let never = std::sync::atomic::AtomicBool::new(false);
        let dir = shared.custom_dir.to_string_lossy().to_string();
        if let Ok(l) = zing_core::enumerate::enumerate(&dir, false, &never, |_| {}) {
            for i in 0..l.len() {
                let name = l.name_string(i);
                if let Some((key, _)) = name.rsplit_once('.') {
                    map.insert(key.to_string(), shared.custom_dir.join(&name));
                }
            }
        }
        *g = Some(map);
    }
    g
}

fn custom_image(shared: &Shared, p: &str) -> Option<PathBuf> {
    // 沒有任何手動縮圖時不開檔
    if custom_map(shared).as_ref().is_none_or(HashMap::is_empty) {
        return None;
    }
    let key = file_key(p)?;
    custom_map(shared).as_ref()?.get(&key).cloned()
}

/// 複製圖片成為 `p` 的手動縮圖（`image` 為 None 時移除）
fn write_custom(shared: &Shared, p: &str, image: Option<&str>) -> Result<(), String> {
    zing_core::guard::io("設定縮圖");
    let key = file_key(p).ok_or(tr!("無法取得檔案 ID（只支援 NTFS 等有檔案 ID 的磁碟）"))?;
    let mut map = custom_map(shared);
    let map = map.get_or_insert_with(HashMap::new);
    if let Some(old) = map.remove(&key) {
        let _ = std::fs::remove_file(old);
    }
    let Some(image) = image else { return Ok(()) };
    let ext = std::path::Path::new(image).extension().map_or("img".into(), |e| e.to_string_lossy().to_lowercase());
    std::fs::create_dir_all(&shared.custom_dir).map_err(|e| e.to_string())?;
    let dest = shared.custom_dir.join(format!("{key}.{ext}"));
    std::fs::copy(path::long_form(image), &dest).map_err(|e| tr!("無法複製圖片：{e}", e))?;
    map.insert(key, dest);
    Ok(())
}

/// 選擇圖片的對話框（模態，UI 執行緒）
pub fn image_dialog(owner: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    use windows::core::w;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (images, all, title) = (wide_z(tr!("圖片")), wide_z(tr!("所有檔案")), wide_z(tr!("選擇縮圖圖片")));
        let filters = [
            COMDLG_FILTERSPEC { pszName: PCWSTR(images.as_ptr()), pszSpec: w!("*.jpg;*.jpeg;*.png;*.bmp;*.gif;*.webp;*.ico;*.tif;*.tiff;*.heic") },
            COMDLG_FILTERSPEC { pszName: PCWSTR(all.as_ptr()), pszSpec: w!("*.*") },
        ];
        let _ = d.SetFileTypes(&filters);
        let _ = d.SetTitle(PCWSTR(title.as_ptr()));
        d.Show(Some(owner)).ok()?;
        let p = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
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

/// 外部命令產生的縮圖放在哪裡：%LOCALAPPDATA%\zing-explore\thumbs；隔離模式（效能、冒煙測試）用測試資料夾
pub fn cache_dir(isolated: bool) -> PathBuf {
    if isolated {
        return zing_core::testdir::dir().join(format!("zing-thumbs-{}", std::process::id()));
    }
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("zing-explore")
        .join("thumbs")
}

/// 手動指定的縮圖放在哪裡：%LOCALAPPDATA%\zing-explore\custom-thumbs（鍵是這台電腦的檔案 ID，不放在程式旁）；
/// 隔離模式用測試資料夾
pub fn custom_dir(isolated: bool) -> PathBuf {
    if isolated {
        return zing_core::testdir::dir().join(format!("zing-custom-thumbs-{}", std::process::id()));
    }
    std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("zing-explore")
        .join("custom-thumbs")
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

    /// 64×64 的單色 24 位元 BMP
    fn bmp(rgb: [u8; 3]) -> Vec<u8> {
        let (w, h) = (64u32, 64u32);
        let data = w * h * 3;
        let mut v = Vec::new();
        v.extend_from_slice(b"BM");
        v.extend_from_slice(&(54 + data).to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&54u32.to_le_bytes());
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&data.to_le_bytes());
        v.extend_from_slice(&[0; 16]);
        for _ in 0..w * h {
            v.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        v
    }

    /// Windows 的資料夾縮圖在裡面加入圖片、就地改寫圖片後是否跟著更新（thumbcache 不會給舊的）：
    /// `cargo test -p zing-app shell_folder_thumb_updates -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn shell_folder_thumb_updates() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dir = zing_core::testdir::dir().join(format!("zing-folder-thumb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        let empty = shell_image(&d, 256).unwrap().bgra;
        std::fs::write(dir.join("a.bmp"), bmp([255, 0, 0])).unwrap();
        let added = shell_image(&d, 256).unwrap().bgra;
        println!("加入圖片後改變：{}", added != empty);
        std::fs::write(dir.join("a.bmp"), bmp([0, 0, 255])).unwrap();
        let rewritten = shell_image(&d, 256).unwrap().bgra;
        println!("就地改寫後改變：{}", rewritten != added);
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(added, empty, "加入圖片後 Windows 仍給沒有圖片的資料夾縮圖");
    }

    #[test]
    fn fnv_differs_by_part_boundary() {
        assert_ne!(fnv(&[b"ab", b"c"]), fnv(&[b"a", b"bc"]));
        assert_eq!(fnv(&[b"x"]), fnv(&[b"x"]));
    }
}
