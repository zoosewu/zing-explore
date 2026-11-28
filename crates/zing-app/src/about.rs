//! 「關於」頁的作者的其他專案（zoo-release 的 docs/UI.md；資料是 apex-code 的專案清單）。
//!
//! - 進入「關於」分頁時才讀取（`App::load_other_projects`），在自己的執行緒：先用一天內的快取
//!   （`%LOCALAPPDATA%\zing-explore\projects\`），否則下載 `projects.json` 與每個專案的封面（第一張截圖），
//!   再以 Shell 的縮圖解碼成點陣圖（`thumbs::shell_image`）。
//! - 依 apex-code 的 API 指引：不認得的欄位略過；介面語言是繁體中文時用 `zh-TW`，其他用 `en`；排除自己。
//! - 讀取失敗就不顯示整個區塊；結果只留在記憶體（整個程序一份），同一次執行不重複下載。
//! - 隔離模式（效能、冒煙測試）不連網：冒煙測試以 `TEST_PROJECTS` 提供專案。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::app::App;
use crate::icons::IconPixels;
use crate::services::Msg;

const PROJECTS_URL: &str = "https://zoosewu.github.io/apex-code/api/v1/projects.json";
/// 快取多久（API 文件建議使用端自行快取，例如一天）
const CACHE_SECS: u64 = 24 * 60 * 60;
/// 封面解碼的大小（畫在設定頁約 120×75）
const COVER_PX: u32 = 256;

/// 冒煙測試：有值時不連網，以這些專案（沒有封面）代替
#[cfg(feature = "smoke")]
pub static TEST_PROJECTS: Mutex<Option<Vec<Project>>> = Mutex::new(None);

/// 這次執行已讀到的結果（None：還沒讀；Some(空)：讀不到，不再試）
static LOADED: Mutex<Option<Vec<Project>>> = Mutex::new(None);

#[derive(Clone, Default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub tagline: String,
    /// 點了開啟（repo）
    pub url: String,
    pub cover: Option<std::sync::Arc<IconPixels>>,
}

fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("zing-explore").join("projects"))
}

fn fresh(p: &std::path::Path) -> bool {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < CACHE_SECS)
}

/// 讀快取或下載（下載的同時寫入快取）
fn cached_or_download(path: &std::path::Path, url: &str) -> Option<Vec<u8>> {
    if fresh(path)
        && let Ok(b) = std::fs::read(path)
    {
        return Some(b);
    }
    let bytes = crate::update::http::get_bytes(url).ok()?;
    let _ = path.parent().map(std::fs::create_dir_all);
    let _ = std::fs::write(path, &bytes);
    Some(bytes)
}

/// `projects.json` 的內容 → 自己以外的專案（沒有封面）
pub fn parse(json: &str, lang: zing_core::i18n::Lang, own_id: &str) -> Vec<Project> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Vec::new() };
    let code = if lang == zing_core::i18n::Lang::ZhTw { "zh-TW" } else { "en" };
    let text = |v: &serde_json::Value| -> String {
        v.get(code).or_else(|| v.get("en")).and_then(|s| s.as_str()).unwrap_or_default().to_string()
    };
    let Some(list) = v.get("projects").and_then(|p| p.as_array()) else { return Vec::new() };
    list.iter()
        .filter_map(|p| {
            let id = p.get("id")?.as_str()?.to_string();
            let url = p.get("links")?.get("repo")?.as_str()?.to_string();
            (id != own_id && url.starts_with("https://")).then(|| Project {
                name: p.get("name").and_then(|n| n.as_str()).unwrap_or(&id).to_string(),
                tagline: p.get("tagline").map(text).unwrap_or_default(),
                id,
                url,
                cover: None,
            })
        })
        .collect()
}

/// 封面的網址（第一張截圖）
fn cover_url(json: &serde_json::Value, id: &str, code: &str) -> Option<String> {
    let p = json.get("projects")?.as_array()?.iter().find(|p| p.get("id").and_then(|i| i.as_str()) == Some(id))?;
    let src = p.get("screenshots")?.as_array()?.first()?.get("src")?;
    let url = src.get(code).or_else(|| src.get("en"))?.as_str()?;
    url.starts_with("https://").then(|| url.to_string())
}

