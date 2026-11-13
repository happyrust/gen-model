#requires -Version 7
<#
.SYNOPSIS
    出覆盖表：变更类型 × 用例 × 各层结果。只读——不开 E3D、不起后端、不编译，只读清单、总表和已有的证据。

.DESCRIPTION
    输入：
      * 清单：suite-manifest.json 加 manifest.d/*.json（SuiteManifest.ps1 的读法）。
      * 片段里登记的「窗」：顶层 replay_windows 下面各个数组里带 id 的项（并窗、腿内切窗、取自别人那条链的窗）。
        它们没有自己的宏，清单的读法不把它们当用例；这张表里它们和用例一样逐层判，宏这一层记不适用。
      * 变更类型总表 docs/specs/incr-gen-audit-aux/increment-change-types.md 第 4 节：编号、类型名；
        「执行资产」一栏点到的主清单用例（主清单里的用例自己不带 change_types，归哪一格以总表为准）；
        「INC」一栏（仓里的回放用例按 registry 里的 INC 号归格）。
      * 证据（本机配置 output/increment-suite/local.json 的 runs_dir 下）：
          sessions.log                          跑宏入口每一步一行；
          <谁>/replay-*/replay-result.json       回放入口每一趟一份。
        不经入口跑出来的回放结果用 -ExtraReplayTsv 补进来（制表符分隔：时刻、用例、结果、秒、备注；可多份）；
        命令行没给，就用本机配置 coverage.extra_replay_tsv 里登记的（相对路径从 workspace 起算）。
      * 各人手写的目录（本机配置 workspace 下的 agents/<谁>/CATALOG.md，或 coverage.catalog 指的地方）：只对名字，
        列出清单里有而目录里没提到的、目录里有而清单里没有的（第六节）。

    各层怎么判（「实测」= 看证据；「自述」= 负责人写在片段里的话，按里面的词归类）：
      宏      实测：用例的每条腿在流水账里最近一次写步的结果。都过 → 过；有一条最近是不过 → 不过；只跑过一部分 → 没跑全。
              流水账里一条腿都没有时用自述（标「自述」）。
      导出    E3D 改前改后的导出符合预期。自述：layers.rvm_truth（T2 除外）/ layers.e3d_rvm / checks.e3d_export_diff。
      规划    离线规划器对这一窗的结论。自述：layers.plan / net_window / plan_offline、checks.plan_offline。
      回放    实测：片段里点到名字的回放用例（replay、layers.replay、checks.replay 里出现的名字）最近一趟的结果，
              分被测后端（主线、r5，加上结果里出现过的别的发布版名字：r6、r7……）。没点到回放用例的用自述。
      对导出  生成结果对 E3D 导出。自述：layers.full_gen、T2 的 layers.rvm_truth。
      L3      隔离后端。自述。
    归类用的词：过 / 过*（过了，但注明还有没做的部分）/ 不过 / 已知缺口 / 没跑 / 有记录（写了东西但没有结论词）/ —（没写）。
    r5 是修之前的后端：它上面的红是新旧对照要的，不算「不过」。r5 以外的后端上最近一趟是红就算「不过」，
    两种情形除外、记「已知缺口」：点到它的用例（或窗）自己声明了已知缺口；红的那几条回放用例每一条都在用例文件里
    标了 known_red 或 _expected_red。

    除了给人看的表，还写一份给机器读的（-Json，缺省和表同名、扩展名 .json）：逐用例逐层归成四个词——
    过 / 不过 / 已知缺口 / 没跑（外加不适用），以及整个用例的结论。Invoke-SuiteAll.ps1 拿它写总报告。

    -Out             写到哪。缺省：本机配置的 workspace 下 COVERAGE.md。
    -Json            给机器读的那一份写到哪。缺省：和 -Out 同名，扩展名换成 .json。
    -ExtraReplayTsv  额外的回放结果表，可多份。

    退出码：0 没有「不过」；1 有「不过」（已知缺口、r5 上的红不算）；2 输入不对。

.EXAMPLE
    pwsh -NoProfile -File scripts/e3d/increment_suite/Get-SuiteCoverage.ps1
