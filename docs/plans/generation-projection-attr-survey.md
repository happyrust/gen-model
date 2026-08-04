# 附录：生成路径属性键普查（element_geom 列集依据）

> 配套 `docs/plans/generation-projection.md` §4。本文件回答一个问题：**模型生成到底读哪些属性键**——用来确定 `element_geom` / `element_vertex` 的列集。
>
> 普查日期：2026-07-30。

## 1. 方法与范围

三个代码域各扫一遍，正则抓取「带字面量键名的属性访问器」与「键名切片」：

```
带键访问器： get_(as_string|str|string|f32|f64|i32|i64|u32|u64|bool|vec3|dvec3|ivec3|val
              |attr_string|refno_by_att|refno_by_att_or_default)\s*\(\s*"([A-Z0-9_]{2,12})"
键名切片：   get_attr_strings(_without_default)?\s*\(\s*&?\[([^\]]*)\]
语义访问器： \.(get_position|get_dpose|get_dposs|get_level|is_visible_by_level
              |get_type_str|get_type|get_name_or_default|get_owner|get_refno_or_default|get_refno)\s*\(
```

| 域 | 路径 | 文件数 | 字面量键 |
|---|---|---|---|
| 生成主体 | `gen-model/src/fast_model/`（递归） | 20 | 56 |
| 目录求值 | `rs-core-pin/src/expression/` | 5 | 51 |
| 变换求解 | `rs-core-pin/src/transform/` | 1 | 13 |

语义访问器再映射回它们真正读的键（`rs-core-pin/src/types/named_attmap.rs`）：

| 访问器 | 实际读取 |
|---|---|
| `get_level()` / `is_visible_by_level()` | `LEVE`（i32 数组，取 `[0]` `[1]` 为可见级别上下界，`:510`） |
| `get_position()` | `POS`，缺失时回退 `POSS`（`:979`） |
| `get_poss()` / `get_dposs()` | `POSS`（`:994`） |
| `get_pose()` / `get_dpose()` | `POSE`（`:1008`） |
| `get_owner()` | `OWNER` |
| `get_refno()` / `get_refno_or_default()` | `REFNO` |
| `get_type_str()` / `get_type()` | `TYPE` |

**并集：84 个键。**

## 2. 按用途分类的完整键集

### 2.1 标识与结构（`hierarchy_node` 已覆盖，投影不重复存）

`REFNO` `OWNER` `TYPE` `NAME`

### 2.2 变换与定位

`POS` `POSS` `POSE` `ORI` `BANG` `NPOS` `YDIR` `OPDI` `DELP` `CUTP` `CUTB` `ZDIS` `PKDI` `LMIRR` `JLIN` `JUSL` `POSL`

`POS` / `ORI` 与 `model_impact.rs:111` 的 `TRANSFORM_ONLY_ATTR_NAMES` 直接对应，其余来自 `get_local_mat4`（`rs-core-pin/src/transform/mod.rs:55`）的分支处理。

### 2.3 可见性

`LEVE`（级别区间）`TUFL` `CLFL`

### 2.4 GmParam 标量

`PRAD` `PANG` `PWID` `PHEI` `POFF` `DRAD` `DWID` `PLAX`

### 2.5 GmParam 定长组（顺序即 `current_gm_param` 的取值顺序，不能乱）

| 组 | 键序 |
|---|---|
| `diameters` | `PDIA` `PBDM` `PTDM` `DIAM` |
| `distances` | `PDIS` `PBDI` `PTDI` |
| `shears` | `PXTS` `PYTS` `PXBS` `PYBS` |
| `lengths` | `PXLE` `PYLE` `PZLE` |
| `xyz` | `PX` `PY` `PZ` `PBBT` `PCBT` `PBTP` `PCTP` `PBOF` `PCOF` |
| `dxy` | `DX` `DY` |
| `paxises` | `PAXI` `PAAX` `PBAX` `PCAX` + `PTS` 展开 + `PLAX` |

### 2.6 P 点 / 轴参数（`get_axis_param`，`expression/query_cata.rs:86`）

`NUMB` `PCON` `PBOR` `PDIS` `PAXI` `PZAXI` `PTCD` `PTCP` `PTCPOS` `PX` `PY` `PZ` `PWID` `PHEI` `TYPE`

### 2.7 目录表达式上下文

`DESP`

**这是最关键的一条。** 目录几何表达式形如 `( ( ( - DESP[1]/2 ) - DESP[2] - ATTRIB CPAR[3] ) )`，其中 `DESP[n]` / `PARAM n` 全部由设计元素的 `DESP` 数组展开成 `DESP1` / `DESP2` / … 的求值上下文（见 `expression/resolve_helper.rs:29-41`、`gen-model/src/test/test_dir.rs:15-18`）。也就是说，**目录求值对设计侧属性的依赖，绝大部分收敛到 `DESP` 这一个数组属性上**——列集因此是可收敛的。

### 2.8 环、顶点与尺寸

`FRAD` `HEIG` `ANGL` `RADI`

### 2.9 cata_model 专用

`ARRI` `LEAV` `NAPP` `SJUS` `HDIR` `HPOS` `TDIR` `TPOS`

### 2.10 resolve 专用

`GTYP` `PARA` `PKEY`

## 3. 静态普查覆盖不到的部分

必须写明，否则会误以为列集已经封闭：

1. **`ATTRIB <name>` 表达式语法。** PDMS 目录表达式允许按名字引用任意属性（上面例子里的 `ATTRIB CPAR[3]`）。这些名字存在**目录数据**里，不在 Rust 源码里，静态扫描原理上抓不到。实践中 `CPAR` 一类落在目录侧（CATA 库），设计侧主要靠 `DESP`；但**不能证明**设计侧不会出现 `ATTRIB`。
2. **UDA。** 用户自定义属性是开放集合，当前生成路径不读，但插件路径（`plug_in/`）会读。
3. **`room_*.rs` / `pdms_inst.rs` 的空间逻辑**未纳入本次扫描范围（它们主要消费已算好的 `GmParam` 与网格，但未逐行确认）。
4. 扫描**包含 `#[cfg(test)]` 块**，所以少数键可能只出现在测试里（例如 `test_dir.rs` 的 `DESP4/5/10/11` 属于展开后的上下文键，不是真实属性名）。

## 4. 对设计的结论

- 列集**可以收敛**，因为目录求值的设计侧入口是 `DESP` 而不是任意属性名。§2 的 84 个键构成 `element_geom` 的列集依据。
- 但**不能假设它已经封闭**。`generation-projection.md` §9 里那条 debug-only 回退计数器是必需的：投影里缺某个属性时回落 SurrealDB 并计数告警，跑一轮全量把漏网的捞出来，再决定是补列还是保留回退。
- `DESP` 必须作为数组列进投影，且它是**目录几何的唯一设计侧参数入口**——漏掉它等于整个目录构件路径都要回落。

## 5. 复现方式

正则见 §1。按域重跑一遍并与 §2 比对；代码变动后（尤其新增几何 noun 支持时）应重跑。
