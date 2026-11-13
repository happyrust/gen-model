# 增量更新变更类型总表（E3D core × gen-model × 案例 × 覆盖）

> 2026-11-03。变更类型编号一律带 `CT-` 前缀：CT-A3 是「位姿变化」这一类，INC-A03 是登记表里的 HVAC 案例，两套编号互不相干。
>
> 案例目录只有一份：INC 登记表 `tests/fixtures/increment_cases.json`（人读版由 `scripts/increment_cases_report.py` 渲染）。每条案例的 `change_types` 是它自身连同所挂资产覆盖的变更类型（取并集）；`assets` 挂执行资产：`suite:<套件用例 id>` 指套件清单里的用例（`scripts/e3d/increment_suite/suite-manifest.json`，原先 35 例已跑 TTY，2026-11-04 加的 7 例已在隔离副本跑通），`inplace:<原地宏对 id>` 指 P79–P85（U 组）。`tests/increment_cases_registry.rs` 钉住编号合法、套件用例存在且只挂一处、与清单的 `registration` 一致。第 4 节「INC」「执行资产」两列由登记表反查得来，改了登记表要同步这两列；对不上时以登记表为准。
>
> 证据：E3D 核心库 core.dll、Core3D.dll 的只读逆向（ida，不存盘）；仓库文档 `docs/2026-09-04_model-cascade-update-inventory.md`、`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md`（下文简称 09-04、09-15）、`docs/specs/incr-gen-audit-aux/partial-update-branch-matrix.md`、`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md`；代码：e3d-model 的 `src/{element_diff,ledger,increment,transform}.rs`，gen-model 的 `src/data_interface/{model_impact,window_root_plan,task_change_list}.rs` 等。
>
> 标记：**待核** = 只有伪代码或推断；**需 TTY** = 只能在 E3D 里跑宏才能定。
>
> 2026-11-12 订正（依据写在各处）：CT-S6 / CT-S7 的 PML 写法（TTY 实测 `REORDER`、`CHANGETYPE TO` 两个句子成立）；刚体前移便宜路径、电缆端点级联的缺省改写为「开」（与 `src/options.rs` 一致）；CT-D5 补主关节的三条腿；新增 CT-D11（支撑件的管外径，GitHub #143），全表由 44 类变 45 类；第 6 节单列「只能回放」一档。

## 0. 结论

- E3D 的事件面已经摸全：元素级 17 个事件、会话级 7 种派发（`DB_Operation` 已认出 0、4–8）、跨会话对比 12 个回调槽（按 vtable 实名，归 7 类）+ 类型变化。
- gen-model 的场景对应 E3D 的 **GETWORK / 跨会话对比** 那条路（读已存会话的两端差异），不是会话内事件流。所以 gen-model 只看「净窗口」：同窗内建了又删、两次写同一属性这类中间步骤天然看不见，这是设计如此。
- gen-model 的 8 类 `ChangeKind` 对的是 `elementsChangedBetween` 实际写出的创建、删除、改挂、成员和属性，不是 `DB_RecordCompare` 虚表上的 12 个槽。这条函数不调用 `ruleChange`、辅链表回调和 `dealWithChangeType`（见 §2.4）。规则和次级列表在 gen-model 里没有专门字段，落成具名属性还是 opaque 仍是推断（CT-A12、CT-A13）。
- 45 类里**没有一类四格全满**：③（gen-model 对 E3D 真值 RVM）、④（属性双侧对拍）在所有案例上都还是空的。
- ② 的 E3D 真值 RVM：E04-spre-swap、P79–P85（apply 腿仍待校准）、2026-11-04 的 7 对（`20261104-112724`）和随后的三对加 ORI 同值（`20261104-140106`、过滤跑 `20261104-164831`）。同一天把旧套件在这份 setup 上重跑（`20261104-145550` 写到 E04 前：pass 62 / fail 6；过滤跑把设备复制、复合变换、规格换件、墙板位移补成 pass）。据此有真值证据的是 18 类：原来的 CT-S1、CT-S3、CT-S6、CT-A1、CT-A6、CT-A8、CT-C2、CT-W1，七对升上来的 CT-S5、CT-A3、CT-A4、CT-A11、CT-D10，再加上这次整套对拍升上来的 CT-S2、CT-D4、CT-D8、CT-W2、CT-W9。P79–P85 的 apply 腿仍待校准。
- 分档（第 6 节）：有真值证据 18 类，只有 ① 4 类，只能回放且回放已盖到 4 类，有 INC 案例但没有宏对 4 类，什么都没有 14 类，不适用 1 类（CT-W8）。

## 1. 读法

- 五族：**S** 结构（7 类）、**A** 属性（13 类）、**C** 目录 / 跨库（4 类）、**D** 派生 / 级联（11 类）、**W** 会话 / 窗口（10 类），共 45 类。
- 会话机制类（CT-W3、CT-W4、CT-W5、CT-W10）只能在回放里造，E3D 侧的 ①② 本来就填不满：覆盖里单列「只能回放」一档，回放盖到就算盖到（2026-11-12 定）。
- 覆盖四格，一类变更四格都有才算覆盖：
  - ① 宏对在 TTY 跑通、会话推进；
  - ② E3D 前后 RVM 符合预期变化，restore 回到 before；不该动几何的（如 CT-A1），② 的判据是「RVM 不变」；
  - ③ RVM 真值对拍：gen-model 输出 vs E3D after-RVM，AABB ≤ 1 mm；
  - ④ 属性双侧对拍：DB 侧 vs E3D 会话。
- 「执行资产」一列：
  - 原先 35 例已在隔离副本上跑通 TTY（清单 `tty_test_status`：2026-10-28 批 34/34 个用例腿；E04-spre-swap 另记在它自己的 `tty_status` 里），① 全有。2026-11-04 的七对、三对和 A09-ori-same 已在隔离副本跑通，RVM 按实测锁定。同一天把旧套件在这份 setup 上重跑，对拍结果见第 6 节；
  - 括号里是 ② 的 apply / restore 两腿在期望模型（`rvm-truth.json`）里的判法：`J` = 可判定（judged），`C` = 待校准（calibrate），`B` = AABB 看不见（aabb_blind），`–` = 没有这条腿。这是期望，不是真值；
  - ★ = 已导出 E3D 真值 RVM：E04-spre-swap 在隔离副本上前 / 后 / 还原三份齐、Compare 通过；P79–P85 原地三份齐、restore 腿过，apply 腿仍是 calibrate（阻塞原因 `requires-db-userchanges-event-observation`，P80 另加 `rvm-omits-empty-containers`）；
  - ③④ 全空，表里不再逐格写。
- 跑法：运行中自己新建的对象（run-owned）可以原地带备份跑；动既有对象只在隔离副本上跑。
- GETWORK 路径对任何 DESI 变化一律「整顶层 REPELE」，下面「E3D 侧」一栏只写会话内交互路径的差别和跨会话回调。

## 2. E3D 侧（ida 实证）

### 2.1 三条更新路径

| 路径 | 链路 | 区分属性吗 | 与 gen-model 的关系 |
|---|---|---|---|
| 会话内交互 | `putAtt` → `internalPutAtt`（同值早退，见 §8：标量 int / double / bool、元素引用、字符串、bool 数组、元素引用数组有；int 数组和 double 数组的函数本体没有）→ WNOEVT 门 → `PostSetAttribute` → `DESDRA_SCPlugs` → `VDESPT`（DCHC≠0 才往下；规则 U/V/W/X）→ `EVALAT`（规则 A–T）→ QCHGLS → `UPGRPH` 0x1022EAEE | 按字典 DCHC 分 0–4 类 | 只作「该重画什么」的参照 |
| GETWORK / UNDO / REDO | `DB_RawChanges` → `DB_IndexTableCompare` → `FNDTOP` → `EVALAT(top, REDRAW=331445106)` → 整顶层 `REPELE` | 不区分；只看 DESI(1)、ISOD(6) | **就是 gen-model 的场景**；gen-model 比它细（跳过 / 变换 / 重生成三档） |
| 缓存 | 只认 `CACHID`（+1 计数）；include / reorder 回调为空 | — | `CACHID` 在 gen-model 是「戳」 |

DCHC 分布（字典 / 运行时）：0 类 3941 / 3657，1 类 7，2 类 4，3 类 3（`POS ORI BFORI`），4 类 315 / 293，无 514。

