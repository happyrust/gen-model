# 会话上下文 — 2026-08-31 · e3d-model 增量更新审核 + grilling 下一步计划

- 本会话 BajieAsk sessionId：`BajieAsk-agent-1-b9fc69a5`
- 任务：① 审核当前 e3d-model 的增量更新实现；② 用 `grilling` 技能盘下一步开发计划
- 工作目录：`D:\work\plant-code\old\gen-model`；被审 crate：`D:\work\plant-code\old\vendor\e3d-model`
- 性质：**纯静态审核 + 已有产物实测复查**。未改一行源码、未跑 cargo（机器此刻空闲，
  但没必要为本轮结论去编译）

## 技能口径

`C:\Users\dpc\.agents\skills\grilling\SKILL.md`：一次只问一条、等回答再问下一条；
**事实自己去代码里查，决策交给用户**；每问都要给出我推荐的答案；未达成共识前不动手实施。
（`grill-me` / `grill-with-docs` 都是它的入口壳。）

## 前序会话（不照单全收，逐条回源码复核）

- `会话-…-e3d-model增量更新审核-dc4c6488.md`（16:0x）：首轮静态审核，出 P0-1 等 6 条
- `会话-…-e3d-model增量更新进展比对-b8ce34bd.md`（16:1x–17:3x）：与三份方案逐条比对，
  并用 ida-bridge 反编译 core.dll，发现 `DES_DrawListManager` 这**第二套**增量机制，
  出对齐差距表 A1–A8

## 我自己核过的事实（每条都指到文件行）

### 复核确认（前序结论成立）

1. `increment.rs::placement_drifted` → `transform.rs::local_placement` 只读 `POS`/`ORI`
   （`transform.rs:38`），而 `world_matrix`（`transform.rs:56`）折叠整条 owner 链。
   `local_placement` 的 doc 自己写「判据必须与矩阵折叠读同一条路，否则会出现矩阵变了
   但漂移检测说没变的静默缺件」——**实现没做到它自己写的这条**。P0-1 成立。
2. `increment.rs:412` `contributed = contributed || fanout > 0`，而
   `collect_positive_subtree` 返回的是**新加进集合**的个数 → 嵌套容器同时漂移时，
   后处理那个 fanout=0 被记进 `no_model`。账目说谎且依赖遍历顺序。成立。
3. `IncrementReport::accounts_for`（`increment.rs:157`）只查两条恒等式，
   **完全不看 `flag_only_drifts`**；`update_ams` 也不因它退非零码。成立。
4. `tests/increment_real.rs:36-38` 三个本机绝对路径硬编码，`fixtures_present()`
   缺件时 `eprintln!("跳过：缺 {path}")` + 各测试 `return` → **测试照样绿**。成立。

### 本轮新查到的四条（前序两会话都没有）

5. **索引差分的 `Modified` 判据 = 记录位置变，这条地基没人写下来。**
   `e3d-io/src/index/diff.rs:331` 判 `base.position() != target.position()`，
   `RecordPosition = { page, offset_words }`（`cursor.rs:52`），**不含 flag**。
   等于赌「dabacon 从不就地改写记录」。这是 core.dll `DB_IndexTableCompare` 自己的判据，
   所以不是我们独有的风险；但它是整条增量的地基，`increment.rs` 里一个字没提、没有测试钉住。
   与前序发现的「`members()` 读属主自己的记录」是同一类不成文载荷假设。

6. **owner 闭包今天成立，但自己人正在打破它（P0 级，且不是未来时）。**
   `rg` 实测：`elmodl.rs` / `solid.rs` / `pipeline.rs` 今天**一个指针属性都不解引用**
   （无 `get_element` / `CATREF` / `SPRE` 命中）——几何纯粹来自 owner 子树。
   这正是 `nearest_unit` 只爬 owner 链却仍然正确的原因。
   但 `src/catalogue_point.rs`（**未跟踪，今天 16:51 新建**）第 7 行自述链路
   `设计构件 --SPRE--> SPCO --CATR--> SCOM --PTRE--> PTSE --成员--> PTAX / PTCA`，
   `src/route.rs`（未跟踪，25 KB）第 18 行同链。SPCO/SCOM **通常不在设计库里**。
   → 它一提交，`nearest_unit` 就不再是依赖闭包；而 `collect_window(db_file, base, target)`
   的签名里**连放第二个库的位置都没有**。

