<#
.SYNOPSIS
    给 plant3d-web 的增量更新 K 组端到端用例拉起一台隔离的 gen-model 服务
    （docs/plans/2026-10-27-plant3d-web-incremental-linkage-test-cases.md §5）。

.DESCRIPTION
    回放形态（缺省）：读 tests/increment_replay/cases/<Case>.json，造影子工程根（New-ShadowProjectRoot.ps1），
    db_session_fixture replay-init 到用例的 init.to，派生 DbOption.toml（与 tests/increment_replay 一样只改六个键），
    起一次性 rocksdb surreal + aios-database（摄入形态），等三水位齐平到 init.to，把 init.generate_prime 的根叫进投影，
    最后写出 stack.json 给 plant3d-web 的 e2e/incremental-linkage-gen-model-v1.spec.ts（LINKAGE_REPLAY_STACK）。
    不带用例里的 service_env（如 H01 的冻结延迟钩）：前端用例要的是普通的窗。

    现场形态（-Live）：不回放，project_path 直接指向 -ProjectsDir——必须是与原件完全断开的隔离副本（E3D 会往里写），
    脚本先核一遍。stack.json 多带 E3D 一腿的脚本（scripts/e3d/Invoke-LinkageE3dLeg.ps1）与它要的参数（LINKAGE_LIVE_STACK）。

    -Stop <stack.json>：只停 stack.json 里记下的那两个进程（服务与 surreal）。

.EXAMPLE
    scripts/replay/Start-LinkageReplayStack.ps1 -Case ams8000-equi-move-chain
.EXAMPLE
    scripts/replay/Start-LinkageReplayStack.ps1 -Case db8000-two-delete
.EXAMPLE
    scripts/replay/Start-LinkageReplayStack.ps1 -Live -ProjectsDir E:\reverse\e3d\linkage-live\E3D3.1
.EXAMPLE
    scripts/replay/Start-LinkageReplayStack.ps1 -Stop D:\work\plant-code\old\_runs\plantui-linkage\ams8000-equi-move-chain-20261027-150000\stack.json
#>
[CmdletBinding()]
param(
    [string]$Case = 'ams8000-equi-move-chain',
    [switch]$Live,
    [string]$ProjectsDir,
    [int]$Dbnum = 8000,
    [string]$Stop,
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1',
    [string]$RunRoot,
    [int]$Port = 9199,
    [int]$SurrealPort = 8199,
    [string]$TargetDir = 'D:\Rust\target',
    [ValidateSet('release', 'debug')][string]$BuildProfile = 'release',
    [string]$OutputProject = 'AvevaMarineSample',
    [string]$E3dInstallDir = 'E:\reverse\e3d\shadow_e3d31_gen_model_test',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [int]$ReadyTimeoutSec = 900
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $repo 'scripts\e3d\LinkageE3dCommon.ps1')

if ($Stop) {
    $stack = Get-Content -LiteralPath $Stop -Raw -Encoding UTF8 | ConvertFrom-Json
    foreach ($procId in @($stack.server_pid, $stack.surreal_pid)) {
        if ($procId -and (Get-Process -Id $procId -ErrorAction SilentlyContinue)) {
            Stop-Process -Id $procId -Force
            "stopped pid $procId"
        }
    }
    return
}

function Quote-Arg([string]$Value) {
    if ($Value -match '\s') { return '"' + $Value + '"' }
    return $Value
}

function Test-PortBusy([int]$PortNumber) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1', $PortNumber)
        return $true
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

function Get-DbnumRow([string]$BaseUrl, [int]$Db) {
    try {
        $body = Invoke-RestMethod -Uri "$BaseUrl/api/v1/dbnums" -TimeoutSec 30
        return @($body.dbnums | Where-Object { $_.dbnum -eq $Db })[0]
    } catch {
        return $null
    }
}

if (-not $RunRoot) { $RunRoot = Join-Path (Split-Path $repo -Parent) '_runs\plantui-linkage' }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$runName = if ($Live) { "live-$stamp" } else { "$Case-$stamp" }
$runDir = Join-Path $RunRoot $runName
New-Item -ItemType Directory -Force -Path $runDir | Out-Null

$serverExe = Join-Path $TargetDir "$BuildProfile\aios-database.exe"
$fixtureExe = Join-Path $TargetDir "$BuildProfile\db_session_fixture.exe"
$surrealExe = Join-Path $repo 'bin\surreal.exe'
$baseConfig = Join-Path $repo 'DbOption.toml'
$required = @($serverExe, $surrealExe, $baseConfig)
if (-not $Live) { $required += $fixtureExe }
foreach ($path in $required) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "缺少前置：$path（二进制先用 CARGO_TARGET_DIR=$TargetDir 构建 aios-database / db_session_fixture，$BuildProfile 档）"
    }
}
foreach ($portNumber in @($Port, $SurrealPort)) {
    if (Test-PortBusy $portNumber) { throw "端口 $portNumber 已被占用：先 -Stop 上一套的 stack.json，或换 -Port / -SurrealPort" }
}

