<#
.SYNOPSIS
    以「端口号 = 站点」的方式解析一个 dbnum：起独立 SurrealDB、备好站点运行目录、跑完基线四步并验收。

.DESCRIPTION
    一个站点 = 一个运行目录 + 一个 SurrealDB 端口。目录里只有该站点自己的 DbOption.toml，
    端口默认取 dbnum 本身（7997 站点 → 127.0.0.1:7997），因此与 8009 上的工作库天然隔离，
    互不覆盖也不会指错——ns 1516 与库名 AvevaMarineSample 在两个实例上完全同名，
    只有端口能把它们区分开（见 docs/runbook-local-stack-and-dbnum-parse.md）。

    gen-model 的各个 bin 用 config::File::with_name("DbOption") 读**当前工作目录**的配置，
    同时 aios_core 的 define_common_functions 会读 CWD 下的 resource/surreal。所以站点目录里
    除了自己的 DbOption.toml，还要有指回仓库的 resource / rs_surreal 联接点。

.PARAMETER Dbnum
    要解析的设计库编号，例如 7997。

.PARAMETER Port
    站点的 SurrealDB 端口，默认与 Dbnum 同号。

.PARAMETER ProjectPath
    可选的项目集合根目录。用于从静态 AMS 副本初始化，同时通过同目录下的目录联接
    复用 Catalogue 等只读参考项目；不传时沿用仓库根配置。

.PARAMETER Storage
    memory = 内存实例，进程一停即清，适合测试；rocksdb = 落盘到 .surreal/site-<dbnum>。

.EXAMPLE
    powershell -File scripts\Invoke-SiteDbnumParse.ps1 -Dbnum 7997
    在 127.0.0.1:7997 起内存实例，把 ams7997 解析进去并验收。

.EXAMPLE
    powershell -File scripts\Invoke-SiteDbnumParse.ps1 -Dbnum 7997 -SkipStart -Storage memory
    复用已经在 7997 上跑着的实例，只重跑解析四步。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][int]$Dbnum,
    [int]$Port = 0,
    [string]$Project = "AvevaMarineSample",
    [string]$ProjectPath,
    [string[]]$DbFiles,
    [ValidateSet("memory", "rocksdb")][string]$Storage = "memory",
    [string]$ReleaseDir = (Join-Path (Split-Path -Parent $PSScriptRoot) "target\release"),
    [switch]$SkipStart,
    [switch]$SkipSysSync
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
if ($Port -le 0) { $Port = $Dbnum }
if (-not $DbFiles -or $DbFiles.Count -eq 0) {
    $DbFiles = @("ams${Dbnum}_0001", "amssys")
}

$SiteDir = Join-Path $RepoRoot ".sites\$Dbnum"
$BaseConfig = Join-Path $RepoRoot "DbOption.toml"
$SurrealExe = Join-Path $RepoRoot "bin\surreal.exe"

foreach ($required in @($BaseConfig, $SurrealExe)) {
    if (-not (Test-Path $required)) { throw "缺少必需文件: $required" }
}

# ---------------------------------------------------------------- 站点运行目录

New-Item -ItemType Directory -Force -Path $SiteDir | Out-Null
foreach ($link in @("resource", "rs_surreal")) {
    $linkPath = Join-Path $SiteDir $link
    if (-not (Test-Path $linkPath)) {
        New-Item -ItemType Junction -Path $linkPath -Target (Join-Path $RepoRoot $link) | Out-Null
    }
}

