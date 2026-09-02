# 2026-09-02 模型增量更新流程审核（gen-model × e3d-model × core.dll）与下一步开发计划

- 日期：2026-09-02
- 范围：`gen-model`（`src/data_interface/{batch_worker,model_refresh,model_update_pending,generation_root}.rs`、
  `src/fast_model/e3d_model_service.rs`）、`vendor/e3d-model`（`increment.rs` / `element_diff.rs` /
  `ledger.rs` / `category.rs`）、`vendor/e3d-io`（`index::IndexDiff` / `DbSet`）。
- 取证手段：**ida-bridge 活桥**，`core.dll`（`idalib-22484`，`D:\ida_scratch\replica\core.dll.i64`，带 MSVC 符号）
  与 `Core3D.dll`（`idalib-35724`）。凡本文新增的地址都是本轮实读，可按地址复查。
- 状态：**审核结论 + 提案（未实施）**。只读审核，未改任何源码、未跑服务。
- 承接：`2026-08-31-core-aligned-increment-architecture.md`（五层管线取证）、
  `2026-09-01_increment-implementation-review.md`（凭证时效审核，F1–F4）、
  `2026-09-01-refno-scope-model-reconcile-plan.md`（reconcile 提案）、
  `vendor/e3d-model/docs/2026-09-01-next-step-development-plan.md`（几何覆盖线）。
  本文**不重复**上述文档已坐实的内容，只记增量与修正。

---

## 0. 一句话结论

增量链路现在是**两套规划器并存、只有一套在生产路上跑**：

- **gen-model 侧**（生产主路）：数据窗口经 old-pdms-io 回放落 `pe` → `model_impact` /
  `generation_root` 选根 → durable `model_update_pending`（v2 起带 `cohort_id = dbnum:sesno`）
  → drain 逐根 **整根重生成**（`E3dModelService::generate_roots`，靶标缓存 + manifest 哈希去重）。
- **e3d-model 侧**（core 对齐的 L0–L4 单元级增量 `increment_update`）：只从 `batch_worker::run_unit_worklist`
  的 **暂存窗口内** `source_window` 分支进 `ModelRefreshPolicy::apply_window`，而该入口按现码
  很可能**每次都被 pin 守卫拒绝**（§3 S1），随后整批回退到逐根整根重生成。

也就是说 core.dll 那套「索引候选 → 逐属性差分 → 语义账 → 顶层上卷」在 e3d-model 里已经实装、
118+ 项测试与五窗真库门全绿，**但生产几何并不是它算出来的**；生产靠的是「根粒度全量重算 +
manifest 哈希相等就不写」——正确性有保证（先算再比），省的是**写**不是**算**。
下一步的主轴不是再补 L2/L3，而是**把两套规划器收成一套、把 e3d-model 的增量判据接到生产的选根
位置上**，同时把本轮 core.dll 新取证的两条规则（`graphicsBehaviour==1` 图形忽略、
`attributeModified` 的依赖递归）落成守卫。

**追记（2026-09-02，P0-2 对拍落地后）**：五窗对拍把 old-pdms-io 净窗口的两处缺陷坐实——
ams7999 45→46 **幻删 2 个活元素、漏掉 22 个新建元素**，ams1112 721→722 **整窗报错**，e3d-io 同窗
账平（`docs/evidence/2026-09-02-planner-parity.md`）。

**生产形态修正（2026-09-02，用户指正「old-pdms-io 默认是不开启使用的」）**：生产跑的是
`AIOS_DATA_READ_MODE=direct`——**不起 watcher / worker，不跑 old-pdms-io 数据增量**，读取全走
e3d-io，模型按需生成（`/api/v1/model/ensure` → `generation_roots_in_subtree` →
`ensure_model_scope_generated_from_roots` → `E3dModelService::generate_roots`）。同日落地的
**ADR-054** 又把生成时点换成**文件最新会话**、凭证判据改**单调**（`source_end_sesno >= 最新`），
`apply_window` 的 pin 断言已拆。据此本文的结论要重新分层：

| 结论 | 在 direct 生产路上的地位 |
|---|---|
| §1 core.dll 取证、§3 S4（e3d-model 账本）、S5（gb==1） | **不变**，直接适用 |
| S1（暂存窗口内 pin 守卫） | 已被 ADR-054 实施约束 5 收掉；且该路径只在 legacy worker 里，direct 不走 |
| S2（两套规划器） | direct 只有 e3d-model 一套规划器可接；gen-model `model_impact` 规划器只在 legacy 路上。**S2 在 direct 上简化为「e3d-model 规划器尚未接进按需路径」** |
| P0-0（old-pdms-io 幻删/漏增/报错） | **legacy 专属**，不影响 direct；保留为「不得重新启用 legacy 增量」的证据，以及 e3d-io 底座（若将来要做文件监听式增量）的两个验收窗口 |
| **新 · S8【重大 · 效率】direct 模式没有变更检测，每次 SAVEWORK 让全部凭证过期** | ADR-054 之后「当前」= `cred >= 文件最新`；文件每前进一个会话，**该库所有根**的凭证同时失效，下一次显示到哪个根就整根重算哪个（manifest 哈希只省写不省算）。ADR-054 自己把「首显整片重生成」记为一次性切换代价，但它**每个新会话都重演一遍**。e3d-model 的 L1–L3 正是补这个洞的零件——它能回答「S→T 之间哪些根真的动了」，其余根只需**凭证前移**。这是 direct 路上的 P1-1 |
| S7（CATA → DESI） | direct 零解析时**没有 Surreal `ref_rev`**，CATA 会话推进只能靠 `ModelTarget.catalogue` 指纹失配 → 全部根重算。§1.5 的 dab 引用表读法（P2-2）在 direct 上升为 P1 |

