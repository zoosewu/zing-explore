//! 核心效能基準：排序、篩選、列舉 10 萬檔。
//! 列舉測試需要設定 `FX_BENCH_DIR`（由 tools/perf.ps1 建立的測試資料夾）。

use std::hint::black_box;
use std::sync::atomic::AtomicBool;

use criterion::{Criterion, criterion_group, criterion_main};
use fx_core::enumerate::enumerate;
use fx_core::listing::{ATTR_DIRECTORY, Listing};
use fx_core::sort::{SortKey, SortSpec, filter_order, sorted_order};
use fx_core::text::Pattern;

const N: usize = 100_000;

fn synthetic() -> Listing {
    let mut l = Listing::with_capacity(N);
    let exts = ["txt", "rs", "jpg", "docx", "mp4", "zip"];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..N {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let attrs = if i % 50 == 0 { ATTR_DIRECTORY } else { 0 };
        let name = format!("檔案 {} report_{}.{}", seed % 9973, i, exts[i % exts.len()]);
        l.push_str(&name, attrs, seed % 10_000_000, seed);
    }
    l
}

fn benches(c: &mut Criterion) {
    let l = synthetic();
    c.bench_function("sort_100k_name", |b| b.iter(|| sorted_order(black_box(&l), SortSpec::default())));
    c.bench_function("sort_100k_modified", |b| {
        b.iter(|| sorted_order(black_box(&l), SortSpec { key: SortKey::Modified, ascending: false }))
    });
    let sorted = sorted_order(&l, SortSpec::default());
    let p = Pattern::new("report_99");
    c.bench_function("filter_100k_substring", |b| b.iter(|| filter_order(black_box(&l), &sorted, &p, true)));
    let g = Pattern::new("*9?.rs");
    c.bench_function("filter_100k_glob", |b| b.iter(|| filter_order(black_box(&l), &sorted, &g, true)));

    if let Ok(dir) = std::env::var("FX_BENCH_DIR") {
        let cancel = AtomicBool::new(false);
        let mut group = c.benchmark_group("enumerate");
        group.sample_size(20);
        group.bench_function("enumerate_bench_dir", |b| b.iter(|| enumerate(&dir, false, &cancel, |_| {}).unwrap()));
        group.finish();
    }
}

criterion_group!(core, benches);
criterion_main!(core);
