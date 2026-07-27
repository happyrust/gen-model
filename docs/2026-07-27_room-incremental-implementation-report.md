# 房间归属增量更新实施报告（2026-07-27）

范围：把 ADR-010 从「判定层与排序层已落地、增量本体一行未写」推进到**整条链路贯通并通过
验收对拍**。横跨 `gen-model` 与 `rs-core-pin` 两个仓。

结论一句话：**包围盒变了 → 入队 → 第三阶段消费 → 先清后写**，四段全部接上；ADR-010 的九条
决策加上 §4 的删除例外全部落地，缺陷 **D4 / D6** 关闭；唯一的硬标准「增量收敛结果 == 全量
重建结果」已在真实 SurrealDB 实例上跑通。

但**在本项目的数据上仍然跑不出房间边**，阻塞点都不在代码里，见 §6.1 当日实测。

设计决策与落地进度见 `docs/adr/ADR-010-room-membership-incremental-update.md`；本文补的是
变更清单、验证证据与残留风险。

---

## 1. 链路

```
属性变更 → 水位事务落队列
  ↓
drain 阶段 1  非 regen：transform → 刷包围盒 → 变了的入房间队列
     阶段 2  regen：重生成几何 → 刷包围盒 → 变了的入房间队列
     阶段 3  房间：整间分支先跑并报回已写成员 → 元素分支跳过已被吸收的
删除    不入队列：DeleteCleanup 当场清边并从空间树摘除
```

手动路径（`manual_update`）分两处调用队列，第三阶段补在单元重生成之后——漏掉它，手动跑就
永远不算房间。

---

## 2. 变更清单

### 2.1 写入层：先清后写 + 幂等（ADR §8，缺陷 D6）

`src/fast_model/room_model.rs`

| 改动 | 说明 |
|---|---|
| `render_room_relate_write` / `save_room_relate` | `DELETE room_relate WHERE in = <panel>` → 整批 `RELATE`，同一个事务。成员集为空时只剩那条 DELETE——面板挪走、房间清空正是靠它收敛 |
| `render_room_panel_relate_write` | 补上确定的 `{room}_{panel}` record id，先删后写，每间房一个事务、按 100 间分批 |
| 两处 `SUL_DB.query` | 补 `.check()`。此前 record id 固定为 `panel_member`，第二次跑同 id 会报「已存在」而被静默吞掉，于是全量重建既删不掉陈旧边也不报错 |
| `RoomPanelMap` | 拆成 `rooms`（通过命名校验、参与归属计算）与 `all_panels`（排除集须覆盖所有面板）。此前共用一个列表，命名不合规的房间照样被写进 `room_relate` 而它的 `room_panel_relate` 被跳过，两张表就此对不上 |
| `build_room_relations` | 三个 `.unwrap()` → 按面板逐条聚合失败原因再统一上抛；单块面板算不出来不再拖垮其余 123 间 |
| `cal_room_refnos` | `query_insts` 的 `unwrap_or_default()` 与两处 `let Ok(..) else { continue }` → 带上下文上抛。合成夹具首跑被藏了半天的「整间房静悄悄算成 0 个成员」，源头就是这里 |

`src/data_interface/increment_pipeline.rs`：`wrap_in_transaction` 私有 → `pub(crate)`，两侧复用。

`src/lib.rs:241`：启动调用点 `.unwrap()` → 打印告警。房间归属是可事后重建的派生数据，不该
让一次面板失败顶掉 `async_watch` 之前的整个启动。

### 2.2 队列层（ADR §1/§2/§7）

`src/data_interface/model_update_plan.rs`：新增 `RoomRecalcElement` / `RoomRecalcPanel`
与 `is_room_recalc()`。

`src/data_interface/model_update_pending.rs`

- **行 id 去掉 dbnum**：房间任务的 record id 是 `{action}_{target}`。一块面板天然跨库，
  带上 dbnum 会让同一间房在一轮里排出多行、被重算多遍；失败后又只能等同一个 dbnum 的新会话
  复活，而真正触发它的其它库永远够不着——审计里 **B6** 的放大版。
- **复活语义随之改为无条件**：行既然不带 dbnum，跨库比 sesno 只会让一个库的 500 永久压住
  另一个库的 80。而房间任务的入队条件本身就是「AABB 真的变了」，每一次入队都是全新的重算
  理由。`dbnum` / `source_end_sesno` 降为字段，取 max 记录最后一次触发来源。
