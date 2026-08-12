<#
.SYNOPSIS
    把真实 AVEVA 项目的库文件目录（*000）镜像到 Python 测试沙箱的项目副本。

.DESCRIPTION
    只镜像 dabacon 库文件所在的 *000 目录（解析/基线/生成只读这些），
    宏、图纸、图片等目录不拷。/MIR 语义：源里删了副本也删，保持一致。
    副本被 .gitignore 排除，坏了/旧了随时重跑本脚本重灌。

.PARAMETER Source
    真实 E3D 项目根（默认 D:/AVEVA/Projects/E3D3.1，与仓库根 DbOption.toml 一致）。
#>
[CmdletBinding()]
param(
    [string]$Source = "D:/AVEVA/Projects/E3D3.1"
)

$ErrorActionPreference = "Stop"
$dest = Join-Path $PSScriptRoot "projects"

$pairs = @(
    @{ Project = "AvevaMarineSample"; Dir = "ams000" },
    @{ Project = "AvevaCatalogue";    Dir = "acp000" },
    @{ Project = "ZDJ";               Dir = "ZDJ000" }
)

foreach ($p in $pairs) {
    $from = Join-Path $Source "$($p.Project)/$($p.Dir)"
    $to = Join-Path $dest "$($p.Project)/$($p.Dir)"
    if (-not (Test-Path -LiteralPath $from)) {
        throw "源目录不存在：$from"
    }
    Write-Host "镜像 $from → $to"
    robocopy $from $to /MIR /MT:8 /NFL /NDL /NP /R:1 /W:1 | Out-Null
    if ($LASTEXITCODE -ge 8) {
        throw "robocopy 失败（退出码 $LASTEXITCODE）：$from"
    }
}
Write-Host "完成。副本根：$dest"
