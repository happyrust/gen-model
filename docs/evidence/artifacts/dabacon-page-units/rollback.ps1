param([switch]$Apply)

$ErrorActionPreference = 'Stop'
$artifactDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$targets = @(
    @{ Repo = 'D:\work\plant-code\pdms-io-fork-engine-v2'; Patch = Join-Path $artifactDir 'engine.patch' },
    @{ Repo = 'D:\work\plant-code\old-parse-pdms-db-paged'; Patch = Join-Path $artifactDir 'parse.patch' },
    @{ Repo = 'D:\work\plant-code\old\gen-model-occ-retire-endgame'; Patch = Join-Path $artifactDir 'main.patch' }
)

foreach ($target in $targets) {
    git -C $target.Repo apply --reverse --check $target.Patch
    if ($LASTEXITCODE -ne 0) {
        throw "rollback preflight failed: $($target.Repo)"
    }
}

if (-not $Apply) {
    Write-Output 'rollback preflight passed; rerun with -Apply to reverse the recorded patches'
    exit 0
}

foreach ($target in $targets) {
    git -C $target.Repo apply --reverse $target.Patch
    if ($LASTEXITCODE -ne 0) {
        throw "rollback failed: $($target.Repo)"
    }
}

Write-Output 'rollback applied'
