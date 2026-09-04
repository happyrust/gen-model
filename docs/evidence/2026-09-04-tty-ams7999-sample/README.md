# AMS 7999 deterministic TTY sample

The sample list is derived only from the latest physical refno index read by
e3d-io. It contains 50 equidistant entries from 34,707 indexed elements.

```powershell
D:\Rust\target\release\e3d_tty_sample_refs.exe `
  --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
  --count 50 `
  --out docs\evidence\2026-09-04-tty-ams7999-sample\refnos.txt
```

Literal output, exit status `0`:

```text
E3D_TTY_SAMPLE_OK indexed=34707 sampled=50 out=docs\evidence\2026-09-04-tty-ams7999-sample\refnos.txt
```

The first all-at-once TTY capture did not complete. E3D exited with Windows
status `3221225477`. Its TTY log contains complete end markers for samples
`0..12` and the begin marker for sample `13`; sample `13` is refno
`24383/75484`, which e3d-io physically decodes as noun `VALV`. No parity result
is claimed for this run. The next run must isolate samples in smaller sessions
and retain this refno as an explicit failing witness.

## First isolated chunk

Samples `0..9` were captured in one E3D session using
`chunk-00-refnos.txt`. Literal result, exit status `0`:

```text
TTY_ATTLIST_OK refno=16191/0
TTY_ATTLIST_OK refno=24383/67149
TTY_ATTLIST_OK refno=24383/67843
TTY_ATTLIST_OK refno=24383/68537
TTY_ATTLIST_OK refno=24383/69231
TTY_ATTLIST_OK refno=24383/69925
TTY_ATTLIST_OK refno=24383/70619
TTY_ATTLIST_OK refno=24383/71313
TTY_ATTLIST_OK refno=24383/72008
TTY_ATTLIST_OK refno=24383/72702
TTY_ATTLIST_BATCH_OK count=10
```

Each log was compared with e3d-io using the complete `schema/e3d31` pack,
the same six DICT databases recorded by the batch smoke, and physical reference
databases selected by ref0. Final aggregate:

```text
reports=10 tty_rows=379 equivalent=379 different=0 deferred=0 omitted=0
```

One real comparison defect was found and fixed during this chunk. SCTN
`24383/71313` has system `SPRE=23984/110872` and also multiple UDA rows named
`:SPREF`. The reconciler previously let the UDA overwrite the system heading.
Vendor commit `9d74754` separates the two name indexes. Adding the physical
ref0 owner `ZDJ/zdj7600_0001` resolves the system reference to the TTY value
`/CHINA-SPEC/[22b`, closing that element at `105/105`.

## Completed sample

The remaining non-crashing refs were captured in `chunk-01-refnos.txt` through
`chunk-05-refnos.txt`. All five E3D sessions exited `0`, so the completed set is
49 of the 50 deterministic samples. Refno `24383/75484` remains isolated in
`crash-witness-refnos.txt`: E3D itself exits `3221225477` while querying this
`VALV`, and no attribute-parity result is assigned to it.

Every successful capture was regenerated with vendor commit `3e40c31`, the
e3d-io E3D 3.1 schema pack, and the six physical DICT databases used by the
batch smoke. Reference names were resolved from the physical databases selected
by each ref0:

```text
13244 -> AvevaMarineSample/ams5052_0001
13245 -> AvevaMarineSample/ams5053_0001
15512 -> AvevaCatalogue/acp7320_0001
21438 -> AvevaMarineSample/ams5054_0001
23984 -> ZDJ/zdj7600_0001
```

Literal aggregate over the 49 JSON reports:

```text
reports=49 tty_rows=3123 equivalent=3119 mismatch=4
```

The four non-equivalent rows have explicit boundaries:

- `:CutLength` on ELBO `24383/76178` and `24383/85896`: the physical DICT
  defines dynamic pseudo UDA `/PFCutLengthForBend`, while the binary catalog
  default is unset. E3D runtime PML renders `41mm` and `50mm`. These are two
  runtime-derived values, not persisted binary decoder disagreements.
- `:Room` on STRU `24383/88673` and `24383/97697`: E3D returns the different
  heading `:Roomfloor unset` for `Q :Room`. The parser now preserves that full
  returned heading rather than incorrectly stripping the `:Room` prefix. These
  two rows are invalid abbreviated TTY queries, not parity claims.

The same vendor change closes five typed `IMPSTC` defaults (`INTEGER 0` in the
attlib-backed layout, rendered `Undefined` by E3D) and three multi-stage E3D
directions such as `W 5 S 0.0083 D`. Focused results were `20` TTY unit tests
and `6` `e3d-descriptor` integration tests, all passing.
