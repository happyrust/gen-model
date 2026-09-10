# 回退谓词的索引量测（只读）——ADR-072 D2「回退到水位」落地前的一次称重

> 任务：t-468（T5）。计划 `docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md` §4.3 / §4.5 / §9-6 / §9-7；ADR-072 D2 / D5。
> 重跑：`pwsh -File scripts/Measure-RollbackPredicateIndexes.ps1 -Endpoint http://127.0.0.1:8010/sql`（只读）
> 　　　`pwsh -File scripts/Test-PeVertexCascadeOnMem.ps1`（一次性 `memory` 实例，退出即销毁）

---

## 0. 一句话结论

**回退的三组谓词，只有一处真要补索引（`inst_info`），但有一处会把每一条谓词删都变成全表扫：`<int>sesno` 那个转型。**
转型写在列上，SurrealDB 2.1.4 的优化器直接放弃索引并给出 `Fallback: Unsupported value: <int> sesno`；
去掉转型，同一条谓词从 `Iterate Table` 变成 `Iterate Index`，语义一字不差（两种写法在 1112 上返回同样的行数）。

给 T1 的三句话：

1. **`fast_delete::render_rollback` 里的 `<int>sesno` / `<int>direct_model.sesno` / `<int>source_end_sesno` 全部去掉转型。**
   这是本轮唯一一处「不改就每次回退都全表扫」的东西，而且它是纯收益：行数相同、计划从 `Iterate Table` 变 `Iterate Index`。
2. **补一条索引：`DEFINE INDEX idx_inst_info_dbnum ON inst_info FIELDS dbnum;`**。`inst_info` 现在**一条索引都没有**，
   `WHERE dbnum = X` 落 `Iterate Table`。其余四张（`pe` / `inst_relate` / `tubi_relate` / `gen_root`）都已有 `dbnum` 前缀索引，够用。
3. **房间面那两张（`room_relate` / `room_panel_relate`）索引救不了**：谓词是 `in.dbnum`（边行上没有 `dbnum` 列），
   跨记录取属性，索引选不中，必然 `Iterate Table`。要么让 T3 在边行上补一列 `dbnum` + 一条索引，要么回退时房间面继续走
   Ref0 record-id 区间删（`fast_delete::range_of` 现成）。本轮不阻塞——两张表按整库删，一次全表扫的量级与库内边数同阶。

另外量了一条计划没问、但 §4.3-2 直接依赖的事实：**删 `pe` 顶点确实级联带走挂在它身上的边**，
包括**属主名下那条指向被删元素的成员边**——那一条重放补不回来，见 §4。

---

## 1. 量测环境

| | |
|---|---|
| 实例 | `http://127.0.0.1:8010/sql`（`D:\ams-local-0.1.21\workspace\bin\surreal.exe`） |
| ns / db | `1516` / `AvevaMarineSample` |
| 引擎 | SurrealDB `2.1.4+20250317.45013fc9 for windows on x86_64` |
| 时刻 | 2026-09-10 18:15 (+08:00) |

库存量（`pe` 全表 90 617 行，20 个 dbnum）：

| dbnum | pe 行数 | 状态 |
|---|---:|---|
| **7997** | — | **NOT RUN**：本实例里没有这个库的任何 `pe` 行（另一台 8009 实例整个 `pe` 为空）。任务点名的两个库只量到 1112。 |
| **1112** | 30 940 | 量了。`applied_sesno = 725`，`sesno` 跨度 4..725，本轮 `S = 362` |
| 7326 | 35 856 | 未量（不在点名名单里） |
| 7324 | 13 948 | 未量 |
| 其余 16 个 | ≤ 4 224 | 未量 |

其它相关表的全表行数：`inst_relate` 8 404、`gen_root` 8 343、`pe_owner` 101 036、`inst_info` 592、
`room_relate` / `room_panel_relate` / `ref_rev` 各 0。

---

