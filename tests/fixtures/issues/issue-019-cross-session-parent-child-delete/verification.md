# Issue #19 verification record

Date: 2026-08-10

## Baseline — pinned parser before the fix

Command:

```powershell
cargo run --quiet --bin incr_fold_probe -- --file ".codex-artifacts\db8000-two-delete-fixture-20260810\final\ams8000_0001" --from 25 --to 26 --dbnum 8000
```

Exit status: `0`

Literal result summary:

```text
会话数=2 操作总数=3（Add 0 / Modified 1 / Deleted 2 / None 0） 去重 refno=2
sesno=26 refno=24384_24775 noun=ZONE children=Some(MemberChanged)
sesno=25 refno=24384_24778 Deleted
sesno=26 refno=24384_24778 Deleted
```

The sesno 25 BOX tombstone was missing and the EQUI tombstone appeared one session early.

## Modified parser — historical OWNER lookup

Input: the `parent_deleted` file extracted from `db8000-sesno24-26.zip`.

Command:

```powershell
cargo test --test db8000_two_delete_fixture -- --nocapture
```

Exit status: `0`

Literal output:

```text
running 6 tests
test archive::tests::archive_paths_must_stay_relative_and_normal ... ok
test archive_contains_the_three_declared_db8000_sessions ... ok
collect sesno: 25
collect sesno: 26
test final_file_window_preserves_child_then_parent_delete_sessions ... ok
test final_history_matches_the_session_25_point_in_time_snapshot ... ok
test combined_window_equals_the_union_of_its_session_slices ... ok
test window_folds_to_box_and_equi_deleted_with_zone_modified ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The test asserts the exact four operations:

```text
sesno 25: 24384_24778 EQUI Modified
sesno 25: 24384_24779 BOX Deleted
sesno 26: 24384_24775 ZONE Modified
sesno 26: 24384_24778 EQUI Deleted
```

## Fixture packaging

Generator command:

```powershell
cargo run --bin db8000_two_delete_fixture -- --source "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001"
```

Exit status: `0`

Literal output:

```text
fixture=tests\fixtures\issues\issue-019-cross-session-parent-child-delete
archive=tests\fixtures\issues\issue-019-cross-session-parent-child-delete\db8000-sesno24-26.zip bytes=4497104
window=25..=26
operations: 24384_24779 Deleted -> 24384_24778 Deleted
```

Default overwrite guard:

```text
EXPECTED_OVERWRITE_GUARD_EXIT=1
Error: tests\fixtures\issues\issue-019-cross-session-parent-child-delete already exists; pass --force to replace it
```

Forced regeneration preserves the unmanaged review artifacts:

```text
PATCH_PRESERVED=True
VERIFY_PRESERVED=True
```

Archive path validation:

```text
test archive::tests::archive_paths_must_stay_relative_and_normal ... ok
test result: ok. 1 passed; 0 failed
```

## Model-plan and repository regression

Commands and results:

```text
cargo test --lib child_delete_then_parent_delete_across_sessions_schedules_only_the_parent -- --nocapture
=> 1 passed; 0 failed

cargo check --locked --bin db8000_two_delete_fixture
=> exit 0

cargo clippy --locked --bin db8000_two_delete_fixture --test db8000_two_delete_fixture -- -D warnings
=> exit 0, no warnings

cargo test --locked --lib
=> 607 passed; 0 failed; 79 ignored

sigmap validate
=> [sigmap] ✓ config valid  coverage: 131%
```

## Dependency and rollback

`pdms_io` fix:

```text
commit 5c9e00e3c46f7d6f7c548583020b66e0ad23368a
branch codex/record-boundary-pin
push e2c2636c..5c9e00e3
```

Rollback smoke procedure: create a detached worktree at baseline `231e6185`, apply
`changes.patch`, add the ZIP and rollback artifacts, execute
`scripts/rollback_issue19.ps1`, then require an empty `git status`.

Literal output:

```text
Issue #19 rollback complete.
EXACT_ROLLBACK_SMOKE=PASS
```
