# RVM 基准对拍方案：用 E3D 3.1 导出的 RVM 校验模型生成正确性

- 项目路径：`D:\work\plant-code\old\gen-model`（crate `aios-database` v0.1.4，分支 `codex/github-public-dependencies`）
- E3D：`D:\AVEVA\Everything3D3.1`，项目根 `D:\AVEVA\Projects\E3D3.1`
- 目标项目：`AvevaMarineSample`（`project_code=1516`，DESI 库 dbnum 7997 / 7998 / 8000）
- 首个样本：BRAN `/C-IY-1R330-B`（FTUB/BEND 交替；当前生成结果中有三个构件明显脱离管线轴线）
- 制定日期：2026-08-04

---

## 1. 要解决的问题

模型生成结果目前**没有客观判据**：只能靠人眼在 Web 视图里看"像不像"。图里 `/C-IY-1R330-B` 的 BEND 偏位就是典型——看得出错，但说不出错多少、错在哪个参数、修完有没有真的修好、下次会不会回归。

方案的核心是：**把 AVEVA E3D 3.1 官方导出的 RVM 当作几何基准，与本仓生成的模型数据做结构化对拍，产出可复跑、带退出码的判据。**

---

## 2. 决策记录（2026-08-04 grill 结论）

| # | 决策点 | 结论 |
|---|--------|------|
| Q1 | 工具落点 | **整体移植**：把 `rvm_import` + `rvm_compare` 搬进 `old/gen-model`，自成闭环，不跨仓依赖 |
| Q2 | 目标项目 | `AvevaMarineSample`（dbnum 7997/7998/8000） |
| Q3 | gen 侧数据源 | **直读本仓 SurrealDB**，并把 L2 从"只比 noun"**升级为几何参数级对拍** |
| Q4 | RVM 侧落地 | **拆两步 + JSON 快照**，不引 rusqlite / 不搬 `ModelRelationStore` |
| Q5 | 驱动入口 | 新增 `src/bin/rvm_verify.rs` 探针（clap 驱动），不动 `run_app` 主流程 |
| Q6 | 首个导出范围 | 只导 BRAN `/C-IY-1R330-B` |
| Q7 | 导出口径 | **全导**（含障碍/保温），对拍侧按 `geo_type` 分桶豁免 |
| Q8 | 容差 | 平移 ≤1mm；长度/半径 ≤max(0.1mm, 0.1%)；角度 ≤0.1°；AABB 分量 ≤1mm |
| Q9 | 交付顺序 | **先建判据后修缺陷**：工具先把 BEND 偏位量化成红灯，再修，修完复跑转绿 |

约定（未单独提问，按本仓既有惯例定）：

- 样本 RVM/ATT 放 `test_data/rvm/`
- 机器报告落 `output/rvm-verify/<root>-<时间戳>.json`
- 人读摘要写 `docs/<日期>_rvm-baseline-verify-report.md`

---

## 3. 现状事实（已核实，非推测）

### 3.1 本仓（old/gen-model）

- 主程序**没有 CLI 开关**：`src/main.rs` 只有 `run_app(None)`，一切靠 `DbOption.toml` + `src/bin/*_probe.rs`（现有 19 个）+ HTTP `0.0.0.0:8022` 驱动。
- **没有 `export_model` 模块**（无 OBJ/GLB/Parquet 导出），生成结果全量落 SurrealDB（`v_port=8009`）+ `assets/meshes`。
- `Cargo.toml` **没有** `rvm-rs` / `rusqlite` / `parquet` / `arrow` / `polars`，也没有 `ModelRelationStore`。
- `src/rvm/` 是**同名但无关**的内部模块（PDMS 元素遍历，不解析 RVM 文件）——不要复用这个名字，避免混淆。

生成侧的 SurrealDB 数据面（取自 `src/fast_model/pdms_inst.rs`）：

| 表/边 | 形态 |
|-------|------|
| `inst_relate` | `<pe:refno> -> inst_info:⟨id⟩`，带 `world_trans: trans:⟨h⟩`、`aabb: aabb:⟨h⟩`、`generic`、`has_cata_neg`、`solid`、`zone_refno` |
| `geo_relate` | `inst_info:⟨id⟩ -> inst_geo:⟨geo_hash⟩`，带 `trans: trans:⟨h⟩`、`geom_refno: pe:R`、`pts:[...]`、`geo_type`、`visible` |
| `inst_geo` | 单位形状的**参数化几何**（由 rs-core 的 `gen_unit_geo_sur_json()` 生成）—— L2 参数级对拍的数据来源 |
| `tubi_relate` | `<bran> -> tubi_relate:[bran, idx] -> inst_geo:⟨tubi_geo_hash⟩`，TUBI 段 |
| `trans` / `aabb` | 值表，`{id: trans:⟨h⟩, d: <矩阵>}` / `{id: aabb:⟨h⟩, d: <包围盒>}` |

### 3.2 待移植的源（plant-model-gen，spec 009）