## 2. 索引清单（`INFO FOR TABLE`，原文）

```
pe
  DEFINE INDEX pe_dbnum_noun ON pe FIELDS dbnum, noun
inst_relate
  DEFINE INDEX idx_inst_relate_anc   ON inst_relate FIELDS anc
  DEFINE INDEX idx_inst_relate_dbnum ON inst_relate FIELDS dbnum
  DEFINE INDEX idx_inst_relate_in    ON inst_relate FIELDS in
inst_info
  （无索引）
tubi_relate
  DEFINE INDEX idx_tubi_relate_anc   ON tubi_relate FIELDS anc
  DEFINE INDEX idx_tubi_relate_dbnum ON tubi_relate FIELDS dbnum
  DEFINE INDEX idx_tubi_relate_in    ON tubi_relate FIELDS in
gen_root
  DEFINE INDEX gen_root_dbnum        ON gen_root FIELDS dbnum
  DEFINE INDEX gen_root_dbnum_status ON gen_root FIELDS dbnum, status
  DEFINE INDEX gen_root_pe           ON gen_root FIELDS pe UNIQUE
pe_owner
  （无索引）
ref_rev
  （无索引）
```

`pe_dbnum_noun` 是复合索引，前缀正是 `dbnum`，所以 `WHERE dbnum = X` 用得上它——不必为回退另建。

---

## 3. 谓词 EXPLAIN（dbnum = 1112，S = 362）

> **读法**：`count() … GROUP ALL` 会走 `Iterate Index Count` 这条**只读索引计数**的快路，
> 它连行都不迭代，于是把第二个条件（`sesno > S`）整个吞掉——计数看着走了索引，其实没执行那一半。
> 证据：裸 `sesno > 362` 的 `count()` 回 30 940，与「只按 dbnum」一模一样；而真迭代行的子查询
> `SELECT count() FROM (SELECT id FROM …)` 回 30 939。
> **`DELETE t WHERE p` 走的是逐行那条路**，所以下表以「逐行计划」为准。

| 谓词 | 逐行计划（= DELETE 走的） | 判定 |
|---|---|---|
| `pe WHERE dbnum = 1112 AND <int>sesno > 362` | `Iterate Table {pe}` + `Fallback {reason: "Unsupported value: <int> sesno"}` | ❌ 全表扫 |
| `pe WHERE dbnum = 1112 AND sesno > 362` | `Iterate Index {index: pe_dbnum_noun, operator: "=", value: 1112}` | ✅ 走索引 |
| `pe WHERE dbnum = 1112` | `Iterate Index {index: pe_dbnum_noun, …}` | ✅ |
| `inst_relate WHERE dbnum = 1112 AND direct_model.sesno > 362` | `Iterate Index {index: idx_inst_relate_dbnum, …}` | ✅ |
| `inst_info WHERE dbnum = 1112 AND direct_model.sesno > 362` | `Iterate Table {inst_info}` | ❌ 无索引 |
| `tubi_relate WHERE dbnum = 1112 AND direct_model.sesno > 362` | `Iterate Index {index: idx_tubi_relate_dbnum, …}` | ✅ |
| `gen_root WHERE dbnum = 1112 AND source_end_sesno > 362` | `Iterate Index {index: gen_root_dbnum, …}` | ✅ |
| `room_relate WHERE in.dbnum = 1112` | `Iterate Table {room_relate}` | ❌ 跨记录取属性，索引选不中 |
| `room_panel_relate WHERE in.dbnum = 1112` | `Iterate Table {room_panel_relate}` | ❌ 同上 |

### 3.1 `<int>` 转型的代价（同一条谓词，两种写法）

真迭代行的计时（`SELECT count() FROM (SELECT id FROM pe WHERE …)`，避开索引计数快路）：

| 写法 | S = 362 | S = 724 |
|---|---|---|
| `<int>sesno > S` | 30 939 行 / **464.5 ms** | 1 行 / **358.6 ms** |
| `sesno > S` | 30 939 行 / **344.8 ms** | 1 行 / **228.0 ms** |

