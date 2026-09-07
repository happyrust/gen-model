# ADR-060：模型行 id 就是记录 id；写入只有「先删后插」，没有 UPDATE

状态：**已接受**（2026-09-04 用户逐条拍板：「我不需要前缀，帮我开始修改正确，包括 plant-ui」、
「把本地变换改成写在 `geo_relate` 行上，连 `_local` 那个后缀一起去掉」、
「我们在更新时都是要先删除，然后再增加。所以用 ID 就好了，不要加什么前缀」、
「初始化时不需要考虑更新，直接都是 insert batch ignore。然后增量更新时，我们对模型数据都要采用针对 ids
先删除，然后再 insert batch ignore。所以不存在 update 等」）
日期：2026-09-04
关联：
- **承接** 同日的「模型生成只写进程内投影」（`changelog.md` 2026-09-04）：生成侧只写
  `fast_model::model_memory_store`，落库由 `fast_model::model_db_adapter` 单独承担；本 ADR 定的就是
  适配器写出去的那套行长什么样。
- **修订** 旧发布事务的行形状（`e3d_model_service.rs` 撤掉的那一整套 UPSERT 渲染）。
- **不动** ADR-057（e3d-model 是纯模型面）、ADR-053（direct 模式取数）、ADR-054（生成时点与凭证单调）、
  ADR-041（并行生成与切片）。
- 实现：`src/fast_model/model_db_adapter.rs`、`src/fast_model/e3d_mesh_store.rs::geometry_record_id`；
  读侧 `plant-ui/vendor/rs-core/src/rs_surreal/inst.rs`、`plant-ui/crates/plant-ui-data/src/lib.rs`、
  `vendor/old-aios-core/src/rs_surreal/inst.rs`、本仓 `fast_model::{pdms_inst, aabb_refresh, aabb_tree,
  manifold_bool}`。

## 背景

旧发布事务给每一行都戴了前缀：`inst_info:direct_<id>`、`aabb:direct_world_<id>`、
`trans:direct_local_<id>`、`aabb:direct_shared_<mesh>`，派生几何的记录 id 本身还带 `derived_`，
历史快照再叠一层 `hist_<key>_`。这些前缀各有出处：

- `direct_`：`inst_info` / `aabb` / `trans` 三张表在旧世界里**按几何实例做键**（旧行长这样：
  `inst_info:zz_f1_10`、`inst_info:⟨1⟩`、`type::thing('inst_info', cata_hash)`），e3d-model 要在同一张表
  里自己铸 id，就先划了个命名空间；
- `world_` / `local_` / `shared_`：一个共享图元元素在 `trans` 表里要占两行（世界一行、本地一行），
  在 `aabb` 表里还要与按 mesh 做键的共享盒共存；
- `derived_`：按根删除时普通行能靠 refno 猜到 id，派生行猜不到；
- `hist_`：历史快照与当前投影挤在同一批表里。

到 2026-09-04 这些前提逐条不成立了：三张表**只剩落库适配器一个生产写口**（旧生成器的入口在非默认的
`legacy_model` feature 后面，`aios_core` 与 plant-ui 侧都没有 SQL 写口）；更新一律先删后插，"猜 id"
这件事本身没有了；历史快照根本不进生产库（按需快照由 `historical_model` 从进程内投影直接回包 JSON）。

## 决策

### D1 行 id 就是记录 id

| 行 | id |
| --- | --- |
| `inst_relate` / `inst_info` / `aabb` 世界盒 / `trans` 世界变换 | `<记录 id>` |
| `aabb` 共享本地盒 / `inst_geo` | `<mesh id>` |
| `geo_relate` | `[type::thing('inst_info','<记录 id>'), inst_geo:⟨<mesh id>⟩]` |
| `tubi_relate` | `[pe:⟨<branch>⟩, <分支全局槽位>]` |

记录 id 只有两种形状，天生分得开：真实元素是 `<word0>_<word1>`（十进制加下划线），运行时派生的隐式
管身是身份 JSON 的 SHA-256（64 位纯十六进制，不含下划线）。**任何一层前缀都不再出现。**

### D2 共享的东西按 mesh 做键，且只增不删

`inst_geo` 与共享本地盒 `aabb:<mesh id>` 按内容寻址的 mesh id 做键，多个元素共用一行。删几何**绝不**
删它们——盘上的 `<mesh id>.mesh` 同理。