### 2.2 元素级事件（`DB_ElementChangesPlugger`，vtable 0x5d19234，17 槽）

| # | 事件 | Core3D `DESDRA_SCPlugs` 订阅 | 处理 |
|---|---|---|---|
| 1 | PostCreate | ✓ | 见 09-15 §9.1 |
| 2 | PreDelete | ✓ | `VDESDL`：INTUBE + REDRAW |
| 3 | CopyBegin | ✗ | — |
| 4 | CopyEnd | ✗ | — |
| 5 | PostCopyAll | ✓ | `VDESAC` → `EVALCD(el,4)`，绕过规则 |
| 6 | PreCopyAll | ✓ | 非 DRAFT：`sub_101F1633`，只做 GTPROT / PROTST 保护检查 |
| 7 | PostCopyAllMembers | ✓ | 非 DRAFT：`sub_101F14B6` = `VDESMC`，依次调 `EVALNW`（`sub_1022C43D`）、`MLSADD`（`sub_10220F82`）、`NWSCAN`（`sub_101F16DE`）；见表下 |
| 8 | PostCreateCopy | ✗ | — |
| 9 | PostInclude | ✓ | `VDESIR` → `EVALNW`（新属主端） |
| 10 | PreInclude | ✓ | `sub_101F23F4` = `VDESI1`（旧属主端） |
| 11 | PreReorder | ✓ | 见 09-15 §9.1 |
| 12 | PostReorder | ✓ | 成员相关的 QCHGLS 写者：`VDESRO`、`VDESR1`（BRAN 前一成员 / FLRCOV 属主）、`RACKO` |
| 13 | PostSetAttribute（3 参，带限定词，+52） | ✗ | Core3D 收不到限定词 |
| 14 | PostSetAttribute（2 参，+56） | ✓ | `VDESPT` → `EVALAT` |
| 15 | PostSetRefAttribute | ✓ | `VDESPF`：BAKREF 回写对端 |
| 16 | PostSetRefListAttribute | ✓ | 见 09-15 §9.1 |
| 17 | PostSetName | ✓ | 见 09-15 §9.1 |

没有 PostDelete、Move、ChangeType 事件。合法性另走 `DB_LegalityChecksPlugger`（19 槽，含 `ModifyElementAllowed(noun,noun)` = 改类型、`MoveElementAllowed`）。

PostCopyAllMembers 的两个函数（按函数内 trace 名认出）：

- `EVALNW`（trace `change/EVALNW`）：整体受图形开关门控（`UQGRAF(293)` 不成立直接返回）。目录元件类（`ICCOMP`）直接对该元素跑遍历回调；其他类型先求一个关联元素（`sub_10381250`），按它是不是自己、`sub_1021DB75` 是否成立，决定对谁跑遍历回调或再调 `sub_1022C679`，最后都调 `sub_1041FC30`；GENPRI 类型特判。PostInclude 的 `VDESIR` 也落到它。
- `NWSCAN`（trace `descases/NWSCAN`）：在新复制出的子树里逐件扫描，按类型过滤后对每件调 `EVALNW`，GENPRI 再补一次遍历回调。
- 图形关着就什么都不做，所以两者是画表 / 重画登记，不是模型数据维护（遍历回调 `sub_1021B142` / `sub_1041FC30` 未展开）。对 gen-model：复制落盘后就是一组新建（CT-S2 = CT-S1）。

### 2.3 会话级事件（`DB_DBPlugger`；派发器 `sub_5A42C80`，调用者 `sub_5A438E0`）

| kind | 事件 | 触发 | Core3D 订阅者 |
|---|---|---|---|
| 0 | PostDBFileFailedAction | `PostSaveworkFailedAction` | 无 |
| 1 | PreDBFileChanges | SAVEWORK / FLUSH 写文件（合法性 `DBFileChangesAllowed` 由 `DBSaveWorkAllowed`、`DBFlushAllowed` 调） | 无 |
| 2 | PostDBFileChanges | 同上 | 无 |
| 3 | PreLocalChanges | 本地变化前 | `DES_DrawListManager`；FAB_*（PipePieceManager、BendingMachine、FabricationMachineManager） |
| 4 | PostLocalChanges(op) | op 取值见下表 | 同上 + `DDES_GraphicsMngr` 的 `sub_1052E9F0`（处理 op 4/5/6） |
| 5 | PostClaim | — | 无 |
| 6 | PostRelease | — | 无 |

`DB_Operation` 取值（看各调用点前压栈的立即数）：

| 值 | 操作 | 出处 |
|---|---|---|
| 0 | SAVEWORK | `DB_DB::saveWork` → CommitPending(0) |
| 1–3 | 未认出 | CommitPending、Pre/PostLocalChanges 的调用点里都没有；DBFileChanges 一侧的调用点没查（待核） |
| 4 | GETWORK | PostLocalChanges |
| 5 | UNDO | `DB_Undo::prevMark` |
| 6 | REDO | `DB_Undo::nextMark` |
| 7 | QUITWORK | `DB_DB::quitWork`、`DB_MDB::quitWork`：同时压给 CommitPending、PreLocalChanges、PostLocalChanges |
| 8 | Undo 终态 | `DB_Undo::getEndState` / `getEndStateImpl` → CommitPending(8) |

带 `DB_Operation` 参数的函数共 11 个：`DB_DBPlugger` 的 CommitPending、DBFileChangesAllowed、PostDBFileChanges、PostDBFileFailedAction、PostLocalChanges、PreDBFileChanges、PreLocalChanges，另有 Status DB 一条通知链（`DB_StatusDBChanges::DBFileChangesAllowed / PostDBFileChanges`、`DB_StatusEvents::FirePre/PostDBChangesEvent`）。操作名字符串 Claim / Release / Issue / Flush / EXTRACT / Transfer 由 extract 一族函数（`sub_5834120` 等与 `DB_DB::internalRelease`）引用，BackTrack / Revert 由 `sub_5A20280`、`sub_5A24050` 引用；它们各对应哪个 op 值没查。

另有 `Pre/handle/PostHandleUserChanges`（`DB_UserChanges::TransmitChanges` 调）、`ClearCaches`、`handleUdaBuild`；`DB_ProjectEventsPlugger::PostOpenProject`；`DB_MDBPlugger` 的 postOpenMDB / preCloseMDB / postCloseMDB。图形侧没人订阅 DBFileChanges / Claim / Release：存盘、认领、释放都不触发重画。

对 gen-model：这些都是会话内或会话边界的通知。gen-model 只读已存会话，op 值只用来判断哪种操作会产生新会话：落盘的只有 SAVEWORK（op 0）和 extract 的 FLUSH（kind 1 / 2 的写文件通知）；UNDO / REDO / QUIT 不接 SAVEWORK 就不产生会话（CT-W8）。

### 2.4 跨会话对比回调 ↔ gen-model

槽位按 vtable 实名坐实：

| `DB_RecordCompare` 槽（vtable 0x5d56668） | 回调 | 参数（`DB_RevertCompare` 0x5d565e0 的签名 / 调用处推断） |
|---|---|---|
| [2] | deleteAll | (el) |
| [3] / [4] / [5] | createAfter / createFirst / createNull | (el, 前一兄弟) / (el, 属主) / (el) |
| [6] / [7] | reorderAfter / reorderFirst | — |
| [8] / [9] | includeAfter / includeFirst | — |
| [10] | attChange | (el, 属性, 旧值, 新值) |
| [11] | ruleChange | (el, 属性, 旧 `DB_Rule`, 新 `DB_Rule`)：规则挂在单个属性上 |
| [12] / [13] | insertInSecondaryList / removeFromSecondaryList | — |

基类 `DB_Compare`（0x5d5662c）槽 0 = scan、槽 1 = 析构、槽 2–13 纯虚；`DB_RevertCompare` 是同一组 12 个回调、从槽 0 排起，REVERT 对 gen-model 就是普通的前进会话。其余对比类：`DB_FullCompare`（12 个纯虚）、`RCF_Compare`（14 槽未命名）、`DB_IndexTableCompare` / `DB_ClaimTableCompare` / `DB_SystemTableCompare`（各 1 槽，比的是表不是元素）。`DB_RawChanges` 四个集合：created / modified / deleted / changedType。

