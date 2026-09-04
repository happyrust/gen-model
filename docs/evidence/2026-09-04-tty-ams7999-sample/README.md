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
