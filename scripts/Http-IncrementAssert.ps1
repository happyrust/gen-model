<#
.SYNOPSIS
    纯 HTTP 增量断言（L2-1）：SAVEWORK 前抓读面快照，收敛到 in_sync 后复抓对拍。
    只走 aios-database 的 HTTP 面（/dbnums、/model/records、/element/attributes、
    /tree/children、/search、/update/pending-units、/batch-failures、/error-log），
    不读库、不碰 e3d-io——这是「内置 plant-ui 之后浏览器能看到什么」那个口径的验收。

.DESCRIPTION
    两相使用（由 scripts/Run-ExternalIncrementLive.ps1 的 mark / watch 阶段自动挂接，
    也可手动单跑）：

      capture  在 SAVEWORK **之前**（外部操作单里是 mark 时刻）抓：
               /dbnums 全表、变更根与对照根的 model/records（分页取全）、被改元素的
               element/attributes、树切片（tree/children）、搜索切片（/search）、
               pending-units、batch-failures 与 error-log 基线。
               写 marks/<label>-http-capture.json。
      assert   在 watch 判到 in_sync **之后**复抓同一批，逐项判：
               1. 变更根：records 必须变化（items 规范化文本或 total 不同）；
               2. 删除根：capture 时非空、现在必须为空（records 空数组）；
               3. 对照根：records 逐字不变（规范化后 sha 相同；只有顺序变 = WARN）；
               4. 被改元素：attributes 响应必须变化；
               5. 树切片 / 搜索切片：复抓成功即过，差异全文写进报告（净变化的证据腿，
                  期望值因腿而异，不在这里硬编码）；
               6. pending-units：无死信、目标库无遗留 pending（404 = S 系列退役，SKIP）；
               7. batch-failures / error-log：相对 capture **零新增行**；
               8. 给了 -Dbnum 时复核该库 model_verdict=in_sync 且三水位相等。
               写 marks/<label>-http-assert.json；FAIL 退出码 3。

    判据纪律（2026-09-08 计划 §5 与交接注记）：只认两枚水位 + 读面证据，
    不断言 pending 表的形状——那张表会随 S 系列重构退役，端点 404 一律记 SKIP 不记 FAIL。

    「逐字不变」的口径：双方响应都经同一个规范化序列化器（不变文化、数字 R 格式、
    属性序保留）后比对 sha256——比原始字节稳（PowerShell 版本间 JSON 格式化差异不影响），
    比语义比较严（顺序、精度都算数）。

.EXAMPLE
    # 由外部操作单自动挂接（推荐）：
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase mark -Label bran-modify `
        -ChangedRoots 24381/100817 -ControlRoots 24381/100677 -Elements 24381/100823
    ./scripts/Run-ExternalIncrementLive.ps1 -Phase watch -Label bran-modify -Dbnum 7997

    # 手动单跑（服务地址取 output/external-live/current.json，或 -BaseUrl 指定）：
    ./scripts/Http-IncrementAssert.ps1 -Phase capture -Label t1 -ChangedRoots 24381/100817
    ./scripts/Http-IncrementAssert.ps1 -Phase assert  -Label t1 -Dbnum 7997

    # 离线自检（不碰网络，验规范化器与对拍逻辑）：
    ./scripts/Http-IncrementAssert.ps1 -Phase selftest
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('capture', 'assert', 'selftest')]
    [string]$Phase,
    [string]$Label,
    [string[]]$ChangedRoots = @(),
    [string[]]$DeletedRoots = @(),
    [string[]]$ControlRoots = @(),
    [string[]]$Elements = @(),
    [string[]]$TreeParents = @(),
    [string[]]$SearchQueries = @(),
    [int]$Dbnum,
    [string]$BaseUrl,
    [int]$RecordsLimit = 5000
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

# ───────────────────────── 规范化 JSON（版本无关、可逐字比对） ─────────────────────────

function Format-JsonStringLiteral {
    param([string]$Text)
    $sb = [System.Text.StringBuilder]::new()
    [void]$sb.Append('"')
    foreach ($ch in $Text.ToCharArray()) {
        switch ($ch) {
            '"' { [void]$sb.Append('\"') }
            '\' { [void]$sb.Append('\\') }
            "`n" { [void]$sb.Append('\n') }
            "`r" { [void]$sb.Append('\r') }
            "`t" { [void]$sb.Append('\t') }
            "`b" { [void]$sb.Append('\b') }
            "`f" { [void]$sb.Append('\f') }
            default {
                if ([int]$ch -lt 0x20) { [void]$sb.Append('\u{0:x4}' -f [int]$ch) }
                else { [void]$sb.Append($ch) }
            }
        }
    }
    [void]$sb.Append('"')
    $sb.ToString()
}

