param(
    [int]$ProcessId = 0,
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = 'Stop'
$reverseRoot = 'E:\reverse\e3d'
$hook = Join-Path $reverseRoot 'e3d-license-hook'
$execClr = Join-Path $hook 'exec_clr_method.py'
$bootstrap = Join-Path $hook 'E3DBootstrap_20260713_192908.dll'
$macro = Join-Path $PSScriptRoot 'ams_limits_ce.pmlmac'
$trace = Join-Path $reverseRoot 'ams_limits_ce_trace.txt'
$python = (Get-Command python.exe -ErrorAction Stop).Source
$injectTemp = Join-Path $reverseRoot 'temp\frida'
New-Item -ItemType Directory -Force -Path $injectTemp | Out-Null
$env:TEMP = $injectTemp
$env:TMP = $injectTemp

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
if ($LASTEXITCODE -ne 0) { throw 'Limits CE PML command rebind could not be queued' }

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    Start-Sleep -Milliseconds 500
    $passed = (Test-Path -LiteralPath $trace) -and
        (Select-String -LiteralPath $trace -SimpleMatch 'PASS LIMITS_CE registered' -Quiet)
    $failed = (Test-Path -LiteralPath $trace) -and
        (Select-String -LiteralPath $trace -SimpleMatch 'FAIL ' -Quiet)
} while (-not $passed -and -not $failed -and (Get-Date) -lt $deadline)

if (-not $passed) {
    $detail = if (Test-Path -LiteralPath $trace) { Get-Content -LiteralPath $trace -Raw } else { 'trace missing' }
    throw "Limits CE PML command rebind failed: $detail"
}
Write-Host "[OK] LIMITS_CE_COMMAND PID=$($des.ProcessId) TRACE=$trace"
