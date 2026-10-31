# Lua スクリプト

> この文書は `crates/zing-core/src/scriptapi.rs` から生成されます。直接編集しないでください。
> API を変更したら `ZING_UPDATE_DOCS=1 cargo test -p zing-core` を実行して更新します。

スクリプトは `zing.exe` の隣の `scripts\*.lua` に置き、起動後にバックグラウンドで読み込まれます（ファイル名の順）。「Lua スクリプトを再読み込み」（`script.reload`）で読み込み直せます。
スクリプトは専用のスレッドで実行され、画面が固まることはありません。1 回の実行で CPU 時間を 30 秒以上使うと中止されます（`ask()`、外部プログラム、ファイル I/O の待ち時間は含まれないので、暴走したループだけが止まります）。エラーはステータスバーに表示され、`scripts\error.log` に記録されます（ファイル名、行番号、呼び出し履歴つき）。
スクリプトでは Lua 5.4 の標準ライブラリをすべて使えます（`io`、`os` を含む）。`os.execute`、`io.popen` は現在のフォルダーで、コンソールウィンドウを表示せずに実行されます
（`io.popen` の読み取りモードでは実行中に出力を読め、書き込みモードではコマンドの標準入力に書き込みます。`close()` はコマンドの終了を待って終了状態を返します）。

## カスタムコマンド

グローバル関数がそのままコマンドになります：コマンドパレットに表示され（タイトルは関数の上の `---` コメント）、設定ファイルでショートカットキーやボタンに割り当てられます（`cmd = "関数名"`）。
`local` 関数は補助用で、コマンドにはなりません。関数名には英字だけを使えます。タイトルは日本語でもかまいません。
`---@label` はコマンドバーに追加したときのボタンの名前です（ないときはタイトル）。スクリプトを変えるとボタンの名前も変わります。
スクリプトは保存すると自動的に再読み込みされます。「⋯」→「スクリプト」→「Lua スクリプトを再読み込み」でも再読み込みできます。

```lua
--- 選択した写真を「写真\年-月」に移動
---@label 写真を整理
function sort_photos()
  local dest = ask("移動先のフォルダー", path.join(cwd(), "写真"))
  if not dest then return end
  local photos = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
  copy(photos, { to = dest, move = true })
end
```

## 補完とデバッグ

- スクリプトを読み込むと `scripts\.zing\zing.d.lua` にすべての API の型定義が書き込まれ、`scripts\.luarc.json` も設定されます。
  VS Code で `scripts` フォルダーを開き、Lua 拡張機能（Lua Language Server）を入れると、補完、説明、型チェックが使えます。
- コマンドパレットで `=` から始まる Lua の式を入力するとすぐに実行され、結果がステータスバーに表示されます。例：`= #sel()`、`= sel() | map(path.ext) | list`。
- `log(...)` は `scripts\zing.log` に書き込みます。デバッガーの接続中は VS Code の「デバッグ コンソール」にも表示されます。

### ブレークポイントによるデバッグ

zing には Lua デバッガー（Debug Adapter Protocol）が組み込まれており、VS Code から接続します：

1. 拡張機能をインストール：zing のソースフォルダーで `pwsh tools/install-vscode-ext.ps1` を実行し（Lua Language Server も一緒に入ります）、VS Code を再読み込みします。
2. VS Code で `scripts` フォルダーを開き（zing が `.vscode\launch.json` を用意しています）、zing を開いたまま F5 でアタッチします。
3. スクリプトにブレークポイントを設定し、zing でコマンドを実行します：停止中は呼び出し履歴、ローカル変数、上位値、グローバル変数を確認でき、
   「デバッグ コンソール」やウォッチで現在のフレームの式を評価したり、ステップ実行（ステップオーバー、ステップイン、ステップアウト）、続行、一時停止ができます。

