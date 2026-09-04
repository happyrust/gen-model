param(
    [Parameter(Mandatory = $true)] [int] $BaseSesno,
    [Parameter(Mandatory = $true)] [int] $TargetSesno,
    [Parameter(Mandatory = $true)] [string] $Label
)

$ErrorActionPreference = 'Stop'

$db = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001'
$schema = 'D:\work\plant-code\old\vendor\e3d-io\schema\e3d31\binaries'
$attlib = Join-Path $schema 'attlib.dat'
$updateExe = 'D:\Rust\target\release\update_ams.exe'
$genExe = 'D:\Rust\target\release\gen_ams.exe'
$root = '24383/73948'
$catalogues = @(
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001',
    'D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000\aps7600_0001'
)
$projectDirs = @(
    'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000',
    'D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000'
)
$out = Join-Path $PSScriptRoot $Label
$updateOut = Join-Path $out 'update'
$baseOut = Join-Path $out "full-$BaseSesno"
$targetOut = Join-Path $out "full-$TargetSesno"

function Invoke-Probe {
    param([string] $Exe, [string[]] $Arguments, [string] $Log)

    & $Exe @Arguments 2>&1 | Tee-Object -FilePath $Log | ForEach-Object { Write-Host $_ }
    $status = $LASTEXITCODE
    if ($status -ne 0) {
        throw "$Exe failed with exit status $status; see $Log"
    }
    return $status
}

function Get-Key($element) {
    return ($element.geometry_id | ConvertTo-Json -Compress -Depth 20)
}

function Get-Signature($element) {
    $projection = [ordered]@{
        refno = $element.refno
        noun = $element.noun
        volume_mm3 = $element.volume_mm3
        aabb_min = $element.aabb_min
        aabb_max = $element.aabb_max
        world = $element.world
        triangles = $element.triangles
        genus = $element.genus
        negatives_applied = $element.negatives_applied
        negatives_skipped = $element.negatives_skipped
    }
    return ($projection | ConvertTo-Json -Compress -Depth 20)
}

function Get-ElementMap([object[]] $elements) {
    $map = @{}
    foreach ($element in $elements) {
        $key = Get-Key $element
        if ($map.ContainsKey($key)) { throw "duplicate geometry_id: $key" }
        $map[$key] = $element
    }
    return $map
}

$common = @('--attlib', $attlib, '--db', $db, '--template', $schema)
foreach ($catalogue in $catalogues) { $common += @('--catalogue', $catalogue) }
foreach ($projectDir in $projectDirs) { $common += @('--project-dir', $projectDir) }

New-Item -ItemType Directory -Force -Path $out | Out-Null
$updateStatus = Invoke-Probe $updateExe ($common + @(
    '--base-sesno', "$BaseSesno", '--target-sesno', "$TargetSesno", '--out', $updateOut
)) (Join-Path $out 'update.stdout.txt')
$baseStatus = Invoke-Probe $genExe ($common + @(
    '--sesno', "$BaseSesno", '--root', $root, '--out', $baseOut
)) (Join-Path $out 'full-base.stdout.txt')
$targetStatus = Invoke-Probe $genExe ($common + @(
    '--sesno', "$TargetSesno", '--root', $root, '--out', $targetOut
)) (Join-Path $out 'full-target.stdout.txt')

$base = Get-ElementMap @((Get-Content (Join-Path $baseOut 'elements.json') -Raw | ConvertFrom-Json))
$target = Get-ElementMap @((Get-Content (Join-Path $targetOut 'elements.json') -Raw | ConvertFrom-Json))
$upserts = Get-ElementMap @((Get-Content (Join-Path $updateOut 'upserts.json') -Raw | ConvertFrom-Json))
$removals = @((Get-Content (Join-Path $updateOut 'removals.json') -Raw | ConvertFrom-Json) | ForEach-Object {
    $_ | ConvertTo-Json -Compress -Depth 20
})
$removalSet = @{}
foreach ($key in $removals) { $removalSet[$key] = $true }

