# 模型加载层级查询优化：inst_relate 祖先链（u64）+ dbnum 开发方案

日期：2026-08-07
状态：待评审
牵涉仓库：gen-model（写入侧 + schema）、plant-ui（读侧，vendor/rs-core）

## 1. 背景与问题

plant-ui 全场景模型冷加载在 AvevaMarineSample / MDB ALL 上实测 **88 秒**
（`plant-ui/crates/plant-ui-data/src/lib.rs:52-70` 注释里的实测账）：

| 阶段 | 耗时 | 占比 |
|---|---|---|
| `query_deep_visible_inst_refnos`（19 个根串行） | 69.9 s | 79% |
| `query_insts`（107 批 × 500） | 15.9 s | 18% |
| `query_filter_deep_children` | 2.2 s | 2.5% |

慢的根源不是数据量（inst_relate 全表仅 3.8 万行），而是查询形态：

- 「根 → 子树全部可见几何」被做成 **12 层 `<-pe_owner<-` 向下图遍历**
  （`rs-core/src/rs_surreal/graph.rs:33`），每根还要跑**两遍**（BRAN/HANG 一遍、
  可见类型一遍，`rs-core/src/rs_surreal/geom.rs:55-73`）；
- 遍历结果再把**几万个 pe key 内联进 SQL 文本**做 IN 过滤（`graph.rs:106-131`），
  几 MB 的 SQL 让服务端解析；
- 这套查询在 gen-model 增量生成写库期间还会与写入互挤（同一台
  fork surreal 2.1.4 @ localhost:8009），实测把「展开一层树」拖到 65 秒。

而这本质上是一个**简单层级查询**：给一个根，取子树下全部可见几何实例。

## 2. 方案核心

**写入时物化祖先链，查询时一条索引查询完事。**

给 `inst_relate`（含 TUBI 行）与 `tubi_relate` 增加两个写入时计算的字段：

| 字段 | 类型 | 含义 |
|---|---|---|
| `anc` | `array<int>`（RefU64 打包值） | 自身 → SITE 的完整祖先链（含自身） |
| `dbnum` | `int` | 该元素所属设计库号 |

读侧任意根的全部可见实例变成一条查询（直接产出 `GeomInstQuery`，
同时替代 `query_deep_visible_inst_refnos` **和** 分批 `query_insts`）：

```sql
SELECT in.id AS refno, in.old_pe AS old_refno, in.owner AS owner, generic,
       aabb.d AS world_aabb, world_trans.d AS world_trans, out.ptset.d.pt AS pts,
       (SELECT trans.d AS transform, record::id(out) AS geo_hash
          FROM out->geo_relate
         WHERE visible && out.meshed && trans.d != none && geo_type='Pos') AS insts,
       booled_id != none AS has_neg, dt AS date
  FROM inst_relate
 WHERE anc CONTAINS $root AND aabb.d != none
```

`$root` 直接绑 u64 数字。tubi 同治：`WHERE anc CONTAINS $root AND leave.id != none`，
不再需要「先深遍历找子树全部 BRAN」。

### 既有先例与现成缺陷（方案的地基）

1. `inst_relate` 建行时已经写 `zone_refno: fn::find_ancestor_type(refno,'ZONE')`
   （gen-model `src/fast_model/pdms_inst.rs:298`）——「写入时物化祖先」这条路
   已被 ZONE 路径验证可行，本方案是把它推广到任意根。
2. 向上走链本来就便宜：`pe.owner` 是记录链接，`fn::ancestor` 就是 9 跳
   `owner.owner...`（rs-core `resource/surreal/common.surql:6`）。贵的只是向下边遍历。
3. **F1 缺陷顺手修**：`idx_inst_relate_zone_refno` 的 `TYPE BTREE` 在 fork 2.1.4
   语法非法，错误被 `let _ =` 吞掉，**索引生产从未建成**
   （`docs/2026-08-05_fork-surreal-compat-findings.md`）。

### 为什么 anc 用 u64 而不是 record 链接（决策记录）

