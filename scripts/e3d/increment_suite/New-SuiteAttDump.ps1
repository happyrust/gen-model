<#
.SYNOPSIS
    从 PML 侧生成 rvm_verify 能吃的 ATT 文件，把 RVM 成员的 refno 身份补齐。

.DESCRIPTION
    为什么需要这个脚本：
    窄口径 RVM 只写几何，不写元素身份。`rvm_verify import` 不带 ATT 时，
    148 个成员里只有导出根那 1 个能解析（cli_root），其余 147 个是
    `stable_hash` 未解析。身份没解析不影响按名字比对（Compare-SuiteRvmTruth.ps1
    用相对路径当键），但会：
      - 让 `resolved/unresolved` 元数据永远是 1/147，掩盖真实覆盖率；
      - 让任何按 refno 的交叉引用（如与 PML 侧 Q REF 对账）无法进行。

    官方路径是 GenModelRvmExport CAF addin 的 pair 模式（env 变量
    GENMODEL_ATT_OUT 驱动，产出 .rvm + .att）。本机不可用：
      - launch_e3d_sample_repaired.ps1 硬编码
        C:\Program Files (x86)\AVEVA\Everything3D3.1\Startup.dll，该路径已不存在；
      - run_ams_c_entrymacro.bat 根本不加载 addin。
    所以改为从 PML 侧把同样的信息 dump 出来自己拼 ATT。

    ATT 格式（src/rvm_baseline/att.rs:34-35 的常量 + :73-113 的 merge 解析器）：
        NEW <section-key>
        KEY:= VALUE
        END
    ★ 键值分隔符是 `:=`，不是 `=`；一行里多组用 `&end&` 分隔（att.rs:34-35）。
      写成 `NAME = =ref` 会被 split_once(":=") 判为无键值对而整行丢弃——
      section 建起来了（identity_source 变成 att_name），但 NAME 取不到，
      resolved 仍是 false。这是第一版踩过的坑。
    section-key 必须与 RVM 组名逐字相同（import.rs:199 用 `self.att.get(&name)`）；
    身份字段是 `NAME:= =ref0/ref1`（import.rs:211-213 的 refno_from_att_name，
    att.rs:124-134 要求 ref0/ref1 都非 0），命中即 identity_source = "att_direct"、
    resolved = true。

.PARAMETER Element
    导出根（默认 /AIOS-SUITE-8000）。

.PARAMETER OutFile
    ATT 输出路径。

.PARAMETER DbFile
    隔离副本里的目标库文件（与 RVM 导出同一次会话口径）。

.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/New-SuiteAttDump.ps1 `
        -OutFile output\rvm-att-export\suite.att `
        -IsolatedProjectsDir E:\reverse\e3d\replay-src\E3D3.1
#>
[CmdletBinding()]
param(
    [string]$Element = '/AIOS-SUITE-8000',
    [Parameter(Mandatory)][string]$OutFile,
    [string]$IsolatedProjectsDir = 'E:\reverse\e3d\replay-src\E3D3.1',
    [string]$MainProject = 'AvevaMarineSample',
    [string]$DbFile = 'AvevaMarineSample\ams000\ams8000_0001',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$E3dInstallDir = 'E:\reverse\e3d\shadow_e3d31_gen_model_test',
    [string]$EvidenceDir = ''
)

$ErrorActionPreference = 'Stop'

$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir '..\LinkageE3dCommon.ps1')

if (-not $EvidenceDir) {
    $EvidenceDir = Join-Path (Split-Path -Parent $OutFile) 'att-dump'
}
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$EvidenceDir = (Resolve-Path -LiteralPath $EvidenceDir).Path
$logPath = Join-Path $EvidenceDir 'att-dump.log'
$macroPath = Join-Path $EvidenceDir 'att-dump.mac'

