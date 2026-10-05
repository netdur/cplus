param(
    [string]$Packages = (Join-Path $PSScriptRoot '../playground/qwen_winui_preview/packages'),
    [string]$WindowsMetadata = 'C:\Program Files (x86)\Windows Kits\10\UnionMetadata\10.0.26100.0\Windows.winmd',
    [string]$Output = (Join-Path $PSScriptRoot '../vendor/winui')
)
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$mainMetadata = Join-Path $Packages 'Microsoft.WindowsAppSDK.WinUI.2.3.9/metadata/Microsoft.UI.Xaml.winmd'
$refs = @($WindowsMetadata,
    (Join-Path $Packages 'Microsoft.WindowsAppSDK.InteractiveExperiences.2.1.9/metadata/10.0.17763.0/Microsoft.UI.winmd'),
    (Join-Path $Packages 'Microsoft.WindowsAppSDK.InteractiveExperiences.2.1.9/metadata/10.0.17763.0/Microsoft.Foundation.winmd'),
    (Join-Path $Packages 'Microsoft.WindowsAppSDK.InteractiveExperiences.2.1.9/metadata/10.0.17763.0/Microsoft.Graphics.winmd'),
    (Join-Path $Packages 'Microsoft.Web.WebView2.1.0.3719.77/lib/Microsoft.Web.WebView2.Core.winmd'))
foreach ($file in @($mainMetadata) + $refs) { if (!(Test-Path -LiteralPath $file)) { throw "Missing SDK metadata: $file" } }
$arguments = @('--winmd', $mainMetadata, '--selection', (Join-Path $repoRoot 'vendor/winui/selection.json'), '--package', 'winui', '--out', $Output)
foreach ($reference in $refs) { $arguments += @('--reference', $reference) }
& (Join-Path $repoRoot 'target/release/cpc-bindgen.exe') @arguments
if ($LASTEXITCODE -ne 0) { throw 'WinMD generation failed' }
