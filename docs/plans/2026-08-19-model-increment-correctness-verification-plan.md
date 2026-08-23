# 模型/几何增量正确性收口计划（第二阶段，草案）

状态：**草案，待 grill 裁决**（D1–D6 全部未决，2026-08-19 起草）。
前置：`docs/plans/2026-08-16-data-increment-correctness-verification-plan.md`（第一阶段：
数据 + 层级树）。该计划 §1 把「模型/几何正确性对拍（mesh、RVM、AABB）」显式划为
第二阶段——本文就是那半边的收口计划。
证据引用均为台账 `docs/2026-08-12_live-test-ledger.md` 与 `docs/evidence/` 现有文件。

## 1. 目标与不做什么

**目标**：把「模型的增量更新正确性」验证到可宣称的程度——E3D 里做一次修改
（增/删/改/移），引擎增量落库并完成模型任务（Regen / Transform / DeleteCleanup /
按需生成）后，库侧几何事实（inst_relate / geo_relate / inst_geo / trans / aabb /
assets mesh）与 E3D 导出的几何基准在容差内一致，plant-ui 三维呈现一致。

阶段一（08-16 计划 D5b）验到「对的根进了队列且任务成功」为止；本阶段接着验
「任务做出来的几何本身对不对」。

**不做什么（本期显式出界）**：
- 数据面（pe / pe_owner / 水位 / 树）正确性——归第一阶段计划，且以其收口为
  前置（先后顺序见 D5）。
- 净窗口翻默认——独立计划已存在（`2026-08-13-net-window-default-on-development-plan.md`）。
- 房间归属线收口（issue7_e2e 重估、f8 场景）——归房间自动化测试计划与 oracle
  review；本计划的场景只顺带断言「归属不漂移」（现骨架自带），不承担房间线宣称。
- E3D 侧缺陷的修复——只决定绕过策略（沿第一阶段 D3 口径）。
- CATA 库自身变更传播（`skip_cata` 语义）——另案，沿 08-07 扩展计划 §8。
- 三角网格顶点级 diff、视觉像素级对拍——V 级只登记边界（总纲 §2 口径）。

## 2. 现状盘点（2026-08-19 审计结论）

已有资产（均有最近通过记录）：
- **静态基线对拍工具链**：`rvm_verify` import/compare（L1 成员 / L3 位置尺寸，
  `docs/2026-08-04_rvm-baseline-verification-plan.md`；L2 参数级已在该计划复核中
  裁决降为信息项，两侧几何表达不是一套）；mesh 级双向采样表面距离
  （`rvm_baseline::mesh_compare`）；装配 union 手法已解决元素边界拆分口径差
  （gen 弯头含腿 vs RVM 腿归 FTUB，08-14 判定装配无害）。
- **mesh-verify-8009 批次**：`scripts/live-batches/mesh-verify-8009.json` 共 8 用例
  （6 条带断言守卫 + 2 条取证诊断），覆盖 WALL 圆弧墙 / STWALL / GWALL（含 NXTR
  负体布尔）/ C-OR 圆管整条 BRANCH union / C-IY 槽盒；08-14 standard runner 全绿。
- **noun × 位移矩阵全绿**：`scripts/e3d/ams_model_type_cases.json` 58/58 verified
  （2026-08-19 覆盖门通过：actual=58 / manifest=58 / verified=58 / pending=0 /
  no_geometry=0）。双腿断言含水位、AABB 变化与回归、房间归属收敛——**但只到
  AABB，不到几何形状**。
- **变更形态实机**：l3_suite 8 场景与 db8000 系列——FTUB 位移/恢复、F6 OWNER
  搬移、EQUI BOX 增删（含模型清理与接口 404 断言）、会话 239 复制 STRU 几何
  法线/绕向修复后对拍通过（均 08-19 有台账记录）。
