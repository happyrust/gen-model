# db8000 model-increment regression suite design

## 1. Prioritized regression matrix

### Scope assumption

The first deliverable must stay strictly inside the existing Issue #19 fixture:

* real dbnum: **8000**
* snapshots:

  * sesno 24 baseline
  * sesno 25 child deleted
  * sesno 26 parent deleted
* archive:

  * `tests/fixtures/issues/issue-019-cross-session-parent-child-delete/db8000-sesno24-26.zip`
  * 4.49 MB ZIP, no LFS requirement
* no AVEVA runtime, no SurrealDB, no external tools.

The fixture manifest already fixes the window as `24 → 25 → 26`, refs:

* ZONE `24384_24775`
* parent EQUI `24384_24778`
* child BOX `24384_24779`


---

## Priority matrix

| Priority | Case                                       | Source                     | Range      | Purpose                                                | New snapshot required? |
| -------- | ------------------------------------------ | -------------------------- | ---------- | ------------------------------------------------------ | ---------------------- |
| P0       | Cross-session child delete + parent delete | Existing Issue #19         | `25..=26`  | Protect fixed bug: historical OWNER lookup             | No                     |
| P0       | Per-session extraction correctness         | Existing ZIP               | `25`, `26` | Ensure collector does not move events between sessions | No                     |
| P0       | Net-change fold correctness                | Existing ZIP               | `25..=26`  | Verify Delete/Delete/Modified folding                  | No                     |
| P0       | Delete scheduling collapse                 | Existing ZIP + pure helper | `25..=26`  | Ensure only topmost recursive cleanup is scheduled     | No                     |
| P1       | Transform-only regression                  | None                       | TBD        | Validate `Transform` path                              | Yes                    |
| P1       | Cascade/reverse-reference regression       | None                       | TBD        | Validate `CascadeExpand`                               | Yes                    |
| P2       | Room recalculation regression              | None                       | TBD        | Validate `RoomRecalcPanel/Element`                     | Yes                    |

The first release should ship only the four P0 cases. The remaining cases require intentionally-created PDMS histories because the current ZIP does not contain those semantics.

---

# 2. Exact cases and expected assertions

## Case P0-1: collect_changes preserves historical session ownership

### Input

```
fixture: db8000-sesno24-26.zip
dbnum: 8000
file: sesno-026-parent-deleted/ams8000_0001
range: 25..=26
```

Production API:

```rust
IncrementPipeline::collect_changes(&final_file, 25..=26)
```

The existing test already uses this production path.


### Expected raw operations

Total operations:

```
4
```

Expected:

| sesno | refno       | operation | noun |
| ----- | ----------- | --------- | ---- |
| 25    | 24384_24778 | Modified  | EQUI |
| 25    | 24384_24779 | Deleted   | BOX  |
| 26    | 24384_24775 | Modified  | ZONE |
| 26    | 24384_24778 | Deleted   | EQUI |



Failure signature to prevent:

```
sesno25:
    EQUI Deleted  ❌

missing:
    BOX Deleted @25 ❌
```

The old behavior produced exactly that false result.


---

## Case P0-2: session partition integrity

Input:

```
range: 25..=26
```

Expected map:

```rust
changes.keys()
==
vec![25,26]
```

Assertions:

```rust
changes[&25].len() == 2
changes[&26].len() == 2
```

Expected session 25:

```
24384_24778 Modified
24384_24779 Deleted
```

Expected session 26:

```
24384_24775 Modified
24384_24778 Deleted
```

The expected session snapshots already encode this split.



---

## Case P0-3: merge_net_changes regression

Input:

```rust
merge_net_changes(&collected)
```

Expected:

```text
24384_24779 -> Deleted
24384_24778 -> Deleted
24384_24775 -> Modified
```

Reason:

The collector returns session events; the planner must merge them into final semantic changes.

Existing merge logic:

```rust
fold_net_op()
merge_net_change_details()
```

already defines:

```
Modify + Delete = Deleted
Add + Delete = Cancelled
multiple Modify = Modified
```



---

## Case P0-4: model work plan only deletes topmost subtree

Input:

Synthetic ownership graph from real refs:

```
ZONE
 |
 EQUI 24384_24778
 |
 BOX 24384_24779
```

Expected:

Net changes:

```
BOX Deleted
EQUI Deleted
ZONE Modified
```

But model work:

```
DeleteCleanup(24384_24778)
```

Only parent cleanup.

Reason:

`delete_inst_relate_subtree` already recursively removes children. Child cleanup duplicates work.


Existing pure regression:

```
child_delete_then_parent_delete_across_sessions_schedules_only_the_parent
```

uses exactly this db8000 topology.


---

# 3. Rust test/helper structure

Avoid comparison JSON as the oracle.

JSON may remain as documentation, but tests should execute production code.

Recommended layout:

```
tests/
├── db8000_model_increment.rs
├── fixtures/
│   └── issues/
│       └── issue-019-cross-session-parent-child-delete/
│           ├── manifest.json
│           └── db8000-sesno24-26.zip

src/
└── data_interface/
    ├── increment_pipeline.rs
    ├── manual_update.rs
    └── model_update_plan.rs
```

---

## Fixture helper

Reuse:

```rust
archive::verify_and_extract()
```

It already:

* validates manifest
* validates SHA256
* checks archive size
* extracts into temporary directory
* verifies extracted snapshots



---

## Integration test

