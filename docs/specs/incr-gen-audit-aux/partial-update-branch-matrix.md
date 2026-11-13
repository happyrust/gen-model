# PartialUpdateDesiMgr 分支与 AMS 测试矩阵

## 证据范围

本矩阵来自 `D:\ida_scratch\plant3\Core3D.dll.i64` 的 IDA bridge 实例
`idalib-73568`，反编译入口集中在 `PartialUpdateDesiMgr` 和
`DB_UserChanges`。它描述的是待测业务分支，不把静态反编译结果当成 TTY 通过证据。

## 已确认的运行语义

| 分支/入口 | 反编译结论 | 必须验证的外部行为 |
|---|---|---|
| `elementCreated` | 新元素和 owner 都记录为 created；后续 `NewModelToUpdate` 使用状态 1 | 新建对象只生成一次 new root，随后改属性不重复入队 |
| `elementDeleted` | 删除遍历 logical tree；非 created 元素走 deleted；created 元素保留 created 语义 | 新建后删除不残留 pending/model；生产对象删除需阻止 |
| `elementIncluded` | created owner/child 有特殊合并；既有 child 改挂同时记录旧/新 owner | reparent 两侧 root 都更新，created owner 不产生错误 ancestor |
| `elementReordered` | 既有成员记录 owner 与成员；created 成员只更新最后引用 | 真实成员顺序变化必须区分旧 owner 与 new window |
| `attributeModified` | 无 qualifier 与 qualifier 走不同路径，最终合并到元素/owner 变更集 | NAME/DESC/PURP、SPRE/CATREF 等分别验证 |
| `ElementsMoved` | 移动事件进入 changed 路径 | 多次移动同一元素应被 `IsPending` 去重 |
| `ElementsMemberChanged` | 成员变化进入 granularity expansion | 成员增删/改挂需验证 primitive、negative member 分支 |
| `ModelToUpdate` | 仅 DESI DB；过滤 `XGEOM` 后代；pending 时不再扩展 | 非 DESI、XGEOM descendant、重复事件分别不误建任务 |
| `IsPrimitive` / `IsNegative` | 通过 noun field 查询判断 primitive；`IsNegative` 查询 field id `0x92663` | 正实体与负实体必须进入不同成员集合，不能只按 NAME/TYPE 推断 |
| `GranularityExpansion` | primitive/member/negative member 选择不同 root；状态 3/4 走删除，状态 5 走 absent cleanup | primitive 删除、negative member 删除、owner 删除分别对拍 RVM |
| `SignificantOwner` | 沿 owner 上溯直到 managed structure 边界 | 修改 primitive 与直接修改容器产生相同/不同 root 的边界要明确 |
| `AbsentPrimitives` | owner 当前成员不在 IDList 时加入状态 5 | 断链/成员列表缩短必须清理旧 primitive |
| `AncestorDeletes` | 删除状态 3/4 沿 owner 链补状态 2，避免漏删父级模型 | 子先父后删除不能重复 erase，也不能漏掉父 root |
| `UpdateOn` | 只切换调度开关，不清空 pending | 关闭期间多事件合并，重新打开后一次性更新 |
| `Refresh` | 仅当元素为 `VIEW` 且等于当前 sheet 时重置游标 | refresh 不扩大模型；delete 后 refresh 不崩溃 |
| `Update` | 状态 <=1 DrawModel；状态 5 EraseModel；结束时 Finish | new/changed 绘制、absent erase、失败重试状态分别可观测 |
| `ErasePrimitive*` | candidate model 按 CPS/PGP 删除；unknown model 逐 CPS 尝试；WWSegment 受 session flag 影响 | 已生成候选模型、未知模型、双库/多 session 删除都要有 RVM 证据 |

## 新增案例清单

这些案例已经登记到 `ams-inplace-manifest.json`。`calibrate` 表示宏语义仍需在现有 AMS TTY 上校准，不能报告通过。

