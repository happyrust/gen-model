# 增量更新链路第二轮审核（未覆盖四项 + SurrealDB 实测取证）

日期：2026-07-26
审核对象：`gen-model` 增量模型更新链路，重点是上一轮 [`2026-07-26_increment-update-chain-audit-report.md`](./2026-07-26_increment-update-chain-audit-report.md) §六「本轮未覆盖」列出的四项
审核方式：本地源码追踪 + `bin/surreal.exe` 2.1.4 一次性内存实例（`127.0.0.1:8098`）实测语义，未连生产/开发实例（`:8009` 全程未触碰）
关联：`docs/specs/incr-gen-fixes/spec.md`、`docs/2026-07-26_increment-persist-optimization-report.md`、ADR-001/003/008

> 与上一轮一样，本报告只记录**结论与取证**，不含代码改动。

## 结论

- 上一轮的 **A1（High）、A2（Medium）、A3（Medium）三条在当前代码中原样存在**，均未修复。
- 四项未覆盖项**没有一项是上一轮猜测的那种缺陷**：折叠等价性成立，`inst_info` 同批脏读不存在，`init_watcher` 的 `retain` 兜底有效，`attempts` 归零语义成立。四项全部实测/取证定性完毕。
- 但顺着这四条查出 **4 个新问题**：1 个 Medium（`B1` 几何清理在中途失败后永久孤儿、且上报成功）、3 个 Low。
- 最值得注意的是 **B5**：`render_upsert` 的死信复活语义**依赖 SET 子句的书写顺序**——把 `attempts = …` 挪到 `source_end_sesno = …` 之后，复活功能会静默失效。已实测证明，且**没有任何测试守护这个顺序**。

## 一、上一轮三条结论的现状核对

| 项 | 位置（当前行号） | 状态 |
|---|---|---|
| A1 `delete_work` 失败中断整轮 drain | `model_update_pending.rs:401`、`:482`、`:492`、`:499` | **未修**，四处仍是裸 `?` |
| A2 「整窗口单事务」注释漂移 | `increment_pipeline.rs:83-87`、`:683-686` vs 实现 `:718-730` | **未修**，两处旧注释仍在 |
| A3 `datacenter_version` 脱离 finalize 事务 | `increment_pipeline.rs:565-576`（只记 warning）vs `:581-589`（finalize） | **未修** |

A2 的两处注释与 `:713-717` 那段准确注释同处一个函数，自相矛盾的观感比上一轮更明显。

## 二、未覆盖项 #1 · 窗口折叠的语义等价性 — 无缺陷

**位置**：`increment_pipeline.rs:98-313`（`fold_attr_namespace` / `fold_modified_run` / `fold_window`）、`../pdms-io/src/io.rs:582-682`（`to_modify_surql`）

逐条核对折叠依赖的四个前提，全部成立：

1. **run 检测正确**。`:274-288` 按 refno 的位置序列走，任何非 `Modified` 打断 run，`run.len() > 1` 才成 run。`Add → Modified×N → Deleted` 这类混合序列里，建记录与立墓碑的相对位置原封不动。
2. **last-writer-wins 而非 union**。`fold_attr_namespace:135-146` 用单张 `HashMap<K, LastWrite<V>>` 归并，**天然保证一个 key 只落进 added/modified/deleted 三桶之一**。这一点比未折叠更安全：`to_modify_surql:635` 把 `deleted` 渲染成 `NULL` 且排在 added/modified 之后，未折叠时若源解析同时给出同 key 的 added 与 deleted，值会被抹成 `NULL`；折叠消除了这种可能。
3. **`children_changed` 只用 new 侧**。`to_modify_surql:595-613` 的模式是 `DELETE pe:{id}<-pe_owner;` + 全量 `INSERT RELATION`，`old` 侧被 `let Some((_, new_children))` 直接丢弃。所以 `fold_modified_run:212-222` 取「最旧 old + 最新 new」里的 old 是死数据，不影响结果。
4. **「没有语句读另一条记录」基本成立**。渲染出的值要么是 JSON 字面量，要么是 `KEY: pe:{refno}` 这种 record link 字面量（`:619`/`:628`/`:653`/`:672`），没有子查询。唯一读图的是 `DELETE pe:{id}<-pe_owner`，而它删的是该元素**自己**的全部 child 边、随即整体重建，与中间态无关。

