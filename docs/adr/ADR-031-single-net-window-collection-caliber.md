# ADR-031：增量窗口收集统一到净窗口单一口径，逐会话回放降级为 legacy 诊断

状态：已接受（2026-08-18）

引用的既有决策：

- **ADR-022**（净窗口收集）：本 ADR **取代**它的「灰度开关 + 分两步翻默认」的推行方式
  与**验收 4 / 验收 5**；它的决策 1（回放退出执行主路径、保留为诊断工具）、决策 2
  （输出形状兼容）、决策 3（纯文件判定分界）与「算法来源与正确性边界」一节**原样
  继续生效**，本 ADR 不重述、不放宽。
- **ADR-011**（一条队列、一个消费者、同一份谓词）：口径决定点从「一个开关」变成
  「没有开关」，同谓词纪律由结构保证而非约定保证。
- **ADR-001**（水位是承诺不是进度）：窗口**起点**仍由 `applied_sesno` 给出，本 ADR
  只管窗口内怎么读，不动水位规则。
- **ADR-021**（回退默认整库重建）：文件回退 / 幽灵水位不经本路径。
- **ADR-002 / ADR-004 / ADR-009**（core.dll 是属性语义与桶语义的唯一权威）：不变。

术语见 `CONTEXT.md`「会话索引差分」「净变化」。

## 背景

### core.dll 侧的事实（2026-08-18 live 复核）

用 ida-bridge 在 `D:\AVEVA\Everything3D3.1\core.dll.i64`（client `idalib-48392`，
core.dll 3.1，SHA `3c1f…417d`）重跑了一遍反编译，复核 2026-08-13
`docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md` 的结论，逐条吻合。
与本 ADR 直接相关的一点是**核内对一个会话区间不做逐会话循环**：

```c
// 0x5900230
bool DB_DB::elementsChangedSince(this, int from, int to, DB_UserChanges *out)
{ return DB_DB::elementsChangedBetween(this, from, to, 0, 0, out); }

// 0x58ffb20（删除 0x5900250 / 新建 0x5901010 与之同构，只换谓词）
if ( DB_DB::switchToOldSession(this, a4, a5, 0) ) {
  DB_IndexTableCompare cmp(this, a2, a3);     // 一对端点，不是循环
  while ( !DB_SystemTableCompare::finished(&cmp) ) {
    DB_Element e = DB_SystemTableCompare::dbele(&cmp);
    if ( DB_SystemTableCompare::modified(&cmp) ) append(e);
    DB_SystemTableCompare::next(&cmp);
  }
  DB_DB::switchBackSession(this, 0);
}
```

比较器构造（`0x5a18b20`）把整个区间收成一次 dabacon begin：
`sub_5AAF570(dbnum, 13387743, sesA, sesB)` = opcode **266**，处理器 `sub_5B026C0`
对**同一张主索引表**取两端根（`sub_5AF6840` ×2，根页须 `type == 5` 表页）。
`next()`（`0x5a18db0`）走 opcode **270**（`sub_5AFFCB0`）按 refno 键归并两棵 B+ 树，
哨兵键 `-2147483647`（`0x80000001`）作键空间边界；状态字 `*(this+48)`：

| 值 | 谓词 | 地址 | 含义 |
|---:|---|---|---|
| 1 | `modified` | `0x5a18da0` | 两端都有该键、记录位置变了 |
| 2 | `inserted` | `0x5a18d90` | 只在新根 |
| 3 | `deleted` | `0x5a18d70` | 只在旧根 |
| 4 | `finished` | `0x5a18d80` | 归并走完 |

载荷只有 `dataOnFirst` / `dataOnSecond`（`0x5a18cd0` / `0x5a18cf0`）给出的
`(pgno, offset)`。`DB_RawChanges::iterate`（`0x5983c30`）是同一比较器的第二个消费者，
形状相同。桶语义在候选集之上再做：`DB_Element::attributesChangedBetween`
（`0x5928100`）逐属性归桶，OWNER 走 `elementIncluded`，`primaryList` 走成员差分。

即核内是两层：**候选集 = 两端索引双根差分（与会话数无关）→ 桶 = 只对候选元素做
属性 / 成员 diff**。仓内 `session_index_diff` + `net_window` 就是这两层的纯文件
重实现（共享子树整枝剪枝是 gen-model 自有的文件层加速，不宣称逐指令复刻）。

