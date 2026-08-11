$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$patch = Join-Path $repo '.codex-artifacts\db8000-two-delete-fixture-20260810\changes.patch'
if (Test-Path -LiteralPath $patch) {
    git -C $repo apply --reverse --whitespace=nowarn -- $patch
    if ($LASTEXITCODE -ne 0) { throw "reverse patch failed: $patch" }
}
$artifact = Join-Path $repo '.codex-artifacts\db8000-two-delete-fixture-20260810'
if (Test-Path -LiteralPath $artifact) {
    Remove-Item -LiteralPath $artifact -Recurse -Force
}
Write-Output 'Rolled back db8000 two-delete fixture files and source changes.'
