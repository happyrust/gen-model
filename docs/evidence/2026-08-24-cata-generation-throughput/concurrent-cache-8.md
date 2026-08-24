# 8000 CATA parsed-cache / geometry concurrency run

## Fixture

- Run: `.scratch/cata-throughput/concurrent-cache-8`
- Storage: a new independent SurrealDB 2.1.4 RocksDB directory; no memory store.
- Configuration: `geometry_permits=8`, local path dependencies, no OCC feature.
- Completion boundary: literal `初始化完成：项目 AvevaMarineSample`; `pending=0` was
  captured only as a final-state observation and was not the timer stop condition.
- Command: `run-empty-init.ps1 -RunName concurrent-cache-8 -Port 8180 -GeometryPermits 8`.
- Exit status: `0`.

## Result

| Metric | Serial baseline | Cached/concurrent | Change |
|---|---:|---:|---:|
| Wall time | 1875.891 s | 808.312 s | -56.9% |
| CATA page total | 1,541,469 ms | 453,721 ms | -70.6% |
| CATA page p50 | 36,185 ms | 9,722 ms | -73.1% |
| CATA page p95 | 67,774 ms | 20,591 ms | -69.6% |
| App peak working set | 182,886,400 B | 219,222,016 B | +19.9% |

The run covered 44 CATA pages and 1,702 page-local unique identities. Parsed
`ScomInfo` is now published by SCOM refno and reused by later pages; database
cache invalidation clears the whole parsed SCOM generation because a changed
GMRE/GSTR/NGMR descendant is not necessarily keyed by its SCOM refno.

## Equivalence

- Final table counts exactly match the serial run:
  `pe=21950`, `inst_relate=2681`, `inst_info=1309`, `geo_relate=9554`,
  `inst_geo=3606`, `world_trans=0`, `aabb=4759`, `geom_error=2593`.
- The 3,555 `.mesh` files have zero path/hash mismatches. Both sorted manifest
  aggregates are
  `b9297131b3ae06dac2a022dc5812378d10379fa28e344143a5bd3b7737c78cab`.
- Canonical row hashes match for `inst_relate`, `inst_info`, `geo_relate`,
  `world_trans`, and `aabb`.
- `inst_geo` matches after sorting its set-valued `pts`; `geom_error` matches
  after excluding `first_seen_at`/`last_seen_at`. Their normalized hashes are
  respectively
  `1bcb65b1ac5905c390eaa813b2a90bfd0e4239f8dea82bcb705c59883923620d`
  and `5814b0765144c53478b86040ae4e935628902a22e2992df93af0fa9b7bb33fe1`.

## Verification records

- `parsed_scom_is_published_for_later_pages`: 1 passed, exit 0.
- `database_invalidation_drops_parsed_scom_cache`: 1 passed, exit 0.
- `cata_throughput_tests`: 2 passed, exit 0.
- `cargo fmt --check`: exit 0.
- `cargo check --locked --no-default-features
  --features ws,gen_model,manifold,project_hd,http_api`: exit 0.
- `cargo build --locked --bin aios-database --no-default-features
  --features ws,gen_model,manifold,project_hd,http_api`: exit 0.
- `cargo tree --locked`: one local `aios_core`; local parse/pdms dependencies;
  zero `opencascade`, `opencascade-sys`, or `occt-rs` matches.
- Python extension built and installed from this worktree. Offline tests reported
  `84 passed, 23 deselected, 1 failed`: the pre-existing
  `test_collect_changes_reports_both_deletes` fixture expected sessions 25 and
  26, while the current local parse dependency returned only 26. This is not
  classified as a CATA throughput pass, so T08 remains open.
- `sigmap verify-plan specs/027-cata-generation-throughput/plan.md`: exit 0.
- `sigmap verify-ai-output .scratch/cata-throughput/answer.md`: exit 0.
- `sigmap review-pr --staged`: source review completed with six test-file-name
  heuristics (the regression tests are inline Rust modules); the staged docs
  review completed with zero findings.
