# T-D-6 盘点：空库 `Initial` 直写 × 带几何指针的 legacy 行（`model_sesno = NONE`）

日期：2026-10-28
结论：**红**。夹具层面可以复现，证实了 ADR-088 D7 里「仍开着」的那条风险，应转成修复任务。生产上能不能走到这个状态未证实（见第五节）。
复核方式：`rg` + 定向 `Read`，未跑 `cargo`。行号按 2026-10-28 工作树（`refactor` 分支，含未提交改动）。

## 一、ADR-088 自己怎么说

- D7「管身与初始化这次不改」（`docs/adr/ADR-088-….md:115-120`）写的是：
  - 初始化那条写路（ADR-077 D4：`WriteMode::Initial`、不经整片删）不动；
  - 「初始化撞上老行是什么样没有验过，记在「仍开着」」。
- 「仍开着」一节（`:175`）写的是：「要是也被挡住，第一次增量碰到那根时由 D1 收掉」。
- 本盘点回答的就是「会不会也被挡住」：**会**。

## 二、代码路径

### 2.1 初始化直写只插不删

- `src/fast_model/model_initial_persist.rs:6-8`：模块文档写明初始化取 `WriteMode::Initial`，理由是「初始化前库里没有旧行」，所以不做整片删、不读 `slice_sources_on`。
- `model_initial_persist.rs:104-110`：`render_root` 直接 `render_records(records, WriteMode::Initial)`。
- `WriteMode::Initial` 就是 `INSERT IGNORE` / `INSERT RELATION IGNORE`：撞上同 id 的旧行时保留旧行（spec R-D-12；渲染前缀见 `src/fast_model/model_db_adapter.rs:3836-3838` 的断言）。
- 增量路把 legacy 行圈走靠的是 `render_slice_delete` 的 legacy 分支：`model_sesno = NONE AND aabb != NONE AND world_trans != NONE`（`model_db_adapter.rs:890-892`）。它只在有回执的落库里出现（`persist_root_impl` 的 `:2186-2190`、`:2248`；`persist_group` 的 `:2001`）。初始化写路里**没有**这一句。

### 2.2 「初始化前清库」只看 `pe`

首次初始化 / 基线走 `src/data_interface/manual_update.rs:3486-3502`：

```
applied_sesno = DbnumState::applied_sesno(dbnum)                       :3486
(count, root_count) = baseline_counts(dbnum)                           :3487
  └─ 只数 pe：SELECT count() FROM pe WHERE dbnum = X              dbnum_initialize.rs:703-710
if baseline_has_uncommitted_rows(count, applied_sesno)   // count > 0 && applied == 0   :3077-3079
    wipe_dbnum_for_reinit(dbnum)                                       :3488-3496
if baseline_needs_full_parse(count, applied_sesno)       // count == 0 || applied == 0  :3069-3071
    initialize(...)   → 模型 lane 用 Initial 直写                    :3499-3502
```

- 清库的条件只看 `pe` 行数，不看模型面（`inst_relate` / `tubi_relate`）。
- 所以「`pe` 为 0，但本库还留着 legacy 的 `inst_relate` 行」时，不清库，直接进 `Initial` 直写。
- `src/data_interface/dbnum_initialize.rs` 的编排只在 lane 失败后清库（`wipe_and_bail`，`:23`、`:248`、`:259`、`:385`），写之前不清。
- `rg` 在 `src/data_interface` 里没找到初始化路径上对 `inst_relate` / `tubi_relate` / legacy 行的任何检查。（`room_recompute.rs:1639-1640` 是房间重算的空库判断，不在这条路上。）

### 2.3 会先整库 wipe 的三条路（这些场景下不撞）

- 半截基线残留：`pe > 0 && applied == 0`（`manual_update.rs:3488-3496`）。
- 回退 / 人工重新解析的重建裁决（`manual_update.rs:5068-5097`，`reinit_wipe_decision` 判 `Wipe`）。
- 崩溃恢复：遇到 `Initialize` / `Reinitialize` 意图的 `writing` 标记时，`rollback_to_watermark` 整库 wipe（`src/data_interface/fast_delete.rs:471-485`、`:522-525`；`write_state.rs:74-76` 的 `wipes_whole_dbnum`）。

这三条走的都是 `RANGE_TABLES`（`fast_delete.rs:15-45`）：含 `inst_relate` / `tubi_relate` / `geo_relate` / `inst_info` / `aabb` / `trans`，按 Ref0 区间删，不看 `model_sesno`。所以 legacy 的关系行会被一起清掉。

同文件 `:31-37` 的注释已经说破了前提：「空库初始化改走 `WriteMode::Initial`（只插不删）之后，「`inst_relate` / `tubi_relate` 为空」这枚证据必须真的意味着没有任何几何行，否则 `INSERT IGNORE` 让旧盒 / 旧变换赢。」——但初始化入口用的证据是 `pe` 行数，不是这两张表。

