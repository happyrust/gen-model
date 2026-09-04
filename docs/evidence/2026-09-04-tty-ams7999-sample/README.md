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
