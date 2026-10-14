<#
.SYNOPSIS
    严尺子：把我们生成的 `.rvm` 与 E3D 导出的 `.rvm` 逐件对拍（RVM ↔ RVM）。

.DESCRIPTION
    `Compare-SlykRvm.ps1` 走的是「E3D 快照 ↔ elements.json」那条路，判据只有**局部系 AABB**。
    这条路两侧都是 `.rvm`、都由 `e3d_model::rvm_read` 读，于是多出一样 AABB 给不出的东西：

        kind_match —— 两侧写出的 RVM 图元种类是不是同一套（多重集比较，顺序无关）

    那是「我们的几何分解是否与 E3D 同构」的度量，比 AABB 严。AABB 对得上、kind 对不上的，
    是形状错了而包围盒恰好一样：2026-09-14 实测，35 只六角螺母被写成圆柱、17 块支架底板的
    四棱台被写成圆锥（错 47.6 mm），**宽松那把尺子一件都没报**。

    本脚本**复用** `Compare-SlykRvm.ps1` 已经落盘的基准 `.rvm`，自己不开 TTY 会话：
    先 `gen_ams --rvm` 写出我们的 `<dir>/gen/generated.rvm`，再 `rvm_compare --generated` 对拍，
    明细落 `<dir>/compare-rvm.json`，汇总落 `<out>/summary-strict.csv`。

    先跑 `Compare-SlykRvm.ps1` 拿到基准；没有 `<tag>.rvm` 的根按 SKIP 记，不去补导出。

.PARAMETER FromCsv
    读一份 CSV，要有 `zone_refno` 与 `dbnum` 两列（`output/slyk-rvm/zones.csv` 就是）。

.PARAMETER Roots
    要对拍的生成根，形如 `21909/16772`。与 `-FromCsv` 二选一。

.PARAMETER Dbnum
    `-Roots` 模式下每个根所在的设计库号。

.PARAMETER Out
    产物目录，默认 `output/slyk-rvm/sweep`（与宽松那条共用，只多写两个文件）。

.PARAMETER BaselineDir
    基准 `.rvm` 所在的目录（`<dir>/<tag>/<tag>.rvm`），默认与 `-Out` 相同。两条严尺子同时在跑时
    把 `-Out` 指到别处、基准仍从这里读，互不覆盖对方的 `gen/` 与 `compare-rvm.json`。

.EXAMPLE
    ./scripts/Compare-SlykRvmStrict.ps1 -FromCsv output/slyk-rvm/zones.csv
