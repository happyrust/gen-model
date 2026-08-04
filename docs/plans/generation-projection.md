# 开发方案：生成投影（element_geom）——让模型生成从缓存跑起来

> 目标场景：初始化解析时把生成所需的 PE 数据一并写进 DuckLake，模型生成时自动加载到进程内缓存，从而提高生成速度。
>
> 相关：`docs/plans/kv-mem-hierarchy-cache-remediation.md`（层级投影层整改，本文件的**前置**）、`docs/plans/incremental-update-refno-only-greenfield.md`（release/readiness 一致性模型）、`CONTEXT.md`（术语表）。
>
> 实现涉及：`src/versioned_db/database.rs`（解析期写入）、`src/data_interface/hierarchy_projection.rs`（DuckLake 发布）、`src/data_interface/hierarchy_mem_query.rs`（kv-mem 分片）、`src/data_interface/current_hierarchy.rs`（读路径包装层）、`src/fast_model/query.rs`（生成侧调用点）。

## 1. 目标与验收

- 目标一（速度）：把模型生成的属性读取从「每元素一次远端 SurrealDB 往返」改成「每 dbnum 一次批量冷加载 + 进程内命中」。
- 目标二（正确性）：把当前**无 release 绑定、手工失效**的属性缓存，换成与层级投影同一个 `release_id` 绑定、可丢弃、失效即重载的投影。
- 验收（速度）：单个生成根的属性读取远端往返次数，从 `O(n)`（n = 根内元素数）降到 `O(dbnum 数)`；一次冷启动全量生成的属性 IO 时间可观测、可对比。
- 验收（正确性）：
  - (a) 生成过程中读到的属性与层级必须来自**同一个 release**；release 切换时生成任务必须失败或重启，不得混读。
  - (b) 增量窗口更新属性后，投影必须同步更新；构造一个「只改属性、不改结构」的窗口，生成结果必须反映新属性。
  - (c) 投影 schema 版本与 release 元数据绑定，版本不符时 fail-closed，不得静默用旧列集生成。
- 验收（内存）：进程内属性副本只有一份（投影分片），`aios_core` 的 `#[cached]` 属性备忘不再参与当前态生成路径。

## 2. 事实基线（2026-07-30 复核）

### 2.1 现在的投影只有树结构

DuckLake `hierarchy_node` 只有六列：`dbnum / refno / owner / noun / name / sibling_order`。生成真正要读的东西一个都不在里面。三处代码注释把这条边界写得很明确：

- `fast_model/query.rs:118` —— `Only the hierarchy expansion moves to kv-mem; PE attributes and inst metadata remain in the persistent Surreal store.`
- `data_interface/current_hierarchy.rs:51` —— `Only refno/owner/noun/name/dbnum are meaningful; attribute-domain fields (sesno, cata_hash, ...) stay default.`
- `data_interface/current_hierarchy.rs:480` —— `Persistent geometry attributes still come from Surreal; only parent discovery moves to kv-mem.`

### 2.2 生成热路径上的逐元素读取

| 位置 | 形态 |
|---|---|
| `fast_model/query.rs:14-23` `load_named_attmaps` | 逐 refno `aios_core::get_named_attmap().await`；注释自带 `ponytail: attributes remain per-item until a persistent batch API exists` |
| `fast_model/query.rs:150-155` `current_group_by_cata_hash` | 逐 refno `aios_core::get_pe().await`，同样的 ponytail 注释 |
| `data_interface/current_hierarchy.rs:495-501` `current_world_transform` | 逐祖先层 `aios_core::transform::get_local_mat4().await` |
| `fast_model/query.rs:285-326` `query_gm_params` | 嵌套：子 → 孙，每层再走一遍上面的逐项取属性 |

`fast_model/` 下这类调用点共 51 处，其中 `cata_model.rs` 23 处。

此外，每一次夹在中间的 kv-mem 层级调用还要再打一次 readiness RPC（见整改方案 H1），所以连「内存遍历」那部分也是网络绑定的。

### 2.3 已经存在一层「意外的」属性缓存

这是本方案最重要的一条事实基线。`aios_core`（当前 patch 到 `../../rs-core-pin`）用 `cached` crate 宏在**函数级**做了备忘：

- `rs_surreal/query.rs:525` `#[cached(result = true)] pub async fn get_named_attmap(...)`
- 同一批还有 `GET_PE` / `GET_TYPE_NAME` / `GET_CHILDREN_REFNOS` / `GET_CHILDREN_NAMED_ATTMAPS` / `GET_CHILDREN_PES` / `QUERY_ANCESTOR_REFNOS` / `QUERY_DEEP_CHILDREN_REFNOS` / `GET_SIBLINGS` / `GET_CAT_ATTMAP` / `GET_CAT_REFNO` / `GET_SELF_AND_OWNER_TYPE_NAME` / `GET_WORLD_TRANSFORM` / `GET_WORLD_MAT4`。

