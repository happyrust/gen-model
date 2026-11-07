# rs-server 对 gen-model 现行数据库存储方案的适配

- 日期：2026-10-27
- 范围：`D:\work\plant-code\old\rs-server`（分支 `new_standard`，HEAD `fbe33fc`）对 `D:\work\plant-code\old\gen-model`（`refactor`，v0.1.44）现行存储契约的差距
- 性质：只读分析 + 变更方案，不含任何代码改动

---

## 第一部分：现行存储架构（gen-model 侧，事实基线）

### 1.1 一句话

持久层只有一个 SurrealDB，RocksDB 后端；**服务进程只连不拉起**（ADR-074）。进程内 `kv-mem` 只作暂存，不作持久。MySQL 落库路（`feature = "sql"`）已于 2026-09-07 整条退役。

### 1.2 两种运行形态（由介质派生，不是独立开关）

| 形态 | 介质 | 数据面 | 水位 | `pe` 行来源 |
|---|---|---|---|---|
| `ingest` | rocksdb（durable） | 整库基线 + 稳态增量窗口 | 有，三枚 | 基线 / 增量窗口写入 |
| `read-through` | mem | 不做基线、不开窗口 | 无 | 生成副产品（写透）+ `POST /element/ensure` |

判据链：`AIOS_STORE_MODE` 环境变量 > `DbOption.toml` 的 `store_mode` > 默认 → `StoreMode` → `DataFace`（`src/options.rs`：`data_face_for_store`）。

**关键含义**：`read-through` 形态下 `pe` 行是**缓存**，缺行 = 还没读到，不等于不存在（`src/data_interface/read_through.rs` 模块头不变量 R3）。

### 1.3 数据面表（`pe` 及其卫星）

| 表 | 角色 | 主键 / 形状 |
|---|---|---|
| `pe` | 元素**顶点** | `pe:⟨ref0_ref1⟩`；字段 `refno / owner / name / noun / dbnum / sesno / status_code / cata_hash / lock / deleted / op` |
| `<NOUN>`（`BRAN` / `EQUI` / …） | 按 noun 分表的属性行 | `⟨ref0_ref1⟩` |
| `ATT_UDA` | 字典 UDA 行 | `ATT_UDA:⟨…⟩` |
| `pe_owner` | 层级**边** | `[pe:父, pe:子]` |
| `ref_rev` | 反向引用索引 | `⟨…⟩` |

写法则（`src/data_interface/element_batch.rs`，基线与增量共用）：

- `WriteMode::Fresh` —— 只 `INSERT IGNORE`，已有行赢。
- `WriteMode::Replace` —— noun 行 / UDA 行 / 边**先删后插**；**`pe` 行例外**，走 `INSERT … ON DUPLICATE KEY UPDATE` 原地整行替换。
- `pe` 不删的理由：它是图上顶点，`pe_owner` / `ref_rev` / `tubi_relate` / 房间边都以它为端点，删顶点会静默带走这些边。
- 墓碑：`pe.deleted = true`，行**保留**（noun / owner / name 不擦）。

### 1.4 模型面表

`inst_relate`、`tubi_relate`、`inst_info`、`aabb`、`trans`、`geo_relate`、`room_relate`、`room_panel_relate`。

- record id 多为 `<ref0>_<ref1>`，可按 Ref0 字符串区间寻址。
- `inst_relate` / `tubi_relate` 带 `anc`、`dbnum` 索引（`src/fast_model/pdms_inst.rs::INST_RELATE_INDEX_SQL`）。
- 几何本体（`.mesh` / `.glb`）在文件系统，按内容寻址（`geo_hash`），不进库。

### 1.5 水位与控制面表

```
dbnum_watermark      —— 每库三枚水位：applied_sesno / model_sesno / file_latest_sesno
dbnum_info_table     —— 库元信息
dbnum_write_state    —— 写入标记 writing{S,T} / finished（崩溃回退，ADR-072）
queue_control        —— 队列暂停/恢复（跨重启保留）
model_scope_regen    —— 强制范围重生成的未完成留痕（issue #59）
```