| 文件 | 行数 | 移植处置 |
|------|------|----------|
| `src/rvm_import.rs` | 1207 | **大部分照搬**：RVM/ATT 解析、身份解析、几何 payload 编码 |
| `src/rvm_compare.rs` | 573 | **对拍骨架照搬，`load_gen_side` 重写，L2 升级** |
| `src/model_relation_store.rs` | 425 | **不搬**（Q4=A） |

`rvm_import.rs` 的可复用零件（函数级）：

- 身份解析：`parse_default_name` / `named_owner_from_desc` / `full_noun_to_short` / `direct_refno_from_attrs` / `PeIdentRow`（按 name 查 `pe`）
- ATT 索引：`AttAttrIndex` / `load_att_attr_index` / `merge_att_attr_index`
- 几何编码：`geometry_kind_name` / `geometry_detail_payload` / `geometry_type_name` / `compute_payload_aabb` / `encode_geometry_blob`
- 回退身份：`stable_refno` / `stable_geo_hash`

`rvm_compare.rs` 现状的重要局限：注释里写明，因为基准侧是 Parquet（没有参数化几何），**L2 被降级成了只比 `noun`**。本仓有 `inst_geo`，这个降级不必继承。

### 3.3 E3D 3.1 导出侧

`PMLLIB\common\commands\designexport.pmlcmd` 显示，DESIGN 的 Export 按钮实际执行：

```
CALLXR XEXPMAIN RUN | CREATE | MODIFY | DELETE     -- 导出定义的增删改与执行
CDXDRVSEL                                          -- 导出驱动选择
!!cdxattdump                                       -- 属性导出（"Dumps attributes for CE and its offspring into a file for review"）
```

两个推论：

1. **几何（RVM）与属性（ATT）是两个独立动作**，必须分别做。
2. 导出是 **PML 可驱动**的（`!!runSynonym('CALLXR XEXPMAIN RUN')`）——导出定义建一次，之后可用宏一键重跑，不必每次手点。

---

## 4. 目标架构

```
E3D 3.1 (AvevaMarineSample)
   │  ① Export 驱动=RVM，CE=/C-IY-1R330-B，全导（含障碍/保温）
   │  ② cdxattdump 导 .att
   ▼
test_data/rvm/C-IY-1R330-B.rvm + .att
   │  ③ rvm_verify import
   ▼
test_data/rvm/C-IY-1R330-B.rvm.json        ← 基准快照（已解析真实 refno、含 geo_type 分桶）
   │
   │           SurrealDB(8009)  inst_relate / geo_relate / inst_geo / tubi_relate / trans / aabb
   │                  │
   └────────┬─────────┘
            │  ④ rvm_verify compare（L1 成员 / L2 参数 / L3 空间）
            ▼
   output/rvm-verify/<root>-<ts>.json   +   退出码 0/1
```

---

## 5. 工作分解

> 进度（2026-08-04）：阶段 0 完成，阶段 1 完成，阶段 2 完成 T2.1/T2.3/T2.4
> （身份解析 T2.2 只做了组名解析，查站点库那半截未接）。阶段 3 起未动。

### 阶段 0：样本落地（前置，无代码）✅

- **T0.1** 在 SurrealDB 里反查 `/C-IY-1R330-B` 的 dbnum 与 refno，确认落在 7997/7998/8000 中的哪个库。
- **T0.2** 在 E3D 3.1 打开 AvevaMarineSample，导出该 BRAN 的 RVM + ATT，落 `test_data/rvm/`（步骤见 §6）。
- **T0.3** 记录导出选项快照（驱动、范围、勾选项）到 `test_data/rvm/C-IY-1R330-B.export-note.md`，保证他人可复现同一基准。

**出口判据**：`.rvm` 与 `.att` 两个文件存在且非空；导出选项有文字记录。

### 阶段 1：依赖与骨架

- **T1.1** `Cargo.toml` 增 `rvm-rs = { git = "https://github.com/happyrust/rvm-rs" }`（与本仓刚完成的"public GitHub dependencies"方向一致），新增 feature `rvm_verify`，默认不开。
- **T1.2** 新建模块 `src/rvm_baseline/`（**避开已被占用的 `src/rvm/`**）：
  - `mod.rs`
  - `import.rs`：RVM/ATT → 基准快照
  - `identity.rs`：组名 → 真实 refno
  - `snapshot.rs`：快照 JSON 的读写与结构定义
  - `compare.rs`：三层对拍
  - `mapping.rs`：RVM 原语 ↔ PDMS 几何类型映射表
- **T1.3** 新建 `src/bin/rvm_verify.rs`（clap，两个子命令 `import` / `compare`），风格对齐现有 `*_probe.rs`。

**出口判据**：`cargo build --features rvm_verify --bin rvm_verify` 通过；`--help` 输出两个子命令。

### 阶段 2：import（RVM → 基准快照）

