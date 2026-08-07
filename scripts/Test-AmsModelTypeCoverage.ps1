[CmdletBinding()]
param(
    [string]$Endpoint = 'ws://127.0.0.1:8009',
    [string]$Namespace = '1516',
    [string]$Database = 'AvevaMarineSample',
    [switch]$RequireVerified
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content (Join-Path $repo 'scripts/e3d/ams_model_type_cases.json') -Raw |
    ConvertFrom-Json
$sql = 'RETURN array::sort(array::distinct(SELECT VALUE in.noun FROM inst_relate));'
# Windows PowerShell 5.1 给管道进原生程序的文本带 UTF-8 BOM，SurrealQL 解析直接失败
# （实测 2026-08-07）。改走 cmd 的 stdin 重定向：文件字节原样进程序，两代 PowerShell 通吃。
$sqlFile = Join-Path ([System.IO.Path]::GetTempPath()) 'ams-model-noun-query.sql'
[System.IO.File]::WriteAllText($sqlFile, $sql, [System.Text.Encoding]::ASCII)
$surreal = Join-Path $repo 'bin\surreal.exe'
$raw = & cmd /c "`"$surreal`" sql -e $Endpoint -u root -p root --ns $Namespace --db $Database --json --hide-welcome < `"$sqlFile`""
# 输出是 [[noun, ...]]。ConvertFrom-Json 的数组摊开行为两代不同（7 在管道里摊开外层、
# 5.1 不摊），先拼整段再按形状归一成字符串数组。
$parsed = ($raw -join "`n") | ConvertFrom-Json
$actual = @($parsed)
if ($actual.Count -eq 1 -and $actual[0] -is [System.Array]) { $actual = $actual[0] }
if ($LASTEXITCODE) { throw 'Failed to query AMS model nouns' }

# coverage 语义：verified = 增量验证跑绿；pending = 已注册待验证；
# no_geometry = 探针证实该 noun 在 AMS 目录下不产出独立几何（不算欠账，但若它日后
# 出现在 inst_relate 里就是矛盾，说明判定过时，必须报错重审）。
# stale 只约束 verified 行：pending 行的实例可能来自探针的按需生成，金基线恢复
# （ADR-018）会把它们抹掉，等用例跑绿并铸进基线才要求常驻（实测 2026-08-07：
# 一次恢复把 7999 的探针产物 57 条清回 2 条）。
$noGeometry = $manifest | Where-Object coverage -eq 'no_geometry'
$expected = $manifest | Where-Object coverage -ne 'no_geometry'
$verified = $expected | Where-Object coverage -eq 'verified'

$duplicate = $manifest | Group-Object noun | Where-Object Count -ne 1 | ForEach-Object Name
$missing = $actual | Where-Object { $_ -notin $manifest.noun }
$stale = $verified.noun | Where-Object { $_ -notin $actual }
$contradicted = $noGeometry.noun | Where-Object { $_ -in $actual }
$pending = $expected | Where-Object coverage -ne 'verified' | ForEach-Object noun

Write-Host "AMS model nouns: actual=$($actual.Count) manifest=$($manifest.Count) verified=$($verified.Count) pending=$($pending.Count) no_geometry=$($noGeometry.Count)"
if ($missing) { Write-Host "Missing: $($missing -join ', ')" }
if ($stale) { Write-Host "Stale: $($stale -join ', ')" }
if ($duplicate) { Write-Host "Duplicate: $($duplicate -join ', ')" }
if ($contradicted) { Write-Host "Contradicted no_geometry (now has instances): $($contradicted -join ', ')" }
if ($pending) { Write-Host "Pending: $($pending -join ', ')" }

if ($missing -or $stale -or $duplicate -or $contradicted -or ($RequireVerified -and $pending)) { exit 1 }