它的三个性质决定了它**不适合承担生成加速**这件事：

1. **无界**。`#[cached]` 默认是不淘汰、无 TTL 的 `HashMap`。一轮全量生成之后，进程里驻留的是整个属性域的副本；再叠加 kv-mem 分片（同样无上界，见整改方案 H3），等于两份无界缓存并存。
2. **无 release 绑定，失效靠手工列表**。`rs_surreal/query.rs:833` `clear_all_caches_batch(refnos)` 逐个 `cache_remove`，两个 world 缓存则整体 `cache_clear()`。失效范围由增量窗口的「变更元素 + 其属主」集合驱动——**漏一个就静默读到陈旧属性**。层级侧靠 `release_id` + readiness 严防死守的「混合 release」，在属性侧完全没有对应机制。
3. **冷未命中仍是一次往返**。第一次全量生成——也就是最贵的那次——拿不到任何好处。

设计者自己知道这层缓存危险。`rs_surreal/geom.rs:53-61` 有一段少见的坦白注释，解释 `query_deep_visible_inst_refnos` 为什么**刻意不加** `#[cached]`：

> `clear_all_caches_batch` 只按「变更元素 + 其属主」失效，从不知道这里还有一份按**生成根**为键的快照；缓存命中一次陈旧值，新加的构件就整体缺席本轮刷新（mesh 不生成、aabb 不落库、房间不触发），且无任何报错。

### 2.4 有一套更好的缓存，但被注释掉了

`rs_surreal/mod.rs:11-12`：

```rust
// pub mod cache_manager;
// pub mod queries;
```

`cache_manager.rs` 里是一个带 TTL（默认 300s）与容量配置的 `QueryCacheManager`，`queries/` 下的 `attributes.rs` / `basic.rs` / `hierarchy.rs` 是接入它的查询层。整套是死代码，不参与编译。所以线上跑的是 `query.rs` 里那批 `#[cached]`。

### 2.5 没有任何预热

kv-mem 分片是第一次查询时懒加载的（`hierarchy_mem_query.rs:376` → `load_current_snapshot`）。生成侧唯一用到投影服务的地方是 `fast_model/gen_model.rs:65` 的 `refnos_by_nouns`。全仓搜不到预热/prefetch。而那次冷加载会持着全局 `PROJECTION_GATE` 读整个分区（见整改方案 C2），把其余所有库一起堵住。

## 3. 为什么是投影，而不是继续加一层缓存

结论先行：**`element_geom` 不是「再加一层缓存」，它是用来替换 §2.3 那层意外缓存的。** 主要理由是正确性，速度是顺带的。

| | `#[cached]` 备忘（现状） | `element_geom` 投影（本方案） |
|---|---|---|
| 失效机制 | 手工 refno 列表，漏了就静默陈旧 | 绑定 `release_id`，CAS 不符即整片驱逐重载 |
| 内存 | 无界、不可回收 | 分片可丢弃，纳入 H3 的统一上界与观测 |
| 冷启动 | 无帮助，仍是 n 次串行往返 | 每 dbnum 一次批量列存读 |
| 与层级的一致性 | 无关联，可能属性新、层级旧 | 同事务、同 release，天然一致 |
| 数据来源 | 生成时回查 SurrealDB | 解析时顺手写出（数据已在内存里） |

关键的实现便利：**解析时这些数据已经在手上。** `sync_total_async_threaded` 的 chunk 循环里 `total_attr_map` 就在旁边，`HierarchyRowV1::from_named_attr_map` 正是在那里构造的（`versioned_db/database.rs:965-987`）。多搭一个行构造器，解析期的边际开销接近于零。

### 3.1 为什么不是「把 PE 全量放进 DuckLake」

属性域是开放的：按 noun 分表 + 用户自定义属性（UDA）。全量搬过去等于把整个 SurrealDB 再存一份，投影层会变得和源一样复杂，且每加一个 noun 就要动 schema。

而**生成需要的子集是封闭且已知的**——`GmParam`（`aios_core::pdms_data::GmParam`）这个结构体就是现成的契约，`fast_model/query.rs:258-282` 的 `current_gm_param` 把它填满所用到的属性，就是列集的下界。有界 schema 才维护得住。

SurrealDB 仍旧是属性域的唯一权威；`element_geom` 是可丢弃的读优化反范式化，语义与 `hierarchy_node` 完全一致。

