# pipe-model crate 骨架计划（Phase A 起点）

需求规格：docs/specs/pipe-modeling.md；行为依据：docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md §15-§18。

## 变更

- 根 `Cargo.toml`：workspace `members` 增加 `"pipe-model"`（`sigmap scaffold` 因无既有文件命名惯例拒绝自动命名，本计划显式定名，与 `python`/`tools/aios-patch` 同为平铺成员目录）。
- 新 crate `pipe-model/`（lib，edition 2024，publish=false）：
  - 依赖 `aios_core`（git+rev 与根 crate 逐字一致，走根 `[patch]` 的 vendor 重定向）+ `serde`。**不依赖 aios-database**——其 lib 在非默认 feature 集下编不过；骨架只需 RefU64/NamedAttrMap，EleData/persist_writer 接缝 Phase A 填充时引入。
  - `pipe-model/src/batch.rs`：`MutationBatch`/`MutationOp`/`Confirm`——服务唯一产出物，与 e3d_collector 落库行同构，UNDOABLE 边界。
  - `pipe-model/src/context.rs`：`Context`/`Flow`/`BranchEnd`——CE + Forwards/Backwards 显式化。
  - `pipe-model/src/error.rs`：`PipeError` 四级（NeedsConfirmation/RequiresChoice/Blocking/Raised）。
  - `pipe-model/src/outcome.rs`：`ConnectOutcome`/`ComponentOutcome`/`FabStatus`/`FabMessage`/`PipeWarning`——E3D 静默降级路径显式化。
  - `pipe-model/src/choice.rs`：CHOICE 答卷串编解码（§18.8），已实现 + 4 测试。
  - `pipe-model/src/naming.rs`：branch_name/split_name/normalise_new_name（§18.9），已实现 + 4 测试。
  - `pipe-model/src/settings.rs`：`PipingSettings` 默认表（§18.4/§18.13）。
  - `pipe-model/src/spec_resolver.rs`：`SpecCatalog` trait + 三级回落 `resolve`（§18.6）。
  - `pipe-model/src/service.rs`：`PipeModel` trait A-J 组签名 + `PipeModelService<C: PipeCatalog>`（读面注入）。
  - `pipe-model/src/catalog.rs`：`PipeCatalog` 读门面（noun/name/owner/members/attr/name_exists/writable/alloc_refno）。
  - `pipe-model/src/pipeline.rs`：**Phase A 已实现**——`create_pipe`（ZONE/EQUI 校验→PIPE Create→首支管自动建）、`create_branch`（PSPE/BORE 继承+自动名探测窗 1..=成员数+1）、`rename_cascade`（子树前缀改写）、`update_process_data`（只级联现值==旧值的子元素，空级联出 warning）、`edit_branch_end`（xBOR→POS→DIR→CON→L*=TRUE 逐属性保序）、`connect_end`（xREF 写入 + Connected；目标缺失→FCONN+minTube 占位显式 Unassigned）、`disconnect_end`（xREF 置 nulref）；MockCatalog + 8 测试。其余 C-J 组 stub。
  - `pipe-model/src/component.rs`：**Phase C 首批已实现**——`create_component`（BRAN+PSPE 校验→`spec_resolver::resolve` 三级回落→SPREF 写入；SELE 回落命中与落空均显式 warning；Connect/At3D 定位与逆流交换的内核依赖项入 warning 不静默）、`reselect_component`（命中写 SPREF，落空 `NeedsSelection` 空批）、`flip_component`（ARIVE/LEAVE 互换 + ANGL+180°）；`SpecCatalog` 增 `root_selector`，`PipeModelService::with_spec` 注入。
  - `pipe-model/src/testkit.rs`（#[cfg(test)]）：`MockCatalog` + `MockSpec` 共享 mock。
  - `pipe-model/src/splitmerge.rs`：**E 组已实现**——`split_branch` Feature 模式全链（切点直属+非首末校验、复制管 `COPY ATT` 过程面、`-Split(n)` 名、下游成员原序迁入、TREF↔HREF 互写；SEGMENT/PLANE 无几何内核显式 Blocking；buildInTube/法兰 return-ref 内核项进 warning）；`merge_branches`（成员原序迁入 Target 尾部、Target 尾数据拿 Source、Source 空即删+管全空级联删、跨属主移动入 requires_confirm）。
  - `pipe-model/src/specchange.rs`：**F 组已实现**——`change_spec`（PIPE/BRAN/元件三域；答卷经 `SpecCatalog::derive_answers` 沿 SELE 祖先链反推；命中写 SPREF、全落空 `NeedsSelection`；ATTA `SPKBRK` 钉住-复位序、TEE off-leg `6-(arrive+leave)` 点号 warning、BEND RADI 提醒；逐件 `SpecChangeResult` 供 Story 32 三态着色）。
  - `pipe-model/src/loader.rs`：**J 组已实现**——`import_rows` 三段协议（表头 NAME+TYPE 闸 → 行预检 VALID/EXISTS/ERROR → processData(new,update) 复选框语义）；逐列四规则（空/'unset'/LOCK 列/DBREF 补 `/`）；OWNER 空=CE；批内同名撞 handle(47,15)→FAIL；`(批, ImportReport)` 双返回。
  - `pipe-model/src/weld.rs`：**H 组已实现**——`create_welds`（相邻非 TUBI-TUBI 对接缝；`CHOOSE SPEC` 降级实测 gap→`DESP[1]`；`wldNumber=0` 手工焊不自增；`WeldPolicy{shop,styp,weld_spec,first_number}`）+ `renumber_welds`（成员序 WLDNUM）。
  - `pipe-model/src/slope.rs`：**G 组已实现**——`apply_slope`（逐 TUBI leg Rodrigues 旋转 DIR；竖直平行/无 DIR 逐 leg 显式原因 `LegResult`；位置重算待内核进 warning）。
  - `pipe-model/src/tapping.rs`：**D 组已实现**——`create_tapping` 两步提交（开孔件挂宿主成员位 + 新支管 `HREF=件/HCON='TAP'` 自动名）；host_tube 非直属成员拒绝。
  - `pipe-model/src/fabrication.rs`：**I 组结构面已实现**——`generate_spools`（每支管 PSPOOL `PSTYP=SHOP` + 每 TUBI PPIECE `BENDMC=/BENDING-MACHINE-1(3D)`/`PPCUTL=6000`，断管算法本体标 FAB_* 边界）；`validate_fabrication`（属性级缺陷→三级聚合 Valid/ValidWithWarning/Failed，`FabMessage` 带 element 定位面）；`datacon_blocking` 谓词表（§18.10）。
  - `MutationOp::Create` 增 `refno` 字段：编辑段分配标识，同批 BRAN.under 引用 PIPE.refno 得以闭环（spec 决策 source=edit 隔离段）。
  - `pipe-model/src/render.rs`：**持久化边界已实现**——`render_batch(&MutationBatch, &dyn PipeCatalog) -> RenderedBatch`，输出与 `EleOperationDetail`/`EleData`/`ModifiedElement` 字段级同构的中性 DTO（`EditOp`/`EditElement`/`EditModify`）；`NAME` 只进显式命名空间（`to_modify_surql` 契约），noun 存 `db1_hash`+`noun_name`（ModifiedElement.noun 用），`EditModify.children_changed` 带旧新成员表；批内累积状态滚动投影——同批 Create→后续 Include/SetAttrs 引用新 refno 闭环、连续 SetAttrs 出逐条 Modify（E3D 写序保留）；`Remove` 属主经 catalog 反查；基值缺失降级全量 added + warning 不吞错；`PipeCatalog` 增 `attmap_of`（diff 基值，默认 None 可省略）。
  - 根 `Cargo.toml`：新增 `pipe-model = { path = "pipe-model" }` 依赖（单向：根 → pipe-model，无环）。
  - `src/data_interface/pipe_model_adapter.rs`：**消费侧适配**——`edit_element_to_ele_data`/`edit_op_to_ele_operation`/`rendered_batch_to_ele_operations` 逐字段映射 `EditOp → EleOperationData`；产出行经既有 `terminal_persist_changes` 收口（终态合并/墓碑/refno 复用退役复用收集器同一条道）；`deleted_*`/`uda_*` 面恒空（服务不发属性删除与 UDA 写）；`sesno` 调用方赋。含 3 个字段映射测试（已执行通过）。
  - `src/data_interface/direct_pipe_catalog.rs`：**生产 `PipeCatalog`**——架 `DirectStore`（e3d-io 文件直读）上：noun=`named_attmap.get_type()`、name=`get_name()`、owner=attmap `OWNER` 快路→`element_snapshot.extraction.owner` 兜底（与 assemble_ele_data 同出处）、members=`store.members`（原序不重排）、attr/attmap=`named_attmap`、name_exists/refno_by_name=扫已 pin 库 `DbIndexes::find_named`（NAME 原文，规范化归调用方）。三个编辑会话语义显式注入不猜：`writable_dbnums`（可写库集，空集=全只读→服务建批前拒）、`edit_dbnum`（alloc_refno 取该库派生索引 refno 高水位起递增；索引不可得退 `EDIT_FALLBACK_BASE=1_000_000` 段——与摄入段隔离纪律同 MockCatalog）。