- **`drain` 三阶段**：新增 `drain_rooms` 排在 regen 之后。它没有复用通用的 `drain_where`，
  因为有两件事通用循环表达不了——房间映射要按轮加载一次（那是一次房间类型表全表扫描加逐行
  图遍历），以及整间分支必须先于元素分支跑完。
- 三个阶段的 action 白名单提成常量，由 `every_action_is_consumed_by_exactly_one_drain_phase`
  守着「每种 action 恰好被一个阶段消费」。

`src/data_interface/manual_update.rs`：单元重生成之后补第三阶段。

### 2.3 两条重算分支（ADR §2/§8）

`src/fast_model/room_model.rs`

| 分支 | 做法 |
|---|---|
| 整间 `recalc_panel_membership` | 复用 `cal_room_refnos` + 先清后写，返回本次写入的成员集合 |
| 元素 `recalc_element_membership` | 从全局树按 `noun == 'PANE'` 取候选、与在册面板取交集，调共享谓词 `element_in_panel`，再「删该构件的所有入边 → 写回」 |

两条分支共用判定、共用 `{panel}_{element}` 边 id、都是先清后写，因此在同一份数据上收敛到
同一个边集。这让 ADR §8 那条同轮冲突规则降级了：整间分支已写过的构件，其元素任务被吸收
跳过——但那只是省一次网格加载与点检测，**不再是正确性前提**。

配套：`build_room_panels_relate_common` 拆成「只读加载」与「写回」两半（增量只要前一半）；
第二轮逐点兜底的查询抽成 `query_geom_pts`，正反两个方向从同一处取点——取法不同就等于判定
口径不同。

### 2.4 触发源（ADR §4）

`src/fast_model/occ_generate.rs`：`update_inst_relate_aabbs_by_refnos` 改为返回
`Vec<AabbChange>`。新旧两个值它本来就同时握着，比一下几乎零成本，此前只算不比、外面拿不到
任何信号。没有旧值（几何刚生成）同样算变。

两个调用点各自把变更集转成房间任务入队，按 `noun` 分流（PANE → 整间，其余 → 元素）：

- `src/data_interface/increment_manager.rs`：TransformOnly 路径，即「设备从 A 房挪到 B 房」；
- `src/fast_model/occ_generate.rs`：regen 路径。

### 2.5 删除路径（ADR §4 的删除例外，缺陷 D4）

`src/data_interface/helper.rs`：`delete_inst_relate_subtree` 在级联删几何之后再清房间归属。
**两个方向都删**——作为成员是 `room_relate` 入边，作为面板还有出边与 `room_panel_relate`
入边。不按 noun 分情况：`pe.noun` 此刻已随软删一起不可靠，而对非面板元素那两条子句本就是
空操作。

此前生产路径上**从来没有人删过** `room_relate`：全仓只有夹具清理里有一条删除语句。

`rs-core-pin/src/accel_tree/acceleration_tree.rs`：新增 `remove_by_refnos`。`rstar` 的
`remove` 要求按整值相等匹配旧包围盒，而删除路径手里没有那个值——元素连同它的
`inst_relate.aabb` 一起没了。留在树里的话 `locate_intersecting_bounds` 会继续把它当候选，
重算就会把一个已经不存在的构件算进某间房。

---

## 3. 三处刻意的收窄

入队口放开一点就会出大事，这部分比功能本身更需要记下来。

**3.1 `gen_spatial_tree` 关着时一条都不排。** 这是一个真实的数据安全隐患：那个开关同时管着
全量重建与空间树对账，关着时跑增量不只是徒劳——元素分支是「先删该构件的所有入边再写回」，
而候选面板取自那棵没人维护的树，捞不到候选就只剩下那条 DELETE，**等于把上一次全量建出来
的边悄悄抹掉**。门控做在 `enqueue_room_recalc` 里面，两个调用点想忘也忘不掉。

**3.2 regen 路径只在定向重生成时入队**（`debug_root_refnos.is_some()`，也就是
`gen_all_geos_data` 区分两条分支用的同一个信号，定向那条由 `ModelRefreshPolicy` 独家设置）。
否则全量生成会把整库元素都算成「包围盒从无到有」，逐个入队。

**3.3 房间清理不与 `delete_inst_relate_cascade` 合成一个事务。** 那个函数同时服务于重生成时
的「先删旧几何再写新几何」，而那条路径上元素还活着，它的房间边不该被动。两段各自幂等，中间
崩了 `DeleteCleanup` 任务会重试。

---

## 4. 一处刷录在案的 ADR 偏离