**它们只增不删，这是设计而不是欠账**（2026-09-04 用户确认：「只增不删、永久单调增长是对的，本来就是
为了共享而设计的」）。内容寻址的东西没有「属于哪一根」这回事：同一份网格随时可能被下一次重生成、别的
元素、别的库重新指上。所以既没有按根回收，也不打算有可达性 GC；代价是重复镶蹦出来的孤儿行与孤儿
`.mesh` 会留着，换来的是重生成时零重算、零重写。

### D3 共享图元的本地变换写在 `geo_relate` 边上

字段名 `transform`，对象形状与旧 `trans.d` 逐字相同，与 `insts_flat[].transform` 同名同形。
`trans` 表因此回到「一个元素一行」，读侧还少一次解引用。

**读侧一律写成 `transform ?? trans.d`**：旧生成器写的行与切换前的存量仍是 `trans` 记录链接，两种一起认。

### D4 写入只有两档，没有 UPDATE / UPSERT

- **初始化**：库里还没有这一批，不考虑更新，直接批量 `INSERT IGNORE`，撞上已有 id 就跳过。
- **增量**：先按 id 把这些记录的**整组表示**删掉，再批量 `INSERT IGNORE`。

删这一步不是保险而是必需：`INSERT IGNORE` 撞上已有 id 会**保留旧行**，不删就等于把上一轮的几何留在
库里。一条记录的"整组表示"是：`geo_relate`（按 `in` 删）、`inst_info`、`inst_relate`、`aabb:<id>`、
`trans:<id>`——两种形状（烘焙 / 共享图元）的行一起删，因为一次重投影可能把同一个元素从一种换成另一种。

一张表一条批量语句，被引用的行（盒、变换、网格、实例信息）排在引用它们的关系之前。

**实施注记（2026-09-06，ADR-064 D6）**：`inst_geo` 此前在实现里是 `INSERT … ON DUPLICATE KEY UPDATE direct_model, aabb`
——是本条「没有 UPDATE / UPSERT」的唯一漏网。它按内容寻址、同 id 必同内容（N3 第一种），那个 UPDATE 改不了任何读者
看得见的东西，已降为 `INSERT IGNORE`；模型面从此零 UPDATE。数据面（`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`）同日
并到同一口径（ADR-064 D1–D4），`pe` 因是图顶点例外为批量原地整行替换（ADR-064 D2）。

### D5 关系表要带 `RELATION`，`inst_relate` 不带

`geo_relate` 与 `tubi_relate` 走 `INSERT RELATION IGNORE INTO`（语法上 `RELATION` 在 `IGNORE` 之前）：
普通 `INSERT` 只把 `in` / `out` 当两个字段写下去，**不登记成图上的边**，`out->geo_relate` 与
`pe:⟨branch⟩->tubi_relate` 就都走不通。

`inst_relate` 反过来**不能**带：烘焙实体那一行只有 `in` 没有 `out`（网格直接挂在 `booled_id` 上），
而关系插入两端都是必填；读侧对它也从来是按 id 点查，不走图跳。

### D6 管身按分支整段重置

一个 BRAN 的隐式管身是整段重写的：插新段之前先 `DELETE pe:⟨branch⟩->tubi_relate`。关系 id 里的槽位
是**分支全局**连号，不是 `route_ordinal`——后者只在同一对 `(from, to)` 内唯一，无关的两段常常都带 0。

### D7 只服务当前投影

没有前缀，两个投影就不能共存在同一批表里。历史快照因此**不落库**：按需快照由 `historical_model` 从
投影存储直接回包 JSON。要改这一条，得先重新拿一层命名空间回来。

### D9 `inst_relate` 行上带 Plant UI 平表快路要的三个副本

`aabb_d`（= `aabb.d`）、`world_trans_d`（= `world_trans.d`）、`insts_flat`。三件齐活的行在
`query_insts_flat` 里零解引用零子查询直接成型；缺任一件，那一行整行退回 `query_insts_slim` 现值兜底。
**只买速度，不影响正确性**——实库 53,582 行上那两档解引用与子查询约 10.7s，平表读约 0.7s。

`owner` 由读侧从 `anc[1]` 还原，所以祖先链的顺序（自身在 [0]、owner 在 [1]、依次上溯）是写入侧要守的
约定，不只是给 `anc CONTAINS` 用的集合。

管身不需要这三件：`query_tubi_insts_by_brans` 走的是 `aabb.d` / `world_trans.d` 解引用。

### D8 `gen_root` 回执没有 revision CAS

回执里没有 revision，所以 `published_revision` 跟着行上的 `desired_revision` 走。「同一根不许被两个源
会话混着发布」那条守卫仍在，判据读库里的活行（`desired_target.source_end_sesno`），与写在同一条事务
边界上。