---

## 1. core.dll 本轮新增取证（对既有文档的补全与修正）

### 1.1 `DB_Compare` 三段扫描的真实次序（结掉 0831 文档 §5 第 1、2 条「未验」）

`DB_Compare::scan`（`0x5a46600`）→ `scanOld`（`0x5a46a40`）→ `scanNew`（`0x5a46730`），**先 base 端后 target 端**。

**`scanOld`（base 端 DFS）** 对每个 base 元素切到 target 端问 `isValid`：

| 情形 | 动作 |
|---|---|
| target 端仍在，且 noun `primaryList` | 用成员表比较器（`sub_5A44CA0` / `sub_5A45410` / `sub_5A44F90`）比两端成员表，状态码 **3** 的成员塞进 `this+0x50` 集合（= **次序变了**的元素集） |
| target 端仍在 | `hasAttributeChangedBetween(el, ATT_TYPE)` 为真 → 塞进 `this+0x58` 集合（**类型变更集**） |
| target 端不在（删除） | 再开一个 base 端子树迭代器：子树里**仍在 target 端的后代**不算删除（那是被挪走的，只查 TYPE），其余收进删除向量；向量**反转**（孩子先于父）后追加到 `this+0x44..0x48`；然后 `DB_Iterator::skip` 跳过该子树 |

**`scanNew`（target 端 DFS）**：先 `checkEle(root)`；每个元素若在类型变更集里 → `skip` 整棵子树；否则
`hasElementChangedBetween` 为门，**在次序集里的元素即使记录没变也强制进 `checkEle`**；`checkEle` 置
`a4` 时 `skip` 子树（新建子树只报顶）。遍历完：类型变更集逐个 `dealWithChangeType`；最后切回 **base 会话**，
对删除向量逐条调 vtable 槽 2（`deleteAll`）。

修正三条既有认知：

1. 0831 文档说「`checkEle` 里 `sub_58E8090`/`this+20` 那个位置变了的判据没拆」——`sub_58E8090` 只是
   `std::set<DB_Element>` 的查找；**判据本体是 `scanOld` 对 `primaryList` 属主做的成员表比较（状态 3）**。
   e3d-model 的 `member_delta(...).reordered` 在属主上判、语义等价 ✅。
2. **`DB_Compare` 路径对删除子树是逐元素上报的**（孩子先于父），只有 `DB_RawChanges::isElementTopLevel*`
   才做祖先抢占。e3d-model `ledger.rs` 的「只标记不丢条目」与 `DB_Compare` 一致，不必再为 §3.5
   「已产出模型集」阻塞——那条输入只在照抄 `DB_UserChanges` 抢占口径时才是必需的。**§3.5 降级为可选**。
3. **被删子树里仍活着的后代 = 搬迁，不是删除**——`scanOld` 明确跳过。e3d-model 的 `Deleted` 候选来自索引键集差，
   搬走的元素键仍在、天然不进 Deleted 桶，**by construction 一致**（ADR-036 目标成员存活口径）。

### 1.2 `hasElementChangedBetween`（`0x593d6d0`）逐指令确认

`DB_Blob::getContentType(...) == NOUN_TUBI` → 依次 `ATT_POS / ATT_ORI / ATT_ITLE / ATT_SPRE` 四问，否则
`sub_5AAD8E0(el, s1, s2, …)`（dab `DCHELE` 记录级判据）。e3d-model `TUBI_CHANGE_GATE` 字面一致 ✅。

### 1.3 ★ `DES_DrawList::isGraphicsIgnoredBetween`（Core3D `0x1051c7d0`，结掉 0831 文档 §5 第 6 条）

```
isGraphicsIgnoredBetween(top, el):
    cur = el.owner()
    while cur.isValid() && cur != top:
        if DB_Noun::getField(cur.actualType(), 5099119 /*graphicsBehaviour*/) == 1: return true
        cur = cur.owner()
    return false
```

指令实读：`0x1051c86a  push 4DCE6Fh`（= 5099119 = `graphicsBehaviour`），`0x1051c888  cmp [ebp+var_3C], 1`。

即：**从元素上溯到顶层单元的路上，只要有一个中间祖先的 noun 字典字段 `graphicsBehaviour == 1`，
该元素的图形就被忽略**（不作为独立可画项）。这不是 XGEOMETRY，而是字典级「几何数据容器」标记。
按 `vendor/old-parse-pdms-db/noun_flags.json`（E3D 3.1，1931 noun）统计：

