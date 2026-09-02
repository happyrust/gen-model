# 旧版 pdms-io 与 core.dll（db1~db5）数据读取方式差异审计与开发计划

> 日期：2026-08-30。状态：**待审**（plannotator 门）。
> 审计对象：`../vendor/old-pdms-io`（`codex/room-panel-wire-repair-deps` @ `5e4e4d7`，
> gen-model 经 `[patch]` 消费）。
> 对照基准（分层权威沿用 ADR-055 Q1）：
> - db1–db3（页 / 会话 / B 树）：core.dll E3D 3.1 逆向产物
>   `pdms-io.git@13a17e1:docs/ida-3.1-structures.md`（IDB MD5 `b7def476…`，带指令级证据），
>   以及按它重建、已过 `core_dll_oracle` 对拍的 `pdmsdb_engine_v2` db1~db5 实现；
> - db4 及以上（元素语义）：Core3D.dll（ADR-055 / `specs/034-core3d-semantics/`，**不在本计划射程**）。
> 关联：ADR-053（direct 模式生成读）、ADR-055、`docs/plans/pdms-io-v2-core3d-alignment.md`、
> `tests/pdms_record_boundary.rs`、`specs/003-net-window-collection`、
> `docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`（**本计划 db1/db3 结论的指令级补证**）、
> `docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`（**本计划 db4 返工的依据：记录层自寻址实测**）、
> `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md`（姊妹计划：clean 重写路线；其 §9 是本次路线重估的落点）。

> **2026-08-30 IDA 补证（复用实例 `idalib-48392`，指令级）—— 本计划核心判断获独立坐实**：
> 直接读 `sub_5AFFCB0` 指令流，确认索引结点是 **AoS**（条目步进 = `游标 + key_dwords + value_dwords`，
> `0x5affe2f`/`0x5affe31`；值紧跟键之后，`0x5b007bc`），页头 `[2]`level@0x08 / `[3]`key_dwords@0x0C /
> `[4]`value_dwords@0x10 / `[6]`free_dwords@0x18。由此：
> - **D3-1（条目计数）** 从「engine-v2 这么写」升为 **core.dll 指令级坐实**：
>   `count = (容量 unk_6453DC4[0] − 7 − free_dwords) / (key_dwords+value_dwords)`，逐项对上；旧栈 scan-to-zero
>   多读的就是 copy-on-write 残留槽位。**P3 门可视为已开**（真库归零仍需 P1 尺子量化）。
> - **D3-2（键/值宽）** 坐实：宽度取自页头 `[3]/[4]`，内部结点值宽恒 2；旧栈硬编 4 字步长在非 2+2 页会错位。
> - **D1-1/D1-4** 坐实：页大小/页型/extent 都是运行期页头 + `(pgno, extno)` 二元地址，旧栈硬编 0x800、无视 extent 属实。
> - **D3-4** 补强：叶值 `value_dwords==2`（无独立第 3 字 flag），`packed=offset<<12|flag` 的位拆与旧栈/engine-v2 一致；
>   V1（文件态 `>>12/&0xFFF` vs 内存态 `>>13/&0x1FFF`）仍按原计划走 FFI oracle。
>
> 顺带：这轮 IDA 也**推翻了姊妹计划（clean 重写）的头号前提**「core.dll 是 SoA、现存实现全错」——
> 实测 AoS 恰恰说明**本计划「就地硬化」在索引层（db1–db3）更站得住**（旧栈读模型本就对，
> 只需修 count/stride/extent）。详见该证据文件与姊妹计划顶部订正框。
> **但同日的记录层补证把 db4 这一半翻了回去，见下框。**

> **⚠️ 2026-08-30 记录层补证（`docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`，
> 438 库 / 2 923 428 条记录全量统计）—— 本计划 db4 部分需实质返工**：
> **记录写着它自己各部分在哪。** 元素头是 **11 个 dword**（不是 6 个）：第 6/7 字是显式属性流
> 起始地址、第 8/9 字是成员表起始地址、第 10 字是格式字；每个尾块头 20 字节，其中 +12..+20 是
> **续接地址**。三者都用索引叶值那套 `(page_no, packed)` 编码（`offset_words = packed >> 12`）。
> 全量统计：这些地址够到 3 195 015 个块，**指空 0 个、类型对不上 0 个**；相邻性够不到其中
> **46 566** 个（分布在 40 263 条记录上），且「跳 0/7 填充继续找」在 **36 106** 条记录上跨出本页、
> 把同一元素的**后来副本**接了上去；另有 **5 444** 个块压在页边界却**不**声明续接——
> 「压在页边界」不等于「有后续」，只有那个地址说了算。
>
> 对本计划的三点影响（**逐条落在 §1 与 §2**）：
> ① **新增 D4-4 / D4-5 两行**（记录头地址槽、块续接地址），等级〖高〗，**P0–P4 原文一条都没盖到**；
> ② **P4 必须重写**：原文「优先转调 engine v2 `RecordReaderV2`」的补救对象**本身就是同一形状的
>    搜索式读法**，只是窗口从 2 KiB 换成 16 KiB→1 MiB（核实见 §1 D4-4 注与姊妹计划 §9.2）；
> ③ **P1 的尺子不覆盖记录层**：`src/bin/legacy_v2_read_parity.rs` 全部 1126 行零处读记录，
>    所以 db4 这一半**现在没有验收面**，必须先造门。
> 路线层面的重估（三选一、带行号的改动量）已写进姊妹计划
> `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md` §9，**本计划不重复，只落自己的阶段改动**。