- 条件付きブレークポイントとログポイント（`{式}` が値に置き換わる）に対応しています。「スクリプトのエラーで一時停止」は既定でオンで、エラーが起きたその場で止まり、スタックも残っています。
- 一時停止中も zing の画面は通常どおり操作でき、ほかのスクリプトのコマンドは続行するまで待機します。一時停止の時間は CPU 時間の上限に含まれません。
- スクリプトを再読み込みしてもブレークポイントは保持されます。
- 接続できるのは現在の Windows ユーザーだけです（名前付きパイプ `\\.\pipe\zing-lua-debug-ユーザー名`、リモート接続は拒否）。デバッガーの接続を受け付ける zing は同時に 1 つだけです。

## グローバル関数

現在の状態やユーザーとのやり取り。`sel`、`cwd`、`focus` はコマンド開始時のスナップショットを返す。

### `sel()`

選択中の項目。コマンド開始時のスナップショットで、実行中のコマンドでは変わらない

- 戻り値：string[] 選択中の項目のフルパス（変更不可のリスト）

```lua
for _, p in ipairs(sel()) do msg(p) end
```

### `cwd()`

現在のフォルダー（スナップショット）

- 戻り値：string 現在のフォルダー（「PC」は空文字列）

```lua
msg(cwd())
```

### `focus()`

フォーカスのある項目（スナップショット）

- 戻り値：string? フォーカスのある項目のフルパス

```lua
local f = focus()
```

### `go(path)`

フォルダーへ移動

- `path`：string フォルダー

```lua
go("D:\\Photos")
```

### `msg(text)`

ステータスバーにメッセージを表示

- `text`：string

```lua
msg("完了")
```

### `ask(prompt, default?)`

ユーザーに入力を求める（回答を待つ）

- `prompt`：string 質問の文
- `default?`：string 既定値
- 戻り値：string? 入力された文字列。キャンセル時は nil

```lua
local name = ask("新しいフォルダーの名前", "新しいフォルダー")
```

### `pick_folder(title?, default?)`

フォルダーを選ぶダイアログを表示（選ばれるまで待つ）

- `title?`：string ダイアログのタイトル
- `default?`：string 最初に表示するフォルダー
- 戻り値：string? 選択されたフォルダー。キャンセル時は nil

```lua
local dest = pick_folder("展開先", cwd())
```

### `clipboard(text?)`

クリップボードの文字列を読み書きする（改行は常に \n。書き込み時は Windows の \r\n に変換）

- `text?`：string クリップボードに入れる文字列。省略すると読み取り
- 戻り値：string|boolean|nil 読み取り時はクリップボードの文字列（文字列がなければ nil）。書き込み時は成功したかどうか

```lua
clipboard(table.concat(sel() | map(path.name) | list, "\n"))
local text = clipboard()
```

### `exec(script)`

コマンドスクリプトを実行

- `script`：string コマンドスクリプト（設定ファイルの cmd と同じ。{sel}、{ask:質問} などの変数が使える）

```lua
exec("copy {sel} --to={dest}")
```

### `log(...)`

scripts\zing.log に書き込む（デバッグ用）

- `...`：any

```lua
log("選択", #sel())
```

## 関数型ライブラリ

遅延評価、不変、純粋関数、合成。パイプラインは `|` でつなぐ：`ソース | 段階 | 段階 | 終端`。ソースはリスト（`sel()` など）または `seq(...)`。段階（`map`、`filter`…）は何をするかを記述するだけで、終端（`list`、`each`、`reduce`…）に来て初めて 1 項目ずつ計算される。`each` と `is_dir` 以外は純粋関数で、返されるリストは変更できない。パイプラインは式なので、Lua では単独の行として書けない（`sel() | each(f)` は構文エラー）：結果を変数に代入するか、副作用だけが目的ならメソッド呼び出し `seq(sel()):each(f)` を使う。

### `seq(source)`

遅延ストリームを作る：つないだ段階は値を取り出すときに 1 項目ずつ計算され、中間のリストは作られない

- `source`：table|function リストまたは反復関数
- 戻り値：Seq

```lua
local jpgs = seq(sel()):filter(function(p) return path.ext(p) == "jpg" end):take(10):list()
```

### `map(f)`

パイプラインの段階：各項目を変換

- `f`：fun(x: any): any
- 戻り値：Stage

```lua
local names = sel() | map(path.name) | list
```

