# Lua scripts

> This document is generated from `crates/zing-core/src/scriptapi.rs`; do not edit it directly.
> After changing the API, run `ZING_UPDATE_DOCS=1 cargo test -p zing-core` to update it.

Scripts live in `scripts\*.lua` next to `zing.exe` and are loaded in the background at startup (in file name order); "Reload Lua scripts" (`script.reload`) reloads them.
Scripts run on a dedicated thread and never freeze the UI; a single run that uses more than 30 seconds of CPU time is stopped (waiting for `ask()`, external programs and file I/O does not count, so only runaway loops are caught). Errors are shown in the status bar and logged to `scripts\error.log` (with file name, line number and call stack).
Scripts can use the complete Lua 5.4 standard library (including `io` and `os`). `os.execute` and `io.popen` run in the current folder without showing a console window
(in read mode `io.popen` can read the output while the command runs; in write mode it writes to the command's standard input; `close()` waits for the command to finish and returns its exit status).

## Custom commands

Global functions are commands: they appear in the command palette (the title comes from the `---` comment above the function) and can be bound to keys or buttons in the config file (`cmd = "function_name"`).
`local` functions are helpers only and do not become commands. Function names must be ASCII; titles can be in any language.

```lua
--- Move the selected photos to "Photos\year-month"
function sort_photos()
  local dest = ask("Destination folder", path.join(cwd(), "Photos"))
  if not dest then return end
  local photos = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
  copy(photos, { to = dest, move = true })
end
```

## Completion and debugging

- When scripts are loaded, type definitions for the whole API are written to `scripts\.zing\zing.d.lua`, and `scripts\.luarc.json` is set up.
  Open the `scripts` folder in VS Code and install the Lua extension (Lua Language Server) to get completion, documentation and type checking.
- In the command palette, a Lua expression starting with `=` runs immediately and shows the result in the status bar, for example `= #sel()` or `= sel() | map(path.ext) | list`.
- `log(...)` writes to `scripts\zing.log`; while a debugger is attached it also appears in VS Code's Debug Console.

### Breakpoint debugging

zing has a built-in Lua debugger (Debug Adapter Protocol); connect with VS Code:

1. Install the extension: in the zing source folder run `pwsh tools/install-vscode-ext.ps1` (it also installs the Lua Language Server), then reload VS Code.
2. Open the `scripts` folder in VS Code (zing already wrote `.vscode\launch.json`) and, with zing running, press F5 to attach.
3. Set breakpoints in a script, then run the command in zing: when stopped you can inspect the call stack, locals, upvalues and globals,
   evaluate in the current frame in the Debug Console or watches, step (over, into, out), continue or pause.

- Conditional breakpoints and logpoints (`{expression}` is replaced by its value) are supported; "Pause on script errors" is on by default and stops right where the error happens, with the stack intact.
- While paused, zing's UI stays usable; other script commands are queued until you continue. Time spent paused does not count toward the CPU time limit.
- Breakpoints are kept when scripts are reloaded.
- Only the current Windows user can connect (named pipe `\\.\pipe\zing-lua-debug-username`, remote connections refused); only one zing at a time accepts debugger connections.

## Global functions

Interact with the current state and the user. `sel`, `cwd` and `focus` return a snapshot taken when the command started.

### `sel()`

The selected items. A snapshot taken when the command started; running commands do not change it

- Returns: string[] full paths of the selected items (immutable list)

```lua
for _, p in ipairs(sel()) do msg(p) end
```

### `cwd()`

The current folder (snapshot)

- Returns: string current folder (empty string for "This PC")

```lua
msg(cwd())
```

### `focus()`

The focused item (snapshot)

- Returns: string? full path of the focused item

```lua
local f = focus()
```

### `go(path)`

Go to a folder

- `path`: string folder

```lua
go("D:\\Photos")
```

### `msg(text)`

Show a message in the status bar

- `text`: string

```lua
msg("Done")
```

### `ask(prompt, default?)`

Ask the user for input (waits for the answer)

- `prompt`: string prompt
- `default?`: string default value
- Returns: string? the text entered; nil when cancelled

```lua
local name = ask("New folder name", "New folder")
```

### `pick_folder(title?, default?)`

Show a folder picker dialog (waits for the user to choose)

- `title?`: string dialog title
- `default?`: string folder shown initially
- Returns: string? the selected folder; nil when cancelled

```lua
local dest = pick_folder("Extract to", cwd())
```

### `clipboard(text?)`

Read or write the clipboard text (line breaks are always \n; converted to Windows \r\n when writing)

- `text?`: string text to put on the clipboard; reads when omitted
- Returns: string|boolean|nil when reading, the clipboard text (nil if there is no text); when writing, whether it succeeded

```lua
clipboard(table.concat(sel() | map(path.name) | list, "\n"))
local text = clipboard()
```

### `exec(script)`

Run a command script

- `script`: string command script (same as cmd in the config file; variables such as {sel} and {ask:prompt} are allowed)

```lua
exec("copy {sel} --to={dest}")
```

### `log(...)`

Write to scripts\zing.log (for debugging)

- `...`: any

```lua
log("selected", #sel())
```

## Functional library

Lazy evaluation, immutability, pure functions, composition. Pipelines are chained with `|`: `source | stage | stage | sink`; the source can be a list (such as `sel()`) or `seq(...)`. Stages (`map`, `filter`…) only describe what to do; nothing is computed until a sink (`list`, `each`, `reduce`…). Except for `each` and `is_dir`, these are pure functions; returned lists cannot be modified. A pipeline is an expression, and Lua does not allow it alone on a line (`sel() | each(f)` is a syntax error): assign the result to a variable, or for side effects only use a method call `seq(sel()):each(f)`.

### `seq(source)`

Create a lazy stream: chained stages are computed item by item only when values are taken, without intermediate lists

- `source`: table|function list or iterator function
- Returns: Seq

```lua
local jpgs = seq(sel()):filter(function(p) return path.ext(p) == "jpg" end):take(10):list()
```

### `map(f)`

Pipeline stage: transform each item

- `f`: fun(x: any): any
- Returns: Stage

```lua
local names = sel() | map(path.name) | list
```

### `filter(pred)`

Pipeline stage: keep only matching items

- `pred`: fun(x: any): boolean
- Returns: Stage

```lua
local jpgs = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
```

### `take(n)`

Pipeline stage: take the first n items

- `n`: integer
- Returns: Stage

```lua
local first3 = sel() | take(3) | list
```

### `skip(n)`

Pipeline stage: skip the first n items

- `n`: integer
- Returns: Stage

```lua
local rest = sel() | skip(1) | list
```

### `take_while(pred)`

Pipeline stage: take until the first item that does not match

- `pred`: fun(x: any): boolean
- Returns: Stage

```lua
local small = seq({1, 2, 5, 1}) | take_while(function(x) return x < 3 end) | list
```

### `sort_by(key)`

Pipeline stage: sort by key (reads the whole stream)

- `key`: fun(x: any): any sort key
- Returns: Stage

```lua
local sorted = sel() | sort_by(path.name) | list
```

### `each(f)`

Pipeline sink: run for each item (actions with side effects go here)

- `f`: fun(x: any)
- Returns: Stage

```lua
seq(sel()):each(msg) -- use method calls when only side effects matter: Lua does not allow a | b alone on a line
```

### `reduce(f, init)`

Pipeline sink: fold into one value

- `f`: fun(acc: any, x: any): any
- `init`: any
- Returns: Stage

```lua
local sum = seq({1, 2, 3}) | reduce(function(a, x) return a + x end, 0)
```

### `list(s)`

Pipeline sink: collect all items into an immutable list

- `s`: Seq|table
- Returns: table immutable list

```lua
local names = sel() | map(path.name) | list
```

### `count(s)`

Pipeline sink: number of items

- `s`: Seq|table
- Returns: integer

```lua
local dirs = sel() | filter(is_dir) | count
```

### `first(s)`

Pipeline sink: the first item (nil if none)

- `s`: Seq|table
- Returns: any

```lua
local f = sel() | first
```

### `compose(...)`

Compose functions (right to left): compose(f, g)(x) == f(g(x))

- `...`: function
- Returns: function

```lua
local stem = compose(string.lower, path.stem)
```

### `pipe(x, ...)`

Pass a value through functions in order (left to right): pipe(x, f, g) == g(f(x))

- `x`: any
- `...`: function
- Returns: any

```lua
pipe(focus(), path.name, string.upper)
```

### `partial(f, ...)`

Fix the first few arguments

- `f`: function
- `...`: any arguments to apply first
- Returns: function

```lua
local to_backup = partial(path.join, "D:\\Backup")
```

### `freeze(t)`

Return a read-only immutable version (modifying it raises an error)

- `t`: table
- Returns: table

```lua
local cfg = freeze({ dest = "D:\\" })
```

### `is_dir(p)`

whether it is a folder (reads the file system)

- `p`: string
- Returns: boolean

```lua
local dirs = sel() | filter(is_dir) | list
```

### `stat(p)`

File information: size, modified/created/accessed times, attributes (reads the file system)

- `p`: string file or folder
- Returns: {size: integer, modified: integer?, created: integer?, accessed: integer?, is_dir: boolean, readonly: boolean, hidden: boolean, system: boolean}?, string? times are Unix seconds (usable with os.date); size is 0 for folders; nil and an error message on failure

```lua
local month = os.date("%Y-%m", stat(sel()[1]).modified)
local big = sel() | filter(function(p) return stat(p).size > 100 * 1024 * 1024 end) | list
```

### `path.name(p)`

file name (with extension)

- `p`: string
- Returns: string

```lua
path.name("C:\\a\\b.txt") --> "b.txt"
```

### `path.stem(p)`

file name without extension

- `p`: string
- Returns: string

```lua
path.stem("b.tar.gz") --> "b.tar"
```

### `path.ext(p)`

extension (lowercase, without the dot; empty string if none)

- `p`: string
- Returns: string

```lua
path.ext("A.JPG") --> "jpg"
```

### `path.dir(p)`

parent folder

- `p`: string
- Returns: string

```lua
path.dir("C:\\a\\b.txt") --> "C:\\a"
```

### `path.join(...)`

join paths with \

- `...`: string
- Returns: string

```lua
path.join(cwd(), "New folder")
```

## Events

Defining global functions with these names in a script makes them take effect.

### `on_navigate(path)`

Called when entering a folder (the result is not awaited)

- `path`: string folder entered

```lua
function on_navigate(p)
  if p:find("\\Photos") then view.mode("large") end
end
```

### `thumbnail(path)`

Choose the thumbnail source (called once per thumbnail, CPU time limit 0.3 s)

- `path`: string file or folder
- Returns: string? path of the picture to use as the thumbnail; nil uses the default

```lua
function thumbnail(p)
  if path.ext(p) == "mp4" then
    local jpg = path.join(path.dir(p), path.stem(p) .. ".jpg")
    if io.open(jpg) then return jpg end
  end
end
```

## Built-in commands

The same commands as in the config file and the command palette, called as functions. String and number arguments are positional; integer keys of a table are positional (lists are expanded), string keys become `--key=value` (`true` gives `--key`, `false` is omitted). Arguments are not variable-expanded, so `{` `}` in paths are not misread. The script continues as soon as the command is sent to the UI; it does not wait for the command to finish.

### `nav.back()`

Back

```lua
nav.back()
```

### `nav.forward()`

Forward

```lua
nav.forward()
```

### `nav.up()`

Up one level

```lua
nav.up()
```

### `nav.open()`

Open the focused item (enters folders, opens files with the default program)

```lua
nav.open()
```

### `nav.refresh()`

Refresh

```lua
nav.refresh()
```

### `nav.home()`

Go back to the user folder

```lua
nav.home()
```

### `nav.goto(path)`

Go to a folder (same as `go`)

- `path`: string path

```lua
nav["goto"]("C:\\Windows") -- goto is a Lua keyword, so write it like this; or use go()
```

### `address.edit()`

Edit address bar

```lua
address.edit()
```

### `palette.open()`

Open the command palette

```lua
palette.open()
```

### `search.open()`

Search all drives: search every NTFS drive by name or path, with filters and sorting (syntax in the README; needs the NTFS index service)

```lua
search.open()
```

### `jobs.toggle()`

Show/hide the file operations panel

```lua
jobs.toggle()
```

### `jobs.clear()`

Clear finished file operations

```lua
jobs.clear()
```

### `file.copy()`

Copy the selected items to the clipboard

```lua
file.copy()
```

### `file.cut()`

Cut the selected items to the clipboard

```lua
file.cut()
```

### `file.paste()`

Paste files from the clipboard into the current folder

```lua
file.paste()
```

### `file.delete(opts?)`

Delete the selected items

- `opts?`: {permanent?: boolean} when permanent is true, skips the Recycle Bin

```lua
file.delete{ permanent = true }
```

### `file.rename()`

Rename (batch rename when multiple items are selected)

```lua
file.rename()
```

### `file.batchrename()`

Batch rename the selected items

```lua
file.batchrename()
```

### `file.newfolder()`

Create a new folder in the current folder

```lua
file.newfolder()
```

### `file.properties()`

Show the properties dialog of the selected items

```lua
file.properties()
```

### `menu.context()`

Show the context menu of the focused item

```lua
menu.context()
```

### `file.copypath()`

Copy the full paths of the selected items

```lua
file.copypath()
```

### `copy(args)`

Copy or move to a folder (through the file operation queue)

- `args`: {to: string, move?: boolean, [integer]: string} to is the destination folder; positional arguments are the sources (the selection when omitted)
- Also: `fun(sources: string[]|string, opts: {to: string, move?: boolean})`

```lua
copy{ to = "D:\\Backup", move = true }
```

### `run(program, ...)`

Run a program (in the current folder). Console programs (including .bat, .cmd) run without a console window, window programs show normally; in the config file use `run --console program` to show a console

- `program`: string program
- `...`: string|{wait?: boolean, console?: boolean} arguments; with wait = true in a table, waits for the program to exit and returns its exit code (scripts only; waiting does not count toward the time limit); console = true shows a console window (interactive terminal)
- Returns: integer?, string? exit code (with wait); nil and an error message when it cannot start
- Also: `fun(args: {[integer]: string, wait?: boolean, console?: boolean}): integer?, string?`

```lua
run("code", cwd())
local code = run{ "tar", "-xf", "a.zip", "-C", "D:\\out", wait = true }
run{ "pwsh", console = true }
```

### `open(...)`

Open files with the default program

- `...`: string files

```lua
open("C:\\a.pdf")
```

### `sel.all()`

Select all

```lua
sel.all()
```

### `sel.none()`

Select none

```lua
sel.none()
```

### `sel.invert()`

Invert selection

```lua
sel.invert()
```

### `tab.new(opts?)`

Open a new tab (the current folder by default)

- `opts?`: {path?: string}

```lua
tab.new{ path = "C:\\" }
```

### `tab.close()`

Close tab

```lua
tab.close()
```

### `tab.next()`

Next tab

```lua
tab.next()
```

### `tab.prev()`

Previous tab

```lua
tab.prev()
```

### `pane.split()`

Split off a new pane

```lua
pane.split()
```

### `pane.close()`

Close pane

```lua
pane.close()
```

### `pane.focusnext()`

Switch to next pane

```lua
pane.focusnext()
```

### `tree.toggle()`

Show/hide the tree

```lua
tree.toggle()
```

### `view.sort(opts)`

Sort

- `opts`: {by?: "name"|"modified"|"type"|"size", asc?: boolean, desc?: boolean} with only asc/desc, the current column is kept

```lua
view.sort{ by = "size", desc = true }
```

### `view.foldersizes()`

Calculate folder sizes

```lua
view.foldersizes()
```

### `thumb.set(opts?)`

Set a custom thumbnail (the picture is copied to %LOCALAPPDATA% and keyed by NTFS file ID, so it survives renames and moves)

- `opts?`: {path?: string, image?: string} path defaults to the current item; without image, opens a dialog to choose a picture

```lua
thumb.set{ image = "D:\\cover.jpg" }
```

### `thumb.clear(opts?)`

Remove the custom thumbnail

- `opts?`: {path?: string}

```lua
thumb.clear()
```

### `cmdbar.edit()`

Open the command bar customization panel (saved in toolbar.toml next to the program)

```lua
cmdbar.edit()
```

### `cmdbar.hide()`

Hide the command bar (written back to command_bar.visible in config.toml)

```lua
cmdbar.hide()
```

### `file.newfile()`

Create a text document in the current folder and start renaming it

```lua
file.newfile()
```

### `settings.open()`

Open the settings page (changes are written back to the matching line of config.toml, keeping comments)

```lua
settings.open()
```

### `keys.edit()`

Open the keyboard shortcut editor (saved in keys.toml next to the program)

```lua
keys.edit()
```

### `view.preview()`

Show/hide the preview pane

```lua
view.preview()
```

### `view.mode(mode)`

Switch between list and thumbnail views

- `mode`: "list"|"small"|"medium"|"large"|"thumbnails"

```lua
view.mode("medium")
```

### `view.thumbsize(opts)`

Change the thumbnail size

- `opts`: {delta: integer} pixels to add or subtract

```lua
view.thumbsize{ delta = 16 }
```

### `view.togglehidden()`

Show/hide hidden files

```lua
view.togglehidden()
```

### `index.install()`

Enable the NTFS index service (needs administrator rights)

```lua
index.install()
```

### `index.uninstall()`

Remove NTFS index service

```lua
index.uninstall()
```

### `index.status()`

Show the NTFS index service status

```lua
index.status()
```

### `filter.clear()`

Clear filter

```lua
filter.clear()
```

### `filter.set(opts)`

Set the filter

- `opts`: {text: string} filter text (wildcards * ? allowed)

```lua
filter.set{ text = "*.jpg" }
```

### `favorites.add(opts?)`

Add to Favorites (the current folder by default)

- `opts?`: {path?: string}

```lua
favorites.add()
```

### `favorites.remove(opts?)`

Remove from Favorites

- `opts?`: {path?: string}

```lua
favorites.remove{ path = cwd() }
```

### `favorites.move(opts)`

Change the order of Favorites (saved in session.toml; favorites from the config file can be reordered too)

- `opts`: {by: integer, path?: string} by is how many positions to move (negative moves up); path defaults to the favorite focused in the tree, otherwise the current folder

```lua
favorites.move{ by = -1 }
```

### `theme.set(name)`

Switch theme

- `name`: string theme name

```lua
theme.set("fluent")
```

### `theme.toggle()`

Toggle dark/light (keeps the style)

```lua
theme.toggle()
```

### `theme.edit()`

Open the theme editor

```lua
theme.edit()
```

### `theme.editfile()`

Open the current theme file in a text editor

```lua
theme.editfile()
```

### `theme.import()`

Import a theme

```lua
theme.import()
```

### `theme.export()`

Export the current theme

```lua
theme.export()
```

### `config.edit()`

Open the config file in a text editor

```lua
config.edit()
```

### `config.reload()`

Reload settings

```lua
config.reload()
```

### `script.reload()`

Reload Lua scripts

```lua
script.reload()
```

### `app.exit()`

Exit the program

```lua
app.exit()
```

### `update.check()`

Check for a new version now (regardless of the once-a-day limit and the setting; any newer version is shown, including patch releases and skipped versions)

```lua
update.check()
```

### `welcome.show()`

Open the welcome screen (the common settings shown on first launch)

```lua
welcome.show()
```

### `cursor.up(opts?)`

List: move the focus up and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.up{ extend = true }
```

### `cursor.down(opts?)`

List: move the focus down and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.down{ extend = true }
```

### `cursor.left(opts?)`

List: move the focus left and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.left{ extend = true }
```

### `cursor.right(opts?)`

List: move the focus right and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.right{ extend = true }
```

### `cursor.pageup(opts?)`

List: move the focus one page up and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.pageup{ extend = true }
```

### `cursor.pagedown(opts?)`

List: move the focus one page down and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.pagedown{ extend = true }
```

### `cursor.first(opts?)`

List: move the focus to the first item and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.first{ extend = true }
```

### `cursor.last(opts?)`

List: move the focus to the last item and select it (in thumbnail views left/right is one cell, up/down one row)

- `opts?`: {extend?: boolean, keep?: boolean} extend extends the selection (Shift); keep moves only the focus without changing the selection (Ctrl); both extend while keeping the existing selection

```lua
cursor.last{ extend = true }
```

### `cursor.select()`

List: select only the focused item

```lua
cursor.select()
```

### `cursor.toggle()`

List: toggle the selection of the focused item (others unchanged)

```lua
cursor.toggle()
```

### `filter.backspace()`

Delete the last character of the filter (the filter ends when it is empty)

```lua
filter.backspace()
```

### `filter.start()`

Start filtering: characters typed next become the filter (for when typing in the list is set to do nothing)

```lua
filter.start()
```

### `tree.up()`

Tree: move the focus up

```lua
tree.up()
```

### `tree.down()`

Tree: move the focus down

```lua
tree.down()
```

### `tree.expand()`

Tree: expand the focused node (move to its first child if already expanded)

```lua
tree.expand()
```

### `tree.collapse()`

Tree: collapse the focused node (move to its parent if already collapsed)

```lua
tree.collapse()
```

### `tree.open()`

Tree: go to the focused node's folder

```lua
tree.open()
```

### `tree.leave()`

Tree: move the keyboard focus back to the file list

```lua
tree.leave()
```

### `tree.moveup()`

Tree: move the focused favorite up (does nothing on other nodes)

```lua
tree.moveup()
```

### `tree.movedown()`

Tree: move the focused favorite down (does nothing on other nodes)

```lua
tree.movedown()
```

### `ui.up()`

Popups (menus, command palette, command bar, side panels): up

```lua
ui.up()
```

### `ui.down()`

Popups (menus, command palette, command bar, side panels): down

```lua
ui.down()
```

### `ui.left()`

Popups (menus, command palette, command bar, side panels): left (command bar, submenus)

```lua
ui.left()
```

### `ui.right()`

Popups (menus, command palette, command bar, side panels): right (command bar, submenus)

```lua
ui.right()
```

### `ui.pageup()`

Popups (menus, command palette, command bar, side panels): one page up

```lua
ui.pageup()
```

### `ui.pagedown()`

Popups (menus, command palette, command bar, side panels): one page down

```lua
ui.pagedown()
```

### `ui.first()`

Popups (menus, command palette, command bar, side panels): to the first item

```lua
ui.first()
```

### `ui.last()`

Popups (menus, command palette, command bar, side panels): to the last item

```lua
ui.last()
```

### `ui.accept()`

Popups (menus, command palette, command bar, side panels): accept (run the selected item)

```lua
ui.accept()
```

### `ui.cancel()`

Popups (menus, command palette, command bar, side panels): cancel (close)

```lua
ui.cancel()
```

### `ui.select()`

Popups (menus, command palette, command bar, side panels): toggle the focused item (switches, options)

```lua
ui.select()
```

### `ui.next()`

Popups (menus, command palette, command bar, side panels): next field

```lua
ui.next()
```

### `ui.prev()`

Popups (menus, command palette, command bar, side panels): previous field

```lua
ui.prev()
```

### `search.sort(opts)`

Whole-drive search: sort by a column (again to reverse); none removes the sort

- `opts`: {by: "name"|"path"|"ext"|"size"|"date"|"none"}

```lua
search.sort{ by = "size" }
```

### `search.openitem()`

Whole-drive search: open the selected result (enter folders; Enter goes to its location instead)

```lua
search.openitem()
```

### `cmdbar.focus()`

Move the keyboard focus to the command bar (←→ move, Enter runs, Esc leaves); run again to leave

```lua
cmdbar.focus()
```
