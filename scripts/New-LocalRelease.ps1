<#
.SYNOPSIS
    一键产出 AMS 本地发布包（-full 档：程序 + 一体化 workspace\ 依赖资源）。

.DESCRIPTION
    全链路（Q6=本机脚本 / Q7=全链路）：
        构建 backend + web(wasm)  →  组包（程序 + workspace\ 资源）  →  组装 CHANGELOG
        →  冒烟门（起服务、health + 首页含 plant_ui_app.js）  →  zip + sha256 清单

    包内布局（Q3=A 一体化，解压即跑）：
        <包根>\
          aios-database.exe            后端（gen-model）
          web\                         plant-ui Web 静态资源（wasm/字体/贴图；不含冗余 meshes）
          Start-AMS.ps1 / .cmd         默认 -Workspace 指向包内 workspace\
          Rollback.ps1
          CHANGELOG.md                 后端 + 前端两节（Q4=A）
          release.json                 程序文件 sha256 清单 + 资源摘要 + 冒烟结果
          workspace\                   运行依赖（Q2=全资源）
            DbOption.toml, config\, resource\, search-index\, accel_tree_*.snapshot
            assets\                    模型网格/字体等（PLANT_ASSET_ROOT 指向它）
            bin\surreal.exe            本仓 fork 的 SurrealDB 2.1.x
            Start-Surreal.ps1          用包内 surreal 起一台本地库（可选）

    版本纪律（Q8）：版本号以两仓 Cargo.toml 为准；输出 zip 已存在则拒绝执行（-Force 覆盖）；
    CHANGELOG 找不到当版条目时只警告、不中止。releases 目录不自动清理旧版。

    出厂形态（2026-09-09 向导计划 Q10/Q15/Q16）：包内 workspace\DbOption.toml 是出厂模板
    （scripts\templates\DbOption.factory.toml：project_path 空、external 且 v_port="0" 哨兵、
    监听 0.0.0.0:9099），另落 .first-run-pending 首跑哨兵——解压首跑自动进 /setup.html 配置向导。

    冒烟门（Q9，三腿）：A 腿出厂配置起服务考「设置模式 + 向导页」；B 腿本机配置全启动考
    health + 首页；C 腿本机配置改 spawned-rocksdb@18010 考「自带持久库」（ADR-070：
    medium/durable + .surreal 目录落盘）。端口被占或起不来则跳过/失败并在 release.json /
    产物名旁标「未冒烟」，不静默放行（-StrictSmoke 改为任一腿失败即中止、不出包）。

    资源不进包的部分（见 ADR-068）：SurrealDB 数据目录与源模型不打入 zip。

.PARAMETER SkipBuild
    复用现成程序产物（本次 0.1.19 发布走这条：Q1=A 用已随发布验证的包内那份 exe/web），
    从 -SourcePackage 取 aios-database.exe 与 web\（自动剔除冗余的 web\assets\meshes）。

.PARAMETER SkipSmoke
    跳过冒烟门，产物标「未冒烟（-SkipSmoke）」。

.PARAMETER SmokeStoreMode
    冒烟时后端的存储形态：spawned-mem（默认，自带内存库、不依赖外部 8010）或 external（连 8010）。

.PARAMETER Force
    覆盖已存在的同名输出 zip / 暂存目录。

.EXAMPLE
    # 本次 0.1.19：复用包内 exe/web，自带内存库冒烟
    ./scripts/New-LocalRelease.ps1 -SkipBuild
