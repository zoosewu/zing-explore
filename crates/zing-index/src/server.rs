//! 索引服務的主體：每個本機 NTFS 固定磁碟一條執行緒（建立索引、跟著變更日誌更新、儲存快照），
//! 以及回答 zing 查詢的 named pipe。
//!
//! 安全：服務以 LocalSystem 執行，所以每個查詢都先以呼叫者的身分開啟該資料夾（模擬用戶端），
//! 呼叫者無法列出的資料夾一律拒絕；只回答大小與名稱，不提供檔案內容。
//! 全磁碟搜尋的每個結果都先以呼叫者的身分開啟它所在的資料夾，能列出才回傳（等同使用者自己能列舉到的）。
//! 管線只接受本機、已驗證的使用者；快照與記錄檔放在服務帳號（LocalSystem）的 %LOCALAPPDATA%，
//! 只有 SYSTEM 與系統管理員能讀取（快照含有整顆磁碟的資料夾名稱）。

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_CONNECTED, HANDLE,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, RevertToSelf, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FlushFileBuffers, GetDriveTypeW, GetFileInformationByHandle, GetLogicalDrives, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
use windows::core::PCWSTR;
use zing_core::indexproto::{self, Request, Response, SearchItem, VolumeStatus};
use zing_core::namequery::{self, NameQuery, SortKey};
use zing_core::ntfs::{self, Record};
use zing_core::text::wide_z;
use zing_core::volindex::{Found, SearchState, VolumeIndex};

use crate::volume::{JournalError, Volume, volume_info};

const DRIVE_FIXED: u32 = 3;
/// 變更日誌的檢查間隔
const POLL: Duration = Duration::from_secs(1);
/// 有變動時多久存一次快照
const SAVE_EVERY: Duration = Duration::from_secs(600);

pub enum State {
    Scanning(u8),
    Ready(VolumeIndex),
    Failed(String),
}

pub struct Vol {
    pub letter: char,
    pub serial: u32,
    pub state: RwLock<State>,
}

#[derive(Default)]
pub struct Server {
    vols: Mutex<Vec<Arc<Vol>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    stop: AtomicBool,
    /// 主控台模式：記錄同時印到標準錯誤
    pub console: bool,
}

/// 快照與記錄檔的資料夾：服務帳號 LocalSystem 的 %LOCALAPPDATA%\zing-explore。
/// 不用 %ProgramData%：一般使用者可以在那裡先建立同名資料夾，讀取或偽造快照。
/// 以系統管理員身分執行 `--uninstall`、`--console` 時也用同一個位置。
pub fn data_dir() -> PathBuf {
    let windows = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    windows.join(r"System32\config\systemprofile\AppData\Local\zing-explore")
}

/// 以 SDDL 建立安全性描述元（存活到程序結束）
fn security(sddl: &str) -> Option<SECURITY_ATTRIBUTES> {
    let w = wide_z(sddl);
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(w.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
        .ok()?;
    Some(SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    })
}

pub fn ensure_data_dir() -> PathBuf {
    let dir = data_dir();
    // 繼承 systemprofile 的 ACL（只有 SYSTEM 與系統管理員）
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn log(console: bool, msg: &str) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {msg}\n",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    if console {
        eprint!("{line}");
    }
    let path = ensure_data_dir().join("index.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("old.log"));
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn epoch_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_secs()) << 20
}

fn background_mode() {
    // 磁碟與 CPU 的優先權都降到最低，不影響使用者正在做的事
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// 本機固定磁碟中檔案系統為 NTFS 的：(代號, 序號)
pub fn ntfs_fixed_volumes() -> Vec<(char, u32)> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| (b'A' + i) as char)
        .filter_map(|c| {
            let root = format!("{c}:\\");
            let w = wide_z(&root);
            if unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } != DRIVE_FIXED {
                return None;
            }
            let (fs, serial) = volume_info(&root)?;
            (fs == "NTFS").then_some((c, serial))
        })
        .collect()
}

impl Server {
    pub fn new(console: bool) -> Self {
        Server { console, ..Default::default() }
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// 要求停止：結束管線迴圈與各磁碟的執行緒（會先儲存快照）
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // 連到自己的管線，讓等待連線的 ConnectNamedPipe 返回
        let _ = indexproto::query(&Request::Status);
    }

