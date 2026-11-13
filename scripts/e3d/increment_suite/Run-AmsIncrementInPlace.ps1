[CmdletBinding()]
param(
    [ValidateSet('preflight', 'capture', 'apply', 'restore', 'verify', 'run')]
    [string]$Mode = 'preflight',
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$DbFile = '',
    [int]$Dbnum = 8000,
    [int]$CompanionDbnum = 7998,
    [string]$MainProject = 'AvevaMarineSample',
    [string]$E3dProject = 'AMS',
    [string]$E3dMdb = '/ALL',
    [string]$Case = 'equi-move',
    [string]$TargetName = '',
    [string]$ApplyMacro = '',
    [string]$RestoreMacro = '',
    [string]$SetupMacro = '',
    [string]$TeardownMacro = '',
    [string]$L3Exe = '',
    [string]$E3dInstallDir = '',
    [string]$Output = '',
    [string]$RunId = '',
    [string]$RvmElement = '',
    [string]$RvmVerifyExe = '',
    [switch]$EnableRvm,
    [switch]$SkipMoveComparison,
    [switch]$RootBaseline,
    # RVM 导出宏不查 Q POS WRT：BRAN 等无 POS 属性的根会让 des.exe 段错误。
    [switch]$RvmNoPosQuery,
    [switch]$AllowExistingE3dSession
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$suiteDir = $PSScriptRoot
. (Join-Path $repo 'scripts\e3d\LinkageE3dCommon.ps1')

if (-not $RunId) { $RunId = (Get-Date -Format 'yyyyMMdd-HHmmss') }
if ($RunId -notmatch '^[A-Za-z0-9][A-Za-z0-9_-]{2,63}$') { throw "RunId 格式非法：$RunId" }

# l3_suite check-driver 是 detached：l3 先退、des.exe 还在跑（重会话 ~40s+）。若不等它退出，
# 下个 TTY 会话会和它并发，读快照能看到后一步已提交的数据（P82 rvm-before 被 apply 的 FTUB-ADD 污染）。
$script:BaselineDesCount = @(Get-CimInstance Win32_Process -Filter "Name='des.exe'" -ErrorAction SilentlyContinue).Count

function Wait-TtySettled {
    $waited = 0
    while (@(Get-CimInstance Win32_Process -Filter "Name='des.exe'" -ErrorAction SilentlyContinue).Count -gt $script:BaselineDesCount -and $waited -lt 300) {
        Start-Sleep -Seconds 2; $waited += 2
    }
    if ($waited -ge 300) { Write-Warning 'des.exe 等待超时（300s）：上个 TTY 步骤可能还没完全退出' }
}

function Require-Path([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "$Label 不存在：$Path" }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Invoke-StartupPreflight {
    $checker = Join-Path $repo 'scripts\e3d\Test-E3dAmsStartup.ps1'
    $install = if ($E3dInstallDir) { $E3dInstallDir } else { 'D:\work\plant-code\old\e3d31-runtime' }
    $startupEvidence = Join-Path $Output 'e3d-startup.json'
    & pwsh -NoProfile -File $checker -ProjectDir $ProjectDir -ProjectName $MainProject -InstallDir $install -Dbnums "$Dbnum,$CompanionDbnum" -Output $startupEvidence | Out-Host
    if ($LASTEXITCODE -eq 2) { throw "E3D 启动项目检查未通过；证据：$startupEvidence" }
}

function Resolve-L3 {
    if ($L3Exe) { return Require-Path $L3Exe 'l3_suite' }
    $candidates = @(
        (Join-Path (Split-Path $repo -Parent) 'target\debug\l3_suite.exe'),
        'D:\Rust\target\debug\l3_suite.exe',
        (Join-Path $repo 'target\debug\l3_suite.exe')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) { return (Resolve-Path $candidate).Path }
    }
    throw '找不到 l3_suite.exe；请用 -L3Exe 指定。'
}

function Get-CaseDefaults {
    switch ($Case) {
        'equi-move' { return [pscustomobject]@{ Target = '/1-LNR-Q005-PJ'; ApplyDelta = 'BY E 500 WRT /*'; RestoreKind = 'absolute-pos' } }
        'pipe-whole' { return [pscustomobject]@{ Target = '/1RX-345'; ApplyDelta = 'BY E 500 WRT /*'; RestoreKind = 'relative-west' } }
        default { return [pscustomobject]@{ Target = ''; ApplyDelta = ''; RestoreKind = 'custom' } }
    }
}

function Get-ManifestCase([string]$Id) {
    $manifestPath = Join-Path $suiteDir 'suite-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath)) { return $null }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    return @($manifest.cases | Where-Object { $_.id -eq $Id }) | Select-Object -First 1
}

function Test-OneSavework([string]$Path) {
    $count = @(Select-String -LiteralPath $Path -Pattern '(?im)^\s*SAVEWORK\b').Count
    if ($count -ne 1) { throw "宏必须恰好包含一个 SAVEWORK：$Path（实际 $count）" }
}

function New-RunMacro([string]$Template, [string]$Path, [hashtable]$Values) {
    $text = Get-Content -LiteralPath $Template -Raw -Encoding UTF8
    foreach ($key in $Values.Keys) {
        $text = $text.Replace(('{{' + $key + '}}'), [string]$Values[$key])
    }
    [IO.File]::WriteAllText($Path, $text, [Text.UTF8Encoding]::new($false))
    return $Path
}

function New-ApplyMacro([string]$Path, [string]$Target, [string]$Delta, [string]$LogPath) {
    $text = @"
ALPHA LOG "$($LogPath -replace '\\','/')" OVER
`$P AMS-INPLACE-APPLY-ALIVE
/1RX03-EQUI
OWNER
$Target
Q REF
Q NAME
Q TYPE
$Delta
Q POS WRT /*
SAVEWORK 'AMS in-place incremental apply'
`$P AMS-INPLACE-APPLY-DONE
ALPHA LOG END
"@
    [IO.File]::WriteAllText($Path, $text, [Text.UTF8Encoding]::new($false))
}

function New-RelativeRestoreMacro([string]$Path, [string]$Target, [string]$Delta, [string]$LogPath) {
    $text = @"
ALPHA LOG "$($LogPath -replace '\\','/')" OVER
`$P AMS-INPLACE-RESTORE-ALIVE
/1RX03-EQUI
OWNER
$Target
$Delta
Q POS WRT /*
SAVEWORK 'AMS in-place incremental restore'
`$P AMS-INPLACE-RESTORE-DONE
ALPHA LOG END
"@
    [IO.File]::WriteAllText($Path, $text, [Text.UTF8Encoding]::new($false))
}

function Invoke-Tty([string]$Macro, [string]$Evidence, [string]$DbPath, [switch]$ReadOnly) {
    $l3 = Resolve-L3
    $projectsRoot = Split-Path $ProjectDir -Parent
    $saved = @{
        L3_E3D_INSTALL_DIR = $env:L3_E3D_INSTALL_DIR
        L3_E3D_PROJECTS_DIR = $env:L3_E3D_PROJECTS_DIR
        L3_E3D_PROJECT_EVAR = $env:L3_E3D_PROJECT_EVAR
    }
    if ($E3dInstallDir) { $env:L3_E3D_INSTALL_DIR = $E3dInstallDir }
    $env:L3_E3D_PROJECTS_DIR = $projectsRoot
    $env:L3_E3D_PROJECT_EVAR = Join-Path $ProjectDir 'evarsAvevaMarineSample.bat'
    $env:L3_ALLOW_EXISTING_E3D_SESSION = if ($AllowExistingE3dSession) { '1' } else { $env:L3_ALLOW_EXISTING_E3D_SESSION }
    New-Item -ItemType Directory -Force -Path $Evidence | Out-Null
    try {
        $args = @('--check-driver', $Macro, '--project-dir', $ProjectDir,
            '--e3d-project', $E3dProject, '--e3d-mdb', $E3dMdb,
            '--aios-project', $MainProject, '--output', $Evidence)
        if (-not $ReadOnly) { $args += @('--target-db-file', $DbPath, '--target-dbnum', "$Dbnum") }
        & $l3 @args 2>&1 | Tee-Object -FilePath (Join-Path $Evidence 'driver.stdout.log')
        if ($LASTEXITCODE -ne 0) { throw "l3_suite 失败，exit=$LASTEXITCODE" }
        $report = Join-Path $Evidence 'check-driver-evidence.json'
        if (Test-Path -LiteralPath $report) {
            $json = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
            if ($json.outcome -and "$($json.outcome)" -match 'failed|indeterminate') { throw "driver outcome=$($json.outcome)" }
        }
        Wait-TtySettled
    } finally {
        foreach ($key in $saved.Keys) {
            if ($null -eq $saved[$key]) { Remove-Item "Env:$key" -ErrorAction SilentlyContinue }
            else { Set-Item "Env:$key" $saved[$key] }
        }
    }
}

function Get-LatestSesno([string]$Path) {
    $fixtureCandidates = @(
        (Join-Path (Split-Path $repo -Parent) 'target\debug\db_session_fixture.exe'),
        'D:\Rust\target\debug\db_session_fixture.exe',
        (Join-Path $repo 'target\debug\db_session_fixture.exe')
    )
    $fixture = $fixtureCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $fixture) { return $null }
    $raw = & $fixture inspect --source $Path 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    try { return [int](($raw | ConvertFrom-Json).latest_sesno) } catch { return $null }
}

