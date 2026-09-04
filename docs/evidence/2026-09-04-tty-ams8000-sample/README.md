# AMS 8000 E3D TTY sample

## Authority

- Candidate values come from e3d-io reading `ams8000_0001`, the checked-in
  `schema/e3d31` pack, and the six pinned physical DICT databases in
  `verify.ps1`. The explicit-UDA sampler is e3d-io commit `b754baf`.
- External values come from read-only E3D TTY `ATTLIST` and `Q MEMBERS` in
  project `AMS`, MDB `/ALL`.
- No legacy parser output or persisted legacy snapshot is an input.

## Attribute sample

The physical live index contains 6,605 refnos. The deterministic sampler chose
50; E3D completed 49 captures in four ten-element batches plus two isolated
sub-batches. The attempted fifth ten-element batch exited with Windows status
`3221225477` while querying
`24384/24814`, after samples 0 through 2 had complete end markers. That refno is
kept in `crash-witness-refnos.txt` and has no parity verdict.

Reproduction:

```powershell
D:\Rust\target\release\e3d_tty_sample_refs.exe --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001 --count 50 --out docs\evidence\2026-09-04-tty-ams8000-sample\refnos.txt
powershell -NoProfile -ExecutionPolicy Bypass -File docs\evidence\2026-09-04-tty-ams8000-sample\verify.ps1
```

The explicit-UDA list was selected in the e3d-io checkout with:

```powershell
$env:CARGO_TARGET_DIR='D:\Rust\target-codex-uda-probe'
cargo run --release --example genmodel_gap_probe -- uda D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001
```

The capture batches and every successful `extract-complete` invocation exited
0. The verification script deliberately exits non-zero while any raw mismatch
remains. Aggregate from `verification.json`:

```text
reports=68
tty_rows=5131
equivalent=5093
mismatch=23
deferred=0
omitted=0
```

The 23 mismatches repeat three runtime/query boundary shapes and do not expose
a new binary decoder defect:

- 12 STRU `:Room` rows: `24384/48`, `/63`, `/78`, `/93`, `/108`, `/123`,
  `/138`, `/153`, `/168`, `/440`, `/1629`, and `/24946`. Querying the
  abbreviated heading returns `:Roomfloor unset`; the binary value for
  `:Room` is unset.
- Seven `:CutLength` rows: BEND `24384/3082`, `/22830`, `/24548`, `/24680`,
  `/24936`, `/24943`, and ELBO `24384/23491`. Physical DICT/attlib yields
  unset, while E3D runtime PML computes a length.
- Four duplicate `:HXYsize` rows on BRAN `24384/24935` and `/24942`.
  Physical DICT/attlib yields unset while E3D runtime PML returns
  `1500 1000` twice per element.

The six-DICT probe then selected ten elements containing physical explicit UDA
values. Their TTY reports contain 35 such rows: all 35 are equivalent, with 0
mismatch. This closes the ten-element explicit-UDA quota; applicable-but-unset
headings are not counted.

## Owner/member order

`direct_tree_tty_compare --tty` compared four sampled e3d-io subtrees against
E3D `Q MEMBERS`: roots `24384/176`, `24384/308`, `24384/440`, and
`24384/572`. Aggregate from `owner-verification.json`:

```text
roots=4
direct_nodes=24
parents_compared=24
mismatch_count=0
direct_exit_statuses=[0]
tty_exit_statuses=[0]
```

This closes the AMS 8000 owner/member-order and explicit-UDA quotas. The
database's attribute sample is fully accounted but retains 23 explicit
runtime/query-boundary rows, so the plan's literal all-equivalent attribute
gate remains open rather than being silently weakened.
