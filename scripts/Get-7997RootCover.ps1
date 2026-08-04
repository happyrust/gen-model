#Requires -Version 7
<#
计算 dbnum=7997 的“全量生成根覆盖”：
- 交付单元根：BRAN/EQUI/HANG/SUPPO（无 MDU 祖先的最外层）
- 残差根：无 MDU 祖先的元素，取其“SITE/ZONE 之下、子树不含 MDU 的最高祖先”
两类根的子树互不重叠，并集覆盖所有非容器存活元素。
输出 _7997_roots.csv（refno,kind,noun,subtree）与覆盖校验。
#>
[CmdletBinding()]
param(
    [string]$DumpPath = "d:\work\plant-code\old\gen-model\_7997_tree_dump.json",
    [string]$OutPath  = "d:\work\plant-code\old\gen-model\_7997_roots.csv"
)

$ErrorActionPreference = "Stop"
$mduSet = @('BRAN','EQUI','HANG','SUPPO')
$containerSet = @('WORL','WORLD','SITE','ZONE')

$raw = Get-Content $DumpPath -Raw | ConvertFrom-Json
$rows = $raw[0].result

$owner = @{}; $noun = @{}
foreach ($r in $rows) { $owner[$r[0]] = $r[1]; $noun[$r[0]] = $r[2] }

# children 索引
$children = @{}
foreach ($r in $rows) {
    $o = $r[1]
    if (-not $children.ContainsKey($o)) { $children[$o] = [System.Collections.Generic.List[string]]::new() }
    $children[$o].Add($r[0])
}

# hasMduDesc: 自底向上标注“子树是否含 MDU”（含自身）
$hasMduDesc = @{}
function Compute-MduDesc([string]$id) {
    $stack = [System.Collections.Generic.Stack[object]]::new()
    $stack.Push(@($id, $false))
    $order = [System.Collections.Generic.List[string]]::new()
    $visited = @{}
    while ($stack.Count -gt 0) {
        $top = $stack.Pop()
        $cur = $top[0]; $expanded = $top[1]
        if ($expanded) { $order.Add($cur); continue }
        if ($visited.ContainsKey($cur)) { continue }
        $visited[$cur] = $true
        $stack.Push(@($cur, $true))
        if ($children.ContainsKey($cur)) {
            foreach ($c in $children[$cur]) { $stack.Push(@($c, $false)) }
        }
    }
    foreach ($cur in $order) {
        $flag = ($noun[$cur] -in $mduSet)
        if (-not $flag -and $children.ContainsKey($cur)) {
            foreach ($c in $children[$cur]) { if ($hasMduDesc[$c]) { $flag = $true; break } }
        }
        $hasMduDesc[$cur] = $flag
    }
}

# 子树大小
$subtreeSize = @{}
function Compute-Sizes([string]$id) {
    $stack = [System.Collections.Generic.Stack[object]]::new()
    $stack.Push(@($id, $false))
    while ($stack.Count -gt 0) {
        $top = $stack.Pop()
        $cur = $top[0]; $expanded = $top[1]
        if ($expanded) {
            $n = 1
            if ($children.ContainsKey($cur)) { foreach ($c in $children[$cur]) { $n += $subtreeSize[$c] } }
            $subtreeSize[$cur] = $n
            continue
        }
        $stack.Push(@($cur, $true))
        if ($children.ContainsKey($cur)) { foreach ($c in $children[$cur]) { $stack.Push(@($c, $false)) } }
    }
}

# 顶层：owner 不在 dump 里（WORL 记录本身不属于 7997 的 dump? SITE 的 owner=pe:16189_0）
$topIds = $rows | Where-Object { -not $noun.ContainsKey($_[1]) } | ForEach-Object { $_[0] }
foreach ($t in $topIds) { Compute-MduDesc $t; Compute-Sizes $t }

# MDU 根：noun 是 MDU 且无 MDU 真祖先
$mduRoots = [System.Collections.Generic.List[string]]::new()
foreach ($r in $rows) {
    $id = $r[0]
    if ($noun[$id] -notin $mduSet) { continue }
    $cur = $owner[$id]; $hasMduAnc = $false
    while ($noun.ContainsKey($cur)) {
        if ($noun[$cur] -in $mduSet) { $hasMduAnc = $true; break }
        $cur = $owner[$cur]
    }
    if (-not $hasMduAnc) { $mduRoots.Add($id) }
}