- **T2.1** 移植 RVM/ATT 解析与几何 payload 编码（`geometry_kind_name` / `geometry_detail_payload` / `compute_payload_aabb` / `encode_geometry_blob`）。
- **T2.2** 移植身份解析，把新仓的 SurrealDB 查询改接本仓 `SUL_DB`：命名元素按 name 精确查 `pe`；未命名成员按 `<NOUN> <n> of <OWNER>` 在 owner 的同 noun 子序列中定位；同名多解时按 owner 链逐级约束消歧；失败则回退 `stable_refno` 并标 `identity_source=stable_hash` / `resolved=false`。
- **T2.3** 定义快照结构并落盘（`snapshot.rs`）：

```jsonc
{
  "meta": { "dbnum": 7997, "root_refno": 0, "root_name": "/C-IY-1R330-B",
            "rvm_file": "...", "att_file": "...", "exported_at": "...",
            "resolved": 0, "unresolved": 0 },
  "members": [
    { "refno": 0, "name": "...", "noun": "BEND", "parent": 0,
      "resolved": true, "identity_source": "pe_name|default_name|stable_hash",
      "geo_type": "Primitive|Obstruction|Insulation",
      "aabb_world": [minx,miny,minz,maxx,maxy,maxz],
      "geometries": [
        { "kind": "CircularTorus", "detail": { /* 原语参数 */ },
          "transform": [ /* 4x4 列主序 */ ], "bbox_world": [ /* 6 */ ] }
      ] }
  ]
}
```

- **T2.4** import 结束打印统计：成员数、几何数、`resolved/unresolved`、按 `geo_type` 分桶计数。

**出口判据**：样本 BRAN 及全部成员 `resolved=true`、`unresolved=0`；BRAN 自身解析出的 refno 与 T0.1 反查一致。

**当前实测**：

```
cargo run --features rvm_verify --bin rvm_verify -- \
    import --rvm test_data/rvm/C-IY-1R330-B.rvm \
           --att test_data/rvm/C-IY-1R330-B.att \
           --dbnum 8000 --root-refno 24384/22404

root           : /C-IY-1R330-B
成员 / 几何    : 40 / 141
身份 已解析/未解析: 37 / 3
geo_type 分桶  : Primitive=141
退化包围盒     : 0
```

**身份解析比原计划简单得多**：ATT 里未命名元素的 `NAME` 字段直接就是
`=ref0/ref1` 形式的真实 refno（`FTUBE 1 of BRANCH /C-IY-1R330-B` → `=24384/22405`），
根本不用去站点库按 owner + 序号反查。`TYPE` 字段还给出权威 noun，比从默认命名猜更可靠。

未解析的 3 个是 SITE `/1RX03-EQUI`、ZONE `/1RX03-LCT`、PIPE `/1RX-330`——命名元素的
refno 不在 ATT 里，但它们几何数为 0，不是对拍目标，只作层级存在。根元素（BRAN）
同属此列，用 `--root-refno` 钉上即可。

**踩到的坑**：rvm-rs 的 `parse_att` 不会把属性挂回 `group.attributes`（实测 40 个成员
全空），所以 `src/rvm_baseline/att.rs` 自己解了一遍 ATT 文本。

成员形态：`/AMS/1RX03-EQUI/1RX03-LCT/1RX-330/C-IY-1R330-B/` 下 18 个 FTUBE + 18 个 BEND
交替，noun 与序号均已从默认命名解析出来，每个成员 3 或 9 个原语。

两个实现上踩到的点：

- RVM 的 File 节点带的是导出横幅（`AVEVA E3D Design Mk3.1.9…`），不是层级的一部分，
  放进路径会污染 `stable_id` 和报告可读性，已剔除。
- 导出根不能取「层级最浅的组」——RVM 会把 SITE/ZONE/PIPE 祖先一并带出来，
  取最浅会指到 SITE 上。正确口径是「第一个默认命名成员的 owner」。

### 阶段 3：compare（快照 vs SurrealDB）

- **T3.1** 写 `load_gen_side`（本仓版，**这是全新代码，不是照搬**）：按 root refno 展开 `inst_relate` 子树 → 关联 `inst_info` → `geo_relate` → `inst_geo` → 取 `trans` / `aabb`，并单独装载 `tubi_relate:[bran, idx]`。
- **T3.2** L1 成员清单：RVM 子树 vs gen 子树 ∪ tubi 段，输出 `matched / missing_in_gen / extra_in_gen`。
  - 豁免规则：`resolved=false` 单列 `unresolved_identity`，不计 missing；RVM 侧零几何成员（GASKET 等）豁免；BRAN 的隐式管段由 `tubi_relate` 代表。
  - **geo_type 分桶（Q7=B 的核心）**：`Obstruction` / `Insulation` 桶默认豁免 missing 判定，只记账不判红；`Primitive` 桶严格判定。
- **T3.3** L2 参数级：按 `mapping.rs` 做类型映射，逐参数比对（长度/半径/角度），超容差记 `param_mismatch`（含参数名、两侧值、偏差）。
- **T3.4** L3 空间级：world transform 平移偏差（mm）、旋转偏差（deg）、AABB 各分量偏差与中心/尺寸偏差。
- **T3.5** 报告落盘 + 控制台摘要 + 退出码（0=容差内全过，1=存在差异）。

