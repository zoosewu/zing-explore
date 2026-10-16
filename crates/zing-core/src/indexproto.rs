//! zing 與索引服務（zing-index）之間的 named pipe 協定。
//!
//! 每個連線一問一答，訊息為 UTF-8 文字：第一行是種類與欄位，之後每行一筆，欄位以 Tab 分隔；
//! 名稱中的 `\`、Tab、換行以跳脫序列表示。

use crate::volindex::{Child, Totals};

pub const PIPE_NAME: &str = r"\\.\pipe\zing-explore-index";
/// 服務自己的版號：修改 zing-index（行為、協定、快照格式）時加 1。zing 啟動時比對已安裝的服務，
/// 較舊就以 UAC 更新。1 是還沒有 `Request::Version` 的版本（回答「無法解析請求」）
pub const SERVICE_VERSION: u32 = 3;
/// 回應的上限（子資料夾非常多的資料夾）；超過時截斷並標記
pub const MAX_RESPONSE: usize = 16 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// 資料夾的加總與子資料夾清單；`since` 等於目前的變動標記時只回 `Same`
    Children { path: String, since: u64 },
    Status,
    /// 全磁碟搜尋（`zing_core::namequery` 的語法）：最多 `limit` 個，只含呼叫者能列出的資料夾中的項目
    Search { query: String, limit: u32 },
    /// 服務的版號（`SERVICE_VERSION`）
    Version,
}

/// 搜尋到的一個項目
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchItem {
    pub path: String,
    /// 檔案的大小；資料夾為遞迴加總
    pub size: u64,
    pub dir: bool,
    pub hidden: bool,
    /// 修改時間（FILETIME）；0 表示不知道
    pub modified: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeStatus {
    pub volume: String,
    /// ready、scanning、error
    pub state: String,
    /// 掃描進度（0–100）或錯誤訊息
    pub detail: String,
    pub records: u64,
    pub dirs: u64,
    pub memory: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Children { touched: u64, totals: Totals, children: Vec<Child> },
    Same { touched: u64 },
    /// 磁碟區正在建立索引
    Scanning { percent: u8 },
    /// 不是有索引的磁碟區（非 NTFS、網路磁碟…），改用逐資料夾掃描
    Unindexed,
    /// 呼叫者沒有權限列出這個資料夾
    Denied,
    NotFound,
    Status(Vec<VolumeStatus>),
    /// 搜尋結果；`more` 表示還有更多（達到數量上限或時間上限）
    Found { items: Vec<SearchItem>, more: bool },
    Version(u32),
    Error(String),
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some(c) => o.push(c),
            None => {}
        }
    }
    o
}

impl Request {
    pub fn encode(&self) -> String {
        match self {
            Request::Children { path, since } => format!("children\t{}\t{since}", escape(path)),
            Request::Status => "status".into(),
            Request::Search { query, limit } => format!("search\t{}\t{limit}", escape(query)),
            Request::Version => "version".into(),
        }
    }

    pub fn decode(s: &str) -> Option<Request> {
        let mut f = s.trim_end_matches('\n').split('\t');
        match f.next()? {
            "children" => Some(Request::Children { path: unescape(f.next()?), since: f.next()?.parse().ok()? }),
            "status" => Some(Request::Status),
            "search" => Some(Request::Search { query: unescape(f.next()?), limit: f.next()?.parse().ok()? }),
            "version" => Some(Request::Version),
            _ => None,
        }
    }
}

const SUBDIRS: u32 = 1;
const VISIBLE_SUBDIRS: u32 = 2;
const HIDDEN: u32 = 4;
const UNKNOWN: u32 = 8;

