[CmdletBinding()]
param(
    [ValidateSet('plan','run')][string]$Mode = 'plan',
    [Parameter(Mandatory)][string]$Case,
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$DbFile = '',
    [int]$Dbnum = 8000,
    [string]$RunId = '',
    [string]$Output = '',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$RvmVerifyExe = 'D:\Rust\target\debug\rvm_verify.exe',
    [string]$E3dInstallDir = 'D:\work\plant-code\old\e3d31-runtime',
    [switch]$AllowExistingE3dSession
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $repo 'scripts\e3d\LinkageE3dCommon.ps1')
if (-not $RunId) { $RunId = Get-Date -Format 'yyyyMMdd-HHmmss' }
if ($RunId -notmatch '^[A-Za-z0-9][A-Za-z0-9_-]{2,63}$') { throw "RunId 格式非法：$RunId" }
$manifest = Get-Content -LiteralPath (Join-Path $suiteDir 'ams-inplace-manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$spec = @($manifest.cases | Where-Object { $_.id -eq $Case }) | Select-Object -First 1
$alias = $null
if (-not $spec -and $manifest.aliases.PSObject.Properties[$Case]) {
    $alias = $manifest.aliases.PSObject.Properties[$Case].Value
    $spec = @($manifest.cases | Where-Object { $_.id -eq $alias.case }) | Select-Object -First 1
}
if (-not $spec) { throw "ams-inplace-manifest.json 没有 case：$Case" }
if (-not $DbFile) { $DbFile = Join-Path $ProjectDir ("ams000\ams{0}_0001" -f $Dbnum) }
if (-not $Output) { $Output = Join-Path $repo ("output\ams-increment-in-place\{0}\{1}" -f $RunId, $Case) }
New-Item -ItemType Directory -Force -Path $Output | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
$rvmDir = Join-Path $Output 'rvm'; $snapshotDir = Join-Path $Output 'snapshots'
New-Item -ItemType Directory -Force -Path $rvmDir,$snapshotDir | Out-Null

function Write-Text([string]$Path, [string]$Text) { [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false)) }
function Copy-ActionAsset([string]$Relative, [string]$Destination, [string]$TargetName) {
    if (-not $Relative) { return $false }
    $source = Join-Path $suiteDir ($Relative.Replace('/', '\'))
    if (-not (Test-Path -LiteralPath $source)) { throw "action/restore 宏不存在：$source" }
    $text = Get-Content -LiteralPath $source -Raw -Encoding UTF8
    $log = [IO.Path]::ChangeExtension($Destination, '.log').Replace('\','/')
    $text = $text.Replace('{{LOG_PATH}}', $log).Replace('{{WORLD_ANCHOR}}', "/1RX03-EQUI`nOWNER").Replace('{{RUN_ID}}', $RunId).Replace('{{TARGET_NAME}}', $TargetName)
    Write-Text $Destination $text
    return $true
}
function Assert-OneSavework([string]$Path) {
    $count = @(Select-String -LiteralPath $Path -Pattern '(?im)^\s*SAVEWORK\b').Count
    if ($count -ne 1) { throw "ready case 宏必须恰好一个 SAVEWORK：$Path（实际 $count）" }
}
function New-ReadyAction([string]$Kind, [string]$Target, [string]$Path) {
    if ($Kind -eq 'action') {
        Write-Text $Path @"
ALPHA LOG "$((Join-Path $Output 'apply.log').Replace('\','/'))" OVER
`$P AMS-INPLACE-APPLY-ALIVE
/1RX03-EQUI
OWNER
$Target
BY E 500 WRT /*
SAVEWORK 'AMS in-place $Case apply'
`$P AMS-INPLACE-APPLY-DONE
ALPHA LOG END
"@
    } else {
        Write-Text $Path @"
ALPHA LOG "$((Join-Path $Output 'restore.log').Replace('\','/'))" OVER
`$P AMS-INPLACE-RESTORE-ALIVE
/1RX03-EQUI
OWNER
$Target
BY W 500 WRT /*
SAVEWORK 'AMS in-place $Case restore'
`$P AMS-INPLACE-RESTORE-DONE
ALPHA LOG END
"@
    }
}

function Resolve-CaseRoot([string]$Root) {
    if (-not $Root) { return $null }
    return $Root.Replace('<run-id>', $RunId)
}

function New-CaseRvmMacro([string]$Label, [string]$Element, [string]$RvmFile, [string]$LogFile) {
    $path = Join-Path $Output ("rvm-export-{0}.mac" -f $Label)
    New-RvmExportMacro -Path $path -Element $Element -RvmFile $RvmFile -LogFile $LogFile -Note "AMS case $Case $Label" -QueryRef
    $text = Get-Content -LiteralPath $path -Raw -Encoding UTF8
    $text = $text.Replace("`$P LINKAGE-RVM-EXPORT-BEGIN`n", "`$P LINKAGE-RVM-EXPORT-BEGIN`n/1RX03-EQUI`nOWNER`n")
    Write-Text $path $text
    return $path
}

$manifestRoot = if ($alias -and $alias.rvm_root) { [string]$alias.rvm_root } elseif ($spec.rvm_root) { [string]$spec.rvm_root } else { $null }
$target = if ($alias -and $alias.target) { [string]$alias.target } elseif ($spec.target) { [string]$spec.target } elseif ($manifestRoot) { Resolve-CaseRoot $manifestRoot } else { "/AIOS-INC-$RunId" }
$target = Resolve-CaseRoot $target
$rvmRoot = if ($manifestRoot) { Resolve-CaseRoot $manifestRoot } else { $target }
$effectiveStatus = if ($alias -and $alias.status) { [string]$alias.status } else { [string]$spec.status }
$baselineMode = if ($spec.baseline_mode) { [string]$spec.baseline_mode } else { 'positional' }
$actionPath = Join-Path $Output 'action.mac'; $restorePath = Join-Path $Output 'restore.mac'
if ($spec.action_macro -and -not $alias) { [void](Copy-ActionAsset $spec.action_macro $actionPath $target) }
elseif ($effectiveStatus -eq 'ready') { New-ReadyAction 'action' $target $actionPath }
else { Write-Text $actionPath "-- $effectiveStatus case ${Case}: $($spec.blocked_reason)`n" }
if ($spec.restore_macro -and -not $alias) { [void](Copy-ActionAsset $spec.restore_macro $restorePath $target) }
elseif ($effectiveStatus -eq 'ready') { New-ReadyAction 'restore' $target $restorePath }
else { Write-Text $restorePath "-- no restore macro ($effectiveStatus): $($spec.blocked_reason)`n" }
$setupPath = Join-Path $Output 'setup.mac'; $hasSetup = $false
$teardownPath = Join-Path $Output 'teardown.mac'; $hasTeardown = $false
if ($spec.setup_macro -and -not $alias) { $hasSetup = Copy-ActionAsset $spec.setup_macro $setupPath $target }
if ($spec.teardown_macro -and -not $alias) { $hasTeardown = Copy-ActionAsset $spec.teardown_macro $teardownPath $target }
$rvmBefore = New-CaseRvmMacro 'before' $rvmRoot (Join-Path $rvmDir 'before.rvm') (Join-Path $rvmDir 'before.log')
$rvmAfter = New-CaseRvmMacro 'after' $rvmRoot (Join-Path $rvmDir 'after.rvm') (Join-Path $rvmDir 'after.log')
$rvmRestored = New-CaseRvmMacro 'restored' $rvmRoot (Join-Path $rvmDir 'restored.rvm') (Join-Path $rvmDir 'restored.log')
# Keep the historical name as a before-export alias for existing callers.
Copy-Item -LiteralPath $rvmBefore -Destination (Join-Path $Output 'rvm-export.mac') -Force
if ($effectiveStatus -eq 'ready') {
    Assert-OneSavework $actionPath
    Assert-OneSavework $restorePath
}
$expected = [ordered]@{ added=@(); removed=@(); moved=@{}; resized=@{}; renamed=@{}; owner_changes=@(); member_changes=@() }
function Resolve-RunIdPlaceholder($Value) {
    if ($null -eq $Value) { return $null }
    if ($Value -is [string]) { return $Value.Replace('<run-id>', $RunId) }
    if ($Value -is [System.Collections.IDictionary]) {
        $copy = [ordered]@{}
        foreach ($key in $Value.Keys) { $copy[$key] = Resolve-RunIdPlaceholder $Value[$key] }
        return $copy
    }
    if ($Value -is [System.Collections.IEnumerable]) {
        return ,@($Value | ForEach-Object { Resolve-RunIdPlaceholder $_ })
    }
    if ($Value -is [System.Management.Automation.PSCustomObject]) {
        $copy = [ordered]@{}
        foreach ($property in $Value.PSObject.Properties) {
            $copy[$property.Name.Replace('<run-id>', $RunId)] = Resolve-RunIdPlaceholder $property.Value
        }
        return $copy
    }
    return $Value
}
$expectedSource = if ($alias -and $alias.expected_change) { $alias.expected_change } else { $spec.expected_change }
if ($expectedSource) {
    $expectedSource = Resolve-RunIdPlaceholder $expectedSource
    foreach ($key in $expectedSource.Keys) { $expected[$key] = $expectedSource[$key] }
}
$caseJson = [ordered]@{ id=$Case; run_id=$RunId; dbnum=$Dbnum; companion_dbnum=$(if($spec.companion_dbnum){$spec.companion_dbnum}else{$null}); target=$target; rvm_root=$rvmRoot; baseline_mode=$baselineMode; action_macro='action.mac'; restore_macro=$(if($spec.restore_macro -or $effectiveStatus -eq 'ready'){'restore.mac'}else{$null}); setup_macro=$(if($hasSetup){'setup.mac'}else{$null}); teardown_macro=$(if($hasTeardown){'teardown.mac'}else{$null}); rvm_export_macro='rvm-export.mac'; rvm_export_macros=[ordered]@{ before='rvm-export-before.mac'; after='rvm-export-after.mac'; restored='rvm-export-restored.mac' }; expected=$expected; restore=$spec.restore_expectation; destructive_policy=$spec.destructive_policy; status=$effectiveStatus; blocked_reason=$(if($effectiveStatus -eq 'blocked'){$spec.blocked_reason}else{$null}); calibration_reason=$(if($effectiveStatus -eq 'calibrate'){$spec.blocked_reason}else{$null}); artifacts=[ordered]@{ before_rvm='rvm/before.rvm'; after_rvm='rvm/after.rvm'; restored_rvm='rvm/restored.rvm'; before_snapshot='snapshots/before.json'; after_snapshot='snapshots/after.json'; restored_snapshot='snapshots/restored.json'; result='result.json' } }
$caseJson | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $Output 'case.json') -Encoding UTF8
$summary = [ordered]@{ status=$effectiveStatus; case=$Case; run_id=$RunId; output=$Output; case_json=(Join-Path $Output 'case.json'); started_at=(Get-Date).ToString('o') }
if ($Mode -eq 'plan') {
    $summary.status = $effectiveStatus
    $summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $Output 'result.json') -Encoding UTF8
    Write-Host "case=$Case status=$effectiveStatus output=$Output"
    exit 0
}
if ($effectiveStatus -eq 'blocked') {
    $summary.blocked_reason = $spec.blocked_reason
    $summary.finished_at = (Get-Date).ToString('o')
    $summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $Output 'result.json') -Encoding UTF8
    Write-Host "状态：blocked（$($spec.blocked_reason)）"
    exit 0
}
$runner = Join-Path $suiteDir 'Run-AmsIncrementInPlace.ps1'
$skipMoveComparison = -not ($expectedSource -and $expectedSource.moved -and $expectedSource.moved.all)
try {
    $runnerParams = @{ Mode='run'; Case=$Case; ProjectDir=$ProjectDir; DbFile=$DbFile; Dbnum=$Dbnum; TargetName=$target; ApplyMacro=$actionPath; RestoreMacro=$restorePath; L3Exe=$L3Exe; RvmVerifyExe=$RvmVerifyExe; E3dInstallDir=$E3dInstallDir; EnableRvm=$true; Output=$Output; RunId=$RunId; RvmElement=$rvmRoot; AllowExistingE3dSession=[bool]$AllowExistingE3dSession }
    if ($hasSetup) { $runnerParams.SetupMacro = $setupPath }
    if ($hasTeardown) { $runnerParams.TeardownMacro = $teardownPath }
    if ($skipMoveComparison) { $runnerParams.SkipMoveComparison = $true }
    if ($baselineMode -eq 'root') { $runnerParams.RootBaseline = $true }
    if ($spec.PSObject.Properties['rvm_no_pos_query'] -and $spec.rvm_no_pos_query) { $runnerParams.RvmNoPosQuery = $true }
    & pwsh -NoProfile -File $runner @runnerParams
    $runnerSummary = Join-Path $Output 'summary.json'
    if (Test-Path -LiteralPath (Join-Path $Output 'rvm-before.rvm')) { Copy-Item (Join-Path $Output 'rvm-before.rvm') (Join-Path $rvmDir 'before.rvm') -Force }
    if (Test-Path -LiteralPath (Join-Path $Output 'rvm-after.rvm')) { Copy-Item (Join-Path $Output 'rvm-after.rvm') (Join-Path $rvmDir 'after.rvm') -Force }
    if (Test-Path -LiteralPath (Join-Path $Output 'rvm-restored.rvm')) { Copy-Item (Join-Path $Output 'rvm-restored.rvm') (Join-Path $rvmDir 'restored.rvm') -Force }
    foreach ($label in @('before', 'after', 'restored')) {
        $snapshotSource = Join-Path $Output ("rvm-{0}.json" -f $label)
        if (Test-Path -LiteralPath $snapshotSource) { Copy-Item $snapshotSource (Join-Path $snapshotDir ("{0}.json" -f $label)) -Force }
    }
    $summary.status = if (Test-Path -LiteralPath $runnerSummary) { (Get-Content $runnerSummary -Raw | ConvertFrom-Json).status } else { 'failed' }
} catch {
    $summary.status = if ($_.Exception.Message -match 'TTY|driver|rvm|recovery|l3_suite|des\.exe|E3D session|C0020001|runtime') { 'blocked' } else { 'failed' }
    $summary.error = "$($_.Exception.GetType().Name): $($_.Exception.Message)"
}
$summary.finished_at = (Get-Date).ToString('o')
$summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $Output 'result.json') -Encoding UTF8
Write-Host "状态：$($summary.status)"