function ConvertTo-CanonicalJson {
    param($Value)
    $invariant = [System.Globalization.CultureInfo]::InvariantCulture
    if ($null -eq $Value -or $Value -is [System.DBNull]) { return 'null' }
    if ($Value -is [bool]) { return $(if ($Value) { 'true' } else { 'false' }) }
    if ($Value -is [string]) { return Format-JsonStringLiteral $Value }
    if ($Value -is [datetime]) { return Format-JsonStringLiteral ($Value.ToString('o', $invariant)) }
    if ($Value -is [double] -or $Value -is [float]) { return $Value.ToString('R', $invariant) }
    if ($Value -is [decimal] -or $Value -is [sbyte] -or $Value -is [byte] -or
        $Value -is [int16] -or $Value -is [uint16] -or $Value -is [int] -or $Value -is [uint32] -or
        $Value -is [long] -or $Value -is [uint64] -or $Value -is [System.Numerics.BigInteger]) {
        return [string]($Value.ToString($invariant))
    }
    if ($Value -is [System.Collections.IDictionary]) {
        $parts = foreach ($key in $Value.Keys) {
            (Format-JsonStringLiteral ([string]$key)) + ':' + (ConvertTo-CanonicalJson $Value[$key])
        }
        return '{' + ($parts -join ',') + '}'
    }
    if ($Value -is [System.Collections.IEnumerable]) {
        $parts = foreach ($item in $Value) { ConvertTo-CanonicalJson $item }
        return '[' + ($parts -join ',') + ']'
    }
    if ($Value -is [System.Management.Automation.PSCustomObject]) {
        $parts = foreach ($prop in $Value.PSObject.Properties) {
            (Format-JsonStringLiteral $prop.Name) + ':' + (ConvertTo-CanonicalJson $prop.Value)
        }
        return '{' + ($parts -join ',') + '}'
    }
    # 兜底：标量当字符串。宁可比对偏严也不静默丢字段。
    return Format-JsonStringLiteral ([string]$Value)
}

function Get-Sha256Hex {
    param([string]$Text)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $hash = [System.Security.Cryptography.SHA256]::Create().ComputeHash($bytes)
    ($hash | ForEach-Object { $_.ToString('x2') }) -join ''
}

# ───────────────────────── HTTP 面（只读；错误带回状态码不抛） ─────────────────────────

function Resolve-Context {
    $marksDir = $null
    $url = $BaseUrl
    $currentPath = Join-Path $repo 'output\external-live\current.json'
    if (Test-Path -LiteralPath $currentPath) {
        $current = Get-Content -LiteralPath $currentPath -Raw | ConvertFrom-Json
        if (-not $url) { $url = "http://127.0.0.1:$($current.http_port)" }
        $marksDir = Join-Path $current.run_dir 'marks'
    }
    if (-not $url) { throw '没有 output/external-live/current.json，也没给 -BaseUrl：先跑 Run-ExternalIncrementLive -Phase prepare，或显式指定服务地址' }
    if (-not $marksDir) { $marksDir = Join-Path $repo 'output\http-assert' }
    New-Item -ItemType Directory -Force $marksDir | Out-Null
    @{ BaseUrl = $url.TrimEnd('/'); MarksDir = $marksDir }
}

function Invoke-ApiGet {
    param([string]$Base, [string]$PathAndQuery)
    try {
        $resp = Invoke-WebRequest -Uri ($Base + $PathAndQuery) -TimeoutSec 30
        @{ ok = $true; status = [int]$resp.StatusCode; value = ($resp.Content | ConvertFrom-Json) }
    }
    catch {
        $status = 0
        try { $status = [int]$_.Exception.Response.StatusCode } catch { }
        @{ ok = $false; status = $status; error = $_.Exception.Message }
    }
}

