# G3 分组规则组成员（GPWL / GPSET / SCOSEL）：实施与验收记录

日期：2026-11-09 · 状态：已实施并验证 · 依据：[`2026-11-08-gpwl-gpset-group-nodes.md`](./2026-11-08-gpwl-gpset-group-nodes.md)（源计划）+ [`2026-11-08-gpwl-gpset-group-nodes-dev-plan.md`](./2026-11-08-gpwl-gpset-group-nodes-dev-plan.md)（跨仓开发方案 §3.1 G3）+ [`2026-11-08-scosel-opcode-reference.md`](./2026-11-08-scosel-opcode-reference.md)（opcode 取证）

本文是**实施与验收记录**，回答三个问题：最终怎么实现的、为什么这么定、实测数据是什么。设计阶段的取舍（D1–D6 拍板、里程碑划分、风险表）不回写，看上面两份。

代码基线：gen-model `refactor@675f5b1e6` → `c493ab821`（8 个 commit）、plant3d-web `main@b2d50173`（1 个 commit）、plant-ui `master@108a16532`（1 个 commit）。

## 0. 结论先行

1. **规则组（`SCOSEL`）现在能解析出成员**：`resolved_members_with_rules` 在存储成员之后按规则求值补上命中成员，来源是新增的 `ResolvedMember::Rule { element }`——它没有 GPITEM，`via()` 回 `None`。
2. **求值域是 MDB 声明范围，不是全盘全扫**。这是本 mission 与「按库查」的关键分歧：APS 装 27 个规则组的 `aps250204` 里 **0 个 ZONE**，ZONE 散在别的库；只在规则组所在库求值，`/AREA01_PIPE` 必得 0。
3. **`ScoselRule.attr` 是 noun hash，不参与筛选**。它和 `Attr.qualifiers` 里的 noun 限定在 27 条真实样本里逐条相同；拿它再取一次交，一旦 scope 对某个 noun 没有索引就会静默清零——正是本 mission 要消灭的失败模式。
4. **认不出的限定让整条规则归空**（`fully_understood` 闸门），所以「空 members + 空 unsupported」才是真正的没命中，上层据此出 `scosel_no_match`；有认不出的形状则出 `scosel_unsupported`，两者不叠。
5. **开关默认关，关着时响应逐字不变**（契约快照 `tree-children-resolved-gpset-switch-off.json` 与开关引入前的既有快照逐字相同）。
6. `scosel.rs`（decode / encode）**整段 mission 一行未改**——它的口径在 G3 之前已经被真实字节钉死，本次只消费它。

## 1. 范围

| 仓 | 分支 / HEAD | 改了什么 |
|---|---|---|
| gen-model | `refactor@c493ab821` | 后端：字符串属性倒排索引、SCOSEL 求值器、`ResolvedMember::Rule`、`MdbScoselScope`、响应三键分键、契约快照、真库对照测试与夹具 |
| plant3d-web | `main@b2d50173` | 前端：模型树「规则」角标（amber），4 文件 90+/1- |
| plant-ui | `master@108a16532` | 桌面端：树行「规则」标签（`ph::FUNNEL`），3 文件 |

**不含** B 线写侧真库测试（G4 / M5）：`Transaction` 建 GPWL / GPSET / GPITEM 与写 `SCOSEL` 的落点见 [`2026-11-08-gpwl-gpset-m5-write-side.md`](./2026-11-08-gpwl-gpset-m5-write-side.md)，本 mission 不碰真库写入。

## 2. 实现清单

从基线 `675f5b1e6` 起 8 个 commit，按依赖顺序：

