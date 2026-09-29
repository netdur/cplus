param(
    [switch]$Release,
    [string]$RuntimeDirectory = (Join-Path $PSScriptRoot 'out/runtime'),
    [string]$Packages = (Join-Path $PSScriptRoot '../../playground/qwen_winui_preview/packages'),
    [string]$ManifestTool = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\mt.exe'
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$shared = Join-Path $PSScriptRoot 'src/shared'
New-Item -ItemType Directory -Force -Path $shared | Out-Null
# Compile the existing gallery sources. The staging copy is ignored, not a fork.
foreach ($source in @('gallery.cplus','shell.cplus','demos')) {
    Copy-Item -LiteralPath (Join-Path $repo "examples/facet_gallery/src/$source") -Destination $shared -Recurse -Force
}
$compiler = Join-Path $repo 'target/release/cpc.exe'
& python (Join-Path $PSScriptRoot '../winui_standalone/prepare_runtime.py') --packages $Packages --out $RuntimeDirectory
if ($LASTEXITCODE -ne 0) { throw 'Runtime staging failed' }
$runtime = (Resolve-Path $RuntimeDirectory).Path
Copy-Item -LiteralPath (Join-Path $repo 'vendor/facet/assets/MaterialSymbolsOutlined.ttf') -Destination $runtime -Force
foreach ($required in @($compiler, $ManifestTool, (Join-Path $runtime 'resources.pri'), (Join-Path $runtime 'Microsoft.UI.Xaml.dll'))) {
    if (!(Test-Path -LiteralPath $required)) { throw "Missing prerequisite: $required" }
}
if (!(Test-Path -LiteralPath (Join-Path $PSScriptRoot 'vendor'))) {
    New-Item -ItemType Junction -Path (Join-Path $PSScriptRoot 'vendor') -Target (Join-Path $repo 'vendor') | Out-Null
}
$executable = Join-Path $runtime 'facet_gallery_winui.exe'
Push-Location $PSScriptRoot
try {
    $arguments = @('build', '-o', $executable)
    if ($Release) { $arguments += '--release' }
    & $compiler @arguments
    if ($LASTEXITCODE -ne 0) { throw 'C+ build failed' }
    & $ManifestTool '-nologo' '-manifest' (Join-Path $runtime 'app.manifest') "-outputresource:$executable;#1"
    if ($LASTEXITCODE -ne 0) { throw 'Manifest embedding failed' }
    Write-Output "Built: $executable"
} finally { Pop-Location }
