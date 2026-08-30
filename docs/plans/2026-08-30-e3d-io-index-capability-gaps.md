# e3d-io 索引能力缺口盘点与补齐计划（core.dll 对齐 · 索引篇收尾）

> 日期：2026-08-30。状态：**已完成（P0–P4 全落地，门全绿）**——用户 14:18 侧栏拍板「按推荐方案」：
> §4 五个决策点全取推荐项（extent 自动 attach **做**；索引切分 =
> e3d-io 原语 + gen-model 持久化；type 索引**进**第一轮；Named 档**只解 NAME**；
> P0 反证则 G1 自动降级为观测）。
> 前置调研：本日「core.dll 索引盘点」（`上下文/会话-2026-08-30-core-dll索引分析.md`）——
> dabacon 文件里的索引结构清单与四条消费线现状，本文不重复，只落「e3d-io 还缺什么、怎么补」。
> 权威与证据：
> - `docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`（页头/AoS/free_dwords，指令级）
> - `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（双根差分/表根获取 `sub_5AF6840`）
> - `docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`（记录自寻址，438 库全量）
> - `docs/plans/direct-dbelement-read-api.md`（D3：name/backref 索引需求）、ADR-053（direct 生成读）
> - 覆盖分析缺口 Q-B（`上下文/会话-2026-08-30-数据接口覆盖分析.md`）：文件侧**无类型索引**，
>   `query_type_refnos_by_dbnum` 一族（生成主链 ×7 调用点）在 direct 下无路可走

---

## 0. 一句话

core.dll 的两张系统表里，e3d-io 已把**主索引表（13387743）吃透**（点查/枚举/双根差分三道硬门全绿）；
剩下的缺口是：**数据表（7618377）页头没校验**、**多 extent 路由只有缝没接线**、
**direct 模式要的三个派生索引（type / name / backref）文件里本来就没有、必须一次全树扫描预建**。
本计划按「先探针、后实现、每步带门」补齐这三块，研究项（表目录 / claim 页 / flag / 页头 `[5]`）只观测不实现。

---

## 1. 现状盘点：已有的不再做

| 能力 | 位置 | 门（已绿） |
|---|---|---|
| B+ 树点查 | `engine::search_index` → `index::choose_child`（唯一路由） | 429 库 789 831 键点查=枚举同存在性集合；冷点查读页数==树高 |
| 活树枚举 | `IndexCursor::walk` / `engine::indexed_refnos`（只走会话根的活树；旧全页扫描多读 16% 死树条目已废） | 同上 |
| 双根差分 | `index::diff`（t-327） | 428 对相邻会话与全量枚举集合差逐键相等；读页数==两树页集**对称差**（精确门） |
| 会话/时点 | `open_at`/`open` 单一实现（`session::walk_chain`） | `session_l1_contract` 11 条 |
| extent fail-loud | `PageId{ext,page}`、`ExtentSet::resolve` → `MissingExtent`（含期望路径） | `page_l0_contract`；缝 `ExtentSet::attach` 已预留（io.rs L114） |
| 记录自寻址 | `record::read_element_record`（头地址槽 + 块续接地址） | 789 831/789 831 + 自报 RefNo 一致 |
| NAME 读取 | `element_name::stored_name`（显式属性 NAME）+ 无名元素位置式命名 | TTY 样本对拍 |

## 2. 缺口清单

### G1 · 数据表（table_id `7618377`）页头校验缺失 —— 〖中〗fail-loud 缺一块

core.dll 的页读取器对**两张**系统表都硬校验 `page[0]==5 && page[1]==<表id>`（索引 `sub_5AEE4E0`、
数据页 `sub_5AFB660: v3[1] == 7618377`）。e3d-io 只在索引下降时校（`cursor::read_node`）；
**记录层读数据页不看页头**（`record/mod.rs` 零处引用 `DATA_PAGE_MAIN`，常量躺在 `meta/constants.rs` 没人用）。
坏页号/错 extent 目前靠「记录自报 RefNo 不符」兜住，但那是**读完才发现**，且 `read_element_direct`
这类裸地址入口自报校验是可选的。**先探针再落地**：语料里数据页头是否恒为 `(5, 7618377)` 未全量量过
（E3D 2.10 是 7618321，版本敏感）。

### G2 · 多 extent 路由：缝在、线没接 —— 〖低〗（语料 0 例）

`ExtentSet::attach(ext, path)` 与 `ExtentNaming::path_for`（`<stem>_NNNN` 命名探测）都在，
但 `open_selected` 从不调用 attach——遇到 `PageId{ext:2}` 即 `MissingExtent`（fail-loud，行为正确）。
本机 1002 个 dabacon 文件 **0 个多 extent**（ADR-055 Q7 实测），direct 计划 D1 也已定「显式拒绝」姿势。
要不要在本轮把「开库时探测 `_0002+` 兄弟文件并自动 attach」接上，是决策点（§4-1）。

### G3 · 派生索引三件套：type / name / backref —— 〖高〗direct 模式的硬前置

dabacon 文件里**只有 refno 一把主键**。direct 模式（ADR-053）要的三类查找文件侧都没有：

| 索引 | 谁在等它 | 文件侧原料 | 建造成本 |
|---|---|---|---|
| **type（noun→refnos）** | 覆盖分析 Q-B：`query_type_refnos_by_dbnum` 一族，生成主链 7 个调用点「按 noun 收根」 | 记录头 `[3]` 就是 noun_hash——**只需读每条记录的 11-dword 头** | 一次全树走查 + 每元素 1 次头读 |
| **name（name→refno）** | D3：`Db.FindElement(name)` / `MDB.FindElement(name)` | 显式属性 `NAME`（`element_name::stored_name` 已会读） | 同上 + 每元素解 1 个显式属性 |
| **backref（refno→入边）** | D3：反向引用消费方（ADR-002/003 B 工作流） | 各 ref 型属性（SPRE/CATR/LSTU/PSPE/OWNER…，schema 可枚举 ref 类型） | 同上 + 按 schema 过滤解全部 ref 型属性 |

三者共用**同一次全树扫描**（D3 原文也是这么定的：与 `scan_identity_ref0s` 同一次遍历顺带产出）。
分工切法见决策点 §4-2：**扫描原语放 e3d-io、持久化与失效放 gen-model**（推荐），
理由：e3d-io 是纯格式库（重写方案 Q3 已裁 L5 语义不进），索引的磁盘格式、指纹失效、
跨库聚合是 gen-model 的产品决策；且 D3 已把 `name_index.rs`/`backref_index.rs` 定在
`src/data_interface/direct/`。

### G4 · 研究项（只观测，不实现）

| 项 | 现状 | 动作 |
|---|---|---|
| ~~数据表根的文件态来源~~ **（R1 已闭合）** | `sub_5AF6840` 内存控制块 +20/+24 = **会话页 0x24**（e3d-io 现名 `claim`，实为数据表根，429/429）；该树 = refno→(0,会话号) 的**主元素会话索引**，按 noun 全有或全无覆盖，`sub_5AFB660` 读之 | ✅ 证据 `docs/evidence/2026-08-30-e3d-io-data-table-tree-7618377.md`；探针 `examples/data_table_tree_probe.rs` / `data_table_tree_dump.rs`。遗留：`SessionPage.claim` 改名单列 |
| 表目录 | 两张表之外核内走「表目录数组线性查」，文件态未见第三张表 | 出现未知 table_id 再立项 |
| claim 页（`claim_pgno`） | 写侧并发控制，只读生成用不到 | 不做 |
| 叶值 `flag` 低 12 位 / 页头 `[5]` | 逆向未闭合（C3/C4） | 维持 R4：路由与存在性不看 flag，探针只做直方图 |

---

## 3. 阶段与门

| 阶段 | 内容 | 门 |
|---|---|---|
| **P0 · 数据页头探针** ✅ | 已跑（`examples/data_page_header_probe.rs`，429 库 162 446 页）。**反证成立，§4-5 预案生效**：活记录部件 100% 落在 `word0==7` 的页（106 807 页零例外）；`(5, 7618377)` 每库都有但活树零引用——它是**另一棵树**，`DATA_PAGE_MAIN`「元素数据页」旧注释是错的（word0=5 = B+树页，word1 = 树标识，语料见 60 种）。会话页 word5/word9 = 数据表根候选，word13/14/15 = 年/月/日时间戳（喂 R1） | ✅ 证据：`docs/evidence/2026-08-30-e3d-io-data-page-header.md` |
| **P1 · 元素页校验落地**（按 P0 修订） | ~~校 `(5, 7618377)`~~ → `record::read_record` 取页处校 **`page[0]==7`**（唯一钉死不变量；word1/word2 语义未定不校）；错报带页号/extent/实际 word0；`meta/constants.rs` 常量注释按证据修正 | 429 库 789 831/789 831 **保持**；新增合成用例：把索引页（word0=5）喂给记录读取器必须红、错串点名页型 |
| **P2 · `scan_elements` 扫描原语**（e3d-io）✅ | 落地 `engine::scan_elements(tier) -> ElementScan`（流式，键序，单元素失败可续扫）。三档定名 `Header` / `Named` / **`Full`**（原 Refs 档改为交全量 `ParsedElement`：members + 全显式属性含 ref 型；隐式区 ref 解码需要 noun 描述符 = schema，按边界归 gen-model 描述符机器，e3d-io 不半实现 schema） | ✅ 键集 429 库逐键 == `indexed_refnos`；名字与单元素路径对拍；**吞吐：Header 274ms / Full 769ms 全语料 789 831 元素**（`tests/scan_elements_contract.rs`） |
| **P3 · 三个派生索引**（gen-model direct 层）✅ | 落地为**单文件** `src/data_interface/direct_index.rs`（三索引同一次 `scan_elements(Full)` 产出、同一份指纹失效，拆三文件会把一次遍历写成三次）：`DbIndexes{by_type, by_name, inbound}`；入边分类 `BackRefVia::Owner/Member/Attr(hash)`，隐式区 ref 走描述符抽取（只收 Decoded/DecodedExplicit，默认值不是出边；OWNER 去重）；`IndexFingerprint` = 格式版本+dbnum+sesno+文件长度+mtime，bincode 落 `%TEMP%/aios-direct-index`（`AIOS_DIRECT_INDEX_DIR` 可改），旁文件+rename 原子写；`DirectStore::indexes(dbnum)` 挂接，pin 变更连带失效 | ✅ `tests/direct_index_contract.rs`：三索引逐条用单元素读法回查（不碰 Surreal 即可裁真伪）；指纹缓存门（命中 1.1ms、缓存文件不动）；换会话号强制重建。**ams8000：6 605 元素 / 1 829 有名 / 21 082 入边 / 构建 65ms**——D3 成本判据闭合。Surreal 对拍三道留 `#[ignore]` 探针（需活库手动跑） |
| **P4 · extent 自动 attach**（§4-1 选 A）✅ | 落在 io 层唯一权威 `ExtentSet::open_primary`（engine/DbView/探针统一受益）：按 `ExtentNaming` 从 1 连续探测兄弟文件逐个 `attach`，跳过 primary、遇缺口即止；**在跑的兄弟文件 attach 不上=开库失败**（它答应了库的命名法，「missing」是假话）；命名法不合/无兄弟保持 fail-loud | ✅ io 单测：开库自动挂 `_0002`、缺口规则（`_0004` 无 `_0002` 不挂）、非常规名手动 attach 缝保留、页大小不合拒开库；`tests/extent_attach_contract.rs` 合成双 extent 夹具（会话页根指 ext 2）：跨 extent 枚举/点查/成员块链/scan 全通，缺 `_0002` 时报错点名文件；语料三门重跑 **789 831/789 831 保持**（scan 274/769ms、cursor、record L3 全绿） |

