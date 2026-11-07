# 对 ams8000 的一个会话窗口 S→T 做三方核对：
#   e3d-model 增量（update_ams）  vs  两端全量（gen_ams --root）  vs  E3D TTY 导出的 RVM（rvm_compare）。
# 与 scripts/e3d/db8000_bran_scratch_*.mac 五条腿配套；证据见
# docs/evidence/2026-09-02-db8000-bran-scratch-tty-increment.md。
#
# 用法：
#   verify_db8000_bran_window.ps1 -RunDir output/db8000-bran-scratch/<ts> -Leg 1-create -Base 266 -Target 267 `
#       -Root 24384/26261 [-Rvm 1-created] [-RootAbsentAtBase] [-RootGoneAtTarget]
# -Rvm 给 output/db8000-bran-scratch/rvm/<name>.rvm 的文件名（宏里 SAVEWORK 前导出的那份）。
# 判据：upserts 逐 GeometryId 与 full@T 的 AABB/体积/三角数相同；removals 覆盖 full@S \ full@T 的全部键；
#       full@T 里增量没碰的键必须与 full@S 相同（没有漏改）。RVM 一侧只报配对与 AABB 档位，不设门。
# TUBI 也进对拍（2026-09-02 起 gen_ams 给管身发 `TUBI n of BRAN /X` 键，n 为容器内路由序；此前是 owner=PIPE +
# 支管兄弟序、全支管同键，rvm_compare 会以「生成侧主键重复」bail）。E3D 不导管身的支管上 TUBI 会落在 only_generated。
# 只能用 pwsh 7 跑：Windows PowerShell 5.1 的 ConvertFrom-Json 不展开顶层数组，upserts/elements 会被当成 1 条，
# 逐键对拍全部失真（表现为 upserts=1、[math]::Round 报无重载）。
param(
    [Parameter(Mandatory)][string]$RunDir,
    [Parameter(Mandatory)][string]$Leg,
    [Parameter(Mandatory)][int]$Base,
    [Parameter(Mandatory)][int]$Target,
    [Parameter(Mandatory)][string]$Root,
    [string]$Rvm,
    [switch]$RootAbsentAtBase,
    [switch]$RootGoneAtTarget
)
$ErrorActionPreference = 'Continue'
$REL = 'D:\work\plant-code\old\vendor\e3d-model\target\release'
$db = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001'
$common = @('--attlib','E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat','--db',$db,
    '--template','E:\reverse\e3d\shadow_e3d31_aps_all',
    '--catalogue','D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001',
    '--catalogue','D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps7600_0001',
    '--project-dir','D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000',
    '--project-dir','D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000')
$out = Join-Path $RunDir $Leg
New-Item -ItemType Directory -Force $out | Out-Null

function Key($e) { $e.geometry_id | ConvertTo-Json -Compress }
function Sig($e) { "$(($e.aabb_min|%{[math]::Round($_,3)}) -join ',')|$(($e.aabb_max|%{[math]::Round($_,3)}) -join ',')|vol=$([math]::Round($e.volume_mm3,1))|tri=$($e.triangles)" }
function LoadMap($p) { $m=@{}; if (Test-Path $p) { (Get-Content $p -Raw | ConvertFrom-Json) | % { $m[(Key $_)] = (Sig $_) } }; $m }

Write-Output "################ $Leg : window $Base -> $Target, root $Root ################"
Write-Output "==== update_ams ===="
& "$REL\update_ams.exe" @common --base-sesno $Base --target-sesno $Target --out "$out\update" 2>&1 | ? { $_ -notmatch '^project-dir' }
Write-Output "update_ams EXIT=$LASTEXITCODE"
$rep = Get-Content "$out\update\update-report.json" -Raw | ConvertFrom-Json
Write-Output ("changes: " + ($rep.changes | ConvertTo-Json -Compress))
Write-Output ("units regen=$($rep.units_to_regenerate) remove=$($rep.units_to_remove) derived=$($rep.derived_to_regenerate) | regenerated=$($rep.regenerated) derived_regen=$($rep.derived_regenerated) retired=$($rep.derived_retired) tubes +$($rep.tube_upserts) -$($rep.tube_removals) | unresolved=$(($rep.unresolved|Measure-Object).Count) no_model=" + ($rep.no_model | ConvertTo-Json -Compress))
if ($rep.cascades) { $rep.cascades | % { Write-Output "cascade: $($_.refno) $($_.noun): $($_.detail)" } }
if ($rep.notes) { $rep.notes | select -First 8 | % { Write-Output "note: $_" } }

Write-Output "==== gen_ams full generation ===="
if (-not $RootAbsentAtBase) {
    & "$REL\gen_ams.exe" @common --root $Root --sesno $Base --out "$out\full-S" 2>&1 | ? { $_ -match '^(visited|按 noun|Error|error)' }
    Write-Output "gen_ams@S EXIT=$LASTEXITCODE"
}
& "$REL\gen_ams.exe" @common --root $Root --sesno $Target --out "$out\full-T" 2>&1 | ? { $_ -match '^(visited|按 noun|Error|error)' }
Write-Output "gen_ams@T EXIT=$LASTEXITCODE  (RootGoneAtTarget=$RootGoneAtTarget)"