7. **`CACHID` 在设计库记录里读不出来 —— core 那条判据不能照搬（实测，非字典层）。**
   前序会话从 ida 读出 core 判「几何变没变」只看 `ATT_CACHID`，并标为「未实测」。
   我拿仓库里已有的**活库提取产物**核了 4 份，全是 BRAN，两个不同 refno：
   | 产物 | refno | CACHID |
   |---|---|---|
   | `runtime-derived/baseline.json` | 24383/73930 | `value=null`，`encoded_location=0` |
   | `runtime-derived/modified.json` | 24383/73930 | `value=null`，`encoded_location=0` |
   | `.scratch/p3-attr-info/extract-85432-fixed.json` | 24383/85432 | `value.raw=0`，`loc=0` |
   | `…/extract-24383-85432-uda.json` | 24383/85432 | `value.raw=0`，`loc=0` |
   提取器**确实会填值**（98 个属性里 82 个有值），取不到值的 16 个
   （`CACHID` / `UDTYPE` / `HEADW` / `TAILW` / `RREF` / `UDATAB` / `UDAFTB` / `UDASTB` …）
   **`encoded_location` 一律为 0**，即该元素记录布局里根本没给它storage slot。
   → `CACHID` 是 core 在**内存元素**上维护的图形缓存标识，不是落盘字段。
   **A7/A8「照 CACHID 对齐」在文件层不可直接执行。**
   ⚠️ 留口：4 份样本全是 BRAN。EQUI/PANE/STRU/GENSEC 是否落盘未验，一条探针即可关掉。

8. **增量在防守一个主计划已经投票废掉的产物形态。**
   `increment.rs` 模块文档「与 Core3D 的一处有意分歧」明说：我们的产物是**世界系网格**
   （世界矩阵烘进顶点），所以容器一动就必须级联重建子树。
   而已批准主计划 `task_plan.md:86` 拍板：「**默认保留 (共享几何, 实例变换) 的分离形态**；
   只有该元素确实要做布尔时，才把变换烘进」，`:122` 又要求 JSON/glTF **保留实例表**。
   → 变换级联这套机制（`placement_drifted` + `collect_positive_subtree` + `cascades` 账）
   **按计划是要删掉的，不是要修好的**。P0-1 是在给一个计划已判死刑的机制打补丁。

## 结论

增量的**骨架是对的**（三段式分离、账本恒等式、先擦后画、删除侧钉 base 端点），
真库门也是真门；但它**站在三条没写下来的假设上，其中一条今天就要被自己人打破**，
而它最花力气的那套变换级联，主计划已经决定要删。

## grilling 第一问（已拍板）

原选项：A 先钉不变量（推荐）/ B 先修 P0-1 / C 先立项过门禁 / D 先做目录跨库闭包。

**用户答复**：四个都不选——「用 ida-bridge 分析 core.dll 和 core3d.dll 是怎么做增量更新的，
按它的方式设计架构」。下面第二篇即由此展开。

---

# 第二篇 · core 增量机制活桥取证 → 五层管线设计

产物：`docs/plans/2026-08-31-core-aligned-increment-architecture.md`（本仓 `gen-model/docs/`）。

## core 的五层管线（每层都有可复验地址）

| 层 | core 里的东西 | 职责 |
|---|---|---|
| L0 会话定位 | `switchToOldSession(db, sesno, extno)` | 把两端钉到具体会话 |
| L1 索引候选 | `DB_IndexTableCompare` | 出 (element, ins\|mod\|del) |
| L2 元素分类 | `DB_Compare::checkEle` → 12 个回调 | **权威判定**，逐属性按值比 |
| L3 语义记账 | `DB_UserChanges` | 8 个语义桶 |
| L4 消费者上卷 | `findTopLevelElement` | 折算成顶层可绘单元 |

