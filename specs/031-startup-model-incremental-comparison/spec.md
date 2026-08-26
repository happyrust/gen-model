# Feature Specification: startup model incremental comparison

**Feature Branch**: `codex/libgm-primitive-caliber`  
**Created**: 2026-08-25  
**Status**: Accepted  
**Decision**: `docs/adr/ADR-051-model-switches-do-not-request-startup-full-build.md`

## Goal

Keep model and mesh processing enabled while ensuring service startup only
processes work justified by the authoritative file-session/watermark comparison.

## Requirements

1. `gen_model=true` or `gen_mesh=true` MUST NOT invoke a whole-db build during
   `run_cli` startup.
2. Startup MUST continue through `init_watcher`, `scan_and_check_file`, and
   `discover_batch`, comparing `file_latest_sesno` with `applied_sesno`.
3. First import, file rollback/reinit, and real incremental windows MUST retain
   their existing queue behavior.
4. Once data is ready, the model gate MUST open for work produced by those
   batches; no-change startup MUST settle with no model generation.
5. Explicit full-build callers outside service startup MUST remain available.
6. Startup output MUST state that model switches enable the incremental model
   stage and do not request a whole-db build.

## Acceptance

- Source regression proves `run_cli` contains none of `is_gen_mesh_or_model`,
  `gen_all_geos_data(&db_option)`, or `begin_full_model`.
- Existing initialization ordering still proves data-ready precedes model-ready
  and room processing.
- With deployed `gen_model=true`, `gen_mesh=true`, `manual_db_nums=[8000]`, and
  unchanged file sessions, restart logs contain the incremental strategy notice
  and contain no `正在生成模型` / `开始8000的模型生成`.