### 现状：默认口径不是核内算法

`IncrementPipeline::collect_window` 的默认臂是 `pdms_io::collect_increment_eles`
的逐会话回放：对窗口内**每个会话**认领本会话新写的记录，再对每个 refno 调
`get_refno_operation_status` 付「latest + prev + owner 三场解析 + 一次全属性 diff」。
成本 O(会话数 × 每会话触达 × 3 次解析)，与净变更量无关。2026-08-18 现场
dbnum=7999 的窗口 `121..=185` 就是一例：日志逐条打出 65 行 `collect sesno:`。

### 灰度设计本身的代价

ADR-022 为谨慎推行留了 `net_window_collection` 开关 + `AIOS_NET_WINDOW`，代价是：

- **两个口径决定点的风险**：开关每次收集现读，规范上要求「同批次不换口径」，
  代码层没有钉死，于是要额外造一个批次级冻结快照（specs/003 T17）——这条复杂度
  **只为双臂而存在**。
- **两道本地关不掉的门**：T13（Added 独立夹具）要受控 E3D 录制、T18 的 SYST
  `250206` 硬门在客户现场。它们卡住的是「翻默认」，而翻默认是双臂设计的产物。
- **回执与运维的二义**：回执上「没有净口径行」既可能是走了回放，也可能是没收集。

机制层的核心疑问（双根差分是否为核内算法、删除是不是墓碑、flag 是否参与变更检测）
已由 2026-08-13 live 逆向闭合，2026-08-18 复核无出入。继续留双臂，买到的是一行
兜底值的回退便利，付出的是上面三项。

## 决策

### 1. 收集口径唯一：`collect_window` 只走净窗口

`IncrementPipeline::collect_window` 删除口径分支，五个调用点（手动预览、执行体
主收集、`apply_one` fresh 重收集、崩溃恢复固定区间重收集、worker 尾段
`roots_touched_since`）全部无条件走
`net_window::collect_net_window`。`CollectionMode` 与 `CollectedWindow.mode`
一并删除——单路径下这个字段恒为 `Net`，留着是假选择。

首条口径警告 `net_caliber_warning` **保留**：它携带 `unchanged_rewrites` /
`unparseable_finals` / 不可读子页 / 层级异常计数，是 spec-003 FR-8 的反静默出口，
与「有没有第二条臂」无关。

### 2. 逐会话回放降级为 legacy 诊断入口

`IncrementPipeline::collect_changes` 保留，但：

- `old-pdms-io` 默认关闭 `legacy_session_replay` feature；单 refno 操作判定、单会话
  实体解析、增量/最近会话/最新实体收集及保存、benchmark 包装都在该编译边界内；
- 主仓同名 feature 只转发到 `pdms_io/legacy_session_replay`，正常生产与正式 release
  feature 集均不启用；因此生产依赖图里不存在 `IncrementPipeline::collect_changes`
  及其 vendor 回放入口，误调会在编译期报缺失符号；
- `aios-py`、诊断 bin 与两个 replay oracle 测试目标必须显式启用 feature；
- 无 feature 的 `compile_fail` doctest 与生产 `cargo check` 证明入口缺席，有 feature
  的类型测试证明诊断面完整。旧 body-scoped `include_str!` 字符串验收删除——它只能
  证明几个函数体的字面文本，helper 间接调用可绕过，不是可达性证明；
- 它的两个终态补丁（`retain_finally_live_adds` / `restore_finally_live_deletes`）
  随它一起进 legacy——净路径构造性地没有它们的输入。

**为什么不删**：回放是净路径**唯一的跨结构独立参照臂**。性质 h（差分 ≡ 回放折叠）、
性质 i（Modified 负载逐桶相等）、`live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`
与 `live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay` 都靠它。
删掉它等于把交叉验证换成自证。这四条都**直接调**收集器、不经 `collect_window`，
因此单路径切换后原样继续跑。

`fold_window` 留在落库路径：净输入没有可折叠的连续 Modified 串，它成为构造性
no-op；诊断路径仍会用到。

### 3. 开关退役，且退役要出声

删除 `net_window_collection()` / `NET_WINDOW_ENV` / `effective_net_window_collection`
/ `NetWindowOverride` / `DbOptionExt.net_window_collection`。