    fn log(&self, msg: &str) {
        log(self.console, msg);
    }

    /// 開始為一個磁碟區建立索引（已在處理時不重複）
    pub fn add_volume(self: &Arc<Self>, letter: char, serial: u32) {
        let mut vols = self.vols.lock().unwrap();
        if vols.iter().any(|v| v.letter == letter && v.serial == serial) {
            return;
        }
        vols.retain(|v| v.letter != letter);
        let vol = Arc::new(Vol { letter, serial, state: RwLock::new(State::Scanning(0)) });
        vols.push(vol.clone());
        let me = self.clone();
        let h = std::thread::Builder::new()
            .name(format!("zing-index-{letter}"))
            .spawn(move || me.volume_main(&vol))
            .expect("無法建立執行緒");
        self.threads.lock().unwrap().push(h);
    }

    /// 等待 `d`，停止時提早返回 true
    fn wait(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.stopping() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.stopping()
    }

    fn snapshot_path(vol: &Vol) -> PathBuf {
        ensure_data_dir().join(format!("{:08X}.zidx", vol.serial))
    }

    fn save(&self, vol: &Vol, journal: (u64, i64)) {
        let path = Self::snapshot_path(vol);
        let tmp = path.with_extension("tmp");
        let t = Instant::now();
        let r = (|| -> std::io::Result<()> {
            let st = vol.state.read().unwrap();
            let State::Ready(idx) = &*st else { return Ok(()) };
            let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
            idx.save(&mut w, journal)?;
            use std::io::Write;
            w.flush()?;
            drop(w);
            std::fs::rename(&tmp, &path)
        })();
        match r {
            Ok(()) => self.log(&format!("{}: 已儲存快照（{} ms）", vol.letter, t.elapsed().as_millis())),
            Err(e) => self.log(&format!("{}: 無法儲存快照：{e}", vol.letter)),
        }
    }

    fn load(&self, vol: &Vol, max_records: usize) -> Option<(VolumeIndex, (u64, i64))> {
        let f = std::fs::File::open(Self::snapshot_path(vol)).ok()?;
        VolumeIndex::load(&mut std::io::BufReader::with_capacity(1 << 20, f), epoch_now(), max_records).ok()
    }

    fn set(&self, vol: &Vol, s: State) {
        *vol.state.write().unwrap() = s;
    }

    fn volume_main(&self, vol: &Vol) {
        background_mode();
        while !self.stopping() {
            let v = match Volume::open(vol.letter) {
                Ok(v) => v,
                Err(e) => {
                    self.log(&format!("{}: 無法開啟磁碟區：{e}", vol.letter));
                    self.set(vol, State::Failed(e));
                    self.wait(Duration::from_secs(60));
                    continue;
                }
            };
            let journal = match v.journal() {
                Ok(j) => j,
                Err(e) => {
                    self.log(&format!("{}: 無法讀取變更日誌：{e}", vol.letter));
                    self.set(vol, State::Failed(e));
                    self.wait(Duration::from_secs(60));
                    continue;
                }
            };
            // MFT 只會變大；快照的記錄數不應超過目前的兩倍（損毀的檔案不能讓我們配置過多記憶體）
            let mut usn = match self.load(vol, v.record_count().saturating_mul(2)) {
                Some((idx, (id, usn))) if id == journal.id && usn >= journal.first => {
                    self.log(&format!("{}: 載入快照，從變更日誌接續", vol.letter));
                    self.set(vol, State::Ready(idx));
                    usn
                }
                _ => {
                    // 先記下目前的日誌位置：掃描期間的變動之後重播（重讀目前狀態，重複套用無害）
                    let start = journal.next;
                    let t = Instant::now();
                    match v.scan(epoch_now(), &self.stop, |p| self.set(vol, State::Scanning(p))) {
                        Ok(idx) => {
                            self.log(&format!(
                                "{}: 索引完成：{} 筆記錄、{} 個資料夾、約 {} MB、{} ms",
                                vol.letter,
                                idx.records(),
                                idx.dir_count(),
                                idx.memory() >> 20,
                                t.elapsed().as_millis()
                            ));
                            self.set(vol, State::Ready(idx));
                            self.save(vol, (journal.id, start));
                            start
                        }
                        Err(e) => {
                            if !self.stopping() {
                                self.log(&format!("{}: 建立索引失敗：{e}", vol.letter));
                                self.set(vol, State::Failed(e));
                                self.wait(Duration::from_secs(60));
                            }
                            continue;
                        }
                    }
                }
            };
            let mut dirty = false;
            let mut last_save = Instant::now();
            loop {
                if self.wait(POLL) {
                    if dirty {
                        self.save(vol, (journal.id, usn));
                    }
                    return;
                }
                let mut frns = Vec::new();
                match v.read_journal(journal.id, usn, &mut frns, 200_000) {
                    Ok(next) => usn = next,
                    Err(JournalError::Reset) => {
                        self.log(&format!("{}: 變更日誌已重設或覆蓋，重新建立索引", vol.letter));
                        break;
                    }
                    Err(JournalError::Other(e)) => {
                        self.log(&format!("{}: 讀取變更日誌失敗：{e}", vol.letter));
                        continue;
                    }
                }
                if !frns.is_empty() {
                    apply_changes(&v, vol, frns);
                    dirty = true;
                }
                if dirty && last_save.elapsed() >= SAVE_EVERY {
                    self.save(vol, (journal.id, usn));
                    dirty = false;
                    last_save = Instant::now();
                }
            }
        }
    }