三枚水位严格区分、互不替代：

- `applied_sesno` —— 数据已提交到哪一版（等值判据）。
- `model_sesno` —— 数据与模型一致到哪一版（等值判据）。
- `file_latest_sesno` —— 文件观察值（严格区分，不当水位用）。

### 1.6 已退役（rs-server 若仍依赖即需改）

| 退役物 | 时间 | 依据 |
|---|---|---|
| `gen_root` 表及其 12 个 `fn::gen_root_*` | ADR-079 | 逐根成败只活在进程内账本，模型面唯一持久事实是 `model_sesno` |
| `model_update_pending` 表 | ADR-072 D5 | 第二条异步账退役 |
| `model_watermark` 启动对账 | ADR-079 | 库里无逐根证据 |
| MySQL / `feature = "sql"` 整条 | 2026-09-07 | `AiosDBMgr::get_global_pool` / `get_project_pools` 已不在 `aios_core` |
| `old-pdms-io` / `parse_pdms_db` | 2026-09-04 | 两道源码级护栏钉死 |
| `included_projects` 配置 | ADR-078 | 项目集合改由 MDB 声明派生 |
| `room_incremental` / `AIOS_ROOM_INCREMENTAL` | ADR-073 D7 | 房间归属不是第三个阶段 |

### 1.7 与 rs-server 的共享表（双写者区）

| 表 | gen-model 的角色 | rs-server 的角色 |
|---|---|---|
| `datacenter_version` | **写**：`update … set status = 'Modify'` / `'Delete', belong_zone = …` | 写（提资状态账） |
| `datacenter_delete` | **只写不删**：`INSERT IGNORE`，`id = datacenter_delete:⟨ref0_ref1⟩` | **删**：`DELETE FROM datacenter_delete WHERE name IN $names` |

`datacenter_delete` 的字段（`increment_pipeline::render_datacenter_delete_insert_ignore`）：

```
id, refno(pe 链接), name(含首位 /), type(noun), zone_refno(pe 链接或 NONE),
dbnum, sesno, sesno_time, reason('deleted' | 'renamed')
```

gen-model 明确把删除权留给 rs-server（`datacenter_delete_reader.rs` 模块头 D7），且 `fast_delete::RANGE_TABLES` 不含这两张表 —— 回退 / 重初始化 / 删库都不清它们。

---

## 第二部分：rs-server 现状与差距

### 2.1 结构

62 个 `.rs`，约 3.26 万行，edition 2021，axum 0.8.1。三类后端并存：SurrealDB（主）、MySQL（普华材料 / 登录）、ArangoDB（图，多数已注释）。

### 2.2 差距清单

#### A. 构建层（阻塞级，当前无法编译）

| # | 问题 | 证据 |
|---|---|---|
| A1 | `aios_core` 指向 `../new-crates/aios-core`，**该目录不存在** | `Cargo.toml:14`；`Test-Path ../new-crates/aios-core` = False |
| A2 | 无 `Cargo.lock`，`surrealdb` 依赖未钉 rev | `surrealdb = { git = "https://gitee.com/happydpc/surrealdb" }` 会拉到最新，与 fork 2.1.4 服务端握不上手 |
| A3 | `edition = "2021"`，而 `aios_core` / gen-model 已迁 `2024` | `Cargo.toml:3` |
| A4 | 仓库根有 `compile_error.txt`（140 KB），记录的是另一台机器 `E:\aios_workspace` 的失败构建 | 不可作为当前基线 |

**目标状态**：`aios_core` 改为与 gen-model 逐字一致的声明 ——
`git = "https://github.com/happyrust/old-aios-core.git", rev = "4a7a00f93f3afa4069922ea52fab174cbbbddcde"`，并让根 `[patch]` 把整张图收敛到同一份源码；`surrealdb` 钉到 `45013fc9e74f6a112628e26fb5376f21cc1f9675`。

#### B. `sql` feature 退役（阻塞级）