依赖序：P0 → P1；P2 → P3；P4 独立。P2/P3 不等 P0/P1。

---

## 4. 决策点（plannotator 审阅时请重点批注）

1. **extent 路由（G2）**：A. 本轮接上自动 attach（P4，缝已在，改动 ~30 行 + 夹具）；
   B. 保持显式拒绝，等真实多 extent 库出现再做。**推荐 A**——报错路径已经点名期望文件路径，
   自动 attach 只是把「你去把文件给我」变成「文件在我就用」，且合成夹具能钉住行为；
   但若认为 0 语料 = 0 优先级，选 B 零成本。
2. **派生索引归属（G3）**：A. 扫描原语在 e3d-io、持久化在 gen-model（**推荐**，理由见 §2-G3）；
   B. 索引整个做进 e3d-io（格式库背上产品失效策略，Q3 分层被打穿）；
   C. 全在 gen-model（gen-model 手写树走查，等于第二份下降实现，撞 R5「唯一实现」红线）。
3. **type 索引是否进第一轮**：D3 原文只列了 name/backref 两个。覆盖分析 Q-B 证明 type 是
   生成主链必踩（7 个调用点）。**推荐进**——原料最便宜（记录头自带 noun_hash），不做则
   D4 的 fail-loud 路由在第一个 `query_type_refnos_by_dbnum` 上就会炸。
