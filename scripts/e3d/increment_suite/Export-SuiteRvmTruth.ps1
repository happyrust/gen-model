<#
.SYNOPSIS
    给 PML 增量套件（scripts/e3d/increment_suite）导 RVM 真值：每条写腿（恰好一个 SAVEWORK 的宏）跑完后，
    整棵 /AIOS-SUITE-8000 只读导出一份 RVM，rvm_verify import 成 rvm-truth/<sesno>.json，并按 rvm-truth.json
    登记的预期与「它跑之前那份快照」对拍（Compare-SuiteRvmTruth.ps1）。

.DESCRIPTION
    三种模式：
      -Mode plan   不碰 E3D、不碰库：只读 suite-manifest.json + rvm-truth.json，打印将要跑的腿与导出顺序（强制链、
                   数组链、被 blocked 的用例都按最终顺序列出来），用来核对清单。
      -Mode state  只导「现在这一刻」的真值（要导的状态已经在库里，比如 Lead 刚手工跑完某条腿）：写
                   <TruthDir>/<sesno>.json 并登记 index.json；-Label 给这份快照起名（如 A02-box-add-apply）。
      -Mode batch  按 suite-manifest.json 的顺序逐用例 apply → 导出 → 对拍 → restore → 导出 → 对拍；
                   `chain` 为字符串的用例整条强制链只跑一次，`chain` 为数组的用例按数组顺序跑；
                   rvm-truth.json 里 blocked 的用例跳过（7998 相关）；-Case 子串过滤；-RunSetup 先跑
                   common/setup_8000.mac 再导基线（基线必须在任何用例之前导，否则 restore 腿没有可比对象）。

    真值的键是**写腿 SAVEWORK 落地后、导出登录之前**读到的 latest_sesno（db_session_fixture inspect）：只读导出那次
    登录本身也会在库里追加一个只含 DBMDAT 的尾会话（K 组 §7「现场腿的会话数」），所以导出之后再读会多 1，
    index.json 两个值都记。

    与 Export-ReplayRvmTruth.ps1（K 组回放链）同一套只读导出口径与导入口径，差别只有三处：
      * 元素按名字导（/AIOS-SUITE-8000），根 refno 不写死——SITE 每次 setup 都删了重建，从导出日志的 `Ref =` 回读；
      * 不 replay-init：真值跟着现场录制走，录成 L2 夹具后把 rvm-truth/ 整目录搬到夹具目录旁即可；
      * 每份真值落地就与上一份对拍，录制当场发现真值不自洽。

    E3D 只许在与原件完全断开的隔离工程副本上跑（Assert-IsolatedProjects 先核：不是原件、无 junction、库文件无硬链接）。
    本脚本不做 cargo build：缺 db_session_fixture.exe / l3_suite.exe 直接报错；rvm_verify.exe 缺则 Import-RvmSnapshot
    退回 cargo run（LinkageE3dCommon.ps1 的既有行为）。

.EXAMPLE
    # 只看顺序
    pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode plan
.EXAMPLE
    # 整套：setup → 基线 → 34 个用例逐腿导出并对拍
    pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode batch -RunSetup -IsolatedProjectsDir E:\reverse\e3d\replay-src\E3D3.1
.EXAMPLE
    # 只跑 xform 组
    pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode batch -Case C01 -IsolatedProjectsDir E:\reverse\e3d\replay-src\E3D3.1
.EXAMPLE
    # 手工跑完某条腿后补一份真值
    pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode state -Label A02-box-add-apply -IsolatedProjectsDir E:\reverse\e3d\replay-src\E3D3.1 -TruthDir output\increment-suite-rvm-truth\20261028-1500\rvm-truth
