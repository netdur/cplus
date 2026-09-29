$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$previousWalk=$env:FACET_GALLERY_WALK
$env:FACET_GALLERY_WALK='1'
try {
    $app=Start-Process -FilePath (Join-Path $root 'out/runtime/facet_gallery_winui.exe') -WorkingDirectory (Join-Path $root '../facet_gallery') -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'out/walk.log') -RedirectStandardError (Join-Path $root 'out/walk.err') -PassThru
} finally {if($null -eq $previousWalk) {Remove-Item Env:FACET_GALLERY_WALK} else {$env:FACET_GALLERY_WALK=$previousWalk}}
try {
    $deadline=(Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 200
        if($app.HasExited) {throw "Walk app exited: $($app.ExitCode)"}
        $done=(Get-Content (Join-Path $root 'out/walk.log') -Raw) -match 'visited 36 demos, no crash'
    } while(!$done -and (Get-Date) -lt $deadline)
    if(!$done) {throw 'Gallery walk did not complete'}
    $app.CloseMainWindow() | Out-Null
    if(!$app.WaitForExit(10000)) {throw 'Walk close timed out'}
    if($app.ExitCode -ne 0) {throw "Walk exit: $($app.ExitCode)"}
    Write-Output 'PASS: all 36 gallery pages and process exit code 0'
} finally {if(!$app.HasExited) {Stop-Process -Id $app.Id}}
