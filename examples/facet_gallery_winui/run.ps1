param(
    [switch]$Verify,
    [ValidateSet("all","carousel","menus","tree-rows")][string]$VerifyMode="all",
    [switch]$Release,
    [switch]$SkipBuild,
    [string]$RuntimeDirectory = (Join-Path $PSScriptRoot 'out/runtime'),
    [string]$Packages = (Join-Path $PSScriptRoot '../../playground/qwen_winui_preview/packages')
)
$ErrorActionPreference = 'Stop'
if (!$SkipBuild -or !(Test-Path -LiteralPath (Join-Path $RuntimeDirectory 'facet_gallery_winui.exe'))) {
    & (Join-Path $PSScriptRoot 'build.ps1') -Release:$Release -RuntimeDirectory $RuntimeDirectory -Packages $Packages
}
$runtime = (Resolve-Path $RuntimeDirectory).Path
$logs = Join-Path $PSScriptRoot 'out'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
$app = Start-Process -FilePath (Join-Path $runtime 'facet_gallery_winui.exe') -WorkingDirectory (Join-Path $PSScriptRoot '../facet_gallery') -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs 'run.log') -RedirectStandardError (Join-Path $logs 'run.err') -PassThru
if ($Verify) {
    try {
        $deadline = (Get-Date).AddSeconds(20)
        do {
            Start-Sleep -Milliseconds 100
            if ($app.HasExited) { throw "App exited before ready: $($app.ExitCode). See out/run.log and out/run.err." }
            $ready = (Get-Content -LiteralPath (Join-Path $logs 'run.log') -Raw) -match 'READY: facet_winui'
        } while (!$ready -and (Get-Date) -lt $deadline)
        if (!$ready) { throw 'Gallery did not load within 20 seconds' }
        $verificationArguments = @()
        if ($VerifyMode -ne 'all') {$verificationArguments += "--$VerifyMode-only"}
        & python (Join-Path $PSScriptRoot 'verify_ui.py') @verificationArguments
        if ($LASTEXITCODE -ne 0) { throw 'Pointer/cleanup verification failed' }
        if (!$app.WaitForExit(10000)) { throw 'App did not exit after window close' }
        if ($app.ExitCode -ne 0) { throw "App exit code: $($app.ExitCode)" }
        $mode = if ($Release) { 'release' } else { 'debug' }
        Copy-Item -LiteralPath (Join-Path $logs 'run.log') -Destination (Join-Path $logs "verified-$mode.log")
        Write-Output "PASS: $mode process exit code 0"
    } finally {
        if (!$app.HasExited) { Stop-Process -Id $app.Id }
    }
} else {
    Write-Output "App PID: $($app.Id). Browse the gallery and try the controls. Log: $logs/run.log"
}