#>
[CmdletBinding()]
param(
    [ValidateSet('plan', 'state', 'batch')][string]$Mode = 'plan',
    # 隔离工程副本根（里面有 AvevaMarineSample\ams000\...）；plan 模式不需要
    [string]$IsolatedProjectsDir = '',
    [string]$Label = 'state',
    # 用例 id 子串过滤（与 .scratch/tty_batch.py 同口径：子串命中即跑），可多个
    [string[]]$Case = @(),
    [switch]$RunSetup,
    # 某条腿失败（E3D 报错 / sesno 没前进）后继续跑下一个用例；默认停，因为库里的状态已经不可信
    [switch]$ContinueOnError,
    [switch]$SkipCompare,
    # 真值目录；默认 output\increment-suite-rvm-truth\<时间戳>\rvm-truth。batch 续跑 / state 补导时指向已有目录即可
    [string]$TruthDir = '',
    [string]$Element = '/AIOS-SUITE-8000',
    # 把 CE 挪进 db8000 的名称导航（{{WORLD_ANCHOR}} 展开成「锚点 + OWNER」两行，README「跑之前必须先读这一条」）
    [string]$Anchor = '/1RX03-EQUI',
    [int]$Dbnum = 8000,
    [string]$DbFile = 'AvevaMarineSample\ams000\ams8000_0001',
    [string]$Source = 'D:\AVEVA\Projects\E3D3.1',
    [string]$MainProject = 'AvevaMarineSample',
    [string]$TargetDir = 'D:\Rust\target',
    # ★ 默认 debug：release 下**没有** rvm_verify.exe（实测 2026-10-28：D:\Rust\target\release 只有
    #   l3_suite.exe 和 db_session_fixture.exe），选 release 会让 rvm_verify import 这一步直接失败。
    [ValidateSet('release', 'debug')][string]$BuildProfile = 'debug',
    [string]$L3Exe = 'D:\Rust\target\debug\l3_suite.exe',
    [string]$E3dInstallDir = 'E:\reverse\e3d\shadow_e3d31_gen_model_test'
)

$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir '..\LinkageE3dCommon.ps1')

