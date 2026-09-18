<#
.SYNOPSIS
    一键产出 AMS 本地运行包（程序 + Web；不带 workspace\）。

.DESCRIPTION
    全链路（Q6=本机脚本 / Q7=全链路）：
        构建 backend + web(wasm)  →  组包（程序 + Web）  →  组装 CHANGELOG
        →  冒烟门（起服务、health + 首页含 plant_ui_app.js）  →  zip + sha256 清单

    包内布局（只有这一档 runtime）：
        <包根>\
          aios-database.exe            后端（gen-model）
          web\                         plant-ui Web 静态资源（wasm/字体/贴图；不含冗余 meshes）
          bin\surreal.exe              切到 rocksdb 档时用来起库的 SurrealDB 2.1.x
          resource\surreal\*.surql     连接后初始化数据库函数所需的同版脚本
          schema\e3d210|e3d31\         e3d_version 选择的 attlib / *vir.dat（未配置默认 e3d210）
          DbOption.toml                不带本机路径的出厂配置
          .first-run-pending           首跑哨兵，向导「完成」时删除
          Start-AMS.ps1 / .cmd         通过 -Workspace / AMS_WORKSPACE 指向外部运行环境
          Start-Surreal.ps1            用包根 bin\surreal.exe 起一台本地 rocksdb 库
          Rollback.ps1
          CHANGELOG.md                 后端 + 前端两节（Q4=A）
          release.json                 程序文件 sha256 清单 + 资源摘要 + 冒烟结果

    workspace\ 一律不进包（2026-09-15 起，-IncludeWorkspace / -ExcludeMeshes 两个开关已删）。
    那套东西是**打包机现场**的运行目录：config\、search-index\、accel_tree_*.snapshot 与
    assets\ 全都带着本机项目的痕迹（快照按项目名命名、assets 里混着这台机器跑出来的
    meshes / temp / archives），发出去既撑大 zip 又让现场以为那是自己的数据。运行期资源
    由目标环境自己提供：Start-AMS.ps1 -Workspace <路径> 或 AMS_WORKSPACE 指过去，不指就用
    包根（assets 缺失时后端自己在包根下建）。

    版本纪律（Q8）：版本号以两仓 Cargo.toml 为准；每次正常构建另生成唯一 build_id，
    默认 ZIP 名同时携带版本号与 build_id。输出 zip 已存在则拒绝执行（-Force 覆盖）；
    CHANGELOG 找不到当版条目时只警告、不中止。releases 目录不自动清理旧构建，
    让后续二进制 patch 始终有精确的源包可取。

    存储只有两档（2026-09-13 拍板，ADR-074）：**出厂是 mem**（库在后端进程内，无 Surreal
    TCP 端口，真正解压即跑）。要数据活过重启的现场在向导第②步改选 rocksdb——**服务只连、不拉起**
    那台库，包根附 bin\surreal.exe 与 Start-Surreal.ps1 让人先把它起起来。包里
    保留 .first-run-pending 首跑哨兵，自动进入配置向导。

    冒烟门（Q9，三腿）：A 腿出厂配置起服务考「设置模式 + 向导页」；B 腿本机配置全启动考
    health + 首页 + medium；C 腿由脚本用包内 surreal.exe 在 18010 起一台
    rocksdb 库、服务以 store_mode=rocksdb 连它，考 medium=rocksdb + durable=true + 数据目录
    真落盘——这一腿验的是「包内二进制起得来、服务连得上」。端口被占或起不来则跳过/失败并在
    release.json / 产物名旁标「未冒烟」，不静默放行（-StrictSmoke 改为任一腿失败即中止、不出包）。

    资源不进包的部分（见 ADR-068）：SurrealDB 数据目录与源模型不打入 zip。

.PARAMETER SkipBuild
    复用现成程序产物（本次 0.1.19 发布走这条：Q1=A 用已随发布验证的包内那份 exe/web），
    从 -SourcePackage 取 aios-database.exe 与 web\（自动剔除冗余的 web\assets\meshes）。

.PARAMETER ReuseWebFromZip
    正常重建后端，但从一份既有、三腿冒烟全过的 -full zip 复用 web\。
    适合后端单独发版，避免把 plant-ui 工作树中的未提交改动夹进同一版本号。

.PARAMETER BuildId
    本次后端二进制的唯一构建号。正常构建默认生成
    <Cargo版本>+release.<UTC时间戳>.g<提交>[.dirty]，写入 EXE ProductVersion、
    version --json 与 release.json；默认 ZIP 文件名也带这个构建号，禁止同一 Cargo
    版本的后续包覆盖前一份 patch 基线。-SkipBuild 时必须沿用源 EXE 自报的构建号。

.PARAMETER SkipSmoke
    跳过冒烟门，产物标「未冒烟（-SkipSmoke）」。

.PARAMETER Workspace
    打包机上的现场运行目录。**它的内容一个字节都不进包**，只被三处读：B / C 两腿冒烟拿它的
    DbOption.toml 当「本机真配置」、B 腿把 PLANT_ASSET_ROOT 指向它的 assets\、Rollback.ps1
    的默认回退目标。缺它只影响冒烟，不影响产物。

.PARAMETER SurrealExe
    本仓 fork 的 SurrealDB 2.1.x 路径。必须提供；未传时按仓库与默认
    Workspace 路径自动寻找。放进包根 bin\。

.PARAMETER SmokeStoreMode
    B 腿冒烟时后端的存储形态（mem / rocksdb）。省略时两档都用 mem；给 rocksdb 时脚本会先用
    包内 surreal.exe 在 18011 起一台库给服务连（服务自己不拉库）。

.PARAMETER BackendTargetDir
    可选的独立 Cargo target 目录；机器上已有 release exe 正在运行、Windows 锁住默认输出时使用。

.PARAMETER Force
    覆盖已存在的同名输出 zip / 暂存目录。

.EXAMPLE
    # 本次 0.1.19：复用包内 exe/web，自带内存库冒烟
    ./scripts/New-LocalRelease.ps1 -SkipBuild

    # 只重建后端，复用上一版已验证 Web
    ./scripts/New-LocalRelease.ps1 -ReuseWebFromZip releases\ams-local-0.1.21-full.zip -StrictSmoke
#>
[CmdletBinding()]
param(
    [string]$BackendVersion,
    [string]$BuildId,
    [string]$WebVersion,
    [string]$RepoRoot,
    [string]$PlantUiRoot,
    [string]$Workspace = 'D:\work\plant-code\old\test-worklspace\bin',
    [string]$SourcePackage,
    [string]$SurrealExe,
    [string]$BackendTargetDir,
    [string]$OutDir,
    [string]$Name,
    [switch]$SkipBuild,
    [string]$ReuseWebFromZip,
    [switch]$SkipSmoke,
    [ValidateSet('mem', 'rocksdb')][string]$SmokeStoreMode,
    [int]$SmokeTimeoutSeconds = 90,
    [ValidateRange(1, 65535)][int]$SmokeHttpPort = 9099,
    [switch]$StrictSmoke,
    [ValidateSet('Optimal', 'Fastest', 'NoCompression')][string]$CompressionLevel = 'Optimal',
    [switch]$Force
)

function Get-Sha256Compat([string]$LiteralPath) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($LiteralPath)
        try { return ([System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '').ToLowerInvariant() }
        finally { $stream.Dispose() }
    } finally { $sha.Dispose() }
}

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem | Out-Null

function Write-Step([string]$msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Warn2([string]$msg) { Write-Host "[!] $msg" -ForegroundColor Yellow }

function Copy-WebFromReleaseZip([string]$ZipPath, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $ZipPath -PathType Leaf)) {
        throw "复用 Web 的发布包不存在：$ZipPath"
    }
    New-Item -ItemType Directory -Force $Destination | Out-Null
    $resolvedZip = (Resolve-Path -LiteralPath $ZipPath).Path
    $archive = [System.IO.Compression.ZipFile]::OpenRead($resolvedZip)
    $copied = 0
    try {
        foreach ($entry in $archive.Entries) {
            if (-not $entry.FullName.StartsWith('web/', [StringComparison]::OrdinalIgnoreCase)) { continue }
            if (-not $entry.Name) { continue }
            if ($entry.FullName -match '^web/assets/meshes/') { continue }
            $relative = $entry.FullName.Substring(4).Replace('/', [IO.Path]::DirectorySeparatorChar)
            $target = Join-Path $Destination $relative
            New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $target, $true)
            $copied++
        }
    }
    finally {
        $archive.Dispose()
    }
    if ($copied -eq 0) { throw "$ZipPath 中没有可复用的 web/ 文件" }
    return $copied
}

