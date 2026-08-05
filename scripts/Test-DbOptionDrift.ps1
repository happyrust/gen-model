<#
.SYNOPSIS
    拦下混进 DbOption.toml 的本机台架旋钮。

.DESCRIPTION
    DbOption.toml 里 74 个键中，绝大多数是该入库的共享决策（project_path、
    included_projects、交付单元类型、房间关键字，注释里还带着 ADR 引用与评审决议），
    只有少数几个是跑台架时必须按机器改的旋钮：换端口、换 MDB、换本期执行的库。

    两类东西挤在同一个被跟踪的文件里，于是每次实测都会把本机旋钮带进提交——289 个
    提交碰过这个文件就是这么来的。本脚本只做一件事：提交前发现旋钮值变了就拦下来。

    这是止血，不是根治。根治是给加载器加一层 gitignore 的 DbOption.local.toml
    覆盖层，且必须落在 aios_core::get_db_option() 那一层，否则本仓库 33 个调用点会
    绕过它。覆盖层到位之前，被拦下时的做法是把那几行改回基线值再提交。

.PARAMETER Mode
    Staged   比对暂存区与 HEAD，适合当 pre-commit 钩子（默认）。
    Worktree 比对工作区与 HEAD，适合随手自查。

.PARAMETER Knob
    视为本机旋钮的键名。

.PARAMETER AllowKnobDrift
    基线值确实要改时（例如团队真的迁了 API 端口）用它放行。

.EXAMPLE
    ./scripts/Test-DbOptionDrift.ps1 -Mode Worktree

.EXAMPLE
    # 装成 pre-commit 钩子（仓库目前没有钩子约定，需要各自装一次）
    'powershell -NoProfile -File scripts/Test-DbOptionDrift.ps1' |
        Set-Content .git/hooks/pre-commit
#>
[CmdletBinding()]
param(
    [ValidateSet("Staged", "Worktree")]
    [string]$Mode = "Staged",
    [string[]]$Knob = @("mdb_name", "project_name", "v_port", "http_api_addr", "manual_db_nums"),
    [switch]$AllowKnobDrift
)

$ErrorActionPreference = "Stop"
$ConfigPath = "DbOption.toml"

# git show 的输出要按 UTF-8 解码：文件里的注释是中文，值也可能是。
$previousEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = [Text.Encoding]::UTF8

function Get-TomlPairs {
    param([string[]]$Lines)

    $pairs = [ordered]@{}
    foreach ($line in $Lines) {
        # 注释掉的键（`# manual_db_nums = ...`）不参与比对：它们本来就不生效。
        if ($line -notmatch '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=(.*)$') { continue }

        $key = $Matches[1]
        $value = ""
        $inQuote = $false
        foreach ($char in $Matches[2].ToCharArray()) {
            if ($char -eq '"') { $inQuote = -not $inQuote }
            if ($char -eq '#' -and -not $inQuote) { break }
            $value += $char
        }
        $pairs[$key] = $value.Trim()
    }
    return $pairs
}

try {
    $baseline = @(git show "HEAD:$ConfigPath" 2>$null)
    if ($LASTEXITCODE -ne 0) {
        Write-Host "$ConfigPath 尚未入库，无基线可比，跳过。"
        exit 0
    }

    if ($Mode -eq "Staged") {
        $candidate = @(git show ":$ConfigPath" 2>$null)
        if ($LASTEXITCODE -ne 0) {
            Write-Host "$ConfigPath 不在暂存区，跳过。"
            exit 0
        }
    } else {
        $candidate = @(Get-Content $ConfigPath -Encoding utf8)
    }

    $baselinePairs = Get-TomlPairs -Lines $baseline
    $candidatePairs = Get-TomlPairs -Lines $candidate

    $allKeys = @($baselinePairs.Keys) + @($candidatePairs.Keys) | Select-Object -Unique
    $changed = foreach ($key in $allKeys) {
        $before = if ($baselinePairs.Contains($key)) { $baselinePairs[$key] } else { "(缺)" }
        $after = if ($candidatePairs.Contains($key)) { $candidatePairs[$key] } else { "(缺)" }
        if ($before -ne $after) {
            [pscustomobject]@{
                key      = $key
                baseline = $before
                current  = $after
                knob     = $Knob -contains $key
            }
        }
    }

    if (-not $changed) {
        Write-Host "$ConfigPath 与 HEAD 一致（$Mode）。"
        exit 0
    }

    $changed | Format-Table key, knob, baseline, current -AutoSize | Out-String -Width 160 | Write-Host

    $knobDrift = @($changed | Where-Object knob)
    if (-not $knobDrift) {
        Write-Host "改动均为共享配置，未触及本机旋钮，放行。"
        exit 0
    }
    if ($AllowKnobDrift) {
        Write-Host "本机旋钮有改动，但已指定 -AllowKnobDrift，放行。"
        exit 0
    }

    # 当钩子跑时 throw 会把 PowerShell 的堆栈一并糊上来，把真正要看的那几行淹掉。
    $detail = $knobDrift | ForEach-Object { " - $($_.key): $($_.baseline) -> $($_.current)" }
    [Console]::Error.WriteLine(@"
$ConfigPath 里的本机台架旋钮被改动，这些值不该进提交：
$($detail -join "`n")
把它们改回基线值再提交；基线值确实要变（例如团队真的迁了端口）就加 -AllowKnobDrift。
"@)
    exit 1
} finally {
    [Console]::OutputEncoding = $previousEncoding
}
