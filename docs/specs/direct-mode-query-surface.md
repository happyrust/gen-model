# 规格：direct 模式生成期查询面

> 决策见 `docs/adr/ADR-053-direct-mode-generation-reads.md`（已接受）；计划见
> `docs/plans/direct-mode-model-generation.md` P1。本文件只回答「要什么、怎样算成功」，
> 不写实现。术语以 `CONTEXT.md` 为准。

盘点日期：2026-08-30。盘点范围：`src/fast_model/**`（生成链）。
盘点方式：对 `src/fast_model/**` 全量枚举 `aios_core::<符号>` 的出现，再逐个回读源码判定，
**不是**照抄 ADR-053 正文那组估数。

## 0. 本次盘点推翻了什么

ADR-053「生成期查询面」节列了 12 个收口函数 + 「表达式求值 3 处」。逐条核实的结论：

- **那 12 个函数的计数基本准确**，只有 `get_children_pes` 例外：ADR 记 3 处，实际**活的只有 2 处**，
  第 3 处是 `src/fast_model/cata_model.rs:1394` 的一行注释掉的调用。
- **但那 12 个不是全部。** 生成链还调用了 **13 个 ADR 从未列出的 aios_core 读函数**，
  其中 7 个读的是源模型数据、必须路由到 direct。这正是 ADR-053 风险 R3（查询面盘点遗漏）
  说的那件事，它已经发生了。
- 生成链里还有 **3 处直连 `aios_core::SUL_DB`**，完全绕过收口函数。收口函数加多少路由都管不到它们。
- 「表达式求值 3 处」数的是 **3 个 `use` 语句**，不是 3 个调用点。

**盘点面本身也是有边界的**：本文件只盘 `src/fast_model/**`。生成链若经由
`src/data_interface/**` 或 `src/api/**` 间接触发查询，那些调用点不在本表内 —— 它们由
「fail loud」兜底：进了 direct 上下文而走到未覆盖的查询，报错，不回落。

## 1. 判定口径

每个 aios_core 读函数落进三档之一：

| 档 | 含义 | 处置 |
|---|---|---|
| **D（direct 必须覆盖）** | 读的是**源模型数据**（pe / ATT 行 / owner-children 图），DB 模式下由解析期写入 | provider trait 必须有对应方法；direct 上下文内路由过去 |
| **P（产物侧，不进 direct）** | 读的是**生成产物**（inst / geo / 空间索引），不是源模型 | direct 上下文内照旧走 Surreal —— 产物本来就只在库里 |
| **C（控制面 / 非读）** | 项目结构、缓存清理、测试脚手架 | 不路由 |

判据是「这份数据在文件里有没有」：源模型数据 e3d-io 读得到，产物读不到。

## 2. D 档：direct 必须覆盖的收口函数

### 2.1 ADR-053 已列出的（核实后）

| 收口函数 | 处数 | 调用点（file:line） | 需要什么数据 |
|---|---:|---|---|
| `get_named_attmap` | 16 | `gen_model.rs:829,1060`；`prim_model.rs:102`；`cata_model.rs:715,991,1063,1266,1400,1425,1518,1813,1818,2087`；`resolve.rs:15,166`；`loop_model.rs:57` | 单元素整行命名属性集 |
| `get_world_transform` | 9 | `cal_model/equip_model.rs:32`；`prim_model.rs:89`；`cata_model.rs:1056,1075,1261,1406,1500,2098`；`loop_model.rs:61` | owner 祖先链 POS/ORI 折叠 |
| `query_single_by_paths` | 5 | `cata_model.rs:961,974,1555,1642`；`resolve.rs:30` | 引用路径 1–n 跳走查后取字段 |
| `query_multi_deep_versioned_children_filter_inst` | 5 | `gen_model.rs:818,846,954,983`；`coverage_audit.rs:74` | 深层 children + noun 过滤 **+ 产物 inst 反查**（见 §2.3 警告） |
| `query_group_by_cata_hash` | 4 | `gen_model.rs:581,637,855,882` | 按 cata_hash 分组（**含产物侧复用判定**，见 §2.3） |
| `get_cat_refno` | 4 | `cata_model.rs:945,2110`；`resolve.rs:90,177` | CATR/SPRE/PRTREF 链 1–3 跳 |
| `get_children_named_attmaps` | 4 | `query.rs:10,15`；`prim_model.rs:211`；`resolve.rs:44` | 直接子元素的属性集 |
| `get_type_name` | 3 | `prim_model.rs:144`；`cata_model.rs:1453,1827` | 单元素 noun |
| `query_filter_children` | 3 | `prim_model.rs:289`；`loop_model.rs:75,81` | 直接子元素 + noun 过滤 |
| `get_children_pes` | **2** | `gen_model.rs:572,894` | 直接子元素的 pe 行 |
| `query_filter_deep_children_atts` | 2 | `prim_model.rs:165,182` | 深层 children + noun 过滤，取属性 |
| `get_or_create_cata_context` | 2 | `resolve.rs:95,193` | 目录上下文（内部再打上面几个） |

