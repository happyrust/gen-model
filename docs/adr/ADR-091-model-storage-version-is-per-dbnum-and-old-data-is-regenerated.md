# ADR-091：模型面用一条全局存储架构版本记录识别旧数据，旧数据一律按重新生成处理

- 状态：已采纳（2026-10-28）。同日修订一次：版本载体从「每 dbnum 一列 + 每行一格」
  收成「整库一条记录」，见下文「修订记录」。
- 相关：`aios_core::model_storage`、`gen-model/src/data_interface/storage_version_reconcile.rs`、
  `gen-model/src/data_interface/task_end.rs`、`gen-model/src/data_interface/fast_delete.rs`、
  `gen-model/src/data_interface/dbnum_state.rs`、`rs-server/src/error.rs`、
  `vendor/old-aios-core/src/rs_surreal/geom.rs`、
  `vendor/old-aios-core/src/pdms_pluggin/heat_dissipation.rs`、
  `docs/plans/2026-10-26-ptset-split-table-plan.md`、`docs/plans/2026-10-27-rs-server-ptset-alignment-and-surreal-connection-plan.md`
- 取代/修订：ptset 分表计划的 D2 补充（旧兜底读取一并退役）

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
2. **引入模型面存储架构版本号 `MODEL_STORAGE_VERSION`（当前 = 1）**，定义在 aios_core
   （`rs_surreal/model_storage.rs`），读写两侧共用同一个数。
3. **粒度是整库一条记录。**

   ```text
   storage_version:current = {
       version: 1,      // 行形状版本，与 MODEL_STORAGE_VERSION 同值
       shape:   "…",    // 这一版的行形状说明（人读）
       rebuild: "…",    // 改版本号之后必须做的事（人读）
   }
   ```

   三格永远一起写：`shape` / `rebuild` 是代码里的常量（`SHAPE_NOTE` / `REBUILD_NOTE`），
   随 `version` 同代落库（`task_end.rs` 的 `render_storage_version_record`，
   与模型水位同一笔事务），不靠人工在库里改，所以不会与 `version` 分叉。

   **没有逐库的副本，也没有逐行的副本。** 行形状是整个模型面共用的一份约定，
   不是某张表、也不是某个 dbnum 的属性：同一份代码写出来的行不会有第二种形状。
   重建的单位仍然是 dbnum，但「重建完没有」不靠另一处版本号回答——重建会重写那个库的
   全部模型行，逐库进度看的是水位（`dbnum_watermark.model_sesno`）。

   判「老库」只看这一条记录。**「没有 `pe_ptset` 行」不等于老库**——一根还没按新版发布过、
   或者这件本来就没有 P 点，都会没有行。
4. **不匹配时的动作：读侧报错，写侧只点名。**
   - aios_core 读侧返回 `ModelStorageVersionMismatch`，`Display` 里带稳定码
     `MODEL_STORAGE_VERSION_MISMATCH`、dbnum、stored、expected 与重建命令。
     `MismatchSource` 只有两种：`Missing`（没有这条记录 = 前版本化）与 `Version`（有记录但读不懂）。
   - SurrealQL 的 `fn::pe_ptset_of` 在版本不符时 `THROW` 同一段文本，长度函数不再静默回 0。
   - rs-server 把它映射成 **409 Conflict** + JSON（`code` / `dbnum` / `stored` / `expected` /
     `source` / `action`），与真正的 500 分开；只影响该 dbnum 的点集查询。
   - gen-model 启动时点名（日志 + `/health.model_cache.storage_version`），
     **不阻断启动、不自动排队重建**。
5. **旧数据一律按重新生成处理**：跑 `POST /api/v1/dbnums/{dbnum}/model/rebuild`。
   与 ptset 计划的 D5（不做补扫）一致。

## 修订记录：为什么从两层收成一条

初版把号摊在两处：`dbnum_watermark:{dbnum}.model_storage_version` 管整库，
`pe_ptset.storage_version` 管单行。实现完发现第二层没有意义，且带来实际成本：