| # | 问题 | 证据 |
|---|---|---|
| B1 | `aios_core` 已无 `get_global_pool` / `get_project_pools`，但 rs-server 直接调用 | `src/handle_api/projects.rs:17` |
| B2 | 登录走 `pb_human`（MySQL） | `src/login.rs` 的 `sqlx::query_as` |
| B3 | 材料同步走 `aios_core::material::*`（MySQL 口径） | `src/mysql/material.rs` 全文 |
| B4 | `project_ip` 表本身建在 MySQL（`sql.txt` 的 DDL） | 仓库根 `sql.txt` |

gen-model 的 `Cargo.toml` 已把这条路写死为「`sqlx` 依赖随之删除、持久层只有 SurrealDB 一个（宪法 I）」。rs-server 要么把这三类数据搬进 SurrealDB，要么明确标注为外部依赖并去掉对 `aios_core` 的 `sql` feature 期望。

#### C. 配置层（静默降级）

| # | 问题 | 证据 |
|---|---|---|
| C1 | `DbOption.toml` 里 `sync_localdb` / `publish_code` 在 `aios_core::DbOption` 已无对应字段 | 两键在 `options.rs` 命中 0 |
| C2 | `DbOption` 不 `deny_unknown_fields`，多余键被静默吃掉 | 未检出该属性 |
| C3 | 配置里没有 `store_mode`，介质实际由环境变量决定 | `store_mode` 在 `aios_core::options` 命中 0 |
| C4 | `included_projects = [...]` 已退役，非空只会打一句「已忽略」 | ADR-078 |

**风险**：C1 / C4 都属于「配置说了不算、但不报错」。启动横幅是唯一出口，脚本化部署看不出来。

#### D. 表契约层（语义级，最需要改）

| # | 问题 | 影响 |
|---|---|---|
| D1 | `datacenter_delete` **按 `name` 删**，而 gen-model 的主键是 `refno` 值 | 交付单元改名后（`reason = 'renamed'`）`name` 已不是原值，按 name 删会漏删，事件表留下永不消失的行 |
| D2 | rs-server 用 `record::id(id)` 反解 refno（`_` → `/`） | 与 gen-model 的 `id = ⟨ref0_ref1⟩` 约定一致，**这条没问题**，但必须显式钉住，不能靠巧合 |
| D3 | `get_pe` 的 basic_query 是 `SELECT * OMIT id FROM ONLY pe:X LIMIT 1`，**不判 `deleted`** | 已删元素仍能被查到，且带 `deleted = true` 返回给调用方；`children_query` 反而判了 `!in.deleted`，两处不一致 |
| D4 | 假设 `pe` 全量存在 | `read-through` 形态下 `pe` 是缓存，缺行 ≠ 不存在；rs-server 会把「还没读到」当成「没有这个元素」 |
| D5 | `room_data` 表与 gen-model 的 `room_relate` 边是两套房间表示 | 需明确谁是权威；gen-model 的房间归属由生成路径逐根写 `room_relate`，`room_data` 是插件上报的独立台账 |
| D6 | 无任何水位概念 | 读到的是「当前」，无法判断是否与 `model_sesno` 一致；数据窗口未落库时会读到半态 |

#### E. 并发与启动层

| # | 问题 | 影响 |
|---|---|---|
| E1 | rs-server 不拿实例锁、不做数据面签名校验 | 与 gen-model 同进程组运行时可能同时写同一台库 |
| E2 | 第二单元走 `SECOND_SUL_DB`（`SecondUnitDbOption.toml`，ns 1517） | gen-model 侧没有对应概念，需确认这是独立的库还是同一库的另一个 ns |
| E3 | ArangoDB 调用大量保留为注释 | 图关系已迁 `ref_rev` / `pe_owner`，注释代码是噪声，建议清掉 |

#### F. 已经对得上的部分（不必改）

