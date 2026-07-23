# 0004 — E3D Attribute Data File（dabacon 属性/类型字典）格式与离线解析规格

- **日期**：2026-07-23
- **背景**：为在 gen-model 直接离线解析 noun 分类 flag（`primitive`/`geomset`/…），RE 出 core.dll 加载的 dabacon 属性/类型字典文件格式（会话 `core31-retrace`）。
- **相关**：`0003`（DB_Noun 分类器实现）；`docs/adr/ADR-004`；`docs/plans/db-noun-classifier.md`。

## 关键洞见（均有地址/反编译证据）

1. **打开**：`ATTOPE`(`sub_55F4290`) 以 `OLD, READ` / `READONLY, DB, BL` 打开 **Attribute Data File**（错误串 "Unable to open the Attribute Data File"）。文件句柄存 `dword_6E336C0`。
2. **分页**：页 = **512×int32 (2KB)**。低层读 `FHDBRN`(`0x5b8d4f0`) = `sub_5B9B400(handle, pageNum, dest, 4*count, 1)`（通用 dabacon 页读，另被 `FICPY`/`MR_Message::Open` 等大量复用 —— 与 gen-model 现有解析器同款分页层）。
3. **页缓存**：`ATRDRC`(`sub_5391D48`) LRU 页缓存(≤1000 槽) `dword_6C3F6C0[512*slot..]`；缺页时 `FHDBRN` 读入；trace "Read attlib Page number N"。
4. **两套索引**（`ATGTIX`(`sub_55F4FFC`) 全库扫描建，key 范围 `[531442, 387951929]`）：
   - **字段索引**：fieldId(hash) --`sub_5392270`--> 列号 `col`；类型 = `dword_6C21070[col]`（**1=bool, 3=int, 4=array/vec**）。
   - **noun 索引**：nounHash --`sub_5392324`--> 记录号 `rec` → 页 `dword_6C10EE0[rec]`、行基址 `dword_6C18EE0[rec]`。
5. **取值**（`ATNLOG`=`sub_55BC98B`）：`value = dword_6C3F6C0[512*slot - 514 + rowBase + col]`，即 **2D：noun 记录行 × 字段列**。
   - **继承**：若该格 == 0，取 base_type（记录里的基类型字段）沿**继承链上溯**再取 —— noun 从 base_type 继承字段。
   - **默认**：仍无 → `dword_6C21200`/`dword_6C21390` 默认表兜底。
   - **类型化返回**：1→bool(`==1`)、3→int、4→数组(长度+元素)。
6. **分类字段就是此机制里的具体 fieldId**：`primitive 659518` / `geomset 859903` / `extrusion 663225` / `isPointsetPoint 290555737` / `graphicsBehaviour 5099119`（值随字典）。

## 离线解析规格（可直接实现）

1. 复用现有 dabacon **2KB 分页读**打开 Attribute Data File。
2. 复现 `ATGTIX` 索引：扫描建 `fieldId → (col, type)` 与 `nounHash → (page, rowBase)` 两表。
3. 对每个 noun：读其记录页，按 `col` 取分类字段；**空则走 base_type 继承链**，再走默认表；按 type 解。
4. 导出 `noun_flags.json`（per noun 的 5 个 flag，顺带全量字段）。

## 对 gen-model 的启示

- **分页层可复用**（`sub_5B9B400` 等价已在解析器里），新增只是「字典索引 + 2D 取值 + 继承/默认 + 类型」。
- **noun 继承(base_type) 是关键**：分类 flag 可能继承自基类型，解析必须实现继承链，否则漏判。
- 无需活 E3D / dump —— 纯离线读该文件即可复刻 core.dll 的分类语义。

## 实测校准（`D:\AVEVA\Everything3D3.1\attlib.dat`）