**但不能静默**：`DbOptionExtFields` 没有 `deny_unknown_fields`，直接删字段会让配置里
残留的 `net_window_collection = false` 被安静忽略——运维以为自己关着回放，实际跑的是
净口径。这正是宪法点名的静默失效。因此保留一个**退役探测**：配置键或
`AIOS_NET_WINDOW` 仍被设置时，启动打一条显眼告警，说明该键已随本 ADR 退役、当前唯一
口径是净窗口。比 `watermark_realign` 的纯注释退役更严一档，理由是它改的是**算法**
而不是档位。

### 4. 回退语义变更（这是本决定换来的代价）

翻默认值时代的回退是「改一行兜底值」。单路径之后**没有这一行**，回退手段是
`git revert` 单路径提交。这一条必须写在显眼处：它是本 ADR 用便利换结构确定性的
明码标价，不是遗漏。

灰度期的现场审计能力不受影响——`net_changes_probe.py --verify` 与
`aios_db.parse.collect_changes` / `parse.net_changes` 都是纯文件离线入口，
不依赖生产开关。

### 5. 取代 ADR-022 的验收 4 与验收 5

ADR-022 的验收 4（性能门）与验收 5（翻默认的证据门）是**为「要不要把默认值从回放
翻到净窗口」这个决定服务的**。单路径之后没有这个决定，两道门失去它们守卫的对象，
因此按下面重定级。**重定级是显式的，写在这里，与代码切换分属两个提交**——宪法禁止
静默降门。

## 门的重定级（逐条，附实测值）

### T13 Added 独立夹具 → 残余风险，不再是门

- **原门**：`db_session_fixture` 录一个「创建 → Save Work」案例，`AIOS_SESSION_FIXTURE`
  指入后复用性质 h / i。
- **卡住的原因（2026-08-13 实查，本 ADR 不推翻）**：仓内**不存在**同时满足
  「Added > 0」且「raw 净集 == 回放折叠集」的真实窗口——带 Added 的窗口都伴随回放旧
  口径盲区，两集不等，性质 h / i 指过去必红。要点亮它必须用受控 E3D 录
  `scratch-create` 案例。
- **重定级理由**：它守的是「切臂时 Added 形状不回归」。单路径下 Added 形状的现有
  覆盖如实列举：① `synthesize_net_window` 七条纯单测覆盖三形状与三条降级，其中
  `a_net_added_entry_becomes_an_add_on_its_last_touch_session` 直接钉 Add；
  ② live 全窗 `1..=230` 的 **Add 6,496 条负载与回放逐字符相等**（2026-08-18 复验，
  1245ms vs 回放 10959ms）；③ 全窗 6,609 条 added 过生产 B+ 点查仲裁零分歧。
- **仍缺什么（不藏）**：缺的是**夹具级、带台账真值的** Added 案例。上面三项里没有
  一项是「独立录制的受控 Added 案例」。**绝不允许**为点亮它而放宽性质 h / i。
  T13 保持 open，降级为覆盖缺口而非切换门。

### T18 完整收集倍数门（≥10×）→ 记录项

- **原门**：≥20 会话完整收集（含终稿合成）relase 实测 ≥10×，不达则必须显式修订
  ADR-022 验收 4。
- **重定级理由**：倍数门的作用是「净收集要快到值得切过去」。单路径下没有备选臂，
  倍数不再决定走哪条路；真实收益也不在倍数，而在消除 amssys 全窗口 43% / 818 条
  旧口径盲区。
- **保留动作**：仍按原测量协议（同机同构建、1 warmup + ≥5 次、median / min / p95、
  warm 判定 cold 另报、高复触窗 + Add 地板窗两类、记复触率与环境项）跑一轮 release
  实测入 `docs/evidence/`。数字**如实记录，不作门**。
- **已有参照**：debug 完整收集 8.8×；release 方向性单点高复触窗 `104..=209` 17.7×
  （n=1、内层 `collect_net_window`）、Add 地板窗 `1..=209` 6.3×（形态决定）；纯差分 15–34×。
  `A/B probe 的 4.4×` 仍标注为混层比较、只作下界参考。
- **2026-08-18 正式协议实测**（生产入口 `collect_window`，1 warmup + 5 次，
  testbed 8000，latest=209，release，Ryzen 9 7950X）：高复触窗 `104..=209`
  warm median **10ms vs 53ms ≈5.3×**（复触率 3.21）；Add 地板窗 `1..=209`
  **128ms vs 908ms ≈7.1×**（复触率 1.05）。与 T18a 内层 17.7× 不可直接比——
  本轮含每次打开文件。明细见
  [`docs/evidence/2026-08-18-single-caliber-net-window.md`](../evidence/2026-08-18-single-caliber-net-window.md)。