# 站点配置从仓库根的 DbOption.toml 派生，只强制覆盖会让站点之间互相踩到的项：
# 端口、目标库、以及所有会写几何 / 房间树 / Web 端口的开关。project_path、project_code、
# surreal_ns 等跟着根配置走，免得两边漂移。
$dbFilesToml = "[" + (($DbFiles | ForEach-Object { "`"$_`"" }) -join ", ") + "]"
$overrides = [ordered]@{
    v_port                  = "$Port"
    project_name            = "`"$Project`""
    manual_db_nums          = "[$Dbnum]"
    # 单站点基线也必须激活同一限定域。否则 CATA locator 会把 included_projects
    # 下全部 DESI 文件都建 Ref0 索引；7997 会在已落完 PE 后继续扫描范围外大库。
    watch_dbnums            = "[$Dbnum]"
    included_db_files       = $dbFilesToml
    total_sync              = "false"
    incr_sync               = "false"
    sync_live               = "false"
    replace_dbs             = "false"
    gen_model               = "false"
    gen_mesh                = "false"
    gen_spatial_tree        = "false"
    load_spatial_tree       = "false"
    save_spatial_tree_to_db = "false"
}
if ($ProjectPath) {
    $resolvedProjectPath = (Resolve-Path -LiteralPath $ProjectPath).Path.Replace('\', '/')
    $overrides['project_path'] = "`"$resolvedProjectPath`""
}

$lines = [System.Collections.Generic.List[string]]::new()
foreach ($line in Get-Content -LiteralPath $BaseConfig) {
    # http_api_addr 归主站点（8021）所有，站点解析不起 Web 服务，否则第二个站点绑不上端口。
    if ($line -match '^\s*http_api_addr\s*=') {
        $lines.Add("# $line  # 站点解析不起 Web 服务")
        continue
    }
    $matched = $false
    foreach ($key in $overrides.Keys) {
        if ($line -match "^\s*$key\s*=") {
            $lines.Add("$key = $($overrides[$key])")
            $overrides[$key] = $null
            $matched = $true
            break
        }
    }
    if (-not $matched) { $lines.Add($line) }
}
$lines.Add("")
$lines.Add("# 由 scripts\Invoke-SiteDbnumParse.ps1 追加：根配置里没有的站点覆盖项")
foreach ($key in @($overrides.Keys)) {
    if ($null -ne $overrides[$key]) { $lines.Add("$key = $($overrides[$key])") }
}
Set-Content -LiteralPath (Join-Path $SiteDir "DbOption.toml") -Value $lines -Encoding UTF8

Write-Host "站点目录: $SiteDir  (dbnum=$Dbnum, surreal=127.0.0.1:$Port, storage=$Storage)"

# ---------------------------------------------------------------- SurrealDB

$listening = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue
if ($SkipStart) {
    if (-not $listening) { throw "-SkipStart 要求 $Port 上已有实例在跑，但没监听到" }
    Write-Host "复用已在 $Port 上运行的实例 (pid $($listening[0].OwningProcess))"
} elseif ($listening) {
    throw "端口 $Port 已被 pid $($listening[0].OwningProcess) 占用；确认那是本站点的实例后加 -SkipStart 复用"
} else {
    $target = if ($Storage -eq "memory") { "memory" } else { "rocksdb:$RepoRoot/.surreal/site-$Dbnum" }
    $proc = Start-Process -FilePath $SurrealExe -PassThru -WindowStyle Hidden `
        -ArgumentList 'start', '--user', 'root', '--pass', 'root', '--bind', "127.0.0.1:$Port", $target `
        -RedirectStandardOutput (Join-Path $SiteDir "surreal.out.log") `
        -RedirectStandardError (Join-Path $SiteDir "surreal.err.log")
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 500
        if (Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue) { break }
    }
    if (-not (Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue)) {
        throw "SurrealDB 未能在 $Port 上起来，看 $SiteDir\surreal.err.log"
    }
    Write-Host "SurrealDB 已启动 (pid $($proc.Id))"
}

# ---------------------------------------------------------------- 解析四步

function Invoke-SiteBin {
    param([string]$Exe, [string[]]$BinArgs = @(), [string]$LogName)

    $path = Join-Path $ReleaseDir $Exe
    if (-not (Test-Path $path)) { throw "缺少 $path；先跑 cargo build --release --features console" }

    Push-Location $SiteDir
    try {
        & $path @BinArgs *>&1 | Tee-Object -FilePath (Join-Path $SiteDir $LogName) | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$Exe 退出码 $LASTEXITCODE，看 $SiteDir\$LogName" }
    } finally {
        Pop-Location
    }
}

# 1) SYS 元数据：DESI 解析要靠 MDB/WORL 定位世界根，缺了它每个 dbnum 都解析出 0 个元素。
if (-not $SkipSysSync) {
    Write-Host "[1/4] SYS 元数据同步..."
    Invoke-SiteBin -Exe "sync_sys_only.exe" -LogName "sys_sync.log"
}

# 2) 基线全量解析 + 水位收口（与手动增量更新确认执行走同一个初始化入口）。
Write-Host "[2/4] dbnum $Dbnum 基线解析..."
Invoke-SiteBin -Exe "initialize_ams_dbnums.exe" -BinArgs @("$Dbnum") -LogName "baseline.log"
Get-Content (Join-Path $SiteDir "baseline.log") | Select-String -Pattern '^BASELINE\|' | ForEach-Object { Write-Host "      $_" }

# 3) 预览扫描：file_latest_sesno 是扫描观察值，不做这步验收会报 applied/latest 对不上。
Write-Host "[3/4] 预览扫描登记..."
Invoke-SiteBin -Exe "manual_scan_probe.exe" -BinArgs @($Project) -LogName "scan.log"

# 4) 验收：pe 条数、dbnum_info_table 统计、水位三项对账。
Write-Host "[4/4] 验收..."
& (Join-Path $PSScriptRoot "Test-AmsDbnumIntegrity.ps1") -Dbnums $Dbnum -Endpoint "http://127.0.0.1:$Port/sql" -Database $Project