**出口判据**：对样本产出报告；图里三个偏位 BEND 在报告中以 `translation_delta_mm` 明确超限的形式出现。

**当前实测（首个红灯已拿到）**：

```
cargo run --features rvm_verify --bin rvm_verify -- \
    compare --snapshot test_data/rvm/C-IY-1R330-B.rvm.json

参与判定       : 35
L1 匹配/缺失/多出: 35 / 0 / 0
豁免 无几何/未解析/非Primitive: 5 / 0 / 0
L2 RVM 原语分布 : Box=51 FacetGroup=90
L3 平移 比较/超限: 35 / 0    (最大 0.001 mm，容差 1 mm)
L3 AABB 比较/超限: 35 / 35   (最大 310.236 mm，容差 1 mm)
判定           : FAIL        退出码 1
```

**结论：成员齐、位置对、几何尺寸错。**

- L1 零缺零多 —— 生成侧一个构件都没漏、没多。
- L3 平移最大偏差 0.001 mm —— 实例的世界定位是准的，「BEND 偏位」不是定位问题。
- L3 AABB 全体超限 —— 问题出在几何本身的尺寸。

典型样本（X/Y 完全吻合，只差 Z）：

```
FTUBE 1  rvm: -21556.6, -9943.8, 430.0 .. -20829.3, -8540.8, 580.0   Z 跨度 150
         gen: -21556.6, -9943.8, 430.0 .. -20829.3, -8540.8, 480.0   Z 跨度  50
BEND 1   rvm: -21549.8, -8614.7, 430.0 .. -21467.5, -8505.9, 580.0
         gen: -21603.2, -8671.9, 427.2 .. -21410.5, -8443.5, 530.0
```

每个 FTUB 的 Z 跨度**整整少 100 mm**，而 X/Y 一毫米不差。

### 根因排查（含一次被推翻的中间结论）

> ⚠ 本小节前半段保留当时的原始观察与推理链，但**「漏几何」这个结论已于同日复核推翻**。
> 直接看末尾的「复核订正」。

拆开看 FTUBE 1 两侧的几何构成就清楚了。RVM 侧 **3 个 Box**：

```
[1] Box  Z 430..480     底板（略内缩）
[2] Box  Z 430..480     底板
[3] Box  Z 480..580     上方另一个 100mm 高的盒子
```

生成侧**只有 1 个可见几何**，正好等于 RVM 的 [2]：

```
PrimBox 单位立方, trans.scale = [1500, 103, 50], translation = [750, 0, 25]
```

再看目录侧，该构件的几何集 `pe:13244_51881` 下有 5 个 SBOX：

```
/ACP1000-TFVL-GS-BOARD         ← 只生成了这一个
/ACP1000-TFVL-GS-COVER
/ACP1000-TFVL-GS-CABLEWAY
/ACP1000-TFVL-GS-OBSTRUCTION   ← 障碍体，本就该排除
/ACP1000-TFVL-GS-RESERVED      ← 预留空间
（外加 CENTRELINE / OUTLINE-* 等 LINE 元素）
```

而 BOARD 自身的尺寸表达式求值是**对的**：

```
PXLE := ATTRIB RPRO TLEN                        → 1500  ✓（= 实例 HEIG）
PYLE := ( ATTRIB PARA[1] + 2 * ATTRIB PARA[4] ) →  103  ✓
PZLE := ATTRIB PARA[2]                          →   50  ✓（RVM 的底板正是 430..480）
```

继续往下追，当时判断 5 个 SBOX 里只有 GS-COVER 是真缺口（**此判断已被下方「复核订正」推翻**）：

| SBOX | LEVE | OBST | TUFL | 生成 | 判断（已订正） |
|------|------|------|------|------|----------------|
| GS-BOARD | [1,10] | 0 | true | ✓ | 正常 |
| GS-CABLEWAY | **[0,0]** | 0 | true | ✗ | `query.rs` 的 `is_visible_by_level` 正确过滤（0 级不显示） |
| GS-OBSTRUCTION | [1,10] | 2 | **false** | ✗ | 被 **TUFL=false** 挡在 `resolve_gms`，不是被 OBST 挡的 |
| GS-RESERVED | [1,10] | 1 | **false** | ✗ | 同上 |
| GS-COVER | [1,10] | 0 | true | ✗ | **求值结果本来就是 0（`COVR=0`），两侧一致，不是缺陷** |

COVER 与 BOARD 的 LEVE/OBST/NAPP 完全相同，唯一差别在尺寸表达式引用了什么：

```
BOARD  PZLE := ATTRIB PARA[2]
       PYLE := ( ATTRIB PARA[1] + 2 * ATTRIB PARA[4] )
COVER  PZLE := ( ATTRIB RPRO COVR * ATTRIB RPRO B )      ← RPRO 上的具名属性
       PYLE := ( ATTRIB RPRO A + ( 2 * ATTRIB RPRO C ) )
       PZ   := ( ( ATTRIB PARA[2] - ( ATTRIB RPRO B / 2 ) ) + ATTRIB RPRO C )
```

