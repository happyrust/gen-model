$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$verification = Get-Content -LiteralPath (Join-Path $here 'atta-root\verification.json') -Raw |
    ConvertFrom-Json
$children = @($verification.parents[0].children | Where-Object noun -eq 'SECT')
if ($children.Count -lt 10) { throw "only $($children.Count) SECT candidates" }
$selected = for ($index = 0; $index -lt 10; $index++) {
    $children[[Math]::Floor($index * $children.Count / 10)].refno
}
$selected | Out-File -LiteralPath (Join-Path $here 'explicit-uda-refnos.txt') -Encoding ascii
Write-Output "CATA_UDA_SAMPLE candidates=$($children.Count) sampled=$($selected.Count)"