## 验证

- `cargo check -p pipe-model`：绿，零警告。
- `cargo test -p pipe-model`：72/72 绿（全部 A-L 组 + render 渲染边界 + undo_group/HSTU）。
- `cargo check -p aios-database` 与 `cargo check --tests`：绿。
- `cargo test -p aios-database --lib pipe_model_adapter`：**3/3 绿**（此前报错的 `room_membership_notice`（位于 `src/` 根的 lib 入口 1976 行）系构建时文件中间态，现行代码调用的是存在的 `room_membership_notices`，复测通过）。

## 剩余接缝

- `SpecCatalog` 生产实现缺口：仓内没有现存 SPRE→SPCO 规格表求解器（`table_lookup`/`sele_descend`/`derive_answers` 需要 E3D 规格表语义——SELE 树问答题面与 SPCO 判据匹配，§18.6/§18.8 的 PML 语义是规格依据但落库表结构未取证）。不写猜测实现；元件组操作接生产库前需先完成此面。
- `DirectPipeCatalog` 无单测：需要真库夹具（`DirectStore` 依赖 e3d-io 文件与 locator）；`PipeCatalog` 契约已由 `MockCatalog` 侧 54 测试覆盖，生产面是薄委托，接真库时以 `verify_db8000_bran_window` 类脚本做端到端验证。