### T18 SYST `250206` 单趟 collect < 30s → 上线后现场复测项

- **原门**：本地硬门。
- **重定级理由**：**该库在客户现场**，本地 amssys 只是代理形态，代理达标从来不等于
  硬门达标——这条在开发计划里已写明。本地无法关闭它，把它挂成本地门只会让它永远
  卡着或被伪绿。
- **改法**：列为上线后现场复测项，复测不达标时的处置是 `git revert` 单路径提交
  （决策 4 的回退手段），而不是重新引入开关。复测结果补记进 evidence 与本 ADR。

### T17 批次口径冻结快照 → CANCELLED

开关删除后没有口径可冻，ADR-022 决策 4 的「同批次不换口径」由结构保证。
不再实现 `collection_verdict.rs`，相关 task 标 CANCELLED 并写明理由。

## 明示的行为变化（口径升级，非回归）

ADR-022 §5 已逐条立案，本 ADR 只是让它们从「灰度可选」变成**无条件生效**，
必须进 changelog：

1. **改了又改回不再触发 regen**（净差集为空）。「宁多勿漏」的挂名处不变——
   未知属性仍保守触发，`classify_operation_impact` 一个字不动。
2. **加了又删不再留墓碑行**（两端都不在场，什么都不写）。
3. **删了又建判净修改**（落库语义等价：全量覆盖 + owner 边重插）。
4. **逐会话明细退出主口径**。`merged_sesnos` 已由 T12 改为文件会话页清单，
   空保存与自抵消会话照常进回执；需要逐会话归属时用 legacy 诊断入口。
5. **`pe.sesno` 戳 last-touch 会话**（与现状一致）。

## 残余风险（登记，不藏）

- **Added 夹具缺口**（T13）：见上，覆盖靠纯单测 + live 负载对拍 + 点查仲裁三项，
  缺受控录制的夹具级真值。
- **SYST 现场硬门未测**：`250206` 的 30s 目标只有本地代理形态旁证。
- **`flag` 链路外语义未闭合**（2026-08-13 report C3 / C4）：已证「净窗口所依赖的
  权威变更检测链路（页取 + begin + 双根归并）不读、不按 flag 过滤」，
  **不得**泛化成「flag 全无功能」。raw 叶内 flag 的位偏移 / 位宽 / 取值枚举，
  以及它在变更检测链路之外是否另有可见性门，仍未逆向。不影响本路径正确性。
- **qualifier 维**（ADR-022 §qualifier / specs/003 T19）：`ModifiedElement` 按属性名
  聚合会丢 core.dll 的 `(attribute, qualifier)` 维。这是回放与净路径**共享的既有
  输出形状限制**，单路径不新增回归；是否扩展输出形状仍待评估。
- **执行层 A/B 能力消失**：`test_net_and_replay_full_executions_land_equivalent_states`
  靠切 env 驱动两臂全链执行，单路径下无法保留，退役为历史证据（两轮全绿已在
  evidence 与 live 台账留档）。**保留的**是收集层交叉验证（性质 h / i + 两条 live
  对拍），这才是等价性的实质证据所在。

## 验收

1. **单路径结构成立**：`collect_window` 无口径分支；默认生产依赖图不启用
   `legacy_session_replay`，无 feature compile-fail 证明回放 API 缺席，生产 check
   证明执行链可编译；`CollectionMode` 已删。
2. **交叉验证不弱化**：性质 h / i、`db8000_session_pairs` 全部案例窗口、两条 live
   对拍全绿，一条都未放宽。
3. **退役不静默**：残留配置键或 `AIOS_NET_WINDOW` 触发显式告警，单测钉住。
4. **回执仍自报口径与容忍计数**：`net_caliber_warning` 的前缀与八项计数不变
   （`the_net_caliber_warning_carries_the_tolerated_shape_counts`）。
5. **性能数字入档**：按原协议的 release 实测记入 evidence，**标明为记录项非门**。
6. **T11b 删除等价仍有牙**：净臂单跑版本保留受控夹具声明 + `before_apply` 活行断言
   + `AIOS_T11B_FORCE_EMPTYRUN=1` 强制空跑变异必须准确变红。
