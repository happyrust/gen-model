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
      **2026-08-07 审核修复 P2**：重算语句只对 **DESI 窗口**渲染
      （`anc_repair_statements_for_window`，与 datacenter 语句同门）——anc 只含
      设计元素链，CATA/SYST 搬迁渲出的 UPDATE 全是收口事务里的空转子查询扫描
      （目录重组一次搬上千元素会拖慢收口一个量级），且其 `fn::anc_u64` 依赖
      只受 DESI 预检保护；钉子 `anc_repair_is_rendered_for_desi_windows_only`；
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
### P4 写时物化（2026-08-07 设计定稿并落地，as-built 见节末）

目标：读投影的两档解引用成本（aabb/trans +2.0s、insts 子查询 +8.7s）在写入时
付掉，读侧变纯平表投影，整场 13.9s → ~2s。

**行内新字段（inst_relate）**：

| 字段 | 内容 | 维护纪律 |
|---|---|---|
| `aabb_d` | `aabb.d` 的行内副本（serde 同形 JSON） | 与 aabb 指针**同语句**写，永不分离 |
| `world_trans_d` | `world_trans.d` 的行内副本 | 与 world_trans 指针**同语句**写 |
| `insts_flat` | 读投影 insts 子查询结果的**派生缓存** `[{transform, geo_hash}]` | 不进 journal；持久层清扫维护；读侧兜底 |

**写点与 journal 纯数据纪律（不破 W4）**：

1. **建行**（`save_instance_data`）：普通行 +`world_trans_d`（值在内存渲染纯字
   面量）；TUBI 行 +`aabb_d`+`world_trans_d`（建行即带 aabb）；`insts_flat`
   不写（建行时 meshed 状态未知，宁缺毋错）。
2. **aabb 刷新**（`update_inst_relate_aabbs_by_refnos`）：指针 UPDATE 同语句
   追加 `aabb_d = <字面量>`（computed 在内存）。指针回退分支（TUBI 等）不写
   ——值未变，建行时已置。
3. **transform 便宜路径**（`refresh_world_transform_products`）：指针 UPDATE
   同语句追加 `world_trans_d = <字面量>`（world_transform 在内存）；aabb_d
   由该函数末尾的 aabb 刷新到位。
4. **清扫**（`sweep_inst_relate_flat`，持久层非 journal，与
   `backfill_inst_relate_anc` 同族）：批量圈 `insts_flat = NONE AND
   aabb.d != none` → `SET insts_flat = (insts 子查询), aabb_d = aabb.d,
   world_trans_d = world_trans.d`。挂两处：启动序列（anc 回填后；存量回填 =
   首轮全量）+ 批次 worker 空闲轮（脏位门控，生成/刷新过才扫）。
5. ~~`gen_inst_meshes` 置 meshed 反向刷 + `idx_inst_relate_out`~~（原草图项，
   **取消**）：置 meshed 的生成批与建行同任务同 refno 锚点，任务成功 ⇒ 可达
   geo 全部 meshed|bad，清扫按 refno 收口即可；「共享 geo 迟到 meshed 使他行
   already-materialized 的 insts_flat 变旧」的路径不存在（他行成功过 ⇒ 其 geo
   已 meshed|bad；失败过 ⇒ 行随重试重建回 NONE）。反向索引不再需要。

**一致性论证**：`aabb_d`/`world_trans_d` 与指针同语句原子写 → 无「指针新副本
旧」窗口；`insts_flat` 只会「缺」（NONE）不会「错」——缺由读侧兜底 + 清扫自愈
（崩溃窗口同理）。journal 里只有渲染期纯字面量，写回重放零求值（W4 源码钉不受
影响）。

**读侧（plant-ui）两段式**：解析（`anc CONTAINS`，纯索引扫描）→ pass1 平表
投影 `in as refno, anc, generic, aabb != NONE as has_aabb, aabb_d,
world_trans_d, insts_flat`（零解引用零子查询）→ 客户端三分法：副本齐活直接
成型；仅 aabb 链接在而副本缺（清扫未及/pre-P4 存量）聚拢 pass2 走
`query_insts_slim` 现值兜底；连链接都没有的行丢弃。**正确性不依赖物化覆盖
率**，覆盖率只买速度；不依赖 `??` 合并算子的短路语义（2.1.4 上未验证）。

**验收判据**：双跑用例钉三条语句形态在 2.1.4 双引擎一致（建行字面量、指针+
副本同语句 UPDATE、清扫语句含 UPDATE SET 里 `out->geo_relate` 遍历）；AMS
清扫全表后 flat 读与 slim 读对拍（refno/owner/aabb/trans/insts 哈希五口径）
一致，整场 ≤3s 判定达标。

**落地情况（2026-08-07，as-built）**：

- [x] gen-model 写侧五点全部落地；双跑新用例
      `dual_inst_relate_flat_materialization_agrees` 一次通过（UPDATE SET 值位
      里的 `out->geo_relate` 图遍历在 2.1.4 双引擎成立，平表投影 == 解引用投影
      逐字段相等）；双跑 13/13、lib 498/498 全绿。
- [x] AMS 存量清扫（`live_sweep_inst_relate_flat_on_configured_db`）：
      53,582 行一次付清，51.4s，覆盖复核无残留。
- [x] plant-ui 读侧两段式落地；**最终验收（release，AMS 41 根 / 51,423 行）：
      旧路径 253.3s → 新路径 2.73s（93×，≤3s 达标）**，refno/owner/网格 hash
      五口径完全一致（51,371 唯一实例）。
- **as-built 偏差三处**：字段名 `wt_d` → `world_trans_d`（可读性）；解析查询
  去掉 `and aabb.d != none`（原是每命中行一次点查，~2s——可见性判定挪进读侧
  三分法，最终集合与旧口径逐行一致）；原草图的「置 meshed 反向刷 +
  `idx_inst_relate_out`」取消（见上文第 5 点论证）。
- **读侧压秒历程（release 口径，13.9s slim → 2.7s，四步各有教训）**：
  1. 解析层零解引用（`in.id`→`in` 省每行 pe 点查、砍 `aabb.d != none` 谓词）
     + 平表投影只取 `anc[1]`：5.4s → 3.9s；
  2. 批大小 500→1500：往返省一半但**每行序列化成本不变**，收益有限；
  3. **单条 WS 连接的响应流是串行管道**——根间 8 路"并发"下平表阶段 wall
     几乎等于各批串行之和；加 4 条只读连接池（rs-core `flat_read_db`，惰性
     并行握手 + `prewarm_flat_read_pool` 启动预热——signin 单条 ~2s，串行建
     池曾造成首轮 12s 尖刺）；
  4. **根偏斜让根级并发白并**——巨型 SITE 的十几个批在自己根的 future 里
     串成尾巴；改全局块队列 + `tokio::spawn` 真任务（`buffered` 是单任务轮询，
     51k 行反序列化 ~56µs/行 会挤在一个线程），并行地板实测 1.7s。
- **成本剖面（AMS 实测）**：单根解析 2,035 行 43ms（EXPLAIN 走
  `idx_inst_relate_anc`）；整表平表投影 count 口径 ~0.7s、全并行含载荷 1.7s。
  `CONTAINSANY` 41 根实测 55.8s，再次证实逐根 CONTAINS 是唯一正解；整表单
  响应投影再次撑爆 WS 单条消息（`receiving from an empty and closed
  channel`），分块是硬约束。

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
