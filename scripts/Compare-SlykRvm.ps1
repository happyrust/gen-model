<#
.SYNOPSIS
    把一批生成根逐个拿 E3D 原生 RVM 对拍：TTY 导出 → 快照 → 直读生成 → 逐件比 AABB。

.DESCRIPTION
    一条流水线，五步，每步都用仓里现成的工具，本脚本只负责串起来并把判决收成一张表：

      1. E3D 3.1 DESIGN TTY（`scripts/e3d/run_ams_c_entrymacro.bat`）跑一段窄口径导出宏
         （`repre insu/obst off`），产出 `<root>.rvm`。只读，宏里没有 SAVEWORK。
      2. `rvm_verify import --scope narrow` 把 RVM 解成基准快照。
      3. `gen_ams --root` 直读库文件生成同一棵子树，产出 `elements.json`。
      4. `rvm_facets_dump` 从面片顶点实算世界 AABB——**不能信 RVM 记录自带的 bbox**，
         rvm-rs 会把它当局部盒再乘一次几何 transform（见 `rvm_compare` 头注）。
      5. `rvm_compare` 逐件比，`off` 一件都不许有。

    为什么不接服务：对拍要的是「我们的几何对不对」，`gen_ams` 直读库文件走的是同一条
    e3d-model 管线，少一层服务与存储；服务那一侧另有 `Probe-SlykLoad.ps1` 覆盖。

.PARAMETER Roots
    要对拍的生成根，形如 `21909/16772`。与 `-FromCsv` 二选一。

.PARAMETER FromCsv
    读一份 CSV，要有 `zone_refno` 与 `dbnum` 两列（`output/slyk-rvm/zones.csv` 就是）。

.PARAMETER Dbnum
    `-Roots` 模式下每个根所在的设计库号；给一个就是全体共用。

.PARAMETER Out
    产物目录，默认 `output/slyk-rvm/sweep`。

.PARAMETER Skip
    跳过前 N 个根（断点续跑）。

.EXAMPLE
    ./scripts/Compare-SlykRvm.ps1 -Roots 21909/16772 -Dbnum 5525

.EXAMPLE
    ./scripts/Compare-SlykRvm.ps1 -FromCsv output/slyk-rvm/zones.csv -Out output/slyk-rvm/sweep