1. `RefU64(pub u64)` 是全客户端通用键，查询参数零转换，省掉 `to_pe_key` 拼接；
2. 彻底绕开 record id 形制雷区：F4 已证明 pe 的 id 形制不统一
   （字符串 id 与历史数组 id 并存），record 链接的 anc 会继承这份混乱；
3. 数字数组索引比 record id（字符串比较 + ⟨⟩ 转义）更小更快；
4. 写入侧更省：gen-model 解析库文件时 owner 链就在内存
   （`refno_table_map`/`EleDataEntry`），Rust 直接算好写入，
   连现在每行一次的服务端 `fn::find_ancestor_type` 调用都退役。

**边界约束**：SurrealDB int 是 i64。refno 打包 = high<<32|low，高位实测两万级
（24383/17496），距 2^31 很远；写入侧仍加断言，越界报错、不静默截断。

**范围排除**：pe 表 record id 迁移 u64 不在本方案内。影响面为全部边表、
几十个 fn::（F4 的 `array::at` 假设即现成教训）、his_pe 版本 id 与暂存管线
SQL，收益配不上风险，另行立项评估。

## 3. 分期任务

### P0 验证地基（约半天）——已完成，判定 **Go**（2026-08-07）

- [x] 修 F1：`DEFINE INDEX IF NOT EXISTS idx_inst_relate_zone_refno ON TABLE inst_relate COLUMNS zone_refno;`
      （去 `TYPE BTREE`），生产与暂存共用常量 `INST_RELATE_ZONE_INDEX_SQL`
      （`src/fast_model/pdms_inst.rs`、`src/data_interface/staging/lifecycle.rs`）；
      吞错改为显式上抛，`run_cli` 调用点原有 eprintln 兜底不变。
- [x] 双跑套件新增用例 `dual_inst_relate_anc_u64_contains_index_agrees`：
  - 数组列普通索引 DEFINE 两引擎合法建成；
  - `anc CONTAINS <u64>` EXPLAIN **走 `idx_ir_anc`**（两引擎绝对断言通过）；
  - `dbnum = n` EXPLAIN 走 `idx_ir_dbnum`（备选路线地板同时成立）；
  - `CONTAINSANY` 多根合并查两引擎一致；`i64::MAX` 边界往返保真。
- [x] **Go/No-Go 判定：Go，主路线成立**。`AIOS_COMPAT_REQUIRE=1` 下全套 11 条
      用例全绿；结论已记入 `docs/2026-08-05_fork-surreal-compat-findings.md`
      2026-08-07 增补节。

### P1 写入侧（gen-model，约 1 天）——已完成（2026-08-07）

- [x] `inst_relate` 行构造增写 `anc` + `dbnum`
      （`src/fast_model/pdms_inst.rs` `save_instance_data`，普通行与 TUBI 行都写；
      TUBI 行顺手补上了历史上一直缺的 `zone_refno`；
      `zone_refno` 保留，读侧切换完成后再退役）；
- [x] `tubi_relate` 边构造同款（`src/fast_model/cata_model.rs` 三处 RELATE 写入点，
      anc/dbnum 取自 BRAN）；
- [x] **实现偏差（as-built）**：anc/dbnum 不走 Rust 内存 owner 链，改为服务端
      写入时计算——`fn::anc_u64`（12 跳 `pe.owner` 上溯 + RefU64 打包，滤
      NONE/pe:0_0 哨兵）+ `fn::refno_u64`（兼容历史数组 id），定义在本仓
      `resource/surreal/common.surql`（`DEFINE FUNCTION OVERWRITE`）。
      理由：与既有 `zone_refno: fn::find_ancestor_type(...)` 同模式，天然覆盖
      全部调用方（全量/增量/手动 ensure），且暂存 journal 写回重放时在持久层
      按活链重新求值。dbnum 直接取 `pe.dbnum` 字段（pe 行自带且有索引）。
      2.1.4 上 OVERWRITE 语法 + 闭包执行由双跑用例
      `dual_anc_u64_functions_execute_and_agree` 钉住（含生产字面量形态端到端）；
