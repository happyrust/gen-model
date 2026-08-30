# core.dll 索引结点物理布局（**AoS**）与条目计数来源 —— live 逆向补证 + 一处结论订正

- **日期**：2026-08-30
- **角色**：逆向研究（fable-5）
- **任务**：为两份 2026-08-30 计划取指令级证据——
  `docs/plans/2026-08-30-e3d-io-core-aligned-rewrite.md`（clean 重写）与
  `docs/plans/2026-08-30-old-pdms-io-core-dll-read-gap.md`（就地硬化）——
  裁定 SoA/AoS、条目计数来源、extent、哨兵、页头字段偏移，并核实/订正 2026-08-13 报告。
- **关联**：`docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`（上一轮 live 逆向）、
  ADR-055（页/会话/B 树以 core.dll 为准）、上列两份计划。

> ⚠️ **本报告订正一处旧结论**：2026-08-13 报告 §4.5 C2 把索引结点判成 **SoA**（连续键数组 + 连续数据数组），
> **是错的**。本轮读 `sub_5AFFCB0` 的**指令流**证明结点是 **AoS**：每条目 = `[键: key_dwords][值: value_dwords]`
> 连续排布，游标每步进 `key_dwords + value_dwords`。「键按 dword 连续比较」这一观察对 SoA 与「2-dword 键的
> AoS」**同样成立**，不能据此判 SoA——决定性证据是**游标步进公式**（见 §2）。此订正与 FFI 已对拍的
> `pdmsdb_engine_v2`（AoS，页头 `key_dwords/value_dwords`）一致，也与 `old-pdms-io`（AoS 定长 4 字步长）一致。

> 本轮**只读**，复用既有 headless 实例 `idalib-48392`（`D:\AVEVA\Everything3D3.1\core.dll.i64`，
> 与 2026-08-13 同一份二进制，SHA-256 `3C1F52DA…417D`）。未写入 / 改名 / 注释；不 stop 该共享实例。

---

## 0. 一句话

索引结点是 **AoS**（每条目 `[键][值]` 定长连续），条目数由**页头 free_dwords** 反推、不是「扫到零为止」，
extent 是一等寻址维。`pdmsdb_engine_v2` / `old-pdms-io` 对布局的建模**本就正确**；旧栈真正的读取偏差在
**条目计数口径**（scan-to-zero）与**硬编步长**，不在「AoS vs SoA」。

---

## 1. 相对 2026-08-13 报告的增量与订正

| 项 | 2026-08-13 结论 | 本轮（指令级） |
|---|---|---|
| 结点结构 | **SoA**（C2，伪码级） | **订正为 AoS**（§2，指令级：游标步进 = 键宽 + 值宽） |
| 条目计数 | 未闭合 | **由页头 `free_dwords` 反推**（§4，H1 得证） |
| 页头字段偏移 | 未给全 | `[0]`type@0x00 `[1]`table_id@0x04 `[2]`level@0x08 `[3]`key_dwords@0x0C `[4]`value_dwords@0x10 `[6]`free_dwords@0x18（§3） |
| 表 id 十六进制 | §9 写成 `0xCC441F`/`0x743F89`（错） | 订正 `0xCC47DF`/`0x743F49`（§7） |

仍未由本链路直接闭合、但有 engine-v2 FFI 佐证的一点：值第二字 `packed` 的位拆
（`offset = packed>>12`、`flag = packed & 0xFFF`、字节 = `offset×2`）见 §6。

---

## 2. AoS：指令级证明（级别：事实）

归并推进器 `sub_5AFFCB0`（opcode 270）里「算当前条目在页内的下一位置」的裸指令
（`edi` = 页指针，`esi` = 当前游标 dword 位置，`ebx` = 游标）：

```asm
0x5affe09  mov  edx, [edi+0Ch]     ; edx = page[3] = key_dwords（键宽）
0x5affe0e  jns  short 0x5affe14    ; page[3] ≥ 0 用定长；<0 走变长（读游标处长度前缀 +1）
0x5affe10  mov  edx, [edi+ebx*4]   ;   变长：key_dwords = page[cursor] + 1
0x5affe13  inc  edx
0x5affe14  cmp  dword ptr [edi+8], 0 ; page[2] = level
0x5affe18  jle  short 0x5affe21    ; level ≤ 0 → 叶；否则内部结点：
0x5affe1a  mov  ecx, 2             ;   内部结点值宽恒 = 2（子指针 (pgno, extno)）
0x5affe21  mov  ecx, [edi+10h]     ; 叶：ecx = page[4] = value_dwords（值宽）
0x5affe2f  add  edx, esi           ; edx = key_dwords + 当前游标
0x5affe31  add  edx, ecx           ; edx = key_dwords + 游标 + value_dwords = 下一条目游标
```

值取出（deleted 出口，`edi` = 页指针，`edx` = 游标，`ebx` = key_dwords）：