| `graphicsBehaviour` | noun 数 | 代表 |
|---|---:|---|
| 0 | 1652 | BOX / PANE / BRAN / EQUI … 普通元素 |
| **1** | **109** | `LOOP` `POLOOP` `RSECT` `PFACE` `PLAFAC` `PLASOL` `PSURF` `SRFDEF` `CURGEO` `BOUSUR` `EXTGEO` `PRIGEO` 及 `D*` 船体/出图数据（`DPLATE` `DSTIFF` `DSEAM`…） |
| 2 | 98 | `WORL` `SITE` `ZONE` `ROOM` `STRU*` `HPANEL` … 层级容器 |
| 3 | 72 | `CABLE` `CT*` 桥架件、`HANDRA`、`*ATTA`、`AID*`、`REF*` 等特殊画法 |

对 e3d-model 的含义：`ProfileData` 那张手抄表（PLOO/PAVE/LOOP/VERT/SPINE/…）**不等于** gb==1 集合
（`PLOO`/`PAVE`/`VERT`/`SPINE` 都是 gb==0；`RSECT`/`PFACE`/`PLASOL`/`PSURF`… 是 gb==1 却不在表里）。
今天不炸是因为语料里没有后者；船体/出图库一进来，gb==1 的中间容器若被当 `List` 下钻、其子元素若被当独立单元，
就是「core 不画、我们画了」的静默分歧。落地见 §4 P0-4。

### 1.4 ★ `DB_UserChanges::attributeModified` 是**递归**的：变更集会沿依赖扩散

`DB_UserChanges::attributeModified(el, att, qual)`（`0x5987090`）：

```
if el ∈ created: 只更新「最后新建元素」游标，不记属性       # 新建元素不记 attChange
elif att == ATT_MEMB: 记进 member-changed 集
else: 记进 (el → {att[+qualifier]}) 映射
deps = DB_UserChangesDependency::getDependencies(el, att)      # 0x59a11a0
for (dep_el, dep_att) in deps: attributeModified(dep_el, dep_att)   # ★ 递归
```

`getDependencies` 三个来源：

| 来源 | 实现 | 数据从哪来 |
|---|---|---|
| `ATT_XRPNTR` | 元素自己的交叉引用指针 → 直接加入 | 元素属性 |
| **`DB_Attribute::backDependencies(noun)`**（`0x58cf120`）→ `DB_MDB::findElements(dbType, refAttr, el)`（`0x59f4720`） | 每条 (引用属性, 受影响属性) 对，在 MDB 全部同类型库里找「谁经 refAttr 指向 el」 | 表由 `DB_AttributeValues::InsertBackDependency`（`0x58e33c0`）填充，**唯一调用方是 `DB_Uda::internalReadData`**（`0x597f6ec`）——即**只来自 UDA 定义**（UDA 表达式依赖别的元素属性） |
| `invokeSubscibers(nounHash, attId)` | 程序注册的 `DB_DependencyBase` | `addSubsciber`（`0x59a1140`）在 core.dll 内**唯一调用方是自测 `DB_Element_test_Get_Pseudo`**（`sub_5968300`）；Core3D.dll **不导入**该符号 |

**结论（修正 0831 文档 §3.4 的措辞）**：core 的 `DB_UserChanges` 确实会把变更沿依赖扩散，但扩散边只有
XRPNTR 与 **UDA 定义的依赖**，**不含 SPRE/CATR → SCOM 这类目录引用**。目录件内容变了对设计件的影响
仍走 `PostDBFileChanges` / `ClearCaches` 整库失效——与 0831 §3.4 结论一致，但现在有了正面证据。
对我们：gen-model 已有的 CATA 反向级联 planner（`model_update_plan.rs::the_cata_planner`，v2 验证清单第 4 条）
是**比 core 更细**的做法，方向正确；UDA 依赖这条 core 有、我们没有——今天 UDA 不参与几何，**记录不做**。

### 1.5 `DB_MDB::findElements` 走的是 dab **引用表**（`DB_RefTableIterator`）

`DB_RefTableIterator::startSearch`（`0x5a1d090`）= `sub_5AAF4E0(dbno, attId, targetRef, handle)` 起一次
表搜索；`increment`（`0x5a1b710`）= `sub_5AAE610(handle, …)` 取下一条、`db_finish_table_search` 收尾。
即 dabacon 库文件里有一张按 (属性, 目标 ref) 索引的**反向引用表**，`BREF`/`SPBREF` 一类伪属性与
`findElements` 都从它取。e3d-io 目前没有这张表的读法（0830 缺口体检 G8「无反向索引」）。
它对增量的价值是「目录件变了 → 谁引用它」的**权威反查**；gen-model 现用 Surreal 侧反向索引（ADR-003）
替代。是否在 e3d-io 补这张表的读法，见 §4 P2-2（先取证表结构，再决定）。