**一处验证层次低于观测面的地方（记录在案，非缺陷）**：三个命名空间在折叠侧是**独立**归并的，等价性测试 `FinalState`（`:1210-1264`）也把 `attrs` / `explicit` / `uda` 建成三张独立的 map 比对。但真正落库的观测面是**一个** MERGE 对象——`to_modify_surql` 把普通属性（`:621`/`:630`/`:636`）与显式属性（`:655`/`:675`/`:681`）写进同一个 `main_fields`，显式循环在后、同名 key 必然覆盖普通属性。也就是说：折叠把「后一个会话胜出」的判定，换成了「显式命名空间恒胜」。

两个命名空间确实可能重名——`rs-core-pin/src/types/whole_attmap.rs:65-79` 的 `refine` 会把 `explicit_attmap` 里的条目复制进 `attmap`，且**只有 `info.offset > 0` 时才从 explicit 侧移除**，`offset <= 0` 的属性同时活在两张表里。但同一处逻辑也保证了两侧值恒等（复制而来），因此两个命名空间对同一 key 的变更必然同会话、同值，**构造不出实际发散**。

结论：判定为「成立但依赖一条未被断言的不变量」。建议加一条断言（两个 delta 命名空间对同名 key 的新值必须相同），否则将来 `refine` 一改，等价性会在两层测试都通过的情况下悄悄失效。

## 三、未覆盖项 #2 · 共享 `inst_info` 引用计数 — 脏读假设证伪，但查出真缺陷

**位置**：`helper.rs:65-101`（`delete_inst_relate_cascade`）

### 脏读假设不成立

用一次性实例实跑 `helper.rs:79-86` 的原样 SQL（两个 refno 共享一个 `inst_info`，拼在同一条 query 里）：

| 时点 | `inst_info:s` | `inst_geo:g` |
|---|---|---|
| 删完第一个 refno | 存在 | 存在 |
| 删完第二个 refno | 已删 | 已删 |

同一条 query 内语句**顺序执行且后一条读得到前一条的写**，`array::len($old_inst<-inst_relate) = 0` 不会读到中间态。跨 chunk 同理（更强的隔离）。**Oracle 提出的脏读风险不存在。**

### B1 · 清理条件依赖自己刚删掉的边，中途失败后永久孤儿 — Medium

同一段代码有个更实在的问题。三条语句的顺序是：

```sql
let $old_inst = (select value out from inst_relate:X)[0];   -- 先记住 inst_info
delete from inst_relate:X;                                   -- 删边
if $old_inst != none and array::len($old_inst<-inst_relate) = 0 { ... }  -- 再决定要不要删 inst_info
```

**这三条不在事务里**（`:90` 只是 `join(";")`），而团队自己实测过「一条语句报错不阻断后续语句」。于是存在这个窗口：`delete from` 已执行、`if` 块未执行（`if` 自身报错、连接中断、服务端重启）。此时 `.check()` 会返回 Err → DeleteCleanup 任务标记 failed → 下一轮重试。

**重试救不回来**。实测：

```
删掉 inst_relate:c 之后，再跑完整的三条语句
→ {inst_info: true, inst_geo: true, geo_relate: true}
```

因为 `$old_inst` 这时读到的是 `NONE`，`if` 条件短路，整段清理被跳过——而函数**返回 `Ok`**，任务被当作完成删除。结果是 `inst_info / geo_relate / inst_geo` 三件套永久残留，且没有任何告警。这正是 F1 当初要消灭的孤儿几何，只是换了个入口回来。

**修复方向**：把每个 refno 的三条语句包进 `BEGIN/COMMIT`（改动最小）；或者把清理改成「先读出 `out` 集合 → 删边 → 按 **inst_info id** 逐个 GC」，让 GC 条件不依赖已删除的边，从而重试可自愈。后者顺带能清掉历史遗留的孤儿。

### B2 · 没有 `geo_relate` 的 `inst_info` 永不删除 — Low