| Case | 覆盖分支 | 预期判定 |
|---|---|---|
| `P01-refresh-view` | `Refresh` VIEW 命中 | 只刷新当前 view，不新增 root |
| `P02-refresh-after-delete` | deleted 后 Refresh | 不崩溃、不重建无效 ref |
| `P03-repeat-change-dedupe` | `IsPending` | 同对象两次移动只有一个 regen root |
| `P04-new-then-change` | created + changed | 最终只有 state 1 语义 |
| `P05-new-then-delete` | created + deleted | pending 清零、RVM 无残留 |
| `P06-recreate-same-name` | deleted + created | 名称/几何恢复，refno 可变 |
| `P07-created-owner-include` | `elementIncluded` created 分支 | 新 owner/child 关系只产生一次 root |
| `P08-existing-reparent` | `elementIncluded` existing 分支 | 旧 owner 与新 owner 都可见变化 |
| `P09-existing-reorder` | `elementReordered` | 成员顺序/根集合变化可对拍 |
| `P10-attribute-plain` | 无 qualifier `attributeModified` | NAME/DESC/PURP 改动可恢复 |
| `P11-attribute-qualified` | qualifier `attributeModified` | CATREF/SPRE/连接 qualifier 独立验证 |
| `P12-xgeom-descendant` | `ModelToUpdate` XGEOM filter | 不产生普通 primitive task |
| `P13-negative-member-change` | `NegativeMembers` | 负实体变化走正确 root |
| `P14-negative-member-delete` | state 5 + `EraseModel` | 只清理 absent negative primitive |
| `P15-significant-owner-boundary` | `SignificantOwner` | primitive/container root 边界稳定 |
| `P16-ancestor-delete` | `AncestorDeletes` | 父级补删且不重复 erase |
| `P17-absent-primitive` | `AbsentPrimitives` | 成员缺失生成 state 5 |
| `P18-multi-event-combine` | move + member + attr + reorder | 合并为有限唯一 root 集合 |
| `P19-update-off-on` | `UpdateOn(false/true)` | 关闭期间延迟，打开后集中更新 |
| `P20-non-desi-gate` | DB type gate | 非 DESI 修改被明确跳过 |
| `P21-cross-db-same-window` | 两库 pending/restore | 8000 与 7998 互不污染 |
| `P22-failure-retry` | Update/Finish 失败重试 | 失败写 `recovery-required`，重试可收敛 |
| `P23-changed-since-between` | `elementsChangedSince/Between` | session 区间内事件完整且顺序稳定 |
| `P24-dependent-attribute-cascade` | `attributeModified` dependency recursion | 依赖元素一起更新但不重复入队 |
| `P25-qualifier-map-replay` | `AttributesQualsModified` | qualifier 集合重放与普通属性修改一致 |
| `P26-candidate-model-erase` | `ErasePrimitiveFromCandidateModel` | 已有候选模型只擦除命中的 PGP |
| `P27-unknown-model-erase` | `ErasePrimitiveFromUnknownModel` | 未知模型逐 CPS 尝试且不误删其他 CPS |
| `P28-cps-session-flag` | `GetNextCPS` session flag | 同一对象在不同 session flag 下删除集合一致 |
| `P29-load-sheet-context` | `LoadSheet` + `Refresh` | 切 sheet 后只刷新新 VIEW，不污染旧 view |
| `P30-erase-significant-first` | `EraseModel` significant path | 有 significant 模型时不落入 candidate/unknown |
| `P31-erase-candidate-fallback` | `EraseModel` state 2 fallback | pending candidate 存在时只清理对应 primitive |
| `P32-erase-unknown-fallback` | `EraseModel` unknown fallback | 无 candidate 时逐 CPS 清理且保持其他模型 |
| `P33-draw-finish-command` | `DrawModel` + `Finish` | PUPDES 命令目标、SUPPRESS 和 finish 清理均正确 |
| `P34-exists-cps-or-idlist` | `Exists` | primitive 在 CPS/IDList 任一路径命中才保留 |
| `P35-no-current-cps` | `GetDesiCPS` 空段 | 无当前 view/CPS 时不误报 primitive 存在 |
| `P36-descendant-present` | `IsDecendantPresent` | pending ancestor 已覆盖后代时不重复扩展 |
| `P37-invalid-idlist` | `GetIDList` 无效元素 | 无有效属性时空 IDList 不导致崩溃或误删 |
| `P38-mute-new-notify` | `Mute/UnMute/NewModelNotify` | muted 期间不入队，unmute 后新事件可入队 |
| `P39-set-view-vs-forced` | `SetView`/`SetViewForced` | SHEE/PICORF 校验与强制 VIEW 路径分离 |
| `P40-reset-view` | `ResetView` | 清空当前 view 后 update 不发送错误 PUPDES |
| `P41-add-idlist-writeback` | `AddIDList` | 有效 IDList 才写回，空列表不产生伪成员 |
| `P42-delete-state-gates` | `IsDeletesPresent`/`IsAncestorDeletesPresent` | 状态 2/3 分别触发正确删除阶段 |
| `P43-dump-before-after` | `Dump(true/false)` | BEFORE/AFTER 日志和 CPS dump 可区分 |
| `P44-significant-classification` | `IsSignificant`/`IsManagedStructure` | managed/significant 边界与 XGEOM 过滤一致 |

