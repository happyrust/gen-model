[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param(
    [switch]$Execute,
    [string]$EvidenceDir = 'docs\evidence\2026-08-21-zero-ncyl-dead-letter-recovery\live',
    [string]$SourceFile = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001',
    [string]$SurrealEndpoint = 'ws://127.0.0.1:8009',
    [string]$ServiceExe = 'D:\work\plant-code\old\test-worklspace\bin\aios-database.exe'
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$evidence = Join-Path $repo $EvidenceDir
$surreal = Join-Path $repo 'bin\surreal.exe'
$sourceBackup = Join-Path $evidence 'ams7997_0001.before'
$export = Join-Path $evidence 'AvevaMarineSample-1516.surql'
function Get-Sha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        $sha = [Security.Cryptography.SHA256]::Create()
        try { ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
        finally { $sha.Dispose() }
    } finally { $stream.Dispose() }
}
$requiredBaseline = @($surreal, $sourceBackup, $export, (Join-Path $evidence 'paired-baseline.json'))
Write-Host "[DRY-RUN=$(-not $Execute)] stop all aios-database; restore $SourceFile; replace database 1516/AvevaMarineSample from $export"
if (-not $Execute) {
    $missing = @($requiredBaseline | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) })
    if ($missing.Count) { Write-Host "[DRY-RUN] 尚未生成的现场基线: $($missing -join ', ')" }
    return
}
foreach ($path in $requiredBaseline) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "回滚基线缺失: $path" }
}
$baseline = Get-Content -LiteralPath (Join-Path $evidence 'paired-baseline.json') -Raw | ConvertFrom-Json
if ((Get-Sha256 $sourceBackup) -ne $baseline.source.sha256) { throw '源文件备份哈希不匹配' }
if ((Get-Sha256 $export) -ne $baseline.export.sha256) { throw 'Surreal 导出哈希不匹配' }
if (-not $PSCmdlet.ShouldProcess('E3D source + 1516/AvevaMarineSample', 'restore paired baseline')) { return }
Get-Process -Name 'aios-database' -ErrorAction SilentlyContinue | Stop-Process -Force
Copy-Item -LiteralPath $sourceBackup -Destination $SourceFile -Force
if ((Get-Sha256 $SourceFile) -ne $baseline.source.sha256) { throw '源文件恢复后哈希不匹配' }
$pair = 'root:root'; $auth = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($pair))
$headers = @{ Authorization = "Basic $auth"; 'Surreal-NS' = '1516'; 'Surreal-DB' = 'AvevaMarineSample' }
Invoke-WebRequest -Method Post -Uri 'http://127.0.0.1:8009/sql' -Headers $headers -ContentType 'application/surrealql' -UseBasicParsing -Body 'REMOVE DATABASE AvevaMarineSample;' | Out-Null
& $surreal import --endpoint $SurrealEndpoint --username root --password root --namespace 1516 --database AvevaMarineSample $export
if ($LASTEXITCODE) { throw "Surreal 基线导入失败: $LASTEXITCODE" }
if (Test-Path -LiteralPath (Join-Path $evidence 'aios-database.before.exe')) {
    Copy-Item -LiteralPath (Join-Path $evidence 'aios-database.before.exe') -Destination $ServiceExe -Force
}
@{ restored_at = (Get-Date).ToUniversalTime().ToString('o'); source_sha256 = (Get-Sha256 $SourceFile); import_exit = $LASTEXITCODE } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidence 'rollback-verification.json') -Encoding utf8