ADR §4 的第二个例外要求「形状变了但包围盒恰好不变」的元素仍入队一次。**没有做。**

那等价于「把每个重生成过的元素都入队」——一个 BRAN 重生成会连带它全部构件，其中绝大多数根本
没动。真正要区分的是「实体变了而包围盒没变」，这需要比几何哈希，而刷新包围盒这一层手里没有。

残留风险很窄：仅当一个**跨面板边界**的构件内部几何改变、且包围盒逐位不变时，它的第二轮逐点
判定结果才可能变而无人重算。要补的话，正确的位置是几何写入层带出一个「实体确实变了」的信号，
而不是在这里放宽成全量入队。

---

## 5. 验证证据

### 5.1 不连库单测

`cargo test --lib` → **216 passed / 0 failed / 51 ignored**。本轮新增 **16 条**，钉住的是
最容易在后续改动里被悄悄破坏、且破坏了不会报错的性质：

| 性质 | 破坏了会怎样 |
|---|---|
| DELETE 排在所有 RELATE 之前且同处一个事务 | 中途失败若只落了 DELETE，这块面板的归属凭空消失 |
| 成员集为空时仍然发那条 DELETE | 面板挪走后旧成员永远留着 |
| 边 id 由两个端点推出 | 同一条边每重建一次就新增一行 |
| 渲染对 `HashMap` 遍历顺序不敏感 | 「跑两遍 == 跑一遍」无从验证，而 §9 对拍正押在这上面 |
| 两条分支的边 id 逐字一致 | 各写一行，`fn::room_relate_of` 取到哪条全看存储顺序 |
| 元素分支删入边、整间分支删出边 | 方向写反，一条分支会把另一条的结果整片抹掉 |
| 房间任务的行 id 不带 dbnum | 同一间房一轮里排多行、重算多遍 |
| 房间任务无条件复活 | 跨库比 sesno，一个库的 500 永久压住另一个库的 80 |
| 每种 action 恰好被一个 drain 阶段消费 | 那种任务入队后永远躺在表里，不报错也不执行 |
| PANE 走整间分支、其余走元素分支 | 面板一动整间成员全变，元素级表达不了 |
| 删除时两个方向都清 | 留下指向已删元素的悬空边，`fn::room_relate_of` 照样取得出来 |

### 5.2 连库用例

用仓里的 fork 版 `bin\surreal.exe` 起一次性内存实例（**不要指向共享工作库**）：

```text
bin\surreal.exe start --user root --pass root --bind 127.0.0.1:8071 memory
```

| 用例 | 覆盖 | 结果 |
|---|---|---|
| `live_room_fixture_parity` | 全量侧 6 条边，两轮判定都走到 | ok |
| `live_room_incremental_parity` | **ADR §9 硬标准**：搬构件 → 元素分支 → 再全量 → 逐边比较 | ok |
| `live_room_panel_move_parity` | 搬**面板** → 整间分支 → 再全量 → 逐边比较 | ok |
| `live_room_panel_task_absorbs_element_task_in_the_same_round` | §8 同轮冲突规则：面板任务与其成员的元素任务同轮入队，跑完整第三阶段 | ok |
| `live_room_delete_clears_membership` | 删构件（入边）、删面板（出边 + `room_panel_relate` + 摘树） | ok |

`live_room_fixture_parity` 这次顺带清了一笔旧账：ADR 里一直挂着「§6 两树合一改完之后夹具没能
复跑」（当时本机 SurrealDB 实例被本会话之外的一方关停）。

**§9 对拍怎么做的**：全量建基线（6 条边）→ 把完全在 A 房内的构件搬进 B 房 → 刷包围盒拿到变更集
并入队（顺带断言排出来的正是 `room_recalc_element_4000000001_20` 这一行）→ 只跑元素分支 → 在
同一份数据上再跑一遍全量 → 逐边比较。两侧相等**之外**还断言搬家确实发生了（B 房收着它、A 房
不再收着）——只比「增量 == 全量」的话，两边同时算成空集也会相等。

搬家只改几何侧的包围盒与点集（面板还要重写 `.mesh`，它的判定读的是三角网而不是包围盒），
不碰 `inst_relate.aabb`：后者由刷新函数从 `geo_relate` 重算，测试直接改它就绕过了触发源，
等于没测。

**整间分支怎么测的**：把 B 房的面板从 `900..1900` 挪到 `1400..2400`，原本骑在 A/B 重叠区上
的跨界构件就此掉出 B 房，而 B 房原有的两个成员仍在里面。只跑整间分支，再与全量重建逐边
比较；另外断言跨界构件确实掉出了 B 房、且它在 A 房的归属没被牵连。