### 1.6 `DB_UserChanges` 的桶比 0831 文档记的多两个

`ElementsCreated / ElementsDeleted / ElementsModified / ElementsReordered / ElementsMemberChanged /
ElementsMoved`（`isElementMoved` `0x5983b60`），另有 `elementsChangedSince(sesno, …)`（`0x5900230`）
的单端形态。e3d-model `ChangeKind` 的 `Reparented` ↔ `Moved`、`MembersChanged` ↔ `MemberChanged` 一一对应 ✅。

---

## 2. 现状对照表（三层）

| 层 | core.dll / Core3D | e3d-model `increment.rs` | gen-model 生产路 | 判定 |
|---|---|---|---|---|
| L0 会话定位 | `switchToOldSession` | 两端 `DbSet` 各钉 sesno | `E3dModelService::build_set(dbnum, sesno)` | ✅ |
| L1 索引候选 | `DB_IndexTableCompare` 三态 | `e3d_io::IndexDiff` → `IndexCandidate` | 数据侧仍是 old-pdms-io 回放 + `session_index_diff.rs`（0830 已坐实其异常计数是 bug） | ◐ 两套 L1 |
| L2 逐元素差分 | `checkEle` + `hasAttributeChangedBetween` 按类型比值；TUBI 四属性门 | `element_diff::diff_element`（记录级快筛 + 属性表按 hash 配对比值 + `opaque` 哨兵 + TUBI 门） | **无**（`model_impact.rs` 按 noun/属性名单判 `Regen/TransformOnly/Skip`） | ◐ e3d-model 有、生产没接 |
| L3 语义账 | `DB_UserChanges` 6 桶 + UDA 依赖递归 + 祖先抢占 | `ChangeLedger` 7 桶 + 抢占只标不丢 | `EleOperationDetail{Add/Modified/Deleted}` + ADR-009 `Moved` | ◐ |
| L4 上卷 | `findTopLevelElement` / `LISTOP` / `FNDTOP` / `isGraphicsIgnoredBetween(gb==1)` | `nearest_unit`（`is_model_unit` ∥ `is_derived_unit`）+ 世界系级联 | `generation_root.rs`（MDU / significant owner；core `significant`/`primitive` 位快照已入库，判据未切） | ◐ 两套 L4，且**没有 gb==1 规则** |
| 执行 | 段树按元素重画 | 单元级 upsert/remove（`GeometryId`） | 根级整根重算 + 靶标缓存 + manifest 哈希去重（v2） | 生产是根粒度 |
| 时效凭证 | 无（进程内事件即时消化） | 无 | `gen_root.source_end_sesno == 水位` 等值（F1：未变根随水位整体失效） | ⚠️ 已知欠账 |

---

## 3. 审核发现（按严重度）

### S1【重大 · 待实测坐实】暂存窗口内的 `apply_window` 很可能永远过不了 pin 守卫

证据链（现码）：

1. `batch_worker.rs` ≈2911–2923：只有 `staged == true`（`active_staging_writes().is_some()`）的分支把
   `model_window = (start_sesno, end_sesno)` 传给 `run_unit_worklist`；非暂存路径传 `None`。
2. `run_unit_worklist` ≈3359：`source_window.is_some()` → `ModelRefreshPolicy::apply_window(dbnum, start, end)`，
   **忽略 `roots`**；`base_sesno = start_sesno - 1`（`model_refresh.rs:119`）。
3. `E3dModelService::from_current()` → `pins_from_watermark()`（`direct_store.rs:719`）**直接
   `aios_core::SUL_DB.query("SELECT … FROM dbnum_watermark")`**，读的是持久层水位；暂存窗口尾事务
   才推进水位（ADR-017 / `lifecycle.rs:432`），窗口内它还是 base。
4. `apply_window`（`e3d_model_service.rs:288`）：`ensure!(pin.sesno == Some(target_sesno))`。
   → 在窗口内 `pin.sesno == base ≠ target` → `Err("increment target session … is not the current direct pin")`。
5. `e3d_model_service.rs` **全文没有一处** `staging` / `journal` 引用；`generate_refs` 的发布事务
   `aios_core::SUL_DB.query(publication_transaction(...))` 也直写持久层。

推论（未跑，按码推）：暂存窗口内 (a) `apply_window` 恒失败 → 整批标失败 → 回退逐根 `generate_roots`；
(b) 逐根路径 `source_sesno = pin.sesno = base`——**用窗口前的数据生成几何**，且发布绕过 journal；
若尾事务失败，几何已落而数据没落。**这两条只要有一条成立就是 ADR-017 提交单元被打穿。**

坐实方法（一次）：`model_incremental = true`、epoch 0、任意稳态窗口，抓日志：出现
`批量重生成 K 个根失败（耗时 …），回退逐根重试以定位问题根: increment target session N is not the
current direct pin Some(M)`（`batch_worker.rs:3448` 那行拼上 `apply_window` 的错误）且 M = 窗口前水位
⇒ (a) 坐实；随后 `run_single_unit` 逐根走 `generate_roots`，若发布出来的 `direct_model.sesno == M`
（而非 N）⇒ (b) 坐实。

