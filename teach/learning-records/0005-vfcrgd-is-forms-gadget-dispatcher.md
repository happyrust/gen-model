# 0005 — `VFCRGD` 是 Forms&Menus 的 GUI 控件重绘派发器（修正 0002 的几何派发误判）

- **日期**：2026-07-23
- **背景**：为对齐 gen-model 的类型分类器（ADR-004），本轮要"逐个反编译 VFCRGD 的 14 个 case 构建器、命名 `graphicsBehaviour 枚举→几何类别` 权威表"。逐个反编译后发现**前提被推翻**：这些构建器不是 3D 几何构建器，而是 GUI 控件构建器 → VFCRGD 不是几何派发器（IDA 会话 `core31-retrace`，`D:\AVEVA\Everything3D3.1\core.dll`，imagebase `0x5170000`）。
- **相关**：修正 `0002`（§4/§7 曾写"VFCRGD 31-case 按 noun 分派几何"）；`0003` DB_Noun 分类器实现；`docs/plans/core-dll-aligned-incremental-gen.md` §2.1；`docs/plans/db-noun-classifier.md`。
- **原始反编译**：`.ida_scratch/analysis/*.c`（VFCRGD_52DB664 / 14 个构建器 / FZBG3D / HQTYPE / FZ3SGL / V3MAP / FZV3AR / FRS3VW / F3BDAD / FDF3DV）。

## 核心结论（一句话）

`VFCRGD`(sub_52DB664) 的 MTRENT 真名是 **`fmgadget/VFCRGD`**——它是 **AVEVA Forms&Menus (PML 表单/菜单) 的 GUI 控件(gadget)重绘派发器**：遍历一个表单的 gadget，按 `HQTYPE`(=**控件类型**枚举，不是 `DB_Noun::graphicsBehaviour`) 的 switch 分派到各 GUI 控件构建器。**它分类的是 GUI 控件，不是 3D 元素几何**；因此"从 VFCRGD 的 case 得出 graphicsBehaviour→几何类别表"是一个类别错误（category error）。

## VFCRGD 真实派发表（`switch(HQTYPE(gadget))`）

派发变量 `v13 = HQTYPE(&v8)`；主 switch 在 `(*a2 & 1) != 0`（重绘模式）分支内。每个构建器开头的 `MTRENT("module/NAME")` 给出真名：

| HQTYPE | 构建器 sub_ | MTRENT 真名 | GUI 控件 |
|--:|---|---|---|
| 2 | `sub_5323EEB` | `fm2dcanv/FZBG2D` | 2D 画布 |
| **3, 16(0x10)** | `sub_5296A17` | `fm3dcanv/FZBG3D` | **3D 画布/视图控件**（→ FZ3SGL，GUI→3D 桥） |
| 4 | `sub_530DFD4` | `fmframe/FZBFRA` | 框(frame) |
| 5 | *(内联)* HGETS1 + `sub_52F59D1` | — | 组合/成员递归（取字符串+走子项） |
| 6 | `sub_530A6C1` | `fmbutton/FZBBTN` | 按钮 |
| 7 | `sub_53141FD` (+`sub_531544B` 文字) | `fmtext/FZBTEX` | 文本 |
| 8 | `sub_530C65F` | `fmtoggle/FZBTGL` | 开关(toggle) |
| 9 | `sub_532CCD1` | `fmpara/FZBPGR` | 段落(paragraph) |
| 10(0xA), 12(0xC) | `sub_531E237` | `fmlist/FZBLIS` | 列表 |
| 11(0xB) | `sub_5311498` | `fmnewradio/FZBNRG` | 单选组 |
| 13(0xD) | `sub_5331E59` | `fmtextpane/FZBTPN` | 文本窗格 |
| 14(0xE) | `sub_5330D63` *(特判前置)* | `fmbar/FZBBAR` | 滚动条 |
| 29(0x1D) | `sub_531AEA4` | `fmNumberIn/FZBNMI` | 数字输入 |
| 30(0x1E) | `sub_533890E` | `fmLine/FZBLNE` | 线 |
| 31(0x1F) | `sub_5334C68` | `fmcontainer/FZBCTR` | 容器 |
| 32(0x20) | `sub_52F688A` | `fmslider/FZBSLD` | 滑块 |
| default | — | — | 无（该控件类型无重绘几何） |

> 说明：`case 5` 与 `case 14` 在主 switch 之前被特判（case 5 用 `HGETS1` 取字符串 + `sub_52F59D1` 递归成员；case 14 经 `sub_52F3886` 门控后调 `sub_5330D63`=FZBBAR）。非 `a2&1` 分支是控件的非重绘轻量路径（case 10 特判 `sub_531CE96`/`sub_53208B2`/`sub_53205E4`，其余 `sub_52DB092`）。

