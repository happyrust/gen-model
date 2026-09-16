# 属性变化分类对齐 core 交互路（DCHC × `UPGRPH`）：开发计划

> 来源：`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md`（ida-bridge 取证：core 的三条更新通路、
> DCHC 四档在 `change/UPGRPH` 上的执行语义、我方三档的差距表 §6.2 与计划草案 §7）。本文把 §7 的 P0–P4 写成可开工的切片。
> 状态：**已批**（2026-09-15，Plannotator 过目；§8 四条 Q 全按推荐项）。**S1 ✅ S2 ✅ S3 ✅ S5 ✅ S6 ✅ S7 ✅ S8 ✅ 已落地**（2026-09-15，S1–S5 会话 fable-5-1-21
> 接手 fable-5-1-13 完成，S6 代码会话 fable-5-1-40 接手 fable-5-1-21 完成，S6 全语料门 + 契约 2 改口 + changelog 会话 fable-5-1-44 于 2026-09-16
> 接手 fable-5-1-40 完成，S8 探针 + 结论会话 fable-5-1-49 于 2026-09-16 接手 fable-5-1-44 完成；S7 探针 + 口径纠错附录由协同组 dchc 的 opus-5-74
> 于 2026-09-16 完成、结论与 ADR 由会话 opus-5-10 于同日收口；见 `changelog.md` 当日条目），**S4 ✅ 两个半程都已落地**
> （离线半程同会话 fable-5-1-49：对拍门 `probe_rigid_exclusion_windows` 建成、语料里合格锚点两态全等、负体 / 规则 M / 路由容器三条排除坐实有内容；
> P23 / 开孔 / CWBRAN 等七条排除语料里没有自然窗；E3D 造窗半程 2026-09-16 由 fable-5-1-7 开、fable-5-1-20 收、opus-5-23 补齐 P23 那一窗：NBOX 在 BOX 下造出两窗、
> 排除坐实有内容；PLOPEN 在 PANE 下在本装机 / db8000 **造不出**，记为结构性不可造；P23 绕道 APS 自然 `STRFLT` 造出**窗 C**（用户已批、事后位精确复原），
> 两态相等且有判别力 = **这形状上排除保守**——见 §4 ★0916 S4 E3D 结论）。
> 代码事实对到 `gen-model-refactor@1bd2cab4c` + 当日工作树、`vendor/e3d-model@7da42b5` + 当日工作树。
> 会话 fable-5-1-13 → fable-5-1-21 → fable-5-1-40 → fable-5-1-44 → fable-5-1-49 →（S7 支线 fable-5-1-60 / opus-5-74）→ opus-5-10 →（S4 E3D 支线 fable-5-1-7 → fable-5-1-20 → opus-5-23）。
> S8 之后多两条待拍板（§8 Q5 / Q6）；S4 的对拍给了 Q5 第二票；S7 的结论收口成 **ADR-078**（§8 Q4）。

## 0. 一句话

**「少算」的两档（Skip / Transform）都改成两票制、并把 core 类别 3 的例外补齐；「多算」的报表噪音顺手消掉。**
核心变化只有两处判据：刚体前移的 `rigid_anchor` 多两族排除（G1 / G2），元数据门从 5 个名字改成「DCHC = 0 ∧ 不在生成读取集合」。
不动根级执行模型（ADR-056 D3）、不动任务边界（ADR-076）、不动 CATA 闭包（在途计划）。

## 1. 现状（代码事实，2026-09-15）

### 1.1 三档今天的落点

| 档 | 判据 | 执行物 | 开关 |
|---|---|---|---|
| Skip | `model_impact::attribute_is_metadata_only`（效果表 `DataOnly` ∧ 字典 DCHC = 0，恰好 `NAME DESC PURP FUNC LCHKDA`）对**全部**变更属性成立 + `window_root_plan::change_is_metadata_only` 的四条结构条件（没换类型 / 属主 / 成员表、不 `opaque`） | e3d-model `plan_update_with(.., metadata_only)` 记 `no_model`，不上卷不级联 | `model_skip_metadata_only_changes` 默认 **开** |
| Transform | `ElementDiff::is_placement_only`（只有 `POS`/`ORI` 变）→ `window_root_plan::rigid_anchor`：`Δ = W_T·W_S⁻¹`；排除 = 候选是桥架件 ∨ T 端严格祖先含 `CWBRAN` / 点环容器（`graphics_ignored_below`，gb==1）/ 路由容器（`derives_implied_tubes`）；再过 ADR-066 D1 的五条（未安定 / 退化 / 级联扩展进来 / 子树有非锚点候选 → `regen` 吞掉） | `rigid_motion.rs`：投影记录左乘 Δ，根级 CAS 发布 | `model_transform_cheap_path` 默认 **关**（对拍门未过） |
| Regen | 其余（`Unknown` / UDA / `opaque` / 结构变化 / 一切 DCHC ≠ 0） | 根级整根重算 | — |

### 1.2 core 交互路的口径（取证文档 §3–§4，本计划的 oracle）

- 类别 0（3941 个属性）→ 不动；类别 3（`POS ORI BFORI`）→ 只改段矩阵（`css/STRTRA` / `css/SEGTRA` / `change/TRASCN`）；类别 4（315 个）→ 重建（整件 `REPELE` / 单图元 `SEGREP` / 目录 `REPALL`）；
  类别 1 / 2（11 个头尾连通属性）→ 自身只重刷颜色，几何靠 `EVALAT` 转成类别 4 落到前一元件 / 自身 / 所属分支。
- 类别 3 的四条例外：**P23**（`DRAW CLNTIL STRFLT INFITT PLTFRM ELFITT STRLNG HVACFI RAIL DOOR WINDOW FPFITT LINDIM FLRCOV MLABEL FIXTUR HRPOST HRTERM HRGATE HRPANE KICKPL CTSUPP FURNIT`）整件重建；
  管件 / 支吊架元件两侧隐含管段 `SEGREP` + 规则 P 分支整件；`EVALAT` 规则 M / J / P / R / S（PANE·GENSEC·SCTN·BPANEL 成员、孔洞 `HOLC`、元件、`PLOPEN`/`DP*`、点类）转成类别 4 落到属主 / 结构件 / 顶层件；刷新观察者钩子。

### 1.3 用户问的 `HPOS` / `TPOS` 一族：**已覆盖，且两边口径一致（都是重算，不是位姿）**

| 属性 | DCHC | 属主 noun（运行库） | core 交互路 | 我方 |
|---|---:|---|---|---|
| `HPOS HDIR HBOR LHEA` | 1 | `BRAN HANG LUG SUPC TRUNNI` | 规则 H：前一元件记类别 4；分支自身 `(BRAN, 1)` 只 `COLRES`；分支是顶层件时 `UPGRPH` 对 `(BRAN, 4)` 走 `REPELE` 整件（含负 ref 那段头管） | `DEPENDENCY_CASCADE` → `Regen`；`LHEA` 落 `Unknown` → `Regen` |
| `HSTU` | 1 | `BRAN HANG LUG SCBRAN` | 同上 | `DEPENDENCY_CASCADE` → `Regen` |
| `TPOS TDIR TBOR LTAI` | 2 | `BRAN HANG LUG SUPC TRUNNI` | 规则 I：自身记类别 4 → `REPELE` 整件 | `DEPENDENCY_CASCADE` → `Regen`；`LTAI` 落 `Unknown` → `Regen` |
| `HZAXI TZAXI` | 1 | `BRAN` | 同类别 1 | `Unknown` → `Regen` |