`get_children_pes` 的第 3 处（ADR 记的）是 `cata_model.rs:1394` 的注释行，不计。

### 2.2 ADR-053 遗漏的（本次新增）

| 收口函数 | 处数 | 调用点（file:line） | 需要什么数据 |
|---|---:|---|---|
| `get_children_refnos` | 1 | `prim_model.rs:137` | 直接子元素 refno 列表 |
| `query_filter_children_atts` | 1 | `prim_model.rs:154` | 直接子元素 + noun 过滤，取属性 |
| `get_ancestor_types` | 1 | `query.rs:40` | 祖先链 noun 列表 |
| `query_filter_ancestors` | 1 | `cata_model.rs:1816` | 祖先链 + noun 过滤 |
| `query_multi_deep_children_filter_spre` | 1 | `gen_model.rs:879` | 深层 children + SPRE 引用过滤 |
| `query_multi_filter_deep_children` | 1 | `loop_model.rs:86` | 多根深层 children + noun 过滤 |
| `fetch_loops_and_height` | 1 | `loop_model.rs:113` | LOOP 顶点串 + 高度（源属性派生） |

七个都读源模型数据，七个 provider 都得实现。少任何一个，生成链在 direct 上下文里就会在
那一点 fail loud 停住 —— 这是设计要的行为，但不是可以交付的状态。

### 2.3 两个混合读（必须拆，不能整体路由）

`query_multi_deep_versioned_children_filter_inst` 与 `query_group_by_cata_hash` 的 SQL 里
**同时**读源模型（children 树 / noun / cata_hash）和产物（`->inst_relate` / `->tubi_relate`
是否已存在、按 geo hash 复用）。

它们不能整体交给 provider：产物只在 Surreal 里。要求是**拆成两半** —— 源模型那半走 direct，
产物那半照旧查 Surreal，再在收口函数里合并。

**这是本规格里最容易做错的一条**：整体路由会让「已生成过的元素」判定失效，
表现为重复生成或漏生成，而且对拍**照样绿**（两模式产物 hash 一致，只是数量不同）。

## 3. P 档：产物侧，direct 上下文内照旧查 Surreal

| 函数 | 调用点 | 为什么不进 direct |
|---|---|---|
| `query_refnos_by_geo_hash` | `occ_generate.rs:704,737` | 按已生成 geo 的 hash 反查受影响 refno —— 产物 |
| `query_refnos_has_pos_neg_map` | `cata_model.rs:1094` | 正负体映射，产物侧记账 |
| `query_arrive_leave_points_by_cata_hash` | `cata_model.rs:1491` | 按 cata_hash 取已存在的进出点 —— 产物 |
| `query_insts` | `room_fixture.rs:374` | inst 表 —— 产物（且是房间夹具） |
| `get_inst_relate_keys` | `aabb_tree.rs:72` | inst 关系键 —— 产物 |

这五个在 direct 上下文里**不报错**，正常查 Surreal。规格要求：路由层必须能表达
「这个函数有意不走 direct」，而不是靠「忘了加路由」达到同样效果 —— 两者代码长得一样，
但一个是决定、一个是遗漏。**要有显式的 P 档标注，让遗漏可被审计出来。**

## 4. C 档：不是读