## 0. 审计范围与生产消费面

旧版 `pdms_io` 在 gen-model 里承担的是**增量引擎的文件侧地基**，与 engine v2 是两条并行读取栈：

| 读取栈 | 入口 | 生产消费方 |
|---|---|---|
| 旧栈（本计划对象） | `PdmsIO` / `DabaconSnapshot` / `net_window` / `session_index_diff` | `increment_pipeline`（AMS 8000 净窗口）、`manual_update`、`increment_manager`、`window_repair`、`staging/*`、`versioned_db/database`、`batch_worker`、`sesno_range`、`on_demand_db`（权威 token）、`cata_closure`（会话指纹） |
| engine v2 栈 | `parse_pdms_db::paged::PagedDbSession` → `pdmsdb_engine_v2` | `on_demand_db`（paged 读数）、direct 线（D0–D6，规划中） |

要点：净窗口/水位/暂存这条**主增量链完全跑在旧栈上**；direct 线只接管生成读。旧栈的读取正确性
在可见未来仍是水位承诺（宪法「水位是承诺」条）的直接依赖，这是本计划存在的理由。

**W0 · 环境阻断（先于一切）**：gen-model `Cargo.toml` 第 225–226 行第四路 patch
`pdmsdb_engine_v2 = { path = "../../pdms-io-fork-engine-v2/…" }` 处于**未注释**状态，而该目录
在本机不存在（`d:\work\plant-code\` 下无 `pdms-io-fork*`），workspace 当前无法解析依赖图。
`../vendor/old-parse-pdms-db/src/paged.rs` 已在引用只有 engine-v2 工作副本才有的 API
（`page_size_bytes_hint`、`open_read_at`），印证三仓 vendor 链与 engine-v2 工作副本本就是
配套开发状态——是**工作副本目录丢了/被移走**，不是 patch 加错。处置：恢复该工作副本
（或从 `pdms-io.git` 重新检出到该路径），不能简单注释掉两行了事。

> **2026-08-30 现状核实**：`Cargo.toml` 那两行**已改回注释态**（现在在 229–230 行，
> 带一条 2026-08-30 的说明），`d:\work\plant-code\pdms-io-fork-engine-v2` 确不存在。
> 也就是说 **W0/P0 事实上按「注释 patch」这条落地了**，engine-v2 回到 git rev
> `13a17e1`（源码在 `D:\Rust\.cargo\git\checkouts\pdms-io-565ad9bfb921054d\13a17e1`，可只读查阅）。
> 计划正文仍写着「默认恢复工作副本」，与现状不符——**决策点 1 请按现状重新确认**：
> 若接受现状，`old-parse-pdms-db/src/paged.rs` 是否还在用只有工作副本才有的
> `page_size_bytes_hint` / `open_read_at` 需一并核（当时是「退版本会连带退功能」的理由）。

## 1. 逐层差异矩阵

标注：〖高〗直接影响读取正确性；〖中〗有触发条件或有补偿层；〖低〗行为等价性问题。
每条给证据位置，可复核。

### db0/db1 · 文件与页

| # | 维度 | core.dll 3.1 | 旧版 pdms-io | 等级 |
|---|---|---|---|---|
| D1-1 | 页大小来源 | 头部 `0x34` 按 **4 字节字**存页大小（512 字 → 2048 字节），运行时从头部取（ida-3.1 §1、§13.5） | `PAGE_SIZE = 0x800` 编译期硬编码（`defines.rs:14`），`PdmsHeader` 只解析到 `0x2C`，**根本不读 `0x34`**，也无任何断言 | 〖高〗静默前提 |
| D1-2 | 页读取健壮性 | `FHDBRN` 失败（error 11）→ `SYWAIT 0.5s` 重试 → `FHSWIT` 关闭/重开切模式再试 → `FLFINI ABORT`（ida-3.1 §10）；页缓存读头页+目标页 | 裸 `seek + read_exact`，零重试（`io.rs:1442`）；元素页每次直读磁盘无缓存，仅索引页/会话页有缓存 | 〖中〗并发写撕裂由 `DabaconSnapshot` 的 4 次稳定捕获在**打开时刻**补偿，打开后无防护 |
| D1-3 | 页头校验 | 页型/type_id 入口硬校验：表页搜索 `*page != 5` 直接置错 659（engine v2 `db3/index.rs:8-10` 引证）；数据页 `0x743F49`、索引页 `0xCC47DF` 常量比对 | 索引页仅 deku `assert_eq noun==0xCC47DF`（**不看 page_type**）；会话页、元素页完全无校验，坏页号会被硬解成垃圾数据 | 〖中〗 |
| D1-4 | 扩展文件（extent） | 页地址是 `(ext_no, page_no)` 二元组；per-DB 状态 216B 里有多文件一致性链接页（ida-3.1 §8） | 结构里有 `ext_no` 字段（`SessionPageData.last_ses_extno`、`RefnoIndexPgId.ext_no`）但**所有读取路径一律无视**，多 extent 库会拿主文件页号硬读主文件 | 〖高〗须显式拒绝（与 `on_demand_db.rs:87-90` 已定姿势对齐） |

### db2 · 头 / 会话

| # | 维度 | core.dll 3.1 | 旧版 pdms-io | 等级 |
|---|---|---|---|---|
| D2-1 | 头部字段覆盖 | `version(0x04)`、`session_page_no(0x30)`、`page_size(0x34)`、`stored_page_count(0x38)`（engine v2 `db2/header.rs`） | `PdmsHeader` 止步 `ext_no(0x2C)`，上述四个自检锚点全部不读 | 〖中〗`stored_page_count` 与文件长是天然的截断探测器，白白放弃 |
| D2-2 | 会话链回溯 | `page_type==3` 校验后走 `last_ses_pageno`，`<=0` 终止（engine v2 `db2/session.rs:39-44,221-224`） | 同样走 `last_ses_pageno`，但无页型校验；终止条件是经验值 `cur_ses_pgno > 4`（`io.rs:1818`）；有环检测 ✓、有冻结长度守卫 ✓ | 〖低〗 |
| D2-3 | 会话归属反查 | core.dll 无「页号→会话」概念（它按会话根直接下降） | `get_sesno(pgno)` 靠 `ses_range_map` 页号区间启发（append-only 假设）；engine v2 `sesno_for_page` 同款启发 | 不算差异，共同假设；compact/页复用会破坏它，记档不动 |
| D2-4 | claim 页 | 会话页带 claim 根，core.dll 读它做锁管理 | 解析出 `claim_pageno` 字段但从不读内容 | 只读增量不需要，**不追** |

### db3 · B+ 树索引（差异最重的一层）

| # | 维度 | core.dll 3.1 | 旧版 pdms-io | 等级 |
|---|---|---|---|---|
| D3-1 | **条目计数口径** | 页头第 7 字 `free_dwords` 反推：`count = (page_dwords − 7 − free_dwords) / stride`（engine v2 `db3/index.rs:51-53,146-150`，附「已释放槽位不算条目」回归测试与真库陈旧槽位探针） | 「**读到首个 0 字为止**」（`defines.rs:412-428` 零终止扫描）→ 已释放/陈旧槽位残留的非零字节被当成**有效条目**读进来。旧栈为此付出的整套补偿：搜索端去重+首见者胜（`io.rs:2389-2408`）、`session_index_diff` 的 12 项异常记账（`duplicate_child_pointers`、`out_of_range_leaf_entries` 等，**ams8000 实测非零**，见 `session_index_diff.rs:83-96`） | 〖高〗**正确性根因**。core.dll 按 free_dwords 天然看不见陈旧槽位；旧栈是「先读进垃圾、再层层过滤」，穷举遍历的正确性建立在补偿逻辑完备之上 |
| D3-2 | 键/值宽 | 页头 `key_dwords/value_dwords` 声明（0 回退 2+2；内部节点值宽恒 2，忽略声明）（`db3/index.rs:71-111`） | 硬编 2+2（`RefnoDataLoc` 固定 4 dword 步长），页头三个字被当 `unknowns` 扔掉 | 〖中〗遇到非 2+2 声明页直接整页错位 |
| D3-3 | 下降语义 | 小于→前一分支；等于→当前分支；大于全部→末条；哨兵 `0x80000001` 对是最左子树指针；同键首见者胜 | **已对齐**（曾错过一次：`target_r1 <= entry.refno_1` 选当前分支的错误路径选择，`issues/btree-search-algorithm-fix.md` 已修并带回归测试）；额外能处理「内部页只剩哨兵」吗——engine v2 显式处理（`db3/index.rs:343-352`），旧版哨兵分支要求 `unique_entries` 非空才走（`io.rs:2446-2459`），空普通条目+仅哨兵时旧版返回 None | 〖低〗边缘 case 待 P1 对拍确认 |
| D3-4 | 叶值字解码 | engine v2：`offset_words = packed >> 12`、`flag = packed & 0xFFF`、字节偏移 = offset_words × 2（`db3/index.rs:165-167`）——与旧版 `RefnoDataLoc`（20 位 offset ×2 + 12 位 flag）**一致** ✓。但 ida-3.1 §5 记录的 core.dll **内存态**搜索结果编码是 `& 0x1FFF / >>13 & 0xFFF`，两种口径未互证 | 同左 | 〖验证项 V1〗FFI oracle 一锤定音，纯文档风险 |
| D3-5 | flag 的使用 | 语义未完全逆向（12 位） | 自身口径不一致：点查**不看** flag（凡键可达即命中）；认领扫描 `filter_index_data` 要求 **`flag == 1`**（`io.rs:4293`）。两处判据的差异没有证据说明谁对 | 〖中〗先记账后裁决 |

### db4 · 元素记录 / 属性

| # | 维度 | core.dll 3.1 | 旧版 pdms-io + old-parse-pdms-db | 等级 |
|---|---|---|---|---|
| D4-1 | **记录读取窗口** | ~~页感知窗口 16KB 起、×2 增长至 1MB~~ **（2026-08-30 订正：这一列填的不是 core.dll，是 engine v2 自己的做法，而它也是搜索式的，见 D4-4）**；起始页页型必须 5/7 是对的 | `read_raw_element_record` 固定 **2KB 平面窗口**（`io.rs:3047-3053`），且 `parse_element`（`io.rs:3002-3006`）另有**同一段读的第二份拷贝**；无页型校验；段合并 `get_merged_data`（相邻性续接，只在窗口内）；显式属性流靠 `collect_explict_data` **启发式跳过嵌入的索引页头** + `MAX_RESYNC=64` 重同步（`parse.rs:784-898`），跳过量不记账 | 〖高〗从记录起点算 >2KB 的一切**静默不可见**：children 不全 → `member_alive_at` 误判 Deleted 的通路存在。`tests/pdms_record_boundary.rs` 只钉了「不越界读」，没钉「跨页完整性」 |
| **D4-4** | **记录头的两个地址槽**（2026-08-30 新增） | 头是 **11 dword**：`[6..7]` 显式属性流地址、`[8..9]` 成员表地址、`[10]` 格式字，编码同索引叶值 `(page_no, packed)`。语料 3 195 015 个块指空 0 / 类型错 0 | **四个解析入口一个都不读**，读完 `input[16..24]`（owner）就跳 `padded_implicit_end`：`parse_raw_element_identity`（`parse.rs:363-388`）、`parse_ele_membs`（`438-473`）、`parse_ele_children`（`485-524`）、`parse_raw_ele_data_with_info`（`541-725`，关键 `580`/`592-600`/`602`）。`parse.rs` 全文对 `input[24..44]` **零处按地址读**。块靠相邻性找（`padded_implicit_end` `527-539` 跳 0/7 填充；`602` 的 `explicit_start = actual_impl_len + memb_bytes_len` 是「成员在前显式在后」的第二重假设） | 〖高〗**静默少读**。相邻性够不到 46 566 个块 / 40 263 条记录；「跳填充继续找」在 36 106 条记录上接到同元素的后来副本。**注意量的归属**：这批数是在**已删 e3d-io 那版读法**上量的，旧栈机制同源但 2KB 窗口会让两个方向的数都变，**必须由旧栈自己的探针重量**（见 P1b） |
| **D4-5** | **块头布局与续接地址**（2026-08-30 新增） | 块头 **20 字节**：`[kind:u16][words:u16][owner.w0][owner.w1][cont_pgno][cont_packed]`，**payload 恒从 +20 起**；`cont_*` 两字全零表示不续接。语料 158 526 个声明续接的块 100% 落在本元素的块上；另有 5 444 个块压在页边界却不声明续接 | 分两套不一致的口径：成员侧 `get_merged_data`（`parse.rs:3177-3206`）payload 从 **+20** ✓ 但续接靠扫紧邻的 `00 00 00 07 00 02`（`3190`）✗；显式侧 `collect_explicit_segmented_payload`（`902-949`）`MEMBERS_BASE_PAYLOAD_OFFSET = 12`（`907`）——**把两个续接字当属性流字节读了**，于是逼出两层兜底：「drain 8 还是 drain 12」自适应（`875-891`）与 `has_unfinished_packed_expression_entry`（`953-977`） | 〖高〗证据文件里 BRAN 24383/85432 的幽灵成员 18010/8193、「payload 起点 0 和 8 都试一遍」，对应的就是这几行。5 444 个「压边界不续接」的块是相邻性会**误接**的地方 |
| **D4-6** | **块头 payload 起点三方不一致**（2026-08-30 新增，此前未记账） | e3d-io 实测 **+20**（`old/vendor/e3d-io/src/record/block.rs:117-133`） | 旧栈成员侧 **+20**、显式侧 **+12**；engine v2 **+16**（`db4/explicit_attrs.rs:47-59`：`hash@4..8`、`self_ref@8..16`、`payload@16`，即把 e3d-io 口径下的 `owner.w0` 叫 hash、把 `owner.w1 + cont_pgno` 当 self_ref）；engine v2 写侧同款（`db4/element.rs:107-127`） | 〖高〗**转调 engine v2 等于用第三种口径换掉一个对了一半的口径**。P4 决策前必须先裁这一格 |
| D4-2 | 属性解码的 schema 来源 | noun 模板从 `%AVEVA_DESIGN_EXE%/<db>vir.dat` 官方 schema 文件加载（GALFE，511+1 链式读，ida-3.1 §14.7），槽位偏移由 attlib ATGTDF 全局位置决定 | 手工维护的 info 字典（`get_default_pdms_db_info()`），f32/f64 布局靠「末属性偏移+步长 vs impl_len」**启发式猜**（`parse.rs:611-625`），未知 noun 直接 `UnknownNoun` 拒解 | 〖高〗但**归 engine v2 D 线管**（specs/034 / e3d31-attribute-parsing），旧栈不再投入，只记录 |
| D4-3 | 元素使用语义（noun 位表 / members 三模 / climb / significant_owner / CE 栈） | Core3D 层 | 旧栈不承担 | 引 `specs/034-core3d-semantics/`，**不在本计划** |

### db5 · 库级

| # | 维度 | core.dll 3.1 | 旧版 pdms-io | 等级 |
|---|---|---|---|---|
| D5-1 | open 语义 | `db_open` 216B per-DB 状态、模板表匹配、版本比对（ida-3.1 §8） | 打开=句柄+**急扫全部会话链**（每会话一次 2KB 读）；engine v2 相同姿势 | 〖低〗只读场景可行，不追 |
| D5-2 | C API / dispatcher / 错误码体系 | `db_*` → dispatcher（命令名表、命令历史、错误码 534/659/…） | anyhow 直抛中文错误串，无错误码映射 | 〖低〗对拍需要时再映射，不追 |
| D5-3 | mark / refresh / compact | 有 | 无 | 写侧冻结（ADR-055 Q8），**不追** |

### 旧栈独有资产（差异矩阵之外，改造时必须保住）

1. `DabaconSnapshot`（`snapshot.rs`）：稳定文件身份（volume+file_index）、4 次稳定捕获、冻结前缀读、
   `open_verified*` 世代证明——core.dll 靠文件锁与进程内状态，根本没有「文件被原子替换」这个威胁模型。
2. `session_index_diff` 双根 COW 差分 + `net_window`：core.dll 读侧没有对应能力。这是增量引擎地基，
   动 db3 口径时它是最大受益者也是最大回归面。
3. 12 项异常记账（`WalkStats`）：真实库病理的证据面。改条目计数口径后**保留记账**，用归零来证明修对了。

## 2. 开发计划

原则：先修环境（P0），再造尺子（P1），证据齐了才动语义（P3/P4 有决策门）；一律 fail loud 不回落；
改 Rust 跑 `cargo fmt` + `cargo check`；vendor 三仓改动走升 rev 流程，不带本地 patch 推 main。

### P0 · 环境修复（阻断项，半天）

- 恢复 `d:\work\plant-code\pdms-io-fork-engine-v2` 工作副本（从 `pdms-io.git` 检出，rev 对齐
  `old-parse-pdms-db` Cargo.toml 钉的版本或工作 HEAD `348d187`），或按纪律注释第四路 patch 并把
  `old-parse-pdms-db` 退回到 `13a17e1` 兼容 rev——**二选一由决策人拍板，默认前者**（vendored
  `paged.rs` 已消费新 API，退版本会连带退功能）。
- 验收：`cargo check` 通过；`git -C ../vendor/old-parse-pdms-db status` 干净。

### P1 · 对拍尺子：legacy↔v2 读取对拍探针（1~2 天）

> **2026-08-30 已完成**。探针落在 `src/bin/legacy_v2_read_parity.rs`，批跑 ams000
> 语料 431 库全绿（错误 0、探针自检 0），证据见
> `docs/evidence/2026-08-30-legacy-v2-read-parity.md`。要点：活叶页尾部多读
> 779,558 条（168 库，D3-1 坐实）、纯幽灵键 8,807 个、抽样幽灵 35.6% 可被旧栈
> 点查够到（含 ams8000_0001 上 64 个）；点查位置不一致 0（D3-4 印证）；
> 键数与 e3d-io 429 库门 789,831 逐键吻合。与原方案的两点偏差：
> ① 探针进了 gen-model `src/bin/` 而非独立仓；② 逐页双口径解码取代
> 「两树各走各的再比键集」（自由走查降级为观察项，理由见证据 §6）。

没有尺子，P2~P4 每一步都是「我觉得对」。两条栈已在同一依赖图里，对拍零基建成本。

- 新增 `src/bin/legacy_v2_read_parity.rs`（gen-model，读侧只读，不连库）：对同一 dabacon 文件，
  同一 target sesno，逐项对比——
  1. 会话链（sesno 集合、每会话 index_root）；
  2. 索引全量叶条目：v2 `free_dwords` 口径 vs 旧栈零终止口径，输出**多读集合**（陈旧槽位命中）
     与**少读集合**（不应存在）；
  3. 点查抽样（每库 ≥1000 refno）：`(pgno, offset)` 全等；
  4. 页头声明统计：`key_dwords/value_dwords` 非 2+2 的页数、`page_size(0x34)` ≠ 512 字的文件数。
- 批跑本机全部真库（~490/1002 个 dabacon），产出
  `docs/evidence/2026-08-3x-legacy-v2-read-parity.md`：陈旧槽位命中率、宽度漂移率、页大小漂移率、
  点查不一致清单（预期为空，非空即 P3 的直接证据）。
- 验收：报告落档；每个非零差异都能归到 D3-1/D3-2/D3-4 之一，出现新类别即扩矩阵。

### P2 · 低风险硬化：fail loud，不改读取语义（1~2 天，可与 P1 并行）

全部是「把静默前提变成显式断言」，不改任何解析结果：

- `defines.rs::PdmsHeader` 补读 `0x30/0x34/0x38`；`PdmsIO::open` 断言 `page_size_words == 512`
  （×4 == `PAGE_SIZE`），不等**报错点名文件**，不回落——2048 是本机 490 库实测全对的前提，
  值得一条断言把它从「碰巧对」变成「验证过」。
- 会话页解析加 `page_type == 3` 校验（对齐 engine v2 `db2/session.rs:39`）；索引页加
  `page_type == 5` 校验（对齐 core.dll err 659 语义），deku 断言失败的错误信息带页号。
- extent 显式拒绝：`init_ses_range_map_from_header` 遇 `last_ses_extno > 1`、
  `filter_index_data`/搜索遇 `RefnoIndexPgId.ext_no > 1` 时报错点名文件与页号
  （与 `on_demand_db.rs` 已定的「多 extent 必须显式失败」姿势同源）。
- `stored_page_count(0x38) × page_size` vs 文件实长的守卫加进 `DabaconSnapshot::open`
  的稳定捕获（观察值记账，先警告不阻断，一个窗口期后升级）。
- 验收：新增纯函数单测（不连库）；P1 探针复跑无行为变化；那 17 个曾坑过 v2 探测器的文件在
  旧栈下仍读出正确 sesno（旧栈本就是当时的权威，这里是防回归）。

### P3 · db3 条目计数对齐 core.dll（语义修正，2~3 天，**P1 证据门**）

**门条件**：P1 报告显示零终止口径存在「多读」（陈旧槽位）或「少读」实例。多读已有 ams8000
实测记账佐证，预期开门。
**→ 2026-08-30 门已开（实测）**：P1 批跑 431 库测得多读 779,558 条/168 库、少读 0，
见 `docs/evidence/2026-08-30-legacy-v2-read-parity.md` §4。

- `IndexPageData` 解析改为 free_dwords 反推条目数 + 页头键值宽（0 回退 2+2、内部节点值宽恒 2），
  与 engine v2 `db3/index.rs` 同一公式；**过渡期双口径**：保留零终止读法作对照字段，
  差异计入 `WalkStats` 新增项 `stale_slots_excluded`，跑满一个 live 批次周期后摘除旧口径。
- 受益面收敛：`btree_search_optimized_recursive` 的去重层、`session_index_diff` 的
  `duplicate_*`/`out_of_range_*` 补偿——**保留记账不删逻辑**，用计数归零证明新口径正确，
  归不了零的类别就是新证据。
- 验收：`session_index_diff` 异常计数在新口径下归零或逐项可解释；
  `cargo test --locked --test db8000_two_delete_fixture / db_session_fixture_selfcheck /
  db8000_session_pairs / pdms_record_boundary` 四件套绿；
  `scripts\Run-LiveBatch.ps1` ams8000 批次绿并更新 live 台账（`docs/2026-08-12_live-test-ledger.md`）。

### P1b · 记录层的尺子（2026-08-30 新增，**db4 一切改动的前置**，1~2 天）

P1 造的尺子只量索引层：`src/bin/legacy_v2_read_parity.rs` 全部 1126 行里，`v2_walk`(317)、
`free_walk`(403)、`sample_entry`(453)、`read_raw_header`(462)、`process_file_inner`(481)
**零处读记录**。所以 db4 现在没有验收面，P4 改完无从证伪。

- 新增记录层探针（旧栈侧，只读）：对同一语料，逐个索引键读记录，输出——
  1. **地址够到、相邻性够不到**的块数（D4-4 的旧栈版本，不能直接搬 46 566）；
  2. **落在 2 KB 窗口之外**的块数（这是旧栈独有的，e3d-io 那版读法没有对应项）；
  3. **相邻性会误接**的记录数（块压在页边界却不声明续接，语料级 5 444 的旧栈版本）；
  4. 显式 payload 起点按 12 / 16 / 20 三种口径解出的属性条数差（D4-6）。
- 参照实现与对照基准：`old/vendor/e3d-io/src/record/{mod,block,explicit}.rs`（`522a252`）与
  它的门 `tests/record_l3_contract.rs`（429 库 **789 831 / 789 831**）。
- 验收：报告落 `docs/evidence/2026-08-3x-legacy-record-layer-gap.md`；每个非零差异归到
  D4-1/D4-4/D4-5/D4-6 之一，出现新类别即扩矩阵。

### P4 · db4 记录读取改为地址驱动（语义修正，**P1b 门**）

> **2026-08-30 重写。** 原文是「2 KB 平窗 → 页感知增长窗口，优先转调 engine v2
> `RecordReaderV2`」。记录层补证之后这条**整体作废**，原因是实读
> `pdms-io.git@13a17e1 crates/pdmsdb_engine_v2/src/db4/` 的结果：
> `record_reader.rs` `find_record_end`（`105-173`）**仍是搜索式判端**——跳 0/7 填充
> （`skip_padding_len` `175-186`、`extend_impl_len` `188-198`），再找 `00000000`+`00000007` 对
> （`128-133`）或不跟 `00 01/02` 的裸 `00000007`（`135-143`）；它读的唯一头字段是 `impl_len`（`111`）；
> `element.rs:3` 的 `ELEMENT_HEADER_WORDS = 6` 说明**它的头模型里根本没有那两个地址槽**；
> `advance_over_segments`（`200-217`）与 `explicit_attrs.rs`（`62-81`）的续接同样靠相邻性；
> 窗口 16 KiB→1 MiB（`9-10`），超限报 `record 超出上限 1048576B`（`57-61`）——
> 就是「把完好记录报成截断」那一类，只是阈值更大；`read_window` `96` 行硬编 `ext_no: 1`。
> **换过去只是把同一缺陷放到更大的窗口里，还附送一个第三种块头口径（D4-6）。**

**门条件**：P1b 报告在旧栈上量出①②③任一非零。（②几乎必然非零——2 KB 窗口是硬边界。）

- **改法（分层）**：记录装配**上移到 `pdms_io`**（它才有文件句柄），装配出一段**连续字节**
  再交给现有的 `parse_raw_ele_data(&[u8])`。不要给 `parse_pdms_db` 加 `PageSource` trait：
  依赖方向是 `pdms_io → parse_pdms_db`，反过来吃 I/O 要穿过 4 个入口，而
  `parse_ele_membs`/`parse_ele_children` 的调用方手上只有字节切片。e3d-io 也是这么切的
  （`record::read_record` 交连续字节，`record::explicit::parse_tail` 在连续缓冲上走查）。
- **`old-pdms-io/src/io.rs`**：`read_raw_element_record`（`3047-3053`）与 `parse_element`
  （`3002-3006`）两份平窗读合并成一处地址驱动装配；`raw_element_payload`（`1432-1438`）退役；
  `PdmsIO`（`1402-1430`）加**数据页缓存**（今天只有 `index_page_cache` @`1425`，数据页每次直读磁盘，
  地址驱动会反复取同一页）——照 `read_index_data`（`3283-3296`）的样子；
  失败一律结构化报错（指空 / 类型错 / 成环 / 越页 / 声明续接接不上），**不得降级成「属性少几条」**。
- **`old-parse-pdms-db/src/parse.rs`**：`padded_implicit_end`（`527-539`）、`get_merged_data`
  （`3177-3206`）、`collect_explict_data`（`784-898`）、`collect_explicit_segmented_payload`
  （`902-949`）、`has_unfinished_packed_expression_entry`（`953-977`）、
  `collect_explict_data_legacy`（`981-1038`）、`take_off_007_explicit`（`1040-1056`）
  **整体删除**（约 306 行），换成「按块自报 kind 分派的一次走查」（约 110 行）；
  `parse_ele_membs`（`438-473`）、`parse_ele_children`（`485-524`）、
  `parse_raw_ele_data_with_info`（`580-604`）三处原地重写（约 101 行）。
  顺带修掉今天连「一个元素有两个成员块」都处理不了的问题。
- **`old-pdms-io/src/snapshot.rs`**：跨页取页必须受**冻结前缀长度**约束——`io.rs:3050`
  今天是裸 `seek + read_exact`，绕过了 `DabaconSnapshot`。
- **调用点**：`net_window.rs`（`217`/`454`/`458`）、`session_index_diff.rs`（`586`）
  今天把解析失败当「跳过 / warn」，新错误要上浮，不得吞。
- 起始页页型校验（5/7）随手补上（D1-3 收尾）。
- 造跨页大 members fixture 与「块不相邻」fixture（真库提取，脱敏进 `tests/fixtures/`；
  语料里最干净的病例是 `ams5100_0001` 的 ROOM_NO 定义 **13292/122**，头第 6/7 字写着 `(60, 0x2001)`）。
- **验收**：P1b 的①②③在新读法下归零；fixture 测试绿；`pdms_record_boundary` 扩展用例绿
  （现有 3 条建立在「168 字节末尾填充即记录尾」上，前提变了，要一并改）；
  net_window live 复跑绿并更新 live 台账。
- **改动量参考**（供排期，含上面全部条目）：6 个文件、删约 341 行、增约 385 行、原地重写约 101 行、
  **7 处结构性改动**；逐项拆解见姊妹计划
  `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md` §9.3。**该节同时给出一条成本更低的
  替代路线（转调 e3d-io 已过门的记录层，只留约 110 行）**，选哪条属路线决策，见本文 §5 决策点 5。

### 验证项（不改代码，出证据）

- **V1** 叶值字解码互证：用 pdms-io 仓已有的 `core_dll_oracle` FFI 通道，对同一叶页比对
  `packed>>12/&0xFFF`（文件态）与 ida-3.1 §5 `&0x1FFF/>>13`（内存态）两种口径，确认后者是
  core.dll 搜索结果的重编码、修订 ida-3.1-structures.md §5 的措辞。归 pdms-io 仓执行。
- **V2** flag 语义（D3-5）：P1 探针顺带输出 flag 直方图按「点查可达/不可达」分层，
  为「认领扫描 flag==1 vs 点查无视 flag」的口径分裂找证据。

## 3. Non-Goals

- 属性字典换 `*vir.dat` noun 模板、ATGTDF 位置解码（D4-2）——engine v2 / specs-034 地盘。
- Core3D 元素语义（D4-3）、CE 导航栈、写侧（mark/refresh/compact/writeback）。
- 多 extent 的**实现**（只做显式拒绝；实现排 engine v2 的 goals/e3d31-multi-extent）。
- 错误码体系映射（D5-2）、db5 dispatcher 仿真。
- 把净窗口/会话差分迁移到 engine v2——那是 ADR-053 direct 线收口之后的独立决策，本计划只保证
  旧栈在其存续期内读得对。

## 4. 风险

| # | 风险 | 对策 |
|---|---|---|
| R1 | P3 改条目计数后，净窗口在某些库上结果变化（少了陈旧槽位捞出的幽灵条目） | 双口径过渡期 + 异常记账归零证明 + live 批次全绿才摘旧口径；变化本身就是修复的证明，逐库留 evidence |
| R2 | P4 转调 v2 RecordReader 引入两栈行为耦合 | 转调只在字节读取层，判端结果用旧解析复核；fixture 钉死双栈一致 |
| R3 | 2048 页大小断言在未知库型上误伤 | 断言信息点名文件与 0x34 实值，出现即是 D1-1 的真实触发案例，按 fail-loud 原则这正是想要的 |
| R4 | vendor 三仓 rev 漂移（改 vendored 代码但忘了升 rev） | 沿用 pre-push 守卫 + `Toggle-LocalDeps.ps1 -Status` 检查，验收清单里加一条 |
| R5 | P1 批跑 490 库耗时 | 只读、可并行（rayon per-file）、可断点续跑；探针带 `--limit` 抽样档 |
| R6 | P4 改地址驱动后，某些元素**多出**今天读不到的属性与成员（D4-4 的 46 566 个块那一类） | 这是修复的证明不是回归，但会实打实改变 `model_impact` 分类与 `member_alive_at` 结果：P1b 先出「改前 vs 改后」的属性/成员差异清单逐库留 evidence，再跑 live 批次；差异非零的库单独列出，不允许一句「符合预期」带过 |
| R7 | 把「量的归属」搞混：拿 e3d-io 那版读法上量的 46 566 / 36 106 / 1 033 当旧栈的数 | P1b 的存在就是为了避免这件事。文档里凡引用这批数必须标明**是在哪一版读法上量的**；旧栈的数出来之前，D4-4/D4-5 的「量」列一律写「同机制，未在旧栈上单独量过」 |

## 5. 决策点（plannotator 审阅时请重点批注）

1. **P0 二选一**：恢复 engine-v2 工作副本（原默认） vs 注释 patch 退 rev。
   → **2026-08-30 现状已是后者**（`Cargo.toml:229-230` 注释态，工作副本目录不存在）。
   请确认「就这样」还是仍要恢复；确认前先核 `paged.rs` 有没有在用工作副本独有 API。
2. **P3/P4 的门**：接受「P1 / P1b 证据开门」的节奏，还是直接排期？
3. ~~**P4 转调 v2** 是否可接受~~ → **2026-08-30 撤回**。实读 `13a17e1` 的
   `db4/record_reader.rs` / `element.rs` / `explicit_attrs.rs` 后确认：`RecordReaderV2` 是同一形状的
   搜索式读法（`find_record_end` `105-173`）、头模型只有 6 字（`element.rs:3`，没有地址槽）、
   续接靠相邻性（`200-217`）、块头 payload 起点是第三种口径（D4-6）。**这条不再是候选**，
   P4 已按地址驱动重写。
4. flag==1 认领扫描口径（D3-5）现阶段只记账不裁决，是否同意。
5. **【新】记录层走哪条路**（本计划最大的未决项）：
   - **B · 纯就地硬化**：按重写后的 P4 在旧栈里实现地址驱动，6 个文件、增约 385 行、7 处结构性改动，
     且必须先做 P1b 造门；
   - **C · 分层收口**：`PdmsIO::read_raw_element_record` 转调 `old/vendor/e3d-io` 的
     `record::read_record`（已过 429 库 789 831/789 831 门），旧栈只留约 110 行的 kind 分派走查，
     省掉 `io.rs` 那约 210 行装配。代价是新依赖边 `pdms_io → e3d_io`，动工前须核 e3d-io 的依赖面
     与 `PageCache` 能否接受外部句柄/冻结前缀。
   两条的完整对照与推荐（推荐 C）在姊妹计划
   `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md` §9.4/§9.5。**本计划不代拍**，
   选定后回填到 P4。
6. **【新】D4-6 块头 payload 起点**：旧栈 +20/+12、engine v2 +16、e3d-io 实测 +20。
   建议直接采 e3d-io 口径（它是 438 库全量统计得出、且有门），但这一格属于「改解码结果」，
   请明确拍板而不是随 P4 顺手带过。