三条旁证：(1) 它们**不是**类别 3，core 自己也不把分支头尾当位姿；(2) 09-04 清单 §2.9 已坐实 `HPOS`/`TPOS` 是**存储属性**（无伪属性插件），文件差分看得见；
(3) e3d-model `route.rs` 读 `HPOS TPOS HDIR TDIR HBOR TBOR HSTU`（读取集合里，`generation_read_set` 会把它们钉成几何输入），
`is_placement_only` 只认 `POS`/`ORI`，所以改了头尾属性的分支**永远进不了** `transform` 桶；分支又是路由容器，名下任何锚点也被排除。
`descases/VDESPT` 层另有几条 noun 特判（`PLOO.HEIG` 转到属主 PANE 评估、`SJOI.JFRE`、`COCO.CTYP`、`DATA.PPRO/DPRO`），都在「重算」方向，不需要对应物。
本计划对这一族只做一件事：把 `LHEA LTAI HZAXI TZAXI` 显式登记（S1），消掉 `Unknown` 噪音。

### 1.4 数字

- 字典 4270 条：`0: 3941 / 1: 7 / 2: 4 / 3: 3 / 4: 315`；运行库 1878-noun 表出现 4478 个属性名：`0: 3657 / 无: 514 / 4: 293 / 3: 3 / 1: 7 / 2: 4`。
- 我方五张表：DCHC=4 命中 102，未登记 213（运行库出现 191）；`DEPENDENCY_CASCADE` 里 13 个、`DIRECT_GEOMETRY` 里 6 个是 DCHC = 0 却判影响模型（正确：它们是我们的几何输入）。
- e3d-model `src` 正则扫得 131 个运行库属性读点：`4 ×108 / 3 ×2 / 1·2 ×7 / 0 ×6（DIR FILL GTYP PKEY UKEY VXREF）/ 字典无 ×8`。

## 2. 目标与非目标

**目标**

1. `model_transform_cheap_path` 可以开：`rigid_anchor` 的排除与 core 类别 3 的例外逐条对得上（G1 P23、G2 被属主消费的几何 / 规则 M），对拍门过。
2. 元数据门从 5 个名字扩到「DCHC = 0 ∧ ∉ 生成读取集合」，读取集合是受护栏的产物，预览与执行同一谓词。
3. 回执 / 报表里不再把 DCHC=4 与 DCHC=1/2 的已知属性显示成 `Unknown`。
4. 三处既有文档改正（R5 注、0906 `IDCHNG` 行、0831 `changesBetweenSessions` 句）；`PLCF` 随 DCHC 一起导出。

**非目标**

- 不做单元级 / 图元级发布（core 的 `SEGREP` 粒度）——只量不做（S7），需要时另立 ADR。
- 不动 `is_placement_only` 的定义（仍只认 `POS`/`ORI`；`BFORI` 只挂 `BPFITT`，我们不生成，只记文档）。
- 不把 `CACHID` 当判据；只做一致性普查（S8，探针）。★0916 S8 已普查：它是图形缓存 `+1` 计数（不限顶层件——改谁就盖谁整棵子树里每个带这个字的元素，S4 窗 C 现场纠正）、按应用分写、92.8% 的跳动是整库扫写形状；
  单向「有 DCHC≠0 变 ⇒ 跳」在有值元素上 98.9% 成立、反向 2.7%——只能作单向离线校验，不接线；两处派生的待拍板见 §8 Q5 / Q6。
- 不动 ADR-066 D2–D7（执行物、落款、回退、开关形状）；不动 e3d-model `increment.rs` 的上卷规则。
- 不加配置键：P23 名单、读取集合都是常量 / 生成产物，不进 `DbOption`。

## 3. 设计决策

### D1　刚体锚点的资格 = 「自己是独立单元或纯容器，且没有任何一处网格把它的摆放当输入」

`rigid_anchor` 在现有三条排除之外再加两条，全在 T 端判：

- **D1-a P23**：锚点 noun ∈ `PLACEMENT_DERIVED_NOUNS`（常量入 e3d-model `category.rs`，注释带 `0x1022EAEE`）→ `Other`。
- **D1-b 被消费的几何**：锚点 `!is_model_unit(el)` 且 `classify(noun)` 不是纯容器（`List` / `Composite`）→ `Other`——覆盖负体 `N*`、`PLOPEN`/`WLOPEN`/`BPOPEN`、
  `FIXING`/`TMPL` 开孔、剖面 / 点成员（后者今天已由 gb==1 排除接住，这里是同一语义的通式）。
  另按 `EVALAT` 规则 M：锚点不是独立单元且 T 端祖先含 `PANE`/`GENSEC`/`SCTN`/`BPANEL` → `Other`（型钢上的附件挪位 = 型钢重建）。
- 判据位置不变：`WindowCandidate.kind` 的 `Anchor{rigid: Err(reason)}`，reason 文本区分五种排除，回执 `transform_fallback_reason` 原样带出。
- 效果：G2 那个「属主网格上的孔停在旧位置」在开关开着时也不可达；ADR-066 D1 规则 3 补两条，落修订注。

★0915 S3 落地时按 §7 的提醒先核了 `category`，两处按核出来的事实改：
(1) **`FIXING` 今天是模型单元**（2026-09-14 起走目录构件 GMRE 求值，`is_model_unit` 真），所以「不是独立单元 ∧ 属主消费」那条接不住它；
规则 M 改成对**任何**锚点看严格祖先——FIXING 在 SCTN 下照样回 regen（core 那一票是「重建」），结构件自己挪位不在此列；
(2) `PLOPEN`/`WLOPEN`/`BPOPEN` 在 3.1 字典里带 `IPFCOM`、被 `classify` 判成 `Composite(Structural)` 非单元——D1-b 原文把 `Composite` 与 `List` 一起
列为「纯容器」是错的；改成「非单元 ∧ 直接属主 `consumes_members()`」（正体 / 目录件 / 复合件 / `TMPL`），`List` 属主之下的非单元（SUBE、ELCONN）照旧放行。
另外补了一条原文没写的：`PLOO` 的 gb 是 0，`PANE → PLOO → PAVE` 那条老的「gb==1 容器之下」接不住，剖面 / 路径数据（`ProfileData`）改按锚点自己的 noun 排除。

### D2　「不影响模型」= DCHC = 0 ∧ ∉ 生成读取集合，两票缺一不放行

- 读取集合 `generation_read_set.json`：e3d-model `src` 全部属性字面量读点（含 `INDIRECT_READ_SITES` 同款的间接读点登记）∪ gen-model 模型面读点
  （`cata_model` / 房间 / `structural_connection` / `cable_endpoint` / `window_root_plan` 的 `PLACEMENT_ATTRIBUTES`）。生成脚本 `output/gen_read_set.py`，产物随源码入库、编译期嵌入。
- 护栏 `every_attribute_literal_in_the_generator_is_in_the_read_set`（e3d-model `tests/`）与 gen-model 同款各一条：源码里出现、集合里没有 → 红。
- `attribute_is_metadata_only(name)` 改为 `raw_dchc_code(name) == Some(0) && !generation_read_set().contains(name)`；`DATA_ONLY_ATTR_NAMES` 退成文档 / 报表分类，不再是判决输入。
  UDA（`UDA:<id>`）、字典没收的名字、`opaque` 照旧不放行。`DIR FILL GTYP PKEY UKEY VXREF` 由读取集合那一票挡住。