#>
[CmdletBinding()]
param(
    [string]$BackendVersion,
    [string]$WebVersion,
    [string]$RepoRoot,
    [string]$PlantUiRoot,
    [string]$Workspace = 'D:\work\plant-code\old\test-worklspace\bin',
    [string]$SourcePackage,
    [string]$SurrealExe,
    [string]$OutDir,
    [string]$Name,
    [switch]$SkipBuild,
    [switch]$SkipSmoke,
    [ValidateSet('spawned-mem', 'external')][string]$SmokeStoreMode = 'spawned-mem',
    [int]$SmokeTimeoutSeconds = 90,
    [switch]$StrictSmoke,
    [ValidateSet('Optimal', 'Fastest', 'NoCompression')][string]$CompressionLevel = 'Optimal',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null

function Write-Step([string]$msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Warn2([string]$msg) { Write-Host "[!] $msg" -ForegroundColor Yellow }

function Get-CargoVersion([string]$cargoToml) {
    if (-not (Test-Path -LiteralPath $cargoToml)) { throw "找不到 Cargo.toml：$cargoToml" }
    $m = [regex]::Match((Get-Content -LiteralPath $cargoToml -Raw), '(?m)^\s*version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw "$cargoToml 里没读到 version" }
    return $m.Groups[1].Value
}

# ── 0. 解析路径与版本 ────────────────────────────────────────────────────────
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent $scriptDir }
if (-not $PlantUiRoot) { $PlantUiRoot = Join-Path (Split-Path -Parent $RepoRoot) 'plant-ui' }
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'releases' }
if (-not $SurrealExe) { $SurrealExe = Join-Path $RepoRoot 'bin\surreal.exe' }
if (-not $BackendVersion) { $BackendVersion = Get-CargoVersion (Join-Path $RepoRoot 'Cargo.toml') }
if (-not $WebVersion) { $WebVersion = Get-CargoVersion (Join-Path $PlantUiRoot 'Cargo.toml') }
if (-not $Name) { $Name = "ams-local-$BackendVersion-full" }
if (-not $SourcePackage) { $SourcePackage = Join-Path $OutDir "ams-local-$BackendVersion" }

$stage = Join-Path $OutDir $Name
$zip = Join-Path $OutDir "$Name.zip"

Write-Step "版本：backend $BackendVersion / plant-ui $WebVersion"
Write-Step "输出：$zip"

# ── 1. 版本纪律：拒绝覆盖（Q8） ──────────────────────────────────────────────
if ((Test-Path -LiteralPath $zip) -and -not $Force) {
    throw "输出已存在：$zip`n同版已发布则拒绝执行；确要覆盖加 -Force。releases 旧版不自动清理，人工删。"
}
if (Test-Path -LiteralPath $stage) {
    if (-not $Force) { throw "暂存目录已存在：$stage（-Force 清掉重来）" }
    Remove-Item -LiteralPath $stage -Recurse -Force
}
New-Item -ItemType Directory -Force $stage | Out-Null
$wsStage = Join-Path $stage 'workspace'
New-Item -ItemType Directory -Force $wsStage | Out-Null

if (-not (Test-Path -LiteralPath $Workspace)) { throw "找不到资源工作区：$Workspace" }

# ── 2. 程序产物：构建 或 复用（Q1=A 复用包内已验证那份） ─────────────────────
$webStage = Join-Path $stage 'web'
if ($SkipBuild) {
    Write-Step "复用现成程序产物：$SourcePackage"
    $srcExe = Join-Path $SourcePackage 'aios-database.exe'
    $srcWeb = Join-Path $SourcePackage 'web'
    if (-not (Test-Path -LiteralPath $srcExe)) { throw "缺 exe：$srcExe" }
    if (-not (Test-Path -LiteralPath $srcWeb)) { throw "缺 web：$srcWeb" }
    Copy-Item -LiteralPath $srcExe -Destination (Join-Path $stage 'aios-database.exe')
    # 复制 web，但剔除冗余的 web\assets\meshes（后端从 PLANT_ASSET_ROOT=workspace\assets 供 mesh）
    $skipped = 0
    Get-ChildItem -LiteralPath $srcWeb -Recurse -File | ForEach-Object {
        if ($_.FullName -match '\\assets\\meshes\\') { $script:skipped++; return }
        $rel = $_.FullName.Substring($srcWeb.Length + 1)
        $dest = Join-Path $webStage $rel
        New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $dest
    }
    if ($skipped -gt 0) { Write-Step "web 剔除冗余 meshes $skipped 个（后端从 workspace\assets 供数）" }
}
else {
    Write-Step "构建 web(wasm)：$PlantUiRoot"
    Push-Location $PlantUiRoot
    try {
        & (Join-Path $PlantUiRoot 'scripts\build-wasm.ps1')
        if ($LASTEXITCODE) { throw "build-wasm 失败（exit $LASTEXITCODE）" }
    }
    finally { Pop-Location }
    Write-Step "构建 backend：$RepoRoot"
    Push-Location $RepoRoot
    try {
        cargo build --release --features http_api --bin aios-database
        if ($LASTEXITCODE) { throw "cargo build 失败（exit $LASTEXITCODE）" }
    }
    finally { Pop-Location }
    $builtExe = Join-Path $RepoRoot 'target\release\aios-database.exe'
    if (-not (Test-Path -LiteralPath $builtExe)) { $builtExe = 'D:\Rust\target\release\aios-database.exe' }
    if (-not (Test-Path -LiteralPath $builtExe)) { throw "构建后找不到 aios-database.exe" }
    Copy-Item -LiteralPath $builtExe -Destination (Join-Path $stage 'aios-database.exe')
    $pub = Join-Path $PlantUiRoot 'web\public'
    Get-ChildItem -LiteralPath $pub -Recurse -File | ForEach-Object {
        if ($_.FullName -match '\\assets\\meshes\\') { return }
        $rel = $_.FullName.Substring($pub.Length + 1)
        $dest = Join-Path $webStage $rel
        New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $dest
    }
}
if (-not (Test-Path -LiteralPath (Join-Path $webStage 'pkg\plant_ui_app_bg.wasm'))) {
    throw "web 组装后缺 pkg\plant_ui_app_bg.wasm，无法继续"
}
# 配置向导页的磁盘副本（exe 内嵌版兜底；磁盘版便于现场热替换——与 ops.html 同一套逻辑）
$setupHtml = Join-Path $RepoRoot 'web\setup.html'
if (Test-Path -LiteralPath $setupHtml) { Copy-Item -LiteralPath $setupHtml -Destination (Join-Path $webStage 'setup.html') -Force }
else { Write-Warn2 "缺 $setupHtml —— 包内只有 exe 内嵌的向导页副本" }

# ── 3. workspace\ 小资源（assets 不进暂存，组 zip 时从源直灌，省磁盘） ─────────
Write-Step "组装 workspace\ 依赖资源（assets 延后直灌）"
# 出厂配置模板 + 首跑哨兵（计划 2026-09-09 向导 Q10/Q15/Q16）：包不再带本机配置快照。
# 模板是「设置模式」形态（project_path 空、external 且 v_port="0" 哨兵、监听 0.0.0.0:9099），
# 解压首跑自动进 /setup.html 配置向导；哨兵由向导「完成」时删除。
$factoryConfig = Join-Path $scriptDir 'templates\DbOption.factory.toml'
if (-not (Test-Path -LiteralPath $factoryConfig)) { throw "缺出厂配置模板：$factoryConfig" }
Copy-Item -LiteralPath $factoryConfig -Destination (Join-Path $wsStage 'DbOption.toml')
Set-Content -LiteralPath (Join-Path $wsStage '.first-run-pending') -Value ("shipped by New-LocalRelease " + (Get-Date -Format 'o')) -Encoding ascii
foreach ($d in 'config', 'resource', 'search-index') {
    $src = Join-Path $Workspace $d
    if (Test-Path -LiteralPath $src) { Copy-Item -LiteralPath $src -Destination (Join-Path $wsStage $d) -Recurse }
    else { Write-Warn2 "workspace 资源缺 $d（跳过）" }
}
$snapCount = 0
Get-ChildItem -LiteralPath $Workspace -Filter 'accel_tree_*.snapshot' -File -ErrorAction SilentlyContinue | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $wsStage $_.Name); $script:snapCount++
}
if ($snapCount -eq 0) { Write-Warn2 "没找到 accel_tree_*.snapshot" }
# 包内 surreal 二进制 + 起库脚本
New-Item -ItemType Directory -Force (Join-Path $wsStage 'bin') | Out-Null
if (Test-Path -LiteralPath $SurrealExe) { Copy-Item -LiteralPath $SurrealExe -Destination (Join-Path $wsStage 'bin\surreal.exe') }
else { Write-Warn2 "没找到 surreal.exe：$SurrealExe（包内不含本地库二进制）" }