```asm
0x5b007b9  mov  ecx, [edi+10h]     ; value_dwords
0x5b007bc  lea  eax, [ebx+edx]     ; 数据起点 index = key_dwords + 游标（紧跟本条目的键之后）
0x5b007c7  cmovns edx, eax         ; page[4] ≥ 0 用定长起点；<0 起点 +1（跳长度前缀）
0x5b007db  lea  edx, [edi+edx*4]   ; 数据指针 = 页基址 + (游标 + key_dwords)*4
```

**判读**：一条目占 `key_dwords + value_dwords` 连续 dword，值紧跟键之后，下一条目 = 游标 + 键宽 + 值宽。
这是 **AoS**（结构体数组），不是 SoA（键区在前、值区在后）。SoA 的话，值区基址会是「页头 + 条目数 × 键宽」，
而不会是「本条目游标 + 键宽」。默认 `key_dwords=2, value_dwords=2`（叶）→ 4 字/条目，与 `old-pdms-io` 的
`RefnoDataLoc` 定长 4 字步长、engine-v2 的 `key_dwords/value_dwords`（0 回退 2+2、内部值宽恒 2）逐项对上。

键比较（`0x5b00541`–`0x5b005cb`）：键指针每轮 `add …,4`（1 dword）、标量**无符号**比较（`jb`/`ja`）、
哨兵 `0x80000001` 特判为键空间**最小值**。这段是对**当前条目的 2-dword 键字段**逐字比，
与 AoS 不冲突（旧结论正是把这段误当成「node 级键数组」才判成 SoA）。

---

## 3. 结点页头布局：7 dword / 28 字节（级别：事实）

| dword | 字节偏移 | 字段 | 证据 |
|---:|---:|---|---|
| `[0]` | `0x00` | `page_type`（表页 = 5） | `sub_5B026C0`/`sub_5AFFCB0`/`sub_5AEE4E0` 硬校验 `*page==5`，失败报 `"Page is not a table page (page is type %d)"` |
| `[1]` | `0x04` | `table_id`（主索引 `13387743`；另一系统表 `7618377`） | `sub_5AEE4E0`：`v44[1]==13387743`、`v47[1]!=7618377` 字面量比较 |
| `[2]` | `0x08` | `level`（≤0 叶；下降子 = 父 − 1，硬校验） | `sub_5AFFCB0` `cmp [edi+8],0`；`"Expected page of level %d, but got level %d"` |
| `[3]` | `0x0C` | `key_dwords`（键宽；`<0` 变长带长度前缀） | `sub_5AFFCB0` `mov edx,[edi+0Ch]` |
| `[4]` | `0x10` | `value_dwords`（值宽；`<0` 变长；内部结点恒 2） | `sub_5AFFCB0` `mov ecx,[edi+10h]` / 内部 `mov ecx,2` |
| `[5]` | `0x14` | —（本轮未定用途） | — |
| `[6]` | `0x18` | `free_dwords`（本页空闲 dword；条目区上界 = 全局页容量 `unk_6453DC4[0]` − `[6]`） | `sub_5AFFCB0` `v21 = unk_6453DC4[0] - v15[6]` |
| `[7]…` | `0x1C…` | 条目区（AoS：`[键][值][键][值]…`） | 游标 0 时初始位置 = 7（`lea edx,[esi+7]`） |

游标栈：begin（`sub_5B026C0`）`malloc(20 * (level+1))`，每层 20 字节 = 5 dword
`{page_ptr, pgno, extno, 0, read_result}`；首树、次树各一份（`unk_6A541F8` / `unk_6A541FC`）。

---

## 4. H1：条目数来自页头 free_dwords，不是「扫到零为止」（级别：事实）

```c
条目区 = [dword 7, 容量 unk_6453DC4[0] − free_dwords)
条目数 = (容量 − 7 − free_dwords) / (key_dwords + value_dwords)
```

- **`pdmsdb_engine_v2` 的口径正确且逐项对上**：它的 `count = (page_dwords − 7 − free_dwords)/stride`——
  `7` = 页头 dword 数、`free_dwords@0x18` = 页头 `[6]`、`page_dwords` = 核内全局页容量 `unk_6453DC4[0]`、
  `stride` = `key_dwords + value_dwords`。
- **`old-pdms-io` 的「扫到首个 0 字为止」错**：core.dll copy-on-write、不清理离开生效区的槽位（残留非零），
  扫零必然把陈旧槽位当有效条目读入——`session_index_diff` 那批异常计数
  （`duplicate_child_pointers` / `out_of_range_leaf_entries` / `nonlive_leaf_entries` 等，ams8000 实测非零）
  是这么来的。**权威口径 = free_dwords 页头驱动。**

> 重写/硬化时「容量」取运行期全局页容量 `unk_6453DC4[0]`（随页大小变），别写死 512。

---

## 5. 条目字段、extent 一等公民（级别：事实 + FFI 佐证）