- **文件 = `%AVEVA_DESIGN_EXE%/attlib.dat`**（`sub_55CC570` 里 `ATTOPE` 的实参；siblings `message.dat`/`longtext.txt`）。
- 大小 **5,840,896 B = 2852 × 2KB 页**（整除 ✓）。
- **大端(BE)**：page0 = 长度前缀 BE 文本头 —— `\x13"Attribute Data File" \x07"2.0.009" (Microsoft Windows …)`（每字符占 1 个 BE int32）。→ 解析必须 `from_be_bytes`（`dict.rs` 已修正）。
- **字段元数据表**（~page 2167）：每字段 **4-int 记录 `[field_id, type, x, y]`**，`type` 紧跟 id。实测：`graphicsBehaviour 5099119 → type 3(int)`、`primitive 659518 → 1(bool)`、`extrusion 663225 → 1(bool)`、`661624/661628 → 1` —— **类型码 1/3 与 RE 完全一致**（强验证）。
- 字段 hash（BE）位置：`graphicsBehaviour` 2167:112、`primitive` 2167:247、`extrusion` 2167:255（同页连续 4-int 记录）；`geomset`/`isPointsetPoint` 也落在 2167–2168 段。

### 待续校准（下一步）
1. noun 记录区 + `nounHash → (page, rowBase)` 索引位置。
2. `col`(列号) 与 4-int 记录里 `x/y` 的确切含义（是否 col 即字段在表中的序号）。
3. page0/header 里 index 起始页指针（`ATTOPE` 的 `unk_5DB4608/5DB4610`）。
4. `base_type` 字段号（继承链）与默认表。

### 文件页图（实测 attlib.dat，续）

- **page0** = 文本头（长度前缀 BE 字符串："Attribute Data File" / "2.0.009" / 构建时间；word 108+ 为 0）。
- **page1** = 区段目录：观测 `[3, 4, 2098, 2123, 2168, 2169, 2830, 2838, 0…]` —— 一串区段边界页号（field/noun/string 各区）；已知 field 记录落在 ~2123–2168 与 ~2830–2838。
- **page2+** = 字段定义表：变长记录 `[field_id, type, …]`，type 紧跟 id（实测 `649072→3`、`3426347→1`、`865138→3`…，与 page2167 同构）。
- **ATTOPE 引导**：常量 `unk_5DB4610 = 2` → 读 page2 取结构指针启动 `ATGTIX` 扫描；`unk_5DB460C = 931537` 仅调试用。
- **待续（读出某 noun 的 flag 的最后一环）**：noun 记录区布局 —— noun 如何存/引用各字段值（col 列号 ↔ field 表序号），以及 noun→(page,rowBase) 索引在区段目录里的具体区。

### noun 区（实测）

- **noun 索引 @ page 2830–2831**：**偶数偏移的 `(nounHash, addr)` 对**。实测 `SCYL@2830:496`、`SCOM@2830:12`、`SITE@2830:260`、`CYLI@2830:358`、`EQUI@2830:384`、`SBOX@2831:446`（与 page1 目录里的 `2830,2838` 区吻合）。
- **noun 记录 @ ~page 2169–2830**（SCYL 的记录 addr → **page 2686:299**）：是**按 col 索引的稀疏字段值数组** —— `-1(0xFFFFFFFF)=未设/继承`、`0=false/继承`、其它=值。与 ATNLOG 完全对应：非 0 非 -1 → 命中；`0` → 沿 `base_type` 继承；`-1` → 默认表。
- **最后一环（field→col）**：`ATNLOG` 用 `sub_5392270(fieldId, 6C20EE0, 6C3F6B4, &col)` 把 fieldId 哈希成 `col`；拿到 col 即可 `record[rowBase+col]` 读出 SCYL 的 `primitive/geomset`。两条获取途径：① RE `sub_5392270` 的哈希/查表；② 用「已知 primitive=true 的多个 noun」交叉定位 col（经验校准）。

### field→col / noun→rec 解析（实测，**算法完整**）

- **`sub_5392270 = ATFIND`（线性查找）**：`col` = `fieldId` 在**字段键数组** `6C20EE0` 里的 **1-based 序号**（即字段在字段表中的出现/扫描顺序）。
- **`sub_5392324 = ATCHOP`（二分查找）**：**noun 键数组** `6C08EE0` 是**升序排序**的，二分得 `rec` 序号。
- **取值**：`value = noun_page[rowBase + col]`（`page=6C10EE0[rec]`、`rowBase=6C18EE0[rec]`、`col=ATFIND 序号`）；非 0 非 -1 = 命中；`0` → `base_type` 继承；`-1` → 默认表。ATNLOG 里的实际下标是 `512*slot - 514 + rowBase + col`（比页起始多 -2 的对齐，需真实文件微调）。

⇒ **算法已完整 RE，无剩余概念缺口**。实现只需：① 按扫描顺序建字段键表（定 col）② 二分建 noun 表（rec→page/rowBase）③ 读 `record[rowBase+col]` + 继承/默认。剩「字段扫描顺序 + `-2` 对齐」用真实文件对拍标定即可。

