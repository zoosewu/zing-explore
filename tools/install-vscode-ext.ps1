<#
.SYNOPSIS
    安裝 zing-explore 的 Lua 除錯擴充功能（tools/vscode-zing-lua）到 VS Code。
    打包成 .vsix（不需要 Node.js 或 vsce）後以 `code --install-extension` 安裝；VS Code 與 VS Code Insiders 有安裝的都會裝。
    相依的 Lua Language Server（sumneko.lua，自動補全與型別檢查）會一起從擴充功能市集安裝。
    安裝後重新載入 VS Code 視窗，用 VS Code 開啟 zing 旁的 scripts 資料夾，按 F5 即可附加到 zing。

.EXAMPLE
    pwsh tools/install-vscode-ext.ps1
#>
$ErrorActionPreference = 'Stop'
$src = Join-Path $PSScriptRoot 'vscode-zing-lua'
$pkg = Get-Content (Join-Path $src 'package.json') -Raw | ConvertFrom-Json
$vsix = Join-Path ([IO.Path]::GetTempPath()) ("{0}-{1}.vsix" -f $pkg.name, $pkg.version)

$manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="$($pkg.name)" Version="$($pkg.version)" Publisher="$($pkg.publisher)" />
    <DisplayName>$($pkg.displayName)</DisplayName>
    <Description xml:space="preserve">$($pkg.description)</Description>
    <Categories>Debuggers</Categories>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="$($pkg.engines.vscode)" />
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
  </Assets>
</PackageManifest>
"@
$types = '<?xml version="1.0" encoding="utf-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension=".json" ContentType="application/json" /><Default Extension=".js" ContentType="application/javascript" /><Default Extension=".vsixmanifest" ContentType="text/xml" /></Types>'

Add-Type -AssemblyName System.IO.Compression
if (Test-Path $vsix) { Remove-Item $vsix }
$zip = [IO.Compression.ZipFile]::Open($vsix, 'Create')
try {
    $utf8 = [Text.UTF8Encoding]::new($false)
    foreach ($entry in @(@('extension.vsixmanifest', $manifest), @('[Content_Types].xml', $types))) {
        $w = [IO.StreamWriter]::new($zip.CreateEntry($entry[0]).Open(), $utf8)
        $w.Write($entry[1])
        $w.Dispose()
    }
    foreach ($f in 'package.json', 'extension.js') {
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, (Join-Path $src $f), "extension/$f")
    }
} finally {
    $zip.Dispose()
}

$installed = 0
foreach ($cli in 'code', 'code-insiders') {
    if (Get-Command $cli -ErrorAction SilentlyContinue) {
        & $cli --install-extension $vsix --force
        if ($LASTEXITCODE -eq 0) { $installed++ }
    }
}
Remove-Item $vsix
if ($installed -eq 0) { throw '找不到 code 或 code-insiders 命令：請確認 VS Code 已加入 PATH' }
Write-Host "已安裝 $($pkg.displayName) $($pkg.version)。請重新載入 VS Code 視窗。"
