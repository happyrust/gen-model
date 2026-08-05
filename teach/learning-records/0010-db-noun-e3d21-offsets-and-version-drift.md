# 0010 — DB_Noun 在 E3D 2.1 上的实测布局；以及"偏移会漂、字段号不漂"

- **日期**：2026-08-05
- **背景**：手头只有 `/Volumes/DPC/reverse/core.dll`（MD5 `099b9237a64002e46b918c18841f547a`，15,330,816 B，imagebase `0x10000000`，字符串含 "Upgrades the database to 12.1.1"）= **E3D 2.1 / PDMS 12.1.1**。用 `ida-bridge` headless 挂 `core.dll.i64` 全量反编译 `DB_Noun` 的简单访问器，与记录 `0003`（E3D **3.1**）对照。
- **相关**：`0003`（同一个类，3.1 基线）、课 `0004`、参考 `reference/db-noun.html`。
- **本机没有 3.1 的 core.dll**（全卷只有两份 core.dll，MD5 相同，都是 2.1），`ida-pro-mcp`(13338) 也未运行 —— 记录 0003 的 `core31-retrace` 会话当前不可复现。

## 关键洞见

1. **记录 0003 与本记录描述的是两个不同的二进制，不是矛盾。** 之前差点把两套偏移当成"分析错了"。判据：3.1 的地址形如 `0x58xxxxx`、`hashValue@0x5C`、`dataLoaded@this+96`、`operator new(0x12C)`；2.1 是 `0x1045xxxx`、`hashValue@0x74`、`dataLoaded@0x78`、结构体 ≥ `0x144`。

2. **偏移跨版本漂移，且不是线性平移。** 同一语义字段 2.1→3.1 的差值：hashValue/dataLoaded 差 24 B，visible/graphicsBehaviour 差 44 B，modifiable 差 52 B，字段 13953605 差 56 B。差值递增 ⇒ 两版之间**多处**增删字段，无法整体换算。

3. **dabacon 字段号跨版本完全一致。** `5099119`=graphicsBehaviour、`621476`=modifiable、`722704`=visible、`661628`=toplevel、`89369995`=defaultVolumeQuery、`750400`=pickable、`206078421`=clasherWithin、`46622793`=clasherSection、`204468292`=statusEligible、`843594`=world、`261556351`、`281413407`、`861007`、`13953605` —— 在两版 `ReadData()` 里都出现，只是落到不同偏移。**这是唯一可移植的资产。**

4. **有两条互不触发的懒加载链**（0003 未覆盖）：
   - 主链 `0x78 dataLoaded` → `ReadData()` @ `0x10457D00`，灌 ~20 个字段。
   - DAB 链 `0x79 dabLoaded` → `ReadDataDab()` @ `0x10454F50`，目前只见 `primaryList`(0xA4)。
   调 `ReadData()` **不会**让 `primaryList` 就绪，反之亦然。

5. **`isValid()` 语义是反的**：`return *((_DWORD*)this + 29) == 0;` —— `hashValue == 0` 时返回真，更接近 `isNull`。旁证：`ReadData()` 里正是 `hashValue==0` 分支走 `"Unknown Element Type"` / `DB_Udtg::findUdtg` 兜底。按字面用会全盘反掉。

6. **字典主键取法绕了一层**：`internalGetField` 不直接用 `this->hashValue`，而是 `*(this+0x70)`（baseType 指针）`→ +0x74`（它的 hashValue）。普通类型 `baseType == this`；`isUDET()` 的实现正是 `*(this+0x70) != this`。

## 顺带纠正了 `/Volumes/DPC/reverse/` 下的旧分析笔记

那批 `DB_Noun_*.md` 描述的是同一个 2.1 二进制（MD5 对得上），四个层级指针偏移全对，但：

- "中间填充 (0xAC–0x10B, 96字节)" **不是填充**：内含 hardType 0xAC、visible 0xD0、spatialMap 0xD4、changeType 0xD8、toplevel 0xDC、defaultVolumeQuery 0xDD、graphicsBehaviour 0xE0、pickable 0xE4、clasherWithin 0x108、clasherSection 0x109、modifiable 0x10A、statusEligible 0x10B。
- "权限标志 (0x10C–0x10D)" **位置错**：实际在 0x108–0x10B；0x10C 是 `world`。
- "尾部填充 (0x10E–0x13D)" 内含 `oldkey` @ 0x138。
- "总大小 322 (0x142)" **偏小**：`isUDTG` 是 dword @ 0x140 ⇒ ≥ 0x144。
- "0xA4 属于层级指针区" **错**：0xA4 是 `primaryList` 布尔，且走 DAB 链。
- 版本信息 "0xE9EE00 (~235.8 MB)" 换算错：`0xE9EE00` = 15,330,816 B ≈ **14.6 MiB**（十六进制本身对）。

## 对 gen-model 的启示

- **不要把任何 DB_Noun 偏移写进 Rust 代码**。要复刻的是"按字段号问元数据"的机制；`all_attr_info.json` 这类语义快照方向正确。
- **加载分组是有意设计**：AVEVA 把字段拆成两条代价不同的懒加载链，说明"一次全读"并非必然最优，可按访问频率/代价分组。
- **UDET 独立通道**：`baseType != this` → `DB_Udtg::findUdtg`，与增量里"UDA 属性按未知保守处理"一致。

## 后续

- 未命名字段号还剩 5 个：`261556351`(0xE8)、`281413407`(0xF8)、`861007`(0x120)、`602413`(0x130)、`13953605`(0x134)。
- 若日后拿到 3.1 的 `core.dll`，应把参考文档做成双列版本对照，而不是再开一份。
