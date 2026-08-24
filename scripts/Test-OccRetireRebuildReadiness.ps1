[CmdletBinding()]
param(
    [string]$Endpoint = "http://127.0.0.1:8009/sql",
    [string]$Namespace = "1516",
    [string]$Database = "AvevaMarineSample",
    [string]$MeshDir,
    [string]$OutJson,
    [switch]$IncludeIds,
    [switch]$RequireReady
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path $PSScriptRoot -Parent
$invoke = Join-Path $PSScriptRoot "Invoke-Surreal8009.ps1"
$variants = @(
    "PrimLCylinder",
    "PrimSphere",
    "PrimLSnout",
    "PrimDish",
    "PrimCTorus",
    "PrimRTorus"
)

function Invoke-AuditSql([string]$Sql) {
    $response = & $invoke -Endpoint $Endpoint -Namespace $Namespace -Database $Database -Sql $Sql |
        ConvertFrom-Json
    foreach ($statement in @($response)) {
        if ($statement.status -ne "OK") {
            throw "Surreal statement failed: $($statement | ConvertTo-Json -Compress -Depth 8)"
        }
    }
    return @($response)
}

function First-Result([string]$Sql) {
    $statements = @(Invoke-AuditSql $Sql)
    if ($statements.Count -eq 0) { return $null }
    $rows = @($statements[0].result)
    if ($rows.Count -eq 0) { return $null }
    return $rows[0]
}

$variantRows = @()
$reusableIds = New-Object System.Collections.Generic.List[string]
foreach ($variant in $variants) {
    $summary = First-Result @"
SELECT count() AS total,
       count(param.$variant.mesh_caliber) AS with_caliber,
       count(IF param.$variant.mesh_caliber = NONE THEN id ELSE NONE END) AS missing_caliber,
       count(IF bad = true THEN id ELSE NONE END) AS bad
FROM inst_geo WHERE param.$variant != NONE GROUP ALL;
"@
    if ($null -eq $summary) {
        $summary = [pscustomobject]@{ total = 0; with_caliber = 0; missing_caliber = 0; bad = 0 }
    }
    $referencedMissing = First-Result @"
SELECT count() AS count FROM geo_relate
WHERE out.param.$variant != NONE AND out.param.$variant.mesh_caliber = NONE GROUP ALL;
"@
    $idsResponse = @(Invoke-AuditSql @"
SELECT VALUE <string>record::id(id) FROM inst_geo
WHERE param.$variant != NONE AND meshed = true;
"@)
    if ($idsResponse.Count -gt 0) {
        foreach ($id in @($idsResponse[0].result)) {
            if ($null -ne $id) { $reusableIds.Add([string]$id) }
        }
    }
    $variantRows += [pscustomobject]@{
        variant = $variant
        total = [int64]$summary.total
        with_caliber = [int64]$summary.with_caliber
        missing_caliber = [int64]$summary.missing_caliber
        referenced_missing_caliber = if ($null -eq $referencedMissing) { 0 } else { [int64]$referencedMissing.count }
        bad = [int64]$summary.bad
    }
}

$queueResponse = @(Invoke-AuditSql @"
SELECT action, status, count() AS count FROM model_update_pending
WHERE status != 'done' GROUP BY action, status ORDER BY action, status;
"@)
$pendingRows = if ($queueResponse.Count -eq 0) { @() } else { @($queueResponse[0].result) }

$orphanResponse = @(Invoke-AuditSql @"
SELECT VALUE <string>record::id(id) FROM inst_geo
WHERE (param.PrimLCylinder != NONE OR param.PrimSphere != NONE OR
       param.PrimLSnout != NONE OR param.PrimDish != NONE OR
       param.PrimCTorus != NONE OR param.PrimRTorus != NONE)
  AND array::len(<-geo_relate) = 0;
"@)
$orphanIds = if ($orphanResponse.Count -eq 0) { @() } else { @($orphanResponse[0].result) }

$missingMeshFiles = @()
$resolvedMeshDir = $null
if ($MeshDir) {
    $candidate = $MeshDir
    if (-not [IO.Path]::IsPathRooted($candidate)) { $candidate = Join-Path $repoRoot $candidate }
    $resolvedMeshDir = [IO.Path]::GetFullPath($candidate)
    foreach ($id in @($reusableIds | Sort-Object -Unique)) {
        $path = Join-Path $resolvedMeshDir "$id.mesh"
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { $missingMeshFiles += $id }
    }
}

$missingCaliber = [int64](($variantRows | Measure-Object missing_caliber -Sum).Sum)
$referencedMissing = [int64](($variantRows | Measure-Object referenced_missing_caliber -Sum).Sum)
$badReusable = [int64](($variantRows | Measure-Object bad -Sum).Sum)
$ready = $missingCaliber -eq 0 -and
    $referencedMissing -eq 0 -and
    $badReusable -eq 0 -and
    @($pendingRows).Count -eq 0 -and
    @($orphanIds).Count -eq 0 -and
    @($missingMeshFiles).Count -eq 0

$reportedOrphans = if ($IncludeIds) { @($orphanIds) } else { @($orphanIds | Select-Object -First 20) }
$reportedMissingMeshes = if ($IncludeIds) { @($missingMeshFiles) } else { @($missingMeshFiles | Select-Object -First 20) }
$report = [ordered]@{
    schema = "occ-retire-rebuild-readiness-v1"
    generated_at = (Get-Date).ToUniversalTime().ToString("o")
    endpoint = $Endpoint
    namespace = $Namespace
    database = $Database
    mesh_dir = $resolvedMeshDir
    variants = $variantRows
    totals = [ordered]@{
        missing_caliber = $missingCaliber
        referenced_missing_caliber = $referencedMissing
        bad_reusable = $badReusable
        pending_queue_groups = @($pendingRows).Count
        orphan_reusable_ids = @($orphanIds).Count
        missing_mesh_files = @($missingMeshFiles).Count
    }
    pending_queue = @($pendingRows)
    orphan_reusable_ids = @($reportedOrphans)
    orphan_ids_truncated = (-not $IncludeIds -and @($orphanIds).Count -gt $reportedOrphans.Count)
    missing_mesh_ids = @($reportedMissingMeshes)
    missing_mesh_ids_truncated = (-not $IncludeIds -and @($missingMeshFiles).Count -gt $reportedMissingMeshes.Count)
    ready = $ready
}

$json = $report | ConvertTo-Json -Depth 8
if ($OutJson) {
    $outPath = $OutJson
    if (-not [IO.Path]::IsPathRooted($outPath)) { $outPath = Join-Path $repoRoot $outPath }
    $parent = Split-Path $outPath -Parent
    if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
    $json | Set-Content -LiteralPath $outPath -Encoding UTF8
}
$json

if ($RequireReady -and -not $ready) { exit 1 }
