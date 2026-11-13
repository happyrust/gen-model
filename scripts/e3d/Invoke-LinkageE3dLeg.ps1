<#
.SYNOPSIS
    增量更新 K15 现场一腿（docs/plans/2026-10-27-plant3d-web-incremental-linkage-test-cases.md §3）：在隔离工程副本上用 E3D TTY
    施加一次变化（apply = BY U <ShiftMm>、restore = BY D <ShiftMm>，恰好一个 SAVEWORK），再只读导出同一元素的 RVM 并
    rvm_verify import。最后一行打印 {"leg":…,"snapshot":…}，由 plant3d-web 的 e2e/incremental-linkage-gen-model-v1.spec.ts 读。

.PARAMETER StackFile
    scripts/replay/Start-LinkageReplayStack.ps1 -Live 写出的 stack.json：隔离副本、E3D 安装、l3_suite、rvm_verify 都从这里取。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('apply', 'restore')][string]$Leg,
    [Parameter(Mandatory)][string]$StackFile,
    [string]$Element = '=24384/24776',
    [int]$ShiftMm = 500,
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'LinkageE3dCommon.ps1')

$stack = Get-Content -LiteralPath $StackFile -Raw -Encoding UTF8 | ConvertFrom-Json
if ($stack.mode -ne 'live') { throw "$StackFile 不是现场形态（要 Start-LinkageReplayStack.ps1 -Live 写的）" }
$projects = Assert-IsolatedProjects $stack.project_root $Source
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$evidence = Join-Path $stack.run_dir "e3d-$Leg-$stamp"
New-Item -ItemType Directory -Force -Path $evidence | Out-Null

# 与 scripts/e3d/replay/equi_apply.mac / equi_restore.mac 同一个动作，去掉录制用的 SYSCOM 拷文件；宏里只写 ASCII。
$direction = if ($Leg -eq 'apply') { 'U' } else { 'D' }
$legLog = Join-Path $evidence "$Leg.log"
$legMacro = Join-Path $evidence "$Leg.mac"
$legLogForward = $legLog -replace '\\', '/'
$legText = @"
-- plant3d-web linkage K15 live leg ($Leg): $Element BY $direction $ShiftMm mm.
-- Exactly one SAVEWORK; the l3_suite driver owns QUIT.
ONERROR CONTINUE
ALPHA LOG "$legLogForward" OVER
`$P LINKAGE-LEG-ALIVE
$Element
Q NAME
Q REF
Q POS
BY $direction ${ShiftMm}mm
Q POS
SAVEWORK 'plant3d-web linkage K15 ${Leg}: $Element BY $direction $ShiftMm mm'
`$P LINKAGE-LEG-DONE
ALPHA LOG END
"@
Write-Utf8NoBom $legMacro (($legText -replace "`r`n", "`n") + "`n")
Invoke-TtyMacro -Repo $stack.gen_model_repo -L3Exe $stack.l3_exe -Macro $legMacro -ProjectsDir $projects -MainProject $stack.main_project `
    -InstallDir $stack.e3d_install_dir -Evidence (Join-Path $evidence 'leg-driver') -LogFile $legLog -DoneMarker 'LINKAGE-LEG-DONE'

$rvm = Join-Path $evidence "$Leg.rvm"
$exportLog = Join-Path $evidence 'rvm_export.log'
$exportMacro = Join-Path $evidence 'rvm_export.mac'
New-RvmExportMacro -Path $exportMacro -Element $Element -RvmFile $rvm -LogFile $exportLog -Note "linkage K15 $Leg $Element"
Invoke-TtyMacro -Repo $stack.gen_model_repo -L3Exe $stack.l3_exe -Macro $exportMacro -ProjectsDir $projects -MainProject $stack.main_project `
    -InstallDir $stack.e3d_install_dir -Evidence (Join-Path $evidence 'export-driver') -LogFile $exportLog -DoneMarker 'LINKAGE-RVM-EXPORT-DONE'
if (-not (Test-Path -LiteralPath $rvm) -or (Get-Item -LiteralPath $rvm).Length -le 0) { throw "RVM 没导出来：$rvm" }

$snapshotPath = Join-Path $evidence "$Leg.rvm.json"
$snapshot = Import-RvmSnapshot -Repo $stack.gen_model_repo -RvmVerifyExe $stack.rvm_verify_exe -RvmFile $rvm `
    -Dbnum ([int]$stack.dbnum) -RootRefno $Element.TrimStart('=') -OutFile $snapshotPath
[ordered]@{
    leg        = $Leg
    snapshot   = $snapshotPath
    evidence   = $evidence
    members    = $snapshot.meta.member_count
    unresolved = $snapshot.meta.unresolved
} | ConvertTo-Json -Compress
