param([ValidateSet('start','start-service','stop-service','status','restart-db','stop')][string]$Action = 'status')
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$run = Join-Path $repo 'output/rocksdb-8000-20260905'
$statePath = Join-Path $run 'processes.json'
$surreal = Join-Path $repo 'bin/surreal.exe'
$service = 'D:/Rust/target/debug/aios-database.exe'
function Start-Db {
    $p = Start-Process -FilePath $surreal -ArgumentList @('start','--user','root','--pass','root','--bind','127.0.0.1:18049',('rocksdb:' + ($run -replace '\\','/') + '/store')) -WorkingDirectory $run -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $run 'db.stdout.log') -RedirectStandardError (Join-Path $run 'db.stderr.log')
    return $p.Id
}
function Stop-Owned([int]$ProcessId, [string]$Expected) {
    $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($p) {
        if ([IO.Path]::GetFullPath($p.Path) -ne [IO.Path]::GetFullPath($Expected)) { throw "PID $ProcessId no longer belongs to $Expected" }
        Stop-Process -Id $ProcessId
        $p.WaitForExit()
    }
}
if ($Action -eq 'start') {
    if (Test-Path $statePath) { throw 'Run already exists; inspect its state before starting again' }
    if (Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object LocalPort -in 18049,18050) { throw 'Test ports are occupied' }
    New-Item -ItemType Directory -Path $run -Force | Out-Null
    $config = [IO.File]::ReadAllText((Join-Path $repo 'DbOption.toml'))
    $overrides = @{
        v_ip='"127.0.0.1"'; v_port='18049'; surreal_ns='"rocks8000_20260905"';
        http_api_addr='"127.0.0.1:18050"'; store_mode='"rocksdb"'; data_face='"ingest"';
        watch_dbnums='[8000]'; manual_db_nums='[8000]'; startup_autorun='true';
        gen_model='false'; gen_mesh='true'; gen_spatial_tree='false';
        meshes_path=('"' + ($run -replace '\\','/') + '/meshes"')
    }
    foreach ($key in $overrides.Keys) {
        $pattern = '(?m)^' + $key + '\s*=.*$'
        if ($config -match $pattern) { $config = [regex]::Replace($config,$pattern,($key+' = '+$overrides[$key])) }
        else { $config = $key+' = '+$overrides[$key]+"`n"+$config }
    }
    [IO.File]::WriteAllText((Join-Path $run 'DbOption.toml'),$config)
    foreach ($name in @('all_attr_info.json','noun_flags.json')) {
        Copy-Item -LiteralPath (Join-Path $repo $name) -Destination $run
    }
    Copy-Item -LiteralPath (Join-Path $repo 'resource') -Destination $run -Recurse
    $env:DB_OPTION_FILE = Join-Path $run 'DbOption'
    $env:AIOS_STORE_MODE = 'rocksdb'
    $env:AIOS_DATA_FACE = 'ingest'
    $env:AIOS_STARTUP_AUTORUN = '1'
    $env:RUST_MIN_STACK = '134217728'
    $dbPid = Start-Db
    Start-Sleep -Seconds 2
    $p = Start-Process -FilePath $service -ArgumentList @('serve','--watch-dbnum','8000') -WorkingDirectory $run -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $run 'service.stdout.log') -RedirectStandardError (Join-Path $run 'service.stderr.log')
    @{ db_pid=$dbPid; service_pid=$p.Id; service_exe=$service; db_exe=$surreal } | ConvertTo-Json | Set-Content $statePath
}
if ($Action -eq 'start-service') {
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    if (Get-Process -Id $state.service_pid -ErrorAction SilentlyContinue) { throw 'Service is still running' }
    if (!(Test-Path (Join-Path $run 'resource'))) {
        Copy-Item -LiteralPath (Join-Path $repo 'resource') -Destination $run -Recurse
    }
    $env:DB_OPTION_FILE = Join-Path $run 'DbOption'
    $env:AIOS_STORE_MODE = 'rocksdb'
    $env:AIOS_DATA_FACE = 'ingest'
    $env:AIOS_STARTUP_AUTORUN = '1'
    $env:RUST_MIN_STACK = '134217728'
    $p = Start-Process -FilePath $service -ArgumentList @('serve','--watch-dbnum','8000') -WorkingDirectory $run -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $run 'service.stdout.log') -RedirectStandardError (Join-Path $run 'service.stderr.log')
    $state.service_pid = $p.Id
    $state | ConvertTo-Json | Set-Content $statePath
}
if ($Action -eq 'restart-db') {
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    Stop-Owned $state.service_pid $state.service_exe
    Stop-Owned $state.db_pid $state.db_exe
    $state.db_pid = Start-Db
    $state | ConvertTo-Json | Set-Content $statePath
}
if ($Action -eq 'stop') {
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    Stop-Owned $state.service_pid $state.service_exe
    Stop-Owned $state.db_pid $state.db_exe
}
if ($Action -eq 'stop-service') {
    $state = Get-Content $statePath -Raw | ConvertFrom-Json
    Stop-Owned $state.service_pid $state.service_exe
}
Get-Content $statePath
if ($Action -eq 'status') {
    Get-Content (Join-Path $run 'service.stdout.log') -Tail 12
    Get-Content (Join-Path $run 'service.stderr.log') -Tail 12
    try { Invoke-RestMethod 'http://127.0.0.1:18050/api/v1/health' -TimeoutSec 3 | ConvertTo-Json -Depth 8 } catch { Write-Output $_.Exception.Message }
}