- [x] 存量回填工具：`pdms_inst::backfill_inst_relate_anc`（每轮圈 `anc = NONE`
      的 2000 行批量重算，幂等自愈），挂在 `run_cli` 启动序列
      （`src/lib.rs`，索引初始化之后）；
- [x] OWNER 搬家维护：重生成路径天然自洽；容器级搬家（PIPE/ZONE 改挂 OWNER，
      不产生模型工作项）由 `IncrementPipeline::moved_refnos`（`ChangeBucket::Moved`
      口径）圈出，`render_anc_repair_statements` 渲染 `anc CONTAINS 搬家元素`
      的定点重算（连 `zone_refno` 一起修——其陈旧是既有隐性 bug），语句并入
      finalize 事务 `window_statements`，与水位共命运；单测
      `owner_moves_render_anc_repair_statements_and_others_do_not` 钉住；
- [x] schema DEFINE 落两处：`INST_RELATE_INDEX_SQL` 常量（zone_refno/anc/dbnum
      × inst_relate + anc/dbnum × tubi_relate 共 5 条索引）为唯一事实来源，
      生产启动（`init_inst_relate_indices`）与 `init_staging_schema` 共用。
- 验证：`AIOS_COMPAT_REQUIRE=1` 双跑套件 12/12 全绿；`cargo test --lib` 475 通过。

### P2 前置验证：SurrealQL 模拟性能对比（2026-08-07，bench 已入库）

手动 bench `bench_anc_contains_vs_deep_traversal`（`src/test/fork_surreal_compat.rs`，
`--ignored`，可复跑）：同一台 fork rocksdb 服务器（生产同款 release 二进制）、
AMS 量级合成树（19 SITE / 63,860 pe / 40,850 inst_relate），旧读路径 1:1 复刻
vendor/rs-core 查询形态，逐根断言新旧 refno 集合完全一致（全部通过）。

| 项 | 实测 |
|---|---|
| 旧路径合计（19 根串行） | **346.5 s** |
| ├─ 12 层深遍历 ×2 | 16.2 s |
| ├─ 巨型 IN 内联过滤 ×2 | 2.2 s（单条 SQL 峰值 ~50 KB；子树更大时到 MB 级） |
| ├─ 每 BRAN 一次子查询（19,950 次往返） | **310.5 s**（占 90%） |
| └─ query_insts（500/批投影） | 17.5 s |
| 新路径（19 条 `anc CONTAINS`，串行） | **16.4 s**（21.1×） |
| 新路径（`CONTAINSANY` 19 根一把查） | 19.4 s（17.9×；单次大结果集序列化，不如逐根） |

**校准与推论**：
- 合成 query_insts 17.5 s ≈ AMS 实测 15.9 s——投影成本对得上，模拟可信；
  合成 BRAN 数（19,950）偏多放大了旧路径子查询项，AMS 上旧路径实测 88 s 同构成立。
- 新路径耗时 ≈ 旧路径的 query_insts 分量：**解析成本（69.9 s 一档）被消灭**，
  剩下的地板是 GeomInstQuery 投影本身（每行 geo_relate 子查询 + aabb/trans 解引用，
  本机约 2.4 k 行/s）。
- 落地口径：整场 88 s → 串行 ~16 s；**P2 必须做根间并发**（8 路 → 2-4 s 达标）；
  `CONTAINSANY` 合并不可取（慢于逐根且不可并发）。若并发后仍不达 ≤3 s，
  下一档优化是投影瘦身（pts / insts 子查询拆出或按需加载）。

### P2 读侧（plant-ui vendor/rs-core，约 1 天）——已完成，AMS 实库验收通过（2026-08-07）

- [x] `vendor/rs-core/src/rs_surreal/inst.rs` 新增：
      `query_inst_refnos_by_root_anc(root)`（`anc CONTAINS` 索引查询解出子树
      实例 refno 列表，根类型无关）、`query_bran_refnos_by_root_anc(root)`
      （tubi 同款，`tubi_relate` 上解出带直管段的支管列表）、
      `inst_relate_anc_ready()`（回填覆盖探针，空库视作就绪）；
