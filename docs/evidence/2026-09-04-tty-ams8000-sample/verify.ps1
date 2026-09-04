param(
    [string]$Descriptor = 'D:\Rust\target\release\e3d-descriptor.exe'
)

$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$reports = Join-Path $here 'reports'
New-Item -ItemType Directory -Force -Path $reports | Out-Null

$db = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001'
$schema = 'D:\work\plant-code\old\vendor\e3d-io\schema\e3d31'
$dictionaries = @(
    'D:\AVEVA\Projects\E3D3.1\SCB\scb000\scb6002_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5100_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7043_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7006_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7323_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5101_0001'
) -join ';'
$references = @(
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5052_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5053_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5054_0001'
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7320_0001'
    'D:\AVEVA\Projects\E3D3.1\ZDJ\zdj000\zdj7600_0001'
) -join ';'

$results = foreach ($log in Get-ChildItem -LiteralPath (Join-Path $here 'logs') -File -Filter '*.log' | Sort-Object Name) {
    $refno = $log.BaseName -replace '-', '/'
    $report = Join-Path $reports ($log.BaseName + '.json')
    $output = & $Descriptor extract-complete --db $db --schema $schema --uda on `
        --dictionary-db-list $dictionaries --reference-db-list $references `
        --refno $refno --tty-log $log.FullName
    $exitStatus = $LASTEXITCODE
    $output | Out-File -LiteralPath $report -Encoding utf8
    if ($exitStatus -ne 0) {
        throw "extract-complete failed for $refno with exit $exitStatus"
    }
    $json = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
    [pscustomobject]@{
        refno = $refno
        exit_status = $exitStatus
        tty_rows = $json.tty_row_count
        equivalent = [int]($json.comparison_counts.equivalent)
        mismatch = [int]($json.comparison_counts.mismatch)
        deferred = [int]($json.deferred_uda_row_count)
        omitted = [int]($json.omitted_row_count)
    }
}

$summary = [pscustomobject]@{
    candidate = [pscustomobject]@{ db = $db; schema = $schema; dictionaries = $dictionaries; references = $references }
    authority = 'E3D TTY ATTLIST from AMS /ALL'
    reports = $results.Count
    tty_rows = ($results | Measure-Object tty_rows -Sum).Sum
    equivalent = ($results | Measure-Object equivalent -Sum).Sum
    mismatch = ($results | Measure-Object mismatch -Sum).Sum
    deferred = ($results | Measure-Object deferred -Sum).Sum
    omitted = ($results | Measure-Object omitted -Sum).Sum
    results = $results
}
$summary | ConvertTo-Json -Depth 6 | Out-File -LiteralPath (Join-Path $here 'verification.json') -Encoding utf8
$summary | Select-Object reports, tty_rows, equivalent, mismatch, deferred, omitted | Format-List

if ($summary.mismatch -or $summary.deferred -or $summary.omitted) { exit 2 }
