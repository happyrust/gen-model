$ErrorActionPreference = 'Stop'
$vendor = 'D:\work\plant-code\old\vendor\e3d-io'
$patch = Join-Path $PSScriptRoot 'catalogue-pml-text-literal.patch'
git -C $vendor apply --check --reverse $patch
if ($LASTEXITCODE -ne 0) { throw "rollback preflight failed with exit $LASTEXITCODE" }
git -C $vendor apply --reverse $patch
if ($LASTEXITCODE -ne 0) { throw "rollback failed with exit $LASTEXITCODE" }

