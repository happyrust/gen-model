# ADR-006：生成覆盖对齐 core.dll —— 路由判定从硬编码名单迁到 dict flag

状态：提议（待确认）
日期：2026-07-24
关联：`ADR-002`（core.dll 权威范围）、`ADR-004`（分类器数据/直接解析 dabacon）、`docs/plans/db-noun-classifier.md`、`docs/plans/stage3-noun-routing-gaps.md`、`teach/learning-records/0001·0004·0005`、`vendor/aios-parse-pdms/src/dict.rs`(NounClassifier)

## 背景（对比 core.dll 的覆盖审核结论）

- **core.dll = 数据驱动**：元素是否有几何、属哪类几何，由其 noun 的 dict flag（primitive #659518 / geomset #859903 / extrusion #663225 / graphicsBehaviour 5099119）逐 noun 决定，无硬编码类型名单（见 0001/0003/0005）。dict 里 primitive 或 extrusion 的直接几何 noun 有 355 个。
- **gen-model = 硬编码名单 + SPRE 近似**：`gen_geos_data` 只有 4 桶（BRAN/HANG→cata、有SPRE→cata、GNERAL_LOOP_OWNER(9)→loop、GNERAL_PRIM(22)→prim）+ TOTAL_NEG(26) 布尔，末尾无 catch-all（gen_model.rs:872-886）。直接几何名单只列 77/355。
- **相对 core.dll 的漏点**：名单外、无 SPRE 的直接几何 noun 被静默跳过——高置信真漏 = A* 关联图元（ABOX/ACYLI/ACONE/ADISH/APYRA/AREVO…）、名单外负体（NPOLYH/NSLC，不在 TOTAL_NEG）、AID*/GENPRI/CURVE/MESH/SPINE 等（278 为名单法上界，多数学科件经 SPRE 动态兜底）。
- 本会话已落地 NounClassifier（dict flag 权威源）+ 守护测试（路由名单 ⊆ 对应 flag）+ 缺口清单/三角定性。

## 决策

1. **覆盖判定口径以 core.dll 的 dict flag 为准**：元素"是否有几何/哪类几何"由 NounClassifier 的 primitive/geomset/extrusion（+负体 flag）决定，逐步取代硬编码类型名单。
2. **目录路径保留**：SPRE→SCOM→GMSET→PARA（cata_model）已与 core.dll 的 geomset()+catalogue 一致，不改；dict flag 用于"直接几何"路径（prim/loop/负体）对齐。
3. **负体检测按 flag**：取代 TOTAL_NEG 名单（负体 flag 来源待定，见未决）。
4. **加 catch-all 告警**：重生成根子树里出现"dict 认几何、却不落任何桶"的 noun → 告警日志 + 计数（把静默漏变可见），先不强行生成。
5. **graphicsBehaviour 暂不纳入**：它决定画法风格，超出"覆盖（几何有无）"范围（且其消费非静态 switch，0005），列为未来 ADR。
6. **验收 = 行为对齐**（非逐码一致）：给定变更，gen-model "产出 mesh 的 noun 集"与 dict-flag 判定一致；守护测试保持绿；A*/名单外负体等高置信漏点由动态覆盖审计坐实。

## 关键约束（避免误路由）

- **primitive=true 是广义"设计级几何叶子"**（含管件 ELBO/VALV/TUBI…），不等于 prim_model 路由桶。迁移时 prim 桶必须是 (primitive 且 无SPRE 且 非 BRAN/HANG 子)（管件带 SPRE → 仍走 cata_model），extrusion 同理排除 SPRE-catalogue。分歧图见 stage3-noun-routing-gaps.md。
- 目录路径仍以 SPRE 存在性为准（数据驱动，天然覆盖任意学科件）；flag 只补"无 SPRE 的直接几何"。

## 未决 / 风险

- **负体 flag 来源**：已 RE 的 5 个 flag 里没有显式 negative 字段。需三选一并验证：① RE dabacon 的负体字段号；② N-前缀 over 全 noun 集（NounClassifier 全表）；③ graphicsBehaviour 枚举映射。→ 阶段 1 验证。
- **行为变化**：更多 noun 渲染 → 输出变大/性能；A* 关联图元是否该进"模型导出"是产品决策（建议可配置开关，默认对齐 core.dll = 生成）。
- **性能**：全局 NounClassifier 加载 noun_flags.json（建议 embed）；查询过滤从固定名单改为 flag-派生名单（可预计算成名单后仍用现有 query_* 接口，零查询改造）。
- **动态坐实**：278 是名单法上界；真漏集须动态跑（对真实项目比对 dict 认几何却无 mesh 的 noun）。

## 结果

- 不依赖活 E3D：dict flag 已离线解析（attlib.dat → noun_flags.json）。
- 分期实施见 `docs/plans/generation-coverage-align.md`：阶段 0 已完成（分类器+守护+缺口）；阶段 1 负体来源+全局接入；阶段 2 catch-all 告警（零行为变化）；阶段 3 路由键迁移；阶段 4 A*/名单外负体+可配置；阶段 5 动态坐实。
