param([switch]$Release, [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
foreach ($name in @('agent_winui_smoke', 'facet_agent_winui_smoke')) {
    $example = Join-Path $repo "examples/$name"
    if (!$SkipBuild) {
        & (Join-Path $example 'build.ps1') -Release:$Release
    }
    $runtime = Join-Path $example 'out/runtime'
    $log = Join-Path $example 'out/run.log'
    $err = Join-Path $example 'out/run.err'
    $app = Start-Process -FilePath (Join-Path $runtime "$name.exe") -WorkingDirectory $runtime -WindowStyle Hidden -RedirectStandardOutput $log -RedirectStandardError $err -PassThru
    $handle = $app.Handle
    try {
        if (!$app.WaitForExit(30000)) { throw "$name timed out; see $log and $err" }
        Get-Content -LiteralPath $log
        Get-Content -LiteralPath $err
        if ($app.ExitCode -ne 0) { throw "$name exited with $($app.ExitCode)" }
        $marker = if ($name -eq 'agent_winui_smoke') { 'all stages completed and closed cleanly' } else { 'Facet agent detached and native resources released' }
        if (!(Select-String -LiteralPath $log -SimpleMatch $marker -Quiet)) { throw "$name exited without completing the assertions" }
        Write-Output "PASS: $name process exit 0"
    } finally {
        if (!$app.HasExited) { Stop-Process -Id $app.Id }
    }
}
