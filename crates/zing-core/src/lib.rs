//! zing-explore 核心：與 UI 無關、可單元測試與基準測試的部分。

// 翻譯的巨集（tr!、tr_key!）要在其他模組之前定義
#[macro_use]
pub mod i18n;

pub mod batchrename;
pub mod cmdbar;
pub mod command;
pub mod config;
pub mod dirsize;
pub mod enumerate;
pub mod folderview;
pub mod format;
pub mod grid;
pub mod guard;
pub mod indexproto;
pub mod keyedit;
pub mod keymap;
pub mod listing;
pub mod namequery;
pub mod ntfs;
pub mod palette;
pub mod path;
pub mod prefs;
pub mod scriptapi;
pub mod sort;
pub mod testdir;
pub mod text;
pub mod textpreview;
pub mod theme;
pub mod volindex;
