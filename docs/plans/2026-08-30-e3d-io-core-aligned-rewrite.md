# e3d-io：与 core.dll 同构的 dabacon 直读库（重写方案）

> 状态：**待裁决**。本文是 grill 过程的中间产物，Q1–Q3 已拍板；Q6 已由 IDA 补证收敛（见下），
> Q5/Q7/Q8/Q9 仍未决。
> 位置：新库落在 `D:\work\plant-code\old\vendor\e3d-io`。
> 关联：ADR-055（pdms-io v2 语义分层）、ADR-053（direct 模式生成读）、
> `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（core.dll live 逆向）、
> `docs/evidence/2026-08-30-e3d-io-index-node-aos-layout.md`（**本方案的 AoS/页头/H1 指令级补证 + 订正**）、
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
> **对本方案的影响（需裁决人重估）**：头号「重写」理由（布局错）被证伪后，「clean 重写」vs
> 「就地硬化 old-pdms-io」（姊妹计划）的天平明显偏向后者——因为旧栈的核心读模型**本就对**。
> 重写仍可由「条目计数根因 + 硬编步长 + 无判别力测试 + 四份并行实现 + extent 无视」支撑，但这些
> **姊妹计划的 P0–P4 已逐条覆盖**。见下方 §Q5 附注与末尾「路线抉择」。

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
> 这三条**不需要推倒重写**就能修（正是姊妹计划 P2/P3 的内容）。是否仍走「clean 重写」见末尾「路线抉择」。

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

**待定**：L2 的 ②「点查与枚举必须给出同一个存在性集合」是否设为硬门。
设了它，H1 那批「幽灵 refno」就会立刻暴露；不设，就可能带着幽灵一路上到生成层。

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

refno / noun hash / owner / members 块 / 隐式区 / 显式区分块。跨页记录必须支持。
失败一律结构化错误，不得降级为「不存在」。

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
| **P1** | L0 页 I/O + L1 头/会话 | 那 17 个坑探测器的文件不给 hint 也读出正确页大小与 `sesno` |
| **P2** | L2 B+ 树点查 + 顺序游标 | 冷缓存点查读页数 ≈ 树高；点查与枚举给出同一个存在性集合（Q7 待定） |
| **P3** | L2 双根差分 | 与 `session_index_diff` 在同一窗口上结果相同 |
| **P4** | L3/L4 接管或重写（取决 Q5） | 属性值与 E3D TTY 导出对拍 |
| **P5** | gen-model 接入 | 另开计划，不在本文范围 |

---

## 7. 风险

| # | 风险 | 对策 |
|---|---|---|
| R1 | ~~SoA/AoS 两种读法在样本上都自洽，探针裁不出来~~ **已消解且订正**：2026-08-30 读 `sub_5AFFCB0` 指令流定案 **AoS**（条目步进 = 键宽+值宽，`0x5affe31`）。原方案「SoA」是误判，已改。残留只有 `packed` 位拆的真库复核 | 已闭合；残留项走 P0 真库探针 |
| R2 | 没有可用真库样本（`pdms-test-data` 已不在） | 动工前先定样本来源并记进证据文件 |
| R3 | 重写 L0–L2 后 L4 大面积红 | 这正是预期信号（旧值本来就是错的）；需要一份「修之前 vs 修之后」的属性值对照留痕 |
| R4 | flag 语义未闭合，实现时忍不住发明 | 明确写死：路由与存在性都不看 flag；flag 只做直方图观测 |
| R5 | 又一个「唯一权威实现」被拆成两份（点查一套、枚举一套） | 点查、顺序游标、双根差分共用同一个下降原语，写成源码顺序断言测试钉住 |
| R6 | 旧仓 git 历史已丢，工作文件是唯一副本 | P-1 的 `git init` 是硬前置，不是可选项 |

---

## 8. Non-Goals

- 写回、`mark`/`refresh`/`compact`。
- Core3D 元素语义（位表分类、`Members(mode)`、`significant_owner`、`climb`）。
- 复刻 core.dll 的内存布局（页缓存槽编码、14 位页号等内部限制）。
- gen-model 的接入改造。

---

## 9. 路线抉择：clean 重写（本方案）vs 就地硬化（姊妹计划）—— 待裁决人拍板

2026-08-30 IDA 补证后，两条路线的取舍变了，必须重估：

| 维度 | 本方案（clean 重写 → `old\vendor\e3d-io`） | 姊妹计划（就地硬化 `old-pdms-io`，`…old-pdms-io-core-dll-read-gap.md`） |
|---|---|---|
| 头号前提是否成立 | **被证伪**：布局不是 SoA、现存实现不是「全错」 | 前提本就是「布局对、count/stride/extent 错」，**与实测一致** |
| 核心缺陷覆盖 | 需从零重建 L0–L2 才能修那三条 bug | P2/P3/P4 已逐条对准行号（scan-to-zero、硬编步长、2KB 窗口、extent） |
| 增量引擎地基（净窗口/水位/COW 差分/快照守卫） | 需重新长出来 | **原样保住**，只换读取口径 |
| 风险面 | 大（新库 + 迁移 + 对拍基线重建） | 小（fail-loud 硬化 + 双口径过渡 + 异常计数归零自证） |
| 仍支持重写的理由 | 四份并行实现难维护、无判别力测试、想要干净的 `PageId{ext,page}`/newtype 边界 | —（这些也可在硬化时顺带补） |

**建议**：把「clean 重写」**降级为可选目标**，先按姊妹计划 P0–P4 就地把三条 bug 修掉、拿真库证据；
若之后仍要 clean crate，则以「已验证的读模型」为蓝本重写，而不是以「布局错」为由重写。
最终选哪条由裁决人定；本方案文档保留，但其头号论证已按证据订正。
