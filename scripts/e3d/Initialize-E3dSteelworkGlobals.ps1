param(
    [int]$ProcessId = 0,
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = 'Stop'
$reverseRoot = 'E:\reverse\e3d'
$hook = Join-Path $reverseRoot 'e3d-license-hook'
$execClr = Join-Path $hook 'exec_clr_method.py'
$bootstrap = Join-Path $hook 'E3DBootstrap_20260713_192908.dll'
$macro = Join-Path $PSScriptRoot 'ams_steelwork_globals.pmlmac'
$trace = Join-Path $reverseRoot 'ams_steelwork_globals_trace.txt'
$python = (Get-Command python.exe -ErrorAction Stop).Source

$des = if ($ProcessId -gt 0) {
    Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId"
} else {
    Get-CimInstance Win32_Process -Filter "Name='des.exe'" |
        Where-Object { $_.CommandLine -match '\sams\s' } |
        Sort-Object CreationDate -Descending |
        Select-Object -First 1
}
if (-not $des) { throw 'AMS des.exe not found' }

Remove-Item -LiteralPath $trace -ErrorAction SilentlyContinue
$command = '$M "' + $macro.Replace('\', '/') + '"'
& $python $execClr --pid $des.ProcessId --dll $bootstrap `
    --type E3DBootstrap.Entry --method QueueThreadedDirectMacroNoRefreshFromHost --arg $command
if ($LASTEXITCODE -ne 0) { throw 'Steelwork PML initialization could not be queued' }

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    Start-Sleep -Milliseconds 500
    $passed = (Test-Path -LiteralPath $trace) -and
        (Select-String -LiteralPath $trace -SimpleMatch 'PASS STEELWORKGSETTINGS ready' -Quiet)
    $failed = (Test-Path -LiteralPath $trace) -and
        (Select-String -LiteralPath $trace -SimpleMatch 'FAIL ' -Quiet)
} while (-not $passed -and -not $failed -and (Get-Date) -lt $deadline)

if (-not $passed) {
    $detail = if (Test-Path -LiteralPath $trace) { Get-Content -LiteralPath $trace -Raw } else { 'trace missing' }
    throw "Steelwork PML initialization failed: $detail"
}
Write-Host "[OK] STEELWORK_GLOBALS PID=$($des.ProcessId) TRACE=$trace"
