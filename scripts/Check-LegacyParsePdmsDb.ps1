# old-parse-pdms-db 退出护栏（docs/plans/2026-09-04-parse-pdms-db-retirement-inventory.md B9；
# 2026-09-04 从「只许在 legacy 门后」收紧为「一处都不许有」）。
#
#   1. 整个工作区（默认 features）的依赖图里没有 parse_pdms_db：`cargo tree --workspace -e normal`
#      一行都不许提到它，`cargo tree -i parse_pdms_db` 必须匹配不到任何包；
#   2. 根 Cargo.toml / python/Cargo.toml 不得再声明 parse_pdms_db 依赖、old-parse-pdms-db 的
#      [patch]，或 `legacy_parse_pdms_db` feature；
#   3. src/、tests/、python/src/ 的代码行不许写 `parse_pdms_db::` / `use parse_pdms_db`，也不许
#      再挂 `feature = "legacy_parse_pdms_db"` 门；
#   4. 基线编排（versioned_db/database.rs）全文不含 legacy 解析器的中间态（DbBasicData /
#      member_prune）——这一条与依赖无关，是防止基线偷偷退回第二条收集路（宪法 II）。
#
# 第 2–4 条另有 `data_interface::mod.rs::legacy_parse_pdms_db_guard` 三条源码测试逐文件判定；
# 这里是不进 cargo test 也能跑的「文件级」同款扫描。基线全量解析走 e3d-io 退化窗口
# （e3d_collector::stream_baseline + versioned_db::baseline），夹具验证闸走
# db_file_header::snapshot_presence，差分 oracle 走 e3d_collector::read_ele_data；对拍以 E3D TTY
# 为准（共识 d-6），所以 legacy 解析器连「参照臂」这个身份也不再有。
#
# Usage: powershell -File scripts\Check-LegacyParsePdmsDb.ps1
$ErrorActionPreference = 'Stop'
$root = Join-Path $PSScriptRoot '..' | Resolve-Path | Select-Object -ExpandProperty Path
Push-Location $root
try {
    # 1. 依赖图。
    $tree = (& cargo tree --workspace -e normal --locked 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { $tree; throw 'cargo tree --workspace (default features) failed' }
    $hits = @(($tree -split "`n") | Where-Object { $_ -match 'parse_pdms_db|old-parse-pdms-db' })
    if ($hits.Count -gt 0) {
        $hits
        throw 'parse_pdms_db is still in the workspace dependency graph'
    }
    'WORKSPACE_GRAPH_HAS_NO_PARSE_PDMS_DB'

    # 期望的结果是 cargo 报「package ID specification `parse_pdms_db` did not match any packages」
    # 并以非零退出；那个 stderr 在 Stop 模式下会被当成异常，所以这一段临时放宽。
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $inverse = (& cargo tree --workspace -e normal --locked -i parse_pdms_db 2>&1 | ForEach-Object { "$_" } | Out-String)
    $inverseExit = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($inverseExit -eq 0 -or $inverse -match 'parse_pdms_db v' -or $inverse -notmatch 'did not match any packages') {
        $inverse
        throw 'cargo tree -i parse_pdms_db still resolves a package - some manifest still depends on old-parse-pdms-db'
    }
    'NO_PACKAGE_DEPENDS_ON_PARSE_PDMS_DB'

    # 2. manifest。
    $manifestLeaks = @()
    foreach ($manifest in @('Cargo.toml', 'python\Cargo.toml')) {
        $lineNo = 0
        foreach ($line in [System.IO.File]::ReadAllLines((Join-Path $root $manifest))) {
            $lineNo++
            if ($line.TrimStart().StartsWith('#')) { continue }
            if ($line -match 'parse_pdms_db|old-parse-pdms-db|legacy_parse_pdms_db') {
                $manifestLeaks += ('{0}:{1}: {2}' -f $manifest, $lineNo, $line.Trim())
            }
        }
    }
    if ($manifestLeaks.Count -gt 0) {
        $manifestLeaks
        throw 'a manifest still declares old-parse-pdms-db or its feature gate'
    }
    'MANIFESTS_HAVE_NO_PARSE_PDMS_DB'

    # 3. 源码。只认代码行：注释行与字符串字面量里的 "parse_pdms_db::" 不算。
    #
    # 引号一律写成 `\x22`，不许出现字面量 `"`：Windows PowerShell 5.1 把带引号的参数交给原生
    # rg 时会把它吃掉，正则退化成匹配不上任何东西、rg 以「无命中」退出，这道门于是静默变成
    # 空门——而 CI 正是用 `powershell`（5.1）跑它。2026-09-04 实测：同一条 `(?<!")` 模式在
    # pwsh 7 上命中 2 个文件、在 5.1 上命中 0 个，且两处都不报错。
    $pattern = '^(?!\s*//).*(?<!\x22)(parse_pdms_db::|use parse_pdms_db|feature = \x22legacy_parse_pdms_db\x22)'

    # 护栏的护栏（宪法 III）：开扫之前先拿一份必然命中的样本证明扫描是活的，
    # 否则「零命中」既可能是干净、也可能是扫描根本没跑，两者在输出上分不出来。
    function Assert-ScannerIsAlive([string] $scanPattern, [string] $mustMatch) {
        $probe = Join-Path ([System.IO.Path]::GetTempPath()) ('guard-canary-' + [guid]::NewGuid())
        New-Item -ItemType Directory -Path $probe | Out-Null
        try {
            [System.IO.File]::WriteAllText((Join-Path $probe 'canary.rs'), $mustMatch)
            & rg -q --pcre2 --glob '*.rs' $scanPattern $probe
            if ($LASTEXITCODE -ne 0) {
                throw ('guard scanner is dead (rg exit {0}): 样本行没被模式命中，零命中不可信' -f $LASTEXITCODE)
            }
        } finally {
            Remove-Item -Recurse -Force $probe
        }
    }
    Assert-ScannerIsAlive $pattern 'use parse_pdms_db::parse::EleData;'
    'SCANNER_IS_ALIVE'

    $leaks = & rg -n --pcre2 --glob '*.rs' $pattern src tests python\src
    if ($LASTEXITCODE -eq 0) {
        $leaks
        throw 'these source lines still reference old-parse-pdms-db or its feature gate'
    } elseif ($LASTEXITCODE -ne 1) {
        throw "rg failed with exit code $LASTEXITCODE"
    }
    'SOURCE_HAS_NO_PARSE_PDMS_DB'

    # 4. 基线编排不许退回 legacy 解析器的中间态：DbBasicData 是 parse_db_basic_data 的产物，
    #    member_prune 是补链轮的事后裁决，e3d-io 退化窗口一个都不经过。
    $baselineFile = 'src/versioned_db/database.rs'
    $intermediatePattern = '^(?!\s*//).*(DbBasicData|member_prune)'
    Assert-ScannerIsAlive $intermediatePattern 'let basic: DbBasicData = read_full_basic_data();'
    $intermediates = & rg -n --pcre2 $intermediatePattern (Join-Path $root $baselineFile)
    if ($LASTEXITCODE -eq 0) {
        $intermediates
        throw "$baselineFile fell back to the legacy parser's intermediates (DbBasicData / member_prune)"
    }
    if ($LASTEXITCODE -ne 1) { throw "rg failed with exit code $LASTEXITCODE" }
    'BASELINE_HAS_NO_LEGACY_INTERMEDIATES'
} finally {
    Pop-Location
}