`:82` 的删除集是 `select value [out, id, in] from $old_inst->geo_relate`——`in` 才是 `inst_info` 本身。若某个 `inst_info` 没有任何 `geo_relate` 边（几何生成半途失败留下的），该集合为空，`inst_info` 就删不掉。实测确认：`{inst_info: true}`。

而 `:53` 的文档注释明写「inst_info: 实例信息节点」在删除范围内。注释与实现在这个边界上不一致。

## 四、未覆盖项 #3 · `init_watcher` 重复 dbnum 的逃逸路径 — 兜底有效

**位置**：`increment_manager.rs:749-764`（判定）、`:814-819`（`retain` 兜底）

确认兜底成立：第一个同名 dbnum 文件确实已经进了 `params`（`:803`），但 `:816-818` 的 `retain` 按 `bi.pdms_header.db_num` 过滤，会把它一并摘掉。`params` 这条主路径没有逃逸。

顺带查出两个次要问题：

### B3 · `record_scan` 早于重复判定，重复文件会覆盖 dbnum 的身份字段 — Low

`:750` 的 `scan_and_check_file` 无条件写观察字段（`:515` 的 `record_scan`），而重复判定在 `:757`。所以两个重复文件都会写库，后扫到的那个把 `file_path` 覆盖成自己——尽管这个 dbnum 随后就被阻断了。由于 `init_watcher` 按文件大小降序遍历（`:696-700`），最终留在库里的身份取决于文件大小，且每轮可能翻转。

`applied_sesno` 不受影响（ADR-001 不变量由 `record_scan` 保证，T605 已覆盖），所以只是状态记录被污染，不影响水位与数据。

同一位置还有个更轻的：`#[cfg(feature = "mqtt")]` 的 `SyncPublisher::ensure_archive`（`:770`）对第一个文件已经执行，为一个随后被阻断的 dbnum 留下存档。因为发布只走 `incr.successes`，不会真的发出去，属于无用产物。

### B4 · init 递归扫描，watch 的跨事件兜底只查一层 — Low

`init_watcher` 用的是不限深度的 `WalkDir::new(watch_dir)`（`:696`），而 `async_watch` 的跨事件重复兜底 `duplicate_dbnums_across_watch_dirs` 用 `max_depth(1)`（`:386`）。

好在监控注册本身就是 `RecursiveMode::NonRecursive`（`:908`），事件只会来自目录直属文件，所以 `max_depth(1)` 与事件源是对齐的、**不构成漏判**。真正的后果是两条路径的**候选集合不一致**：子目录里的库文件启动时会被处理，之后却永远收不到变更事件。这更像是一个未言明的设计约定，建议要么在 `init_watcher` 也限深，要么在文档里写明「只有直属文件参与增量」。

## 五、未覆盖项 #4 · `cascade_expand` 派生任务的 `attempts` 语义 — 成立

**位置**：`model_update_pending.rs:136-153`（`render_upsert`）、`:368-387`（派生）、`:424-441`（drain 门槛）

直接回答上一轮的提问——**「新会话才复活死信」的语义在反复展开下仍然成立**。实测复刻 `render_upsert` 的 SET 子句：

| 场景 | `attempts` | 判定 |
|---|---|---|
| 首次入队（sesno 100） | 0 | — |
| 连续失败 5 次 | 5 | 进死信，被 `:431` 的 `(attempts?:0) < 5` 排除 |
| **同 sesno 再次 upsert**（cascade 重复展开继承父 sesno） | **5** | 保持死信，未被误复活 ✅ |
| 更新会话（sesno 120） | **0**，`last_error` 清空 | 正常复活 ✅ |

### B5 · 复活语义依赖 SET 子句的书写顺序，且无测试守护 — Low（但很脆）

SurrealDB 对 `UPSERT … SET a = …, b = …` 是**顺序求值、后面的子句读得到前面刚写的值**。实测同一条记录（`source_end_sesno=100, attempts=5`）在两种写法下的结果：

| SET 子句顺序 | 结果 |
|---|---|
| `attempts` 在前（= 现有实现 `:146-148`） | `attempts = 0` ✅ 复活 |
| `source_end_sesno` 在前 | `attempts = 5` ❌ 永不复活 |