# ── 4. 生成包内脚本与文档 ────────────────────────────────────────────────────
Write-Step "生成 Start-AMS / Rollback / Start-Surreal / README / CHANGELOG"

$startAms = @'
param(
    [string]$Workspace = (Join-Path $PSScriptRoot 'workspace'),
    [switch]$NoBrowser,
    [int]$TimeoutSeconds = 600
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$exe = Join-Path $root 'aios-database.exe'
$url = 'http://127.0.0.1:9099/'
$logRoot = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logRoot | Out-Null
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing backend: $exe" }
if (-not (Test-Path -LiteralPath (Join-Path $root 'web/pkg/plant_ui_app_bg.wasm'))) { throw 'Web package missing' }
if (-not (Test-Path -LiteralPath (Join-Path $Workspace 'DbOption.toml'))) { throw "Missing workspace config: $Workspace" }
$existing = Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" | Where-Object { $_.ExecutablePath -eq $exe }
if ($existing) {
    $process = Get-Process -Id $existing.ProcessId
} else {
    $listener = Get-NetTCPConnection -LocalPort 9099 -State Listen -ErrorAction SilentlyContinue
    if ($listener) { throw "Port 9099 already belongs to PID $($listener.OwningProcess). Stop the prior release first." }
    $env:DB_OPTION_FILE = Join-Path $Workspace 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $root 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
    $surreal = Join-Path $Workspace 'bin\surreal.exe'
    if (Test-Path -LiteralPath $surreal) { $env:AIOS_SURREAL_EXE = $surreal }
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $process = Start-Process -FilePath $exe -ArgumentList 'serve' -WorkingDirectory $Workspace -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logRoot "$stamp.stdout.log") -RedirectStandardError (Join-Path $logRoot "$stamp.stderr.log")
    @{ pid=$process.Id; start_time=$process.StartTime.ToString('o'); executable=$exe } | ConvertTo-Json | Set-Content (Join-Path $root 'process.json')
}
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    $process.Refresh()
    if ($process.HasExited) { throw "Backend exited: $($process.ExitCode); see $logRoot" }
    try {
        $health = Invoke-WebRequest -Uri ($url + 'api/v1/health') -UseBasicParsing -TimeoutSec 3
        $page = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 3
        # 首跑（哨兵在场）首页 302 到 /setup.html（含 ams-setup-wizard）；配置完成后是 plant-ui。
        if ($health.StatusCode -eq 200 -and ($page.Content.Contains('plant_ui_app.js') -or $page.Content.Contains('ams-setup-wizard'))) {
            Write-Output "READY PID=$($process.Id) URL=$url"
            if (-not $NoBrowser) { Start-Process $url }
            exit 0
        }
    } catch {}
    Start-Sleep -Seconds 2
} while ((Get-Date) -lt $deadline)
throw "Startup still pending; process left running. Inspect $logRoot"
'@
Set-Content -LiteralPath (Join-Path $stage 'Start-AMS.ps1') -Value $startAms -Encoding UTF8

