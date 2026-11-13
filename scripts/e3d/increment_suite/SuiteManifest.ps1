<#
    增量套件清单的读法：主清单 suite-manifest.json 加 manifest.d/*.json 里的片段，合成一张用例表。
    点源引入，不单独跑；命令行入口是 Get-SuiteManifest.ps1，跑宏入口 Invoke-SuiteMacro.ps1 的 -Case 也读它。

    片段的规矩（谁写、写什么）：
      * 文件名 manifest.d/<T1|T2|…>-<主题>.json，UTF-8；顶层 version=1、owner、topic、copy、cases，可选 mdb、sites、setup、teardown、chains、probes。
      * 用例：id（变更类型的格号加短名，如 D5-pjoi-only，全套唯一）、apply、restore（可为 null）、可选 chain（字符串 = 引用本片段 chains[].id；
        数组 = 腿）、db（库号或库号数组）、expected_net、change_types、status、expect、known_gap、evidence……
      * 规定之外的键原样带过去（Read-SuiteManifest 给出的每条用例的 raw 就是片段里那一条的原文）。
    主清单里的用例没有 status / db：status 记成 suite，db 取伴生库号（有 companion_dbnum 时）或主库号。

    键按区分大小写读（片段里可以有只差大小写的键），所以这里用 ConvertFrom-Json -AsHashtable，要 PowerShell 7。
#>

function Read-SuiteJson([string]$Path) {
    return (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable)
}

# 腿的写法：宏的相对路径，后面可以带 @库号 或 @库号+库号；没带的补上用例自己的库。
function Add-LegDb([string]$Leg, [string[]]$Db) {
    if ($Leg -match '@[0-9+]+$' -or -not $Db.Count) { return $Leg }
    return ('{0}@{1}' -f $Leg, ($Db -join '+'))
}

function Read-SuiteManifest {
    param([Parameter(Mandatory)][string]$SuiteDir)

    $problems = [System.Collections.Generic.List[string]]::new()
    $warnings = [System.Collections.Generic.List[string]]::new()
    $cases = [System.Collections.Generic.List[object]]::new()
    $sources = [System.Collections.Generic.List[object]]::new()
    $seen = @{}
    # 变更类型总表各族的最大编号（2026-11-12 起 D 族到 D11）。与 tests/increment_cases_registry.rs 的 CHANGE_TYPE_FAMILIES 一起改。
    $familyMax = @{ S = 7; A = 13; C = 4; D = 11; W = 10 }
    $nets = @('modified', 'cancelled', 'deleted', 'unchanged', 'added')
    $statuses = @('draft', 'macro_ok', 'verified', 'known_gap')

    $macroExists = { param([string]$rel) $p = ($rel -replace '@[0-9+]+$', '') -replace '/', '\'; return (Test-Path -LiteralPath (Join-Path $SuiteDir $p) -PathType Leaf) }
    $asList = { param($value) if ($null -eq $value) { return @() } elseif ($value -is [string]) { return @($value) } else { return @($value | ForEach-Object { [string]$_ }) } }
    $get = { param($table, [string]$key) if ($table -is [System.Collections.IDictionary] -and $table.Contains($key)) { return $table[$key] } else { return $null } }

    # ── 主清单 ──
    $mainPath = Join-Path $SuiteDir 'suite-manifest.json'
    $main = Read-SuiteJson $mainPath
    $mainChains = @{}
    foreach ($c in @($main['chains'])) { $mainChains[[string]$c['id']] = @($c['legs'] | ForEach-Object { [string]$_ }) }
    $primaryDb = [string]$main['databases']['primary']['dbnum']
    $mainCount = 0
    foreach ($c in @($main['cases'])) {
        $id = [string]$c['id']
        $mainCount++
        if ($seen.ContainsKey($id)) { $problems.Add("suite-manifest.json：用例号 $id 重复") } else { $seen[$id] = 'suite-manifest.json' }
        $companion = & $get $c 'companion_dbnum'
        $db = if (& $get $c 'db') { & $asList $c['db'] } elseif ($companion) { @([string]$companion) } else { @($primaryDb) }
        $chain = & $get $c 'chain'
        $chainId = ''
        if ($chain -is [string]) {
            $chainId = $chain
            if ($mainChains.ContainsKey($chain)) { $legs = $mainChains[$chain] } else { $legs = @(); $problems.Add("suite-manifest.json：$id 引用的链 $chain 不在 chains 里") }
        }
        elseif ($null -ne $chain) { $legs = & $asList $chain }
        else { $legs = @([string]$c['apply']); if ($c['restore']) { $legs += [string]$c['restore'] } }
        foreach ($leg in $legs) { if (-not (& $macroExists $leg)) { $problems.Add("suite-manifest.json：$id 的宏不存在 $leg") } }
        $setup = @('common/setup_8000.mac@' + $primaryDb)
        if ($companion -or $db -contains '7998') { $setup += 'h03/setup_7998.mac@7998' }
        $cases.Add([pscustomobject][ordered]@{
                id = $id; source = 'suite-manifest.json'; owner = 'suite'; topic = ''; copy = 'ams'; mdb = ''
                db = @($db); legs = @($legs | ForEach-Object { Add-LegDb $_ $db }); chain = $chainId
                apply = [string]$c['apply']; restore = $(if ($c['restore']) { [string]$c['restore'] } else { $null })
                expected_net = [string]$c['expected_net']; status = 'suite'; change_types = @(); registration = [string]$c['registration']
                tickets = @(); name_case = $null; situation = ''; known_gap = $null; evidence = [string](& $get $c 'tty_status')
                setup = @($setup); teardown = @(); touches_production = @(); raw = $c
            })
    }
    $sources.Add([pscustomobject]@{ file = 'suite-manifest.json'; owner = 'suite'; cases = $mainCount })

    # ── 片段 ──
    $fragDir = Join-Path $SuiteDir 'manifest.d'
    $fragFiles = if (Test-Path -LiteralPath $fragDir) { @(Get-ChildItem -LiteralPath $fragDir -Filter '*.json' -File | Sort-Object Name) } else { @() }
    foreach ($file in $fragFiles) {
        $tag = "manifest.d/$($file.Name)"
        $frag = $null
        try { $frag = Read-SuiteJson $file.FullName } catch { $problems.Add("${tag}：JSON 读不出来：$($_.Exception.Message)"); continue }
        if ($frag -isnot [System.Collections.IDictionary]) { $problems.Add("${tag}：顶层不是对象"); continue }
        if ($file.Name -notmatch '^(T[0-9]+)-[A-Za-z0-9-]+\.json$') { $problems.Add("${tag}：文件名要是 <T1|T2|…>-<主题>.json") }
        $owner = [string](& $get $frag 'owner')
        if ($file.Name -match '^(T[0-9]+)-') { if ($owner -ne $Matches[1]) { $problems.Add("${tag}：owner=$owner 与文件名前缀 $($Matches[1]) 不一致") } }
        if ([string](& $get $frag 'version') -ne '1') { $problems.Add("${tag}：version 要是 1") }
        $fragCopy = [string](& $get $frag 'copy'); if (-not $fragCopy) { $fragCopy = 'ams' }
        if ($fragCopy -notin @('ams', 'aps')) { $problems.Add("${tag}：copy=$fragCopy 不认识（ams / aps）") }
        $fragMdb = [string](& $get $frag 'mdb')
        $fragChains = @{}
        foreach ($c in @(& $get $frag 'chains')) {
            if ($null -eq $c) { continue }
            $cid = [string]$c['id']; $legs = & $asList $c['legs']
            $fragChains[$cid] = $legs
            foreach ($leg in $legs) { if (-not (& $macroExists $leg)) { $problems.Add("${tag}：链 $cid 的宏不存在 $leg") } }
        }
        foreach ($key in 'setup', 'teardown') { foreach ($m in (& $asList (& $get $frag $key))) { if (-not (& $macroExists $m)) { $problems.Add("${tag}：$key 的宏不存在 $m") } } }
        foreach ($p in @(& $get $frag 'probes')) { if ($null -ne $p -and -not (& $macroExists ([string]$p['macro']))) { $problems.Add("${tag}：探针宏不存在 $($p['macro'])") } }
        $n = 0
        foreach ($c in @(& $get $frag 'cases')) {
            if ($null -eq $c) { continue }
            $n++
            $id = [string](& $get $c 'id')
            $where = "${tag} 的 $id"
            if ($id -cnotmatch '^([SACDW])([1-9][0-9]?)-[a-z0-9][a-z0-9-]*$') { $problems.Add("${where}：用例号要是 <格号>-<短名>（如 D5-pjoi-only；小写字母、数字、短横）") }
            elseif ([int]$Matches[2] -gt $familyMax[$Matches[1]]) { $problems.Add("${where}：变更类型总表里没有 $($Matches[1])$($Matches[2]) 这一格") }
            if ($seen.ContainsKey($id)) { $problems.Add("${where}：用例号和 $($seen[$id]) 里的撞了") } else { $seen[$id] = $tag }
            $db = & $asList (& $get $c 'db')
            if (-not $db.Count) { $problems.Add("${where}：没有 db（SAVEWORK 落在哪个库）") }
            elseif (@($db | Where-Object { $_ -notmatch '^[0-9]+$' }).Count) { $problems.Add("${where}：db 要是库号或库号数组") }
            $chain = & $get $c 'chain'
            $chainId = ''
            $apply = [string](& $get $c 'apply'); $restore = & $get $c 'restore'
            if ($chain -is [string]) {
                $chainId = $chain
                if ($fragChains.ContainsKey($chain)) { $legs = $fragChains[$chain] } else { $legs = @(); $problems.Add("${where}：引用的链 $chain 不在本片段的 chains 里") }
            }
            elseif ($null -ne $chain) { $legs = & $asList $chain }
            else {
                $legs = @()
                if (-not $apply) { $problems.Add("${where}：没有 apply，也没有 chain") } else { $legs += $apply }
                if ($restore) { $legs += [string]$restore }
            }
            foreach ($m in @($apply, $restore) + $legs) { if ($m -and -not (& $macroExists ([string]$m))) { $problems.Add("${where}：宏不存在 $m") } }
            $cts = & $asList (& $get $c 'change_types')
            if (-not $cts.Count) { $problems.Add("${where}：没有 change_types") }
            foreach ($ct in $cts) {
                if ($ct -cnotmatch '^CT-([SACDW])([1-9][0-9]?)$' -or [int]$Matches[2] -gt $familyMax[$Matches[1]]) { $problems.Add("${where}：变更类型 $ct 不是总表里的 CT-<族><n>") }
            }
            $net = [string](& $get $c 'expected_net')
            if ($net -notin $nets) { $problems.Add("${where}：expected_net=$net 不在 $($nets -join ' / ') 里") }
            $status = [string](& $get $c 'status')
            if ($status -notin $statuses) { $problems.Add("${where}：status=$status 不在 $($statuses -join ' / ') 里") }
            $gap = & $get $c 'known_gap'
            if ($status -eq 'known_gap' -and -not $gap) { $problems.Add("${where}：status=known_gap 要在 known_gap 里写现在的表现和草稿票") }
            $expect = & $get $c 'expect'
            if ($expect -isnot [System.Collections.IDictionary]) { $warnings.Add("${where}：没有 expect（该重建 / 该删 / 不该动的）") }
            elseif (-not @(& $get $expect 'untouched').Count) { $warnings.Add("${where}：expect.untouched 是空的——至少要一条「不该动的」当对照") }
            if (-not [string](& $get $c 'situation')) { $warnings.Add("${where}：没有 situation（一句话说用户做了什么）") }
            if ($status -in @('macro_ok', 'verified') -and -not [string](& $get $c 'evidence')) { $warnings.Add("${where}：status=$status 却没有 evidence（跑通那一批的证据目录）") }
            $caseCopy = [string](& $get $c 'copy'); if (-not $caseCopy) { $caseCopy = $fragCopy }
            $caseMdb = [string](& $get $c 'mdb'); if (-not $caseMdb) { $caseMdb = $fragMdb }
            $cases.Add([pscustomobject][ordered]@{
                    id = $id; source = $tag; owner = $owner; topic = [string](& $get $frag 'topic'); copy = $caseCopy; mdb = $caseMdb
                    db = @($db); legs = @($legs | ForEach-Object { Add-LegDb $_ $db }); chain = $chainId
                    apply = $apply; restore = $(if ($restore) { [string]$restore } else { $null })
                    expected_net = $net; status = $status; change_types = @($cts); registration = [string](& $get $c 'registration')
                    tickets = @(& $asList (& $get $c 'tickets')); name_case = (& $get $c 'name_case'); situation = [string](& $get $c 'situation')
                    known_gap = $gap; evidence = [string](& $get $c 'evidence')
                    setup = @(& $asList (& $get $frag 'setup') | ForEach-Object { Add-LegDb $_ $db })
                    teardown = @(& $asList (& $get $frag 'teardown') | ForEach-Object { Add-LegDb $_ $db })
                    touches_production = @(& $asList (& $get $c 'touches_production')); raw = $c
                })
        }
        $sources.Add([pscustomobject]@{ file = $tag; owner = $owner; cases = $n })
    }
    return [pscustomobject]@{ cases = $cases; sources = $sources; problems = $problems; warnings = $warnings }
}

# 把若干用例展开成入口的步：同一条链只跑一次；先跑完一个用例（apply → restore 或链的各腿）再跑下一个。
function Expand-SuiteCaseSteps {
    param([Parameter(Mandatory)]$Merged, [Parameter(Mandatory)][string[]]$Ids, [switch]$WithSetup, [switch]$WithTeardown)
    $picked = [System.Collections.Generic.List[object]]::new()
    foreach ($id in $Ids) {
        $hit = @($Merged.cases | Where-Object { $_.id -ceq $id })
        if (-not $hit.Count) { throw "清单和片段里都没有用例 $id" }
        $picked.Add($hit[0])
    }
    $copies = @($picked | ForEach-Object copy | Sort-Object -Unique)
    $mdbs = @($picked | ForEach-Object mdb | Sort-Object -Unique)
    if ($copies.Count -gt 1 -or $mdbs.Count -gt 1) { throw "这几个用例不在同一个副本 / MDB 上（$($copies -join '、')；$($mdbs -join '、')），分成几批跑" }
    $steps = [System.Collections.Generic.List[string]]::new()
    if ($WithSetup) { foreach ($m in @($picked | ForEach-Object { $_.setup } | Select-Object -Unique)) { $steps.Add([string]$m) } }
    $doneChains = @{}
    foreach ($c in $picked) {
        if ($c.chain) { $key = "$($c.source)|$($c.chain)"; if ($doneChains.ContainsKey($key)) { continue }; $doneChains[$key] = $true }
        foreach ($leg in $c.legs) { $steps.Add([string]$leg) }
    }
    if ($WithTeardown) { foreach ($m in @($picked | ForEach-Object { $_.teardown } | Select-Object -Unique)) { $steps.Add([string]$m) } }
    return [pscustomobject]@{ steps = @($steps); copy = $copies[0]; mdb = $mdbs[0]; cases = @($picked) }
}