- 四条结构条件不变；`no_model` 里区分 `metadata_only`（老 5 名单）与 `unread_attribute`（新档）两个计数，回执 `totals_line` 跟着多一格。
- 预览与执行同一谓词：`window_root_plan::change_is_metadata_only` 与 `model_impact::classify_operation_effects` 都问 `attribute_is_metadata_only`（0908 纪律，源码钉 `the_skip_verdict_uses_the_same_double_vote_as_the_window_metadata_gate` 继续成立）。

★0915 S5 落地口径（与原文的三处不同）：
(1) **扫描规则**不是「属性字面量读点 + 逐处登记间接读点」，而是更宽的「源码里全部大写字面量 `"[A-Z][A-Z0-9]{1,7}"` ∩（字典 ∪ 运行库 schema）∪ 直接传进
e3d-io 读取口的字面量」，每文件截到第一个 cfg(test) 标记、`src/bin` 除外——间接读点（`get_double(name)`）的名字总来自某处字面量，这一扫就收得到，不必逐处登记；
多收只少跳一次。直接读点无条件算属性，是为了收 Core3D 伪属性 `CPAR` 这类字典没有的名字。生成脚本与两边守卫用**逐字相同**的规则，守卫按嵌入产物逐名相等判绿。
(2) 产物分两个仓：e3d-model 自己嵌 `data/attribute-read-set.json`（133 名 + 44 个非属性字面量，`e3d_model::attribute_read_set()`），gen-model 的
`generation_read_set.json` 记 `e3d_model` 原样一份 + `gen_model` 模块 8 名（`PARA CFRA CREF JOIS JOIE PFRE STAREF ENDREF`）+ 并集 **138** 名（判决用）；守卫钉两仓产物相等。
(3) 多出一处原文没提的读点：**e3d-io 目录求值环境**（`db_param_env.rs` 等三文件）替生成器读设计侧 `ANGL RADI HEIG DESP UNIPAR` 与目录侧 `DATA.PURP/DKEY/NUMB`、
`SELE.QUES/ANSW/MAXA/TANS`。它单列在产物里、`vote: false`：设计侧那几个 DCHC≠0 或已在 e3d-model 一侧，DCHC=0 的七个全在目录元素上读，而元数据门只跑 DESI 窗口。
名单变了守卫会红，逼人重新回答「新名字是在设计元素上读的吗」。老五个元数据名一个都不在并集里，S6「`NAME` 等仍放行」的前提已钉。

★0915 S6 落地口径（与原文的三处不同 / 补充）：
(1) **两票压过手写表**：五张手写表里登记为几何 / 级联输入、却字典 DCHC=0 且生成器不读的名字**恰好 16 个**——`DEPENDENCY_CASCADE` 的
`HREF TREF PSPE DKEY DDPR FSPREF SCREF STYP CURD JFRE PTYP`（11）与 `DIRECT_GEOMETRY` 的 `SIZE MAXA NGMR ORRF PTOF`（5）。原文 §1.4 说「13 + 6 个
DCHC=0 却判影响模型的是我们的几何输入（正确）」，S5 的扫描把这句话推翻了一半：真读的只有 `GTYP PKEY VXREF` 三个（都在读取集合里、照旧挡住），
其余 16 个一处读点都没有（`HREF`/`TREF` 的几何读的是 `HPOS HDIR HBOR …` 存储属性；`PSPE` 是管道级默认、元件各有 `SPRE`；`DKEY`/`MAXA` 只在目录
`DATA`/`SELE` 上读；`NGMR` 负体走 `GMRE`）。判决以两票为准：这 16 个名字只改它们时 **Skip**，`classify_attribute_effect` 的表项不动（建边资格、
报表分类照旧），守卫 `the_two_votes_override_the_hand_tables_for_exactly_sixteen_names` 逐个点名——多出一个是读取集合漏了、少一个是有人开始读它了。
(2) **预览同步换口径**：`classify_operation_effects` 逐属性**先**问 `attribute_is_metadata_only`，放行的记 `DataOnly` 不排工作（此前只对效果表 DataOnly
的名字问第二票）；`query_service::change_impact` 的 `action` / `affects_model` 也改由两票说，回执多 `dchc` / `generator_reads` 两格解释判决；
`manual_update::merge_net_change_details_with` 不动（它本来就问 `classify_operation_impact`）。
(3) **分档**：`model_impact::metadata_only_tier(names)` 是唯一一份名单 + 分档——全部落在老名单（`attribute_is_curated_metadata` = 效果表 DataOnly ∧
DCHC=0，恰好五个，按构造是新门的子集）记 `MetadataOnly`，否则 `UnreadAttribute`；e3d-model `plan_update_with` 的谓词从 `Fn(&ElementDiff) -> bool` 改成
`-> Option<NoModelTier>`，`IncrementReport` 多 `unread_attribute` 一格、`totals_line` 跟着多一格、`accounts_for` 钉两档之和 ≤ `no_model`。
(4) 规模：字典 3941 个 DCHC=0 的名字里读取集合挡六个，其余 3935 个放行（守卫钉 `3941 − 6`）。UDA / 字典没收的名字（含 `CACHID`）/ `opaque` 照旧不放行。
两态门的三条契约抽成 `assert_metadata_gate_two_state`，单库门与全语料门共用；全语料门 `live_corpus_metadata_only_gate_two_state_over_every_desi_db`
顺带出「DCHC=0 属性变化频次表」（`docs/evidence/2026-09-15-metadata-gate-two-vote-corpus.md`，按政策不入库）。
(5) **契约 2 改口（全语料门第一次跑撞出来的，2026-09-16 会话 fable-5-1-44）**：原文「两态祖先闭包逐字相同」在 ams8000 链上成立只是因为那条链
没有这种窗——aps250514 3→4 两个 CWBRAN 只改 `HCREF`/`TCREF`（DCHC=0、生成器不读，新档放行），关着时 e3d-model 的桥架分支粗级联
（`cascade_cable_tray_branch`：CWBRAN 子树里改了任何东西就整条重建，按 noun 不按属性判）拖进 20 个 CTRAY，闭包多 42 个 refno；开着时候选不上卷
不级联，那片单元连同它们的祖先就不进闭包。这是两票该省的（core 不为 `HCREF` 重画、`cable_tray.rs` 也不读它，托盘网格不可能变），不是漏收：
契约 2 改成「闭包只少不多 ∧ 账本种子两态逐字相同（候选与祖先链照进闭包，候选所在的根一个都不少收）∧ 少掉的 refno 全是少重建的单元或其祖先」，
少了别的就红；回执 / 报告多一格 `closure_dropped`。0908 那句「凭证前移那一层不因此少收」相应改口为「候选所在的根不少收；粗级联扇出的根按两票前移」
（`increment.rs` / `options.rs` / `change_metadata_tier` 文档同步）。

### D3　字典类别整表导入，只消噪音不改行为

