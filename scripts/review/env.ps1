# 校审验收的 worktree 隔离 cargo 环境；默认目录名随当前 worktree 变化。
#
# 为什么：本机 User 级环境变量 CARGO_TARGET_DIR=D:\Rust\target 被所有 gen-model 系 worktree 共用，
# 多人并行时 cargo 会把别的 worktree 刚编出的二进制判成 fresh（0.7s Finished），跑出
# `0 passed; N filtered out` 或模块缺失的假结果（2026-09-13 共享记忆 mem-189）。
# 环境变量优先级高于 .cargo/config.toml 的 build.target-dir，所以只能在会话里覆盖，不能靠配置文件。
#
# 用法：每个新的 PowerShell 会话里，在跑任何 cargo 命令之前先执行
#     . .\scripts\review\env.ps1
# （注意前面的点号，是 dot-source，不是新进程。）
#
# 磁盘：D: 剩余空间已不多，独立 target dir 首次全量编译约需 12–15 GB（debug/test），
# 因此本 worktree 不要 --release（release 二进制再吃 8–12 GB）；起服务用 debug 二进制即可。

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$worktreeName = Split-Path $repoRoot -Leaf
$defaultTarget = if (Test-Path 'D:\Rust') { "D:\Rust\target-review-$worktreeName" } else { Join-Path $repoRoot '.target-review' }
$env:CARGO_TARGET_DIR = if ($env:REVIEW_CARGO_TARGET_DIR) { $env:REVIEW_CARGO_TARGET_DIR } else { $defaultTarget }

$freeGb = (Get-PSDrive -Name D).Free / 1GB
if ($freeGb -lt 15) {
    Write-Warning ("D: 仅剩 {0:N1} GB（阈值 15 GB）。先清理再编译，磁盘写满会拖垮整台机器上所有人的构建。" -f $freeGb)
}
Write-Host ("CARGO_TARGET_DIR={0}  (D: free {1:N1} GB)" -f $env:CARGO_TARGET_DIR, $freeGb)