    /// 執行服務直到停止：開始為所有本機 NTFS 固定磁碟建立索引，並回答查詢
    pub fn run(self: &Arc<Self>) {
        self.log("服務啟動");
        for (c, serial) in ntfs_fixed_volumes() {
            self.add_volume(c, serial);
        }
        self.serve_pipe();
        let threads = std::mem::take(&mut *self.threads.lock().unwrap());
        for t in threads {
            let _ = t.join();
        }
        self.log("服務停止");
    }

    fn serve_pipe(self: &Arc<Self>) {
        // 只允許本機已驗證的使用者讀寫；拒絕遠端連線
        let sa = security("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
        let name = wide_z(indexproto::PIPE_NAME);
        let mut first = true;
        while !self.stopping() {
            let mut open = PIPE_ACCESS_DUPLEX;
            if first {
                // 確保管線是自己建立的，不是其他程式先佔用的
                open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let pipe = unsafe {
                CreateNamedPipeW(
                    PCWSTR(name.as_ptr()),
                    open,
                    PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    1 << 20,
                    64 << 10,
                    0,
                    sa.as_ref().map(|s| s as *const _),
                )
            };
            if pipe.is_invalid() {
                let e = windows::core::Error::from_thread();
                self.log(&format!("無法建立管線：{}", e.message()));
                if first {
                    self.log("另一個程式已使用這個管線名稱，服務結束");
                    self.stop.store(true, Ordering::Relaxed);
                    return;
                }
                self.wait(Duration::from_secs(1));
                continue;
            }
            first = false;
            let connected = match unsafe { ConnectNamedPipe(pipe, None) } {
                Ok(()) => true,
                Err(e) => e.code() == ERROR_PIPE_CONNECTED.to_hresult(),
            };
            if !connected || self.stopping() {
                unsafe {
                    let _ = CloseHandle(pipe);
                }
                continue;
            }
            let me = self.clone();
            let pipe_raw = pipe.0 as usize;
            let spawned = std::thread::Builder::new().name("zing-index-client".into()).spawn(move || {
                let pipe = HANDLE(pipe_raw as *mut _);
                me.handle_client(pipe);
                unsafe {
                    let _ = FlushFileBuffers(pipe);
                    let _ = DisconnectNamedPipe(pipe);
                    let _ = CloseHandle(pipe);
                }
            });
            if spawned.is_err() {
                unsafe {
                    let _ = CloseHandle(pipe);
                }
            }
        }
    }

    fn handle_client(self: &Arc<Self>, pipe: HANDLE) {
        let mut buf = vec![0u8; 64 << 10];
        let mut n = 0u32;
        if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() {
            return;
        }
        let resp = match std::str::from_utf8(&buf[..n as usize]).ok().and_then(Request::decode) {
            Some(Request::Status) => self.status(),
            Some(Request::Children { path, since }) => self.children(pipe, &path, since),
            Some(Request::Search { query, limit }) => self.search(pipe, &query, limit),
            Some(Request::Version) => Response::Version(indexproto::SERVICE_VERSION),
            None => Response::Error(indexproto::UNPARSABLE.into()),
        };
        unsafe {
            let _ = WriteFile(pipe, Some(resp.encode().as_bytes()), None, None);
        }
    }

    fn status(&self) -> Response {
        let vols = self.vols.lock().unwrap().clone();
        Response::Status(
            vols.iter()
                .map(|v| {
                    let st = v.state.read().unwrap();
                    let (state, detail, records, dirs, memory) = match &*st {
                        State::Scanning(p) => ("scanning", p.to_string(), 0, 0, 0),
                        State::Ready(idx) => ("ready", String::new(), idx.records(), idx.dir_count(), idx.memory()),
                        State::Failed(e) => ("error", e.clone(), 0, 0, 0),
                    };
                    VolumeStatus {
                        volume: format!("{}:", v.letter),
                        state: state.into(),
                        detail,
                        records: records as u64,
                        dirs: dirs as u64,
                        memory: memory as u64,
                    }
                })
                .collect(),
        )
    }

    fn children(self: &Arc<Self>, pipe: HANDLE, path: &str, since: u64) -> Response {
        let (serial, frn) = match resolve_as_client(pipe, path) {
            Ok(x) => x,
            Err(r) => return r,
        };
        let vol = self.vols.lock().unwrap().iter().find(|v| v.serial == serial).cloned();
        let Some(vol) = vol else {
            // 開機後才接上的磁碟：第一次查詢時開始建立索引
            let letter = path.chars().next().filter(|c| c.is_ascii_alphabetic() && path[1..].starts_with(":\\"));
            if let Some(c) = letter.map(|c| c.to_ascii_uppercase())
                && let Some((_, s)) = ntfs_fixed_volumes().into_iter().find(|v| v.0 == c && v.1 == serial)
            {
                self.add_volume(c, s);
                return Response::Scanning { percent: 0 };
            }
            return Response::Unindexed;
        };
        let st = vol.state.read().unwrap();
        match &*st {
            State::Scanning(p) => Response::Scanning { percent: *p },
            State::Failed(_) => Response::Unindexed,
            State::Ready(idx) => match idx.dir(frn) {
                None => Response::NotFound,
                Some((_, touched)) if touched == since => Response::Same { touched },
                Some((totals, touched)) => {
                    Response::Children { touched, totals, children: idx.children(frn).unwrap_or_default() }
                }
            },
        }
    }
}

impl Server {
    /// 全磁碟搜尋。沒有指定排序時依磁碟區、記錄編號的順序邊找邊回傳；有排序時先找完所有磁碟區，
    /// 只保留排序在前的 `2 × limit` 個候選（不必保存全部符合的項目），再依序檢查權限。
    /// 每批候選在讀取鎖內取出（含路徑），鎖外以呼叫者身分確認能列出所在的資料夾；
    /// 數量或時間（3 秒）到上限時停止並標記還有更多
    fn search(&self, pipe: HANDLE, query: &str, limit: u32) -> Response {
        let q = NameQuery::parse(query);
        if q.is_empty() {
            return Response::Found { items: Vec::new(), more: false };
        }
        let limit = limit.clamp(1, 5000) as usize;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut vols = self.vols.lock().unwrap().clone();
        vols.sort_by_key(|v| v.letter);
        let mut check = Allowed { pipe, cache: HashMap::new() };
        let Some(sort) = q.sort else {
            let mut items = Vec::new();
            for vol in &vols {
                let mut st = SearchState::new(&format!("{}:", vol.letter));
                while st.next.is_some() {
                    // 在鎖內取得路徑，鎖外以呼叫者身分檢查（會開啟資料夾）
                    let batch: Vec<_> = {
                        let st_lock = vol.state.read().unwrap();
                        let State::Ready(idx) = &*st_lock else { break };
                        idx.search(&q, &mut st, 256).into_iter().filter_map(|f| located(idx, f)).collect()
                    };
                    for (f, rel, parent) in batch {
                        if check.ok(vol, f.parent, &parent) {
                            items.push(item(vol, &f, &rel));
                            if items.len() >= limit {
                                return Response::Found { items, more: true };
                            }
                        }
                    }
                    if st.next.is_some() && Instant::now() >= deadline {
                        return Response::Found { items, more: true };
                    }
                }
            }
            return Response::Found { items, more: false };
        };
        let keep = limit * 2;
        let mut heap = BinaryHeap::with_capacity(keep + 1);
        // 有候選因為超出保留數量而被捨棄，或時間到了
        let mut more = false;
        'vols: for (vi, vol) in vols.iter().enumerate() {
            let mut st = SearchState::new(&format!("{}:", vol.letter));
            while st.next.is_some() {
                {
                    let st_lock = vol.state.read().unwrap();
                    let State::Ready(idx) = &*st_lock else { break };
                    for f in idx.search(&q, &mut st, 4096) {
                        let key = match sort.key {
                            SortKey::Size => Key::Num(f.size),
                            SortKey::Date => Key::Num(f.mtime as u64),
                            SortKey::Name => Key::Text(namequery::fold(idx.name(f.frn).as_bytes())),
                            SortKey::Ext => {
                                let n = namequery::fold(idx.name(f.frn).as_bytes());
                                let mut k = match (f.dir, n.iter().rposition(|&b| b == b'.')) {
                                    (false, Some(i)) => n[i + 1..].to_vec(),
                                    _ => Vec::new(),
                                };
                                // 資料夾排在檔案前面（遞增時）
                                k.insert(0, if f.dir { 0 } else { 1 });
                                k.push(0);
                                k.extend_from_slice(&n);
                                Key::Text(k)
                            }
                            SortKey::Path => match idx.path(f.frn) {
                                Some(p) => Key::Text(namequery::fold(p.as_bytes())),
                                None => continue,
                            },
                        };
                        heap.push(Cand { key, desc: sort.desc, vol: vi, frn: f.frn });
                        if heap.len() > keep {
                            heap.pop();
                            more = true;
                        }
                    }
                }
                if st.next.is_some() && Instant::now() >= deadline {
                    more = true;
                    break 'vols;
                }
            }
        }
        let mut items = Vec::new();
        for c in heap.into_sorted_vec() {
            let vol = &vols[c.vol];
            let located = {
                let st_lock = vol.state.read().unwrap();
                let State::Ready(idx) = &*st_lock else { continue };
                idx.found(c.frn).and_then(|f| located(idx, f))
            };
            let Some((f, rel, parent)) = located else { continue };
            if check.ok(vol, f.parent, &parent) {
                items.push(item(vol, &f, &rel));
                if items.len() >= limit {
                    return Response::Found { items, more: true };
                }
            }
        }
        Response::Found { items, more }
    }
}

/// 排序的候選：`BinaryHeap` 的頂端是排在最後的一個
struct Cand {
    key: Key,
    desc: bool,
    vol: usize,
    frn: u32,
}

/// 同一次搜尋中只會有同一種鍵；文字（已轉成小寫）以自然排序比較
#[derive(PartialEq, Eq)]
enum Key {
    Num(u64),
    Text(Vec<u8>),
}

impl Ord for Key {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        match (self, o) {
            (Key::Num(a), Key::Num(b)) => a.cmp(b),
            (Key::Text(a), Key::Text(b)) => namequery::natural_cmp(a, b),
            (Key::Num(_), Key::Text(_)) => std::cmp::Ordering::Less,
            (Key::Text(_), Key::Num(_)) => std::cmp::Ordering::Greater,
        }
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Cand {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        let k = self.key.cmp(&o.key);
        let k = if self.desc { k.reverse() } else { k };
        k.then((self.vol, self.frn).cmp(&(o.vol, o.frn)))
    }
}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl PartialEq for Cand {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o).is_eq()
    }
}
impl Eq for Cand {}