$startCmd = "@echo off`r`npowershell.exe -NoProfile -ExecutionPolicy Bypass -File `"%~dp0Start-AMS.ps1`"`r`nif errorlevel 1 pause`r`n"
Set-Content -LiteralPath (Join-Path $stage 'Start-AMS.cmd') -Value $startCmd -Encoding ascii

$rollback = @'
param([string]$Workspace = 'D:\work\plant-code\old\test-worklspace\bin')
$ErrorActionPreference = 'Stop'
$newExe = Join-Path $PSScriptRoot 'aios-database.exe'
$oldExe = Join-Path $Workspace 'aios-database.exe'
if (-not (Test-Path -LiteralPath $oldExe)) { throw 'Prior installation missing' }
$running = Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'"
foreach ($item in $running | Where-Object { $_.ExecutablePath -eq $newExe }) {
    Stop-Process -Id $item.ProcessId
}
if (-not ($running | Where-Object { $_.ExecutablePath -eq $oldExe })) {
    Remove-Item Env:DB_OPTION_FILE -ErrorAction SilentlyContinue
    Remove-Item Env:PLANT_UI_WEB_ROOT -ErrorAction SilentlyContinue
    Remove-Item Env:PLANT_ASSET_ROOT -ErrorAction SilentlyContinue
    Start-Process -FilePath $oldExe -ArgumentList 'serve' -WorkingDirectory $Workspace -WindowStyle Hidden
}
Write-Output 'Prior installation restored; files and database were not replaced.'
'@
Set-Content -LiteralPath (Join-Path $stage 'Rollback.ps1') -Value $rollback -Encoding UTF8

$startSurreal = @'
# 用包内 surreal.exe（本仓 fork 的 2.1.x）手动起一台本地库。多数情况不必用它：
# 配置向导第②步「新建本机持久库（spawned-rocksdb）」由服务自己拉起并管生命周期。
# 只有 external 部署（库独立于本服务起停）才需要本脚本。
# 注意：本包不含既有库数据；这台是空库，属性/搜索/补齐/更新仍需连回原始库或先导入数据。
param(
    [int]$Port,
    [string]$Datastore,
    [string]$User = 'root',
    [string]$Password = 'root'
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$exe = Join-Path $root 'bin\surreal.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "包内没有 surreal.exe：$exe" }
if (-not $Port) {
    $dbo = Get-Content -LiteralPath (Join-Path $root 'DbOption.toml') -Raw
    $m = [regex]::Match($dbo, '(?m)^\s*v_port\s*=\s*"?(\d+)"?')
    $Port = if ($m.Success) { [int]$m.Groups[1].Value } else { 8010 }
}
# 出厂模板的 v_port="0" 是「未设置」哨兵，不是端口——读到 0 一律回落 8010。
if ($Port -le 0) { $Port = 8010 }
if (-not $Datastore) { $Datastore = 'rocksdb:' + (Join-Path $root '.surreal\ams') }
$ver = (& $exe version) -join ' '
if ($ver -notmatch '(\d+)\.(\d+)\.(\d+)' -or [int]$Matches[1] -ne 2) {
    throw "surreal 版本不对：$ver（本仓客户端是 fork 2.1.x；别用 PATH 上 cargo install 的 3.x）"
}
Write-Host "SurrealDB $ver  bind 127.0.0.1:$Port  data $Datastore"
& $exe start --user $User --pass $Password --bind "127.0.0.1:$Port" $Datastore
'@
Set-Content -LiteralPath (Join-Path $wsStage 'Start-Surreal.ps1') -Value $startSurreal -Encoding UTF8

# README（-full）
$readme = @"
# AMS 本地发布包（-full）：gen-model $BackendVersion + plant-ui $WebVersion（Web）

解压后双击 **Start-AMS.cmd**（或运行 Start-AMS.ps1）。服务就绪后自动打开 http://127.0.0.1:9099/ 。
重复运行会复用本包已启动的进程；9099 被别的版本占用会明确报错，不打开错版本网页。

## 首跑：配置向导
本包出厂配置是「未配置」形态（``workspace\DbOption.toml`` 出厂模板 + ``.first-run-pending`` 哨兵）。
**第一次启动会自动进入配置向导**（浏览器打开的首页跳到 ``/setup.html``）：
①选 E3D 项目目录与项目 → ②选存储（连已有 SurrealDB，或试用模式）→ ③定监听端口
→ ④选「只导数据 / 一步到位」并保存重启 → ⑤勾库初始化。向导页只许在**本机**打开
（局域网访问应用没问题，但改配置的接口只收 127.0.0.1 的请求）。
不想用向导：直接改 ``workspace\DbOption.toml`` 后重启，并删掉 ``workspace\.first-run-pending``。

## 包内布局（一体化，解压即跑）
- ``aios-database.exe`` —— 后端服务（gen-model $BackendVersion）
- ``web\`` —— plant-ui $WebVersion 的 Web 静态资源（wasm + 字体 + 贴图）
- ``workspace\`` —— 运行所需依赖资源，Start-AMS 默认 ``-Workspace`` 指向这里
  - ``DbOption.toml`` / ``config\`` / ``resource\`` / ``search-index\`` / ``accel_tree_*.snapshot``
  - ``assets\`` —— 模型网格/字体等（``PLANT_ASSET_ROOT`` 指向它）
  - ``bin\surreal.exe`` —— 本仓 fork 的 SurrealDB 2.1.x；``Start-Surreal.ps1`` 起一台本地库
- ``CHANGELOG.md`` —— 本版更新说明（后端 + 前端两节）
- ``release.json`` —— 程序文件 sha256 清单 + 资源摘要 + 冒烟结果