- 未引用任何已退役表：`gen_root` / `model_update_pending` / `dbnum_watermark` / `dbnum_write_state` / `model_sesno` 命中均为 0。
- `datacenter_delete` 按 `zone_refno` 过滤，与 gen-model 建的 `idx_datacenter_delete_zone` 索引一致。
- 属性读取全部经 `aios_core`（`get_attr` / `get_spre_attr` / `get_catr_attr` / `get_named_attmap`），没有自拼属性 SQL —— 换存储时这部分自动跟随。
- 世界变换经 `get_world_transform` → `get_local_mat4` → `get_named_attmap`，同一条链。

---

## 第三部分：变更方案

### 3.1 原则

1. **单一写者**：每张表只有一个写者。跨仓共享的表，写权归属写进本文档，不靠约定。
2. **契约以 gen-model 为准**：表名、record id 形状、字段名、状态枚举，rs-server 跟随。
3. **读侧显式声明形态**：读 `pe` 的地方必须知道自己在 `ingest` 还是 `read-through`。
4. **退役不留后门**：不再有第二条落库路。

### 3.2 分阶段

**阶段 0：能编起来 —— 已完成（2026-10-27）**

实际做法与最初设想有一处关键差异，记录如下：

1. `aios_core` 换成与 gen-model 逐字一致的 `git + rev = 4a7a00f9…`，并由文末
   `[patch."https://github.com/happyrust/old-aios-core.git"]` 重定向到
   `../vendor/old-aios-core`（与 gen-model 的 `LOCAL-DEPS PATCH` 同款）。
2. **`features = ["sql"]` 保留**，与最初计划不同。rs-server 仍在用
   `AiosDBMgr::get_global_pool` / `get_puhua_pool`（MySQL 连接池），这两个函数在
   old-aios-core 里是 `#[cfg(feature = "sql")]`。删掉它阶段 1 才能做，不是阶段 0。
3. `surrealdb` **不写 `rev`**，只写 `git`，与 `aios_core` 的声明形状逐字一致，
   具体提交由 `Cargo.lock` 锁在 `45013fc9…`。
   这是本次唯一一个非显然的坑：rs-server 原先写了 `rev`，而 core 声明不带 `rev`，
   cargo 会把同一仓库算成两个 source，依赖图里出现两份 `surrealdb`，而
   `Surreal<Any>` 摆在两家的 API 面上，跨边界必然 `E0308`。
   想改版本请改 lock，不要在 `Cargo.toml` 里加 `rev`。
4. `edition` 提到 `2024`。`Cargo.lock` 已生成并提交。

阶段 0 修掉的 API 差异（共 50 处 bin / 58 处 test 错误，归并为 6 类）：

| 类 | 差异 | 处理 |
|---|---|---|
| A | `DataCenterRelations::build_relations` 已删，只剩 `new` | 31 处全量替换为 `new`；参数形状 `(start, end)` 本来就一致 |
| B | `ThreeDDatacenterResponse` 无 `status_code` 字段 | 删掉 7 处 `status_code: 200,` 字面量 |
| C | `DataCenterInstanceWithAdd` / `DataCenterProjectWithAdd` 已删 | 改用 `DataCenterInstance` / `DataCenterProjectWithRelations`；前者补 `instance_code`，后者补 `relations` |
| D | `SUL_DB` / `SECOND_SUL_DB` 从 `static ref` 变成 `Lazy<Surreal<Any>>` | 11 处 `&SUL_DB` → `&*SUL_DB` |
| E | 依赖图里两份 `surrealdb` | 去掉 `Cargo.toml` 里的 `rev`，由 lock 锁定（见上第 3 条） |
| F | `DataCenterRelations.attributes` 从 `Vec<DataCenterAttr>` 变成 `Vec<u8>` | 测试改读 `start_instance_code` / `end_instance_code` 字段 |

验收：`cargo check --all-targets` 零错误；`cargo tree -i surrealdb` 只有一份
`2.1.4 (…git#45013fc9)`。

`compile_error.txt` 未删，留待确认其内容是否仍有参考价值。

#### 阶段 0 追加：数据中心数据结构迁入本仓（2026-10-27）

上游有两个 `aios_core` 来源，对「数据中心交付契约」建模**互不兼容**：

