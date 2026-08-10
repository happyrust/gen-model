# Issue #20 verification record

Date: 2026-08-10

## Oracle design review

Command family:

```text
oracle --engine browser --model gpt-5.6-sol --browser-thinking-time heavy ...
```

Result:

```text
Session: db8000-model-increment-suite
Model selection: GPT-5.6 Sol, verified
Exit status: 0
```

The saved review is `oracle-review.md`. Its first-deliverable recommendation is implemented: reuse
the existing Issue #19 ZIP, exercise production collection/folding, keep external-service cases for
new snapshots, and run the portable suite on Windows pull requests and `main`.

## Portable real-file suite

Command:

```powershell
cargo test --locked --test db8000_two_delete_fixture `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

Exit status: `0`

Literal result summary:

```text
running 6 tests
test archive::tests::archive_paths_must_stay_relative_and_normal ... ok
test archive_contains_the_three_declared_db8000_sessions ... ok
test final_file_window_preserves_child_then_parent_delete_sessions ... ok
test final_history_matches_the_session_25_point_in_time_snapshot ... ok
test combined_window_equals_the_union_of_its_session_slices ... ok
test window_folds_to_box_and_equi_deleted_with_zone_modified ... ok
test result: ok. 6 passed; 0 failed; 0 ignored
```

## Model work action

Command:

```powershell
cargo test --locked --lib `
  child_delete_then_parent_delete_across_sessions_schedules_only_the_parent `
  --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

Exit status: `0`; result: `1 passed; 0 failed`.

The test asserts one `ModelWorkAction::DeleteCleanup`, targeting EQUI `24384_24778` at source
sesno 26; no separate BOX cleanup is emitted.

## Static and repository checks

```text
cargo clippy --locked --lib --test db8000_two_delete_fixture
  --no-default-features --features ws,gen_model,manifold,project_hd -- -D warnings
=> exit 0

cargo test --locked --lib
=> 617 passed; 0 failed; 79 ignored

sigmap validate
=> config valid; coverage 131%

Python yaml.safe_load(.github/workflows/windows-tests.yml)
=> WORKFLOW_YAML_PARSE=PASS
```

## Fixture identity

```text
archive bytes: 4497104
archive SHA256: 6f7abbf548b37d8c016d2b8a2b52f3eddb1610fce1a00eca85fe71c9aa23f871
```

The suite extracts only manifest-declared relative paths into RAII temporary directories. GitHub
checkout supplies the ZIP directly; Git LFS and external extraction tools are not used.

## Rollback

Run:

```powershell
pwsh -File scripts/rollback_issue20.ps1
```

The script verifies and reverse-applies `changes.patch`, removes Issue #20 review artifacts, and
deletes itself after completion.

Detached-worktree smoke output:

```text
Issue #20 rollback complete.
ISSUE20_FINAL_ROLLBACK_SMOKE=PASS
```