| core 回调 | 含义 | gen-model L2 字段 | gen-model L3 `ChangeKind` |
|---|---|---|---|
| createFirst / createAfter / createNull | 新建（按插入位置） | L1 Inserted 候选 | Created |
| deleteAll | 删除 | L1 Deleted 候选 | Deleted |
| dealWithChangeType（0x5a46460） | 类型变：删了按原位重建整棵子树。只从 `scanNew` 进入，`elementsChangedBetween` 不调用 | `type_changed`（noun_hash 不同；属性不比） | TypeChanged |
| includeFirst / includeAfter | 改挂（回调读旧属主） | `owner_changed` | Reparented |
| attChange(ATT_MEMB) / reorderFirst / reorderAfter | 成员增删 / 纯重排 | `members`（增、删、纯换序分开） | MembersChanged |
| attChange | 属性改 | `attributes`（按 hash 配对；无名显式条目报 `#%08X` 伪名） | AttributesModified |
| ruleChange | 某属性的规则改了（值不一定变） | 没有专门字段：规则存储若在显式区 → 伪名属性；若在记录别处 → `opaque`；规则执行改了值则另有 attChange | AttributesModified 或 OpaqueRecordChange（推断，CT-A12） |
| insertIn / removeFromSecondaryList | 次级列表（主层级之外的列表成员关系） | 解码层没有这个概念（e3d-io 里 secondary 只指文件次级区段）；落到宿主记录 → 属性或 `opaque` | 同上（推断，CT-A13） |
| （无） | 记录字节变了但指不出哪项 | `opaque` | OpaqueRecordChange |
| （无，core 会直接抛错） | 读不出 / 属主链断 | — | Unresolved |

2026-11-04 用 ida-bridge 对 `core.dll` 复核了这些槽实际被谁调用（`idalib-10104`，查完已退出，没有保存数据库）。gen-model 的 `ChangeBucket` 对齐的是 `DB_DB::elementsChangedBetween`（`0x58ffc50`），不是 `DB_RecordCompare`。

`elementsChangedBetween` 对索引里每个变化元素调用 `attributesChangedBetween`（`0x5928100`）。这个函数先调用 `hasElementChangedBetween`：`TUBI` 上 `POS`、`ORI`、`ITLE`、`SPRE` 四个都没变就返回失败，属性名单保持空，于是这根管子不产生 `attributeModified`。gen-model 的 `attribute_diff_passes_noun_gate` 做的是同一件事。`ATT_OWNER` 记 `elementIncluded`，其余属性记 `attributeModified`。`DB_Noun::primaryList` 为真时再比成员表：差异码 `== 3` 记 `elementReordered`，循环里同时记 `attributeModified(ATT_MEMB)`。然后 `elementsDeletedBetween`（`0x5900250`）和 `elementsInsertedBetween`（`0x5901010`）各自把索引里每一行记成 `elementDeleted` / `elementCreated`。`elementsInsertedBetween` 走整张 `DB_IndexTableCompare`，插入行逐条入列，没有「新建父节点就跳过子树」。

这条函数不调用 `rulesChangedBetween`、`dealWithChangeType`，也不扫辅链表。`primaryList` 为假的名词连成员表都不比。

`DB_RecordCompare` 上一次看到的四种折叠，不在这条路上：

- 构造时 `stopAtFirstCreate(true)`（`0x59a0570`：`*((_BYTE *)this + 64) = 1`），`scanNew` 在新建元素处跳过子树。
- `ruleChange`（`0x59a0e50`）和 `attChange` 都调用 `attributeModified`。属性哈希对不上 `findAttribute` 时这条回调直接返回。
- `insertInSecondaryList` / `removeFromSecondaryList` 只有调试日志，不写 `DB_UserChanges`。
- `elementReordered`（`0x5988040`）如果发现该元素已在本次比较里被创建，就改创建记录，不再另记一条。

core.dll 里 `DB_RecordCompare` 的四个构造函数，只有 `(element, int, tree)` 有代码引用，调用点是 `sub_596FB60`。这个函数的日志是 `Test 1- DB element construction/destruction/copy`，对 `/ATEST` 构造一个 `DB_RecordCompare` 后立刻析构，不调用 `scan`。另外三个构造函数和拷贝构造只有数据引用。`DB_Compare::scan`（`0x5a46600`）的唯一代码调用在 `sub_5A97440`，函数内日志是 `RCF_Compare::scan`，调用链向上到 Reconfigurer（`sub_53E4D58` 的字符串是 `Save and release all for Reconfigurer`）。那是 `RCF_Compare` 自己的虚表，不是 `DB_RecordCompare`。所以这四种折叠是 `DB_RecordCompare` 回调体里的事实，core.dll 内没有生产路径执行它们。别的 DLL 仍可能经导出构造函数进来，这次没有加载 Core3D，那一侧未查。

### 2.5 类型变化单独说

- 会话内：`DB_Element::changeTypeTo` 0x5928b70 只调 setType，**不发任何 plugger 事件**。
- `dealWithChangeType`（`0x5a46460`）先调槽 [2] deleteAll(el)，再对每个 include 根按位置调 [3] createAfter(comp, prev) / [4] createFirst(comp, owner) / [5] createNull(comp)，即「删了按原位重建」。槽名已按 `DB_RecordCompare` vtable 坐实。它只被 `DB_Compare::scanNew` 调用。`elementsChangedBetween` 不走这里：引用号还在的类型变化留在变化元素名单里，继续做属性比较和成员表比较，没有单独的类型事件。
- 相关：`DB_Noun::changeType` 0x58d7630、`DB_ClientElement::changeTypeTo` 0x59a1980、`DB_RawChanges::getModifiedTypesAcrossSessions` 0x5983890（`anyTypeChange` = this+7≠0）。
- PML 入口：`CHANGETYPE TO <类型>`（2026-11-12 TTY 实测句子成立；BOX → CYLI 被拒，报 `(2,704) Element type BOX may not be changed to type CYLI`，与上面的合法性门 `ModifyElementAllowed(noun,noun)` 对得上）。哪些类型对许改待探。

## 3. gen-model 侧处理流水线

1. **窗口**：按库 `(dbnum, S, T)` 取两端索引差，得到 L1 候选 Inserted / Deleted / Modified。候选数超过 `max_candidates` 整窗退化成整库重建（`degraded_to_full`）。
2. **L2 逐元素差分**（`element_diff.rs::diff_element`，对标 `DB_Compare::checkEle`）：
   - 记录级快筛：noun_hash、属主、成员、隐式区（忽略块地址字 6..9，即纯搬迁）、显式区都相同 → `unchanged`，只计数、不算改动；
   - 否则分出 `type_changed` / `owner_changed` / `members` / `attributes`（NaN==NaN、-0==0；无名显式条目报伪名）/ `opaque`；
   - NOUN_TUBI 门：TUBI 只认 4 个门属性，其余差异算重写噪音。
3. **L3 账本**（`ledger.rs`）：8 类 `ChangeKind`；一个元素可以有多条（改挂 + 属性变各一条）；新建 / 删除子树只标记祖先抢占，不丢条目。
4. **元数据门**（gen-model `model_impact` 给谓词 → e3d-model `NoModelTier`；开关 `model_skip_metadata_only_changes`，默认开）。只对「Modified 且成员 / 属主 / 类型 / opaque 都没动」起作用：
   - `metadata_only`：只动 NAME / DESC / PURP / FUNC / LCHKDA；
   - `unread_attribute`：有属性是「DCHC=0 ∧ 生成器不读」；
   - `cache_stamp`：只有戳（`CACHID`，09-19 起加 `SPAMAP`）跳了。
   - 去向记 `no_model`：不上卷、不级联，但仍是闭包种子；房间等读账本的消费者照样看得见。
5. **L4 上卷与级联**（`increment.rs::plan_update_with`）：
   - 新建：target 端最近单元重建；跨过路由容器则容器进派生集（重算隐含管）。
   - 删除：base 端上卷；单元自身被删 → Remove；否则属主单元重建（属主也删就归并）。
   - 修改：target 端最近单元重建；**改挂 / 类型变再回 base 端上卷**（旧属主那侧也重建）；`placement_input_changed`（POS/ORI、改挂、类型变、opaque）→ **整棵子树单元重建**（比 Core3D 只改段矩阵重，属有意分歧）。
   - 桥架分支级联：CWBRAN 子树里任何变化 → 整条分支重建。
   - 现算标架级联：GENSEC / WALL / SCTN / STWALL / 挂 PFIT 的板 / 挂 TREAD 的梯段 / HANDRA 的输入（SPINE、JLDATU / PLDATU、FITT、PLOO / LOOP、RPATH 等）→ 整棵宿主子树重建。
   - 计划收齐后三道结构级联：梁 → 经 CREF 读它的关节（FIXING / SJOI）；关节 → JOIS / JOIE 指它的梁（CREF 指到别的库只记 note）；FIXING → 挂它的 GENSEC（螺栓孔 / 底板让位）。2026-11-12 起同一条腿也认带目录正体的主关节（PJOI）：改主关节 → 被它削的梁柱；改梁柱 → 认领它的主关节；挪 `PNOD` → 它名下的主关节。这三条不受 gen-model 的 `model_structural_connection_cascade` 管，开关关着也重算（09-04 第 12 类 ★1112）。
   - Remove 优先于重建；派生容器的旧槽位键（取 base 端）进 `derived_stale`。
