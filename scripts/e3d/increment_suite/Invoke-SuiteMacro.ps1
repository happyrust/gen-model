#requires -Version 7
<#
.SYNOPSIS
    在 E3D 私有副本上跑一份或一串 PML 宏：静态审宏 → 等锁、看内存 → 逐份开 TTY 会话 → 收日志 → 判成败。

.DESCRIPTION
    只认带标记的私有副本：副本在哪由本机的 output/increment-suite/local.json 给出（不进仓库），
    调用方只能用 -Copy 选副本的名字，给不了路径；副本根下必须有 .private-copy.json，且轮次、路径都对得上。
    正式项目目录、别人的副本都没有这个标记，所以这份脚本开不到它们。

    一次调用 = 一批：-Macro 里的各步按顺序跑，每步一次 E3D TTY 会话，整批只占一次会话锁
    （别人的会话插不进来，录会话链时窗口是连着的）。步的写法：

        create/box_apply.mac            套件目录下的相对路径；也可以是绝对路径
        t1-pipe/x_apply.mac@7998        @<库号>：这一步的 SAVEWORK 应当落在哪个库（缺省 -Dbnum，再缺省副本的主库）
        t1-pipe/x_apply.mac@8000+7998   一次存盘写两个库，两个库的会话号都得前进
        export:/AIOS-SUITE-8000         只读导出这个元素的 RVM（窄口径，与 Export-SuiteRvmTruth 同一份导出宏）
        exportnp:/SOME-BRAN             同上，但不查 Q POS（BRAN 这类没有 POS 的根，查了 E3D 会崩）

    也可以不写步、直接给用例号：-Case D5-pjoi-only[,另一个用例号]。入口从主清单 suite-manifest.json 和
    manifest.d/*.json 的片段里找到它，按它登记的副本、MDB、库展开成步（apply → restore，或链的各腿；
    几个用例共用一条链时这条链只跑一次）。-WithSetup / -WithTeardown 把所在片段的 setup / teardown 接在前后。
    -Case 和 -Macro 都给时先跑用例的步、再跑 -Macro 的步。清单的规矩见 SuiteManifest.ps1。

    -ModuleArgs '-module=81 -modulename=Paragon'：这一批的会话进别的模块（开关接在 MDB 后面交给 des.exe；
    取证时证过 Paragon 这样能不开界面进去、能跑宏、能存盘）。别的模块号没有人试过。

    占位符：{{LOG_PATH}} → 渲染后的宏同目录同名 .log；{{WORLD_ANCHOR}} → 两行（锚点名 + OWNER，
    README「跑之前必须先读这一条」）；其余用 -Var NAME=值 给。渲染后还剩 {{ 的宏不跑。

    静态审（不过就整批不跑，不碰 E3D）：不许自带 QUIT / FINISH（会话归驱动器）；不许 MERGE / PURGE / COMPACT
    （除非 -AllowSessionRewrite）；SAVEWORK 至多一句（除非 -AllowMultiSave）；ALPHA LOG 成对；
    有 `$P ...-DONE` 结束标记（除非 -NoDoneMarker）。

    -DryRun（试宏用）：渲染时在宏头加一句 ONERROR CONTINUE，并把 SAVEWORK 换成一句 `$P DRYRUN-SAVEWORK-SKIPPED`。
    这样哪一句报错、报的什么，都带着行号留在宏日志里，而且什么都不存盘（会话收尾是 QUIT，改动丢弃）。
    2026-11-12 实测：宏里没有 ONERROR CONTINUE 时，任何一句报错（语法错、导航到不存在的元素……）都会让这次
    TTY 会话以 0xC0000005 崩掉，宏日志只剩 3 个字节；有了它，报错行形如 `(2,109)   Undefined name /X`，宏接着往下走。
    正式的写步不要带 ONERROR CONTINUE：出错就崩、什么都不存，比「半截存盘」安全。

    开会话之前等到：没有 output/ops/swap.lock、没有会话锁 output/.e3d-session.lock、没有 E3D 进程、
    可提交内存余量 ≥ -MinCommitGB、副本所在盘的余量够。每 5 秒看一次，最多等 -WaitMinutes 分钟。
    排队先来先得：每次调用在 output/.e3d-session.queue/ 放一张票，排在前面的票的主人还活着就让它先拿锁。
    会话经 `l3_suite --check-driver` 投递（它自己包 L3-ALIVE / L3-DONE / QUIT，并核对目标库会话号前进）。

    每步的判定（全过才算 pass）：
        驱动器退出码 0；宏日志里有结束标记；日志里没有 E3D 报错行（形如 `(2,109) ...`，除非 -AllowErrorLines）；
        写步（宏里有 SAVEWORK）点名的库会话号都前进了；启动脚本报出来的项目目录就是私有副本。
        写步把没点名的库的会话号也推进了，只提醒不判红（记在 result.json 的 warnings 里）。
    一步不过，后面的步不跑（除非 -ContinueOnFail）：库里的状态已经不可信。

    证据目录 <runs_dir>/<Who>/<时间戳>-<Label>/：每步的渲染宏、宏日志、驱动器证据、控制台输出，
    整批的 result.json、会话前后副本的文件清单与「大小变了的文件」。另往 <runs_dir>/sessions.log 追加一行一步。
    result.json 就是这一批的会话链记录：每步前后各库的会话号、从日志回读的「名字 → refno」（宏里 Q REF 紧跟 Q NAME）。
    带 SAVEWORK 的一步让目标库 +2 个会话：N+1 是存盘那一个（改动在这里），N+2 是 QUIT 时写的只含记账的尾会话。

    退出码：0 全过；1 有步没过；2 参数或静态审不过；6 没等到能开会话；7 本机配置或私有副本不对；
            8 同一批（同一个人、同一个标签、步完全相同）已经在排队或在跑，这一次没有再排。

.EXAMPLE
    # 只审不跑
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteMacro.ps1 -Who T1 -CheckOnly -Macro t1-pipe/a_apply.mac,t1-pipe/a_restore.mac
.EXAMPLE
    # 一对宏：apply → restore
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteMacro.ps1 -Who T1 -Label a -Macro t1-pipe/a_apply.mac,t1-pipe/a_restore.mac
.EXAMPLE
    # 带 E3D 真值：前 / 后 / 还原各导一份 RVM
    pwsh -NoProfile -File scripts/e3d/increment_suite/Invoke-SuiteMacro.ps1 -Who T2 -Label b `
        -Macro export:/AIOS-SUITE-8000,t2-stru/b_apply.mac,export:/AIOS-SUITE-8000,t2-stru/b_restore.mac,export:/AIOS-SUITE-8000
#>
[CmdletBinding()]
param(
    [string[]]$Macro = @(),
    [string[]]$Case = @(),
    [switch]$WithSetup,
    [switch]$WithTeardown,
    [Parameter(Mandatory)][string]$Who,
    [string]$Label = '',
    [string]$Copy = 'ams',
    [int]$Dbnum = 0,
    [string]$Mdb = '',
    [string]$Anchor = '',
    [string]$ModuleArgs = '',
    [string[]]$Var = @(),
    [switch]$AllowErrorLines,
    [switch]$NoDoneMarker,
    [switch]$AllowMultiSave,
    [switch]$AllowSessionRewrite,
    [switch]$ContinueOnFail,
    [switch]$CheckOnly,
    [switch]$DryRun,
    [int]$WaitMinutes = 30,
    [double]$MinCommitGB = 14,
    [int]$TimeoutSecs = 900,
    [string]$LocalConfig = ''
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir '..\LinkageE3dCommon.ps1')

function Stop-With([int]$code, [string]$message) { Write-Host $message; exit $code }
function Split-List([string[]]$items) { @($items | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }

# ── -Case：按用例号从主清单和片段里取腿 ──────────────────────────────────────────
$caseSteps = @()
$caseIds = @(Split-List $Case)
if ($caseIds.Count) {
    . (Join-Path $suiteDir 'SuiteManifest.ps1')
    $merged = Read-SuiteManifest -SuiteDir $suiteDir
    try { $expanded = Expand-SuiteCaseSteps -Merged $merged -Ids $caseIds -WithSetup:$WithSetup -WithTeardown:$WithTeardown }
    catch { Stop-With 2 $_.Exception.Message }
    $caseSteps = @($expanded.steps)
    if ($PSBoundParameters.ContainsKey('Copy') -and $Copy -ne $expanded.copy) { Stop-With 2 "这些用例登记在副本 $($expanded.copy) 上，和 -Copy $Copy 不一致" }
    $Copy = $expanded.copy
    if (-not $PSBoundParameters.ContainsKey('Mdb') -and $expanded.mdb) { $Mdb = $expanded.mdb }
    if (-not $Label) { $Label = $caseIds[0] }
}
elseif ($WithSetup -or $WithTeardown) { Stop-With 2 '-WithSetup / -WithTeardown 要和 -Case 一起用' }

# ── 本机配置与私有副本 ────────────────────────────────────────────────────────────
if (-not $LocalConfig) { $LocalConfig = Join-Path $repo 'output\increment-suite\local.json' }
if (-not (Test-Path -LiteralPath $LocalConfig)) {
    Stop-With 7 "没有本机配置 $LocalConfig（它指出私有副本、E3D 安装目录和现成程序在哪；由准备私有副本的人生成，不进仓库）"
}
$cfg = Get-Content -LiteralPath $LocalConfig -Raw -Encoding UTF8 | ConvertFrom-Json
$copyCfg = $cfg.copies.PSObject.Properties[$Copy]
if (-not $copyCfg) { Stop-With 7 "本机配置里没有副本 '$Copy'（有：$(@($cfg.copies.PSObject.Properties.Name) -join '、')）" }
$copyCfg = $copyCfg.Value
$root = ([string]$copyCfg.root).TrimEnd('\')
$markerPath = Join-Path $root '.private-copy.json'
if (-not (Test-Path -LiteralPath $markerPath)) { Stop-With 7 "不是私有副本（没有标记 .private-copy.json）：$root" }
$marker = Get-Content -LiteralPath $markerPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([string]$marker.round -ne [string]$cfg.round) { Stop-With 7 "标记的轮次 $($marker.round) 与本机配置的 $($cfg.round) 不符：$root" }
if (([string]$marker.root).TrimEnd('\') -ine $root) { Stop-With 7 "标记里记的路径不是这个目录（标记是从别处拷来的？）：$root" }
if ([string]$marker.key -ne $Copy) { Stop-With 7 "标记的副本名 $($marker.key) 不是 $Copy：$root" }
$projectsHome = (Join-Path ([string]$cfg.workspace) 'project').TrimEnd('\') + '\'
if (-not ($root + '\').StartsWith($projectsHome, [StringComparison]::OrdinalIgnoreCase)) { Stop-With 7 "私有副本不在这一轮工作区的 project 目录下：$root" }
$projectName = [string]$marker.project_name
$projectCode = [string]$marker.project_code
$projectDir = Join-Path $root $projectName
$evarBats = @((Join-Path $root 'custom_evars.bat'), (Join-Path $projectDir "evars$projectName.bat"))
$evarBats += @(Get-ChildItem -LiteralPath $root -Directory | ForEach-Object { Get-ChildItem -LiteralPath $_.FullName -File -Filter 'evars*.bat' } | ForEach-Object FullName)
foreach ($bat in ($evarBats | Select-Object -Unique)) {
    if (-not (Test-Path -LiteralPath $bat)) { Stop-With 7 "私有副本不完整，缺 $bat" }
    $text = [IO.File]::ReadAllText($bat)
    if ($text -match '(?i)%projects_dir%') { Stop-With 7 "环境变量脚本里还有 %projects_dir%，没有只指向私有副本：$bat" }
    # 盘符路径只许指到这一轮 project 目录下另一份带标记的私有副本（两份副本共用一份目录库时是这样）
    foreach ($hit in [regex]::Matches($text, '(?im)^\s*(?:set|call)\b[^\r\n]*?([a-z]:\\[^"\r\n]*)')) {
        $abs = $hit.Groups[1].Value
        $inside = $abs.StartsWith($projectsHome, [StringComparison]::OrdinalIgnoreCase)
        $other = if ($inside) { Join-Path ($projectsHome + ($abs.Substring($projectsHome.Length) -split '\\')[0]) '.private-copy.json' } else { '' }
        $sameRound = $inside -and (Test-Path -LiteralPath $other) -and ([string](Get-Content -LiteralPath $other -Raw -Encoding UTF8 | ConvertFrom-Json).round -eq [string]$cfg.round)
        if (-not $sameRound) { Stop-With 7 "环境变量脚本指到了这一轮私有副本之外：$bat" }
    }
}
$reparse = Get-ChildItem -LiteralPath $root -Recurse -Depth 2 -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue | Select-Object -First 1
if ($reparse) { Stop-With 7 "私有副本里有 junction / 符号链接：$($reparse.FullName)" }
if (-not $Mdb) { $Mdb = [string]$copyCfg.mdb }
if (-not $Anchor) { $Anchor = [string]$copyCfg.anchor }
if (-not $Dbnum -and $copyCfg.PSObject.Properties['default_dbnum']) { $Dbnum = [int]$copyCfg.default_dbnum }
# 副本没有主库（如 APS：各用例写各自的库）时，写步必须用 @库号 点名
$defaultTargets = if ($Dbnum) { @("$Dbnum") } else { @() }
$dbFiles = @{}
foreach ($p in $copyCfg.dbs.PSObject.Properties) { $dbFiles[[string]$p.Name] = Join-Path $projectDir ([string]$p.Value) }
function Get-DbFile([string]$num) {
    if (-not $dbFiles.ContainsKey($num)) { Stop-With 2 "本机配置没有登记库 $num 的文件（副本 $Copy 登记了：$(@($dbFiles.Keys | Sort-Object) -join '、')）" }
    $file = $dbFiles[$num]
    if (-not (Test-Path -LiteralPath $file)) { Stop-With 7 "私有副本里没有库文件：$file" }
    $names = @(fsutil hardlink list $file 2>$null | Where-Object { $_ })
    if ($names.Count -gt 1) { Stop-With 7 "库文件是硬链接（$($names.Count) 个名字），E3D 会写穿：$file" }
    return $file
}
$l3Exe = [string]$cfg.l3_exe
$fixtureExe = [string]$cfg.fixture_exe
$installDir = [string]$cfg.install_dir
$launcher = if ($cfg.PSObject.Properties['launcher'] -and $cfg.launcher) { [string]$cfg.launcher } else { Join-Path $repo 'scripts\e3d\run_ams_c_entrymacro.bat' }
$rvmVerifyExe = ''
if ($cfg.PSObject.Properties['rvm_verify_exe'] -and $cfg.rvm_verify_exe -and (Test-Path -LiteralPath ([string]$cfg.rvm_verify_exe))) { $rvmVerifyExe = [string]$cfg.rvm_verify_exe }
foreach ($need in @($l3Exe, $fixtureExe, $launcher, (Join-Path $installDir 'des.exe'))) {
    if (-not (Test-Path -LiteralPath $need)) { Stop-With 7 "缺少前置：$need（本脚本不编译）" }
}
$ready = Join-Path ([string]$cfg.workspace) 'READY-run.md'
if ($Who -ne 'T0' -and -not $CheckOnly -and -not (Test-Path -LiteralPath $ready)) { Stop-With 7 "READY-run.md 还没发布：现在只有 T0 开 E3D（-CheckOnly 只审不跑，随时可以用）" }
if ($Who -notmatch '^[A-Za-z0-9_-]+$') { Stop-With 2 '-Who 只用字母、数字、下划线、短横' }
# 进别的模块（如 Paragon）：模块开关接在 MDB 后面交给 des.exe。只认 `-module=<号> -modulename=<名>` 这一种写法。
if ($ModuleArgs -and $ModuleArgs -notmatch '^-module=\d+\s+-modulename=[A-Za-z]+$') { Stop-With 2 "-ModuleArgs 要写成 '-module=<号> -modulename=<名>'（如 '-module=81 -modulename=Paragon'）" }

# ── 展开各步、渲染、静态审 ───────────────────────────────────────────────────────
$steps = @($caseSteps) + @(Split-List $Macro)
if (-not $steps.Count) { Stop-With 2 '要给 -Macro（步）或 -Case（用例号）' }
$vars = [ordered]@{}
foreach ($pair in (Split-List $Var)) {
    $at = $pair.IndexOf('=')
    if ($at -lt 1) { Stop-With 2 "-Var 要写成 NAME=值：$pair" }
    $vars[$pair.Substring(0, $at)] = $pair.Substring($at + 1)
}
if (-not $Label) {
    # 先算好再传给方法：方法调用的括号里，-replace 的逗号会被当成「第二个实参」（requests\T2-04）
    $firstStep = ($steps[0] -replace '^export(np)?:', 'export') -replace '@.*$', ''
    $Label = [IO.Path]::GetFileNameWithoutExtension($firstStep) -replace '[^A-Za-z0-9_-]', '-'
}
if ($Label -notmatch '^[A-Za-z0-9_-]+$') { Stop-With 2 '-Label 只用字母、数字、下划线、短横' }
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$runsDir = [string]$cfg.runs_dir
$evidence = Join-Path $runsDir "$Who\$stamp-$Label"
if ($evidence -match '\s') { Stop-With 2 "证据目录路径里有空格，启动脚本不支持：$evidence" }
New-Item -ItemType Directory -Force -Path $evidence | Out-Null

function Resolve-MacroPath([string]$spec) {
    if ([IO.Path]::IsPathRooted($spec)) { return $spec }
    $inSuite = Join-Path $suiteDir ($spec -replace '/', '\')
    if (Test-Path -LiteralPath $inSuite) { return $inSuite }
    return (Join-Path (Get-Location).Path $spec)
}

$plan = New-Object System.Collections.Generic.List[object]
$problems = New-Object System.Collections.Generic.List[string]
$n = 0
foreach ($spec in $steps) {
    $n++
    $prefix = '{0:d2}' -f $n
    if ($spec -match '^(export|exportnp):(.+)$') {
        $exportMode = $Matches[1]
        $element = $Matches[2]
        $name = "$prefix-export"
        $macroOut = Join-Path $evidence "$name.mac"
        $logOut = Join-Path $evidence "$name.log"
        $rvmOut = Join-Path $evidence "$name.rvm"
        New-RvmExportMacro -Path $macroOut -Element $element -RvmFile $rvmOut -LogFile $logOut -Note "suite export $element ($Who $Label step $n)" -QueryRef -NoPosQuery:($exportMode -eq 'exportnp')
        # 同一次会话里顺手把这棵子树的「名字 → refno」倒进日志：窄口径 RVM 不带元素身份，导成快照时要靠它
        # （没有它，rvm_verify 会把给的根 refno 钉到「一路只有一个子组」的最深那一层上——设备下只有一只命名
        # 基本体时就钉错了）。导出宏加 ONERROR CONTINUE：倒名字那几句万一报错，留一行报错，不把会话弄崩。
        $exportText = [IO.File]::ReadAllText($macroOut)
        $dump = @(
            '$P SUITE-ATT-DUMP-BEGIN'
            $element
            'VAR !suiteAttAll COLLECT ALL FOR CE'
            'DO !suiteAttE VALUES !suiteAttAll'
            '  !!CE = !suiteAttE.dbref()'
            '  Q CE'
            '  Q REF'
            'ENDDO'
            '$P SUITE-ATT-DUMP-END'
            ''
        ) -join "`n"
        $doneLine = '$P LINKAGE-RVM-EXPORT-DONE'
        if ($exportText.Contains($doneLine)) {
            $exportText = "ONERROR CONTINUE`n" + $exportText.Replace($doneLine, $dump + $doneLine)
            Write-Utf8NoBom $macroOut $exportText
        }
        $plan.Add([pscustomobject]@{ index = $n; spec = $spec; kind = 'export'; macro = $macroOut; log = $logOut; marker = 'LINKAGE-RVM-EXPORT-DONE'; dbs = $defaultTargets; rvm = $rvmOut; element = $element })
        continue
    }
    $targets = $defaultTargets
    $path = $spec
    if ($spec -match '^(.+)@([0-9+]+)$') { $path = $Matches[1]; $targets = @($Matches[2] -split '\+' | Where-Object { $_ }) }
    $template = Resolve-MacroPath $path
    if (-not (Test-Path -LiteralPath $template -PathType Leaf)) { $problems.Add("第 $n 步：宏不存在 $path"); continue }
    $stem = [IO.Path]::GetFileNameWithoutExtension($template) -replace '[^A-Za-z0-9_-]', '-'
    $name = "$prefix-$stem"
    $macroOut = Join-Path $evidence "$name.mac"
    $logOut = Join-Path $evidence "$name.log"
    $text = Get-Content -LiteralPath $template -Raw -Encoding UTF8
    if (-not $Anchor -and $text.Contains('{{WORLD_ANCHOR}}')) { $problems.Add("第 $n 步 ${path}：宏里用了 {{WORLD_ANCHOR}}，但副本 $Copy 没有登记世界锚点（用 -Anchor 给一个生产元素的名字，或者在宏里按名字导航）") }
    $text = $text.Replace('{{LOG_PATH}}', ($logOut -replace '\\', '/')).Replace('{{WORLD_ANCHOR}}', "$Anchor`nOWNER")
    foreach ($k in $vars.Keys) { $text = $text.Replace('{{' + $k + '}}', [string]$vars[$k]) }
    $left = @($text -split "`n" | Where-Object { $_ -match '\{\{' })
    if ($left.Count) { $problems.Add("第 $n 步 ${path}：还有没渲染的占位符（用 -Var 给）：$(($left | ForEach-Object { $_.Trim() }) -join ' | ')") }
    $bare = @($text -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('--') })
    $saves = @($bare | Where-Object { $_ -match '^(SAVEWORK\b|SAVE\s+WORK\b)' }).Count
    if ($DryRun) {
        $text = [regex]::Replace($text, '(?im)^[ \t]*(SAVEWORK\b|SAVE[ \t]+WORK\b).*$', '$$P DRYRUN-SAVEWORK-SKIPPED')
        if ($text -notmatch '(?im)^[ \t]*ONERROR[ \t]+CONTINUE\b') { $text = "ONERROR CONTINUE`n" + $text }
    }
    if (@($bare | Where-Object { $_ -match '^(QUIT|FINISH)\b' }).Count) { $problems.Add("第 $n 步 ${path}：不得自带 QUIT / FINISH，会话由驱动器收尾") }
    if (-not $AllowSessionRewrite -and @($bare | Where-Object { $_ -match '^(MERGE|PURGE|COMPACT)\b' }).Count) { $problems.Add("第 $n 步 ${path}：含 MERGE / PURGE / COMPACT，会改写会话链（确实要测就加 -AllowSessionRewrite，并先在 requests 里说一声）") }
    if ($saves -gt 1 -and -not $AllowMultiSave) { $problems.Add("第 $n 步 ${path}：SAVEWORK 有 $saves 句；一份宏一次存盘（要多次就拆成几份，或加 -AllowMultiSave）") }
    $opens = @($bare | Where-Object { $_ -match '^ALPHA\s+LOG\s+"' }).Count
    $closes = @($bare | Where-Object { $_ -match '^ALPHA\s+LOG\s+END\b' }).Count
    if ($opens -lt 1 -or $opens -ne $closes) { $problems.Add("第 $n 步 ${path}：ALPHA LOG 没成对（开 $opens 关 $closes）；不闭合整段日志不落盘") }
    $doneMarker = $null
    foreach ($line in $bare) { if ($line -match '^\$P\s+(\S+-DONE)\s*$') { $doneMarker = $Matches[1] } }
    if (-not $doneMarker -and -not $NoDoneMarker) { $problems.Add("第 $n 步 ${path}：没有 `$P ...-DONE 结束标记，判不了宏有没有跑完（确实没有就加 -NoDoneMarker）") }
    $nonAscii = @($bare | Where-Object { $_ -match '[^\x00-\x7F]' }).Count
    if ($nonAscii) { Write-Host "提醒：第 $n 步 $path 的非注释行里有 $nonAscii 行带非 ASCII 字符；E3D 怎么读它的编码没有验证过" }
    Write-Utf8NoBom $macroOut (($text -replace "`r`n", "`n").TrimEnd() + "`n")
    $kind = if ($saves -gt 0 -and -not $DryRun) { 'write' } else { 'probe' }
    if ($kind -eq 'write' -and -not $targets.Count) { $problems.Add("第 $n 步 ${path}：这是写步，但没说 SAVEWORK 落在哪个库；副本 $Copy 没有缺省主库，步要写成 宏.mac@库号") }
    $plan.Add([pscustomobject]@{ index = $n; spec = $spec; kind = $kind; macro = $macroOut; log = $logOut; marker = $doneMarker; dbs = $targets; rvm = $null; element = $null })
}
if ($problems.Count) {
    $problems | ForEach-Object { Write-Host "静态审不过：$_" }
    Stop-With 2 "整批不跑（$($problems.Count) 处）。渲染出来的宏在 $evidence"
}
$watch = @{}
foreach ($k in $dbFiles.Keys) { $watch[$k] = Get-DbFile $k }
foreach ($p in $plan) { foreach ($d in $p.dbs) { [void](Get-DbFile $d) } }
Write-Host "副本 $Copy = $root（项目 $projectCode，MDB $Mdb）；证据 $evidence"
foreach ($p in $plan) { Write-Host ('  {0,2}. {1,-6} 库 {2,-10} {3}' -f $p.index, $p.kind, ($p.dbs -join '+'), $p.spec) }
if ($CheckOnly) { Stop-With 0 "-CheckOnly：静态审过了，没有开 E3D。渲染出来的宏在 $evidence" }

# ── 等到能开会话 ─────────────────────────────────────────────────────────────────
$lock = Join-Path $repo 'output\.e3d-session.lock'
$swapLock = Join-Path $repo 'output\ops\swap.lock'
$e3dNames = @('des', 'mon', 'adm', 'pdmsconsole')
$minDiskGb = if ($cfg.PSObject.Properties['min_free_disk_gb']) { [double]$cfg.min_free_disk_gb } else { 10 }
$drive = (Get-Item -LiteralPath $root).PSDrive.Name
function Get-CommitFreeGb { $m = Get-CimInstance Win32_PerfFormattedData_PerfOS_Memory; return [math]::Round(($m.CommitLimit - $m.CommittedBytes) / 1GB, 1) }
function Get-E3dProcesses { @(Get-Process -Name $e3dNames -ErrorAction SilentlyContinue) }
# 排队票：经本入口排队的人先来先得。票在拿到锁或者放弃时删掉；主人进程不在了的票，后面的人当废票清掉。
$queueDir = Join-Path $repo 'output\.e3d-session.queue'
[void](New-Item -ItemType Directory -Force -Path $queueDir)
# 同一批（同一个人、同一个标签、步完全相同）已经在排队或在跑时，后来的这一次直接退出，不再排一遍：
# 工具层偶尔会把同一条命令执行两遍，带相对位移的写步重复一遍会把库弄歪（requests\T1-02）。
$batchKey = '{0}|{1}|{2}|{3}|{4}' -f $Who, $Label, $Copy, $Mdb, ($steps -join ',')
$batchHash = [BitConverter]::ToString([Security.Cryptography.SHA1]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($batchKey))).Replace('-', '').Substring(0, 16)
foreach ($other in (Get-ChildItem -LiteralPath $queueDir -Filter '*.ticket' -File -ErrorAction SilentlyContinue)) {
    $text = try { [IO.File]::ReadAllText($other.FullName) } catch { '' }
    $m = [regex]::Match($other.Name, '-(\d+)\.ticket$')
    if ($text -match "batch=$batchHash\b" -and $m.Success -and (Get-Process -Id ([int]$m.Groups[1].Value) -ErrorAction SilentlyContinue)) { Stop-With 8 "同一批（$Who / $Label，步完全相同）已经在排队了（进程 $($m.Groups[1].Value)），这一次不再排" }
}
if (Test-Path -LiteralPath $lock) {
    $text = try { [IO.File]::ReadAllText($lock) } catch { '' }
    $owner = [regex]::Match($text, 'pid:\s*(\d+)')
    if ($text -match "batch: $batchHash\b" -and $owner.Success -and (Get-Process -Id ([int]$owner.Groups[1].Value) -ErrorAction SilentlyContinue)) { Stop-With 8 "同一批（$Who / $Label，步完全相同）正在跑（进程 $($owner.Groups[1].Value)），这一次不再排" }
}
$ticket = Join-Path $queueDir ('{0:yyyyMMddHHmmssfff}-{1}-{2}.ticket' -f (Get-Date), $Who, $PID)
[IO.File]::WriteAllText($ticket, "who=$Who pid=$PID label=$Label steps=$($plan.Count) batch=$batchHash")
function Get-AheadOfMe {
    $mine = Split-Path -Leaf $ticket
    foreach ($t in (Get-ChildItem -LiteralPath $queueDir -Filter '*.ticket' -File -ErrorAction SilentlyContinue | Sort-Object Name)) {
        if ($t.Name -eq $mine) { break }
        $m = [regex]::Match($t.Name, '-(\d+)\.ticket$')
        $alive = $m.Success -and (Get-Process -Id ([int]$m.Groups[1].Value) -ErrorAction SilentlyContinue)
        if (-not $alive) { Remove-Item -LiteralPath $t.FullName -Force -ErrorAction SilentlyContinue; continue }
        return ($t.Name -replace '^\d+-', '' -replace '-\d+\.ticket$', '')
    }
    return $null
}
$waitStart = Get-Date
$waited = 0.0
$lastSay = [datetime]::MinValue
try {
while ($true) {
    $why = @()
    if (Test-Path -LiteralPath $swapLock) { $why += 'swap.lock 在' }
    if (Test-Path -LiteralPath $lock) {
        $text = try { [IO.File]::ReadAllText($lock) } catch { '' }
        $owner = [regex]::Match($text, 'pid:\s*(\d+)')
        $mine = $text -match [regex]::Escape("round: $($cfg.round)")
        $dead = $owner.Success -and -not (Get-Process -Id ([int]$owner.Groups[1].Value) -ErrorAction SilentlyContinue)
        if ($mine -and $dead -and -not (Get-E3dProcesses).Count) {
            Write-Host "会话锁是这一轮留下的死锁（进程 $($owner.Groups[1].Value) 已不在、没有 E3D 进程），清掉"
            Remove-Item -LiteralPath $lock -Force -ErrorAction SilentlyContinue
        }
        else { $why += '会话锁在（' + (($text -split "`n" | Select-Object -First 3 | ForEach-Object { $_.Trim() }) -join '；') + '）' }
    }
    $running = Get-E3dProcesses
    if ($running.Count) { $why += '已有 E3D 进程（' + (($running | ForEach-Object { $_.Name + ':' + $_.Id }) -join '、') + '）' }
    $free = Get-CommitFreeGb
    if ($free -lt $MinCommitGB) { $why += "可提交余量 $free GB，不到 $MinCommitGB GB" }
    $disk = [math]::Round((Get-PSDrive $drive).Free / 1GB, 1)
    if ($disk -lt $minDiskGb) { $why += "$drive 盘余 $disk GB，不到 $minDiskGb GB" }
    if (-not $why.Count) { $ahead = Get-AheadOfMe; if ($ahead) { $why += "排在 $ahead 后面" } }
    if (-not $why.Count) {
        try {
            $fs = [IO.File]::Open($lock, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
            try {
                $bytes = [Text.Encoding]::UTF8.GetBytes((@(
                            "who:  $Who（增量套件 Invoke-SuiteMacro）"
                            "when: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
                            "what: $Label —— $($plan.Count) 步，私有副本 $Copy，项目 $projectCode，MDB $Mdb"
                            "pid:  $PID"
                            "round: $($cfg.round)"
                            "batch: $batchHash"
                        ) -join "`n"))
                $fs.Write($bytes, 0, $bytes.Length)
            }
            finally { $fs.Dispose() }
            break
        }
        catch [IO.IOException] { $why += '会话锁刚被别人拿走' }
    }
    $waited = [math]::Round(((Get-Date) - $waitStart).TotalMinutes, 1)
    if ($waited -ge $WaitMinutes) { Remove-Item -LiteralPath $ticket -Force -ErrorAction SilentlyContinue; Stop-With 6 "等了 $([math]::Round($waited)) 分钟还开不了（$($why -join '；')），这一批没跑" }
    if (((Get-Date) - $lastSay).TotalSeconds -ge 120) { Write-Host ("[{0}] 等：{1}" -f (Get-Date -Format 'HH:mm:ss'), ($why -join '；')); $lastSay = Get-Date }
    # 5 秒看一次：锁一放开就接上，机器不空转
    Start-Sleep -Seconds 5
}
}
finally { Remove-Item -LiteralPath $ticket -Force -ErrorAction SilentlyContinue }

# ── 跑 ───────────────────────────────────────────────────────────────────────────
function Get-Listing([string]$dir) {
    $h = [ordered]@{}
    if (-not $dir -or -not (Test-Path -LiteralPath $dir)) { return $h }
    foreach ($f in Get-ChildItem -LiteralPath $dir -Recurse -Force -File | Sort-Object FullName) {
        $h[$f.FullName.Substring($dir.TrimEnd('\').Length + 1)] = @($f.Length, $f.LastWriteTimeUtc.ToString('yyyy-MM-ddTHH:mm:ss.fffffff'))
    }
    return $h
}
function Save-Listing($h, [string]$path) { $h.GetEnumerator() | ForEach-Object { "{0}`t{1}`t{2}" -f $_.Key, $_.Value[0], $_.Value[1] } | Set-Content -LiteralPath $path -Encoding utf8 }
function Get-Sesno([string]$file) {
    try {
        $json = & $fixtureExe inspect --source $file 2>$null
        if ($LASTEXITCODE) { return $null }
        return [int](($json -join "`n") | ConvertFrom-Json).latest_sesno
    }
    catch { return $null }
}
function Get-AllSesno { $h = [ordered]@{}; foreach ($k in ($watch.Keys | Sort-Object)) { $h[$k] = Get-Sesno $watch[$k] }; return $h }

$t0 = Get-Date
$results = New-Object System.Collections.Generic.List[object]
$journal = Join-Path $runsDir 'sessions.log'
$originDir = if ($marker.PSObject.Properties['source']) { [string]$marker.source } else { '' }
$batchOk = $true
try {
    $before = Get-Listing $root
    Save-Listing $before (Join-Path $evidence 'files-before.tsv')
    $originBefore = Get-Listing $originDir
    foreach ($p in $plan) {
        $stepDir = Join-Path $evidence ('{0:d2}-driver' -f $p.index)
        New-Item -ItemType Directory -Force -Path $stepDir | Out-Null
        $console = Join-Path $stepDir 'console.log'
        $sesBefore = Get-AllSesno
        $targetFile = if (@($p.dbs).Count) { $watch[[string]@($p.dbs)[0]] } else { $null }
        $mdbArg = if ($ModuleArgs) { "$Mdb $ModuleArgs" } else { $Mdb }
        $l3Args = @('--check-driver', $p.macro, '--project-dir', $projectDir, '--e3d-project', $projectCode, '--e3d-mdb', $mdbArg, '--output', $stepDir)
        if ($p.kind -eq 'write') { $l3Args += @('--target-db-file', $targetFile, '--aios-project', $projectName) }
        $psi = [Diagnostics.ProcessStartInfo]::new($l3Exe)
        foreach ($a in $l3Args) { $psi.ArgumentList.Add($a) }
        # 驱动器把当前目录当仓根：证据（output/l3-suite/driver-*）落在这一步自己的目录里，不落进仓库
        $psi.WorkingDirectory = $stepDir
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.UseShellExecute = $false
        foreach ($name in @($psi.Environment.Keys | Where-Object { $_ -like 'L3_E3D_*' -or $_ -eq 'L3_ALLOW_EXISTING_E3D_SESSION' })) { [void]$psi.Environment.Remove($name) }
        $psi.Environment['L3_E3D_INSTALL_DIR'] = $installDir
        $psi.Environment['L3_E3D_PROJECTS_DIR'] = $root
        $psi.Environment['L3_E3D_DRIVER'] = $launcher
        $stepStart = Get-Date
        $timedOut = $false
        $out = [IO.FileStream]::new($console, [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
        $err = [IO.FileStream]::new("$console.stderr", [IO.FileMode]::Create, [IO.FileAccess]::Write, [IO.FileShare]::ReadWrite)
        try {
            $proc = [Diagnostics.Process]::Start($psi)
            $copyOut = $proc.StandardOutput.BaseStream.CopyToAsync($out)
            $copyErr = $proc.StandardError.BaseStream.CopyToAsync($err)
            while (-not $proc.HasExited) {
                Start-Sleep -Milliseconds 1000
                if (((Get-Date) - $stepStart).TotalSeconds -gt $TimeoutSecs) {
                    $timedOut = $true
                    & taskkill.exe /PID $proc.Id /T /F 2>&1 | Out-Null
                    break
                }
            }
            $proc.WaitForExit()
            [void][Threading.Tasks.Task]::WaitAll(@($copyOut, $copyErr), 15000)
            $rc = $proc.ExitCode
        }
        finally { $out.Dispose(); $err.Dispose() }
        # 会话收干净了没有：只收这一批开始之后起的进程
        $left = @()
        for ($i = 0; $i -lt 30; $i++) { $left = @(Get-E3dProcesses | Where-Object { try { $_.StartTime -ge $t0 } catch { $false } }); if (-not $left.Count) { break }; Start-Sleep -Seconds 2 }
        $killed = 0
        foreach ($proc2 in $left) { try { Stop-Process -Id $proc2.Id -Force; $killed++ } catch { } }
        $secs = [math]::Round(((Get-Date) - $stepStart).TotalSeconds)
        $sesAfter = Get-AllSesno

        $reasons = New-Object System.Collections.Generic.List[string]
        $warnings = New-Object System.Collections.Generic.List[string]
        if ($timedOut) { $reasons.Add("超过 $TimeoutSecs 秒，已收掉") }
        elseif ($rc -ne 0) { $reasons.Add("驱动器退出码 $rc") }
        # 不用「if 当表达式」赋值：日志只剩 BOM 时 Get-Content -Raw 给空串，经 if 表达式会变成 $null（requests\T2-02）
        $logText = ''
        if (Test-Path -LiteralPath $p.log) { $logText = [string](Get-Content -LiteralPath $p.log -Raw -Encoding UTF8) }
        if ($null -eq $logText) { $logText = '' }
        $driverLog = Get-ChildItem -LiteralPath $stepDir -Recurse -Filter driver.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
        $e3dExit = $null
        if ($driverLog) {
            $exitLine = Select-String -LiteralPath $driverLog.FullName -Pattern 'detached pid=\d+ exit=(\d+)' | Select-Object -Last 1
            if ($exitLine) { $e3dExit = [long]$exitLine.Matches[0].Groups[1].Value }
        }
        $crashed = $e3dExit -eq 3221225477
        # E3D 每崩一次，系统在 %LOCALAPPDATA%\CrashDumps 下留一个 des.exe.<进程号>.dmp（一百多 MB，在系统盘上）。
        # 只删这次会话那个进程号的那一个，别的不动（requests\C-02）。转储是进程收尾时才写完的，崩了就多等几秒。
        $desPids = @()
        if ($driverLog) { $desPids = @(Select-String -LiteralPath $driverLog.FullName -Pattern 'detached pid=(\d+)' -AllMatches | ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value } | Select-Object -Unique) }
        $dumpsRemoved = 0
        if ($desPids.Count -and $env:LOCALAPPDATA) {
            for ($try = 0; $try -lt $(if ($crashed) { 10 } else { 1 }); $try++) {
                $pending = $false
                foreach ($desPid in $desPids) {
                    $dump = Join-Path $env:LOCALAPPDATA "CrashDumps\des.exe.$desPid.dmp"
                    if (Test-Path -LiteralPath $dump) { try { Remove-Item -LiteralPath $dump -Force -ErrorAction Stop; $dumpsRemoved++ } catch { $pending = $true } }
                    elseif ($crashed -and -not $dumpsRemoved) { $pending = $true }
                }
                if (-not $pending) { break }
                Start-Sleep -Seconds 2
            }
        }
        if (-not $logText.Trim()) {
            if ($crashed) { $reasons.Add('E3D 崩了（0xC0000005），宏日志是空的：宏里某一句报错了。没有 ONERROR CONTINUE 时任何报错都是这个结果——用 -DryRun 看是哪一句；写步先查它导航的元素是不是被前一步删了') }
            else { $reasons.Add('宏日志是空的：宏没有走到 ALPHA LOG END（看这一步 driver 目录里的 driver.log）') }
        }
        # 宏走完之后 E3D 退出时再崩不算数（这份安装常在进程收尾时以 0xC0000005 退出）：只在宏没走完时把它写进原因
        $markerSeen = $p.marker -and $logText.Contains([string]$p.marker)
        if ($p.marker -and -not $markerSeen) { $reasons.Add("宏日志里没有结束标记 $($p.marker)（宏中途停了$(if ($crashed -and $logText.Trim()) { '，E3D 以 0xC0000005 退出' })）") }
        $errorLines = @($logText -split "`r?`n" | Where-Object { $_ -match '^\s*\(\d+,\s*\d+\)' })
        if ($errorLines.Count -and -not $AllowErrorLines) { $reasons.Add("宏日志里有 $($errorLines.Count) 行 E3D 报错，第一行：$($errorLines[0].Trim())") }
        # 名字 → refno：宏里相邻的 Q REF / Q NAME 在日志里是 `Ref =a/b` 紧跟 `Name /X`
        $refs = [ordered]@{}
        $pendingRef = $null
        foreach ($line in ($logText -split "`r?`n")) {
            $t = $line.Trim()
            if ($t -match '^Ref\s+=?(\d+/\d+)$') { $pendingRef = $Matches[1]; continue }
            if ($pendingRef -and $t -match '^Name\s+(/\S+)$') { $refs[$Matches[1]] = $pendingRef }
            $pendingRef = $null
        }
        $moved = @($sesAfter.Keys | Where-Object { $null -ne $sesBefore[$_] -and $null -ne $sesAfter[$_] -and $sesAfter[$_] -gt $sesBefore[$_] })
        if ($p.kind -eq 'write') {
            foreach ($d in $p.dbs) { if ($moved -notcontains $d) { $reasons.Add("库 $d 的会话号没前进（$($sesBefore[$d]) → $($sesAfter[$d])）：SAVEWORK 没落到它上面") } }
            $stray = @($moved | Where-Object { $p.dbs -notcontains $_ })
            if ($stray.Count) { $warnings.Add("没点名的库 $($stray -join '、') 的会话号也前进了（锚点错了会写进缺省库；一次存盘写两个库的步要写成 @库+库）") }
        }
        elseif ($DryRun -and $moved.Count) { $warnings.Add("试跑不存盘，但库 $($moved -join '、') 的会话号前进了（登录自己记的账；要是每次都这样就不是宏的事）") }
        # 这次会话开的是哪份工程：启动脚本报出的项目目录的上一级必须带着同一份标记
        $projectLine = if ($driverLog) { Select-String -LiteralPath $driverLog.FullName -Pattern '^E3D project root:\s*(.+)$' | Select-Object -Last 1 } else { $null }
        $opened = if ($projectLine) { $projectLine.Matches[0].Groups[1].Value.Trim() } else { '' }
        if (-not $opened) { $reasons.Add('启动脚本没有报出项目目录，核不了这次会话开的是哪份工程') }
        else {
            $openedMarker = Join-Path (Split-Path -Parent $opened.TrimEnd('\')) '.private-copy.json'
            $same = (Test-Path -LiteralPath $openedMarker) -and ([IO.File]::ReadAllText($openedMarker) -ceq [IO.File]::ReadAllText($markerPath))
            if (-not $same) { $reasons.Add("启动脚本开的项目目录不是私有副本：$opened") }
        }
        if ($p.kind -eq 'export' -and (-not (Test-Path -LiteralPath $p.rvm) -or (Get-Item -LiteralPath $p.rvm).Length -le 0)) { $reasons.Add('RVM 没导出来') }
        # 导出步：顺手把 RVM 导成快照 JSON（rvm_verify import，窄口径）。根的 refno 从导出日志回读，库号从 refno 算。
        # 导不成只提醒，不算这一步失败：E3D 那一侧是好的。
        $snapshot = $null
        if ($p.kind -eq 'export' -and -not $reasons.Count -and $rvmVerifyExe) {
            $rootRef = $null
            foreach ($line in ($logText -split "`r?`n")) { if ($line.Trim() -match '^Ref\s+=?(\d+/\d+)$') { $rootRef = $Matches[1]; break } }
            if (-not $rootRef) { $warnings.Add('导出日志里没有 Ref 行，快照没导') }
            else {
                $word = [uint32](($rootRef -split '/')[0])
                $rootDb = ($word -band 0x1FFF) -bor (($word -shr 13) -band 0x3E000)
                $snapPath = [IO.Path]::ChangeExtension($p.rvm, '.snapshot.json')
                # 日志里 SUITE-ATT-DUMP-BEGIN … END 之间是逐元素的 `<类型> /名字`（或无名的 `<类型> n of …`）紧跟 `Ref =a/b`：拼成 ATT
                $attPath = [IO.Path]::ChangeExtension($p.rvm, '.att')
                $att = New-Object System.Text.StringBuilder
                [void]$att.AppendLine('AVEVA_Attributes_File v1.0 , start: NEW , end: END , name_end: := , sep: &end&')
                [void]$att.AppendLine('NEW Header Information')
                [void]$att.AppendLine("Element:= $($p.element)")
                [void]$att.AppendLine('END')
                $inDump = $false; $pendingKey = $null; $pendingNoun = $null; $attRows = 0
                foreach ($line in ($logText -split "`r?`n")) {
                    $t = $line.Trim()
                    if ($t -eq 'SUITE-ATT-DUMP-BEGIN') { $inDump = $true; continue }
                    if ($t -eq 'SUITE-ATT-DUMP-END') { break }
                    if (-not $inDump) { continue }
                    if ($t -match '^(?<noun>[A-Z][A-Z0-9]*)\s+(?<name>/\S+)$') { $pendingKey = $Matches.name; $pendingNoun = $Matches.noun; continue }
                    if ($t -match '^(?<noun>[A-Z][A-Z0-9]*)\s+\d+\s+of\s+.+$') { $pendingKey = $t; $pendingNoun = $Matches.noun; continue }
                    if ($pendingKey -and $t -match '^Ref\s*=\s*(\d+/\d+)$') {
                        [void]$att.AppendLine("NEW $pendingKey"); [void]$att.AppendLine("NAME:= =$($Matches[1])"); [void]$att.AppendLine("TYPE:= $pendingNoun"); [void]$att.AppendLine('END')
                        $attRows++; $pendingKey = $null
                    }
                }
                $importArgs = @('import', '--rvm', $p.rvm, '--dbnum', "$rootDb", '--root-refno', $rootRef, '--scope', 'narrow', '--out', $snapPath)
                if ($attRows) { Write-Utf8NoBom $attPath $att.ToString(); $importArgs += @('--att', $attPath) }
                else { $warnings.Add('导出日志里没有倒出名字和 refno，快照里的成员身份多半解不出来') }
                & $rvmVerifyExe @importArgs *> ([IO.Path]::ChangeExtension($p.rvm, '.import.log'))
                if ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $snapPath)) {
                    $meta = (Get-Content -LiteralPath $snapPath -Raw -Encoding UTF8 | ConvertFrom-Json).meta
                    $snapshot = [ordered]@{ file = $snapPath; root_refno = $rootRef; dbnum = $rootDb; members = $meta.member_count; geometries = $meta.geometry_count; resolved = $meta.resolved; unresolved = $meta.unresolved; att_rows = $attRows }
                }
                else { $warnings.Add("RVM 导成快照失败（rvm_verify import 退出码 $LASTEXITCODE，看同名 .import.log）") }
            }
        }
        $verdict = if ($reasons.Count) { 'fail' } else { 'pass' }
        $sesNote = (@($sesAfter.Keys) | ForEach-Object { '{0}:{1}→{2}' -f $_, $sesBefore[$_], $sesAfter[$_] }) -join ' '
        $notes = @($reasons) + @($warnings | ForEach-Object { "提醒：$_" })
        Write-Host ('  {0,2}. {1,-4} {2,-6} {3,4}s  {4}  {5}{6}' -f $p.index, $verdict, $p.kind, $secs, $sesNote, $p.spec, $(if ($notes.Count) { "`n        " + ($notes -join "`n        ") } else { '' }))
        $results.Add([pscustomobject][ordered]@{
                index = $p.index; spec = $p.spec; kind = $p.kind; dbs = $p.dbs; verdict = $verdict; reasons = @($reasons); warnings = @($warnings)
                seconds = $secs; exit = $rc; e3d_exit = $e3dExit; crash_dumps_removed = $dumpsRemoved; timed_out = $timedOut; done_marker = [bool]$markerSeen; error_lines = $errorLines.Count
                sesno_before = $sesBefore; sesno_after = $sesAfter; refs = $refs; opened_project = $opened; killed_leftover = $killed
                macro = $p.macro; log = $p.log; rvm = $p.rvm; snapshot = $snapshot; element = $p.element; driver = $stepDir
            })
        Add-Content -LiteralPath $journal -Encoding utf8 -Value ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}`t{7}s`t{8}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Who, $Label, $p.index, $p.kind, $verdict, $sesNote, $secs, $p.spec)
        if ($verdict -ne 'pass') { $batchOk = $false; if (-not $ContinueOnFail) { Write-Host '  后面的步不跑：库里的状态已经不可信'; break } }
    }
}
finally {
    try { if ((Test-Path -LiteralPath $lock) -and ([IO.File]::ReadAllText($lock) -match "pid:\s*$PID\b")) { Remove-Item -LiteralPath $lock -Force } }
    catch { Write-Host "没删掉会话锁：$($_.Exception.Message)" }
}

# ── 收尾：副本里哪些文件变了、来源目录动没动 ─────────────────────────────────────
$after = Get-Listing $root
Save-Listing $after (Join-Path $evidence 'files-after.tsv')
$grown = @(); $touched = 0; $added = @(); $removed = @()
foreach ($k in $after.Keys) {
    if (-not $before.Contains($k)) { $added += $k }
    elseif ($before[$k][0] -ne $after[$k][0]) { $grown += ('{0}（{1} → {2} 字节）' -f $k, $before[$k][0], $after[$k][0]) }
    elseif ($before[$k][1] -ne $after[$k][1]) { $touched++ }
}
foreach ($k in $before.Keys) { if (-not $after.Contains($k)) { $removed += $k } }
$originAfter = Get-Listing $originDir
$originDiff = 0
foreach ($k in $originAfter.Keys) { if (-not $originBefore.Contains($k) -or $originBefore[$k][0] -ne $originAfter[$k][0] -or $originBefore[$k][1] -ne $originAfter[$k][1]) { $originDiff++ } }
foreach ($k in $originBefore.Keys) { if (-not $originAfter.Contains($k)) { $originDiff++ } }
$notRun = $plan.Count - $results.Count
$summary = [ordered]@{
    who = $Who; label = $Label; copy = $Copy; root = $root; project = $projectCode; mdb = $Mdb; dry_run = [bool]$DryRun
    started = $t0.ToString('yyyy-MM-dd HH:mm:ss'); waited_minutes = $waited; seconds = [math]::Round(((Get-Date) - $t0).TotalSeconds)
    verdict = $(if ($batchOk -and -not $notRun) { 'pass' } else { 'fail' })
    steps_planned = $plan.Count; steps_run = $results.Count
    steps = @($results.ToArray())
    copy_files_resized = @($grown); copy_files_added = @($added); copy_files_removed = @($removed); copy_files_touched_only = $touched
    origin_listing_changes = $originDiff
    lock_released = -not (Test-Path -LiteralPath $lock)
    leftover_e3d = @(Get-E3dProcesses | ForEach-Object { $_.Name + ':' + $_.Id })
}
Write-Utf8NoBom (Join-Path $evidence 'result.json') ((ConvertTo-Json -InputObject $summary -Depth 8) + "`n")
Write-Host ("{0}：{1} / {2} 步跑了，{3}；副本里大小变了 {4} 个文件、只动文件头 {5} 个；锁已放 = {6}；证据 {7}" -f $summary.verdict, $results.Count, $plan.Count, $(if ($batchOk) { '都过了' } else { '有没过的' }), $grown.Count, $touched, $summary.lock_released, $evidence)
if ($originDiff) { Write-Host "注意：来源目录的文件清单在这一批期间有 $originDiff 处不同（不一定是这一批造成的，但要查）：$originDir" }
exit $(if ($summary.verdict -eq 'pass') { 0 } else { 1 })