BOARD 只用 `ATTRIB RPRO TLEN` 和 `PARA[n]`，COVER 还要 `ATTRIB RPRO A / B / C / COVR`。

**吞错点**在 `src/fast_model/query.rs:30`：

```rust
let geom = query_gm_param(&geo_am, is_spro).await.unwrap_or_default();
gms.push(geom);
```

求值失败被 `unwrap_or_default()` 吞成一个空 `GmParam`，既不报错也不落库——
几何就这么静默消失了。这正是缺陷此前一直看不见的原因。

### 复核订正（2026-08-04，推翻上面的「漏几何」结论）

上面这条推理链错在**没有验证 `query_gm_param` 的实际行为**就把它当成了吞错点。逐条推翻：

**一、`query.rs:30` 不可能吞掉一个 SBOX。**
`query_gm_param` 返回的是 `Option<GmParam>` 而非 `Result`，全函数只有一个 `?`
（`expression/query_cata.rs:211`），且只存在于 `SEXT / NSEX / SREV / NSRE` 分支里——
SBOX 走不到那条路径，必然返回 `Some`。更关键的是 `GmParam` 里存的是**表达式原文字符串**
（`lengths: att.get_attr_strings(&["PXLE","PYLE","PZLE"])`），**求值根本不在这里发生**。
`unwrap_or_default()` 确实是个该修的坏味道，但它不是本缺陷的成因，也没有「错误」可打印。

**二、真正的过滤链和原先写的不一样。**
求值与几何丢弃发生在 rs-core 的 `expression/resolve.rs::resolve_gms`，它的第一道闸是
`if g.visible_flag`，而 `visible_flag` 来自 `TUFL`。所以 GS-OBSTRUCTION / GS-RESERVED
是被 **TUFL=false** 挡掉的，不是被 `OBST` 挡的（见上表已订正）。
`resolve_gms` 的 `Err` 分支本来就有 `println!("{}", e)`，并非静默。

**三、GS-COVER 本来就该是零高，两侧都没有它。**
它的 `PZLE := ( ATTRIB RPRO COVR * ATTRIB RPRO B )`，而 `RPRO COVR` 来自 DTSE
`/ACP1000-TFVL-DTSE`（`SCOM /ACP1000-TFVL-100` 的 `DTRE = pe:13244_51838`）下 `DKEY=COVR` 的 DATA：

```
PPRO := ( MIN ( 1, ( MAX (0, ATTRIB DESP[3 ]) ) ) )      PTYP := INT
```

该 FTUB 实例 `DESP = [0,0,0,1,1,0]` → `DESP[3] = 0` → **`COVR = 0`** → `PZLE = 0 × 10 = 0`。
**这条槽本来就没有盖板。** E3D 侧同样没有导出它（RVM 里找不到宽 109 的盒子）。

> 顺带排除另一个怀疑：`RPRO COVR` 是 `INT` 而求值上下文是 `DIST`，会走 `check_unit_compatible`。
> 但 `COMPATIBLE_UNIT_MAP` 把 `DIST` 与 `INT` 显式登记为兼容（`rs_surreal/resolve.rs:22-27`），不会报错。

### 那 100 mm 差额到底是什么

把 RVM 那 3 个盒子按世界 AABB 反解宽度（该构件在平面内旋转 65.0°，
解 `1500·cosθ + W·sinθ = X跨度`、`1500·sinθ + W·cosθ = Y跨度`）：

| RVM 盒 | 反解宽度 W | Z 范围 | 对应 SBOX | 目录表达式 |
|--------|-----------|--------|-----------|-----------|
| [1] | 100.0 | 430..480 | **GS-CABLEWAY** | `PYLE := PARA[1]` = 100，LEVE=[0,0] |
| [2] | 103.0 | 430..480 | **GS-BOARD** | `PYLE := PARA[1] + 2·PARA[4]` = 103 |
| [3] | 103.0 | 480..580 | **GS-RESERVED** | `PZLE := RPRO CLEA` = 100，`PZ := PARA[2] + CLEA/2` |

生成侧只出 [2]（`1500 × 103 × 50`），**完全正确**。

AABB 少的那 100 mm 就是 `GS-RESERVED` 的预留净空；基准侧还多出一个 0 级的 `GS-CABLEWAY`。
两者都是**导出口径**带进来的：Q7 选了「全导（含障碍/保温）」，E3D 侧 `repre obst on` 就把它们写进了 RVM。

**结论：首个红灯是对拍口径的假阳性，不是生成缺陷。**
Q7=B 原本配套的豁免规则（T3.2 的「Obstruction / Insulation 分桶豁免」）从未生效——
import 阶段 rvm-rs 把 141 个几何**全部标成了 `Primitive`**（快照里 `geo_type 分桶: Primitive=141`），
豁免分支永远命中不了。

### 修正后的下一步（三选一）

