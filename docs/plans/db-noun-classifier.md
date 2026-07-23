# 开发方案：DB_Noun 类型分类器（以 core.dll 为准 · 直接解析字典）

> `/grill-with-docs` 产出。决策见 `docs/adr/ADR-004`；core.dll 实测见 `teach/learning-records/0003`；术语见 `CONTEXT.md`。独立于 ADR-002/003（增量/关联）的"类型分类层"。

## 1. 目标与验收
- 在 gen-model 实现与 core.dll 一致的分类器：`primitive` / `geomset` / `extrusion` / `isPointsetPoint` / `graphicsBehaviour`（+ `hashValue` / `findNoun`）。
- 数据以 core.dll 为准 = **直接离线解析 dabacon 字典**（不走 dump）。
- 验收（无 dump 黄金）：与现有 curated 名单交叉核对无冲突 + 已知 noun 抽查一致（SCYL=primitive、SCOM 系目录…）。

## 2. core.dll 事实基线（详见 `teach/learning-records/0003`）
- 方法读 dabacon 字段：`primitive#659518` / `geomset#859903` / `extrusion#663225` / `isPointsetPoint#290555737` / `graphicsBehaviour←5099119`；`hashValue`=`this+0x5C`；`findNoun`=静态 `dictionary_` map + 懒建 + UDET 分流。
- 运行期表 `dword_6C3F6C0`(512 stride)+平行表 `6C08EE0/6C10EE0/6C18EE0`+count `6C3F6BC`；实测 **.bss 全 0、运行期加载**（不可从二进制静态读）。
- **加载器 = `sub_55F4290` = `ATTOPE`**（"ATTribute OPEn"）：以 `OLD, READ` / `READONLY, DB, BL` 打开 **Attribute Data File**（E3D dabacon 属性/类型定义字典，错误串 "Unable to open the Attribute Data File"），填 512-int-stride `[record][field]` 表 + 平行索引（`6C08EE0` hash→rec、`6C356B0/6C2B6B0` 名序索引、`6C216B0` 键）。这就是字典文件格式规格。
- flag 不在 `all_attr_info.json`。

## 3. 分期（直接解析，ADR-004）

### 阶段 1 · RE 定位（已完成初判）
- **路 A 已判**：现有 dabacon 元素解析器是 **schema 驱动**，且 `659518/…` 的 schema 不在 in-repo（`all_attr_info.json` 无）→ 单靠现有解析器**不能**解出 flag。
- **路 B（采用）**：加载器 `sub_55F4290` = `ATTOPE`（已定位）——打开 **Attribute Data File**（dabacon 属性/类型定义字典）填 512-int-stride `[record][field]` 表。
- **下一步**：① RE 记录格式细节（`sub_55F4FFC` 填表 + `sub_5391D48` 索引）；② 在 E3D 安装里定位该 Attribute Data File 实体（路径/格式头）；③ 写离线解析。

### 阶段 2 · 离线 dict 解析器 → `noun_flags.json`  ✅ 已完成（2026-07-24 · gen-model-10）

> **值 cell 已打通**（详见 `teach/learning-records/0004` 末节「✅ 值 cell 已打通」）：`dict.rs` 两级取值（step1 off → step2 value + base_type 继承 + 默认表）与 `ATNLOG`(`sub_55BC98B`) 反编译**逐行吻合**，已对真实 `attlib.dat`（1931 noun/93 field）跑通并**严谨交叉核对**：设计图元 8/8、元件库几何 31/31、挤出 9/9 一致（NSBO/NSCY 属名单双列，dict 更准）；分布 primitive=347/geomset=44/extrusion=38、primitive∩geomset=0。**产出仓库根 `noun_flags.json`**。
> **发现**：管件 noun（ELBO/VALV/TUBI/NOZZ/… 28 个）在 dict 里 `primitive=true`（= "设计级几何叶子"语义，非"数学基本形状"），与现 `PIPING_NOUN_NAMES`/目录实例化桶口径不同 → 阶段 3 应以 dict flag 为准重估分类。

