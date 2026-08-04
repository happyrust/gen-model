# 案例 02 · TRANSFORM_ONLY 名单过宽：7 条属性走了便宜路径

<sub>族 A 变化语义 · High · 已收窄到安全侧，实库对拍仍缺 · 证据层 A（字典）+ B（单测）</sub>

## 一句话

`POSS` / `POSE` / `CPOS` 这些名字里带 POS 的属性**定义的是几何本身**，却被归进「只更新 world transform、
不重建 mesh」的便宜路径——改一根型材的端点，mesh 长度不跟着变。

## 现象

`TRANSFORM_ONLY_ATTR_NAMES` 原有 9 条：`POS` `ORI` `CPOS` `NPOS` `POSE` `POSL` `POSS` `YDIR` `ZDIR`。
命中它们的变更只重算世界变换，网格原样复用。于是对一根 `SCTN`（结构型材）改 `POSE`：

- world transform 更新了，元素被搬到新位置；
- **mesh 还是旧长度**——型材的几何本来就是把目录截面沿 `POSS`→`POSE` 拉伸出来的。

## 证据

三份互相独立的数据源交叉核对，出处
[`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md)：

| 数据源 | 内容 |
|---|---|
| `output/noun_attr_fields.json` | E3D 属性字典导出，4270 个属性名 + 每条的 `DCHC` 设计变化类 |
| `all_attr_info.json` | 运行库 schema，339 noun / 6554 个 (noun, attr) 对 / 701 个属性名 |
| `noun_flags.json` | dabacon noun 表，1931 noun 及其几何 flag |

**第一步先把 `DCHC` 的语义查清**（否则「码 4 = 要重建」只是猜测）。码 1/2/3 的成员全集只有 14 条，
连同字典自带描述一起摊开，歧义就没了：

| DCHC | 属性 | 含义 |
|---:|---|---|
| 1 | `HBOR` `HDIR` `HPOS` `HSTU` `HZAXI` `LHEA`（+异类 `TZAXI`） | 元件**头端**连通性变化 |
| 2 | `LTAI` `TBOR` `TDIR` `TPOS` | 元件**尾端**连通性变化 |
| 3 | `BFORI` `ORI` `POS` | 元件自身**刚体位姿** |
| 4 | 其余 315 条 | 通用兜底类 |

逆向确认的两个 forced code 正好落在这个解释上：`INTUBE`（管件是否在管内 = 头尾连通概念）= 1，
`REDRAW`（通用重画）= 4。**所以 DCHC 是「变化类别」枚举，不是严重度分级**——不能由「码 4」推出「必须重建」。

但这反而让问题更清楚：**内核专门给「纯位姿」留了一个类（3），里面只放了 3 条属性**，而我们放了 9 条。

| | 我们的 TRANSFORM_ONLY | 内核位姿类（DCHC=3） |
|---|---|---|
| 交集 | `POS` `ORI` | `POS` `ORI` |
| 只有我们有 | `CPOS` `NPOS` `POSE` `POSL` `POSS` `YDIR` `ZDIR` | — |
| 只有内核有 | — | `BFORI` |

**第二步查这 7 条的字典描述**：`CPOS` = conditioning position for **curve geometry**；
`POSS` = Start point position；`POSE` = End point position；`POSL` = Positioning line。
描述里直接写着 geometry / start / end。

**第三步查属主 noun**（数据源 `all_attr_info.json`，括号内为 dabacon 几何 flag）：

| 属性 | 属主 noun |
|---|---|
| `POSS` `POSE` | `SCTN`、`STWALL` |
| `CPOS` | `CURVE`(geo) |
| `POSL` | `CMFI`(geo) `ENDATU`(geo) `FITT`(geo) `PLDATU`(geo) `SCOJ`(geo) `SJOI`(geo) `SEVE` |
| `YDIR` | `ENDATU`(geo) `PLDATU`(geo) `RPATH`(geo) `SPINE`(geo) |
| `ZDIR` | `CURVE`(geo) `SBFI`(geo) `SUBJ`(geo) |
| `NPOS` | `PNOD` |
| 对照 `POS` / `ORI` | **118 / 108 个 noun** |

`POSS`/`POSE` 只挂在两个 noun 上，而 `POS`/`ORI` 挂在上百个 noun 上——这个反差本身就说明前者是
**某类几何的定义参数**，后者才是通用位姿。

## 根因

名单是按**名字形态**攒出来的：名字里有 POS / DIR，看起来就像位姿。没有任何一步去问
「这个属性挂在哪些 noun 上」「这些 noun 的几何是不是由它定义的」。

## 修法

[`../../src/data_interface/model_impact.rs:111`](../../src/data_interface/model_impact.rs) 收窄为：

```rust
pub const TRANSFORM_ONLY_ATTR_NAMES: &[&str] = &["POS", "ORI"];
```

移出的 7 条**不需要另外加分支**：`classify_attribute_effect` 的判定顺序是
DATA_ONLY → STRUCTURAL → TRANSFORM_ONLY → CASCADE → 直接几何表，移出后它们自动落 `DirectGeometry`
走完整重生成。方向上回到项目自己的「宁多勿漏」，代价是这几类变更从便宜路径转为重建。

`BFORI`（内核算位姿类、我们没收）**故意没有补进来**：往便宜路径里加属性是「少算」方向，
没有实证之前不做；何况它在当前 339 noun 的快照里没有任何属主，补了也没有实际作用。

## 验证

- `cargo test --lib`：160 passed / 0 failed / 27 ignored；`model_impact` 模块 25 项全过。
- 守护测试 `exemption_tables_match_the_dictionary_change_class` 把「减免名单 vs 字典设计变化类」钉成断言。
- **仍未做**：实库对拍——改一根 `SCTN` 的 `POSE` 看 mesh 长度是否跟随。做完这一步才能把
  「疑似漏更新」正式定性；在那之前本次改动的定位是**按内核口径收窄到安全侧**，不是「已确认 bug 的修复」。

## 规律

**「便宜路径」的准入必须靠证据，不能靠名字。** 判断一个属性是不是纯位姿，最强的三条证据依次是：
内核把它归在哪个变化类、字典描述怎么写、**它挂在哪些 noun 上**。第三条最容易被忽略也最有说服力——
一个只挂在两个 noun 上的「位姿属性」，几乎一定是那两个 noun 的几何参数。

反过来，往便宜路径里**加**属性和从里面**删**属性，风险完全不对称：删只是变慢，加可能漏更新。
不确定时永远往慢的那边站。

## 关联

- [`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md)（DCHC 语义、三源核对、属主反查）
- 案例 [03 分类名单单一事实源](case-03-attribute-effect-single-source.md)（这次收窄之所以「移出即自动落 DirectGeometry」，踩的正是链的顺序）
- [`ADR-002`](../../docs/adr/ADR-002-core-dll-authority-scope.md)（`REDRAW` / `INTUBE` 是内核伪属性，字典里没有）