function Get-CargoVersion([string]$cargoToml) {
    if (-not (Test-Path -LiteralPath $cargoToml)) { throw "找不到 Cargo.toml：$cargoToml" }
    $m = [regex]::Match((Get-Content -LiteralPath $cargoToml -Raw), '(?m)^\s*version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw "$cargoToml 里没读到 version" }
    return $m.Groups[1].Value
}

function ConvertTo-FileToken([string]$value) {
    $token = ($value -replace '[^A-Za-z0-9._-]', '_').Trim('_')
    if ($token.Length -gt 64) { $token = $token.Substring(0, 64) }
    if (-not $token) { throw "构建号无法转换成文件名：$value" }
    return $token
}

function Get-AiosBuildInfo([string]$exe) {
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $json = @(& $exe version --json 2>&1)
        $jsonExit = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $savedPreference
    }
    if ($jsonExit -eq 0) {
        try {
            $parsed = ($json -join "`n") | ConvertFrom-Json
            if ($parsed.product -eq 'aios-database' -and $parsed.version -and $parsed.build_id) {
                return $parsed
            }
        }
        catch {}
    }
    $plain = @(& $exe --version 2>&1)
    if ($LASTEXITCODE -ne 0) { throw "无法读取后端版本：$exe" }
    $match = [regex]::Match(($plain -join ' '), '(?i)\baios-database\s+([^\s]+)')
    if (-not $match.Success) { throw "后端 --version 输出无法识别：$($plain -join ' ')" }
    return [pscustomobject]@{
        product = 'aios-database'
        version = $match.Groups[1].Value
        build_id = $match.Groups[1].Value
        git_commit = $null
    }
}

# ── 0. 解析路径与版本 ────────────────────────────────────────────────────────
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $RepoRoot) { $RepoRoot = Split-Path -Parent $scriptDir }
if (-not $PlantUiRoot) { $PlantUiRoot = Join-Path (Split-Path -Parent $RepoRoot) 'plant-ui' }
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'releases' }
if (-not $SurrealExe) {
    $surrealCandidates = @(
        (Join-Path $RepoRoot 'bin\surreal.exe'),
        (Join-Path (Split-Path -Parent $RepoRoot) 'gen-model\bin\surreal.exe'),
        (Join-Path $Workspace 'bin\surreal.exe')
    )
    $SurrealExe = $surrealCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $SurrealExe -or -not (Test-Path -LiteralPath $SurrealExe -PathType Leaf)) {
    throw "找不到本仓 fork 的 surreal.exe；发布包要带它给 rocksdb 档起库（Start-Surreal.ps1）。请用 -SurrealExe 显式指定。"
}
$surrealVersion = (& $SurrealExe version 2>&1) -join ' '
if ($LASTEXITCODE -ne 0 -or
    $surrealVersion -notmatch '(?<!\d)(\d+)\.(\d+)\.(\d+)' -or
    [int]$Matches[1] -ne 2) {
    throw "surreal 版本不对：$surrealVersion（发布包要求本仓 fork 的 2.1.x）"
}
if (-not $BackendVersion) { $BackendVersion = Get-CargoVersion (Join-Path $RepoRoot 'Cargo.toml') }
if (-not $WebVersion) { $WebVersion = Get-CargoVersion (Join-Path $PlantUiRoot 'Cargo.toml') }
if (-not $SourcePackage) { $SourcePackage = Join-Path $OutDir "ams-local-$BackendVersion" }
$backendGitCommit = (& git -C $RepoRoot rev-parse HEAD 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $backendGitCommit) { $backendGitCommit = 'unknown' }
else { $backendGitCommit = $backendGitCommit.Trim() }
$e3dIoRoot = Join-Path (Split-Path -Parent $RepoRoot) 'vendor\e3d-io'
$e3dIoGitCommit = (& git -C $e3dIoRoot rev-parse HEAD 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $e3dIoGitCommit) { $e3dIoGitCommit = 'unknown' }
else { $e3dIoGitCommit = $e3dIoGitCommit.Trim() }
if ($SkipBuild -and $ReuseWebFromZip) {
    throw "-SkipBuild 与 -ReuseWebFromZip 互斥：前者复用 exe+web，后者只复用 web"
}

$webSource = 'built-from-plant-ui'
$webSourceSha256 = $null
if ($ReuseWebFromZip) {
    $ReuseWebFromZip = (Resolve-Path -LiteralPath $ReuseWebFromZip -ErrorAction Stop).Path
    if ($ReuseWebFromZip -notmatch '(?i)\.zip$') {
        throw "-ReuseWebFromZip 必须指向 .zip：$ReuseWebFromZip"
    }
    $priorManifestPath = [regex]::Replace($ReuseWebFromZip, '(?i)\.zip$', '.release.json')
    if (-not (Test-Path -LiteralPath $priorManifestPath -PathType Leaf)) {
        throw "复用 Web 前必须有同名发布清单：$priorManifestPath"
    }
    $priorManifest = Get-Content -LiteralPath $priorManifestPath -Raw | ConvertFrom-Json
    if ($priorManifest.plant_ui_version -ne $WebVersion) {
        throw "复用包 plant-ui=$($priorManifest.plant_ui_version)，当前声明=$WebVersion；拒绝错标版本"
    }
    $priorSmoke = $priorManifest.smoke
    if ($priorSmoke.status -ne 'passed' -or
        $priorSmoke.setup_leg.status -ne 'passed' -or
        $priorSmoke.rocks_leg.status -ne 'passed') {
        throw "复用包三腿冒烟未全部通过，不能作为 Web 来源：$priorManifestPath"
    }
    $webSourceSha256 = Get-Sha256Compat $ReuseWebFromZip
    if ($priorManifest.zip.sha256 -and $priorManifest.zip.sha256 -ne $webSourceSha256) {
        throw "复用包 SHA-256 与发布清单不一致：$ReuseWebFromZip"
    }
    $webSource = "reused-from:$([IO.Path]::GetFileName($ReuseWebFromZip))"
}

$buildEpoch = $null
if ($SkipBuild) {
    $sourceExeForIdentity = Join-Path $SourcePackage 'aios-database.exe'
    if (-not (Test-Path -LiteralPath $sourceExeForIdentity -PathType Leaf)) {
        throw "缺可读取构建号的源 EXE：$sourceExeForIdentity"
    }
    $sourceBuildInfo = Get-AiosBuildInfo $sourceExeForIdentity
    if ($BuildId -and [string]$sourceBuildInfo.build_id -ne [string]$BuildId) {
        throw "-SkipBuild 不能改写构建号：指定 $BuildId，源 EXE 自报 $($sourceBuildInfo.build_id)"
    }
    $BuildId = [string]$sourceBuildInfo.build_id
}
else {
    $buildEpoch = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    if (-not $BuildId) {
        $stamp = [DateTimeOffset]::UtcNow.ToString('yyyyMMddHHmmss')
        $gitCommit = ((& git -C $RepoRoot rev-parse --short=12 HEAD 2>$null) -join '').Trim()
        if ($LASTEXITCODE -ne 0 -or -not $gitCommit) {
            throw "无法读取 git 提交，不能生成可追溯的发布构建号"
        }
        $gitStatus = @(& git -C $RepoRoot status --porcelain --untracked-files=no 2>$null)
        if ($LASTEXITCODE -ne 0) {
            throw "无法读取 git 工作树状态，不能判断发布构建是否 dirty"
        }
        $dirtySuffix = if ($gitStatus.Count -gt 0) { '.dirty' } else { '' }
        $BuildId = "$BackendVersion+release.$stamp.g$gitCommit$dirtySuffix"
    }
}
$BuildId = $BuildId.Trim()
if (-not $BuildId.StartsWith("$BackendVersion+", [StringComparison]::Ordinal)) {
    if ($SkipBuild -and $BuildId -eq $BackendVersion) {
        Write-Warn2 "源 EXE 是旧式版本身份（只有 $BackendVersion，没有独立 build_id）；允许复用，但不满足新版 patch 基线约定"
    }
    else {
        throw "构建号必须以 Cargo 版本加 '+' 开头：期望 $BackendVersion+...，实际 $BuildId"
    }
}

