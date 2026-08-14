<#
.SYNOPSIS
    RVM 回归判据:BRAN /C-OR-1R345-C(dbnum 8000,24384/23257)的逐构件几何对拍。
    以 E3D 导出的 RVM 为真值,和 gen-model 落库的 inst_relate 包围盒逐件比尺寸。

.DESCRIPTION
    2026-08-12 db8000 BRAN 增量几何缺陷的回归门。背景与取证见本目录的
    db8000_ftub_joint_probe.mac / db8000_btol_probe.mac 与 changelog。

    真值来源:test_data/rvm/C-OR-1R345-C.rvm.json(E3D `export ... /expdri.so` 导出、
    rvm_verify import 解析;窄口径 insu/obst off)。gen 侧读 8009 生产验证库。

    成员顺序两侧一致(E3D Q MEMB 与 RVM 成员序同为 FTUBE1,BEND1,FTUBE2..7),按序号
    配对,不依赖 ATT 身份解析。

    判定(相对尺寸比,吸收面片化表示差异、抓覆盖/尺寸类缺陷):
      每个构件三维尺寸 |gen-e3d| <= max(ABS_TOL, REL_TOL*e3d) 记 OK,否则 FAIL。
      - 覆盖缺陷签名(修复前):gen 维度 << e3d(如 20 vs 857)→ FAIL。
      - 面片化表示差异(圆柱/FTUB):~2-3mm 或 <1% → OK。
      - 弯头几何缺陷(BEND,已知独立问题):gen >> e3d(如 202 vs 51)→ FAIL(known)。

    退出码 0=全过,1=有 FAIL。

.NOTES
    修前(薄饼覆盖):7 FTUB + BEND1 FAIL(差 800~1100mm),仅 BEND2 一直独立错。
    D2 修复后(TUBI_CONNECT_TOL=5mm):7 FTUB 转 OK(差 2.2~2.5mm 面片化),
    2 BEND 仍 FAIL(弯头目录几何独立缺陷,局部几何带 100mm 竖直,待另修)。
#>
[CmdletBinding()]
param(
    [string]$Snapshot = '',
    [string]$Endpoint = 'http://127.0.0.1:8009/sql',
    [string]$Ns = '1516',
    [string]$Database = 'AvevaMarineSample',
    [double]$AbsTol = 3.0,
    [double]$RelTol = 0.03
)

$ErrorActionPreference = 'Stop'

if (-not $Snapshot) {
    $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $Snapshot = Join-Path $repo 'test_data/rvm/C-OR-1R345-C.rvm.json'
}

# 成员序号 → gen 侧 inst_relate refno(与 RVM 成员序一一对应)。
$ordered = @(
    @{ rvm = 'FTUBE 1'; gen = '24384_23258' }
    @{ rvm = 'BEND 1';  gen = '24384_23259' }
    @{ rvm = 'FTUBE 2'; gen = '24384_23260' }
    @{ rvm = 'FTUBE 3'; gen = '24384_23261' }
    @{ rvm = 'FTUBE 4'; gen = '24384_23262' }
    @{ rvm = 'BEND 2';  gen = '24384_23263' }
    @{ rvm = 'FTUBE 5'; gen = '24384_23264' }
    @{ rvm = 'FTUBE 6'; gen = '24384_23265' }
    @{ rvm = 'FTUBE 7'; gen = '24384_23266' }
)

$snap = Get-Content $Snapshot -Raw | ConvertFrom-Json

$auth = 'Basic ' + [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes('root:root'))
$headers = @{ Authorization = $auth; Accept = 'application/json'; 'surreal-ns' = $Ns; 'surreal-db' = $Database; NS = $Ns; DB = $Database }
$keys = ($ordered | ForEach-Object { "inst_relate:$($_.gen)" }) -join ', '
$sql = "SELECT record::id(id) AS refno, aabb_d.mins AS mins, aabb_d.maxs AS maxs FROM [$keys] WHERE id != NONE;"
$rows = (Invoke-RestMethod -Uri $Endpoint -Method Post -Headers $headers -Body $sql -ContentType 'text/plain')[0].result
$genMap = @{}
foreach ($r in $rows) { $genMap[$r.refno] = $r }

function Span($b) { if (-not $b) { return $null }; return @(($b[3] - $b[0]), ($b[4] - $b[1]), ($b[5] - $b[2])) }
function Fmt($v) { if ($null -eq $v) { return '       -' }; return ('{0,8:N1}' -f $v) }

"{0,-9} {1,-28} {2,-28} {3}" -f 'member', 'E3D (dx,dy,dz)', 'gen (dx,dy,dz)', 'verdict'
'-' * 88
$fail = 0
foreach ($pair in $ordered) {
    $m = $snap.members | Where-Object { $_.name -like "$($pair.rvm) *" } | Select-Object -First 1
    $rvmBox = if ($m) { $m.aabb_world_mm } else { $null }
    $g = $genMap[$pair.gen]
    $genBox = if ($g -and $g.mins -and $g.maxs) { @($g.mins[0], $g.mins[1], $g.mins[2], $g.maxs[0], $g.maxs[1], $g.maxs[2]) } else { $null }
    $rs = Span $rvmBox
    $gs = Span $genBox
    $verdict = 'n/a'
    if ($rs -and $gs) {
        $worst = 0.0
        for ($i = 0; $i -lt 3; $i++) {
            $tol = [math]::Max($AbsTol, $RelTol * $rs[$i])
            $d = [math]::Abs($rs[$i] - $gs[$i])
            if ($d - $tol -gt $worst) { $worst = $d - $tol }
        }
        if ($worst -le 0) { $verdict = 'OK' } else { $verdict = 'FAIL'; $fail++ }
    }
    $rtxt = if ($rs) { "$(Fmt $rs[0]),$(Fmt $rs[1]),$(Fmt $rs[2])" } else { '-' }
    $gtxt = if ($gs) { "$(Fmt $gs[0]),$(Fmt $gs[1]),$(Fmt $gs[2])" } else { '-' }
    "{0,-9} {1,-28} {2,-28} {3}" -f $pair.rvm, $rtxt, $gtxt, $verdict
}
''
"FAIL: $fail / $($ordered.Count)  (FTUB 应全 OK;BEND 若 FAIL 为已知的弯头几何独立缺陷)"
if ($fail -gt 0) { exit 1 } else { exit 0 }