### S2【重大 · 设计】两套规划器并存、没有对拍

gen-model 的根集合（回放 + `model_impact` + `generation_root`）与 e3d-model 的计划
（`plan_update` → `regenerate/remove/regenerate_derived`）对同一窗口各算一份，**任何路径都不比较两者**。
`run_unit_worklist` 的 `apply_window` 分支成功时按 gen-model 的 `batchable` 根集合收口凭证，
而几何是 e3d-model 计划写的：e3d-model 多算的单元（级联扇出）没有凭证；gen-model 多算的根若在
e3d-model 判 `unchanged`，凭证照样推进（无害）；**两边都漏的那部分没有任何东西会响**。

### S3【中 · 效率/正确性】生产增量 = 根级全量重算，`unchanged` 与 L2 的省算没有进生产

v2 的靶标缓存（`ModelTarget` 数字指纹 + `published_geometry_count` + mesh 文件齐全）与 manifest 哈希
相等抑制写入，都是**算完再比**；L2 判 `unchanged`、TUBI 门吞噪音、只重建单元而非整根——这些 e3d-model
已实装的省算，生产一分没拿到。与 2026-09-01 审核 F1（未变根随水位整体过期）叠加：窗口一动，
全库根进入「过期」，启动 seed 风暴全额付算力。

### S4【中 · e3d-model 内部欠账，0831 §4 表遗留】

- `IncrementReport::accounts_for` 仍只有候选账与执行账，`ChangeTally` 不参与判定；`ledger.created/deleted/record`
  只在 `Ok` 分支入账，上卷失败与差分失败的候选不进账 → L3 守恒式无从成立。
- `collect_unit_subtree` 的 `visited` 每次调用新建（`increment.rs:525`）：一窗多个级联把重叠子树重复读 N 遍。
- 大窗口无成本护栏（`PROBE_MAX_CANDIDATES` 已撤，但没有「超阈值退化为整库」的策略）。
- `contributed || fanout > 0` 那行仍在（`increment.rs:731`）。

### S5【中 · 新取证】`graphicsBehaviour == 1` 规则缺失（§1.3）

`nearest_unit` 上卷与 `collect_unit_subtree` 级联都不看中间祖先的 gb；`category.rs` 对 109 个 gb==1 noun
只手抄了 `LOOP`/`POLOOP` 两个进 `ProfileData`，其余落 `dictionary_composite_category` → 多数 `Unknown`
（下钻无条件化后其子元素会被当独立元素走）。

### S6【低 · 设计注意】`base_sesno = start_sesno - 1` 是数字假设

`collect_window` 按**链序**校验端点；`start_sesno = applied_sesno + 1` 时 `base = applied_sesno`
必在链上（它是上一窗的 target），稳态无事；重建批次（水位归零后重建）与幽灵水位路由下这个等式不成立，
届时是 `UnknownSession` 整批失败 → 逐根整根重算。可接受，但要**出声**（现在只在 warnings 里）。

### S7【信息】CATA 变更 → DESI 重生成的通道在 direct/e3d 路径上是「靶标失配」不是「反查」

`ModelTarget.catalogue[]` 带每个 CATA 库的 (文件身份, 会话) 指纹，CATA 会话推进后
`cached_root_target_matches` 失配 → 下一次 `generate_roots` 全量重算该根。**谁来入队这些根**仍靠
gen-model 的 CATA 反向级联 planner（Surreal `ref_rev`，ADR-003/008）。方向对（比 core 细），
但反查源只有 Surreal 一份；e3d-io 无 G8 引用表读法（§1.5）。

---

## 4. 下一步开发计划

优先级原则（按 §0「生产形态修正」重排）：**生产是 direct 模式，主轴是把 e3d-model 的窗口差分接到
按需生成的凭证判定上（S8），其次补 e3d-model 自身欠账与 core 新规则；legacy 路上的 S1/P0-0 只留证据**。
每条带验收，不做到验收不算完成。

### P0 — direct 路上先坐实一件事（半天）

**P0-A 量 ADR-054 的重算代价，给 S8 一个数**
- 做法：direct 模式起服务，对 ams8000 任一 ZONE `ensure` 一遍（凭证全到最新）；在 E3D 里对该库做一次
  只改一个 BOX 的 SAVEWORK；再 `ensure` 同一 ZONE，看回执 `generated_root_count / cached_root_count`
  与耗时。
- 预期（按码推）：`cached_root_count = 0`，全部根重算，耗时 ≈ 首显；若如此 S8 坐实。
- 验收：数字写进 `docs/evidence/`，作为 P1-1 的 before 基线。

### P1 — 把 e3d-model 增量接进 direct 按需路径（1–2 周）