### ⚠️ 校准修正（noun 记录字节映射**未定**，勿据上文 noun 区结论实现）

实测反证：把 page2830 的 noun hash 当作 `(hash, addr)` 对、取 addr 跟到"记录"，落到的是**字段名文本区**（SCYL→2686:299 出现 `"AREADEF"`/`"Area definition"`）；且并排 dump `SCYL/SBOX/CYLI/SITE/EQUI` 的"记录"列对不齐、含大量 ASCII（字段名）。

⇒ 结论：**「page2830 = noun 索引 (hash,addr) 对」与「noun 记录 = 纯 col 索引稠密数组」这两条是过度推断，与字节实测不符**。noun 记录更可能是「(字段名/引用, 值) 列表」，或 noun 索引根本在别处。要读出「某 noun 的某 flag」需更谨慎的 RE，不能据此实现。

**仍然可靠（已交叉验证，可放心用）**：
- core.dll 取值**算法**：`ATCHOP`(二分 noun) / `ATFIND`(线性 col) / `value=page[rowBase+col]` + 继承 + 默认 + 类型(1/3/4)。
- 文件事实：`attlib.dat` = **大端 · 2KB 分页 · 2852 页**；page0 文本头；**字段表 `[field_id,type,…]` 且类型码 1=bool/3=int 与 core.dll 逐一对上**（primitive/extrusion=1、graphicsBehaviour=3）。

**下一步（谨慎做，勿快速试错）**：① 重核 page2830 到底是名字表还是索引、addr 语义；② 或直接反编译 `ATNLOG` 在**真实运行内存表**上一次完整取值（含 `6C08EE0/6C10EE0/6C18EE0/6C20EE0` 如何由 ATGTIX 从文件填出），拿到确切的「文件区 → 内存表 → (rec,col) → cell」映射，再落地实现。

### ATTOPE 表加载全景（`sub_55F4290` 完整反编译 —— 权威）

ATTOPE 打开 `attlib.dat` 后，读**逻辑页 2 的头记录**（`unk_5DB4610=2`；`v47[0..7]` = 8 个区段起始页），据此加载 6 张内存表：

| 表 | 起始页 | 填充函数 | 输出 (key / …) | count |
|---|---|---|---|---|
| A | `v47[2]` | `sub_55F4FFC`(ATGTIX) | `6BCA6E0` / `6BD26E0` / `6BDA6E0` | `6C08EB0` |
| B | `v47[0]` | `sub_55F53B8` | `6BE26E0` / `6BE2870` / `6BE2A00` / `6BE2B90` | `6C08EB4/EB8` |
| C | `v47[3]` | `sub_55F594C` | `6BE2EB0` / `6BECEB0` / `6BF6EB0` | `6C08EBC` |
| **NOUN** | **`v47[6]`** | **`sub_55F4FFC`(ATGTIX)** | **`6C08EE0`(key) / `6C10EE0`(page) / `6C18EE0`(rowBase)** | **`6C3F6B0`** |
| **FIELD** | **`v47[4]`** | **`sub_55F53B8`** | **`6C20EE0`(key) / `6C21070`(type) / `6C21200`+`6C21390`(default)** | **`6C3F6B4`** |
| SORT-IDX | `v47[7]` | `sub_55F594C` | `6C216B0` / `6C2B6B0` / `6C356B0` | `6C3F6BC` |

- `ATNLOG` 取值用 **NOUN 表**(`6C08EE0`二分→rec→`6C10EE0`page/`6C18EE0`rowBase) + **FIELD 表**(`6C20EE0`线性→col、`6C21070`type、默认表`6C21200/6C21390`) + 数据页缓存 `6C3F6C0`。
- ATGTIX 条目 = `(key, addr)` 2-int 对，`addr = page*512 + off`；noun 用 `sub_55F4FFC`(key,page,off 三路)、field 用 `sub_55F53B8`(key,type,defA,defB 四路)。