### `filter(pred)`

パイプラインの段階：条件を満たす項目だけ残す

- `pred`：fun(x: any): boolean
- 戻り値：Stage

```lua
local jpgs = sel() | filter(function(p) return path.ext(p) == "jpg" end) | list
```

### `take(n)`

パイプラインの段階：先頭の n 項目を取る

- `n`：integer
- 戻り値：Stage

```lua
local first3 = sel() | take(3) | list
```

### `skip(n)`

パイプラインの段階：先頭の n 項目を飛ばす

- `n`：integer
- 戻り値：Stage

```lua
local rest = sel() | skip(1) | list
```

### `take_while(pred)`

パイプラインの段階：最初に条件を満たさない項目まで取る

- `pred`：fun(x: any): boolean
- 戻り値：Stage

```lua
local small = seq({1, 2, 5, 1}) | take_while(function(x) return x < 3 end) | list
```

### `sort_by(key)`

パイプラインの段階：キーで並べ替え（ストリーム全体を読む）

- `key`：fun(x: any): any 並べ替えのキー
- 戻り値：Stage

```lua
local sorted = sel() | sort_by(path.name) | list
```

### `each(f)`

パイプラインの終端：各項目に対して実行（副作用のある処理はここで）

- `f`：fun(x: any)
- 戻り値：Stage

```lua
seq(sel()):each(msg) -- 副作用だけが目的ならメソッド呼び出しで：Lua では a | b だけの行は書けない
```

### `reduce(f, init)`

パイプラインの終端：1 つの値に畳み込む

- `f`：fun(acc: any, x: any): any
- `init`：any
- 戻り値：Stage

```lua
local sum = seq({1, 2, 3}) | reduce(function(a, x) return a + x end, 0)
```

### `list(s)`

パイプラインの終端：すべての項目を変更不可のリストにする

- `s`：Seq|table
- 戻り値：table 変更不可のリスト

```lua
local names = sel() | map(path.name) | list
```

### `count(s)`

パイプラインの終端：項目数

- `s`：Seq|table
- 戻り値：integer

```lua
local dirs = sel() | filter(is_dir) | count
```

### `first(s)`

パイプラインの終端：最初の項目（なければ nil）

- `s`：Seq|table
- 戻り値：any

```lua
local f = sel() | first
```

### `compose(...)`

関数を合成（右から左へ）：compose(f, g)(x) == f(g(x))

- `...`：function
- 戻り値：function

```lua
local stem = compose(string.lower, path.stem)
```

### `pipe(x, ...)`

値を関数に順に通す（左から右へ）：pipe(x, f, g) == g(f(x))

- `x`：any
- `...`：function
- 戻り値：any

```lua
pipe(focus(), path.name, string.upper)
```

### `partial(f, ...)`

最初のいくつかの引数を固定

- `f`：function
- `...`：any 先に適用する引数
- 戻り値：function

```lua
local to_backup = partial(path.join, "D:\\バックアップ")
```

### `freeze(t)`

変更不可の読み取り専用版を返す（変更するとエラー）

- `t`：table
- 戻り値：table

```lua
local cfg = freeze({ dest = "D:\\" })
```

### `is_dir(p)`

フォルダーかどうか（ファイルシステムを読む）

- `p`：string
- 戻り値：boolean

```lua
local dirs = sel() | filter(is_dir) | list
```

### `stat(p)`

ファイルの情報：サイズ、更新／作成／アクセス日時、属性（ファイルシステムを読む）

- `p`：string ファイルまたはフォルダー
- 戻り値：{size: integer, modified: integer?, created: integer?, accessed: integer?, is_dir: boolean, readonly: boolean, hidden: boolean, system: boolean}?, string? 時刻は Unix 秒（os.date に渡せる）。フォルダーの size は 0。失敗時は nil とエラーメッセージ

```lua
local month = os.date("%Y-%m", stat(sel()[1]).modified)
local big = sel() | filter(function(p) return stat(p).size > 100 * 1024 * 1024 end) | list
```

### `path.name(p)`

ファイル名（拡張子を含む）