- 一条目（叶，默认）= `[refno_0, refno_1, pgno, packed]` 4 dword（key_dwords=2 + value_dwords=2）。
  比较器 `DB_SystemTableCompare`：`dbele`（`0x5a18d10`）从 `this+1` 取 2 dword 构 `DB_Ref`（refno）;
  `dataOnFirst`（`0x5a18cd0`）拷 `this+3/this+4` = 2 dword（pgno, packed）。
- **extent 是一等寻址维**：全链路页地址都是 `(pgno, extno)` 2-dword 对（begin 取根、内部结点子指针
  `(pgno, extno)`、`sub_5AEE4E0` 入参），页地址从不压成单一页号。→ 两份计划里
  `PageId{ext,page}` / extent 显式拒绝方向确定。
- 页缓存描述符 = 28 字节/槽，`& 0x3FFF`（14 位页号）`& 0x10000`（有效位）——这是**内存缓存编码**，
  非文件格式（对齐要对「文件怎么读」，不是「内存里怎么摆」）。

---

## 6. 值第二字 `packed` 的位拆（级别：engine-v2 FFI 佐证，本链路未独立复解）

比较器把值第二字当**不透明整字**存/比，本归并链路裁不出它内部怎么拆。但 `pdmsdb_engine_v2`
（已过 `core_dll_oracle` FFI 对拍）与 `old-pdms-io` **口径一致**：

```
offset = packed >> 12        // 高 20 位
flag   = packed & 0xFFF      // 低 12 位
byte_offset_in_page = offset * 2   // offset 以「半字(2 字节)」计
```

本轮 IDA 独立确认了**值宽 = 2 dword**（无独立第 3 字 flag），与上式「flag 挤在值第二字低 12 位」自洽。
另注：2026-08-13 报告 §5 记的 core.dll **内存态**搜索结果编码是 `& 0x1FFF / >>13 & 0xFFF`，
与**文件态**的 `>>12 / &0xFFF` 不同——那是搜索结果的重编码，别把它当文件布局（此点即两份计划的 V1 验证项）。
在文件态位拆用 FFI oracle 复核前，路由与存在性都**不看 flag**。

---

## 7. 订正 2026-08-13 报告 §9 的十六进制笔误（级别：事实）

| 表 id（十进制，核内字面量） | 报告 §9 | 正确 |
|---:|---|---|
| `13387743` 主索引 | `0xCC441F` ❌ | **`0xCC47DF`** ✅ |
| `7618377` 另一系统表 | `0x743F89` ❌ | **`0x743F49`** ✅ |

（`0xCC441F=13386783≠13387743`；`0x743F89=7618441≠7618377`。十进制一直对，十六进制笔误。）

---

## 8. 关键地址（core.dll 3.1，SHA `3c1f…417d`，本轮 live）

| 用途 | 地址 |
|---|---:|
| begin（opcode 266，取两根 + 建双栈） `sub_5B026C0` | `0x5b026c0` |
| advance（opcode 270，双树 AoS 归并） `sub_5AFFCB0` | `0x5affcb0` |
| 条目步进（键宽+游标+值宽） | `0x5affe09`–`0x5affe31` |
| 值取出（游标+键宽 起点） | `0x5b007b9`–`0x5b007db` |
| 键比较（哨兵特判 + 指针 +4） | `0x5b00541`–`0x5b005cb` |
| 页读取（校 `page[0]==5`、`page[1]==表id`，28B 缓存槽） `sub_5AEE4E0` | `0x5aee4e0` |
| `dbele`（构 DB_Ref） / `dataOnFirst`（拷 2 dword） | `0x5a18d10` / `0x5a18cd0` |
| 主索引表 id / 另一表 id / 哨兵键 | `13387743`=`0xCC47DF` / `7618377`=`0x743F49` / `0x80000001` |

---

## 9. 复现

```powershell
ida-bridge list   # 复用 core.dll.i64 的 client_id（本轮 idalib-48392），勿再开同一 IDB
$cid = 'idalib-48392'
# AoS 决定性证据：条目步进 = 键宽 + 值宽
ida-bridge exec $cid --sql "SELECT hex(address), disasm FROM instructions WHERE func_ea=func_start(0x5AFFCB0) AND address BETWEEN 0x5affe00 AND 0x5affe40 ORDER BY address"
ida-bridge exec $cid --sql "SELECT hex(address), disasm FROM instructions WHERE func_ea=func_start(0x5AFFCB0) AND address BETWEEN 0x5b007a0 AND 0x5b007ee ORDER BY address"
# 页头字段 / 计数
ida-bridge exec $cid --sql "SELECT decompile(0x5AFFCB0) AS text"
ida-bridge exec $cid --sql "SELECT decompile(0x5B026C0) AS text"
# 页型/表id 校验
ida-bridge exec $cid --sql "SELECT n,line FROM pseudocode WHERE func_ea=func_start(0x5AEE4E0) AND (line LIKE '%== 5%' OR line LIKE '%13387743%' OR line LIKE '%7618377%') ORDER BY n"
```
