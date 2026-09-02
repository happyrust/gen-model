# e3d-io：与 core.dll 同构的 dabacon 直读库（重写方案）

> 状态：**待裁决**。本文是 grill 过程的中间产物，Q1–Q3 已拍板；Q6 已由 IDA 补证收敛（见下），
> Q5/Q7/Q8/Q9 仍未决。**路线抉择（§9）已于 2026-08-30 二次重估，结论从「降级为可选」改为
> 「推荐分层收口」——记录层新证据推翻了上一次重估的前提。**
> 位置：新库落在 `D:\work\plant-code\old\vendor\e3d-io`。
> 关联：ADR-055（pdms-io v2 语义分层）、ADR-053（direct 模式生成读）、
> `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（core.dll live 逆向）、
> `docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`（**本方案的 AoS/页头/H1 指令级补证 + 订正**）、
> `docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`（**记录层自寻址实测，§9 二次重估的依据**）、
> `docs/plans/2026-08-30-old-pdms-io-core-dll-read-gap.md`（**姊妹计划：就地硬化 old-pdms-io，本方案的替代路线**）、
> `docs/specs/core3d-partial-update-conformance.md`（R0–R29）。
>
> **⚠️ 2026-08-30 IDA 补证摘要 —— 含一处推翻本方案头号前提的订正**（详见上列 AoS 证据文件，复用实例 `idalib-48392`）：
> ① **Q6 定案 AoS，不是 SoA**——读 `sub_5AFFCB0` 指令流：条目步进 = `游标 + key_dwords + value_dwords`
>   （`0x5affe2f`/`0x5affe31`），值紧跟本条目键之后（`0x5b007bc`）。结点是 **AoS**（`[键][值]` 定长条目）。
>   「键按 dword 连续比较」对「2-dword 键的 AoS」同样成立，不能据此判 SoA——2026-08-13 报告 C2「SoA」是**误判**。
> ② **本方案第 1 节的头号前提（「core.dll 是 SoA、现存四份实现全错」）不成立。** 实测 AoS 与
>   `pdmsdb_engine_v2`（页头 `key_dwords/value_dwords`，已过 FFI 对拍）、`old-pdms-io`（定长 4 字步长）
>   的建模**一致**；`old-pdms-io` 与 engine-v2 的 `packed` 位拆其实**相同**（`offset=packed>>12`、`flag=&0xFFF`），
>   并非「四种不相容」。真正的读取偏差在**条目计数**（scan-to-zero）与**硬编步长**，**不在布局**。
> ③ 结点页头 = **7 dword / 28 字节**：`[0]`类型(5)@0x00 `[1]`表id@0x04 `[2]`level(≤0叶)@0x08
>   `[3]`key_dwords@0x0C `[4]`value_dwords(内部=2)@0x10 `[6]`free_dwords@0x18；条目区自 `[7]` 起，AoS。
> ④ **H1 得证**：条目数 = `(容量 unk_6453DC4[0] − 7 − free_dwords) / (key_dwords+value_dwords)`——
>   **engine-v2 的 free_dwords 口径正确**，`old-pdms-io` 的 scan-to-zero 会多读陈旧槽位。
> ⑤ extent 全链路是一等寻址维（页地址恒 `(pgno, extno)`）；值第二字 `packed` 位拆用 engine-v2 FFI 口径。
>
> **对本方案的影响**：头号「重写」理由（布局错）被证伪后，天平一度明显偏向「就地硬化」。
> **但那次重估的另一半前提当天就被推翻了**——见下框。

> **⚠️ 2026-08-30 记录层补证 —— 天平第二次翻转**（`docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`，
> 438 库 / 2 923 428 条记录全量统计）：
> ⑥ **记录写着它自己各部分在哪**：头第 6/7 字是显式属性流地址、第 8/9 字是成员表地址、
>   块尾是续接地址，都用索引叶值那套 `(page_no, packed)` 编码。这些地址够到的 3 195 015 个块里
>   指空 0 个、类型错 0 个；相邻性够不到其中 46 566 个，且「跳填充继续找」在 36 106 条记录上
>   把同一元素的**后来副本**接了上去。
> ⑦ 由此，上一版说的「真实偏差只有条目计数 / 硬编步长 / extent 三条，且姊妹计划 P0–P4 已逐条覆盖」
>   **两半都不成立**：记录层还有两条，**P0–P4 一条都没盖到**；P4 拟转调的 engine-v2
>   `RecordReaderV2` 本身就是同一形状的搜索式读法（逐行核实见 §9.2）。
>
> **裁决人请直接看末尾 §9**，该节已按本条整体改写，含带行号的改动量估算与三选一建议。

---

## 0. 一句话

把 dabacon 文件的**页 / 会话 / B+ 树 / 记录 / 属性**五层，按 core.dll 的真实读法重写一遍，
只读，独立 crate，不沿用现存任何一份索引解析实现。

---

## 1. 为什么要重写：四份实现，读法不齐（但**没有原先以为的那么分裂**）

> **2026-08-30 订正**：本节原标题与论证是「四份实现对 `packed` 有四种互不相容拆法、且都不对」。
> IDA 实测后这个论证**部分证伪**：`old-pdms-io` 与 `pdmsdb_engine_v2` 的 `packed` 拆法**其实相同**
> （`offset = packed>>12`、`flag = packed & 0xFFF`，deku 的 `offset:20+flag:12` 就是同一件事的另一种写法），
> 且都正确建模了 **AoS** 布局；真正跑偏的是**已删 e3d-io**（用了内存态编码 `&0x1FFF/>>13`）与
> 2026-08-13 报告的「SoA」误判。下表保留原始对照，但 core.dll 行已按实测订正为 AoS。

仓里（含已删除的）先后有过四份 dabacon 索引解析。它们对索引条目里同一个 32 位 `packed`
字段的记录方式**并非全都相容**：

| 实现 | 对 `packed` 的拆法 | 记录位置公式 |
|---|---|---|
| `old-pdms-io` `RefnoDataLoc` | `offset:20` + `flag:12`（deku 位域） | `pgno * 0x800 + offset * 2` |
| `pdmsdb_engine_v2` `IndexEntry` | `offset_words = packed >> 12`；`flag = packed & 0xFFF` | `byte_offset = offset_words * 2` |
| 已删除的 `plant-code\e3d-io` | 先 `packed >> 12`，**再把结果拆成** `slot_offset = ow & 0x1FFF` + `slot_index = (ow >> 13) & 0xFFF` | — |
| core.dll（2026-08-30 指令级实测，级别「事实」；**订正 2026-08-13 §4.5 C2 的「SoA」误判**） | **AoS**：每条目 `[键 key_dwords][值 value_dwords]` 定长连续，步进 = 键宽+值宽（`0x5affe31`）。默认 2+2 | 与 `old-pdms-io`（定长 4 字）、engine-v2（页头键值宽）**一致**；`packed` 位拆 `offset=packed>>12`/`flag=&0xFFF` 两者相同 |

**订正后的真实分歧**：AoS 布局本身 `old-pdms-io`/engine-v2 都对；`packed` 拆法这两者相同、
只有已删 e3d-io 用错了内存态编码。**布局不是重写的理由**。剩下能站住的「没有判别力的测试」仍成立：

- `pdmsdb_engine_v2` 里看着最硬的 `real_db_index_pages_round_trip_byte_for_byte`，是
  「AoS 解码 → AoS 编码 → 比字节」。错的解码配上匹配的错编码，照样逐字节相等，**自证不了布局**。
- 唯一有语义强度的 `free_dwords_walk_of_live_tree_has_no_duplicate_refnos` 依赖
  `pdms-test-data/sam7200_0001`，**该目录当前不存在**，测试一直在静默跳过。

除布局外，还有三处同类分歧：

1. **条目数从哪来。** `old-pdms-io` 与已删 `e3d-io` 都是「扫到第一个全零字为止」；
   `pdmsdb_engine_v2` 按页头 `0x18` 的 `free_dwords` 反推
   （`count = (page_dwords - 7 - free_dwords) / stride`）。前两者把 `0x18` 命名为 `pfno` 并丢弃。
   **已裁（2026-08-30 IDA）：`pdmsdb_engine_v2` 对。** `sub_5AFFCB0` 用页头
   `[6]`（= `free_dwords`，字节偏移 `0x18` = dword 6）算条目区上界
   `容量 unk_6453DC4[0] − page[6]`，条目区起点固定 dword `7`，stride 由页头 `[3]/[4]` 定——
   与 engine-v2 的 `(page_dwords − 7 − free_dwords)/stride` 逐项对上。core.dll copy-on-write、
   不清理离开生效区的槽位（残留非零），所以「扫到零为止」必然多读残留槽位。**权威口径 = 页头驱动。**
2. **哨兵位置。** `0x80000001` 对：有的实现只看第一条（`get_start_page`），
   有的遍历全页找。逆向已证它是**页内键常量**、作键空间边界参与归并。
3. **多 extent。** 三份实现的结构体上都有 extent 号，但真正 seek 的那一行**从不使用它**
   —— `PageId{ext:2, page:N}` 会安静地读成 extent 1 的第 N 页并返回成功。
   **核内相反（2026-08-30 IDA）：extent 是一等寻址维**——全链路每个页地址都是 `(pgno, extno)`
   的 2-dword 对（begin 取根、内部结点子指针、页读取器 `sub_5AEE4E0` 入参无一例外），
   页地址从不压成单一页号。这正面支持 L0 的 `PageId{ext,page}`「取不到 extent 就 `Err`」。

> **命题 H1（IDA 已证方向，样本待复核）**：`old-pdms-io` `session_index_diff.rs` 里
> 记录的一整套「异常计数」——`duplicate_leaf_entries`、`duplicate_child_pointers`、
> `out_of_order_child_keys`、`out_of_range_leaf_entries`、`nonlive_leaf_entries`
> ——**不是文件格式的性质，而是「扫到零为止」多读出的残留槽位**。
> 该文件注释里那个实例（ams8000 上键 25843 的条目躺在覆盖 [7415, 7790) 的叶子里）
> 正是「读到了生效区以外」的典型征状。
>
> **2026-08-30 核内佐证**：`sub_5AFFCB0` 遍历结点时，条目数取自**页头字段** `[3]`（键宽/数）、
> `[4]`（数据宽），并以 `[6]`（水位）判本页是否走完（`page_pos < 容量 - page[6]`），**从不**扫零。
> core.dll copy-on-write、不清理离开生效区的槽位（残留非零），所以「扫到零为止」必然把残留读进来。
> → H1 的**机制**已由指令级坐实；剩下的是在真库上量出那批异常计数在改用页头驱动后归零（P0 样本复核）。
> `pdmsdb_engine_v2` 用页头反推计数**正确**：它的 `free_dwords@0x18` 就是页头 `[6]`（`0x18` 字节 = dword 6），
> `page_dwords` 就是全局页容量 `unk_6453DC4[0]`——`(page_dwords − 7 − free_dwords)/stride` 与核内逐项对上。
> 重写时把「容量」取运行期 `unk_6453DC4[0]`（随页大小变），别写死 512。

---

## 2. 已拍板（grill Q1–Q3）

| # | 决策点 | 结论 |
|---|---|---|
| Q2 | 新库与现存 e3d-io 的关系 | **干净新仓**，落 `old\vendor\e3d-io`；现存的弃用 |
| Q3 | 上边界切在哪 | **L0–L4**（页 I/O → 头/会话 → B+ 树 → 记录头 → 属性解码）。**L5 Core3D 元素语义不进** |
| Q3 | schema 从哪来 | 依赖现有 `e3d-attlib`，**不重写**；把它一并搬进 `old\vendor\e3d-attlib`，两库并排 |
| — | 写侧 | 冻结，只读（沿用 ADR-055 Q8） |

**L5 为什么必须拦在外面**：它的权威是 Core3D.dll 而非 core.dll（ADR-055 Q1 已按证据来源分层定权威），
且需要 1931 noun 的位表快照（带 `core_sha256` 版本绑定）。混进一个纯格式库，
以后任何一条行为对不上都说不清该查哪个 dll。

---

## 3. 环境事实（动工前必须先处理）

| 事实 | 影响 |
|---|---|
| `plant-code\e3d-io`（主仓）已删除，其 `.git` 一并消失 | 该仓所有 worktree 的**对象库随之消失**。`old\e3d-io-noun-descriptor` 与 `plant-code\e3d-io-noun-descriptor` 的 `.git` 都指向已死的 `D:/work/plant-code/e3d-io/.git/worktrees/…`，**git 历史不可恢复**；工作文件完好且两份逐字相同（src 各 33 个文件） |
| `old\e3d-io-noun-descriptor\Cargo.toml` 的 `e3d-attlib = { path = "../e3d-attlib" }` | 解析到 `old\e3d-attlib`，**不存在** → 该副本当前编不了。只有 `plant-code\e3d-attlib` 在（独立仓，git 完好） |
| `pdms-io-fork-engine-v2` 目录已不存在 | gen-model `Cargo.toml` 里那段手动 `[patch]` 仍然激活并指向它 → **gen-model 目前应当是编不过的** |
| `pdms-test-data` 不存在 | engine-v2 唯一有判别力的真库测试一直在静默跳过 |
| `ida-bridge` 可用（`C:\Users\dpc\.agents\tools\ida-bridge\.venv\Scripts\ida-bridge.exe`，v-）；`core.dll`(50,071,544 B) 与 `core.dll.i64`(421 MB) 都在本机 | 2026-08-13 报告里每一条 SQL 都能重跑，**新证据可现取** |

**动工第一步（无条件执行）**：把 `old\e3d-io-noun-descriptor` 与 `e3d-attlib` 搬进
`old\vendor\`，立刻 `git init` + initial commit 把当前状态钉死。之后无论怎么推倒重来，
都还能 `git diff` 回去看旧实现当时怎么读的。

---

## 4. 待裁决

### Q5 · L3 / L4 是接管现有代码，还是一起重写

现存那 33 个源文件按层分：

| 层 | 文件 | 行数 | 可信度 |
|---|---|---:|---|
| L0–L2 | `page/{io,cache,mod}` `index/mod` `session/mod` `meta/*` `refno` | ~700 | **低**。第 1 节列的四处分歧全在这里 |
| L3 | `record/{element,mod,explicit}` | ~800 | 中。正确性取决于 L2 给的 `(page, offset)` |
| L4 | `record/{atnlog,attrs,text,orientation,point_list,axis_spec,direction_spec,template,template_file,descriptor,catalogue_pml,catalogue_expr}` `noun_catalog` `uda_catalog` `attlib_metadata` `element_name` | ~5400 | **较高**。解错会直接读出垃圾值，可被属性值本身证伪 |
| 周边 | `engine` `tty` `bin/e3d-descriptor` `error` | ~2800 | — |

| 选项 | L0–L2 | L3–L4 |
|---|---|---|
| **A（推荐）** | 推倒重写 | 搬过来，逐个对新 L2 接口改适配，不重写逻辑 |
| B | 推倒重写 | 也推倒重写 |
| C | 推倒重写 | 第一轮不做，只交 L0–L2 |

推荐 A：要抛的「旧解析方式」实体在 L0–L2；L4 与页布局正交，且它自带证伪手段——
旧 README 的示例输出里 `NAME = Text(0xB851EB85)`、`UNIT = RealArray([2.12e-313, 1e-323])`
正是 L2 给错位置的症状。L0–L2 修对之后这些值应当自己就正了，**这反过来是一条很强的验收信号**；
重写 L4 就把这个信号弄没了。

> **2026-08-30 订正**：上表把 L0–L2 可信度标「低」的理由是「第 1 节四处分歧全在这里」。IDA 实测后，
> 这里的 AoS 布局与 `packed` 拆法 `old-pdms-io`/engine-v2 **本就对**（见 §1 订正框），L0–L2 的真实缺陷
> 收敛为**三条明确的、姊妹计划已定位到行号的 bug**：条目计数 scan-to-zero（应改 free_dwords）、
> 硬编 4 字步长（应读页头 `key_dwords/value_dwords`）、extent 无视（应显式拒绝或路由）。
> 这三条**不需要推倒重写**就能修（正是姊妹计划 P2/P3 的内容）。
>
> **2026-08-30 二次补注**：上面这句对 **L0–L2** 仍然成立，但它当时被当作「整条路线可以就地硬化」的
> 论据，那一步不成立——**L3 记录层另有两条同形状缺陷**（头地址槽、块续接地址），且姊妹计划
> P0–P4 一条未盖。索引层就地硬化与记录层就地硬化是两笔账，见末尾 §9。

### Q6 · SoA/AoS 怎么裁 —— **已裁：AoS（2026-08-30 IDA，指令级）**

原方案的 B（读 `sub_5AFFCB0` 指令流）**已执行完毕**，结论写进
`docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`：

- 决定性证据是**条目步进公式**：`0x5affe2f add edx,esi`（+游标）`0x5affe31 add edx,ecx`（+值宽）
  → 下一条目 = `游标 + key_dwords + value_dwords`；值取出起点 = `游标 + key_dwords`（`0x5b007bc`）。
  即每条目 `[键][值]` 定长连续——**AoS**，不是 SoA。
- 键归并循环（`0x5b00541`–`0x5b005cb`）确是「键按 dword 连续比较、指针每轮 `+4`、哨兵 `0x80000001`
  无符号特判」，但那是在比**当前条目的 2-dword 键字段**，对 AoS 同样成立——**这一段不能拿来判 SoA**
  （2026-08-13 报告 C2 与本方案前一版都在此处误判）。
- 结点页头 7 dword、条目区 AoS，字段含义见 §5「L0–L4 设计要点 · L2」。

**因此 L2 照 AoS 实现（页头驱动的 `key_dwords/value_dwords` 步长 + free_dwords 计数）。**
这与 engine-v2、`old-pdms-io` 的建模一致，不是新写法。唯一保留的纯文件探针（并入 P0）只剩：

1. 真库上确认叶 `value_dwords == 2` 且值第二字 `packed` 按 `offset=packed>>12 / flag=&0xFFF / byte=offset×2`
   解出的属性值正确（engine-v2 已过 FFI 对拍，此处是重写侧的独立复核）；
2. 异常计数在改用 free_dwords 口径后归零。

探针仍需一个真库样本——`pdms-test-data` 已不在，需指定替代（本机 E3D 工程库或 ams8000 归档）。
在闭合前按 R4 处置：**路由与存在性都不看 flag。**

### Q7 · 「对齐 core.dll」的验收口径

| 层 | 可选口径 |
|---|---|
| L0–L1 | 页大小与会话根：对 490 个真库文件全跑，**含那 17 个会坑探测器的文件**，`sesno` 与权威值逐个相等 |
| L2 | ① 点查 I/O 预算（冷缓存读页数 ≈ 树高）；② 全树枚举与点查的**存在性定义一致**（枚举出的每个 refno 都能被点查命中，反之亦然）；③ 双根差分与 `session_index_diff` 在同一窗口上结果相同 |
| L3–L4 | 与 E3D TTY 导出的属性值对拍（`docs/2026-08-26_e3d-tty-ams-agent-usage-guide.md` 那套） |

**已裁（2026-08-30）：设为硬门，且已实测通过。** L2 的 ②「点查与枚举必须给出同一个
存在性集合」写进 `old/vendor/e3d-io/tests/index_cursor_real.rs`：429 个真库、枚举出的
**789831** 个键逐个点查可达并落到同一条记录，枚举没给出的邻近键点查一律 `None`。
这道门在旧「扫到零为止」计数下不可能通过——凭空造出的条目没有任何下降路径能路由到，
所以它同时也是 H1 的第三个独立佐证。

### Q8 · 多 extent 排在哪

ADR-055 Q7 的量化结论是「本机 E3D3.1 的 1002 个 dabacon 文件里 0 个多 extent」，
所以排在后期、补齐前显式拒绝。但本方案的 L0 设计里 `PageId` 会带 `ext` 且 `read_into`
真的按它选句柄——**取不到 extent 就 `Err`，不静默读主文件**。
**核内佐证（2026-08-30 IDA）**：core.dll 全链路每个页地址都是 `(pgno, extno)` 的 2-dword 对
（begin 取根、内部结点子指针、`sub_5AEE4E0` 入参），页地址从不压成单一页号——`PageId{ext,page}` 与之同构，
方向确定。待定的只是**排期**：第一轮是「实现 extent 路由」还是「实现到报错为止」。

### Q9 · 命名与术语

新 crate 的 package name 与已弃用的重名（都叫 `e3d-io`）。旧副本删除还是改名归档？
另：本仓 `CONTEXT.md` 是术语表，新库的公开类型名（`PageId` / `RecordLoc` / `SessionRoot` …）
是否需要与它对齐。

---

## 5. L0–L4 设计要点

### L0 · 页 I/O（最薄的一层，不模仿 FORTRAN）

FORTRAN direct access 本身没有格式可言，`offset = (rec-1) * recl` 而已。能错的只有
**recl 的单位**与**记录号起点**。仓里已在前者上摔过：文件头 `0x34` 是按 4 字节**字**计的
页大小，被当成字节数，490 个真库文件里 17 个中招（`ams7329_0001` 读出 `sesno=0`，权威值 221）。

```rust
/// 物理页地址。core.dll 的地址就是 (extent, page) 二元组，不压成单一页号。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageId { pub ext: u16, pub page: u32 }

/// 页大小是 newtype，构造只有这两条路。
pub struct PageSize(u32);
impl PageSize {
    pub fn from_header_words(words: u32) -> Result<Self, IoError>; // ×4，越界即 Err
    pub fn words(self) -> u32;
    pub fn bytes(self) -> usize;
}

pub trait PageSource {
    fn page_size(&self) -> PageSize;
    fn read_into(&self, id: PageId, buf: &mut [u8]) -> Result<(), IoError>;
    fn stats(&self) -> IoStats;
}
```

七条硬约束：

1. `PageSize` 是 newtype，`words()` 与 `bytes()` 分开——把「字 vs 字节」钉到类型层。
2. **页大小不合法即 `Err`，不回落默认值。** 已删的 `PageReader::open` 正是 fail-open：
   `if bytes >= 128 && … is_power_of_two() { … } else { DEFAULT_PAGE_SIZE }`。
3. `read_into(&mut [u8])` 是唯一原语，不提供 `read_page() -> Vec<u8>`。
   现存实现每读一页分配一次，一次树下降就是树高次堆分配。
4. `PageId.ext` 必须在 `read_into` 里真的用来选句柄；没有该 extent 就
   `Err(MissingExtent { ext, expected_path })`。
5. 用 positioned read（Windows `FileExt::seek_read` / Unix `read_at`），不用 `seek` + `read_exact`。
   `read_into` 因此可以收 `&self`，为并发留门，也没有「文件指针被别人挪了」的隐患。
6. **不用 mmap**（至少第一轮）。dabacon 文件在 E3D 运行时会被追加写；mmap 上遇到截断/替换
   是 SIGBUS / SEH 异常而不是 `Err`。`old-pdms-io` 那套 `SnapshotToken`
   （volume serial + file index 身份守卫、四次重捕）的存在本身就说明「我们读的时候文件在变」是常态。
7. **计数器内建在这一层，且 prefetch 单独计。** engine-v2 把预读页混进了
   `physical_pages_read`，而索引计数又是拿它的差值算的——结果「点查读了几页」量不准。

**不抄的东西**：逆向报告量到 core.dll 的页缓存描述符是 28 字节/槽，页号字段 `& 0x3FFF`（14 位）、
有效位 `& 0x10000`。14 位页号封顶 16384 页，2 KB 页就是 32 MB——那是它内部缓存槽的编码限制，
不是文件格式事实。**对齐要对的是「文件怎么读」，不是「它内存里怎么摆」。** 这条界线后面每层都会碰到。

**retry**：不在页层重试。读到撕裂页时重试掩盖的是「你的快照不成立」；撕裂应上浮到快照层，
用文件身份 + 长度 + header 重捕。

### L1 · 头与会话链

- 文件头：`db_num`、`noun`、`latest_ses_pgno`、`ext_no`、页大小（`0x34`，单位为字）。
- 会话页 `page_type == 3`，携带 `sesno`、`last_ses_pgno/extno`、`end_pgno/extno`、
  **`index_root_pageno/extno`**、`claim_pageno/extno`、时间戳、机器名、注释。
- 索引是 copy-on-write：页一经写入不再改动，所以**每个会话页各自携带当时的索引根**，
  这正是双根差分能成立的前提。
- `open_at(path, sesno)` 与 `open(path)`（= latest）**必须是同一条实现**，
  后者只是前者取链尾。找不到会话 → `Err`，不是 `None`。

### L2 · B+ 树

**结点二进制布局（2026-08-30 IDA 指令级实测，级别「事实」）** —— 全部按 32 位 dword 索引：

| dword | 字段 | 说明 |
|---:|---|---|
| `[0]` | `page_type` | 索引「表页」恒 `5`；`sub_5B026C0`/`sub_5AFFCB0`/`sub_5AEE4E0` 硬校验，失败报 `"Page is not a table page (page is type %d)"` |
| `[1]` | `table_id` | 主索引 `13387743`；另一系统表 `7618377`（页读取器按此字面量校验） |
| `[2]` | `level` | `≤0` = 叶；`>0` = 内部。下降时子层级 = 父层级 − 1（硬校验，失败报 `"Expected page of level %d, but got level %d"`） |
| `[3]` | `key_stride` | 键元素宽度（dword）：`≥0` 定长；`<0` 变长（实宽 = 游标处长度前缀 `+1`） |
| `[4]` | `data_stride` | 数据元素宽度：`≥0` 定长；`<0` 变长；**内部结点恒 = 2**（子指针 `(pgno, extno)`） |
| `[5]` | —（本轮未定用途） | — |
| `[6]` | `free_dwords` | 本页剩余空闲 dword（字节偏移 `0x18`）；条目区上界 = 全局页容量 `unk_6453DC4[0]` − `[6]` |
| `[7]…` | 条目区 | **AoS**：`[键 key_dwords][值 value_dwords]` 定长条目连续排布；下一条目 = 游标 + `key_dwords` + `value_dwords`（`0x5affe31`），值紧跟本条目键之后（`0x5b007bc`）。默认叶 2+2 = 4 字/条目 |

- **条目数由页头 free_dwords 反推，不扫零**：`条目数 = (容量 unk_6453DC4[0] − 7 − free_dwords) / (key_dwords + value_dwords)`
  （见 §1 分歧点 1 与 H1）。这是与旧「扫到第一个全零字」实现的根本分歧，**必须照页头实现**；
  与 engine-v2 `db3/index.rs` 同一公式。
- 索引页 `page_type == 5`；表 id `13387743`。
  > 顺带修一处文档错误：2026-08-13 报告 §9 把它写成 `13387743(0xCC441F)` 与 `7618377(0x743F89)`，
  > 两个十六进制都不对。`13387743 = 0xCC47DF`（与代码常量 `INDEX_PAGE_NOUN` 一致）、
  > `7618377 = 0x743F49`。十进制是对的，十六进制是笔误（核内字面量比较证实十进制值）。
- 下降时校验层级递减（子 = 父 − 1；leaf `level ≤ 0`），并带 visited 环保护。
  **现存的生产点查路径两者都没有**，只有差分路径有。
- 键 = refno，**无符号比较**；哨兵 `0x80000001` 单独特判、当作键空间**最小值**（最左边界）——
  它作无符号约 2.15G（几乎最大），语义却是最小，重写时不能只按无符号序排。非叶层最左是子树边界、叶层非数据。
- 叶值 = `value_dwords`（默认 2）= `(pgno, packed)`；`packed = offset<<12 | flag`
  （`offset = packed>>12`、`flag = packed & 0xFFF`、页内字节 = `offset×2`；engine-v2 已过 FFI 对拍，
  `old-pdms-io` 同口径）。比较器 `dataOnFirst`/`dataOnSecond` 各拷 2 dword 作不透明整字比较。
- 同键重复子指针：core.dll 的规则是**不看 flag、同键首见者胜**。
- **flag 不参与路由，也不参与存在性判定**（逆向已证变更检测全链路——页取 + 双根归并 + begin
  ——都不读 flag）。`flag` 是叶值第二字 `packed` 的低 12 位，语义未完全逆向；重写侧只做直方图观测，
  **不据此发明存在性/可见性语义**（R4）。
- 双根差分 `compare(old_root, new_root)`：键在旧根不在新根 → deleted；仅新根 → inserted；
  两边都在但记录位置不同 → modified；位置相同 → 未动。
  **删除判据是键集差，不是墓碑位，也不是 `owner.children` 包含性**（`elementsDeletedBetween`
  的 callee 全集里没有任何 owner/children/primaryList 调用）。
  可用 copy-on-write 性质剪枝：页号 ≤ base 会话 `end_pgno` 的子树在两棵树上是同一个页号，直接跳过。

### L3 · 记录头

> **2026-08-30 实测订正（`docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`）**：
> 记录**自己写着**它各部分在哪，不需要靠相邻性推、更不需要找结束标记。头是 **11 个 dword**：

| dword | 字段 |
|---:|---|
| `[0]` | 隐式区长度（dword 计，含本头） |
| `[1..2]` | 本元素 RefNo |
| `[3]` | noun hash |
| `[4..5]` | owner RefNo |
| `[6..7]` | **显式属性流起始地址** `(page_no, packed)` |
| `[8..9]` | **成员表起始地址** `(page_no, packed)` |
| `[10]` | 存储格式字（`layout_from_format_word` 用的就是它） |
| `[11]…` | 属性存储区 |

尾块布局 **20 字节头**：`[kind:u16][words:u16][owner.w0][owner.w1][cont_pgno][cont_packed]`，
payload 自 +20 起。`kind` 2 = 成员、1 = 显式。`cont_*` 两字是**续接地址**（同一套编码），
为零表示不续接。语料统计见证据文件；`(page_no, packed)` 的 `packed` 低 12 位恒为 1，
与索引叶值同口径，**不参与寻址**。

refno / noun hash / owner / members 块 / 隐式区 / 显式区分块。跨页记录靠**声明的地址**
支持——隐式区与单个块都不跨页（语料 0 例），跨页发生在块与块之间，由 `cont_*` 指路。
失败一律结构化错误，不得降级为「不存在」，也不得降级为「属性少几条」。

### L4 · 属性解码

schema 全走 `e3d-attlib`（ATGTIX 索引、ATGTDF 类型定义、ATGTSX noun-属性系列、
EXMAP 分派表复现自 `sub_51D368F`、基-27 哈希对过 `PDMS_Hash::String @ 0x588cb87`、
6377 条系统属性名抓自 core.dll 的 `?ATT_<NAME>@@3...` 全局符号）。
本层只做「按 hash 取值 + 类型分派 + 数组 + SYNO 链」。

**字节序**：文件是大端（FORTRAN 遗留）；`f64` 是 **Fortran word-swap**——
两个大端 32 位字按 `(lo, hi)` 顺序存，不是标准 IEEE 754 BE。

---

## 6. 阶段与门

| 阶段 | 内容 | 门 |
|---|---|---|
| **P-1** | 搬库 + `git init` 存档 + 修 path 依赖 + `cargo check` 通过 | 两个库都能编；gen-model 那段悬空 `[patch]` 一并处理 |
| **P0** | AoS（Q6）与 H1 的**机制**已由 IDA 定案（`docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`）；本阶段只剩真库样本复核：叶 `value_dwords==2` + `packed` 位拆解出的属性值正确、异常计数改 free_dwords 口径后归零 | 探针结论回填 `docs/evidence/`，标「已证实 / 高可信推断 / 待样本验证」 |
| **P1** | L0 页 I/O + L1 头/会话 | **已完成**（`aaf14a5`）。429 个真库不给 hint 全部读出 2048 字节/页与正确 `sesno`；`page_l0_contract` 7 条、`session_l1_contract` 11 条 |
| **P2** | L2 B+ 树点查 + 顺序游标 | **已完成**（`6be8847` 解码器、`a9ade97` 游标）。两道门都设成硬门并通过：429 个真库 789831 个键点查与枚举同集合；单页缓存下冷点查读页数**恰好等于**树高（语料最深树高 3）。`index_page_decode` 5+1 条、`index_cursor_real` 9+1 条 |
| **P3** | L2 双根差分 | **已完成**（`src/index/diff.rs` + `tests/index_diff_real.rs`）。**门已重定**：原定「与 `session_index_diff` 结果相同」不可用——那份差分器的异常计数正是 scan-to-zero 的产物（§1 分歧点 1），拿它当基准等于把旧 bug 当参考答案。改为两道自洽门：① 429 个真库、428 对相邻会话，差分结果与「两棵树各自全量枚举后取集合差」**逐键相等**（171 559 条变化），剪枝档与不剪枝档也逐键相等；② 页预算是**精确门**而非比例门——读页数恰好等于两棵树页集合的**对称差**，多一页都不行（不剪枝档恰好等于并集）。总量 5 423 页 vs 全量归并 14 795 页。差分无自己的下降：复用 `cursor::read_node` / `cursor::located` / `index::cmp_refno`，一次都没调 `choose_child`，并新增源码顺序断言「把页当结点这一步只能有一处实现」 |
| **P4** | L3/L4 接管（Q5 选 A） | **已完成**（`522a252` 记录层、`357512d` 查找路径）。硬门：429 个真库 **789831 / 789831** 个索引键全部读出记录且自报 RefNo 一致；交出的字节数 = 解析器消费的字节数。`record_l3_contract` 4 条、`sampled_elements_resolve` 1 条（211 个真元素）。L4 未重写，按 A 方案沿用，`dictionary_uda_real` 等既有对拍全绿 |
| **P5** | gen-model 接入 | 另开计划，不在本文范围 |

---

## 7. 风险

| # | 风险 | 对策 |
|---|---|---|
| R1 | ~~SoA/AoS 两种读法在样本上都自洽，探针裁不出来~~ **已消解且订正**：2026-08-30 读 `sub_5AFFCB0` 指令流定案 **AoS**（条目步进 = 键宽+值宽，`0x5affe31`）。原方案「SoA」是误判，已改。残留只有 `packed` 位拆的真库复核 | 已闭合；残留项走 P0 真库探针 |
| R2 | 没有可用真库样本（`pdms-test-data` 已不在） | 动工前先定样本来源并记进证据文件 |
| R3 | 重写 L0–L2 后 L4 大面积红 | 这正是预期信号（旧值本来就是错的）；需要一份「修之前 vs 修之后」的属性值对照留痕 |
| R4 | flag 语义未闭合，实现时忍不住发明 | 明确写死：路由与存在性都不看 flag；flag 只做直方图观测 |
| R5 | 又一个「唯一权威实现」被拆成两份（点查一套、枚举一套） | **已落地**：路由规则收敛成 `index::choose_child` 一处，点查与顺序游标共用 `read_node`；`ReadOnlyEngine::search_index` 自带的那份下降已删除改为委派。源码顺序断言 `choosing_a_child_has_one_implementation` 钉住「索引模块之外不得选子页」。双根差分（P3）接上同一条原语 |
| R6 | 旧仓 git 历史已丢，工作文件是唯一副本 | P-1 的 `git init` 是硬前置，不是可选项 |

---

## 8. Non-Goals

- 写回、`mark`/`refresh`/`compact`。
- Core3D 元素语义（位表分类、`Members(mode)`、`significant_owner`、`climb`）。
- 复刻 core.dll 的内存布局（页缓存槽编码、14 位页号等内部限制）。
- gen-model 的接入改造。

---

## 9. 路线抉择：clean 重写（本方案）vs 就地硬化（姊妹计划）—— 待裁决人拍板

> **2026-08-30 第二次重估（本节整体改写）。** 上一版在 IDA 补证之后把「clean 重写」降为可选，
> 理由是「旧栈的核心读模型本就对，真正的偏差只有条目计数 / 硬编步长 / extent 三条，而这三条
> 姊妹计划 P0–P4 已逐条覆盖」。`docs/evidence/2026-08-30-e3d-io-record-self-addressing.md`
> 让这个理由**的两半都不成立**：偏差不止三条（**记录层还有两条**），而且这两条**姊妹计划
> P0–P4 一条都没盖到**——最接近的 P4 不但没盖到，它给出的补救办法会把同一缺陷以更大的窗口
> 重新引进来。本节的每一条都基于实读 `old/vendor/old-pdms-io`、`old/vendor/old-parse-pdms-db`
> 与 engine-v2 `13a17e1` 的源码，具体到文件与行号。

### 9.1 两条新发现落在旧栈的哪几行

新证据的两条是：**① 记录头第 6/7 字写着显式属性流地址、第 8/9 字写着成员表地址；
② 每个尾块尾部写着自己的续接地址**。对照旧栈：

**① 旧栈从不读那两个地址槽。** 记录头在旧栈里被建模成 **6 个 dword**（`[0]`impl_len、
`[1..2]`refno、`[3]`noun_hash、`[4..5]`owner），第 6 字以后一律当**隐式属性存储区**。
证据是四个解析入口读完 `input[16..24]`（owner）就直接跳到 `padded_implicit_end`，
`parse.rs` 全文对 `input[24..44]` 这 5 个字**零处按地址读**：

| 位置 | 行 | 干了什么 |
|---|---|---|
| `old-parse-pdms-db/src/parse.rs` `parse_raw_element_identity` | 363–388 | 只读到 byte 24 |
| 同上 `parse_ele_membs` | 438–473 | owner 后直接 `padded_implicit_end` |
| 同上 `parse_ele_children` | 485–524 | 同上（第二份拷贝） |
| 同上 `parse_raw_ele_data_with_info` | 541–725（关键 580、592–600、602） | 同上（第三份拷贝）；`explicit_start = actual_impl_len + memb_bytes_len`（602）是**成员块紧跟隐式区、显式块紧跟成员块**的双重相邻性假设 |
| 同上 `padded_implicit_end` | 527–539 | 「隐式区之后连续跳 0 / 7 字，跳到第一个非 0/7 为止」——这就是相邻性推测本体 |

**② 旧栈把续接地址当别的东西用，且分两套不一致的口径。**

| 位置 | 行 | 口径 | 与实测（`(page_no, packed)` 在块头 +12..+20，payload 恒从 +20 起）比 |
|---|---|---|---|
| `parse.rs` `get_merged_data`（成员侧） | 3177–3206 | payload 从 **+20** ✓；续接靠**扫紧邻的** `00 00 00 07 00 02`（3190） | 偏移对、**寻址错**：用相邻性代替声明地址 |
| `parse.rs` `collect_explicit_segmented_payload`（显式侧） | 902–949（`MEMBERS_BASE_PAYLOAD_OFFSET = 12` @907、`SEGMENT_PAYLOAD_OFFSET = 24` @908） | payload 从 **+12** ✗ | **把两个续接字当成属性流字节读进去了** |
| `parse.rs` `collect_explict_data` 的「drain 8 还是 drain 12」自适应 | 875–891 | 事后猜着裁掉 8 或 12 字节 | 正是上一行偏移错逼出来的兜底 |
| `parse.rs` `has_unfinished_packed_expression_entry` | 953–977 | 「上一段末尾像个没写完的 packed 表达式就再跳 4 字节」 | 同一个偏移错逼出的第二层兜底 |

证据文件里 BRAN 24383/85432 冒出的幽灵成员 18010/8193、以及「payload 起点 0 和 8 两个都试一遍」，
在旧栈里对应的就是 907–908 这两个常量和 875–891、953–977 这两段兜底。

**③ 旧栈的「记录范围」比证据描述的那一版还紧：不是找 `0x00000007`，是固定 2048 字节平窗。**
`old-pdms-io/src/io.rs` `read_raw_element_record`（3047–3053）与 `parse_element`（3002–3006，
**同一段读的第二份拷贝**）都是 `seek + read_exact(vec![0u8; 0x800])`。后果与证据描述的那一版
**方向相反但同源**：那一版会一路吃到 64 KiB（把完好记录报成截断 1 033 条），旧栈则是
**超出记录起点 2048 字节的一切一律看不见，且不报错**。两者共用同一个根因——不读声明地址。

> ⚠ **量的归属要分清。** 证据文件的 46 566 / 36 106 / 29 027 / 1 033 是在**已删 e3d-io 那版
> 读法**上量的。旧栈与它共用「相邻性 + 跳 0/7 填充」这套机制（`padded_implicit_end` 527–539、
> `get_merged_data` 3190、`collect_explict_data` 813–897 的 word 对齐 resync，上限 `MAX_RESYNC = 64`），
> 所以**机制同源、量未在旧栈上单独量过**。2048 字节窗口会让两个方向的数都变：够不到的块更多
> （窗口外的一概够不到），误接的块更少（最多只能跨一个页边界）。**这个量必须由旧栈自己的探针出**
> ——姊妹计划已为此新增 **P1b（记录层的尺子）**，是 db4 一切改动的前置。

### 9.2 姊妹计划 P0–P4 有没有盖到？—— 逐条核实，结论：**一条都没有**

| 阶段 | 射程 | 盖到 ①？ | 盖到 ②？ | 核实依据 |
|---|---|:--:|:--:|---|
| **P0** 环境修复 | 恢复 engine-v2 工作副本 / 注释第四路 patch | ✗ | ✗ | 纯环境。附带核实：`Cargo.toml` 229–230 已改回注释态，`d:\work\plant-code\pdms-io-fork-engine-v2` 确不存在，**P0 事实上按「注释 patch」这条落地了**，计划正文还写着「默认前者」，需同步 |
| **P1** 对拍尺子 | `src/bin/legacy_v2_read_parity.rs` | ✗ | ✗ | 通读该 bin 全部 1126 行：`v2_walk`(317)、`free_walk`(403)、`sample_entry`(453)、`read_raw_header`(462)、`process_file_inner`(481) ——**零处读记录**。它是索引/会话/页层的尺子，记录层没有尺子 |
| **P2** 低风险硬化 | 头字段 `0x30/0x34/0x38`、`page_type` 校验、extent 拒绝、`stored_page_count` 守卫 | ✗ | ✗ | 全在 db1/db2 |
| **P3** 条目计数对齐 | `IndexPageData` 改 free_dwords 反推 | ✗ | ✗ | 全在 db3 |
| **P4** 记录窗口对齐 | 2 KB 平窗 → 页感知增长窗口；**优先转调 engine-v2 `RecordReaderV2`** | ✗ | ✗ | **见下，这条要重写** |

**P4 为什么不但没盖到、还会把缺陷放大**——实读 `pdms-io.git@13a17e1`
`crates/pdmsdb_engine_v2/src/db4/`：

- `record_reader.rs` `find_record_end`（105–173）**仍然是搜索式判端**：跳 0/7 填充（`skip_padding_len`
  175–186、`extend_impl_len` 188–198），然后找 `00000000`+`00000007` 对（128–133 返回 `pos+8`）、
  或找不跟 `00 01/02` 的裸 `00000007`（135–143 返回 `pos+4`）。它读的唯一头字段是 `impl_len`（111）。
- `element.rs:3` `ELEMENT_HEADER_WORDS = 6` —— **engine-v2 的头模型里根本没有那两个地址槽**，
  第 6 字以后被当成隐式属性字（`build` 82–92）。
- `advance_over_segments`（200–217）与 `explicit_attrs.rs` 的续段循环（62–81）都是**相邻性续接**，
  和旧栈 `get_merged_data` 3190 同一套。
- 窗口从 16 KiB 倍增到 1 MiB（9–10），超限报 `record 超出上限 1048576B`（57–61）——
  正是「把完好记录报成截断」那一类，只是阈值从 64 KiB 变成 1 MiB。
- `read_window` 96 行硬编 `ext_no: 1`，extent 同样无视。
- 另有一条**新发现的三方分歧**（此前未记账）：块头 payload 起点，旧栈成员侧 **+20**、
  旧栈显式侧 **+12**、engine-v2 **+16**（`explicit_attrs.rs` 47–59：`hash@4..8`、`self_ref@8..16`、
  `payload@16`）、e3d-io 实测 **+20**。engine-v2 把 e3d-io 口径下的 `owner.word0` 叫 `hash`、
  把 `owner.word1 + cont_pgno` 当 `self_ref`。**转调它等于用一个第三种口径换掉一个已经对了一半的口径。**

> 一句话：**P4 的补救对象本身就是同一形状的缺陷，只是窗口更大。** 姊妹计划必须先改 P4，
> 再谈排期；照现文执行会把「静默少读」换成「静默少读 + 一个新的块头口径」。

### 9.3 就地硬化旧栈记录层要改哪几个文件、多少行、哪几处是结构性的

前提：采用**唯一可行的分层方式**——记录装配上移到 `pdms_io`（它有文件句柄），装配出**一段连续字节**
再交给现有的 `parse_raw_ele_data(&[u8])`。理由：`parse_pdms_db` 是被依赖方（`pdms_io → parse_pdms_db`），
让它反过来吃 I/O 就要新加 `PageSource` trait 并穿过 4 个入口，而 `parse_ele_membs` /
`parse_ele_children` 的调用方手上只有字节切片。e3d-io 也正是这么切的（`read_record` 交连续字节，
`parse_tail` 在连续缓冲上走查）。

| 文件 | 改法 | 删 | 增 | 原地重写 |
|---|---|---:|---:|---:|
| `old-pdms-io/src/io.rs` | `read_raw_element_record`(3047–3053) 与 `parse_element`(3002–3006) 两份平窗读合并成一处地址驱动装配；`raw_element_payload`(1432–1438) 退役；`PdmsIO`(1402–1430) 加数据页缓存（照 `read_index_data` 3283–3296 的样子）；5 类结构化失败（指空 / 类型错 / 成环 / 越页 / 声明续接接不上） | ~20 | ~210 | — |
| `old-parse-pdms-db/src/parse.rs` | `padded_implicit_end`(527–539)、`get_merged_data`(3177–3206)、`collect_explict_data`(784–898)、`collect_explicit_segmented_payload`(902–949)、`has_unfinished_packed_expression_entry`(953–977)、`collect_explict_data_legacy`(981–1038)、`take_off_007_explicit`(1040–1056) 整体删除；换成「按块 kind 分派的一次走查」 | ~306 | ~110 | ~101（`parse_ele_membs` 438–473、`parse_ele_children` 485–524、`parse_raw_ele_data_with_info` 580–604） |
| `old-pdms-io/src/net_window.rs` | 217 / 454 / 458 三个调用点：新错误从「跳过 / warn」改成上浮 | — | ~12 | — |
| `old-pdms-io/src/session_index_diff.rs` | 586 同上 | — | ~8 | — |
| `old-pdms-io/src/snapshot.rs` | 跨页取页必须受冻结前缀长度约束（今天 3050 的 `seek+read_exact` 绕过了它） | — | ~15 | — |
| `gen-model/tests/pdms_record_boundary.rs` | 3 条用例建立在「168 字节末尾填充即记录尾」上，前提变了 | ~15 | ~30 | — |
| **合计（6 个文件、2 个 vendor crate + gen-model）** | | **~341** | **~385** | **~101** |

**结构性（不是机械替换）的有 7 处**：

1. 记录装配从 `parse_pdms_db` 的纯函数**上移**到 `pdms_io`——分层翻转，是整件事的地基；
2. `PdmsIO` 今天**只缓存索引页**（1425），数据页每次直读磁盘；地址驱动会反复取同一页，必须先有数据页缓存；
3. 「隐式区末尾跳 0/7 就是成员块」（527–539 + 580）→ 读头第 8/9 字；
4. 「成员块在前、显式块在后」（602）→ 单次走查、按块自报 kind 分派（旧栈今天连「一个元素有两个成员块」都处理不了）；
5. 续接从相邻性（3190、902–949）→ 块尾声明地址；
6. 显式 payload 起点 12 → 20，连带删掉两层兜底（875–891、953–977）；
7. 失败语义从「属性少几条 / 静默截断」→ 结构化报错（宪法「静默失效是最高级别缺陷」条）。

**对照量**：e3d-io 的记录层 = `src/record/{mod,block,explicit,element}.rs` = **646 + 179 + 295 + 82 = 1 202 行**，
已在 `522a252` / `357512d` 落地，硬门 **789 831 / 789 831**（429 库全部索引键读出记录且自报 RefNo 一致，
`tests/record_l3_contract.rs`）。上表那 ~385 行新代码，就是这 1 202 行的**第二份实现**，
而且旧栈**没有对应的语料门**（P1 的 1126 行探针零处读记录），改完无从验收。

### 9.4 重估后的路线对照

| 维度 | A · clean 重写全面替换 | B · 纯就地硬化（姊妹计划现文） | **C · 分层收口（新增，推荐）** |
|---|---|---|---|
| 记录层从哪来 | e3d-io，已过 789 831/789 831 门 | 旧栈内重写 ~385 行，无门 | **旧栈 `read_raw_element_record` 转调 e3d-io `record::read_record`** |
| 增量引擎地基（净窗口 1424 行 / 双根差分 1373 行 / `DabaconSnapshot` 479 行 / 水位 / staging） | **要重建**（本方案 P3 未做、P5 另开计划） | 原样保住 | **原样保住** |
| 索引层三条已定 bug（count / stride / extent） | e3d-io P1/P2 已完成 | P2/P3 照旧执行 | **P2/P3 照旧执行**（不受本次重估影响） |
| 新增代码量 | P3 + P5（未估） | ~385 行新 + ~341 行删 + ~101 行重写，7 处结构性 | ~110 行新（只剩 kind 分派走查）+ ~341 行删；io.rs 那 ~210 行不用写 |
| 新依赖边 | — | 无 | `pdms_io → e3d_io`（**动工前须核** e3d-io 的依赖面是否干净、`PageCache` 能否接受外部句柄/冻结前缀） |
| 验收面 | 已有 429 库四道硬门 | **要先造门**才能验收 | 直接复用 e3d-io 的 `record_l3_contract`，另加一条「旧栈经转调后与 e3d-io 逐字节相同」 |

### 9.5 建议（供业主拍板，本文不代拍）

**推荐 C（分层收口）**：索引层照姊妹计划 P2/P3 就地硬化不变；**记录层不在旧栈里重写第二遍**，
改由 `PdmsIO::read_raw_element_record` 转调 e3d-io 的 `record::read_record`，拿回一段连续字节，
`parse.rs` 只保留按块 kind 分派的尾部走查。

理由三条，都带数字：
1. 就地重写记录层要 **~385 行新代码 + 7 处结构性改动**，而它要复刻的 **1 202 行已经存在且已过
   789 831/789 831 的门**；
2. 旧栈**没有记录层验收面**（P1 探针 1126 行零处读记录），7 处结构性改动落地即无从证伪；
3. 姊妹计划原定的补救对象 `RecordReaderV2` **已被本次核实排除**（9.2），P4 无论选哪条都要重写。

**若选 B（坚持纯就地硬化）**，必须额外做三件事，缺一不可：
1. **先造门**：在旧栈上建 429 库记录层门（e3d-io `tests/record_l3_contract.rs` 的等价物），
   并先用它在**旧栈自己**身上量出 9.1 那批数的旧栈版本（够不到的块、误接的块、窗口外丢失的块）——
   证据文件的 46 566 / 36 106 是另一版读法上的量，不能直接搬；
2. **重写 P4**：把「转调 `RecordReaderV2`」整条删掉，换成地址驱动装配；同时把「块头 payload
   起点三方不一致（旧栈 +20/+12、engine-v2 +16、e3d-io +20）」立为新的差异项（建议编号 D4-4）；
3. **接上 snapshot**：跨页取页必须走冻结前缀，`io.rs:3050` 今天是裸 `seek`。

**若选 A（clean 重写全面替换旧栈）**，必须额外做：
1. 本方案 P3（双根差分）+ `net_window` 等价物——旧栈这两块合计 2 797 行，且是水位承诺的直接依赖；
2. `DabaconSnapshot`（479 行，volume+file_index 身份守卫、4 次稳定捕获、冻结前缀）在 e3d-io 里没有对应物；
3. staging / 水位 / live 台账（`docs/2026-08-12_live-test-ledger.md`）全部重建对拍基线。

**不推荐「先不动」**：这两条不是性能或整洁度问题。它们**不报错**——表现只是某个元素少几条属性、
或多出一条不存在的成员。而 `model_impact` 的三态分类、`member_alive_at` 的存活判定都吃这些属性，
`member_alive_at` 误判 `Deleted` 的通路姊妹计划 D4-1 已经点名。按宪法「静默失效是最高级别缺陷」条，
这类缺口不该停在「记账观察」。