`model_impact.rs` 第六张表 `DICTIONARY_CHANGE_CLASS_ATTR_NAMES`：DCHC ∈ {1,2,4} 而五张表未登记的全部名字（213 + `LHEA LTAI HZAXI TZAXI`），效果
`DirectGeometry`（1/2 归 `DependencyCascade`），来源字典、由 `gen_dchc_fixture.py` 生成、守卫钉「与快照差集为空」。行为不变（`Unknown` 与 `DirectGeometry` 都是 `Regen`），
守卫：ams8000 链两态计划逐字相同。

★0915 落地时的一处收紧：DCHC = 4 里 **schema `att_type == ELEMENT` 的 18 个引用名**（`STAREF ENDREF JOIS JOIE CFRA PFRE CRFA HPREF TPREF POSREF …`）也归
`DependencyCascade` 侧，不进 `DirectGeometry`——它们此前经 `classify_attribute_effect_with_meta` 的 A2 升级本来就落 `DependencyCascade`，
写进几何侧会让不带 meta 的 `classify_attribute_effect` 换答案（`change_impact` 报表、`reference_edge_eligible` 的名字半边）。守卫
`dictionary_change_class_table_routes_schema_references_to_the_cascade_side` 钉「带 / 不带 meta 对这 217 个名字同答」。落地形态是嵌入 JSON
`dictionary_change_class_attr_names.json`（两侧各一列表）+ 访问器 `dictionary_change_class_attr_names()`，合进 `attribute_effects()`；
`ATTRIBUTE_EFFECT_TABLES` 仍是五张手写表的 const。

### D4　`PLCF` 随 DCHC 导出，只作报表

`gen_dchc_fixture.py` 同时提炼字段 652066（`PLCF`）→ `dchc_change_classes.json` 旁加 `plcf_flags.json`；`raw_plcf(name)`。不进任何判决——它是 core 隐含管段
放置缓存（`PLCDEL`）的失效旗标，我们没有对应缓存。

### D5　开关顺序：先开 Skip 的新门，再开 Transform

D2 是「少算」但两票都是我们自己能验的（字典 + 读取集合 + 两态门），先开；D1 补齐后 `model_transform_cheap_path` 仍先关，过 ADR-066 验收 1 / 2 再翻。

## 4. 切片

| # | 内容 | 落点 | 验收 |
|---|---|---|---|
| S1 ✅ | **文档改正 + 字典整表导入**（D3 / D4）：R5 加注、0906 `IDCHNG` 行、0831 `changesBetweenSessions` 句、CONTEXT.md「模型影响」词条补 core 对应物；`gen_dchc_fixture.py` 导 `PLCF`；第六张表生成 | `docs/specs/core3d-partial-update-conformance.md`、`docs/evidence/…evalat…`（按政策不入库，从 p3b-verify 快照取回本地副本后改）、`上下文/…800a23b3.md`、`CONTEXT.md`、`output/gen_dchc_fixture.py`、`src/data_interface/model_impact.rs` + `dchc_change_classes.json` / `plcf_flags.json` / `dictionary_change_class_attr_names.json` | `dictionary_change_class_table_covers_every_unregistered_nonzero_attribute`（差集为空）；`attribute_effect_tables_have_no_duplicate_names` 仍绿；ams8000 九个钉死窗两态计划逐字相同；`cargo test --lib model_impact` 全绿 |
| S2 ✅ | **P23 排除**（D1-a） | e3d-model `category.rs`（`PLACEMENT_DERIVED_NOUNS` 常量 + `is_placement_derived(noun)`）；`window_root_plan::rigid_anchor`（自身 noun 的两条排除抽成纯函数 `anchor_noun_exclusion`，桥架件 + P23）；ADR-066 D1 规则 3 修订注 | `placement_derived_nouns_are_exactly_the_twenty_three_from_upgrph`（逐个点名）；`a_placement_derived_anchor_never_qualifies_for_rigid_motion`（HRPOST / DOOR / CTSUPP 各一形状） |
| S3 ✅ | **被消费几何排除 + 规则 M**（D1-b） | `window_root_plan::rigid_anchor`（noun 级判决全部抽成纯函数 `anchor_lineage_exclusion` = `anchor_noun_exclusion` + `anchor_owner_exclusion` + 祖先三条，读 e3d-model `is_model_unit` / `classify` / `consumes_members`）；ADR-066 修订注 | `a_consumed_member_anchor_regenerates_its_owner_unit`（NBOX 在 BOX 下、PLOPEN 在 PANE 下、FIXING 在 SCTN 下各一）；`a_pure_container_anchor_still_qualifies`（ZONE 挪位不受影响）；开关关着时计划逐位相同（`rigid_motion::the_switch_off_yields_no_transform_and_no_rejections` 继续绿；计划里写的 `transform_bucket_is_empty_when_the_switch_is_off` 是它的别名，仓里没有这个名字） |
| S4 ✅ | **对拍样本**：~~ams8000 链上~~ ★0916 沿五个目录全语料找只动了摆放的候选（`probe_rigid_exclusion_windows`，`#[ignore]`），逐个过 `rigid_anchor` 分类、按生成根归组做两态对拍；无自然窗的排除按 ADR-066 验收 1 用 `AIOS_PROJAMS_GEOMETRY_FILE` + `AIOS_TRANSFORM_WINDOW` 造一窗 | ~~e3d-model `tests/increment_real.rs`~~ ★0916 gen-model `window_root_plan::tests::probe_rigid_exclusion_windows`（排除判决 `rigid_anchor` 只在 gen-model 一侧；生成用 e3d-model `generate_subtree`，目录库经 `DirectoryDbResolver` 按需补开）、`docs/evidence/2026-09-16-rigid-exclusion-parity.md`（+ `-ignore-cachid.md`，按政策不入库）；E3D 造窗：`scripts/e3d/s4_db8000_*.mac`（l3_suite `--check-driver` 跑，db8000）、`docs/evidence/2026-09-16-rigid-exclusion-e3d-windows.md` | 每个样本两态（Δ 前移 vs T 端整根重算）`GeometryId` 集合相同、体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵 1e-6 内相等；被排除的锚点若照 Δ 前移必须**不等**（证明排除有内容）。★0916 语料结果：合格锚点 27 个有判别力的根全等（0 红）；负体 / 规则 M / 路由容器三条排除各有干净样本坐实不等；P23、开孔在消费属主下、CWBRAN、点环容器、桥架件、剖面、几何集**零个自然窗**。★0916 E3D 造窗：NBOX 在 BOX 下两窗——609→610 六个不变量全同（通孔平移，探针报「无判别力」，**不是**生成器没扣）、614→615 属主链 BOX 三角 / 亏格 / 体积变，**排除有内容**；PLOPEN 在 PANE 下**造不出**（`NEW PLOPEN` 崩 des.exe）；P23 **造不出只动摆放的窗**（HRPOST 无 POS/ORI；带 POS/ORI 的十个在 db8000 建不出带几何的子树）——见 ★0916 S4 E3D 结论 |
| S5 ✅ | **读取集合产物 + 护栏**（D2 前半） | `output/gen_read_set.py`、`src/data_interface/generation_read_set.json`（嵌入，`model_impact::generation_read_set()`）、e3d-model `data/attribute-read-set.json` + `src/attribute_reads.rs` + `tests/attribute_reads_guard.rs`（推广 `reference_reads_guard.rs`）、gen-model 同款守卫（`model_impact` 三条测试） | 两条护栏绿；集合里 DCHC=0 的成员逐个点名（今天 6 个 + gen-model 侧的 0 个）；`INDIRECT_READ_SITES` 纪律沿用（改成更宽的「全部大写字面量 + 直接读点」扫描，间接读点的名字仍来自某处字面量，无需逐处登记） |
| S6 ✅ | **元数据门改两票**（D2 后半） | `model_impact::attribute_is_metadata_only`（两票）+ `attribute_is_curated_metadata`（老名单，只分档）+ `metadata_only_tier`；`window_root_plan::change_metadata_tier`（`change_is_metadata_only` 退成布尔包装）；`classify_operation_effects` / `query_service::change_impact` 预览同口径；e3d-model `increment.rs`（`NoModelTier`、`IncrementReport::unread_attribute`、`totals_line`、`accounts_for`）；回执 warning 两档分列 | `metadata_only_needs_dchc_zero_and_an_unread_attribute`（真值表：读取集合里的 DCHC=0 属性不放行；`NAME` 等老 5 个仍放行；UDA / 字典无 / `opaque` 不放行；放行数 = 3941 − 6）；`the_metadata_tier_is_curated_only_when_every_name_is_on_the_old_list`；`the_two_votes_override_the_hand_tables_for_exactly_sixteen_names`（D2 ★S6 (1)）；`live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure`（三条契约抽成 `assert_metadata_gate_two_state`）+ 全语料版 `live_corpus_metadata_only_gate_two_state_over_every_desi_db`：新门放行的窗只多不少、闭包不缩、账本守恒；产出「DCHC=0 属性变化频次表」进证据文档 |
| S7 ✅ | **图元级重建先量**：每窗被重算的根里单元记录与上一版逐字相同的占比。~~manifest 逐条比~~ ★0916 实际做法是**两端各整根 `generate_subtree` 一次、逐件按 `GeometryId` 配对、比 `ParityInvariants`**（与 S4 对拍门同一把尺子：noun / 体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵，旋转绝对 1e-6、其余相对 1e-6，不比网格顶点） | ~~`window_root_plan` warning 多一格 `unchanged_unit_ratio`~~ ★0916 **只量不接线**（§6「不做」第一条：本轮不做单元级发布，回执不该先长出一个没人消费的字段）：`window_root_plan::tests::probe_unit_level_rebuild_ratio`（`#[ignore]`）+ `docs/evidence/2026-09-16-unit-level-rebuild-measure.md` 与同名 `-addendum.md`（按政策不入库） | 有数即可；>70% 才立 ADR，否则记结论。★0916：按「真·重算」口径 **86.5% > 70%**，故立 **ADR-078**（见 §8 Q4） |
| S8 ✅ | **`CACHID` 一致性普查**（探针） | ~~e3d-model `tests/increment_real.rs::probe_cachid_consistency`~~ ★0916 落在 gen-model `window_root_plan::tests::probe_cachid_consistency`（`#[ignore]`；DCHC 字典、两票门、`rigid_anchor` 三样都只在 gen-model 一侧，e3d-model 里放不下「DCHC 类别 × 我方判定」两维）；`AIOS_POSREF_DIRS` / `AIOS_CACHID_PROBE_STEPS_PER_DB` / `AIOS_CACHID_PROBE_REPORT` 可改 | 三维交叉表（CACHID 四态 × 除 CACHID 之外的变更类别 × 我方判定）+ 「只 CACHID」成因 + 「忽略 CACHID 再判」位移 + 属性名 × 四态，进 `docs/evidence/2026-09-16-cachid-consistency-corpus.md`（按政策不入库）；结论进取证文档 §4.4 末条；只作结论，不接线 |

