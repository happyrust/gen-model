# scripts/mission-gate.ps1 —— 里程碑门禁（scrutiny 逐字执行）
#
# 为什么需要这个脚本而不是把一长串命令塞进 services.yaml：
#   1. L2 必须用独立 CARGO_TARGET_DIR（否则 os error 5，退出码同为 101，会被误读成 L2 红）；
#   2. L2 必须 --test-threads=1（整个档是一个 #[test]）；
#   3. 预算 check 在 0 样本时按设计返回 1，门里要把它当「无样本」而不是「超预算」；
#   4. 分项结论：某一层红要能看出是哪一层，而不是一个笼统的非 0。
#
# 用法：
#   pwsh -NoProfile -File scripts/mission-gate.ps1            # 全档（约 24 min）
#   pwsh -NoProfile -File scripts/mission-gate.ps1 -OnlyL2    # 只跑 L2

[CmdletBinding()]
param(
    [switch]$OnlyL2,
    [string]$ShadowRoot = '',
    # L2 的独立 CARGO_TARGET_DIR，只能填实测可用的目录（与 scripts/replay/Run-L2Replay.ps1 的 -TargetDir 同一个默认值）：
    #   - 不能用 D:\Rust\target：本机常驻的 aios-database.exe 就是从它的 debug\aios-database.exe 起的，Windows 锁着运行中的
    #     exe，cargo 重链接时 "failed to remove file ... (os error 5)"，退出码 101 与 L2 真红一模一样，门禁会把环境错误
    #     读成 L2 红；
    #   - 不能随手新建目录：新目录要从零编译整棵依赖树，实测 D:\Rust\target-l2-verify8 从零编译崩溃 0xc0000409；旧默认值
    #     D:\Rust\target-l2 压根不存在。D:\Rust\target-l2-m1cwd 已实测可用（整档 1476 s 跑通）。要换目录先手工验证通过，
    #     再把这里与 Run-L2Replay.ps1 的默认值一起改。
    #   判绿三条见 docs/2026-10-28_l2-isolated-cargo-target.md：退出码 0 + 整档墙钟 > 1000 s + 日志无 "skip increment_replay:"。
    [string]$L2TargetDir = 'D:\Rust\target-l2-m1cwd'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$features = 'ws,gen_model,manifold,project_hd'
$featuresHttp = 'ws,gen_model,manifold,project_hd,http_api'
$results = [System.Collections.Generic.List[object]]::new()

function Invoke-Gate {
    param(
        [string]$Name,
        [scriptblock]$Body,
        [switch]$Required
    )
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host ''
    Write-Host "===== [$Name] =====" -ForegroundColor Cyan
    $code = 0
    try {
        & $Body
        $code = if ($null -ne $LASTEXITCODE) { $LASTEXITCODE } else { 0 }
    } catch {
        Write-Host "  EXCEPTION: $_" -ForegroundColor Red
        $code = 1
    }
    $sw.Stop()
    $ok = ($code -eq 0)
    $results.Add([pscustomobject]@{
        Name       = $Name
        ExitCode   = $code
        Seconds    = [math]::Round($sw.Elapsed.TotalSeconds, 1)
        Status     = if ($ok) { 'PASS' } else { 'FAIL' }
        Required   = [bool]$Required
    })
    if (-not $ok) {
        Write-Host "  [$Name] FAIL exit=$code elapsed=$([math]::Round($sw.Elapsed.TotalSeconds,1))s" -ForegroundColor Red
    } else {
        Write-Host "  [$Name] PASS elapsed=$([math]::Round($sw.Elapsed.TotalSeconds,1))s" -ForegroundColor Green
    }
}

# ---------------------------------------------------------------- L2
function Resolve-ShadowRoot {
    if ($ShadowRoot) { return $ShadowRoot }
    # 每个 worker 独占一个影子根，绝不共用（实测有外部会话删改过共享影子根）
    return Join-Path $repo "_runs\gate-shadowroot"
}

if ($OnlyL2) {
    Invoke-Gate -Name 'L2-replay' -Required {
        $root = Resolve-ShadowRoot
        if (-not (Test-Path $root)) {
            Write-Host "  影子根不存在，先造：$root"
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'replay\New-ShadowProjectRoot.ps1') -Target $root
        }
        $env:AIOS_REPLAY_PROJECT_ROOT = $root
        $env:CARGO_TARGET_DIR = $L2TargetDir
        try {
            & cargo test --locked --test increment_replay --features $features -- --test-threads=1 --nocapture
        } finally {
            Remove-Item Env:\AIOS_REPLAY_PROJECT_ROOT -ErrorAction SilentlyContinue
            Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        }
    }
} else {
    Invoke-Gate -Name 'typecheck' -Required {
        & cargo check --locked --features $featuresHttp
    }

    Invoke-Gate -Name 'lint' -Required {
        # 仓库无 rustfmt.toml，CI 与 githooks 都不跑 fmt —— 只跑 clippy。
        # --no-deps 必需：vendor 的 e3d-model / pipe-model 带 1 / 13 条既有警告，
        # 不加 --no-deps 时 -D warnings 会把这些升级成错误，门禁永久红（已实测）。
        # aios-database 自身 lib 实测 0 条警告，故 -D warnings 对本包成立。
        & cargo clippy --locked --features $featuresHttp --no-deps --lib -- -D warnings
    }

    Invoke-Gate -Name 'L0-lib' -Required {
        & cargo test --locked --lib --features $features
    }

    Invoke-Gate -Name 'L1-offline' -Required {
        & cargo test --locked `
            --test pipe26229_increment `
            --test equi_increment `
            --test hvac_bran_increment `
            --test ctray_increment `
            --test model_impact `
            --test db8000_two_delete_fixture `
            --test db_session_fixture_selfcheck `
            --test db8000_session_pairs `
            --test baseline_stream_8000 `
            --test read_through_rows_parity `
            --test increment_cases_registry `
            --features $features
    }

    Invoke-Gate -Name 'L2-replay' -Required {
        $root = Resolve-ShadowRoot
        if (-not (Test-Path $root)) {
            Write-Host "  影子根不存在，先造：$root"
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'replay\New-ShadowProjectRoot.ps1') -Target $root
        }
        $env:AIOS_REPLAY_PROJECT_ROOT = $root
        $env:CARGO_TARGET_DIR = $L2TargetDir
        try {
            & cargo test --locked --test increment_replay --features $features -- --test-threads=1 --nocapture
        } finally {
            Remove-Item Env:\AIOS_REPLAY_PROJECT_ROOT -ErrorAction SilentlyContinue
            Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        }
    }

    Invoke-Gate -Name 'budget' {
        # 0 样本时 increment_budget.py 按设计返回 1（不是超预算）。
        # 这里把「无样本」与「超预算」区分开：无样本记 SKIP 不算红。
        $out = & python scripts/increment_budget.py check 2>&1
        $code = $LASTEXITCODE
        $out | ForEach-Object { Write-Host "  $_" }
        if ($code -ne 0 -and ($out -join "`n") -match 'no samples|0 samples|未找到样本') {
            Write-Host '  [budget] SKIP：无样本（0 样本时门按设计返回 1）' -ForegroundColor Yellow
            $script:budgetSkipped = $true
            $global:LASTEXITCODE = 0
        } else {
            $global:LASTEXITCODE = $code
        }
    }
}

# ---------------------------------------------------------------- 汇总
Write-Host ''
Write-Host '===== 门禁汇总 =====' -ForegroundColor Cyan
$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

$failed = $results | Where-Object { $_.Status -eq 'FAIL' -and $_.Required }
if ($failed) {
    Write-Host "失败项：$($failed.Name -join ', ')" -ForegroundColor Red
    exit 1
}

Write-Host '全部必需项通过。' -ForegroundColor Green
exit 0
