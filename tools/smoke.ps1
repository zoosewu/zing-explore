<#
.SYNOPSIS
    zing-explore 自動化 UI 冒煙測試。建立暫存測試資料夾，以 --smoke 模式執行 zing（debug build，含 UI 執行緒 I/O 防呆），
    印出每一步的結果；任一步失敗、狀態列出現錯誤或防呆有回報時結束代碼為 1。

    測試在程序內送出視窗訊息，不移動真實游標、視窗不搶前景，使用電腦時也可以執行。
    截圖存在 target\smoke\（BMP），失敗時可以對照。

.EXAMPLE
    pwsh tools/smoke.ps1             # 建置 debug 並測試
    pwsh tools/smoke.ps1 -Release    # 測 release build（沒有 I/O 防呆；之後 perf.ps1 會重新建置不含測試的版本）
    pwsh tools/smoke.ps1 -Keep       # 保留測試資料夾以便檢查
#>
param(
    [switch]$Release,
    [switch]$SkipBuild,
    [switch]$Keep,
    [int]$TimeoutSec = 120
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$profileDir = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $root "target\$profileDir\zing.exe"
$shots = Join-Path $root 'target\smoke'

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}
if (-not $SkipBuild) {
    Push-Location $root
    if ($Release) { cargo build --release -p zing-app --features smoke } else { cargo build -p zing-app --features smoke }
    if ($LASTEXITCODE -ne 0) { throw 'cargo build 失敗' }
    Pop-Location
}

# 測試資料夾（內容必須與 crates/zing-app/src/smoke.rs 的步驟一致）
$dir = Join-Path $env:TEMP ("zing-smoke-{0}" -f [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory $dir | Out-Null
$a = New-Item -ItemType Directory (Join-Path $dir '資料夾A')
$b = New-Item -ItemType Directory (Join-Path $dir '資料夾B')
Set-Content (Join-Path $a '內層.txt') 'inner'
for ($i = 1; $i -le 200; $i++) { [IO.File]::Create((Join-Path $b ('項目 {0:D3}.txt' -f $i))).Dispose() }
foreach ($n in 'a.txt', 'b.txt', '報告 9.txt', '報告 10.txt') { Set-Content (Join-Path $dir $n) $n }
Set-Content (Join-Path $dir '拖放來源.txt') '拖放測試'
$hidden = New-Item -ItemType File (Join-Path $dir '.hidden.txt')
$hidden.Attributes = 'Hidden'
# 檔案作業佇列測試用的大檔案（放在隱藏資料夾，不影響清單的項目數）；SetLength 立即完成
$jobs = New-Item -ItemType Directory (Join-Path $dir '.jobs')
$jobs.Attributes = 'Hidden, Directory'
$fs = [IO.File]::Create((Join-Path $jobs '大檔案.bin'))
$fs.SetLength(384MB)
$fs.Dispose()

if (Test-Path $shots) { Remove-Item -Recurse -Force $shots }
$out = Join-Path $env:TEMP ("zing-smoke-{0}.json" -f [guid]::NewGuid())
$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath $exe -ArgumentList "--smoke-out=`"$out`"", "--smoke-dir=`"$dir`"", "--smoke-shots=`"$shots`"" -PassThru
$timedOut = -not $p.WaitForExit($TimeoutSec * 1000)
if ($timedOut) { $p.Kill() }
$sw.Stop()

try {
    if (-not (Test-Path $out)) {
        $why = if ($timedOut) { "zing 逾時（$TimeoutSec 秒）" }
               elseif ($p.ExitCode -eq 2) { 'zing 不含冒煙測試功能：請不要加 -SkipBuild，或先以 --features smoke 建置' }
               else { "zing 沒有輸出結果（結束代碼 $($p.ExitCode)）" }
        throw $why
    }
    $r = Get-Content $out -Raw | ConvertFrom-Json
    Remove-Item $out

    $r.steps | ForEach-Object {
        [pscustomobject]@{
            結果 = if ($_.ok) { 'OK' } else { 'FAIL' }
            步驟 = $_.name
            ms   = $_.ms
            說明 = $_.detail
        }
    } | Format-Table -AutoSize -Wrap | Out-String -Width 200 | Write-Host
    $notes = ($r.notes.PSObject.Properties | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join '  '
    if ($notes) { Write-Host "觀察值：$notes" }
    foreach ($n in $r.not_run) { Write-Host "  未執行：$n" -ForegroundColor DarkGray }
    foreach ($e in $r.status_errors) { Write-Host "狀態列錯誤：$e" -ForegroundColor Red }
    foreach ($v in $r.guard_violations) { Write-Host "防呆回報：$v" -ForegroundColor Red }

    $passed = @($r.steps | Where-Object ok).Count
    $summary = "{0}/{1} 步通過，費時 {2:N1} 秒；截圖：{3}" -f $passed, ($r.steps.Count + $r.not_run.Count), $sw.Elapsed.TotalSeconds, $shots
    if ($r.ok) {
        Write-Host "冒煙測試通過：$summary" -ForegroundColor Green
    } else {
        Write-Host "冒煙測試失敗：$summary" -ForegroundColor Red
        exit 1
    }
} finally {
    if ($Keep) { Write-Host "測試資料夾：$dir" } else { Remove-Item -Recurse -Force $dir }
}
