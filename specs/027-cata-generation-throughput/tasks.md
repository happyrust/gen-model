# 027 CATA 模型生成提速任务

- [x] T01 建立 `codex/cata-generation-throughput` 与干净的 core/parse/pdms 本地依赖工作树。
- [x] T02 在 `old-aios-core/src/rs_surreal/query.rs` 和 `transform/mod.rs` 增加五个批量 API。
- [x] T03 在 `src/fast_model/gen_model.rs` 批量加载分页 BRAN/HANG 子节点。
- [x] T04 在 `src/fast_model/cata_model.rs` 实现页内预取和唯一 CATA 解析。
- [x] T05 在 `src/fast_model/concurrency.rs` 建立进程级几何额度并接入 CATA。
- [x] T06 在 `src/fast_model/manifold_bool.rs` 接入同一个几何额度。
- [x] T07 在 `src/fast_model/cata_model.rs`、`concurrency.rs` 补确定性、串行和错误回归测试。
- [ ] T08 运行 nightly 单测、无 OCC 构建和 Python offline tests。
- [x] T09 在两个新的 RocksDB 目录运行完整 8000，并写入 `docs/evidence/`。
- [x] T10 更新 `changelog.md`、`docs/2026-08-12_live-test-ledger.md` 并运行 SigMap 验证。
