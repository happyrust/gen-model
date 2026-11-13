[CmdletBinding()]
param(
    [string]$Manifest = '',
    [string]$MethodCoverage = '',
    [string]$EventCoverage = '',
    [string]$StateMatrix = '',
    [string]$BranchCoverage = '',
    [string]$Assets = '',
    [string]$ReportPath = ''
)

$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $suiteDir '..\..\..')).Path
if (-not $Manifest) { $Manifest = Join-Path $suiteDir 'ams-inplace-manifest.json' }
if (-not $MethodCoverage) { $MethodCoverage = Join-Path $repo 'docs\specs\incr-gen-audit-aux\partial-update-method-coverage.json' }
if (-not $EventCoverage) { $EventCoverage = Join-Path $repo 'docs\specs\incr-gen-audit-aux\db-userchanges-event-coverage.json' }
if (-not $StateMatrix) { $StateMatrix = Join-Path $repo 'docs\specs\incr-gen-audit-aux\partial-update-state-matrix.json' }
if (-not $BranchCoverage) { $BranchCoverage = Join-Path $repo 'docs\specs\incr-gen-audit-aux\partial-update-branch-coverage.json' }

function Read-Json([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "JSON 不存在：$Path" }
    return Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

$manifestObject = Read-Json $Manifest
$methodObject = Read-Json $MethodCoverage
$eventObject = Read-Json $EventCoverage
$stateObject = Read-Json $StateMatrix
$branchObject = Read-Json $BranchCoverage
$caseMap = @{}
foreach ($case in @($manifestObject.cases)) { $caseMap[[string]$case.id] = $case }
$errors = [System.Collections.Generic.List[string]]::new()
$refs = [System.Collections.Generic.HashSet[string]]::new()

foreach ($group in @($methodObject.methods.PSObject.Properties)) {
    foreach ($id in @($group.Value)) {
        [void]$refs.Add([string]$id)
        if (-not $caseMap.ContainsKey([string]$id)) { $errors.Add("method $($group.Name) 引用不存在 case $id") }
    }
}
foreach ($group in @($eventObject.cases.PSObject.Properties)) {
    if (@($group.Value).Count -eq 0) { $errors.Add("event $($group.Name) 没有覆盖 case") }
    foreach ($id in @($group.Value)) {
        [void]$refs.Add([string]$id)
        if (-not $caseMap.ContainsKey([string]$id)) { $errors.Add("event $($group.Name) 引用不存在 case $id") }
    }
}
foreach ($group in @($stateObject.cases.PSObject.Properties)) {
    [void]$refs.Add([string]$group.Name)
    if (-not $caseMap.ContainsKey([string]$group.Name)) { $errors.Add("state matrix 引用不存在 case $($group.Name)") }
    if (@($group.Value.sequence).Count -eq 0) { $errors.Add("state matrix $($group.Name) 没有 sequence") }
    $sequence = @($group.Value.sequence | ForEach-Object { [int]$_ })
    foreach ($state in $sequence) {
        if ($state -lt 0 -or $state -gt 5) { $errors.Add("state matrix $($group.Name) 有非法 state $state") }
    }
    $pending = @($group.Value.expected_pending | ForEach-Object { [int]$_ })
    foreach ($state in $pending) {
        if ($state -lt 0 -or $state -gt 5) { $errors.Add("state matrix $($group.Name) 有非法 expected_pending $state") }
    }
    if ($sequence -contains 5 -and [int]$group.Value.expected_draw -ne 0) { $errors.Add("state matrix $($group.Name) 的 state 5 不得 draw") }
    if ($sequence -contains 4 -and [int]$group.Value.expected_draw -ne 0) { $errors.Add("state matrix $($group.Name) 的 state 4 删除路径不得 draw") }
    if ($sequence -contains 1 -and $sequence -contains 0 -and -not ($pending -contains 1)) { $errors.Add("state matrix $($group.Name) new+changed 必须保留 state 1") }
    if ($sequence -contains 2 -and $sequence -contains 0 -and -not ($pending -contains 2)) { $errors.Add("state matrix $($group.Name) ancestor-delete+changed 必须保留 state 2") }
    if ($sequence -contains 3 -and $sequence -contains 0 -and -not ($pending -contains 3)) { $errors.Add("state matrix $($group.Name) delete+changed 必须保留 state 3") }
}
foreach ($group in @($branchObject.branches.PSObject.Properties)) {
    if (@($group.Value).Count -eq 0) { $errors.Add("branch $($group.Name) 没有覆盖 case") }
    foreach ($id in @($group.Value)) {
        [void]$refs.Add([string]$id)
        if (-not $caseMap.ContainsKey([string]$id)) { $errors.Add("branch $($group.Name) 引用不存在 case $id") }
    }
}

if ($Assets) {
    $assetRoot = (Resolve-Path -LiteralPath $Assets).Path
    foreach ($case in @($manifestObject.cases)) {
        $dir = Join-Path $assetRoot ([string]$case.id)
        foreach ($subdir in @('rvm', 'snapshots')) {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $subdir) -PathType Container)) {
                $errors.Add("$($case.id) 缺少 $subdir 目录")
            }
        }
        foreach ($file in @('action.mac','restore.mac','rvm-export-before.mac','rvm-export-after.mac','rvm-export-restored.mac','case.json','result.json')) {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { $errors.Add("$($case.id) 缺少 $file") }
        }
        $caseJsonPath = Join-Path $dir 'case.json'
        if (Test-Path -LiteralPath $caseJsonPath) {
            try {
                $caseJson = Read-Json $caseJsonPath
                foreach ($field in @('expected', 'restore', 'rvm_export_macros', 'artifacts')) {
                    if (-not $caseJson.PSObject.Properties[$field]) {
                        $errors.Add("$($case.id) case.json 缺少 $field")
                    }
                }
                foreach ($rvmField in @('before_rvm', 'after_rvm', 'restored_rvm')) {
                    if ($caseJson.artifacts -and -not $caseJson.artifacts.PSObject.Properties[$rvmField]) {
                        $errors.Add("$($case.id) case.json artifacts 缺少 $rvmField")
                    }
                }
            } catch {
                $errors.Add("$($case.id) case.json 无法解析：$($_.Exception.Message)")
            }
        }
    }
}

$counts = [ordered]@{}
foreach ($group in @($manifestObject.cases | Group-Object { [string]$_.status })) { $counts[$group.Name] = $group.Count }
$coverageReport = [ordered]@{
    manifest_cases = @($manifestObject.cases).Count
    method_groups = @($methodObject.methods.PSObject.Properties).Count
    event_groups = @($eventObject.cases.PSObject.Properties).Count
    state_cases = @($stateObject.cases.PSObject.Properties).Count
    branch_groups = @($branchObject.branches.PSObject.Properties).Count
    referenced_cases = $refs.Count
    status_counts = $counts
    asset_root = if ($Assets) { $assetRoot } else { $null }
    errors = @($errors)
    status = if ($errors.Count) { 'failed' } else { 'passed' }
}
$reportJson = $coverageReport | ConvertTo-Json -Depth 12
if ($ReportPath) {
    $reportParent = Split-Path -Parent $ReportPath
    if ($reportParent) { New-Item -ItemType Directory -Force -Path $reportParent | Out-Null }
    Set-Content -LiteralPath $ReportPath -Value $reportJson -Encoding UTF8
}
Write-Output $reportJson
if ($errors.Count) { exit 1 }