## 状态竞争矩阵

`IsPending` 的状态竞争必须单独验证：changed(state 0) 会被 new(state 1)、已有 changed(state 0)、后代 new(state 1) 和 ancestor-delete(state 2) 抑制；deleted(state 3) 会检查 state 3/4；new(state 1) 会检查 owner 链上的 state 1。

| Case | 状态序列 | 预期 |
|---|---|---|
| `P45-change-after-delete` | delete(3) -> change(0) | 不重新 Draw 已删除 primitive |
| `P46-new-after-delete` | delete(3) -> new(1) | 新对象可入队，不复用旧 ref |
| `P47-delete-after-change` | change(0) -> delete(3) | 删除优先，最终无残留模型 |
| `P48-change-while-new-pending` | new(1) -> change(0) | 保留 new 语义，不能重复扩展 |
| `P49-change-under-ancestor-delete` | ancestor delete(2) -> change(0) | 由父删除覆盖，不能重绘子件 |
| `P50-repeated-delete-state4` | delete(3) -> delete(4) | 状态 4 幂等擦除，不重复 CPS 操作 |
| `P51-session-range-noop` | `elementsChangedBetween` 无变化 | 返回 false，不产生任何事件 |
| `P52-session-range-delete-only` | `elementsDeletedBetween` only | 只产生 deleted/owner 事件，不伪造 inserted |
| `P53-session-range-insert-only` | `elementsInsertedBetween` only | 只产生 created 事件，恢复链完整 |
| `P54-attribute-member-special` | `attributeModified(ATT_MEMB)` | 成员属性走直接 owner/member 记录，不走普通 qualifier 分支 |
| `P55-created-delete-with-includes` | `elementDeleted` created + logical includes | 创建后删除时清理 include roots，不伪造生产删除 |
| `P56-existing-delete-multiple-includes` | `elementDeleted` existing + multiple include roots | 每个 include root 都记录删除影响和 owner |
| `P57-leaf-delete-no-includes` | `elementDeleted` leaf path | 无 include root 时只记录目标和 owner 一次 |
| `P58-include-same-session` | `elementIncluded` existing child, same session | 旧 owner 与新 owner 记录顺序稳定 |
| `P59-include-cross-session` | `elementIncluded` existing child, cross session | 跨 session 改挂同时记录目标和两侧 owner |
| `P60-include-created-child` | `elementIncluded` created child | created child 不重复记录旧 owner |
| `P61-reorder-created-member` | `elementReordered` created member | 只更新最后 created ref，不生成旧 owner 变更 |
| `P62-attribute-created-element` | `attributeModified` created branch | created 元素只更新 created ref，不创建普通 qualifier map |
| `P63-attribute-reentrant-guard` | `attributeModified` self guard | 内部当前元素重入事件被忽略 |
| `P64-attribute-dependency-fanout` | dependency recursion | 一个属性修改递归触发依赖元素，且不重复入队 |
| `P65-attribute-multi-qualifier` | `stringQualifiers` count != 1 | 多 qualifier 集合按组合键保存和重放 |