1. **收紧导出口径**：重导时 `repre obst off` / `repre insu off`，并把层级设成只含 `LEVE≥1`，
   让 RVM 只剩真正参与显示的实体几何。改动最小，但每换一个样本都要重导一次。
2. **在 import 侧补分桶**：不依赖 rvm-rs 的 `GeometryType`，改用「RVM 原语 ↔ 目录 SBOX」的
   尺寸/位置反解（就是上表的做法）给每个原语打标，再按 `TUFL` / `LEVE` 决定它该不该参与判定。
3. **改判据口径**：L3 的 AABB 从「成员包围盒整体比」改成「逐几何配对后再比」，
   基准侧多出来的几何单列 `extra_in_rvm`，只记账不判红。

在这三条之一落地之前，`compare` 的 AABB 判定对**任何**带预留空间/障碍体的构件都会误报，
不能作为回归判据使用。

### 关于 L2：只能是信息项，不能作判据

原计划（Q3=A）要把 L2 升到「几何参数级」，实测下来两侧几何表达根本不是一套：

- RVM 侧是 E3D 为渲染做的分解，本样本是 `Box=51` + `FacetGroup=90`（三角化）。
- 生成侧 `inst_geo.param` 是 catalogue 参数化几何（`PrimExtrusion` 等），
  同一个 BEND 给 6 个可见几何 + 若干 `CataNeg`，与 RVM 的 3 个原语无法一一对应。

所以判定落在 L1（成员在不在）+ L3（位置与尺寸对不对），L2 退为「RVM 原语分布」
的信息项。这两层已经足够把上面的缺陷定位到「几何尺寸」这一层。

### 阶段 4：首个红灯 → 修复 → 转绿

- **T4.1** 把首轮报告归档为 `docs/2026-08-04_rvm-baseline-verify-report.md`，作为缺陷基线。
- **T4.2** 依据报告定位 BEND 偏位根因（是 `inst_geo` 参数错、`geo_relate.trans` 局部变换错，还是 `inst_relate.world_trans` 世界变换错——三层报告可直接区分）。
- **T4.3** 修复后复跑同一条命令，退出码转 0。

### 阶段 5：常态化

- **T5.1** `scripts/rvm-verify.ps1`：一条命令跑完 import + compare + 归档。
- **T5.2** 样本集扩到该 BRAN 所在 PIPE / ZONE，覆盖 ELBO / VALV / FLAN / REDU / TEE。
- **T5.3** E3D 导出宏化（§6.3），让基准刷新也变成一键。

---

## 6. E3D 3.1 导出手册

### 6.1 底层原生命令（已从 E3D 自带 PML 反推出来，无需走表单）

`PMLUI/intf/review/mexpmain` 是 Design Export 表单点 Run 时**动态生成并执行**的临时宏。
把它写出来的命令固化下来，就得到一条完全可脚本化的导出序列，不需要建导出模板、
不需要表单、不需要驱动选择对话框：

```
repre insu on transl 0                      -- 保温：实体输出（Q7=B 全导）
repre obst on transl 0                      -- 障碍：实体输出
repre tube on                               -- 管子表现
export implied tube into separate containers -- 隐含管段各自独立容器（便于与 tubi_relate 逐段对齐）
export system /expdri.so                    -- Review(RVM) 驱动
export file "<绝对路径>.rvm"
export filenote '<备注>'
export holes on
export autocolour displayexport on
export repr on
export /C-IY-1R330-B                        -- 包含列表：目标元素
export finish                               -- 执行
```

驱动名来自 `%AVEVA_DESIGN_DFLTS%/export/driver-config`
（实际路径 `D:\AVEVA\Everything3D\Data\DFLTS3.1\export\driver-config`），
该文件里**只有一个驱动**：`DISPLAY\Review` / `EXECUTABLE\expdri.so`，
对应 `D:\AVEVA\Everything3D3.1\expdri.dll`。

现成宏：`scripts/e3d/rvm_export_c_iy_1r330_b.mac`，在 E3D 命令行里一句 `$M "<宏路径>"` 即可。

### 6.2 属性导出（ATT）

Export → **Attribute Dump**（`!!cdxattdump`，"Dumps attributes for CE and its offspring
into a file for review"）→ CE 设为同一 BRAN → 输出 `test_data\rvm\C-IY-1R330-B.att`。

> ATT 是身份解析的关键输入：有它才能把 RVM 组名稳定映射到真实 refno，把 `unresolved` 压到 0。

### 6.3 会话启动方式（实测结论）

