# Tasks：当期项目文件夹扫描范围

- [x] T001（串行）修改 `src/data_interface/project_paths.rs` 的共享项目范围解析。
- [x] T002（串行，依赖 T001）更新 `src/data_interface/project_paths.rs` 单元测试。
- [x] T003（可并行）修订 `docs/adr/ADR-016-watch-dir-resolution-and-project-data-domain.md`、
  `docs/2026-08-04_dboption-config-changelog.md` 与 `changelog.md`。
- [x] T004（串行，依赖 T001-T003）运行格式化、定向测试、`cargo check` 与 SigMap 审查。