- `p`：string
- 戻り値：string

```lua
path.name("C:\\a\\b.txt") --> "b.txt"
```

### `path.stem(p)`

拡張子を除いたファイル名

- `p`：string
- 戻り値：string

```lua
path.stem("b.tar.gz") --> "b.tar"
```

### `path.ext(p)`

拡張子（小文字、ドットなし。ない場合は空文字列）

- `p`：string
- 戻り値：string

```lua
path.ext("A.JPG") --> "jpg"
```

### `path.dir(p)`

親フォルダー

- `p`：string
- 戻り値：string

```lua
path.dir("C:\\a\\b.txt") --> "C:\\a"
```

### `path.join(...)`

\ でパスを連結

- `...`：string
- 戻り値：string

```lua
path.join(cwd(), "新しいフォルダー")
```

## イベント

スクリプトでこれらの名前のグローバル関数を定義すると有効になります。

### `on_navigate(path)`

フォルダーに入ったときに呼ばれる（結果は待たない）

- `path`：string 移動先のフォルダー

```lua
function on_navigate(p)
  if p:find("\\Photos") then view.mode("large") end
end
```

### `thumbnail(path)`

サムネイルの元を決める（サムネイルごとに 1 回呼ばれる。CPU 時間の上限 0.3 秒）

- `path`：string ファイルまたはフォルダー
- 戻り値：string? サムネイルにする画像のパス。nil なら既定

```lua
function thumbnail(p)
  if path.ext(p) == "mp4" then
    local jpg = path.join(path.dir(p), path.stem(p) .. ".jpg")
    if io.open(jpg) then return jpg end
  end
end
```

## 組み込みコマンド

設定ファイルやコマンドパレットと同じコマンドを関数として呼ぶ。文字列・数値の引数は位置引数。テーブルの整数キーは位置引数（リストは展開される）、文字列キーは `--キー=値`（`true` は `--キー`、`false` は省略）。引数は変数展開されないので、パス中の `{` `}` が誤解されることはない。コマンドを画面に送るとスクリプトはすぐに続行し、コマンドの完了は待たない。

### `nav.back()`

戻る

```lua
nav.back()
```

### `nav.forward()`

進む

```lua
nav.forward()
```

### `nav.up()`

上の階層へ

```lua
nav.up()
```

### `nav.open()`

フォーカスのある項目を開く（フォルダーなら移動、ファイルは既定のプログラムで開く）

```lua
nav.open()
```

### `nav.refresh()`

最新の情報に更新

```lua
nav.refresh()
```

### `nav.home()`

ユーザーフォルダーへ戻る

```lua
nav.home()
```

### `nav.goto(path)`

フォルダーへ移動（`go` と同じ）

- `path`：string パス

```lua
nav["goto"]("C:\\Windows") -- goto は Lua の予約語なのでこう書く。go() も使える
```

### `address.edit()`

アドレスバーを編集

```lua
address.edit()
```

### `palette.open()`

コマンドパレットを開く

```lua
palette.open()
```

### `search.open()`

全ドライブ検索：すべての NTFS ドライブを名前やパスで検索し、絞り込みと並べ替えができる（構文は README を参照。NTFS インデックス サービスが必要）

```lua
search.open()
```

### `jobs.toggle()`

ファイル操作パネルの表示／非表示

```lua
jobs.toggle()
```

### `jobs.clear()`

完了したファイル操作を消去

```lua
jobs.clear()
```

### `file.copy()`

選択中の項目をクリップボードにコピー

```lua
file.copy()
```

### `file.cut()`

選択中の項目をクリップボードに切り取り

```lua
file.cut()
```

### `file.paste()`

クリップボードのファイルを現在のフォルダーに貼り付け

```lua
file.paste()
```

### `file.delete(opts?)`

選択中の項目を削除

- `opts?`：{permanent?: boolean} permanent が true のときはごみ箱を使わない

```lua
file.delete{ permanent = true }
```

### `file.rename()`

名前の変更（複数選択時は一括変更）

```lua
file.rename()
```

### `file.batchrename()`

選択中の項目の名前を一括変更