关键取证：

- `DB_Element::attributesChangedBetween` / `hasAttributeChangedBetween`：指针属性用
  `DB_Ref::operator==` 比，即**改挂在 core 里就是普通的属性变更**。
- `NOUN_TUBI` 硬编码 `(POS, ORI, ITLE, SPRE)` 四个属性；`FNDTOP` 让 TUBI 上卷跨过 BRAN。
  → P0-2 隐式管身有现成答案可抄。
- `noun.toplevel`（字段 661628）是「模型单元」的权威定义，不是我们手写的正体名单。
- `ATT_CACHID` 只在 GETWORK / 本地改动路径上用，**不参与跨会话比较**；
  4 份活库实测无存储位（本会话 b9fc69a5 亲测），确认它不是我们要找的东西。

## e3d-model 的落差

`plan_update` 实际是把 **L1 直接接到 L4**，中间缺掉的 L2/L3 用两个几何启发式顶替：
`placement_drifted`（比两端 POS/ORI）+ `collect_positive_subtree`（变换级联）。
**这两个函数在 core 里没有任何对应物**——它们是为了补偿缺失的属性级差分而发明的。

## 用户拍板：先建 L2/L3 两层

---

# 第三篇 · L2/L3 落地（已完成，全绿）

## 新增两个模块

**`src/element_diff.rs`（L2，对标 `DB_Compare::checkEle`）**

- `diff_element(base, target, refno)`：先整条记录字节比一次判「真没变」，
  再分出 `type_changed` / `owner_changed` / 成员表增删换序（`MemberDelta`）/
  逐属性按值差分 / `opaque`（比出不同但归不进上面任何一类，如 UDA）。
- `values_equal`：`Some(DescriptorValue::Unset)` 与 `None` 归一；
  `reals_equal` 让 `NaN == NaN`、`-0.0 == 0.0`——否则全是假差分。
- `placement_input_changed()`：`POS`/`ORI` 任一变 ∨ `owner_changed` ∨ `type_changed`
  ∨ `opaque`。这是级联的新触发判据，**含 OWNER 即修掉 P0-1**。

**`src/ledger.rs`（L3，对标 `DB_UserChanges`）**

- 8 个语义桶（`ChangeKind`）+ `ChangeTally` 计数。
- **祖先抢占**：子树整棵新建/删除时只有子树顶算一条，其余标 `preempted_by`。
  效果立竿见影——ams1112 那一窗 24673 条删除实际只有 **6 个子树顶**。

## 改动的既有文件

- `transform.rs`：导出 `PLACEMENT_ATTRIBUTES: [&str; 2] = ["POS", "ORI"]`，
  级联判据与矩阵折叠共用一份属性名。
- `increment.rs`：`plan_update` 改接 L2/L3；`IncrementReport` 加 `changes: ChangeTally`
  与 `unchanged`；删掉 `placement_drifted`；改挂时**旧属主那侧也上卷**；
  `UpdatePlan` 带出 `ledger` 供测试查证。
- `lib.rs`：导出两个新模块。

## 设计文档的两处自我修正（已回填 §3.2 与 §4 表）

**① 「变换级联可以整体删掉」是错的。** core3d 删得掉，是因为它的 draw list 存局部变换、
渲染期沿层级折叠；**我们产出的是世界系网格**，祖先矩阵已经烘进后代顶点。
真库门 ams8000 195→196 实证：有 1 件后代的**索引记录完全没变**，只有级联捞得回来。
→ 级联保留，换的只是触发判据。

**② P0-1 的修复在现有语料里证不出输出差异。** 全部 443 个库扫下来只有 22 个改挂窗口
（ams8000 十八个、ams7999 四个）：

