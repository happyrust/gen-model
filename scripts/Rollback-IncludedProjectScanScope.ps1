[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$patch = Join-Path $repo 'docs\evidence\2026-08-24-included-project-scan-scope.patch'
$expectedBaseline = '04db947c45c642a7d6b97723bef8ca187bce503e'

git -C $repo apply --reverse --check -- $patch
if ($LASTEXITCODE -ne 0) {
    throw "rollback preflight failed with exit code $LASTEXITCODE"
}

git -C $repo apply --reverse -- $patch
if ($LASTEXITCODE -ne 0) {
    throw "rollback apply failed with exit code $LASTEXITCODE"
}

$actual = (git -C $repo hash-object -- 'src/data_interface/project_paths.rs').Trim()
if ($actual -ne $expectedBaseline) {
    throw "rollback hash mismatch: expected=$expectedBaseline actual=$actual"
}

Write-Output "rollback verified: src/data_interface/project_paths.rs blob=$actual"