## 运行依赖（不在包内 —— 见 ADR-068）
- **SurrealDB 数据与源模型不打入包。** DbOption 默认 ``store_mode=external``，连 ``127.0.0.1:8010`` 上「已经在跑」的库。
  - 已生成模型的**三维可直接看**：树走文件、几何走 ``assets`` + accel 快照，不必等库。
  - 但**属性 / 搜索 / 补齐 / 更新仍需连回原始库**。换机器且无外部库时：可跑 ``workspace\Start-Surreal.ps1``
    用包内 surreal 起一台空库，或设 ``AIOS_STORE_MODE=spawned-mem`` 让后端自带内存库（都不含既有数据）。
- **SurrealDB 必须是本仓 fork 的 2.1.x**（就是包内 ``bin\surreal.exe``）。别用 PATH 上 ``cargo install`` 装的 3.x：
  它会要求对数据做不可逆迁移、且与 2.1.4 客户端握不上手。

## 回退
``Rollback.ps1`` 停本包服务并拉起 ``-Workspace`` 里原有的 ``aios-database.exe``（默认 ``test-worklspace\bin``）。
原安装文件与配置保留；不回退数据库数据。

启动日志在本包 ``logs\``。
"@
Set-Content -LiteralPath (Join-Path $stage 'README.md') -Value $readme -Encoding UTF8

# CHANGELOG（Q4=A：后端手工提炼稿 + 前端 CHANGELOG 当版段）
$backendNotes = Join-Path $RepoRoot "docs\release-notes\$BackendVersion.md"
if (Test-Path -LiteralPath $backendNotes) { $backendSection = (Get-Content -LiteralPath $backendNotes -Raw).TrimEnd() }
else { Write-Warn2 "缺后端提炼稿 $backendNotes —— CHANGELOG 后端节留占位"; $backendSection = "_（缺 docs\release-notes\$BackendVersion.md，本节未提炼）_" }

$plantChangelog = Join-Path $PlantUiRoot 'CHANGELOG.md'
$plantSection = "_（缺 plant-ui CHANGELOG 的 $WebVersion / 未发布 段，本节未提炼）_"
if (Test-Path -LiteralPath $plantChangelog) {
    $txt = Get-Content -LiteralPath $plantChangelog -Raw
    $pat = '(?ms)^##\s+(?:' + [regex]::Escape($WebVersion) + '|未发布)\b.*?(?=^##\s|\z)'
    $mm = [regex]::Match($txt, $pat)
    if ($mm.Success) {
        $body = $mm.Value
        # 把首个 "## 未发布 / ## <ver>" 标题行去掉（外层已经给了本节标题）
        $body = [regex]::Replace($body, '(?m)\A##\s+.*?(\r?\n)', '')
        $plantSection = $body.Trim()
    }
    else { Write-Warn2 "plant-ui CHANGELOG 没匹配到 $WebVersion / 未发布 段" }
}
else { Write-Warn2 "缺 plant-ui CHANGELOG：$plantChangelog" }

$changelog = @"
# AMS 本地发布包 更新说明

> 面向使用者 / 运维：每条说的是界面或运行上会不一样的地方。实现层细节见各仓 ``docs/``。
> 版本口径：后端 gen-model **$BackendVersion** / 前端 plant-ui **$WebVersion**。

## 后端 gen-model $BackendVersion

$backendSection

## 前端 plant-ui $WebVersion（Web）

$plantSection
"@
Set-Content -LiteralPath (Join-Path $stage 'CHANGELOG.md') -Value $changelog -Encoding UTF8

# ── 5. 冒烟门（Q9） ──────────────────────────────────────────────────────────
$url = 'http://127.0.0.1:9099/'
$smoke = [ordered]@{ status = 'unknown'; reason = ''; url = $url; store_mode = $SmokeStoreMode }
function Invoke-Smoke {
    $exe = Join-Path $stage 'aios-database.exe'
    $listener = Get-NetTCPConnection -LocalPort 9099 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) {
        return [ordered]@{ status = 'skipped'; reason = "9099 被 PID $($listener.OwningProcess) 占用"; url = $smoke.url; store_mode = $SmokeStoreMode }
    }
    # 冒烟工作区：把包内 workspace\（不含 assets）复制到临时盘，assets 用环境变量指回源。
    # exe 以临时目录为 cwd（包内 bin\surreal.exe 就在手边，spawned-mem 找得到），
    # 跑出的日志/锁落在临时目录、不污染待打包的暂存目录。
    $smokeWs = Join-Path $env:TEMP ("ams-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    Copy-Item -LiteralPath $wsStage -Destination $smokeWs -Recurse
    # 全启动腿考的是「真配置 + 包内程序」：包里现在装的是出厂模板（设置模式形态），
    # 这一腿换回本机 workspace 的配置、摘掉首跑哨兵；出厂形态由 Invoke-SetupSmoke 那一腿考。
    Copy-Item -LiteralPath (Join-Path $Workspace 'DbOption.toml') -Destination (Join-Path $smokeWs 'DbOption.toml') -Force
    Remove-Item -LiteralPath (Join-Path $smokeWs '.first-run-pending') -Force -ErrorAction SilentlyContinue
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $smokeWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
    if (Test-Path -LiteralPath $SurrealExe) { $env:AIOS_SURREAL_EXE = $SurrealExe }
    if ($SmokeStoreMode -eq 'spawned-mem') {
        $env:AIOS_STORE_MODE = 'spawned-mem'; $env:AIOS_STARTUP_AUTORUN = '0'; $env:RUST_MIN_STACK = '134217728'
    }
    else { Remove-Item Env:AIOS_STORE_MODE -ErrorAction SilentlyContinue }
    $surrealBefore = @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" | Select-Object -ExpandProperty ProcessId)
    $proc = Start-Process -FilePath $exe -ArgumentList 'serve' -WorkingDirectory $smokeWs -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $logDir 'smoke.stdout.log') -RedirectStandardError (Join-Path $logDir 'smoke.stderr.log')
    $result = [ordered]@{ status = 'failed'; reason = 'readiness timeout'; url = $smoke.url; store_mode = $SmokeStoreMode }
    $deadline = (Get-Date).AddSeconds($SmokeTimeoutSeconds)
    try {
        do {
            $proc.Refresh()
            if ($proc.HasExited) { $result.status = 'failed'; $result.reason = "backend exited $($proc.ExitCode)"; break }
            try {
                $h = Invoke-WebRequest -Uri ($smoke.url + 'api/v1/health') -UseBasicParsing -TimeoutSec 3
                $p = Invoke-WebRequest -Uri $smoke.url -UseBasicParsing -TimeoutSec 3
                if ($h.StatusCode -eq 200 -and $p.Content.Contains('plant_ui_app.js')) {
                    $result.status = 'passed'; $result.reason = 'health 200 + 首页含 plant_ui_app.js'; break
                }
            }
            catch {}
            Start-Sleep -Seconds 2
        } while ((Get-Date) -lt $deadline)
    }
    finally {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1 }
        $surrealAfter = @(Get-CimInstance Win32_Process -Filter "Name='surreal.exe'" | Select-Object -ExpandProperty ProcessId)
        foreach ($spid in ($surrealAfter | Where-Object { $_ -notin $surrealBefore })) {
            Stop-Process -Id $spid -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $smokeWs -Recurse -Force -ErrorAction SilentlyContinue
    }
    return $result
}

