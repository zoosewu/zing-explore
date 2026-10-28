// 以 MSVC 連結器直接嵌入應用程式資訊清單；圖示與版本資訊以 Windows SDK 的 rc.exe 編譯成 .res 後連結
use std::path::{Path, PathBuf};

fn main() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let manifest = assets.join("zing.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    // 只有檢查新版本（update.rs，第一幀之後的背景執行緒）用到：延遲到第一次呼叫才載入，不增加啟動時間
    for dll in ["winhttp.dll", "bcrypt.dll"] {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bins=delayimp.lib");

    let rc_file = assets.join("zing.rc");
    println!("cargo:rerun-if-changed={}", rc_file.display());
    println!("cargo:rerun-if-changed={}", assets.join("zing.ico").display());
    println!("cargo:rerun-if-env-changed=RC");
    let rc = find_rc().expect("找不到 rc.exe：請安裝 Windows SDK，或以環境變數 RC 指定路徑");
    let res = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("zing.res");
    let status = std::process::Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc_file)
        .current_dir(&assets)
        .status()
        .expect("無法執行 rc.exe");
    assert!(status.success(), "rc.exe 編譯資源失敗");
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// 環境變數 RC，否則取 Windows Kits 10 中版本最新、符合主機架構的 rc.exe
fn find_rc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RC").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits).ok()?.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions.into_iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.is_file())
}
