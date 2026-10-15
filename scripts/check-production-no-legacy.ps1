$ErrorActionPreference = 'Stop'

$production = @(
  'src/main.rs',
  'src/lib.rs',
  'src/data_interface',
  'src/web_service',
  'python/src'
)
$forbidden = 'fast_model::legacy|fast_model::mesh_generate|fast_model::gen_model|use\s+[^;]*gen_all_geos_data|process_meshes_update_db_deep\s*\(|process_meshes_by_dbnos\s*\('

# 不用 rg：GitHub 的 Windows runner 上没有它，这一步会在生产构建已经通过之后误红。
$files = foreach ($root in $production) {
  if (Test-Path -LiteralPath $root -PathType Leaf) {
    Get-Item -LiteralPath $root
  } elseif (Test-Path -LiteralPath $root -PathType Container) {
    Get-ChildItem -LiteralPath $root -Recurse -Filter *.rs -File
  } else {
    throw "production path missing: $root"
  }
}
$hits = @($files | Select-String -Pattern $forbidden)
if ($hits.Count -gt 0) {
  $hits | ForEach-Object { '{0}:{1}:{2}' -f $_.Path, $_.LineNumber, $_.Line.Trim() }
  throw 'production source references legacy model generation'
}
'PRODUCTION_LEGACY_SCAN_OK'
exit 0