依赖：S1 独立；S2 → S3 → S4 顺序；S5 → S6；S7、S8 独立。S2–S4 是开 `model_transform_cheap_path` 的前置；S6 落地后 `model_skip_metadata_only_changes` 语义变宽但开关名不变。
建议顺序 **S1 → S2 → S3 → S5 → S6 → S4 → S8 → S7**（S4 要等有空的 E3D 造窗）。

★0916 S8 结论（453 库 / 2961 窗 / 36162 个真变了的 Modified 候选，全文见取证文档 §4.4 末条）：
(1) `CACHID` 是 `WORD` 型 **`+1` 计数**（13724 次跳动全是 `0→1` 或 `n→n+1`），不是旗标；
(2) 写方按应用分——带它的 26627 个候选里只有 13753 个非 0，写过的只有 `GENSEC HANG PANE BRAN SUBE EQUI SCTN HANDRA TRNCUR STRFLT GENPRI PNOD SUBS FLOOR TMPL STRU TRNPRI`，
船体 / `CABLE` / 栏杆件 / `CWBRAN` / `VOLM` 上没写过；
(3) 语义是**图形缓存版本**（~~顶层件的~~ ★0916 S4 窗 C 现场纠正：**不限顶层件**——APS 挪一个自然 STRFLT 的 `POS`，同窗给锚点自己和它整棵子树里 14 个带 `CACHID` 的元素各盖一次，容器 STRSTR / TREADS / RPATH 不带这个字、一个没盖；
**这条纠正推不翻 Q6 的账**，已另核：`docs/evidence/2026-09-16-q6-cachid-only-cause-direction.md`——把成因表反向那一格（「祖先在同窗真变了」）全语料判了一遍，12476 个里**一个都没有**，唯一的 56 个全是我们自己造窗造出来的；
12476 的标签因此从「子树内一个都没变」加固成「上下两个方向都没有真改动」）：跳动里 2.7% 伴随本元素 DCHC≠0 / 结构变化、4.5% 伴随只 DCHC=0 / 字典外 / 成员表变化（成员增删 132/132 跳——子树图形变记在属主身上）、
**92.8%（12734）只有它变**，其中 12476 连子树里都没有同窗变化，形状是整库扫写（同一会话同 noun 全体 `+1`）；
(4) 单向 oracle 在有值元素上 **98.9%** 成立（369/373，4 个反例全是 BRAN 头尾类别 1/2），没写过的元素（213 个 DCHC≠0 变化停在 0）是覆盖不是纪律；反向 2.7%，不能当信号；
(5) 副作用：字典没收它，混进差分就既不放行也不刚——有值元素上 88 次含 `POS`/`ORI` 的变化全部同时跳（84 次若忽略它本可 transform），只 DCHC=0 的 83 次跳动里 67 次从 skip 推成 regen
（`LCHKDA` 81/83、`HREF` 4/5、`TREF` 1/1、`NAME` 2/25），12734 个只 `CACHID` 的候选今天全部上卷重算 → §8 Q5 / Q6。
顺带：字典外名字 `SPAMAP` 单独出现在 14289 个候选上（PANE / GENSEC / SCTN，占 40%），有值元素上 406/406 与 `CACHID` 同跳，今天全部 regen；不在本计划范围，记下备查。