$fullS = LoadMap "$out\full-S\elements.json"
$fullT = LoadMap "$out\full-T\elements.json"
Write-Output "full@S elements=$($fullS.Count)  full@T elements=$($fullT.Count)"

Write-Output "==== increment upserts vs full@T (same GeometryId key) ===="
$ups = @(); if (Test-Path "$out\update\upserts.json") { $ups = @(Get-Content "$out\update\upserts.json" -Raw | ConvertFrom-Json) }
$match=0; $mismatch=0; $missing=0
foreach ($u in $ups) {
    $k = Key $u; $s = Sig $u; $f = $fullT[$k]
    if ($null -eq $f) { $missing++; Write-Output "MISSING-IN-FULL $k -> $s" }
    elseif ($f -eq $s) { $match++ }
    else { $mismatch++; Write-Output "MISMATCH $k`n  incr: $s`n  full: $f" }
}
Write-Output "upserts=$($ups.Count) match=$match mismatch=$mismatch missing_in_full=$missing"

Write-Output "==== increment removals vs (full@S \ full@T) ===="
$rems = @(); if (Test-Path "$out\update\removals.json") { $rems = @(Get-Content "$out\update\removals.json" -Raw | ConvertFrom-Json) }
$remKeys = @{}; foreach ($rm in $rems) { $remKeys[($rm | ConvertTo-Json -Compress)] = 1 }
$gone = @($fullS.Keys | ? { -not $fullT.ContainsKey($_) })
$goneNotRemoved = @($gone | ? { -not $remKeys.ContainsKey($_) })
$removedStillInT = @($remKeys.Keys | ? { $fullT.ContainsKey($_) })
$removedNeverExisted = @($remKeys.Keys | ? { -not $fullS.ContainsKey($_) -and -not $fullT.ContainsKey($_) })
Write-Output "removals=$($rems.Count) | keys gone S->T=$($gone.Count) | gone-but-NOT-removed=$($goneNotRemoved.Count) | removed-but-still-in-T=$($removedStillInT.Count) | removed-never-existed(zero-length ensure-absent)=$($removedNeverExisted.Count)"
$goneNotRemoved | % { Write-Output "GONE-NOT-REMOVED $_" }
$removedStillInT | % { Write-Output "REMOVED-STILL-IN-T $_" }

Write-Output "==== coverage: full@T keys not touched by increment must be unchanged from full@S ===="
$upKeys = @{}; foreach ($u in $ups) { $upKeys[(Key $u)] = 1 }
$changedUntouched = @($fullT.Keys | ? { -not $upKeys.ContainsKey($_) -and $fullS.ContainsKey($_) -and $fullS[$_] -ne $fullT[$_] })
$newUntouched = @($fullT.Keys | ? { -not $upKeys.ContainsKey($_) -and -not $fullS.ContainsKey($_) })
Write-Output "changed-but-not-upserted=$($changedUntouched.Count) new-but-not-upserted=$($newUntouched.Count)"
$changedUntouched | % { Write-Output "CHANGED-NOT-UPSERTED $_`n  S: $($fullS[$_])`n  T: $($fullT[$_])" }
$newUntouched | % { Write-Output "NEW-NOT-UPSERTED $_" }

if ($Rvm) {
    Write-Output "==== RVM: $Rvm.rvm vs full@T (TUBI included) ===="
    $rvmFile = "D:\work\plant-code\old\gen-model\output\db8000-bran-scratch\rvm\$Rvm.rvm"
    $snap = "$rvmFile.json"
    & D:\Rust\target\debug\rvm_verify.exe import --rvm $rvmFile --dbnum 8000 --scope narrow --root-refno $Root --out $snap 2>&1 | select -Last 3
    Write-Output "rvm_verify import EXIT=$LASTEXITCODE"
    & "$REL\examples\rvm_facets_dump.exe" --rvm $rvmFile --contains "of BRANCH /CODEX_DB8000_BRAN_SCRATCH" --out "$out\rvm-facets.json" 2>&1 | select -Last 2
    & "$REL\rvm_compare.exe" --baseline $snap --elements "$out\full-T\elements.json" --facets "$out\rvm-facets.json" --out "$out\rvm-compare.json" 2>&1
    Write-Output "rvm_compare EXIT=$LASTEXITCODE"
    if (Test-Path "$out\rvm-compare.json") {
        $cmp = Get-Content "$out\rvm-compare.json" -Raw | ConvertFrom-Json
        Write-Output ("rvm by_noun: " + ($cmp.by_noun | ConvertTo-Json -Compress))
        $cmp.only_generated | % { Write-Output "RVM-ONLY-GENERATED $_" }
        $cmp.only_baseline | % { Write-Output "RVM-ONLY-BASELINE $_" }
    }
}

