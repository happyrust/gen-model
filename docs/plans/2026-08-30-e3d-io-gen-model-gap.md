# e3d-io 能力缺口体检：对照 gen-model 生成链的真实需求

> 日期：2026-08-30。状态：**侦察结论，待排期**。
> 前提：业主已拍板 —— **gen-model 直读 E3D 库文件生成模型，用 `old/vendor/e3d-io`，不用 `old-pdms-io`**。
> 对象：`D:\work\plant-code\old\vendor\e3d-io`（HEAD `357512d`，工作树另有他人在飞的 `src/index/diff.rs`）。
> 需求侧：ADR-053「生成期查询面」+ `src/fast_model/{resolve,cata_model,prim_model,loop_model}`。
> 关联：ADR-053（direct 模式生成读）、ADR-055、`docs/plans/direct-mode-model-generation.md`（P0 已完成）、
> `docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`（记录层自寻址，本文的前提之一）。
>
> **本文每一条带数字的结论都来自真跑**，探针在
> `old/vendor/e3d-io/examples/genmodel_gap_probe.rs`（本轮新增，**只加 examples，未动 src/tests**）。
> 复现命令见 §5。凡未实测的判断都显式标了「未测」。

## 0. 一句话

**读一个元素这件事，e3d-io 又快又准，可以放心用。卡住生成链的是「一个元素之外」的三件事：
跨库、会话时点、表达式求值。** 前两件是硬阻塞，第三件决定目录几何能不能算对。

| # | 项 | 结论 | 阻塞 | 工量 |
|---|---|---|:--:|---|
| G1 | 跨库引用（CATR/SPRE/LSTU…） | e3d-io 一次只开一个文件，**没有 dbnum→文件 的定位器** | **阻塞** | 中（在 DirectStore 侧建，不必改 e3d-io） |
| G2 | 会话时点 pin（`applied_sesno`） | `DbView` 能 pin，**`ReadOnlyEngine` 不能**，而元素/UDA 高层 API 全挂在后者上 | **阻塞** | 小（给 `ReadOnlyEngine` 加 `open_at`，约 30 行） |
| G3 | children 顺序 | 成员表集合 100% 正确，**顺序有 0.26% 不等于 refno 序** | 不阻塞（**是正确性陷阱**） | 0（照原序用即可，别排序） |
| G4 | 表达式属性 | 五路渲染器齐全但**分派器私有**，且**只渲染成 E3D 显示文本、不求值**，方言与现有求值器不同 | **阻塞**（目录几何） | 中（公开分派器 + 方言映射） |
| G5 | UDA（含 OLDKEY） | 直接可用，0 issue | 否 | 0 |
| G6 | 深层 children 遍历 + noun 过滤 | 可做，代价可忽略；但**没有 noun 索引，过滤在读之后** | 否 | 小 |
| G7 | 性能 | 热读 0.005 ms/元素、带全属性解码 0.026 ms/元素 | 否 | 0 |
| G8 | 反向引用（BREF/SPBREF） | **无反向索引**，与 ADR-002/003 的 Surreal 反查等价物缺失 | 视用法 | 中 |
| G9 | 快照守卫（文件身份 / 冻结前缀） | 无 `DabaconSnapshot` 等价物 | 否（有替代口径） | 小 |
| G10 | 并发形态 | `DbView` / `ReadOnlyEngine` 都是 `&mut self` | 否 | 0（按 dbnum 建池，与 ADR-053 已定姿势相同） |

---

## 1. 阻塞项

### G1 · 跨库引用：**e3d-io 只认一个文件**

**生成链需要什么。** ADR-053 的收口清单里 `get_cat_refno`(4 处) 是「存量引用 1–3 跳走查
（`CATR`/`SPRE`/`PRTREF` 链收口 SCOM/SPRF/SFIT/JOIN）」，`query_group_by_cata_hash`(4)、
`get_or_create_cata_context`(2) 同样落在目录库上。DESI 元素拿元件、拿规格、拿材质，全要跳库。