## 条件分支补充（IDA bridge 第二轮）

这一组不是重复方法覆盖，而是把反编译中会改变结果的条件臂单独拆开：

| Case | IDA 条件路径 | 必须证明 |
|---|---|---|
| `P66-pending-invalid-state` | `IsPending` 对非 0/1/3 状态直接返回 false | state 5 不被当作 pending changed |
| `P67-granularity-primitive-members` | primitive 且 `Members(..., NEGATIVE)` 非空 | significant owner 与成员集合一起上卷 |
| `P68-granularity-primitive-empty-members` | primitive 且成员列表为空 | 退回 primitive 自身，不伪造成员 |
| `P69-granularity-delete-direct` | primitive、all-CPS 关闭、state 3 | 直接记录 deleted，不走 absent cleanup |
| `P70-granularity-absent-cleanup` | primitive、all-CPS 关闭、非 state 3 | 调用 `AbsentPrimitives` 生成 state 5 |
| `P71-cps-no-picture` | `GetDesiCPS` 当前 picture 不存在 | 返回空 CPS，后续不误删 |
| `P72-cps-empty-segment` | picture 存在但 DESI segment 为空 | 返回空 CPS，保持队列稳定 |
| `P73-cps-current-only` | `ExistsCPS` 当前 segment 搜索模式 | 只检查当前 CPS |
| `P74-cps-all-segments` | `ExistsCPS`/`GetNextCPS` 全 CPS 迭代模式 | 跨 segment 命中且不误删其他 primitive |
| `P75-update-precondition-noop` | `Update` 的 disabled/invalid-root/empty/busy 门禁 | 不发送 PUPDES、不调用 Finish |
| `P76-finish-delete-attachment` | `Finish` 发现 state 3 后清理当前 ATT | 删除后 attachment 清理一次且可恢复 |
| `P77-new-idlist-owner-dedupe` | `NewModelToUpdate` 的 IDList owner 去重 | owner 已存在时不重复写回 |
| `P78-members-search-modes` | `Members` 的 significant/primitive/negative 三种模式 | 三类成员过滤结果互斥且完整 |

## DB_UserChanges 第三轮（`core.dll.i64` 交叉核对）

这一轮直接反编译 `core.dll.i64` 的 `DB_UserChanges` 与 `DB_DB` 会话区间入口，补上此前只按方法名登记、没有单独条件案例的路径。关键地址：

- `DB_DB::elementsChangedBetween` `0x58ffc50`
- `DB_DB::elementsDeletedBetween` `0x5900250`
- `DB_DB::elementsInsertedBetween` `0x5901010`
- `DB_UserChanges::elementCreated` `0x5987a90`
- `DB_UserChanges::elementDeleted` `0x5987b70`
- `DB_UserChanges::elementIncluded` `0x5987ea0`
- `DB_UserChanges::elementReordered` `0x5988040`
- `DB_UserChanges::attributeModified` `0x5987090`
- `DB_UserChanges::AttributesQualsModified` `0x5986b50`
- `DB_UserChanges::combine` `0x5987520`
- `DB_UserChanges::isElementCreated` `0x59886f0`

**一处修正**：`elementsDeletedBetween` / `elementsInsertedBetween` 的返回值是 `switchBackSession` 的成功与否，不是“是否存在删除/插入行”。因此 `elementsChangedBetween` 中的 inserted 查询不是“有删除才执行”，而是 session 切换成功后继续执行；空删除区间同样会走到 inserted 查询。

**另一处补充**：`elementsChangedSince` 只是 `elementsChangedBetween(since, current, 0, 0, changes)` 的薄封装，区间语义由四参重载决定。

方法覆盖复核：IDA 当前有 47 个 `PartialUpdateDesiMgr` 符号，其中 46 个业务方法已映射到案例；唯一未映射的是静态 `Instance` 单例访问器，不承载增量分支。

反编译要点：

