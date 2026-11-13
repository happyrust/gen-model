[CmdletBinding()]
param(
    [string]$RunId = '',
    [string]$Output = '',
    [string]$Manifest = '',
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$E3dInstallDir = 'D:\work\plant-code\old\e3d31-runtime',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$RvmVerifyExe = 'D:\Rust\target\debug\rvm_verify.exe',
    [switch]$ExecuteReady,
    [switch]$AllowExistingE3dSession
)

$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $suiteDir '..\..\..')).Path
if (-not $RunId) { $RunId = Get-Date -Format 'yyyyMMdd-HHmmss' }
if ($RunId -notmatch '^[A-Za-z0-9][A-Za-z0-9_-]{2,63}$') { throw "RunId 格式非法：$RunId" }
if (-not $Manifest) { $Manifest = Join-Path $suiteDir 'ams-inplace-manifest.json' }
if (-not $Output) { $Output = Join-Path $repo ("output\ams-increment-rvm-assets\{0}" -f $RunId) }
New-Item -ItemType Directory -Force -Path $Output | Out-Null
$Output = (Resolve-Path -LiteralPath $Output).Path
$branchCoverage = Join-Path $repo 'docs\specs\incr-gen-audit-aux\partial-update-branch-coverage.json'
if (Test-Path -LiteralPath $branchCoverage) {
    Copy-Item -LiteralPath $branchCoverage -Destination (Join-Path $Output 'partial-update-branch-coverage.json') -Force
}
$manifestObject = Get-Content -LiteralPath $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$exporter = Join-Path $suiteDir 'Export-AmsIncrementCaseRvm.ps1'
$rows = [System.Collections.Generic.List[object]]::new()

foreach ($spec in @($manifestObject.cases)) {
    $caseOutput = Join-Path $Output ([string]$spec.id)
    try {
        $mode = if ($ExecuteReady -and [string]$spec.status -eq 'ready') { 'run' } else { 'plan' }
        $args = @('-NoProfile', '-File', $exporter, '-Mode', $mode, '-Case', ([string]$spec.id),
            '-RunId', $RunId, '-Output', $caseOutput, '-ProjectDir', $ProjectDir,
            '-E3dInstallDir', $E3dInstallDir, '-L3Exe', $L3Exe, '-RvmVerifyExe', $RvmVerifyExe)
        if ($AllowExistingE3dSession) { $args += '-AllowExistingE3dSession' }
        & pwsh @args 2>&1 | ForEach-Object { Write-Host $_ }
        if ($LASTEXITCODE -ne 0) { throw "case exporter exit=$LASTEXITCODE" }
        $resultPath = Join-Path $caseOutput 'result.json'
        if (-not (Test-Path -LiteralPath $resultPath)) { throw "缺少 result.json" }
        $result = Get-Content -LiteralPath $resultPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $rows.Add([ordered]@{ id=$spec.id; status=$result.status; output=$caseOutput; case_json=(Join-Path $caseOutput 'case.json') })
    } catch {
        $rows.Add([ordered]@{ id=$spec.id; status='failed'; output=$caseOutput; error="$($_.Exception.GetType().Name): $($_.Exception.Message)" })
    }
}

$index = [ordered]@{
    version = 1
    suite = 'ams-increment-in-place'
    run_id = $RunId
    generated_at = (Get-Date).ToString('o')
    project = 'AvevaMarineSample'
    execution = if ($ExecuteReady) { 'ready cases executed by E3D TTY; blocked cases remain plan-only' } else { 'plan-only; RVM files are generated later by E3D TTY' }
    tolerance_mm = 1.0
    cases = @($rows)
    counts = [ordered]@{}
}
foreach ($group in @($rows | Group-Object { $_['status'] })) { $index.counts[$group.Name] = $group.Count }
$index | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $Output 'index.json') -Encoding UTF8
Write-Host "RVM 资产准备完成：$Output"
Write-Host (($index.counts | ConvertTo-Json -Compress))