/// 項目與所在資料夾的路徑（不含磁碟代號）；上層鏈斷掉時為 None
fn located(idx: &VolumeIndex, f: Found) -> Option<(Found, String, String)> {
    let rel = idx.path(f.frn)?;
    let parent = idx.path(f.parent)?;
    Some((f, rel, parent))
}

fn item(vol: &Vol, f: &Found, rel: &str) -> SearchItem {
    let modified = if f.mtime == 0 { 0 } else { (f.mtime as u64 + 11_644_473_600) * 10_000_000 };
    SearchItem { path: format!("{}:\\{rel}", vol.letter), size: f.size, dir: f.dir, hidden: f.hidden, modified }
}

/// 呼叫者能否列出結果所在的資料夾（同一次搜尋中快取）
struct Allowed {
    pipe: HANDLE,
    cache: HashMap<(u32, u32), bool>,
}

impl Allowed {
    fn ok(&mut self, vol: &Vol, parent: u32, parent_path: &str) -> bool {
        let pipe = self.pipe;
        *self.cache.entry((vol.serial, parent)).or_insert_with(|| {
            let dir = format!("{}:\\{parent_path}", vol.letter);
            can_list_as_client(pipe, &dir, vol.serial, parent)
        })
    }
}

/// 呼叫者能否列出這個資料夾，且它就是索引中的那一個（磁碟區序號與記錄編號相同）
fn can_list_as_client(pipe: HANDLE, dir: &str, serial: u32, frn: u32) -> bool {
    matches!(resolve_as_client(pipe, dir), Ok((s, f)) if s == serial && f == frn as u64)
}

