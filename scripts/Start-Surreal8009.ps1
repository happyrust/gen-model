<#
.SYNOPSIS
    启动本项目用的 SurrealDB（8009），并挡住版本不匹配的二进制。

.DESCRIPTION
    本仓库的客户端 SDK 是 fork 的 SurrealDB **2.1.4**
    （Cargo.lock: git+https://gitee.com/happydpc/surrealdb.git#45013fc9…）。
    服务端必须是同一条线的 2.1.x，否则会撞上两个都很难自己看明白的错误：

    1. 用 3.x 打开 `.surreal/ams-8009`（存储版本 2）：
         The data stored on disk is out-of-date with this version
         (Expected: 3, Actual: 2)
       它的字面提示是「按升级指南迁移」——**别照做**，那是不可逆的，
       而你需要的只是换一个二进制。

    2. 就算绕开数据目录（例如 `memory`），3.x 服务端也和 2.1.4 客户端握不上手：
         WebSocket protocol error: SubProtocol error: Server sent no subprotocol

    偏偏 `PATH` 上很可能是 `cargo install` 装的 3.x（`~/.cargo/bin/surreal.exe`），
    而对的那个就躺在仓库里（`bin/surreal.exe`）。所以这个脚本存在的唯一理由是：
    **别让人再去手敲 `surreal start`。**

.PARAMETER Memory
    用一次性的内存后端起（跑 live 测试用），不碰 .surreal/ams-8009 里的真实数据。

.PARAMETER Datastore
    数据后端。默认 `rocksdb:.surreal/ams-8009`；`-Memory` 会覆盖它。

.PARAMETER SurrealExe
    显式指定服务端二进制。默认按 `bin/surreal.exe` → `AIOS_SURREAL_EXE` 环境变量
    的顺序找；**不回退到 PATH**（PATH 上那个正是要防的）。

.EXAMPLE
    # 日常：连项目真实数据
    ./scripts/Start-Surreal8009.ps1

.EXAMPLE
    # 跑 live 测试：空库即可，不碰真实数据
    ./scripts/Start-Surreal8009.ps1 -Memory

.NOTES
    连接参数与 DbOption.toml 对齐：v_ip=localhost / v_port=8009 /
    surreal_ns=1516 / project_name=AvevaMarineSample。
    改端口请同时改 DbOption.toml，否则程序仍然连 8009。
#>
[CmdletBinding()]
param(
    [switch]$Memory,
    [string]$Datastore = "rocksdb:.surreal/ams-8009",
    [string]$SurrealExe,
    [string]$Bind = "127.0.0.1:8009",
    [string]$User = "root",
    [string]$Password = "root"
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

function Resolve-SurrealExe {
    param([string]$Explicit, [string]$RepoRoot)

    $candidates = @()
    if ($Explicit) { $candidates += $Explicit }
    if ($env:AIOS_SURREAL_EXE) { $candidates += $env:AIOS_SURREAL_EXE }
    $candidates += (Join-Path $RepoRoot "bin/surreal.exe")

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    # 刻意不回退到 PATH：那上面大概率是 cargo install 装的 3.x，
    # 而它的失败方式会把人引向「迁移数据」。宁可在这里报错说清楚。
    throw @"
找不到可用的 SurrealDB 服务端二进制。

查找顺序：-SurrealExe 参数 → `$env:AIOS_SURREAL_EXE → bin/surreal.exe

注意本脚本**不会**回退到 PATH：PATH 上通常是 cargo install 装的 3.x，
用它会撞上存储版本不兼容与 WS 子协议不兼容两个坑。
请把 2.1.x 的二进制放到 bin/surreal.exe，或用 -SurrealExe 指定。
"@
}

# Cargo.lock 里锁的那个 rev，用来和二进制自报的版本对账。
function Get-LockedSurrealRev {
    param([string]$RepoRoot)

    $lock = Join-Path $RepoRoot "Cargo.lock"
    if (-not (Test-Path -LiteralPath $lock)) { return $null }
    $line = Select-String -LiteralPath $lock -Pattern 'surrealdb\.git#([0-9a-f]{7,40})' |
        Select-Object -First 1
    if (-not $line) { return $null }
    return $line.Matches[0].Groups[1].Value
}

$exe = Resolve-SurrealExe -Explicit $SurrealExe -RepoRoot $repoRoot
$version = (& $exe version) -join " "

if ($version -notmatch '(?<major>\d+)\.(?<minor>\d+)\.(?<patch>\d+)') {
    throw "无法从 '$exe' 解析版本号，实际输出：$version"
}
$major = [int]$Matches['major']
if ($major -ne 2) {
    throw @"
服务端版本不匹配：$exe 是 $version，但本仓库的客户端 SDK 是 2.1.4
（Cargo.lock: gitee.com/happydpc/surrealdb.git）。

用它会撞上两个坑：
  * 打开 .surreal/ams-8009（存储版本 2）会要求不可逆迁移；
  * 即便用 memory 后端，也会报 'Server sent no subprotocol'。

请改用 2.1.x 的二进制（仓库里应有 bin/surreal.exe），或用 -SurrealExe 指定。
"@
}

# 版本大类对上了，再核一次 git rev——fork 的 2.1.4 与上游 2.1.4 不是一回事。
$lockedRev = Get-LockedSurrealRev -RepoRoot $repoRoot
if ($lockedRev) {
    $shortLocked = $lockedRev.Substring(0, 8)
    if ($version -notmatch [regex]::Escape($shortLocked)) {
        Write-Warning @"
服务端自报 '$version'，其中没有 Cargo.lock 锁定的 rev $shortLocked。
大版本对得上，所以继续启动；但如果遇到解码或协议层的怪问题，先怀疑这里。
"@
    }
}

if ($Memory) { $Datastore = "memory" }

Push-Location $repoRoot
try {
    Write-Host "SurrealDB : $exe"
    Write-Host "版本      : $version"
    Write-Host "监听      : $Bind"
    Write-Host "数据后端  : $Datastore"
    if ($Memory) {
        Write-Host "（内存后端：进程退出即全部丢弃，不碰 .surreal/ams-8009）" -ForegroundColor Yellow
    }
    Write-Host ""
    & $exe start --user $User --pass $Password --bind $Bind $Datastore
}
finally {
    Pop-Location
}