| 符号 | 调用点 | 说明 |
|---|---|---|
| `query_mdb_db_nums` | `gen_model.rs:227` | MDB 成员（哪些 dbnum 在本期范围内）—— 项目结构，控制面，ADR-053 Q1 范围外 |
| `clear_all_caches_batch` | `room_live_issue7.rs:548,572,654,666,775,840` | 缓存清理，且全在 live 测试里 |
| `init_surreal` / `init_test_surreal` | 测试脚手架 | — |

## 5. 直连 `SUL_DB`：收口函数管不到的洞

| 位置 | 形态 | 处置要求 |
|---|---|---|
| `occ_generate.rs:1922` | `aios_core::SUL_DB.query(...)` 内联 SQL | 判定读的是源模型还是产物；产物则标 P 档并注明，源模型则必须收口 |
| `pdms_inst.rs:4` + `:648` | `use aios_core::SUL_DB`；`active_staging_reads().map(\|_\| SUL_DB.clone())` | 已有 staging 分流意识，direct 下的语义要一并定 |
| `room_fixture.rs:27` | `use aios_core::SUL_DB` | 房间夹具，Q1 范围外，标注即可 |

ADR-053 R3 说的「编译期 deny 直连 SUL_DB」就是冲这三处。**本规格要求**：`src/fast_model/**`
内不得新增直连 `SUL_DB`；存量三处逐个定性后，要么收口、要么带理由标注豁免。
没有这道闸，第 2 节的清单每加一个新调用点就可能被绕过一次。

## 6. 表达式求值

ADR-053 记「表达式求值(3)」，实际那是 3 个 `use` 语句：

- `resolve.rs:2` — `aios_core::expression::query_cata::{query_axis_params, resolve_cata_comp}`
- `resolve.rs:3` — `aios_core::expression::resolve::{SCOM_INFO_MAP, resolve_axis_param}`
- `query.rs:1` — `aios_core::expression::query_cata::query_gm_param`

真正的调用点数与这四个符号各自内部是否再打库，**本次未核实**，列为待办。
`SCOM_INFO_MAP` 是全局映射，需单独判定它的填充时机是否依赖 Surreal。

## 6.5 provider 契约语义（三条硬约束）

来源：opus-5-20 的 t-357 真库侦察（探针 `e3d-io/examples/genmodel_gap_probe.rs`，release，
ams8000_0001 / ams5052_0001 实测）。这三条不是实现建议，是**等价性的前提**——
违反任何一条，direct 与 DB 两模式读出的东西就不是同一个东西了。

### 6.5.1 children 族必须保留记录里的成员原序

`get_children_pes` / `get_children_refnos` / `get_children_named_attmaps` /
`query_filter_children*` 返回的顺序**是语义的一部分**：BRAN 的成员序就是管路走向。

实测（ams8000_0001，2332 个带成员表的元素）：成员**集合**与「按 owner 反向分组」100% 一致
（未索引成员 0、归错人 0、重复 0），**但有 6 个元素的成员顺序不等于 refno 序** ——
例 `24384/24775` 的成员是 `[26195, 24776, 24780…]`。

**要求**：provider 一律用记录自带的成员表原序（e3d-io 侧即 `ParsedElement.members`）。
**不得排序，不得从索引反向重建 children。** 集合一致会让对拍绿，顺序错了模型才错，
而且错得不显眼 —— 这是本规格里第二个「对拍假绿」入口（第一个见 §2.3）。

### 6.5.2 `get_cat_refno` 的引用链 82% 跨库

实测 ams8000_0001 的 6605 个活键按描述符取命名引用属性：指向本库 1415 条、
**指向其他库 6461 条**（SPRE 2659 / LSTU 2579 / PSPE 607 / HSTU 531 / CATR 34 / MATR 28 /
ISPE 23），目标集中在 db 5052、7328、6890。

`RefNo` 自带 `dbno()`，但读取器侧**没有 dbnum → 文件路径的注册表**。跨库定位归 DirectStore
（`cata_closure::CataDbLocator` 复用），不归收口函数，也不归 provider trait 的签名 ——
但 provider 的**实现**必须能跨库，否则 §2.1 里 `get_cat_refno` 那 4 处会在 direct 下大面积
fail loud。

对照：**owner 链不跨库**（实测 6605/6605 本库内），所以 `get_world_transform` 的祖先上溯
单库句柄就够。

### 6.5.3 表达式是「渲染成字符串」，两边**是同一种方言**

