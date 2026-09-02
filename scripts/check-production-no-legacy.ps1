$ErrorActionPreference = 'Stop'

$production = @(
  'src/main.rs',
  'src/lib.rs',
  'src/data_interface',
  'src/web_service',
  'python/src'
)
$forbidden = 'fast_model::legacy|fast_model::occ_generate|fast_model::gen_model|use\s+[^;]*gen_all_geos_data|process_meshes_update_db_deep\s*\(|process_meshes_by_dbnos\s*\('
$hits = & rg -n --glob '*.rs' $forbidden @production
if ($LASTEXITCODE -eq 0) {
  $hits
  throw 'production source references legacy model generation'
}
if ($LASTEXITCODE -ne 1) {
  throw "rg failed with exit code $LASTEXITCODE"
}
'PRODUCTION_LEGACY_SCAN_OK'
