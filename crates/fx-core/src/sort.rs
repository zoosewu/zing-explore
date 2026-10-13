//! 排序與篩選：產生「索引陣列」，不搬動 Listing 本身。

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::listing::Listing;
use crate::text::{Pattern, fold, natural_cmp};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "name" => Self::Name,
            "modified" | "date" => Self::Modified,
            "type" | "ext" => Self::Type,
            "size" => Self::Size,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self { key: SortKey::Name, ascending: true }
    }
}

/// 回傳排序後的索引；資料夾永遠排在檔案前面（不受升降冪影響）。
pub fn sorted_order(l: &Listing, spec: SortSpec) -> Vec<u32> {
    if spec.key == SortKey::Name {
        return sorted_by_name(l, spec.ascending);
    }
    let mut order: Vec<u32> = (0..l.len() as u32).collect();
    let by_name = |a: u32, b: u32| natural_cmp(l.name(a as usize), l.name(b as usize));
    let cmp = |&a: &u32, &b: &u32| -> Ordering {
        let (ea, eb) = (&l.entries[a as usize], &l.entries[b as usize]);
        let dir = eb.is_dir().cmp(&ea.is_dir());
        if dir != Ordering::Equal {
            return dir;
        }
        let ord = match spec.key {
            SortKey::Name => by_name(a, b),
            SortKey::Modified => ea.modified.cmp(&eb.modified).then_with(|| by_name(a, b)),
            SortKey::Size if !ea.is_dir() => ea.size.cmp(&eb.size).then_with(|| by_name(a, b)),
            SortKey::Size => by_name(a, b),
            SortKey::Type => natural_cmp(l.ext(a as usize), l.ext(b as usize))
                .then_with(|| by_name(a, b)),
        };
        if spec.ascending { ord } else { ord.reverse() }
    };
    order.sort_unstable_by(cmp);
    order
}

/// 依名稱排序的快速路徑。
///
/// 隨機存取字串池會造成大量快取未命中，所以先為每個項目算出 128 位元的前綴鍵，
/// 對連續的 (鍵, 索引) 陣列排序，只有鍵相同時才做完整的自然排序比較。
/// 前綴鍵與 `natural_cmp` 保持單調一致：鍵較小者，完整比較也必定較小。
fn sorted_by_name(l: &Listing, ascending: bool) -> Vec<u32> {
    const DIR_BIT: u128 = 1 << 127;
    let mut keyed: Vec<(u128, u32)> = (0..l.len())
        .map(|i| {
            // 資料夾的最高位元為 0、檔案為 1，資料夾自然排在前面
            let file = if l.entries[i].is_dir() { 0 } else { DIR_BIT };
            (file | name_prefix_key(l.name(i)) >> 1, i as u32)
        })
        .collect();
    let full = |a: u32, b: u32| natural_cmp(l.name(a as usize), l.name(b as usize));
    if ascending {
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| full(a.1, b.1)));
    } else {
        keyed.sort_unstable_by(|a, b| {
            (a.0 & DIR_BIT)
                .cmp(&(b.0 & DIR_BIT))
                .then_with(|| (b.0 & !DIR_BIT).cmp(&(a.0 & !DIR_BIT)))
                .then_with(|| full(b.1, a.1))
        });
    }
    keyed.into_iter().map(|(_, i)| i).collect()
}