impl Response {
    pub fn encode(&self) -> String {
        match self {
            Response::Children { touched, totals: t, children } => {
                let mut s = format!("children\t{touched}\t{}\t{}\t{}\n", t.bytes, t.files, t.dirs);
                for c in children {
                    let flags = match (c.subdirs, c.visible_subdirs) {
                        (Some(a), Some(v)) => (a as u32 * SUBDIRS) | (v as u32 * VISIBLE_SUBDIRS),
                        _ => UNKNOWN,
                    } | if c.hidden { HIDDEN } else { 0 };
                    let t = c.totals;
                    let line = format!("{}\t{}\t{}\t{}\t{flags}\n", escape(&c.name), t.bytes, t.files, t.dirs);
                    if s.len() + line.len() > MAX_RESPONSE {
                        break;
                    }
                    s.push_str(&line);
                }
                s
            }
            Response::Same { touched } => format!("same\t{touched}"),
            Response::Scanning { percent } => format!("scanning\t{percent}"),
            Response::Unindexed => "unindexed".into(),
            Response::Denied => "denied".into(),
            Response::NotFound => "notfound".into(),
            Response::Status(vols) => {
                let mut s = String::from("status\n");
                for v in vols {
                    s.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\n",
                        escape(&v.volume),
                        v.state,
                        escape(&v.detail),
                        v.records,
                        v.dirs,
                        v.memory
                    ));
                }
                s
            }
            Response::Found { items, more } => {
                let mut s = format!("found\t{}\n", *more as u8);
                for it in items {
                    let flags = (it.dir as u32 * SUBDIRS) | if it.hidden { HIDDEN } else { 0 };
                    let line = format!("{}\t{}\t{flags}\t{}\n", escape(&it.path), it.size, it.modified);
                    if s.len() + line.len() > MAX_RESPONSE {
                        break;
                    }
                    s.push_str(&line);
                }
                s
            }
            Response::Version(v) => format!("version\t{v}"),
            Response::Error(e) => format!("error\t{}", escape(e)),
        }
    }

    pub fn decode(s: &str) -> Option<Response> {
        let mut lines = s.lines();
        let head = lines.next()?;
        let mut f = head.split('\t');
        let kind = f.next()?;
        let mut num = || f.next()?.parse::<u64>().ok();
        Some(match kind {
            "children" => {
                let touched = num()?;
                let totals = Totals { bytes: num()?, files: num()?, dirs: num()? };
                let mut children = Vec::new();
                for line in lines {
                    let mut f = line.split('\t');
                    let name = unescape(f.next()?);
                    let mut num = || f.next()?.parse::<u64>().ok();
                    let totals = Totals { bytes: num()?, files: num()?, dirs: num()? };
                    let flags = num()? as u32;
                    let known = flags & UNKNOWN == 0;
                    children.push(Child {
                        name,
                        totals,
                        subdirs: known.then_some(flags & SUBDIRS != 0),
                        visible_subdirs: known.then_some(flags & VISIBLE_SUBDIRS != 0),
                        hidden: flags & HIDDEN != 0,
                    });
                }
                Response::Children { touched, totals, children }
            }
            "same" => Response::Same { touched: num()? },
            "scanning" => Response::Scanning { percent: num()?.min(100) as u8 },
            "unindexed" => Response::Unindexed,
            "denied" => Response::Denied,
            "notfound" => Response::NotFound,
            "status" => {
                let mut vols = Vec::new();
                for line in lines {
                    let mut f = line.split('\t');
                    let volume = unescape(f.next()?);
                    let state = f.next()?.to_string();
                    let detail = unescape(f.next()?);
                    let mut num = || f.next()?.parse::<u64>().ok();
                    vols.push(VolumeStatus { volume, state, detail, records: num()?, dirs: num()?, memory: num()? });
                }
                Response::Status(vols)
            }
            "found" => {
                let more = num()? != 0;
                let mut items = Vec::new();
                for line in lines {
                    let mut f = line.split('\t');
                    let path = unescape(f.next()?);
                    let size = f.next()?.parse().ok()?;
                    let flags: u32 = f.next()?.parse().ok()?;
                    // 版號 2 的服務沒有修改時間
                    let modified = f.next().and_then(|m| m.parse().ok()).unwrap_or(0);
                    items.push(SearchItem { path, size, dir: flags & SUBDIRS != 0, hidden: flags & HIDDEN != 0, modified });
                }
                Response::Found { items, more }
            }
            "version" => Response::Version(num()? as u32),
            "error" => Response::Error(unescape(head.split_once('\t').map_or("", |x| x.1))),
            _ => return None,
        })
    }
}

/// 查詢失敗的原因
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryError {
    /// 服務沒有安裝或沒有執行
    NotRunning,
    Failed(String),
}

/// 從 `Request::Version` 的結果判斷已安裝服務的版號；沒有安裝、沒有執行或查詢失敗時為 None
pub fn installed_version(r: &Result<Response, QueryError>) -> Option<u32> {
    match r {
        Ok(Response::Version(v)) => Some(*v),
        // 版號 1 的服務不認得這個請求
        Ok(Response::Error(e)) if e == "無法解析請求" => Some(1),
        _ => None,
    }
}