- [x] **实现教训（as-built，与 §2 草图的偏差）**：anc 查询**只做解析、只回
      id 列表**，投影仍走既有分批 `query_insts` / `query_tubi_insts_by_brans`
      （500/批）。整根全投影一条响应的形态在 AMS 实库（41 根 / 5.5 万实例）
      上直接把 WS 连接打死（单条超大消息 → `receiving from an empty and
      closed channel`）——旧路径的 500/批分块正是在守这条线，新路径继承之。
      id 列表载荷百 KB 级封顶，无此风险，且 bench 已证明新路径耗时地板本来
      就是投影本身，此改法不损失收益；
- [x] `model_instances_with_progress`（`crates/plant-ui-data/src/lib.rs`）切换：
      自动选路——anc 已回填 → `model_instances_anc`（每根解析 + 分块投影，
      `buffered(8)` 根间并发、保输入序），未回填/探测失败 → 旧路径并打一行
      提示（部署顺序安全）。SITE→ZONE 中转、辨名词、深遍历在新路径全部消失；
      tubi 换装收口为共用的 `tubi_to_geom`；
- [x] 旧路径开关：运行时环境变量 `PLANT_UI_LEGACY_MODEL_QUERY=1`（现场可回退，
      不用重编译），旧实现保留为 `model_instances_legacy`（pub，对拍基线），
      一个版本后随开关退役；
- [x] 对拍验收：`crates/plant-ui-data/tests/anc_model_query_parity.rs`
      （ignored live）——refno 集合 + 每 refno 网格 hash 集双口径。
      **AMS 实库实测通过（2026-08-07）**：41 SITE 根 / 51,422 实例，
      新旧集合完全一致；**旧路径 151.9 s → 新路径 16.8 s（9.0×）**。
      （库已比 §1 画像时长大：当年 19 根 / 3.8 万实例 / 88 s。）
- [x] 部署步已固化：gen-model `pdms_inst::tests::live_backfill_anc_on_configured_db`
      （ignored live）——灌两函数 + 建索引 + 幂等回填一键完成，AMS 实库
      55,021 行 inst_relate + 93 行 tubi_relate 回填 34.4 s，无残留 NONE。

**验收口径修订**：`≤3 s` 原目标按「解析归零后整场只剩毫秒级」估计，实测投影
本身（每行 geo_relate 子查询 + aabb/trans 解引用）就是 ~16 s 的服务端地板
（8 路并发下服务端 CPU 已打满，加并发不再线性提速）。88 s（今 152 s）中
**解析那一档已消灭**；要进 3 s 档需投影瘦身（pts / insts 子查询拆出、或写入时
物化 insts 数组），列为 P3 后续优化项，不阻塞本方案收尾。

### P2+ 投影瘦身（2026-08-07 探索结论，slim 版已落地）

**实库成本画像**（AMS 整表 53,582 行，逐句探针，扣除 CLI 基线 ~0.6 s）：

| 投影成分 | 净耗时 | 说明 |
|---|---|---|
| 平表扫描（无解引用） | 0.7 s | 物化后的理论地板 |
| + `aabb.d` / `world_trans.d` 解引用 | +2.0 s | 每行 2 次点查 |
| + `insts` 子查询 | +8.7 s | **大头**：图跳 + 每边 trans/meshed 解引用 |
| + `in.id`/`in.old_pe`/`in.owner`/ptset/dt | +3.0 s | **可砍**：UI 全链路无消费者 |

**去重批查此路不通**（实测锤死）：aabb / world_trans / inst_info 是世界坐标系
数据，天然每实例一份（distinct 51,425 / 50,711 / 52,827 vs 55,021 行），
按 distinct 分面批查无收益。

