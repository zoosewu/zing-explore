//! zing-index：zing-explore 的 NTFS 索引服務。
//!
//! 以 Windows 服務（LocalSystem）執行，讀取每個本機 NTFS 固定磁碟的主檔案表（MFT）建立資料夾大小索引，
//! 之後跟著變更日誌（USN Journal）更新；zing 以一般使用者身分透過 named pipe 查詢。

#[macro_use]
extern crate zing_core;

mod server;
mod service;
mod volume;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use zing_core::indexproto::{self, QueryError, Request, Response};
use zing_core::volindex::VolumeIndex;

const USAGE: &str = tr_key!("zing-index：zing-explore 的 NTFS 索引服務

  zing-index --install                 安裝（或更新）並啟動服務，需要時跳出 UAC
  zing-index --uninstall               停止並移除服務，刪除快照與記錄檔
  zing-index --status                  顯示服務與各磁碟的索引狀態
  zing-index --version                 顯示這個執行檔的服務版號
  zing-index --scan C: [--verify 資料夾]
                                       （系統管理員）讀取一次 MFT，顯示耗時與記憶體；
                                       --verify 以逐資料夾掃描比對該資料夾與其子資料夾的大小
  zing-index --console                 （系統管理員）在前景執行服務，偵錯用
  zing-index --service                 由服務控制管理員使用
");

/// 安裝或移除服務
#[derive(Clone, Copy)]
enum Action {
    Install,
    Uninstall,
}

impl Action {
    fn done(self) -> &'static str {
        match self {
            Action::Install => tr!("已安裝服務"),
            Action::Uninstall => tr!("已移除服務"),
        }
    }

    fn failed(self, e: &str) -> String {
        match self {
            Action::Install => tr!("安裝失敗：{e}", e),
            Action::Uninstall => tr!("移除失敗：{e}", e),
        }
    }
}

fn main() {
    // 服務以 LocalSystem 執行時為系統的預設語言
    zing_core::i18n::set(zing_core::i18n::system());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("--service") => match service::dispatch() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{}", tr!("只能由服務控制管理員啟動：{e}", e));
                1
            }
        },
        Some("--install") => elevated_action("--install", Action::Install, service::install),
        Some("--uninstall") => elevated_action("--uninstall", Action::Uninstall, service::uninstall),
        Some("--status") => status(),
        Some("--version") => {
            println!("{}", indexproto::SERVICE_VERSION);
            0
        }
        Some("--scan") => scan(&args[1..]),
        Some("--console") => {
            if !service::is_elevated() {
                eprintln!("{}", tr!("需要系統管理員權限：請在系統管理員終端機執行"));
                1
            } else {
                Arc::new(server::Server::new(true)).run();
                0
            }
        }
        _ => {
            print!("{}", zing_core::i18n::tr(USAGE));
            if args.is_empty() { 0 } else { 1 }
        }
    };
    std::process::exit(code);
}

/// 已是系統管理員時直接執行；否則以 UAC 重新執行自己。結束代碼：0 成功、1 失敗、2 使用者取消
fn elevated_action(flag: &str, action: Action, f: fn() -> Result<(), String>) -> i32 {
    if service::is_elevated() {
        return match f() {
            Ok(()) => {
                println!("{}", action.done());
                0
            }
            Err(e) => {
                let msg = action.failed(&e);
                server::log(false, &msg);
                eprintln!("{msg}");
                1
            }
        };
    }
    match service::run_elevated(flag) {
        Ok(Some(0)) => {
            println!("{}", action.done());
            0
        }
        Ok(Some(c)) => {
            let log = server::data_dir().join("index.log");
            eprintln!("{}", action.failed(&tr!("結束代碼 {c}，詳見 {log}", c, log = log.display())));
            1
        }
        Ok(None) => {
            eprintln!("{}", tr!("已取消"));
            2
        }
        Err(e) => {
            eprintln!("{}", tr!("無法以系統管理員身分執行：{e}", e));
            1
        }
    }
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
}

fn gb(b: u64) -> String {
    format!("{:.2} GB", b as f64 / (1u64 << 30) as f64)
}

fn status() -> i32 {
    match indexproto::query(&Request::Status) {
        Ok(Response::Status(vols)) => {
            let running = match indexproto::query(&Request::Version) {
                Ok(Response::Version(v)) => v,
                _ => 1,
            };
            let note = if running < indexproto::SERVICE_VERSION { tr!("，比這個執行檔舊：zing-index --install 更新") } else { "" };
            println!("{}", tr!("服務執行中（版號 {running}{note}）", running, note));
            for v in vols {
                let line = match v.state.as_str() {
                    "ready" => tr!(
                        "就緒：{records} 筆記錄、{dirs} 個資料夾、索引約 {memory}",
                        records = v.records,
                        dirs = v.dirs,
                        memory = mb(v.memory)
                    ),
                    "scanning" => tr!("建立索引中 {percent}%", percent = v.detail),
                    _ => tr!("錯誤：{detail}", detail = v.detail),
                };
                println!("  {}  {line}", v.volume);
            }
            0
        }
        Ok(other) => {
            eprintln!("{}", tr!("非預期的回應：{other}", other = format!("{other:?}")));
            1
        }
        Err(QueryError::NotRunning) => {
            println!("{}", tr!("服務沒有執行（安裝：zing-index --install）"));
            1
        }
        Err(QueryError::Failed(e)) => {
            eprintln!("{}", tr!("查詢失敗：{e}", e));
            1
        }
    }
}

