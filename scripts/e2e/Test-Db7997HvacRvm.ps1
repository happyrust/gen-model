#Requires -Version 7
[CmdletBinding()]
param(
    [string]$EvidenceDir = 'output/rvm-7997-e3d',
    [string]$MeshDir = '.sites/7997/assets/meshes',
    [string]$Verifier = 'D:\Rust\target\release\rvm_verify.exe',
    [int]$Samples = 4000,
    [double]$ModelP95Mm = 1.0,
    [double]$ModelMaxMm = 2.0,
    [double]$RvmFacetTolMm = 10.0
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Push-Location $repo
try {
    $evidence = (Resolve-Path $EvidenceDir).Path
    $mesh = (Resolve-Path $MeshDir).Path
    $verifierPath = (Resolve-Path $Verifier).Path
    $exportManifestPath = Join-Path $evidence '7997-rvm-export-manifest.json'
    $exportManifest = Get-Content -LiteralPath $exportManifestPath -Raw | ConvertFrom-Json

    $expectedTargets = 942
    $pairTotal = 0
    $runs = [System.Collections.Generic.List[object]]::new()
    $allPairs = [System.Collections.Generic.List[object]]::new()

    foreach ($site in $exportManifest.files) {
        $key = [string]$site.export_root
        $rvm = (Resolve-Path ([string]$site.path)).Path
        $actualRvmHash = (Get-FileHash -LiteralPath $rvm -Algorithm SHA256).Hash
        if ($actualRvmHash -ne [string]$site.sha256) {
            throw "RVM hash drift for $key expected=$($site.sha256) actual=$actualRvmHash"
        }

        $pairFile = Join-Path $evidence "pairs-$key.json"
        $pairs = @(Get-Content -LiteralPath $pairFile -Raw | ConvertFrom-Json)
        $pairTotal += $pairs.Count
        $reportPath = Join-Path $evidence "compare-all-$key.json"
        $consolePath = Join-Path $evidence "compare-all-$key.console.log"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        & $verifierPath mesh-compare `
            --rvm $rvm `
            --mesh-dir $mesh `
            --pair-file $pairFile `
            --url ws://127.0.0.1:7997 `
            --ns 1516 `
            --db AvevaMarineSample `
            --user root `
            --password root `
            --samples $Samples `
            --include-descendants `
            --tol-p95-mm $ModelP95Mm `
            --tol-max-mm $ModelMaxMm `
            --rvm-facet-tol-mm $RvmFacetTolMm `
            --report $reportPath *> $consolePath
        $exit = $LASTEXITCODE
        $sw.Stop()

        if (-not (Test-Path -LiteralPath $reportPath)) {
            throw "Verifier produced no report for $key exit=$exit log=$consolePath"
        }
        $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
        $reportedPairs = @($report.pairs)
        foreach ($pair in $reportedPairs) {
            $allPairs.Add([pscustomobject]@{
                export_root = $key
                rvm_group = $pair.rvm_group
                refno = $pair.refno
                passed = [bool]$pair.passed
                generated_to_rvm = $pair.generated_to_rvm
                rvm_to_generated = $pair.rvm_to_generated
                note = $pair.note
            })
        }
        $runs.Add([pscustomobject]@{
            export_root = $key
            rvm = $rvm
            rvm_sha256 = $actualRvmHash
            pair_file = $pairFile
            pair_file_sha256 = (Get-FileHash -LiteralPath $pairFile -Algorithm SHA256).Hash
            expected_pairs = $pairs.Count
            reported_pairs = $reportedPairs.Count
            passed_pairs = @($reportedPairs | Where-Object passed).Count
            failed_pairs = @($reportedPairs | Where-Object { -not $_.passed }).Count
            verifier_exit = $exit
            elapsed_ms = $sw.ElapsedMilliseconds
            report = $reportPath
            report_sha256 = (Get-FileHash -LiteralPath $reportPath -Algorithm SHA256).Hash
            console = $consolePath
        })
        "RVM_PROGRESS site=$key pairs=$($reportedPairs.Count) passed=$(@($reportedPairs | Where-Object passed).Count) exit=$exit elapsed_ms=$($sw.ElapsedMilliseconds)"
    }

    if ($pairTotal -ne $expectedTargets) {
        throw "7997 pair census drift expected=$expectedTargets actual=$pairTotal"
    }

    $failed = @($allPairs | Where-Object { -not $_.passed })
    $summary = [pscustomobject]@{
        dbnum = 7997
        target_scope = 'all HVAC BRAN/HANG'
        expected_targets = $expectedTargets
        pair_count = $allPairs.Count
        passed_count = @($allPairs | Where-Object passed).Count
        failed_count = $failed.Count
        completed_at = (Get-Date).ToString('o')
        verifier = $verifierPath
        verifier_sha256 = (Get-FileHash -LiteralPath $verifierPath -Algorithm SHA256).Hash
        mesh_dir = $mesh
        samples_per_direction = $Samples
        model_tol_p95_mm = $ModelP95Mm
        model_tol_max_mm = $ModelMaxMm
        rvm_facet_tol_mm = $RvmFacetTolMm
        runs = $runs
        failures = $failed
    }
    $summaryPath = Join-Path $evidence 'compare-all-7997-hvac-branch-summary.json'
    $summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding utf8
    "RVM_ALL_DONE pairs=$($allPairs.Count) passed=$($summary.passed_count) failed=$($summary.failed_count) summary=$summaryPath"

    if (@($runs | Where-Object verifier_exit -ne 0).Count -ne 0 -or $summary.failed_count -ne 0) {
        exit 1
    }
}
finally {
    Pop-Location
}