| # | commit | 改的文件 | 改了什么 / 为什么 |
|---|---|---|---|
| 1 | `6c8d7c3cd` | `src/data_interface/direct_index.rs`（+469/-7） | `DbIndexes` 加 `by_attr_text: HashMap<u32, HashMap<String, Vec<u64>>>` 倒排索引与 `refnos_of_attr_text(attr_hash, text)`；`FORMAT_VERSION` 1→2。**为什么**：规则是「`DESC` 全串等于某串」，没有文本倒排就得每次全库扫属性；索引随库快照持久化，跨请求复用。版本号必须提，否则旧快照会被 bincode 反序列化成「`by_attr_text` 为空」的新结构，规则静默全不命中——少任何一道都会踩这个坑。 |
| 2 | `2bc915b65` | 新建 `src/data_interface/scosel_eval.rs`（784 行）、`mod.rs` +2 | SCOSEL 求值器：`ScoselScope` trait（`refnos_with_attr_text` / `refnos_of_noun`）+ `evaluate()`。纯函数，**不碰 store、不碰文件系统、不知道库和 MDB 的概念**——「求值域是 MDB 全量跨库」是 trait 实现方的事。**为什么**：把语义与数据面切开，求值器就能用 27 条真实字节做单测（往返 + 求值 + 栈自检），不必起真库。 |
| 3 | `538dbbd05` | `direct_store.rs`、`direct_tree.rs`、`group_membership.rs`（+458）、`mdb_read_store.rs`、`options.rs`、`DbOption.toml` | `ResolvedMember::Rule { element }` 第三变体 + `resolved_members_with_rules()`；`TreeStore` 加 `explicit_int_array` 通道（默认 `Ok(None)`）；`group_scosel` 开关（默认 false）。**为什么**：规则成员与 GPITEM 成员在树上是同一种行，但「它从哪来」不同，必须有个变体承载；`SCOSEL` 走单独通道读原字，是因为 `named_attmap` 交的是 schema 投影后的兼容视图，形状随库而变，而 `scosel::decode` 对字节敏感。 |
| 4 | `de18a5634` | `src/options.rs`（+75/-8） | 把 `group_scosel` 的覆盖守卫与配置行各钉成独立用例。**为什么**：进程级静态量的开关最容易「谁的守卫漏了归零」，测试并发时静默串台；单独钉住两个分支 + `DbOption.toml` 的生效行，才不会在后续改动里悄悄变成常开或常关。 |
| 5 | `764150f07` | `direct_tree.rs`（+378）、`group_membership.rs`（+126） | `MdbScoselScope`（跨库聚合：逐库 `DbIndexes` + `problems` 逐库点名）+ `DirectTreeService::scosel_scope()` 懒建（`Arc` 复用，与冻结会话同寿命）+ 三消费者接线。**为什么**：求值域要跨库，但「跨哪些库」由 `membership.element_databases()` 里 `path.is_some()` 的库决定；建不成的库不静默跳过，进 `problems` 并 `log::warn!`，否则规则少命中时查不到是谁缺的。 |
| 6 | `5150c409c` | `direct_tree.rs`（+24）、`handlers.rs`（+216） | `ResolvedChild.rule: bool`；`resolved_children_body` 把三键（`via` / `secondary` / `rule`）从同一个 `if let (Some(via), Some(object))` 守卫里拆开，按行类型独立决定。**为什么**：`via == None` 的行有两种——主链子节点与规则成员，旧写法把两者一起吞掉，于是规则成员三键全不输出、`rule: true` 永远发不出去。只看 `via` 分不开这两种行，所以响应层需要 `rule` 这个标记。 |
| 7 | `0f3072bae` | `handlers.rs`（+259）、`tests/fixtures/group-contract/tree-children-resolved-gpset-{switch-off,with-rules}.json` | 两份契约快照基准：开关关（与引入前逐字相同）与开关开（含规则成员）。**为什么**：开关的语义是「关着时逐字不变」，只有快照能证明「逐字」；开关开的那份把三键矩阵固化成可 diff 的文本。 |
| 8 | `c493ab821` | `group_membership.rs`（+183）、`handlers.rs`（+539）、`tests/fixtures/group-contract/e3d-rule-groups.json`（362 行）、`scripts/e3d/g3-rule-groups/`（5 文件） | 5 条真库 `#[ignore]` 测试（VAL-LIVE-001..005）+ E3D 真值夹具（27 条规则组的 `Q ATT` 原文、`Q ALL` 成员名、成员 refno）+ 新原因码 `scosel_no_match` / `group_empty` + 只读宏与固化脚本。**为什么**：Rust 侧自洽不证明对，必须与 E3D 自己那套求值口径逐条比；E3D 侧不能从 Rust 测试里驱动（`des.exe` 会话要串行、单次 6-13 秒、`l3_suite` 是独立二进制），所以走「先跑宏、再把日志固化成夹具」。 |