# 残差根：对每个“无 MDU 祖先且自身非 MDU 且非容器”的元素，
# 取链上（容器之下）最高的、子树不含 MDU 的祖先。
$residueRoots = [System.Collections.Generic.HashSet[string]]::new()
$uncovered = [System.Collections.Generic.List[string]]::new()
foreach ($r in $rows) {
    $id = $r[0]
    if ($noun[$id] -in $mduSet) { continue }
    if ($noun[$id] -in $containerSet) { continue }
    # 链：自身 → … → 容器之下最后一个节点；若途中遇 MDU 祖先则跳过（归 MDU 根）
    $chain = [System.Collections.Generic.List[string]]::new()
    $cur = $id; $underMdu = $false
    while ($noun.ContainsKey($cur) -and ($noun[$cur] -notin $containerSet)) {
        if ($noun[$cur] -in $mduSet) { $underMdu = $true; break }
        $chain.Add($cur)
        $cur = $owner[$cur]
    }
    if ($underMdu) { continue }
    # chain 最末 = 容器之下最高祖先；从最高往下找第一个“子树不含 MDU”的节点
    $root = $null
    for ($i = $chain.Count - 1; $i -ge 0; $i--) {
        if (-not $hasMduDesc[$chain[$i]]) { $root = $chain[$i]; break }
    }
    if ($null -ne $root) { [void]$residueRoots.Add($root) }
    else { $uncovered.Add($id) }
}

# 残差根去掉“被其它残差根覆盖”的嵌套（理论上不会有：都取了最高点；防御一遍）
$residueFinal = [System.Collections.Generic.List[string]]::new()
foreach ($rt in $residueRoots) {
    $cur = $owner[$rt]; $nested = $false
    while ($noun.ContainsKey($cur) -and ($noun[$cur] -notin $containerSet)) {
        if ($residueRoots.Contains($cur)) { $nested = $true; break }
        $cur = $owner[$cur]
    }
    if (-not $nested) { $residueFinal.Add($rt) }
}

# 输出
$out = [System.Collections.Generic.List[pscustomobject]]::new()
foreach ($id in $mduRoots)     { $out.Add([pscustomobject]@{ refno = $id.Replace('_','/'); kind='mdu';     noun=$noun[$id]; subtree=$subtreeSize[$id] }) }
foreach ($id in $residueFinal) { $out.Add([pscustomobject]@{ refno = $id.Replace('_','/'); kind='residue'; noun=$noun[$id]; subtree=$subtreeSize[$id] }) }
$out | Sort-Object subtree -Descending | Export-Csv -Path $OutPath -NoTypeInformation -Encoding UTF8

"MDU 根: $($mduRoots.Count)"
"残差根: $($residueFinal.Count)  (去嵌套前 $($residueRoots.Count))"
"无根可归元素: $($uncovered.Count)"
if ($uncovered.Count -gt 0) {
    $uncovered | Group-Object { $noun[$_] } | Sort-Object Count -Descending | Select-Object Count, Name | Format-Table -AutoSize
}

# 覆盖校验：所有存活非容器元素应位于某根子树内
$rootSet = [System.Collections.Generic.HashSet[string]]::new()
foreach ($id in $mduRoots) { [void]$rootSet.Add($id) }
foreach ($id in $residueFinal) { [void]$rootSet.Add($id) }
$missed = 0; $missedNouns = @{}
foreach ($r in $rows) {
    $id = $r[0]
    if ($noun[$id] -in $containerSet) { continue }
    $cur = $id; $ok = $false
    while ($noun.ContainsKey($cur)) {
        if ($rootSet.Contains($cur)) { $ok = $true; break }
        $cur = $owner[$cur]
    }
    if (-not $ok) {
        $missed++
        $n = $noun[$id]
        if (-not $missedNouns.ContainsKey($n)) { $missedNouns[$n] = 0 }
        $missedNouns[$n]++
    }
}
"覆盖校验：未被任何根覆盖的存活非容器元素 = $missed"
if ($missed -gt 0) {
    $missedNouns.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 20 | Format-Table -AutoSize
}
"根总数: $($out.Count)，csv: $OutPath"