| 来源 | `DataCenterRelations` | 关系语义 |
|---|---|---|
| `../vendor/old-aios-core`（gen-model 现行） | `start/end_object_code` + `start/end_instance_code` + `attributes: Vec<u8>` | 只有 `new()`，产出 `RELAPOPO`，属性为空 |
| `gitee.com/happydpc/rs-core`（本仓原始配套，`new_standard` 分支） | `object_model_code` + `attributes: Vec<DataCenterAttr>` | `build_relations()` 按端点类型分流，产出 `REL_PBS_OBJ` / `REL_PBS_3D_MOD` |

**决议：数据中心的数据结构以 rs-core 为准，整份迁入 rs-server，不放进 core。**

理由：`build_relations` 承载的是中台实际消费的对象模型编码与属性装配（`REL_PBS_OBJ` 带
`START_OBJ_CODE`/`START_INST_CODE`/`EDN_OBJ_CODE`/`END_INST_CODE`；`REL_PBS_3D_MOD` 带
`PBS_ID`/`THR_DIM_MOD_ID`，用于「设计管理活动-附件」关联）。换成 `new()` 会产出
`RELAPOPO` 且 `attributes` 为空，下游直接认不出关系。

落地：

1. 把 rs-core 的 `src/data_center.rs`（1216 行）整份复制到 `rs-server/src/datacenter/data_center.rs`。
   只改外部路径：原 `crate::X` → `aios_core::X`（`metadata_manager` / `schema` / `types` 仍由 core 提供），
   `crate::data_center` → 本模块自身。
2. `Cargo.toml` 补 `bevy_ecs`（必须用同一个 `gitee.com/happydpc/bevy` fork，否则 `Resource` /
   `Component` derive 与 core 侧不是同一个 crate）、`uuid = "1.3.0"`、`futures = "0.3.30"`。
3. 24 个文件的 `aios_core::data_center::…` 全部改为 `crate::datacenter::data_center::…`。
4. 回退先前基于 old-aios-core 做的三处错误适配：
   - 32 处 `DataCenterRelations::new` → 恢复 `build_relations`
   - 7 处被删的 `status_code: 200` → 恢复（rs-core 版 `ThreeDDatacenterResponse` 有该字段）
   - `draft.rs` 的 `DataCenterInstance` → 恢复 `DataCenterInstanceWithAdd`（后者无 `instance_code`，
     由 serde 层处理），`build_room_code_project` 返回类型恢复 `DataCenterProjectWithAdd`
5. `auto_test/gy_test.rs` 的关系断言恢复为从 `attributes` 里找 `START_INST_CODE` / `PBS_ID` 与
   `END_INST_CODE` / `THR_DIM_MOD_ID`。

验收：`cargo check --all-targets` 零错误；`rg 'aios_core::data_center' src` 为 0；25 个文件引用本地
`datacenter::data_center`；`cargo tree -i surrealdb` 仍只有一份 `2.1.4`。

**遗留约束**：`RawHoleData` / `TiziVirtualHoleData` 在 `aios_core` 内部也有 2-3 处引用，本地化后
这两处跨边界调用仍需走 core 的版本；当前编译通过，说明实际调用面没冲突，但若后续 core 侧改动
这两个类型，需要重新核对。其余 17 个类型在 core 里只出现在 `data_center.rs` 自身定义处，无函数
签名引用，本地化安全。

#### 持久层形态：rocksdb external（2026-10-27 固化）

rs-server 的持久层**只有一种形态：rocksdb external**。RocksDB 由另一台进程
（gen-model 或独立 SurrealDB 服务端）持有并落盘，rs-server 只通过 `ws://v_ip:v_port`
连上去读写，**不内嵌 `kv-rocksdb`、不拉起服务端**。这对应 gen-model 的 ADR-074
（「进程只连不拉起」）与 `StoreMode::RocksDb` 的语义。

改之前的状态是坏的，两个具体缺陷：