# 只剩这一档：workspace\ 不再进包，runtime-workspace / full 两档随两个开关一起退役。
$packageTier = 'runtime'
# B 腿默认 mem：服务不拉库，mem 是唯一不需要脚本先起一台库就能全启动的档；
# rocksdb 档由 C 腿或显式 -SmokeStoreMode rocksdb 来考。
if (-not $SmokeStoreMode) { $SmokeStoreMode = 'mem' }
if (-not $Name) {
    $Name = "ams-local-$(ConvertTo-FileToken $BuildId)-$packageTier"
}

$stage = Join-Path $OutDir $Name
$zip = Join-Path $OutDir "$Name.zip"

Write-Step "版本：backend $BackendVersion / build $BuildId / plant-ui $WebVersion"
Write-Step "Web 来源：$webSource"
Write-Step "输出：$zip"

# ── 1. 版本纪律：拒绝覆盖（Q8） ──────────────────────────────────────────────
if ((Test-Path -LiteralPath $zip) -and -not $Force) {
    throw "输出已存在：$zip`n同一构建已发布则拒绝执行；确要覆盖加 -Force。releases 旧构建不自动清理，供 patch 使用。"
}
if (Test-Path -LiteralPath $stage) {
    if (-not $Force) { throw "暂存目录已存在：$stage（-Force 清掉重来）" }
    Remove-Item -LiteralPath $stage -Recurse -Force
}
New-Item -ItemType Directory -Force $stage | Out-Null
# $Workspace 只供冒烟与 Rollback 默认值使用；缺席不影响出包，到冒烟那一步再说。
if (-not (Test-Path -LiteralPath $Workspace)) {
    Write-Warn2 "打包机现场目录不存在：$Workspace（不影响产物；B / C 两腿冒烟会因此跳过）"
}

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
    if ($ReuseWebFromZip) {
        Write-Step "复用已验证 Web：$ReuseWebFromZip"
        $copiedWeb = Copy-WebFromReleaseZip $ReuseWebFromZip $webStage
        Write-Step "Web 复用完成：$copiedWeb 个文件"
    }
    else {
        Write-Step "构建 web(wasm)：$PlantUiRoot"
        Push-Location $PlantUiRoot
        try {
            & (Join-Path $PlantUiRoot 'scripts\build-wasm.ps1')
            if ($LASTEXITCODE) { throw "build-wasm 失败（exit $LASTEXITCODE）" }
        }
        finally { Pop-Location }
    }
    Write-Step "构建 backend：$RepoRoot"
    Push-Location $RepoRoot
    $previousTargetDir = $env:CARGO_TARGET_DIR
    $previousBuildId = $env:AIOS_BUILD_ID
    $previousBuildEpoch = $env:AIOS_BUILD_EPOCH
    try {
        if ($BackendTargetDir) { $env:CARGO_TARGET_DIR = $BackendTargetDir }
        $env:AIOS_BUILD_ID = $BuildId
        $env:AIOS_BUILD_EPOCH = [string]$buildEpoch
        cargo build --release --features http_api --bin aios-database
        if ($LASTEXITCODE) { throw "cargo build 失败（exit $LASTEXITCODE）" }
    }
    finally {
        if ($null -eq $previousTargetDir) { Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
        else { $env:CARGO_TARGET_DIR = $previousTargetDir }
        if ($null -eq $previousBuildId) { Remove-Item Env:AIOS_BUILD_ID -ErrorAction SilentlyContinue }
        else { $env:AIOS_BUILD_ID = $previousBuildId }
        if ($null -eq $previousBuildEpoch) { Remove-Item Env:AIOS_BUILD_EPOCH -ErrorAction SilentlyContinue }
        else { $env:AIOS_BUILD_EPOCH = $previousBuildEpoch }
        Pop-Location
    }
    $builtExe = if ($BackendTargetDir) {
        Join-Path $BackendTargetDir 'release\aios-database.exe'
    } else {
        Join-Path $RepoRoot 'target\release\aios-database.exe'
    }
    if (-not $BackendTargetDir -and -not (Test-Path -LiteralPath $builtExe)) {
        $builtExe = 'D:\Rust\target\release\aios-database.exe'
    }
    if (-not (Test-Path -LiteralPath $builtExe)) { throw "构建后找不到 aios-database.exe" }
    Copy-Item -LiteralPath $builtExe -Destination (Join-Path $stage 'aios-database.exe')
    if (-not $ReuseWebFromZip) {
        $pub = Join-Path $PlantUiRoot 'web\public'
        Get-ChildItem -LiteralPath $pub -Recurse -File | ForEach-Object {
            if ($_.FullName -match '\\assets\\meshes\\') { return }
            $rel = $_.FullName.Substring($pub.Length + 1)
            $dest = Join-Path $webStage $rel
            New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null
            Copy-Item -LiteralPath $_.FullName -Destination $dest
        }
    }
}
if (-not (Test-Path -LiteralPath (Join-Path $webStage 'pkg\plant_ui_app_bg.wasm'))) {
    throw "web 组装后缺 pkg\plant_ui_app_bg.wasm，无法继续"
}
$backendBuildInfo = Get-AiosBuildInfo (Join-Path $stage 'aios-database.exe')
if ([string]$backendBuildInfo.version -ne [string]$BackendVersion) {
    throw "后端版本与发布名不一致：期望 $BackendVersion，EXE 自报 $($backendBuildInfo.version)"
}
if ([string]$backendBuildInfo.build_id -ne [string]$BuildId) {
    throw "后端构建号与发布名不一致：期望 $BuildId，EXE 自报 $($backendBuildInfo.build_id)"
}
Write-Step "后端构建身份：$($backendBuildInfo.build_id)"
# 配置向导页的磁盘副本（exe 内嵌版兜底；磁盘版便于现场热替换——与 ops.html 同一套逻辑）
$setupHtml = Join-Path $RepoRoot 'web\setup.html'
if (Test-Path -LiteralPath $setupHtml) { Copy-Item -LiteralPath $setupHtml -Destination (Join-Path $webStage 'setup.html') -Force }
else { Write-Warn2 "缺 $setupHtml —— 包内只有 exe 内嵌的向导页副本" }
# 浏览器包必须以当前页面 origin 为服务地址，不能把打包机的 127.0.0.1:9099/8022
# 烘进发给别人的 ZIP。即使 -SkipBuild 复用旧 Web，也用前端仓当前入口覆盖并把
# config.json 归一成“model legacy sentinel + data 空值”，由 index.html 映射到 origin。
$browserIndex = Join-Path $PlantUiRoot 'web\public\index.html'
if (Test-Path -LiteralPath $browserIndex) {
    Copy-Item -LiteralPath $browserIndex -Destination (Join-Path $webStage 'index.html') -Force
}
else {
    throw "缺浏览器入口：$browserIndex"
}
$browserConfigPath = Join-Path $webStage 'config.json'
if (-not (Test-Path -LiteralPath $browserConfigPath)) {
    $browserConfigTemplate = Join-Path $webStage 'config.example.json'
    if (-not (Test-Path -LiteralPath $browserConfigTemplate)) {
        throw "web 组装后缺 config.json / config.example.json，无法保证浏览器使用同源 API"
    }
    Copy-Item -LiteralPath $browserConfigTemplate -Destination $browserConfigPath
    Write-Step "从 config.example.json 生成发布用 config.json"
}
$browserConfig = Get-Content -LiteralPath $browserConfigPath -Raw | ConvertFrom-Json
$browserConfig | Add-Member -NotePropertyName model_api_url -NotePropertyValue 'http://127.0.0.1:8022' -Force
$browserConfig | Add-Member -NotePropertyName data_api_url -NotePropertyValue '' -Force
[System.IO.File]::WriteAllText(
    $browserConfigPath,
    ($browserConfig | ConvertTo-Json -Depth 8),
    [System.Text.UTF8Encoding]::new($false)
)