> **2026-08-30 更正。** 本节原来写的是「方言不同，`ATTRIB PARA[10 ]` 与 `ATTRIB RPRO G`
> 现有求值器没见过」，来源是 t-357 的判断，而那个判断**来自读测试用例文件、不是读实现**。
> t-403 做了逐对对拍之后推翻：**那两种写法本来就是写库侧自己的输出格式**
> （`old-parse-pdms-db/src/parse_explict_tools.rs:334/361/364`），`eval_str_to_f64`
> 对它们各有专门分支（`resolve.rs:241` 删 `ATTRIB`、`:315` 的参数正则吃带空格的方括号、
> `:319-322` 把 `RPRO G` 变成 context 键 `RPRO_G`）。全文见
> `docs/specs/direct-mode-expression-dialect.md`。

读取器侧渲染出的确实是 E3D 显示文本而不是数，但它与写库侧存的串是同一种方言。
证据是**逐对对拍**（不是看例子）：e3d-io 全量扫 5052 + 5053 + 8000（729 468 个元素、
1 039 467 个属性值、0 提取失败）对上 8009 库里 10 个目录 noun 的 36 617 行，
按 refno + 属性名 join 得 **50 307 对**：

| 分类 | 对数 | 占比 |
|---|---:|---:|
| 逐字节相同 | 18 395 | 36.6% |
| 只差首尾空白 | 16 455 | 32.7% |
| 只差括号 / 空格 | 14 956 | 29.7% |
| 规则 A：`AXIS ` 前缀 | 323 | 0.6% |
| 规则 B：`P(1000+n)` ↔ `-P n` | 102 | 0.2% |
| 规则 C：`DD#n` ↔ `n/10` | 67 | 0.1% |
| 系数精度（e3d-io 更准） | 7 | 0.0% |
| **解释不了** | **2** | **0.0%**（写库侧存了空串） |

**结论：不需要扩求值器，三条映射规则约 60 行。** 归属仍是转换器（P1），不归本规格的路由层。
§6 那三个 `use` 点因此不再是风险点。

规则 C 顺带暴露了 e3d-io 的一个渲染缺陷（`catalogue_expr.rs:76-79` 的
`DesignDimension` 兜底分支把十分之一制常量当成了设计尺寸编号），修好之后该规则作废。

## 6.9 路由已落地在哪几行（2026-08-30）

交付物 ②③ 已实现在 `../vendor/old-aios-core`：

| 件 | 位置 |
|---|---|
| provider trait + task-local 上下文 + 与 staging 互斥 | `src/rs_surreal/direct.rs`（新增） |
| 模块导出 | `src/rs_surreal/mod.rs` |
| 收口函数入口的 direct 分支 | 见下表 |

**已接线的 D 档收口函数（21 处）**，每处形态都是入口 `if let Some(ctx) =
super::direct::active_direct_reads()`，**排在 staging 分支之前**：

| 文件 | 函数 | 形态 |
|---|---|---|
| `query.rs` | `get_named_attmap` / `get_type_name` / `get_cat_refno` / `get_children_pes` / `get_children_refnos` / `get_children_named_attmaps` / `get_ancestor_types` / `get_ancestor_attmaps` / `query_ancestor_refnos` / `query_filter_children` / `query_filter_children_atts` / `query_single_by_paths` | 叶子读，直接转 provider |
| `query.rs` | `query_group_by_cata_hash` | **拆两半**：源模型走 `cata_hash_of` + `get_children_pes`，产物（`inst_info` 是否存在、ptset）留在 `query_group_by_cata_hash_direct` 里查库 |
| `graph.rs` | `query_deep_children_refnos` / `query_filter_deep_children` | 叶子读 |
| `graph.rs` | `query_filter_deep_children_atts` | **复合**：`query_filter_deep_children` + `get_named_attmap`，就地组合，不加 provider 方法 |
| `graph.rs` | `query_deep_children_refnos_filter_spre` / `query_multi_deep_versioned_children_filter_inst` | **拆两半**：源模型走 provider，产物半边统一走新的 `retain_ungenerated`（`inst_relate` / `tubi_relate` 为空），**direct 上下文里也查 Surreal，且是有意的** |
| `geom.rs` | `fetch_loops_and_height` | 叶子读 |
| `spatial.rs` | `get_spline_pts` | 叶子读 |
| `spatial.rs` | `get_world_transform` | **复合**：只绕开进程缓存，折叠算法不进 provider（原料三个来源各自已路由） |