## 三、撞上之后的样子

- legacy 的 `inst_relate:<a>_<b>` 与模型面同 id、不带 `model_sesno`（见 `model_db_adapter.rs:844-851` 的说明）。`INSERT RELATION IGNORE` 撞上它就被忽略，模型面的关系行落不进库。
- 模型面同键的 `inst_info` / `aabb` / `trans:<a>_<b>` 槽位可能落得进去（legacy 自己的盒 / 变换按摘要另键，`:851`）。但冷读顺着关系行上的 `aabb` / `world_trans` 指针走，答的仍是 legacy 几何。这就是 ADR-088 在增量路上修掉的那个症状（夹具 8023：FTUBE 在 E3D 里已在 U3400、库上永远 2900）。
- 自愈只发生在「第一次有回执的增量碰到那根」时（D1 的 legacy 分支）。永远不再被窗口碰到的根，会一直答旧几何。

## 四、夹具（证红）

- 放在 `src/fast_model/model_initial_persist.rs` 的测试模块（纯渲染 + `mem://` 执行），或 `model_db_adapter.rs` 现有的 `surreal_mem()` / `seed_pe` 夹具旁。
- 先种一行 legacy：
  - `CREATE inst_relate:⟨24383_600⟩ SET in = pe:⟨24383_600⟩, dbnum = 7997, aabb = aabb:⟨<摘要>⟩, world_trans = trans:⟨<摘要>⟩`；
  - 不带 `model_sesno`，并建好那两个摘要行。
- 再用 `render_root(root, &commit, None, cap)` 渲出同 id 记录的初始化事务，交给 `publication_transaction` 执行。
- 断言（今天会成立，即「红」）：
  - `inst_relate:⟨24383_600⟩.model_sesno` 仍是 `NONE`；
  - `aabb` 指针仍是摘要键。
- 修复之后，这条应改在「初始化入口」一层断言：先清库、再直写，模型面的行落得进去。

## 五、生产可达性（未证实）

要撞上，需要「本库 `pe` 为 0（或 `applied_sesno` 为 0、且 `pe` 为 0）、但本库 Ref0 区间里还有带指针的 legacy `inst_relate`」。

- 常规清库路径都把 `pe` 和模型面一起清（`RANGE_TABLES`），造不出这个状态。
- 可能的来源（都未核实）：
  - 老生成链填的库里，`pe` 行不带或带了别的 `dbnum` 值，导致 `WHERE dbnum = X` 数出 0；
  - 人工 / 工具只删了 `pe`；
  - 其它只清数据面的路径（本次没找到）。

## 六、修复方案要点

1. **扩大初始化前的清库证据（推荐，改动最小）**：
   - `baseline_counts`（`dbnum_initialize.rs:690`）加一个模型面计数，例如 `SELECT VALUE id FROM inst_relate WHERE dbnum = X LIMIT 1` 和 `tubi_relate` 的同句，写法同 `room_recompute.rs:1639-1640`；
   - `manual_update.rs:3488` 的条件改成「将要走 `Initial` 直写，且本库还有任何模型面行」→ 先 `wipe_dbnum_for_reinit`（或只清模型面的 `wipe_dbnum_model_face_on`）。
   - 这样 `model_initial_persist.rs:6-8` 的前提「初始化前库里没有旧行」由入口保证，而不是靠假设。
2. **入口报错兜底**：初始化开工前如果发现模型面非空就拒绝开工并点名。比方案 1 保守，但需要人介入。
3. **不推荐**：给初始化每根加 legacy 分支的整片删。这会把 ADR-077 D4「只插不删」的性能前提整个推翻。

若决定不修，就应在 ADR-077 / ADR-088 里写明一句：「初始化（`WriteMode::Initial`）前必须保证本库模型面为空（`inst_relate` / `tubi_relate` 无本库行）；入口只看 `pe` 行数，有 legacy 模型行时须先 wipe 模型面」。

## 七、假设与未确认

- 未跑 `cargo`，夹具也没写。「红」是按 `INSERT IGNORE` 语义与写路读出来的，和 ADR-088 增量路已实测的机制相同。
- 初始化另有一条 SYS meta / CATA 的路（`ModelHalf::Skip`，`dbnum_initialize.rs:48-55`），不写模型面，与本问题无关。
- `batch_worker` 里 `Reinitialize` 意图的开工路径本次只核到 `manual_update.rs:5068-5097` 的清库裁决。若还有不经过它就直达 `initialize()` 的入口，也在同一风险里。
- legacy 行是否总带 `dbnum` 字段未核；修复方案 1 若按 `dbnum` 查模型面，需要先确认这一点，否则改按 Ref0 区间查。
