<#
.SYNOPSIS
    fx 效能門檻檢查。每次變更後執行；任一指標超標或相對基準退步 >5% 即失敗（結束代碼 1）。

.EXAMPLE
    pwsh tools/perf.ps1                 # 建置 release 並量測
    pwsh tools/perf.ps1 -UpdateBaseline # 量測後把結果存成新基準（確認改善後才更新）
#>
param(
    [int]$Runs = 10,
    [switch]$UpdateBaseline,
    [switch]$SkipBuild,
    [int]$Files = 100000
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\fx.exe'
$baselinePath = Join-Path $root 'perf\baseline.json'
$resultsDir = Join-Path $root 'perf\results'
New-Item -ItemType Directory -Force $resultsDir | Out-Null

# 門檻：CLAUDE.md「核心開發注意事項」
$limits = [ordered]@{
    startup_ms             = 150
    private_working_set_mb = 30
    first_screen_ms        = 500
    frame_p99_ms           = 16
    exe_mb                 = 10
    theme_switch_ms        = 50
}
# 退步判定：同時超過 5% 與絕對容差才算（避免極小數值的雜訊）
$tolerance = @{
    startup_ms = 5; private_working_set_mb = 1; first_screen_ms = 15; load_all_ms = 20
    # frame_p99 受 OS/GPU 偶發停頓影響，同一版本重跑會差約 ±1.5 ms；繪製成本的退步由 p50 把關
    frame_p50_ms = 0.5; frame_p99_ms = 2; exe_mb = 0.05; theme_switch_ms = 2
}

# 剛安裝 rustup、尚未重開終端機時 PATH 可能還沒有 cargo
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

if (-not $SkipBuild) {
    Push-Location $root
    cargo build --release -p fx-app
    if ($LASTEXITCODE -ne 0) { throw 'cargo build 失敗' }
    Pop-Location
}

function Invoke-Fx([string[]]$fxArgs, [int]$timeoutMs = 60000) {
    $out = Join-Path $env:TEMP ("fx-perf-{0}.json" -f [guid]::NewGuid())
    $p = Start-Process -FilePath $exe -ArgumentList (@("--perf-out=$out") + $fxArgs) -PassThru
    if (-not $p.WaitForExit($timeoutMs)) { $p.Kill(); throw "fx 逾時：$fxArgs" }
    if (-not (Test-Path $out)) { throw "fx 沒有輸出結果：$fxArgs" }
    $r = Get-Content $out -Raw | ConvertFrom-Json
    Remove-Item $out
    return $r
}

# 測試資料夾：$Files 個檔案（每 100 個有一個資料夾），只建立一次
$benchDir = Join-Path $env:TEMP "fx-perf-$Files"
if (-not (Test-Path (Join-Path $benchDir '.complete'))) {
    Write-Host "建立測試資料夾 $benchDir（$Files 個項目，只需一次）..."
    New-Item -ItemType Directory -Force $benchDir | Out-Null
    $exts = 'txt', 'jpg', 'docx', 'rs', 'mp4', 'zip'
    for ($i = 0; $i -lt $Files; $i++) {
        if ($i % 100 -eq 0) {
            [IO.Directory]::CreateDirectory((Join-Path $benchDir "資料夾 $i")) | Out-Null
        } else {
            $name = 'IMG_{0:D6} 報告.{1}' -f $i, $exts[$i % $exts.Count]
            [IO.File]::Create((Join-Path $benchDir $name)).Dispose()
        }
    }
    New-Item -ItemType File (Join-Path $benchDir '.complete') | Out-Null
}

$results = [ordered]@{}
$results.exe_mb = [math]::Round((Get-Item $exe).Length / 1MB, 3)

Write-Host "量測冷啟動（$Runs 次）..."
$startups = 1..$Runs | ForEach-Object { (Invoke-Fx @()).startup_ms }
$sorted = $startups | Sort-Object
$results.startup_ms = [math]::Round($sorted[[int][math]::Floor(($sorted.Count - 1) / 2)], 2)

Write-Host '量測閒置記憶體（啟動 5 秒後）...'
$results.private_working_set_mb = [math]::Round((Invoke-Fx @('--perf-idle-ms=5000')).private_working_set_mb, 2)

# 單次的 p99 雜訊大，跑 3 次取各指標的中位數
Write-Host "量測開啟 $Files 個項目的資料夾、捲動與主題切換（3 次）..."
$folderRuns = 1..3 | ForEach-Object { Invoke-Fx @("--perf-dir=$benchDir", '--perf-scroll-frames=300', '--perf-theme') }
foreach ($k in 'first_screen_ms', 'load_all_ms', 'frame_p50_ms', 'frame_p99_ms', 'frame_max_ms', 'theme_switch_ms', 'listing_mb', 'scroll_warp') {
    $vals = @($folderRuns | ForEach-Object { $_.$k } | Where-Object { $null -ne $_ } | Sort-Object)
    if ($vals.Count -gt 0) { $results[$k] = [math]::Round($vals[[int][math]::Floor(($vals.Count - 1) / 2)], 2) }
}

$baseline = if (Test-Path $baselinePath) { Get-Content $baselinePath -Raw | ConvertFrom-Json } else { $null }
$failed = $false
$rows = foreach ($k in $results.Keys) {
    $v = $results[$k]
    $limit = $limits[$k]
    $base = if ($baseline -and $null -ne $baseline.$k) { [double]$baseline.$k } else { $null }
    $status = 'OK'
    if ($null -ne $limit -and $v -ge $limit) { $status = '超標'; $failed = $true }
    elseif ($null -ne $base -and $tolerance.ContainsKey($k) -and $v -gt $base * 1.05 -and ($v - $base) -gt $tolerance[$k]) {
        $status = '退步'; $failed = $true
    }
    [pscustomobject]@{ 指標 = $k; 數值 = $v; 門檻 = $limit; 基準 = $base; 狀態 = $status }
}
$rows | Format-Table -AutoSize | Out-String | Write-Host

$json = $results | ConvertTo-Json
Set-Content (Join-Path $resultsDir 'latest.json') $json -Encoding utf8
if ($UpdateBaseline) {
    Set-Content $baselinePath $json -Encoding utf8
    Write-Host "已更新基準：$baselinePath"
}
if ($failed) {
    Write-Host '效能檢查失敗：有指標超標或退步。' -ForegroundColor Red
    exit 1
}
Write-Host '效能檢查通過。' -ForegroundColor Green
