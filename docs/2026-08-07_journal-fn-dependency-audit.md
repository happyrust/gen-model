# journal / 收口事务的 `fn::` 依赖审计（2026-08-07，W6）

> 出处：`docs/plans/2026-08-07-staged-ancestor-parse-preload-plan.md` §4 W6。
> 口径：W3（datacenter resolve-then-render）与 W4（生成字面量已解值渲染）落地后，
> 逐语句渲染器盘点。**「硬依赖」= 该语句在写回重放或收口尾事务里对持久层求值
> `fn::`，函数缺失 ⇒ 水位推进确定性失败（issue #16 形态）。**

## 1. 结论

1. **journal（Both / CommitOnly）语句已全部纯数据化**：写回重放不再对持久层求值
   任何 `fn::`。
2. 收口尾事务剩**一处** `fn::` 硬依赖：OWNER 搬迁的 `anc`/`zone_refno` 定点重算
   （层级查询优化 P1 的搬家维护），用 `fn::anc_u64` + `fn::find_ancestor_type`，
   仅出现在含搬迁的窗口。
3. **issue #16 预检：保留，探针对象已改**——从「datacenter 曾用的
   `fn::find_ancestor_types`」改为「剩余收口硬依赖 `fn::anc_u64` +
   `fn::find_ancestor_type`」。三者同出 `resource/surreal/common.surql`，探针的
   实质仍是「这份脚本灌没灌进当前库、版本够不够新」——但按实际消费者探，旧版
   脚本（有 find_ancestor_type、没有 P1 新增的 anc_u64）现在会被正确拒绝。

## 2. 逐渲染器清单

### 2.1 journal（Both）——写回逐块重放的语句

| 渲染器 | 内容 | fn:: |
|---|---|---|
| `IncrementPipeline::render_persist_statements`（解析写：pe / 名词表 / ses / pe_owner） | pdms_io `to_surql` 纯字面量 | 无 |
| `cata_closure::ensure_cata_refnos_parsed`（CATA/DESI 按需解析产物） | `gen_sur_json` 系列纯字面量 + OwnerReplace 事务 | 无 |
| `manual_update::build_reverse_index_statements`（ref_rev） | 显式 id UPSERT/DELETE | 无 |
| `pdms_inst::save_instance_data`（inst_relate 普通行 + TUBI 行，替换事务） | **W4 起** zone_refno/anc/dbnum/dt 全为已解值 | 无（回退即红钉：`generation_literals_are_pure_data_with_no_inline_fn_calls`） |
| `cata_model::gen_cata_geos`（tubi_relate 边） | **W4 起** anc/dbnum 已解值 | 无（同一钉覆盖） |
| inst_geo / geo_relate / inst_info / trans / aabb / vec3 / neg_relate / ngmr_relate | 内容寻址纯字面量 | 无 |
| `increment_manager::refresh_world_transform_products`（world_trans 指针 + aabb） | 固定 id UPDATE | 无 |
| `helper::render_cascade_delete`（删除级联） | 事务内变量只引用**本事务**刚读的行 | 无 |
| 房间边（`room_model` 的 room_relate / room_panel_relate 写） | 显式 id INSERT/DELETE（2026-08-06 修复后形态） | 无 |

### 2.2 收口尾事务（`render_finalize_tail` + `window_statements`）

| 来源 | 内容 | fn:: | 裁决 |
|---|---|---|---|
| `render_finalize_tail`（水位 / durable pending / attempts 清除 / 空间意图 / revision 收口） | 固定 id 语句 | 无 | — |
| datacenter 状态语句（`resolve_datacenter_statements_with`） | **W3 起**固定目标 id 纯 UPDATE | 无（回退即红钉：`resolved_statements_carry_no_server_side_walks`） | — |
| `render_anc_repair_statements`（OWNER 搬迁的 anc/zone_refno 子树定点重算，P1） | `UPDATE (SELECT … WHERE anc CONTAINS n) SET anc = fn::anc_u64(in), zone_refno = fn::find_ancestor_type(in, 'ZONE')` | **`fn::anc_u64` + `fn::find_ancestor_type`** | **剩余唯一收口硬依赖**。提交时对持久层重算是它的设计选择（受影响行集只在重放后可枚举）；预检探针已对准它。仅出现在含搬迁的 **DESI** 窗口（2026-08-07 审核修复 P2：非 DESI 窗口不再渲染，消费者范围与 DESI 预检的保护范围自此对齐） |

### 2.3 非收口路径（缺函数不卡水位）

| 来源 | fn:: | 性质 |
|---|---|---|
| `pdms_inst::backfill_inst_relate_anc`（启动自愈回填） | `fn::anc_u64` / `fn::find_ancestor_type` | 直打持久层、失败只 eprintln、下次启动重试（软依赖） |
| `selfcheck_surreal_functions` / `desi_finalize_preflight` 探针 | `fn::anc_u64` / `fn::find_ancestor_type` | 探针本体 |
| 读侧（rs-core 查询、材料表 surql、房间归属） | `fn::ancestor`、`fn::find_ancestor_type(s)`、`fn::ses_date`、`fn::room_code`、`fn::room_num_of`、`fn::room_relate_of`、`fn::newest_pe`、`fn::get_mdb_dbnums` … | 影响读查询正确性，不 gate 水位；灌库版本管理是同一份 common.surql 的部署议题 |

### 2.4 已消失的依赖（对照 2026-08-07 方案事实基线）

| 曾经 | 去向 |
|---|---|
| 收口 datacenter 语句的 `fn::find_ancestor_types` / `$pe.owner.owner` 现场上溯（事实基线 4，issue #16 的直接故障面） | W3：渲染时 Rust 合成链（窗口 overlay + 持久层窗口前态）解出固定目标 |
| journal 里 inst_relate 字面量的 `fn::find_ancestor_type` / `fn::ses_date`（事实基线 5）与 P1 内联的 `fn::anc_u64` / `.dbnum` | W4：渲染期已解值（`resolve_inst_meta`），引擎内 `==` 对拍钉住等价 |
| CommitOnly 的 `zone_refno = NONE` 全局回填（事实基线 5 提到的擦屁股语句） | 生产代码中已不存在（仅测试夹具保留其形态作 CommitOnly 语义示例）；其角色由启动自愈回填 `backfill_inst_relate_anc` 承担 |

## 3. 待立项（本审计只登记，不实施）

1. **`fn::ancestor` 展开层数扩容 + 灌库版本验证**（D9 另立项）：9 跳静默截断在
   直写模式今天就存在；W1 的 Rust 探针只保护模型工作项种子。扩容要连着
   common.surql 的部署故事（版本指纹 / 启动校验）一起做。
2. **`render_anc_repair_statements` 的 resolve-then-render 评估**（P1 线）：受影
   响行集要在重放后才可枚举，搬进窗口需要「按 anc CONTAINS 预枚举 + 净态重算」
   的新机制；在那之前它是预检探针存在的理由。
3. 双跑套件里仍在排练旧字面量形态的用例（`dual_anc_u64_functions_execute_and_agree`
   的字面量段）可在 P1 读侧切换完成后改为排练已解值形态。