## 4. `element_geom` 列集

与 `hierarchy_node` 同 catalog、同样 `PARTITIONED BY (dbnum)`。

**注意：PDMS 目录属性经常是表达式而不是字面量**（`current_gm_param` 全程用 `get_as_string` 取值，交由后续表达式求值）。因此几何参数列**一律用 `VARCHAR` 存原始串，不要提前转数值**，否则会丢掉表达式。

列集依据是一次完整的属性键普查，结果见 **`docs/plans/generation-projection-attr-survey.md`**：扫 `fast_model/`（56 键）、`rs-core-pin/src/expression/`（51 键）、`rs-core-pin/src/transform/`（13 键），再把语义访问器映射回真实键，**并集 84 个**。

表会比较宽，但这正好是列存该干的事——DuckDB 只读 `SELECT` 到的列，宽表不会让窄查询变慢。

```sql
CREATE TABLE hierarchy.element_geom (
    -- 主键与分区
    dbnum          UINTEGER NOT NULL,
    refno          UBIGINT  NOT NULL,

    -- PE 域（替代 get_pe 的热字段）
    cata_hash      VARCHAR,
    deleted        BOOLEAN  NOT NULL,
    has_spre       BOOLEAN  NOT NULL,   -- SPRE.id != none
    has_catr       BOOLEAN  NOT NULL,   -- CATR.id != none

    -- 目录表达式上下文：DESP[n] / PARAM n 的唯一设计侧入口，漏了整条目录路径都要回落
    desp           VARCHAR[],           -- DESP

    -- 变换域。POS/ORI 对应 model_impact.rs:111 TRANSFORM_ONLY_ATTR_NAMES，
    -- 其余来自 get_local_mat4 的分支处理
    pos            VARCHAR,  ori   VARCHAR,  poss  VARCHAR,  pose VARCHAR,
    bang           VARCHAR,  npos  VARCHAR,  ydir  VARCHAR,  opdi VARCHAR,
    delp           VARCHAR,  cutp  VARCHAR,  cutb  VARCHAR,  zdis VARCHAR,
    pkdi           VARCHAR,  lmirr BOOLEAN,  jlin  VARCHAR,  jusl VARCHAR,
    posl           VARCHAR,

    -- 可见性（is_visible_by_level → get_level() → LEVE 的上下界）
    level_lo       UINTEGER, level_hi UINTEGER,
    tufl           BOOLEAN,  clfl     BOOLEAN,

    -- GmParam 标量，全部原样存串
    prad VARCHAR, pang VARCHAR, pwid VARCHAR, phei VARCHAR, poff VARCHAR,
    drad VARCHAR, dwid VARCHAR, plax VARCHAR,

    -- 环 / 尺寸
    heig VARCHAR, angl VARCHAR, radi VARCHAR, frad VARCHAR,

    -- P 点 / 轴参数（get_axis_param，query_cata.rs:86）
    numb   INTEGER, pcon VARCHAR, pbor VARCHAR, pzaxi VARCHAR,
    ptcd   VARCHAR, ptcp VARCHAR, ptcpos VARCHAR,

    -- cata_model 专用
    arri INTEGER, leav INTEGER, napp INTEGER, sjus VARCHAR,
    hdir VARCHAR, hpos VARCHAR, tdir VARCHAR, tpos VARCHAR,

    -- resolve 专用
    gtyp VARCHAR, para VARCHAR, pkey VARCHAR,

    -- GmParam 定长组，按 current_gm_param 的取值顺序存 LIST（顺序不能乱）
    diameters VARCHAR[],   -- PDIA, PBDM, PTDM, DIAM
    distances VARCHAR[],   -- PDIS, PBDI, PTDI
    shears    VARCHAR[],   -- PXTS, PYTS, PXBS, PYBS
    lengths   VARCHAR[],   -- PXLE, PYLE, PZLE
    xyz       VARCHAR[],   -- PX, PY, PZ, PBBT, PCBT, PBTP, PCTP, PBOF, PCOF
    dxy       VARCHAR[],   -- DX, DY
    paxises   VARCHAR[]    -- PAXI, PAAX, PBAX, PCAX + PTS 展开 + PLAX
);
ALTER TABLE hierarchy.element_geom SET PARTITIONED BY (dbnum);
```

**顶点序列另开一张表**，因为它是一对多（SLOO 子顶点、SPRO 子顶点、LOOP/PLOO 顶点），塞进上表会让行宽剧烈波动：