function Invoke-ApiPost {
    param([string]$Base, [string]$Path, [hashtable]$Body)
    try {
        $json = $Body | ConvertTo-Json -Depth 8
        $resp = Invoke-WebRequest -Uri ($Base + $Path) -Method Post -ContentType 'application/json; charset=utf-8' -Body $json -TimeoutSec 60
        @{ ok = $true; status = [int]$resp.StatusCode; value = ($resp.Content | ConvertFrom-Json) }
    }
    catch {
        $status = 0
        try { $status = [int]$_.Exception.Response.StatusCode } catch { }
        @{ ok = $false; status = $status; error = $_.Exception.Message }
    }
}

# ───────────────────────── 各读面的快照形状 ─────────────────────────

function Get-RecordsSnapshot {
    # POST /model/records 分页取全；items 逐条规范化，sha 对整根比对。
    param([string]$Base, [string]$Root)
    $items = New-Object System.Collections.Generic.List[string]
    $cursor = 0
    $pages = 0
    $envelope = $null
    while ($true) {
        $resp = Invoke-ApiPost -Base $Base -Path '/api/v1/model/records' -Body @{ generation_root = $Root; limit = $RecordsLimit; cursor = $cursor }
        if (-not $resp.ok) {
            return @{ ok = $false; status = $resp.status; error = $resp.error }
        }
        $envelope = $resp.value
        foreach ($item in @($envelope.items)) { $items.Add((ConvertTo-CanonicalJson $item)) }
        $pages++
        if ($null -eq $envelope.next_cursor -or $pages -ge 20) { break }
        $cursor = [int]$envelope.next_cursor
    }
    @{
        ok           = $true
        status       = 200
        total        = [int]$envelope.total
        model_source = [string]$envelope.model_source
        items_canon  = @($items)
        sha          = Get-Sha256Hex ($items -join "`n")
    }
}

function Get-AttributesSnapshot {
    param([string]$Base, [string]$Refno)
    $resp = Invoke-ApiPost -Base $Base -Path '/api/v1/element/attributes' -Body @{ refno = $Refno }
    if (-not $resp.ok) { return @{ ok = $false; status = $resp.status; error = $resp.error } }
    $canon = ConvertTo-CanonicalJson $resp.value
    @{ ok = $true; status = 200; canon = $canon; sha = Get-Sha256Hex $canon }
}

function Get-TreeSnapshot {
    # 只留结构判读要用的列；mod_cnt / children_updated 一类观察字段不进对拍。
    param([string]$Base, [string]$Parent)
    $resp = Invoke-ApiGet -Base $Base -PathAndQuery ('/api/v1/tree/children?refno=' + [uri]::EscapeDataString($Parent))
    if (-not $resp.ok) { return @{ ok = $false; status = $resp.status; error = $resp.error } }
    $nodes = @(@($resp.value.nodes) | ForEach-Object {
            [ordered]@{
                refno          = $_.refno
                noun           = $_.noun
                name           = $_.name
                owner          = $_.owner
                order          = $_.order
                children_count = $_.children_count
                dbnum          = $_.dbnum
            }
        })
    @{ ok = $true; status = 200; nodes_canon = @($nodes | ForEach-Object { ConvertTo-CanonicalJson $_ }) }
}

function Get-SearchSnapshot {
    param([string]$Base, [string]$Query)
    $resp = Invoke-ApiGet -Base $Base -PathAndQuery ('/api/v1/search?limit=500&query=' + [uri]::EscapeDataString($Query))
    if (-not $resp.ok) { return @{ ok = $false; status = $resp.status; error = $resp.error } }
    $items = @(@($resp.value.items) | ForEach-Object { ConvertTo-CanonicalJson $_ })
    @{ ok = $true; status = 200; total = [int]$resp.value.total; items_canon = $items }
}

function Get-FailureRows {
    # batch-failures / error-log：整行规范化，之后按多重集找「新增行」。
    param([string]$Base, [string]$PathAndQuery)
    $resp = Invoke-ApiGet -Base $Base -PathAndQuery $PathAndQuery
    if (-not $resp.ok) { return @{ ok = $false; status = $resp.status; error = $resp.error } }
    $rows = $resp.value
    if ($rows -isnot [System.Collections.IEnumerable] -or $rows -is [string]) { $rows = @($rows) }
    @{ ok = $true; status = 200; rows_canon = @(@($rows) | ForEach-Object { ConvertTo-CanonicalJson $_ }) }
}