| 方式 | 脚本 | 实测结果 |
|------|------|----------|
| 分离式 + 控制台注入 | `run_export_console.bat` + `console_inject.ps1` | 按键写进 CONIN$ 成功，但 des.exe 不消费——命令不生效 |
| 无图形 + stdin 重定向 | `run_rvm_export_nogfx.bat` | 会话起得来，stdin 被 core.dll 忽略，且报 LaaS 许可告警 |
| 启动入口宏 | `run_rvm_export_entrymacro.bat` | `AVEVA_DESIGN_ENTRYMACRO` 不被直接 des.exe 启动识别（与 `run_export_ams.bat` 里的既有考据一致） |
| 完整 appware GUI | `run_rvm_export_gui_appware.bat` | 首次弹 "The MDB is currently closed no definition sets can be created or loaded"；**清掉遗留 des.exe 会话后不再弹** |
| 完整 appware + 控制台通道 | 同上（加 `PDMS_SHOWCONSOLE=1`）| 会话正常，但注入的按键仍不被消费：`AttachConsole` 拿到的是 des.exe 自己分配的空控制台，不是 PDMSConsole 实际读的那个 |
| **CAF addin** | `GenModelRvmExport.dll` + `run_rvm_export_addin.bat` | **发命令这一环打通了**：addin 稳定加载、`Application.Idle` 后按时触发、`Command.CreateCommand(...).RunInPdms()` 执行成功（`$P` / `VAR` 探针全部返回 True）。卡在会话没有数据 |

### 6.4 命令执行 API（已确认）

```csharp
using Aveva.Core.Utilities.CommandLine;
Command c = Command.CreateCommand("export finish");
bool ok  = c.RunInPdms();          // 原生 PDMS 命令走这个
PdmsMessage err = c.Error;         // 失败时的 模块号/消息号
```

### 6.5 可用配方（2026-08-04 实测跑通）

**根因**：`D:\AVEVA\Everything3D3.1` 这份安装的 license 是死的
（`License check-out error (AVEVA201) : No LaaS Access Token configured`），
没 license → MDB 打不开 → `dbs=0` → 所有 `export` 命令回 `pdms 47/15`。
换成 `E:\reverse\e3d` 那套带 license 修复的启动链就正常。

```powershell
$env:GENMODEL_RVM_ELEMENT  = '/C-IY-1R330-B'
$env:GENMODEL_RVM_OUT      = 'D:\work\plant-code\old\gen-model\test_data\rvm\C-IY-1R330-B.rvm'
$env:GENMODEL_RVM_LOG      = 'D:\work\plant-code\old\gen-model\output\rvm_export_addin.log'
$env:GENMODEL_RVM_DELAY_MS = '30000'

pwsh -NoProfile -ExecutionPolicy Bypass -File "E:\reverse\e3d\launch_e3d_sample_repaired.ps1" `
    -UseShadowInstall -EarlyInit3DState `
    -ProjectCode ams -ProjectDirectory AvevaMarineSample -ProjectEnvPrefix AMS -Mdb /ALL
```

前置（已完成，换样本不用重做）：