```sql
CREATE TABLE hierarchy.element_vertex (
    dbnum          UINTEGER NOT NULL,
    owner_refno    UBIGINT  NOT NULL,   -- 顶点所属的几何元素
    vertex_order   UINTEGER NOT NULL,   -- 保持解析顺序
    px             VARCHAR,
    py             VARCHAR,
    pz             VARCHAR,
    prad           VARCHAR,             -- 顶点圆角
    dx             VARCHAR,
    dy             VARCHAR
);
ALTER TABLE hierarchy.element_vertex SET PARTITIONED BY (dbnum);
```

### 4.1 列集收敛性：能收敛，但不能假设已封闭

普查（见附录）得出的关键结论：**列集是可以收敛的**，因为目录几何表达式对设计侧属性的依赖收敛到 `DESP` 一个数组属性上——`DESP[n]` / `PARAM n` 都由它展开成求值上下文（`expression/resolve_helper.rs:29-41`）。不存在「目录里写什么名字就得存什么列」的发散。

但**不能证明它已经封闭**，有三个静态扫描抓不到的口子：

1. `ATTRIB <name>` 表达式语法允许按名字引用任意属性（例如 `ATTRIB CPAR[3]`），名字存在目录数据里而不在源码里。实践中这类引用主要落在目录侧（CATA 库），设计侧靠 `DESP`，但原理上抓不全。
2. UDA 是开放集合。当前生成路径不读，`plug_in/` 读。
3. `room_*.rs` / `pdms_inst.rs` 的空间逻辑未逐行确认（它们主要消费已算好的 `GmParam`）。

因此 §9 里那条 **debug-only 回退计数器是必需的，不是锦上添花**：投影里缺某属性时回落 SurrealDB 并计数告警，跑一轮全量把漏网的捞出来，再决定补列还是保留回退。代码变动（尤其新增几何 noun 支持）后应按附录 §5 重跑普查。

## 5. 发布与一致性

### 5.1 与 hierarchy_node 同事务、同 release

`element_geom` / `element_vertex` 的写入必须**并入 `publish_baseline_inner` 与 `apply_change_set_inner` 已有的那个 DuckLake 事务**，共用同一个 `release_id`、同一个 snapshot。

这条不能让步。如果两张表各自发布、各自有 release，就会重新造出一个「层级是 release N、几何是 release N-1」的混合态，而现有的 readiness 门只检查一个 `hierarchy_release_id`，根本发现不了。

具体：`change_hash` 的计算要把几何行一并纳入（否则同结构、只改属性的窗口会算出与上一个 release 相同的 hash，被幂等短路误判为「已提交」直接跳过）。这是本方案对现有代码**最容易漏、后果最严重**的一处改动。

### 5.2 投影 schema 版本

`hierarchy_release` 增加一列 `projection_schema_version UINTEGER NOT NULL`。读侧加载分片时比对：版本不符 → fail-closed，报「需要重建基线」，不得用旧列集继续生成。

理由见 §7 的风险项：以后每加一个几何属性都要改列集，没有版本号就会出现「新代码读旧投影，缺列静默按默认值生成」。

### 5.3 增量维护

change-set 结构扩展为同时携带层级行与几何行的 `deletes` / `upserts`。触发条件与 `model_impact.rs` 的判定天然对齐：

- `TRANSFORM_ONLY_ATTR_NAMES = ["POS","ORI"]`（`model_impact.rs:111`）→ 只更新变换列。
- 其他几何属性变化 → 更新对应列并触发 Regen。
- 结构变化（OWNER / children）→ 层级行与几何行一起动。

**新增的一致性面**：`model_impact.rs` 现在只需要判断「要不要重生成」，之后还要额外承担「哪些列要同步进投影」。两者的属性集合必须保持一致，否则会出现「判定说要重生成、但投影里还是旧值」——生成跑了，结果是错的。这是本方案最大的风险，见 §7。

## 6. 读路径改造

**不要去改 `aios_core`。** 它是外部 crate（git 依赖，当前临时 patch 到 `../../rs-core-pin`），改它会让这份依赖更不可复现。

正确的接缝是复用现有模式：`current_hierarchy.rs` 已经是「当前态走投影、历史态回落 `aios_core`」的包装层。照它再加一个 `current_attributes.rs`：

```rust
// 当前 refno：从投影分片批量取；历史 refno：回落 aios_core，语义不变
pub async fn current_named_attmaps(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<NamedAttrMap>>
pub async fn current_pe_meta(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<PeMeta>>
pub async fn current_gm_params(refno: RefnoEnum) -> anyhow::Result<Vec<GmParam>>
```

然后把 `fast_model/query.rs` 的 `load_named_attmaps` / `current_group_by_cata_hash` / `query_gm_params` 改成调它。51 个调用点大多是间接经由这三个函数，直接改动面比数字看起来小。

