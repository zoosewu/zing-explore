<#
.SYNOPSIS
    產生作品集（apex-code）的截圖：docs/screenshots/ 的 PNG 與 captions.json。

    以示範資料執行 zing 的 --smoke-showcase 模式（crates/zing-app/src/smoke/showcase.rs），繁體中文與英文各一輪。
    示範資料以 subst 掛成一個磁碟機的根目錄，畫面上不出現真實的路徑；結束時卸下並刪除。
    與冒煙測試相同：在程序內送出視窗訊息，不移動真實游標、不搶前景。視窗的用戶區是 1280×800。

.EXAMPLE
    pwsh tools/screenshots.ps1            # 建置（debug-abort，含冒煙測試功能）並產生截圖
    pwsh tools/screenshots.ps1 -SkipBuild
#>
param(
    [switch]$SkipBuild,
    [int]$TimeoutSec = 90
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug-abort\zing.exe'
$outDir = Join-Path $root 'docs\screenshots'

if (-not $SkipBuild) {
    Push-Location $root
    cargo build --profile debug-abort -p zing-app --features smoke
    if ($LASTEXITCODE -ne 0) { throw 'cargo build 失敗' }
    Pop-Location
}

# 示範資料的名稱（showcase.rs 的 Names 必須一致）
$langs = [ordered]@{
    'zh-TW' = @{
        docs = '文件'; photos = '相片'; backup = '備份'; music = '音樂'; projects = '專案'
        report = '2026 年度報告.docx'; notes = '會議紀錄 11-12.md'; budget = '預算.xlsx'; contract = '合約草稿.pdf'
        todo = '待辦清單.txt'; slides = '簡報'; receipts = '收據'; oldReport = '2025 年度報告.docx'
        photoNames = '湖畔', '山景', '夕陽'; slide = '產品簡介'; receipt = '收據'; song = '曲目'
        projectNames = '網站改版', '旅行相簿', '季度規劃'
        notesText = "# 會議紀錄`n`n- 確認第四季的時程`n- 新版網站在 12 月上線`n- 下次會議：11 月 26 日`n"
        todoText = "買機票`n寄出合約`n整理相片`n"
    }
    'en' = @{
        docs = 'Documents'; photos = 'Photos'; backup = 'Backup'; music = 'Music'; projects = 'Projects'
        report = 'Annual report 2026.docx'; notes = 'Meeting notes 11-12.md'; budget = 'Budget.xlsx'; contract = 'Draft contract.pdf'
        todo = 'To-do.txt'; slides = 'Slides'; receipts = 'Receipts'; oldReport = 'Annual report 2025.docx'
        photoNames = 'Lakeside', 'Mountains', 'Sunset'; slide = 'Product overview'; receipt = 'Receipt'; song = 'Track'
        projectNames = 'Website redesign', 'Travel album', 'Quarterly plan'
        notesText = "# Meeting notes`n`n- Confirm the Q4 schedule`n- New website goes live in December`n- Next meeting: Nov 26`n"
        todoText = "Book flights`nSend the contract`nSort photos`n"
    }
}

$captions = [ordered]@{
    '01-main-window'          = @{ 'zh-TW' = '主視窗：樹狀列、檔案清單與資料夾大小'; 'en' = 'Main window: folder tree, file list and folder sizes' }
    '02-thumbnails'           = @{ 'zh-TW' = '縮圖檢視'; 'en' = 'Thumbnail view' }
    '03-folder-compare'       = @{ 'zh-TW' = '資料夾比對與同步'; 'en' = 'Compare and sync folders' }
    '04-settings'             = @{ 'zh-TW' = '設定'; 'en' = 'Settings' }
    '05-light-theme-preview'  = @{ 'zh-TW' = 'Fluent 淺色主題與預覽窗格'; 'en' = 'Fluent light theme with the preview pane' }
}

$rng = [Random]::new(20261119)

function Set-Time([string]$path, [datetime]$when) {
    (Get-Item -LiteralPath $path).LastWriteTime = $when
}

function New-Blob([string]$path, [int]$bytes, [datetime]$when) {
    $buf = [byte[]]::new($bytes)
    $rng.NextBytes($buf)
    [IO.File]::WriteAllBytes($path, $buf)
    Set-Time $path $when
}

function New-Text([string]$path, [string]$text, [datetime]$when) {
    [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))
    Set-Time $path $when
}