**e3d-io 现状。** `ReadOnlyEngine::open(path)` / `DbView::open_at(path, …)` 都只接一个文件路径；
`PageReader` 会自动发现同名多 extent（`ExtentNaming`，比旧栈强），但**没有任何 dbnum→路径的注册表**。
`RefNo::dbno()`（`src/refno.rs:17-19`）能算出目标属于哪个库，仅此而已。

**实测（6 个真库、588 142 个活键，`genmodel_gap_probe crossdb`）。** 按 `desvir.dat` 描述符取**命名**
引用属性，统计目标落在本库内还是别的库：

| 库 | 活键 | 指向本库 | **指向别的库** | 出库大头 |
|---|---:|---:|---:|---|
| ams8000_0001 | 6 605 | 1 415 | **6 461（82%）** | SPRE 2659 / LSTU 2579 / PSPE 607 / HSTU 531 / CATR 34 / MATR 28 / ISPE 23 |
| ams7999_0001 | 34 707 | 5 378 | **32 560（86%）** | SPRE 16192 / LSTU 11249 / ISPE 2012 / PSPE 1773 / HSTU 1174 |
| ams7333_0001 | 199 096 | 21 013 | **332 526（94%）** | SPRE 158818 → db 7320 |
| ams7327_0001 | 42 250 | 3 177 | **66 566（95%）** | SPRE 34210 → db 7320 |
| ams1112_0001 | 30 940 | 6 | **812** | SPRE 812 → db 7001 |
| ams7000_0001 | 274 544 | 0 | 0 | （desvir 模板与该库类型不符，命名遍全部提取失败，见下「注意」） |

具体样例：`24384/2501 --SPRE--> 13244/109369 (db 5052)`、`24384/24823 --CATR--> 13244/131367 (db 5052)`、
`24384/24948 --MATR--> 15520/3252 (db 7328)`、`23717/46 --SPRE--> 31896/11559 (db 7320)`。

**好消息：owner 从不跨库。** 上表 6 个库、588 142 个元素，`owner.dbno() != 自身 dbno()` 的
**一个都没有**。ADR-053 R2 担心的「跨库 owner（DESI→SITE 库）」在本语料**未出现**，
所以 `get_world_transform` 的祖先链折叠单文件就够。
（措辞留边界：这是 6 个库的实测，不是格式保证。）

**缺口。** 没有「dbnum → 文件路径」的定位器，也没有多库句柄池。

**工量与归属。** **不必改 e3d-io**：定位器本就是 gen-model 的知识（`CataDbLocator` + `dbnum_watermark`
已有，ADR-053 P1 的 `DirectStore` 就写了「dbnum→会话池 + ref0→dbnum 定位复用 `CataDbLocator`」）。
DirectStore 侧新增约 120–180 行：`DashMap<dbnum, Mutex<DbView>>` + 未注册 dbnum 显式报错（fail loud，
不静默返回 None）。**阻塞点在于：接线的人不要指望 e3d-io 帮忙跳库。**

> **注意（探针自身的边界）**：ams7000 那一行的 0/0 不代表它没有跨库引用，而是命名遍用
> `DESIGN_TEMPLATE_TYPE` 打开 `desvir.dat`，该库不是 Design 类型，`extract_element_with_descriptors`
> 全部失败被跳过。字扫描遍（不依赖描述符）在 ams8000 上给出 6 756 个出库引用字，
> 与命名遍 6 461 同量级，可互证；但字扫描有假阳性（`dbno=65536` 这类 706 次命中是两个整数
> 恰好像引用），**不要引用字扫描的数当权威**，权威是命名遍。

### G2 · 会话时点：**`ReadOnlyEngine` 没有 `open_at`**

**生成链需要什么。** ADR-053 Q3 已拍板 **A**：按 dbnum pin `applied_sesno` 读，与 DB 模式同一逻辑时点，
否则 Q5 的双跑对拍失去意义。

**e3d-io 现状（两套入口，能力不对齐）：**

