# 把 Cargo.toml 的 [workspace.package] version 改成指定的版本號。
# 發佈時由 CI 執行（.github/workflows/release.yml）：以標籤的版本建置，Release 建立後再 commit 回 master。
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