## 不变量

- **N1** 一个元素在每张表最多一行；两种表示不共存（增量先删，把两种形状一起删掉）。
- **N2** 共享网格行（`inst_geo`、`aabb:<mesh id>`、盘上的 `.mesh`）只增不删。
- **N3** `INSERT IGNORE` 的正确性前提二选一：同 id 必同内容（共享网格，内容寻址），或者本轮先删过
  （每元素行）。新增第三种行时必须落在这两条里的一条。
- **N4** 读侧不解析 id 形状。取本地变换走 `transform ?? trans.d`，取派生行走 `in` 反查加排除 refno 键，
  都不做 `string::starts_with` 之类的前缀匹配——那种写法还会让查询规划器退回整表扫。

## 影响面

**写口**：`model_db_adapter.rs` 是**唯一**的行渲染器。取证探针 `bin/direct_model_persist.rs` 原先自带
一套（`legacy-instanced-v2` 口径：每个元素都写 `inst_info`、`geo_relate` 带 `pts:[]`、`inst_geo` 上多
一份 `param.primitive_key`），2026-09-04 已并过来：探针只把生成结果摊成 `ProjectedGeometry`，SQL 由
`render_records` 出。两套渲染器各改各的迟早对不上，而**取证探针出的行要是与生产不同，它取的证就不是
生产的证**。它原先那套「写前备份 `inst_relate` 行 → 产出 `rollback.sql` 用 UPDATE 退回去」也随之取消
——没有 UPDATE 就没有可退回的旧值，要退回就重跑上一版生成。`inst-relation-backup.json` 与
`rollback.sql` 两份产物不再产生。

**生产调用点**（本 ADR 定稿时还没有，现已落地两个）：`data_interface::model_refresh` 的定向生成收尾
（一根一次 `persist_root`，落不进的那根算它自己失败进重试账）与 `python/src/exec_api.rs` 的整库生成
收尾（`persist_dbnum`）。

**读口**：Plant UI 读几何全程走记录链接（`aabb.d` / `world_trans.d` / `out->geo_relate` / `booled_id` /
`insts_flat`）与 `in` 反查，从不解析 id 形状，所以 id 那一半零改动；改的只有本地变换那一处
（七个读口，见「关联」）。

**索引**：`inst_relate` / `tubi_relate` 上只有 `anc` / `dbnum` / `in` 三个索引，没有一个建在 id 形状或
`direct_model` 上，本 ADR 不牵动索引。七张表都是 `TYPE ANY SCHEMALESS`，加字段不需要 DEFINE。

## 迁移

旧命名空间的清扫**按库一次，自动发生**：`model_db_adapter::render_legacy_prefix_sweep_once()` 挂在
`persist_dbnum` 的收尾，按 `string::starts_with(<string>record::id(id),'direct_')` 扫
`inst_info` / `aabb` / `trans` 与指向它们的 `geo_relate`，扫完在 `queue_control:model_legacy_prefix_sweep`
上盖标记，之后不再扫。新 id 的逐条删除语句够不着旧行（`inst_relate` 会被同 id 插入接管，但它原先指向的
`inst_info:direct_*` / `aabb:direct_world_*` / `trans:direct_*` 会悬空）。模型本来就是从文件重生成的，
删干净不丢东西；`inst_geo` 与 `.mesh` 不在其中。

守卫两道，都写在语句里而不是进程里：

1. **标记行**——「一次」的作用域是**库**：进程记的那一份，换个进程连同一个库又扫一遍。扫描与盖标记同
   一条事务，扫到一半崩掉就两样都不算数。
2. **旧命名空间没有活引用**——`inst_relate` 的 `out` / `aabb` / `world_trans` 与 `tubi_relate` 的
   `aabb` / `world_trans`，五处一个都不许还指着 `direct_*`。

另外两处指进旧命名空间的链接**刻意不在判据里**，各有理由（2026-09-04 真库跑完之后核出来的）：

- `geo_relate.in`：`in` 指着旧 `inst_info` 的那些 `geo_relate` 行**自己就是旧行**，清扫第一条语句删的
  就是它们。算成活引用，守卫就永远拦住自己——7997 现场正是这 7,908 行拦住了它。
- `inst_geo.aabb`：旧共享本地盒 `aabb:direct_shared_<mesh>` 确实还被活着的 `inst_geo` 行指着，而
  `inst_geo` 按 N2 只增不删，这一条**永远为真**。处置不是拦住整次清扫，而是把那几个盒子从 `aabb` 的
  删除里排除掉（`id NOTINSIDE (SELECT VALUE aabb FROM inst_geo …)`）。7997 现场是 22 个。