/// 以呼叫者的身分開啟資料夾（呼叫者無權列出時拒絕），取得磁碟區序號與記錄編號
fn resolve_as_client(pipe: HANDLE, path: &str) -> Result<(u32, u64), Response> {
    if unsafe { ImpersonateNamedPipeClient(pipe) }.is_err() {
        return Err(Response::Denied);
    }
    let w = wide_z(&zing_core::path::long_form(path));
    let r = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .and_then(|h| {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let r = unsafe { GetFileInformationByHandle(h, &mut info) };
        unsafe {
            let _ = CloseHandle(h);
        }
        r.map(|()| info)
    });
    // 無法恢復成服務自己的身分時不能繼續（之後的動作會以錯誤的身分執行）
    if unsafe { RevertToSelf() }.is_err() {
        std::process::abort();
    }
    match r {
        Ok(info) if FILE_FLAGS_AND_ATTRIBUTES(info.dwFileAttributes).contains(FILE_ATTRIBUTE_DIRECTORY) => {
            let frn = (((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64) & ntfs::FRN_MASK;
            Ok((info.dwVolumeSerialNumber, frn))
        }
        Ok(_) => Err(Response::NotFound),
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(Response::Denied),
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() || e.code() == ERROR_PATH_NOT_FOUND.to_hresult() => {
            Err(Response::NotFound)
        }
        Err(e) => Err(Response::Error(e.message())),
    }
}

/// 套用一批變更：讀取每個項目目前的記錄（不持有鎖），補讀索引中還沒有的上層資料夾，
/// 再一次寫入。上層資料夾先套用，新資料夾裡的新檔案才算得進去。
fn apply_changes(v: &Volume, vol: &Vol, frns: Vec<u64>) {
    let mut seen = HashSet::new();
    let mut batch: Vec<(u64, Option<Record>)> =
        frns.into_iter().filter(|f| seen.insert(*f)).map(|f| (f, v.read_full(f))).collect();
    let parent_of = |r: &Option<Record>| r.as_ref().and_then(|r| r.names.first()).map(|n| n.parent);
    {
        let st = vol.state.read().unwrap();
        let State::Ready(idx) = &*st else { return };
        let (mut i, mut extra) = (0, 0);
        while i < batch.len() && extra < 4096 {
            if let Some(p) = parent_of(&batch[i].1)
                && !idx.has_dir(p)
                && seen.insert(p)
            {
                let r = v.read_full(p);
                batch.push((p, r));
                extra += 1;
            }
            i += 1;
        }
    }
    let pos: HashMap<u64, usize> = batch.iter().enumerate().map(|(i, b)| (b.0, i)).collect();
    let mut done = vec![false; batch.len()];
    let mut st = vol.state.write().unwrap();
    let State::Ready(idx) = &mut *st else { return };
    for i in 0..batch.len() {
        // 依上層鏈找出還沒套用的祖先，由上往下套用
        let mut chain = vec![i];
        while let Some(&j) = parent_of(&batch[*chain.last().unwrap()].1).and_then(|p| pos.get(&p)) {
            if done[j] || chain.contains(&j) || chain.len() > 64 {
                break;
            }
            chain.push(j);
        }
        for &j in chain.iter().rev() {
            if !std::mem::replace(&mut done[j], true) {
                idx.update(batch[j].0, batch[j].1.as_ref());
            }
        }
    }
}
