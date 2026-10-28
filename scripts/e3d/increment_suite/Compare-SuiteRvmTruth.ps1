<#
.SYNOPSIS
    按名对拍两份 rvm_verify import 快照，并按 rvm-truth.json 里登记的预期判定一条腿的 RVM 真值是否自洽。

.DESCRIPTION
    PML 增量套件（scripts/e3d/increment_suite）的 RVM 真值是整棵 /AIOS-SUITE-8000 每腿一份快照
    （Export-SuiteRvmTruth.ps1 产出）。本脚本只读两份快照 JSON，不碰 E3D、不碰 SurrealDB：

      1. 取两边「有几何的成员」（aabb_world_mm 非空），键 = 导出根以下的相对路径
         （如 AIOS-SUITE-8000-Z/AIOS-SUITE-EQ-A/AIOS-SUITE-BOX-A1）；套件元素都带名字，
         所以 restore 重建换 refno 不影响对拍；
      2. 算差分：added / removed / moved（六分量同向平移）/ resized（其它变化）/ unchanged；
      3. 给了 -Leg 时，按 rvm-truth.json `legs.<宏相对路径>` 的预期逐项核对：
         added / removed / moved / resized / renamed / subtree / unchanged_elsewhere / aabb_blind /
         equals_state_before；`calibrate: true` 的项只报观测值不判红（首轮录制后把观测值填回模型）；
         `blocked` 的腿直接 skipped。
      4. 退出码：0 = pass / calibrate / skipped / 纯 diff；1 = fail。-Report 写机器可读报告。

    容差：AABB 六分量各 ≤ -ToleranceMm（默认 1 mm，与 K 组 Q5 同一口径）。

.EXAMPLE
    scripts/e3d/increment_suite/Compare-SuiteRvmTruth.ps1 -Before rvm-truth/748.json -After rvm-truth/750.json -Leg xform/move_apply.mac
.EXAMPLE
    scripts/e3d/increment_suite/Compare-SuiteRvmTruth.ps1 -Before a.json -After b.json      # 只出 diff，不判定
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Before,
    [Parameter(Mandatory)][string]$After,
    [string]$Model = (Join-Path $PSScriptRoot 'rvm-truth.json'),
    # 宏相对套件目录的路径，如 xform/move_apply.mac；不给就只输出差分
    [string]$Leg = '',
    # 导出根（RVM 路径里不带前导斜杠的那一段名字之前的部分都会被剥掉）
    [string]$Root = '/AIOS-SUITE-8000',
    [double]$ToleranceMm = 1.0,
    [string]$Report = '',
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'