| 入口 | 能 pin 会话？ | 有元素级 API？ |
|---|:--:|---|
| `session::DbView::open_at(path, SessionSelector::Exact(sesno))`（`src/session/mod.rs:201`） | **能** | 无（只给 `index_root()` / `pages_mut()`） |
| `ReadOnlyEngine::open(path)`（`src/engine.rs:29`） | **不能**（只有 `open`/`open_with_cache`） | `find_element` / `extract_element_with_descriptors` / `indexed_refnos` |
| `UdaCatalog::read(dictionary_db, dicvir, attlib)`（`src/uda_catalog.rs:139`） | 不能（内部 `ReadOnlyEngine::open`） | — |

**实测（`genmodel_gap_probe session`，ams8000_0001）：**

```
sessions=264
newest sesno=264 index_root=PageId { ext: 1, page: 8890 }
pinned  sesno=176 index_root=PageId { ext: 1, page: 7499 } sessions_read=89
keys newest=6605 pinned=6540 only-in-newest=65 only-in-pinned=0
pinned read ok: 16192/0 noun=0x000BEB83 bytes=156 blocks=2
ReadOnlyEngine::open selects sesno=264 (no open_at exists on it)
```

**不 pin 就差 65 个键**，正是「摄入还没追平」时 direct 与 DB 分叉的那批。

**可用的绕行路径（已跑通，DirectStore 现在就能用）：**

```rust
let mut view = DbView::open_at(path, SessionSelector::Exact(applied_sesno))?;
let root = view.index_root();
let found = IndexCursor::at_root(view.pages_mut(), root).seek(refno)?.found;
let record = record::read_record(view.pages_mut(), found.loc.page, found.loc.offset_words)?;
let parsed = ParsedElement::parse(&record.bytes)?;
```

**缺口。** 高层 API（描述符提取、UDA 物化）拿不到 sesno。

**工量。** 小：给 `ReadOnlyEngine` 加 `open_at(path, impl Into<SessionSelector>)`，内部换成
`session::resolve` 那条路（`DbView` 已经有），约 30 行；`UdaCatalog::read` 再加一个带 sesno 的重载。
**这是要改 e3d-io 的 src，不在我的写入面内，写成缺口交给业主派人。**

**另一个数**：pin 到链中段要回走会话链，`sessions_read=89`（89 次页读）。这是**开库一次性成本**，
按 dbnum 缓存住 `DbView` 就摊掉了，不影响单元素读。

### G4 · 表达式属性：渲染有，求值没有，方言还不一样

**生成链需要什么。** 目录几何（PTCA/SEXT/SCTN…）存的不是数，是表达式；
`fast_model` 经 `aios_core::eval_str_to_f64(expr, &CataContext, …)` 求出 `f64`
（底层 `aios_core::tiny_expr::expr_eval::interp`）。DB 模式里，写库侧把表达式落成**字符串**，
生成期再求值。

**e3d-io 现状。** 五个渲染器都在，且都是 `pub`：
`record::catalogue_expr::render`、`record::axis_spec::render`、`record::direction_spec::render`、
`record::point_list::render`、`record::catalogue_pml::render`、`record::text::packed`。
**但把它们串起来的那个分派器 `rendered_by_shape` 是 `src/tty.rs:364` 的私有函数**，
`extract_element_with_descriptors` 交给调用方的是裸 `DescriptorValue::RawWords(Vec<u32>)`。

**实测（`genmodel_gap_probe expr`，ams5052_0001 目录库，抽 3 000 个元素）：**

| 度量 | 值 |
|---|---:|
| 值是数字的属性 | 1 120 |
| 值是 raw 字元组的属性 | **4 001** |
| ├ `catalogue_expr` 能渲染 | 1 496 |
| ├ `axis_spec` 能渲染 | 436 |
| ├ `direction_spec` 能渲染 | 129 |
| ├ `point_list` 能渲染 | 27 |
| └ **五路都渲染不了** | 1 913 |
| 显式流里已被解成 PML 文本的 | 1 852 |

