# ADR-091：数据库架构版本是一条整库记录，旧数据一律按重新生成处理

- 状态：已采纳（2026-10-28）。同日两次修订：先由「每 dbnum 一列 + 每行一格」收成
  「整库一条记录」；再改为「只在进程启动时全局核对，升级走一键入口」，见下文
  「修订记录」。
- 文件名保留 `model-storage-version-is-per-dbnum-…`：它记录的是这篇 ADR 的起点
  （曾经把号摊到每个 dbnum 上）。现行语义以正文为准——**不是 per-dbnum**。
- 相关：`aios_core::model_storage`、`gen-model/src/data_interface/storage_version_reconcile.rs`、
  `gen-model/src/data_interface/storage_upgrade.rs`、`gen-model/src/data_interface/startup_reconcile.rs`、
  `gen-model/src/data_interface/task_end.rs`、`gen-model/src/data_interface/fast_delete.rs`、
  `gen-model/src/data_interface/dbnum_state.rs`、`rs-server/src/db_mode.rs`、
  `vendor/old-aios-core/src/rs_surreal/geom.rs`、
  `vendor/old-aios-core/src/pdms_pluggin/heat_dissipation.rs`、
  `docs/plans/2026-10-26-ptset-split-table-plan.md`、
  `docs/plans/2026-10-28-storage-version-closure-plan.md`

## 背景

`pe_ptset` 分表落地后，读侧原本留了一条兜底：

```text
(type::thing('pe_ptset', record::id($pe)).ptset ?? $pe->inst_relate.out.ptset[0]) ?: {} as ptset_map
```

兜底是为了让还没重建的老库照常出点集。但它的失败模式很难看：老库上读不到时返回的是**空集**，
调用方无法区分「这根确实没有 P 点」和「这个库还没按新版发布过」。长度函数同理——老库静默回 `0.0`，
被当成「这根管件长度为 0」。这类静默错误在数据提取链路上会被写进下游元数据，事后极难追。

## 决策

1. **删掉兜底，只读 `pe_ptset`。** 旧的 `->inst_relate.out.ptset[0]` 读法从三个 `*_point_map`
   查询里整段移除（`geom.rs` 的 `ptset_map_projection`）。
2. **引入数据库架构版本号 `MODEL_STORAGE_VERSION`（当前 = 1）**，定义在 aios_core
   （`rs_surreal/model_storage.rs`），读写两侧共用同一个数。它回答的是
   「**整个数据库**的架构，本进程读不读得懂」，不是某张表、也不是某个 dbnum 的属性。
3. **粒度是整库一条记录。**

   ```text
   storage_version:current = {
       version: 1,      // 数据库架构版本，与 MODEL_STORAGE_VERSION 同值
       shape:   "…",    // 这一版的架构说明（人读）
       rebuild: "…",    // 改版本号之后必须做的事（人读，指向升级入口）
   }
   ```

   三格永远一起写：`shape` / `rebuild` 是代码里的常量（`SHAPE_NOTE` / `REBUILD_NOTE`），
   随 `version` 同代落库，不靠人工在库里改，所以不会与 `version` 分叉。

   **没有逐库的副本，也没有逐行的副本。** 同一份代码写出来的库不会有第二种架构。
   判「老库」只看这一条记录。**「没有 `pe_ptset` 行」不等于老库**——一根还没按新版发布过、
   或者这件本来就没有 P 点，都会没有行。

4. **版本只在进程启动时全局核对一次。**
   - gen-model 启动时读 `storage_version:current`，不匹配只点名：日志、`/health.storage_version`
     （`upgrade_required = true`）与运维面板横幅；**不阻断启动、不自动排队**。
   - rs-server 启动时读同一条记录，不匹配**拒绝启动**，错误文本带稳定码
     `MODEL_STORAGE_VERSION_MISMATCH` 与「请用 aios-database 升级数据库」。
     开发机可用 `AIOS_STORAGE_VERSION_CHECK=warn` 临时降成告警（默认严格）。
   - **运行期不对任何表、任何查询、任何 SurrealQL 函数做版本判断。** SurrealQL 的
     `fn::pe_ptset_of` 不再 `THROW`；aios_core 的 `query_*_point_map` 不再带版本列、
     不再 `check`；rs-server 的 409 映射与文本解析整段删除。没有行就是 NONE / 空集。
   - `ModelStorageVersionMismatch` 保留给启动核对；去掉 `dbnum` 字段，`Display` 与
     `rebuild_hint()` 都指向升级入口。

5. **升级 = 清空旧数据 → 重新解析 → 重新生成模型**，走一键入口
   `POST /api/v1/storage-version/upgrade`：

   - 体 `{ project?, mdb?, namespace?, dbnums?: [u32] }`；缺省名单 = 当前 MDB 声明的
     全部设计库 ∩ 监听范围，显式名单必须是这个范围的子集。
   - 逐库排既有的 `intent=reinitialize` 批次（复用 `manual_update` 的逐库入口，
     不另写清库 / 解析 / 生成路径）。
   - 进度写在 `storage_upgrade:current`：`target_version / requested / done / failed /
     started_at / finished_at`，另有完成判定所需的 `tasks` 与 `full_scope`。
   - 只有**全部请求的库都 `Succeeded` 且请求集就是全范围**时才写 `storage_version:current`。
     部分名单只更新进度，不写版本——那会把一份仍然老形状的库冒充成新版。
   - 判定放在数据批次 worker 的空闲轮与 `/health` 的按需刷新里，不在 handler 里等。