1. **假成功**。`AiosDBMgr::init_from_db_option`（old-aios-core `aios_mgr.rs:49-77`）
   在 `init_surreal()` 失败时只 `dbg!` 打一行，**不向上返回错误**。rs-server 随后照样
   打印「成功连接到数据库」并开始监听端口。一个连不上库的进程看起来是健康的，而它对
   调用方来说和「所有数据都不存在」无法区分。
2. **测试挂死**。11 处测试调 `init_test_surreal()`，其中 10 处直接丢弃返回值。库不在时
   不快速失败，后续第一条真实查询卡在连接超时上。实测 `cargo test` 跑 1800 秒未完成。

新增 `src/db_mode.rs`：

| 项 | 作用 |
|---|---|
| `StoreMode::parse` | 介质解析。缺省/空白一律按 `rocksdb`；`mem` / `spawned-rocksdb` **显式拒绝**并说明后果 |
| `verify_external_rocksdb` | 真发一条 `RETURN 'ok';` 探针。ws 的 `connect` 是惰性的，只有真发语句才会暴露握手失败 |
| `describe` | 启动日志一行，明说「本进程不拉起它」 |
| `init_test_db` | 取代裸调 `init_test_surreal`。缺库即 panic，报错里给出期望地址、ns、db 与原因 |

接入点：

- `main.rs` 启动段：解析介质 → 打印 → `verify_external_rocksdb` → **失败即中止启动**。
  删掉了原来无条件的 `println!("成功连接到数据库")`。
- 11 处测试调用全部切到 `crate::db_mode::init_test_db()`，并清理 4 个文件里因此
  变成未使用的 `init_test_surreal` import。
- `DbOption.toml` 的「版本数据库服务配置」段补了形态说明。
  `surreal_ns` 由 `1516` 改成 `"1516"`（core 里该字段是 `String`）。

验收：

- `cargo check --all-targets` 零错误。
- `cargo test db_mode::` 5 个测试全过（含「内存档必须被拒绝」与「空白地址在连接前被拒」）。
- 缺库场景从挂死 1800s 变成 **0.01s 失败**，报错直指
  `ws://127.0.0.1:1516` 与「RocksDB 由另一个进程持有，rs-server 只连不拉起」。
- 纯逻辑测试 55 个：49 通过、6 失败（失败项见下节，均与本次改动无关）。

**阶段 1：切掉 MySQL 三处**

| 数据 | 现状 | 目标 |
|---|---|---|
| 用户 / 登录 | `pb_human`（MySQL） | 迁 SurrealDB 表，或改走普华接口；需产品确认 |
| 材料（`pdms_mat_gy` / `pdms_mat_yk`） | `aios_core::material::*` | 改为读 SurrealDB 对应表，或明确保留为外部系统只读 |
| `project_ip` | MySQL 表 | 迁 SurrealDB，或改为配置文件 |

**阶段 2：修表契约**

1. `datacenter_delete` 的删除改为**按 `refno`**：`DELETE FROM datacenter_delete WHERE refno IN $refs`，`$refs` 由 `instance_code` 解出 refno 后渲成 `pe:⟨…⟩`。这样改名后仍能命中（`id` 与 `refno` 都不随 `name` 变）。
2. 若必须保留按 name 删，则同时按 `refno` 删一次，两条并集。
3. `get_pe` 之后统一补 `deleted` 判据，或在每个 handler 入口显式判断；不能一处判一处不判。
4. 显式声明 rs-server 只支持 `ingest` 形态（`pe` 全量存在），启动时检查 `/api/v1/health` 的 `medium` 与 `durable`，不满足直接拒绝启动并说明原因。

**阶段 3：接水位（可选但建议）**

读侧在关键查询上带 `sesno` 判据：`GET /api/v1/dbnums` 给 `applied_sesno` / `model_sesno`，rs-server 用它判断「这个库的数据是否已到我要的版本」，避免读到半态。

**阶段 4：并发与清理**

1. 启动时校验数据面签名（与 gen-model 的 `data_face_signature::validate_or_claim` 同一判据），避免两个进程认领同一持久库。
2. 清掉 `review_model_api.rs` / `hole_api.rs` 里的 ArangoDB 注释块。

