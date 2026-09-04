# AMS 7351 CATA E3D TTY sample

## Authority

- Candidate values are read by e3d-io from physical `ams7351_0001`, the
  e3d31 `catvir.dat`/`attlib.dat` schema, and the six MDB-ordered physical DICT
  files named in `verify.ps1`.
- External values are read-only E3D TTY `ATTLIST` captures from `AMS /ALL`.
- Legacy parser output and persisted legacy snapshots are not inputs.

The physical file SHA-256 is
`1208f44e1a822a60df63ed8d897b9944cd4993ba0e884a78194abb0f2a7feac0`.
Its live index contains 3,345,855 elements. The deterministic sampler selected
50 equidistant refnos; five ten-element E3D sessions all exited successfully.

```powershell
D:\Rust\target\release\e3d_tty_sample_refs.exe --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7351_0001 --count 50 --out docs\evidence\2026-09-04-tty-ams7351-sample\refnos.txt
powershell -NoProfile -ExecutionPolicy Bypass -File docs\evidence\2026-09-04-tty-ams7351-sample\capture.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File docs\evidence\2026-09-04-tty-ams7351-sample\verify.ps1
```

## Result

The initial comparison exposed 31 `DATA.PPRO` mismatches. Their implicit tuple
is zero, while the physical explicit stream carries a PML program under the
two's-complement-negated `PPRO` hash. Example `23735/201571` stores
`[7, 1, 118, 3, 52, 56, 52]`, which E3D renders as `( '484' )`.

e3d-io commit `958b5194fae51a604c87ac6234ed476f28d9da6e` adds the observed
opcode-118 text literal to the existing fail-closed PML decoder. Re-running the
same reports produced:

```text
reports    : 50
tty_rows   : 737
equivalent : 737
mismatch   : 0
deferred   : 0
omitted    : 0
```

The 50-attribute CATA gate is closed. The broader per-database 10-UDA and
10-owner/member gates remain separate: this uniform sample contains UDA rows
on only three elements, and the existing direct-tree comparator is DESI-only.