function Parse-RootBaseline([string]$LogPath, [int]$Sesno) {
    $text = Get-Content -LiteralPath $LogPath -Raw -Encoding UTF8
    $ref = [regex]::Match($text, '(?im)^Ref\s*=\s*([0-9]+/[0-9]+)')
    $name = [regex]::Match($text, '(?im)^Name\s+(/\S+)')
    $type = [regex]::Match($text, '(?im)^Type\s+(\S+)')
    $owner = [regex]::Match($text, '(?im)^Owner\s+(/\S+|=\S+)')
    [ordered]@{
        name = if ($name.Success) { $name.Groups[1].Value } else { $TargetName }
        type = if ($type.Success) { $type.Groups[1].Value } else { $null }
        owner = if ($owner.Success) { $owner.Groups[1].Value } else { $null }
        refno = if ($ref.Success) { $ref.Groups[1].Value } else { $null }
        pos = $null
        ori = $null
        desc = $null
        purp = $null
        xlen = $null; ylen = $null; zlen = $null
        hpos = $null; tpos = $null; members = @(); room = $null; catalog_ref = $null
        captured_sesno = $Sesno
        baseline_mode = 'root'
        raw_log = $LogPath
    }
}

function Parse-Baseline([string]$LogPath, [int]$Sesno) {
    $text = Get-Content -LiteralPath $LogPath -Raw -Encoding UTF8
    $pos = [regex]::Match($text, '(?im)Position\s+E\s+(-?[0-9.]+)mm?\s+N\s+(-?[0-9.]+)mm?\s+(?:U|D)\s+(-?[0-9.]+)mm?')
    $ref = [regex]::Match($text, '(?im)^Ref\s*=\s*([0-9]+/[0-9]+)')
    $name = [regex]::Match($text, '(?im)^Name\s+(/\S+)')
    $type = [regex]::Match($text, '(?im)^Type\s+(\S+)')
    $owner = [regex]::Match($text, '(?im)^Owner\s+(/\S+|=\S+)')
    $desc = [regex]::Match($text, '(?im)^Description\s*(?:=|:)\s*(.*)$')
    $purp = [regex]::Match($text, '(?im)^Purpose\s*(?:=|:)\s*(.*)$')
    function Q([string]$label) {
        $m = [regex]::Match($text, "(?im)^$label\\s+(?:=|:)\\s*(-?[0-9.]+)")
        if ($m.Success) { return [double]$m.Groups[1].Value }
        return $null
    }
    if (-not $pos.Success) { throw "无法从 baseline 日志解析 POS：$LogPath" }
    [ordered]@{
        name = if ($name.Success) { $name.Groups[1].Value } else { $TargetName }
        type = if ($type.Success) { $type.Groups[1].Value } else { $null }
        owner = if ($owner.Success) { $owner.Groups[1].Value } else { $null }
        refno = if ($ref.Success) { $ref.Groups[1].Value } else { $null }
        pos = @([double]$pos.Groups[1].Value, [double]$pos.Groups[2].Value, [double]$pos.Groups[3].Value)
        ori = $null
        desc = if ($desc.Success) { $desc.Groups[1].Value.Trim() } else { $null }
        purp = if ($purp.Success) { $purp.Groups[1].Value.Trim() } else { $null }
        xlen = Q 'XLEN'; ylen = Q 'YLEN'; zlen = Q 'ZLEN'
        hpos = $null; tpos = $null; members = @(); room = $null; catalog_ref = $null
        captured_sesno = $Sesno
        raw_log = $LogPath
    }
}

