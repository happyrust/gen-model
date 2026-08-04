[CmdletBinding()]
param(
    [string]$DumpPath = "d:\work\plant-code\old\gen-model\_7997_tree_dump.json"
)

$ErrorActionPreference = "Stop"
$raw = Get-Content $DumpPath -Raw | ConvertFrom-Json
$rows = $raw[0].result

# id -> owner / noun
$owner = @{}
$noun  = @{}
foreach ($r in $rows) {
    $owner[$r[0]] = $r[1]
    $noun[$r[0]]  = $r[2]
}

# 每个元素归属到其 SITE 祖先（memoized 向上走）
$siteOf = @{}
function Resolve-Site([string]$id) {
    $chain = New-Object System.Collections.Generic.List[string]
    $cur = $id
    while ($true) {
        if ($siteOf.ContainsKey($cur)) { $site = $siteOf[$cur]; break }
        if (-not $noun.ContainsKey($cur)) { $site = $null; break }
        if ($noun[$cur] -eq 'SITE') { $site = $cur; break }
        $chain.Add($cur)
        $cur = $owner[$cur]
        if ([string]::IsNullOrEmpty($cur)) { $site = $null; break }
    }
    foreach ($c in $chain) { $siteOf[$c] = $site }
    return $site
}

$stats = @{}
$mduOf = @{}
foreach ($r in $rows) {
    $id = $r[0]
    $site = Resolve-Site $id
    if ($null -eq $site) { continue }
    if (-not $stats.ContainsKey($site)) {
        $stats[$site] = [pscustomobject]@{ site=$site; name=''; total=0; bran=0; equi=0; hang=0; suppo=0 }
    }
    $s = $stats[$site]
    $s.total++
    switch ($noun[$id]) {
        'BRAN'  { $s.bran++ }
        'EQUI'  { $s.equi++ }
        'HANG'  { $s.hang++ }
        'SUPPO' { $s.suppo++ }
    }
}

$stats.Values | Sort-Object total -Descending | Format-Table -AutoSize