★0916 S4 离线半程结论（同一份 453 库 / 2961 窗语料，`docs/evidence/2026-09-16-rigid-exclusion-parity.md`）：
(1) 只动了摆放的候选 **710** 个：合格锚点 79（NOZZ 14 / CYLI 13 / ELCONN 10 / ATTA 6 / BEND 6 / BOX 5 / DAMP 4 / 支吊架件 …）、路由容器之内 519（FLAN / ELBO / GASK / VALV / BEND / INST / TEE …）、
规则 M 结构件之下 62（FIXING 51、POINSP 11）、负体 50（全是 NCYL）；**P23 / 开孔在消费属主下 / CWBRAN / 点环容器 / 桥架件 / 剖面数据 / 几何集七条排除一个自然窗都没有**；
另有 51 个 `Δ` 是 |t| < 1e-3 mm 的纯平移（重写 POS 的末位噪声），只数不拍。
(2) **门（ADR-066 验收 1）**：合格锚点按根一起前移（兄弟锚点各自 Δ）对拍 56 根——27 个有判别力的两态全等（HANG 下 8 种支吊架件、SUBS 下 CYLI、SVOLM 锚点即根、
acw5525 四个 EQUI 各 2–3 个 NOZZ 同根）、**0 红**；22 根锚点子树无生成物（ams7997 HVAC 分支的 BEND / ATTA / DAMP、ams8000 的 ELCONN / STRT——没判别力）、7 根窗内另有改动（生产按 D1 规则 4 整根 regen，不作数）。
(3) **排除有内容**：负体——同根 5 个 / 2 个 NCYL 挪 200 mm，属主 FLOOR 三角 / 亏格 / 体积变（2 个干净样本）；规则 M——7 个 FIXING 与 2 个 POINSP 挪位，属主 GENSEC 体积 / 包围盒 / 亏格变（2 个干净样本）；
路由容器——VALV 挪 500 mm 隐式管身增删、ELBO 挪 25 mm 两根管身变、GASK 转向管身增删（5 个干净样本）；**保守**的形状也看见了：全是显式 FTUB、没有隐式管身的分支里挪 BEND / 单独挪 FTUB，Δ 前移也对（5 样本）。
(4) **Q5 的第二票**：`AIOS_RIGID_PROBE_IGNORE_CACHID=1` 把 `CACHID` 剔出比较集再判，多出 84 个合格锚点（绝大多数是整台 EQUI 挪位、锚点即根、1–30 件全部由 Δ 前移，另有 FLOOR 一例），
对拍 139 根 **108 个有判别力的全等、0 红**（`-ignore-cachid.md`）。E3D 真实的 EQUI 挪位就是刚体，`CACHID` 只是同一次编辑写下的缓存计数。
(5) E3D 造窗那一半（P23 `HRPOST`、`PLOPEN` 在 PANE 下、`NBOX` 在 BOX 下）见下一段 ★0916 S4 E3D 结论；对拍是文件侧（e3d-model 生成物的不变量），不是生产投影记录的 `record_id` / `mesh_id`——生产路径的 `mem://` 真跑仍在 ADR-066 验收 4。

★0916 S4 E3D 造窗结论（db8000 = `ams8000_0001`，会话 607→619 全是这一半写的；E3D 3.1 TTY 经 l3_suite `--check-driver`；`docs/evidence/2026-09-16-rigid-exclusion-e3d-windows.md`）：
(1) **负体在正体下——排除有内容**：EQUI `/1-LNR-Q005-PJ` 下建 BOX 1000³ + 竖穿的 NBOX 300×300×1400（608），只挪 NBOX 的 `POS`。第一窗 609→610（E 0 → E 200，孔仍整个在体内）探针判「负体（属主 CSG 消费）」、Δ 200 mm，两态**相等且报「无判别力」**——
**不是生成器没扣**：两端 BOX 记录体积都是 9.09999e8（= 1e9 − 300²×1000）、亏格 1、32 三角，`elmodl::collect_negatives` 收直属负体成员；是通孔平移不改体积 / 三角数 / 亏格 / 包围盒，六个不变量全同。第二窗 614→615（E 200 → E 400，孔越出 +E 面成槛）两态**不等、落在属主链** `BOX [triangles genus volume]`
（9.09999e8 → 9.24999e8、32 → 28 三角、亏格 1 → 0，窗内另改 0）——排除有内容 ✔；若把 NBOX 当合格锚点前移，BOX 记录原样不动就是错的。
(2) **开孔在消费属主下——造不出**：SBFR 下建 PANE + PLOO(HEIG 12) + 4 PAVE 都正常，CE = PANE 时 `NEW PLOPEN` 让 des.exe 0xC0000005 崩掉（分段试探把崩点钉在这一行；崩的那次不写会话、不落盘）；五个语料项目里 PLOPEN / WLOPEN / BPOPEN 零实例。
这条排除（`anchor_owner_exclusion`）今天只有单测 `a_consumed_member_anchor_regenerates_its_owner_unit` 护着，E3D 侧没有两态样本。
(3) **P23——db8000 造不出，改挪 APS 自然 `STRFLT` 造出窗 C：两态相等、有判别力 = 这形状上排除保守 ✔**：字典里 `HRPOST`（及 HRTERM / HRGATE / HRPANE / RAIL / KICKPL / CLNTIL）**没有 `POS` / `ORI`**（HR* 四个由 `POSREF` + `ZDIS` 派生，RAIL / KICKPL / CLNTIL 只有 `SPRE`、靠属主 / 相邻件定位），`is_placement_only` 对它们恒假，S2 那条排除在这 7 个 noun 上按构造不会被触到；
带 `POS` / `ORI` 的十个（STRFLT PLTFRM STRLNG DOOR WINDOW FLRCOV MLABEL FIXTUR CTSUPP FURNIT）在 db8000 从零建不出带几何的子树——PLTFRM 能在 STRU 下建、带 POS/ORI，但拥有不了 SBFR / SCTN / GENSEC / FLOOR（"Cannot create … at this level"），PANE / FRMW 被挂成 STRU 的兄弟；
FIXTUR / FURNIT / CTSUPP / ELFITT 在 ZONE / STRU / FRMW / SBFR 下一律 "Cannot create … at this level"；STRFLT 真实实例（aps250110）之下是 STRSTR / TREADS / RPATH / HANDRA，ASL 应用生成、TTY 里建不出。
于是走了当时摆出来的第一条路、**用户当日批准**：登录 APS（绕过 `l3_suite --check-driver`，直接 `run_ams_c_entrymacro.bat` + 自写 driver 宏）挪自然实例 `aps250110` 2013286654/8575 `STRFLT /B01_LEVEL_03_ACCESS/SF_001` `BY E 300` —— **窗 C = 32→33**。
探针判「P23 摆放派生 noun」、Δ 300 mm、锚点即根，**两态相等且有判别力**（锚点子树里两件 HANDRA 记录 984 三角 / 亏格 3，B 态全部由 Δ 前移，与 T 端整根重算逐个不变量相同）——core 遇 P23 改 `POS` 会 `REPELE` 整件重建，我方左乘 Δ 也对，**S2 那条排除的代价是「少算」，方向安全；到今天既没有反例、也没有必须它不可的样本**。
复原腿 33→34（`BY W 300`）跑完，`POS` **位精确**回到基线（STRFLT 记录 324 字节里，move 那一窗动过的位姿载荷两字节在 32→34 上消失；零增删、成员逐字相同），留痕只有 `CACHID` ——这一窗 E3D 给 STRFLT **和它整棵子树里 14 个带 `CACHID` 的元素**各盖了一次（容器 STRSTR / TREADS / RPATH 不带这个字、一个没盖）：
**Q5 接线前 `is_placement_only` 会被这 14 个戳判假、窗 C 根本进不了候选**，等于 Q5 的第三票（现场版）；顺带修正 S8 的措辞——`CACHID` 不只盖在「顶层件」上，改谁就盖谁整棵子树里每个带这个字的元素。
(4) 探针读法一条：负体 / 开孔类锚点自己没有记录，`moved` 恒为 0，两态相等时一律报「无判别力」——它分不清「属主没消费」和「不变量看不见那一挪」（ams1112 18→19 那条 NCYL 78 mm 大概率同因）；判别力只能来自属主记录的不变量变化，样本要挑会改体积 / 亏格 / 包围盒的挪法。
(5) 副产品（E3D TTY 行为）：不 SAVEWORK 的 QUIT 也会写一条只改内务 / `CACHID` 戳的会话（612 / 613 / 614 / 617），崩掉的不写；每次 SAVEWORK 之后紧跟一条内务会话（609 / 611 / 616 / 619）；`DELETE BOX` 连带 NBOX 成员的整段收尾腿崩掉，先 `DELETE NBOX` 再 `DELETE BOX` 干净。
ams8000 会话链从 607 长到 619，aps250110 从 32 长到 36（APS 一腿只写一个 sesno，SAVEWORK 后不跟内务会话——「一腿两号」是 db8000 的脾气，不是通则；35 / 36 是 fable-5-1-20 与 opus-5-23 两个会话各跑一遍复原腿撞车后的「重复复原 + 纠偏」，一对互逆的纯 POS 窗，36 与 32 位精确相同——取证文档 §4.6）——引用「453 库 / 2961 窗」那份语料总数时，ams8000 多 12 窗、aps250110 多 4 窗。