function New-AbsoluteRestore([string]$Path, [string]$Target, [object]$Baseline, [string]$LogPath) {
    $p = $Baseline.pos
    $text = @"
ALPHA LOG "$($LogPath -replace '\\','/')" OVER
`$P AMS-INPLACE-RESTORE-ALIVE
/1RX03-EQUI
OWNER
$Target
POS E $($p[0]) N $($p[1]) U $($p[2])
Q POS WRT /*
SAVEWORK 'AMS in-place incremental restore'
`$P AMS-INPLACE-RESTORE-DONE
ALPHA LOG END
"@
    [IO.File]::WriteAllText($Path, $text, [Text.UTF8Encoding]::new($false))
}

function Invoke-Rvm([string]$Label, [string]$Element, [string]$DbPath, [string]$RootRefno) {
    if (-not $EnableRvm) { return $null }
    if (-not $RootRefno) { throw "RVM $Label 缺少 root refno；先确保 baseline 捕获了 Ref" }
    $rvmFile = Join-Path $Output ("rvm-{0}.rvm" -f $Label)
    $rvmLog = Join-Path $Output ("rvm-{0}.log" -f $Label)
    $rvmMacro = Join-Path $Output ("rvm-{0}.mac" -f $Label)
    New-RvmExportMacro -Path $rvmMacro -Element $Element -RvmFile $rvmFile -LogFile $rvmLog -Note "AMS in-place $Label run $RunId" -QueryRef -NoPosQuery:$RvmNoPosQuery
    $rvmText = Get-Content -LiteralPath $rvmMacro -Raw -Encoding UTF8
    $rvmText = $rvmText.Replace("`$P LINKAGE-RVM-EXPORT-BEGIN`n", "`$P LINKAGE-RVM-EXPORT-BEGIN`n/1RX03-EQUI`nOWNER`n")
    [IO.File]::WriteAllText($rvmMacro, $rvmText, [Text.UTF8Encoding]::new($false))
    Invoke-Tty $rvmMacro (Join-Path $Output ("rvm-{0}-driver" -f $Label)) $DbPath -ReadOnly
    if (-not (Test-Path -LiteralPath $rvmFile)) { throw "RVM $Label 未生成：$rvmFile" }
    $verify = $RvmVerifyExe
    if (-not $verify) {
        $candidates = @((Join-Path (Split-Path $repo -Parent) 'target\debug\rvm_verify.exe'), 'D:\Rust\target\debug\rvm_verify.exe')
        $verify = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    }
    if (-not $verify) { throw 'EnableRvm 已启用但找不到 rvm_verify.exe；请用 -RvmVerifyExe 指定。' }
    $snapshotPath = Join-Path $Output ("rvm-{0}.json" -f $Label)
    [void](Import-RvmSnapshot -Repo $repo -RvmVerifyExe $verify -RvmFile $rvmFile -Dbnum $Dbnum -RootRefno $RootRefno -OutFile $snapshotPath)
    return $snapshotPath
}

