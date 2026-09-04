# Direct model correctness check

This check uses e3d-io/e3d-model physical database reads as the candidate and
the E3D TTY-exported narrow RVM/ATT pair for `/C-IY-1R330-B` as the reference.
No legacy parser output participates.

## Source state

```text
gen-model HEAD: 84706d6a841d9dbe181c6d25ba24a8ea7fd451cc
e3d-io HEAD:    3e40c31b34eef0a2251abc03e32b640db09bfe04
e3d-model HEAD: d440af36acdb4d511be2a773886284b6e860cb5e
e3d-model worktree diff hash: 62b65d66da5c9ca951a7c88cacbeedf641bcd633
gen-model worktree diff hash:  5bc74d810a5abb16ec685f110be7fe5c9f1ccc65
physical DESI session: ams8000_0001 sesno 281
```

The worktrees already contained unrelated changes. They were retained and are
identified by diff hash rather than folded into the validation commits.

## Direct generation

`D:\Rust\target\release\gen_ams.exe` was run with the e3d-io E3D 3.1
`attlib.dat`/template pack, physical `ams8000_0001`, explicit `ams7000_0001`
and `aps7600_0001` catalogues, both AMS/ACP project directories, root
`24384/22404`, and output `c-iy`. Exit status `0`:

```text
visited=37 generated=35 skipped=0 failed=0 route_containers=1
route_members=36 implied_tubes=37 unsupported=0 unknown=0 orphans=0
by noun: BEND=18 FTUB=17
```

The independently imported E3D RVM/ATT baseline reported 40 members, 141
primitive geometries, 37 resolved identities, and zero degenerate AABBs. Import
exit status was `0`.

## Comparison

The element-level `rvm_compare` joined all candidate/reference solids by E3D
identity (`BEND 18/18`, `FTUB 17/17`, no one-sided members). Its AABB verdict
was intentionally red even after facet reconstruction: E3D represents the
ACP1000 tray shell about 100 mm beyond the candidate tube surface. The raw
exit status is retained as `1`; it is a representation-scope result, not hidden
as a pass.

The established full-branch union acceptance was then run:

```powershell
cargo test --offline --locked --lib --no-default-features `
  --features ws,gen_model,manifold,project_hd,rvm_verify `
  mesh_c_iy_full_branch -- --ignored --nocapture
```

Literal result, exit status `0`:

```text
C-IY skipped zero-surface members: ["FTUBE 6 (pe:24384_22416)"]
FULL BRANCH C-IY (35 gen / 35 rvm members)
both mean=10.27 p95=97.83 hausdorff=100.02
gen->rvm mean=0.06 p95=0.00 max=6.98
rvm->gen mean=20.47 p95=100.00 max=100.02
test result: ok. 1 passed; 0 failed
```

Verdict: the current direct-generated model passes the repository's declared
model correctness gate (`gen->RVM p95 <= 10 mm`, max `<= 30 mm`). It does not
claim symmetric shape equality: the reverse 100 mm delta is the documented E3D
tray-shell representation that the candidate deliberately does not generate.

## Persist and rollback

To ensure the test read this run's current direct output, `direct_model_persist`
temporarily published the 35 generated relations to the local validation store.
It found zero pre-existing records for those exact keys, wrote no new mesh files
(all 35 content hashes were reused), and produced:

- `persist/inst-relation-backup.json`
- `persist/update.sql`
- `persist/rollback.sql`
- `persist/persisted-elements.json`

`rollback.sql` SHA-256 is
`607de2b8eebad84563808e78373c4204bfb8ef07f45ae00b9bbd3bec43873e5b`.
Executing it against `ws://127.0.0.1:8009`, namespace `1516`, database
`AvevaMarineSample`, exited `0`. A follow-up query over all 35 exact
`inst_relate` keys returned `[[]]` and exited `0`, confirming restoration.
