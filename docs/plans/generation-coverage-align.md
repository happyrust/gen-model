# 开发方案：生成覆盖对齐 core.dll（路由判定名单 → dict flag）

> 决策见 `docs/adr/ADR-006`；分类器与 dict 事实见 `docs/plans/db-noun-classifier.md`、`teach/learning-records/0004`；缺口/三角定性见 `docs/plans/stage3-noun-routing-gaps.md`。本文件只写"做什么、按序、怎么验收"。

## 1. 目标与验收
- 目标：让 gen-model 的"某 noun 是否有几何/哪类几何"判定以 core.dll 的 dict flag（primitive/geomset/extrusion + 负体）为准，取代硬编码类型名单；补 catch-all 让"名单外几何"不再静默漏。
- 验收（行为对齐）：对给定变更，gen-model "产出 mesh 的 noun 集"与 dict-flag 判定一致；`routing_lists_are_dict_validated` 守护保持绿；动态覆盖审计里"dict 认几何却无 mesh"的 noun 集为空或仅剩刻意排除项。

## 2. 事实基线（本会话已确立）
- core.dll 数据驱动：primitive #659518 / geomset #859903 / extrusion #663225；直接几何(prim∪extr) 355 noun。
- gen-model：`gen_geos_data` 4 桶 + TOTAL_NEG 布尔，无 catch-all（gen_model.rs:872-886）；直接几何名单仅 77/355。
- 已落地：`NounClassifier`（primitive/geomset/extrusion/graphics_behaviour + hash_value/find_noun + 集合访问器）、守护测试、缺口报告、`noun_flags.json`(1931 noun)。
- 关键约束：primitive 广义（含管件），prim 桶须 = primitive 且 无SPRE 且 非BRAN/HANG子；目录仍以 SPRE 为准。

## 3. 分期

### 阶段 0 · 已完成（本会话）
NounClassifier + 守护（名单 ⊆ flag）+ 缺口清单 §1-5 + noun_flags.json + 三角定性（无真漏几何，上界 291/278）。

### 阶段 1 · 负体来源 + 全局分类器接入（只读，零行为变化）
- 定"负体 flag 来源"（ADR-006 未决三选一）：① RE dabacon 负体字段；② N-前缀 over 全 noun 集；③ graphicsBehaviour 映射。先做经验校验：对 TOTAL_NEG 的 26 个与 NounClassifier 全表 N-前缀交叉核对，若一致则 N-前缀足够。
- 全局 `default_noun_classifier()`（OnceLock + embed `noun_flags.json`，仿 all_attr_info 加载）。
- 派生名单接口：`primitive_nouns()/extrusion_nouns()/geomset_nouns()/negative_nouns()`（已有前三个）。
- 与现有硬编码名单交叉核对（守护已覆盖），产出差异供审。

### 阶段 2 · catch-all 告警（零行为变化，先看不改）
- `gen_geos_data` 子树查询后，对"dict 认几何(primitive∪geomset∪extrusion) 但不落任何桶（非 prim/loop/有SPRE/BRAN·HANG子）"的 noun → `warn!` 日志 + 计数上报。
- 目的：把"静默漏"变可见，收集真实运行的"名单外几何 noun"实证（轻量动态，不改生成结果）。

### 阶段 3 · 路由键迁移（direct 几何，逐桶灰度）
- prim 桶：查询过滤 `GNERAL_PRIM_NOUN_NAMES` → `classifier.primitive_nouns()` 减去 (piping ∪ use_cate ∪ 有SPRE)（避免管件误入 prim_model）。
- loop 桶：`GNERAL_LOOP_OWNER_NOUN_NAMES` → `classifier.extrusion_nouns()` 同样排除 SPRE-catalogue。
- 负体布尔：`TOTAL_NEG_NOUN_NAMES` → `classifier.negative_nouns()`。
- 每桶迁移后跑行为对齐语料 + 守护回归；差异逐条裁决。

### 阶段 4 · A* 关联几何 + 名单外负体 + 可配置
- 名单外负体（NPOLYH/NSLC…）随阶段 3 负体桶自动纳入。
- A* 关联图元等：加配置开关 `render_associated_geometry`（默认对齐 core.dll = 生成；可关以保持旧行为/减小输出）。

### 阶段 5 · 动态坐实 + 文档
- 对真实项目跑生成，确认"dict 认几何却无 mesh"的 noun 集清空（或仅剩刻意排除集）。
- 更新 teach/learning-record + 本 plan 状态。

## 4. 风险与回归
- 行为变化：更多 noun 渲染 → 输出/性能；用配置开关 + 分桶灰度 + 语料回归控制。
- 误路由：primitive 广义，必须排除 SPRE-catalogue（管件），否则 prim_model 崩；守护 + 分歧图钉死。
- 负体来源不确定 → 阶段 1 先经验校验（TOTAL_NEG vs N-前缀）。
- 性能：全局 classifier 一次加载；flag-派生名单可预计算，复用现有 query_* 零改造。
