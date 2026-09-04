param([switch]$Restore)
$ErrorActionPreference = 'Stop'
$repo = 'D:\work\plant-code\old\gen-model'
$path = 'src/bin/e3d_tty_cata_members_compare.rs'
$commit = git -C $repo log -1 --format=%H -- $path
if ($LASTEXITCODE -ne 0 -or -not $commit) { throw 'tool commit was not found' }
$patch = git -C $repo show --format= $commit -- $path
$direction = if ($Restore) { @() } else { @('--reverse') }
$action = if ($Restore) { 'restore' } else { 'rollback' }
$patch | git -C $repo apply --check @direction
if ($LASTEXITCODE -ne 0) { throw "$action preflight failed with exit $LASTEXITCODE" }
$patch | git -C $repo apply @direction
if ($LASTEXITCODE -ne 0) { throw "$action failed with exit $LASTEXITCODE" }
Write-Output "$action succeeded for $path from $commit"
