# old-pdms-io 退出护栏（docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md §0；
# 2026-09-04 从「只许在 legacy 门后」收紧为「一处都不许有」）。
#
#   1. 整个工作区（默认 features）的依赖图里没有 pdms_io：`cargo tree --workspace -i pdms_io`
#      必须匹配不到任何包，`cargo tree --workspace -e normal` 一行都不许提到它。这一条同时
#      认连字符的 `pdms-io`：`pdmsdb_engine_v2`（git = pdms-io.git）就是从这个仓出来、但包名
#      里没有 `pdms_io` 的，2026-09-04 之前它挂在 parse_pdms_db 下面躲过了本条的下划线匹配；
#   2. 根 Cargo.toml / python/Cargo.toml 不得再声明 pdms_io 依赖、old-pdms-io 的 [patch]，
#      或 `legacy_pdms_io` / `legacy_session_replay` / `debug_parse` 任何一个 feature；
#   3. src/、tests/、python/src/ 的代码行不许写 `pdms_io::` / `use pdms_io`，也不许再挂
#      `feature = "legacy_pdms_io"` / `"legacy_session_replay"` 门。
#
# 第 2、3 条另有 `data_interface::mod.rs::legacy_pdms_io_guard` 两条源码测试逐文件判定；
# 这里是不进 cargo test 也能跑的「文件级」同款扫描。对拍的判据是 E3D TTY 录下的会话链 / 宏
# （tests/db8000_session_pairs 夹具、scripts/e3d/Test-TtyNetWindow.ps1、verify_db8000_bran_window.ps1），
# 不是第二套解析器——所以 old-pdms-io 连「参照臂」这个身份也不再有。
#
# Usage: powershell -File scripts\Check-LegacyPdmsIo.ps1
$ErrorActionPreference = 'Stop'
$root = Join-Path $PSScriptRoot '..' | Resolve-Path | Select-Object -ExpandProperty Path
Push-Location $root
try {
    $tree = (& cargo tree --workspace -e normal --locked 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { $tree; throw 'cargo tree --workspace (default features) failed' }
    $hits = @(($tree -split "`n") | Where-Object { $_ -match 'pdms_io|pdms-io' })
    if ($hits.Count -gt 0) {
        $hits
        throw 'pdms_io (or a package sourced from pdms-io.git) is still in the workspace dependency graph'
    }
    'WORKSPACE_GRAPH_HAS_NO_PDMS_IO'

    # 期望的结果是 cargo 报「package ID specification `pdms_io` did not match any packages」并以非零退出；
    # 那个 stderr 在 Stop 模式下会被当成异常，所以这一段临时放宽。
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $inverse = (& cargo tree --workspace -e normal --locked -i pdms_io 2>&1 | ForEach-Object { "$_" } | Out-String)
    $inverseExit = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($inverseExit -eq 0 -or $inverse -match 'pdms_io v' -or $inverse -notmatch 'did not match any packages') {
        $inverse
        throw 'cargo tree -i pdms_io still resolves a package - some manifest still depends on old-pdms-io'
    }
    'NO_PACKAGE_DEPENDS_ON_PDMS_IO'

    $manifestLeaks = @()
    foreach ($manifest in @('Cargo.toml', 'python\Cargo.toml')) {
        $lineNo = 0
        foreach ($line in [System.IO.File]::ReadAllLines((Join-Path $root $manifest))) {
            $lineNo++
            if ($line.TrimStart().StartsWith('#')) { continue }
            if ($line -match 'pdms_io|old-pdms-io|legacy_pdms_io|legacy_session_replay|debug_parse') {
                $manifestLeaks += ('{0}:{1}: {2}' -f $manifest, $lineNo, $line.Trim())
            }
        }
    }
    if ($manifestLeaks.Count -gt 0) {
        $manifestLeaks
        throw 'a manifest still declares old-pdms-io or one of its feature gates'
    }
    'MANIFESTS_HAVE_NO_PDMS_IO'

    # 只认代码行：注释行与字符串字面量里的 "pdms_io::"（护栏测试自己的禁词表）不算。
    $pattern = '^(?!\s*//).*(?<!")(pdms_io::|use pdms_io|feature = "legacy_pdms_io"|feature = "legacy_session_replay")'
    $leaks = & rg -n --pcre2 --glob '*.rs' $pattern src tests python\src
    if ($LASTEXITCODE -eq 0) {
        $leaks
        throw 'these source lines still reference old-pdms-io or its feature gates'
    } elseif ($LASTEXITCODE -ne 1) {
        throw "rg failed with exit code $LASTEXITCODE"
    }
    'SOURCE_HAS_NO_PDMS_IO'
} finally {
    Pop-Location
}