```lua
file.batchrename()
```

### `file.newfolder()`

現在のフォルダーに新しいフォルダーを作成

```lua
file.newfolder()
```

### `file.properties()`

選択中の項目のプロパティを表示

```lua
file.properties()
```

### `menu.context()`

フォーカスのある項目の右クリックメニューを表示

```lua
menu.context()
```

### `file.copypath()`

選択中の項目のフルパスをコピー

```lua
file.copypath()
```

### `copy(args)`

指定したフォルダーへコピーまたは移動（ファイル操作のキュー経由）

- `args`：{to: string, move?: boolean, [integer]: string} to は移動先フォルダー。位置引数はコピー元（省略時は選択中の項目）
- 別の書き方：`fun(sources: string[]|string, opts: {to: string, move?: boolean})`

```lua
copy{ to = "D:\\バックアップ", move = true }
```

### `run(program, ...)`

プログラムを実行（現在のフォルダーで）。コンソールプログラム（.bat、.cmd を含む）はコンソールウィンドウを表示せず、ウィンドウのあるプログラムは通常どおり表示。設定ファイルでは `run --console プログラム` でコンソールを表示

- `program`：string プログラム
- `...`：string|{wait?: boolean, console?: boolean} 引数。テーブルで wait = true のときはプログラムの終了を待って終了コードを返す（スクリプト内のみ。待ち時間は時間制限に含まれない）。console = true でコンソールウィンドウを表示（対話型ターミナル）
- 戻り値：integer?, string? 終了コード（wait のとき）。起動できないときは nil とエラーメッセージ
- 別の書き方：`fun(args: {[integer]: string, wait?: boolean, console?: boolean}): integer?, string?`

```lua
run("code", cwd())
local code = run{ "tar", "-xf", "a.zip", "-C", "D:\\out", wait = true }
run{ "pwsh", console = true }
```

### `open(...)`

既定のプログラムでファイルを開く

- `...`：string ファイル

```lua
open("C:\\a.pdf")
```

### `sel.all()`

すべて選択

```lua
sel.all()
```

### `sel.none()`

選択を解除

```lua
sel.none()
```

### `sel.invert()`

選択を反転

```lua
sel.invert()
```

### `tab.new(opts?)`

新しいタブを開く（既定は現在のフォルダー）

- `opts?`：{path?: string}

```lua
tab.new{ path = "C:\\" }
```

### `tab.close()`

タブを閉じる

```lua
tab.close()
```

### `tab.next()`

次のタブ

```lua
tab.next()
```

### `tab.prev()`

前のタブ

```lua
tab.prev()
```

### `pane.split()`

新しいペインに分割

```lua
pane.split()
```

### `pane.close()`

ペインを閉じる

```lua
pane.close()
```

### `pane.focusnext()`

次のペインへ移動

```lua
pane.focusnext()
```

### `tree.toggle()`

ツリーの表示／非表示

```lua
tree.toggle()
```

### `view.sort(opts)`

並べ替え

- `opts`：{by?: "name"|"modified"|"type"|"size", asc?: boolean, desc?: boolean} asc／desc だけのときは現在の列のまま

```lua
view.sort{ by = "size", desc = true }
```

### `view.foldersizes()`

フォルダーのサイズを計算

```lua
view.foldersizes()
```

### `thumb.set(opts?)`

サムネイルを手動で指定する（画像は %LOCALAPPDATA% にコピーされ、NTFS のファイル ID をキーにするので名前の変更や移動後も有効）

- `opts?`：{path?: string, image?: string} path の既定は現在の項目。image がなければ画像を選ぶダイアログを開く

```lua
thumb.set{ image = "D:\\カバー.jpg" }
```

### `thumb.clear(opts?)`

手動で指定したサムネイルを削除

- `opts?`：{path?: string}

```lua
thumb.clear()
```

### `cmdbar.edit()`

コマンドバーのカスタマイズ パネルを開く（結果はプログラムと同じ場所の toolbar.toml に保存）

```lua
cmdbar.edit()
```

### `cmdbar.hide()`