function Read-Json([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "文件不存在：$Path" }
    return Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

# 快照 → 哈希表：相对键 → @{ aabb = double[6]; path = 原路径 }
function Get-GeometryMembers($Snapshot, [string]$RootName) {
    $map = [ordered]@{}
    foreach ($m in @($Snapshot.members)) {
        if ($null -eq $m.aabb_world_mm) { continue }
        $path = [string]$m.path
        $marker = "/$RootName/"
        $idx = $path.IndexOf($marker)
        if ($idx -lt 0) {
            # 导出根自己或根之外的祖先：不参与对拍（SITE / ZONE 本身没有几何）
            continue
        }
        $key = $path.Substring($idx + $marker.Length)
        if ($map.Contains($key)) { throw "快照里同一相对路径出现两次：$key（$path）——按名对拍失去唯一性" }
        $map[$key] = @{ aabb = @($m.aabb_world_mm | ForEach-Object { [double]$_ }); path = $path }
    }
    return $map
}

# 把 after 快照里改过名的路径折回 before 的叫法：模型里 renamed = [{from: 旧前缀, to: 新前缀}]
# 描述的是这条腿发生的改名，所以这里把 after 键里的 to 前缀换回 from。
function Apply-Renames($Members, $Renames) {
    if (-not $Renames) { return $Members }
    $out = [ordered]@{}
    foreach ($key in $Members.Keys) {
        $newKey = $key
        foreach ($r in @($Renames)) {
            $from = [string]$r.from
            $to = [string]$r.to
            if ($newKey -eq $to) { $newKey = $from }
            elseif ($newKey.StartsWith("$to/")) { $newKey = $from + $newKey.Substring($to.Length) }
        }
        if ($out.Contains($newKey)) { throw "改名折回后两条路径撞在一起：$newKey（来自 $key）" }
        $out[$newKey] = $Members[$key]
    }
    return $out
}

function Get-Delta([double[]]$a, [double[]]$b) {
    $d = New-Object 'double[]' 6
    for ($i = 0; $i -lt 6; $i++) { $d[$i] = $b[$i] - $a[$i] }
    return $d
}

function Test-Within([double[]]$values, [double]$tol) {
    foreach ($v in $values) { if ([math]::Abs($v) -gt $tol) { return $false } }
    return $true
}

function Get-Shift([double[]]$delta, [double]$tol) {
    # 六分量是「min 与 max 同向同量」才算平移；返回 [dx,dy,dz] 或 $null
    for ($i = 0; $i -lt 3; $i++) {
        if ([math]::Abs($delta[$i] - $delta[$i + 3]) -gt $tol) { return $null }
    }
    return @($delta[0], $delta[1], $delta[2])
}

function Get-UnionAabb($Members, [string]$Prefix) {
    $union = $null
    $count = 0
    foreach ($key in $Members.Keys) {
        if (-not ($key -eq $Prefix -or $key.StartsWith("$Prefix/"))) { continue }
        $count++
        $a = $Members[$key].aabb
        if ($null -eq $union) { $union = @($a) ; continue }
        for ($i = 0; $i -lt 3; $i++) { if ($a[$i] -lt $union[$i]) { $union[$i] = $a[$i] } }
        for ($i = 3; $i -lt 6; $i++) { if ($a[$i] -gt $union[$i]) { $union[$i] = $a[$i] } }
    }
    return @{ count = $count; aabb = $union }
}

function Format-Vec($v) {
    if ($null -eq $v) { return '-' }
    return '[' + (($v | ForEach-Object { '{0:0.##}' -f [double]$_ }) -join ', ') + ']'
}

# ── 读入 ──────────────────────────────────────────────────────────────────────────
$beforeSnap = Read-Json $Before
$afterSnap = Read-Json $After
$rootName = $Root.TrimStart('/')
$beforeMembers = Get-GeometryMembers $beforeSnap $rootName
$afterMembers = Get-GeometryMembers $afterSnap $rootName

$expect = $null
$truthModel = $null
if ($Leg) {
    $truthModel = Read-Json $Model
    if ($truthModel.PSObject.Properties['tolerances'] -and $truthModel.tolerances.PSObject.Properties['aabb_mm'] -and -not $PSBoundParameters.ContainsKey('ToleranceMm')) {
        $ToleranceMm = [double]$truthModel.tolerances.aabb_mm
    }
    $legProp = $truthModel.legs.PSObject.Properties[$Leg]
    if (-not $legProp) { throw "rvm-truth.json 的 legs 里没有登记 $Leg" }
    $expect = $legProp.Value
}

$renames = $null
if ($expect -and $expect.PSObject.Properties['renamed']) { $renames = $expect.renamed }
$afterMembers = Apply-Renames $afterMembers $renames

# ── 差分 ──────────────────────────────────────────────────────────────────────────
$added = [ordered]@{}
$removed = [ordered]@{}
$moved = [ordered]@{}
$resized = [ordered]@{}
$unchanged = New-Object System.Collections.Generic.List[string]

foreach ($key in $afterMembers.Keys) {
    if (-not $beforeMembers.Contains($key)) { $added[$key] = $afterMembers[$key].aabb }
}
foreach ($key in $beforeMembers.Keys) {
    if (-not $afterMembers.Contains($key)) { $removed[$key] = $beforeMembers[$key].aabb; continue }
    $delta = Get-Delta $beforeMembers[$key].aabb $afterMembers[$key].aabb
    if (Test-Within $delta $ToleranceMm) { $unchanged.Add($key); continue }
    $shift = Get-Shift $delta $ToleranceMm
    if ($null -ne $shift) { $moved[$key] = $shift } else { $resized[$key] = @{ before = $beforeMembers[$key].aabb; after = $afterMembers[$key].aabb; delta = $delta } }
}

# ── 判定 ──────────────────────────────────────────────────────────────────────────
$findings = New-Object System.Collections.Generic.List[object]
function Add-Finding([string]$Level, [string]$What, [string]$Detail) {
    $script:findings.Add([pscustomobject]@{ level = $Level; what = $What; detail = $Detail })
}

$verdict = 'diff-only'
if ($expect) {
    $calibrateAll = $expect.PSObject.Properties['calibrate'] -and [bool]$expect.calibrate
    $mismatchLevel = 'fail'
    if ($calibrateAll) { $mismatchLevel = 'calibrate' }

    if ($expect.PSObject.Properties['blocked'] -and $expect.blocked) {
        $verdict = 'skipped'
        Add-Finding 'info' 'blocked' ([string]$expect.blocked)
    } else {
        # 这些键在逐项核对里被「认领」了；其余的差分按 unchanged_elsewhere 处理
        $claimed = New-Object System.Collections.Generic.HashSet[string]

        if ($expect.PSObject.Properties['added']) {
            foreach ($p in $expect.added.PSObject.Properties) {
                $key = $p.Name; [void]$claimed.Add($key)
                if (-not $added.Contains($key)) { Add-Finding $mismatchLevel 'added-missing' "预期新增 $key，after 里没有（或 before 里已有）"; continue }
                $want = $p.Value
                if ($null -eq $want) { Add-Finding 'ok' 'added' "$key 出现（未断言 AABB）"; continue }
                if ($want -is [System.Array]) {
                    $d = Get-Delta (@($want | ForEach-Object { [double]$_ })) $added[$key]
                    if (Test-Within $d $ToleranceMm) { Add-Finding 'ok' 'added' "$key AABB 吻合 $(Format-Vec $added[$key])" }
                    else { Add-Finding $mismatchLevel 'added-aabb' "$key 预期 $(Format-Vec $want) 实得 $(Format-Vec $added[$key])" }
                } elseif ($want.PSObject.Properties['like']) {
                    $src = [string]$want.like
                    if (-not $beforeMembers.Contains($src)) { Add-Finding $mismatchLevel 'added-like' "$key 的参照 $src 不在 before 里"; continue }
                    $ref = @($beforeMembers[$src].aabb)
                    if ($want.PSObject.Properties['shift']) { $s = @($want.shift | ForEach-Object { [double]$_ }); for ($i = 0; $i -lt 3; $i++) { $ref[$i] += $s[$i]; $ref[$i + 3] += $s[$i] } }
                    $d = Get-Delta $ref $added[$key]
                    if (Test-Within $d $ToleranceMm) { Add-Finding 'ok' 'added' "$key 与参照 $src 平移后吻合" }
                    else { Add-Finding $mismatchLevel 'added-like' "$key 预期 $(Format-Vec $ref)（参照 $src）实得 $(Format-Vec $added[$key])" }
                } else {
                    Add-Finding 'ok' 'added' "$key 出现（预期值形式未识别，不断言 AABB）"
                }
            }
        }
        if ($expect.PSObject.Properties['removed']) {
            foreach ($key in @($expect.removed)) {
                [void]$claimed.Add([string]$key)
                if ($removed.Contains([string]$key)) { Add-Finding 'ok' 'removed' "$key 已消失" }
                else { Add-Finding $mismatchLevel 'removed-missing' "预期删除 $key，after 里仍在（或 before 里本就没有）" }
            }
        }
        if ($expect.PSObject.Properties['moved']) {
            foreach ($p in $expect.moved.PSObject.Properties) {
                $key = $p.Name; [void]$claimed.Add($key)
                $want = @($p.Value | ForEach-Object { [double]$_ })
                if ($moved.Contains($key)) {
                    $got = $moved[$key]
                    $ok = $true
                    for ($i = 0; $i -lt 3; $i++) { if ([math]::Abs($got[$i] - $want[$i]) -gt $ToleranceMm) { $ok = $false } }
                    if ($ok) { Add-Finding 'ok' 'moved' "$key 平移 $(Format-Vec $got)" }
                    else { Add-Finding $mismatchLevel 'moved-vector' "$key 预期平移 $(Format-Vec $want) 实得 $(Format-Vec $got)" }
                } elseif ($unchanged.Contains($key)) {
                    Add-Finding $mismatchLevel 'moved-missing' "$key 预期平移 $(Format-Vec $want)，实际未动"
                } elseif ($resized.Contains($key)) {
                    Add-Finding $mismatchLevel 'moved-not-rigid' "$key 预期平移，实际六分量不同向：$(Format-Vec $resized[$key].delta)"
                } else {
                    Add-Finding $mismatchLevel 'moved-absent' "$key 不在两份快照的交集里"
                }
            }
        }
        if ($expect.PSObject.Properties['resized']) {
            foreach ($p in $expect.resized.PSObject.Properties) {
                $key = $p.Name; [void]$claimed.Add($key)
                if (-not $afterMembers.Contains($key)) { Add-Finding $mismatchLevel 'resized-absent' "$key 不在 after 里"; continue }
                $want = @($p.Value | ForEach-Object { [double]$_ })
                $d = Get-Delta $want $afterMembers[$key].aabb
                if (Test-Within $d $ToleranceMm) { Add-Finding 'ok' 'resized' "$key AABB 吻合 $(Format-Vec $afterMembers[$key].aabb)" }
                else { Add-Finding $mismatchLevel 'resized-aabb' "$key 预期 $(Format-Vec $want) 实得 $(Format-Vec $afterMembers[$key].aabb)" }
            }
        }
        if ($expect.PSObject.Properties['subtree']) {
            foreach ($p in $expect.subtree.PSObject.Properties) {
                $prefix = $p.Name
                $spec = $p.Value
                $level = $mismatchLevel
                if ($spec.PSObject.Properties['calibrate'] -and [bool]$spec.calibrate) { $level = 'calibrate' }
                $b = Get-UnionAabb $beforeMembers $prefix
                $a = Get-UnionAabb $afterMembers $prefix
                foreach ($key in @($beforeMembers.Keys) + @($afterMembers.Keys)) {
                    if ($key -eq $prefix -or $key.StartsWith("$prefix/")) { [void]$claimed.Add($key) }
                }
                $obs = "成员 $($b.count)→$($a.count)，并集 $(Format-Vec $b.aabb)→$(Format-Vec $a.aabb)"
                if ($spec.PSObject.Properties['count_delta'] -and $null -ne $spec.count_delta) {
                    $wantDelta = [int]$spec.count_delta
                    if (($a.count - $b.count) -eq $wantDelta) { Add-Finding 'ok' 'subtree-count' "$prefix 成员数变化 $wantDelta（$obs）" }
                    else { Add-Finding $level 'subtree-count' "$prefix 预期成员数变化 $wantDelta 实得 $($a.count - $b.count)（$obs）" }
                }
                if ($spec.PSObject.Properties['union_shift'] -and $null -ne $spec.union_shift) {
                    $want = @($spec.union_shift | ForEach-Object { [double]$_ })
                    if ($null -eq $b.aabb -or $null -eq $a.aabb) { Add-Finding $level 'subtree-union' "$prefix 一侧没有几何（$obs）" }
                    else {
                        $d = Get-Delta $b.aabb $a.aabb
                        $shift = Get-Shift $d $ToleranceMm
                        $ok = $false
                        if ($null -ne $shift) { $ok = $true; for ($i = 0; $i -lt 3; $i++) { if ([math]::Abs($shift[$i] - $want[$i]) -gt $ToleranceMm) { $ok = $false } } }
                        if ($ok) { Add-Finding 'ok' 'subtree-union' "$prefix 并集平移 $(Format-Vec $shift)" }
                        else { Add-Finding $level 'subtree-union' "$prefix 预期并集平移 $(Format-Vec $want)，实得 Δ $(Format-Vec $d)（$obs）" }
                    }
                }
                if ($spec.PSObject.Properties['like']) {
                    $ref = Get-UnionAabb $beforeMembers ([string]$spec.like)
                    if ($null -eq $ref.aabb -or $null -eq $a.aabb) { Add-Finding $level 'subtree-like' "$prefix 或参照 $($spec.like) 没有几何（$obs）" }
                    else {
                        $d = Get-Delta $ref.aabb $a.aabb
                        if (Test-Within $d $ToleranceMm -and $a.count -eq $ref.count) { Add-Finding 'ok' 'subtree-like' "$prefix 与参照 $($spec.like) 同形同位（$($a.count) 件）" }
                        else { Add-Finding $level 'subtree-like' "$prefix 预期与 $($spec.like) 同形同位，实得 Δ $(Format-Vec $d)，成员 $($ref.count) vs $($a.count)" }
                    }
                }
                if (-not ($spec.PSObject.Properties['count_delta'] -or $spec.PSObject.Properties['union_shift'] -or $spec.PSObject.Properties['like'])) {
                    Add-Finding 'calibrate' 'subtree-observe' "$prefix $obs"
                }
            }
        }

        $isEquals = $expect.PSObject.Properties['equals_state_before'] -and $expect.equals_state_before
        $isBlind = $expect.PSObject.Properties['aabb_blind'] -and [bool]$expect.aabb_blind
        $elsewhere = $true
        if ($expect.PSObject.Properties['unchanged_elsewhere']) { $elsewhere = [bool]$expect.unchanged_elsewhere }
        $except = @()
        if ($expect.PSObject.Properties['except']) { $except = @($expect.except | ForEach-Object { [string]$_ }) }
        foreach ($e in $except) { [void]$claimed.Add($e) }

        if ($elsewhere -or $isEquals -or $isBlind) {
            $leaks = New-Object System.Collections.Generic.List[string]
            foreach ($key in $added.Keys) { if (-not $claimed.Contains($key)) { $leaks.Add("多出 $key $(Format-Vec $added[$key])") } }
            foreach ($key in $removed.Keys) { if (-not $claimed.Contains($key)) { $leaks.Add("少了 $key") } }
            foreach ($key in $moved.Keys) { if (-not $claimed.Contains($key)) { $leaks.Add("意外平移 $key $(Format-Vec $moved[$key])") } }
            foreach ($key in $resized.Keys) { if (-not $claimed.Contains($key)) { $leaks.Add("意外变形 $key Δ $(Format-Vec $resized[$key].delta)") } }
            if ($leaks.Count -eq 0) {
                $what = 'unchanged-elsewhere'
                if ($isEquals) { $what = 'equals-state-before' }
                if ($isBlind) { $what = 'aabb-blind' }
                Add-Finding 'ok' $what "其余成员逐一吻合（≤ $ToleranceMm mm，共 $($unchanged.Count) 件）"
            } else {
                foreach ($l in $leaks) { Add-Finding $mismatchLevel 'unexpected-diff' $l }
            }
        }
        if ($isBlind -and $expect.PSObject.Properties['note']) { Add-Finding 'info' 'note' ([string]$expect.note) }

        $levels = @($findings | ForEach-Object { $_.level })
        if ($levels -contains 'fail') { $verdict = 'fail' }
        elseif ($levels -contains 'calibrate') { $verdict = 'calibrate' }
        else { $verdict = 'pass' }
    }
}

# ── 输出 ──────────────────────────────────────────────────────────────────────────
$result = [ordered]@{
    before = (Resolve-Path -LiteralPath $Before).Path
    after = (Resolve-Path -LiteralPath $After).Path
    leg = $Leg
    root = $Root
    tolerance_mm = $ToleranceMm
    verdict = $verdict
    counts = [ordered]@{ before = $beforeMembers.Count; after = $afterMembers.Count; added = $added.Count; removed = $removed.Count; moved = $moved.Count; resized = $resized.Count; unchanged = $unchanged.Count }
    added = $added
    removed = $removed
    moved = $moved
    resized = $resized
    findings = $findings
}

if (-not $Quiet) {
    Write-Host ("RVM 对拍 {0}：before {1} 件 / after {2} 件 → 新增 {3} 删除 {4} 平移 {5} 变形 {6} 未变 {7}" -f `
        $(if ($Leg) { $Leg } else { '(diff-only)' }), $beforeMembers.Count, $afterMembers.Count, $added.Count, $removed.Count, $moved.Count, $resized.Count, $unchanged.Count)
    foreach ($key in $added.Keys) { Write-Host "  + $key $(Format-Vec $added[$key])" }
    foreach ($key in $removed.Keys) { Write-Host "  - $key" }
    foreach ($key in $moved.Keys) { Write-Host "  ~ $key 平移 $(Format-Vec $moved[$key])" }
    foreach ($key in $resized.Keys) { Write-Host "  # $key Δ $(Format-Vec $resized[$key].delta)" }
    foreach ($f in $findings) { Write-Host ("  [{0}] {1}: {2}" -f $f.level, $f.what, $f.detail) }
    Write-Host "判定：$verdict"
}

if ($Report) {
    $dir = Split-Path -Parent $Report
    if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    [System.IO.File]::WriteAllText($Report, (($result | ConvertTo-Json -Depth 8) + "`n"), (New-Object System.Text.UTF8Encoding $false))
}

if ($verdict -eq 'fail') { exit 1 }
exit 0