两点：

- **行数逐字相同**，两种写法在这个库上语义等价——转型是防御性的，不是必需的。
  `pe.sesno` 的两个写口（`SPdmsElement::gen_sur_json` 与 `ElementRows::minimal_tombstone_row`）都写 JSON 数字。
- 计时差只有 1.3–1.6×，因为子查询强制两边都物化。**真正的差在计划**：带转型是 `Iterate Table`（扫 `pe` 全表 90 617 行、
  20 个 dbnum），不带是 `Iterate Index`（只扫 1112 的 30 940 行）。这台上 1112 占 34%，所以差 2.9 倍；
  库越多、目标库占比越小，差得越多。7997 那种一个 MDB 几十个 DESI 的现场，这一项是数量级。
- `count() … GROUP ALL` 那一栏的 388.8 ms vs 14.3 ms（27×）不能直接当 DELETE 的代价——那是「全表扫」对
  「索引计数」，后者压根没执行第二个条件。放在这里只当「优化器有没有放弃索引」的信号。

---

## 4. 删 `pe` 顶点的级联范围（一次性 `memory` 实例）

夹具：属主 `pe:o31`（sesno 41）名下挂两个孩子 `pe:c30`（sesno **43**，本窗口改过）与 `pe:c33`（sesno 41，没改）；
`c30` 另有一条 `tubi_relate` 出边、一条 `ref_rev` 入边。执行回退那一句 `DELETE pe WHERE dbnum = 8000 AND sesno > 42;`
（只命中 `c30`，329.2 µs）：

| 表 | 删之前 | 删之后 |
|---|---|---|
| `pe` | `pe:c30`, `pe:c33`, `pe:o31` | `pe:c33`, `pe:o31` |
| `pe_owner` | `[pe:o31, 0]`（→ c30）, `[pe:o31, 1]`（→ c33） | **只剩 `[pe:o31, 1]`** |
| `tubi_relate` | `[pe:c30, 0]` | 空 |
| `ref_rev` | `ref_rev:r1`（`in: o31, out: c30`） | 空 |

结论三条：

1. **计划 §4.3-2 的前提成立**：删 `pe` 顶点确实级联带走 `pe_owner` / `ref_rev` / `tubi_relate`，两个方向都算
   （`ref_rev:r1` 的 `out` 才是被删的那一端，照样没了）。模型面的残留边不必单独写 DELETE。