- 行形状不是某一行的属性。同一份代码写出来的行只有一种形状，
  不存在「这一行是旧形状、那一行是新形状」这种可靠可分辨的状态。
  逐行判据只能覆盖两种想象出来的中间态（半重建、手工塞行），而这两种都无法从行上验证。
- 行上多一格 = 多一处会分叉的副本，也要求每个写点记得带上它。
- 更关键的是判据顺序问题：两层判据要先判库再判行，而库级那格本身随水位盖章写，
  与「这份库的模型面是哪一版代码建的」并不是同一件事。

收成一条之后，语义变成单一问题：「这份库有没有那条记录、`version` 是不是我读得懂的」。
这是版本号原本要回答的问题，不多也不少。

## 为什么是并列而不是扩展 `data_face_signature`

`data_face_signature:current.schema_version` 记的是「这个库归谁、是什么数据面」，
不匹配时 gen-model **直接拒绝启动**（`data_face_signature.rs` 的 `DATA_FACE_SIGNATURE_MISMATCH`）。
拿它管行形状有两个问题：一是所有现存库会被一起拒在门外；二是它答的是数据面归属，
与「模型行的形状」是两件互不蕴含的事，混在一起会让两个升级节奏互相牵制。

## 三条正交的版本轴（不得混用）

| 轴 | 载体 | 回答的问题 |
|---|---|---|
| 存储架构版本 `MODEL_STORAGE_VERSION` | `storage_version:current.version` | 行形状**能不能读** |
| 点集求值版本 `PTSET_EVAL_VERSION` | `pe_ptset.generator` | 点**算得对不对** |
| 几何生成器指纹 | `dbnum_watermark.model_generator` | 哪一版 e3d-model **算的几何** |

求值规则改了只动 `PTSET_EVAL_VERSION`（老行照样读得懂，只是数值要重算）；
换 e3d-model 只动生成器指纹；**只有行形状变了才动 `MODEL_STORAGE_VERSION`**：
字段增删或改类型、点号的键语义变化、坐标单位或参考系变化。改读法但形状没变不加。
动 `MODEL_STORAGE_VERSION` 时必须同时改 `SHAPE_NOTE` 与 `REBUILD_NOTE`。

## 为什么不自动重建

与 `generator_reconcile`（发现生成器变了就自动整库重排）刻意不同：存储架构升级的重算量级与
影响面远大于换生成器版本，重建时机必须由人挑（低峰、逐库、可回退）。自动排队还会让
「重建中该库不可见」这段时间不受控。

## 后果

- 升级到新存储架构后，**所有未重建的库都读不到点集**，报 409 并给出重建命令。
  这是有意为之：宁可显式报错，不要静默空集。
- 老库上的 `fn::bran_comp_len` / `fn::elbo_len` / `fn::tee_len` 会 `THROW` 而不是回 0。
- 版本记录是新增表，老库没有它即「前版本化」，对旧代码无害；回退只需 `git revert`。
  曾经写过 `dbnum_watermark.model_storage_version` 与 `pe_ptset.storage_version` 的库，
  那两处残留列不再被读，`CONTENT` 写会自然把它们挤掉。
- rs-server 的 `resource/surreal/gy_common.surql` 已删除（gen-model 是唯一出厂方），
  否则 rs-server 每次启动都会把旧函数体打回库里、覆盖新版本。
- `history.txt` 里基于 `inst_relate.out.ptset` 的旧查询已标注退役。

## 未做的事

- 没有数据迁移脚本：旧数据不转换，按重新生成处理。
- 没有把版本号做进 `inst_relate` 的 `pts` 槽位（该槽位沿 ptset 计划 D6 恒为 NONE）。
- 没有处理 `common.surql` / `material_common.surql` 里其它跨仓重名函数（本 ADR 只覆盖 ptset 相关三件）。
- 没有做「哪些 dbnum 还没重建」的逐库清单：一条记录答不了这个问题，也不该由版本号回答。
