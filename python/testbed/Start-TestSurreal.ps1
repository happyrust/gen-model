<#
.SYNOPSIS
    启动 Python 测试沙箱专用的 SurrealDB（8019，数据目录 python/testbed/.surreal/pytest-ams）。

.DESCRIPTION
    复用 scripts/Start-Surreal8009.ps1 的二进制查找与版本守卫（必须是仓库自带的
    fork 2.1.x，绝不回退到 PATH 上的 3.x），只是换端口、换数据目录：
      监听      127.0.0.1:8019   （正式库在 8009，互不影响，可同时跑）
      数据后端  rocksdb:python/testbed/.surreal/pytest-ams
    与 python/testbed/DbOption-pytest.toml 的 v_port=8019 对齐；改端口两边一起改。

.PARAMETER Memory
    用一次性内存后端（不落盘，进程退出全部丢弃）。

.EXAMPLE
    ./python/testbed/Start-TestSurreal.ps1          # 常驻前台，Ctrl+C 停
#>
[CmdletBinding()]
param(
    [switch]$Memory,
    [string]$Bind = "127.0.0.1:8019",
    [string]$Datastore = "rocksdb:python/testbed/.surreal/pytest-ams"
)

$ErrorActionPreference = "Stop"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
& (Join-Path $repoRoot "scripts/Start-Surreal8009.ps1") -Bind $Bind -Datastore $Datastore -Memory:$Memory