/// 背景：讀取專案清單與封面（失敗時是空的）
fn load() -> Vec<Project> {
    zing_core::guard::io("讀取作者的其他專案");
    let Some(dir) = cache_dir() else { return Vec::new() };
    let Some(bytes) = cached_or_download(&dir.join("projects.json"), PROJECTS_URL) else { return Vec::new() };
    let json = String::from_utf8_lossy(&bytes);
    let lang = zing_core::i18n::lang();
    let mut projects = parse(&json, lang, "zing-explore");
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else { return projects };
    let code = if lang == zing_core::i18n::Lang::ZhTw { "zh-TW" } else { "en" };
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    for p in &mut projects {
        let Some(url) = cover_url(&value, &p.id, code) else { continue };
        // 檔名只用 id 與網址的副檔名（id 是 GitHub 的 repo 名稱，不會有路徑字元；還是過濾一次）
        let ext = url.rsplit('.').next().filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("png");
        let safe: String = p.id.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
        let file = dir.join(format!("{safe}-{code}.{ext}"));
        if cached_or_download(&file, &url).is_some() {
            p.cover = crate::thumbs::shell_image(&file.to_string_lossy(), COVER_PX).map(std::sync::Arc::new);
        }
    }
    projects
}

impl App {
    /// 進入「關於」分頁時：這次執行讀過就直接用，否則在背景讀取（隔離模式不連網）
    pub fn load_other_projects(&mut self) {
        if let Some(list) = LOADED.lock().unwrap().clone() {
            return self.on_other_projects(list);
        }
        #[cfg(feature = "smoke")]
        if let Some(list) = TEST_PROJECTS.lock().unwrap().clone() {
            return self.on_other_projects(list);
        }
        if self.isolated {
            return;
        }
        let n = self.notifier.clone();
        std::thread::spawn(move || {
            let list = load();
            *LOADED.lock().unwrap() = Some(list.clone());
            n.send(Msg::OtherProjects(list));
        });
    }

    pub fn on_other_projects(&mut self, list: Vec<Project>) {
        if let Some(page) = self.settings_page_mut() {
            page.projects = list;
            self.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zing_core::i18n::Lang;

    const SAMPLE: &str = r#"{
        "apiVersion": "1.3.0", "futureField": [1, 2],
        "projects": [
            { "id": "zing-explore", "name": "zing-explore", "tagline": { "zh-TW": "檔案總管", "en": "File manager" },
              "links": { "repo": "https://github.com/zoosewu/zing-explore" }, "screenshots": [] },
            { "id": "zoo-type", "name": "Zoo Type", "tagline": { "zh-TW": "輸入法", "en": "Input method" }, "status": "new-value",
              "links": { "repo": "https://github.com/zoosewu/zoo-type" },
              "screenshots": [ { "src": { "zh-TW": "https://x.test/zh.png", "en": "https://x.test/en.png" } } ] },
            { "id": "no-repo", "name": "壞的", "links": {} },
            { "id": "insecure", "name": "壞的", "links": { "repo": "http://example.com" } }
        ]
    }"#;

    #[test]
    fn parses_projects_and_skips_unknown_fields() {
        let zh = parse(SAMPLE, Lang::ZhTw, "zing-explore");
        assert_eq!(zh.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["zoo-type"], "排除自己、沒有 repo、不是 https 的");
        assert_eq!((zh[0].name.as_str(), zh[0].tagline.as_str()), ("Zoo Type", "輸入法"));
        assert_eq!(parse(SAMPLE, Lang::Ja, "zing-explore")[0].tagline, "Input method", "日文用英文");
        let v: serde_json::Value = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(cover_url(&v, "zoo-type", "zh-TW").as_deref(), Some("https://x.test/zh.png"));
        assert_eq!(cover_url(&v, "zing-explore", "en"), None, "沒有截圖");
        assert!(parse("not json", Lang::En, "x").is_empty());
        assert!(parse(r#"{"projects": 3}"#, Lang::En, "x").is_empty());
    }
}