也就是说 `:146-148` 这三行的**相对顺序是功能性的，不是格式**。一次自动格式化、一次「把字段按字母序排一排」的整理，就会让所有死信永久失去复活能力，而 `attempts` 门槛（`:431`）会让它们彻底消失在自动路径里——没有报错、没有告警。

现有测试只覆盖 `render_drain_select`（`:559-567`）、`joins_regen_batch`（`:570-597`）和 finalize 事务的组装（`:600-619`），**没有一条断言 `attempts` 归零语义**。建议补一条 live 测试（老 sesno 重放不复活 / 新 sesno 复活），或至少在 `render_upsert` 的渲染串上断言 `attempts` 出现在 `source_end_sesno` 之前。

### B6 · 派生根按目录库 dbnum 记账 — Low

`:373-380` 派生出的 `RegenRoot` 继承的是 `item.dbnum` / `item.db_type`，即**触发级联的那个目录/规格库**的 dbnum，而根本身通常属于某个设计库。`record_id`（`:62-69`）由 `dbnum + action + target_refno` 组成，于是同一个生成根可能同时存在两行：一行来自 CATA 级联，一行来自 DESI 直接变更。

后果有限：批量路径 `:471-476` 按 `target_refno` 去重，同轮不会重复生成；两行都会被 `delete_work` 清掉。残留的是复活口径——CATA 级联派生的死信只能靠**新的 CATA 会话**复活，设计库那侧再怎么改都碰不到它（record_id 不同）。若该目录元件此后不再变更，这行死信会永久留在表里，并一直出现在手动预览中。

顺带一个观感问题：`:149` 的 `status = 'pending'` 是无条件写的，所以一条 `attempts = 5` 的死信在表里显示为 `status = 'pending'`。手动路径 `load_pending_model_units` 不带 attempts 门槛，预览里它看起来像「排队中」，实际自动路径永远不会执行它。

## 建议处置

| 项 | 严重度 | 建议 |
|---|---|---|
| A1 `delete_work` 失败中断整轮 drain | High | 仍是最该先修的一条（上一轮已定性，改动为一个 match 分支） |
| B1 几何清理中途失败后永久孤儿且上报成功 | Medium | 与 A1 一起修；三条语句包事务是最小改法，GC 改按 inst_info id 是治本改法 |
| A2 单事务注释漂移 | Medium | 纯注释，零风险，随手改掉 |
| A3 datacenter 脱离 finalize 事务 | Medium | 排期，倾向并进 `render_finalize_transaction` |
| B5 SET 子句顺序决定死信能否复活 | Low（脆） | 补一条断言把顺序钉死，成本几行 |
| B2 无 `geo_relate` 的 `inst_info` 泄漏 | Low | 随 B1 的治本改法一并解决 |
| B3 重复文件覆盖 dbnum 身份字段 | Low | 把 `record_scan` 挪到重复判定之后，或对已阻断 dbnum 跳过写入 |
| B6 派生根按目录库 dbnum 记账 | Low | 若要处理，让派生根按其**自身所属**设计库 dbnum 记账 |
| B4 init 递归 / watch 只查一层 | Low | 二选一：init 也限深，或在文档里写明约定 |
| 折叠的命名空间重名假设 | 记录 | 加一条断言，防 `refine` 变更导致等价性静默失效 |

## 复现命令

```powershell
# 一次性内存实例（勿连 :8009）
bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8098 memory

# 语义探针：把下列 SQL 写成 .surql 后
Get-Content probe.surql -Raw |
  bin\surreal.exe sql --endpoint http://127.0.0.1:8098 --user root --pass root --ns audit --db audit --multi
```

三组探针分别是：

1. **死信复活**：复刻 `render_upsert:142-152` 的 SET 子句，依次跑「入队 sesno 100 → 置 attempts=5 → 同 sesno 重放 → sesno 120 重放」，观察 `attempts`。
2. **SET 顺序**：对同一条 `source_end_sesno=100, attempts=5` 的记录，分别用 `attempts` 在前 / `source_end_sesno` 在前两种写法 upsert 到 sesno 120，比对 `attempts`。
3. **引用计数与孤儿**：按 `helper.rs:79-86` 原样构造「两个 refno 共享一个 inst_info」，跑同一条 query 的两组语句；再单独构造「先删边、后跑清理」验证 B1。