# 冒烟第二腿（计划 2026-09-09 向导 §7）：出厂配置 + 哨兵起服务，考「设置模式」面——
# /api/v1/health 报 mode=setup、/setup.html 200 且含向导标记。这一腿不连库、不扫描，几秒就完。
function Invoke-SetupSmoke {
    $exe = Join-Path $stage 'aios-database.exe'
    $listener = Get-NetTCPConnection -LocalPort 9099 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) {
        return [ordered]@{ status = 'skipped'; reason = "9099 被 PID $($listener.OwningProcess) 占用" }
    }
    $setupWs = Join-Path $env:TEMP ("ams-setup-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    New-Item -ItemType Directory -Force $setupWs | Out-Null
    Copy-Item -LiteralPath (Join-Path $wsStage 'DbOption.toml') -Destination (Join-Path $setupWs 'DbOption.toml')
    Set-Content -LiteralPath (Join-Path $setupWs '.first-run-pending') -Value 'setup-smoke' -Encoding ascii
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $setupWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    Remove-Item Env:AIOS_STORE_MODE -ErrorAction SilentlyContinue
    Remove-Item Env:AIOS_STARTUP_AUTORUN -ErrorAction SilentlyContinue
    $proc = Start-Process -FilePath $exe -ArgumentList 'serve' -WorkingDirectory $setupWs -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $logDir 'setup-smoke.stdout.log') -RedirectStandardError (Join-Path $logDir 'setup-smoke.stderr.log')
    $result = [ordered]@{ status = 'failed'; reason = 'readiness timeout' }
    $deadline = (Get-Date).AddSeconds(30)
    try {
        do {
            $proc.Refresh()
            if ($proc.HasExited) { $result.reason = "backend exited $($proc.ExitCode)"; break }
            try {
                $h = Invoke-WebRequest -Uri 'http://127.0.0.1:9099/api/v1/health' -UseBasicParsing -TimeoutSec 3
                $p = Invoke-WebRequest -Uri 'http://127.0.0.1:9099/setup.html' -UseBasicParsing -TimeoutSec 3
                if ($h.StatusCode -eq 200 -and $h.Content.Contains('"mode":"setup"') -and $p.Content.Contains('ams-setup-wizard')) {
                    $result.status = 'passed'; $result.reason = 'health mode=setup + /setup.html 含向导标记'; break
                }
                if ($h.StatusCode -eq 200 -and -not $h.Content.Contains('"mode":"setup"')) {
                    $result.reason = '起来了但不在设置模式（出厂配置该判「配置无效」）'; break
                }
            }
            catch {}
            Start-Sleep -Seconds 1
        } while ((Get-Date) -lt $deadline)
    }
    finally {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500 }
        Remove-Item -LiteralPath $setupWs -Recurse -Force -ErrorAction SilentlyContinue
    }
    return $result
}

