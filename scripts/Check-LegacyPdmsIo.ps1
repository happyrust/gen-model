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
# 交互式 pwsh 的 profile 可能把 rg 换成函数（例如转发给 tgrep，没有服务进程时只查过期索引），
# 护栏会因此静默漏报；这里优先固定调用 PATH 上真正的 ripgrep 可执行文件。
#
# 但 GitHub 的 windows-latest runner **不自带 rg**，workflow 也不装它。脚本自 B9
# （`06bb56fa3`）引入这条硬依赖起，这一步在 CI 上一直是 `CommandNotFoundException`，
# 从来没绿过——只是没人往触发分支推过。所以 rg 只当加速手段：找不到就回退到
# PowerShell 原生实现（.NET 正则同样支持这里的 lookahead / lookbehind）。
$rgExe = (Get-Command rg -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1).Path
Push-Location $root
try {
    # 冷缓存下 cargo 会往 stderr 打进度（`Downloading crates ...` / `Updating crates.io index`）。
    # `powershell.exe`（5.1）在 Stop 模式下把原生命令的任何 stderr 行都当异常抛出，
    # 于是这条护栏在 CI 的干净 runner 上必然红（本地有缓存所以看不到）。这一段只放宽
    # `$ErrorActionPreference`，判定仍靠 `$LASTEXITCODE` 与 `$tree` 的内容。
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $tree = (& cargo tree --workspace -e normal --locked 2>&1 | ForEach-Object { "$_" } | Out-String)
    $treeExit = $LASTEXITCODE
    $ErrorActionPreference = $previous
    if ($treeExit -ne 0) { $tree; throw 'cargo tree --workspace (default features) failed' }
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
    if ($rgExe) {
        # 同样要放宽 Stop：rg 在无匹配时按约定以 1 退出，5.1 会把它算成原生错误。
        $previous = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $leaks = & $rgExe -n --pcre2 --glob '*.rs' $pattern src tests python\src
        $rgExit = $LASTEXITCODE
        $ErrorActionPreference = $previous
        if ($rgExit -eq 0) {
            $leaks
            throw 'these source lines still reference old-pdms-io or its feature gates'
        } elseif ($rgExit -ne 1) {
            throw "rg failed with exit code $rgExit"
        }
    } else {
        # runner 上没有 rg：同样的正则交给 .NET，逐文件逐行扫。判定口径与 rg 分支一致。
        Write-Host 'rg 不在 PATH 上，改用 PowerShell 原生扫描'
        $regex = [regex]::new($pattern, 'Compiled')
        $leaks = @()
        foreach ($dir in @('src', 'tests', 'python\src')) {
            $full = Join-Path $root $dir
            if (-not (Test-Path -LiteralPath $full)) { continue }
            foreach ($file in Get-ChildItem -LiteralPath $full -Recurse -Filter '*.rs' -File) {
                $lineNo = 0
                foreach ($line in [System.IO.File]::ReadAllLines($file.FullName)) {
                    $lineNo++
                    if ($regex.IsMatch($line)) {
                        $rel = $file.FullName.Substring($root.Length).TrimStart('\', '/')
                        $leaks += ('{0}:{1}: {2}' -f $rel, $lineNo, $line.Trim())
                    }
                }
            }
        }
        if ($leaks.Count -gt 0) {
            $leaks
            throw 'these source lines still reference old-pdms-io or its feature gates'
        }
    }
    'SOURCE_HAS_NO_PDMS_IO'
} finally {
    Pop-Location
}