## 非目标

Phase A 已填建管/支管/改名/级联/端编辑/连接批生成；**不**接连接内核（xCON 推导词、GASK/FLAN 自动补件、几何对齐兜底留 Phase C——当前显式 warning 占位）；不直连 SurrealDB；不接 ROUTER 内核（接口槽见 §18.11）。HSTU 头管选择已于 2026-10-22 经 `SpecCatalog` 接入（见末节）。

## 2026-10-20 生产读面完成

- `src/data_interface/direct_spec_catalog.rs`：`SpecCatalog` 生产实现，架 `DirectStore` 直读面。
  SPEC 即根问题节点（`root_selector`=spref 本身）；`table_lookup` 严格全答下钻（注入 `TYPE`=comp_type + TEE 老规格第三槽改写 'BORE(1)' 兜底），`sele_descend` 走 `DEFA`/`TDEF`/`EQUA`→member[1]；`derive_answers` 祖先链反推 root→leaf 序。问键编码 `QUAL>0→QUES&QUAL`，索引族（PBOR/PSHA/PWID/PHEI）QUAL=0 仍带 0 后缀；`TANS='ANY'` 通配、`ANSW..MAXA±0.001` 区间、`TUBI→TUBE` 显式别名。
- `tests/pipe_spec_forensics.rs` 新增 `spec_catalog_roundtrip`：真 CATA 库（acp7000_0001）端到端回环——SPCO `derive_answers` → `table_lookup` 命中**同一 refno**，答卷逐对相等。
- `pipe-model` 公共导出面补 `SpecCatalog`/`QuestionAnswers`。
- 验证：pipe-model 54/54；direct_spec_catalog 单测 4/4；回环 1/1；verify-plan 0 错。
- 至此 `PipeModelService` 两个读门面都有生产实现，A-J 全部功能组可对真库出批。

## 2026-10-20 L 组（制造断点/余量/管端，spec Story 51-56）