三处结构性决定，都在代码注释里写明了理由：

1. **fail loud 不靠接线人记得写。** `DirectReadProvider` 每个方法的默认实现就是报错
   （`direct.rs` 的 `unsupported` / `unsupported_batch`），所以「provider 没实现」与
   「报错」是同一件事。忘了加路由才会回落 Surreal —— 那是零回归要的行为，
   由下面第 3 条钉住。
2. **direct 优先于 staging。** 两个上下文都在场时谁赢，本来取决于两个 `if let`
   恰好谁写在前面。现在定成 direct 优先，并由源码顺序断言钉住。
   `with_direct_reads` 另有一道拒绝（staging 在场时不允许进入，ADR-053 R6）。
3. **源码顺序断言**：`direct.rs` 的 `every_routed_collector_asks_the_provider_first`
   逐个读 `src/rs_surreal/{query,graph,geom,spatial}.rs`，要求上表 21 个签名都存在、
   都有 `active_direct_reads()`、且都排在 `active_staging_reads()` 之前。
   **上表加一行，那个测试就要加一行。** 忘了接线编译不报错、运行也不报错，
   只是静默回到 SurrealDB —— 这道断言就是冲它去的。

`cargo test --lib rs_surreal::direct` 4 条全绿（2026-08-30）。

**§3 的 P 档尚未加显式标注**（成功判据 6 未满足）：那五个函数现在是「没写路由」，
与「有意不走 direct」在代码上长得一样。建议后续加一个 `#[doc]` 级别的标记或一条
把 P 档也列进去的源码顺序断言，让遗漏可被审计出来。

## 7. 成功判据

1. **清单可复核**：第 2–6 节每一行都能用 `src/fast_model/**` 的一次搜索复现；
   新增调用点必须同步本表。
2. **fail loud**：direct 上下文内，D 档里未被 provider 覆盖的查询**显式报错**，
   绝不静默回落 Surreal。回落会让 Q5 对拍假绿。
3. **零回归**：不进 direct 上下文时，第 2–5 节所有函数的行为逐字节不变
   （分流点是入口 `if let Some(..)`，不改原路径）。
4. **与 staging 互斥**（R6）：direct 上下文与 staging 读上下文不得同时在场，入口断言。
5. **两态可编译**：aios_core 改动在 `Toggle-LocalDeps -On`（本地 patch）与 `-Off`（升 rev 后）
   两态都 `cargo check` 通过。
6. **P 档显式**：第 3 节五个函数在代码里有可审计的「有意不走 direct」标注，
   而不是靠没写路由。

## 8. 未决

- ~~§2.3 两个混合读的拆分边界~~ → **已拆并落地**（见 §6.9）：源模型半边进 provider，
  产物半边留在收口函数里走 Surreal（`query_group_by_cata_hash_direct` 与
  `retain_ungenerated`）。仍需 opus-5-22 在 DirectStore 侧复核产物半边的查询形态是否够用。
- §5 `occ_generate.rs:1922` 那条内联 SQL 的定性。
- ~~§6 表达式求值~~ → 方言问题已由 `docs/specs/direct-mode-expression-dialect.md` 结清
  （同一种方言，三条映射规则）。**仍未决的是那四个符号自身是否再打库**，
  以及 `SCOM_INFO_MAP` 的填充时机是否依赖 Surreal。
- **§3 的 P 档还没有显式标注**（成功判据 6）：五个函数目前靠「没写路由」达成
  「不走 direct」，与遗漏在代码上不可区分。
- 生成链经 `src/data_interface/**` / `src/api/**` 间接触发的查询未盘（靠 fail loud 兜底）。
- `with_staging_reads` 侧没有反向拒绝：它的签名返回 `F::Output` 而不是 `Result`，
  加不进错误。互斥现在由两件事保证 —— `with_direct_reads` 拒绝在 staging 内进入
  （ADR-053 R6），以及「direct 分支一律排在 staging 之前」由源码顺序断言钉住。
  要做成双向拒绝得改 `with_staging_reads` 的签名，牵动全部既有调用点，未做。
