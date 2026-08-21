[CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
param([switch]$Execute)

$ErrorActionPreference = 'Stop'
$invoke = Join-Path $PSScriptRoot 'Invoke-Surreal8009.ps1'
$read = "SELECT kind, target, noun, generation_root FROM geom_error:['primitive', '24381/38635'];"
$current = & powershell -NoProfile -ExecutionPolicy Bypass -File $invoke -Sql $read
Write-Host $current
Write-Host "[DRY-RUN=$(-not $Execute)] DELETE geom_error:['primitive', '24381/38635']"
if (-not $Execute) { return }
if ($current -notmatch '"kind":"primitive"' -or
    $current -notmatch '"target":"24381/38635"' -or
    $current -notmatch '"noun":"NCYL"' -or
    $current -notmatch '"generation_root":"24381/38436"') {
    throw '当前记录不再匹配本次回填基线，停止删除'
}
if ($PSCmdlet.ShouldProcess("geom_error:['primitive','24381/38635']", 'delete exact backfill')) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $invoke -Sql "DELETE geom_error:['primitive', '24381/38635']; SELECT * FROM geom_error:['primitive', '24381/38635'];"
    if ($LASTEXITCODE) { throw "回滚查询失败: $LASTEXITCODE" }
}
