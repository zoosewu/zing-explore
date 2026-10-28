//! 檢查新版本（zing-app 的 update.rs）：版號、是否提示、檢查紀錄、GitHub Release 的網址。
//!
//! - 最新版：`/releases/latest` 轉址到 `/releases/tag/<標籤>`，只讀轉址的目標，不用 GitHub API（沒有額度限制、不必解析 JSON）。
//! - 只在主版號或次版號較新時提示（`0.1.0 → 0.1.5` 不提示，`→ 0.2.0` 才提示）；略過的版本以同樣的方式比較，
//!   略過 0.2.0 後 0.2.x 都不再提示，0.3.0 照常提示。
//! - 紀錄（`update.tsv`）：上次檢查的時間、當時的最新版、略過的版本。距上次檢查不到 24 小時就沿用記下的最新版。

pub const REPO: &str = "zoosewu/zing-explore";
/// Release 附的壓縮檔與它的 SHA-256（檔名不含版本，見 .github/workflows/release.yml）
pub const ASSET: &str = "zing-explore-windows-x64.zip";
/// 兩次檢查至少相隔的秒數
pub const INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// `v0.2.0`、`0.2.0`、`0.2`；預先發行版（`0.2.0-rc.1`）與其他格式為 None
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let mut it = s.split('.');
        let num = |p: Option<&str>| -> Option<u32> {
            let p = p?;
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).then(|| p.parse().ok()).flatten()
        };
        let major = num(it.next())?;
        let minor = num(it.next())?;
        let patch = match it.next() {
            Some(p) => num(Some(p))?,
            None => 0,
        };
        it.next().is_none().then_some(Version { major, minor, patch })
    }

    /// 這個執行檔的版本（Cargo.toml 的 workspace.package.version）
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo.toml 的版號格式")
    }

    /// 功能版本（主、次版號）：只有這兩個改變時才提示
    fn feature(self) -> (u32, u32) {
        (self.major, self.minor)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// `latest` 是否值得提示：主或次版號比目前的版本新，也比略過的版本新
pub fn worth_prompting(current: Version, latest: Version, skipped: Option<Version>) -> bool {
    latest.feature() > current.feature() && skipped.is_none_or(|s| latest.feature() > s.feature())
}

/// `/releases/latest` 轉址的目標中的標籤；還沒有任何 Release 時轉到 `/releases`，回傳 None
pub fn tag_from_location(location: &str) -> Option<&str> {
    let (_, tag) = location.split_once("/releases/tag/")?;
    let tag = tag.split(['?', '#', '/']).next()?;
    (!tag.is_empty()).then_some(tag)
}

pub fn latest_url() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// 版本說明（Release 頁面）
pub fn release_page(tag: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/{tag}")
}

/// 指定版本附的檔案（不用 `latest/download`：下載途中發佈新版時，壓縮檔與雜湊才不會分屬兩個版本）
pub fn asset_url(tag: &str, name: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/{name}")
}

/// `.sha256` 檔（`<64 個十六進位字元>  <檔名>`）的雜湊值
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let hex = text.split_whitespace().next()?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// 檢查紀錄（`%LOCALAPPDATA%\zing-explore\update.tsv`，每行 `名稱<Tab>值`）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// 上次成功檢查的時間（Unix 秒）
    pub last_check: u64,
    /// 當時的最新版標籤（沒有任何 Release 時為 None）
    pub latest: Option<String>,
    pub skipped: Option<Version>,
}