### 3.3 表归属表（写权唯一）

| 表 | 唯一写者 | rs-server 权限 |
|---|---|---|
| `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` / `ref_rev` | gen-model | **只读** |
| `inst_relate` / `tubi_relate` / `inst_info` / `aabb` / `trans` / `geo_relate` | gen-model | **只读** |
| `room_relate` / `room_panel_relate` | gen-model | **只读** |
| `dbnum_watermark` / `dbnum_write_state` / `queue_control` / `model_scope_regen` | gen-model | **只读** |
| `datacenter_version` | gen-model（status / belong_zone） | 写（提资状态账） |
| `datacenter_delete` | gen-model 只插 | **删**（中台 remove-apply 成功后） |
| `room_data` | rs-server | 读写 |
| `hole_data` / `embed_data` / `tizi_virtual_hole` / `HOLE_RECORD` | rs-server | 读写 |
| `threed_view_form` / `threed_review` | rs-server | 读写 |
| `datacenter_receipt` | rs-server | 读写 |

### 3.4 验收

1. `cargo check --locked` 通过（无 `sql` feature）。
2. 对 `ingest` 形态的 AMS 库：`/get_tree/root/handle`、`/get_gy_bran_data`、`/get_hole_data_all`、`/get_room_data` 返回与改造前逐字段一致。
3. `datacenter_delete` 场景：单元改名 → gen-model 写 `reason = 'renamed'` → rs-server 按 refno 删除成功，表内无残留。
4. 墓碑场景：删掉一个元素后，`get_pe` 路径不再把它当活元素返回。
5. 并发场景：gen-model 与 rs-server 同时启动，不出现双写者告警。

### 3.5 查询面逐条核查（2026-10-27）

扫描范围：62 个 `.rs`、约 60 条路由、49 处手写 `SUL_DB.query`、`aios_mgr` 的 160+ 调用点。

#### 已对齐，不用动

| 面 | 结论 |
|---|---|
| 表名 | `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` / `inst_relate` / `tubi_relate` / `room_relate` / `datacenter_version` / `datacenter_delete` 全部是现行表 |
| 退役表 | `gen_root`、`model_watermark`、`included_projects`、`room_incremental`、`model_update_pending` —— rs-server **零引用** |
| 属性读法 | `get_attr` → `get_named_attmap_with_uda`，查 `<NOUN>` 表 + `ATT_UDA:<refno>`，与现行契约一致 |
| record id | `to_table_key("datacenter_version")` 用 core 同一个函数，gen-model 也用它，形状一致 |

#### P0：`deleted` 墓碑判据缺失

gen-model 删元素是**软删**（`deleted = true` 保留整行），见 `src/data_interface/read_through.rs:19`。

而 core 的 `get_pe` 不过滤：

```rust
// old-aios-core/src/rs_surreal/query.rs:61-63
r#"select * omit id from only {} limit 1;"#
```

同一个文件里的 `get_children_ele_nodes` 却带了 `!in.deleted`（`query.rs:994`）——core 自身不一致。

rs-server 全仓 `grep '\.deleted'` **零命中**，于是所有走 `aios_mgr.get_pdms_element` / `get_pe` 的路径都会把已删元素当活的返回。

**不能改 core**：gen-model 的 `effective_view`（`increment_pipeline.rs:1770`）依赖 `get_pe` 读到墓碑来拿删除前的 `noun` / `name`，全局加判据会破坏渲 `datacenter_delete` 的能力。只能在 rs-server 侧逐点收口：

```rust
async fn get_pe_alive(refno: RefU64) -> anyhow::Result<Option<SPdmsElement>> {
    let Some(pe) = get_pe(refno.into()).await? else { return Ok(None) };
    if pe.deleted { return Ok(None); }
    Ok(Some(pe))
}
```

手写查询的两处要补 SQL 判据：`handle_api/heat_dissipation.rs:106`、`:260`。

#### P0：`datacenter_delete` 按 name 删，改名必漏删