- **1112 面板几何缺口已基本闭合**：08-07 盘点的「351 块无几何面板」到 08-19
  房间重建实测只剩 1 块（按口径跳过），testbed 缺陷面板计数 0。
- 单测面：U4 模型工作计划 / U8 模型生成与空间树 / U9 删除清理 / U11 按需生成
  健康（总纲 §6）。

未收口缺口：
- **G1 增量后的几何复验缺位（本阶段主标的）**：mesh-verify 验的是「存量模型 vs
  一次性导出的 RVM 基线」的静态快照对拍；没有任何闭环把「E3D 改一次 → 增量
  重生成 → 改动根 mesh 与改后重导基线对拍 → restore 后与原基线复拍」跑通。
  增量场景（l3_suite / 模型类型矩阵）只断言到 AABB 变化与任务成功。任务级
  （阶段一）到几何级之间差的就是这一格。
- **G2 mesh 守卫覆盖面窄**：守卫只有墙族 + 两条 BRANCH；管道六大头
  （VALV / FLAN / COUP / OLET / INST / WELD，7999 已有 inst）无 mesh 守卫；
  HANG 家族（7326/7327）零生成零对拍。
- **G3 P0-1 连接容差回归未落**：`TUBI_CONNECT_TOL = 5.0mm` 已在生产判定
  （`src/fast_model/cata_model.rs`、`src/data_interface/db_model.rs`），总纲 P0-1
  规划的 5 条谓词测试在当前源码搜不到（rg 无 `joint_slack` / `connect_tolerance`
  测试名）。隐含直管段恰是增量重生成的高频路径。
- **G4 派生几何离线快照缺**（HANG / BOXI / 派生直管段）——总纲 §8 P2 挂账。
- **G5 基线刷新是手工的**：RVM 导出已可宏化 / CAF addin 化（08-04 计划 §6.5
  配方实测跑通），但「增量之后重导基线」没有一键化，G1 的闭环依赖它。

## 3. 判定口径（待决 D1）

推荐口径：
- **外部权威**：E3D 导出的 RVM（几何的机读投影）+ 宏 Q 位置/AABB 旁证（人读
  投影）。导出口径二选一（D1b）：收紧（`repre obst off / insu off`、层级只含
  LEVE≥1——08-04 首个红灯即假阳性的教训），或保留全导 + import 侧分桶豁免。
- **被验对象**：`inst_relate / geo_relate / inst_geo / trans / aabb` 与就地重建的
  gen 世界网格（`booled_id` 对齐、param 空的复合几何回退 `assets/meshes`——沿
  mesh-verify 既有口径；GWALL 大体量教训：不对齐 booled 口径会误报数百 mm）。
- **判定层级**：L1 成员（Primitive 桶严判 missing/extra=0）；L3 world 平移
  ≤1mm；mesh 双向采样表面距离按几何族分档（D3）。L2 参数级仅作信息项
  （08-04 已裁决，不可作判据）。
- **元素边界拆分口径差**用装配 union 化解，不逐元素判红（BEND 腿教训）。

## 4. 工作项（顺序草案，待 D5 裁决）

| 顺序 | 编号 | 内容 | 依赖 |
|---|---|---|---|
| 1 | W3 | P0-1 连接容差 5 条纯函数回归落地（总纲 §7 P0-1 原表照抄：0/0.66/2.70/4.18/5.0mm 不产管、6.70mm 产管、同向/被排除不产管、两调用点同谓词、3mm 实证不产管），不占 E3D 通道 | 无 |
| 2 | W0 | 基线刷新一键化：RVM 导出宏参数化（元素、输出路径，复用 08-04 §6.5 addin 配方）+ `rvm_verify import` 自动化，产出「改后重导」脚本 | E3D 通道 |
| 3 | W1 | 增量 → mesh 闭环样机：选已有基线的靶（C-OR BRAN 或 1112 WALL），E3D 位移/参数改 → 增量落库重生成 → 改动根 gen mesh vs **改后重导基线**对拍 → restore 后 vs **原基线**复拍全绿；固化为可复跑脚本 | W0 + E3D 空窗 |
| 4 | W2 | mesh 守卫扩面：VALV / FLAN / COUP / OLET 各一条 + HANG 根一条，新导基线、按族立档，进 mesh-verify 批次 | W0 |
| 5 | W4 | 删除/替换的几何侧守卫：把 EQUI BOX 增删手法固化（删除后 inst/mesh 消失 + 模型接口 404，恢复后回归基线） | W1 骨架 |