**第二道是 2026-09-04 在真库上撞出来的，不是防御性代码。** 本 ADR 原文说「新 id 的逐条删除够不着旧行」，
默认的前提是那些旧行已经悬空；实测 AMS 7997 不是这样：切换之前写下的 `inst_relate` 行**用的就是不带前缀
的 id**（`direct_` 只戴在另外三张表上），所以它们还活着，22,040 行的 `aabb` / `world_trans` 全部指着
`aabb:direct_world_*` / `trans:direct_*`，7,908 行的 `out` 指着 `inst_info:direct_*`。那个状态下扫一次，
删掉的是整库还在用的包围盒与世界变换。

旧行是**重生成一根、悬空一根**的，所以「悬空」只有整库口径才判得准——这也是清扫挂在 `persist_dbnum`
收尾而不是单根落库上的原因：整库重写完那一刻才是旧命名空间真正悬空的那一刻。判不准就不扫，失败方向朝着
「多留一批只占空间的死行」。想在整库重生成之后单独补一次，用
`model_db_adapter::sweep_legacy_prefixes()`，守卫同一套。

## D10 拼 SQL 分两档：值转义，id 拒绝（2026-09-04 补）

字符串字面量位置（`noun`、`generic`、`manifest_hash`、`dependency_digest`、`root_refno`、
`type::thing` 的 id 段）一律走 `sql_string()`，转义口径与 `dbnum_state::escape_surql_str` 相同。
`表:⟨…⟩` 位置没有转义序列可用，只能走 `bracketed_id()` **拒绝**：那些 id 全是机器铸的
（`<word0>_<word1>`、内容寻址摘要），字母表就是 `[0-9A-Za-z_]`，真出现别的字符说明上游铸 id 的地方
变了，早失败强过发出一条能被从中间拆开的语句。

`noun` 撞上引号的概率约等于零，但这是**口径不是概率**：裸插值留一处，下一个往行上加字段的人就会照抄。

## D11 多块发布在凭证行上留一道可见性栅栏（2026-09-04 补）

切块之后一根不再是原子边界（D4 之外的代价）：块与块之间库里躺着一个只更新了一半的根，`inst_relate`
的读者看不出这件事。第一块因此先写 `publication_status='publishing'`，最后一块的回执把它翻回 `ready`。
凭证前移（`window_root_plan::render_credential_advance`）判的就是 `publication_status='ready'`，所以
半截根不会被当成「S 时刻几何有效」的证明。

**只在会盖章的多块发布上写**：没有回执就没人解除它，那时写栅栏等于把这根永久钉死；只有一块时事务本身
就是原子的，中间态根本不存在。

## D12 几何之外的三件跟着落库走（2026-09-04 补）

旧发布事务除了几何行还挂着房间派生面与空间收敛，e3d-model 接管生成之后断了一段；现在从投影记录上
重新驱动，落在落库适配器里：

- **房间边清理**跟着**删除那一块**，**房间重算入队**跟着**回执那一块**。两笔不能对调：清边落在删除
  后面的块，中途失败就留下一批指着已删几何的悬空边，而下一轮对账再也看不见那些几何（行已经没了）；
  排重算落在前面的块，任务一被消费就在照着只写了一半的几何算房间归属。定向落库排重算 + 清边，
  全库落库（`persist_dbnum`）只清边（ADR-010 §4 收窄 1）。
- **`spatial_epoch` +1** 写在动过几何的每一块里（多 bump 无害，见 `render_spatial_epoch_bump`）。
- **进程内空间树**在事务之外按已提交指针收敛（`aabb_tree::converge_tree_after_persist`）；只有写全局库
  的 `PersistTarget::process()` 才做，自带句柄的目标写的是别处，让它去动全局树就是拿另一个库的指针
  污染这棵树。管身不在收敛名单里——它不写 `inst_relate`，树上本来就没有它的条目。

## 未做

- **`direct_model` 字段名**没动。它记的是「这行几何是谁、按什么口径、从哪个会话写的」，`format` 里还有
  `legacy-v1` / `legacy-instanced-v2` / `legacy-model`，所以改名要另立一条，方案与影响面已在会话里给过。
- **共享网格的可达性 GC** 仍然没有实现：`inst_geo`、`aabb:<mesh id>`、盘上的 `.mesh` 只增不删
  （N2）。2026-09-04 用户确认这是**设计如此**——它们本来就是为共享而生的内容寻址物，不按根回收。