function Compare-RvmPair([string]$BeforePath, [string]$AfterPath, [double[]]$ExpectedDelta, [string]$Label) {
    $before = Get-Content -LiteralPath $BeforePath -Raw -Encoding UTF8 | ConvertFrom-Json
    $after = Get-Content -LiteralPath $AfterPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $bm = @{}; $am = @{}
    foreach ($m in @($before.members)) { if ($m.aabb_world_mm) { $bm[[string]$m.path] = @($m.aabb_world_mm) } }
    foreach ($m in @($after.members)) { if ($m.aabb_world_mm) { $am[[string]$m.path] = @($m.aabb_world_mm) } }
    $errors = New-Object System.Collections.Generic.List[string]
    foreach ($key in $bm.Keys) {
        if (-not $am.ContainsKey($key)) { $errors.Add("$key removed"); continue }
        $expected = @($ExpectedDelta[0],$ExpectedDelta[1],$ExpectedDelta[2],$ExpectedDelta[0],$ExpectedDelta[1],$ExpectedDelta[2])
        for ($i = 0; $i -lt 6; $i++) {
            $actual = [double]$am[$key][$i] - [double]$bm[$key][$i]
            if ([math]::Abs($actual - $expected[$i]) -gt 1.0) { $errors.Add("$key aabb[$i] delta=$actual expected=$($expected[$i])") }
        }
    }
    foreach ($key in $am.Keys) { if (-not $bm.ContainsKey($key)) { $errors.Add("$key added") } }
    $result = [ordered]@{ label = $Label; before = $BeforePath; after = $AfterPath; expected_delta_mm = $ExpectedDelta; status = if ($errors.Count) { 'failed' } else { 'passed' }; errors = @($errors) }
    $resultPath = Join-Path $Output ("{0}.json" -f $Label)
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resultPath -Encoding UTF8
    if ($errors.Count) { throw "RVM $Label 对拍失败：$($errors[0])" }
    return $resultPath
}

