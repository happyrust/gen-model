[CmdletBinding()]
param(
    [string]$Exe = 'D:\Rust\target\release\aios-database.exe',
    [string]$Config = 'db_options/DbOption-issue7-e2e',
    [int]$Port = 8022,
    [switch]$Console
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo ("output\manual-increment\{0}" -f (Get-Date -Format yyyyMMdd-HHmmss))
New-Item -ItemType Directory -Force $out | Out-Null

$listener = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($listener) {
    Stop-Process -Id $listener.OwningProcess
    Wait-Process -Id $listener.OwningProcess -Timeout 20 -ErrorAction SilentlyContinue
}

$env:DB_OPTION_FILE = $Config
# 这个脚本只负责起服务，库由人自己先起好（`Start-Surreal8009.ps1`）：明写 rocksdb，
# 显式写出来、不靠默认：介质是这一跑的事（ADR-083），默认档 2026-09-18 起是 rocksdb，但默认会翻，
# 而这个脚本要连的就是手工起的那台库——写死它，翻多少次都不影响。
# 服务在任何档位都不自己拉库（ADR-074）。
$env:AIOS_STORE_MODE = 'rocksdb'
# 数据面同理：这个脚本服务的是摄入部署（基线 + 增量窗口），明写 ingest，免得 data_face 的
# 新默认（read-through，2026-09-05）让它开机不做基线。要试读透形态就把这一行改成 read-through。
$env:AIOS_DATA_FACE = 'ingest'
# GEN_MODEL_DIRECT_INCREMENT 随 kv-mem 暂存窗口退役（ADR-056 P1）：稳态增量只有直写一条路。
$env:RUST_MIN_STACK = '134217728'
$env:RUST_BACKTRACE = '1'

$stdout = Join-Path $out 'aios-database.stdout.log'
$stderr = Join-Path $out 'aios-database.stderr.log'
if ($Console) {
    $process = Start-Process -FilePath $Exe -WorkingDirectory $repo -NoNewWindow -PassThru
}
else {
    $process = Start-Process -FilePath $Exe -WorkingDirectory $repo -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
}

$ready = $false
foreach ($null in 1..120) {
    Start-Sleep -Seconds 1
    if ($process.HasExited) { break }
    try {
        $health = Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/health" -TimeoutSec 2
        if ($health.status -eq 'ok') { $ready = $true; break }
    }
    catch {}
}

if (-not $ready) {
    Get-Content $stdout, $stderr -Tail 80 -ErrorAction SilentlyContinue
    throw "aios-database did not become healthy; logs: $out"
}

[ordered]@{
    pid = $process.Id
    output_dir = $out
    stdout = $stdout
    stderr = $stderr
    health = "http://127.0.0.1:$Port/api/v1/health"
} | ConvertTo-Json | Set-Content (Join-Path $repo 'output\manual-increment\current.json')

Write-Host "aios-database PID: $($process.Id)"
if ($Console) {
    Write-Host '日志正在当前控制台输出；Info 日志同时写入仓库根目录的 *_dblog.txt。'
}
else {
    Write-Host "stdout: $stdout"
    Write-Host "stderr: $stderr"
    Write-Host "follow: Get-Content '$stdout' -Wait"
}