**P1-1 ★ 凭证前移（credential advance）：用 `plan_update(S → T)` 判「哪些根真的动了」**
- 位置：`ensure_model_scope_generated_from_roots` 判凭证之前，按库聚合一次：`S = 该库根凭证的最小
  source_end_sesno`，`T = 文件最新`（`model_source`）。`S == T` → 全部命中；`S < T` →
  `e3d_model::increment::collect_window(file, S, T)` + `plan_update(base@S, target@T)`
  （结果按 `(dbnum, S, T, 文件身份)` 缓存），得到受影响单元集 `regenerate ∪ remove ∪ regenerate_derived`
  与 ledger。
- 判「根 r 受影响」= 某个受影响单元的祖先链（含自身，target 图；remove 用 base 图）含 r，**或** ledger
  `Reparented(old_owner)` 的旧属主链含 r（对拍 §2.2 的旧根 manifest）。判据实现就是
  `increment_planner_parity` 里 `ancestors_inclusive` 那段，抽成 e3d-model 的 pub 函数
  `UpdatePlan::touches_roots(&[RefNo], base, target) -> BTreeSet<RefNo>`。
- 未受影响的根：`UPDATE gen_root SET source_end_sesno = T, source_end_sesno_time = …`（**不动几何、不动
  manifest、不动 revision**），一条语句批量前移；受影响的根照旧 `ensure_exact_generation_root`。
- 大窗口护栏：候选数 > 索引键数 30% 或 `plan_update` 报 `unresolved` 非空 → 放弃前移，回到全部重算
  （今天的行为），报告记 `credential_advance_degraded`。
- 验收：P0-A 同一场景 `cached_root_count = N−1`、只重算那一个 BOX 的根；`increment_real.rs` 五窗改成
  「前移后的凭证集 ≡ 两端全量生成差集的根集」再加一道门；不得让任何 `only_e3d_model`（对拍口径）
  的根被前移。

**P1-2a e3d-model 账本闭合（S4）**
- `accounts_for` 加第四条守恒：`ledger.entries` 中每个 refno 必须落在 `rolled_up ∪ no_model ∪ unresolved`
  之一；`unresolved` 侧补 `ledger.unresolved(refno, reason)`；删 `contributed || fanout > 0` 那句，
  级联 fanout 单独计 `cascade_hits`。
- `collect_unit_subtree` 的 `visited` 提到 `plan_update` 作用域共享。
- 大窗口护栏：`candidates > threshold`（默认取索引键数的 30%）时 `plan_update` 返回
  `UpdatePlan::FullRebuild{reason}`，调用方走整库；报告记 `degraded_to_full`。P1-1 的前移护栏就吃它。
- 验收：现有 160 项 + 新增 3 项单测；五窗真库门数字不变（`totals_line` 逐字节对比留档）。

**P1-2b `graphicsBehaviour` 守卫（S5，§1.3）**
- `e3d-model/data/noun-family-matrix.json` 导出加 `gb` 列（`scripts/noun_family_probe.py:116` 已在读
  `graphics_behaviour`，只差落盘）。
- `category.rs` 新增 `graphics_behaviour(noun) -> Option<u8>`；测试：`gb == 1` 的 109 个 noun
  `!is_model_unit && !is_derived_unit`，且 `classify` 不得是 `List`（否则下钻会把子元素当独立件）。
- `nearest_unit` / `collect_unit_subtree`：跨过 gb==1 中间祖先时，该候选归到上方单元（与
  `isGraphicsIgnoredBetween` 同口径），子树遍历不进 gb==1 节点之下。
- 验收：五窗真库门数字不变（语料里 gb==1 只有 LOOP/POLOOP，已按 ProfileData 消费）；新增
  `graphics_ignored_ancestors_are_not_units` 测试。

**P1-3 reconcile 一期 + 二期**（2026-09-01 方案）——ADR-054 已把等值判据改成单调，二期只剩
`data_sesno`（根子树数据版本）这一格；与 P1-1 的「根受影响集」是同一份事实，可以合并实现：
`data_sesno(r) = 最近一次 plan_update 判 r 受影响的 T`。

**P1-4 CATA 变更 → 受影响 DESI 根（direct 零解析形态）**
- 今天：CATA 会话推进 → `ModelTarget.catalogue` 指纹失配 → 全部根重算。
- 目标：对 CATA 窗口跑 `plan_update` 得变了的目录件集合，再反查「哪些设计件经 SPRE/CATR 指向它们」。
  反查源两选一：① e3d-io 实现 dab 反向引用表读法（§1.5，先取证 `sub_5AAF4E0`/`sub_5AAE610` 的表结构）；
  ② 开库时一次全库扫描建内存反向表（0830 体检 G8：30 万键约 0.4 s）。**先做 ②**，① 取证后再换。
- 验收：改一个 SCOM 参数的 CATA 窗口，只有引用它的 BRAN 成员所在根被重算。

### legacy 路上的证据（不排期，不许重新启用）