/// 送出一個請求並等待回應（阻塞，在背景執行緒呼叫）。服務只在本機、以呼叫者的身分檢查權限。
pub fn query(req: &Request) -> Result<Response, QueryError> {
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, SECURITY_IMPERSONATION, SECURITY_SQOS_PRESENT,
        WriteFile,
    };
    use windows::Win32::System::Pipes::{PIPE_READMODE_MESSAGE, SetNamedPipeHandleState, WaitNamedPipeW};
    use windows::core::PCWSTR;

    crate::guard::io("查詢索引服務");
    let name = crate::text::wide_z(PIPE_NAME);
    let fail = |e: windows::core::Error| QueryError::Failed(e.message());
    let mut attempt = 0;
    let handle: HANDLE = loop {
        let h = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                // 允許服務以呼叫者身分檢查資料夾權限
                SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION,
                None,
            )
        };
        match h {
            Ok(h) => break h,
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Err(QueryError::NotRunning),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() && attempt < 3 => {
                attempt += 1;
                unsafe {
                    let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 200);
                }
            }
            Err(e) => return Err(fail(e)),
        }
    };
    struct Close(HANDLE);
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let _close = Close(handle);
    unsafe {
        SetNamedPipeHandleState(handle, Some(&PIPE_READMODE_MESSAGE), None, None).map_err(fail)?;
        WriteFile(handle, Some(req.encode().as_bytes()), None, None).map_err(fail)?;
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let mut n = 0u32;
        let r = unsafe { ReadFile(handle, Some(&mut buf), Some(&mut n), None) };
        out.extend_from_slice(&buf[..n as usize]);
        match r {
            Ok(()) => break,
            Err(e) if e.code() == ERROR_MORE_DATA.to_hresult() && out.len() <= MAX_RESPONSE => {}
            Err(e) => return Err(fail(e)),
        }
    }
    let text = String::from_utf8(out).map_err(|_| QueryError::Failed("回應不是 UTF-8".into()))?;
    Response::decode(&text).ok_or_else(|| QueryError::Failed("無法解析回應".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_roundtrip() {
        for r in [
            Request::Children { path: "C:\\a\tb\\新資料夾".into(), since: 42 },
            Request::Status,
            Request::Search { query: "\"年度 報告\" *.docx\t".into(), limit: 500 },
            Request::Version,
        ] {
            assert_eq!(Request::decode(&r.encode()), Some(r));
        }
        assert_eq!(Request::decode("children\tC:\\"), None);
        assert_eq!(Request::decode("bogus"), None);
    }

    #[test]
    fn responses_roundtrip() {
        let child = |name: &str, subdirs, visible, hidden| Child {
            name: name.into(),
            totals: Totals { bytes: 1 << 40, files: 3, dirs: 1 },
            subdirs,
            visible_subdirs: visible,
            hidden,
        };
        let all = [
            Response::Children {
                touched: 9,
                totals: Totals { bytes: 5, files: 6, dirs: 7 },
                children: vec![
                    child("a\\b\tc\nd", Some(true), Some(false), true),
                    child("空", Some(false), Some(false), false),
                    child("junction", None, None, false),
                ],
            },
            Response::Children { touched: 1, totals: Totals::default(), children: vec![] },
            Response::Same { touched: 3 },
            Response::Scanning { percent: 55 },
            Response::Unindexed,
            Response::Denied,
            Response::NotFound,
            Response::Status(vec![VolumeStatus {
                volume: "C:".into(),
                state: "error".into(),
                detail: "存取被拒\t2".into(),
                records: 1,
                dirs: 2,
                memory: 3,
            }]),
            Response::Found {
                items: vec![
                    SearchItem { path: "C:\\a\tb\\相片.jpg".into(), size: 12, dir: false, hidden: true, modified: 133_000_000_000_000_000 },
                    SearchItem { path: "D:\\資料夾".into(), size: 1 << 40, dir: true, hidden: false, modified: 0 },
                ],
                more: true,
            },
            Response::Found { items: vec![], more: false },
            Response::Version(SERVICE_VERSION),
            Response::Error("x\ty".into()),
        ];
        for r in all {
            assert_eq!(Response::decode(&r.encode()), Some(r));
        }
        assert_eq!(Response::decode("children\t1\t2"), None);
        // 版號 2 的服務：搜尋結果沒有修改時間
        let old = Response::decode("found\t0\nC:\\\\a.txt\t5\t0\n").unwrap();
        assert_eq!(old, Response::Found { items: vec![SearchItem { path: "C:\\a.txt".into(), size: 5, dir: false, hidden: false, modified: 0 }], more: false });
    }

    #[test]
    fn installed_versions() {
        assert_eq!(installed_version(&Ok(Response::Version(7))), Some(7));
        assert_eq!(installed_version(&Ok(Response::Error("無法解析請求".into()))), Some(1));
        assert_eq!(installed_version(&Ok(Response::Error("其他".into()))), None);
        assert_eq!(installed_version(&Err(QueryError::NotRunning)), None);
        assert_eq!(installed_version(&Err(QueryError::Failed("x".into()))), None);
    }
}