$changedTarget = @($target.Keys | Where-Object {
    -not $base.ContainsKey($_) -or (Get-Signature $base[$_]) -cne (Get-Signature $target[$_])
})
$disappeared = @($base.Keys | Where-Object { -not $target.ContainsKey($_) })
$missingUpserts = @($changedTarget | Where-Object { -not $upserts.ContainsKey($_) })
$staleUpserts = @($upserts.Keys | Where-Object {
    -not $target.ContainsKey($_) -or (Get-Signature $upserts[$_]) -cne (Get-Signature $target[$_])
})
$missingRemovals = @($disappeared | Where-Object { -not $removalSet.ContainsKey($_) })
$liveRemovals = @($removalSet.Keys | Where-Object { $target.ContainsKey($_) })

$baseReport = Get-Content (Join-Path $baseOut 'report.json') -Raw | ConvertFrom-Json
$targetReport = Get-Content (Join-Path $targetOut 'report.json') -Raw | ConvertFrom-Json
$knownMissingCatalogueTarget = 'db 7600 会话 12 的索引里没有 23984/105633'
$unexpectedBaseFailures = @($baseReport.failed | Where-Object detail -ne $knownMissingCatalogueTarget)
$unexpectedTargetFailures = @($targetReport.failed | Where-Object detail -ne $knownMissingCatalogueTarget)

$atta = foreach ($refno in @('24383/73951', '24383/73956')) {
    $baseItems = @($base.Values | Where-Object refno -eq $refno)
    $targetItems = @($target.Values | Where-Object refno -eq $refno)
    [ordered]@{
        refno = $refno
        base_count = $baseItems.Count
        target_count = $targetItems.Count
        changed = (@($targetItems | Where-Object {
            $key = Get-Key $_
            -not $base.ContainsKey($key) -or (Get-Signature $base[$key]) -cne (Get-Signature $_)
        })).Count
        upserted = (@($upserts.Values | Where-Object refno -eq $refno)).Count
        base_world_translation = @($baseItems | ForEach-Object { $_.world[3] })
        target_world_translation = @($targetItems | ForEach-Object { $_.world[3] })
    }
}

$result = [ordered]@{
    label = $Label
    base_sesno = $BaseSesno
    target_sesno = $TargetSesno
    root = $root
    exit_status = [ordered]@{ update = $updateStatus; full_base = $baseStatus; full_target = $targetStatus }
    counts = [ordered]@{
        full_base = $base.Count
        full_target = $target.Count
        changed_or_added = $changedTarget.Count
        disappeared = $disappeared.Count
        upserts = $upserts.Count
        removals = $removalSet.Count
    }
    mismatches = [ordered]@{
        missing_upserts = $missingUpserts.Count
        stale_upserts = $staleUpserts.Count
        missing_removals = $missingRemovals.Count
        live_removals = $liveRemovals.Count
        unexpected_base_failures = $unexpectedBaseFailures.Count
        unexpected_target_failures = $unexpectedTargetFailures.Count
    }
    known_data_gap = [ordered]@{
        detail = $knownMissingCatalogueTarget
        base_refnos = @($baseReport.failed | Where-Object detail -eq $knownMissingCatalogueTarget | ForEach-Object refno)
        target_refnos = @($targetReport.failed | Where-Object detail -eq $knownMissingCatalogueTarget | ForEach-Object refno)
    }
    atta = @($atta)
    verdict = 'PASS'
}

if ($missingUpserts.Count -or $staleUpserts.Count -or $missingRemovals.Count -or $liveRemovals.Count -or
    $unexpectedBaseFailures.Count -or $unexpectedTargetFailures.Count) {
    $result.verdict = 'FAIL'
}

$resultPath = Join-Path $out 'verification.json'
$result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $resultPath -Encoding UTF8
$result | ConvertTo-Json -Depth 20
if ($result.verdict -ne 'PASS') { exit 1 }