4. **Named 档解码范围**：只解 NAME 一个显式属性（**推荐**；`stored_name` 复用，成本可控），
   还是全量属性解码（backref 反正要 Refs 档全解 ref 型——但 name 单独建时不必陪跑）。
5. **P0 反证兜底**：若 429 库里数据页头**不是**恒 `(5, 7618377)`（如版本混杂 7618321），
   G1 改为「记录分布、不加校验」，P1 撤销——先说好，避免探针出来后现场发明降级。

---

## 5. 风险

| # | 风险 | 对策 |
|---|---|---|
| R1 | 数据页校验把好记录挡在门外（页头语义没吃透） | P0 全量探针先行；P1 门要求 789 831/789 831 **保持** |
| R2 | 扫描原语变成第二份树下降实现 | 复用 `IndexCursor::walk`；沿用源码顺序断言「页→结点只有 `read_node` 一处」 |
| R3 | 派生索引失效不干净（文件被 E3D 追加写后缓存陈旧） | 指纹 = 文件身份 + 长度 + 会话号（与 `DirectStore` FileIdentity 同口径）；键控 `(dbnum, pinned_sesno)` |
| R4 | backref 的 ref 型属性清单不全（漏一种属性 = 漏一批入边） | 从 schema（attlib `default_val` 为 ref 型）枚举，不手写清单；与 Surreal 反向边对拍兜底 |
| R5 | 索引构建成本吃掉免入库收益（D3/R9 原话） | P3 门 ④ 强制出对比数字，交 ADR-054 裁决，本计划不预设结论 |
| R6 | 三个索引与 DB 模式各答各的 | 全部门用 Surreal 侧同口径对拍（type/name/backref 三道） |

## 6. Non-Goals

- 写侧（claim / SaveWork / 索引写回）。
- flag / 页头 `[5]` 的语义发明（R4 纪律不变）。
- 表目录第三张表的支持（未见语料）。
- 净窗口/水位搬家（那是姊妹计划 §9 路线 C 与 ADR-053 P5 的事，本文不越界）。

## 7. 文件清单（预估）

| 动作 | 文件 |
|---|---|
| 新增 | `vendor/e3d-io/examples/data_page_header_probe.rs`（P0）、`vendor/e3d-io/src/engine.rs::scan_elements` + payload 类型（P2）、`gen-model/src/data_interface/direct/{type_index,name_index,backref_index}.rs`（P3）、合成双 extent 夹具（P4） |
| 改 | `vendor/e3d-io/src/record/mod.rs`（P1 校验入口）、`vendor/e3d-io/src/page/io.rs::open_selected`（P4 自动 attach）、`gen-model/src/data_interface/direct_store.rs`（P3 挂接） |
| 证据 | `docs/evidence/2026-08-30-e3d-io-data-page-header.md`（P0）、P3 成本对比入 ADR-054 输入档 |