- spec `docs/specs/pipe-modeling.md` 新增 L 组 51-56（断点双形态/候选/替换、余量编辑、FTUB 管端、焊件批改）；UI spec 同步新增 K 组 41-45。证据面 §18.16。
- `pipe-model/src/fabrication.rs`：`SpoolBreakMode{NoWeld,Weld}`、`break_boundary`（Adjacent/WithTube/TeeCref 三类）、`break_candidate_ok`（TUBI itlength≥1mm、ATTA SPKBRK）、`field_weld_at`（SHOP=false WELD 成员级近似）；`set_spool_break`（标志对/焊件互替+confirm）、`remove_spool_break`、`set_end_excess`（LEXCES/AEXCES）、`set_feed_excess`（BEND.USEFED）、`create_pipe_end`（FTUB+DESP[1..3]+LSTU 继承+LIMBOSPEC+斜切校验）、`resolve_weldgap`（SpecCatalog 注入求 /WELDGAP STYPE=FWS）。
- `pipe-model/src/weld.rs`：`WeldAttrEdit`/`modify_welds_impl`——WELDTY+SHOP 成对写；MANU/MACH/ORBI 无落库列显式 warning（noun_layout 取证 WELD 词表无对应列）。
- 属性名一律 attlib 落库名（ASFBR/LSFBR/CSFBR/TSFBR/LEXCES/AEXCES/USEFED/WELDTY/LSTU/MATN/MTOC），PML 长名是别名面不落库。
- 测试 +10：64/64 绿；verify-plan 0 错。

## 2026-10-20 §18.17 归档 + splitpipe 校正（E 组证据回写实现）

- 证据文档新增 **§18.17**（老 spooler/分段实现源/边际界面归档，§20 余项近乎清空）：
  - §18.17.1 `splitpipe.pmlfnc` 全文级取证——`!!splitPipe(component, moveTo['PIPE'|'BRANCH'], number)` 精确写序；
  - §18.17.2 `splitsegment.pmlobj`（41KB）段长分段引擎方法面 + `pipesplitdebug` 配套调试表；
  - §18.17.3 `splitgensec` 结构域同名工具消歧；
  - §18.17.4 老 `spl*` spooler（spooler/ 84 文件）：`splcommonn` 的 setAtts 四参签名（xrefAtt/numAtt/maxAtt/prefAtt）编号框架 + 子类参数表（splspooln/splweldn/spljointn/splfieldn/splpartn/splbendn）；attlib 核实落库面：SPLN（全管件归属号）、SPOOL(SPLNUM/OLDSPL/SPLP)、AWELD/IWELD/RWELD(WLDN/WLDP/WLDXRF)、IJOINT(JNTNUMBER/WLDXRF)、FIELD(OLDSPL)、PIPE/BRAN(SMAX/WMAX/SPLP)、SPLDRG/ISOREGI/ISODEPT。两代数据模型并存：spl*=SPOOL+SPLN+SPLDRG/ISO 面（读兼容参照，不实现），pf*=PSPOOL/PPIECE/FTUB/WELDGAP（实现面）；
  - §18.17.5 `isocutpipe`（isodraft 切管清单报表格式编辑器，非建模）；
  - §18.17.6 `pfCallMergeSpools`/`pfmergespools`：spool 图合并=外挂批处理（写 temp_login_*.tmp、pfspoolmerge.dat、pfmergespools.mac 三件后另起 spooler 进程）——非库内语义；
  - §18.17.7 `pipeassemblyrules`：装配规则编辑表（Apply to Similar Items/Force this SPEC STYPE、EDG pickOrigin），装配生成属内核管线。
- **`pipe-model/src/splitmerge.rs` 按 PML 全文取证校正四处**（splitpipe 是 split_branch 的源头证据）：
  1. 切点边界在元件到侧——**切点元件含在迁移集**、成新支管首成员（原实现只迁下游成员）；
  2. `CONN PH TO FIRST MEM`——新支管 **HREF=切点元件**（原实现误写旧支管）；`CONN PT TO PH OF`→旧支管 TREF=新支管照旧；
  3. 末件分段**函数级放行**（PML 源码标 REVISIT 缺口，产出单成员新支管；UI 层拦截属表单侧）——校验改为仅拒首件+非直属成员；
  4. 新增 `MoveTo{NewPipe,SamePipe}`（`SplitAt::Feature.move_to`）：NewPipe=复制管+支管名 `<新管>/B1`；SamePipe=不建管+名 `<管>/B<mCount(BRAN)+1>`，撞名显式 warning（PML `handle ANY → User should rename`）；
  5. `naming::split_name` 先剥 `base` 里既有 `-Split(` 后缀再自增探测（PML `before('-Split(')`）；
  6. HCON/TCON 交叉互换（`old.tconn=new.hconn`/`new.hconn=old.tconn`）标内核依赖 warning——CONN 推导值不伪造。