## 3. 关键设计决定

### 3.1 `ScoselRule.attr` 是 noun hash，不是属性 hash，不参与筛选

真实字节里 `100007,<attr_hash>` 这一格，27 条样本的取值是 `640493`（`ZONE`，24 条）与 `67635315`（`REFGRD`，3 条）——都是 noun，不是属性。它们与 `Attr.qualifiers` 里 `1602,4,2,<noun>,6,1701` 的 noun 逐条相同。

**踩过的坑**：把它当属性 hash 去取交，等于向 scope 多问一次「有没有属性叫 ZONE」。全库没有任何元素的属性叫 ZONE，于是交出来必是空集。如果 scope 对某个 noun 恰好没有索引，那次查询返回空集而不是报错——**结果被静默清零，看起来像「这条规则没命中」**。这正是本 mission 要消灭的失败模式，所以求值器完全不拿 `attr` 做筛选，求值域只由 `Attr.qualifiers` 里的 noun 限定表达（`ALL ZONE WITH (...)` 的 ZONE 就在那里）。

### 3.2 求值域是 MDB 声明范围，不是全库全扫

`MdbScoselScope` 只收 `membership.element_databases()` 里 `path.is_some()` 的库。

**踩过的坑**：全盘全扫会多出 MDB 未声明库里的同名副本——`/AREA01_PIPE` 多出 `2013287044_56`，而它的库在 `dbnum_of` 回 `None`（不属于本 MDB）。这些副本不属于求值域，一旦混进来，前端会看到「同一个组里有两份同名 ZONE」，其中一份点不开（它没有 dbnum）。另一个坑是只在规则组所在库求值：`aps250204` 里 0 个 ZONE，`/AREA01_PIPE` 必得 0。

实测口径：求值域内 ZONE **1088 个 / 36 库**；全盘全扫是 2622 / 68，口径不同，不是数据不一致。

代价与懒建：`MdbScoselScope` 建一次索引（APS 59 个有文件的元素库，实测 **1.3 秒**），所以钉在 `DirectTreeService` 上懒建、`Arc` 复用，与冻结会话同寿命——文件一动 `is_current()` 就否、provider 换整只服务，scope 随服务一起换，不需要失效逻辑。开关判据放在 `resolved_scope()`（接线点）而不是求值器里：关着时连 scope 都不建，一个字节都不多读；`resolved_members_with_rules` 内部仍有同一道判据（它不假定调用方看过开关），两层都不成立才逐字不变。

### 3.3 认不出的限定让整条规则归空（`fully_understood` 闸门）

求值器遇到 `Before(..)`、非 `DESC` 属性、非 noun 限定、hunk 值，一律记 `EvalIssue` 并让**该子树**回空集。整条规则只有 `fully_understood` 时才允许把「空集」解释成「没命中」。

**为什么**：不设这道闸门，「算不出来」与「命中 0 个」在结果里长得一样（都是空 members），上层只能二选一地误报。设了之后三种情形分开：空 members + 空 unsupported = `scosel_no_match`；unsupported 非空 = `scosel_unsupported`（已算出的合法成员仍然返回，部分成功）；`SCOSEL` 字节解不出来 = `scosel_eval_failed`。

### 3.4 大小写不折叠（两处都不做 `to_ascii_lowercase`）

