[CmdletBinding()]
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath $RepoRoot).Path
$fixtureRelative = 'tests\fixtures\issues\issue-020-db8000-model-increment-ci-suite'
$fixture = [IO.Path]::GetFullPath((Join-Path $repo $fixtureRelative))
if (-not $fixture.StartsWith($repo, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Fixture escaped repository root: $fixture"
}

$patch = Join-Path $fixture 'changes.patch'
if (-not (Test-Path -LiteralPath $patch -PathType Leaf)) {
    throw "Missing rollback patch: $patch"
}

Push-Location $repo
try {
    & git apply --check --reverse -- $patch
    if ($LASTEXITCODE -ne 0) { throw 'Issue #20 reverse patch check failed' }
    & git apply --reverse -- $patch
    if ($LASTEXITCODE -ne 0) { throw 'Issue #20 reverse patch failed' }

    foreach ($path in @(
        (Join-Path $fixture 'changes.patch'),
        (Join-Path $fixture 'verification.md')
    )) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
    }

    if (Test-Path -LiteralPath $fixture) {
        $unexpected = @(Get-ChildItem -LiteralPath $fixture -Recurse -File -Force)
        if ($unexpected.Count -ne 0) {
            throw "Rollback left unexpected fixture files: $($unexpected.FullName -join ', ')"
        }
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }

    Write-Output 'Issue #20 rollback complete.'
}
finally {
    Pop-Location
}

# The script is excluded from changes.patch so it remains readable through reverse application.
if (Test-Path -LiteralPath $PSCommandPath) {
    Remove-Item -LiteralPath $PSCommandPath -Force
}