那 1 913「渲染不了」的**基本不是缺口**：抽样看到的是全零元组
（`PX: [0,0,0,0]`、`PRAD: [0,0,0,0]` —— PML 正文在显式流里，已计入上面 1 852 那行）
与 `TYPEX: [1, 644065]`、`LEVE: [0, 10]` 这类**本来就不是表达式**的值。

**真正的两个坑：**

1. **分派器私有。** 消费方要自己按顺序试五个渲染器（我在探针 `render_by_shape` 里抄了一遍，约 18 行）。
2. **只有显示文本，没有数，而且方言与现有求值器不同。** 渲染出来长这样：

   | 渲染器 | 实测样例 |
   |---|---|
   | `catalogue_expr` | `PARAM 3`、`- PARAM 3`、`2550`、`0` |
   | `axis_spec` | `-X`、`Y`、`P5`、`P1011` |
   | `direction_spec` | `Y ( ATTRIB PARA[10 ] ) X`、`AXIS -Y ( ATTRIB RPRO G ) Z`、`Y ( ATTRIB PARA[12 ] / 2 ) X` |
   | `point_list` | `P61 P71`、`P78 T76 P80` |

   而 `aios_core` 现有求值器吃的是写库侧那套：`DESIGN PARAM 1`、`DESI[1.1]`、`RPRO_CPAR`、
   `X ( 45 ) Y ( 35 ) Z`、`TANF PARAM 2 DDANGLE`、`LBOR OF 24381/88991`（见
   `src/test/test_cata_expression.rs`）。**`ATTRIB PARA[10 ]` 与 `ATTRIB RPRO G` 这两种写法
   现有求值器没见过。**

**缺口。** ①公开一个 `render_by_shape` 等价物；②E3D 显示方言 → 现有求值器方言的映射（或反过来扩 eval）；
③`P61 P71` 这类点号列表根本不是标量表达式，消费方要按属性语义分流。

**工量。** ① 约 20 行（e3d-io 侧，或消费方自己抄）；② **中，且是这次最需要先做对拍的一项**：
建议做法是取同一批目录元素，DB 模式读出的表达式字符串 vs e3d-io 渲染出的字符串**逐条并排**，
把差异归成有限几类方言规则再动手，**不要凭样例猜规则**。③ 归转换器。

---

## 2. 不阻塞、但会踩的

### G3 · children 顺序不能重建

**实测（`genmodel_gap_probe children`，ams8000_0001，6 605 键全量）：**

```
elements stating a member list: 2332
member RefNos the session does not index: 0
members whose own owner is another element: 0
members listed twice by the same parent: 0
stated list == owner-grouped set, same order:  2326
stated list == owner-grouped set, other order: 6
stated list != owner-grouped set:              0
```

**集合层面 100% 干净**：成员表列出的每一个 RefNo 都被本会话索引、owner 都反指回来、没有重复，
而且与「把索引走查按 owner 反向分组」得到的集合**完全相同**。

**但顺序不同**，2 332 个里有 6 个（0.26%）：

```
24384/24775: stated [26195, 24776, 24780, ...] vs refno-sorted [24776, 24780, 24782, ...]
24384/24932: stated [26199, 24933, 24940, 24945, 25586] vs refno-sorted [24933, ..., 26199]
16192/0    : stated [46, 24932, 22399]        vs refno-sorted [46, 22399, 24932]
```

**含义**：成员表的顺序是**存进去的顺序**，不是 refno 序，也**不能从索引重建**。
BRAN 的成员序就是管路走向，ZONE 下的顺序影响遍历序。

**要求（写给转换器与 DirectStore）**：直接用 `ParsedElement.members` 的原序，
**不要 sort、不要 dedup、不要用「索引走查 + 按 owner 分组」代替**。
DB 模式那边 `get_children_named_attmaps` 走的是 `pe_owner` 图（顺序取决于摄入写入顺序），
**双跑对拍时如果只比集合就发现不了这 0.26%**，建议对拍比较序列而不是集合。