## 证据链（已复核）

1. **14 个构建器真名全是 `fm*` GUI 控件**（按钮/文本/开关/列表/单选/滑块/滚动条/数字输入/文本窗格/框/容器/段落/线/2D画布），无一例外 → GUI，而非几何图元。
2. **`HQTYPE`(0x5CBC3D0)** 是 408 字节小访问器（串 `"HQTYPE"`/`"ELM"`，调 `sub_5CBBA80`）= 取控件类型，非 `DB_Noun::graphicsBehaviour`(0x58d9760, 字段 5099119, 见 `0003`)。
3. **case 3/16 = `FZBG3D`(`fm3dcanv/FZBG3D`)** 是 3D 视图控件：内部调 `FZ3SGL`(sub_5297141) 建 GL 段、`UIALCN/UISTIN/UIALBL` 建 GUI、`HPUTI1/HGETI1` 存取属性句柄、`V3MAP`+`FZV3AR`(视图排布) → 它是 GUI→3D 的桥。
4. **`FZ3SGL`(`fm3dcanv/FZ3SGL`, 1479B)** 无 switch，只调 `GLVCRE/GLVSEG/GLSVCR/GLSVSE/GLSVSA/GLSVBO`(GL 段创建/属性) → 仅"为 3D 画布建 GL 段容器"的底层 plumbing，同样**不是逐-noun 几何派发**。

## 对 gen-model / 方案的含义

- **`0002` §4/§7 的"VFCRGD 按 noun 分派几何"已更正**为"按 gadget 类型分派 GUI 控件重绘；FZBG3D 桥入 3D"。`docs/plans/core-dll-aligned-incremental-gen.md` §2.1 同步更正。
- **ADR-004 分类器要对齐的 `graphicsBehaviour→几何类别` 表不来自 VFCRGD**。`DB_Noun::graphicsBehaviour`(字段 5099119) 仍是逐-noun 的权威画法 flag（`0003`），但它的**消费/派发不是一个可静态定位的代码 switch**（见下）。
- 高层结论不受影响：core.dll 在线 viewer「不算最小重生成集、`FZXUPD→FUPALL→GLUPDA` 全量刷新」仍成立（GL 显示层 flush，与本修正正交）；离线 gen-model 仍须自算最小集（ADR-002 C/D/E "分歧才对齐"）。

## 定位真 3D 几何派发器：两条线索已走查（排除图谱）

1. **xref `graphicsBehaviour`(0x58d9760) → 死路**：全库仅 1 个引用，且是 **vtable 数据引用**(`0x5e14028`, `fn=null`)。`graphicsBehaviour`/`primitive`/`geomset` 是 `DB_Noun` **虚方法**，只经 vtable 间接调用 → **静态查不到"按其返回值 switch"的消费者**（印证 0001/0003："经 0x5e14028 分派表间接调用"）。
2. **FZBG3D 之下的场景填充 → 全是 GUI/视图**：`sub_5296A17`(FZBG3D) 整棵子树都属 **`fm3dcanv` 3D 视图 gadget 模块**——`FZ3SGL`(建 GL 段容器)、`FZV3AR`(视图排布)、`FRS3VW`/`FRP3VW`(刷新→GLUPDA)、`V3ARNG/V3SHAD/V3EDGS/V3REFL/V3SPOT/V3WALK…`(边线/着色/反射/漫游等**渲染选项**)、`F3BDAD`(建视图罗盘 azim/elev 标签)、`FDF3DV`("定义3D视图"命令：Rotate/SHADED/CLIPPING/EDGES…)。**无一层遍历设计元素做几何生成**。

## 战略结论（important）

**core.dll 里不存在一个"按 `graphicsBehaviour` 枚举 switch → 几何构建器"的集中派发函数**（原始需求的前提本身不成立）。原因（与 `0001` 一致）：
- core.dll 是**编排层**；几何数学在 `libgeom`、三角化在 `sgl5NET`（经 metafile FACET 令牌交换）。
- 几何路由是**数据驱动**：逐 noun 的 `primitive/geomset/graphicsBehaviour` flag 存在 **dabacon 字典**里（`attlib.dat`，见 `0004`），代码按 flag 走**目录展开** `SPRE→SCOM→GMSET→PARA→图元`，而非一个大 switch；虚方法分派使"代码级几何派发表"在静态反编译下不可得。

⇒ **要对齐的"noun→几何类别"权威来源是 dabacon 字典 flag（ADR-004 / `dict.rs`），不是某个 core.dll 代码 switch。** gen-model 侧的目录展开链（`cata_model` 的 `SPRE→GMSET→PARA`）本就已实现，几何路由的"分类"输入应来自 dict flag。VFCRGD 这条线到此收敛（它属 GUI，本就与几何无关）。