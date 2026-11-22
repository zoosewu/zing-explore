# 把 Cargo.toml 的 [workspace.package] version 改成指定的版本號，Cargo.lock 中本專案的套件跟著改。
# 發佈時由 CI 執行（.github/workflows/release.yml）：以標籤的版本建置，Release 建立後再 commit 回 master。
# Cargo.lock 直接改文字、不經 cargo：`cargo metadata --offline` 要讀每個平台的相依套件，CI 的快取（rust-cache）
# 只留 Windows 建置用到的，少了就失敗（v0.6.3、v0.6.4 版本號 commit 回 master 失敗的原因）。
param(
    [Parameter(Mandatory)][string]$Version
)

$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "版本號需為 主.次.修補（例如 0.2.0）：$Version" }
$path = Join-Path $PSScriptRoot '..\Cargo.toml'
$text = [IO.File]::ReadAllText($path)
$re = [regex]'(?ms)(^\[workspace\.package\][^\[]*?^version\s*=\s*)"[^"]*"'
if (-not $re.IsMatch($text)) { throw 'Cargo.toml 中找不到 [workspace.package] 的 version' }
$new = $re.Replace($text, { param($m) $m.Groups[1].Value + '"' + $Version + '"' }, 1)
[IO.File]::WriteAllText($path, $new)
Write-Host "Cargo.toml 的版本：$Version"

# Cargo.lock：本專案的套件是沒有 source 的那些（路徑相依）；它們都用 workspace 的版本
$lock = Join-Path $PSScriptRoot '..\Cargo.lock'
$text = [IO.File]::ReadAllText($lock)
$re = [regex]'(?m)(^\[\[package\]\]\r?\nname = "[^"]+"\r?\nversion = )"[^"]*"(?!\r?\nsource = )'
$n = $re.Matches($text).Count
if ($n -eq 0) { throw 'Cargo.lock 中找不到本專案的套件' }
[IO.File]::WriteAllText($lock, $re.Replace($text, { param($m) $m.Groups[1].Value + '"' + $Version + '"' }))
Write-Host "Cargo.lock 中本專案的 $n 個套件：$Version"