### G6 · 深层 children 遍历 + noun 过滤

**实测（`genmodel_gap_probe deep`，ams8000_0001）：**

| 起点 | 读到记录 | noun 过滤命中 | 深度 | 页读 | 耗时 |
|---|---:|---:|---:|---:|---:|
| 库根 `16192/0`，过滤 `FTUB` | 6 604 | 1 538 | 8 | 1 090 | **9.2 ms（1.4 µs/条）** |
| `24384/47`（124 个成员），不过滤 | 2 451 | 2 451 | 6 | 317 | **2.6 ms（1.1 µs/条）** |

两次都是 **0 个成员落在库外、0 个成员本会话不索引** —— Design 库内的子树是闭合的。

**缺口（很轻）**：没有按 noun 的索引，过滤只能读完再判。但 1.4 µs/条的代价下这不值得优化。
`query_multi_deep_versioned_children_filter_inst`(5 处)、`query_filter_deep_children_atts`(2 处)
这类查询等价物**可以直接在成员树上做**。

### G5 · UDA 可以直接用

**实测（`genmodel_gap_probe uda`，Dictionary=ams5100_0001，Design=ams8000_0001）：**

```
definitions=185 issues=0 read=3 ms
definitions carrying OLDKEY: 184, pseudo: 0, dynamic name: 0
sampled elements with at least one UDA: 2305
UDA values materialised: 21130
sampled elements storing a value under an OLDKEY hash: 0
descriptor extraction failed on: 0
nouns carrying UDAs: FTUB 731 / BEND 347 / BRAN 274 / SCTN 268 / ATTA 150 / PNOD 118 / ...
```

`UdaCatalog::read` 3 ms 读完整个 Dictionary，`materialize_for_element` 把 UDA 值与默认值一起并进
`ElementExtraction`。**OLDKEY 两代 key 已内建**（`UdaDefinition.old_key`，185 条里 184 条带；
本库当前没有元素还存在旧 key 下，所以这条路径在 ams8000 上没被走到 —— **能力在，未被本语料压测**）。

唯一要注意的是它继承 G2：`UdaCatalog::read` 内部 `ReadOnlyEngine::open`，**读的是 Dictionary 库的最新会话**。
Dictionary 变动很少，风险低，但严格 pin 的话要一起改。

### G7 · 性能不是问题

**实测（`genmodel_gap_probe perf`，ams8000_0001，200 个抽样元素，`--release`）：**

| 形态 | 每元素 |
|---|---:|
| 冷读（每个元素重开一次文件，绕过索引） | **0.028 ms**，3.0 页 |
| 热读（一个 view，seek + read） | **0.005 ms** |
| 热读 + 解出全部描述符属性（均 50.8 个属性/元素） | **0.026 ms** |

对照 `direct-mode-model-generation.md` P0 记的 **direct ≈5.0–11.0 ms/元素**（debug 构建、
经 `PdmsIO`、含进程内首连开销）与 **DB ≈13.0–15.9 ms/元素**。

**注意口径差**：P0 是 debug 构建且做的是完整 attmap 转换与 diff，这里是 release 且到
`ElementExtraction` 为止（还没转成 `NamedAttrMap`），两者不能直接相除得倍数。
能说的是：**e3d-io 侧的读取与解码开销是微秒级，转换与调度才是后面要量的**。
200/200 元素描述符提取成功，0 失败。

---

## 3. 还缺、但这次没实测的

