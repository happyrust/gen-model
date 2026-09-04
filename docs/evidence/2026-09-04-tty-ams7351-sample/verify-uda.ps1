param(
    [string]$Descriptor = 'D:\Rust\target\release\e3d-descriptor.exe'
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$reports = Join-Path $here 'explicit-uda-final-reports'
New-Item -ItemType Directory -Force -Path $reports | Out-Null
$db = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7351_0001'
$schema = 'D:\work\plant-code\old\vendor\e3d-io\schema\e3d31'
$dictionaries = @(
    'D:\AVEVA\Projects\E3D3.1\SCB\scb000\scb6002_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5100_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7043_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7006_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7323_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5101_0001'
) -join ';'

$results = foreach ($log in Get-ChildItem -LiteralPath (Join-Path $here 'explicit-uda-final-logs') -File -Filter '*.log' | Sort-Object Name) {
    $refno = $log.BaseName -replace '-', '/'
    $report = Join-Path $reports ($log.BaseName + '.json')
    & $Descriptor extract-complete --db $db --template-type CATA --schema $schema --uda on `
        --dictionary-db-list $dictionaries --refno $refno --tty-log $log.FullName |
        Out-File -LiteralPath $report -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "extract-complete failed for $refno" }
    $json = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    $explicit = @($json.attributes | Where-Object {
        $_.name -like ':*' -and $_.source -eq 'binary_explicit'
    })
    [pscustomobject]@{
        refno = $refno
        tty_rows = $json.tty_row_count
        mismatch = [int]$json.comparison_counts.mismatch
        deferred = [int]$json.deferred_uda_row_count
        omitted = [int]$json.omitted_row_count
        explicit_uda_rows = $explicit.Count
        explicit_uda_equivalent = @($explicit | Where-Object comparison -eq 'equivalent').Count
    }
}

$summary = [pscustomobject]@{
    elements = $results.Count
    elements_with_explicit_uda = @($results | Where-Object explicit_uda_rows -gt 0).Count
    explicit_uda_rows = ($results | Measure-Object explicit_uda_rows -Sum).Sum
    explicit_uda_equivalent = ($results | Measure-Object explicit_uda_equivalent -Sum).Sum
    mismatch = ($results | Measure-Object mismatch -Sum).Sum
    deferred = ($results | Measure-Object deferred -Sum).Sum
    omitted = ($results | Measure-Object omitted -Sum).Sum
    results = $results
}
$summary | ConvertTo-Json -Depth 4 | Out-File -LiteralPath (Join-Path $here 'uda-verification.json') -Encoding utf8
$summary | Select-Object elements, elements_with_explicit_uda, explicit_uda_rows, explicit_uda_equivalent, mismatch, deferred, omitted | Format-List

if ($summary.elements_with_explicit_uda -ne 10 -or
    $summary.explicit_uda_rows -ne $summary.explicit_uda_equivalent -or
    $summary.mismatch -or $summary.deferred -or $summary.omitted) { exit 2 }