Example:

```rust
#[test]
fn db8000_issue19_window_25_26_regression()
{
    let fixture = verify_and_extract(...)?;

    let final_file =
        fixture.path_for_role("parent_deleted")?;

    let changes =
        IncrementPipeline::collect_changes(
            &final_file,
            25..=26
        )?;

    assert_raw_operations(&changes);

    let merged =
        merge_net_changes(&changes);

    assert_net_changes(&merged);

    let plan =
        build_model_update_plan(...);

    assert_delete_cleanup(plan);
}
```

Important:

Do not deserialize:

```
expected-after-fix-window-25-26.json
```

inside tests.

That only checks that JSON matches JSON.

The real contract is:

```
PDMS snapshot
    ↓
PdmsIO
    ↓
collect_changes()
    ↓
merge_net_change_details()
    ↓
build_model_update_plan()
    ↓
ModelWorkItem
```

---

# 4. GitHub Actions design

## Current problem

Existing workflow only:

```
cargo build --release
```

and does not execute tests.

It already has the correct build feature set:

```yaml
--features ws,gen_model,manifold,occ,project_hd,http_api
```



However:

```
ws,project_hd
```

alone currently fails because `fast_model` imports are unconditional.

Therefore CI should not use arbitrary feature combinations.

---

# Proposed workflows

## PR workflow

New:

```
.github/workflows/windows-test.yml
```

Trigger:

```yaml
on:
  pull_request:
  workflow_dispatch:
```

Steps:

```
checkout

install nightly-2026-08-02

restore cargo cache

cargo test
    --locked
    --no-default-features
    --features ws,gen_model,manifold,project_hd,http_api
    --tests
```

Do not run:

```
--ignored
```

for normal PR.

The first fixture test should become non-ignored because:

* ZIP is only 4.49 MB
* extraction is local
* no external dependency

---

## Main workflow

Keep current binary workflow.

Add:

```
needs: regression-test
```

before release packaging.

Pipeline:

```
PR:
    compile
    regression

main:
    regression
       |
    release build
       |
    package
```

---

## Cache

Reuse:

```
~/.cargo/registry
~/.cargo/git
target
```

The existing workflow already caches those paths.


Recommended key:

```
cargo-test-${{ hashFiles('Cargo.lock') }}
```

separate from release cache.

---

# 5. False-positive risks

## 1. Overfitting to refnos

Risk:

```
24384_24778
```

only protects one model.

Mitigation:

Keep fixture test for regression, but add generated synthetic topology tests later.

---

## 2. Testing JSON instead of behavior

Risk:

A broken parser can still produce the expected JSON if test bypasses production.

Mitigation:

Never load:

```
expected-after-fix-window-25-26.json
```

as assertion source.

Use it only as review evidence.

---

## 3. ZIP integrity mistaken for model correctness

Current archive verification proves:

```
correct bytes
correct snapshot
```

but not:

```
correct incremental semantics
```

Need both:

```
archive verification
+
production pipeline assertion
```

---

## 4. Planner test hiding collector regression

Risk:

Synthetic `EleOperationData` tests pass while PDMS parsing breaks.

Mitigation:

Maintain two layers:

```
Layer A:
real ZIP → collect_changes()

Layer B:
pure operations → planner logic
```

---

## 5. Feature drift in CI

Risk:

A future Cargo feature change makes regression unavailable.

Mitigation:

Pin CI feature list explicitly:

```yaml
ws,gen_model,manifold,project_hd,http_api
```

and add a compile-only smoke test.

---

# 6. Staged implementation plan

## Stage 0 — first deliverable (only existing ZIP)

Goal:

Portable regression.

Changes:

1. Add:

```
tests/db8000_model_increment.rs
```

2. Move fixture from ignored-only test to normal integration test.

3. Add assertions:

```
collect_changes()
    ↓
4 operations

merge_net_changes()
    ↓
3 net changes

model plan
    ↓
1 parent cleanup
```

No:

* SurrealDB
* AVEVA
* RVM
* Git LFS
* external tools

---

## Stage 1 — CI integration

Add:

```
windows-test.yml
```

Run:

```
cargo test --tests
```

on PR.

---

## Stage 2 — broaden model regression

Need new snapshots:

### Transform case

Example history:

```
sesno 40
    POS change

expected:
Transform
```

Tests:

```
ModelWorkAction::Transform
```

---

### Geometry regeneration case

Need:

```
BRAN/HANG member movement
```

Expected:

```
RegenRoot
```

because BRAN/HANG geometry depends on member positions.


---

### Cascade case

Need:

```
CATA/spec/reference change
```

Expected:

```
CascadeExpand
```

---

### Room case

Need:

```
PANE move
ROOM rename
```

Expected:

```
RoomRecalcPanel
RoomRecalcElement
```

The model work enum already contains these actions.


---

## Final recommended first PR scope

Keep it narrow:

```
PR #1
========

+ db8000 fixture test
+ remove ignored requirement
+ Windows PR CI test job
+ cargo cache split
+ assertions on:
    - raw operations
    - net changes
    - delete cleanup plan

No new fixtures.
No SurrealDB.
No UI.
No AVEVA.
```

This gives a high-value regression gate around the exact historical OWNER bug fixed in `pdms_io@5c9e00e3c46f7d6f7c548583020b66e0ad23368a`, whose purpose was to read OWNER membership at the requested session boundary rather than the final file state.
