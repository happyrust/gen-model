# old-parse-pdms-db 退出护栏（docs/plans/2026-09-04-parse-pdms-db-retirement-inventory.md B9；
# 2026-09-04 从「只许在 legacy 门后」收紧为「一处都不许有」）。
#
#   1. 整个工作区（默认 features）的依赖图里没有 parse_pdms_db：`cargo tree --workspace -e normal`
#      一行都不许提到它，`cargo tree -i parse_pdms_db` 必须匹配不到任何包；
#   2. 根 Cargo.toml / python/Cargo.toml 不得再声明 parse_pdms_db 依赖、old-parse-pdms-db 的
#      [patch]，或 `legacy_parse_pdms_db` feature；
#   3. src/、tests/、python/src/ 的代码行不许写 `parse_pdms_db::` / `use parse_pdms_db`，也不许
#      再挂 `feature = "legacy_parse_pdms_db"` 门；
#   4. 基线编排（versioned_db/baseline_ingest.rs 与 database.rs）全文不含 legacy 解析器的中间态
#      （DbBasicData / member_prune）——这一条与依赖无关，是防止基线偷偷退回第二条收集路（宪法 II）。
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
# 交互式 pwsh 的 profile 可能把 rg 换成函数（例如转发给 tgrep，没有服务进程时只查过期索引），
# 护栏会因此静默漏报；这里优先固定调用 PATH 上真正的 ripgrep 可执行文件。
#
# 但 GitHub 的 windows-latest runner **不自带 rg**，workflow 也不装它。脚本自 B9
# 把护栏接进 CI 那天起就硬依赖 rg，这一步在 CI 上一直是 `CommandNotFoundException`。
# 所以 rg 只当加速手段：找不到就回退到 PowerShell 原生实现（.NET 正则支持同样的
# lookahead / lookbehind）。两条路径都走下面的 canary 自证，谁都不会静默变成空门。
$rgExe = (Get-Command rg -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1).Path

# 统一的扫描入口：返回命中行（`路径:行号: 内容`），零命中返回空数组。
# 判定「有没有命中」必须看返回的条数，不能看退出码——两条实现路径的退出码语义不同。
function Invoke-GuardScan {
    param(
        [Parameter(Mandatory)][string] $ScanPattern,
        [Parameter(Mandatory)][string[]] $ScanPaths,
        [string] $FileGlob = '*.rs'
    )
    if ($rgExe) {
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $out = & $rgExe -n --pcre2 --glob $FileGlob $ScanPattern @ScanPaths
        $code = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($code -eq 1) { return @() }
        if ($code -ne 0) { throw "rg failed with exit code $code" }
        return @($out)
    }
    $regex = [regex]::new($ScanPattern, 'Compiled')
    $hits = @()
    foreach ($path in $ScanPaths) {
        $full = if ([System.IO.Path]::IsPathRooted($path)) { $path } else { Join-Path $root $path }
        $items = if (Test-Path -LiteralPath $full -PathType Leaf) {
            @(Get-Item -LiteralPath $full)
        } else {
            @(Get-ChildItem -LiteralPath $full -Recurse -Filter $FileGlob -File -ErrorAction SilentlyContinue)
        }
        foreach ($file in $items) {
            $lineNo = 0
            foreach ($line in [System.IO.File]::ReadAllLines($file.FullName)) {
                $lineNo++
                if ($regex.IsMatch($line)) {
                    $shown = if ([System.IO.Path]::IsPathRooted($path)) { $file.FullName } else {
                        $file.FullName.Substring($root.Length).TrimStart('\', '/')
                    }
                    $hits += ('{0}:{1}: {2}' -f $shown, $lineNo, $line.Trim())
                }
            }
        }
    }
    return $hits
}

Push-Location $root
try {
    # 冷缓存下 cargo 会往 stderr 打进度（`Downloading crates ...`）。`powershell.exe`（5.1）
    # 在 Stop 模式下把原生命令的任何 stderr 行都当异常抛出，护栏在干净 runner 上必然红。
    # 这一段只放宽 `$ErrorActionPreference`，判定仍靠退出码与 `$tree` 的内容。
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $tree = (& cargo tree --workspace -e normal --locked 2>&1 | ForEach-Object { "$_" } | Out-String)
    $treeExit = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($treeExit -ne 0) { $tree; throw 'cargo tree --workspace (default features) failed' }
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
            $hit = @(Invoke-GuardScan -ScanPattern $scanPattern -ScanPaths @($probe))
            if ($hit.Count -eq 0) {
                throw 'guard scanner is dead: 样本行没被模式命中，零命中不可信'
            }
        } finally {
            Remove-Item -Recurse -Force $probe
        }
    }
    Assert-ScannerIsAlive $pattern 'use parse_pdms_db::parse::EleData;'
    'SCANNER_IS_ALIVE'

    $leaks = @(Invoke-GuardScan -ScanPattern $pattern -ScanPaths @('src', 'tests', 'python\src'))
    if ($leaks.Count -gt 0) {
        $leaks
        throw 'these source lines still reference old-parse-pdms-db or its feature gate'
    }
    'SOURCE_HAS_NO_PARSE_PDMS_DB'

    # 4. 基线编排不许退回 legacy 解析器的中间态：DbBasicData 是 parse_db_basic_data 的产物，
    #    member_prune 是补链轮的事后裁决，e3d-io 退化窗口一个都不经过。
    #    2026-09-07 起基线编排在 versioned_db/baseline_ingest.rs（sync_total_async_threaded 壳退役），
    #    database.rs 只剩 SYS 引导与统计修复——两处都扫。
    $baselineFiles = @('src/versioned_db/baseline_ingest.rs', 'src/versioned_db/database.rs')
    $intermediatePattern = '^(?!\s*//).*(DbBasicData|member_prune)'
    Assert-ScannerIsAlive $intermediatePattern 'let basic: DbBasicData = read_full_basic_data();'
    foreach ($baselineFile in $baselineFiles) {
        $intermediates = @(Invoke-GuardScan -ScanPattern $intermediatePattern -ScanPaths @($baselineFile) -FileGlob '*.rs')
        if ($intermediates.Count -gt 0) {
            $intermediates
            throw "$baselineFile fell back to the legacy parser's intermediates (DbBasicData / member_prune)"
        }
    }
    'BASELINE_HAS_NO_LEGACY_INTERMEDIATES'
} finally {
    Pop-Location
}