$ProjectDir = Require-Path $ProjectDir 'AMS 项目目录'
if (-not $DbFile) { $DbFile = Join-Path $ProjectDir ("ams000\ams{0}_0001" -f $Dbnum) }
$DbFile = Require-Path $DbFile '目标 DESI 库'
if ((Split-Path -Leaf $DbFile) -notmatch "ams${Dbnum}_") { throw "目标库文件名与 dbnum 不匹配：$DbFile / $Dbnum" }
$defaults = Get-CaseDefaults
$blockedReason = $null
if (-not $TargetName) { $TargetName = $defaults.Target }
if ($Case -notin @('equi-move','pipe-whole','custom')) {
    $manifestCase = Get-ManifestCase $Case
    $manifestExplicit = [bool]($ApplyMacro -or $RestoreMacro -or $TargetName)
    if (-not $manifestCase -and -not $manifestExplicit) {
        throw "未知 case：$Case（内置 equi-move、pipe-whole、custom，或 suite-manifest.json 的 id；ams-inplace 案例必须显式提供 target/macros）"
    }
    if ($manifestCase -and $manifestCase.PSObject.Properties['blocked'] -and $manifestCase.blocked) {
        $blockedReason = if ($manifestCase.PSObject.Properties['blocked_reason']) { $manifestCase.blocked_reason } else { 'manifest-blocked' }
    }
    if ($manifestCase -and $manifestCase.apply) { $ApplyMacro = Join-Path $suiteDir ([string]$manifestCase.apply -replace '/', '\') }
    if ($manifestCase -and $manifestCase.restore) { $RestoreMacro = Join-Path $suiteDir ([string]$manifestCase.restore -replace '/', '\') }
    if (-not $TargetName) { $TargetName = '/AIOS-INC-' + $RunId }
    if (-not $manifestExplicit) { $blockedReason = 'manifest-case-requires-explicit-ams-target-and-macros' }
}
if ($Case -ne 'custom' -and $Case -in @('equi-move','pipe-whole') -and -not $TargetName) { throw "case=$Case 没有 AMS 目标名称" }
if ($Case -eq 'custom' -and (-not $ApplyMacro -or -not $RestoreMacro)) { throw 'custom 必须同时指定 -ApplyMacro 和 -RestoreMacro' }
if (-not $RvmElement) { $RvmElement = $TargetName }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
if (-not $Output) { $Output = Join-Path $repo "output\ams-increment-in-place\$stamp-$Case" }
New-Item -ItemType Directory -Force -Path $Output | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
$rvmModelSource = Join-Path $suiteDir 'ams-increment-rvm-model.json'
if (Test-Path -LiteralPath $rvmModelSource) {
    Copy-Item -LiteralPath $rvmModelSource -Destination (Join-Path $Output 'rvm-model.json') -Force
}
$teardown = if ($TeardownMacro) { Require-Path $TeardownMacro 'teardown 宏' } else { $null }
function Invoke-CaseTeardown {
    if (-not $teardown) { return }
    Test-OneSavework $teardown
    Invoke-Tty $teardown (Join-Path $Output 'teardown-driver') $DbFile
    $summary.teardown_macro = $teardown
    $summary.teardown_sesno = Get-LatestSesno $DbFile
}
$summaryPath = Join-Path $Output 'summary.json'
$summary = [ordered]@{
    status = 'started'; mode = $Mode; case = $Case; run_id = $RunId; dbnum = $Dbnum; companion_dbnum = $CompanionDbnum
    project_dir = $ProjectDir; db_file = $DbFile; target_name = $TargetName; started_at = (Get-Date).ToString('o')
    output = $Output
}

try {
    Invoke-StartupPreflight
    $preflight = [ordered]@{
        project = $MainProject; e3d_project = $E3dProject; mdb = $E3dMdb; dbnum = $Dbnum
        db_file = $DbFile; project_dir = $ProjectDir; latest_sesno = (Get-LatestSesno $DbFile)
        target_name = $TargetName; case = $Case; in_place = $true; run_id = $RunId
    }
    if ($blockedReason) { $preflight.blocked_reason = $blockedReason }
    $preflight | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $Output 'preflight.json') -Encoding UTF8
    $summary.preflight = $preflight
    if ($blockedReason) { $summary.status = 'blocked'; $summary.blocked_reason = $blockedReason; return }
    if ($Mode -eq 'preflight') { $summary.status = 'passed'; return }

    $setup = if ($SetupMacro) { Require-Path $SetupMacro 'setup 宏' } else { $null }
    if ($setup) {
        Test-OneSavework $setup
        Invoke-Tty $setup (Join-Path $Output 'setup-driver') $DbFile
        $summary.setup_macro = $setup
        $summary.setup_sesno = Get-LatestSesno $DbFile
    }
    $baselineMacro = Join-Path $Output 'capture-baseline.mac'
    $baselineLog = Join-Path $Output 'capture-baseline.log'
    $values = @{ 'LOG_PATH' = ($baselineLog -replace '\\','/'); 'TARGET_NAME' = $TargetName; 'WORLD_ANCHOR' = "/1RX03-EQUI`nOWNER" }
    $baselineTemplate = if ($RootBaseline) { 'scripts\e3d\increment_suite\common\capture_root.mac' } else { 'scripts\e3d\increment_suite\common\capture_baseline.mac' }
    New-RunMacro (Join-Path $repo $baselineTemplate) $baselineMacro $values | Out-Null
    if ($Mode -in @('capture','run','apply','restore','verify')) {
        Invoke-Tty $baselineMacro (Join-Path $Output 'capture-driver') $DbFile
        $baselineSesno = Get-LatestSesno $DbFile
        $baseline = if ($RootBaseline) { Parse-RootBaseline $baselineLog $baselineSesno } else { Parse-Baseline $baselineLog $baselineSesno }
        $baseline | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $Output 'baseline.json') -Encoding UTF8
        $summary.baseline = $baseline
        if ($EnableRvm) { $summary.rvm_before = Invoke-Rvm 'before' $RvmElement $DbFile $baseline.refno }
    }
    if ($Mode -eq 'capture') { Invoke-CaseTeardown; $summary.status = 'passed'; return }

    $apply = if ($ApplyMacro) { Require-Path $ApplyMacro 'apply 宏' } else { Join-Path $Output 'apply.mac' }
    $restore = if ($RestoreMacro) { Require-Path $RestoreMacro 'restore 宏' } else { Join-Path $Output 'restore.mac' }
    if (-not $ApplyMacro) { New-ApplyMacro $apply $TargetName $defaults.ApplyDelta (Join-Path $Output 'apply.log') }
    if (-not $RestoreMacro -and $manifestCase -and -not $manifestCase.restore) {
        $blockedReason = 'restore-macro-missing-for-destructive-or-nonreversible-case'
        $summary.status = 'blocked'; $summary.blocked_reason = $blockedReason; return
    }
    if (-not $RestoreMacro) {
        if ($RootBaseline) { throw 'root baseline 需要显式 restore 宏，无法自动生成绝对位置恢复' }
        if ($defaults.RestoreKind -eq 'absolute-pos') {
            New-AbsoluteRestore $restore $TargetName $baseline (Join-Path $Output 'restore.log')
        } elseif ($defaults.RestoreKind -eq 'relative-west') {
            New-RelativeRestoreMacro $restore $TargetName 'BY W 500 WRT /*' (Join-Path $Output 'restore.log')
        } else { throw '无法为该 case 自动生成 restore；请指定 -RestoreMacro' }
    }
    $summary.apply_macro = $apply; $summary.restore_macro = $restore
    Test-OneSavework $apply
    Test-OneSavework $restore
    if ($Mode -in @('apply','run')) {
        Invoke-Tty $apply (Join-Path $Output 'apply-driver') $DbFile
        $summary.apply_sesno = Get-LatestSesno $DbFile
        if ($EnableRvm) {
            $summary.rvm_after = Invoke-Rvm 'after' $RvmElement $DbFile $baseline.refno
            if ($SkipMoveComparison) { $summary.rvm_apply_verify = 'skipped-non-translation-expectation' }
            else { $summary.rvm_apply_verify = Compare-RvmPair (Join-Path $Output 'rvm-before.json') (Join-Path $Output 'rvm-after.json') @([double]500, [double]0, [double]0) 'rvm-apply-verification' }
        }
    }
    if ($Mode -eq 'apply') { Invoke-CaseTeardown; $summary.status = 'passed'; return }
    if ($Mode -in @('restore','run')) {
        Invoke-Tty $restore (Join-Path $Output 'restore-driver') $DbFile
        $summary.restore_sesno = Get-LatestSesno $DbFile
        if ($EnableRvm) { $summary.rvm_restored = Invoke-Rvm 'restored' $RvmElement $DbFile $baseline.refno }
        if ($EnableRvm) { $summary.rvm_restore_verify = Compare-RvmPair (Join-Path $Output 'rvm-before.json') (Join-Path $Output 'rvm-restored.json') @([double]0, [double]0, [double]0) 'rvm-restore-verification' }
    }
    if ($Mode -eq 'restore') { Invoke-CaseTeardown; $summary.status = 'passed'; return }
    if ($Mode -eq 'verify') {
        Invoke-CaseTeardown
        if (-not $EnableRvm) { $summary.status = 'ready-for-verify' }
        else { $summary.status = 'passed' }
        return
    }
    Invoke-CaseTeardown
    $summary.status = 'passed'
} catch {
    $summary.status = if ($_.Exception.Message -match 'blocked|语料|没有 AMS 目标|recovery|l3_suite|des\.exe|E3D session|C0020001|runtime|TTY') { 'blocked' } else { 'failed' }
    $summary.error = "$($_.Exception.GetType().Name): $($_.Exception.Message)"
    throw
} finally {
    $summary.finished_at = (Get-Date).ToString('o')
    $summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
    Write-Host "状态：$($summary.status)"
    Write-Host "证据：$Output"
}
