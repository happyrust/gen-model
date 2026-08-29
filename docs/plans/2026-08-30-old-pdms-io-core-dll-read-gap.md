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
> `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md`（姊妹计划：clean 重写路线）。

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
> 实测 AoS 恰恰说明**本计划「就地硬化」的路线更站得住**（旧栈读模型本就对，只需修 count/stride/extent）。
> 详见该证据文件与姊妹计划顶部订正框。

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

## 1. 逐层差异矩阵

标注：〖高〗直接影响读取正确性；〖中〗有触发条件或有补偿层；〖低〗行为等价性问题。
每条给证据位置，可复核。

### db0/db1 · 文件与页

| # | 维度 | core.dll 3.1 | 旧版 pdms-io | 等级 |
|---|---|---|---|---|
| D1-1 | 页大小来源 | 头部 `0x34` 按 **4 字节字**存页大小（512 字 → 2048 字节），运行时从头部取（ida-3.1 §1、§13.5） | `PAGE_SIZE = 0x800` 编译期硬编码（`defines.rs:14`），`PdmsHeader` 只解析到 `0x2C`，**根本不读 `0x34`**，也无任何断言 | 〖高〗静默前提 |
| D1-2 | 页读取健壮性 | `FHDBRN` 失败（error 11）→ `SYWAIT 0.5s` 重试 → `FHSWIT` 关闭/重开切模式再试 → `FLFINI ABORT`（ida-3.1 §10）；页缓存读头页+目标页 | 裸 `seek + read_exact`，零重试（`io.rs:1442`）；元素页每次直读磁盘无缓存，仅索引页/会话页有缓存 | 〖中〗并发写撕裂由 `DabaconSnapshot` 的 4 次稳定捕获在**打开时刻**补偿，打开后无防护 |
| D1-3 | 页头校验 | 页型/type_id 入口硬校验：表页搜索 `*page != 5` 直接置错 659（engine v2 `db3/index.rs:8-10` 引证）；数据页 `0x7434F9`、索引页 `0xCC47DF` 常量比对 | 索引页仅 deku `assert_eq noun==0xCC47DF`（**不看 page_type**）；会话页、元素页完全无校验，坏页号会被硬解成垃圾数据 | 〖中〗 |
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
| D4-1 | **记录读取窗口** | 页感知窗口 16KB 起、×2 增长至 1MB；起始页页型必须 5/7；结构化判端（impl_len + 0/7 填充 + `00 01/02` 块 + **`00 00 00 07 00 01/02 len` 跨页续段协议**）（engine v2 `db4/record_reader.rs` 全文） | `read_raw_element_record` 固定 **2KB 平面窗口**（`io.rs:3047-3053`），无页型校验；段合并 `get_merged_data`（同一续段协议，但只在窗口内）；显式属性流靠 `collect_explict_data` **启发式跳过嵌入的索引页头** + `MAX_RESYNC=64` 重同步（`parse.rs:784-830`），跳过量不记账 | 〖高〗从记录起点算 >2KB 的记录（大 members / 大显式区）**静默截断**：children 不全 → `member_alive_at` 误判 Deleted 的通路存在。`tests/pdms_record_boundary.rs` 只钉了「不越界读」，没钉「跨页完整性」 |
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

### P4 · db4 记录窗口对齐 core.dll（语义修正，2~3 天，**P1/实锤门**）

**门条件**：真库找到「从记录起点到记录尾 >2KB」的记录实例（大 members/大显式区），
或对拍出现 children 截断。找不到实例则只做防御性断言版（窗口读满 2KB 仍未判端时**报错**而非
静默截断），全量窗口改造降级为 backlog。

- `read_raw_element_record` 从固定 2KB 改为页感知增长窗口；**优先转调 engine v2
  `RecordReaderV2`**（两栈同图，避免第二实现——对齐 K5「语义层不复制解码逻辑」精神），
  仅在依赖方向不允许时才在旧栈内复刻 find_record_end。
- 起始页页型校验（5/7）随手补上（D1-3 收尾）。
- 造跨页大 members fixture（真库提取，脱敏进 `tests/fixtures/`），钉死
  `member_alive_at`/`parse_ele_children` 在该 fixture 上与 v2 读数一致的回归测试。
- 验收：fixture 测试绿；`pdms_record_boundary` 扩展用例绿；net_window live 复跑绿。

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

## 5. 决策点（plannotator 审阅时请重点批注）

1. **P0 二选一**：恢复 engine-v2 工作副本（默认） vs 注释 patch 退 rev。
2. **P3/P4 的门**：接受「P1 证据开门」的节奏，还是直接排期？
3. **P4 转调 v2** 是否可接受（旧栈引 v2 的记录读取，依赖方向 pdms_io → parse_pdms_db → engine_v2 已存在，无新边）。
4. flag==1 认领扫描口径（D3-5）现阶段只记账不裁决，是否同意。