/// 資料夾的記錄編號（系統管理員身分開啟）
fn dir_frn(path: &str) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };
    let w = zing_core::text::wide_z(path);
    unsafe {
        let h = CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let r = GetFileInformationByHandle(h, &mut info);
        let _ = CloseHandle(h);
        r.ok()?;
        Some((((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64) & zing_core::ntfs::FRN_MASK)
    }
}

fn scan(args: &[String]) -> i32 {
    let Some(letter) = args.first().and_then(|a| a.chars().next()).filter(|c| c.is_ascii_alphabetic()) else {
        eprint!("{}", zing_core::i18n::tr(USAGE));
        return 1;
    };
    let letter = letter.to_ascii_uppercase();
    let verify = args.iter().position(|a| a == "--verify").and_then(|i| args.get(i + 1)).cloned();
    if !service::is_elevated() {
        eprintln!("{}", tr!("讀取 MFT 需要系統管理員權限：請在系統管理員終端機執行"));
        return 1;
    }
    let v = match volume::Volume::open(letter) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", tr!("無法開啟 {letter}:：{e}", letter, e));
            return 1;
        }
    };
    println!(
        "{}",
        tr!(
            "{letter}:  叢集 {cluster} B、記錄 {record} B、MFT {count} 筆記錄（{size}）",
            letter,
            cluster = v.cluster,
            record = v.record,
            count = v.record_count(),
            size = mb((v.record_count() * v.record) as u64)
        )
    );
    let t = Instant::now();
    let cancel = AtomicBool::new(false);
    let idx = match v.scan(1, &cancel, |p| {
        use std::io::Write;
        eprint!("\r{}", tr!("讀取 MFT… {p}%", p = format!("{p:3}")));
        let _ = std::io::stderr().flush();
    }) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("\n{}", tr!("建立索引失敗：{e}", e));
            return 1;
        }
    };
    let elapsed = t.elapsed();
    eprintln!();
    println!(
        "{}",
        tr!(
            "完成：{ms} ms，{dirs} 個資料夾，索引約 {memory}",
            ms = elapsed.as_millis(),
            dirs = idx.dir_count(),
            memory = mb(idx.memory() as u64)
        )
    );
    match v.journal() {
        Ok(j) => println!("{}", tr!("變更日誌：ID {id}，目前位置 {next}", id = format!("{:X}", j.id), next = j.next)),
        Err(e) => println!("{}", tr!("變更日誌：無法讀取（{e}）", e)),
    }
    let root = zing_core::ntfs::ROOT;
    if let Some((t, _)) = idx.dir(root) {
        let total = tr!("合計 {size}，{files} 個檔案，{dirs} 個資料夾", size = gb(t.bytes), files = t.files, dirs = t.dirs);
        println!("{letter}:\\ {total}");
    }
    let mut kids = idx.children(root).unwrap_or_default();
    kids.sort_by_key(|c| std::cmp::Reverse(c.totals.bytes));
    for c in kids.iter().take(15) {
        println!("  {:>12}  {}", gb(c.totals.bytes), c.name);
    }
    if let Some(path) = verify {
        return verify_dir(&idx, &path);
    }
    0
}

/// 以逐資料夾掃描（zing 的備案）比對索引的結果
fn verify_dir(idx: &VolumeIndex, path: &str) -> i32 {
    let Some(frn) = dir_frn(path) else {
        eprintln!("{}", tr!("無法開啟 {path}", path));
        return 1;
    };
    let Some((total, _)) = idx.dir(frn) else {
        eprintln!("{}", tr!("索引中沒有 {path}", path));
        return 1;
    };
    let cancel = AtomicBool::new(false);
    let walk = |p: &str| zing_core::dirsize::dir_size(p, &cancel, std::time::Duration::from_secs(3600), |_| {});
    let t = Instant::now();
    let Some(w) = walk(path) else { return 1 };
    println!("\n{}", tr!("比對 {path}（逐資料夾掃描 {ms} ms）", path, ms = t.elapsed().as_millis()));
    println!("  {:<24}{:>16}{:>16}{:>12}", "", tr!("索引"), tr!("逐資料夾掃描"), tr!("差異"));
    let row = |name: &str, a: u64, b: u64, denied: u32| {
        let note = if denied > 0 { tr!("（{denied} 個資料夾無法存取）", denied) } else { String::new() };
        println!("  {name:<24}{:>16}{:>16}{:>12}{note}", gb(a), gb(b), format!("{:+}", a as i64 - b as i64));
    };
    row(tr!("合計"), total.bytes, w.bytes, w.denied);
    println!(
        "  {}",
        tr!(
            "檔案數：索引 {a}、掃描 {b}；資料夾數：索引 {c}、掃描 {d}",
            a = total.files,
            b = w.files,
            c = total.dirs,
            d = w.dirs
        )
    );
    let mut kids = idx.children(frn).unwrap_or_default();
    kids.sort_by_key(|c| std::cmp::Reverse(c.totals.bytes));
    for c in kids.iter().take(20) {
        if let Some(s) = walk(&zing_core::path::join(path, &c.name)) {
            row(&c.name, c.totals.bytes, s.bytes, s.denied);
        }
    }
    println!("  {}", tr!("（差異來自硬連結：索引每個檔案只算一次，逐資料夾掃描每個連結都算；以及無法存取的資料夾）"));
    0
}