### ⚠️ 真正的最后一桥：逻辑页号 → 文件字节
- 头记录在**逻辑页 2**，但**原始文件 page 2 的字节 ≠ 8 个页号**（实测 `[649072,3,2,1,865138,…]`）——说明 `FHDBRN`/`sub_5B9B400`(DirectAccessToken 消息层) 有**逻辑页→物理偏移映射/基页偏移**。
- ⇒ 落地离线解析的最后一步：RE `sub_5B9B400` 的逻辑页映射（或在原始文件里定位"含 8 个合理页号(<2852)的头页"），据此取 `v47[6]/v47[4]` 的真实起始页，再按 ATGTIX (key,addr) 对建 noun/field 表 → `data_page[rowBase+col]`。此桥一通，`dict.rs` 即可读出任一 noun 的分类 flag。

### ✅ 桥已找到：raw page 1 = 头记录（区段起始页已验证）

实测 **raw page 1** = `[3, 4, 2098, 2123, 2168, 2169, 2830, 2838, 0…]` —— 正是 ATTOPE 的头记录 `v47[0..7]`（它读"逻辑页 2"，物理落在 raw page 1）。**区段起始页即 raw 页号，且与前面实测完全吻合**：

- `v47[4] = 2168` = **FIELD 区**（primitive/geomset/graphicsBehaviour 实测就在 raw 2167–2168 ✓）
- `v47[6] = 2830` = **NOUN 区**（SCYL/SBOX/CYLI 实测就在 raw 2830 ✓）
- `v47[2]=2098`(表A) · `v47[3]=2123`(表C) · `v47[5]=2169` · `v47[7]=2838`(sort-idx) · `v47[0]=3`/`v47[1]=4`(表B)

**映射结论**：逻辑页 N ↔ **raw 页 N−1**（头在 raw1 = 逻辑2；page0 = 文本头）。ATGTIX 的 `addr` 是 word 地址，`raw_page = floor(addr/512)`、`off = addr % 512`（**务必 floor，早前 PowerShell `[int]` 四舍五入把 2685 误成 2686，导致落到错误页**）。

⇒ **概念缺口清零**。剩纯实现：从 raw 2830 walk `(key,addr)` 建 noun 表、raw 2168 建 field 表、`data_page[floor(addr/512)][off + col]` 取值 + 继承/默认/类型。`dict.rs` 的 `read_index_start_pages` 可直接返回 raw page 1 的 v47[6]/v47[4]。

### 实现进展（`dict.rs` 集成测试对真实 `attlib.dat`）

- ✅ **header v47 读对** = `[3,4,2098,2123,2168,2169,2830,2838]`；`noun_count=1931`、`field_count=982`（双索引都建起来了）。
- ✅ **noun 索引 + 记录定位合理**（floor 映射 `logical−1` 正确）：`SCYL@raw2684:299`、`SBOX@raw2668:1`、`CYLI@raw2262:175`、`SITE@raw2695:309`、`ZONE@raw2828:137`、`EQUI@raw2378:1`（记录散落数据区，索引在 raw2829）。
- ❌ **field 索引漏 `primitive/geomset`**：primitive(659518) 在 raw2167:247（offset **非 4 对齐**），当前「按页 4-int 对齐 walk」读不到；field 记录流不按页 0 对齐、run 间有间隙（如 `…120 gb区… [间隙] …247 prim区…`）。
- ⇒ **最后一个精确 bug**：FIELD 表记录框架（流起始偏移 / 步长 / 间隙语义）需反编译 **`sub_55F53B8`**（field 表 builder）定死。此项一解，`raw_field` 即可用 col 读出 SCYL 的 primitive/geomset（noun 侧与页映射均已就绪）。
- 代码：`dict.rs` 已落地真实实现（header/noun 索引/floor 映射/取值+继承默认骨架 + `#[ignore]` 集成测试 `integration_read_attlib`）；`build_field_index` 标了此 TODO。

### field 表解析已修对；剩「-1=默认表 / 0=继承」值解析（真·最后一环）

