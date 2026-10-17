//! 把 i18n/*.toml（`"原文" = "翻譯"`）編譯成依原文排序的靜態表（見 src/i18n.rs）。

use std::fmt::Write;

fn main() {
    let mut out = String::new();
    for (name, file) in [("EN", "i18n/en.toml"), ("JA", "i18n/ja.toml")] {
        println!("cargo:rerun-if-changed={file}");
        let src = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{file}: {e}"));
        let table: toml::Table = toml::from_str(&src).unwrap_or_else(|e| panic!("{file}: {e}"));
        let mut pairs: Vec<(String, String)> = table
            .into_iter()
            .map(|(k, v)| match v {
                toml::Value::String(s) => (k, s),
                _ => panic!("{file}: {k} 的值必須是字串"),
            })
            .filter(|(_, v)| !v.is_empty())
            .collect();
        pairs.sort();
        writeln!(out, "static {name}: &[(&str, &str)] = &[").unwrap();
        for (k, v) in pairs {
            writeln!(out, "    ({k:?}, {v:?}),").unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("i18n_tables.rs");
    std::fs::write(dest, out).unwrap();
}