# 冒烟第三腿（ADR-070）：本机配置副本改成 spawned-rocksdb（v_port=18010，避开真 8010），
# 考「自带持久库」主路——正常模式起来、/health 报 medium=spawned-rocksdb + durable=true、
# rocksdb 数据目录真的落在 workspace\.surreal\。surreal 用暂存包里那只（顺带验包内二进制）。
function Invoke-RocksSmoke {
    $exe = Join-Path $stage 'aios-database.exe'
    foreach ($port in 9099, 18010) {
        $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($listener) { return [ordered]@{ status = 'skipped'; reason = "$port 被 PID $($listener.OwningProcess) 占用" } }
    }
    # 与 B 腿同底：整份包 workspace 拷过去（resource\surreal 的 surql、bin\surreal 都要在），
    # 再换配置——本机真配置改档 spawned-rocksdb + 端口挪 18010，摘掉首跑哨兵。
    $rocksWs = Join-Path $env:TEMP ("ams-rocks-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    Copy-Item -LiteralPath $wsStage -Destination $rocksWs -Recurse
    Remove-Item -LiteralPath (Join-Path $rocksWs '.first-run-pending') -Force -ErrorAction SilentlyContinue
    $cfg = Get-Content -LiteralPath (Join-Path $Workspace 'DbOption.toml') -Raw
    $cfg = $cfg -replace '(?m)^\s*store_mode\s*=.*$', ''
    $cfg = $cfg -replace '(?m)^(\s*v_port\s*=\s*).*$', '$1"18010"'
    $cfg = $cfg + "`r`nstore_mode = `"spawned-rocksdb`"`r`n"
    Set-Content -LiteralPath (Join-Path $rocksWs 'DbOption.toml') -Value $cfg -Encoding utf8
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $rocksWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    $env:AIOS_SURREAL_EXE = Join-Path $wsStage 'bin\surreal.exe'
    Remove-Item Env:AIOS_STORE_MODE -ErrorAction SilentlyContinue
    Remove-Item Env:AIOS_STARTUP_AUTORUN -ErrorAction SilentlyContinue
    $proc = Start-Process -FilePath $exe -ArgumentList 'serve' -WorkingDirectory $rocksWs -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $logDir 'rocks-smoke.stdout.log') -RedirectStandardError (Join-Path $logDir 'rocks-smoke.stderr.log')
    $result = [ordered]@{ status = 'failed'; reason = 'readiness timeout' }
    $deadline = (Get-Date).AddSeconds($SmokeTimeoutSeconds)
    try {
        do {
            $proc.Refresh()
            if ($proc.HasExited) { $result.reason = "backend exited $($proc.ExitCode)"; break }
            try {
                $h = Invoke-WebRequest -Uri 'http://127.0.0.1:9099/api/v1/health' -UseBasicParsing -TimeoutSec 3
                if ($h.StatusCode -eq 200 -and $h.Content.Contains('"medium":"spawned-rocksdb"') -and $h.Content.Contains('"durable":true')) {
                    if (Test-Path -LiteralPath (Join-Path $rocksWs '.surreal')) {
                        $result.status = 'passed'; $result.reason = 'medium=spawned-rocksdb + durable=true + .surreal 目录真落盘'; break
                    }
                    $result.reason = 'health 对了但 workspace\.surreal 目录没出现'; break
                }
            }
            catch {}
            Start-Sleep -Seconds 1
        } while ((Get-Date) -lt $deadline)
    }
    finally {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500 }
        # 强杀后端不走析构，自带 surreal 会留在 18010 上——按端口收掉。
        $orphan = Get-NetTCPConnection -LocalPort 18010 -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($orphan) { Stop-Process -Id $orphan.OwningProcess -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500 }
        Remove-Item Env:AIOS_SURREAL_EXE -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $rocksWs -Recurse -Force -ErrorAction SilentlyContinue
    }
    return $result
}

$setupSmoke = [ordered]@{ status = 'unknown'; reason = '' }
$rocksSmoke = [ordered]@{ status = 'unknown'; reason = '' }
if ($SkipSmoke) {
    $smoke = [ordered]@{ status = 'skipped'; reason = '-SkipSmoke'; url = $smoke.url; store_mode = $SmokeStoreMode }
    $setupSmoke = [ordered]@{ status = 'skipped'; reason = '-SkipSmoke' }
    $rocksSmoke = [ordered]@{ status = 'skipped'; reason = '-SkipSmoke' }
    Write-Warn2 "冒烟门已跳过（-SkipSmoke）——产物标『未冒烟』"
}
else {
    Write-Step "冒烟门 A：出厂配置起服务 → 设置模式 + 向导页（超时 30s）"
    $setupSmoke = Invoke-SetupSmoke
    switch ($setupSmoke.status) {
        'passed' { Write-Host "  [OK] 设置模式腿通过：$($setupSmoke.reason)" -ForegroundColor Green }
        'skipped' { Write-Warn2 "设置模式腿跳过：$($setupSmoke.reason)" }
        default { Write-Warn2 "设置模式腿未过：$($setupSmoke.reason)（看 $stage\smoke-logs\setup-smoke.*.log）" }
    }
    Write-Step "冒烟门 B：本机配置全启动（store_mode=$SmokeStoreMode，超时 ${SmokeTimeoutSeconds}s）"
    $smoke = Invoke-Smoke
    switch ($smoke.status) {
        'passed' { Write-Host "  [OK] 冒烟通过：$($smoke.reason)" -ForegroundColor Green }
        'skipped' { Write-Warn2 "冒烟跳过：$($smoke.reason) —— 产物标『未冒烟』" }
        default {
            Write-Warn2 "冒烟未过：$($smoke.reason)（看 $stage\smoke-logs）"
        }
    }
    Write-Step "冒烟门 C：自带持久库（spawned-rocksdb@18010，超时 ${SmokeTimeoutSeconds}s）"
    $rocksSmoke = Invoke-RocksSmoke
    switch ($rocksSmoke.status) {
        'passed' { Write-Host "  [OK] 持久库腿通过：$($rocksSmoke.reason)" -ForegroundColor Green }
        'skipped' { Write-Warn2 "持久库腿跳过：$($rocksSmoke.reason)" }
        default { Write-Warn2 "持久库腿未过：$($rocksSmoke.reason)（看 $stage\smoke-logs\rocks-smoke.*.log）" }
    }
    $legFailed = ($smoke.status -eq 'failed') -or ($setupSmoke.status -eq 'failed') -or ($rocksSmoke.status -eq 'failed')
    if ($StrictSmoke -and $legFailed) {
        throw "冒烟未过且 -StrictSmoke，中止、不出包。（A=$($setupSmoke.status) / B=$($smoke.status) / C=$($rocksSmoke.status)）"
    }
    if ($legFailed) {
        Write-Warn2 "非严格模式：仍出包，但标『未冒烟』。"
    }
}
$smoke.setup_leg = $setupSmoke
$smoke.rocks_leg = $rocksSmoke
$smoked = (($smoke.status -eq 'passed') -and ($setupSmoke.status -eq 'passed') -and ($rocksSmoke.status -eq 'passed'))

# ── 6. 组 zip + sha256 清单（assets 从源直灌，省磁盘） ───────────────────────
$free = (Get-PSDrive (Split-Path $OutDir -Qualifier).TrimEnd(':')).Free
Write-Step ("组 zip（{0} 压缩）；目标盘剩余 {1:N1} GB" -f $CompressionLevel, ($free / 1GB))

$files = New-Object System.Collections.ArrayList
$resources = New-Object System.Collections.ArrayList
$cl = [System.IO.Compression.CompressionLevel]::$CompressionLevel
$fs = [System.IO.File]::Open($zip, [System.IO.FileMode]::Create)
$archive = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    # 6a. 暂存目录内的一切（程序 + workspace 小资源 + 脚本 + 文档），逐一算 sha256
    Get-ChildItem -LiteralPath $stage -Recurse -File | Where-Object { $_.FullName -notmatch '\\smoke-logs\\' } | ForEach-Object {
        $rel = $_.FullName.Substring($stage.Length + 1)
        $entry = ($rel -replace '\\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, $cl) | Out-Null
        $h = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower()
        [void]$files.Add([ordered]@{ path = $entry; sha256 = $h; bytes = $_.Length })
    }
    # 6b. assets 从源直灌到 workspace/assets/*（不进暂存、不逐一 hash，按目录汇总）
    $assetSrc = Join-Path $Workspace 'assets'
    if (Test-Path -LiteralPath $assetSrc) {
        $aBytes = [long]0; $aFiles = 0
        Get-ChildItem -LiteralPath $assetSrc -Recurse -File | ForEach-Object {
            $rel = $_.FullName.Substring($assetSrc.Length + 1)
            $entry = 'workspace/assets/' + ($rel -replace '\\', '/')
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, $cl) | Out-Null
            $aBytes += $_.Length; $aFiles++
        }
        [void]$resources.Add([ordered]@{ path = 'workspace/assets'; bytes = $aBytes; files = $aFiles })
        Write-Step ("assets 已灌入：{0} 个文件 / {1:N1} MB" -f $aFiles, ($aBytes / 1MB))
    }
    else { Write-Warn2 "源 assets 不存在：$assetSrc（包内无 assets）" }

    # 6c. release.json（不含自身 hash；zip 整体 sha 另写到 sibling）
    $manifest = [ordered]@{
        name             = $Name
        tier             = 'full'
        backend_version  = $BackendVersion
        plant_ui_version = $WebVersion
        created          = (Get-Date).ToString('o')
        smoke            = $smoke
        files            = $files
        resources        = $resources
    }
    $mjson = $manifest | ConvertTo-Json -Depth 8
    $mentry = $archive.CreateEntry('release.json', $cl)
    $sw = New-Object System.IO.StreamWriter($mentry.Open(), [System.Text.UTF8Encoding]::new($false))
    $sw.Write($mjson); $sw.Flush(); $sw.Dispose()
}
finally {
    $archive.Dispose(); $fs.Dispose()
}

# ── 7. 整包 sha256 + sibling release.json ────────────────────────────────────
$zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
$zipBytes = (Get-Item -LiteralPath $zip).Length
"$Name.zip  sha256=$zipHash  bytes=$zipBytes" | Set-Content -LiteralPath "$zip.sha256" -Encoding ascii
$manifest.zip = [ordered]@{ name = "$Name.zip"; bytes = $zipBytes; sha256 = $zipHash }
$manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutDir "$Name.release.json") -Encoding UTF8

# 暂存目录（不含 assets，不是完整包）清掉，回收磁盘；zip 才是交付物
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Step "完成"
Write-Host ("  产物 : {0} ({1:N1} MB)" -f $zip, ($zipBytes / 1MB))
Write-Host ("  sha256: {0}" -f $zipHash)
Write-Host ("  冒烟 : {0}{1}" -f $smoke.status, $(if ($smoke.reason) { " —— $($smoke.reason)" } else { '' })) -ForegroundColor $(if ($smoked) { 'Green' } else { 'Yellow' })
if (-not $smoked) { Write-Host "  注意: 本产物『未冒烟』(见 release.json.smoke)" -ForegroundColor Yellow }