- ✅ **变长记录解析修对**（`ATGTDF`/`sub_55F53B8` 语义）：`primitive col=66 Bool`、`extrusion col=68 Bool`、`geomset col=69 Bool`、`graphicsBehaviour col=32 Int`——**类型全对**；`field_count=93`（首个 field 页的变长记录）。
- ✅ **取值读到真实 cell**（`value = raw_page[rowBase + col − 2]`）：SITE 在 c=30 读到 127、c=37→126、c=39→125；SCYL 在 c=25/26/53/67 读到 150/151/148/149。多数 cell = **-1**。
- ❌ **SCYL 的 primitive cell = -1**（不是直接 1）。⇒ 分类主要靠 **-1 / 0 语义**，而非直接 cell 值。ATNLOG 精确语义（`sub_55BC98B`）：`cell≠0 且 ≠-1` → 值；`cell==-1` → **默认表**（`6C21200`/`6C21390`，即 `ATGTDF` 的 a5/a6 输出）；`cell==0` → 沿 **base_type** 重查（继承）。
- 当前 `dict.rs::raw_field` 对 -1/0 直接回 field-def 默认（不完全对）——需：① `cell==-1` 查默认表 a5/a6；② `cell==0` 走 base_type 继承链；③ 复核 `col−2` 偏移（用 graphicsBehaviour 的已知值对拍）。**此环一解，parser 即给出与 core.dll 一致的分类**（无论 true/false，都是 core.dll 的答案）。

### 状态小结（实现）
`dict.rs` 已能：读 header、建 noun 索引(1931)、建 field 表(变长, 类型正确)、按 `[rowPage][rowBase+col−2]` 读 cell、导出 JSON、`#[ignore]` 集成测试对真实 attlib.dat 跑通到"读出 cell"。**唯一剩余 = 默认表 + base_type 继承的值语义**（含 -2 偏移终校）。RE 侧 ATNLOG 语义已完全明确，属实现收尾。

### 值 cell 读取尚未打通（CYLI 记录 = 名字/引用表，非 bool 值）—— 静态边界

关键交叉校验（用 gen-model 自带 `GNERAL_PRIM_NOUN_NAMES` 作 ground-truth 代理）：
- **`GNERAL_PRIM_NOUN_NAMES` = [BOX, CYLI, SLCY, CONE, DISH, CTOR, RTOR, PYRA, SNOU, POHE, NBOX, NCYL, …]** —— 是**设计图元**；**SCYL/SBOX 是目录(catalogue)实体、不在此表**。⇒ parser 读 `SCYL.primitive=false` **可能本就正确**（目录实体非设计图元）。真正的判据是 **CYLI/BOX 应 primitive=true**。
- 实测 CYLI 记录 @raw2262:175 的 cell（0..90）：全是 **105–148 区间值（疑似名字/字符串表索引）+ 0**，**无任何 `1`**。⇒ 从 noun 索引 `(key,addr)` 跟到的"记录" **不是 primitive/geomset 的 bool 值数组**，而是**名字/引用记录**（或 ATNLOG 的 `6C3F6C0` cell 地址与该记录布局不同、值区在别处）。

**结论**：`value = noun_record[rowBase+col]` 模型对**值 cell 不成立**——noun 索引的 addr 疑似指向名字区。要读出真实 bool/int 值，需：
1. **活 E3D ground truth**（对 CYLI/BOX 跑 `DB_Noun::primitive` 得期望值）来校准；或
2. 更深 RE：`ATNLOG` 的 `6C3F6C0` cell 地址 **相对哪张表/哪个 addr**（noun 索引可能给的是名字/def 记录，值 cell 经另一层间接）。

**确定无误**：文件格式（大端/2KB 分页/header@raw1/区段目录）、FIELD 表（col+类型 1/3/4，`ATGTDF` 变长记录）、NOUN 索引存在且 `(key,addr)` 可 walk（记录定位合理）。**值语义是唯一未通环，静态分析到此为止**（需活 E3D 或更深 ATNLOG 间接层 RE）。

---

### ✅ 值 cell 已打通 —— 两级取值模型证实，与 core.dll `ATNLOG` 完全一致（2026-07-24 · 会话 gen-model-10）

**推翻上一节"静态边界 / 需活 E3D"的悲观结论。** 一级模型 `value = noun_record[rowBase+col]` 确实不成立——但正确模型是**两级间接**，已由 `ATNLOG`(`sub_55BC98B`) 反编译逐行证实、并在真实 `attlib.dat` 上跑通。**不需要活 E3D**：忠实复刻 ATNLOG 在文件上跑出的，就是 core.dll 自己的分类答案。