真实语料的 `DESC` 全大写，没有大小写混合样本，E3D 的真实口径无法判定。首版取保守值「宁可漏命中，不可错命中」，原文原样交给 scope。契约 VAL-EVAL-009 钉住这个决定；将来若拿到混合大小写样本，改的是 scope 侧或求值器的一处，不影响契约形状。

### 3.5 `Typed.words` 的类型码在倒数第二字

`1602,4,2,<noun>,6,1701` 的 `6` 是 noun 限定标记，末字 `1701` 只是终止符标记。按「末字是类型码」读，27 条样本全部解不出限定，`qualifiers` 会被当成 `Value` 型，于是每条规则都进 `unsupported`。

### 3.6 开关默认关，关着时响应与改动前逐字相同

`DbOption.toml` 的 `group_scosel = false`，环境变量 `AIOS_GROUP_SCOSEL` 可覆盖（取值规则同 `AIOS_STARTUP_AUTORUN`，拼错退回配置值）。关着时 `resolved_scope()` 回 `None`，成员解析只看 GPITEM 链——与 M2 完全一致。两份契约快照（开关关 / 开关开）就是这条的判据。

### 3.7 `scosel.rs` 一行未改

decode / encode 的口径在 G3 之前已由 27 条真实字节钉死（`word_count` 含自身、`100009` 的 `N` 含自身、`100015` 的 `len` 含自身；`106` / `1602` 的计数字含 opcode 自身，`118` 只是字符数）。G3 只新增**消费方**（求值器），不碰解码器——这样「27 条能解 + `encode(decode(样本)) == 样本` 逐字相等」这条验收线不会因为新增语义而漂移。

## 4. 契约形状：三键矩阵

`GET /api/v1/tree/children?refno=<分组>&resolved=1` 的 `nodes[]` 上，`via` / `secondary` / `rule` 三键按行类型独立决定（`validation-contract.md` VAL-CONTRACT-001/002/003）：

| 行类型 | `via` | `secondary` | `rule` | 判定依据 |
|---|---|---|---|---|
| 主链子节点 | 键**不出现** | 键**不出现** | 键**不出现** | `!row.rule && row.via.is_none()` → `continue`，三键都不插 |
| GPITEM 显式收录成员 | `<GPITEM refno>` | `true` | `false` | `ResolvedMember::Reference { via, .. }` |
| 规则（SCOSEL）命中成员 | `null`（键**必须出现**） | `true` | `true` | `ResolvedMember::Rule { .. }`，`is_rule()` 为真 |

要点：规则成员的 `via` 是 `null` 而**不是省略**——客户端按「键存在与否」分不开「主链子节点」与「规则成员」；`rule` 这一格才是判别位。旧实现把三键挂在同一个守卫上，规则成员三键全丢，`rule: true` 永远发不出去。

`unresolved[]` 的原因码分两族，语义不复用：

| 原因码 | 含义 |
|---|---|
| `gpref_missing` / `db_not_in_mdb` / `target_missing` | GPREF 链断：没写引用 / 目标库不在本 MDB / 库在元素不在 |
| `scosel_eval_failed` | `SCOSEL` 字节解不出来，整条规则没参与求值 |
| `scosel_unsupported` | 规则解出来了，但求值器有形状认不出（已算出的合法成员仍然返回） |
| `scosel_no_match` | 规则完整解出、无认不出的形状，但一个成员都没命中 |
| `group_empty` | 分组既没有存储成员、也没有规则（或规则为空） |

`group_empty` 与 `scosel_no_match` 都是为了消灭「0 行 + 无原因」的静默空转：前者对应 `/ACCEPT-GROUP`（0 个 GPITEM、0 字 `SCOSEL`），后者对应 `/AREA01_HVAC`、`/AREA03_HVAC`、`/AREA03_SUPP` 三条在当前语料里命中 0 个的规则。

## 5. 实测数据

### 5.1 口径表