- 22 个的本级 `(POS, ORI)` **全都没动**——老判据的盲区是真的、命中率 100%；
- 但被改挂的要么自己就是模型单元（`BOX`，索引已点名，target 端上卷照样重建），
  要么上下都没有单元（`FTUB`，两种实现都不产动作）。

**没有一个窗口能把修好的实现和没修的区分开。** 所以真库门的 `Shape::Reparented` 样本
只钉「L2 看得见这条改挂、且它确实是老判据看不见的形状」，**不**断言「不修就会漏几何」。
正确性依据是 `world_matrix` 沿 owner 链折叠这条构造性事实 + 谓词单元测试。

## 真库门五窗实测

| 窗口 | 候选 | 语义账 | 结果 |
|---|---|---|---|
| ams7999 插入 45→46 | 23 (+22 ~1) | created=22（子树顶 1） | regen 3 |
| ams8000 修改 255→256 | 3 (+0 ~2 -1) | deleted=1、members=1、attrs=1 | regen 1 |
| ams1112 删除 721→722 | 24674 (-24673) | deleted=24673（**子树顶 6**） | remove 3406 |
| ams8000 级联 195→196 | 1 (~1) | attrs=1、cascades=1 | regen 1 |
| ams8000 改挂 45→46 | 3 (~3) | **reparented=1**、members=2、attrs=2 | regen 1 |

`unchanged` 五窗全是 **0**：当前语料里索引点名的 `Modified` 候选没有一个是
「记录重写但内容没动」的虚候选。这一层暂时没省下东西，但它现在能被观测到了。

---

# 第四篇 · 与 RJYQ(8e9b5113) 的交接 + 短 DESP 分歧修复

## 对方做的（我核实后确认无冲突）

他们诊断真库门红灯：ams1112 全量说消失 3398、增量只移除 3356，差 42 件。
根因是 `nearest_unit` 沿 owner 链只认 `is_positive_solid()` 四类，而 `SBFI` 是 `Catalog`。
修法：`category.rs` 新增 `is_model_unit()`，全量与增量共用。

我这轮的 `increment.rs` 重构本来就读这个函数（`nearest_unit` + `execute_plan` 两处），
两边不冲突。他们提的 fmt 两处（`pipeline.rs:249`、`category.rs:197`）已清。

## 他们留给我判断的第二条 —— 是活的分歧，已修

「DESP 够不够长」这道门**只写在 `pipeline.rs` 的 match 守卫里**，产出侧
`model_element` 没有。于是同一个短 DESP 的 SBFI：

- 全量：守卫不匹配 → 落 `catalog` 桶，当没这件事；
- 增量：直接调 `model_element` → `Degenerate` → 记 `regen_failed` 并发 Remove。

**这正是他们刚修的 SBFI 事故的同一形状：判据写了两份。** 对方认为「那是数据门不是
noun 名单，该留在管线里」——但漏件的机理跟是不是名单无关，只跟写了几份有关。

修法（门收回产出侧）：

- `elmodl.rs` 新增 `sbfitting_no_shape_reason`，短 DESP 与尺寸退化都落 `Skipped`
  （与正体那条 `primitive_no_shape_reason` 同款惯例）；
- 新增常量 `SBFI_WHAT` / `SBFI_DESP_MIN`，取代散落字面量；
- `pipeline.rs` 删掉内联守卫，两条路共用同一条 `model_element`。

## 语料里真有这种元素（不是潜在修复）

ams1112 一个库三件：`17496/153987`（DESP 5）、`17496/140426`（7）、`17496/136127`（7）。

改之前它们在全量落 `catalog`、在增量落 `regen_failed`——`failed` 是「本该建出来却没建成」
的信号，对着三件根本不是套管的元素喊狼来了。改之后两边都落 `skipped`，理由带实际长度。
**动作集本来就一致**（目标端也没有它们），所以 `accounts_for` 那类等式判据一个都不响，
只有把两份报告并排看才发现得了。