コマンドバーを非表示にする（config.toml の command_bar.visible に書き戻す）

```lua
cmdbar.hide()
```

### `file.newfile()`

現在のフォルダーにテキスト ドキュメントを作成して名前の変更を始める

```lua
file.newfile()
```

### `settings.open()`

設定ページを開く（変更は config.toml の該当行に書き戻され、コメントは保持されます）

```lua
settings.open()
```

### `keys.edit()`

ショートカットキーのエディターを開く（結果はプログラムの隣の keys.toml に保存）

```lua
keys.edit()
```

### `view.preview()`

プレビューウィンドウの表示／非表示

```lua
view.preview()
```

### `view.mode(mode)`

一覧／サムネイル表示を切り替え

- `mode`："list"|"small"|"medium"|"large"|"thumbnails"

```lua
view.mode("medium")
```

### `view.thumbsize(opts)`

サムネイルのサイズを変更

- `opts`：{delta: integer} 増減するピクセル数

```lua
view.thumbsize{ delta = 16 }
```

### `view.togglehidden()`

隠しファイルの表示／非表示

```lua
view.togglehidden()
```

### `index.install()`

NTFS インデックス サービスを有効にする（管理者権限が必要）

```lua
index.install()
```

### `index.uninstall()`

NTFS インデックス サービスを削除

```lua
index.uninstall()
```

### `index.status()`

NTFS インデックス サービスの状態を表示

```lua
index.status()
```

### `filter.clear()`

フィルターを解除

```lua
filter.clear()
```

### `filter.set(opts)`

フィルターを設定

- `opts`：{text: string} フィルターの文字列（ワイルドカード * ? 可）

```lua
filter.set{ text = "*.jpg" }
```

### `favorites.add(opts?)`

お気に入りに追加（既定は現在のフォルダー）

- `opts?`：{path?: string}

```lua
favorites.add()
```

### `favorites.remove(opts?)`

お気に入りから削除

- `opts?`：{path?: string}

```lua
favorites.remove{ path = cwd() }
```

### `favorites.move(opts)`

お気に入りの順序を変える（session.toml に保存。設定ファイルのお気に入りも並べ替えられる）

- `opts`：{by: integer, path?: string} by は移動する位置の数（負の数で上へ）。path の既定はツリーでフォーカスのあるお気に入り、なければ現在のフォルダー

```lua
favorites.move{ by = -1 }
```

### `theme.set(name)`

テーマを切り替え

- `name`：string テーマ名

```lua
theme.set("fluent")
```

### `theme.toggle()`

ダーク／ライトを切り替え（スタイルは維持）

```lua
theme.toggle()
```

### `theme.edit()`

テーマエディターを開く

```lua
theme.edit()
```

### `theme.editfile()`

現在のテーマファイルをテキストエディターで開く

```lua
theme.editfile()
```

### `theme.import()`

テーマをインポート

```lua
theme.import()
```

### `theme.export()`

現在のテーマをエクスポート

```lua
theme.export()
```

### `config.edit()`

設定ファイルをテキストエディターで開く

```lua
config.edit()
```

### `config.reload()`

設定を再読み込み

```lua
config.reload()
```

### `script.reload()`

Lua スクリプトを再読み込み

```lua
script.reload()
```

### `app.exit()`

プログラムを終了

```lua
app.exit()
```

### `update.check()`

今すぐ新しいバージョンを確認（1 日 1 回の制限と設定に関係なく、修正版やスキップしたバージョンを含め、新しいバージョンがあれば表示）

```lua
update.check()
```

### `welcome.show()`

ようこそ画面を開く（初回起動時に表示されるよく使う設定）

```lua
welcome.show()
```

### `cursor.up(opts?)`

一覧：フォーカスを上へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.up{ extend = true }
```

### `cursor.down(opts?)`

一覧：フォーカスを下へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.down{ extend = true }
```

### `cursor.left(opts?)`

一覧：フォーカスを左へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.left{ extend = true }
```

### `cursor.right(opts?)`

一覧：フォーカスを右へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.right{ extend = true }
```

### `cursor.pageup(opts?)`