- 后端 spec Story 28/30 按函数级语义改写；UI spec Story 22/23 同步（moveTo 双模、切点归新支管、原序迁入、剥后缀命名）。
- 测试 +1（split_same_pipe_mode）+ 断言强化：65/65 绿。

## 2026-10-21 会话层装配（alloc_refno 属主分库 + ACCESSDB 写权限探测）

- **`PipeCatalog::alloc_refno(&self, under)`**：E3D `NEW X` 恒落在**属主所在库**——原单一 `edit_dbnum` 注入词是错抽象（跨库操作时新元素会落错库），改按属主 refno 位解库号、每库独立水位。pipe-model 内 11 处调用点逐一指定正确属主（建管=ZONE、建支管=PIPE、建件=宿主 BRAN、PSPOOL/PPIECE=BRAN、WELD=断口侧 BRAN、FTUB=BRAN、PTAP=宿主 BRAN、分段复制管/新支管=原 PIPE、导入行=OWNER 列/CE）。
- **`src/data_interface/direct_pipe_catalog.rs`**：`edit_dbnum` 字段删除 → `waterlines: DashMap<i32,u64>` 每 dbnum 一条懒播种水位（该库派生索引已见 refno 最大值起递增；种子=0 时拒绝在真号段产裸 refno，退 `EDIT_FALLBACK_BASE` 隔离段）；`alloc_refno` 经 `DirectStore::dbnum_of(under)` 路由。构造签名 `new(store, writable_dbnums)`。
- **`probe_writable_dbnums(store, dbs, access_of)`**：`writable_dbnums` 从"接入方注入"升级为可取证。初版取**文件面** DB 元素的 `ACCESSDB`——真库探测证伪：DESI 数据文件里 `refnos_of_noun("DB")` 为空（数据文件不含 DB 元素），SYS 侧 CURD 声明元素（`TYPE=DB`）全量属性表也没有 `ACCESSDB` 列（AMS 声明面该列缺席是常态）。**证据正源是 CURD 声明元素本身**。
- **`DbAccess` 证据类型 + 穿管**（`src/data_interface/mdb_membership.rs`、`src/data_interface/mdb_closure.rs`）：`read_declaration` 在 CURD 解码处顺手采 `ACCESSDB`/`LREFO`/`CLAI` 进 `BTreeMap<u32, DbAccess>`，经 `UnboundDeclaration → MdbDeclaration → MdbMembership.access`（私有字段 + `access_of` getter）到消费面。`DbAccess::writable()` 裁决=E3D 会话默认：**声明进 MDB 的库默认可写**，仅 `ACCESSDB=Read` 或 `LREFO=true` 收窄只读（缺席≠只读——与初版 fail-closed 相反，fail-closed 只留给文件不可达/打不开的真失读）；`CLAI` 留证据不裁决（claim 是会话动态状态）。
- **`probe_writable_dbnums` 重写**：签名 `(store, dbs: impl Iterator<Item=&MdbDatabase>, access_of: impl Fn(u32)->Option<DbAccess>)`——dbs 传 `membership.element_databases()`、access_of 传 `membership.access_of`（测试可喂本地证据表，不依赖 `#[cfg(test)]` 构造器）。判定：绑不到文件→ro+problem；`store.indexes` 打不开→ro+problem；声明收窄→ro+problem；其余默认可写。
- **真库端到端验证**（`pipe_spec_forensics::pipe_catalog_session_probe` 重写断言）：
  - `ams0001_0001`（dbnum=1）：`writable={1}`（证据缺席→默认可写），alloc 出 `0x2000_00xx` 段 max+1 递增（索引水位播种生效，非降级段；dbnum 编码在打包高位随号段保持）；
  - `ams8000_0001`（DESI dbnum=8000）：`DB elements: []` 实锤文件面无 DB 元素，`writable={8000}`，alloc 同库递增、`dbnum_of` 解码回 8000——**属主→库路由与每库水位在真设计库上成立**；
  - 收窄路径单测断言：`ACCESSDB=Read`/`LREFO=true` → 排除出可写集 + problem 可见。