**P0-0 old-pdms-io 净窗口幻删 + 漏增 + 整窗报错**（P0-2 对拍坐实，
`docs/evidence/2026-09-02-planner-parity.md` §3/§4；**仅 legacy 模式受影响，direct 不跑它**）
- 事实：ams7999 45→46 同一窗，e3d-io 索引差分 `+22 ~1 -0`，old-pdms-io 净窗口 `+0 ~1 -3`；
  被判删的 REST `24383/72318` / HANG `24383/72319` **两读法点查都在 45、46、最新会话里**（`ses30`
  记录未动），是 ADR-036「成员补删」误判；46 新建的 22 个元素（3 个 PANE 子树）old-pdms-io
  **自己的最新会话点查能找到、钉 46 的点查找不到**，一条 Add 都没出。ams1112 721→722 收集器
  直接报 `IndexPageData.noun == 0xCC47DF` 断言、整窗失败，e3d-io 同窗 24674 候选账平。
- 生产后果：活元素被软删 + `DeleteCleanup` 清模型；新子树永不摄入、永不生成，且无账会响；
  某些窗口库水位阻断。
- 处置：direct 模式不跑它，**不排修**；若将来要做文件监听式增量，底座只能是 e3d-io
  （`2026-08-31-e3d-io-indexdiff-core-alignment.md`），这两个窗口是验收门：7999 45→46 出 22 Add /
  0 Delete；1112 721→722 能收集且 24673 Delete。

**P0-1（原）坐实 S1** — 已被 ADR-054 实施约束 5 收掉（`apply_window` 不再断言 pin），且路径只在
legacy worker；留档不排期。原文如下。
- 做法：按 §3 S1 的坐实方法跑一次稳态窗口，抓两行日志。
- 若 (a)/(b) 坐实，两条修法选一（**推荐 A**）：
  - **A · 断**：`run_unit_worklist` 暂存分支不再传 `source_window`（或 `apply_window` 入口
    `ensure!(active_staging_writes().is_none())` fail-loud），几何一律走 durable cohort 路径（写回后由
    drain 消费，`pin.sesno` 此时等于目标）。`apply_window` 保留给 P1-2 的对拍探针用。
  - **B · 修**：`E3dModelService::for_window(dbnum, target_sesno)` 允许 pin 以窗口目标覆盖，且发布语句
    进 `StagingWriteContext` journal 而不是直写 `SUL_DB`。改动面大、且与 v2 cohort 语义重叠，不推荐本期做。
- 验收：稳态窗口日志不再出现 pin 拒绝；`gen_root.source_end_sesno` 与发布几何的 `direct_model.sesno`
  等于窗口 target；写回失败注入测试下持久层无新几何行（沿用 `staging/parity.rs` 的口径加一条）。

**P0-2 双规划器对拍探针（离线，只读）** — ✅ **已落地（2026-09-02）**
- `src/bin/increment_planner_parity.rs`：同一 `(文件, base→target)`，G 侧原样调用生产纯函数
  （`collect_window` → `classify_operation_impact` → `merge_net_change_details` /
  `propagate_deletes_to_descendants` / `build_owner_overlay` → `build_unit_rollup`，持久层 `pe` 图由
  e3d-io base 端顶替），E 侧 `collect_window` → `plan_update`；按**覆盖**对拍（G 根覆盖其子树、E 单元
  被覆盖即生产会重算），三桶 `covered / only_e3d_model / only_gen_model` 逐条归因，另带 `--probe`
  两读法点查交叉核对。
- 五窗结果（`docs/evidence/2026-09-02-planner-parity.md`）：`unexplained = 0`；三窗一致或可解释
  （位姿便宜路径等价、改挂旧根是根级 manifest 的需要）；**两窗暴露的是 L1 生产缺陷**，转成 P0-0。
- 遗留：CATA 窗口未跑（探针 G 侧 `ref_reversal` 留空，跑 CATA 窗要先接 Surreal 反向索引或
  e3d-io 全库反向表）；`only_gen_model` 的 `moved_out` 旧根在 P1-1 里要由 ledger `Reparented(old_owner)`
  补排（evidence §2.2）。

（原 P0-3 / P0-4 已上移为 P1-2a / P1-2b，direct 模式同样适用。）

### （原 P1，legacy 模式下的两套规划器收口；direct 模式不适用，留档）

**原 P1-1 增量选根切到 e3d-model L1–L3**（legacy 数据管线才有 `build_model_update_plan`）
- `build_model_update_plan` 的「变更元素 → 根」输入改为 e3d-model `plan_update` 的产物
  （`UpdatePlan.regenerate/remove/regenerate_derived` + `ledger`），`model_impact.rs` 的三态
  `Regen/TransformOnly/Skip` 由 `ElementDiff` 推出：`attributes ⊆ PLACEMENT ∧ !owner_changed ∧
  !type_changed ∧ !opaque` → `TransformOnly`；`is_unchanged` → 不入队；其余 `Regen`。
  **改挂的旧根必须按 ledger `Reparented(old_owner)` 补排 `RegenRoot`**（对拍 §2.2：E 的单元键按
  refno，不会重发旧根的 manifest）。old-pdms-io 回放 + `model_impact` 降为对拍 oracle
  （`increment_planner_parity` 常驻 CI）。
