[CmdletBinding()]
param(
    [string]$PlantUiRoot = 'D:\work\plant-code\old\plant-ui-ams8000-convergence',
    [string]$MeshDir = '',
    [int]$Port = 8000,
    [string]$SurrealEndpoint = '',
    [string]$EvidenceDir = 'output\ams8000-display-20260825'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$siteRoot = Join-Path $repo '.sites\8000'
if ([string]::IsNullOrWhiteSpace($MeshDir)) {
    $MeshDir = Join-Path $siteRoot 'assets\meshes'
}
if ([string]::IsNullOrWhiteSpace($SurrealEndpoint)) {
    $SurrealEndpoint = "http://127.0.0.1:$Port/sql"
}
$MeshDir = [System.IO.Path]::GetFullPath($MeshDir)
if (-not (Test-Path -LiteralPath $MeshDir -PathType Container)) {
    throw "AMS8000 mesh directory does not exist: $MeshDir"
}
$invoke = Join-Path $PSScriptRoot 'Invoke-Surreal8009.ps1'
$evidence = [System.IO.Path]::GetFullPath((Join-Path $repo $EvidenceDir))
New-Item -ItemType Directory -Force -Path $evidence | Out-Null

$sql = @'
RETURN fn::gen_root_progress(8000);
RETURN (SELECT count() AS c FROM pe WHERE dbnum = 8000 GROUP ALL)[0].c;
RETURN (SELECT count() AS c FROM inst_relate WHERE dbnum = 8000 GROUP ALL)[0].c;
RETURN (SELECT count() AS c FROM inst_relate WHERE dbnum = 8000 AND (aabb = NONE OR world_trans = NONE) GROUP ALL)[0].c;
RETURN SELECT VALUE type::thing('pe', record::id(id)).noun FROM inst_relate WHERE dbnum = 8000 AND (aabb = NONE OR world_trans = NONE);
RETURN (SELECT count() AS c FROM inst_relate WHERE dbnum = 8000 AND aabb != NONE AND world_trans != NONE GROUP ALL)[0].c;
RETURN (SELECT count() AS c FROM tubi_relate WHERE in.dbnum = 8000 AND aabb.d != NONE AND leave.id != NONE GROUP ALL)[0].c;
RETURN (SELECT count() AS c FROM tubi_relate WHERE in.dbnum = 8000 AND (aabb.d = NONE OR leave.id = NONE) GROUP ALL)[0].c;
RETURN SELECT kind, target, targets, last_error FROM geom_error;
RETURN SELECT VALUE <string>record::id(id) FROM inst_geo WHERE bad = true;
RETURN (SELECT count() AS c FROM parse_error WHERE dbnum = 8000 GROUP ALL)[0].c;
RETURN SELECT VALUE name FROM pe WHERE dbnum = 8000 AND noun = 'SITE' ORDER BY name;
RETURN (SELECT count() AS c FROM pe WHERE dbnum = 8000 AND noun IN ['SITE', 'ZONE', 'PIPE', 'HVAC'] GROUP ALL)[0].c;
RETURN SELECT <int>record::id(id) AS ref0, count FROM dbnum_info_table WHERE dbnum = 8000;
'@

$raw = & $invoke -Endpoint $SurrealEndpoint -Sql $sql
if (-not $?) { throw 'AMS8000 census query failed' }
$responses = $raw | ConvertFrom-Json
$progress = $responses[0].result
$peCount = [int]$responses[1].result
$instCount = [int]$responses[2].result
$missingSpatial = if ($null -eq $responses[3].result) { 0 } else { [int]$responses[3].result }
$missingSpatialNouns = @($responses[4].result)
$visibleInstRows = $instCount - $missingSpatial
$visibleTubiRows = if ($null -eq $responses[6].result) { 0 } else { [int]$responses[6].result }
$missingTubiSpatial = if ($null -eq $responses[7].result) { 0 } else { [int]$responses[7].result }
$allErrors = @($responses[8].result)
$badGeoIds = @($responses[9].result | ForEach-Object { [string]$_ } | Sort-Object -Unique)
$parseErrorCount = if ($null -eq $responses[10].result) { 0 } else { [int]$responses[10].result }
$siteNames = @($responses[11].result | ForEach-Object { [string]$_ })
$containerCount = if ($null -eq $responses[12].result) { 0 } else { [int]$responses[12].result }
$ref0Counts = @($responses[13].result | Sort-Object ref0)
$expectedDisplayRows = $visibleInstRows + $visibleTubiRows
$ref0Prefixes = @($ref0Counts | ForEach-Object { [string]$_.ref0 })
$errors = @($allErrors | Where-Object {
    $row = $_
    @($ref0Prefixes | Where-Object {
        $prefix = $_
        ([string]$row.target).StartsWith("$prefix/") -or
        ([string]$row.target).StartsWith("$prefix`_") -or
        @($row.targets | Where-Object {
            ([string]$_).StartsWith("$prefix/") -or ([string]$_).StartsWith("$prefix`_")
        }).Count -gt 0
    }).Count -gt 0
})

if ($progress.total -le 0) { throw 'AMS8000 has no generation roots' }
if ($progress.todo -ne 0 -or $progress.refused -ne 0 -or $progress.done -ne $progress.total) {
    throw "generation roots are not terminal: $($progress | ConvertTo-Json -Compress)"
}
if ($progress.elements_done -ne $progress.elements_total) {
    throw "generation-root element coverage is incomplete: $($progress.elements_done)/$($progress.elements_total)"
}
if ($peCount -le 0 -or $instCount -le 0) { throw "empty AMS8000 data/model census: pe=$peCount inst=$instCount" }
if ($parseErrorCount -ne 0) { throw "AMS8000 data parse errors remain: $parseErrorCount" }
if ($siteNames.Count -le 0) { throw 'AMS8000 has no SITE rows to display' }
# gen_root coverage excludes the non-rendering hierarchy/container nouns.
# Every other PE row must be covered by exactly one terminal generation root.
if ($progress.elements_total + $containerCount -ne $peCount) {
    throw "PE coverage is incomplete: generated_elements=$($progress.elements_total) containers=$containerCount pe=$peCount"
}
if ($missingSpatialNouns.Count -ne $missingSpatial) {
    throw "missing-spatial census mismatch: count=$missingSpatial nouns=$($missingSpatialNouns.Count)"
}
$unexpectedSpatialNouns = @($missingSpatialNouns | Where-Object { $_ -ne 'ATTA' })
if ($unexpectedSpatialNouns.Count -ne 0) {
    throw "renderable inst_relate rows lack AABB/world transform: $($unexpectedSpatialNouns | ConvertTo-Json -Compress)"
}
if ($missingTubiSpatial -ne 0) {
    throw "TUBI rows are not display-ready: missing_aabb_or_leave=$missingTubiSpatial"
}

$unexpected = @($errors | Where-Object {
    $_.kind -notin @('cata_generation', 'mesh')
})
if ($unexpected.Count -ne 0) {
    throw "unexpected AMS8000 geom_error rows: $($unexpected | ConvertTo-Json -Depth 6 -Compress)"
}
$meshErrorIds = @($errors | Where-Object kind -eq 'mesh' | ForEach-Object { [string]$_.target } | Sort-Object -Unique)
$unreportedBadGeos = @($badGeoIds | Where-Object { $_ -notin $meshErrorIds })
$staleMeshErrors = @($meshErrorIds | Where-Object { $_ -notin $badGeoIds })
if ($unreportedBadGeos.Count -ne 0 -or $staleMeshErrors.Count -ne 0) {
    throw "bad mesh/geom_error mismatch: unreported=$($unreportedBadGeos -join ',') stale=$($staleMeshErrors -join ',')"
}

$siteResults = @()
$ref0Result = $null
Push-Location $PlantUiRoot
try {
    $env:PLANT_TEST_MESH_DIR = $MeshDir
    $env:PLANT_TEST_DB_PORT = [string]$Port
    $env:PLANT_TEST_DBNUM = '8000'
    $dbnumLog = Join-Path $evidence 'dbnum-8000-full-display.log'
    # Let cmd merge native stderr before PowerShell receives it. Windows
    # PowerShell 5 otherwise wraps ordinary rustc warnings in NativeCommandError.
    & $env:ComSpec /d /s /c 'cargo +nightly-2026-08-02 test -p plant-ui-data --test mesh_compat configured_dbnum_models_are_displayable -- --ignored --exact --nocapture 2>&1' |
        Tee-Object -FilePath $dbnumLog | Out-Host
    $dbnumExit = $LASTEXITCODE
    if ($dbnumExit -ne 0) { throw "dbnum 8000 full display compatibility failed with exit $dbnumExit" }
    $dbnumSummary = (Get-Content $dbnumLog | Select-String '个唯一 mesh 均可显示' | Select-Object -Last 1).Line
    if ($dbnumSummary -notmatch 'dbnum 8000: ([0-9]+) 个模型记录') {
        throw "dbnum 8000 display summary is missing its model count: $dbnumSummary"
    }
    $actualDisplayRows = [int]$Matches[1]
    if ($actualDisplayRows -ne $expectedDisplayRows) {
        throw "Plant UI model coverage mismatch: actual=$actualDisplayRows expected=$expectedDisplayRows (inst=$visibleInstRows tubi=$visibleTubiRows)"
    }
    $ref0Result = [pscustomobject]@{ dbnum = 8000; ref0s = $ref0Counts; expected_model_records = $expectedDisplayRows; actual_model_records = $actualDisplayRows; exit = $dbnumExit; summary = $dbnumSummary; log = $dbnumLog }
    foreach ($site in $siteNames) {
        $env:PLANT_TEST_SITE_NAME = $site
        $slug = $site.TrimStart('/')
        $log = Join-Path $evidence "site-$slug-full-display.log"
        & $env:ComSpec /d /s /c 'cargo +nightly-2026-08-02 test -p plant-ui-data --test mesh_compat configured_site_models_have_meshes -- --ignored --exact --nocapture 2>&1' |
            Tee-Object -FilePath $log | Out-Host
        $exit = $LASTEXITCODE
        if ($exit -ne 0) { throw "$site mesh/display compatibility failed with exit $exit" }
        $summary = (Get-Content $log | Select-String '个唯一 mesh 已反序列化并转换' | Select-Object -Last 1).Line
        $siteResults += [pscustomobject]@{ site = $site; exit = $exit; summary = $summary; log = $log }
    }
}
finally {
    Pop-Location
}

$report = [ordered]@{
    dbnum = 8000
    pe = $peCount
    hierarchy_containers = $containerCount
    parse_errors = $parseErrorCount
    inst_relate = $instCount
    visible_inst_rows = $visibleInstRows
    visible_tubi_rows = $visibleTubiRows
    expected_display_model_records = $expectedDisplayRows
    missing_aabb_or_transform = $missingSpatial
    missing_tubi_aabb_or_leave = $missingTubiSpatial
    non_renderable_atta = $missingSpatialNouns.Count
    generation = $progress
    reconciled_geom_errors = $errors.Count
    unexpected_geom_errors = $unexpected.Count
    bad_meshes_reported = $badGeoIds.Count
    mesh_directory = $MeshDir
    mesh_files_on_disk = @(Get-ChildItem -LiteralPath $MeshDir -Filter '*.mesh' -File).Count
    dbnum_models = $ref0Result
    sites = $siteResults
    completed_at = (Get-Date).ToString('o')
}
$reportPath = Join-Path $evidence 'ams8000-load-display-verification.json'
$report | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 $reportPath
Write-Host "AMS8000_LOAD_DISPLAY_VERIFY=PASS"
Write-Host "AMS8000_LOAD_DISPLAY_REPORT=$reportPath"
exit 0
