[CmdletBinding()]
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath $RepoRoot).Path
$fixtureRelative = 'tests\fixtures\issues\issue-019-cross-session-parent-child-delete'
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
    if ($LASTEXITCODE -ne 0) { throw 'Issue #19 reverse patch check failed' }
    & git apply --reverse -- $patch
    if ($LASTEXITCODE -ne 0) { throw 'Issue #19 reverse patch failed' }

    $managedRemainder = @(
        (Join-Path $fixture 'db8000-sesno24-26.zip'),
        (Join-Path $fixture 'changes.patch'),
        (Join-Path $fixture 'verification.md')
    )
    foreach ($path in $managedRemainder) {
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

    foreach ($directory in @(
        (Join-Path $repo 'src\bin\db8000_two_delete_fixture'),
        (Join-Path $repo 'tests\fixtures\issues')
    )) {
        if ((Test-Path -LiteralPath $directory) -and
            -not (Get-ChildItem -LiteralPath $directory -Force | Select-Object -First 1)) {
            Remove-Item -LiteralPath $directory -Force
        }
    }

    Write-Output 'Issue #19 rollback complete.'
}
finally {
    Pop-Location
}

# This script is a managed Issue #19 artifact but is intentionally excluded
# from changes.patch so the reverse patch can be read before cleanup.
if (Test-Path -LiteralPath $PSCommandPath) {
    Remove-Item -LiteralPath $PSCommandPath -Force
}
