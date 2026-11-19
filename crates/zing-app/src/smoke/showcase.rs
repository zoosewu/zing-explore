//! 作品集的截圖（tools/screenshots.ps1，`--smoke-showcase=zh-TW|en`）：用示範資料走一遍主要畫面並截圖。
//!
//! 示範資料由 tools/screenshots.ps1 建立在以 `subst` 掛上的磁碟機根目錄（畫面上不出現真實的路徑），
//! 資料夾名稱依語言（`Names`，必須與 screenshots.ps1 一致）。視窗的用戶區調成 1280×800。
//! 與冒煙測試共用驅動（`Driver`）：動作之後等畫面就緒才截圖，逾時就失敗。

use std::time::Duration;

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos};
use zing_core::folderview::ViewMode;
use zing_core::i18n::Lang;
use zing_core::path;
use zing_core::theme::{Mode, Style};

use super::{Ctx, Step, at, click, exec, expect, key, letter, row_pos, screenshot, selected, step};
use crate::window::with_app;

/// 截圖的用戶區大小（實際像素）
const W: i32 = 1280;
const H: i32 = 800;

/// 示範資料的名稱（與 tools/screenshots.ps1 一致）
struct Names {
    docs: &'static str,
    photos: &'static str,
    backup: &'static str,
    /// 01 選取的檔案、05 預覽的圖片
    report: &'static str,
    photo: &'static str,
}

fn names(lang: Lang) -> Names {
    match lang {
        Lang::En => Names {
            docs: "Documents",
            photos: "Photos",
            backup: "Backup",
            report: "Annual report 2026.docx",
            photo: "Lakeside 03.png",
        },
        _ => Names { docs: "文件", photos: "相片", backup: "備份", report: "2026 年度報告.docx", photo: "湖畔 03.png" },
    }
}

/// 畫面安定下來（圖示、樹狀列、背景結果）再截圖
fn shot(c: &mut Ctx, name: &str) -> Result<(), String> {
    if c.started.elapsed() < Duration::from_millis(1500) {
        return Err("等畫面安定".into());
    }
    screenshot(c, name)
}

fn client_size(c: &Ctx) -> (i32, i32) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(c.hwnd, &mut rc);
    }
    (rc.right, rc.bottom)
}

pub(super) fn steps(ctx: &Ctx, lang: Lang) -> Vec<Step> {
    let n = names(lang);
    let root = ctx.dir.clone();
    let docs = path::join(&root, n.docs);
    let photos = path::join(&root, n.photos);
    let backup_docs = path::join(&path::join(&root, n.backup), n.docs);
    let (report, photo) = (n.report, n.photo);
    let mut v = Vec::new();

    v.push(step(
        "用戶區 1280×800、介面語言、開啟文件",
        {
            let docs = docs.clone();
            move |c| {
                unsafe {
                    let (mut wr, mut cr) = (RECT::default(), RECT::default());
                    let _ = GetWindowRect(c.hwnd, &mut wr);
                    let _ = GetClientRect(c.hwnd, &mut cr);
                    let (fw, fh) = (wr.right - wr.left - cr.right, wr.bottom - wr.top - cr.bottom);
                    let _ = SetWindowPos(c.hwnd, None, 0, 0, W + fw, H + fh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                with_app(|a| {
                    a.set_language(lang);
                    a.navigate(0, docs, true);
                });
            }
        },
        {
            let docs = docs.clone();
            move |c, a| {
                expect("用戶區", client_size(c), (W, H))?;
                at(a, &docs)
            }
        },
    ));
    v.push(step("計算資料夾大小", |c| key(c, "cs", letter('s')), |_, a| expect("計算中", a.sizes.pending, 0)).slow());
    v.push(step("01：選取一個檔案", move |c| click(c, "", row_pos(report)), move |c, a| {
        expect("選取", selected(a), vec![report.to_string()])?;
        shot(c, "01-main-window")
    }));

    v.push(
        step(
            "02：相片的大縮圖",
            {
                let photos = photos.clone();
                move |_| {
                    with_app(|a| a.navigate(0, photos, true));
                    exec("view.mode large");
                }
            },
            {
                let photos = photos.clone();
                move |c, a| {
                    at(a, &photos)?;
                    expect("檢視", (a.tab().mode, a.tab().thumb), (ViewMode::Thumbnails, 192))?;
                    // 只取看得到的格子
                    let pl = a.layout.panes[0].list;
                    let g = a.grid(0, pl).ok_or("沒有格狀版面")?;
                    let n = g.visible(a.tab().scroll, pl.h(), a.tab().view.len()).len();
                    let ready = a.thumbs.ready_count();
                    if ready < n {
                        return Err(format!("{ready}/{n} 個縮圖就緒"));
                    }
                    shot(c, "02-thumbnails")
                }
            },
        )
        .slow(),
    );

    v.push(
        step(
            "比對：兩個窗格開著文件與備份",
            {
                let (docs, backup_docs) = (docs.clone(), backup_docs.clone());
                move |_| {
                    with_app(|a| {
                        a.exec("pane.split");
                        a.navigate(0, docs, true);
                        a.navigate(1, backup_docs, true);
                    });
                }
            },
            |_, a| {
                expect("窗格數", a.panes.len(), 2)?;
                expect("載入中", (a.panes[0].tab().is_loading(), a.panes[1].tab().is_loading()), (false, false))?;
                expect("可以比對", a.cmd_disabled("folder.compare"), None)
            },
        )
        .slow(),
    );
    v.push(
        step("03：資料夾比對", |_| exec("folder.compare"), |c, a| {
            let cp = a.compare_panel().ok_or("比對的對話框沒有開")?;
            if cp.state != crate::compare::State::Done {
                return Err(format!("{:?}", cp.state));
            }
            shot(c, "03-folder-compare")
        })
        .slow(),
    );

    v.push(step("04：設定", |_| exec("settings.open"), |c, a| {
        expect("設定", a.settings_page().is_some(), true)?;
        shot(c, "04-settings")
    }));

    v.push(
        step(
            "Fluent 淺色主題、單一窗格、相片清單與預覽窗格",
            {
                let photos = photos.clone();
                move |c| {
                    key(c, "", VK_ESCAPE);
                    with_app(|a| {
                        a.active_pane = 1;
                        a.exec("pane.close");
                        a.exec("theme.set fluent-light");
                        a.navigate(0, photos, true);
                    });
                    exec("view.mode list");
                    exec("view.preview");
                }
            },
            move |_, a| {
                expect("設定", a.settings_page().is_none(), true)?;
                expect("窗格數", a.panes.len(), 1)?;
                expect("主題", (a.theme.style, a.theme.mode), (Style::Fluent, Mode::Light))?;
                at(a, &photos)?;
                expect("檢視", a.tab().mode, ViewMode::List)?;
                expect("預覽窗格", a.preview.visible, true)
            },
        )
        .slow(),
    );
    v.push(
        step("05：預覽一張相片", move |c| click(c, "", row_pos(photo)), move |c, a| {
            expect("選取", selected(a), vec![photo.to_string()])?;
            let ld = a.preview.loaded.as_ref().ok_or("還沒有預覽")?;
            if !ld.path.ends_with(photo) || !matches!(ld.body, crate::preview::Body::Image(Some(_))) {
                return Err("預覽還沒好".into());
            }
            shot(c, "05-light-theme-preview")
        })
        .slow(),
    );
    v
}