- `elementsChangedBetween` 的顺序固定为：changed 元素属性差分 → primary-list 成员比较 → deleted 差分 → inserted 差分。
- changed 元素的属性差分为 `ATT_OWNER` 时走 `switchToOldSession` + `elementIncluded`；其他属性走 `attributeModified`。
- primary-list 元素在成员比较结果为 3 时调用 `elementReordered`；循环内无条件补 `attributeModified(ATT_MEMB)`。
- `elementCreated` 先做 `isElementCreated`（沿 owner 链）判定；已 created 的元素只更新最后 created ref，不再记录自身与 owner。
- `elementDeleted` 对 created 元素走 created ref + include root 清理；只有非 created 元素才记录生产删除的目标与 owner。
- `attributeModified` 依次经过：当前元素 guard → created ref → `ATT_MEMB` 特判 → qualifier 单/多键选择 → dependency 递归。
- `combine` 合并五棵元素集合，并对同元素的 qualifier map 逐项合并；元素已存在且两侧都有 qualifier 时不是整表替换。

| Case | IDA 条件路径 | 必须证明 |
|---|---|---|
| `P79-created-ancestor-dedupe` | `elementCreated` 经 `isElementCreated` 沿 owner 链命中已 created 祖先 | 同一窗口创建 owner 与 child 只产生一个 new root；child 不重复记录 owner |
| `P80-created-ancestor-delete` | `elementDeleted` 经祖先 created 判定进入 created 分支 | 删除新建容器的子件不产生生产删除行；include root 清理一次；删容器时 created root 消失 |
| `P81-created-ancestor-attribute` | `attributeModified` 经祖先 created 判定进入 created 分支 | 新建容器子件改名只更新 created ref，不建立普通 qualifier map 或依赖级联 |
| `P82-primary-list-add-member` | `elementsChangedBetween` primary-list 比较结果不等于 3 | 只产生 `ATT_MEMB` changed，不调用 `elementReordered` |
| `P83-primary-list-remove-member` | 同上，成员删除方向 | 只补 `ATT_MEMB` changed，不得升级为 reorder；被删成员自身是窗口前已存在元素，仍产生一条普通 `deleted` |
| `P84-qualifier-cleared` | `attributeModified` 中 `stringQualifiers` 从 1 个变为 0 个 | 组合键从“属性+qualifier”切到“属性+空”；旧单 qualifier 条目不得被当成仍然有效的变更重放 |
| `P85-combine-qualifier-merge` | `combine` 对同元素 qualifier map 的同一键合并 | 同一窗口两次写同一 SPRE，最终只保留一项且最后写入生效，不得留下两条冲突记录（TTY 已证实两次写都生效、最终值取后者；是否真的形成两个 batch 进入 `combine` 仍需事件级证据） |

## 第三轮案例的可执行绑定

`P79`–`P85` 的 action/restore 已落到 `scripts/e3d/increment_suite/round3/`，并在 `ams-inplace-manifest.json` 登记 `setup_macro` / `teardown_macro`：

| 案例组 | setup | teardown | 语料 |
|---|---|---|---|
| `P79`–`P81` | `round3/site_setup.mac` | `round3/site_teardown.mac` | 新建 `/AIOS-INC-<run-id>-P7x` SITE，带一个基线 BASE |
| `P82`–`P85` | `round3/bran_setup.mac` | `round3/bran_teardown.mac` | 新建 run-owned PIPE/BRAN，`COPY /C-OR-1R345-C` 并补稳定成员名 M1..M9 |

导出器在 baseline 之前执行 setup、在 restored RVM 导出之后执行 teardown；`{{TARGET_NAME}}` 在导出时替换成已解析 `<run-id>` 的目标名，`case.json` 里的预期路径也同步解析。`P79`–`P85` 的根是 SITE/BRAN，baseline 走 `common/capture_root.mac`，只查 REF/NAME/TYPE/OWNE，不查 POS，避免不支持属性让 E3D 提前退出。`expected_change` 除原有 added/removed/renamed/member_changes 外，新增三组键：`regen_roots` 登记应出现的 regen 根，`dc_events` 登记期望的 DB_UserChanges 事件串，`forbidden` 登记必须不出现的事件。写入 `case.json` 时路径里的 `<run-id>` 会解析成具体 run。