**ATNLOG 两级取值（`.ida_scratch/analysis/ATNLOG_getter_55BC98B.c` 逐行核对）：**
```
col  = ATFIND(fieldId)                                  // 字段在 field 表中的 1-based 序号
off  = data_page[512*slot-514 + rowBase + col]          // step1：col 槽存的是"记录内偏移"，非值
  off == -1 → 默认表：  value = 6C21390[ 6C21200[col-1] - 1 ]
  off == 0  → base_type 继承：base = value_at(baseTypeOff)，以 base 为新 noun hash 重查（ATCHOP）
  否则       → step2：    value = data_page[512*slot-513 + (rowBase+off-1)]  == page[rowBase+off-2]
类型：1=bool(cell==1 为真) / 3=int / 4=array
```
`dict.rs::raw_field` 的 `slot_at`(step1) + `value_at`(step2) + 继承 + 默认与此**逐行吻合**。早前 archive 把 step1 的 off（105–148）误当值 → 落到名字/偏移区，才得出"读不通"。

**真实 attlib.dat 实测（`dict::tests::integration_read_attlib`，noun_count=1931 / field_count=93）：**
- 设计图元 BOX/CYLI/CONE/DISH/CTOR/PYRA/SNOU → `primitive=true`（prim.off = 119..149 真实偏移）；
- 目录几何 SCYL/SBOX → `geomset=true, primitive=false`（prim.off = -1 → 默认）；
- 挤出 PANE → `extrusion=true`；容器 SITE/ZONE/WORL/EQUI/PIPE/BRAN → 三 flag 全 false（SITE/ZONE/WORL 的 gb=2）。

**严谨交叉核对（`dict::tests::crosscheck_curated_noun_lists`，对 `aios_core::pdms_types` curated 名单）：**

| 名单 | 期望 | 命中 | 一致 |
|---|---|---|---|
| `PRIMITIVE_NOUN_NAMES`(8) | primitive=true | 8/8 | 100% |
| `GNERAL_PRIM_NOUN_NAMES`(22) | primitive=true | 22/22 | 91%* |
| `TOTAL_CATA_GEO_NOUN_NAMES`(31) | geomset=true | 31/31 | 100% |
| `GNERAL_LOOP_OWNER_NOUN_NAMES`(9) | extrusion=true | 9/9 | 100% |

\* 唯一"不符"是 NSBO/NSCY——它们是**元件库负实体**（同时列在 `TOTAL_CATA_GEO_NOUN_NAMES`），dict 读作 `geomset=true` 反而**比 curated 名单更准**（`GNERAL_PRIM_NOUN_NAMES` 把目录负体误并入设计图元）。即 dict 分类精度 ≥ 手维护名单。

**分布健康（非"全 true"，读取确在鉴别）：** primitive=347 / geomset=44 / extrusion=38 / 三者皆 false=1536（共 1931）；**primitive ∩ geomset = 0（互斥，强正确性信号）**；gb 枚举分布 {0:1652, 1:109, 2:98, 3:72}。

**重要发现（与现实现分歧，正是要对齐 core.dll 之处）：全部管件 noun（ELBO/VALV/FLAN/GASK/TEE/REDU/BEND/TUBI/NOZZ/… 共 28 个）在 dict 里 `primitive=true`。** 逻辑证明其非误读：`primitive` 字段默认值=2（作 bool 为 false），故凡读出 `primitive=true` 者**必**有真实 off 指向值=1 的 cell、绝不可能来自默认表。⇒ core.dll 的 `primitive()` 语义 = "**设计级几何叶子 noun**"（含经典图元 **+ 管件**），而**非**"数学基本形状"；目录侧几何（SCYL/SBOX）才归 `geomset`。gen-model 现把管件单列 `PIPING_NOUN_NAMES`/目录实例化桶，与 core.dll 口径不同——后续 `NounClassifier`（阶段 3）应以 dict flag 为准重估分类归属。

**产物：** 仓库根 `noun_flags.json`（1931 noun × {primitive/geomset/extrusion/isPointsetPoint/graphicsBehaviour}，约 370 KB），供阶段 3 `NounClassifier` 加载（与 `all_attr_info.json` 同级）。
**代码：** `dict.rs` 新增 `has_noun()` 及 `crosscheck_curated_noun_lists` / `export_noun_flags_json` 两个 `#[ignore]` 测试；两级 `raw_field` 未改（本就正确）。全部 dict 单测 + 集成测试通过。

> **状态更新：ADR-004 阶段 2（离线解析器从 `attlib.dat` 读出 per-noun 分类 flag）达成，无剩余概念缺口。** 上文"值 cell 读取尚未打通 / 静态边界"各节均以本节为准更正。