impl State {
    pub fn parse(text: &str) -> State {
        let mut s = State::default();
        for line in text.lines() {
            match line.split_once('\t') {
                Some(("last_check", v)) => s.last_check = v.trim().parse().unwrap_or(0),
                Some(("latest", v)) if !v.trim().is_empty() => s.latest = Some(v.trim().to_string()),
                Some(("skipped", v)) => s.skipped = Version::parse(v),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("last_check\t{}\n", self.last_check);
        if let Some(t) = &self.latest {
            out.push_str(&format!("latest\t{t}\n"));
        }
        if let Some(v) = self.skipped {
            out.push_str(&format!("skipped\t{v}\n"));
        }
        out
    }

    /// 該重新檢查了：從沒檢查過、超過 24 小時，或時鐘被調回上次檢查之前
    pub fn due(&self, now: u64) -> bool {
        self.last_check == 0 || now < self.last_check || now - self.last_check >= INTERVAL_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parse_versions() {
        assert_eq!(v("v0.2.0"), Version { major: 0, minor: 2, patch: 0 });
        assert_eq!(v("1.10.3"), Version { major: 1, minor: 10, patch: 3 });
        assert_eq!(v("V2.1"), Version { major: 2, minor: 1, patch: 0 });
        for bad in ["", "v", "1", "1.2.3.4", "1.x.0", "0.2.0-rc.1", "1..2", "+1.2.3", "v 1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn only_feature_releases_prompt() {
        let cur = v("0.1.3");
        assert!(!worth_prompting(cur, v("0.1.9"), None), "修補版不提示");
        assert!(!worth_prompting(cur, v("0.1.0"), None));
        assert!(!worth_prompting(cur, v("0.0.9"), None));
        assert!(worth_prompting(cur, v("0.2.0"), None));
        assert!(worth_prompting(cur, v("1.0.0"), None));
        // 略過 0.2.0：0.2.x 都不提示，0.3.0 照常
        assert!(!worth_prompting(cur, v("0.2.0"), Some(v("0.2.0"))));
        assert!(!worth_prompting(cur, v("0.2.4"), Some(v("0.2.0"))));
        assert!(worth_prompting(cur, v("0.3.0"), Some(v("0.2.0"))));
        // 略過的版本比目前舊（更新後）不影響
        assert!(worth_prompting(v("0.3.0"), v("0.4.0"), Some(v("0.2.0"))));
    }

    #[test]
    fn location_and_urls() {
        let loc = "https://github.com/zoosewu/zing-explore/releases/tag/v0.2.0";
        assert_eq!(tag_from_location(loc), Some("v0.2.0"));
        assert_eq!(tag_from_location("/zoosewu/zing-explore/releases/tag/v1.0.0?x=1"), Some("v1.0.0"));
        assert_eq!(tag_from_location("https://github.com/zoosewu/zing-explore/releases"), None);
        assert_eq!(tag_from_location("https://github.com/zoosewu/zing-explore/releases/tag/"), None);
        assert_eq!(latest_url(), "https://github.com/zoosewu/zing-explore/releases/latest");
        assert_eq!(
            asset_url("v0.2.0", ASSET),
            "https://github.com/zoosewu/zing-explore/releases/download/v0.2.0/zing-explore-windows-x64.zip"
        );
        assert_eq!(release_page("v0.2.0"), loc);
    }

    #[test]
    fn sha256_file() {
        let hex = "00ff10".repeat(10) + "abcd";
        let h = parse_sha256(&format!("{hex}  {ASSET}")).unwrap();
        assert_eq!((h[0], h[1], h[2], h[31]), (0x00, 0xff, 0x10, 0xcd));
        assert_eq!(parse_sha256(&hex.to_uppercase()), Some(h));
        assert_eq!(parse_sha256("abc  x.zip"), None);
        assert_eq!(parse_sha256(&format!("{}zz", &hex[..62])), None);
        assert_eq!(parse_sha256(""), None);
    }

    #[test]
    fn state_round_trip_and_due() {
        let s = State { last_check: 1_000_000, latest: Some("v0.2.0".into()), skipped: Some(v("0.2.0")) };
        assert_eq!(State::parse(&s.to_text()), s);
        assert_eq!(State::parse(""), State::default());
        assert_eq!(State::parse("last_check\tx\nskipped\tnope\nother\t1\n"), State::default());
        assert!(State::default().due(1));
        assert!(!s.due(1_000_000 + INTERVAL_SECS - 1));
        assert!(s.due(1_000_000 + INTERVAL_SECS));
        assert!(s.due(999_999), "時鐘調回");
    }
}