新测试 `a_short_sbfi_desp_is_no_shape_not_a_failure` 守这个接缝，并钉住反面：
`ZDIR` 退化算不出对齐矩阵是**真坏数据**，必须留在 `failed` 里，不许被顺手咽掉。

## 当前全绿状态

lib 103 / rvm_compare 6 / noun_coverage 6 / 真库门 5 窗 1 passed；
clippy `--lib --tests --bins` 零告警；`cargo fmt --check` 零差异。

---

---

# 第五篇 · 「判据写两份」普查收口（第三例、第四例）

RJYQ 采纳了第 4 条建议并做完了普查，四个面：

| 面 | 结论 |
|---|---|
| match 里的 if 守卫 | 只剩 `Category::Catalog if is_model_unit(&noun)` 一条，判据共用。干净 |
| 分类入口 | 全量走 `decision().category`、增量走 `classify()`；`decision` 第一行就是 `classify`。无分歧 |
| `model_element` 调用面 | `is_positive_solid()` 恰好 = 四条产出臂 + Catalog/SBFI 一臂 = `is_model_unit`。对齐 |
| `push_members` 剪枝 | **第三例** |

## 第三例（RJYQ 修，我复验通过）

`push_members` 把「属主 `consumes_members()` 且成员是 ProfileData 或负体」的成员整棵剪掉，
那些元素连访问都不会被访问；而增量的子树收集不认这条剪枝，只按 `is_model_unit` 挑单元。
两条路不打架的唯一条件是「可被剪掉的 noun 永远不是模型单元」——今天成立，纯属
`is_positive_solid()` 与 `is_negative()` 互斥的副产品，**没有任何东西钉住它**。

钉法：`tests/noun_coverage.rs` 新增 `a_noun_the_traversal_can_prune_is_never_a_model_unit`，
拿 `data/noun-family-matrix.json` 字典全集逐个反查，并用 `pruned > 0` 挡住空转。

## 第四例（我修）—— 那个「恰好对齐」本身没被钉住

RJYQ 第 3 条查出「`is_positive_solid()` 恰好等于 `model_element` 的四条产出臂」，
结论是「对齐」。**对齐是事实，但它是隐式的**——`is_model_unit` 写成
`category.is_positive_solid() || (category == Catalog && noun == "SBFI")` 时，
给 `Category::GeometrySet` 补一条产出臂而忘了改这里，后果不是编译错误：
全量照常出几何，增量永远上卷不到它，**被删的那些永远不出 Remove**，两边账各自还都平。
**这正是 SBFI 那次静默漏件的机理，一字不差。**

修法：把 `is_model_unit` 改写成**无通配符的穷举 `match classify(noun)`**——
四条正体臂 → true，`Catalog` → `noun == "SBFI"`，其余 15 个变体逐条列 → false。
往 `Category` 里加变体，这个函数当场编译不过，加变体的人**必须回答**「它算不算模型单元」。

把「记得同步」从人交给编译器，是这个函数唯一能给出的真保证。`_ => false` 会把
「新变体默认不是单元」这个危险假设变成沉默的默认值，所以那 15 条不许合并成通配符。

## 这次普查本身值得记一笔

第三例与第四例**是同一个形状**——「两条路不打架的唯一条件是 X，今天成立，
但没有东西钉住 X」——被两个人**各抓到一半**：RJYQ 在剪枝那条钉了，
在产出臂对齐那条查完「今天对齐」就收手；同一份报告里两套标准。

单人审计漏的正是**自己刚用过的那把尺子**。这不是谁不细心，是审计者对自己刚建立的
判断范式会失去距离感。交叉复验的价值在这里，不在多一双眼睛看同样的地方。

## 与 RJYQ 的分工边界（避免撞车）

- 他们那条线：BRAN/HANG，剩 `RPRO TLEN`（1529 处 FTUB 的 `PPRO` 空槽）+ 三拨未入库改动。
- **撞车点**：`elmodl` 里 FTUB 的 TLEN 求值。本线若动到那里，先喊 8e9b5113。
- RouteContainer / `GeometryId` 键改造归本线，他们不动，等 L4 与 P0-2 之后。