$watch = @($Dbnum)
$baseSesno = 0
$primeRoots = @()
$fixtureArgs = @()
$workFile = $null
$rvmTruthDir = $null

if (-not $Live) {
    $spec = Get-ReplayCase $repo $Case
    $Dbnum = [int]$spec.dbnum
    $watch = if ($spec.watch_dbnums) { @($spec.watch_dbnums | ForEach-Object { [int]$_ }) } else { @($Dbnum) }
    $baseSesno = [int]$spec.init.to
    $primeRoots = @($spec.init.generate_prime | Where-Object { $_ })
    $dbFile = $spec.db_file -replace '/', '\'
    $shadow = Join-Path $RunRoot 'E3D3.1'
    & (Join-Path $repo 'scripts\replay\New-ShadowProjectRoot.ps1') -Source $Source -Target $shadow -Replayed $dbFile | Out-Null
    $workFile = Join-Path $shadow $dbFile
    $replaySource = Get-ReplaySource $spec $repo $runDir
    $fixtureArgs = @($replaySource.Args)
    $rvmTruthDir = $replaySource.TruthDir
    $initOut = & $fixtureExe replay-init @fixtureArgs --to $baseSesno --out $workFile
    if ($LASTEXITCODE -ne 0) { throw "replay-init 失败：$($initOut -join "`n")" }
    "replay-init: $(@($initOut)[-1])"
    $projectRoot = $shadow
} else {
    if (-not $ProjectsDir) { throw '-Live 要给 -ProjectsDir（与原件完全断开的隔离工程副本）' }
    $projectRoot = Assert-IsolatedProjects $ProjectsDir $Source
}

# 从仓根 DbOption.toml 派生：只改六个键（与 tests/increment_replay 的 derive_config 同一套），其余照本机开发配置。
$project = $projectRoot -replace '\\', '/'
$seen = @{}
$derived = foreach ($line in (Get-Content -LiteralPath $baseConfig -Encoding UTF8)) {
    $trimmed = $line.TrimStart()
    $key = ($line -split '=', 2)[0].Trim()
    $replacement = $null
    if (-not $trimmed.StartsWith('#') -and $line.Contains('=')) {
        switch ($key) {
            'project_path' { $replacement = "project_path = `"$project`"" }
            'http_api_addr' { $replacement = "http_api_addr = `"127.0.0.1:$Port`"" }
            'watch_dbnums' { $replacement = "watch_dbnums = [$($watch -join ', ')]" }
            'auto_initialize_new_dbnums' { $replacement = 'auto_initialize_new_dbnums = true' }
            'v_ip' { $replacement = 'v_ip = "127.0.0.1"' }
            'v_port' { $replacement = "v_port = $SurrealPort" }
        }
    }
    if ($null -ne $replacement) {
        $seen[$key] = $true
        $replacement
    } else {
        $line
    }
}
foreach ($key in @('project_path', 'http_api_addr', 'watch_dbnums', 'auto_initialize_new_dbnums', 'v_ip', 'v_port')) {
    if (-not $seen.ContainsKey($key)) { throw "仓根 DbOption.toml 里没有键 $key，派生不了配置" }
}
Write-Utf8NoBom (Join-Path $runDir 'DbOption.toml') (($derived -join "`n") + "`n")

# 一次性 rocksdb surreal：库目录在运行目录里，跑完就是废纸。
$store = Join-Path $runDir 'store'
$surreal = Start-Process -FilePath $surrealExe -PassThru -WindowStyle Hidden `
    -ArgumentList @('start', '--user', 'root', '--pass', 'root', '--bind', "127.0.0.1:$SurrealPort", (Quote-Arg "rocksdb:$store")) `
    -RedirectStandardOutput (Join-Path $runDir 'surreal.log') -RedirectStandardError (Join-Path $runDir 'surreal.err')
$deadline = (Get-Date).AddSeconds(60)
while (-not (Test-PortBusy $SurrealPort)) {
    if ((Get-Date) -gt $deadline) { throw "60 s 内 surreal $SurrealPort 没有起来（$runDir\surreal.err）" }
    Start-Sleep -Milliseconds 500
}

$serverEnv = [ordered]@{
    DB_OPTION_FILE               = (Join-Path $runDir 'DbOption')
    AIOS_STORE_MODE              = 'rocksdb'
    AIOS_DATA_FACE               = 'ingest'
    AIOS_STARTUP_AUTORUN         = '1'
    PLANT_ASSET_ROOT             = (Join-Path $runDir 'assets')
    AIOS_SKIP_STARTUP_ROOM_BUILD = '1'
    RUST_MIN_STACK               = '134217728'
}
New-Item -ItemType Directory -Force -Path $serverEnv.PLANT_ASSET_ROOT | Out-Null
# 单实例锁、logs/ 都落在服务的当前目录：用运行目录当 cwd，不和仓根上别的实例（开发服务、回放 harness）抢锁。
# aios_core 连库时按 cwd 找 resource/surreal 的函数脚本，给它一个指回仓里的 junction（只读）。
$resourceLink = Join-Path $runDir 'resource'
if (-not (Test-Path -LiteralPath $resourceLink)) {
    cmd /c mklink /J "$resourceLink" "$(Join-Path $repo 'resource')" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "mklink /J 失败：$resourceLink" }
}
$savedEnv = @{}
foreach ($name in $serverEnv.Keys) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $serverEnv[$name], 'Process')
}
try {
    $server = Start-Process -FilePath $serverExe -WorkingDirectory $runDir -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $runDir 'stdout.log') -RedirectStandardError (Join-Path $runDir 'stdout.err')
} finally {
    foreach ($name in $serverEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name], 'Process') }
}

$baseUrl = "http://127.0.0.1:$Port"
$deadline = (Get-Date).AddSeconds($ReadyTimeoutSec)
while ($true) {
    if ($server.HasExited) { throw "服务启动即退出（exit $($server.ExitCode)）：看 $runDir\stdout.log / stdout.err" }
    try {
        Invoke-RestMethod -Uri "$baseUrl/api/v1/health" -TimeoutSec 10 | Out-Null
        break
    } catch {
        if ((Get-Date) -gt $deadline) { throw "$ReadyTimeoutSec s 内 /health 没有应答：看 $runDir\stdout.log" }
        Start-Sleep -Seconds 2
    }
}

# 三水位齐平：回放等到 init.to，现场等到任意非零的齐平点。
while ($true) {
    $row = Get-DbnumRow $baseUrl $Dbnum
    $applied = if ($row) { [int]$row.applied_sesno } else { 0 }
    $inSync = $row -and $applied -gt 0 -and $applied -eq [int]$row.model_sesno -and $applied -eq [int]$row.file_latest_sesno
    if ($inSync -and ($Live -or $applied -eq $baseSesno)) {
        $baseSesno = $applied
        break
    }
    if ($server.HasExited) { throw "服务退出了（exit $($server.ExitCode)）：看 $runDir\stdout.log" }
    if ((Get-Date) -gt $deadline) { throw "$ReadyTimeoutSec s 内 $Dbnum 没有齐平：最后一拍 $($row | ConvertTo-Json -Compress)" }
    Start-Sleep -Seconds 2
}
"dbnum $Dbnum in sync at $baseSesno"

# 冷初始化后投影里没有回执：先把用例点名的根叫进投影，前端第一次显示不必再按需生成（与回放 harness 的 generate_prime 同一件事）。
foreach ($root in $primeRoots) {
    $resp = Invoke-RestMethod -Method Post -Uri "$baseUrl/api/v1/model/generate" -ContentType 'application/json' `
        -Body (@{ refno = $root } | ConvertTo-Json -Compress) -TimeoutSec 600
    "generate_prime $root -> $($resp | ConvertTo-Json -Compress -Depth 4)"
}

$stack = [ordered]@{
    mode           = if ($Live) { 'live' } else { 'replay' }
    case           = if ($Live) { 'live' } else { $Case }
    dbnum          = $Dbnum
    base_sesno     = $baseSesno
    base_url       = $baseUrl
    output_project = $OutputProject
    gen_model_repo = $repo
    run_dir        = $runDir
    project_root   = $projectRoot
    server_pid     = $server.Id
    surreal_pid    = $surreal.Id
    started_at     = (Get-Date).ToString('o')
}
if ($Live) {
    $stack.leg_script = Join-Path $repo 'scripts\e3d\Invoke-LinkageE3dLeg.ps1'
    $stack.e3d_install_dir = $E3dInstallDir
    $stack.l3_exe = $L3Exe
    $stack.main_project = $OutputProject
    $stack.fixture_exe = $fixtureExe
    $stack.rvm_verify_exe = Join-Path $TargetDir "$BuildProfile\rvm_verify.exe"
} else {
    $stack.fixture_exe = $fixtureExe
    $stack.fixture_args = $fixtureArgs
    $stack.work_file = $workFile
    $stack.rvm_truth_dir = $rvmTruthDir
}
$stackPath = Join-Path $runDir 'stack.json'
Write-Utf8NoBom $stackPath ($stack | ConvertTo-Json -Depth 5)

$envName = if ($Live) { 'LINKAGE_LIVE_STACK' } else { 'LINKAGE_REPLAY_STACK' }
"plant3d-web: `$env:$envName='$stackPath'; npx @playwright/test test e2e/incremental-linkage-gen-model-v1.spec.ts"
"stop:        scripts/replay/Start-LinkageReplayStack.ps1 -Stop '$stackPath'"
$stackPath
