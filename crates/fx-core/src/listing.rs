//! 緊湊的資料夾內容儲存。
//!
//! 檔名以 UTF-16 集中放在一塊字串池（`names`），每個檔名以 NUL 結尾，
//! 可直接交給 DirectWrite / Win32 API 使用，不需轉換也不需逐筆配置記憶體。

pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_SYSTEM: u32 = 0x4;

#[derive(Clone, Copy, Debug, Default)]
pub struct Entry {
    /// 檔名在字串池中的起點（UTF-16 單位）
    pub name_off: u32,
    /// 檔名長度（UTF-16 單位，不含 NUL）
    pub name_len: u32,
    /// 副檔名在檔名中的起點；無副檔名時等於 `name_len`
    pub ext_off: u32,
    pub attrs: u32,
    pub size: u64,
    /// FILETIME（100ns，自 1601 年起，UTC）
    pub modified: u64,
}

impl Entry {
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.attrs & ATTR_DIRECTORY != 0
    }

    #[inline]
    pub fn is_hidden(&self) -> bool {
        self.attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub names: Vec<u16>,
}

impl Listing {
    pub fn with_capacity(entries: usize) -> Self {
        Self {
            entries: Vec::with_capacity(entries),
            names: Vec::with_capacity(entries * 24),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 檔名（不含 NUL）
    #[inline]
    pub fn name(&self, i: usize) -> &[u16] {
        let e = &self.entries[i];
        &self.names[e.name_off as usize..(e.name_off + e.name_len) as usize]
    }

    /// 檔名（含結尾 NUL），可直接當 PCWSTR 使用
    #[inline]
    pub fn name_z(&self, i: usize) -> &[u16] {
        let e = &self.entries[i];
        &self.names[e.name_off as usize..=(e.name_off + e.name_len) as usize]
    }

    /// 副檔名（不含點）；無副檔名或為資料夾時回傳空切片
    #[inline]
    pub fn ext(&self, i: usize) -> &[u16] {
        let e = &self.entries[i];
        if e.is_dir() || e.ext_off >= e.name_len {
            return &[];
        }
        let start = (e.name_off + e.ext_off + 1) as usize;
        &self.names[start..(e.name_off + e.name_len) as usize]
    }

    pub fn name_string(&self, i: usize) -> String {
        String::from_utf16_lossy(self.name(i))
    }

    pub fn push(&mut self, name: &[u16], attrs: u32, size: u64, modified: u64) {
        let name_off = self.names.len() as u32;
        self.names.extend_from_slice(name);
        self.names.push(0);
        let name_len = name.len() as u32;
        let ext_off = match name.iter().rposition(|&c| c == b'.' as u16) {
            // 以點開頭的檔名（如 .gitignore）視為沒有副檔名
            Some(p) if p > 0 => p as u32,
            _ => name_len,
        };
        self.entries.push(Entry { name_off, name_len, ext_off, attrs, size, modified });
    }

    pub fn push_str(&mut self, name: &str, attrs: u32, size: u64, modified: u64) {
        let w: Vec<u16> = name.encode_utf16().collect();
        self.push(&w, attrs, size, modified);
    }

    /// 依檔名尋找（不分大小寫），用於重新整理後還原選取
    pub fn find(&self, name: &[u16]) -> Option<usize> {
        (0..self.len()).find(|&i| crate::text::eq_ignore_case(self.name(i), name))
    }

    /// 估計記憶體用量（位元組）
    pub fn heap_bytes(&self) -> usize {
        self.entries.capacity() * size_of::<Entry>() + self.names.capacity() * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_rules() {
        let mut l = Listing::default();
        l.push_str("a.txt", 0, 0, 0);
        l.push_str(".gitignore", 0, 0, 0);
        l.push_str("noext", 0, 0, 0);
        l.push_str("dir.d", ATTR_DIRECTORY, 0, 0);
        l.push_str("archive.tar.gz", 0, 0, 0);
        let ext = |i| String::from_utf16_lossy(l.ext(i));
        assert_eq!(ext(0), "txt");
        assert_eq!(ext(1), "");
        assert_eq!(ext(2), "");
        assert_eq!(ext(3), "");
        assert_eq!(ext(4), "gz");
        assert_eq!(l.name_z(0).last(), Some(&0));
    }
}