#>
[CmdletBinding()]
param(
    [string]$Out = '',
    [string[]]$ExtraReplayTsv = @(),
    [string]$Json = '',
    [string]$LocalConfig = '',
    [switch]$Quiet
)
$ErrorActionPreference = 'Stop'
$suiteDir = $PSScriptRoot
$repo = (Resolve-Path -LiteralPath (Join-Path $suiteDir '..\..\..')).Path
. (Join-Path $suiteDir 'SuiteManifest.ps1')
function Stop-With([int]$code, [string]$message) { Write-Host $message; exit $code }
if (-not $LocalConfig) { $LocalConfig = Join-Path $repo 'output\increment-suite\local.json' }
if (-not (Test-Path -LiteralPath $LocalConfig)) { Stop-With 2 "没有本机配置 $LocalConfig" }
$cfg = Get-Content -LiteralPath $LocalConfig -Raw -Encoding UTF8 | ConvertFrom-Json
$runsDir = [string]$cfg.runs_dir
if (-not $runsDir -or -not (Test-Path -LiteralPath $runsDir)) { Stop-With 2 "本机配置的 runs_dir 不在：$runsDir" }
$workspace = if ($cfg.PSObject.Properties['workspace'] -and $cfg.workspace) { [string]$cfg.workspace } else { '' }
$coverageCfg = if ($cfg.PSObject.Properties['coverage'] -and $cfg.coverage) { $cfg.coverage } else { $null }
if (-not $Out) { $Out = if ($workspace) { Join-Path $workspace 'COVERAGE.md' } else { Join-Path $repo 'output\increment-suite\COVERAGE.md' } }
if (-not $Json) { $Json = [IO.Path]::ChangeExtension($Out, '.json') }
$ExtraReplayTsv = @($ExtraReplayTsv | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
# 命令行没给补充表，就用本机配置 coverage.extra_replay_tsv 里登记的：不带参数重出的表要和上一次是同一张
if (-not $ExtraReplayTsv.Count -and $coverageCfg -and $coverageCfg.PSObject.Properties['extra_replay_tsv']) { $ExtraReplayTsv = @($coverageCfg.extra_replay_tsv | ForEach-Object { [string]$_ } | Where-Object { $_ }) }
$ExtraReplayTsv = @($ExtraReplayTsv | ForEach-Object { if ($workspace -and -not [IO.Path]::IsPathRooted($_)) { Join-Path $workspace $_ } else { $_ } })
foreach ($tsv in $ExtraReplayTsv) { if (-not (Test-Path -LiteralPath $tsv)) { Stop-With 2 "额外的回放结果表不在：$tsv" } }

# ── 1. 清单 ──
$merged = Read-SuiteManifest -SuiteDir $suiteDir
$cases = @($merged.cases)
$mainIds = @($cases | Where-Object { $_.source -eq 'suite-manifest.json' } | ForEach-Object { $_.id })

# 片段里登记的「窗」：顶层 replay_windows 下面各个数组里带 id 的项（并窗、腿内切窗、取自别人那条链的窗、探针窗）。
# 它们没有自己的宏——是在别的用例录下来的会话链上换起止取出来的——所以清单的读法不把它们当用例
# （跑宏的入口没有东西可跑）。在这张表里它们一样是情况：逐层照判，宏这一层记不适用。
$windows = [System.Collections.Generic.List[object]]::new()
$windowDupes = [System.Collections.Generic.List[string]]::new()
$takenIds = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($case in $cases) { [void]$takenIds.Add([string]$case.id) }
foreach ($source in @($merged.sources)) {
    $file = [string]$source.file
    if ($file -notlike 'manifest.d/*') { continue }
    try { $frag = Read-SuiteJson (Join-Path $suiteDir ($file -replace '/', '\')) } catch { continue }
    if ($frag -isnot [System.Collections.IDictionary] -or -not $frag.Contains('replay_windows')) { continue }
    $node = $frag['replay_windows']
    $lists = [ordered]@{}
    if ($node -is [System.Collections.IDictionary]) { foreach ($key in @($node.Keys)) { if ($node[$key] -is [System.Collections.IList]) { $lists[[string]$key] = $node[$key] } } }
    elseif ($node -is [System.Collections.IList]) { $lists['windows'] = $node }
    foreach ($listName in @($lists.Keys)) {
        foreach ($w in $lists[$listName]) {
            if ($w -isnot [System.Collections.IDictionary] -or -not $w.Contains('id') -or -not $w['id']) { continue }
            $id = [string]$w['id']
            if (-not $takenIds.Add($id)) { $windowDupes.Add("$id（$file）"); continue }
            $situation = if ($w.Contains('situation') -and $w['situation']) { [string]$w['situation'] } elseif ($w.Contains('shape') -and $w['shape']) { [string]$w['shape'] } else { '' }
            $gap = if ($w.Contains('known_gap') -and $w['known_gap']) { $w['known_gap'] }
            elseif ($w.Contains('gap_of_case') -and $w['gap_of_case']) { "同 $($w['gap_of_case'])" }
            elseif ($w.Contains('known_gap_candidate') -and $w['known_gap_candidate']) { "（登记时写的是候选）$($w['known_gap_candidate'])" }
            else { $null }
            $windows.Add([pscustomobject]@{
                    id = $id; source = $file; owner = [string]$source.owner; kind = 'window'; window_list = $listName
                    status = $(if ($w.Contains('status') -and $w['status']) { [string]$w['status'] } else { '' })
                    change_types = @($w['change_types'] | Where-Object { $_ } | ForEach-Object { [string]$_ })
                    situation = $situation; legs = @(); known_gap = $gap
                    tickets = @($w['tickets'] | Where-Object { $_ } | ForEach-Object { [string]$_ }); raw = $w
                })
        }
    }
}
$cases = @($cases) + @($windows)

# ── 2. 变更类型总表：编号、类型名、点到的主清单用例、INC 号 ──
$tablePath = Join-Path $repo 'docs\specs\incr-gen-audit-aux\increment-change-types.md'
if (-not (Test-Path -LiteralPath $tablePath)) { Stop-With 2 "变更类型总表不在：$tablePath" }
$replayOnly = @('CT-W3', 'CT-W4', 'CT-W5', 'CT-W10')   # 会话机制类：只能在回放里造（总表第 6 节）
$notApplicable = @('CT-W8')
$cts = [ordered]@{}
foreach ($line in Get-Content -LiteralPath $tablePath -Encoding UTF8) {
    if ($line -notmatch '^\|\s*(CT-[SACDW]\d+)\s*\|') { continue }
    $cells = @($line.Trim().Trim('|') -split '\|' | ForEach-Object { $_.Trim() })
    if ($cells.Count -lt 7) { continue }
    $id = $cells[0]
    $incCell = $cells[$cells.Count - 3]; $assetCell = $cells[$cells.Count - 2]
    # INC：A02、U01–U04 之类
    $incs = [System.Collections.Generic.List[string]]::new()
    foreach ($m in [regex]::Matches($incCell, '([A-Z])(\d{2})(?:\s*[–-]\s*[A-Z]?(\d{2}))?')) {
        $from = [int]$m.Groups[2].Value; $to = if ($m.Groups[3].Success) { [int]$m.Groups[3].Value } else { $from }
        for ($n = $from; $n -le $to; $n++) { $incs.Add(('INC-{0}{1:00}' -f $m.Groups[1].Value, $n)) }
    }
    # 执行资产：C01-move / rot-u / compound 这种省略写法要把前缀补回去；A01-bran-* 按前缀匹配
    $assets = [System.Collections.Generic.List[string]]::new()
    $plain = [regex]::Replace($assetCell, '（[^（）]*）|\([^()]*\)', '')
    foreach ($part in ($plain -split '、')) {
        $prefix = ''
        foreach ($piece in ($part -split '\s*/\s*')) {
            $token = ($piece -replace '^.*：', '').Trim().Trim('★').Trim()
            if ($token -cmatch '^([A-Z]\d{2})-([a-z0-9*-]+)$') { $prefix = $Matches[1] }
            elseif ($prefix -and $token -cmatch '^[a-z][a-z0-9-]*$') { $token = "$prefix-$token" }
            else { continue }
            if ($token.EndsWith('*')) { foreach ($hit in @($mainIds | Where-Object { $_.StartsWith($token.TrimEnd('*')) })) { if (-not $assets.Contains($hit)) { $assets.Add($hit) } } }
            elseif ($mainIds -ccontains $token) { if (-not $assets.Contains($token)) { $assets.Add($token) } }
        }
    }
    $cts[$id] = [pscustomobject]@{ id = $id; name = $cells[1]; incs = @($incs); main_cases = @($assets) }
}
if (-not $cts.Count) { Stop-With 2 "总表里没读到 CT- 开头的行：$tablePath" }
# 主清单用例 → 格
$mainCt = @{}
foreach ($ct in $cts.Values) { foreach ($id in $ct.main_cases) { if (-not $mainCt.ContainsKey($id)) { $mainCt[$id] = [System.Collections.Generic.List[string]]::new() }; $mainCt[$id].Add($ct.id) } }
$unmappedMain = @($mainIds | Where-Object { -not $mainCt.ContainsKey($_) })

# ── 3. 证据：流水账里每条腿最近一次写步 ──
function Get-LegKey([string]$leg) { return (($leg -replace '@[0-9+]+$', '') -replace '\\', '/').ToLowerInvariant() }
$legRuns = @{}
$journal = Join-Path $runsDir 'sessions.log'
$journalRows = 0
if (Test-Path -LiteralPath $journal) {
    foreach ($line in [IO.File]::ReadLines($journal)) {
        $f = $line -split "`t"
        if ($f.Count -lt 9) { continue }
        $journalRows++
        if ($f[4] -ne 'write') { continue }   # 试跑、探针、导出不算「宏跑通」
        $legRuns[(Get-LegKey $f[8])] = [pscustomobject]@{ time = $f[0]; who = $f[1]; label = $f[2]; verdict = $f[5] }
    }
}

# ── 4. 证据：回放 ──
$replayRuns = [System.Collections.Generic.List[object]]::new()
$replayFiles = 0
foreach ($who in Get-ChildItem -LiteralPath $runsDir -Directory) {
    foreach ($dir in Get-ChildItem -LiteralPath $who.FullName -Directory -Filter 'replay-*') {
        $file = Join-Path $dir.FullName 'replay-result.json'
        if (-not (Test-Path -LiteralPath $file)) { continue }
        try { $j = Get-Content -LiteralPath $file -Raw -Encoding UTF8 | ConvertFrom-Json } catch { continue }
        $replayFiles++
        $backend = if ($j.PSObject.Properties['backend'] -and $j.backend) { [string]$j.backend } elseif ([string]$j.label -match '(?:^|-)(r5|r6)$') { $Matches[1] } else { 'local-debug' }
        if ($backend -eq 'local-debug') { $backend = '主线' }
        # 一趟带几条用例时只知道总秒数，平摊（排计划估时间用，不求准）
        $judged = @($j.results.PSObject.Properties | Where-Object { $_.Value -in @('pass', 'fail') }).Count
        $each = if ($judged -and $j.PSObject.Properties['seconds']) { [int][math]::Round([double]$j.seconds / $judged) } else { 0 }
        foreach ($p in $j.results.PSObject.Properties) {
            if ($p.Value -notin @('pass', 'fail')) { continue }
            $replayRuns.Add([pscustomobject]@{ case = [string]$p.Name; backend = $backend; verdict = [string]$p.Value; time = [string]$j.started; where = "$($who.Name)\$($dir.Name)"; cases_dir = [string]$j.cases_dir; seconds = $each })
        }
    }
}
foreach ($tsv in $ExtraReplayTsv) {
    $day = (Get-Item -LiteralPath $tsv).LastWriteTime.ToString('yyyy-MM-dd')
    foreach ($line in [IO.File]::ReadLines($tsv)) {
        $f = $line -split "`t"
        if ($f.Count -lt 3 -or $f[2] -notmatch '^(pass|fail)$') { continue }
        $time = if ($f[0] -match '^\d{2}:\d{2}:\d{2}$') { "$day $($f[0])" } else { $f[0] }
        $backend = if ($f.Count -ge 6 -and $f[5]) { $f[5] } else { '主线' }
        $replayRuns.Add([pscustomobject]@{ case = $f[1].Trim(); backend = $backend; verdict = $f[2].ToLowerInvariant(); time = $time; where = (Split-Path -Leaf $tsv); cases_dir = ''; seconds = $(if ($f.Count -ge 4 -and $f[3] -match '^\d+(\.\d+)?$') { [int][math]::Round([double]$f[3]) } else { 0 }) })
    }
}
# 每条回放用例、每个后端，最近一趟
$replayLatest = @{}
foreach ($run in ($replayRuns | Sort-Object time)) { $replayLatest["$($run.case)|$($run.backend)"] = $run }
$replayNames = @($replayRuns | ForEach-Object { $_.case } | Sort-Object -Unique | Sort-Object { -$_.Length })
# 被测后端：主线（缺省）、r5（修之前的那一版，它上面的红是新旧对照要的），以及结果里出现过的别的发布版名字
# （r6、r7……，跑回放时用 -Backend 指的）。r5 以外的都按「现在的后端」算：最近一趟红了就算不过。
$seenBackends = @($replayRuns | ForEach-Object { $_.backend } | Sort-Object -Unique)
$releaseBackends = @($seenBackends | Where-Object { $_ -match '^r\d+[A-Za-z0-9._-]*$' -and $_ -ne 'r5' } |
        Sort-Object -Property @{ Expression = { [int]([regex]::Match($_, '\d+').Value) }; Descending = $true }, @{ Expression = { $_ }; Descending = $false })
$currentBackends = @('主线') + $releaseBackends
$backends = $currentBackends + @('r5')
$oddBackends = @($seenBackends | Where-Object { $backends -notcontains $_ })
function Test-Current($row, [string]$value) { foreach ($b in $currentBackends) { if ($row.per_backend[$b] -eq $value) { return $true } }; return $false }
function Test-CurrentAny($row) { foreach ($b in $currentBackends) { if ($row.per_backend[$b]) { return $true } }; return $false }
# 仓里的回放用例：按 registry 里的 INC 号归格
$repoReplay = @{}
$repoCasesDir = Join-Path $repo 'tests\increment_replay\cases'
if (Test-Path -LiteralPath $repoCasesDir) {
    foreach ($file in Get-ChildItem -LiteralPath $repoCasesDir -Filter '*.json' -File) {
        try { $spec = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable } catch { continue }
        $repoReplay[$file.BaseName] = @($spec['registry'] | ForEach-Object { [string]$_ })
    }
}
# 回放用例自己声明「本来就红」的：known_red（缺省不跑的那一种，负责人在用例里写了原因）或 _expected_red（故意留着的对照）。
# 这种回放用例红了，点到它的用例或窗记已知缺口，不记不过——哪怕那个用例自己的状态还没来得及改。
$replayDeclaredRed = @{}
# 知道的回放用例目录：仓里的那一个、本机配置 replay.cases_dirs 登记的（{ owner, dir }，相对路径从 workspace 起算）、
# 结果里出现过的。目录里每一份带 steps 的 json 是一条回放用例——跑没跑过都算，没跑过的在第四节单列。
$knownCaseDirs = [ordered]@{}
$knownCaseDirs[$repoCasesDir] = '仓里'
if ($cfg.PSObject.Properties['replay'] -and $cfg.replay -and $cfg.replay.PSObject.Properties['cases_dirs']) {
    foreach ($entry in @($cfg.replay.cases_dirs)) {
        $dir = if ($entry -is [string]) { $entry } else { [string]$entry.dir }
        if (-not $dir) { continue }
        if ($workspace -and -not [IO.Path]::IsPathRooted($dir)) { $dir = Join-Path $workspace $dir }
        if (-not $knownCaseDirs.Contains($dir)) { $knownCaseDirs[$dir] = $(if ($entry -isnot [string] -and $entry.PSObject.Properties['owner']) { [string]$entry.owner } else { '' }) }
    }
}
foreach ($dir in @($replayRuns | ForEach-Object { $_.cases_dir } | Where-Object { $_ } | Sort-Object -Unique)) { if (-not $knownCaseDirs.Contains($dir)) { $knownCaseDirs[$dir] = '' } }
function Get-ShortDir([string]$dir) {
    foreach ($base in @($workspace, $repo)) { if ($base -and $dir.StartsWith($base.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { return $dir.Substring($base.TrimEnd('\').Length + 1) } }
    return $dir
}
$allReplayCases = [ordered]@{}
foreach ($dir in @($knownCaseDirs.Keys)) {
    if (-not (Test-Path -LiteralPath $dir)) { continue }
    foreach ($file in Get-ChildItem -LiteralPath $dir -Filter '*.json' -File | Sort-Object Name) {
        try { $spec = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable } catch { continue }
        if ($spec -isnot [System.Collections.IDictionary] -or -not $spec.Contains('steps')) { continue }
        $why = if ($spec.Contains('_expected_red') -and $spec['_expected_red']) { [string]$spec['_expected_red'] } elseif ($spec.Contains('known_red') -and $spec['known_red'] -eq $true) { 'known_red' } else { '' }
        if ($why) { $replayDeclaredRed[$file.BaseName] = $why }
        if (-not $allReplayCases.Contains($file.BaseName)) {
            $allReplayCases[$file.BaseName] = [pscustomobject]@{
                name = $file.BaseName; dir = (Get-ShortDir $dir); owner = [string]$knownCaseDirs[$dir]; declared_red = $why
                known_red = [bool]($spec.Contains('known_red') -and $spec['known_red'] -eq $true)
                doc = $(if ($spec.Contains('_doc') -and $spec['_doc'] -is [string]) { [string]$spec['_doc'] } else { '' })
                registry = @($spec['registry'] | Where-Object { $_ } | ForEach-Object { [string]$_ })
                project = $(if ($spec.Contains('project') -and $spec['project'] -is [System.Collections.IDictionary]) { [string]$spec['project']['name'] } else { 'AvevaMarineSample' })
            }
        }
    }
}

# ── 5. 自述：把片段里的话归成一个词 ──
function Get-RawText($raw, [string[]]$paths) {
    foreach ($path in $paths) {
        $node = $raw
        foreach ($key in ($path -split '\.')) { if ($node -is [System.Collections.IDictionary] -and $node.Contains($key)) { $node = $node[$key] } else { $node = $null; break } }
        if ($null -eq $node) { continue }
        $text = if ($node -is [string]) { $node } elseif ($node -is [System.Collections.IDictionary]) { ($node | ConvertTo-Json -Depth 6 -Compress) } else { (@($node) | ForEach-Object { if ($_ -is [string]) { $_ } else { $_ | ConvertTo-Json -Depth 6 -Compress } }) -join ' ｜ ' }
        if ($text -and $text.Trim()) { return $text.Trim() }
    }
    return ''
}
function Get-Token([string]$text) {
    if (-not $text -or -not $text.Trim()) { return '—' }
    $t = $text.Trim()
    if ($t -match '^不适用') { return '不适用' }
    $gap = $t -match '已知缺口|known_gap|known gap'
    # 「不过」「没过」后面跟着标点或在句末才算结论；后面接着说话的是转折，或者在说别的事（「没过元数据门」）
    $fail = $t -match '不符|(不过|没过)(?=[，。；、）)\s]|$)|\bfail\b|FAIL|：红|判红'
    $leadNotRun = $t -match '^(没跑|没做|没核|没有|没对|draft|草稿|待|等|未|还没|n/?a\b)'
    $anyNotRun = $t -match '没跑|没做|没核|没对|还没|待补|不作数|草稿已生成|等回放|等新终稿|等隔离'
    $pass = $t -match '^(pass|过|通过|核过|相符|符合)|实跑通过|对拍过|跑通|：过|：相符|（过）|相符（'
    if ($fail -and $gap) { return '已知缺口' }
    if ($fail) { return '不过' }
    if ($leadNotRun) { return '没跑' }
    if ($pass) { if ($anyNotRun) { return '过*' } else { return '过' } }
    if ($anyNotRun) { return '没跑' }
    if ($gap) { return '已知缺口' }
    return '有记录'
}
$layerPaths = [ordered]@{
    macro  = @('layers.tty', 'layers.e3d_run', 'layers.macro', 'checks.macro')
    export = @('layers.e3d_rvm', 'checks.e3d_export_diff')
    plan   = @('layers.plan', 'layers.net_window', 'layers.plan_offline', 'checks.plan_offline')
    replay = @('layers.replay', 'checks.replay')
    gen    = @('layers.full_gen')
    l3     = @('layers.l3', 'checks.l3')
}
# 四选一：过 / 不过 / 已知缺口 / 没跑（外加「不适用」）。上面那些词归到这四个里，给机器读的那一份和总报告用。
# 「没跑全」「有记录」「—」都算没跑：没有结论就是没有结论。
function Get-Four([string]$token) {
    if ($token -match '^不过') { return '不过' }
    if ($token -match '^已知缺口') { return '已知缺口' }
    if ($token -match '^过') { return '过' }
    if ($token -eq '不适用') { return '不适用' }
    return '没跑'
}

# ── 6. 逐用例 ──
$rows = [System.Collections.Generic.List[object]]::new()
foreach ($case in $cases) {
    $raw = $case.raw
    $isMain = $case.source -eq 'suite-manifest.json'
    $isWindow = [bool]($case.PSObject.Properties['kind'] -and $case.kind -eq 'window')
    $caseCts = if ($isMain) { if ($mainCt.ContainsKey($case.id)) { @($mainCt[$case.id]) } else { @() } } else { @($case.change_types) }

    # 宏：实测
    $legKeys = @($case.legs | ForEach-Object { Get-LegKey ([string]$_) })
    $ran = 0; $failed = [System.Collections.Generic.List[string]]::new(); $latest = ''
    foreach ($key in $legKeys) {
        if (-not $legRuns.ContainsKey($key)) { continue }
        $ran++
        $hit = $legRuns[$key]
        if ($hit.verdict -ne 'pass') { $failed.Add($key) }
        if ($hit.time -gt $latest) { $latest = $hit.time }
    }
    $macroSelf = Get-Token (Get-RawText $raw $layerPaths.macro)
    $macro = if ($failed.Count) { '不过' }
    elseif ($legKeys.Count -and $ran -eq $legKeys.Count) { '过' }
    elseif ($ran -gt 0) { "没跑全（$ran/$($legKeys.Count)）" }
    elseif ($macroSelf -in @('过', '过*')) { "$macroSelf（自述）" }
    else { '没跑' }

    # 导出、规划、对导出、L3：自述
    $exportPaths = if ($case.owner -eq 'T2') { $layerPaths.export } else { @('layers.rvm_truth') + $layerPaths.export }
    $genPaths = if ($case.owner -eq 'T2') { @('layers.rvm_truth') + $layerPaths.gen } else { $layerPaths.gen }
    $export = Get-Token (Get-RawText $raw $exportPaths)
    $plan = Get-Token (Get-RawText $raw $layerPaths.plan)
    $gen = Get-Token (Get-RawText $raw $genPaths)
    $l3 = Get-Token (Get-RawText $raw $layerPaths.l3)

    # 回放：先看点到名字的回放用例的实测，没有再用自述
    $replayText = (@((Get-RawText $raw @('replay')), (Get-RawText $raw $layerPaths.replay)) | Where-Object { $_ }) -join ' ｜ '
    $linked = [System.Collections.Generic.List[string]]::new()
    foreach ($name in $replayNames) { if ($replayText -match ('(?<![A-Za-z0-9_-])' + [regex]::Escape($name) + '(?![A-Za-z0-9_-])')) { $linked.Add($name) } }
    $declaredGap = ($case.status -eq 'known_gap') -or ($replayText -match '已知缺口|known_gap')
    $perBackend = [ordered]@{}
    foreach ($backend in $backends) {
        $runs = @($linked | ForEach-Object { $replayLatest["$_|$backend"] } | Where-Object { $_ })
        $reds = @($runs | Where-Object { $_.verdict -eq 'fail' })
        # 红了记已知缺口的两种情形：这个用例（或窗）自己声明了已知缺口；红的那几条回放用例每一条都自己标了「本来就红」
        $allDeclared = $reds.Count -gt 0 -and -not @($reds | Where-Object { -not $replayDeclaredRed.ContainsKey($_.case) }).Count
        $perBackend[$backend] = if (-not $runs.Count) { '' }
        elseif ($reds.Count) { if ($backend -eq 'r5') { '红' } elseif ($declaredGap -or $allDeclared) { '已知缺口' } else { '不过' } }
        else { if ($backend -eq 'r5') { '绿' } else { '过' } }
    }
    $replaySelf = Get-Token $replayText
    $hasEvidence = @($perBackend.Values | Where-Object { $_ }).Count -gt 0
    $replay = if ($hasEvidence) { (@($backends | Where-Object { $perBackend[$_] } | ForEach-Object { "$_ $($perBackend[$_])" }) -join '；') }
    elseif ($replaySelf -in @('—', '没跑')) { $replaySelf }
    else { "$replaySelf（自述）" }

    # 缺口说明：字符串原样；对象取它的 now（现在的表现），没有 now 就把各格连起来
    $gapText = if (-not $case.known_gap) { '' }
    elseif ($case.known_gap -is [string]) { $case.known_gap }
    elseif ($case.known_gap -is [System.Collections.IDictionary] -and $case.known_gap.Contains('now')) { [string]$case.known_gap['now'] }
    elseif ($case.known_gap -is [System.Collections.IDictionary]) { (@($case.known_gap.Keys | ForEach-Object { "${_}：$(if ($case.known_gap[$_] -is [string]) { $case.known_gap[$_] } else { $case.known_gap[$_] | ConvertTo-Json -Depth 3 -Compress })" }) -join '；') }
    else { $case.known_gap | ConvertTo-Json -Depth 4 -Compress }

    # 四选一。宏：没有腿、也没有自述的用例（纯回放造出来的窗）是不适用，不是没跑。
    # 回放：r5 以外的后端里有一个最近一趟不过就是不过；没有实测才看自述。
    $macroFour = if (-not $legKeys.Count -and $macro -eq '没跑') { '不适用' } else { Get-Four $macro }
    $replayFour = if (@($currentBackends | Where-Object { $perBackend[$_] -eq '不过' }).Count) { '不过' }
    elseif (@($currentBackends | Where-Object { $perBackend[$_] -eq '已知缺口' }).Count) { '已知缺口' }
    elseif (@($currentBackends | Where-Object { $perBackend[$_] -eq '过' }).Count) { '过' }
    else { Get-Four $replaySelf }
    $four = [ordered]@{ macro = $macroFour; export = (Get-Four $export); plan = (Get-Four $plan); replay = $replayFour; gen = (Get-Four $gen); l3 = (Get-Four $l3) }
    # 整个用例：有一层不过就是不过；没有不过、但声明了已知缺口（或者哪一层落在已知缺口上）是已知缺口；
    # 再不然有一层过就是过；一层都没有结论是没跑。
    $overall = if (@($four.Values) -contains '不过') { '不过' }
    elseif ($case.status -eq 'known_gap' -or @($four.Values) -contains '已知缺口') { '已知缺口' }
    elseif (@($four.Values) -contains '过') { '过' }
    else { '没跑' }
    $rows.Add([pscustomobject]@{
            id = $case.id; source = $case.source; owner = $case.owner; cts = $caseCts; status = $case.status; is_main = $isMain; is_window = $isWindow
            macro = $(if ($isWindow -and $macro -eq '没跑') { '不适用' } else { $macro }); macro_time = $latest; export = $export; plan = $plan; replay = $replay; per_backend = $perBackend; replay_self = $replaySelf
            gen = $gen; l3 = $l3; linked = @($linked); gap = $gapText; situation = $case.situation
            four = $four; overall = $overall; macro_measured = ($ran -gt 0); replay_measured = (@($currentBackends | Where-Object { $perBackend[$_] }).Count -gt 0)
            tickets = @($case.tickets); legs = $legKeys.Count
        })
}

# ── 7. 逐格 ──
function Test-Pass([string]$token) { return $token -match '^过' }
$ctRows = [System.Collections.Generic.List[object]]::new()
foreach ($ct in $cts.Values) {
    $mine = @($rows | Where-Object { $_.cts -contains $ct.id })      # 用例加窗：下面「过了几个」都连窗一起数
    $mineCases = @($mine | Where-Object { -not $_.is_window })
    $old = @($mineCases | Where-Object { $_.is_main }).Count
    $macroOk = @($mine | Where-Object { Test-Pass $_.macro }).Count
    $exportOk = @($mine | Where-Object { Test-Pass $_.export }).Count
    $planOk = @($mine | Where-Object { Test-Pass $_.plan }).Count
    $replayOk = @($mine | Where-Object { Test-Current $_ '过' }).Count
    $replaySelfOk = @($mine | Where-Object { -not (Test-CurrentAny $_) -and (Test-Pass $_.replay_self) }).Count
    $r5Red = @($mine | Where-Object { $_.per_backend['r5'] -eq '红' }).Count
    $r5Run = @($mine | Where-Object { $_.per_backend['r5'] }).Count
    $gaps = @($mine | Where-Object { $_.status -eq 'known_gap' -or $_.replay -match '已知缺口' }).Count
    $bad = @($mine | Where-Object { $_.macro -eq '不过' -or $_.replay -match '不过' -or $_.export -eq '不过' -or $_.plan -eq '不过' }).Count
    # 仓里的回放链按 INC 号挂到这一格的
    $repoHits = @($repoReplay.Keys | Where-Object { $name = $_; @($repoReplay[$name] | Where-Object { $ct.incs -contains $_ }).Count -gt 0 } | Sort-Object)
    $repoPass = @($repoHits | Where-Object { $run = $replayLatest["$_|主线"]; $run -and $run.verdict -eq 'pass' })
    $reach = if ($notApplicable -contains $ct.id) { '不适用' }
    elseif ($replayOk -gt 0) { '回放过' }
    elseif ($replayOnly -contains $ct.id) { if ($repoPass.Count) { '只能回放，仓里的链盖到' } else { '只能回放，没盖到' } }
    elseif ($replaySelfOk -gt 0) { '回放过（自述）' }
    elseif ($planOk -gt 0) { '规划过' }
    elseif ($exportOk -gt 0) { '导出过' }
    elseif ($macroOk -gt 0) { '宏跑通' }
    elseif ($mine.Count) { '有用例，没跑' }
    elseif ($repoPass.Count) { '只有仓里的回放链' }
    else { '什么都没有' }
    $ctRows.Add([pscustomobject]@{
            id = $ct.id; name = $ct.name; total = $mineCases.Count; old = $old; new = $mineCases.Count - $old; windows = $mine.Count - $mineCases.Count; macro = $macroOk; export = $exportOk; plan = $planOk
            replay = $replayOk; replay_self = $replaySelfOk; r5 = "$r5Red/$r5Run"; gaps = $gaps; bad = $bad; repo = @($repoHits); repo_pass = $repoPass.Count; reach = $reach
        })
}

# ── 7b. 各人的目录文件对清单 ──
# 目录是各人手写的，版式各不相同，所以只对名字：某人在清单里的用例号，在不在他的目录里出现过；
# 目录里出现过、长得像用例号（一个大写字母加一两位数字，后面跟小写的短横词）而清单里没有的名字
# ——计划了没做的、改了名的、写了宏还没登记的。文件在哪：本机配置 coverage.catalog（{owner} 换成负责人，
# 从 workspace 起算），不写就是 agents\{owner}\CATALOG.md。
$catalogRows = [System.Collections.Generic.List[object]]::new()
$catalogPattern = if ($coverageCfg -and $coverageCfg.PSObject.Properties['catalog'] -and $coverageCfg.catalog) { [string]$coverageCfg.catalog } else { 'agents\{owner}\CATALOG.md' }
if ($workspace) {
    $allIds = @($rows | ForEach-Object { $_.id })
    # 两种长得像用例号、其实不是的名字：片段文件自己的名字（T1-cable）；回放用例的名字（跑过的，加上那些用例目录里的文件）
    $sourceNames = @($merged.sources | ForEach-Object { [IO.Path]::GetFileNameWithoutExtension([string]$_.file) })
    $replayFileNames = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($name in $replayNames) { [void]$replayFileNames.Add($name) }
    foreach ($name in @($allReplayCases.Keys)) { [void]$replayFileNames.Add([string]$name) }
    foreach ($group in ($rows | Where-Object { -not $_.is_main } | Group-Object owner | Sort-Object Name)) {
        $relative = $catalogPattern -replace '\{owner\}', $group.Name
        $path = Join-Path $workspace $relative
        $mine = @($group.Group | ForEach-Object { $_.id })
        $mineWindows = @($group.Group | Where-Object { $_.is_window }).Count
        if (-not (Test-Path -LiteralPath $path)) { $catalogRows.Add([pscustomobject]@{ owner = $group.Name; file = ''; time = ''; cases = $mine.Count - $mineWindows; windows = $mineWindows; missing = $mine; extra = @(); replay_only = @() }); continue }
        $mentioned = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($m in [regex]::Matches([IO.File]::ReadAllText($path), '(?<![A-Za-z0-9_/\\.-])[A-Z]\d{1,2}-[a-z][a-z0-9]*(?:-[a-z0-9]+)*(?![A-Za-z0-9_-])')) { [void]$mentioned.Add($m.Value) }
        $unknown = @($mentioned | Where-Object { $allIds -cnotcontains $_ -and $sourceNames -cnotcontains $_ } | Sort-Object)
        $catalogRows.Add([pscustomobject]@{
                owner = $group.Name; file = $relative; time = (Get-Item -LiteralPath $path).LastWriteTime.ToString('MM-dd HH:mm'); cases = $mine.Count - $mineWindows; windows = $mineWindows
                missing = @($mine | Where-Object { -not $mentioned.Contains($_) })
                extra = @($unknown | Where-Object { -not $replayFileNames.Contains($_) })
                replay_only = @($unknown | Where-Object { $replayFileNames.Contains($_) })
            })
    }
}

# ── 8. 写出来 ──
function Format-Cell([string]$text, [int]$max = 0) {
    $t = ($text -replace '\r?\n', ' ') -replace '\|', '｜'
    if ($max -gt 0 -and $t.Length -gt $max) { $t = $t.Substring(0, $max) + '…' }
    return $t
}
$sb = [System.Text.StringBuilder]::new()
function Add-Line([string]$line = '') { [void]$sb.AppendLine($line) }
$now = Get-Date -Format 'yyyy-MM-dd HH:mm'
$caseRows = @($rows | Where-Object { -not $_.is_window })
$windowRows = @($rows | Where-Object { $_.is_window })
$fragCases = @($caseRows | Where-Object { -not $_.is_main })
$badRows = @($rows | Where-Object { $_.macro -eq '不过' -or (Test-Current $_ '不过') -or $_.replay -eq '不过（自述）' -or $_.export -eq '不过' -or $_.plan -eq '不过' })
$reachCount = @{}
foreach ($row in $ctRows) { $reachCount[$row.reach] = 1 + [int]$reachCount[$row.reach] }
$withCases = @($ctRows | Where-Object { $_.total -gt 0 }).Count
$replayCts = @($ctRows | Where-Object { $_.reach -eq '回放过' }).Count

Add-Line '# 覆盖表：变更类型 × 用例 × 各层结果'
Add-Line
Add-Line "> 生成的，别手改；要改就改片段或重跑之后再生成。生成时刻 $now；命令 ``scripts/e3d/increment_suite/Get-SuiteCoverage.ps1``。"
Add-Line "> 输入：清单 $($caseRows.Count) 个用例（主清单 $($mainIds.Count) 个、片段 $($fragCases.Count) 个，$(@($merged.sources).Count) 个来源），片段里另登记了 $($windowRows.Count) 个窗；流水账 $journalRows 行；回放结果 $replayFiles 趟$(if ($ExtraReplayTsv.Count) { "，另加 $($ExtraReplayTsv.Count) 份补充表" })。"
Add-Line
Add-Line '## 一、数出来的'
Add-Line
$windowOnlyCts = @($ctRows | Where-Object { $_.total -eq 0 -and $_.windows -gt 0 })
Add-Line "- 变更类型 $($cts.Count) 类：有用例的 $withCases 类$(if ($windowOnlyCts.Count) { "（另有 $($windowOnlyCts.Count) 类只有窗：$(($windowOnlyCts | ForEach-Object { $_.id }) -join '、')）" })；**在后端上实测回放过的 $replayCts 类**；用例和窗都没有的 $(@($ctRows | Where-Object { $_.total -eq 0 -and $_.windows -eq 0 -and $_.reach -ne '不适用' }).Count) 类。"
Add-Line ('- 各类走到哪：' + ((@('回放过', '回放过（自述）', '只能回放，仓里的链盖到', '只能回放，没盖到', '规划过', '导出过', '宏跑通', '有用例，没跑', '只有仓里的回放链', '什么都没有', '不适用') | Where-Object { $reachCount[$_] } | ForEach-Object { "$_ $($reachCount[$_])" }) -join '；') + '。')
Add-Line "- 用例 $($caseRows.Count) 个：宏实测过 $(@($caseRows | Where-Object { $_.macro -eq '过' }).Count) 个、只有自述 $(@($caseRows | Where-Object { $_.macro -match '自述' }).Count) 个、没跑或没跑全 $(@($caseRows | Where-Object { $_.macro -match '^没跑' }).Count) 个、不过 $(@($caseRows | Where-Object { $_.macro -eq '不过' }).Count) 个。"
if ($windowRows.Count) { Add-Line "- 窗 $($windowRows.Count) 个（片段的 ``replay_windows`` 里登记的并窗、腿内切窗、取自别人那条链的窗；没有自己的宏，宏这一层不适用）：规划过 $(@($windowRows | Where-Object { Test-Pass $_.plan }).Count) 个；回放实测过 $(@($windowRows | Where-Object { Test-Current $_ '过' }).Count) 个、落在已知缺口上 $(@($windowRows | Where-Object { Test-Current $_ '已知缺口' }).Count) 个、不过 $(@($windowRows | Where-Object { Test-Current $_ '不过' }).Count) 个、还没有回放结果 $(@($windowRows | Where-Object { -not (Test-CurrentAny $_) }).Count) 个；标了已知缺口的 $(@($windowRows | Where-Object { $_.status -eq 'known_gap' }).Count) 个。" }
$passPerBackend = (@($currentBackends | ForEach-Object { $b = $_; "$b 上过 $(@($caseRows | Where-Object { $_.per_backend[$b] -eq '过' }).Count) 个、不过 $(@($caseRows | Where-Object { $_.per_backend[$b] -eq '不过' }).Count) 个" }) -join '；')
Add-Line "- 回放（实测，数的是套件用例，窗另见上一条）：$passPerBackend；r5 上跑过 $(@($caseRows | Where-Object { $_.per_backend['r5'] }).Count) 个，其中红 $(@($caseRows | Where-Object { $_.per_backend['r5'] -eq '红' }).Count) 个（新旧对照）。还没有回放结果的 $(@($caseRows | Where-Object { -not (Test-CurrentAny $_) }).Count) 个。"
$runsPerBackend = (@($backends | ForEach-Object { $b = $_; $ofBackend = @($replayRuns | Where-Object { $_.backend -eq $b }); if ($ofBackend.Count) { "$b $($ofBackend.Count) 条（红 $(@($ofBackend | Where-Object { $_.verdict -eq 'fail' }).Count)）" } }) | Where-Object { $_ }) -join '；'
Add-Line "- 回放结果按后端（数的是回放用例的结果条数，重跑的都算）：$runsPerBackend。"
# 回放用例本身（目录里的文件）：每个后端上最近一趟
$neverRun = @($allReplayCases.Values | Where-Object { $name = $_.name; -not @($currentBackends | Where-Object { $replayLatest["$name|$_"] }).Count })
$latestPerBackend = (@($currentBackends | ForEach-Object {
            $b = $_
            $latest = @($allReplayCases.Keys | ForEach-Object { $replayLatest["$_|$b"] } | Where-Object { $_ })
            if ($latest.Count) { "$b 上跑过 $($latest.Count) 条（最近一趟过 $(@($latest | Where-Object { $_.verdict -eq 'pass' }).Count)、红 $(@($latest | Where-Object { $_.verdict -eq 'fail' }).Count)）" }
        }) | Where-Object { $_ }) -join '；'
$linkedReplays = @($allReplayCases.Keys | Where-Object { $name = $_; @($rows | Where-Object { $_.linked -contains $name }).Count }).Count
Add-Line "- 回放用例 $($allReplayCases.Count) 条（$($knownCaseDirs.Count) 个目录里带 ``steps`` 的文件）：被用例或窗点到的 $linkedReplays 条，没被点到、只在回放层的 $($allReplayCases.Count - $linkedReplays) 条（管线行为、会话机制这一类造不出宏的，逐条在总报告里）；$latestPerBackend；r5 以外的后端上一趟都没跑过的 $($neverRun.Count) 条（名字在第四节末尾）；自己标了「本来就红」的 $(@($allReplayCases.Values | Where-Object { $_.declared_red }).Count) 条。"
Add-Line "- 标了已知缺口的用例 $(@($caseRows | Where-Object { $_.status -eq 'known_gap' }).Count) 个。"
if ($badRows.Count) {
    Add-Line "- **不过的 $($badRows.Count) 个**（用例和窗一起数；已知缺口、r5 上的红不算）："
    foreach ($row in $badRows) { Add-Line "  - ``$($row.id)``（$($row.owner)$(if ($row.is_window) { '，窗' })）：宏 $($row.macro)；导出 $($row.export)；规划 $($row.plan)；回放 $($row.replay)" }
}
else { Add-Line '- 不过的用例和窗：0 个（已知缺口、r5 上的红不算）。' }
# 回放用例里最近一趟是红的（r5 以外的后端）：不管有没有套件用例点到它，都列出来
$redReplays = [System.Collections.Generic.List[string]]::new()
$redReplayRows = [System.Collections.Generic.List[object]]::new()
foreach ($name in ($replayNames | Sort-Object)) {
    foreach ($backend in $currentBackends) {
        $run = $replayLatest["$name|$backend"]
        if (-not $run -or $run.verdict -ne 'fail') { continue }
        $users = @($rows | Where-Object { $_.linked -contains $name })
        $gapUsers = @($users | Where-Object { $_.status -eq 'known_gap' -or $_.replay -match '已知缺口' })
        # 回放用例自己声明的「本来就红」：_expected_red 里写了原因的对照，或者标了 known_red（缺省不跑的那一种）
        $declared = if ($replayDeclaredRed.ContainsKey($name)) { [string]$replayDeclaredRed[$name] } else { '' }
        $hasFile = $allReplayCases.Contains($name)
        $firstUsers = ($users | Select-Object -First 3 | ForEach-Object { $_.id }) -join '、'
        $note = if ($declared -and $declared -ne 'known_red') { "本来就该红：$declared" }
        elseif ($declared) { '用例自己标了 known_red' + $(if ($users.Count) { "；点到它的：$firstUsers" } else { '' }) }
        elseif (-not $users.Count) { if ($hasFile) { '没有声明，也没有套件用例点到它' } else { '没有套件用例点到它，用例文件也不在了' } }
        elseif ($gapUsers.Count) { "已知缺口：$firstUsers" }
        else { "点到它的：$firstUsers" }
        # 归类：declared_red 自己声明了；known_gap 点到它的用例声明了缺口；red 点到它的用例因此不过；
        #       unlinked 没声明、没人点到、文件还在（要人看）；stale 没人点到、文件已经不在（旧结果）
        $kind = if ($declared) { 'declared_red' } elseif (-not $users.Count) { if ($hasFile) { 'unlinked' } else { 'stale' } } elseif ($gapUsers.Count) { 'known_gap' } else { 'red' }
        $redReplays.Add("``$name``（$backend，$($run.time -replace '^\d{4}-\d{2}-\d{2} ', '' -replace ':\d{2}$', '')；$note）")
        $redReplayRows.Add([pscustomobject]@{ case = $name; backend = $backend; time = $run.time; where = $run.where; kind = $kind; note = $note; cases = @($users | ForEach-Object { $_.id }) })
    }
}
if ($redReplays.Count) { Add-Line "- 回放用例里最近一趟是红的 $($redReplays.Count) 条（r5 以外的后端，一个后端算一条；是缺陷、已知缺口还是用例没写对，看各人的记录）：$($redReplays -join '、')。" }
$looseReds = @($redReplayRows | Where-Object { $_.kind -eq 'unlinked' })
if ($looseReds.Count) { Add-Line ("- 上面这些里**没有声明、也没有套件用例点到的 $($looseReds.Count) 条**（" + (($looseReds | ForEach-Object { "``$($_.case)`` $($_.backend)" }) -join '、') + '）：不进「不过」的数（那里只数用例和窗），但一条命令 `Invoke-SuiteAll.ps1` 的退出码算它们（只算那一次点的后端，缺省主线）。') }
Add-Line
Add-Line '按负责人：'
Add-Line
Add-Line '| 谁 | 用例 | 宏过 | 有回放实测的 | 回放过（r5 以外任一后端） | r5 上红 | 标了已知缺口 | 窗 | 窗回放过 |'
Add-Line '|---|---|---|---|---|---|---|---|---|'
foreach ($group in ($rows | Group-Object owner | Sort-Object Name)) {
    $g = @($group.Group | Where-Object { -not $_.is_window })
    $gw = @($group.Group | Where-Object { $_.is_window })
    Add-Line ("| {0} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} |" -f $(if ($group.Name -eq 'suite') { '主清单' } else { $group.Name }), $g.Count, @($g | Where-Object { $_.macro -eq '过' }).Count, @($g | Where-Object { @($_.per_backend.Values | Where-Object { $_ }).Count }).Count, @($g | Where-Object { Test-Current $_ '过' }).Count, @($g | Where-Object { $_.per_backend['r5'] -eq '红' }).Count, @($g | Where-Object { $_.status -eq 'known_gap' }).Count, $gw.Count, @($gw | Where-Object { Test-Current $_ '过' }).Count)
}
Add-Line
Add-Line '## 二、按变更类型'
Add-Line
Add-Line '「回放过」只数实测（r5 以外的任一后端上最近一趟是过）；「r5 红」是「红的 / 在 r5 上跑过的」。「仓里的链」是 `tests/increment_replay/cases/` 里按登记号挂到这一格的回放用例（括号里是其中最近一趟过了的条数）。'
Add-Line
Add-Line '「窗」是片段里登记的窗落在这一格的个数；「规划过」「回放过」「已知缺口」「不过」这几栏连窗一起数，「宏过」「导出过」只有用例有。'
Add-Line
Add-Line '| 编号 | 类型 | 用例（旧+新） | 窗 | 宏过 | 导出过 | 规划过 | 回放过 | 回放过（自述） | r5 红 | 已知缺口 | 不过 | 仓里的链 | 走到哪 |'
Add-Line '|---|---|---|---|---|---|---|---|---|---|---|---|---|---|'
foreach ($row in $ctRows) {
    Add-Line ("| {0} | {1} | {2}（{3}+{4}） | {15} | {5} | {6} | {7} | {8} | {9} | {10} | {11} | {12} | {13} | {14} |" -f $row.id, (Format-Cell $row.name 24), $row.total, $row.old, $row.new, $row.macro, $row.export, $row.plan, $row.replay, $row.replay_self, $row.r5, $row.gaps, $row.bad, $(if ($row.repo.Count) { "$($row.repo.Count)（$($row.repo_pass)）" } else { '0' }), $row.reach, $row.windows)
}
Add-Line
Add-Line '## 三、按用例'
Add-Line
Add-Line '「宏」「回放」两栏是实测（看流水账和回放结果）；带「自述」的是没有证据、取负责人的话。其余几栏都是自述。'
foreach ($group in ($rows | Group-Object source)) {
    $owner = $group.Group[0].owner
    Add-Line
    $windowsHere = @($group.Group | Where-Object { $_.is_window }).Count
    Add-Line "### $($group.Name)（$owner，$($group.Count - $windowsHere) 个用例$(if ($windowsHere) { "、$windowsHere 个窗" })）"
    Add-Line
    Add-Line '| 用例 | 格 | 宏 | 导出 | 规划 | 回放 | 对导出 | L3 | 状态 | 回放用例 |'
    Add-Line '|---|---|---|---|---|---|---|---|---|---|'
    foreach ($row in $group.Group) {
        Add-Line ("| ``{0}``{10} | {1} | {2} | {3} | {4} | {5} | {6} | {7} | {8} | {9} |" -f $row.id, (($row.cts | ForEach-Object { $_ -replace '^CT-', '' }) -join ' '), $row.macro, $row.export, $row.plan, (Format-Cell $row.replay), $row.gen, $row.l3, $row.status, (Format-Cell (($row.linked | Sort-Object) -join '、') 60), $(if ($row.is_window) { '（窗）' } else { '' }))
    }
}
Add-Line
Add-Line '## 四、回放用例最近一趟的结果'
Add-Line
Add-Line '每个后端取最近一趟。时刻是那一趟开始的时刻。'
Add-Line
Add-Line ('| 回放用例 | ' + ($backends -join ' | ') + ' | 点到它的套件用例 / 登记号 |')
Add-Line ('|---|' + ((@($backends | ForEach-Object { '---' })) -join '|') + '|---|')
foreach ($name in ($replayNames | Sort-Object)) {
    $cells = @(foreach ($backend in $backends) { $run = $replayLatest["$name|$backend"]; if ($run) { '{0}（{1}）' -f $(if ($run.verdict -eq 'pass') { '过' } else { '不过' }), ($run.time -replace '^\d{4}-\d{2}-\d{2} ', '' -replace ':\d{2}$', '') } else { '' } })
    $users = @($rows | Where-Object { $_.linked -contains $name } | ForEach-Object { $_.id })
    $tag = if ($users.Count) { ($users | Select-Object -First 6) -join '、' } elseif ($repoReplay.ContainsKey($name)) { ($repoReplay[$name] | Select-Object -First 6) -join '、' } else { '（没有套件用例点到它）' }
    if ($users.Count -gt 6) { $tag += " 等 $($users.Count) 个" }
    Add-Line ("| ``{0}`` | {1} | {2} |" -f $name, ($cells -join ' | '), (Format-Cell $tag 90))
}
if ($neverRun.Count) {
    Add-Line
    Add-Line ("目录里有文件、在 r5 以外的后端上一趟都没跑过的回放用例 $($neverRun.Count) 条：" + ((@($neverRun | Group-Object dir | ForEach-Object { "``$($_.Name)`` " + (($_.Group | ForEach-Object { "``$($_.name)``" + $(if ($_.known_red) { '（标了 known_red，缺省不跑）' } else { '' }) }) -join '、') }) -join '；') + '。'))
}
Add-Line
Add-Line '## 五、怎么读、哪里不准'
Add-Line
Add-Line '- **实测的只有两栏**：「宏」看跑宏入口的流水账（每条腿最近一次写步），「回放」看回放入口每一趟留下的结果。别的栏是负责人写在片段里的话，按里面的词归成「过 / 过* / 不过 / 已知缺口 / 没跑 / 有记录」；归类规则在脚本的 `Get-Token` 里，归得不对以片段原文为准。'
Add-Line '- 「过*」= 过了，但同一格里注明还有没做的部分。「有记录」= 写了东西、没有结论词（多半是一份逐件比对的数）。'
Add-Line '- 回放结果按「片段里点到名字的回放用例」挂到套件用例上：一条回放用例常常盖好几个套件用例，它红了，点到它的用例这一栏都算不过。两种情形算已知缺口：点到它的用例（或窗）自己声明了已知缺口；红的那几条回放用例每一条都自己标了「本来就红」（用例文件里的 `known_red` 或 `_expected_red`）。片段里没点名字的，这一栏只有自述。'
if ($windowRows.Count) { Add-Line "- **窗**：片段顶层 ``replay_windows`` 下面各个数组里带 ``id`` 的项，这次读到 $($windowRows.Count) 个。它们是在别的用例录下来的会话链上换起止取出来的，没有自己的宏：「宏」记不适用，「规划」取 ``checks.plan_offline``，「回放」和用例一样按点到名字的回放用例挂实测。清单的读法（``Get-SuiteManifest.ps1``、跑宏的入口）不把它们当用例，所以清单是 $($caseRows.Count) 个、这张表多出 $($windowRows.Count) 行；第三节里标了「（窗）」。" }
if ($windowDupes.Count) { Add-Line ("- 有 $($windowDupes.Count) 个窗的名字和别的用例或窗撞了，没有进表：" + (($windowDupes | ForEach-Object { "``$_``" }) -join '、') + '。') }
Add-Line '- r5 是修之前的后端：在它上面红是新旧对照要的结果，不计入「不过」。r6 不含 r6 之后并进主线的修复，在 r6 上红不一定是缺陷。'
Add-Line ('- 后端这几栏是结果里出现过的名字：' + ($backends -join '、') + '。「主线」是回放入口缺省用的本地调试版（跟着并入重编）；别的名字（r6、r7……）是跑回放时用 `-Backend` 指的发布版，各含到哪一次并入，以协调者的并入记录为准，这张表不知道。')
if ($oddBackends.Count) { Add-Line ('- 结果里还有 ' + $oddBackends.Count + ' 个认不出的后端名，没有进表：' + (($oddBackends | ForEach-Object { "``$_``" }) -join '、') + '。') }
Add-Line '- 主清单的 46 个用例自己不带变更类型，归哪一格取总表「执行资产」一栏；总表没点到的见下。它们的「导出」这一轮没有重做（上一次整套对拍是 2026-11-04，结论在总表第 6 节）。'
if ($unmappedMain.Count) { Add-Line ("- 主清单里总表没点到的用例 $($unmappedMain.Count) 个（不进第二节的数）：" + (($unmappedMain | ForEach-Object { "``$_``" }) -join '、') + '。') }
if (@($merged.problems).Count) { Add-Line "- 清单核对有 $(@($merged.problems).Count) 处问题（``Get-SuiteManifest.ps1 -Mode check`` 看明细）。" }
if ($catalogRows.Count) {
    Add-Line
    Add-Line '## 六、各人的目录和清单对不对得上'
    Add-Line
    Add-Line '目录是各人手写的，这里只对名字。「目录里没提到的」= 清单里有这个用例（或窗）、他的目录里没出现过这个名字。「目录里有、清单里没有的」= 目录里出现过、长得像用例号、但哪一份清单里都没有的名字：计划了没做的、改了名的、写了宏还没登记的，要负责人自己认。名字其实是回放用例的（回放用例不进清单）单独数在一栏，不算缺。'
    Add-Line
    Add-Line '| 谁 | 目录文件（改动时刻） | 清单里的用例 + 窗 | 目录里没提到的 | 是回放用例名字的 | 目录里有、清单里没有的名字 |'
    Add-Line '|---|---|---|---|---|---|'
    foreach ($row in $catalogRows) {
        $missing = if (-not $row.file) { '（没有目录文件）' } elseif ($row.missing.Count) { "$($row.missing.Count) 个：" + (($row.missing | Select-Object -First 12 | ForEach-Object { "``$_``" }) -join '、') + $(if ($row.missing.Count -gt 12) { ' 等' } else { '' }) } else { '0' }
        $extra = if ($row.extra.Count) { "$($row.extra.Count) 个：" + (($row.extra | Select-Object -First 60 | ForEach-Object { "``$_``" }) -join '、') + $(if ($row.extra.Count -gt 60) { ' 等' } else { '' }) } else { '0' }
        Add-Line ("| {0} | {1} | {2} | {3} | {4} | {5} |" -f $row.owner, $(if ($row.file) { "``$($row.file)``（$($row.time)）" } else { '—' }), "$($row.cases) + $($row.windows)", $missing, $row.replay_only.Count, $extra)
    }
}

$outDir = Split-Path -Parent $Out
if ($outDir -and -not (Test-Path -LiteralPath $outDir)) { [void](New-Item -ItemType Directory -Force -Path $outDir) }
[IO.File]::WriteAllText($Out, $sb.ToString(), (New-Object Text.UTF8Encoding($false)))

# ── 9. 给机器读的那一份：逐用例逐层四选一（一条命令 Invoke-SuiteAll.ps1 拿它写总报告、定退出码）──
$layerNames = [ordered]@{ macro = '宏'; export = '导出'; plan = '规划'; replay = '回放'; gen = '对导出'; l3 = 'L3' }
$byLayer = [ordered]@{}
foreach ($key in $layerNames.Keys) {
    $count = [ordered]@{}
    foreach ($word in @('过', '不过', '已知缺口', '没跑', '不适用')) { $count[$word] = @($rows | Where-Object { $_.four[$key] -eq $word }).Count }
    $byLayer[$key] = $count
}
$overallCount = [ordered]@{}
foreach ($word in @('过', '不过', '已知缺口', '没跑')) { $overallCount[$word] = @($rows | Where-Object { $_.overall -eq $word }).Count }
$exitCode = if ($badRows.Count) { 1 } else { 0 }
$doc = [ordered]@{
    generated = $now
    command = 'scripts/e3d/increment_suite/Get-SuiteCoverage.ps1'
    exit_code = $exitCode
    inputs = [ordered]@{
        cases = $caseRows.Count; main = $mainIds.Count; fragments = $fragCases.Count; windows = $windowRows.Count; sources = @($merged.sources).Count
        journal_rows = $journalRows; replay_result_files = $replayFiles; extra_replay_tsv = $ExtraReplayTsv.Count; manifest_problems = @($merged.problems).Count
    }
    layers = $layerNames
    backends = @($backends)
    current_backends = @($currentBackends)
    counts = [ordered]@{ overall = $overallCount; by_layer = $byLayer }
    cases = @($rows | ForEach-Object {
            $row = $_
            [ordered]@{
                id = $row.id; kind = $(if ($row.is_window) { 'window' } else { 'case' }); source = $row.source; owner = $row.owner; status = $row.status
                change_types = @($row.cts); situation = [string]$row.situation
                overall = $row.overall; bad = [bool](@($badRows | Where-Object { $_.id -ceq $row.id }).Count)
                four = $row.four
                said = [ordered]@{ macro = $row.macro; export = $row.export; plan = $row.plan; replay = $row.replay; gen = $row.gen; l3 = $row.l3 }
                macro_measured = [bool]$row.macro_measured; replay_measured = [bool]$row.replay_measured; legs = $row.legs
                per_backend = $row.per_backend; replay_cases = @($row.linked | Sort-Object)
                known_gap = $row.gap; tickets = @($row.tickets)
            }
        })
    change_types = @($ctRows | ForEach-Object {
            [ordered]@{ id = $_.id; name = $_.name; cases = $_.total; main = $_.old; fragments = $_.new; windows = $_.windows; macro = $_.macro; export = $_.export; plan = $_.plan; replay = $_.replay; replay_self = $_.replay_self; known_gap = $_.gaps; bad = $_.bad; repo_replays = @($_.repo); reach = $_.reach }
        })
    red_replays = @($redReplayRows)
    # 回放用例本身：目录里的文件，加上只在结果里见过名字的（文件已经不在，或者结果来自补充表）
    replay_cases = @(@($allReplayCases.Keys) + @($replayNames | Where-Object { -not $allReplayCases.Contains($_) } | Sort-Object) | ForEach-Object {
            $name = [string]$_
            $file = if ($allReplayCases.Contains($name)) { $allReplayCases[$name] } else { $null }
            $latest = [ordered]@{}
            foreach ($backend in $backends) { $run = $replayLatest["$name|$backend"]; if ($run) { $latest[$backend] = [ordered]@{ verdict = $run.verdict; time = $run.time; seconds = $run.seconds; where = $run.where } } }
            [ordered]@{
                name = $name; dir = $(if ($file) { $file.dir } else { '' }); owner = $(if ($file) { $file.owner } else { '' }); project = $(if ($file) { $file.project } else { '' })
                known_red = [bool]($file -and $file.known_red); declared_red = $(if ($file) { $file.declared_red } else { '' })
                doc = $(if ($file) { $file.doc } else { '' }); registry = @($(if ($file) { $file.registry } else { @() }))
                latest = $latest; linked_by = @($rows | Where-Object { $_.linked -contains $name } | ForEach-Object { $_.id })
            }
        })
    replay_case_dirs = @($knownCaseDirs.Keys | ForEach-Object { [ordered]@{ dir = (Get-ShortDir ([string]$_)); owner = [string]$knownCaseDirs[$_]; exists = (Test-Path -LiteralPath ([string]$_)) } })
    catalogs = @($catalogRows)
}
[IO.File]::WriteAllText($Json, ((ConvertTo-Json -InputObject $doc -Depth 8) + "`n"), (New-Object Text.UTF8Encoding($false)))

if (-not $Quiet) {
    Write-Host ("覆盖表 → {0}（机器读的那一份 {1}）" -f $Out, (Split-Path -Leaf $Json))
    Write-Host ("  {0} 类里有用例的 {1} 类，实测回放过的 {2} 类；用例 {3} 个、窗 {4} 个，不过 {5} 个" -f $cts.Count, $withCases, $replayCts, $caseRows.Count, $windowRows.Count, $badRows.Count)
    foreach ($row in $badRows) { Write-Host ("  不过：{0}（{1}{4}）宏 {2}；回放 {3}" -f $row.id, $row.owner, $row.macro, $row.replay, $(if ($row.is_window) { '，窗' } else { '' })) }
}
exit $exitCode