- **代码位置**（决策）：扩展 vendored `vendor/aios-parse-pdms`，新增 `dict` 子模块（复用其 dabacon 2KB 分页读；索引 + 2D 取值 + base_type 继承 + 默认表 + 类型解 + 导出入口）。
- **格式已 RE 清楚**：Attribute Data File = **分页 dabacon 文件，页 = 512×int32(2KB)**；`FHDBRN(handle, pageNum, dest512)` 按页读；`ATGTIX`(`sub_55F4FFC`) 扫描建索引：key(noun/attr hash，范围 `[531442, 387951929]`) → 记录地址 `page*512+offset`；`ATRDRC`(`sub_5391D48`) 按页读入 512-stride 缓存；字段值经 `6C10EE0/6C18EE0` 偏移表 + 2D `[rec][field]` 定位、按类型(int/bool/array)取。
- **格式已完全 RE**（详见 `teach/learning-records/0004`）：`FHDBRN`=`sub_5B9B400`(通用 2KB 页读，gen-model 已有等价)；取值 `value = page[rowBase + col]`（noun 记录行 × 字段列），空则 **base_type 继承链**上溯、再默认表兜底；类型 1=bool/3=int/4=array。
- 解析器 = 现有分页层 + 新增「① `ATGTIX` 索引(fieldId→(col,type)、nounHash→(page,rowBase)) ② 2D 取值 ③ 继承/默认 ④ 类型解」。
- 产出：按 noun 导出 `{noun_hash, noun_name, primitive, geomset, extrusion, isPointsetPoint, graphicsBehaviour}`（顺带全量字段），记录来源字典版本。

### 阶段 3 · gen-model `NounClassifier` 模块  🔄 进行中（2026-07-24 · gen-model-10）
- 像 `all_attr_info.json` 一样加载 `noun_flags.json`；实现 7 方法（布尔/枚举取表；`hashValue`=db1 hash；`findNoun`=hash→noun + UDET 分流）。
- 整合：现有散落名单判定逐步改走 `NounClassifier`；名单暂留作交叉核对。

> **已落地**：`parse_pdms_db::dict::NounClassifier`（`from_flags/from_json_path/from_attr_file`；`primitive/geomset/extrusion/isPointsetPoint/graphics_behaviour` + `hash_value/find_noun/contains` + `primitive_nouns()/geomset_nouns()/extrusion_nouns()` 集合访问器）。测试：`classifier_divergence_map`（分歧图）、`routing_lists_are_dict_validated`（**守护**：路由名单 ⊆ 对应 flag，加载已提交 `noun_flags.json`，随常规构建跑，防漂移）、`export_stage3_gap_report`（产出缺口清单）。
>
> **关键结论（决定迁移策略）**：dict `primitive`=「设计级几何叶子」广义语义（347 个，**含管件**），**≠ 生成路由桶**；现有路由名单（`GNERAL_PRIM`→prim_model 等）都是对应 flag 的**准确子集**，**不能用 `primitive_nouns()` 盲替**（否则管件误路由到 prim_model）。→ 采**保守策略**：分类器作权威 flag 源 + 守护校验；路由改动须逐个人工核对。
>
> **缺口清单**（dict 认几何、gen-model 路由未覆盖，供人工审）：见 `docs/plans/stage3-noun-routing-gaps.md`——extrusion 缺 29、geomset 缺 13、primitive「其它」283（含 A* 关联几何 / AID* 构造辅助 / H*·HV* 吊架暖通 / CT* 桥架 / FE* 分析模型 / DOOR·WINDOW·LADDER 等建筑构件；多数由 cata_model/HANG/结构等**其它路径**覆盖或本就不渲染，非全是漏几何 bug）。

### 阶段 4 · 验证与测试
- 与现有 curated 名单交叉核对（冲突人工裁决，多半名单不全/版本差异）+ 已知 noun 抽查断言。

## 4. 范围（推荐）
- 方法：用户列的 6 个 + `hashValue`/`findNoun` 复用 gen-model 已有 hash↔name。
- 字段：核心 5 flag 必做；解析时顺带全量 ~20 字段（近零成本）备将来。

## 5. 风险与未决
- **主风险**：RE `sub_55F4290` + dabacon 字典文件格式的深度 → 阶段 1 先试"路 A（现有解析器读 UDET 记录）"降风险。
- 字典随 E3D 版本/项目而变，需版本化。
- `graphicsBehaviour` 枚举值语义（画法分派用途）需另行对照。
- 无 dump 黄金 → 验证靠 curated 名单 + 抽查，覆盖度弱于逐 noun 对拍（可接受，ADR-004）。
