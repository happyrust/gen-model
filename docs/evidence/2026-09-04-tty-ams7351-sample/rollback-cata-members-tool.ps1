param([switch]$Restore)
$ErrorActionPreference = 'Stop'
$repo = 'D:\work\plant-code\old\gen-model'
$path = 'src/bin/e3d_tty_cata_members_compare.rs'
$commit = git -C $repo log -1 --format=%H -- $path
if ($LASTEXITCODE -ne 0 -or -not $commit) { throw 'tool commit was not found' }
$patchFile = Join-Path ([System.IO.Path]::GetTempPath()) "e3d-cata-members-$commit.patch"
$action = if ($Restore) { 'restore' } else { 'rollback' }
try {
    git -C $repo show --format= --output=$patchFile $commit -- $path
    if ($LASTEXITCODE -ne 0) { throw "could not create patch with exit $LASTEXITCODE" }
    if ($Restore) {
        git -C $repo apply --check $patchFile
        if ($LASTEXITCODE -ne 0) { throw "$action preflight failed with exit $LASTEXITCODE" }
        git -C $repo apply $patchFile
    } else {
        git -C $repo apply --check --reverse $patchFile
        if ($LASTEXITCODE -ne 0) { throw "$action preflight failed with exit $LASTEXITCODE" }
        git -C $repo apply --reverse $patchFile
    }
    if ($LASTEXITCODE -ne 0) { throw "$action failed with exit $LASTEXITCODE" }
    Write-Output "$action succeeded for $path from $commit"
} finally {
    Remove-Item -LiteralPath $patchFile -ErrorAction SilentlyContinue
}