- 执行仍是 durable cohort + 整根重算（不动 v2 的 CAS / manifest / 靶标缓存）。
- 验收：五窗 + 一个 CATA 窗 planner 对拍零差；入队根数 ≤ 现行（`unchanged` 与 TUBI 门生效）；
  ADR-009 `Moved` 两端根都入队的测试继续绿。

**原 P1-2（reconcile）/ 原 P1-3（CATA 经 `the_cata_planner`）/ 原 P1-4（S6 出声）** — 前两条已并入
上方 direct 版 P1-3 / P1-4；S6 出声只在 legacy `apply_window` 路径上有意义，留档不排期。

### P2 — 深化（P1 之后）

**P2-1 单元级执行（可选，需 ADR）**
- 在 P1-1 稳定后评估把执行从「整根重算」下沉到 e3d-model 的 `execute_plan`（单元 upsert/remove）。
  要解决：`gen_root` 凭证/manifest 是根级；cohort CAS 是根级；`existing_geometry_ids` 是根级 scoped delete。
  没有这三条的单元级对应物之前不做——否则是 ADR-014 分支原子替换被打穿。

**P2-2 dab 引用表读法（§1.5，e3d-io G8）**
- 先取证：`sub_5AAF4E0` / `sub_5AAE610` 对应的 dab 表类型与页布局（`.ida_scratch/probes/` 加一条），
  在 429 个真库上用纯文件探针验「(SPRE, SCOM) → 引用者集合」与全库扫描建的反向表逐键相等。
- 相等且成本可接受再实现；否则保留 Surreal `ref_rev`，把本项关掉写明原因。

**P2-3 几何输入摘要（0831 §3.6）**
- v2 的 `published_manifest_hash` 已经是「产物摘要」；§3.6 要的是「输入摘要」（算之前就知道不用算）。
  在 P1-1 之后评估：`ElementDiff.attributes ∩ 该单元实际消费的属性集 == ∅` ⇒ 跳过，属性消费集由
  `model_element` 各臂显式声明。先做 BOX/CYLI/PANE 三臂试点量收益。

---

## 5. 不许当已证 / 开放问题

1. **S1 是按码推的**，未跑。P0-1 第一步就是坐实；若 `pins_from_watermark` 在窗口内另有路由（我没找到），
   本文 §3 S1 与 P0-1 作废，改记「apply_window 在窗口内可达」并转去做 P0-2。
2. `scanOld` 里 `sub_5986450(&v48, …)` 那几处栈局部量实为 `this+0x4x` 的集合插入，Hex-Rays 类型没套上；
   不影响 §1.1 结论（集合的**用途**由 `scanNew` 读侧确认），但偏移未逐个核对。
3. `sub_5AAF4E0` / `sub_5AAE610` 是哪两个 dab 原语（`db_start_table_search`？）未从 opcode 名表反查；
   P2-2 取证时补。
4. `graphicsBehaviour == 2/3` 的语义未读（只确认 `== 1` 是忽略门）；`LISTOP`/`FNDTOP` 里没有它们，
   暂不影响上卷。
5. `DB_UserChangesDependency` 在 **其它** DLL（非 Core3D）是否有注册者未查；对本仓无影响
   （我们不复刻 UDA 依赖），仅记录。

## 附：本轮新增地址表（`core.dll` = `idalib-22484`；`Core3D.dll` = `idalib-35724`）

| 函数 | 地址 |
|---|---|
| `DB_Compare::scan` / `scanOld` / `scanNew` | `0x5a46600` / `0x5a46a40` / `0x5a46730` |
| `DB_Element::hasElementChangedBetween`（TUBI 门 + `sub_5AAD8E0`） | `0x593d6d0` |
| `DES_DrawList::isGraphicsIgnoredBetween`（字段 `0x4DCE6F`=5099119，`== 1`） | Core3D `0x1051c7d0`（`0x1051c86a` push / `0x1051c888` cmp） |
| `DB_UserChanges::attributeModified(el, att, qual)`（递归） | `0x5987090`（无 qualifier 重载 `0x5987010`） |
| `DB_UserChangesDependency::getDependencies` / `invokeSubscibers` / `addSubsciber` | `0x59a11a0` / `0x59a14b0` / `0x59a1140` |
| `DB_Attribute::backDependencies` → `DB_AttributeValues::InsertBackDependency`（唯一调用方 `DB_Uda::internalReadData`） | `0x58cf120` → `0x58e33c0`（call site `0x597f6ec`） |
| `DB_MDB::findElements(int, DB_Attribute*, DB_Element&)` | `0x59f4720` |
| `DB_RefTableIterator::{ctor, startSearch, increment, next}` | `0x5a19cd0` / `0x5a1d090` / `0x5a1b710` / `0x5a1cc20` |
| `DB_UserChanges::{ElementsMoved, ElementsMemberChanged, isElementMoved}` / `DB_DB::elementsChangedSince` | `0x5986db0` / `0x5986cf0` / `0x5983b60` / `0x5900230` |
| 自测注册者 `sub_5968300`（`DB_Element_test_Get_Pseudo`） | `0x5968300` |
