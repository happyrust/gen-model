# E3D TTY batch capture smoke

## Authority and inputs

- Candidate facts: e3d-io reads `ams7999_0001` with the complete
  `vendor/e3d-io/schema/e3d31` pack and the six physical DICT databases below.
- External authority: E3D TTY `ATTLIST` output captured from project `AMS`, MDB `/ALL`.
- The legacy parser and its persisted output are not comparison inputs.
- RefNos: BRAN `24383/73930` and its first WELD member `24383/73931`.

Dictionary order:

1. `scb6002_0001`
2. `ams5100_0001`
3. `acp7043_0001`
4. `acp7006_0001`
5. `acp7323_0001`
6. `ams5101_0001`

## Capture

```powershell
D:\Rust\target\release\e3d_tty_attlist_capture.exe --project AMS --mdb /ALL --refno-file docs\evidence\2026-09-04-tty-batch-smoke\refnos.txt --out-dir docs\evidence\2026-09-04-tty-batch-smoke\logs
```

Literal output, exit status `0`:

```text
TTY_ATTLIST_OK refno=24383/73930 out=docs\evidence\2026-09-04-tty-batch-smoke\logs\24383-73930.log
TTY_ATTLIST_OK refno=24383/73931 out=docs\evidence\2026-09-04-tty-batch-smoke\logs\24383-73931.log
TTY_ATTLIST_BATCH_OK count=2
```

## Comparison

Both reports were generated with this command shape, substituting the RefNo,
TTY log and report name:

```powershell
D:\Rust\target\release\e3d-descriptor.exe extract-complete `
  --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001 `
  --schema D:\work\plant-code\old\vendor\e3d-io\schema\e3d31 `
  --uda on `
  --dictionary-db-list 'D:\AVEVA\Projects\E3D3.1\SCB\scb000\scb6002_0001;D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5100_0001;D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7043_0001;D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7006_0001;D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000\acp7323_0001;D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5101_0001' `
  --reference-db-list D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams5053_0001 `
  --refno REFNO --tty-log TTY_LOG > REPORT
```

Both invocations exited `0`. Parsed report summaries:

```text
24383/73930 total=121 equivalent=121
24383/73931 total=92 equivalent=92
```

The initial WELD run exposed six TTY rows whose physical UDA definitions use
the pseudo noun `ALLP` (`855442`) in `ELEL`. e3d-io previously required an
exact noun hash. Vendor commit `9ea5cef` treats physical `ALLP` as the all-plant
selector and pins the WELD result with the captured TTY fixture.

## Code verification

```text
cargo test --offline uda_catalog
cargo test: 5 passed, 383 filtered out (65 suites, 0.00s)
exit=0

cargo test --offline --test e3d-descriptor
cargo test: 6 passed (1 suite, 1.23s)
exit=0

cargo test --locked --bin e3d_tty_attlist_capture
test result: ok. 2 passed; 0 failed
exit=0
```

This closes the batch mechanism and the first WELD witness. It does not close
the plan's five-database random `50 ATT / 10 UDA / 10 owner` acceptance gate.