- `pipe-model` 65/65 绿；`pipe_model_adapter` 3/3 绿；`cargo check -p aios-database`/`--tests` 零新警告。
- **端到端链路证明**（`tests/pipe_spec_forensics.rs::pipe_service_end_to_end_on_real_db`，ams8000 DESI 真库）：真 PIPE `/1RX-3AH0`（1 支管）→ `create_branch` 出批 → `render_batch` 出 `Add(BRAN owner=pipe)` 自动名 `/1RX-3AH0/B2`、新 refno `dbnum_of` 解回 8000 → `rendered_batch_to_ele_operations` 吐 `EleOperationData::Add`；`validate_fabrication` 纯读面在真 PIPE 上出 `Failed`+2 条定位消息（真库确有缺陷数据）。链路 `DirectStore→Catalog→Service→MutationBatch→render→adapter→EleOperationData→terminal_persist_changes` 在真数据上闭合。
- `sigmap review-pr`：无 finding（scope/tests/blast-radius/sensitive 全清）；`pipe-model/` 等未跟踪新文件不进其 diff 审计面（仅审 tracked diff）。

## 2026-10-22 undo_group 组合器 + SELECT HSTU 落地

- **`undo_group(name) → [batches]→batch`**（spec 决策 3 承诺的显式跨操作回滚 API，对应 E3D `undo.add/endUndoable`）：`MutationBatch::undo_group` 静态组合器——空集即拒绝、逐批按序拼接 ops/requires_confirm/warnings；`PipeModel::undo_group` trait 默认方法委托。测试 +2。
- **`SELECT HSTU` 两处接点实现**（§18.6 `sel hstu`、componentcreation 建支管序列、specification :404 BRAN.PSPE 变更自动重选）：
  - `PipeModelService::select_hstu(pspe, bore_mm) → (Option<RefU64>, Option<PipeWarning>)`（pub(crate)）：选择键 `PBOR0`——真库 TUBE 树问键即 `PBOR0`（§18.8 取证），**不走** `resolve` 的 `PBOR0→PBOR1` 归一（那是元件 arrive/leave 孔槽语义，管段单孔位）；`table_lookup` 直查空 → `root_selector`+`sele_descend` SELE 默认链回落（与元件同链）；命中 `is_spco` 即返回。规格读面未接/落选均出显式 warning（E3D 此处静默失败，本服务不吞）。
  - `pipe-model/src/pipeline.rs` 的 `branch_create_ops`：BRAN Create 属性面增 `HSTU`（命中才写）——建管/建支管两条路共用。
  - `pipe-model/src/specchange.rs` 的 `rebranch_spec`：原"待内核"warning 换成真实现——写 PSPE 后读 `BRAN.HBOR` 调 `select_hstu(new_spec)`，命中即写 `HSTU` SetAttrs（序：PSPE→HSTU→逐件重选）；HBOR 缺席出"无法重选"warning。
  - 生产侧零改动：`DirectSpecCatalog::table_lookup` 的 `TUBI→TUBE` 别名已在，HSTU 走的就是元件同一张规格表。
- 测试 +5（72/72 绿）：接 spec 建管写 HSTU/未接 spec 出 warning 不阻断/直查+SELE 双落空显式落选/BRAN 换规格 HSTU 重选+写序断言/无 HBOR 显式 warning。

## 2026-10-26 规格表家族级真库验证（证据 §18.19）

- `tests/pipe_spec_forensics.rs` 新增 `spec_catalog_roundtrip_by_family` + `hit_bands_cover` 帮手：真 CATA 库 `ams7000_0001`（20 SPEC/14421 SELE/11858 SPCO）每 25 个抽样 475 个 SPCO 逐个 `derive_answers→table_lookup` 回环——**475/475 通过、23 家族全覆盖**（472 逐字等价 + 3 INSU band-cover 等价）。
- 取证修正（证据文档新增 §18.19）：真库数值档位**相邻共界**（TEMP 11 档 `[-104,-91]…[-1,10]`，边界值同时是两档端点）+ **SPCO 级答案可为重叠区间**（`[300,500]`/`[400,500]` 并存于同一问层）——成员序首匹配即裁决器，命中前一档是 E3D 合法选择非 resolver bug；断言纪律改为"命中各档区间覆盖输入答卷"，逐字相等在共界/重叠数据下会误报。
- 提交 `eb57f9870`（undo_group+HSTU+移交文档 `docs/agent-handoff/pipe-model-handover.md`）。