| 项 | 值 |
|---|---|
| APS 的 MDB 声明范围内 GPSET | **28 个**（27 条规则组在 `aps250204`，`/ACCEPT-GROUP` 在 `aps250220`） |
| 27 条规则组的 E3D `Q ALL` 命中合计 | **57 个成员** |
| 求值域内 ZONE | **1088 个 / 36 库**（全盘全扫 2622 / 68，口径不同） |
| `/AREA01_PIPE` 命中 | **1 个** ZONE `2013286676_490`；域外副本 `2013287044_56` 在 `dbnum_of` 回 `None` 的库里 |
| `/model/records` 路由 | **POST**（GET 回 405） |
| gen-model 测试基线 | `cargo test --lib` = **3029 passed / 0 failed / 139 ignored** |
| plant-ui 测试基线 | `cargo test -p rs-plant` = 247 passed / 0 failed；`-p plant-ui` = 265 passed / 0 failed |
| plant3d-web 测试基线 | `npm test` = 2 failed（既有无关失败）/ 4484 passed / 1 todo；`npm run type-check` exit 0 |

### 5.2 HTTP 端到端

开关开（`AIOS_GROUP_SCOSEL=true`）、rocksdb 档、APS 真库：

- 树：4 个 GPWL → 28 个 GPSET → 24 个规则组共 **57 个 `rule` 成员**；逐成员取子树，**57/57 回 200**。
- 28 个 GPSET = 27 条规则组 + `/ACCEPT-GROUP`。27 条里 24 条有命中（合计 57 个成员），3 条走 `scosel_no_match`（`/AREA01_HVAC`、`/AREA03_HVAC`、`/AREA03_SUPP`）；`/ACCEPT-GROUP` 走 `group_empty`。这两类都不产生 `rule` 行，但都带明确原因码，没有一个是「0 节点 + 0 unresolved」。
- 加载：`/AREA01_PIPE` 的 `generate` 回 **98 个生成根**；同节点 `/model/records`（POST）`total=727`。
- 开关关：规则组的 **28 份回执全部 `nodes: []`**（存储成员为 0，旧口径下就是这个结果），且**没有任何 `rule` 键**——`rule` 是 G3 新增的唯一响应键。`view` / `via` / `secondary` / `unresolved` 是 M2 的既有键，不随 G3 变化（快照 `tree-children-resolved-gpset-switch-off.json` 与开关引入前的既有基准逐字相同）。

### 5.3 与 E3D 的对照

`tests/fixtures/group-contract/e3d-rule-groups.json` 收 27 条规则组的 `Q ATT` 原文、`Q ALL <noun> WITH (...)` 成员名、成员 refno。Rust 侧逐条比对：

- VAL-LIVE-004：E3D 原文与 `decode` 的 `(noun, attr, op, texts)` 四元组逐项相同。
- VAL-LIVE-005：E3D 的成员集合与 `evaluate` 的命中集合**按集合相等**（顺序无关，两边排序后比）。

## 6. 验证方式

四个层面各自验的是不同的事：

| 层面 | 验了什么 | 怎么跑 |
|---|---|---|
| 单元测试 | 求值器语义（27 条真实样本全解 + `unsupported` 全空 + 栈自检）、`ResolvedMember` 变体与去重、三键分键矩阵、原因码互斥（`scosel_no_match` 与 `scosel_unsupported` 不叠、`group_empty` 不叠坏槽）、开关默认值与覆盖两分支、`DbOption.toml` 生效行 | `cargo test --lib` |
| 契约快照 | 「开关关 = 逐字不变」与「开关开 = 三键矩阵」两份基准，字段序与值都可 diff | `cargo test --lib` 内的快照断言 |
| 真库 `#[ignore]` | VAL-LIVE-001..003：`/AREA01_PIPE` 解析出非空 ZONE 行且三键齐、`/AREA01_GRID` 两条规则取并集且 noun 全 REFGRD、GPSET 无一是「0 节点且 0 unresolved」（契约文字写「29 个」是历史口径，实测按 GPWL 展开与按 noun 索引都得 **28**，测试按 28 断言并把两个数都打印出来，不让差异被静默吞掉） | `cargo test --lib live_rule_groups -- --ignored --nocapture --test-threads=1` |
| E3D `Q ATT` + `Q ALL` 只读对照 | 语义与真值：规则原文四元组、命中集合 | 先跑 `scripts/e3d/g3-rule-groups/` 两个只读宏，再 `build_fixture.py` + `merge_refs.py` 固化夹具，最后跑上面的 `live_rule_groups` |
| HTTP e2e | 端到端可用性：树、生成根、记录、开关关的逐字回退 | 起服务 + 开关，按 §5.2 走 |