- `GenModelRvmExport.dll` 编译进影子安装 `E:\reverse\e3d\shadow_e3d31_aps_all\`
- `GenModelRvmExport` 已加进 `E:\reverse\e3d\DesignAddins_no_multicad_with_viewer3d.xml`
  （启动器会把它复制成影子安装的 `DesignAddins.xml`；原文件已备份 `.bak_rvmexport_20260804`）
- 换样本只改 `GENMODEL_RVM_ELEMENT` / `GENMODEL_RVM_OUT` 两个环境变量

实测结果：

```
diag mdb=ALL   diag dbs=69   diag ce=/*
repre insu on transl 0   -> ok      ... 12 条命令全 ok
OK failedCommands=0 bytes=84480 -> test_data/rvm/C-IY-1R330-B.rvm
```

RVM 头部校验：`H E A D ... AVEVA E3D Design Design Mk3.1.9`，格式正确。

### 6.6 历史排查记录：会话起来了但 MDB 是空的

addin 自带的诊断给出确切结论：

```
diag mdb=                 <- MDB 名为空
diag dbs=0                <- 一个数据库都没有
diag ce=<invalid>         <- 没有当前元素
diag probe [$P ...]  RunInPdms=True Run=True InScope=True    <- 命令通道本身完全正常
repre insu on transl 0   -> ERR pdms 47/15
export ...               -> ERR pdms 47/15   (12 条全同)
```

也就是说 **`pdms 47/15` 就是「没有 MDB / 没有数据」**，不是命令写错。
`des.exe ams SYSTEM/XXXXXX /ALL` 起得来，但 MDB `/ALL` 没被打开，有时还会先弹
"The MDB is currently closed" 模态框——而这个模态框会**阻塞消息循环**，导致 addin 的
`Application.Idle` 永远不触发（表现为日志一行都不写）；点掉 OK 后 addin 立刻正常跑。

待查方向：`/ALL` 是否是项目 `ams` 的正确 MDB 名；`SYSTEM` 用户是否已被某个残留会话占用
（注意那个普通权限杀不掉的提权 des.exe，前一天 17:20 起就在跑）；强杀会话是否留下了
未清理的 MDB 占用记录，需要用 ADMIN 模块清。

**已排除**：MDB 打不开是遗留会话占用导致的，清理后消失。注意有一个提权的 des.exe（前一天 17:20 起）
普通权限杀不掉，下次再遇到 MDB 异常先查它。

**尚未打通**：向运行中的 E3D 会话发送一条命令。已知可行的自动化途径只剩 CAF addin
（就是 `scripts/e3d/NounLayoutExport.cs` + `DesignAddins.xml` 那一套已验证的机制），代价是要写 C# 插件。
在那之前，基准文件靠人工敲一行产出即可——每个样本只需一次。

**最短可行路径**：在一个 MDB 正常打开的 E3D DESIGN 会话里，命令窗口敲：

```
$M "D:/work/plant-code/old/gen-model/scripts/e3d/rvm_export_c_iy_1r330_b.mac"
```

跑通一次、确认 `.rvm` 落盘且非空之后，再回头做无人值守化。

---

## 7. RVM 原语 ↔ PDMS 几何映射（首轮草表）

| RVM 原语 | PDMS/E3D 几何 | 参数级比对项 |
|----------|---------------|--------------|
| `Cylinder` | CYLI / TUBE 段 | 半径、高度 |
| `Snout` | CONE / REDU | 上下半径、高度、偏心 |
| `CircularTorus` | ELBO / BEND / CTOR | 主半径、管半径、包角 |
| `RectangularTorus` | RTOR | 内外半径、高度、包角 |
| `Box` | BOX | 长宽高 |
| `Pyramid` | PYRA | 上下底尺寸、高度、偏移 |
| `SphericalDish` / `EllipticalDish` | DISH | 半径、高度 |
| `Sphere` | SPHE | 半径 |
| `FacetGroup` | 多面体 / 网格化几何 | 退化为 AABB + 顶点数（不做逐参数） |
| `Line` | 中心线 | 仅记录，不判定 |

> 这是**草表**，实现时必须按 `inst_geo` 里 `gen_unit_geo_sur_json()` 的真实 kind 字段名逐项校准，不能照抄。

---

## 8. 判定口径

| 层 | 判定项 | 容差 |
|----|--------|------|
| L1 | 成员 missing / extra（仅 `Primitive` 桶） | 0 |
| L2 | 长度 / 半径 | ≤ max(0.1mm, 0.1%) |
| L2 | 角度 | ≤ 0.1° |
| L3 | world 平移偏差 | ≤ 1mm |
| L3 | world 旋转偏差 | ≤ 0.1° |
| L3 | AABB 各分量偏差 | ≤ 1mm |

单位与坐标系：RVM 与生成侧同为 **mm** 与 **E3D world**，无需换算（spec 009 已在样本上验证过这一点，本仓首轮仍需复核一次）。

退出码：`0` = 全部在容差内；`1` = 存在超限差异（可直接进回归脚本）。

---

## 9. 验收标准

1. `rvm_verify import` 对样本产出快照，`resolved` 覆盖率 100%、`unresolved=0`，BRAN refno 与 SurrealDB 反查一致。
2. `rvm_verify compare` 产出报告：`Primitive` 桶 `missing=0` / `extra=0`，或差异逐项可解释（含 refno、参数名、两侧值、偏差）。
3. 图里三个偏位 BEND 在首轮报告中被明确标红，且指明是参数错还是变换错。
4. 报告可复跑（同输入同输出），退出码语义正确。
5. 命令、样本路径、E3D 导出选项全部写入文档，他人可独立复现。

---

## 10. 风险与未决项

| 项 | 说明 | 处置 |
|----|------|------|
| `geo_type` 语义不同 | RVM 的 `GeometryType`（Primitive/Obstruction/Insulation）与生成侧 `geo_relate.geo_type`（正/负实体等）**不是一回事**，不能直接对齐 | 分桶只在 RVM 侧做；生成侧不参与分桶判定 |
| 默认命名格式依赖语言包 | `<NOUN> <n> of <OWNER>` 在非英文环境可能不同 | 首轮样本已可验证；失败即回退 `stable_refno` 并计入 `unresolved` |
| 同名元素多解 | 按 name 查 `pe` 可能命中多个 | 按 owner 链路径逐级约束消歧 |
| `FacetGroup` 无法参数级比对 | 网格化几何没有解析参数 | 降级为 AABB + 顶点数比对 |
| 大样本内存 | 快照 JSON 全量加载，整 ZONE/整库时可能吃内存 | 单 BRAN 无压力；扩到 ZONE 时若成为瓶颈，再考虑改流式或引入 SQLite（即 Q4 的 B 方案） |
| 布尔运算差异 | 生成侧 `has_cata_neg` 的构件做过布尔，RVM 侧可能是原始原语 | 首轮先观察差异分布，必要时对布尔构件降级为 AABB 判定 |

---

## 11. 不做的事

- 不做三角网格顶点级 diff（AABB + 参数级已足够定位问题）。
- 不做 ATT 属性值对比（本期只对几何与结构，ATT 只用于身份解析）。
- 不修改 `gen_model` 生成逻辑本身（阶段 4 的修复另行记录）。
- 不做全库批量对拍调度（单 root 粒度优先）。
- 不引入 `rusqlite` / `arrow` / `parquet` 到本仓。
