# E3D global database type audit (2026-09-04)

## Authority and scope

- Candidate facts come from e3d-io database-header and element decoding with the installed
  `attlib.dat` plus the matching `*vir.dat` file.
- The only external value oracle attempted here is E3D TTY `ATTLIST`; no legacy parser output
  participates in this result.
- Scanned the immediate files in all eight `*000` database directories below
  `D:\AVEVA\Projects\E3D3.1`: 1,067 files total.

## Header result

e3d-io found no physical database whose header type is `GLB` or `GLOB`. The global files in the
installed projects are instead `COMM` and `MISC`:

| Header type | Files |
|---|---:|
| `COMM` | 9 |
| `MISC` | 8 |
| `SYST` | 8 |
| `GLB` | 0 |
| `GLOB` | 0 |

Representative AMS files:

| File | e3d-io header | Matching template |
|---|---|---|
| `ams000\amscom` | `dbnum=8190`, `db_type=COMM`, `latest_ses_pgno=4` | `comvir.dat` |
| `ams000\amsmis` | `dbnum=8189`, `db_type=MISC`, `latest_ses_pgno=4` | `misvir.dat` |

The scan used the committed `e3d-descriptor header --db <path>` command, which opens the file via
`ReadOnlyEngine` and reports its parsed `DbDescriptor`.

## Binary extraction

The physical sampler selected these witnesses:

- `COMM 16382/3` from `amscom`: wide record, 69 implicit words.
- `MISW 16381/2` from `amsmis`: wide record, 76 implicit words.

`e3d-descriptor extract` then used `comvir.dat` / `misvir.dat`, the installed `attlib.dat`, and the
committed noun catalog. Re-readable outputs:

- `2026-09-04-global-comm-16382-3/e3d-io-extract.json`: 21 attributes; 15 `decoded`, 4
  `decoded_explicit`, 2 `non_implicit`.
- `2026-09-04-global-misw-16381-2/e3d-io-extract.json`: 3 attributes; 2 `decoded`, 1
  `decoded_explicit`.

Both extraction commands exited 0.

## E3D TTY boundary

`e3d_tty_attlist_capture --project AMS --mdb /ALL --refno 16382/3` was attempted twice. Both runs
reached the command loop, then the E3D process exited `3221225477` while selecting the COMM refno;
the macro never emitted `MCP-END`. The capture command therefore exited 1 and its empty output is
not an equivalence result. This matches the observed boundary that these global files are not
navigable from the Design `/ALL` MDB.

Accordingly, this audit closes the database-type and template-routing fact, and keeps external TTY
attribute parity open. `GLB/GLOB` remain accepted for installations that physically use those
header types; this installation's `COMM/MISC` types must also follow the Meta path.
