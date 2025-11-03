<!-- 78be7ca7-dd02-46e9-b546-c482d73b825f 7f6fdbe5-799b-4cf6-bb86-f52c259332aa -->
# 几何体共享机制优化

## 关注点

- 明确 `src/fast_model/export_model/export_common.rs` 中现有的几何体分组与缓存实现（geo_hash、mesh 缓存、元件记录）
- 设计自研的几何体共享与引用机制（组件模板、实例引用、变换缓存）以最大化复用
- 规划导出与运行时的数据结构、加载流程，确保共享机制高效落地

## Implementation Todos

- audit-sharing: 总结导出器当前的几何体去重与缓存流程。
- custom-sharing: 设计自定义的几何体模板与实例引用体系，减少几何体重复。
- runtime-optim: 制定运行时加载、实例化与缓存策略，使共享机制发挥效果。

### To-dos

- [ ] Summarize current geometry deduping and instancing behavior in the exporter.
- [ ] Design a grouping/export strategy to batch identical components via mesh instancing.
- [ ] List runtime-side optimizations (LOD, culling, streaming) that pair with the exporter changes.