6. **窗口规划**（gen-model `window_root_plan.rs`）：
   - 跳过层：上面的元数据门（两票：DCHC=0 ∧ 不在生成读集；WNOEVT 门在最前）。
   - 变换层：`model_transform_cheap_path`（默认开，2026-10-14 起，ADR-066；要 `model_design_reference_cascade` 也开着才分桶），只对 `is_placement_only`（POS/ORI + 戳）的根刷世界矩阵；排除 P23 名词与桥接件（`anchor_noun_exclusion`）、路由容器 / 点容器 / CWBRAN、`anchor_lineage_exclusion`。`BFORI` 按决定不进便宜路径，走重生成（`model_impact.rs:112–116`）。
   - 重生成层：根级（ADR-056 D3）。删除 / 改挂 / 类型变读 S 端属主链找旧根（`needs_base_chain`）；删除与类型变恒为闭包种子（清旧几何）。
   - 目录：CATA 窗口 + 反查，命中的根改排带根名单的 ModelOnly 任务（ADR-071 D1 / S4）；CATA 闭包缓存；`cata_dependency.rs` 全量回退。
7. **执行**（`execute_plan`）：逐单元重建；类型变后不再是单元 → Remove（先擦后画）；派生容器整链重算，类型或学科变了（如改挂到 HVAC 下）→ 退休、清旧管身。

## 4. 总表

「INC」列是 `change_types` 含这一类的登记表案例（省去 `INC-` 前缀）；「执行资产」列是这些案例挂的宏对，括号与 ★ 的含义见第 1 节。套件用例 ID 的前缀是登记表建立前的旧编号，不代表 INC 归属（见第 9 节）。

### 4.1 S 结构