`--test-threads=1` 是必须的：开关用进程级静态量，与要求「GPSET 全部 resolved=0」的既有真库测试并发会互相干扰。

## 7. 开关语义

| 项 | 值 |
|---|---|
| 配置键 | `DbOption.toml` 的 `group_scosel`，**默认 `false`** |
| 环境变量 | `AIOS_GROUP_SCOSEL`（压过配置；取值规则同 `AIOS_STARTUP_AUTORUN`，拼错退回配置值） |
| 关着时 | `resolved_scope()` 回 `None`，连 scope 索引都不建；成员解析只看 GPITEM 链；响应与改动前**逐字相同**——`rule` 键完全不出现（`view` / `via` / `secondary` / `unresolved` 是 M2 的既有键，照旧在） |
| 开着时 | 读 `SCOSEL` → `decode` → `evaluate` → 按存储原序去重追加到存储成员之后；规则行三键为 `via: null` / `secondary: true` / `rule: true` |
| 灰度 | 只有规则组受影响；显式成员组（GPITEM 链）与主链子节点两种情形下开关不改变结果 |

## 8. 已知限制

1. **`/AREA01_GRID` 的 `generate` 回 `NoRenderableGeometry`**。这条规则命中 3 个 REFGRD（网格定义），REFGRD 本身没有几何——**这是数据侧的应有表现，不是缺陷**。树与记录都正常，只是没有可渲染的几何体。
2. **E3D 只读会话会刷新真库文件的 mtime，但没有数据写入**。已核实 `latest_sesno=138` 与 `saved_at=2016-01-08` 均未变；宏里没有 `SAVEWORK` / `NEW` / `DELETE` / `MOVE` / `COPY`，也没有 PML 集合对象（`COLL` + `Size()` 实测会让 `des.exe` 崩 `0xC0000005`）。文件时间戳变化是打开会话的副作用，不是数据变更。
3. **大小写不折叠**：见 §3.4。当前语料无混合大小写样本，E3D 口径未定。
4. **`scosel_eval` 只实现等值分组**（`402` 全串相等 + `301` / `302` 集合运算）。`1306` / `1307` / `1308` / `1311` / `1317`–`1319` 这些字符串函数与通配匹配在 27 条真实样本里一个都没出现，首版一律 `unsupported` 报错，不静默跳过。
5. **`unsupported` 与 `scosel_no_match` 互斥**：有认不出的形状时只出 `scosel_unsupported`，不叠一条 `no_match`；同一次解析不出两个互相矛盾的原因码。

## 9. 未做

- **写侧（G4 / M5）**：建 GPWL / GPSET / GPITEM、写 `GPREF` / `SCOSEL` 的便捷封装与真库演练，见 [`2026-11-08-gpwl-gpset-m5-write-side.md`](./2026-11-08-gpwl-gpset-m5-write-side.md)。本 mission 全部是读侧。
- **增量（G5）**：GPITEM 增删 / `GPREF` 改动 / 规则改动的变更分类与缓存失效，未在本 mission 落地。
- **`/tree/ancestors` 的 `secondary_parents`**：契约里留了位置，G3 未接线，前端也没有消费方。