- [x] **slim 版已落地**：`query_insts_slim`（vendor/rs-core）——只投影 UI 消费
      的字段，refno 直接取边上 `in` 链接不解引用，owner 由 `anc[1]` 还原，
      old_refno/pts/dt/has_neg 缺省；anc 路径切用。AMS 实测整场
      **16.8 s → 13.9 s**；对拍新增 owner 口径（anc[1] vs 实时 `in.owner`
      逐行相符——顺带证明存量 anc 链新鲜）。相对旧路径基线（151.9 s）**11×**。
- [ ] **3 s 档 = 写时物化（P4 候选，暂缓）**：inst_relate 行内物化
      `aabb_d(6)` / `wt_d(16)` / `insts_flat[{h,t}]`，读侧变纯平表扫描
      （预计整场 ~2 s）。维护点五处：建行内联（d 值在内存）、
      `gen_inst_meshes` 置 meshed 的同批语句反向刷 `insts_flat`
      （需 `idx_inst_relate_out`）、transform 便宜路径换链接时同步刷、
      `update_inst_relate_aabbs_by_refnos` 同步、存量回填。
      **暂缓原因**：与并行推进的「暂存祖先解析式预载」W4（同在
      `save_instance_data` 字面量上做 resolve-then-render）正面相撞，
      须待其落地后实施或与之同批。

### P3 清理与衔接（半天）

- [ ] 删旧深遍历路径与开关；`fn::find_ancestor_type` 在 inst 写入链上的调用退役；
- [ ] 策略层衔接（独立小改动，不阻塞本方案）：
  - plant-ui 按 `dbnum` 补丁式刷新：数据批次终态只重查该库的模型
    （`WHERE dbnum = $n`），替代整场替换；
  - 「采纳而非丢弃」：重载查询期间新来任务时不再扔结果
    （`plant-ui-app/src/main.rs:1233-1244`），改记增量欠账。

## 4. 验收口径

| 场景 | 现状 | 目标 |
|---|---|---|
| AMS / MDB ALL 全场景冷查询（19 根 / 3.8 万实例） | 88 s | ≤ 3 s |
| 单 BRAN 点眼睛显示（空闲时） | 数秒级 | ≤ 1 s |
| 新旧路径 refno 集合对拍 | — | 完全一致 |
| gen-model 生成期间的交互查询 | 最差 65 s/层 | 单个读查询降两个数量级（写入互挤仍在，另属基础设施议题） |

## 5. 影响面

| 仓库 | 改动 | 不动 |
|---|---|---|
| gen-model | pdms_inst.rs、cata_model.rs、staging/{lifecycle,executor}.rs、回填工具、双跑套件 | pe 表 id 形制、增量管线主体、水位/队列语义 |
| plant-ui | vendor/rs-core inst.rs + plant-ui-data lib.rs | 树/属性查询路径、View3d、gen-model 的 git 依赖 aios_core（读查询只进 vendor） |

两份 rs-core 互不牵连：写侧改的是 gen-model 本仓文件，读侧只改 plant-ui 的 vendor。

## 6. 风险与备选

| 风险 | 概率 | 对策 |
|---|---|---|
| fork 2.1.4 数组列索引 / CONTAINS 不走索引 | 中 | P0 先验。备选：固定标量列 `site_r / zone_r / unit_r / unit_owner_r`（u64）各建普通索引，查询按根类型选列——zone_refno 模式已验证此路可通，代价是多列与根类型分派 |
| 即使全表扫（索引不可用且备选未落） | — | 3.8 万行单遍扫描也比 12 层遍历快两个数量级，方案地板值仍成立 |
| 回填期间读到 anc 缺失行 | 低 | 读侧查询加 `AND anc != NONE` 兜底 + 回填完成前不切默认路径 |
| OWNER 搬家漏重算 | 低 | staged 提交尾定点重算 + 对拍测试覆盖搬家用例 |
| F4（update_dbnum_event 与 UPSERT 冲突） | 无关 | inst_relate/tubi_relate 不在 pe 表上，不触发该事件 |

## 7. 工作量汇总

P0 半天 + P1 一天 + P2 一天 + P3 半天 ≈ **3 人日**（不含 AMS 全库回填运行时间，
回填可夜间离线跑）。
