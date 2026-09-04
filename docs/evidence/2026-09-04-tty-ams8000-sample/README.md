# AMS 8000 E3D TTY sample

## Authority

- Candidate values come from e3d-io reading `ams8000_0001`, the checked-in
  `schema/e3d31` pack, and the six pinned physical DICT databases in
  `verify.ps1`.
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

The capture batches and every successful `extract-complete` invocation exited
0. The verification script deliberately exits non-zero while any raw mismatch
remains. Aggregate from `verification.json`:

```text
reports=49
tty_rows=3163
equivalent=3155
mismatch=8
deferred=0
omitted=0
```

The eight mismatches repeat the two already documented AMS 7999 external
boundaries and do not expose a new binary decoder defect:

- `:Room` on STRU `24384/440`, `24384/1629`, and `24384/24946`: querying the
  abbreviated heading returns `:Roomfloor unset`; the binary value for `:Room`
  is unset.
- `:CutLength` on BEND `24384/3082`, `24384/22830`, `24384/24548`,
  `24384/24680`, and ELBO `24384/23491`: physical DICT/attlib yields unset,
  while E3D runtime PML computes `18mm`, `6mm`, `3mm`, `2mm`, and `19mm`.

Thirty-one reports contain applicable UDA headings, but only three sampled
elements contain binary-explicit UDA values: STRU `24384/440`, `24384/1629`,
and `24384/24946`. Therefore the separate ten-element explicit-UDA quota is
still pending and is not inferred from applicable-but-unset headings.

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

This closes the AMS 8000 owner/member-order quota. The database's attribute
sample is fully accounted but retains eight explicit external-boundary rows;
the ten-element binary-explicit UDA quota remains the next action.
