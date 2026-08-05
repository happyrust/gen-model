# 0003 — DB_Noun 分类器的实现：dabacon 字典 + schema 字段号 + 懒加载缓存

- **日期**：2026-07-23
- **⚠ 版本基线：E3D 3.1**（`D:\AVEVA\Everything3D3.1\core.dll`，地址形如 `0x58xxxxx`）。本记录的**偏移量只对 3.1 成立**；E3D 2.1 / PDMS 12.1.1 的同名字段偏移完全不同，见 `0010`。**dabacon 字段号两版一致，可跨版本引用。**
- **背景**：反编译 AVEVA E3D `core.dll` 的 `DB_Noun` 类型分类器，弄清 `primitive/graphicsBehaviour/geomset/extrusion/isPointsetPoint/hashValue/findNoun` **具体怎么实现**（IDA 会话 `core31-retrace`）。
- **相关**：`0001`（分类器速查表）、`0002`（模型更新逻辑）、`0010`（同一个类的 2.1 基线 + 版本漂移）；`docs/plans/core-dll-aligned-incremental-gen.md`。

## 关键洞见（均有地址 / 反编译证据）

1. **全部数据驱动，读 dabacon 数据字典，无硬编码**。每种元素类型 = 一个 `DB_Noun` 对象；"是否图元 / 什么画法 / 是否 geomset …" 都是去问字典里的 **schema 字段号**，值随字典（不随 core.dll 二进制）确定。

2. **两个基础件**：
   - `DB_Noun::internalGetField(this, fieldId, &out)`(0x58d9bd0)：`dict = *(*(this+0x58)+92)` 取字典句柄 → `sub_55BC98B(dict, fieldId, &out, &err)`（FORTRAN dabacon 取字段）→ `setErrorFromFortran`；`*out = val & 1`。**按字段号现读**。另有 vector 变体 0x58d9b20 / 0x58d9a30；数组/整型字段走 `sub_55BC8DC`。
   - `DB_Noun::ReadData()`(0x58d6d20)：**懒加载**，首访(`this+96==0`)把 ~20 个字段从字典批量读进对象固定偏移，末尾置 `this+96=1`。`hashValue==0` 时置 "Unknown Element Type" / 走 `DB_Udtg::description`。

3. **三类实现风格**（对应用户列的 6 个方法）：
   - **① 按字段号现读布尔**（`internalGetField(this, <id>, &v)`）：
     | 方法 | 地址 | dabacon 字段号 |
     |---|---|---|
     | `primitive` | 0x58da280 | 659518 |
     | `geomset` | 0x58d8a20 | 859903 |
     | `extrusion` | 0x58d8180 | 663225 |
     | `isPointsetPoint` | 0x58d9e10 | 290555737 |
   - **② 读 ReadData 预缓存字段**：`graphicsBehaviour`(0x58d9760) = `if(!this+96) ReadData(); return *((DWORD*)this+45)`（`this+0xB4`，由 ReadData 用字段 **5099119** 灌入）。
   - **③ 类型定位（走 hash / 字典，不读字段）**：
     - `hashValue`(0x58d9860) = `return *((DWORD*)this+23)`（`this+0x5C` 缓存的 noun hash，构造 / 查表时写入）。
     - `findNoun(hash, &out)`(0x58d85c0)：静态 `DB_Noun::dictionary_`(0x642359c，`map<int hash, DB_Noun*>`) 查；未命中且字典校验有效(字段 722704)→ `operator new(0x12C)` 懒建 `DB_Noun(hash)` 入字典；命中但坏 / 禁用(`*(n+65)`)→ `NOUN_UNKNOWN`(0x6421e3c)；`hash > 387951929` 走用户自定义类型 `findUdet` / `DB_Udtg::findUdtg`。返回是否成功。

4. **ReadData 批量读入的其它字段（部分，示意"一次读全、后续读缓存"）**：`internalGetField` 灌 722704→this+164、661628→this+176、89369995→this+177、750400→this+184、261556351(0xF970A7F)→this+188、281413407→this+200、206078421→this+212、46622793→this+213、621476→this+214、204468292→this+215、13953605→this+252、861007→this+236、843594→this+216、3475470→this+264；数组 / 整型经 `sub_55BC8DC` 灌 300373315 / 266716114 / 297966157 / 65664829 / 847458 / 76272573 / 5099119(→this+0xB4) / 602413。

5. **字段号 = dabacon 字典 ID**，语义(是否影响模型 / 画法 / 设计变化 DCHC)编在字典里、不在二进制 —— 与 `0002`/ADR-002 结论一致：core.dll 是"读字典标志来判"的**逻辑**，权威**数据**在字典。

## 对 gen-model 的启示

- **分类不硬编码**：像 core.dll 一样用元数据表(`vendor/aios-parse-pdms/all_attr_info.json` / 字典)标注"是否图元 / 画法 / geomset / 引用类型"，代码只按标志分派 —— 正是方案 A 的"数据驱动判定 + att_meta 兜底"。
- **`findNoun` ≈ gen-model 的 noun 解析**：hash→类型描述符 + 懒建 + UDET/UDA 分流 ≈ `db1_dehash` + `noun_attr_info_map`（`all_attr_info.json` 已是 `noun_hash → attr` 的字典快照）。
- **`ReadData` 一次读全进缓存** ≈ 解析器按 schema 一次解码元素隐含块的思路（`parse_raw_ele_data_with_info` 按 `attr_info.offset` 固定偏移解码）。
- **UDA / 用户类型有独立字典**(`DB_Udtg` / `findUdet`)：印证增量里 UDA 属性(`UDA:<id>`)按未知保守处理是对的。