后置（本轮不做）：G4 派生几何离线快照（归总纲波次 C）；HANG 部件级长尾
（HROD/CLEV/EYRD/VSPR）；1112 剩余 1 块无几何面板的定性。

## 5. 验收标准

每个在范围场景验五项：
1. 增量批次成功、改动根模型任务完成（阶段一口径，作前置门，不重复宣称）；
2. 改动根 gen mesh vs 改后基线：L1 Primitive 桶 missing/extra=0、L3 平移 ≤1mm、
   表面距离在该几何族档位内；
3. restore 后 vs 原基线复拍全绿（幂等半边）；
4. AABB / 空间树 / plant-ui 三维呈现一致（呈现层验证，复用现有断言，不当权威）；
5. mesh-verify 批次扩面后 standard runner 可复跑全绿。

另：跑绿即回写台账与 manifest（不回写视为没跑，沿 noun 矩阵纪律）；改动过
`cargo fmt` + `cargo check`；每条新守卫给出「回退即红」说明（总纲 §13 纪律）。

## 6. 待决问题（grill 清单）

- **D1 判定权威**：改后基线由 E3D 重导 RVM 充当？（推荐：是——与第一阶段
  「文件本身为唯一外部权威」同构，RVM 就是几何侧的文件投影）
  **D1b** 导出口径收紧还是全导+豁免？（推荐：收紧——闭环本来就要重跑导出宏，
  改口径零边际成本，且消灭 08-04 那类假阳性）
- **D2 场景范围**：本轮以 W0–W4 为界？六大头 + HANG 根算本轮、HANG 部件长尾
  下轮？（推荐：是）
- **D3 容差档位**：按 08-14 实测立档（盒状 p95≤1mm、圆弧墙 p95≤12mm、管路
  union p95≤10 / max≤30mm），后续逐步收紧？（推荐：是——先有档再收，不空转）
- **D4 环境**：E3D 侧用 test-increment 隔离副本还是正式项目？（推荐：隔离副本
  ——第一阶段 W3 直接改写正式项目的教训就是 test-increment 存在的理由）
- **D5 与数据线的顺序**：等 08-16 计划 W1–W3 收口再开工，还是 W3/W0 这类
  不占 E3D 通道、不依赖数据线结论的项先行？（推荐：W3 立即可做、W0 找通道
  空窗做；W1 起等数据线收口，避免在 merged_sesnos 回归未闭时叠加变量）
- **D6 提交策略**：沿第一阶段 D6——live 通过、台账更新后连改动一并提交，
  不把未验资产入库？（推荐：是）

## 7. 裁决记录

（待 grill，逐条回填。）

## 8. 关联文档

- `docs/plans/2026-08-16-data-increment-correctness-verification-plan.md`：第一阶段（数据线）
- `docs/2026-08-04_rvm-baseline-verification-plan.md`：rvm_verify 工具、导出配方与假阳性教训
- `scripts/live-batches/mesh-verify-8009.json`：现役 mesh 守卫批次
- `docs/plans/2026-08-07-e3d-model-type-increment-verification-expansion-plan.md` 与
  `docs/e3d-model-type-verification-ledger.md`：noun × 位移矩阵
- `docs/2026-08-06_model-increment-unit-test-plan.md`：单元测试总纲（P0-1、§8 P2、§16 宣称边界）
- `docs/2026-08-12_live-test-ledger.md`：live / E3D 唯一执行台账