## 剩下的已知口子（记账，不是新发现）

RouteContainer 在全量产出隐式管身，按 `GeometryId::ImpliedTube` 索引，
而增量 Remove 按 refno 发，容器 refno 上没有「一件」几何可删。`is_model_unit` 有意排除它，
代价挂在 `IncrementReport::stale_route_containers`。真修要把增量的键从 refno 换成
`GeometryId`——那会一起动到 `update_ams` 与 RVM 对拍，属本条线的活。

## 一条读数更正（RJYQ 报，与本线无关但记下）

管身走向那句「横向偏移 < 1mm 只有 35.3%」是误导性指标：分母把 2493 条零长槽位排除了，
等于专挑模型自己歪的地方打分。正确口径（**RJYQ 二次更正后的终值，本档案初记的
「2833 条 / 88%」是他们第一版口径，漏减了解不出 P 点的 6 条**）：

> 可解槽位 **2827** 条（= 零长 2493 + 有间隙 334；库没开的 339 条与解不出 P 点的
> 6 条不入分母）中，2493 条（**88.2%**）两端重合到 0.051mm 以内；
> 216 条（占可解槽位 7.6%）解得出但走向歪——那是模型自身脏数据，不是求解错误，
> 同一套链在那 2493 条上是精确的。最大两条来自 POS 还停在原点的游离构件 24384/24382。

**本档案与设计文档从未引用过 35.3%**（已 grep 全仓确认）。
`examples/tube_axis_check.rs:642` 的旧口径 RJYQ 已改：判据三保留但加明示分母，
另起一段写全「求解正确率 / 模型体检」两个口径。

## 当前全绿

lib 104 / rvm_compare 6 / noun_coverage 7 / 真库门 5 窗 1 passed；clippy 与 fmt 零告警。

---

## 下一步（未做，按设计文档 §3 排序）

1. ~~**L4 换 `noun.toplevel`**~~ —— **方案已被实测推翻，见第六篇。**
   `toplevel` 与 `is_model_unit` 是两层不同粒度、互不包含，不能替换；
   且取数路径比原计划短得多（只动 Python 探针一行，不动 `old-parse-pdms-db`）。
2. **P0-2 隐式管身**：抄 core 的 TUBI 特例（`(POS, ORI, ITLE, SPRE)` + `FNDTOP` 跨 BRAN 上卷）。
3. **`execute_plan` 补「已产出模型集」输入**：对标 `getChildTOPFElements`——顶层元素在
   target 端已不存在时，库里问不出它下面曾经有过什么，只能问自己的产出。
4. ~~**全量/增量分歧普查**~~ **已做完**，见第五篇：共四例，四例全部钉住或修掉。

---

# 第六篇 · L4 方案实测推翻（还没动代码，只取了数）

趁等指令，先把 L4 的数据取回来验一验——结果推翻了设计文档 §3.1 自己写的方案。

## 取数路径比原计划短

`scripts/noun_family_probe.py` 已持有 `AttrDataFile`，且已在用
`df.raw_field(noun_hash, FIELD_POSITIVE_EQUIVALENT)` 直读任意字段。加 `toplevel`
**不需要动 `old-parse-pdms-db`、不需要重导 `noun_flags.json`**：探针加一行
`df.raw_field(h, 661628) == 1`，重跑 `data/noun-family-matrix.json` 就完事。

原计划指的 `dict.rs:1111` 导出器产出的是 `noun_flags.json`，而 **e3d-model 根本不读它**
（只 `include_str!` 了 `data/route-nouns.json` 与 `data/noun-family-matrix.json`），
`noun_flags.json` 只是探针拿来把 hash 翻短名的中间物。

## 实测分布（3.1 字典，1931 noun，211 个 toplevel=1）