### TTY 证据现状（2026-10-29，P79–P85 全部产出三份 RVM）

| 案例 | run | before/after/restored 成员数 | TTY 已证实 | 仍缺的事件级证据 |
|---|---|---|---|---|
| `P79` | `20261029-p79-rvm4` | 4 / 6 / 4 | created 祖先下的 owner+child 建出两条 RVM 记录，restore 回到 4，按名对拍 0 差分 | created 祖先去重（child 不重复记 owner）不可由 RVM 观测 |
| `P80` | `20261029-p80-rvm` | 4 / 4 / 4 | owner(ref 24384/26561)/child(ref 24384/26562) 建后又删、owner 以空容器留存 | E3D RVM 导出跳过无几何容器（已用 `output/probe-empty-equi/` 证实），deleted 分支需 `dc_events` |
| `P81` | `20261029-p81-rvm2` | 4 / 6 / 4 | child 改名进 RVM，`-OWNER` 与 `-CHILD-RENAMED` 两条几何记录 | created 祖先对属性事件的过滤不可由 RVM 观测 |
| `P82` | `20261029-p82-rvm5` | 13 / 14 / 13 | 成员集 +1；`-FTUB-ADD` 实际插在**第 1 位**而非末尾 | `ATT_MEMB` vs `elementReordered` 需事件级证据 |
| `P83` | `20261029-p83-rvm` | 13 / 13 / 13 | 删 `M1` 后导出器物化无名 `TUBE 1`，`M2`–`M9` 留存；restore 重挂后按名对拍 0 差分 | `deleted` + `ATT_MEMB` vs reorder |
| `P84` | `20261029-p84-rvm` | 13 / 13 / 13 | `SPRE =0/0` 在 TTY 确实把 SPRE 清成 `Nulref`，qualifier 1→0 的模型语义成立 | `DB_UserChanges` 是否记成 qualifier-removed / comb key 切换 |
| `P85` | `20261029-p85-rvm` | 13 / 13 / 13 | 同窗口两次写同 qualifier 都生效，最终值 `TBL90:50X150` | `combine` 是否合并成一项、留下几条记录 |

七个案例一律保持 `calibrate`。共同阻塞原因已统一收窄为 `requires-db-userchanges-event-observation`（`P80` 另加 `rvm-omits-empty-containers`）：RVM 只能给最终态证据，created 祖先去重/过滤、`ATT_MEMB` 与 reorder 的分流、qualifier 清空后的 comb key、`combine` 的合并次数，都必须由事件级采集器（导出 `DB_UserChanges` 表或挂 `dc_events` 探针）判定。

### 窗口操作流（离线净窗口，已补齐）

七个案例的 TTY 窗口已用生产收集器重放，得到各自那次 `SAVEWORK` 的实际操作流（`output/dc-events/<case>-netwindow-*.json`、`<case>-parity-*.json`）。摘要：

| 案例 | 窗口 | Add | Deleted | 关键 Modified |
|---|---|---|---|---|
| `P79` | 740→743 | `EQUI 26555`、`BOX 26556` | — | `ZONE 26552`(MEMBERS) |
| `P80` | 751→754 | `EQUI 26561` | — | `ZONE 26558`(MEMBERS)（child 的建+删已被折叠掉） |
| `P81` | 767→770 | `EQUI 26572`、`BOX 26573` | — | `ZONE 26569`(MEMBERS)（改名被 created 折叠，不出属性行） |
| `P82` | 812→815 | `FTUB 26666` | — | `BRAN 26656`(CACHID,MEMBERS) |
| `P83` | 823→826 | — | `FTUB 26671` | `BRAN 26670`(CACHID,MEMBERS,SPAMAP) |
| `P84` | 834→837 | — | — | `BEND 26695`(SPAMAP,SPRE)、`BRAN 26693`(CACHID) |
| `P85` | 845→848 | — | — | `BEND 26708`(SPAMAP,SPRE)、`BRAN 26706`(CACHID) |