function Get-NewRows {
    # after 相对 before 的多重集差：同一行文本出现次数增加才算新增。
    param([string[]]$Before, [string[]]$After)
    $counts = @{}
    foreach ($row in $Before) { $counts[$row] = 1 + [int]$counts[$row] }
    $new = New-Object System.Collections.Generic.List[string]
    foreach ($row in $After) {
        if ([int]$counts[$row] -gt 0) { $counts[$row] = [int]$counts[$row] - 1 }
        else { $new.Add($row) }
    }
    @($new)
}

# ───────────────────────── capture ─────────────────────────

function Invoke-CapturePhase {
    if (-not $Label) { throw 'capture 需要 -Label' }
    $ctx = Resolve-Context
    $base = $ctx.BaseUrl
    Write-Host "HTTP 断言 capture：$base（label=$Label）"

    $dbnums = Invoke-ApiGet -Base $base -PathAndQuery '/api/v1/dbnums'
    if (-not $dbnums.ok) { throw "读不到 $base/api/v1/dbnums（$($dbnums.status)）：$($dbnums.error)" }

    $records = [ordered]@{}
    foreach ($root in @($ChangedRoots) + @($DeletedRoots) + @($ControlRoots)) {
        $records[$root] = Get-RecordsSnapshot -Base $base -Root $root
        $state = if ($records[$root].ok) { "total=$($records[$root].total) source=$($records[$root].model_source)" } else { "HTTP $($records[$root].status)" }
        Write-Host "  records  $root  $state"
    }
    $attributes = [ordered]@{}
    foreach ($refno in @($Elements)) {
        $attributes[$refno] = Get-AttributesSnapshot -Base $base -Refno $refno
        Write-Host "  attrs    $refno  $(if ($attributes[$refno].ok) { 'ok' } else { "HTTP $($attributes[$refno].status)" })"
    }
    $tree = [ordered]@{}
    foreach ($parent in @($TreeParents)) {
        $tree[$parent] = Get-TreeSnapshot -Base $base -Parent $parent
        Write-Host "  tree     $parent  $(if ($tree[$parent].ok) { "nodes=$($tree[$parent].nodes_canon.Count)" } else { "HTTP $($tree[$parent].status)" })"
    }
    $search = [ordered]@{}
    foreach ($query in @($SearchQueries)) {
        $search[$query] = Get-SearchSnapshot -Base $base -Query $query
        Write-Host "  search   '$query'  $(if ($search[$query].ok) { "total=$($search[$query].total)" } else { "HTTP $($search[$query].status)" })"
    }
    $pending = Invoke-ApiGet -Base $base -PathAndQuery '/api/v1/update/pending-units'
    $batchFailures = Get-FailureRows -Base $base -PathAndQuery '/api/v1/batch-failures?limit=200'
    $errorLog = Get-FailureRows -Base $base -PathAndQuery '/api/v1/error-log?limit=200'

    $capture = [ordered]@{
        label       = $Label
        captured_at = (Get-Date).ToString('o')
        base_url    = $base
        ps_version  = $PSVersionTable.PSVersion.ToString()
        spec        = [ordered]@{
            changed_roots  = @($ChangedRoots)
            deleted_roots  = @($DeletedRoots)
            control_roots  = @($ControlRoots)
            elements       = @($Elements)
            tree_parents   = @($TreeParents)
            search_queries = @($SearchQueries)
            records_limit  = $RecordsLimit
        }
        dbnums      = $dbnums.value
        records     = $records
        attributes  = $attributes
        tree        = $tree
        search      = $search
        pending     = @{ ok = $pending.ok; status = $pending.status; value = $pending.value }
        batch_failures = $batchFailures
        error_log   = $errorLog
    }
    $path = Join-Path $ctx.MarksDir "$Label-http-capture.json"
    $capture | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $path -Encoding UTF8
    Write-Host "已写 $path" -ForegroundColor Green
}

# ───────────────────────── assert ─────────────────────────