#>
[CmdletBinding()]
param(
    [string[]]$Roots,
    [string]$FromCsv,
    [int]$Dbnum,
    [string]$Out = 'output/slyk-rvm/sweep',
    [int]$Skip = 0,
    [int]$Take = 0,
    # 复用上一轮已经落盘的 `.rvm`：E3D 基准与我们的改动无关，改完生成侧重跑时
    # 不必再开 80 次 TTY 会话（一次 ~5s）。导出口径变了就别加这个。
    [switch]$ReuseExport,
    # E3D 会话的 `Display Arctolerance`（mm），缺省是它自己的 10mm。曲面基本体的
    # 面片就按它铺：10mm 下一个 R=14000 的罐顶封头只有二十几段，AABB 比名义值
    # 缩进近 95mm——那是基准的采样，不是我们的几何。要逐件比 AABB 就得让两边
    # 铺得一样密（我们是 0.5mm，见 `discretise::CHORD_TOL_MM`）。
    [double]$ArcTolerance = 0
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$out = if ([IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $repo $Out }
New-Item -ItemType Directory -Force $out | Out-Null

$schema = Join-Path $repo 'vendor\shared\e3d-io\schema\e3d31\binaries'
$acw = 'D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000'
$bin = 'D:\Rust\target\debug'

# E3D 会话的环境：装哪套安装、开哪个工程、进哪个 MDB。改这里之前先看
# `docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md`。
$env:L3_E3D_INSTALL_DIR = 'E:\reverse\e3d\shadow_e3d31_gen_model_test'
$env:L3_E3D_PROJECTS_DIR = 'D:\AVEVA\Projects\E3D3.1'
$env:L3_E3D_PROJECT_EVAR = 'D:\AVEVA\Projects\E3D3.1\YCYK-E3D\evarsYCYK-E3D.bat'
$env:L3_E3D_PROJECT = 'ACW'
$env:L3_E3D_LOGIN = 'SYSTEM/XXXXXX'
$env:L3_E3D_MDB = '/SLYK'
$env:L3_E3D_TIMEOUT_SECONDS = '1200'

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
Write-Host ("对拍 {0} 个根，产物落 {1}" -f @($targets).Count, $out)

function Invoke-Tty {
    param([string]$Body, [string]$Tag)
    $dir = Join-Path $out $Tag
    New-Item -ItemType Directory -Force $dir | Out-Null
    $fw = { param($p) $p -replace '\\', '/' }
    Remove-Item (Join-Path $dir 'alive.log'), (Join-Path $dir 'done.log') -Force -ErrorAction SilentlyContinue
    $mac = Join-Path $dir 'export.mac'
    Set-Content $mac $Body -Encoding ascii
    # 驱动认的是 `L3-ALIVE` / `L3-DONE` 两个标记，不是退出码：影子安装在
    # DLL_PROCESS_DETACH 里必挂，那时宏早跑完了（见 `src/e3d_query.rs` 的注释）。
    $wrapper = @"
ALPHA LOG "$(& $fw (Join-Path $dir 'alive.log'))" OVER
`$P L3-ALIVE
ALPHA LOG END
`$M "$(& $fw $mac)"
ALPHA LOG "$(& $fw (Join-Path $dir 'done.log'))" OVER
`$P L3-DONE
ALPHA LOG END
QUIT
"@
    $wp = Join-Path $dir 'wrapper.mac'
    Set-Content $wp $wrapper -Encoding ascii
    & cmd /d /c (Join-Path $repo 'scripts\e3d\run_ams_c_entrymacro.bat') ($wp -replace '\\', '/') *> (Join-Path $dir 'driver.log')
    return ((Test-Path (Join-Path $dir 'alive.log')) -and (Test-Path (Join-Path $dir 'done.log')))
}

$results = @()
$i = 0
foreach ($t in $targets) {
    $i++
    $tag = $t.refno -replace '/', '-'
    $dir = Join-Path $out $tag
    $rvm = Join-Path $dir "$tag.rvm"
    $row = [ordered]@{ refno = $t.refno; dbnum = $t.dbnum; label = $t.label; stage = ''; verdict = ''; detail = '' }
    Write-Host ("[{0}/{1}] {2} {3}" -f $i, @($targets).Count, $t.refno, $t.label) -ForegroundColor Cyan

    # ── 1. TTY 导出 ─────────────────────────────────────────────────────────
    $body = @"
ALPHA LOG "$((Join-Path $dir 'export.log') -replace '\\','/')" OVER
`$P SLYK-RVM-EXPORT-BEGIN
=$($t.refno)
Q REF
Q TYPE
repre insu off
repre obst off
repre tube on$(if ($ArcTolerance -gt 0) { "`nrepre arctol $ArcTolerance`nQ REPRE ARCTOL" })
export implied tube into separate containers
export system /expdri.so
export file "$($rvm -replace '\\','/')"
export filenote 'SLYK narrow-scope baseline for $($t.refno)'
export holes on
export autocolour displayexport on
export repr on
export CE
export finish
`$P SLYK-RVM-EXPORT-DONE
ALPHA LOG END
"@
    $ok = $true
    if ($ReuseExport -and (Test-Path $rvm)) {
        Write-Host '    （复用已有 .rvm）' -ForegroundColor DarkGray
    }
    else {
        Remove-Item $rvm -Force -ErrorAction SilentlyContinue
        $ok = Invoke-Tty -Body $body -Tag $tag
    }
    if (-not $ok -or -not (Test-Path $rvm)) {
        $row.stage = 'tty-export'; $row.verdict = 'FAIL'
        $row.detail = if ($ok) { 'TTY 跑完了但没落出 .rvm' } else { '会话没走到 L3-DONE' }
        $results += [pscustomobject]$row; continue
    }

    # ── 2. 基准快照 ─────────────────────────────────────────────────────────
    $snap = "$rvm.json"
    $log = & "$bin\rvm_verify.exe" import --rvm $rvm --dbnum $t.dbnum --root-refno $t.refno --scope narrow --out $snap 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $row.stage = 'import'; $row.verdict = 'FAIL'; $row.detail = ($log.Trim() -split "`n")[-1]
        $results += [pscustomobject]$row; continue
    }

    # ── 3. 直读生成 ─────────────────────────────────────────────────────────
    $dbfile = Join-Path $acw ("acw{0:d4}_0001" -f $t.dbnum)
    $gen = Join-Path $dir 'gen'
    $log = & "$bin\gen_ams.exe" --attlib "$schema\attlib.dat" --db $dbfile --template $schema --db-type DESI --root $t.refno --project-dir $acw --out $gen 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $row.stage = 'gen_ams'; $row.verdict = 'FAIL'; $row.detail = ($log.Trim() -split "`n")[-1]
        $results += [pscustomobject]$row; continue
    }
    $row.generated = ([regex]::Match($log, 'generated=(\d+)')).Groups[1].Value

    # ── 4. 面片实算 AABB ────────────────────────────────────────────────────
    $facets = Join-Path $dir 'facets.json'
    $log = & "$bin\examples\rvm_facets_dump.exe" --rvm $rvm --contains "" --out $facets 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        $row.stage = 'facets'; $row.verdict = 'FAIL'; $row.detail = ($log.Trim() -split "`n")[-1]
        $results += [pscustomobject]$row; continue
    }

    # ── 5. 对拍 ─────────────────────────────────────────────────────────────
    $cmp = Join-Path $dir 'compare.json'
    $log = & "$bin\rvm_compare.exe" --baseline $snap --elements (Join-Path $gen 'elements.json') --facets $facets --out $cmp 2>&1 | Out-String
    $code = $LASTEXITCODE
    Set-Content (Join-Path $dir 'compare.log') $log -Encoding utf8
    $row.stage = 'compare'
    $row.verdict = if ($code -eq 0) { 'PASS' } else { 'OFF' }
    $onlyBase = ([regex]::Matches($log, '只在基准 (\d+)') | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
    $onlyGen = ([regex]::Matches($log, '只在生成 (\d+)') | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
    $row.only_baseline = $onlyBase
    $row.only_generated = $onlyGen
    $row.detail = (($log -split "`n") | Where-Object { $_ -match '超出弦高容差|^\s+off ' } | Select-Object -First 3) -join ' | '
    $results += [pscustomobject]$row
    Write-Host ("    {0}  only_baseline={1} only_generated={2}" -f $row.verdict, $onlyBase, $onlyGen) `
        -ForegroundColor $(if ($row.verdict -eq 'PASS') { 'Green' } else { 'Yellow' })
}

$results | Export-Csv (Join-Path $out 'summary.csv') -NoTypeInformation -Encoding utf8
$results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $out 'summary.json') -Encoding utf8
Write-Host ''
Write-Host '=== 汇总 ===' -ForegroundColor Cyan
$results | Group-Object verdict | Sort-Object Count -Descending | ForEach-Object { "  {0,-6} {1}" -f $_.Name, $_.Count }
Write-Host ("明细：{0}" -f (Join-Path $out 'summary.csv'))