| 编号 | 类型 | E3D 侧 | gen-model 侧 | INC | 执行资产（② 两腿） | 缺口 → 建议宏对 |
|---|---|---|---|---|---|---|
| CT-S1 | 新建 | PostCreate；跨会话 create* | L1 Inserted → Created；target 端上卷重建；标祖先抢占 | A02、A03、A08、B05、U01–U04 | A02-box-add（J/J）、A02-subtree-add（J/J）、B02-nbox-add（B/J）、P79★、P80★、P81★、P82★ | — |
| CT-S2 | 复制 | CopyBegin / CopyEnd / PostCreateCopy 无人订阅；PreCopyAll 只查保护；PostCopyAll → `VDESAC` → `EVALCD(el,4)` 绕过规则；PostCopyAllMembers → `VDESMC`（EVALNW + MLSADD + NWSCAN，图形侧登记）；跨会话不单列，等于一组新建 | 同 CT-S1 | A01、A02 | A02-equi-copy（J/J）、A01-bran-copy（J/J）、F03-cross-db-copy（J/J） | 2026-11-04 过滤跑：设备复制两腿 pass。跨库 restore 的无名 BOX 包围盒锁定为基线 [39000,4500,-250,41000,5500,250]（2000×1000×500），离线对拍 pass。7998 半边仍不在主库导出里 |
| CT-S3 | 删除 | PreDelete → `VDESDL`（INTUBE + REDRAW），无 PostDelete；跨会话 deleteAll | L1 Deleted → base 端上卷：单元自删 → Remove，否则属主重建；S 端链找旧根；恒为闭包种子；切片删除（ADR-088）；同窗建了又删折成 Cancelled | A01、A02、A03、B01、B05、U02、U05 | A01-delete-box（J/J）、A01-delete-keep（C/J）、A01-child-parent（J/J）、P80★、P83★ | 既有对象只在隔离副本上删（第 1 节跑法） |
| CT-S4 | 删后同名重建 | PreDelete + PostCreate；跨会话 deleteAll + create*（新 refno） | 按 refno 认身份：旧的 Deleted、新的 Created | A02 | A01-recreate-same（B/–） | 缺 restore 腿；apply 腿 AABB 看不见属预期（几何没变）；按名对拍会误报（P83 教训） |
| CT-S5 | 改挂 | PreInclude → `VDESI1`（旧属主端）、PostInclude → `VDESIR` → `EVALNW`（新属主端）；`MoveElementAllowed` 把关；跨会话 include* 读旧属主 | Reparented：target、base 两端都上卷；`placement_input_changed` → 整棵子树重建；不走便宜路径 | A01、A02、B05 | A07-owner（C/J）、A01-bran-reparent（J/J）、A02-pose-reparent（J/J） | 2026-11-04 实测 sesno 1543：BOX-POSE 从 EQ-PA 改挂到 EQ-PB，refno 不变。世界 AABB 从 [93700,19900,-100,94300,20100,100] 变成 [93900,24700,-100,94100,25300,100]（Y is E 把 600 的长边转到 N）。Q POS WRT /* 从 E 94000 N 20000 变成 E 94000 N 25000。restore（sesno 1547）挂回 EQ-PA |
| CT-S6 | 成员增删 / 纯重排 | 成员增删 = 属主的 ATT_MEMB changed，不调 `elementReordered`（P82 / P83 已证）；纯重排走 reorder* | MembersChanged（增、删、纯换序分开）；BRAN 等路由容器进派生集，重算隐含管 | A01、U04、U05 | A01-bran-add-member（J/J）、A01-bran-del-member（J/J）、P82★（FTUB 实际插在第 1 位）、P83★ | 纯重排：2026-11-12 TTY 实测 `REORDER <成员> BEFORE <成员>`、`REORDER <成员> AFTER <成员>` 成立（CE 在属主上；成员按名、按序号、按类型加序号都行）；早先的语法错是 `REORDER` 后面少写了「挪谁」（清单 `not_encoded` 的 C0x-bran-reorder 要跟着订正）。设备基本体上的两个宏对已在隔离副本跑通：清单片段 `manifest.d/T3-ident.json` 的 S6-member-reorder、S6-unnamed-reorder（②③ 未对）。**BRAN 内纯重排仍没有案例** → 宏对：同一 BRAN 内交换两元件次序，期望 MembersChanged（纯换序）、隐含管重算 |
| CT-S7 | 类型变化 | 会话内 `changeTypeTo` 只 setType、**不发事件**。`dealWithChangeType` 是删了按原位重建，但只挂在 `DB_Compare::scanNew` 上；`elementsChangedBetween` 不调用它，引用号不变的类型变化继续走属性比较。合法性 `ModifyElementAllowed(noun,noun)` | TypeChanged：两端上卷 + 子树级联；新 noun 不出几何 → Remove；路由容器变类型 → 退休清管身；恒为闭包种子；单测 BRAN→ELBO、EQUI→SUBE。`diff_element` 在 `noun_hash` 不同时清空属性差分，和 `elementsChangedBetween` 的「继续比属性」不一致，见 ADR-032 边界 C | — | 无 | **缺口**：先做语料普查（历史窗口里 TypeChanged 的频次与 noun 对，只读），再定宏对；PML 入口是 `CHANGETYPE TO <类型>`（2026-11-12 TTY 实测句子成立，BOX → CYLI 被拒），哪些类型对许改待探 |

### 4.2 A 属性

| 编号 | 类型 | E3D 侧 | gen-model 侧 | INC | 执行资产（② 两腿） | 缺口 → 建议宏对 |
|---|---|---|---|---|---|---|
| CT-A1 | 元数据（DCHC=0 且生成不读） | PostSetAttribute / PostSetName → `VDESPT` 在 DCHC=0 处停，不重画 | 元数据门 `metadata_only` / `unread_attribute` → `no_model`，不上卷不级联；房间等读账本的消费者仍可见 | A05、A07、A08、G03、U03 | A07-rename（B/J）、A07-desc（B/J）、A01-compound-rename（J/J）、G04-frmw-rename（B/J，房间侧会读）、P81★ | apply 腿 `B` 对元数据是**期望结果**：② 的判据是「RVM 不变」 |
| CT-A2 | DCHC=0 但生成读（DIR FILL GTYP PKEY UKEY VXREF） | 交互路径不重画 | 过不了 `unread_attribute` → 照常上卷重建 | A05 | 无 | **缺口** → 宏对：run-owned 元件改 GTYP 或 PKEY，期望重建该根、③ 一致 |
| CT-A3 | 位姿（DCHC=3：POS / ORI / BFORI） | 只改段矩阵（STRTRA / SEGTRA / TRASCN）。例外整重建：23 个 P23 名词；IPCOMP / IHCOMP 件（邻管 + 分支整重建）；点类（规则 S）、PLOPEN / DP*（规则 R）、被消耗成员（规则 M）、HOLC / PIPC / HNGC（规则 J/P）→ 属主重建 | POS/ORI → 整棵子树重建；`is_placement_only` 时变换层只刷矩阵（默认开，2026-10-14 起）；BFORI 不进便宜路径，按直接几何重建 | A01、A02、A03、A08、C03–C08、D06、F04、G02、G03、H03 | C01-move / rot-u / rot-e / rot-n / compound（均 J/J）、C06-noz-move（J/J）、C06-nozz-move（B/J）、G02-pane-move（J/J）、G03-pave-move（J/J）、H03-companion-move（J/J）、C05-pipe-whole（整 PIPE 拒走变换 → 重建；J/J；原地 ready）、A02-box-pos（J/J）、D06-zone-move（J/J）、D06-zone-ori（J/J）、B02-nbox-move（B/J） | 还缺：P23 名词移动（CT-D2）；PLOPEN / FIXING 移动。2026-11-04：BOX-PRIM `POS E 400` 世界平移 [400,0,0]（sesno 1523）；ZONE 子件各 +500 E（sesno 1515，见 CT-D10）；NBOX-N1 `POS E 200` 不进 RVM（sesno 1507）；ZONE-MV `ORI Y IS W AND Z IS U WRT /*` 后 BOX-MV1 世界 AABB [-20200,79500,-100,-19800,80500,100]、BOX-MV2 [-24200,79500,-100,-23800,80500,100]（sesno 1647，见 CT-D10） |
| CT-A4 | 几何（DCHC=4，315 个） | 顶层 REPELE（TOPDEL + ADDDES）；基本体 SADREP → SEGREP；目录 / 几何集合（ICCOMP IGMCOM INGCOM IG2COM IPLCOM IPTCOM SPCO SVER DATA）REPALL；容器 TRASCN | 最近单元重建；根级重生成 | A02、A03、B05 | B01-resize（J/J）、B02-nbox-resize（B/J）、B01-cyli-size（J/J） | 粒度差（E3D 段级 vs gen-model 根级，09-15 G7）只影响性能。2026-11-04 实测 sesno 1499：CYLI-1 `DIAM` 1500→2100，`HEIG` 仍 3000，AABB 从 [51250,4250,-1500,52750,5750,1500] 变成 [50950,3950,-1500,53050,6050,1500]。restore（sesno 1503）回到 1500 |
| CT-A5 | 头尾连接（DCHC=1/2，11 个） | 规则 H/I：把前一元素或自身升为第 4 类重建 | BRAN 属性变 → 路由容器进派生集，重算隐含管 | A01、C05 | A01-bran-hpos（B/J）、C05-pipe-whole | HREF / TREF 改接另一管嘴或分支 → 宏对（对端 CREF 回写归 CT-A6） |
| CT-A6 | 单引用 | PostSetRefAttribute → `VDESPF`（BAKREF 回写对端） | SPRE 等 → 最近单元重建（重解析目录）；CREF / JOIS / JOIE → 三道结构级联；STAREF / ENDREF → `cable_endpoint.rs`（#13，默认开，2026-10-17 起）；LSTU / HSTU 被 `COMPRE` 指着时 → CT-D11 | A01、U06、U07 | E04-spre-swap★（J/J）、P84★、P85★ | 结构关节、电缆端点无案例（见 CT-D5、CT-D6） |
| CT-A7 | 引用列表 | PostSetRefListAttribute | 按普通属性处理 | — | 无 | 低优先 |
| CT-A8 | 带限定词写 | 3 参 PostSetAttribute Core3D 不订阅；`DB_UserChanges` 按「属性 + 限定词」组合键记 | 只看净值，限定词过程不可见 | U06、U07 | P84★、P85★（另有 P11 / P25 / P65 为 DRAFT 类比） | 缺的是 E3D 侧事件级证据（P79–P85 的 apply 腿卡在 `requires-db-userchanges-event-observation`），与 gen-model 无关 |
| CT-A9 | 回写规则 | `VDESPT` 规则 U/V/W/X（MLSADD）、PRTREF：E3D 自己补写派生属性 | 落盘后就是普通属性变化 | — | 无 | 低 |
| CT-A10 | UDA / 伪属性 / CACHID / WNOEVT | UDA 走 `handleUdaBuild`；伪属性不落盘；CACHID 只走缓存路径；WNOEVT 属性不发事件 | UDA 变化大概率落 `opaque`（`ledger.rs` 注释：「UDA 或未解码的存储形态」）→ 过不了元数据门，按「说不出哪变了」**保守子树级联**（待实测）；CACHID / SPAMAP = 戳 → `cache_stamp`；UDA 依赖（#9）、XRPNTR（#10）按决定不做 | A05、A06 | 无 | **缺口** → 宏对：改 run-owned 元件的 UDA，看是跳过还是整棵子树重建 |
| CT-A11 | 同值写 | `internalPutAtt` 里标量 int（`0x59419d0`）、double（`0x5941ed0`）、bool（`0x5942370`）、元素引用（`0x593f220`）是 `getAtt` 后相等就返回。字符串（`0x593f680`）用 `StrEq` 相等就返回。bool 数组（`0x59415a0`，`sub_59204D0`）和元素引用数组（`0x59410c0`，`sub_59205D0` 逐个 `DB_Ref::operator==`）相等也直接返回，都不进 `dabPutAtt` / `postSetAttribute`。int 数组（`0x5940190`）和字符串数组（`0x5940e80`）的函数体里没有旧值比较。double 数组（`0x5940480`，`POS` 走类型 8 的 `putPosAtt`，`ORI` 走类型 7 的 `putDirAtt`）也不比较；这两个函数是 46 字节包装，转到 `sub_58504E0`，那里只按属性找插件，插件体未读 | 记录逐字节相同（忽略块搬迁地址）→ `unchanged`，只计数；有单测 | A09 | A09-pos-same（B/J）、A09-xlen-same（J/J）、A09-desc-same（B/J）、A09-spre-same（B/J）、A09-ori-same（B/J） | 2026-11-04 实测。EQ-SAME 空设备，POS 按原值再写，RVM 125 件不变（sesno 1530）。BOX-A1 的 XLEN 2000 再写，EQ-A 并集不动（sesno 1536）。XLEN 走已确认的标量 double 早退。POS 走 double 数组，函数本体没有早退；sesno 只 +1 是实测，不是这条伪码能解释的。DESC 与 SPRE 走已确认的字符串、元素引用早退：Q DESC 前后都是 `AIOS suite desc same`（sesno 1634，+1）；Q SPRE 前后都是 `/ACP1000-Trough/ACP1000-TBR:50`，宏写的是 `=13244/108844`（sesno 1640，+1）。两条 RVM 都不变。ORI 走方向数组，函数本体没有旧值比较：EQ-PA 的 `ORI Y IS N AND Z IS U` 再写，Q ORI 前后都是 Y is N、Z is U（sesno 2088，+1），RVM 不变。生成侧 0 重建这次没对 |
| CT-A12 | 规则变化 | `elementsChangedBetween` 不调用 `rulesChangedBetween`（`0x594cb70`）。这个函数的代码调用者只有 `ruleMastersChangedBetween` 和 `rulesChangedSince`。`DB_RecordCompare::ruleChange` 会并进 `attributeModified`，但 core.dll 内没有生产路径构造这个类（见 §2.4） | `ElementDiff` 没有规则字段。规则字节如果落在显式属性或未解码区，会变成具名属性或 `opaque`；落点没有存储布局证据，仍是推断 | — | 无 | 宏对（第一批）：给 run-owned 元件某属性设规则、不执行，SAVEWORK 后看落哪类、重建几件 |
| CT-A13 | 次级列表 | `elementsChangedBetween` 只在 `primaryList` 为真时比成员表，不扫辅链表。`DB_Compare::checkEle` 在 `primaryList` 为假时才走辅链表槽，而 `DB_RecordCompare` 的这两个槽只有日志，且该类在 core.dll 内没有生产构造者（见 §2.4） | 解码层没有次级列表概念；落到宿主记录 → 属性或 opaque；不改几何（推断） | — | 无 | 低 |

### 4.3 C 目录 / 跨库

| 编号 | 类型 | E3D 侧 | gen-model 侧 | INC | 执行资产（② 两腿） | 缺口 → 建议宏对 |
|---|---|---|---|---|---|---|
| CT-C1 | 目录真变化（改 CATA 元素） | 交互：目录 / 几何集合 REPALL；GETWORK 只看 DESI / ISOD，**不看 CATA** | CATA 窗口 + 反查（ADR-071 D1 / S4）；CATA 闭包缓存；反查失败走 `cata_dependency_full_refresh` 全量 Replace | E01–E05 | 无（INC-E04 是 Paragon 目录改的语料，没有宏对） | **缺口**：没有 PML 宏对，只能在隔离副本做 |
| CT-C2 | 规格换件（SPRE） | 见 CT-A6 | 见 CT-A6 | A01 | E04-spre-swap★（J/J） | — |
| CT-C3 | 跨库远端 | 一库的改动影响另一库的几何（如关节 CREF 指到别的库） | 单库窗只记 note（「单库窗重建不了那根梁」）；`cross_db_handoff` 待核 | — | 无 | 缺口（隔离副本） |
| CT-C4 | 非 DESI 库 | GETWORK 只看 DESI(1)、ISOD(6) | 非 DESI 窗口没有变化清单（`task_change_list.rs`） | — | 无（P20 为 DRAFT 类比） | — |

### 4.4 D 派生 / 级联

| 编号 | 类型 | E3D 侧 | gen-model 侧 | INC | 执行资产（② 两腿） | 缺口 → 建议宏对 |
|---|---|---|---|---|---|---|
| CT-D1 | 隐含管（#6） | IPCOMP / IHCOMP 件重建邻管、分支整重建 | 路由容器进派生集，`route_tubes_of` 整链重算，旧槽位键做差发 Remove | A01、A03、B05 | 间接：A01-bran-* | — |
| CT-D2 | P23 名词（23 个） | 位姿改也整重建 | 变换层排除（`anchor_noun_exclusion`） | — | 无 | 缺口 → 宏对：移动一个 P23 名词元件（隔离副本） |
| CT-D3 | 被消耗几何（NBOX 等，规则 M） | 属主重建 | 最近单元上卷 | A02 | B02-nbox-resize（B/J）、B02-nbox-add（B/J）、B02-nbox-move（B/J） | 2026-11-04 实测 sesno 1507：NBOX-N1 的 Q POS 从 E 0 到 E 200，窄口径 RVM 仍不导出负体，EQ-NBX 并集不动。PLOPEN 仍不写（`NEW PLOPEN` 曾让 des.exe 崩）。ZONE 级联见 CT-D10 |
| CT-D4 | 点 / 环容器（规则 S；PLOO、PAVE 顶点） | 属主重建 | 现算标架级联（板的 PLOO / LOOP）；变换层排除点容器 | G03 | G03-pave-move（J/J）、G02-pane-move（J/J） | 2026-11-04：墙板 `BY D 200` 两腿 pass。PAVE 的 POS 是局部坐标。V2/V3 沿局部 W 外扩 200（sesno 2166），R201-P1 并集变成 [8657,10820,6570,9040,11179,6870]；写回 W 150 后回到 before（sesno 2170） |
| CT-D5 | 结构关节（#12） | — | e3d-model 三道结构级联（单库内）。2026-11-12 起含带目录正体的主关节：改主关节 → 被它削的梁柱、改梁柱 → 认领它的主关节、挪 `PNOD` → 它名下的主关节，开关关着也重算。gen-model `structural_connection.rs` 默认关：开着时对端的根整根重算，跨库的对端交给跨库交接（分工见 09-04 第 12 类 ★1112） | — | 无 | 缺口 → 宏对：改梁端点，看 FIXING / SJOI 与另一端的梁（隔离副本）。主关节三种窗加两条对照的宏对已写（`joint/`，2026-11-12，待跑） |
| CT-D6 | 电缆端点（#13） | — | `cable_endpoint.rs` 默认开（2026-10-17 起；同库版加跨库版） | — | 无 | 缺口 |
| CT-D7 | 现算标架 / GENSEC datum | — | 现算标架级联（GENSEC / WALL / SCTN / STWALL / PFIT 板 / TREAD 梯段 / HANDRA） | — | 无 | 缺口 → 宏对：挪 GENSEC 的 SPINE 点（隔离副本） |
| CT-D8 | 房间 | — | 房间触发读账本（元数据门放行的也看得见） | D05、F04、G01、G02、G03 | G01-room-member、G02-pane-move、G03-pave-move、G04-frmw-rename | — |
| CT-D9 | 桥架分支 | — | CWBRAN 子树任何变化 → 整条分支重建 | A04、B05 | 无 | 缺口（低） |
| CT-D10 | 容器子树（#1） | 容器 TRASCN，只改矩阵 | 容器 POS/ORI → 整棵子树单元重建 | D06 | D06-zone-move（J/J）、D06-zone-ori（J/J） | 2026-11-04 实测 sesno 1515：ZONE-MV `BY E 500 WRT /*` 之后，Q POS WRT /* 显示 ZONE 自己到 E 500，BOX-MV1 / BOX-MV2 从 E 80000 到 E 80500。子件跟着 ZONE 的世界矩阵走。RVM 子树并集平移 [500,0,0]，件数不变。restore（sesno 1519）回到 E 0 / E 80000。同日 sesno 1647：`ORI Y IS W AND Z IS U WRT /*` 之后 ZONE 仍在原点，Q ORI 从 Y is N 变成 Y is W。两件 BOX 本地朝向不变，世界位置转到 W 20000 N 80000 / W 24000 N 80000。这不是平移，RVM 按两件 BOX 的绝对包围盒锁定：BOX-MV1 [-20200,79500,-100,-19800,80500,100]，BOX-MV2 [-24200,79500,-100,-23800,80500,100]。restore（sesno 1651）回到 before。生成侧整棵子树重建这次没对 |
| CT-D11 | 支撑件的管外径（09-04 第 16 类；GitHub #143） | —（`ANCI` 一族经 `COMPRE` 指着被支撑的构件，读它的 `LSTU` / `HSTU` 走到管子的目录件取外径） | direct 窗口路径 `reverse_reference::DESIGN_CASCADE_TARGET_READS`：设计侧一跳查到 `COMPRE` 的引用者之后，被引用件这两格在窗口两端取值不同才算；同库支撑件的根并进 `touched`，别的库里的随计划交给跨库交接；反查超点查预算的窗 `COMPRE` 表整张走一遍、交接照常。挂在 `model_design_reference_cascade`（默认开）下，没有单独的开关。被指构件只挪位置 / 改名不触发 | — | 无 | **缺口**（2026-11-12 新增这一格）：语料只在 APS（234 条 `COMPRE` 边，224 条跨库；AMS 0 条）→ 宏对已写（`support/`，待跑）：跨库、同库、两条对照（只挪位置、没被支撑）、大窗 |

09-04 清单里另有 AABB、gb 守卫（E3D 侧 GETWORK 判定 `sub_1052F2E0`，gb==3 无代码路径）、XGEOM（P12 类比），不单列。

### 4.5 W 会话 / 窗口

| 编号 | 类型 | E3D 侧 | gen-model 侧 | INC | 执行资产（② 两腿） | 缺口 → 建议宏对 |
|---|---|---|---|---|---|---|
| CT-W1 | 一窗多操作 | `DB_UserChanges` 逐次记，`combine` 合并 | 只看两端净差：建了又删消失（P80），两次写同属性只剩一条（P84 / P85） | A08、C08、U01–U03、U06、U07 | P79★、P80★、P81★、P84★、P85★、A01-two-roots（J/J）、C01-compound（J/J） | 中间步骤不可见是设计如此 |
| CT-W2 | 一窗跨多会话 | — | 窗口 S..T 取净差 | B01 | A01-child-parent（J/J；两腿：会话 1 删子、会话 2 删父） | ③ 要让两腿落进同一个消费窗口才测到这一类 |
| CT-W3 | 只记账会话 | — | 0 候选 → 无动作 | A06、C04、D01、E02 | 无 | 会话机制类，见第 6 节 |
| CT-W4 | 半存会话 | — | 见 INC-H01 | H01 | 无 | 会话机制类，见第 6 节 |
| CT-W5 | 回退 / 同路径换文件 / REVERT | REVERT 复用同一组对比回调（`DB_RevertCompare`） | REVERT 就是普通前进会话；回退与换文件见 INC-B06 / B07 | B06、B07、D02、D05、H04 | 无 | REVERT 无宏对（低）；其余是会话机制类，见第 6 节 |
| CT-W6 | MERGE CHANGES | 压缩会话历史（中间会话被并掉） | 只有目录路径有处理（`cata_cascade_window.rs:240`、`cata_dependency.rs:379` 全量 Replace、`reverse_reference.rs:1152`）；DESI 路径 src 里找不到以 MERGE 命名的处理，窗口 S 端会话被并掉时是报错还是退化整库待核 | — | 无 | 缺口（隔离副本）：并掉窗口 S 端所在会话后消费一次 |
| CT-W7 | Extract 操作 | core 有 doextractClaim / Release / Transfer、doissue、internalFlush、dorefresh、dodrop（操作名字符串 Claim / Release / Issue / Flush / EXTRACT / Transfer 都在这一族函数里）；图形侧无人订阅 DBFileChanges / Claim / Release | 待核（flush 回主库应表现为主库的普通会话） | — | 无 | 缺口（隔离副本） |
| CT-W8 | UNDO / REDO / QUIT | PostLocalChanges op 5 / 6 / 7；落盘要靠 SAVEWORK（op 0） | 只看已存会话 → 不适用。2026-11-12 实测：CE 进过的库即使没有 SAVEWORK，QUIT 时也多一个只记账的会话，那个会话归 CT-W3 | — | — | — |
| CT-W9 | 一窗两库 | — | 按库分窗各自规划；跨库引用只记 note | A02、A08、H03 | H03-companion-move（J/J）、F03-cross-db-copy（J/J）（另有 P21 为 DRAFT 类比） | 主库半边已实测。伙伴库 7998 的 H03 在 batch 里跳过，7998 半边要另导 |
| CT-W10 | 失败 / 退化 | PostDBFileFailedAction（SAVEWORK 失败） | 候选超限 → 整库重建；读不出 → Unresolved；单元重建失败 → Remove（先擦后画） | A04、F05、H02、H03、H05 | 无 | 由 L2 故障注入钩子承载，不需要 E3D 宏对；见第 6 节 |

## 5. P01–P85 归属（`partial-update-branch-matrix.md`）

- **DRAFT 图纸侧独有、gen-model 无对应**（27 条，不适用）：P01、P02、P19、P26–P41、P43、P71–P77。
- **DRAFT 分支、gen-model 有类比**（51 条，只作类型参照，不算覆盖）：P03–P18、P20–P25、P42、P44–P70、P78。已对上的：P06 → CT-S4、P11 / P25 / P65 → CT-A8、P12 → XGEOM、P20 → CT-C4、P21 → CT-W9。
- **core `DB_UserChanges`（与 gen-model 同源，只是 E3D 侧逐次记录）**：P79–P85，依次登记为 INC-U01–U07。
  - P79 created 祖先去重、P80 created 祖先下删除、P81 created 祖先下改属性、P82 / P83 主列表成员 ±1（不升级为 reorder）、P84 限定词清空、P85 同窗两次写同一限定词。
  - 原地三份 RVM 齐、restore 过；七例 apply 腿保持 calibrate，阻塞原因统一为 `requires-db-userchanges-event-observation`（P80 另加 `rvm-omits-empty-containers`）。

## 6. 覆盖结论

四格全满：**0 类**。按目前走到的最好一格分档：

| 档 | 类数 | 变更类型 |
|---|---|---|
| 有 E3D 真值 RVM | 18 | CT-S1 CT-S2 CT-S3 CT-S5 CT-S6 CT-A1 CT-A3 CT-A4 CT-A6 CT-A8 CT-A11 CT-C2 CT-D4 CT-D8 CT-D10 CT-W1 CT-W2 CT-W9 |
| 只有 ①（宏对跑通，② 只有期望模型或 RVM 看不见） | 4 | CT-S4 CT-A5 CT-D1 CT-D3 |
| 只能回放，回放已盖到 | 4 | CT-W3 CT-W4 CT-W5 CT-W10 |
| 有 INC 案例、没有宏对 | 4 | CT-A2 CT-A10 CT-C1 CT-D9 |
| 什么都没有 | 14 | CT-S7 CT-A7 CT-A9 CT-A12 CT-A13 CT-C3 CT-C4 CT-D2 CT-D5 CT-D6 CT-D7 CT-D11 CT-W6 CT-W7 |
| 不适用 | 1 | CT-W8 |

- 升档口径和以前一样：一类里有一条腿的 RVM 已实测锁定，这类就算走到这一格。2026-11-04 整套对拍之后新升的是 CT-S2（设备复制两腿 pass，跨库 restore 按实测包围盒锁定）、CT-D4（墙板位移两腿 pass；顶点沿局部 W 外扩 200，sesno 2166，写回后回到 before）、CT-D8（同一块墙板）、CT-W2（删子再删父三腿 pass）、CT-W9（跨库复制的主库半边）。改挂还原按名把 M5 送回 BRAN-S（sesno 2142，RVM 与改挂前逐件吻合）；链的最后一步重 COPY 并补名 BRAN-S。`20261104-174530` 上这五组对拍 pass 12 / fail 0；去掉校准后再跑 `20261104-175546`，同样 pass 12 / fail 0。CT-S4、CT-A5、CT-D3 留在「只有 ①」：同名重建和 HPOS、NBOX 都不进窄口径 RVM，这次只证实其余成员不动。CT-D1 也留着：支管增删的子树断言仍带 calibrate，并集没有锁定。P79–P85 的 apply 腿仍待校准。
- 2026-11-04 的七对、三对和 ORI 同值已经在隔离副本跑完。旧套件也在这份 setup 上重跑过，不再有「宏已写、TTY 未跑」这一档。生成侧重建次数（0 重建、整棵子树重建）这次没对，③ 仍空。
- 会话机制类（CT-W3、CT-W4、CT-W5、CT-W10）由 L0–L2 回放案例承载，E3D 侧的 ①② 本来就填不满。2026-11-12 定：单列「只能回放」一档，回放盖到就算盖到。现在的承载（`tests/increment_replay/cases/`）：CT-W3 = `ams8000-equi-move-chain`、`ams8000-equi-rotate-chain` 里的登录会话与 QUIT 尾会话；CT-W4 = move-chain 的半存竞态；CT-W5 = move-chain 的回退；CT-W10 = `ams8000-equi-fail-inject`、`ams8000-equi-data-fail-park`。这一档里回放还没盖到的形状：同路径换文件（INC-B07）、REVERT、候选超限的退化、模型根退避与判死。
- 2026-11-12 新增的 CT-D11 归「什么都没有」：离线只有库单测与只读真库测试，宏对已写、还没在 E3D 里跑。CT-D5 的主关节三条腿同样：宏对已写、待跑。
- 有 INC 案例、真缺宏对的是 CT-A2、CT-A10、CT-C1、CT-D9。

## 7. 缺口与建议宏对

顺序：先把 ③ 的承载搭起来，用已有真值做 gen-model 对 E3D after-RVM 的对拍。旧套件的 E3D 真值已经在 2026-11-04 的隔离副本上导出。之后是第一批里还没写的宏，再是第二批。

**第一批：run-owned 对象，可原地带备份跑（便宜）**

| 宏对 | 覆盖 | 期望 |
|---|---|---|
| 移动 run-owned ZONE（下挂两件 EQUI） | CT-D10、CT-A3 | 已跑 `xform/zone_move_*.mac`（INC-D06，sesno 1515）。子件世界坐标各 +500 E。③ 和生成侧整棵子树重建还没对 |
| 旋转 run-owned ZONE（下挂两件 EQUI） | CT-D10、CT-A3 | 已跑 `xform/zone_ori_*.mac`（INC-D06，sesno 1647）。`ORI Y IS W AND Z IS U WRT /*` 后两件 BOX 的世界包围盒按实测锁定，不是平移。③ 和生成侧整棵子树重建还没对 |
| EQUI 下的 BOX 改 POS | CT-A3 | 已跑 `geom/box_pos_*.mac`（INC-A02，sesno 1523）。世界平移 [400,0,0]。生成侧是否只重建该 EQUI 根这次没对 |
| POS 原样写回 + SAVEWORK | CT-A11 | 已跑 `xform/pos_same_*.mac`（INC-A09，sesno 1530）。空设备，RVM 不变。生成侧 0 重建这次没对 |
| XLEN 原样写回 + SAVEWORK | CT-A11 | 已跑 `geom/xlen_same_*.mac`（INC-A09，sesno 1536）。有几何的 DCHC=4 属性，RVM 不变。生成侧 0 重建这次没对 |
| 非空 DESC 原样写回 + SAVEWORK | CT-A11 | 已跑 `attr/desc_same_*.mac`（INC-A09，sesno 1634，+1）。空设备 EQ-DESC，Q DESC 前后都是原句，RVM 不变。生成侧 0 重建这次没对 |
| BEND 的 SPRE 按原引用再写 + SAVEWORK | CT-A11 | 已跑 `pipe/spre_same_*.mac`（INC-A09，sesno 1640，+1）。Q SPRE 前后都是 TBR:50（`=13244/108844`），RVM 不变。生成侧 0 重建这次没对 |
| ORI 原样写回 + SAVEWORK | CT-A11 | 已跑 `xform/ori_same_*.mac`（INC-A09，sesno 2088，+1）。EQ-PA 的朝向再写原句，RVM 不变。方向数组函数本体没有同值比较，+1 是实测。生成侧 0 重建这次没对 |
| 改 run-owned 元件的 UDA | CT-A10 | 判定是被跳过，还是 opaque 保守级联 |
| 改 GTYP 或 PKEY | CT-A2 | 不跳过、重建 |
| 给某属性设规则、不执行（`RULE SET` 一类，写法待核）+ SAVEWORK | CT-A12 | 判定落伪名属性还是 opaque；几何不变，期望 0 重建或保守重建一棵子树 |
| 跨位姿容器改挂 | CT-S5 | 已跑 `attr/pose_reparent_*.mac`（INC-A02，sesno 1543）。世界 AABB 变成 [93900,24700,-100,94100,25300,100]，BOX 的 POS 命令没再下。生成侧两端重建这次没对 |
| BRAN 内纯重排（`REORDER <成员> BEFORE <成员>` / `AFTER`，2026-11-12 在设备基本体上实测成立；BRAN 成员上还没试） | CT-S6 | MembersChanged（纯换序）、隐含管重算 |
| 类型变化语料普查（只读，不跑 TTY） | CT-S7 | 频次 + noun 对，决定要不要写宏 |

CT-W2 不另写宏对：A01-child-parent 已经是「会话 1 删子、会话 2 删父」，③ 只需让两腿落进同一个消费窗口。

**第二批：只在隔离副本**

| 宏对 | 覆盖 |
|---|---|
| 改 CATA 元素 | CT-C1 |
| 改梁端点 / 改 FIXING | CT-D5 |
| 只改主关节 / 只改被接的梁柱 / 只挪 `PNOD`，外加两条对照（宏对已写 `joint/`，待跑；带负体的主关节只有 APS 的 aps7166 有） | CT-D5 |
| 被支撑的管件换规格、支撑件自己不动：跨库、同库、对照、大窗（宏对已写 `support/`，待跑；语料只在 APS） | CT-D11 |
| 移动 P23 名词元件 | CT-D2 |
| 移动 NBOX（已跑 `geom/nbox_move_*`，sesno 1507：Q POS E 0→E 200，RVM 无痕） | CT-A3、CT-D3 |
| 移动 PLOPEN | CT-A3、CT-D3 |
| 挪 GENSEC 的 SPINE 点 | CT-D7 |
| MERGE CHANGES（并掉窗口 S 端所在会话） | CT-W6 |
| Extract flush | CT-W7 |

PLOPEN 不写进套件：`s4_db8000_plopen_in_pane_create.mac` 记过 `NEW PLOPEN` 让 des.exe 以 0xC0000005 崩掉。NBOX 的位移用的是已经写过的 `POS` 句。

## 8. 待核清单

- `DB_Operation` 1–3（DBFileChanges 一侧的调用点没查）；Claim / Release / Issue / Flush / Revert 各对应哪个 op。
- 类型变化：PML 入口是 `CHANGETYPE TO <类型>`（2026-11-12 实测）；哪些类型对许改待探（BOX → CYLI 被拒；管件之间、设备与子设备之间没试）。
- BRAN 内纯重排：`REORDER` 成立（2026-11-12 在设备基本体上实测）；BRAN 成员上还没试。
- 需 TTY 一例坐实的推断：ruleChange 落伪名属性还是 opaque（CT-A12）；UDA 落具名属性还是 opaque（CT-A10）；DESI 路径 S 端会话被 MERGE 掉后的行为（CT-W6）；Extract flush 回主库的表现（CT-W7）。
- `structural_connection.rs`（#12）与 e3d-model 内建关节级联的分工。
- `EVALNW` 里的遍历回调 `sub_1021B142` / `sub_1021B4EC` / `sub_1041FC30` 没展开：只影响 E3D 侧重画范围的描述，不影响 gen-model。
- 09-15 §8 未证项：TRASCN / STRTRA / SEGTRA / SADREP / REPELE / EVALST 只到伪代码；`DB_RawChanges::trim(set/vector)` 未读。`internalPutAtt` 十个重载里，同值早退已读实的是标量 int / double / bool、元素引用、字符串、bool 数组、元素引用数组。int 数组和字符串数组的函数体没有旧值比较。double 数组（含 `POS` / `ORI`）的函数体和 `putPosAtt` / `putDirAtt` 包装也不比较，插件回调未读。
- 已结：`DB_Operation` 0 / 8；`dealWithChangeType` 槽名；PostCopyAllMembers 两个未名函数（EVALNW / NWSCAN）；ruleChange 签名；DESI 路径没有 MERGE 专门处理。

## 9. 执行资产归属

旧套件用例 ID 的前缀是登记表建立前的旧编号，不代表 INC 归属。2026-11-03 按各宏文件头的「登记:」和实际操作，把清单里 20 例的 `registration` 改成真正承载它的 INC 案例，登记表 `assets` 同步挂上。现在的归属：

| INC | 挂的套件用例 |
|---|---|
| INC-A01 | A01-bran-copy、A01-bran-add-member、A01-bran-del-member、A01-bran-reparent、A01-bran-hpos、E04-spre-swap |
| INC-A02 | A02-box-add、A02-subtree-add、A02-equi-copy、F03-cross-db-copy、A01-delete-box、A01-delete-keep、A01-recreate-same、A07-owner、B01-resize、B01-cyli-size、B02-nbox-resize、B02-nbox-add、B02-nbox-move、A02-box-pos、A02-pose-reparent |
| INC-A09 | A09-pos-same、A09-xlen-same、A09-desc-same、A09-spre-same、A09-ori-same |
| INC-D06 | D06-zone-move、D06-zone-ori |
| INC-A07 | A07-rename、A07-desc |
| INC-A08 | A01-compound-rename、A01-two-roots |
| INC-B01 | A01-child-parent |
| INC-C03 | C01-move |
| INC-C05 | C05-pipe-whole |
| INC-C06 | C06-noz-move、C06-nozz-move |
| INC-C08 | C01-rot-u、C01-rot-e、C01-rot-n、C01-compound |
| INC-G02 | G01-room-member |
| INC-G03 | G02-pane-move、G03-pave-move、G04-frmw-rename |
| INC-H03 | H03-companion-move |

前缀容易误读的几例：B01-resize、B02-nbox-* 是几何改（INC-B01 是跨会话父子删除，INC-B02 是历史）；F03-cross-db-copy 是跨库复制（INC-F03 是 /tasks 历史）；A07-owner 是改挂（INC-A07 是元数据门）；E04-spre-swap 是设计侧换规格，归 INC-A01，INC-E04 仍是 Paragon 目录改的语料案例。
