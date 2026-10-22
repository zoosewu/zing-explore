# zing-explore

[繁體中文](README.md) | English | [日本語](README.ja.md)

A lightweight, fast and customizable file manager for Windows 11 (executable `zing.exe`). Rust + Win32 + Direct2D, about 1 MB.

## Build and run

Requires Rust (MSVC toolchain) and Visual Studio Build Tools (C++ and the Windows SDK).

```powershell
cargo build --release
.\target\release\zing.exe
```

`cargo build --release` also builds `zing-index.exe` (the NTFS index service; see "Folder sizes" below).

## Features (MVP)

- Single pane + tree, can be split into several panes (`Ctrl+\`); each pane has tabs (`Ctrl+T`)
- Details list: name / date modified / type / size; click a column header to sort, drag column borders to resize; each folder remembers its own sort order
- Type to filter instantly (wildcards such as `*.rs`; the IME composition window is aligned with the filter label), `Esc` clears
- Search all drives (`Ctrl+F`): instantly search every local NTFS drive by name or path, filter by type, size and date modified, and sort (needs the NTFS index service; see "Folder sizes and search all drives" below).
  Separate several words with spaces (all must match), `*` `?` wildcards, `"…"` for words containing spaces; `Enter` goes to the containing folder and selects the item, `Ctrl+Enter` opens it
- Breadcrumb address bar (`Ctrl+L` to type a path)
- Copy / cut / paste (interoperable with File Explorer), delete to the Recycle Bin, rename (`F2`), new folder (`Ctrl+Shift+N`)
- Batch rename (`F2` with several items selected): find and replace (text or regular expressions, `$1` references groups, `\w`, `\d`, `\p{Han}` work with CJK), scope (name without extension / extension / whole name),
  numbering `{n}`, `{n:3}` (zero-padded), `{n:3:10}` (starting at 10, counting only matching items), case conversion; live preview with conflicts highlighted
  (duplicate names, invalid characters, reserved names); it cannot run while there are conflicts. When old and new names overlap (for example 01→02, 02→03) temporary names are used automatically
- Right-click menu (two levels, like Windows 11): common items that appear instantly (Open, Open with, Cut, Copy, Rename, Delete, Copy path,
  open in new tab, favorites, "Batch rename" when several items are selected, Properties…) plus third-party items such as 7-Zip, WinRAR and PowerToys;
  "Show more options" or Shift+right-click opens the full Windows menu. Third-party items that slow the menu down move to "Show more options" automatically
  (the settings page can keep them one by one or all of them, or switch back to the full Windows menu). Shift+F10 / the menu key also open it. Drag and drop in and out
- System file icons (loaded in the background; executables and shortcuts show their own icons)
- Preview pane (`Alt+P`): text files (UTF-8 / UTF-16 / Big5 detected automatically); PDF, Office, video and so on through Windows preview handlers
  (same as File Explorer; a large thumbnail is shown when there is none or it does not respond), picture thumbnails, item counts for folders,
  and below it the same properties as File Explorer's details pane; the width can be dragged
- Thumbnail views: `Ctrl+Shift+1`–`4` switch between list / small / medium / large thumbnails, `Ctrl+wheel` adjusts continuously (48–256 px), remembered per folder.
  Thumbnails come from Windows (same as File Explorer, so any installed thumbnail handler works); a folder containing `folder.jpg/png` or `cover.jpg/png`
  uses it as its thumbnail; `[[thumbnailer]]` in the config file can generate thumbnails with an external command (see "Customization" below).
  Any file or folder can get a picture of your choice with "Set thumbnail…" in the context menu (it survives renames and moves within the same volume; "Remove custom thumbnail" restores the default)
- Favorites: `Ctrl+D` or "Add to Favorites" in the context menu; remove with the tree's context menu; reorder with `Ctrl+↑` / `Ctrl+↓` in the tree or "Move up / Move down" in the context menu
  (the order is saved in `session.toml`; favorites from the config file can be reordered too)
- Folder contents update automatically when they change
- Three built-in styles, each with dark / light variants that can follow the system: Compact (default; dense, square corners, full path), Fluent (roomy, rounded) and
  Balanced (Fluent look, medium density); switch with "Theme: …" in the command palette; `Ctrl+Shift+T` toggles dark/light while keeping the style; custom theme files can be loaded
- Settings page (`Ctrl+,` or the gear at the top right of the toolbar; pick a category, then a setting): UI language, theme, tree and command palette position, hidden files, startup folder, folder sizes and other common settings;
  changes apply immediately and are written back to the matching line of `config.toml` (the file's comments and layout are kept). The shortcut editor, theme editor and config file can also be opened from here
- Everything can be done with the mouse, no commands to remember: the command bar below the toolbar (Windows 11 File Explorer style) has New▾, Cut, Copy, Paste, Rename, Delete,
  Sort▾ and View▾, and everything else is under "⋯" (grouped submenus, including Lua script commands). Unavailable buttons are grayed out; hovering a button shows its name,
  shortcut and why it is unavailable; menus show shortcuts on the right. When the window gets narrow, labels collapse first, then buttons move into "⋯". Keyboard only: press `F10` to move to the command bar, `←` `→` to move, `Enter` to run or open a menu
  (with a menu open, `←` `→` switch to the neighboring menu), `Esc` to leave. The command palette (`Ctrl+Shift+P`) is for people who know the commands
- Customize the command bar ("⋯ → Customize command bar…" or right-click the command bar): drag to reorder, search for and add any built-in or script command, rename, pick a Fluent icon or use your own image file;
  right-click an item under "⋯" to "Add to command bar". Saved in `toolbar.toml` next to `zing.exe`. The settings page can hide the command bar or show icons only
- Keyboard shortcut editor ("⋯ → Edit keyboard shortcuts…"): commands are grouped; click "＋" and press a key combination to add it; if another command uses it you are asked to "Replace" or "Cancel";
  hover a key to remove it, ↺ to restore, "Restore all"; search by name or "Search by key". Saved in `keys.toml` next to `zing.exe` (it takes precedence over `config.toml`, which is never rewritten)
- Advanced: shortcuts and commands can be written in the config file, and commands can use variables (`{sel}`, `{dest}`, `{path}`…; `{ask:prompt}` asks when run)
- UI language: Traditional Chinese, English and Japanese; follows the Windows display language by default, or set `language = "en"` in the config file (`auto` / `zh-TW` / `en` / `ja`)
- Folder sizes: entering a folder shows the size of every subfolder, and you can sort by size (see "Folder sizes" below)
- Tree: visible nodes load their subfolders in the background, so it is known before clicking whether a node can expand (no arrow when there are no subfolders),
  and clicking expands instantly; network and removable drives are not preloaded and always show an arrow. Expanded folders update automatically when subfolders are added, deleted or renamed elsewhere
- File operation queue (`Ctrl+J`): copy / move / delete run one after another with progress, speed (MB/s) and time left, and can be paused or cancelled; small operations such as renames do not wait in the queue
- Command palette (`Ctrl+Shift+P`): fuzzy search over all commands (by title or command name, such as `tog hid`), shows shortcuts, recently used first; input starting with `>` runs a command script directly, `=` evaluates a Lua expression
- Lua scripts: global functions in `scripts\*.lua` are commands (see "Lua scripts" below)

## Customization

Config file: `config.toml` next to `zing.exe` (`Ctrl+,` opens the settings page; "Edit config file" there or in the command palette opens it in Notepad; `Ctrl+Shift+R` reloads). Settings, themes and the session are all kept in the program's folder (portable),
so copying the folder along with the program is enough after reinstalling Windows or moving to another computer; keep it somewhere ordinary users can write to (not Program Files).
See [`crates/zing-core/src/default_config.en.toml`](crates/zing-core/src/default_config.en.toml) for the template.

```toml
language = "auto"       # auto (follows Windows) | zh-TW | en | ja
theme = "system"        # system | dark | light (compact), fluent | balanced (follow the system), fluent-dark…, a custom theme name

[[key]]
key = "F6"
cmd = "copy --move --to={dest}"

[[button]]               # added before "⋯" on the command bar (while there is no toolbar.toml; adjust later with "Customize command bar")
label = "Open in VS Code"
cmd = "run code {sel}"
```

`run` starts console programs (including `.bat` and `.cmd`) without a console window, while window programs show normally; add `--console` to open an interactive terminal,
for example `run --console pwsh`.

`{ask:prompt}` shows an input box before running and substitutes the answer (each prompt is asked only once; `Esc` cancels the whole command), for example
`cmd = "run git commit -m {ask:Commit message}"`.

Custom thumbnail generators: files whose names match `match` get a PNG produced by the command (`{in}` source, `{out}` output, `{size}` edge length);
results are cached in `%LOCALAPPDATA%\zing-explore\thumbs` and regenerated when the file changes; a command may run for at most 10 seconds.

```toml
[[thumbnailer]]
match = "*.stl;*.3mf"
cmd = 'stl-thumb -s {size} "{in}" "{out}"'
```

## Lua scripts

Scripts go in `scripts\*.lua` next to `zing.exe` (Lua 5.4) and are loaded in the background at startup; "Reload Lua scripts" in the command palette reloads them.
Global functions are commands, and the `---` comment above a function is the title shown in the command palette; `on_navigate(path)` and `thumbnail(path)` are events.
Every built-in command can be called directly (`nav.refresh()`, `copy(sel(), { to = "D:\\Backup" })`), and lists can be chained through lazy processing stages with `|`:

```lua
--- Move the selected pictures to the "Pictures" folder
function move_images()
  local imgs = sel() | filter(function(p) return path.ext(p) == "jpg" or path.ext(p) == "png" end) | list
  if #imgs == 0 then return msg("No pictures selected") end
  copy(imgs, { to = path.join(cwd(), "Pictures"), move = true })
end
```

- Each run has a CPU time limit (30 seconds for commands, 0.3 seconds for thumbnail rules; waiting for input or external programs does not count); scripts run in the background and never freeze the window
- `pick_folder()` shows a folder picker; `run{ ..., wait = true }`, `os.execute` and `io.popen` run external programs without a console window
- Errors are shown in the status bar, with the full call stack in `scripts\error.log`; `log()` writes to `scripts\zing.log`
- On load, LuaLS definitions and `.luarc.json` are generated in `scripts\.zing\`: open the `scripts` folder with VS Code's Lua extension for completion and documentation
- Breakpoint debugging: run `pwsh tools/install-vscode-ext.ps1` to install the VS Code extension, open the `scripts` folder in VS Code and press F5 with zing running to attach;
  breakpoints, conditional breakpoints, logpoints, stepping, call stack, variables and evaluation are supported, and it stops where a script error happens

Examples (copy them to `scripts\` next to `zing.exe` and run "Reload Lua scripts" in the command palette; their messages are in Chinese):

- [extract.lua](examples/scripts/extract.lua): batch-extract archives to a folder with the tar.exe built into Windows
- [rename_pattern.lua](examples/scripts/rename_pattern.lua): batch rename with Lua patterns (keeps extensions, numbers name conflicts), with undo
- [checksum.lua](examples/scripts/checksum.lua): compute SHA-256 into a `.sha256` file (also copied to the clipboard for a single file), or verify against a `.sha256` file (certutil, `io.popen`)
- [organize.lua](examples/scripts/organize.lua): move files into subfolders by extension (pictures, videos, documents…) or into "year-month" folders by date modified (`stat()`)
- [folder_view.lua](examples/scripts/folder_view.lua): event functions that switch photo folders to thumbnails and use a cover picture as a folder's thumbnail

See [docs/lua.en.md](docs/lua.en.md) for the full function reference (generated from the code, so it always matches the program).

Themes (command palette):
- "Edit the current theme" (or "Customize theme…" on the settings page): a panel on the right; every change applies to the whole window right away. No color codes needed:
  pick the style and light/dark from a menu (your changes are kept); click a color to open swatches, or use "More colors…" or type a code; numbers have a slider, −/+ and the wheel;
  pick a font from those installed; add "File colors" by category (images, videos, music…); changed fields have a ↺ to reset them;
  hover a name to see where it is used on screen (for example, "Surface" is the background of the toolbar, tree and so on). "Save" on a built-in theme asks for a name and saves it as your own theme (only the fields that differ from the built-in base); "Cancel" reverts
- "Open the current theme file in a text editor": applied automatically when saved
- "Import theme…" / "Export current theme…": `.zingtheme` files; you can also drag a `.zingtheme` file into the window to import it
- The last chosen theme is remembered; when you change `theme` in the config file and reload, the config file wins

Theme files live in `themes\<name>.toml` next to `zing.exe`. Only write the fields you want to change; everything else comes from the built-in theme chosen by `style` (compact | fluent | balanced)
and `mode` (dark | light):

```toml
style = "fluent"
mode = "dark"
[color]
accent = "#4cc2ff"
[metrics]
row_height = 30
[chrome]
address = "path"        # path (full path) | breadcrumb
```

## Folder sizes and search all drives

- **NTFS index service (recommended)**: run "Enable NTFS index service" in the command palette (a UAC prompt appears). After updating zing, if the service is outdated zing updates it automatically at startup (again with a UAC prompt; if you cancel, it asks again next time).
  The service reads the master file table (MFT) of every local NTFS drive to build an index and then follows the NTFS change journal, so the size of any folder is shown instantly
  and updates as files change; search all drives (`Ctrl+F`) is provided by it as well.
  It works like Everything: only the service (`zing-index`, LocalSystem) may read volumes, `zing.exe` keeps normal rights,
  and for every query the service checks, as the caller, that the folder can be listed; search results contain only items in folders you can list.
  - The index uses about 47 MB of memory per million records (all file names and modification times included); searching a million names takes about 10–20 ms
  - Building the index the first time takes seconds to tens of seconds; after a reboot it resumes from a snapshot and the change journal without rescanning
  - Sizes are logical sizes; hard-linked files are counted once (File Explorer's Properties counts them repeatedly, which is why `C:\Windows` differs by several GB); hidden and system files are included
  - The service executable is installed in `%ProgramFiles%\zing-explore`; snapshots and the log are in the service account's
    `C:\Windows\System32\config\systemprofile\AppData\Local\zing-explore`
  - Command line: `zing-index --status`, `--install`, `--uninstall`; as administrator,
    `zing-index --scan C: --verify C:\Users` measures the time to read the MFT and compares with a folder-by-folder scan
- **Without the index** (service not enabled, non-NTFS or network drives): with `[folder_sizes] auto = true` (the default), entering a folder on a local fixed drive
  calculates sizes folder by folder at background priority; `Ctrl+Shift+S` calculates the selected (or all) folders. Results are cached in `%LOCALAPPDATA%\zing-explore\folder-sizes.tsv`.

### Search syntax

Separate several words with spaces; all must match. `"…"` wraps words containing spaces, and a leading `!` excludes. Matching is case-insensitive (including é, Σ, full-width letters and so on).

| Syntax | Meaning |
| --- | --- |
| `report 2024` | the name contains both "report" and "2024" |
| `*.jpg`, `img_????.png` | wildcards match the whole name (`?` is one character) |
| `src\ main`, `c:\users\*\desktop\*` | words containing `\` match the full path |
| `!node_modules\`, `!*.tmp` | exclude |
| `ext:jpg;png` | extension (files only) |
| `size:>10mb`, `size:<=1k`, `size:1mb..1gb` | size (for folders, the size of the whole folder) |
| `dm:today`, `dm:yesterday`, `dm:7d`, `dm:2024-05`, `dm:>2024-05-01`, `dm:2024-01..2024-03` | date modified |
| `file:`, `folder:` | files / folders only |
| `sort:size`, `sort:date`, `sort:name`, `sort:path`, `sort:ext` (optionally `-asc` / `-desc`) | sort; size and date are largest first by default; name, path and extension use natural order (`file2` before `file10`) |

In the search panel, `Ctrl+1`–`Ctrl+5` sort by name, path, extension, size or date (rewriting `sort:` in the query; press again to reverse) and `Ctrl+0` removes sorting;
the current sort order and the number of results are shown at the right of the input box.

Without a sort order, results are listed per drive in creation order as they are found (fastest); sorting first has to match on all drives, for up to about 3 seconds, after which only what was found is sorted (shown as "500+").
A folder's date modified changes only when its own record changes (adding or removing items inside it does not necessarily update it).

## Performance

Every change must pass `pwsh tools/perf.ps1` and `pwsh tools/smoke.ps1` (thresholds and rules are in [CLAUDE.md](CLAUDE.md), in Chinese).

## Translations

UI text is written in Traditional Chinese, which is the key; translations are in `crates/zing-core/i18n/en.toml` and `ja.toml` (`"source" = "translation"`).
`cargo test -p zing-core i18n` checks that every source string is translated and that parameter names match; with `ZING_I18N_MISSING=1` it lists the missing entries.
The Lua API documents (`docs/lua*.md`) are generated from `scriptapi.rs` and `builtin.rs` (built-in commands) with the same translations.