这些数字坐实了三件事：`P79`/`P81` 的 created 祖先确实同窗口存在（两条 Add + owner 的 MEMBERS 变化）；`P80` 的 child 建后又删在净窗口里完全消失（`fold_net_op` 的 `(Added,Delete)→Cancelled` 真实发生）；`P82`/`P83` 是成员集 ±1 而非纯换序。但净窗口是两端 `IndexDiff` 的终态，**结构上就给不出** `DB_UserChanges` 的逐次调用序列——`P84`/`P85` 两次写 `SPRE` 在净窗口里都只剩一条属性行，`combine` 合并了几项看不出来。

`P83` 的 RVM 差异已按几何判完：三份快照 `members`/`geometries` 恒为 13/9，`M2`–`M9` 的 AABB 逐位相同，只有被删 `M1` 的槽位从 Box(8536.56…9393.56) 变成 Cylinder(8544.01…9387.11)。**按名对拍会误报「消失 M1 / 新增 TUBE 1」，必须以几何与 AABB 为准。**

### 运行时采集器已落地（`tools/dc_events_probe.py`）

只读 Frida hook，挂 core.dll 的 `elementCreated` / `elementDeleted` / `elementIncluded` / `elementReordered` / `attributeModified`（两个重载）/ `combine`，把每次调用的 refno 写 jsonl；不调用目标函数、不替换实现、不改返回值、不写进程内存。`DB_Element` 的 refno 取 `+0xC`/`+0x10`，依据是 `db_userchanges.c` 里 `elementCreated` 末尾把 `a2+3`/`a2+4` 搬进 `this+15`/`this+16`。导出名必须用 mangled 全名交给 loader 解析——core.dll 的导出表在磁盘上是混淆的，直接读 `AddressOfNames` 得到的是代码字节。

本轮没跑成实跑：七个案例 TTY 窗口跑完后 AMS 库被 `aios-database.exe`（pid 70192）独占，`ams8000_0001` 与伴生 `ams7998_0001` 都拿不到写句柄。收口顺序与判据见 [`p79-p85-event-evidence.md`](./p79-p85-event-evidence.md)。

### 这一轮修掉的三个套件缺陷

1. **RVM 导出根错位**：`P82`–`P85` 的 `rvm_root` 原为 BRAN 元素本身，而 `bran_setup.mac` 建的是 `-BRAN-SITE` 容器下的 PIPE/BRAN 树；导出根落在 BRAN 上时 restore 重建整支 BRAN，按名对拍必然报 `FTUB-ADD removed`。已改为 `-BRAN-SITE`，并让 `Export-AmsIncrementCaseRvm.ps1` 把 `RvmElement=$rvmRoot` 显式传给 runner——否则 runner 的 `if (-not $RvmElement) { $RvmElement = $TargetName }` 会用 BRAN 覆盖，manifest 的 `rvm_root` 只影响生成出来的宏，不进入实际执行。
2. **`Q POS WRT /*` 让 des.exe 段错误**：`New-RvmExportMacro` 原来无条件查 POS，在 BRAN 根上触发 `0xC0000005`。已加 `-NoPosQuery`，由 manifest 的 `rvm_no_pos_query` 给 `P82`–`P85` 置位。
3. **弱会话隔离污染 before 快照**：runner 原来不等 detached `des.exe` 退出就进下一步，`P82` 的 `rvm-before` 曾因此包含 apply 已提交的 `-FTUB-ADD`。已加 `Wait-TtySettled`（等 `des.exe` 数量回落到 run 前基线）。

`20261029-p82-rvm4` 那次 `failed` 是上述第 1、3 项叠加的结果，不能作为 `P82` 的结论；有效证据是修完后的 `20261029-p82-rvm5`。另外 `P83` 的 restore 是删 BRAN 后重挂，refno 必然变化，只能按名对拍 RVM，不能按 refno 断言等于 before。

## RVM 验证要求

每个案例都必须有 `before/after/restored` 三个 TTY 导出宏；实际 `.rvm` 只能由 E3D TTY 生成。对拍主键为成员名称和层级，AABB 容差 1 mm，refno 只记录。`P12`、`P20` 等预期“跳过”的案例也必须保留 TTY 日志，证明没有错误任务，而不是用缺少 RVM 代替验证。