**同轮冲突规则怎么测的**：把面板 A 的任务和它名下一个成员的任务塞进同一轮，跑完整的
`drain_rooms`。数据没有任何变化，所以断言是「边集一条不变」加「两行队列都被消费掉」——
后者专门守着被吸收的那行也必须删，否则它会永远卡在队列里。这同时也是一次幂等性检查：
在没有变更的数据上重算，结果必须与基线相同。

### 5.3 复跑方式

这批 live 用例**只能逐个运行**：`SUL_DB` 是进程级全局，而每个用例各建一个 tokio 运行时，
第一个用例结束时连接的后台任务就死了，后面的拿到一条已关闭的连接。连接函数里已把这条约束
写成明确的报错。

```text
AIOS_LIVE_WS=ws://localhost:8071 cargo test --lib \
    fast_model::room_fixture::tests::live_room_incremental_parity -- --ignored --exact --nocapture
```

---

## 6. 还没做的

### 6.1 真实数据：当前这个库里根本没有房间

2026-07-27 收尾时对配置指向的实例（`v_port = 8009` / ns `1516` / db `AvevaMarineSample`，
该实例上只有这一个 ns 和这一个 db）做了只读实测：

| 指标 | 实测 |
|---|---|
| `pe` 总数 | 18530 |
| `inst_relate` 总数 | 655 |
| `FRMW` 总数 | 274 |
| `FRMW` 命中生效关键字 `-RM` | **0** |
| `noun = 'PANE'` 的元素 | **0** |
| `room_relate` | 0 |

这与审计报告 §4.3 记录的「124 个 FRMW 命中 `-RM`、全部挂着 PANE、906 个
`inst_relate.aabb`」**对不上**：那份数据在这台实例上已经不存在了，现库里换成了另一批
数据（按 refno 前缀主要是 13244）。审计报告 §4.7 也记过本机实例曾被会话之外的一方关停。

所以「补生成结构库」并不是给已有结构库补跑一次模型生成那么简单——**结构元素压根不在这个
库里**。要在真实数据上验证房间链路，得先确定目标数据集（哪个项目、哪些 db 文件），再解析
入库、生成模型，最后才是打开 `gen_spatial_tree`。当前库里那 18530 条 pe 是别处的在制品，
在它上面做全量重解析/重生成会覆盖掉。

### 6.2 其余

| 项 | 状态 |
|---|---|
| `update_aabbs` 写反的去重条件 | 未修。gen-model 侧已在唯一调用点绕过；新加的 `remove_by_refnos` 正好可以直接复用来修它 |
| **D10** | `options.rs` 是 `room_key_word`，所有 toml 写的是 `room_keyword`，键名与类型都不匹配，配置恒为 `None`，一直用默认 `-RM`。本项目上恰好是对的，改配置会静默无效 |
| **D11** | `rs-core-pin/src/rs_surreal/function.rs` 无条件按目录顺序加载 `resource/surreal` 下所有文件，`_hh` 永远覆盖 `_hd`，与 Rust 侧编译的 `project_hd` 错位；且加载语句没有 `.check()` |
| 空间树落盘时机 | TransformOnly 一轮更新了库与内存树但不重写 `accel_tree.bin`；房间任务的落盘时机 ADR 也标着「待定」 |

---

## 7. 改动文件

**gen-model**

```text
src/fast_model/room_model.rs          先清后写、两条重算分支、读写分离、11 条单测
src/fast_model/occ_generate.rs        AabbChange + 变更集返回 + regen 侧入队
src/fast_model/room_fixture.rs        move_fixture_body、room_edges、2 条 live 用例
src/data_interface/model_update_plan.rs      两种 action + is_room_recalc
src/data_interface/model_update_pending.rs   record_id / 复活 / 三阶段 / 入队口、4 条单测
src/data_interface/helper.rs          删除路径清房间归属 + 摘树、1 条单测
src/data_interface/increment_manager.rs      TransformOnly 侧入队
src/data_interface/manual_update.rs   手动路径补第三阶段
src/data_interface/increment_pipeline.rs     wrap_in_transaction 提为 pub(crate)
src/lib.rs                            启动调用点告警而非 panic
```

**rs-core-pin**

```text
src/accel_tree/acceleration_tree.rs   remove_by_refnos（+28 行纯新增）
```

该文件此前无未提交改动，与那边其余 7 个改动中的文件不冲突。