★0916 S7 结论（`probe_unit_level_rebuild_ratio`，ams8000_0001 整条会话链；报告 `docs/evidence/2026-09-16-unit-level-rebuild-measure.md` + 口径纠错 `-addendum.md`）：
(1) 规模：**606 窗 / 0 跳过 / 退化窗 4**，regen 根 6122（其中 3153 落在退化窗里），跨 regen 名单**等距抽**实量 978 根（16%，每窗上限 40、十个窗触顶），
进统计 792 根、T 端单元记录 3672 件、一字没变 2442 件、生成失败 0。三轮重跑跨 65 分钟、跨 Q5 接线，聚合数**逐个一致**——探针确定性，且 Q5 没扰动选根。
(2) **结论对分母敏感，四个口径两个答案**：真·重算（稳态窗 ∧ S 端已有这根）488 根 / 2670 单元 / 按单元加权 **86.5%（过线）**；再剔整根白算（稳健性检查）233 根 / 1440 单元 / **75.0%（过线）**；
稳态窗含新根 721 根 / 3366 单元 / 68.6%（不过）；全部合计 792 根 / 3672 单元 / 66.5%（不过）。分歧只在**新根（275 个，S 端没有这棵子树）算不算进分母**——
它们占比按定义恒 0，而 core 遇新根也得 `REPELE` 整件建、`SEGREP` 本来就省不了它，故**不进分母**。引用这个数**必须带口径**，只写一个数必被后人读反。
(3) 形状是**双峰不是均摊**：真·重算 488 根里 0% 档 116 根、100% 档 255 根，中间只有 117 根。按单元过线、**按根不过线**（稳健性组按根中位数 50.0%、均值 40.4%，>70% 的根只有 84/233）——
收益高度集中在少数大根，几乎全是 `BRAN`（380 根扛 2812 单元、75.9%；对比 `EQUI` 256 根才 273 单元）。**ADR 的收益模型要按单元算，按根数拍会拍高。**
(4) **旁证一条不属于 S7 的账**：整根白算 255/488 根按单元占真·重算总量 **1230/2670（46.1%）**——一件都没变却被整根重算。那是**选根**的题（S6 / §8 Q6），
单元级发布在这些根上一个图元都不碰、省不到它。它**比 S7 能省的还大**，提示先治选根可能比先治发布粒度更划算，另开一条线。
(5) 局限：只跑了 ams8000 一条链（全语料 `AIOS_UNIT_PROBE_CORPUS=1` 预计数小时，当时指挥官未批）；抽样 16%；生产行为零改动（回执未加 `unchanged_unit_ratio`，`WindowRootPlan` 一字未动）。

## 5. 验证与验收

- 单测名即判据（§4 各行）。全量 `cargo test --lib`（带在途计划的 `--skip`）不退步；`rustfmt --edition 2024 --check` 干净。
- 真库：ams8000 九个钉死窗 + `ams7999 45→46` 两态逐字相同（S1 / S2 / S3 各一次）；S6 的 486 库全语料两态门；S4 的对拍样本。
- 文档：三处改正过目；ADR-066 修订注；CONTEXT.md 词条。
- **不许当已证**：取证文档 §8 的伪码级结论（`UPGRPH` 分支结构、`TRASCN` 的 `DDEST(-1)`）没有逐指令核——S2/S3 的名单与规则若日后被逐指令推翻，按新证据改名单，判据形状不变。

## 6. 不做（本轮）

- 单元级发布（ADR-056 D3 例外）——S7 只量。★0916 仍然成立：S7 量完、**ADR-078 已立（草案）**，但**本计划内不实施**——
  它是独立的架构变更（改 `source_sesno` 语义 + `CommitScope` + 幂等闸），按 ADR-078 自己的 A1–A7 另开切片。
- `BFORI` 进 `TRANSFORM_ONLY`——只挂 `BPFITT`，不生成。
- 图形缓存路 / `CACHID` 作判据。
- `is_placement_only` 扩名单；`PLACEMENT_ATTRIBUTES` 不动。
- 老 PE 规划器与 legacy `update_world_transforms` 的删除（计划 S4，另一条线）。

## 7. 风险与取舍

- **读取集合漂移**：新增一处属性读点忘了登记 → 护栏红（故意的：这正是它要挡的）；但 gen-model 侧读点分散（房间 / cata_model），第一版可能漏，S5 先用正则扫 + 人工核一遍，护栏钉住之后才让 S6 吃它。
- **S6 变宽的开关沿用老名字**：`model_skip_metadata_only_changes` 的语义从「5 个名字」变成「DCHC=0 ∧ 未读」；关着时行为仍逐位等于今天。若要灰度，S6 再拆一个 `model_skip_unread_attributes` 也行（§8 Q2）。
- **P23 名单是 3.1 的常量**：换版本要按 `UPGRPH` 重取；名单常量注释带地址与重取 SQL。
- **规则 M 的「不是独立单元」判据**依赖 e3d-model `is_model_unit`——FIXING 今天是不是单元、TMPL 开孔怎么分类，S3 动手前先在 `category` 里核一遍，别按想象写。
- 与在途计划不撞文件：任务边界计划动 `batch_worker.rs` / `model_write_behind.rs`；CATA 闭包计划动 `cata_closure*` / `cata_dependency.rs`；本计划动 `model_impact.rs` / `window_root_plan.rs` / `rigid_motion.rs` / e3d-model `category.rs` / `increment.rs`。`window_root_plan.rs` 若同期被别人改，先拿写锁。

## 8. 待拍板

