<#
.SYNOPSIS
    给 L2 回放链导出每一步的 RVM 真值（docs/plans/2026-10-27-plant3d-web-incremental-linkage-test-cases.md §4）。

.DESCRIPTION
    E3D 一登录就改写所有打开的库的文件头，影子工程根又是硬链接 / junction 到原件的，所以真值只在与原件完全断开的
    隔离工程副本上导（-IsolatedProjectsDir；脚本先核一遍）。对每个 -Steps：
      1. db_session_fixture replay-init 把副本里被回放的库切到该步（与回放时服务看到的同一份字节）；
      2. l3_suite --check-driver 跑本脚本现写的只读导出宏（窄口径、无 SAVEWORK），导出 -Element 的 RVM；
      3. rvm_verify import → <链夹具>/rvm-truth/<sesno>.json（plant3d-web e2e/incremental-linkage-gen-model-v1.spec.ts 读它）。
    只给元素存在的那几步；元素在某步已删掉（如 db8000-two-delete 的 26）就不导，e2e 用「场景里一个对象也不剩」判。
    证据（宏、日志、.rvm、driver 记录）在 output/rvm-truth/<Case>-<时刻>/<sesno>/。

.EXAMPLE
    scripts/e3d/Export-ReplayRvmTruth.ps1 -Case ams8000-equi-move-chain -Steps 725,727,730 -IsolatedProjectsDir E:\reverse\e3d\linkage-truth\E3D3.1
.EXAMPLE
    scripts/e3d/Export-ReplayRvmTruth.ps1 -Case db8000-two-delete -Steps 24,25 -Element =24384/24778 -IsolatedProjectsDir E:\reverse\e3d\linkage-truth\E3D3.1
#>
[CmdletBinding()]
param(
    [string]$Case = 'ams8000-equi-move-chain',
    [Parameter(Mandatory)][int[]]$Steps,
    [string]$Element = '=24384/24776',
    [Parameter(Mandatory)][string]$IsolatedProjectsDir,
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1',
    [string]$MainProject = 'AvevaMarineSample',
    [string]$TargetDir = 'D:\Rust\target',
    [ValidateSet('release', 'debug')][string]$BuildProfile = 'release',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$E3dInstallDir = 'E:\reverse\e3d\shadow_e3d31_gen_model_test'
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $PSScriptRoot 'LinkageE3dCommon.ps1')

$projects = Assert-IsolatedProjects $IsolatedProjectsDir $Source
$spec = Get-ReplayCase $repo $Case
$fixtureExe = Join-Path $TargetDir "$BuildProfile\db_session_fixture.exe"
$rvmVerifyExe = Join-Path $TargetDir "$BuildProfile\rvm_verify.exe"
foreach ($path in @($fixtureExe, $L3Exe)) {
    if (-not (Test-Path -LiteralPath $path)) { throw "缺少前置：$path" }
}
$workFile = Join-Path $projects ($spec.db_file -replace '/', '\')
if (-not (Test-Path -LiteralPath $workFile)) { throw "隔离副本里没有被回放的库：$workFile" }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$outRoot = Join-Path $repo "output\rvm-truth\$Case-$stamp"
New-Item -ItemType Directory -Force -Path $outRoot | Out-Null
$replaySource = Get-ReplaySource $spec $repo $outRoot
$fixtureArgs = @($replaySource.Args)
New-Item -ItemType Directory -Force -Path $replaySource.TruthDir | Out-Null
$rootRefno = $Element.TrimStart('=')

foreach ($step in $Steps) {
    $evidence = Join-Path $outRoot "$step"
    New-Item -ItemType Directory -Force -Path $evidence | Out-Null
    $initOut = & $fixtureExe replay-init @fixtureArgs --to $step --out $workFile
    if ($LASTEXITCODE -ne 0) { throw "replay-init --to $step 失败：$($initOut -join "`n")" }
    "replay-init ${step}: $(@($initOut)[-1])"

    $rvm = Join-Path $evidence "$step.rvm"
    $log = Join-Path $evidence 'rvm_export.log'
    $macro = Join-Path $evidence 'rvm_export.mac'
    New-RvmExportMacro -Path $macro -Element $Element -RvmFile $rvm -LogFile $log -Note "linkage truth $Case sesno $step $Element"
    Invoke-TtyMacro -Repo $repo -L3Exe $L3Exe -Macro $macro -ProjectsDir $projects -MainProject $MainProject -InstallDir $E3dInstallDir `
        -Evidence (Join-Path $evidence 'driver') -LogFile $log -DoneMarker 'LINKAGE-RVM-EXPORT-DONE'
    if (-not (Test-Path -LiteralPath $rvm) -or (Get-Item -LiteralPath $rvm).Length -le 0) { throw "RVM 没导出来：$rvm" }

    $truth = Join-Path $replaySource.TruthDir "$step.json"
    $snapshot = Import-RvmSnapshot -Repo $repo -RvmVerifyExe $rvmVerifyExe -RvmFile $rvm -Dbnum ([int]$spec.dbnum) -RootRefno $rootRefno -OutFile $truth
    "sesno $step -> $truth（成员 $($snapshot.meta.member_count)，几何 $($snapshot.meta.geometry_count)，未解析 $($snapshot.meta.unresolved)）"
}
"证据：$outRoot"
