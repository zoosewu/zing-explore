<#
.SYNOPSIS
    CI 用：從一個步驟的輸出找出失敗的原因，以 GitHub Actions 的 `::error` 輸出成錯誤摘要（annotation）。
    摘要在執行頁面上直接看得到，也能不登入從 API 讀取（日誌要登入才能下載）。
    沒有找到已知的失敗格式時，輸出最後 15 行。

.EXAMPLE
    cargo test --workspace 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath test.log
    $code = $LASTEXITCODE
    pwsh -NoProfile tools/ci-annotate.ps1 -Log test.log -Kind test -ExitCode $code
#>
param(
    [Parameter(Mandatory)][string]$Log,
    [Parameter(Mandatory)][ValidateSet('test', 'smoke', 'perf')][string]$Kind,
    [int]$ExitCode = 1
)
if ($ExitCode -eq 0 -or -not (Test-Path $Log)) { exit 0 }
$lines = @(Get-Content $Log)

function Write-Annotation([string]$title, [string[]]$body) {
    # 屬性與訊息裡的換行、% 要跳脫
    $esc = { param($s) $s.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A') }
    $t = (& $esc $title).Replace(':', '%3A').Replace(',', '%2C')
    Write-Host "::error title=$t::$(& $esc (($body | Select-Object -First 30) -join "`n"))"
}

$found = 0
switch ($Kind) {
    'test' {
        # ---- 測試名稱 stdout ---- 到下一個 ---- 或 failures: 為止
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -match '^---- (\S+) stdout ----$') {
                $name = $Matches[1]
                $body = for ($k = $i + 1; $k -lt $lines.Count -and $lines[$k] -notmatch '^(---- |failures:)'; $k++) { $lines[$k] }
                Write-Annotation "測試失敗：$name" @($body | Where-Object { $_.Trim() -and $_ -notmatch '^note: run with' })
                $found++
            }
        }
        # 測試程序當掉（沒有 failures: 清單），例如 STATUS_ACCESS_VIOLATION
        $lines | Where-Object { $_ -match "process didn't exit successfully" } | ForEach-Object {
            Write-Annotation '測試程序異常結束' @($_)
            $found++
        }
    }
    'smoke' {
        $hits = @($lines | Where-Object { $_ -match '^(FAIL\s|狀態列錯誤：|防呆回報：)|Lua API 檢查失敗|冒煙測試失敗|zing 逾時|zing 沒有輸出結果' })
        if ($hits) { Write-Annotation '冒煙測試失敗' $hits; $found++ }
    }
    'perf' {
        $hits = @($lines | Where-Object { $_ -match '超標|退步|效能檢查失敗' -and $_ -notmatch '（允許）' })
        if ($hits) { Write-Annotation '效能檢查失敗' $hits; $found++ }
    }
}
if ($found -eq 0) {
    Write-Annotation "失敗（$Kind，結束代碼 $ExitCode）" @($lines | Select-Object -Last 15)
}