- **Q1** S6 是否等 S4 对拍样本全过再开？推荐：**不等**——两者独立，S6 自己的两态门（486 库）就是它的验收。
- **Q2** S6 变宽用老开关名还是新增 `model_skip_unread_attributes`？推荐：**老名字**（语义就是「只改了不影响模型的属性」，名单怎么来是实现细节），CONTEXT.md 词条改口。★0915 S6 按推荐落地：开关名不变，`options.rs` 文档与 CONTEXT.md「模型影响」词条已改口；灰度靶子是回执里的 `unread_attribute` 计数，不另加开关。
- **Q3** D3 第六张表是生成产物还是手抄常量？推荐：**生成**（`gen_dchc_fixture.py` 已在，多写一份 JSON 嵌入即可），手抄 213 个名字必漂。
- **Q4** S7 的阈值 70% 是否合适？推荐：先按 70% 记，数据出来再议；它只决定要不要立 ADR，不决定行为。
  ★0916 **已收口**：阈值没动，动的是**分母**。同一份探针同一条语料，含新根 68.6% 不过线、剔新根（真·重算）86.5% 过线——
  新根按定义占比恒 0，而 core 遇新根也走 `REPELE` 整件建、`SEGREP` 省不了它，拿它给 `SEGREP` 记过没道理，故**新根不进分母**。
  稳健性检查（再剔掉 255 个整根白算的根）仍有 75.0%，说明过线不是被 100% 那一档抬上去的。用户 2026-09-16 拍板：按此口径过线，
  **立 `docs/adr/ADR-078-unit-level-publish-rebuilds-only-changed-units-under-a-root-receipt.md`**（草案，D2 / D3 / D7 / D9 待逐条过目）。
  Q4 本身到此关闭；单元级发布的后续拍板转到 ADR-078 的 Q1–Q5。
- **Q5**（★0916 S8 派生，S4 / 开 `model_transform_cheap_path` 的前置）`ElementDiff::is_placement_only` 要不要把 `CACHID` 剔出比较集？语料里有值元素上
  88 次含 `POS`/`ORI` 的变化**全部**同时跳 `CACHID`，不剔则刚体前移在 3.1 真库上几乎永远打不着 EQUI / BRAN / SCTN（今天 79 个 transform 判定全落在
  没有 `CACHID` 的 noun 上）。推荐：**倾向剔**——它不是几何输入（生成器不读，S8 坐实它只是缓存计数），但这是「少算」方向，按纪律要两票：字典没给它投票，
  第二票只能是 S4 的两态对拍（造窗时挪一个带 `CACHID` 的 EQUI，`record_id` / `mesh_id` / 矩阵两态相等）过了再动 `is_placement_only`；S4 之前不动。
  ★0916 S4 第二票**已到**：语料里剔掉 `CACHID` 后多出的 84 个合格锚点（整台 EQUI 挪位为主）对拍 139 根、108 个有判别力的两态全等、0 红
  （`docs/evidence/2026-09-16-rigid-exclusion-parity-ignore-cachid.md`）。剩下的是拍板：改 `is_placement_only`（e3d-model `transform.rs` /
  `element_diff.rs`）把 `CACHID` 剔出比较集，`placement_input_changed` 那一半不动；改了之后 `model_transform_cheap_path` 在 3.1 真库上才打得着东西。
  ★0916 **已拍板并接线**（用户拍板，落点 e3d-model `src/element_diff.rs`）：新增 `PLACEMENT_STAMP_ATTRIBUTES = ["CACHID"]`（**戳** = 应用随图形重建写的
  缓存 `+1` 计数，非几何输入，S8 坐实），`is_placement_only` 由「非空 ∧ 全部 ∈ `PLACEMENT_ATTRIBUTES`」改判「**至少一项** ∈ `PLACEMENT_ATTRIBUTES` ∧
  其余只允许戳」——`[POS, CACHID]` / `[ORI, CACHID]` 算只动了摆放，**只有 `CACHID` 变**照旧不算（摆放一项都没动，那是整库缓存重建的扫写，走 regen，见 Q6）。
  `transform.rs` 的 `PLACEMENT_ATTRIBUTES`、`placement_input_changed`、`model_transform_cheap_path` 默认值（仍默认关）一字未动。读取集合走**有名字的排除
  口径**而不是把字面量藏起来（名字消失了守卫就钉不住「它是戳」这件事）：`gen_read_set.py` 的 `STAMP_LITERALS` 把它单列进产物 `stamp_literals`、永不进
  `names`，真被传进 e3d-io 读取口的照旧算读；四条守卫钉住（e3d `tests/attribute_reads_guard.rs` 一条、gen-model `model_impact.rs` 三条）。
  复验（2026-09-16，接线后**不带** `AIOS_RIGID_PROBE_IGNORE_CACHID` 全语料重跑对拍门，2961 窗 / 488s）：合格锚点由 79 升到 **163**，对拍 139 根、
  108 个有判别力的两态全等、**不等·锚点 0**；两仓 `cargo test --lib`（e3d-model 320 绿 / gen-model 2127 绿）、`increment_real` 两态门、
  `attribute_reads_guard`、`model_impact`、ams8000 九窗两条 live 全绿；`rustfmt --check` 无差异。ADR-066 D1 规则 2 已加同日修订注。
  ★0916 **第三票（现场版，S4 窗 C 的副产品）**：APS 里挪一个自然 `STRFLT` 的 `POS`，E3D 同窗给**锚点自己和它整棵子树里 14 个带 `CACHID` 的元素**（10 TREAD / 2 GENSEC / 2 HANDRA，连自己共 15 件）
  各盖了一次 `CACHID`，容器 STRSTR / TREADS / RPATH 不带这个字、一个没盖——接线前 STRFLT 自己那个戳就会让 `is_placement_only` 判假、**这一窗根本进不了候选**，接线后判成「只动了摆放」并两态全等。
  一并修正 S8 的措辞：`CACHID` **不只盖在「顶层件」上，改谁就盖谁整棵子树里每个带这个字的元素**（`docs/evidence/2026-09-16-rigid-exclusion-e3d-windows.md` §4.4–4.5）。
- **Q6**（★0916 S8 派生）只 `CACHID` 的 12734 个候选（92.8% 的跳动，12476 个子树里同窗无变）今天全部上卷重算——要不要把 `CACHID` 当元数据放进两票门
  （`no_model` 第三档，如 `cache_stamp`）？推荐：**先不动**——写方语义是从形状推的（§8 未证），且探针只看 DESI 窗口、排除不了「被引用的目录件变了」
  这一来源；真要省，先量它今天多花了多少根（S7 同款「先量」），再做 S6 同款两态门 + 造窗对拍，另立切片。
  ★0916 **成因表反向一格已补判**（`docs/evidence/2026-09-16-q6-cachid-only-cause-direction.md`，S4 窗 C 引出的复核）：S8 只问过「有没有真改动**以我为严格祖先**」（在我**之下**），
  没问过反向的「我的**祖先**在同窗真变了」。全语料 2977 窗独立重实现判据重跑：12476 个「子树内一个都没变」的候选里**一个都没有**变了的祖先（先滤掉 `WORL` 的 `UPDT`——
  全库根的每会话时间戳，不滤它这一格恒真），唯一的 56 个全来自我们自己造的那四个 aps250110 窗。**Q6 的账（12734 候选 / 9172 多算根 / 四门）一格不动**，
  而且多算根按构造就不受成因影响（它是两态各跑一遍规划器的集合差）；12476 那一格的标签可以从「子树内一个都没变」写强成「**上下两个方向都没有真改动**」。
  往前看一条：语料以后若收进真实设计编辑（而非整库扫写），只戳候选里会出现「子树顺带被盖」的一类——它们的根本来就要 regen，不该记成多算，成因表那时**两个方向都要判**。