$manifest = Get-Content -LiteralPath (Join-Path $suiteDir 'suite-manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$modelPath = Join-Path $suiteDir 'rvm-truth.json'
$truthModel = Get-Content -LiteralPath $modelPath -Raw -Encoding UTF8 | ConvertFrom-Json
$compareScript = Join-Path $suiteDir 'Compare-SuiteRvmTruth.ps1'

function Get-LegExpectation([string]$MacroRel) {
    $p = $truthModel.legs.PSObject.Properties[$MacroRel]
    if ($p) { return $p.Value }
    return $null
}

function Get-BlockedReason([string]$MacroRel) {
    $e = Get-LegExpectation $MacroRel
    if ($e -and $e.PSObject.Properties['blocked'] -and $e.blocked) { return [string]$e.blocked }
    return $null
}

# ── 计划：把 manifest 展开成按顺序的写腿列表 ────────────────────────────────────
function New-LegPlan {
    $plan = New-Object System.Collections.Generic.List[object]
    if ($RunSetup) {
        $plan.Add([pscustomobject]@{ case = 'setup'; leg = 'setup'; macro = 'common/setup_8000.mac'; action = 'run' })
    }
    $plan.Add([pscustomobject]@{ case = 'baseline'; leg = 'baseline'; macro = $null; action = 'export' })
    # run_last：apply 宏在这个名单里的用例挪到最后（cross_db_copy_restore 之后 /AIOS-SUITE-BOX-A1 就不存在了，
    # 导航到不存在的元素会让 E3D 访问违例崩溃——README「db7998 的锚点」下一节）
    $runLast = @()
    if ($truthModel.PSObject.Properties['run_last']) { $runLast = @($truthModel.run_last | ForEach-Object { [string]$_ }) }
    $deferred = New-Object System.Collections.Generic.List[object]
    $doneChains = New-Object System.Collections.Generic.HashSet[string]
    # pwsh -File 下 "-Case A02,C01" 会整个当成一个字符串传进来（K 组 §4 记过 -Steps 的同一坑），这里自己按逗号拆
    $filters = @($Case | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    foreach ($c in @($manifest.cases)) {
        $cid = [string]$c.id
        if ($filters.Count -gt 0) {
            $hit = $false
            foreach ($f in $filters) { if ($cid -like "*$f*") { $hit = $true } }
            if (-not $hit) { continue }
        }
        if ($c.PSObject.Properties['companion_dbnum'] -and $c.companion_dbnum) {
            $plan.Add([pscustomobject]@{ case = $cid; leg = '-'; macro = [string]$c.apply; action = 'skip'; reason = "伙伴库 $($c.companion_dbnum) 的用例：batch 只导主库 $Dbnum；跑完它的腿后用 -Mode state -Element /AIOS-SUITE-$($c.companion_dbnum) -Dbnum $($c.companion_dbnum) -DbFile <伙伴库文件> 单独补导" })
            continue
        }
        $target = $plan
        if ($runLast -contains [string]$c.apply) { $target = $deferred }
        $blocked = Get-BlockedReason ([string]$c.apply)
        if ($blocked) {
            $plan.Add([pscustomobject]@{ case = $cid; leg = '-'; macro = [string]$c.apply; action = 'skip'; reason = $blocked })
            continue
        }
        $chain = $null
        if ($c.PSObject.Properties['chain']) { $chain = $c.chain }
        if ($chain -is [string]) {
            if ($doneChains.Contains($chain)) {
                $plan.Add([pscustomobject]@{ case = $cid; leg = '-'; macro = [string]$c.apply; action = 'skip'; reason = "已随强制链 $chain 跑过" })
                continue
            }
            $def = @($manifest.chains | Where-Object { $_.id -eq $chain }) | Select-Object -First 1
            if (-not $def) { throw "用例 $cid 引用的强制链 $chain 不在 manifest.chains 里" }
            [void]$doneChains.Add($chain)
            $n = 0
            foreach ($m in @($def.legs)) {
                $n++
                $target.Add([pscustomobject]@{ case = "chain:$chain"; leg = "$n"; macro = [string]$m; action = 'run' })
            }
            continue
        }
        if ($chain -is [System.Array]) {
            $n = 0
            foreach ($m in @($chain)) {
                $n++
                $target.Add([pscustomobject]@{ case = $cid; leg = "$n"; macro = [string]$m; action = 'run' })
            }
            continue
        }
        $target.Add([pscustomobject]@{ case = $cid; leg = 'apply'; macro = [string]$c.apply; action = 'run' })
        if ($c.restore) {
            $target.Add([pscustomobject]@{ case = $cid; leg = 'restore'; macro = [string]$c.restore; action = 'run' })
        }
    }
    foreach ($d in $deferred) { $plan.Add($d) }
    return $plan
}

$plan = New-LegPlan

if ($Mode -eq 'plan') {
    $i = 0
    foreach ($p in $plan) {
        $i++
        $macroPath = if ($p.macro) { Join-Path $suiteDir ($p.macro -replace '/', '\') } else { $null }
        $exists = if ($macroPath) { if (Test-Path -LiteralPath $macroPath) { '' } else { '  ← 宏文件缺失' } } else { '' }
        $expectNote = ''
        if ($p.macro) {
            $e = Get-LegExpectation $p.macro
            if (-not $e) { $expectNote = '  ← rvm-truth.json 未登记' }
            elseif ($e.PSObject.Properties['calibrate'] -and $e.calibrate) { $expectNote = '  (calibrate)' }
            elseif ($e.PSObject.Properties['aabb_blind'] -and $e.aabb_blind) { $expectNote = '  (aabb_blind)' }
        }
        if ($p.action -eq 'run' -and $p.case -eq 'setup') { '{0,3}. {1,-28} {2,-8} {3}{4}  → 导出（即基线，不对拍）' -f $i, $p.case, $p.leg, $p.macro, $exists }
        elseif ($p.action -eq 'run') { '{0,3}. {1,-28} {2,-8} {3}{4}{5}  → 导出 + 对拍' -f $i, $p.case, $p.leg, $p.macro, $exists, $expectNote }
        elseif ($p.action -eq 'export') { '{0,3}. {1,-28} {2,-8} （导出基线，不对拍）' -f $i, $p.case, $p.leg }
        else { '{0,3}. {1,-28} {2,-8} 跳过：{3}' -f $i, $p.case, $p.leg, $p.reason }
    }
    $runs = @($plan | Where-Object { $_.action -eq 'run' }).Count
    $skips = @($plan | Where-Object { $_.action -eq 'skip' }).Count
    "写腿 $runs 条（每条之后导出一份真值）+ 基线 1 份；跳过 $skips 个用例。"
    exit 0
}

# ── 真机前置 ────────────────────────────────────────────────────────────────────────
if (-not $IsolatedProjectsDir) { throw "state / batch 模式必须给 -IsolatedProjectsDir（隔离工程副本根）" }
$projects = Assert-IsolatedProjects $IsolatedProjectsDir $Source
$fixtureExe = Join-Path $TargetDir "$BuildProfile\db_session_fixture.exe"
$rvmVerifyExe = Join-Path $TargetDir "$BuildProfile\rvm_verify.exe"
foreach ($path in @($fixtureExe, $L3Exe)) {
    if (-not (Test-Path -LiteralPath $path)) { throw "缺少前置：$path（本脚本不做 cargo build）" }
}
# ★ rvm_verify.exe 只在真正要 import 时才用到，但缺了会等到快照导出完才炸，白跑一遍 E3D。
#   这里提前校验；release 缺这个 exe 是已知事实，提示改用 -BuildProfile debug。
if (-not $SkipCompare -and -not (Test-Path -LiteralPath $rvmVerifyExe)) {
    throw "缺少前置：$rvmVerifyExe（-BuildProfile $BuildProfile 下没有 rvm_verify.exe；release 缺这个文件，改用 -BuildProfile debug）"
}
$workFile = Join-Path $projects $DbFile
if (-not (Test-Path -LiteralPath $workFile)) { throw "隔离副本里没有目标库：$workFile" }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$evidenceRoot = Join-Path $repo "output\increment-suite-rvm-truth\$stamp"
New-Item -ItemType Directory -Force -Path $evidenceRoot | Out-Null
if (-not $TruthDir) { $TruthDir = Join-Path $evidenceRoot 'rvm-truth' }
New-Item -ItemType Directory -Force -Path $TruthDir | Out-Null
$TruthDir = (Resolve-Path -LiteralPath $TruthDir).Path
$indexPath = Join-Path $TruthDir 'index.json'
$index = New-Object System.Collections.Generic.List[object]
if (Test-Path -LiteralPath $indexPath) {
    foreach ($e in @(Get-Content -LiteralPath $indexPath -Raw -Encoding UTF8 | ConvertFrom-Json)) { $index.Add($e) }
}

function Get-Sesno {
    $json = & $fixtureExe inspect --source $workFile
    if ($LASTEXITCODE) { throw "db_session_fixture inspect 失败（exit $LASTEXITCODE）：$workFile" }
    return [int](($json -join "`n") | ConvertFrom-Json).latest_sesno
}

function Save-Index {
    # -InputObject 包一层数组：单元素 List 走管道会被拆成裸对象，index.json 就不再是数组
    Write-Utf8NoBom $indexPath ((ConvertTo-Json -InputObject @($index.ToArray()) -Depth 6) + "`n")
}

# 渲染套件宏：{{LOG_PATH}} → 宏同目录同名 .log；{{WORLD_ANCHOR}} → 锚点 + OWNER 两行
function ConvertTo-RenderedMacro([string]$MacroRel, [string]$OutDir, [string]$Name) {
    $template = Join-Path $suiteDir ($MacroRel -replace '/', '\')
    if (-not (Test-Path -LiteralPath $template)) { throw "宏不存在：$template" }
    $text = Get-Content -LiteralPath $template -Raw -Encoding UTF8
    $out = Join-Path $OutDir "$Name.mac"
    $log = Join-Path $OutDir "$Name.log"
    $text = $text.Replace('{{LOG_PATH}}', ($log -replace '\\', '/')).Replace('{{WORLD_ANCHOR}}', "$Anchor`nOWNER")
    if ($text -match '\{\{') {
        $left = @($text -split "`n" | Where-Object { $_ -match '\{\{' })
        throw "宏 $MacroRel 还有未渲染的占位符：$($left -join ' | ')"
    }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    Write-Utf8NoBom $out (($text -replace "`r`n", "`n").TrimEnd() + "`n")
    $marker = $null
    foreach ($line in ($text -split "`n")) {
        if ($line.Trim() -match '^\$P\s+(\S+-DONE)\s*$') { $marker = $Matches[1] }
    }
    if (-not $marker) { throw "宏 $MacroRel 没有 `$P ...-DONE 结束标记，无法判定跑完" }
    return [pscustomobject]@{ macro = $out; log = $log; marker = $marker }
}

# 跑一条写腿：sesno 必须前进（SAVEWORK 落到目标库才算；写到 db1112 的那种 sesno 不动，驱动器也会报）
function Invoke-SuiteLeg([string]$MacroRel, [string]$Name) {
    $dir = Join-Path $evidenceRoot $Name
    $r = ConvertTo-RenderedMacro $MacroRel $dir $Name
    $before = Get-Sesno
    $saved = $env:L3_ALLOW_EXISTING_E3D_SESSION
    $env:L3_ALLOW_EXISTING_E3D_SESSION = '1'
    try {
        Invoke-TtyMacro -Repo $repo -L3Exe $L3Exe -Macro $r.macro -ProjectsDir $projects -MainProject $MainProject `
            -InstallDir $E3dInstallDir -Evidence (Join-Path $dir 'driver') -LogFile $r.log -DoneMarker $r.marker `
            -ExtraArgs @('--target-db-file', $workFile, '--aios-project', $MainProject)
    } finally {
        $env:L3_ALLOW_EXISTING_E3D_SESSION = $saved
    }
    $after = Get-Sesno
    if ($after -le $before) {
        throw "$MacroRel 跑完 sesno $before -> $after 没前进：SAVEWORK 没落到 $DbFile（锚点错了会写进 db1112）"
    }
    $errors = @(Get-Content -LiteralPath $r.log -Encoding UTF8 | Where-Object { $_.TrimStart().StartsWith('(') })
    if ($errors.Count) {
        throw "$MacroRel 的日志里有 E3D 报错：$($errors[0])（共 $($errors.Count) 行，见 $($r.log)）"
    }
    Write-Host ("  {0,-44} sesno {1} -> {2} (+{3})" -f $MacroRel, $before, $after, ($after - $before))
    return [pscustomobject]@{ sesno_before = $before; sesno = $after; log = $r.log; evidence = $dir }
}

# 只读导出当前状态 → <TruthDir>/<sesno>.json；返回登记条目
function Export-State([string]$Name, [int]$StateSesno, [string]$CaseId, [string]$Leg, [string]$MacroRel) {
    $dir = Join-Path $evidenceRoot "$Name.rvm-export"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $rvm = Join-Path $dir "$StateSesno.rvm"
    $log = Join-Path $dir 'rvm_export.log'
    $macro = Join-Path $dir 'rvm_export.mac'
    New-RvmExportMacro -Path $macro -Element $Element -RvmFile $rvm -LogFile $log -Note "suite truth $Name sesno $StateSesno $Element" -QueryRef
    Invoke-TtyMacro -Repo $repo -L3Exe $L3Exe -Macro $macro -ProjectsDir $projects -MainProject $MainProject `
        -InstallDir $E3dInstallDir -Evidence (Join-Path $dir 'driver') -LogFile $log -DoneMarker 'LINKAGE-RVM-EXPORT-DONE'
    if (-not (Test-Path -LiteralPath $rvm) -or (Get-Item -LiteralPath $rvm).Length -le 0) { throw "RVM 没导出来：$rvm（看 $log）" }
    $rootRefno = $null
    foreach ($line in Get-Content -LiteralPath $log -Encoding UTF8) {
        if ($line.Trim() -match '^Ref\s+=?(\d+/\d+)$') { $rootRefno = $Matches[1]; break }
    }
    if (-not $rootRefno) { throw "导出日志里没有 `Ref =` 行，拿不到 $Element 的 refno：$log" }
    $truth = Join-Path $TruthDir "$StateSesno.json"
    if (Test-Path -LiteralPath $truth) {
        throw "真值 $truth 已存在：同一个 sesno 不可能对应两个状态。续跑请换 -TruthDir，或先确认库没有被回退过"
    }
    $snapshot = Import-RvmSnapshot -Repo $repo -RvmVerifyExe $rvmVerifyExe -RvmFile $rvm -Dbnum $Dbnum -RootRefno $rootRefno -OutFile $truth
    $afterExport = Get-Sesno
    $entry = [ordered]@{
        sesno = $StateSesno
        sesno_after_export = $afterExport
        case = $CaseId
        leg = $Leg
        macro = $MacroRel
        element = $Element
        root_refno = $rootRefno
        truth = (Split-Path -Leaf $truth)
        rvm = $rvm
        member_count = $snapshot.meta.member_count
        geometry_count = $snapshot.meta.geometry_count
        unresolved = $snapshot.meta.unresolved
        exported_at = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        compare = $null
        verdict = $null
    }
    Write-Host ("  真值 {0} ← {1}（根 {2}，成员 {3}，几何 {4}）" -f (Split-Path -Leaf $truth), $Name, $rootRefno, $snapshot.meta.member_count, $snapshot.meta.geometry_count)
    $index.Add([pscustomobject]$entry)
    Save-Index
    return $index[$index.Count - 1]
}

function Invoke-Compare($Entry, [string]$BeforeTruth, [string]$MacroRel) {
    $report = Join-Path $evidenceRoot "$($Entry.case)-$($Entry.leg).compare.json"
    & $compareScript -Before $BeforeTruth -After (Join-Path $TruthDir $Entry.truth) -Leg $MacroRel -Model $modelPath -Root $Element -Report $report -Quiet
    $code = $LASTEXITCODE
    $verdict = 'error'
    if (Test-Path -LiteralPath $report) { $verdict = (Get-Content -LiteralPath $report -Raw -Encoding UTF8 | ConvertFrom-Json).verdict }
    $Entry.compare = $report
    $Entry.verdict = $verdict
    Save-Index
    Write-Host ("  对拍 {0}：{1}（exit {2}）" -f $MacroRel, $verdict, $code)
    return $verdict
}

# ── state：只导当前状态 ──────────────────────────────────────────────────────────────────
if ($Mode -eq 'state') {
    $sesno = Get-Sesno
    $entry = Export-State $Label $sesno $Label 'state' $null
    "sesno $sesno -> $(Join-Path $TruthDir $entry.truth)"
    "index：$indexPath"
    exit 0
}

# ── batch：按计划逐腿 ────────────────────────────────────────────────────────────────
$stateBefore = @{}      # 宏相对路径 → 该宏跑之前那份真值的完整路径（equals_state_before 用）
$lastTruth = $null
$legFailures = New-Object System.Collections.Generic.List[string]
$verdicts = @{}

# 续跑：index.json 里已有的最后一份真值当作当前状态
if ($index.Count -gt 0) {
    $lastTruth = Join-Path $TruthDir $index[$index.Count - 1].truth
    Write-Host "续跑：以 $lastTruth 为当前状态"
}

foreach ($p in $plan) {
    if ($p.action -eq 'skip') {
        Write-Host ("跳过 {0}：{1}" -f $p.case, $p.reason)
        continue
    }
    if ($p.action -eq 'export') {
        if ($lastTruth) { Write-Host '已有当前真值，跳过基线导出'; continue }
        Write-Host '== 导出基线 =='
        $sesno = Get-Sesno
        $entry = Export-State 'baseline' $sesno 'baseline' 'baseline' $null
        $lastTruth = Join-Path $TruthDir $entry.truth
        continue
    }
    # run
    $name = "$($p.case -replace '[:/]', '-')-$($p.leg)"
    Write-Host "== $($p.case) / $($p.leg) : $($p.macro) =="
    if ($lastTruth) { $stateBefore[$p.macro] = $lastTruth }
    $leg = $null
    try {
        $leg = Invoke-SuiteLeg $p.macro $name
    } catch {
        $legFailures.Add("$($p.case)/$($p.leg) $($p.macro)：$($_.Exception.Message)")
        Write-Warning $_.Exception.Message
        if ($ContinueOnError) { continue }
        break
    }
    $entry = Export-State $name $leg.sesno $p.case $p.leg $p.macro
    $thisTruth = Join-Path $TruthDir $entry.truth
    if (-not $SkipCompare -and $lastTruth) {
        $before = $lastTruth
        $e = Get-LegExpectation $p.macro
        if ($e -and $e.PSObject.Properties['equals_state_before'] -and $e.equals_state_before) {
            $ref = [string]$e.equals_state_before
            if ($stateBefore.ContainsKey($ref)) { $before = $stateBefore[$ref] }
            else { Write-Warning "$($p.macro) 要与 $ref 之前的状态比，但本轮没跑过它；退回与上一份比" }
        }
        $v = Invoke-Compare $entry $before $p.macro
        $verdicts[$p.macro] = $v
    }
    $lastTruth = $thisTruth
}

$summary = [ordered]@{
    stamp = $stamp
    truth_dir = $TruthDir
    index = $indexPath
    legs_run = @($index | Where-Object { $_.macro }).Count
    verdicts = [ordered]@{
        pass = @($verdicts.Values | Where-Object { $_ -eq 'pass' }).Count
        calibrate = @($verdicts.Values | Where-Object { $_ -eq 'calibrate' }).Count
        fail = @($verdicts.Values | Where-Object { $_ -eq 'fail' }).Count
        skipped = @($verdicts.Values | Where-Object { $_ -eq 'skipped' }).Count
        error = @($verdicts.Values | Where-Object { $_ -eq 'error' }).Count
    }
    leg_failures = @($legFailures)
}
Write-Utf8NoBom (Join-Path $evidenceRoot 'summary.json') (($summary | ConvertTo-Json -Depth 5) + "`n")
Write-Host ("完成：写腿 {0} 条，对拍 pass {1} / calibrate {2} / fail {3} / skipped {4} / error {5}；腿失败 {6}。真值目录 {7}" -f `
    $summary.legs_run, $summary.verdicts.pass, $summary.verdicts.calibrate, $summary.verdicts.fail, $summary.verdicts.skipped, $summary.verdicts.error, $legFailures.Count, $TruthDir)

if ($legFailures.Count -gt 0) { exit 2 }
if ($summary.verdicts.fail -gt 0 -or $summary.verdicts.error -gt 0) { exit 1 }
exit 0