一覧：フォーカスを1 ページ上へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.pageup{ extend = true }
```

### `cursor.pagedown(opts?)`

一覧：フォーカスを1 ページ下へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.pagedown{ extend = true }
```

### `cursor.first(opts?)`

一覧：フォーカスを先頭へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.first{ extend = true }
```

### `cursor.last(opts?)`

一覧：フォーカスを末尾へ移動して選択（縮小表示では左右が 1 マス、上下が 1 行）

- `opts?`：{extend?: boolean, keep?: boolean} extend は選択を広げる（Shift）。keep はフォーカスだけ移動し選択を変えない（Ctrl）。両方なら既存の選択を保ったまま広げる

```lua
cursor.last{ extend = true }
```

### `cursor.select()`

一覧：フォーカスの項目だけを選択

```lua
cursor.select()
```

### `cursor.toggle()`

一覧：フォーカスの項目の選択を切り替える（ほかは変えない）

```lua
cursor.toggle()
```

### `filter.backspace()`

フィルターの最後の 1 文字を削除（空になるとフィルター終了）

```lua
filter.backspace()
```

### `filter.start()`

フィルター開始：続けて入力した文字がフィルターになる（一覧での入力を「何もしない」にしたとき用）

```lua
filter.start()
```

### `tree.up()`

ツリー：フォーカスを上へ

```lua
tree.up()
```

### `tree.down()`

ツリー：フォーカスを下へ

```lua
tree.down()
```

### `tree.expand()`

ツリー：フォーカスのノードを展開（展開済みなら最初の子へ）

```lua
tree.expand()
```

### `tree.collapse()`

ツリー：フォーカスのノードを折りたたむ（折りたたみ済みなら親へ）

```lua
tree.collapse()
```

### `tree.open()`

ツリー：フォーカスのノードのフォルダーへ移動

```lua
tree.open()
```

### `tree.leave()`

ツリー：キーボード フォーカスをファイル一覧に戻す

```lua
tree.leave()
```

### `tree.moveup()`

ツリー：フォーカスのお気に入りを上へ移動（お気に入り以外では何もしない）

```lua
tree.moveup()
```

### `tree.movedown()`

ツリー：フォーカスのお気に入りを下へ移動（お気に入り以外では何もしない）

```lua
tree.movedown()
```

### `ui.up()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：上へ

```lua
ui.up()
```

### `ui.down()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：下へ

```lua
ui.down()
```

### `ui.left()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：左へ（コマンド バー、サブメニュー）

```lua
ui.left()
```

### `ui.right()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：右へ（コマンド バー、サブメニュー）

```lua
ui.right()
```

### `ui.pageup()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：1 ページ上へ

```lua
ui.pageup()
```

### `ui.pagedown()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：1 ページ下へ

```lua
ui.pagedown()
```

### `ui.first()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：先頭へ

```lua
ui.first()
```

### `ui.last()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：末尾へ

```lua
ui.last()
```

### `ui.accept()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：決定（選択した項目を実行）

```lua
ui.accept()
```

### `ui.cancel()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：キャンセル（閉じる）

```lua
ui.cancel()
```

### `ui.select()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：フォーカスの項目を切り替える（スイッチ、選択肢）

```lua
ui.select()
```

### `ui.next()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：次の欄

```lua
ui.next()
```

### `ui.prev()`

ポップアップ（メニュー、コマンド パレット、コマンド バー、サイド パネル）：前の欄

```lua
ui.prev()
```

### `search.sort(opts)`

ドライブ全体の検索：列で並べ替え（もう一度で逆順）。none で並べ替えを解除

- `opts`：{by: "name"|"path"|"ext"|"size"|"date"|"none"}

```lua
search.sort{ by = "size" }
```

### `search.openitem()`

ドライブ全体の検索：選択した結果を開く（フォルダーなら入る。Enter は場所へ移動）

```lua
search.openitem()
```

### `cmdbar.focus()`

キーボード フォーカスをコマンド バーへ（←→ 移動、Enter 実行、Esc で戻る）。もう一度で戻る

```lua
cmdbar.focus()
```
