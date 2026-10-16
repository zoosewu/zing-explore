//! 核心效能基準：排序、篩選、列舉 10 萬檔、NTFS 索引。
//! 列舉測試需要設定 `ZING_BENCH_DIR`（由 tools/perf.ps1 建立的測試資料夾）。

use std::hint::black_box;
use std::sync::atomic::AtomicBool;

use criterion::{Criterion, criterion_group, criterion_main};
use zing_core::enumerate::enumerate;
use zing_core::listing::{ATTR_DIRECTORY, Listing};
use zing_core::sort::{SortKey, SortSpec, filter_order, sorted_order};
use zing_core::ntfs::{Name, ROOT, Record};
use zing_core::text::Pattern;
use zing_core::volindex::VolumeIndex;

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

/// 模擬一個磁碟區的 MFT：`n` 筆記錄，約 1/10 是資料夾，隨機掛在較早的資料夾底下
fn synthetic_mft(n: u64) -> Vec<Record> {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut dirs = vec![ROOT];
    let mut out = Vec::with_capacity(n as usize + 1);
    out.push(Record { frn: ROOT, dir: true, names: vec![Name { parent: ROOT, name: ".".into() }], ..Default::default() });
    for frn in 16..16 + n {
        let dir = frn % 10 == 0;
        let parent = dirs[(rnd() % dirs.len() as u64) as usize];
        out.push(Record {
            frn,
            dir,
            names: vec![Name {
                parent,
                name: match frn % 4 {
                    _ if dir => format!("資料夾 {frn}"),
                    0 => format!("IMG_{frn:07}.JPG"),
                    1 => format!("年度報告 {frn}.docx"),
                    2 => format!("report-{frn}-final.pdf"),
                    _ => format!("lib{frn}.so.1"),
                },
            }],
            size: (!dir).then(|| rnd() % 10_000_000),
            attrs: Some(0x20),
            ..Default::default()
        });
        if dir {
            dirs.push(frn);
        }
    }
    out
}

fn index_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("ntfs");
    group.sample_size(10);
    // 解析：10 萬筆已還原更新序列的記錄
    let raw: Vec<Vec<u8>> = (0..100_000u64)
        .map(|i| {
            let b = zing_core::ntfs::build::Builder::file(i + 16).attrs(0x20).name(5, "report.txt", 1).data(&[0; 100]);
            let mut r = b.bytes();
            zing_core::ntfs::apply_fixup(&mut r, 512, true);
            r
        })
        .collect();
    group.bench_function("parse_100k_records", |b| {
        b.iter(|| raw.iter().enumerate().filter_map(|(i, r)| zing_core::ntfs::parse_record(r, i as u64)).count())
    });
    let mft = synthetic_mft(1_000_000);
    group.bench_function("index_build_1m", |b| {
        b.iter(|| {
            let mut v = VolumeIndex::new(1_000_016, 1);
            for r in &mft {
                v.add(r);
            }
            v.finish();
            v
        })
    });
    let mut v = VolumeIndex::new(1_000_016, 1);
    for r in &mft {
        v.add(r);
    }
    v.finish();
    println!("index_build_1m：{} 個資料夾，索引約 {} MB", v.dir_count(), v.memory() >> 20);
    // 變更日誌：1 萬個檔案變動（每筆都沿上層鏈傳遞）
    let changed: Vec<Record> = mft.iter().filter(|r| !r.dir).take(10_000).cloned().collect();
    group.bench_function("index_update_10k", |b| {
        b.iter(|| {
            for r in &changed {
                v.update(r.frn, Some(r));
            }
        })
    });
    group.bench_function("index_children_root", |b| b.iter(|| v.children(ROOT).unwrap().len()));
    // 全磁碟搜尋：100 萬個名稱全部比對一次（找不到，最壞情況）、萬用字元、中文、非 ASCII 大小寫、
    // 篩選、比對完整路徑（每次重建資料夾路徑快取）
    for (name, q) in [
        ("search_1m_substring", "final zzz"),
        ("search_1m_glob", "img_*7.jpg"),
        ("search_1m_cjk", "報告 9999"),
        ("search_1m_unicode_case", "ÉTÉ"),
        ("search_1m_filters", "ext:jpg size:>1mb"),
        ("search_1m_path", r#""資料夾 1\zzz""#),
    ] {
        let q = zing_core::namequery::NameQuery::parse(q);
        group.bench_function(name, |b| {
            b.iter(|| v.search(&q, &mut zing_core::volindex::SearchState::new("C:"), usize::MAX).len())
        });
    }
    group.finish();
}

fn benches(c: &mut Criterion) {
    let l = synthetic();
    c.bench_function("sort_100k_name", |b| b.iter(|| sorted_order(black_box(&l), SortSpec::default())));
    c.bench_function("sort_100k_modified", |b| {
        b.iter(|| sorted_order(black_box(&l), SortSpec { key: SortKey::Modified, ascending: false }))
    });
    c.bench_function("sort_100k_size", |b| {
        b.iter(|| sorted_order(black_box(&l), SortSpec { key: SortKey::Size, ascending: false }))
    });
    let sorted = sorted_order(&l, SortSpec::default());
    let p = Pattern::new("report_99");
    c.bench_function("filter_100k_substring", |b| b.iter(|| filter_order(black_box(&l), &sorted, &p, true)));
    let g = Pattern::new("*9?.rs");
    c.bench_function("filter_100k_glob", |b| b.iter(|| filter_order(black_box(&l), &sorted, &g, true)));

    if let Ok(dir) = std::env::var("ZING_BENCH_DIR") {
        let cancel = AtomicBool::new(false);
        let mut group = c.benchmark_group("enumerate");
        group.sample_size(20);
        group.bench_function("enumerate_bench_dir", |b| b.iter(|| enumerate(&dir, false, &cancel, |_| {}).unwrap()));
        let interval = std::time::Duration::from_millis(150);
        group.bench_function("dir_size_bench_dir", |b| {
            b.iter(|| zing_core::dirsize::dir_size(&dir, &cancel, interval, |_| {}).unwrap())
        });
        group.finish();
    }
}

criterion_group!(core, benches, index_benches);
criterion_main!(core);