# 風景圖：漸層的天空、太陽、兩層山、（湖畔）水面倒影
function New-Landscape([string]$path, [int]$kind, [int]$seed) {
    $r = [Random]::new($seed)
    $w, $h = 1200, 800
    $bmp = [Drawing.Bitmap]::new($w, $h)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $skies = @(
        @([Drawing.Color]::FromArgb(110, 170, 225), [Drawing.Color]::FromArgb(205, 230, 245)),
        @([Drawing.Color]::FromArgb(70, 120, 190), [Drawing.Color]::FromArgb(190, 215, 235)),
        @([Drawing.Color]::FromArgb(250, 140, 80), [Drawing.Color]::FromArgb(255, 210, 140))
    )
    $sky = $skies[$kind]
    $horizon = if ($kind -eq 0) { 470 } else { 620 }
    $brush = [Drawing.Drawing2D.LinearGradientBrush]::new([Drawing.Point]::new(0, 0), [Drawing.Point]::new(0, $horizon), $sky[0], $sky[1])
    $g.FillRectangle($brush, 0, 0, $w, $h)
    $sunX = 200 + $r.Next(800); $sunY = 120 + $r.Next(200)
    $sunColor = if ($kind -eq 2) { [Drawing.Color]::FromArgb(255, 240, 200) } else { [Drawing.Color]::FromArgb(255, 250, 225) }
    $g.FillEllipse([Drawing.SolidBrush]::new($sunColor), $sunX - 50, $sunY - 50, 100, 100)
    $layers = if ($kind -eq 2) {
        @([Drawing.Color]::FromArgb(150, 80, 90), [Drawing.Color]::FromArgb(80, 40, 60))
    } else {
        @([Drawing.Color]::FromArgb(95, 130, 160), [Drawing.Color]::FromArgb(50, 95, 80))
    }
    for ($l = 0; $l -lt 2; $l++) {
        $pts = [Collections.Generic.List[Drawing.Point]]::new()
        $pts.Add([Drawing.Point]::new(0, $horizon))
        $x = 0
        while ($x -lt $w) {
            $x += 80 + $r.Next(160)
            $peak = $horizon - 120 - $r.Next(220) + $l * 90
            $pts.Add([Drawing.Point]::new([Math]::Min($x, $w), $peak))
        }
        $pts.Add([Drawing.Point]::new($w, $horizon))
        $g.FillPolygon([Drawing.SolidBrush]::new($layers[$l]), $pts.ToArray())
    }
    $ground = if ($kind -eq 0) {
        [Drawing.Drawing2D.LinearGradientBrush]::new([Drawing.Point]::new(0, $horizon), [Drawing.Point]::new(0, $h + 1), [Drawing.Color]::FromArgb(70, 120, 170), [Drawing.Color]::FromArgb(30, 60, 100))
    } else {
        [Drawing.SolidBrush]::new($layers[1])
    }
    $g.FillRectangle($ground, 0, $horizon, $w, $h - $horizon)
    if ($kind -eq 0) {
        $pen = [Drawing.Pen]::new([Drawing.Color]::FromArgb(90, 255, 255, 255), 3)
        for ($i = 0; $i -lt 14; $i++) {
            $y = $horizon + 30 + $r.Next($h - $horizon - 40); $x = $r.Next($w - 200)
            $g.DrawLine($pen, $x, $y, $x + 60 + $r.Next(140), $y)
        }
    }
    $g.Dispose()
    $bmp.Save($path, [Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

function New-Demo([string]$dir, $n) {
    $base = [datetime]'2026-11-12 09:30'
    $docs = New-Item -ItemType Directory (Join-Path $dir $n.docs)
    $slides = New-Item -ItemType Directory (Join-Path $docs $n.slides)
    for ($i = 1; $i -le 3; $i++) { New-Blob (Join-Path $slides ("{0} v{1}.pptx" -f $n.slide, $i)) (900KB + $rng.Next(2MB)) $base.AddDays(-20 + $i * 3) }
    $receipts = New-Item -ItemType Directory (Join-Path $docs $n.receipts)
    for ($i = 1; $i -le 8; $i++) { New-Blob (Join-Path $receipts ("{0} 2026-{1:D2}.pdf" -f $n.receipt, $i + 2)) (60KB + $rng.Next(120KB)) $base.AddDays(-250 + $i * 30) }
    New-Blob (Join-Path $docs $n.report) 184KB $base.AddDays(-1).AddHours(5)
    New-Text (Join-Path $docs $n.notes) $n.notesText $base
    New-Blob (Join-Path $docs $n.budget) 46KB $base.AddDays(-3)
    New-Blob (Join-Path $docs $n.contract) 1.2MB $base.AddDays(-6)
    New-Text (Join-Path $docs $n.todo) $n.todoText $base.AddHours(2)
    Set-Time $slides $base.AddDays(-11); Set-Time $receipts $base.AddDays(-10)

    # 備份：較舊的文件，少了待辦、多了去年的報告、會議紀錄是舊的內容
    $backup = New-Item -ItemType Directory (Join-Path $dir $n.backup)
    Copy-Item -Recurse $docs.FullName $backup.FullName
    $bdocs = Join-Path $backup $n.docs
    Remove-Item (Join-Path $bdocs $n.todo)
    New-Text (Join-Path $bdocs $n.notes) ($n.notesText -replace '26', '19') $base.AddDays(-7)
    New-Blob (Join-Path $bdocs $n.oldReport) 171KB $base.AddDays(-320)
    Get-ChildItem -Recurse $bdocs | Where-Object { $_.LastWriteTime -gt $base.AddDays(-7) } | ForEach-Object { $_.LastWriteTime = $base.AddDays(-8) }

    $photos = New-Item -ItemType Directory (Join-Path $dir $n.photos)
    $seed = 7
    for ($k = 0; $k -lt 3; $k++) {
        for ($i = 1; $i -le 4; $i++) {
            $p = Join-Path $photos ("{0} {1:D2}.png" -f $n.photoNames[$k], $i)
            New-Landscape $p $k ($seed++)
            Set-Time $p $base.AddDays(-40 + $k * 4 + $i).AddHours($i)
        }
    }
    $music = New-Item -ItemType Directory (Join-Path $dir $n.music)
    for ($i = 1; $i -le 6; $i++) { New-Blob (Join-Path $music ("{0} {1:D2}.mp3" -f $n.song, $i)) (3MB + $rng.Next(4MB)) $base.AddDays(-90 + $i) }
    $projects = New-Item -ItemType Directory (Join-Path $dir $n.projects)
    foreach ($p in $n.projectNames) { New-Item -ItemType Directory (Join-Path $projects $p) | Out-Null }
}

# 沒有使用中的磁碟機代號（從 Z 往前找）
function Get-FreeLetter {
    $used = [IO.DriveInfo]::GetDrives() | ForEach-Object { $_.Name.Substring(0, 1).ToUpper() }
    foreach ($c in [char[]]'ZYXWVUTSRQPONMLKJIHG') { if ($used -notcontains [string]$c) { return "$($c):" } }
    throw '沒有可用的磁碟機代號'
}

$testRoot = if ($env:ZING_TEST_DIR) { $env:ZING_TEST_DIR } else { $env:TEMP }
New-Item -ItemType Directory -Force $outDir | Out-Null
Get-ChildItem $outDir -Filter '*.png' | Remove-Item
$failed = $false

foreach ($lang in $langs.Keys) {
    $n = $langs[$lang]
    $id = [guid]::NewGuid().ToString('N').Substring(0, 8)
    $dir = Join-Path $testRoot "zing-showcase-$id"
    $shots = Join-Path $testRoot "zing-showcase-shots-$id"
    $out = Join-Path $testRoot "zing-showcase-$id.json"
    New-Item -ItemType Directory $dir | Out-Null
    $drive = $null
    try {
        New-Demo $dir $n
        $drive = Get-FreeLetter
        subst $drive $dir
        if ($LASTEXITCODE -ne 0) { throw "subst $drive 失敗" }
        $p = Start-Process -FilePath $exe -ArgumentList "--smoke-out=`"$out`"", "--smoke-dir=$drive\", "--smoke-shots=`"$shots`"", "--smoke-showcase=$lang" -PassThru
        if (-not $p.WaitForExit($TimeoutSec * 1000)) { $p.Kill(); throw "zing 逾時（$TimeoutSec 秒）" }
        if (-not (Test-Path $out)) { throw "zing 沒有輸出結果（結束代碼 $($p.ExitCode)）" }
        $r = Get-Content $out -Raw | ConvertFrom-Json
        $r.steps | ForEach-Object { '{0,-4} {1}  {2}' -f ($(if ($_.ok) { 'OK' } else { 'FAIL' })), $_.name, $_.detail } | Write-Host
        foreach ($e in $r.status_errors) { Write-Host "狀態列錯誤：$e" -ForegroundColor Red }
        foreach ($v in $r.guard_violations) { Write-Host "防呆回報：$v" -ForegroundColor Red }
        if (-not $r.ok) { $failed = $true; Write-Host "$lang 的截圖失敗" -ForegroundColor Red; continue }
        foreach ($key in $captions.Keys) {
            $bmp = Join-Path $shots "$key.bmp"
            $img = [Drawing.Image]::FromFile($bmp)
            $png = Join-Path $outDir "$key.$lang.png"
            $img.Save($png, [Drawing.Imaging.ImageFormat]::Png)
            $size = '{0}×{1}' -f $img.Width, $img.Height
            $img.Dispose()
            Write-Host ("{0}  {1}  {2:N0} KB" -f (Split-Path -Leaf $png), $size, ((Get-Item $png).Length / 1KB))
        }
    } finally {
        if ($drive) { subst $drive /d | Out-Null }
        Remove-Item -Recurse -Force $dir, $shots -ErrorAction SilentlyContinue
        Remove-Item -Force $out -ErrorAction SilentlyContinue
    }
}

$json = $captions | ConvertTo-Json
[IO.File]::WriteAllText((Join-Path $outDir 'captions.json'), $json + "`n", [Text.UTF8Encoding]::new($false))
if ($failed) { exit 1 }
Write-Host "截圖：$outDir" -ForegroundColor Green