与之配套的两件事（都在层级整改方案里，是本方案的前置）：

- **预热**：`run_unit_worklist`（`batch_worker.rs:500`）入口处，按工作集里出现的 dbnum 批量 `ensure_shard`，而不是等第一次查询去撞冷加载。
- **读会话**（整改方案 H1）：一次生成任务确认一次 readiness。不做这个，属性即使命中内存，中间夹的层级调用仍然每次打网络，收益会被吃掉大半。

## 7. 分期

**前置（在层级整改方案里，必须先落地）**

- C1 `external_owner_routes` 增量化。**硬前置**：`prepare_locator` 现在会全表扫描，投影变宽之后这个扫描的代价会成倍放大。
- C2 锁拆分 + poison 恢复。预热会让冷加载并发，不拆锁反而更堵。
- H5 三段发布强制化。新表加入同一个发布事务，发布协议必须先是一致的。

**阶段 A · 不新增存储，先拿确定收益**

1. 属性普查：51 个调用点的属性键名清单（也是阶段 C 的输入）。
2. H1 读会话 + 生成批次分片预热。
3. `load_named_attmaps` / `current_group_by_cata_hash` / `get_local_mat4` 批量化——纯访问形状改造，不新增存储与一致性面。
4. **在这里打点测一次**：属性 IO 占整根生成耗时的比例。这是要不要继续做阶段 B/C 的判据。

**阶段 B · 投影落地（若阶段 A 的测量显示属性 IO 仍占大头）**

5. `element_geom` / `element_vertex` 建表 + `projection_schema_version`。
6. 解析期写入：在 `database.rs` 的 chunk 循环里，紧挨 `HierarchyRowV1` 构造几何行。
7. 发布并入现有事务，`change_hash` 纳入几何行。
8. kv-mem 分片扩展为承载两张表，冷加载一并拉取。

**阶段 C · 切换读路径**

9. `current_attributes.rs` 接缝 + `fast_model/query.rs` 三个入口切过去。
10. 增量维护：change-set 扩展 + `model_impact.rs` 的列同步。
11. 确认当前态生成路径不再经过 `aios_core` 的 `#[cached]` 属性备忘，回收那部分常驻内存。

## 8. 明确不做（本期）

- **不改 `aios_core` / `rs-core-pin`。** 依赖本身已经是「临时钉版、不可复现」的状态，再叠加改动会让问题复合。所有改造走 gen-model 侧的包装层。
- **不启用 `rs_surreal/queries` + `cache_manager` 那套被注释掉的缓存。** 它解决的是 TTL 与容量，解决不了「无 release 绑定」这个根本问题；启用它等于给错误的缓存形态续命，与本方案方向相反。
- **不把 UDA 纳入投影。** 用户自定义属性是开放集合，生成路径目前也不读它。
- **不把属性域的权威从 SurrealDB 搬走。** `element_geom` 始终是可丢弃的派生数据，丢了就从 SurrealDB 重新解析重建。
- **不在阶段 A 之前动投影。** 阶段 A 的测量结果可能显示批量化就够了；那样阶段 B/C 的复杂度就不必付。

## 9. 风险与回归

- **`model_impact.rs` 与投影列集必须保持同步**，这是本方案最大的新一致性面。判定说「这个属性变化要重生成」，投影却没更新对应列，结果就是「生成跑了、用的还是旧值」——和 §2.3 里那段注释描述的静默失败是同一类。缓解：把「属性 → 影响判定」与「属性 → 投影列」做成**同一张表驱动**，而不是两处各写一份。
- **`change_hash` 必须纳入几何行**，否则「只改属性、不改结构」的窗口会被幂等短路当成重放跳过。必须有一条专门的回归测试。
- **列集不全是运行时才暴露的缺陷**，所以 §4.1 的属性普查不是可选项。建议加一个 debug-only 校验：生成时若某属性在投影里缺失而回落到 SurrealDB，打 warning 并计数，跑一轮全量把漏网的捞出来。
- **必须重建基线**。`initialize()` 对旧 schema 直接 bail（`hierarchy_projection.rs:1516-1527`），存量 DuckLake root 无法原地迁移。运维上要安排一次全量重建窗口。
- **投影变宽会放大所有既有的 DuckLake 问题**：全表扫描（C1）、逐行 DELETE（M8）、事务内 `COUNT(*)`（M5）、快照与文件无回收（H6）。这也是把 C1 列为硬前置的原因。
- **表达式必须原样存串**。任何把几何参数提前转成数值的「优化」都会丢掉目录表达式，且很可能到具体构件才暴露。
