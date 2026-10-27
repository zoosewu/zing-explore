<#
.SYNOPSIS
    以 Lua Language Server 檢查 Lua API 的定義檔（zing.d.lua，由 zing_core::scriptapi 產生）：
    把 examples/scripts/*.lua 與 docs/lua.md（及 lua.en.md、lua.ja.md）的每個範例放在一起，用 `lua-language-server --check` 檢查，
    應該沒有任何問題（自動補全與型別提示才正確）。修改 scriptapi.rs 或 prelude.lua 後執行。

    需要 VS Code 的 Lua 擴充功能（sumneko.lua），或以 -Server（或環境變數 LUALS，CI 用）指定 lua-language-server.exe。
    tools/smoke.ps1 每次都會以 -IfAvailable 執行：沒有安裝時只顯示警告。

.EXAMPLE
    pwsh tools/check-lua-api.ps1
#>
param([string]$Server, [switch]$IfAvailable)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Server) { $Server = $env:LUALS }
if (-not $Server) {
    $Server = Get-ChildItem "$env:USERPROFILE\.vscode\extensions", "$env:USERPROFILE\.vscode-insiders\extensions" -Directory -Filter 'sumneko.lua-*' -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'server\bin\lua-language-server.exe' } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
}
if (-not $Server -and $IfAvailable) {
    Write-Warning '略過 Lua API 檢查：沒有安裝 VS Code 的 Lua 擴充功能（sumneko.lua）'
    exit 0
}
if (-not $Server) { throw '找不到 lua-language-server.exe：請安裝 VS Code 的 Lua 擴充功能（sumneko.lua），或以 -Server 指定' }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

$base = if ($env:ZING_TEST_DIR) { $env:ZING_TEST_DIR } else { $env:TEMP }
$dir = Join-Path $base 'zing-lua-api-check'
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
New-Item -ItemType Directory -Force $dir | Out-Null

# 定義檔與 .luarc.json 由 scriptapi 的測試匯出（每種語言一個資料夾：zh-TW、en、ja）
$env:ZING_EXPORT_LUA_META = $dir
try {
    Push-Location $root
    cargo test -q -p zing-core --lib scriptapi::tests::docs_are_up_to_date | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'docs/lua*.md 不是最新：先執行 ZING_UPDATE_DOCS=1 cargo test -p zing-core' }
} finally {
    Pop-Location
    Remove-Item Env:ZING_EXPORT_LUA_META
}

$problems = @()
foreach ($lang in @(@{ code = 'zh-TW'; doc = 'lua.md' }, @{ code = 'en'; doc = 'lua.en.md' }, @{ code = 'ja'; doc = 'lua.ja.md' })) {
    $sub = Join-Path $dir $lang.code
    Copy-Item (Join-Path $root 'examples\scripts\*.lua') $sub
    # 文件的每個範例各放在 do ... end 裡（函式定義的範例放在最外層）
    $md = Get-Content (Join-Path $root "docs\$($lang.doc)") -Raw
    $blocks = [regex]::Matches($md, '```lua\r?\n(.*?)```', 'Singleline') | ForEach-Object {
        $code = $_.Groups[1].Value
        if ($code -match '^(---.*\r?\n)*function ') { $code } else { "do`n$code`nend`n" }
    }
    Set-Content (Join-Path $sub 'docs_examples.lua') ($blocks -join "`n") -Encoding utf8NoBOM

    $out = Join-Path $sub 'check.json'
    & $Server --check=$sub --checklevel=Information --check_format=json --check_out_path=$out | Out-Null
    if (Test-Path $out) {
        $r = Get-Content $out -Raw | ConvertFrom-Json
        foreach ($f in $r.PSObject.Properties) {
            foreach ($d in $f.Value) {
                $problems += '{0}/{1}:{2}  [{3}] {4}' -f $lang.code, (Split-Path ([uri]$f.Name).LocalPath -Leaf), ($d.range.start.line + 1), $d.code, ($d.message -replace "`r?`n", ' ')
            }
        }
    }
}
if ($problems.Count -gt 0) {
    $problems | ForEach-Object { Write-Host $_ }
    Write-Host "Lua API 檢查失敗：$($problems.Count) 個問題（檔案在 $dir）"
    exit 1
}
Remove-Item -Recurse -Force $dir
Write-Host 'Lua API 檢查通過：定義檔、範例腳本與文件範例都沒有問題。'