| # | 项 | 现状 | 建议 |
|---|---|---|---|
| G8 | **反向引用**（BREF/SPBREF 等） | e3d-io 只有 refno→记录 的正向索引，**没有反向索引**。ADR-053 的表里这一行本就写着「部分（Surreal 侧反向索引）」 | 先盘清生成链到底哪几处要反查；若只有少数几处，用「一次全库走查建内存反向表」（按上表 1.4 µs/条，30 万键的库约 0.4 s）比造索引划算 |
| G9 | **快照守卫** | 无 `DabaconSnapshot` 等价物：`PageReader::open` 只在打开时记一次文件长度、之后用 positioned read（`src/page/io.rs:149-160,291-294`），没有 volume+file_index 身份守卫、没有冻结前缀、没有四次稳定捕获 | direct 是**只读且 pin 了 sesno**，ADR-053 R5 已论证 pin 天然免疫「读到未应用会话」；剩下的「文件被原子替换」沿用 gen-model 侧现有文件身份守卫即可。**不建议**把 `DabaconSnapshot` 搬进 e3d-io |
| G10 | **并发** | `DbView` / `ReadOnlyEngine` 都是 `&mut self`（页缓存可变） | 与 ADR-053 已定姿势相同：按 dbnum 建 `Mutex` 池。不是新问题 |
| G11 | **`NamedAttrMap` 转换** | `ElementExtraction` 已经把词哈希解成 `DescriptorValue::Word{raw, text}`（`src/engine.rs:849-857`），正好对上 P0 残差第 1 类「词哈希归一」 | ADR-053 Q4 要求「与写库侧同源」。**这一层要新写**，是转换器那条活的主体 |
| G12 | **无名元素的显示名** | `element_name::display_name` 已实现（`LCYLINDER 1 of GMSET /XXX` 那套） | 目录几何取名可直接用 |
| G13 | **多 extent** | `PageReader` 自动发现同名多 extent 文件（`ExtentNaming`），比旧栈「有字段但从不用」强 | 无需动作；但本语料 0 个多 extent 库，**能力未被压测** |

---

## 4. 建议的先后

1. **先解 G2**（30 行，改 e3d-io `src/engine.rs`）—— 它挡着所有人，且最便宜。在它落地前，
   DirectStore 用 §G2 那段 `DbView` 绕行链，不要先绑 `ReadOnlyEngine`。
2. **并行做 G1**（DirectStore 侧，不改 e3d-io）—— 多库句柄池 + 未注册 dbnum fail loud。
3. **G4 先做对拍再动手** —— DB 模式表达式字符串 vs e3d-io 渲染字符串逐条并排，把方言差异
   归成有限规则；这一步没做完就写映射，一定是猜。
4. **G3 写进转换器的约束**（一句话的事，但漏了就是几何错位），并把双跑对拍改成比**序列**。
5. G5/G6/G7 不排期。G8 等生成链的反查清单出来再定。

---

## 5. 复现

探针：`old/vendor/e3d-io/examples/genmodel_gap_probe.rs`（只读，不连库，不改任何生产代码）。
固定件：`E:\reverse\e3d\shadow_e3d31_aps_all\{attlib,desvir,dicvir,catvir}.dat`；
语料：`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000`（453 个文件）。

```powershell
cd D:\work\plant-code\old\vendor\e3d-io
cargo build --release --example genmodel_gap_probe

cargo run --release --quiet --example genmodel_gap_probe -- session
cargo run --release --quiet --example genmodel_gap_probe -- crossdb
cargo run --release --quiet --example genmodel_gap_probe -- children
cargo run --release --quiet --example genmodel_gap_probe -- deep
cargo run --release --quiet --example genmodel_gap_probe -- deep <db> 16192/0 FTUB
cargo run --release --quiet --example genmodel_gap_probe -- perf
cargo run --release --quiet --example genmodel_gap_probe -- uda
cargo run --release --quiet --example genmodel_gap_probe -- expr <catalogue_db> 3000
```

不带路径参数时默认 `ams8000_0001`（Design）/ `ams5052_0001`（Catalogue，需显式传）/
`ams5100_0001`（Dictionary）。

> **构建提醒**：`cargo build --release --tests` 当前会失败在 `tests/index_diff_real.rs:551`
> （`Census::small_window` 不存在，他人在飞的未提交改动），与本探针无关。
> 只编探针用 `--example genmodel_gap_probe`。