| noun | `toplevel` | `is_model_unit` |
|---|---|---|
| `BOX` `CYLI` `REVO` `POLYHE` `AEXTR` | **false** | **true** |
| `SBFI` | **false** | **true** |
| `PANE` `GWALL` `FLOOR` `SCTN` `WALL` `STWALL` | true | true |
| `EQUI` `STRU` `TMPL` | **true** | **false** |
| `BRAN` `HANG` `LUG` `SUPC` `TRUNNI` | **true** | **false** |
| `TUBI` `FTUB` `SITE` `ZONE` `WORL` `FIXING` | false | false |

（2.10 字典只有 1384 个 noun、155 个 toplevel，口径不同；以 3.1 为准，
因为真库门用的就是 `shadow_e3d31_aps_all\attlib.dat`。）

## 结论：两个集合互不包含，不是替换关系

不是数据错，是**两层不同粒度**：`toplevel` = 可绘单元（core3d 的 draw list 一条
就是一整件 `EQUI`，底下的 `BOX` 不单独成条）；`is_model_unit` = 网格产出单元
（我们一个 `BOX` 出一件网格、挂自己的 refno）。

硬换会同时坏两头：① 改一个 `BOX` 就得重建整件 `EQUI`，把重建范围凭空放粗一个数量级，
而我们的产物粒度里根本没有「一件 EQUI」；② 五个路由容器全是 `toplevel=true`，
会被重新拉进来——那正是 `is_model_unit` 有意排除的一批。

**L4 应该是新加一层更粗的键，不是替换现有那层。**

## 附带解开一个死结

`BRAN`/`HANG`/`LUG`/`SUPC`/`TRUNNI` 全部 `toplevel=true`，而隐式管身在概念上
就属于它的容器——**「RouteContainer 的 Remove 无键可发」与「引入 toplevel 层」
是同一件工作**：容器就是那条管身的 toplevel 键。原计划把这两件排成先后两项，
应该合并做。

## 状态：只读取证，未改任何代码

本篇全部结论来自只读探测（Python 直读 attlib）。`src/` 一个字没动，仍是 118 项全绿。

---

# 第七篇 · 入库（RJYQ 执行，本线绿灯）

用户让 RJYQ 把两个仓的未入库改动落库。**按文件切分做不到**：新 `category.rs` 比 HEAD 多五个
变体（RouteContainer / RouteMember / GeometrySet / NegGeometrySet / Composite），
老 `pipeline.rs` 的穷举 match 立刻缺臂；新 `pipeline.rs` 又要 `route.rs` + `catalogue_point.rs`
+ 新 `lib.rs`，而新 `lib.rs` 声明了本线的 `element_diff` 与 `ledger`。一条链焊到底。

**依赖方向核实（我不能先提我那部分）**：`increment.rs` 引
`category::{Category, classify, is_model_unit}`，`nearest_unit` 判 `Category::RouteContainer`，
我新写的穷举 match 里列了那五个变体——**全是 RJYQ 那条线加的**；`category.rs` 还
`include_str!` 了他们的 `data/route-nouns.json`。**是本线依赖他们，不是反过来。**

拆法（按仓两笔）：

- 笔一 `e3d-io`：目录求值链为主体，顺带本线的 `index/diff` +116、`index/cursor`、
  `tests/index_diff_real` +93；
- 笔二 `e3d-model`：全量，含本线的 `increment.rs` +418、`element_diff.rs`、`ledger.rs`、
  `tests/increment_real.rs` +664。

**本线终检后给了绿灯**（工作树无半截代码，L4 全程只读，118 项全绿），并提醒两点：

1. `data/` 与 `docs/` 是未跟踪目录，含 `noun-family-matrix.json`（`category.rs` 与
   `noun_coverage` 都 `include_str!` 它）、`route-nouns.json` —— **漏了直接编译不过**，
   别被 `??` 状态骗过去；