```rust
// src/datacenter/increment.rs:461-466
fn datacenter_delete_names(instance_codes: &HashSet<String>) -> Vec<String> {
    instance_codes.iter()
        .map(|instance| format!("/{}", instance.trim_start_matches('/')))
        .collect()
}
// :473-475
"DELETE FROM datacenter_delete WHERE name IN $names"
```

gen-model 写入时 `name` 是**删除前的位号**（`increment_pipeline.rs:711`）。位号一改 `name` 就变，按 name 删必然漏删。

主键与 `refno` 字段都只取 refno 值，不随 name 变（`increment_pipeline.rs:714`）：

```rust
id = record.refno.to_table_key(DATACENTER_DELETE),   // datacenter_delete:⟨ref0_ref1⟩
refno: {pe},                                          // pe:⟨ref0_ref1⟩
```

改为 `DELETE FROM datacenter_delete WHERE refno IN $refs;`。需要先补 `instance_code` → `refno` 的反查——现有 `collect_delete_instance_codes` 只拿到 `PBS_ID` 字符串。

#### P1：`dbnum` 维度缺失

`dbnum` 是 gen-model 控制面的一等概念（`dbnum_watermark` / `dbnum_info_table` / `dbnum_write_state`，`inst_relate` / `tubi_relate` 带该字段与索引）。rs-server 全仓零命中，`aios_core` 的 `DbOption` 也没有这个字段。

- 若只服务单个 dbnum：现在能跑，但须显式声明并在启动时校验，别在多 dbnum 库上静默给错数据。
- 若要跨 dbnum 查：关系表查询必须带 `where dbnum = $n`，否则混入别的库的行。`handle_api/heat_dissipation.rs:838` 的 `->tubi_relate` 属此类。

#### P1：形态假设未显式化

rs-server 隐含假设自己是 `ingest`（`pe` 全量）。若误连 `read-through` 的库，`get_pe` 返回 `None` 会被当成「元素不存在」，而实际是「缓存没命中」。

按 3.2 阶段 2 第 4 条做启动校验：读 `/api/v1/health` 的 `medium` / `durable`，不满足拒绝启动。

#### P2：MySQL 三处残留

16 处调用，分布在 9 个文件。`features = ["sql"]` 就是为它们留的。

| 数据 | 位置 | 处置 |
|---|---|---|
| `pb_human` 登录 | `login.rs:226, 251` | 迁 SurrealDB 或改走普华接口 |
| `pdms_mat_gy` / `pdms_mat_yk` | `datacenter/mod.rs:66-67` 及 6 个调用文件 | 改读 SurrealDB，或标为外部系统只读 |
| `project_ip` | `handle_api/projects.rs` | 迁 SurrealDB 或改配置文件 |
| `v_getpenetraroominfo` | `dq.rs:1750` | 普华侧视图，外部系统 |

#### 分级汇总

| 级别 | 项 | 影响 |
|---|---|---|
| P0 | `deleted` 墓碑判据 | 返回已删元素，数据错 |
| P0 | `datacenter_delete` 按 refno 删 | 改名漏删，删除闭环断裂 |
| P1 | `dbnum` 显式声明 + 关系表带判据 | 跨库混数据 |
| P1 | 形态校验（拒连 read-through） | 静默错数据 |
| P2 | MySQL 三处 | 阻塞摘 `sql` feature |
| P3 | ArangoDB 注释块（13 处 `get_arango_db`） | 死代码 |

---

## 第四部分：待确认（需产品/架构拍板）

1. **`datacenter_delete` 的删除键**：按 `refno` 是技术上正确的解，但需确认中台回传的 `instance_code` 一定能解出 refno。若存在只有 `name` 的历史数据，需一次性回填。
2. **`room_data` 与 `room_relate` 的权威**：插件上报的房间号与生成路径算出的房间归属冲突时，以谁为准。
3. **第二单元（ns 1517）** 是否保留，以及它与 gen-model 的 MDB 闭包是什么关系。
4. **登录源**：`pb_human` 迁 SurrealDB 还是改调普华。
