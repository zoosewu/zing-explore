<#
.SYNOPSIS
    由 crates/zing-app/assets 的 SVG 產生 zing.ico（16–256 px，PNG 壓縮）。修改 SVG 後執行一次並提交 .ico。

    24 px 以下用 zing-small.svg（去掉速度線、加粗筆畫），其餘用 zing.svg。
    以 Edge headless 繪製（與瀏覽器相同的 SVG 反鋸齒），使用暫存的設定檔資料夾，不影響使用者的 Edge。

.EXAMPLE
    pwsh tools/make-icon.ps1
#>
param([switch]$KeepPng)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root 'crates\zing-app\assets'
$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw '找不到 Microsoft Edge' }

$work = Join-Path $env:TEMP ("zing-icon-{0}" -f [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory $work | Out-Null
$sizes = 16, 20, 24, 32, 40, 48, 64, 96, 128, 256
$pngs = @()
try {
    foreach ($s in $sizes) {
        $svg = if ($s -le 24) { 'zing-small.svg' } else { 'zing.svg' }
        Copy-Item (Join-Path $assets $svg) (Join-Path $work $svg) -Force
        $html = Join-Path $work "icon-$s.html"
        Set-Content $html "<!doctype html><html><body style=`"margin:0;background:transparent`"><img src=`"$svg`" width=`"$s`" height=`"$s`" style=`"display:block`"></body></html>" -Encoding utf8
        $png = Join-Path $work "zing-$s.png"
        $profile = Join-Path $work 'profile'
        & $edge --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --default-background-color=00000000 `
            --user-data-dir="$profile" --window-size="$s,$s" --screenshot="$png" ("file:///" + ($html -replace '\\', '/')) 2>$null | Out-Null
        for ($i = 0; $i -lt 50 -and -not (Test-Path $png); $i++) { Start-Sleep -Milliseconds 100 }
        if (-not (Test-Path $png)) { throw "繪製 $s px 失敗" }
        $pngs += [pscustomobject]@{ Size = $s; Bytes = [IO.File]::ReadAllBytes($png) }
    }

    # ICO：ICONDIR + 每個尺寸一筆 ICONDIRENTRY，影像資料直接放 PNG
    $ms = New-Object IO.MemoryStream
    $w = New-Object IO.BinaryWriter $ms
    $w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$pngs.Count)
    $offset = 6 + 16 * $pngs.Count
    foreach ($p in $pngs) {
        $dim = if ($p.Size -ge 256) { 0 } else { $p.Size }
        $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
        $w.Write([uint16]1); $w.Write([uint16]32)
        $w.Write([uint32]$p.Bytes.Length); $w.Write([uint32]$offset)
        $offset += $p.Bytes.Length
    }
    foreach ($p in $pngs) { $w.Write($p.Bytes) }
    $w.Flush()
    $ico = Join-Path $assets 'zing.ico'
    [IO.File]::WriteAllBytes($ico, $ms.ToArray())
    Write-Host ("已產生 {0}（{1} 個尺寸，{2:N0} bytes）" -f $ico, $pngs.Count, $ms.Length)
    if ($KeepPng) { Write-Host "PNG：$work" }
} finally {
    if (-not $KeepPng) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
}