# 宏里只写 ASCII：它由 E3D 读，不赌它的编码。
# 逐元素打三段：Q CE（`TYPE /name`）、Q REF（`Ref =db/ext`）、$P 分隔行。
$logSlash = $logPath -replace '\\', '/'
$macro = @"
ALPHA LOG "$logSlash" OVER
`$P SUITE-ATT-DUMP-BEGIN
$Element
Q CE
VAR !all COLLECT ALL FOR CE
!n = !all.SIZE()
Q VAR !n
`$P SUITE-ATT-DUMP-SIZE
DO !e VALUES !all
  !!CE = !e.dbref()
  Q CE
  Q REF
  `$P SUITE-ATT-DUMP-ROW
ENDDO
`$P SUITE-ATT-DUMP-DONE
ALPHA LOG END
"@
Set-Content -LiteralPath $macroPath -Value $macro -Encoding ascii

# DbFile 已含项目名（AvevaMarineSample\ams000\ams8000_0001），不要再前置 MainProject。
$workFile = Join-Path $IsolatedProjectsDir $DbFile
if (-not (Test-Path -LiteralPath $workFile)) { throw "隔离副本里没有目标库：$workFile" }

# 走现成封装：它负责设 L3_E3D_PROJECTS_DIR、切到仓根、传 --project-dir <ProjectsDir>\<MainProject>。
# 手拼 l3_suite 参数会在 --project-dir 上踩坑（要的是项目子目录，不是 ProjectsDir 根）。
Invoke-TtyMacro -Repo $repo -L3Exe $L3Exe -Macro $macroPath `
    -ProjectsDir $IsolatedProjectsDir -MainProject $MainProject `
    -InstallDir $E3dInstallDir -Evidence $EvidenceDir -LogFile $logPath `
    -DoneMarker 'SUITE-ATT-DUMP-DONE' `
    -ExtraArgs @('--target-db-file', $workFile)

if (-not (Test-Path -LiteralPath $logPath)) { throw "没有生成日志：$logPath" }
$lines = Get-Content -LiteralPath $logPath

# 日志形态（实测 2026-10-28）：
#   SITE /AIOS-SUITE-8000        ← Q CE
#   Ref =24384/27737             ← Q REF
#   AT-ROW                       ← $P
# 逐三行一组切；不按固定步长跳，因为 Q CE 偶尔会多打一行（CE 落在 WORL 时）。
$sections = New-Object System.Collections.Generic.List[object]
$pendingCe = $null
foreach ($raw in $lines) {
    $line = $raw.Trim()
    if ($line -match '^(?<noun>[A-Z][A-Z0-9]*)\s+(?<name>/\S+)$') {
        $pendingCe = [pscustomobject]@{ noun = $Matches.noun; name = $Matches.name }
        continue
    }
    if ($line -match '^Ref\s*=\s*(?<db>\d+)/(?<ext>\d+)$') {
        if ($pendingCe) {
            $sections.Add([pscustomobject]@{
                key  = $pendingCe.name          # section key = RVM 组名 = 元素名
                noun = $pendingCe.noun
                ref  = "$($Matches.db)/$($Matches.ext)"
            })
            $pendingCe = $null
        }
    }
}

if ($sections.Count -eq 0) { throw "ATT dump 没解析出任何元素，检查 $logPath" }

$sb = New-Object System.Text.StringBuilder
# 首行是 E3D 自己的格式声明（att.rs:6 的样例），解析器不读它，但留着便于人工核对。
[void]$sb.AppendLine('AVEVA_Attributes_File v1.0 , start: NEW , end: END , name_end: := , sep: &end&')
[void]$sb.AppendLine('NEW Header Information')
[void]$sb.AppendLine("Element:= $Element")
[void]$sb.AppendLine('END')
foreach ($s in $sections) {
    [void]$sb.AppendLine("NEW $($s.key)")
    # NAME 写 refno：import.rs:211 的 refno_from_att_name 命中即 att_direct + resolved。
    [void]$sb.AppendLine("NAME:= =$($s.ref)")
    [void]$sb.AppendLine("TYPE:= $($s.noun)")
    [void]$sb.AppendLine('END')
}
$dir = Split-Path -Parent $OutFile
if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
Set-Content -LiteralPath $OutFile -Value $sb.ToString() -Encoding utf8

Write-Output "ATT 已写出：$OutFile"
Write-Output "  元素数 : $($sections.Count)"
Write-Output "  证据   : $EvidenceDir"