6. **写入时机收口。** `storage_version:current` 不再随单库 `model_sesno` 水位盖章写
   （`task_end.rs` 那一笔已删）。只有两种写入：
   - 全部库升级完成（上面第 5 条）；
   - 全新空库从零初始化完成（D-7）：启动时既没有版本记录、也没有任何 `pe` 行，
     `model_ready` 第一次翻真时自动写。老库不碰。

## 修订记录

### 第一轮：从两层收成一条

初版把号摊在两处：`dbnum_watermark:{dbnum}.model_storage_version` 管整库，
`pe_ptset.storage_version` 管单行。实现完发现第二层没有意义，且带来实际成本：

- 行形状不是某一行的属性。同一份代码写出来的行只有一种形状，
  不存在「这一行是旧形状、那一行是新形状」这种可靠可分辨的状态。
- 行上多一格 = 多一处会分叉的副本，也要求每个写点记得带上它。
- 判据顺序问题：两层判据要先判库再判行，而库级那格本身随水位盖章写，
  与「这份库的模型面是哪一版代码建的」并不是同一件事。

收成一条之后，语义变成单一问题：「这份库有没有那条记录、`version` 是不是我读得懂的」。

### 第二轮：从「读侧逐次判」收成「启动一次判」

Plannotator 批注指出：这个号指的是**整个数据库的架构版本**，不要和 `pe_ptset` 做版本检测；
启动时做全局检测，不对就提示用户用 aios-database 升级。据此：

- 删除 surql 的版本常量与 `THROW`、aios_core 读点集时的 `check`、rs-server 的 409 映射
  及其测试；读点集路径不再有任何版本判断。
- 写入点从 `task_end.rs` 迁到升级流程；单库 `model/rebuild` 不再是升级手段
  （它只重算模型、不清旧数据）。
- 版本核对只剩两处，且都在进程启动：gen-model 点名放行、rs-server 拒绝启动。

## 为什么是并列而不是扩展 `data_face_signature`

`data_face_signature:current.schema_version` 记的是「这个库归谁、是什么数据面」，
不匹配时 gen-model **直接拒绝启动**（`data_face_signature.rs` 的 `DATA_FACE_SIGNATURE_MISMATCH`）。
拿它管架构版本有两个问题：一是所有现存库会被一起拒在门外；二是它答的是数据面归属，
与「模型行的形状」是两件互不蕴含的事，混在一起会让两个升级节奏互相牵制。

## 三条正交的版本轴（不得混用）

| 轴 | 载体 | 回答的问题 |
|---|---|---|
| 数据库架构版本 `MODEL_STORAGE_VERSION` | `storage_version:current.version` | 架构**能不能读** |
| 点集求值版本 `PTSET_EVAL_VERSION` | `pe_ptset.generator` | 点**算得对不对** |
| 几何生成器指纹 | `dbnum_watermark.model_generator` | 哪一版 e3d-model **算的几何** |

求值规则改了只动 `PTSET_EVAL_VERSION`（老行照样读得懂，只是数值要重算）；
换 e3d-model 只动生成器指纹；**只有架构变了才动 `MODEL_STORAGE_VERSION`**：
字段增删或改类型、点号的键语义变化、坐标单位或参考系变化。改读法但形状没变不加。
动 `MODEL_STORAGE_VERSION` 时必须同时改 `SHAPE_NOTE` 与 `REBUILD_NOTE`。

## 为什么不静默重建

与 `generator_reconcile`（发现生成器变了就自动整库重排）刻意不同：架构升级会清空并重新解析
全部旧数据，量级与影响面远大于换生成器版本，时机必须由人挑。所以不做「发现不匹配就自动排队」，
而是**不静默重建，提示后一键升级**：启动日志 + `/health` 标红 + 运维面板横幅，按钮二次确认
并列出将被清空的库。rs-server 在升级完成前拒绝启动——记录还没写，它没有可读的证据。

## 后果

- 升级到新架构后，**所有未升级的库都读不懂**：gen-model 启动点名、rs-server 拒绝启动。
  这是有意为之：宁可显式拒绝，不要静默空集。
- 升级会清空旧数据并重新解析、生成，期间该库不可用；部分名单完成也不写版本记录，
  `/health.storage_version.upgrade` 会一直显示还差哪些库。
- 版本记录是新增表，老库没有它即「前版本化」，对旧代码无害；回退只需 `git revert`。
  曾经写过 `dbnum_watermark.model_storage_version` 与 `pe_ptset.storage_version` 的库，
  那两处残留列不再被读，`CONTENT` 写会自然把它们挤掉。
- 运行期不再有「架构版本不匹配」这条错误：不匹配的 rs-server 根本不会开始监听；
  gen-model 读点集永远不会因为版本抛错。
- rs-server 的 `resource/surreal/gy_common.surql` 已删除（gen-model 是唯一出厂方），
  否则 rs-server 每次启动都会把旧函数体打回库里、覆盖新版本。
- `history.txt` 里基于 `inst_relate.out.ptset` 的旧查询已标注退役。

## 未做的事

- 没有数据迁移脚本：旧数据不转换，按重新生成处理。
- 没有把版本号做进 `inst_relate` 的 `pts` 槽位（该槽位沿 ptset 计划 D6 恒为 NONE）。
- 没有处理 `common.surql` / `material_common.surql` 里其它跨仓重名函数（本 ADR 只覆盖 ptset 相关三件）。
- 「哪些 dbnum 还没升级」不再由版本记录回答；进度在 `storage_upgrade:current`。