/// 前綴鍵：8 個 16 位元欄位。一般字元放折疊值；遇到數字段時依序放
/// 「'0' 標記、去前導零後的位數、前 4 位數字」，與 `natural_cmp` 的數字比較規則一致，
/// 之後的欄位補 0（相同時交給完整比較）。
fn name_prefix_key(name: &[u16]) -> u128 {
    const SLOTS: usize = 8;
    let is_digit = |c: u16| (b'0' as u16..=b'9' as u16).contains(&c);
    let mut slots = [0u16; SLOTS];
    let mut s = 0;
    let mut i = 0;
    while s < SLOTS && i < name.len() {
        let c = name[i];
        if !is_digit(c) {
            slots[s] = fold(c) as u16;
            s += 1;
            i += 1;
            continue;
        }
        let run_end = name[i..].iter().position(|&c| !is_digit(c)).map_or(name.len(), |p| i + p);
        let run = &name[i..run_end];
        let zeros = run.iter().take_while(|&&c| c == b'0' as u16).count().min(run.len() - 1);
        let trimmed = &run[zeros..];
        slots[s] = b'0' as u16;
        if s + 1 < SLOTS {
            slots[s + 1] = trimmed.len().min(u16::MAX as usize) as u16;
        }
        if s + 2 < SLOTS {
            let mut packed = 0u16;
            for k in 0..4 {
                let d = trimmed.get(k).map_or(0, |&c| c - b'0' as u16);
                packed |= d << (12 - 4 * k);
            }
            slots[s + 2] = packed;
        }
        break;
    }
    slots.iter().fold(0u128, |acc, &v| acc << 16 | v as u128)
}

/// 從已排序索引中篩出符合樣式的項目。
pub fn filter_order(l: &Listing, sorted: &[u32], pattern: &Pattern, show_hidden: bool) -> Vec<u32> {
    if pattern.is_empty() && show_hidden {
        return sorted.to_vec();
    }
    sorted
        .iter()
        .copied()
        .filter(|&i| {
            let i = i as usize;
            (show_hidden || !l.entries[i].is_hidden()) && pattern.matches(l.name(i))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::ATTR_DIRECTORY;

    fn names(l: &Listing, o: &[u32]) -> Vec<String> {
        o.iter().map(|&i| l.name_string(i as usize)).collect()
    }

    /// 快速路徑必須與單純的比較排序結果完全相同
    #[test]
    fn name_fast_path_matches_naive() {
        let alphabet: Vec<char> = "aAbB01 9!_.zZ中文Éé".chars().collect();
        let mut seed: u64 = 12345;
        let mut l = Listing::default();
        for i in 0..3000 {
            let mut s = String::new();
            for _ in 0..(seed % 7) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                s.push(alphabet[(seed >> 33) as usize % alphabet.len()]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            l.push_str(&s, if i % 9 == 0 { ATTR_DIRECTORY } else { 0 }, 0, 0);
        }
        for asc in [true, false] {
            let fast = sorted_order(&l, SortSpec { key: SortKey::Name, ascending: asc });
            let mut naive: Vec<u32> = (0..l.len() as u32).collect();
            naive.sort_by(|&a, &b| {
                let (ea, eb) = (&l.entries[a as usize], &l.entries[b as usize]);
                eb.is_dir().cmp(&ea.is_dir()).then_with(|| {
                    let o = natural_cmp(l.name(a as usize), l.name(b as usize));
                    if asc { o } else { o.reverse() }
                })
            });
            let names = |o: &[u32]| o.iter().map(|&i| l.name_string(i as usize)).collect::<Vec<_>>();
            assert_eq!(names(&fast), names(&naive), "ascending={asc}");
        }
    }

    #[test]
    fn dirs_first_and_natural() {
        let mut l = Listing::default();
        l.push_str("b10.txt", 0, 5, 3);
        l.push_str("b2.txt", 0, 50, 1);
        l.push_str("zdir", ATTR_DIRECTORY, 0, 2);
        l.push_str("a.md", 0, 1, 9);
        let o = sorted_order(&l, SortSpec::default());
        assert_eq!(names(&l, &o), ["zdir", "a.md", "b2.txt", "b10.txt"]);
        let o = sorted_order(&l, SortSpec { key: SortKey::Size, ascending: false });
        assert_eq!(names(&l, &o), ["zdir", "b2.txt", "b10.txt", "a.md"]);
        let o = sorted_order(&l, SortSpec { key: SortKey::Type, ascending: true });
        assert_eq!(names(&l, &o), ["zdir", "a.md", "b2.txt", "b10.txt"]);
        let f = filter_order(&l, &o, &Pattern::new("*.txt"), true);
        assert_eq!(names(&l, &f), ["b2.txt", "b10.txt"]);
    }
}