function Invoke-AssertPhase {
    if (-not $Label) { throw 'assert 需要 -Label' }
    $ctx = Resolve-Context
    $base = $ctx.BaseUrl
    $capturePath = Join-Path $ctx.MarksDir "$Label-http-capture.json"
    if (-not (Test-Path -LiteralPath $capturePath)) { throw "没有 $capturePath：先在 SAVEWORK 前跑 -Phase capture -Label $Label" }
    $capture = Get-Content -LiteralPath $capturePath -Raw | ConvertFrom-Json
    if ($capture.ps_version -ne $PSVersionTable.PSVersion.ToString()) {
        Write-Host "[warn] capture 用 PS $($capture.ps_version)，assert 用 PS $($PSVersionTable.PSVersion)：规范化器自带版本无关格式，继续" -ForegroundColor Yellow
    }
    $checks = New-Object System.Collections.Generic.List[object]
    function Add-Check {
        param([string]$Kind, [string]$Subject, [string]$Verdict, [string]$Detail, $Extra)
        $checks.Add([ordered]@{ kind = $Kind; subject = $Subject; verdict = $Verdict; detail = $Detail; extra = $Extra })
        $color = switch ($Verdict) { 'PASS' { 'Green' } 'WARN' { 'Yellow' } 'SKIP' { 'DarkGray' } default { 'Red' } }
        Write-Host ('  [{0,-4}] {1,-14} {2}  {3}' -f $Verdict, $Kind, $Subject, $Detail) -ForegroundColor $color
    }

    Write-Host "HTTP 断言 assert：$base（label=$Label）"

    # 1. 变更根：records 必须变化。
    foreach ($root in @($capture.spec.changed_roots)) {
        $before = $capture.records.$root
        $after = Get-RecordsSnapshot -Base $base -Root $root
        if (-not $before.ok) { Add-Check 'changed-root' $root 'FAIL' "capture 时就没读到（HTTP $($before.status)）" $null; continue }
        if (-not $after.ok) { Add-Check 'changed-root' $root 'FAIL' "复抓失败 HTTP $($after.status)：$($after.error)" $null; continue }
        if ($after.sha -ne $before.sha -or $after.total -ne $before.total) {
            Add-Check 'changed-root' $root 'PASS' "records 已更新（total $($before.total) → $($after.total)，source=$($after.model_source)）" $null
        }
        else {
            Add-Check 'changed-root' $root 'FAIL' "records 逐字未变（total=$($after.total)）——增量没落到模型读面" $null
        }
    }

    # 2. 删除根：capture 非空 → 现在为空。
    foreach ($root in @($capture.spec.deleted_roots)) {
        $before = $capture.records.$root
        $after = Get-RecordsSnapshot -Base $base -Root $root
        if (-not $before.ok -or [int]$before.total -eq 0) { Add-Check 'deleted-root' $root 'FAIL' "capture 时就无记录（ok=$($before.ok) total=$($before.total)），删除腿证明不了什么" $null; continue }
        if ($after.ok -and [int]$after.total -eq 0) { Add-Check 'deleted-root' $root 'PASS' "records 已清空（$($before.total) → 0）" $null; continue }
        if (-not $after.ok -and ($after.status -eq 404 -or $after.status -eq 409)) {
            Add-Check 'deleted-root' $root 'WARN' "复抓 HTTP $($after.status)（refno 已不可解析，视同已删但口径不同）" $null; continue
        }
        Add-Check 'deleted-root' $root 'FAIL' "records 未清空（total=$($after.total)，HTTP $($after.status)）" $null
    }

    # 3. 对照根：逐字不变；只有顺序变 = WARN。
    foreach ($root in @($capture.spec.control_roots)) {
        $before = $capture.records.$root
        $after = Get-RecordsSnapshot -Base $base -Root $root
        if (-not $before.ok) { Add-Check 'control-root' $root 'FAIL' "capture 时就没读到（HTTP $($before.status)）" $null; continue }
        if (-not $after.ok) { Add-Check 'control-root' $root 'FAIL' "复抓失败 HTTP $($after.status)：$($after.error)" $null; continue }
        if ($after.sha -eq $before.sha) { Add-Check 'control-root' $root 'PASS' "records 逐字不变（total=$($after.total)）——未触根没有被重算的痕迹" $null; continue }
        $beforeSorted = @($before.items_canon) | Sort-Object
        $afterSorted = @($after.items_canon) | Sort-Object
        if (($beforeSorted -join "`n") -eq ($afterSorted -join "`n")) {
            Add-Check 'control-root' $root 'WARN' '内容相同但行序变了——读面排序不稳定，值得单独记一条' $null
            continue
        }
        $diffSample = ''
        $beforeItems = @($before.items_canon)
        $afterItems = @($after.items_canon)
        $max = [Math]::Max($beforeItems.Count, $afterItems.Count)
        for ($i = 0; $i -lt $max; $i++) {
            $b = if ($i -lt $beforeItems.Count) { $beforeItems[$i] } else { '(缺行)' }
            $a = if ($i -lt $afterItems.Count) { $afterItems[$i] } else { '(缺行)' }
            if ($b -ne $a) {
                $trim = 160
                $diffSample = "首个差异 items[$i]：before=$($b.Substring(0, [Math]::Min($trim, $b.Length)))… after=$($a.Substring(0, [Math]::Min($trim, $a.Length)))…"
                break
            }
        }
        Add-Check 'control-root' $root 'FAIL' "对照根 records 变了（total $($before.total) → $($after.total)）——未触根被重算。$diffSample" $null
    }

    # 4. 被改元素：attributes 必须变化。
    foreach ($refno in @($capture.spec.elements)) {
        $before = $capture.attributes.$refno
        $after = Get-AttributesSnapshot -Base $base -Refno $refno
        if (-not $before.ok) { Add-Check 'element-attrs' $refno 'FAIL' "capture 时就没读到（HTTP $($before.status)）" $null; continue }
        if (-not $after.ok) { Add-Check 'element-attrs' $refno 'FAIL' "复抓失败 HTTP $($after.status)：$($after.error)" $null; continue }
        if ($after.sha -ne $before.sha) { Add-Check 'element-attrs' $refno 'PASS' 'attributes 已反映变更' $null }
        else { Add-Check 'element-attrs' $refno 'FAIL' 'attributes 逐字未变——文件侧属性读面没接到这次 SAVEWORK' $null }
    }

    # 5. 树切片：证据腿。复抓成功即过，增删改明细进报告。
    foreach ($parent in @($capture.spec.tree_parents)) {
        $before = $capture.tree.$parent
        $after = Get-TreeSnapshot -Base $base -Parent $parent
        if (-not $before.ok -or -not $after.ok) {
            Add-Check 'tree-slice' $parent 'FAIL' "抓取失败（capture ok=$($before.ok)，assert HTTP $($after.status)）" $null; continue
        }
        $beforeSet = @($before.nodes_canon)
        $afterSet = @($after.nodes_canon)
        $added = @($afterSet | Where-Object { $beforeSet -notcontains $_ })
        $removed = @($beforeSet | Where-Object { $afterSet -notcontains $_ })
        Add-Check 'tree-slice' $parent 'PASS' "节点 $($beforeSet.Count) → $($afterSet.Count)（+$($added.Count) / -$($removed.Count)）" ([ordered]@{ added = $added; removed = $removed })
    }

    # 6. 搜索切片：证据腿。
    foreach ($query in @($capture.spec.search_queries)) {
        $before = $capture.search.$query
        $after = Get-SearchSnapshot -Base $base -Query $query
        if (-not $before.ok -or -not $after.ok) {
            Add-Check 'search-slice' $query 'FAIL' "抓取失败（capture ok=$($before.ok)，assert HTTP $($after.status)）" $null; continue
        }
        $beforeSet = @($before.items_canon)
        $afterSet = @($after.items_canon)
        $added = @($afterSet | Where-Object { $beforeSet -notcontains $_ })
        $removed = @($beforeSet | Where-Object { $afterSet -notcontains $_ })
        Add-Check 'search-slice' $query 'PASS' "命中 $($before.total) → $($after.total)（+$($added.Count) / -$($removed.Count)）" ([ordered]@{ added = $added; removed = $removed })
    }

    # 7. pending-units：无死信、目标库无遗留。404 = S 系列退役，SKIP 不 FAIL。
    $pendingAfter = Invoke-ApiGet -Base $base -PathAndQuery '/api/v1/update/pending-units'
    if (-not $pendingAfter.ok -and $pendingAfter.status -eq 404) {
        Add-Check 'pending' '/update/pending-units' 'SKIP' '端点 404——S 系列已退役 pending 表，按交接注记不作断言' $null
    }
    elseif (-not $pendingAfter.ok) {
        Add-Check 'pending' '/update/pending-units' 'FAIL' "读取失败 HTTP $($pendingAfter.status)：$($pendingAfter.error)" $null
    }
    else {
        $value = $pendingAfter.value
        $dead = [int]$value.dead_letters + [int]$value.room_dead_letters
        $stuck = @()
        # 注意：这里在函数作用域里，$PSBoundParameters 是函数自己的（恒空），
        # 只能判脚本参数值本身；dbnum 没有 0 号库，0 = 未指定。
        if ($Dbnum -gt 0) {
            $stuck = @(@($value.units) | Where-Object {
                    ($_.PSObject.Properties.Name -contains 'dbnum' -and [int]$_.dbnum -eq $Dbnum) -or
                    ($_.PSObject.Properties.Name -contains 'source_dbnum' -and [int]$_.source_dbnum -eq $Dbnum)
                })
        }
        if ($dead -eq 0 -and $stuck.Count -eq 0) {
            Add-Check 'pending' '/update/pending-units' 'PASS' "死信 0，目标库无遗留 pending（units 总数 $(@($value.units).Count)）" $null
        }
        else {
            Add-Check 'pending' '/update/pending-units' 'FAIL' "死信 $dead，目标库遗留 pending $($stuck.Count) 条" (@($stuck | ForEach-Object { ConvertTo-CanonicalJson $_ }))
        }
    }

    # 8. batch-failures / error-log：相对 capture 零新增。
    foreach ($leg in @(
            @{ kind = 'batch-failures'; path = '/api/v1/batch-failures?limit=200'; before = $capture.batch_failures },
            @{ kind = 'error-log'; path = '/api/v1/error-log?limit=200'; before = $capture.error_log })) {
        $before = $leg.before
        $after = Get-FailureRows -Base $base -PathAndQuery $leg.path
        if (-not $before.ok) { Add-Check $leg.kind $leg.path 'WARN' "capture 时没读到基线（HTTP $($before.status)），本腿只报不判" $null; continue }
        if (-not $after.ok) { Add-Check $leg.kind $leg.path 'FAIL' "复抓失败 HTTP $($after.status)：$($after.error)" $null; continue }
        $new = Get-NewRows -Before @($before.rows_canon) -After @($after.rows_canon)
        if ($new.Count -eq 0) { Add-Check $leg.kind $leg.path 'PASS' '零新增行' $null }
        else { Add-Check $leg.kind $leg.path 'FAIL' "新增 $($new.Count) 行" (@($new | Select-Object -First 5)) }
    }

    # 9. 目标库终态复核（watch 传入 -Dbnum 时；0 = 未指定，跳过）。
    if ($Dbnum -gt 0) {
        $dbnums = Invoke-ApiGet -Base $base -PathAndQuery '/api/v1/dbnums'
        if (-not $dbnums.ok) { Add-Check 'dbnums' "dbnum=$Dbnum" 'FAIL' "读取失败 HTTP $($dbnums.status)" $null }
        else {
            $row = @($dbnums.value.dbnums) | Where-Object { [int]$_.dbnum -eq $Dbnum } | Select-Object -First 1
            if ($row -and $row.model_verdict -eq 'in_sync' -and
                [int]$row.applied_sesno -eq [int]$row.file_latest_sesno -and [int]$row.model_sesno -eq [int]$row.applied_sesno) {
                Add-Check 'dbnums' "dbnum=$Dbnum" 'PASS' "in_sync @ applied=model=file=$($row.applied_sesno)" $null
            }
            else {
                $rowText = if ($row) { "applied=$($row.applied_sesno) model=$($row.model_sesno) file=$($row.file_latest_sesno) verdict=$($row.model_verdict)" } else { '（没有这一行）' }
                Add-Check 'dbnums' "dbnum=$Dbnum" 'FAIL' $rowText $null
            }
        }
    }

    $failCount = @($checks | Where-Object { $_.verdict -eq 'FAIL' }).Count
    $warnCount = @($checks | Where-Object { $_.verdict -eq 'WARN' }).Count
    $skipCount = @($checks | Where-Object { $_.verdict -eq 'SKIP' }).Count
    $passCount = @($checks | Where-Object { $_.verdict -eq 'PASS' }).Count
    $overall = if ($failCount -gt 0) { 'FAIL' } elseif ($warnCount -gt 0) { 'WARN' } else { 'PASS' }

    $report = [ordered]@{
        label       = $Label
        asserted_at = (Get-Date).ToString('o')
        base_url    = $base
        dbnum       = $(if ($Dbnum -gt 0) { $Dbnum } else { $null })
        overall     = $overall
        counts      = [ordered]@{ pass = $passCount; warn = $warnCount; fail = $failCount; skip = $skipCount }
        checks      = $checks
    }
    $reportPath = Join-Path $ctx.MarksDir "$Label-http-assert.json"
    $report | ConvertTo-Json -Depth 16 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    $color = switch ($overall) { 'PASS' { 'Green' } 'WARN' { 'Yellow' } default { 'Red' } }
    Write-Host ("HTTP 断言 {0}：{1} pass / {2} warn / {3} fail / {4} skip（{5}）" -f $overall, $passCount, $warnCount, $failCount, $skipCount, $reportPath) -ForegroundColor $color
    if ($overall -eq 'FAIL') { exit 3 }
}

