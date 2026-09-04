$ErrorActionPreference = 'Stop'

$vendor = 'D:\work\plant-code\old\vendor\e3d-model'
$patch = Join-Path $PSScriptRoot 'catalogue-atta-components.patch'

rtk git -C $vendor apply --check --reverse $patch
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
rtk git -C $vendor apply --reverse $patch
exit $LASTEXITCODE