# aios_core 的连接入口仍会从进程 CWD 读取 resource/surreal；即使后端另有内置快照，
# 目录缺失也会在内置兜底执行前以 os error 3 退出。所有发布档位都把本仓同版脚本
# 放进包根，不能依赖打包机或外部 workspace 恰好带着一份。
$repoSurrealDir = Join-Path $RepoRoot 'resource\surreal'
if (-not (Test-Path -LiteralPath $repoSurrealDir -PathType Container)) {
    throw "缺后端 SurrealQL 资源目录：$repoSurrealDir"
}
$surqlSources = @(Get-ChildItem -LiteralPath $repoSurrealDir -Filter '*.surql' -File | Sort-Object Name)
if ($surqlSources.Count -eq 0) {
    throw "后端 SurrealQL 资源目录为空：$repoSurrealDir"
}
$stageSurrealDir = Join-Path $stage 'resource\surreal'
New-Item -ItemType Directory -Force $stageSurrealDir | Out-Null
foreach ($source in $surqlSources) {
    $destination = Join-Path $stageSurrealDir $source.Name
    Copy-Item -LiteralPath $source.FullName -Destination $destination -Force
    if ((Get-FileHash -LiteralPath $source.FullName -Algorithm SHA256).Hash -ne
        (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash) {
        throw "SurrealQL 资源复制后校验失败：$($source.Name)"
    }
}
Write-Step "包根 SurrealQL 资源已就绪：$($surqlSources.Count) 个文件"

# 版本由 DbOption.e3d_version 显式选择，未配置时默认 e3d210；二进制证据只写诊断日志。
# 真正解析仍必须拿到对应版本的 attlib.dat / *vir.dat。
# 开发态可回落到 e3d-io 源码目录，部署机没有源码，所以两个 schema pack 都必须进包根。
$e3dSchemaSource = Join-Path (Split-Path -Parent $RepoRoot) 'vendor\e3d-io\schema'
if (-not (Test-Path -LiteralPath $e3dSchemaSource -PathType Container)) {
    throw "缺 e3d-io schema 目录：$e3dSchemaSource"
}
$stageSchema = Join-Path $stage 'schema'
$schemaSources = New-Object System.Collections.ArrayList
foreach ($release in 'e3d210', 'e3d31') {
    $sourceRelease = Join-Path $e3dSchemaSource $release
    $destRelease = Join-Path $stageSchema $release
    $manifestSource = Join-Path $sourceRelease 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifestSource -PathType Leaf)) {
        throw "缺 E3D schema manifest：$manifestSource"
    }
    New-Item -ItemType Directory -Force $destRelease | Out-Null
    Copy-Item -LiteralPath $manifestSource -Destination (Join-Path $destRelease 'manifest.json') -Force
    [void]$schemaSources.Add((Get-Item -LiteralPath $manifestSource))

    foreach ($folder in 'binaries', 'catalog') {
        $sourceFolder = Join-Path $sourceRelease $folder
        if (-not (Test-Path -LiteralPath $sourceFolder -PathType Container)) { continue }
        Get-ChildItem -LiteralPath $sourceFolder -Recurse -File | ForEach-Object {
            $rel = $_.FullName.Substring($sourceRelease.Length + 1)
            $destination = Join-Path $destRelease $rel
            New-Item -ItemType Directory -Force (Split-Path -Parent $destination) | Out-Null
            Copy-Item -LiteralPath $_.FullName -Destination $destination -Force
            if ((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash -ne
                (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash) {
                throw "E3D schema 复制后校验失败：$release\$rel"
            }
            [void]$schemaSources.Add($_)
        }
    }
    # Global Project 的 MDB 在 *sys、DB 声明在 *glb；运行时会按文件头 dbnum
    # 惰性跨库打开，所以 glbvir（以及其它项目元数据库模板）不是可选诊断资源。
    foreach ($required in 'attlib.dat', 'desvir.dat', 'catvir.dat', 'dicvir.dat',
        'sysvir.dat', 'comvir.dat', 'misvir.dat', 'glbvir.dat') {
        if (-not (Test-Path -LiteralPath (Join-Path $destRelease "binaries\$required") -PathType Leaf)) {
            throw "$release 缺部署必需 schema 文件：$required"
        }
    }
}
Write-Step "E3D schema packs 已就绪：$($schemaSources.Count) 个文件"

# ── 3. 出厂配置 + 首跑哨兵 + surreal 二进制（都进包根） ──────────────────────
$factoryConfig = Join-Path $scriptDir 'templates\DbOption.factory.toml'
if (-not (Test-Path -LiteralPath $factoryConfig)) { throw "缺出厂配置模板：$factoryConfig" }
Write-Step "包根：出厂 DbOption.toml + 首跑哨兵 + bin\surreal.exe（workspace\ 不进包）"
# 模板是「设置模式」形态（project_path 空、store_mode=mem、监听 0.0.0.0:9099），
# 解压首跑自动进 /setup.html 配置向导；哨兵由向导「完成」时删除。原样进包，不做外科改写。
Copy-Item -LiteralPath $factoryConfig -Destination (Join-Path $stage 'DbOption.toml')
Set-Content -LiteralPath (Join-Path $stage '.first-run-pending') -Value ("shipped by New-LocalRelease " + (Get-Date -Format 'o')) -Encoding ascii
$surrealBinDir = Join-Path $stage 'bin'
New-Item -ItemType Directory -Force $surrealBinDir | Out-Null
$packagedSurrealExe = Join-Path $surrealBinDir 'surreal.exe'
Copy-Item -LiteralPath $SurrealExe -Destination $packagedSurrealExe -Force
if ((Get-FileHash -LiteralPath $SurrealExe -Algorithm SHA256).Hash -ne
    (Get-FileHash -LiteralPath $packagedSurrealExe -Algorithm SHA256).Hash) {
    throw "surreal.exe 复制后校验失败：$packagedSurrealExe"
}
Write-Step "SurrealDB 二进制已就绪：$packagedSurrealExe / $surrealVersion"

# ── 4. 生成包内脚本与文档 ────────────────────────────────────────────────────
Write-Step "生成 Start-AMS / Rollback / Start-Surreal / README / CHANGELOG"

$startAms = @'
param(
    [string]$Workspace = $env:AMS_WORKSPACE,
    [switch]$NoBrowser,
    [int]$TimeoutSeconds = 600
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$exe = Join-Path $root 'aios-database.exe'
$logRoot = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logRoot | Out-Null
if (-not $Workspace) {
    $bundledWorkspace = Join-Path $root 'workspace'
    $Workspace = if (Test-Path -LiteralPath $bundledWorkspace) { $bundledWorkspace } else { $root }
}
if (-not (Test-Path -LiteralPath $exe)) { throw "Missing backend: $exe" }
if (-not (Test-Path -LiteralPath (Join-Path $root 'web/pkg/plant_ui_app_bg.wasm'))) { throw 'Web package missing' }
$rootConfig = Join-Path $root 'DbOption.toml'
$configPath = if (Test-Path -LiteralPath $rootConfig) { $rootConfig } else { Join-Path $Workspace 'DbOption.toml' }
if (-not (Test-Path -LiteralPath $configPath)) { throw "Missing workspace config: $Workspace" }
$configText = Get-Content -LiteralPath $configPath -Raw
$addr = [regex]::Match($configText, '(?m)^\s*http_api_addr\s*=\s*["'']?([^"''#\r\n]+)').Groups[1].Value.Trim()
if (-not $addr) {
    $addr = [regex]::Match($configText, '(?m)^\s*server_release_ip\s*=\s*["'']?([^"''#\r\n]+)').Groups[1].Value.Trim()
}
if (-not $addr) { $addr = '127.0.0.1:9099' }
$portMatch = [regex]::Match($addr, ':(\d+)$')
if (-not $portMatch.Success) { throw "Invalid http_api_addr/server_release_ip: $addr" }
$httpPort = [int]$portMatch.Groups[1].Value
$localAddr = if ($addr.StartsWith('0.0.0.0:')) { '127.0.0.1:' + $addr.Substring(8) } else { $addr }
$publicAddr = [regex]::Match($configText, '(?m)^\s*http_public_addr\s*=\s*["'']?([^"''#\r\n]+)').Groups[1].Value.Trim()
if (-not $publicAddr) { $publicAddr = $localAddr }
$configRoot = Split-Path -Parent $configPath
$firstRunPending = Test-Path -LiteralPath (Join-Path $configRoot '.first-run-pending')
$probeUrl = "http://$localAddr/"
# 配置向导的 API 只认回环来源；首跑必须打开本机地址。配置完成后才使用对外展示地址。
$browserUrl = if ($firstRunPending) { $probeUrl } else { "http://$publicAddr/" }
# 启动脚本自己等服务就绪后打开页面；关掉后端的包布局自动打开，避免同次启动弹两次。
$env:AIOS_OPEN_BROWSER = '0'
$existing = Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" | Where-Object { $_.ExecutablePath -eq $exe }
if ($existing) {
    $process = Get-Process -Id $existing.ProcessId
} else {
    # 用 BCL 查监听端口，兼容 Windows PowerShell 5.1 与 PowerShell 7；后者在部分机器上
    # 无法加载 Windows-only 的 NetTCPIP 模块，不能让解压即跑因此失效。
    $listener = [System.Net.NetworkInformation.IPGlobalProperties]::GetIPGlobalProperties().GetActiveTcpListeners() |
        Where-Object { $_.Port -eq $httpPort } |
        Select-Object -First 1
    if ($listener) { throw "Port $httpPort is already in use. Stop the prior release first." }
    $env:DB_OPTION_FILE = $configPath
    $env:PLANT_UI_WEB_ROOT = Join-Path $root 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
    $env:AIOS_E3D_SCHEMA_ROOT = Join-Path $root 'schema'
    # 服务不拉库（ADR-074）：rocksdb 档要先自己跑 Start-Surreal.ps1，这里不再递 AIOS_SURREAL_EXE。
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $stdoutLog = Join-Path $logRoot "$stamp.stdout.log"
    $stderrLog = Join-Path $logRoot "$stamp.stderr.log"
    # 不走 -RedirectStandard*：那条路子让服务连调用方的 stdout 句柄一起继承，调用方（脚本 / CI /
    # 代理）只要接了管道，那一头就永远等不到 EOF——服务是长命的。改由 cmd 把两路输出接到同样的
    # 日志文件上，这一侧只剩 ShellExecuteEx，一个句柄都不带过去。双击 .cmd 那条路本来就不受影响。
    $redirected = '/c ""{0}" serve 1>"{1}" 2>"{2}""' -f $exe, $stdoutLog, $stderrLog
    $shell = Start-Process -FilePath $env:ComSpec -ArgumentList $redirected -WorkingDirectory $Workspace -WindowStyle Hidden -PassThru
    # cmd 只是那层重定向，真正要盯的是服务自己：按可执行路径认回来（同上面认既有实例那一招）。
    $spawnDeadline = (Get-Date).AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 200
        $spawned = Get-CimInstance Win32_Process -Filter "Name='aios-database.exe'" |
            Where-Object { $_.ExecutablePath -eq $exe } | Select-Object -First 1
    } while (-not $spawned -and -not $shell.HasExited -and (Get-Date) -lt $spawnDeadline)
    if (-not $spawned) { throw "Backend did not start; see $logRoot" }
    $process = Get-Process -Id $spawned.ProcessId
    @{ pid=$process.Id; start_time=$process.StartTime.ToString('o'); executable=$exe } | ConvertTo-Json | Set-Content (Join-Path $root 'process.json')
}
$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
do {
    $process.Refresh()
    if ($process.HasExited) { throw "Backend exited: $($process.ExitCode); see $logRoot" }
    try {
        $health = Invoke-WebRequest -Uri ($probeUrl + 'api/v1/health') -UseBasicParsing -TimeoutSec 3
        $page = Invoke-WebRequest -Uri $probeUrl -UseBasicParsing -TimeoutSec 3
        # 首跑（哨兵在场）首页 302 到 /setup.html（含 ams-setup-wizard）；配置完成后是 plant-ui。
        if ($health.StatusCode -eq 200 -and ($page.Content.Contains('plant_ui_app.js') -or $page.Content.Contains('ams-setup-wizard'))) {
            Write-Output "READY PID=$($process.Id) URL=$browserUrl (probe $probeUrl)"
            if (-not $NoBrowser) { Start-Process $browserUrl }
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
# 用包内 surreal.exe（本仓 fork 的 2.1.x）起一台本地持久库（rocksdb 后端）。
# 服务**不会**自己拉库：配置向导第②步选「rocksdb」之前，先跑这个脚本把库起起来，
# 服务只连 DbOption.toml 里 v_ip:v_port 指的这台。选「mem」则完全不需要它。
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
# 兼容旧模板的 v_port="0" 未设置哨兵：读到 0 一律回落 8010。
if ($Port -le 0) { $Port = 8010 }
if (-not $Datastore) { $Datastore = 'rocksdb:' + (Join-Path $root '.surreal\ams') }
$ver = (& $exe version) -join ' '
if ($ver -notmatch '(\d+)\.(\d+)\.(\d+)' -or [int]$Matches[1] -ne 2) {
    throw "surreal 版本不对：$ver（本仓客户端是 fork 2.1.x；别用 PATH 上 cargo install 的 3.x）"
}
# fork 默认的响应缓冲定时器在 Windows 上给每次顺序 RPC 加 ~15 ms；服务以前自己拉库时替你设成 0，
# 现在库由这个脚本起，这一行不能丢——否则模型顺序发布与初始化会明显变慢。显式设过的不覆盖。
if ([string]::IsNullOrEmpty($env:SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE)) {
    $env:SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE = '0'
}
Write-Host "SurrealDB $ver  bind 127.0.0.1:$Port  data $Datastore  response-buffer $env:SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE"
& $exe start --user $User --pass $Password --bind "127.0.0.1:$Port" $Datastore
'@
Set-Content -LiteralPath (Join-Path $stage 'Start-Surreal.ps1') -Value $startSurreal -Encoding UTF8

# README
$assetLayoutLine = if ($ExcludeMeshes) {
    '- ``assets\`` —— 运行期资源目录；本包不含 ``assets\meshes``，网格由目标环境生成或另行提供'
}
else {
    '- ``assets\`` —— 模型网格/字体等（``PLANT_ASSET_ROOT`` 指向它）'
}
$meshRuntimeNote = if (-not $IncludeWorkspace) {
    '- **本包不含 workspace 与 meshes。** 三维网格从外部 workspace 读取，或由目标环境连接 E3D 项目后生成。'
}
elseif ($ExcludeMeshes) {
    '- **本包不含 meshes。** 需要三维网格时，由目标环境连接 E3D 项目后生成，或另行放入 ``workspace\assets\meshes``。'
}
else {
    '- 已生成模型的**三维可直接看**：树走文件、几何走 ``assets`` + accel 快照，不必等库。'
}
$startupInstruction = if ($IncludeWorkspace) {
    '解压后双击 **Start-AMS.cmd**（或运行 Start-AMS.ps1）。服务就绪后自动打开 http://127.0.0.1:9099/ 。'
}
else {
    '本包默认不带 workspace，``store_mode=mem``：数据库在后端进程内，无 Surreal 子进程或数据库端口。根目录仍带 ``bin\surreal.exe``：要数据活过重启，先用它起一台 rocksdb 库、再把配置改成 ``store_mode=rocksdb``（服务只连、不拉起）。assets 等资源可通过 ``Start-AMS.ps1 -Workspace <外部 workspace 路径>`` 或 ``AMS_WORKSPACE`` 提供。'
}
$rootSurrealLayoutLine = if ($IncludeWorkspace) {
    ''
}
else {
    '- ``bin\surreal.exe`` —— 本仓 fork 的 SurrealDB 2.1.x；仅在切到 rocksdb 档、需要自己起一台库时使用'
}
$workspaceLayoutLines = if ($IncludeWorkspace) {
@"
- ``workspace\`` —— 运行所需依赖资源，Start-AMS 未传参时自动使用
  - ``DbOption.toml`` / ``config\`` / ``resource\`` / ``search-index\`` / ``accel_tree_*.snapshot``
  $assetLayoutLine
  - ``bin\surreal.exe`` —— 本仓 fork 的 SurrealDB 2.1.x；``Start-Surreal.ps1`` 起一台本地库（rocksdb 档要先起它，服务不拉库）
"@
}
else {
    '- 默认不含 ``workspace\``；索引、快照与 assets 由外部 workspace 提供'
}
$configLayoutLine = if ($IncludeWorkspace) {
    '- ``workspace\DbOption.toml`` —— 附带 workspace 时唯一一份出厂配置'
}
else {
    '- ``DbOption.toml`` —— 默认运行包唯一一份、不含本机路径的出厂配置'
}
$firstRunNote = if ($IncludeWorkspace) {
    '本包附带的 workspace 使用出厂配置（``DbOption.toml`` + ``.first-run-pending``）；第一次启动会自动进入配置向导。'
}
else {
    '根目录附带一份不含本机路径的出厂 ``DbOption.toml`` 与首跑哨兵；第一次启动会进入配置向导。'
}
$storeRuntimeNote = if ($IncludeWorkspace) {
    '- **SurrealDB 数据与源模型不打入包。** 附带配置出厂 ``store_mode=mem``（库在后端进程内，解压即跑，进程一退数据即清空）。要数据活过重启：先跑 ``workspace\Start-Surreal.ps1`` 起一台 rocksdb 库（数据落 ``workspace\.surreal\``），再在向导第②步改选 rocksdb 连它——服务**只连**、不拉起。'
}
else {
    '- **workspace 默认不打入包。** runtime 默认使用进程内 ``mem``；``bin\surreal.exe`` 留给切到 rocksdb 档时自己起库用。索引、快照与 assets 从 ``-Workspace`` / ``AMS_WORKSPACE`` 指向的外部目录读取。'
}
$readme = @"
# AMS 本地发布包（-$packageTier）：gen-model $BackendVersion + plant-ui $WebVersion（Web）

后端构建 ID：``$($backendBuildInfo.build_id)``。排障与生成二进制 patch 时以此 ID + SHA-256 双重定位。

$startupInstruction
重复运行会复用本包已启动的进程；9099 被别的版本占用会明确报错，不打开错版本网页。
``http_api_addr`` 优先决定监听（推荐 ``0.0.0.0:9099``）；旧配置缺少它时兼容
``server_release_ip``，两者都空时默认 ``127.0.0.1:9099``。``http_public_addr`` 只决定
日志、自动打开和分享链接。首跑向导始终通过回环地址打开，配置 API 不对局域网放行。

## 首跑：配置向导
$firstRunNote
进入配置向导后（浏览器首页跳到 ``/setup.html``）：
①选 E3D 项目目录与项目 → ②选存储：mem（库在进程里，零依赖）或 rocksdb（连一台已在跑的 SurrealDB）→ ③定监听端口
→ ④选「只导数据 / 一步到位」并保存重启 → ⑤勾库初始化。向导页只许在**本机**打开
（局域网访问应用没问题，但改配置的接口只收 127.0.0.1 的请求）。
不想用向导：直接改 ``workspace\DbOption.toml`` 后重启，并删掉 ``workspace\.first-run-pending``。
出厂配置显式使用 ``e3d_version="e3d31"``；若项目来自 E3D 2.10，请在初始化前改成
``e3d_version="e3d210"``。服务不会按目录名或数据库会话字段猜版本。

## 包内布局（一体化，解压即跑）
- ``aios-database.exe`` —— 后端服务（gen-model $BackendVersion）
- ``web\`` —— plant-ui $WebVersion 的 Web 静态资源（wasm + 字体 + 贴图）
- ``resource\surreal\`` —— 与后端同版的数据库函数脚本；直接启动时不可缺
- ``schema\e3d210|e3d31\`` —— ``e3d_version`` 选用的 attlib / *vir.dat；未配置时默认 e3d210
$rootSurrealLayoutLine
$configLayoutLine
$workspaceLayoutLines
- ``CHANGELOG.md`` —— 本版更新说明（后端 + 前端两节）
- ``release.json`` —— 程序文件 sha256 清单 + 资源摘要 + 冒烟结果

## 运行依赖（不在包内 —— 见 ADR-068）
$storeRuntimeNote
  $meshRuntimeNote
  - 两种包出厂都是 ``mem``；切到 ``rocksdb`` 后 kv-mem 保存增量 overlay、RocksDB 保存持久基线。服务在任何档位都**不拉起**数据库进程。
- **rocksdb 档连的 SurrealDB 必须是本仓 fork 的 2.1.x**（包内 ``bin\surreal.exe``）。别用 PATH 上 ``cargo install`` 装的 3.x：
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
$url = "http://127.0.0.1:$SmokeHttpPort/"
$smoke = [ordered]@{ status = 'unknown'; reason = ''; url = $url; store_mode = $SmokeStoreMode }

function Set-SmokeHttpPort([string]$ConfigText, [int]$Port) {
    if ($ConfigText -match '(?m)^\s*http_api_addr\s*=') {
        return [regex]::Replace(
            $ConfigText,
            '(?m)^\s*http_api_addr\s*=.*$',
            "http_api_addr = `"127.0.0.1:$Port`""
        )
    }
    return $ConfigText + "`r`nhttp_api_addr = `"127.0.0.1:$Port`"`r`n"
}

# 两档之后服务不再拉库：冒烟要考 rocksdb 档，就得由**脚本**先用包内 surreal.exe 起一台
# （rocksdb 后端、隔离端口、临时数据目录），服务只连它。这同时验了包内二进制起得来。
# 返回起好的进程；起不来直接 throw，调用方按「腿失败」记账。
function Get-ConfigCredential([string]$ConfigText, [string]$Key, [string]$Default) {
    $m = [regex]::Match($ConfigText, "(?m)^\s*$([regex]::Escape($Key))\s*=\s*[`"']([^`"']*)[`"']")
    if ($m.Success -and $m.Groups[1].Value) { return $m.Groups[1].Value }
    return $Default
}

function Start-SmokeSurreal([string]$SurrealBin, [int]$Port, [string]$DataDir, [string]$User = 'root', [string]$Password = 'root') {
    if (-not (Test-Path -LiteralPath $SurrealBin -PathType Leaf)) { throw "冒烟缺 surreal.exe：$SurrealBin" }
    $listener = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) { throw "$Port 被 PID $($listener.OwningProcess) 占用" }
    New-Item -ItemType Directory -Force $DataDir | Out-Null
    # 与包内 Start-Surreal.ps1 同一条纪律：响应缓冲设 0（退役的 spawner 以前替服务设的），显式设过的不覆盖。
    if ([string]::IsNullOrEmpty($env:SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE)) {
        $env:SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE = '0'
    }
    $proc = Start-Process -FilePath $SurrealBin -WindowStyle Hidden -PassThru `
        -ArgumentList @('start', '--user', $User, '--pass', $Password, '--bind', "127.0.0.1:$Port", "rocksdb:$DataDir")
    $deadline = (Get-Date).AddSeconds(30)
    $up = $null
    do {
        Start-Sleep -Milliseconds 500
        $proc.Refresh()
        if ($proc.HasExited) { throw "包内 surreal.exe 起不来（exit $($proc.ExitCode)）" }
        $up = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    } while (-not $up -and (Get-Date) -lt $deadline)
    if (-not $up) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw "包内 surreal.exe 30s 内没在 $Port 上监听"
    }
    return $proc
}

function Stop-SmokeSurreal($proc, [int]$Port) {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    # 保险：按端口再收一遍，别让隔离端口被上一轮的库占着。
    $left = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($left) { Stop-Process -Id $left.OwningProcess -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 500
}

# B 腿考 rocksdb 时给服务连的那台库绑的端口（避开真 8009 / 8010 与 C 腿的 18010）。
$smokeStorePort = 18011

function Invoke-Smoke {
    $exe = Join-Path $stage 'aios-database.exe'
    $listener = Get-NetTCPConnection -LocalPort $SmokeHttpPort -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) {
        return [ordered]@{ status = 'skipped'; reason = "$SmokeHttpPort 被 PID $($listener.OwningProcess) 占用"; url = $smoke.url; store_mode = $SmokeStoreMode }
    }
    # 冒烟工作区：workspace 档复制 workspace\；runtime 只复制根配置与存储启动资源。
    # assets 用环境变量指回源。exe 以临时目录为 cwd，
    # 跑出的日志/锁落在临时目录、不污染待打包的暂存目录。
    $smokeWs = Join-Path $env:TEMP ("ams-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    if ($IncludeWorkspace) {
        Copy-Item -LiteralPath $wsStage -Destination $smokeWs -Recurse
    }
    else {
        New-Item -ItemType Directory -Force $smokeWs | Out-Null
        Copy-Item -LiteralPath (Join-Path $stage 'resource') -Destination (Join-Path $smokeWs 'resource') -Recurse
        Copy-Item -LiteralPath (Join-Path $stage 'bin') -Destination (Join-Path $smokeWs 'bin') -Recurse
    }
    # 全启动腿考的是「真配置 + 包内程序」：包里现在装的是出厂模板（设置模式形态），
    # 这一腿换回本机 workspace 的配置、摘掉首跑哨兵；出厂形态由 Invoke-SetupSmoke 那一腿考。
    $smokeConfigPath = Join-Path $smokeWs 'DbOption.toml'
    $smokeConfig = Get-Content -LiteralPath (Join-Path $Workspace 'DbOption.toml') -Raw
    if ($smokeConfig -match '(?m)^\s*store_mode\s*=') {
        $smokeConfig = [regex]::Replace(
            $smokeConfig,
            '(?m)^\s*store_mode\s*=.*$',
            "store_mode = `"$SmokeStoreMode`""
        )
    }
    else {
        $smokeConfig += "`r`nstore_mode = `"$SmokeStoreMode`"`r`n"
    }
    # rocksdb 档：服务只连不拉，所以脚本先在隔离端口起一台，再把配置指过去。
    $smokeSurrealProc = $null
    $smokeSurrealData = Join-Path $smokeWs '.surreal-smoke'
    if ($SmokeStoreMode -eq 'rocksdb') {
        try {
            # 凭据跟本机配置走：服务拿配置里的 v_user / v_password 登录，库就得用同一对起。
            $smokeSurrealProc = Start-SmokeSurreal (Join-Path $smokeWs 'bin\surreal.exe') $smokeStorePort $smokeSurrealData `
                (Get-ConfigCredential $smokeConfig 'v_user' 'root') (Get-ConfigCredential $smokeConfig 'v_password' 'root')
        }
        catch {
            Remove-Item -LiteralPath $smokeWs -Recurse -Force -ErrorAction SilentlyContinue
            return [ordered]@{ status = 'failed'; reason = "起冒烟用的 rocksdb 库失败：$($_.Exception.Message)"; url = $smoke.url; store_mode = $SmokeStoreMode }
        }
        $smokeConfig = $smokeConfig -replace '(?m)^(\s*v_ip\s*=\s*).*$', '$1"127.0.0.1"'
        $smokeConfig = $smokeConfig -replace '(?m)^(\s*v_port\s*=\s*).*$', "`$1`"$smokeStorePort`""
    }
    $smokeConfig = Set-SmokeHttpPort $smokeConfig $SmokeHttpPort
    [System.IO.File]::WriteAllText($smokeConfigPath, $smokeConfig, [System.Text.UTF8Encoding]::new($false))
    Remove-Item -LiteralPath (Join-Path $smokeWs '.first-run-pending') -Force -ErrorAction SilentlyContinue
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $smokeWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    $env:PLANT_ASSET_ROOT = Join-Path $Workspace 'assets'
    $env:AIOS_E3D_SCHEMA_ROOT = $stageSchema
    $env:AIOS_STORE_MODE = $SmokeStoreMode
    $env:AIOS_STARTUP_AUTORUN = '0'
    $env:RUST_MIN_STACK = '134217728'
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
                $health = $h.Content | ConvertFrom-Json
                # 服务在任何档位都不许拉起 surreal 子进程——两档之后这是硬约束，不再按档放宽。
                $childStore = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -eq 'surreal.exe' } | Select-Object -First 1
                $modeOk = $health.sul_db.medium -eq $SmokeStoreMode
                $durableOk = [bool]$health.sul_db.durable -eq ($SmokeStoreMode -eq 'rocksdb')
                if ($h.StatusCode -eq 200 -and $p.Content.Contains('plant_ui_app.js') -and $modeOk -and $durableOk -and -not $childStore) {
                    $result.status = 'passed'
                    $result.reason = "health 200 + 首页含 plant_ui_app.js + medium=$SmokeStoreMode + durable=$($health.sul_db.durable) + 服务没拉 surreal 子进程"
                    break
                }
            }
            catch {}
            Start-Sleep -Seconds 2
        } while ((Get-Date) -lt $deadline)
    }
    finally {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1 }
        if ($smokeSurrealProc) { Stop-SmokeSurreal $smokeSurrealProc $smokeStorePort }
        Remove-Item -LiteralPath $smokeWs -Recurse -Force -ErrorAction SilentlyContinue
    }
    return $result
}

# 冒烟第二腿（计划 2026-09-09 向导 §7）：出厂配置 + 哨兵起服务，考「设置模式」面——
# /api/v1/health 报 mode=setup、/setup.html 200 且含向导标记。这一腿不连库、不扫描，几秒就完。
function Invoke-SetupSmoke {
    $exe = Join-Path $stage 'aios-database.exe'
    $listener = Get-NetTCPConnection -LocalPort $SmokeHttpPort -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($listener) {
        return [ordered]@{ status = 'skipped'; reason = "$SmokeHttpPort 被 PID $($listener.OwningProcess) 占用" }
    }
    $setupWs = Join-Path $env:TEMP ("ams-setup-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    New-Item -ItemType Directory -Force $setupWs | Out-Null
    $setupConfig = if ($IncludeWorkspace) {
        Join-Path $wsStage 'DbOption.toml'
    }
    else {
        Join-Path $stage 'DbOption.toml'
    }
    $setupConfigText = Set-SmokeHttpPort (Get-Content -LiteralPath $setupConfig -Raw) $SmokeHttpPort
    [System.IO.File]::WriteAllText(
        (Join-Path $setupWs 'DbOption.toml'),
        $setupConfigText,
        [System.Text.UTF8Encoding]::new($false)
    )
    Set-Content -LiteralPath (Join-Path $setupWs '.first-run-pending') -Value 'setup-smoke' -Encoding ascii
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $setupWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    $env:AIOS_E3D_SCHEMA_ROOT = $stageSchema
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
                $h = Invoke-WebRequest -Uri "http://127.0.0.1:$SmokeHttpPort/api/v1/health" -UseBasicParsing -TimeoutSec 3
                $p = Invoke-WebRequest -Uri "http://127.0.0.1:$SmokeHttpPort/setup.html" -UseBasicParsing -TimeoutSec 3
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

# 冒烟第三腿（ADR-074，取代 ADR-070 的「自带持久库」腿）：带 workspace 的包要能走通
# 「包内 surreal.exe 起一台 rocksdb 库 → 服务以 store_mode=rocksdb 连它」这条主路。
# 脚本用暂存包里那只 surreal.exe 在 18010 起库（避开真 8010）、数据目录落在临时 workspace 的
# .surreal\ 下，再让服务连过去：正常模式起来、/health 报 medium=rocksdb + durable=true、
# 数据目录真落盘、且服务自己没拉任何 surreal 子进程。
$rocksSmokeStorePort = 18010
function Invoke-RocksSmoke {
    $exe = Join-Path $stage 'aios-database.exe'
    foreach ($port in $SmokeHttpPort, $rocksSmokeStorePort) {
        $listener = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($listener) { return [ordered]@{ status = 'skipped'; reason = "$port 被 PID $($listener.OwningProcess) 占用" } }
    }
    # 与 B 腿同底：整份包 workspace 拷过去（resource\surreal 的 surql、bin\surreal 都要在），
    # 再换配置——本机真配置改档 rocksdb + 端口指到脚本起的 18010，摘掉首跑哨兵。
    $rocksWs = Join-Path $env:TEMP ("ams-rocks-smoke-" + (Get-Date -Format 'yyyyMMddHHmmss'))
    Copy-Item -LiteralPath $stage -Destination $rocksWs -Recurse
    Remove-Item -LiteralPath (Join-Path $rocksWs '.first-run-pending') -Force -ErrorAction SilentlyContinue
    $rocksData = Join-Path $rocksWs '.surreal\smoke'
    $cfg = Get-Content -LiteralPath (Join-Path $Workspace 'DbOption.toml') -Raw
    $surrealProc = $null
    try {
        $surrealProc = Start-SmokeSurreal (Join-Path $rocksWs 'bin\surreal.exe') $rocksSmokeStorePort $rocksData `
            (Get-ConfigCredential $cfg 'v_user' 'root') (Get-ConfigCredential $cfg 'v_password' 'root')
    }
    catch {
        Remove-Item -LiteralPath $rocksWs -Recurse -Force -ErrorAction SilentlyContinue
        return [ordered]@{ status = 'failed'; reason = "包内 surreal.exe 起库失败：$($_.Exception.Message)" }
    }
    $cfg = $cfg -replace '(?m)^\s*store_mode\s*=.*$', ''
    $cfg = $cfg -replace '(?m)^(\s*v_ip\s*=\s*).*$', '$1"127.0.0.1"'
    $cfg = $cfg -replace '(?m)^(\s*v_port\s*=\s*).*$', "`$1`"$rocksSmokeStorePort`""
    $cfg = $cfg + "`r`nstore_mode = `"rocksdb`"`r`n"
    $cfg = Set-SmokeHttpPort $cfg $SmokeHttpPort
    Set-Content -LiteralPath (Join-Path $rocksWs 'DbOption.toml') -Value $cfg -Encoding utf8
    $logDir = Join-Path $stage 'smoke-logs'
    New-Item -ItemType Directory -Force $logDir | Out-Null
    $env:DB_OPTION_FILE = Join-Path $rocksWs 'DbOption.toml'
    $env:PLANT_UI_WEB_ROOT = Join-Path $stage 'web'
    $env:AIOS_E3D_SCHEMA_ROOT = $stageSchema
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
                $h = Invoke-WebRequest -Uri "http://127.0.0.1:$SmokeHttpPort/api/v1/health" -UseBasicParsing -TimeoutSec 3
                if ($h.StatusCode -eq 200 -and $h.Content.Contains('"medium":"rocksdb"') -and $h.Content.Contains('"durable":true')) {
                    $childStore = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" -ErrorAction SilentlyContinue |
                        Where-Object { $_.Name -eq 'surreal.exe' } | Select-Object -First 1
                    if ($childStore) { $result.reason = '服务自己拉起了 surreal 子进程——两档之后不许'; break }
                    # 目录是脚本自己建的，不算证据；rocksdb 一打开就会落 CURRENT / MANIFEST / LOCK，数文件才算真落盘。
                    $rocksFiles = @(Get-ChildItem -LiteralPath $rocksData -Recurse -File -ErrorAction SilentlyContinue).Count
                    if ($rocksFiles -gt 0) {
                        $result.status = 'passed'; $result.reason = "包内 surreal 起库 + 服务连上 + medium=rocksdb + durable=true + 数据目录落了 $rocksFiles 个文件"; break
                    }
                    $result.reason = 'health 对了但 rocksdb 数据目录里一个文件都没有'; break
                }
            }
            catch {}
            Start-Sleep -Seconds 1
        } while ((Get-Date) -lt $deadline)
    }
    finally {
        if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500 }
        Stop-SmokeSurreal $surrealProc $rocksSmokeStorePort
        Remove-Item -LiteralPath $rocksWs -Recurse -Force -ErrorAction SilentlyContinue
    }
    return $result
}

$setupSmoke = [ordered]@{ status = 'unknown'; reason = '' }
$rocksSmoke = [ordered]@{ status = 'unknown'; reason = '' }
$smokeSkipReason = '-SkipSmoke'
if ($SkipSmoke) {
    $smoke = [ordered]@{ status = 'skipped'; reason = $smokeSkipReason; url = $smoke.url; store_mode = $SmokeStoreMode }
    $setupSmoke = [ordered]@{ status = 'skipped'; reason = $smokeSkipReason }
    $rocksSmoke = [ordered]@{ status = 'skipped'; reason = $smokeSkipReason }
    Write-Warn2 "冒烟门已跳过（$smokeSkipReason）——产物标『未冒烟』"
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
    if ($IncludeWorkspace) {
        Write-Step "冒烟门 C：包内 surreal 起 rocksdb 库 + 服务连它（rocksdb@$rocksSmokeStorePort，超时 ${SmokeTimeoutSeconds}s）"
        $rocksSmoke = Invoke-RocksSmoke
        switch ($rocksSmoke.status) {
            'passed' { Write-Host "  [OK] 持久库腿通过：$($rocksSmoke.reason)" -ForegroundColor Green }
            'skipped' { Write-Warn2 "持久库腿跳过：$($rocksSmoke.reason)" }
            default { Write-Warn2 "持久库腿未过：$($rocksSmoke.reason)（看 $stage\smoke-logs\rocks-smoke.*.log）" }
        }
    }
    else {
        $rocksSmoke = [ordered]@{ status = 'skipped'; reason = 'runtime tier does not bundle a workspace' }
    }
    $legFailed = ($smoke.status -eq 'failed') -or ($setupSmoke.status -eq 'failed') -or
        ($IncludeWorkspace -and $rocksSmoke.status -eq 'failed')
    if ($StrictSmoke -and $legFailed) {
        throw "冒烟未过且 -StrictSmoke，中止、不出包。（A=$($setupSmoke.status) / B=$($smoke.status) / C=$($rocksSmoke.status)）"
    }
    if ($legFailed) {
        Write-Warn2 "非严格模式：仍出包，但标『未冒烟』。"
    }
}
$smoke.setup_leg = $setupSmoke
$smoke.rocks_leg = $rocksSmoke
$smoked = (($smoke.status -eq 'passed') -and ($setupSmoke.status -eq 'passed') -and
    ((-not $IncludeWorkspace) -or ($rocksSmoke.status -eq 'passed')))

# ── 6. 组 zip + sha256 清单（assets 从源直灌，省磁盘） ───────────────────────
$free = (Get-PSDrive (Split-Path $OutDir -Qualifier).TrimEnd(':')).Free
Write-Step ("组 zip（{0} 压缩）；目标盘剩余 {1:N1} GB" -f $CompressionLevel, ($free / 1GB))

$files = New-Object System.Collections.ArrayList
$resources = New-Object System.Collections.ArrayList
[void]$resources.Add([ordered]@{
    path = 'resource/surreal'
    bytes = [long](($surqlSources | Measure-Object Length -Sum).Sum)
    files = $surqlSources.Count
})
[void]$resources.Add([ordered]@{
    path = 'schema'
    bytes = [long](($schemaSources | Measure-Object Length -Sum).Sum)
    files = $schemaSources.Count
})
$cl = [System.IO.Compression.CompressionLevel]::$CompressionLevel
$fs = [System.IO.File]::Open($zip, [System.IO.FileMode]::Create)
$archive = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    # 6a. 暂存目录内的一切（程序 + workspace 小资源 + 脚本 + 文档），逐一算 sha256
    Get-ChildItem -LiteralPath $stage -Recurse -File | Where-Object { $_.FullName -notmatch '\\smoke-logs\\' } | ForEach-Object {
        $rel = $_.FullName.Substring($stage.Length + 1)
        $entry = ($rel -replace '\\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, $cl) | Out-Null
        $h = Get-Sha256Compat $_.FullName
        [void]$files.Add([ordered]@{ path = $entry; sha256 = $h; bytes = $_.Length })
    }
    # 6b. assets 从源直灌到 workspace/assets/*（不进暂存、不逐一 hash，按目录汇总）
    $assetSrc = Join-Path $Workspace 'assets'
    if ($IncludeWorkspace -and (Test-Path -LiteralPath $assetSrc)) {
        $aBytes = [long]0; $aFiles = 0
        $meshSkippedBytes = [long]0; $meshSkippedFiles = 0
        Get-ChildItem -LiteralPath $assetSrc -Recurse -File | ForEach-Object {
            $rel = $_.FullName.Substring($assetSrc.Length + 1)
            if ($ExcludeMeshes -and $rel -match '^(?i:meshes)[\\/]') {
                $meshSkippedBytes += $_.Length
                $meshSkippedFiles++
                return
            }
            $entry = 'workspace/assets/' + ($rel -replace '\\', '/')
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $_.FullName, $entry, $cl) | Out-Null
            $aBytes += $_.Length; $aFiles++
        }
        [void]$resources.Add([ordered]@{
            path = 'workspace/assets'
            bytes = $aBytes
            files = $aFiles
            meshes_excluded = [bool]$ExcludeMeshes
            excluded_mesh_bytes = $meshSkippedBytes
            excluded_mesh_files = $meshSkippedFiles
        })
        Write-Step ("assets 已灌入：{0} 个文件 / {1:N1} MB" -f $aFiles, ($aBytes / 1MB))
        if ($ExcludeMeshes) {
            Write-Step ("meshes 已排除：{0} 个文件 / {1:N1} MB" -f $meshSkippedFiles, ($meshSkippedBytes / 1MB))
        }
    }
    elseif ($IncludeWorkspace) { Write-Warn2 "源 assets 不存在：$assetSrc（包内无 assets）" }

    # 6c. release.json（不含自身 hash；zip 整体 sha 另写到 sibling）
    $backendFile = @($files | Where-Object { $_.path -eq 'aios-database.exe' })
    if ($backendFile.Count -ne 1) { throw "发布清单中 aios-database.exe 应恰好一条，实际 $($backendFile.Count)" }
    $manifest = [ordered]@{
        name             = $Name
        tier             = $packageTier
        workspace_included = [bool]$IncludeWorkspace
        meshes_included  = [bool]$IncludeWorkspace -and -not [bool]$ExcludeMeshes
        backend_version  = $BackendVersion
        backend_build_id = $backendBuildInfo.build_id
        backend_build_epoch = $backendBuildInfo.build_epoch
        backend_git_commit = $backendBuildInfo.git_commit
        backend_git_dirty  = [bool]$backendBuildInfo.git_dirty
        backend_sha256   = $backendFile[0].sha256
        backend_bytes    = $backendFile[0].bytes
        e3d_io_git_commit = $e3dIoGitCommit
        plant_ui_version = $WebVersion
        web_source       = $webSource
        web_source_sha256 = $webSourceSha256
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
$zipHash = Get-Sha256Compat $zip
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
