<#
    增量更新 K 组（docs/plans/2026-10-27-plant3d-web-incremental-linkage-test-cases.md）三支脚本共用的函数：
    scripts/replay/Start-LinkageReplayStack.ps1、scripts/e3d/Export-ReplayRvmTruth.ps1、scripts/e3d/Invoke-LinkageE3dLeg.ps1。
    点源引入，不单独跑。
#>

function Write-Utf8NoBom([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding $false))
}

# E3D 一登录就改写所有打开的库的文件头：副本不许是原件、不许有 junction / 符号链接、库文件不许是硬链接。返回规范化后的副本根。
function Assert-IsolatedProjects([string]$Root, [string]$Original) {
    if (-not (Test-Path -LiteralPath $Root)) { throw "隔离副本不存在：$Root" }
    $resolved = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\')
    if ($Original -and (Test-Path -LiteralPath $Original)) {
        if ($resolved -ieq (Resolve-Path -LiteralPath $Original).Path.TrimEnd('\')) {
            throw "隔离副本不能是原件本身：$resolved"
        }
    }
    $reparse = Get-ChildItem -LiteralPath $resolved -Recurse -Depth 2 -Attributes ReparsePoint -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($reparse) { throw "隔离副本里有 junction / 符号链接：$($reparse.FullName)——E3D 会顺着它写到原件" }
    $dbFiles = Get-ChildItem -LiteralPath $resolved -Recurse -Depth 3 -File -ErrorAction SilentlyContinue |
        Where-Object { $_.DirectoryName -match '\\[A-Za-z]{3}000$' }
    foreach ($file in $dbFiles) {
        $names = @(fsutil hardlink list $file.FullName 2>$null | Where-Object { $_ })
        if ($names.Count -gt 1) {
            throw "隔离副本里有硬链接：$($file.FullName)（$($names.Count) 个名字）——E3D 改写文件头会改到原件"
        }
    }
    return $resolved
}

function Get-ReplayCase([string]$Repo, [string]$Case) {
    $path = Join-Path $Repo "tests\increment_replay\cases\$Case.json"
    if (-not (Test-Path -LiteralPath $path)) { throw "没有回放用例 $path" }
    return Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json
}

# 用例的终稿来源 → db_session_fixture 的参数对（zip 条目解到 $WorkDir\final_db），顺带给出这条链的 RVM 真值目录。
function Get-ReplaySource($Spec, [string]$Repo, [string]$WorkDir) {
    if ($Spec.fixture_dir) {
        $dir = Join-Path $Repo ($Spec.fixture_dir -replace '/', '\')
        return [pscustomobject]@{ Args = @('--fixture', $dir); TruthDir = (Join-Path $dir 'rvm-truth') }
    }
    if ($Spec.source_zip) {
        $zipPath = Join-Path $Repo ($Spec.source_zip.path -replace '/', '\')
        $finalDb = Join-Path $WorkDir 'final_db'
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
        try {
            $entry = $zip.GetEntry($Spec.source_zip.entry)
            if (-not $entry) { throw "zip 里没有条目 $($Spec.source_zip.entry)：$zipPath" }
            [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $finalDb, $true)
        } finally {
            $zip.Dispose()
        }
        return [pscustomobject]@{ Args = @('--source', $finalDb); TruthDir = (Join-Path (Split-Path $zipPath -Parent) 'rvm-truth') }
    }
    throw "用例 $($Spec.name) 既没有 fixture_dir 也没有 source_zip"
}

# 与 rvm_export_*.mac / adr066_live2_rvm_export_*.mac 同一套窄口径；只读、无 SAVEWORK、不写 QUIT（会话归 driver）。
# 宏里只写 ASCII：它由 E3D 读，不赌它的编码。
# -QueryRef：在 Q CE 后多查一行 Q REF，让调用方能从日志回读元素 refno（按名字导出、refno 不固定的场合，
# 如 increment_suite 的 /AIOS-SUITE-8000）；不给时宏文本与以前逐字相同。
function New-RvmExportMacro([string]$Path, [string]$Element, [string]$RvmFile, [string]$LogFile, [string]$Note, [switch]$QueryRef, [switch]$NoPosQuery) {
    $rvm = $RvmFile -replace '\\', '/'
    $log = $LogFile -replace '\\', '/'
    $refLine = ''
    if ($QueryRef) { $refLine = "Q REF`n" }
    # BRAN 等无 POS 属性的元素上查 Q POS 会让 des.exe 直接段错误（0xC0000005）；调用方确认根元素无 POS 时跳过。
    $posLine = ''
    if (-not $NoPosQuery) { $posLine = "Q POS WRT /*`n" }
    $text = @"
`$(
------------------------------------------------------------------------
$Note
Read-only RVM export written by the plant3d-web linkage scripts (K group).
Narrow scope: repre insu/obst off, tube on, implied tubes in separate
containers. No SAVEWORK; no QUIT/FINISH (the l3_suite driver owns the session).
------------------------------------------------------------------------
`$)

ALPHA LOG "$log" OVER
`$P LINKAGE-RVM-EXPORT-BEGIN
$Element
Q CE
${refLine}Q NAME
Q TYPE
Q OWNE
${posLine}

repre insu off
repre obst off
repre tube on
export implied tube into separate containers
Q REPRE

export system /expdri.so
export file "$rvm"
export filenote '$Note'

export holes on
export autocolour displayexport on
export repr on

export $Element

export finish

`$P LINKAGE-RVM-EXPORT-DONE
ALPHA LOG END
"@
    Write-Utf8NoBom $Path (($text -replace "`r`n", "`n") + "`n")
}

# 经 l3_suite --check-driver 跑一条宏（入口宏通道，见 docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md §2 / §11）。
# 退出码为 0 且宏日志里有 $DoneMarker 才算跑完；不把「进程在 / 日志在」当成通过。
function Invoke-TtyMacro {
    param(
        [string]$Repo,
        [string]$L3Exe,
        [string]$Macro,
        [string]$ProjectsDir,
        [string]$MainProject,
        [string]$InstallDir,
        [string]$ProjectEvar = '',
        [string]$Evidence,
        [string]$LogFile,
        [string]$DoneMarker,
        [string]$E3dProject = 'AMS',
        [string]$E3dMdb = '/ALL',
        # 追加给 l3_suite --check-driver 的参数（如 --target-db-file <库> 让驱动器核对会话号前进）；默认不加，行为不变
        [string[]]$ExtraArgs = @()
    )
    New-Item -ItemType Directory -Force -Path $Evidence | Out-Null
    $savedInstall = $env:L3_E3D_INSTALL_DIR
    $savedProjects = $env:L3_E3D_PROJECTS_DIR
    $savedProjectEvar = $env:L3_E3D_PROJECT_EVAR
    $env:L3_E3D_INSTALL_DIR = $InstallDir
    $env:L3_E3D_PROJECTS_DIR = $ProjectsDir
    if ($ProjectEvar) { $env:L3_E3D_PROJECT_EVAR = $ProjectEvar }
    # l3_suite 以当前目录为仓根找 scripts/e3d/run_ams_c_entrymacro.bat（L3_E3D_DRIVER 的缺省值）
    Push-Location -LiteralPath $Repo
    try {
        & $L3Exe --check-driver $Macro --project-dir (Join-Path $ProjectsDir $MainProject) `
            --e3d-project $E3dProject --e3d-mdb $E3dMdb --output $Evidence @ExtraArgs | Out-Host
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
        $env:L3_E3D_INSTALL_DIR = $savedInstall
        $env:L3_E3D_PROJECTS_DIR = $savedProjects
        if ($null -eq $savedProjectEvar) { Remove-Item Env:L3_E3D_PROJECT_EVAR -ErrorAction SilentlyContinue }
        else { $env:L3_E3D_PROJECT_EVAR = $savedProjectEvar }
    }
    if ($code -ne 0) { throw "TTY 宏失败（exit $code）：$Macro，证据 $Evidence" }
    if (-not (Test-Path -LiteralPath $LogFile) -or -not (Select-String -LiteralPath $LogFile -SimpleMatch $DoneMarker -Quiet)) {
        throw "宏日志里没有 $DoneMarker：$LogFile（宏中途停了，看 $Evidence）"
    }
}

# rvm_verify import → 快照 JSON（带 scope / dbnum / root refno）。有未解析身份的成员只告警：e2e 会红在真值上，不在这里放宽。
function Import-RvmSnapshot {
    param(
        [string]$Repo,
        [string]$RvmVerifyExe,
        [string]$RvmFile,
        [int]$Dbnum,
        [string]$RootRefno,
        [string]$OutFile,
        # 可选 ATT：给了就带上 --att，把成员身份从 stable_hash 提升到 att_direct。
        # 不给时命令行与以前逐字相同（向后兼容）。
        [string]$AttFile
    )
    $importArgs = @('import', '--rvm', $RvmFile, '--dbnum', "$Dbnum", '--root-refno', $RootRefno, '--scope', 'narrow', '--out', $OutFile)
    if ($AttFile) { $importArgs += @('--att', $AttFile) }
    if ($RvmVerifyExe -and (Test-Path -LiteralPath $RvmVerifyExe)) {
        & $RvmVerifyExe @importArgs | Out-Host
    } else {
        Push-Location $Repo
        try {
            & cargo run --quiet --features rvm_verify --bin rvm_verify -- @importArgs | Out-Host
        } finally {
            Pop-Location
        }
    }
    if ($LASTEXITCODE -ne 0) { throw "rvm_verify import 失败：$RvmFile" }
    $snapshot = Get-Content -LiteralPath $OutFile -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$snapshot.meta.unresolved -gt 0) {
        Write-Warning "$OutFile 有 $($snapshot.meta.unresolved) 个成员身份没解析出来：用 GenModelRvmExport 的 pair 模式补 ATT（--att）再 import"
    }
    return $snapshot
}