# ───────────────────────── selftest ─────────────────────────

function Invoke-SelftestPhase {
    $failures = New-Object System.Collections.Generic.List[string]
    function Assert-True {
        param([bool]$Condition, [string]$Name)
        if ($Condition) { Write-Host "  [PASS] $Name" -ForegroundColor Green }
        else { $failures.Add($Name); Write-Host "  [FAIL] $Name" -ForegroundColor Red }
    }

    Write-Host 'HTTP 断言脚本离线自检：'
    # 规范化器：同一 JSON 两次解析 → 同一文本；数字/转义/Unicode 稳定。
    $sample = '{"a":1.5,"b":3,"s":"文 \"q\"\nx","n":null,"arr":[1,2,3],"o":{"k":true}}'
    $c1 = ConvertTo-CanonicalJson ($sample | ConvertFrom-Json)
    $c2 = ConvertTo-CanonicalJson ($sample | ConvertFrom-Json)
    Assert-True ($c1 -eq $c2) '同一 JSON 两次解析规范化结果一致'
    Assert-True ($c1.Contains('1.5') -and $c1.Contains(':3')) '数字不变文化格式（1.5 / 3）'
    Assert-True ($c1.Contains('文')) '非 ASCII 原样保留'
    $roundTrip = ('{"s":' + (Format-JsonStringLiteral "a`"b`n文\") + '}') | ConvertFrom-Json
    Assert-True ($roundTrip.s -eq "a`"b`n文\") '字符串转义可往返'
    # 顺序敏感 + 排序回退。
    $itemsA = @('{"x":1}', '{"y":2}')
    $itemsB = @('{"y":2}', '{"x":1}')
    Assert-True ((Get-Sha256Hex ($itemsA -join "`n")) -ne (Get-Sha256Hex ($itemsB -join "`n"))) '行序不同 → sha 不同（逐字口径）'
    Assert-True (((@($itemsA) | Sort-Object) -join "`n") -eq ((@($itemsB) | Sort-Object) -join "`n")) '排序后相等 → 可判定为仅顺序变化'
    # 多重集新增行。
    $new = Get-NewRows -Before @('r1') -After @('r1', 'r2', 'r2')
    Assert-True ($new.Count -eq 2 -and $new[0] -eq 'r2') '多重集差：r2 记两条新增'
    $none = Get-NewRows -Before @('r1', 'r2') -After @('r2', 'r1')
    Assert-True ($none.Count -eq 0) '同集换序 → 零新增'

    if ($failures.Count -gt 0) { Write-Host "自检失败 $($failures.Count) 项" -ForegroundColor Red; exit 1 }
    Write-Host '自检全部通过。' -ForegroundColor Green
}

switch ($Phase) {
    'capture' { Invoke-CapturePhase }
    'assert' { Invoke-AssertPhase }
    'selftest' { Invoke-SelftestPhase }
}