2. `scripts/` 四个 py 是那些数据的**生产者**（`noun_family_probe.py` 产
   `noun-family-matrix.json`、`icom_field_ids.py` 是字段号出处），数据进库而生产者不进，
   下次重跑只能从头反编译。

待 RJYQ 回 sha 后补记在此。

## 工作日志

| 时间 | 事项 |
|---|---|
| 16:55 | 接任务；定位 `grilling` 技能与两份前序档案 |
| 16:5x | 读 `increment.rs` 全文、`transform.rs` 全文，复核 P0-1 / P1-1 / P1-2 |
| 16:5x | 读 `e3d-io/src/index/diff.rs` + `cursor.rs`，坐实 Modified = RecordPosition 变（新） |
| 16:5x | `rg` 实测建模侧零指针解引用；发现 `catalogue_point.rs` / `route.rs` 的 SPRE→CATR 链（新） |
| 16:5x | 4 份活库提取实查 `CACHID`：value 为 null 或 raw=0，`encoded_location=0`（新） |
| 16:5x | 读主计划 §1.2，发现产物形态已拍板分离，与 increment 模块文档冲突（新） |
| 16:5x | 落本档案；交付审核结论 + grilling 第一问决策卡 |
| 17:2x | 用户否掉四个选项，改派：ida-bridge 取证 core/core3d 增量机制 |
| 17:3x | 活桥取证五层管线；坐实 `attributesChangedBetween` 指针属性比法、TUBI 四属性硬编码、`noun.toplevel` |
| 17:4x | 落 `docs/plans/2026-08-31-core-aligned-increment-architecture.md` + 第二问决策卡 |
| 18:0x | 用户拍板「建 L2/L3 两层」 |
| 18:1x | 写 `element_diff.rs`（L2）、`ledger.rs`（L3）；`transform.rs` 导出 `PLACEMENT_ATTRIBUTES` |
| 18:2x | `plan_update` 改接 L2/L3；删 `placement_drifted`；改挂时旧属主也上卷 |
| 18:3x | 真库门加第五个样本 `Shape::Reparented`；写 `probe_chain_for_reparent` |
| 18:4x | 全库 443 个扫改挂：22 个窗口，**无一具判别性**——回填设计文档，门禁不写它证不了的断言 |
| 18:5x | 回填设计文档 §3.2：「级联可删」是错的（世界系产物必需），有 ams8000 195→196 实证 |
| 19:0x | 收到 RJYQ(8e9b5113) 交接：`is_model_unit` 已共用，核实与我的改动无冲突；清 fmt 两处 |
| 19:0x | 查证短 DESP 分歧是**活的**（ams1112 三件），门收回 `elmodl`，加测试；全套复跑全绿 |
| 19:1x | 回告 RJYQ；实时续写本档案 |
| 19:19 | RJYQ 采纳并做完普查，找到第三例（`push_members` 剪枝），已钉测试；我复验通过 |
| 19:2x | 复验其第 3 条「恰好对齐」——对齐属实但**隐式**，即第四例；`is_model_unit` 改无通配符穷举 match |
| 19:2x | grep 全仓确认从未引用过 35.3%，无需更正；全套复跑 118 项全绿 |
| 19:26 | RJYQ 认第 3 条、改完 `tube_axis_check` 旧口径，并二次更正分母 2833→2827（88.2%）；本档案跟改 |
| 19:2x | 记下分工边界：FTUB TLEN 求值是撞车点，本线动它前先喊 8e9b5113 |
| 19:4x | 等指令期间只读取证 L4：3.1 字典实测 211 个 toplevel，**与 is_model_unit 互不包含** |
| 19:4x | 推翻设计文档 §3.1 的「换判据」方案并回填；发现 toplevel 与 ImpliedTube 键是同一件工作 |
| 19:45 | RJYQ 要落库，给十分钟叫停窗口；终检工作树无半截代码，给绿灯 |
| 19:4x | 核实依赖方向：本线依赖其五个 Category 变体，**不能先提本线**；提醒 data/ 漏提会编译不过 |