#>
[CmdletBinding()]
param(
    [string[]]$Roots,
    [string]$FromCsv,
    [int]$Dbnum,
    [string]$Out = 'output/slyk-rvm/sweep',
    [string]$BaselineDir,
    [int]$Skip = 0,
    [int]$Take = 0
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = if ([IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $repo $Out }
$baselineRoot = if (-not $BaselineDir) { $out } elseif ([IO.Path]::IsPathRooted($BaselineDir)) { $BaselineDir } else { Join-Path $repo $BaselineDir }

$schema = 'd:\work\plant-code\old\vendor\e3d-io\schema\e3d31\binaries'
$acw = 'D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000'
$bin = 'D:\Rust\target\debug'

$targets = @()
if ($FromCsv) {
    $csv = if ([IO.Path]::IsPathRooted($FromCsv)) { $FromCsv } else { Join-Path $repo $FromCsv }
    foreach ($row in Import-Csv $csv) {
        if ([int]$row.kids -le 0) { continue }
        $targets += [pscustomobject]@{ refno = $row.zone_refno; dbnum = [int]$row.dbnum; label = $row.zone }
    }
}
foreach ($r in $Roots) { $targets += [pscustomobject]@{ refno = $r; dbnum = $Dbnum; label = $r } }
if ($Skip -gt 0) { $targets = @($targets | Select-Object -Skip $Skip) }
if ($Take -gt 0) { $targets = @($targets | Select-Object -First $Take) }
Write-Host ("严尺子对拍 {0} 个根（复用已有基准 .rvm）" -f @($targets).Count)

$results = @()
$i = 0
foreach ($t in $targets) {
    $i++
    $tag = $t.refno -replace '/', '-'
    $dir = Join-Path $out $tag
    $rvm = Join-Path (Join-Path $baselineRoot $tag) "$tag.rvm"
    $row = [ordered]@{ refno = $t.refno; dbnum = $t.dbnum; label = $t.label; verdict = ''; both = 0; kind_match = 0; kind_differ = 0; off = 0; detail = '' }
    Write-Host ("[{0}/{1}] {2} {3}" -f $i, @($targets).Count, $t.refno, $t.label) -ForegroundColor Cyan

    if (-not (Test-Path $rvm)) {
        $row.verdict = 'SKIP'; $row.detail = '没有基准 .rvm，先跑 Compare-SlykRvm.ps1'
        $results += [pscustomobject]$row; continue
    }

    # ── 1. 直读生成，额外写一份我们自己的 .rvm ────────────────────────────────
    $dbfile = Join-Path $acw ("acw{0:d4}_0001" -f $t.dbnum)
    $gen = Join-Path $dir 'gen'
    $log = & "$bin\gen_ams.exe" --attlib "$schema\attlib.dat" --db $dbfile --template $schema --db-type DESI `
        --root $t.refno --project-dir $acw --out $gen --rvm 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $row.verdict = 'FAIL'; $row.detail = ($log.Trim() -split "`n")[-1]
        $results += [pscustomobject]$row; continue
    }
    $row.detail = ([regex]::Match($log, '解析图元 (\d+) / 自校未过退回面片 (\d+)')).Value

    # ── 2. RVM ↔ RVM ──────────────────────────────────────────────────────────
    $cmp = Join-Path $dir 'compare-rvm.json'
    $log = & "$bin\rvm_compare.exe" --baseline $rvm --generated (Join-Path $gen 'generated.rvm') --out $cmp 2>&1 | Out-String
    $code = $LASTEXITCODE
    Set-Content (Join-Path $dir 'compare-rvm.log') $log -Encoding utf8
    $row.verdict = if ($code -eq 0) { 'PASS' } else { 'OFF' }
    if (Test-Path $cmp) {
        # `-AsHashtable`：RVM ↔ RVM 这条路给具名元素的 noun 是空串（名字里没有类型信息，
        # 见 `rvm_compare::load_rvm_side`），`by_noun` 因此带一个空键，对象形态解不出来。
        $c = Get-Content $cmp -Raw | ConvertFrom-Json -AsHashtable
        foreach ($tally in $c.by_noun.Values) {
            $row.both += [int]$tally.both
            $row.kind_match += [int]$tally.kind_match
            $row.kind_differ += [int]$tally.kind_differ
            $row.off += [int]$tally.off
        }
    }
    $results += [pscustomobject]$row
    Write-Host ("    {0}  both={1} kind_match={2} kind_differ={3} off={4}" -f $row.verdict, $row.both, $row.kind_match, $row.kind_differ, $row.off) `
        -ForegroundColor $(if ($row.verdict -eq 'PASS' -and $row.kind_differ -eq 0) { 'Green' } elseif ($row.verdict -eq 'PASS') { 'Yellow' } else { 'Red' })
}

$results | Export-Csv (Join-Path $out 'summary-strict.csv') -NoTypeInformation -Encoding utf8
Write-Host ''
Write-Host '=== 汇总 ===' -ForegroundColor Cyan
$results | Group-Object verdict | Sort-Object Count -Descending | ForEach-Object { "  {0,-6} {1}" -f $_.Name, $_.Count }
$both = ($results | Measure-Object both -Sum).Sum
$km = ($results | Measure-Object kind_match -Sum).Sum
$kd = ($results | Measure-Object kind_differ -Sum).Sum
$off = ($results | Measure-Object off -Sum).Sum
Write-Host ("  两侧都有 {0} 件：kind 一致 {1} / 不一致 {2}（{3:P2}），off {4}" -f $both, $km, $kd, $(if ($both) { $km / $both } else { 0 }), $off)
Write-Host ("明细：{0}" -f (Join-Path $out 'summary-strict.csv'))
