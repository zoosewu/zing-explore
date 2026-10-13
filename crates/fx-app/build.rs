// 以 MSVC 連結器直接嵌入應用程式資訊清單（不需要資源編譯器）
fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("fx.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
}