2. **对照组没被误伤**：`[pe:o31, 1]`（指着没被删的 `c33`）原样留着，级联只沿被删顶点走。
3. **⚠ 级联同时带走了一条本该留着的边**：`pe_owner:[pe:o31, 0]` 是**属主 `o31` 名下**的成员边，
   而 `o31` 这一版没被改、不会进 S→T 的重放窗口。重放时 `c30` 自己的块只重建**它自己名下**的槽位
   （`element_batch::render_items` 的 `owner_edges` 来自 `ele_data.children`），边 id 里的槽位号
   （`pe_owner:[owner, slot]`）只存在于属主的成员表里，所以这条入边**补不回来**。

   这正好撞上 T1 已在代码里钉住的那处缺口
   （`fast_delete::tests::the_rollback_does_not_rebuild_the_membership_edge_its_cascade_removes`），
   本次量测把它从「读代码推出来的」升级成「跑出来的」。计划 §4.3-2 说「回退恰好要这个级联」只论证了模型面那一半；
   验收②要求 `pe_owner` 逐表与一次成功相等，两者在这一点上打架。两条出路：
   - 让重放能重建入边（给 `pe_owner` 边一个不依赖槽位号的稳定 id，或让窗口把被改元素的属主一并带进来）；
   - 或者回退不删 `pe` 顶点，只删它的卫星行（noun / `ATT_UDA`），让重放的 `INSERT … ON DUPLICATE KEY UPDATE`
     整行覆盖——代价是「本窗口新增、而重放窗口更窄」的元素会留成残留。

   **已拍板并落地（2026-09-10 18:5x，用户拍第二条）**：回退**不删 `pe` 顶点**，只删卫星行
   （noun / `ATT_UDA`）与模型 / 房间面。理由是这份量测本身给的——数据面靠**行幂等**收敛
   （ADR-071 S3）：`pe` 是每个 refno 一行最新态，重放的
   `INSERT … ON DUPLICATE KEY UPDATE`（列清单 = `element_batch::PE_REPLACE_ASSIGNMENTS`）
   整行覆盖，先删那一步对 `pe` 从来不是必需的；而元素批**本来就不删 `pe`**，正是因为同一条
   级联（`element_batch` 模块头「pe 行为什么不删」）。回退删它等于把那条纪律在恢复路上又
   破了一次。

   代价写在 `fast_delete::render_rollback` 的文档里：`sesno > S` 的 `pe` 行会在回退之后、
   重跑之前短暂高于水位；同一窗口重放逐行覆盖，只有「重放窗口比崩溃那次更窄」（会话预算
   截断）才留得下够不着的行，那一档由下一窗覆盖或人工走
   `DELETE /dbnums/{dbnum}/data/above/{watermark}`。文件被换回旧版本那一档是 `Reinitialize`，
   整库 wipe，不走这条路。

   回归：`direct_window_replay_parity::a_modified_element_keeps_the_membership_edge_its_unchanged_owner_owns`
   ——夹具补上「SITE 名下挂着 EQUI、SITE 不进窗口」那条入边（原夹具里 EQUI 没有入边，
   缺口正好测不出来），崩在第一块之后回退，断言那条成员边还在，再重跑一遍断言逐表与一次
   成功相等。渲染侧另有
   `fast_delete::tests::the_rollback_never_deletes_the_pe_vertex_so_the_membership_edge_survives`
   钉住「任何形态的 `pe` 行删除都不许有」。

---

## 5. 建议的 DEFINE INDEX（可直接落）

```sql
-- 唯一一条「不补就全表扫」的：inst_info 现在一条索引都没有。
DEFINE INDEX idx_inst_info_dbnum ON inst_info FIELDS dbnum;

-- 可选（本轮不必）：给 pe 一条 (dbnum, sesno) 复合索引，让范围条件也压进索引。
-- 现状 pe_dbnum_noun (dbnum, noun) 已经把 dbnum 那一半压进去了，剩下的 sesno > S 是索引内过滤；
-- 只有当某个库里「S 之后的行只占极小一部分」时这条才有额外收益（1112 上是 30939/30940，没有）。
DEFINE INDEX idx_pe_dbnum_sesno ON pe FIELDS dbnum, sesno;
```

`room_relate` / `room_panel_relate` **不给索引建议**：谓词是 `in.dbnum`，索引救不了（见 §0 第 3 条）。

---

## 6. 只读钉

`scripts/Measure-RollbackPredicateIndexes.ps1` 里 `INSERT|UPSERT|UPDATE|DELETE|CREATE|RELATE|REMOVE|DEFINE`
只出现在注释与输出文案里（第 7 / 9 / 10 / 13 / 15 / 146 / 158 行），发给库的每一条都是 `SELECT` / `EXPLAIN` / `INFO`：

```powershell
Select-String -Path scripts\Measure-RollbackPredicateIndexes.ps1 `
  -Pattern '\b(INSERT|UPSERT|UPDATE|DELETE|CREATE|RELATE|REMOVE|DEFINE)\b'
```

带写的那半边刻意分在另一个文件 `scripts/Test-PeVertexCascadeOnMem.ps1`，它起的是一个**全新的一次性 `memory` 实例**
（`surreal start … memory`，`127.0.0.1:8199`，退出前 `Stop-Process`），不连任何项目库。
