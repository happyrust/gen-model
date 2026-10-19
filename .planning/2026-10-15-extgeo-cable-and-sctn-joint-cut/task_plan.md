# GitHub issue 修复收口：EXTGEO 电缆几何 + SCTN 关节端切 + 人工真机项

> 计划 ID：`2026-10-15-extgeo-cable-and-sctn-joint-cut`
> 创建：2026-10-15
> 状态：**approved / in_progress，尚未收口**（`gate_result.json` = approved；截至 2026-10-17 的只读核验见 §10：B 四例原生对拍有通过记录，C 级联已默认开启；A 的非零偏移/7200 覆盖、D 端切裁决、E 人工真机与 F 部分收尾仍待完成）。
> 缘起：issue 审核 → 收口合并（`a9cee1632` 进 refactor、进 feat 分支）已完；本计划承接剩余两条自动化的几何缺口（#35 EXTGEO 段形状、#34 E1 的 SJOI/PJOI 端切）与三件不可自动化的人工真机项。
> 范围仓库：`gen-model`（`feat/issues-36-50-34`，PR #51）+ vendor `e3d-model`（main，远端 `github`）。

## 1. 进展审核（历史快照：截至 gen-model `b6aabd5e` / vendor `07a200f`；当前状态见 §10）

### 1.1 已落地

| 线 | 落点 | 状态 |
|---|---|---|
| 收口合并 | refactor `a9cee1632`（sweep 批 11 提交）、feat `064ab3fc`+`abc4ae68` | 已推送；PR #48/#49 已被取代关闭 |
| #50 AC3 | vendor `1f049e7`（WALL 隐式标架宿主进 main） | ✅ |
| #36 刚体排除 | `anchor_subtree_*` 三类排除在 refactor/feat | ✅ 3733 窗对拍「不等·锚点 = 0」 |
| #34 前置 I1–I3 | 跨库边 12/12、两真库窗对端根进 regen、九窗钉死 | ✅ |
| #34 E1 上半场 | vendor `a7ec940`：SCTN JOIS/JOIE mode 2、成员链 mode 1/2/3/6、TMPL mode 7、宿主 FIXING 扫描 | ⚠️ 部分落地（见 §1.2） |
| #35 上半场 | vendor `420d1f4`：EXTGEO 锚点解算层（STAREF/ENDREF + RNODE/RATTA/HATTA/TATTA + 同 CWBRAN 遍历） | ⚠️ 只解锚点，未出形状（见 §1.2） |
| #38 第二手 | 全语料复量 10744/264072 = 4.1%、红 0；维持 A 不接线，留档 B′ 窄变体 | ✅ 已拍板（dev-plan ★1015、issue 已评） |

### 1.2 两个真实缺口（本计划主线）

**缺口 A（#35）**：`420d1f4` 只解锚点。旧探针的 `cables=32 extgeo=30 resolved=30` 会静默排除任一成员出错的整条 CABLE，不能代表全语料成功率。本轮修正后五目录按物理源文件计数：116 条 CABLE、280 个 EXTGEO，108 个可解、172 个因 FRAD 倒圆路径未支持而显式 unresolved。当前生成器尚未还原借用桥架子路径、端点截取、偏移和电缆截面。Core3D `sub_105D13B0` 已核实只是构造器，路径生成在 `sub_105D1F80`，但未获得原生 RVM 对照。1050–4490mm 间隙既不能据此认定合理，也不能靠猜直线补齐。**在形状语义经原生对拍确认前，gen-model 侧 RPATH 接线与 `model_cable_endpoint_cascade` 翻默认都不该动。**

**缺口 B（#34 E1）**：aps7200/aps250110 中已记账的无源 SJOI 仍保留 `negatives_skipped`。不能推广为所有 SJOI/PJOI 都无目录源：本轮读取的 aps250102 PJOI 带非零 SPRE。Core3D 的 `DB_Sjoi` vtable+8 指向共用 `sub_10722680`，无目录源分支不能直接推出“缺少 CUTP/CUTB 负体刀”。`section_cut_planes` 已按 SCTN 的 POSS/POSE/DRNS/DRNE 裁两端，`GTSTDT@0x1036CEC4` 原文也走这四项；PML 中已核到的 CUTB 位移 + RECONN 属于 SCTN→GENSEC 转换，不能套到原 SCTN；另有修剪/重连宏把 CUTP 与宿主端面方向联动（见 §11）。因此不得仅凭 CUTB/CUTP 再扣负体或把 CUTP 直接当世界系刀面。**需要同一 SCTN 的 SAVEWORK/RVM 与端点属性前后对照，才能裁决是否仍有未覆盖端切**。

### 1.3 人工项（自动化封顶，列计划防丢）

- live #2（#36/#50 共用）：E3D TTY 挪 ZONE → SAVEWORK → 回执/单元对照。
- #34 SAVEWORK 端部切割对拍：顺带裁决缺口 B。
- #45 FLUSH 链：隔离环境会话重编号对照。

### 1.4 工作树杂项

`src/web_service/handlers.rs` 有一处未提交改动（virtual-hole 磁盘副本 JS/WASM MIME 修复 + 测试，与本计划无关）；`src/bin/noun_attr_dump_probe.rs` 等探针文件未入库（按惯例 verification/ 产物不入库，探针源码是否入库见 D3）。

## 2. 下一步开发计划

### Phase A — EXTGEO 段形状取证（vendor，先量后写）

- [x] **A0 取证探针对账修复**：逐成员保留解析错误，不再使一条失败 EXTGEO 隐去整条 CABLE；扫描分母与成员分子独立对账，按源文件独立 DbSet、源工程优先解析同号库，导出带源路径/引用/偏移/失败原因的 JSON。缺失目录、空语料、扫描失败不得通过；严格模式拒绝 unresolved。五目录实测 116/116 CABLE、280/280 EXTGEO 已入账（108 可解、172 未解），不代表几何通过。
- [ ] **A1 样本 RVM 对拍**：从 30 条已解锚的 EXTGEO 里挑 3–5 条代表性样本（覆盖：大间隙 ~4490mm 的、中间隙 ~1050–1303mm 的、XOFF/YGAP 非零的），用 gen_ams/RVM 导出现状输出，与 E3D SAVEWORK 后的 RVM 对照段形状。
- [ ] **A2 `sub_105D13B0` 语义核实**：结合对拍结果定案 EXTGEO 段类型——(a) 直弦到外部锚点；(b) 顺 CWBRAN 路由（途经中间节点）；(c) 外轮廓扫掠。同 CWBRAN 途经点数已在探针里打印，可直接复核。
- [ ] **A3 XOFF/YGAP 语义**：两个偏移量作用在锚点还是段上，随 A2 一并定。
- 验收：每条样本给出「形状类型 + 置信依据」；不确定的样本如实标 `ambiguous`，不许猜。

### Phase B — EXTGEO 段几何实现（vendor，`../vendor/e3d-model/src/cable.rs`）

- [x] **B0 实验构建切片**：OUTD 属性表求值、同一开口 CWBRAN 的直段子路径裁取与反向、部分对正偏移、圆截面直段/斜接、全量与单元共用入口已实现；只在 E3D_MODEL_CABLE_GEOMETRY_EXPERIMENTAL=1 下参与构建和模型单元判定。
- [x] **B1** 按原生 RVM 对拍定论实现 `EXTGEO` 段与交界几何（`FRAD` 桥架内圆角与缩减段基线、`POINTR <-> EXTGEO` `RNODE` 锚点倒圆、共点/共线/立管垂直出口/横向跨桥架角平分面斜切封顶、半径分档圆弧分段数、平行输运 8 边形环直接网格焊接与 `180°` 折返拆件、横向 `RNODE` 跨距不足空体门、`gen_ams` 单 `FacetGroup` 导出）。
- [x] **B2** 4 条原生 TTY 样本复跑对拍通过（见 §9）：`2013286723/135` 双向表面 `max = 2.994 / 3.709 mm`（`p95 = 2.038 / 2.123 mm`，`rel_vol = 0.0208%`）；`2013286723/16` 双向表面 `max = 0.271 / 0.305 mm`（`p95 = 0.179 / 0.209 mm`，`rel_vol = 0.0085%`）；`2013286723/817` 双向表面 `max = 1.144 / 0.681 mm`（`p95 = 0.394 / 0.338 mm`，`rel_vol = 0.1898%`）；`2013286720/575` 输出 `0 PRIM`（与原生空体完全一致）。
- [x] **B3** vendor 单测与真库忽略用例全绿：`cargo test --lib` 373 passed / 0 failed；`cargo test --lib cable -- --ignored` 4 passed / 0 failed（含开关两态与 135/16/817/575 真库回归）。
- 依赖：Phase A 结论；若 A 判出多型并存则按占比排序实现、少数型记 skipped。

### Phase C — gen-model 侧 #35 接线（B 落地后）

- [x] **C1** RPATH/EXTGEO 进增量级联（已完成，2026-10-17）：`cable_endpoint::rule_for` 纳入桥架附着点 `HATTA` / `TATTA` / `RATTA` / `RNODE`（`Rule::Route`），`sources_from_plan_with_owners` 将落在 `CWBRAN` 子树内的 `RPATH` / `POINTR` / 桥架件沿 T 端 `owners` 上卷到所属 `CWBRAN` 根并过滤 `Disposition::NoModel`。
- [x] **C2** 翻默认前置（已完成，2026-10-17）：落盘 `docs/plans/2026-10-17-cable-endpoint-cascade-default-on-checklist.md`，完成 30 个真库窗口两态对拍 + APS 跨库 196 条端点边与 152 条 `EXTGEO` 路由边真库门全绿。
- [x] **C3** 翻默认 + `src/options.rs` / `DbOption.toml` / `scripts/templates/DbOption.factory.toml` / `CONTEXT.md` 四处一致（已完成，2026-10-17）。

### Phase D — SCTN 关节端切裁决与补刀（vendor，#34 E1 收口）

- [x] **D1 `DB_Sjoi` 逆向取证**（2026-10-19 闭合）：静态链路（§11）+ idalib 直开 `Core3D.dll.i64` 补全 `GJLORI→GTJLID→GTPLID→GTNAID`（JUSL 缺失回退中性轴）；动态证据改为**逐类原生 RVM 对拍**——隔离副本 7200 挂载后，/5902（J-C）、/78 /84（J-B）、/6427（M）、/8622 /8626（M+SBFI）、/8599 /8600（P5）、/17 /5951 /8577（P6）全部 `expdri.so` 导出并与生成体逐件对拍，属性探针二分定位崩溃属性（JSTY/POSS/POSE/TMPL/CUTB/JFREE），绕过写路径崩溃族。证据根 `verification/e3d-tty-native-20261015-075723/sctn_7200_evi-20261019/`。
- [x] **D2 分类后实现或记缺**（2026-10-19 闭合）：按拓扑分型记账——J-A/J-B「原生无额外刀/端面已覆盖」→ `joint_cuts_explained`（101+121 @7200、35+35 @250110）；J-C（PJOI 真空，已原生验证 5899/7613）与 M 中 CMFI（原生空组）留 `negatives_skipped` 作已知缺口；SBFI∈CMFI 的 mode-6 NGMR 刀属「确有原生额外刀」→ 已实现（`ImplicitFrame::SubFitting` 平放标架，8622/8626 双向 0.0014 mm）；未凭 CUTP 非零盲补任何刀。
- [x] **D3** aps7200/aps250110 分类复对拍（2026-10-19 闭合）：每类代表样本均有宿主 SCTN + 关节/成员 refno + 原生 RVM + 分类 + 双向采样差——SJOI /78 /84（0.0063 mm）、PJOI 5899/7613（无源，组空）、FITT/SBFITTING 8622/8626（0.0004/0.0004 mm）、宿主 SCTN（0.0014 mm）、centreline 三样本（仅 Line）；统计双计数（使用位置数 = 独立 refno 数）见 `2026-10-18-sctn-model-generation-fixes/plan.md` §9 D3 表；skipped 维持非零的均逐条记为已解释或明列缺口。

### Phase E — 人工真机项（向导 + 记录位，非自动化）

- [ ] **E1 live #2**（#36/#50）：`scripts/Run-AutoVerification.ps1` 向导；产出贴 #36/#50。
- [ ] **E2 SAVEWORK 对拍**（#34）：重点看 skipped SJOI/PJOI 的端切——裁决缺口 B 是真缺口还是 E3D 本就不切。
- [ ] **E3 FLUSH 链**（#45）：隔离环境，会话重编号前后对照。
- 每项完成 → 对应 issue 评论更新验收栏。

### Phase F — 收尾

- [ ] changelog 追记（vendor 两笔 + gen-model 侧接线）。
- [ ] issue #34/#35 最终状态评论（E1/I 系勾完后 #34 才能进「可翻默认评估」）。
- [x] refactor ↔ feat 分支同步（截至 2026-10-17，`0efb7ccaa` 已同时包含在 feat、`origin/refactor`；本地 `refactor` 分支指针另落后当前远端 2 提交，与本计划同步无关）。

### 排序

A → B → C（#35 串行链）‖ D（#34 E1 与 A/B 无依赖，可并行）→ E（人工项穿插，E2 是 D 的裁决输入之一，尽早排）→ F。
**硬约束：A 定论前不动 B/C；E2 出结果前 #34 不翻默认。**

## 3. 风险

- **R1 `sub_105D13B0` 语义误判**：对拍样本太少可能选错型——A1 覆盖三档间隙 + XOFF/YGAP 变体，歧义样本必须显式标注。
- **R2 SJOI/PJOI builder 逆不动**：`CUTP`/`CUTB` 若是 E3D 内部编码（非属性面直读），D2 走「记缺」分支——这是可接受的出口，但要在 issue 上说清「skipped ≠ 已修」。
- **R3 EXTGEO 间隙本就合理**：4490mm 间隙也许是 E3D 真实的「悬空跨段」（电缆出桥架走线管），A 阶段必须区分「我们没接住」与「E3D 本就断开」——判据是 E3D RVM 里那一段长什么样。
- **R4 级联同开交互**：C 接线时注意与 `model_structural_connection_cascade` / `model_design_reference_cascade` 的退化语义一致（参照 D10「级联关着 ⇒ 不分桶」先例）。

## 4. 待拍板

- **D1 EXTGEO 形状**（Phase A 输出后拍）：直弦 / 顺路径 / 扫掠 / 多型混合——A 完成后回来定。
- **D2 SJOI/PJOI 路线**：逆向实现 vs 记为已知缺口——D1 取证后定，倾向「先试着逆，逆不动记缺」。
- **D3 探针入库**：`src/bin/noun_attr_dump_probe.rs`（含本轮加的 `--refno`）要不要提交进 feat 分支——它是 #35/#34 取证的长期工具，倾向入库。
- **D4 `src/web_service/handlers.rs` MIME 修复**：独立提交到 feat（与本计划无关但压着碍事）还是留给你自己处理。
- **D5 人工项排期**：E1/E2/E3 哪天能借到 E3D 环境——不挡 A/B/D，但挡 #34/#36/#45/#50 的最终验收。

## 5. 验证矩阵

| 项 | 验证 | 判据 |
|---|---|---|
| A | 样本 RVM 对拍 | 每条样本形状类型 + 依据，歧义显式标 |
| B | 语料连续性复跑 + vendor 单测 | 间隙收敛 / 单测绿 / 无回归 |
| C | 两真库窗 + 跨库窗两态 | 对端根进 regen、桶位一致 |
| D | aps7200/aps250110 复对拍 | skipped 收窄、新减体全 SCTN |
| E | 人工真机 | 回执/切割/会话号逐项对照，结果贴 issue |

## 6. 当时的执行记录与阻塞（历史快照；后续 §7–§9 及 §10 可覆盖本节结论）

### 已实施与已验证

- vendor 探针回归先红后绿：先补独立分母断言，旧实现复现 `CABLE 32 != 104`；修复逐成员错误保留后，默认三目录 104 CABLE / 248 EXTGEO 全入账，76 可解 / 172 未解。再以五目录复跑，源文件钉住、项目本地 resolver 优先，116 CABLE / 280 EXTGEO 全入账，108 可解 / 172 未解，CABLE 路由级错误为 0。五目录数字按物理源文件计数，含同号副本，不冒充去重后的交付对象数。
- 报告：`verification/extgeo-complete-census.json` 与 `verification/extgeo-complete-census.log`；环境旋钮为 AIOS_CABLE_PROBE_REPORT（JSON 路径）、AIOS_CABLE_PROBE_STRICT=1（拒绝未解），语料目录用 AIOS_CABLE_PROBE_DIRS 显式给出 APS、AMS、AvevaCatalogue、ZDJ、YCYK-E3D 的五个数据目录。
- 严格模式已实测因 172 个未解项退出 101，这是正确拒绝，不是几何通过；普通模式通过只证明扫描/归因对账，JSON 中 geometry_validated 固定为 false。
- 现有 vendor 单测：`cargo test --lib` = 366 passed / 0 failed / 1 ignored；rustfmt 检查和 git diff --check 通过。缺失语料目录也已实测退出 101；未改变生产几何算法及任何默认开关。
- SigMap scaffold 已执行；修正跨仓/简写路径后 verify-plan 与 verify-ai-output 都为 0 问题。review-pr 已执行，但 vendor 报 filesChanged=0，未覆盖当前未暂存 diff，不能当成有效审查证明；本轮另人工逐项检查了实际 git diff。
- 属性探针修复：混用 noun 配额与显式 refno 时，配额耗尽不得提前停止 refno 查找；找不到显式 refno 必须非零退出。清除该探针的两个未使用项；源码仍未提交。release 构建通过；混合模式实测在 noun 配额耗尽后仍输出 aps250179 的 2013286723/140 与 aps7200 的 23584/10358；缺失 refno 实测退出 1。
- D1 属性普查已扩大到 APS 目录全部物理源文件：输出 SJOI 2127 条、PJOI 484 条，产物 `verification/joint-corpus-attrs.log`，保留文件分组、owner、成员与 CUTP/CUTB/SPRE 等完整属性；这些是物理文件扫描数，不是去重后的生产对象数。普查本身不代表坐标系及重新连接语义已还原。

### Core3D 与真实属性证据

- IDA 实例：Core3D 数据库位于 D:/ida_scratch/plant3/Core3D.dll.i64，通过 ida-bridge 只读执行；没有修改厂商文件。
- EXTGEO 的 0x105D13B0 是 MDR_CSGExternalGeometry 构造器；vtable+12 = 0x105D3060（getBranchData），vtable+88 = 0x105D3770（getSegmentData），vtable+16 = 0x105D1F80（createCurvesFromPath）。后者实际遍历桥架路径的 POINTR、EXTGEO、RSECT/CURVE，并处理端点区间与反向，排除了“无条件直弦即可完成”的实现路线。
- XOFF/YGAP 不只是锚点常量平移：0x105D2960 还读 CWJUSL/VWIDT/PDIR/LYDIR；0x10610530 与 0x10611780 分别沿 AYDIR/LYDIR 叠加 YGAP；0x10613090 经 CABLE 的 PROPRE.OUTD 和 CWBRAN.VSHAPE 求截面相关偏移。完整公式和 RVM 对照尚未闭合。本轮 280 个 EXTGEO 的 XOFF/YGAP 全为零，不能宣称覆盖非零偏移样本。
- SCTN 的 GTSTDT（0x1036CEC4）已复核：端点来源为 POSS/POSE、法向来源为 DRNS/DRNE。aps250110 的 SCTN 2013286654/68 已保存非轴向 DRNS/DRNE；其 JOIE 指向 SJOI 2013286654/72，该 SJOI CUTB=0、SPRE=0。此样本支持“切面可能已写入 SCTN 属性”的调查方向，但还不是原生 RVM 结论。
- PJOI 反例：aps250102 的 2013286646/500 和 /506 带 SPRE=23704/1584476、CUTB=8；/582 带 SPRE=23704/1584492、CUTB=6.5。不得继续写“所有 PJOI 无几何源”。

### 下一道门与尚未执行项

- A1 选样：aps250179 的 CABLE 2013286723/135（EXTGEO /140、/141，覆盖大间隙与相邻借用段）；aps7200 的 CABLE 23584/10343（EXTGEO /10347，跨多段桥架）和 23584/10351（EXTGEO /10358，同一引用的退化段）。原生导出与对拍已部分推进，最新结果见 §7；7200 候选仍未导出。非零 XOFF/YGAP 要在隔离工程中另造受控样本。
- B/C 保持未启动：不能用本生成器输出自证 Core3D 几何正确，也不能为了间隙小于 10mm 去补造连接段。结构连接与电缆级联默认仍关；刚体便宜路与单元级发布保持既有开态。
- D1/D2 目前只有只读取证，尚未实现新的端切刀；保留 skipped 账，不以“无源”等同“必然缺刀”，也不以“已有 DRNS/DRNE”等同“全部正确”。D3 的减体/原生对拍门未完成。
- E1 准备检查发现旧向导不可直接当验收：四个宏的日志/RVM 路径仍指向 gen-model-refactor，而 PowerShell 向导从 gen-model 读文件；实际 model/records 路由是 POST，返回模型记录而非脚本期望的 units.kind 增量回执；脚本引用的 Python RVM 对比器不存在，当前仓库只有 Rust 同名 bin。仅 PowerShell 语法检查通过不能盖过这些语义问题。本轮未执行任何挪位/SAVEWORK/FLUSH。
- E2/E3 的 SAVEWORK/FLUSH 链仍未执行。用户随后要求通过 TTY 导出，已启动隔离 TTY 并完成部分现状快照导出（见 §7）；现状导出不等于 SAVEWORK 两态验证，不得把人工项标完成。
- F 尚未收口：未提交/推送、未同步分支、未更新 GitHub issue 完成状态。用户的 MIME 修复与 changelog 保持原样；完成几何与真机门后再收口。

## 7. TTY 原生 RVM 验证交付（用户选择先交付当前结果）

证据主入口：`verification/e3d-tty-native-20261015-075723/summary.json`。同目录保存实际宏、TTY query/done/launcher 日志、原生 RVM、模型生成输出、面片、对拍 JSON、源库哈希与只读 SYS 声明。临时编排脚本留在证据目录，不作为生产工具发布。

### 导出结果

- [x] 通过现有入口宏通道启动 APS /ALL TTY；后续每个样本单独会话，使用证据目录下完整 APS/ACP 项目副本。以下五次隔离导出均为 TTY exit=0、完成标记存在、RVM 解析 exit=0、未知 bbox 帧为 0。

| 类型 | RefNo | 原生实体数 | 当前结论 |
|---|---|---:|---|
| CABLE | 2013286723/135 | 1 FacetGroup | 同根 gen_ams 输出 0 成员，确认生成器几何缺口 |
| CABLE | 2013286723/16 | 1 FacetGroup | 原生基准已就绪，尚未逐面与生成器对拍 |
| CABLE | 2013286723/817 | 1 FacetGroup | 原生基准已就绪，尚未逐面与生成器对拍 |
| CABLE | 2013286720/575 | 0 | 导出成功但原生空体，无实体几何判别力，不计为几何通过样本 |
| SCTN | 2013286654/68 | 1 FacetGroup | 局部 AABB exact；双向表面采样及体积对拍通过，限此样本 |

### SCTN 样本裁决与工具边界

- 2013286654/68：两端非轴向斜切已在原生 RVM 中体现。原生体积 413600.0 mm³，生成 RVM 面片体积约 413602.0 mm³，相对差 4.8356e-6；两侧均 8 顶点、12 三角面，双向顶点/三角形重心到对侧表面的采样最大差 0.0070711 mm，小于 0.05 mm 容差。数值见 `verification/e3d-tty-native-20261015-075723/sctn_250110_68-surface-compare.json`。采样不是连续 Hausdorff 证明，也不是 SAVEWORK 前后试验。
- 该 SCTN 仍记 negatives_skipped=1，但当前实体与原生端面相符，因此“有一条 skipped ⇒ 一定缺少关节刀”在此样本不成立。保留记账，不再重复扣 CUTB；不能据此推断所有 SJOI/PJOI 正确。
- CABLE 2013286723/135 的原生几何存在、生成侧为空是直接缺口。现有 rvm_compare 不能为这个具名 CABLE 根发配对主键，会报告 unkeyable_baseline；其零配对退出 0 绝不是通过。本轮依据原生实体记录与生成计数判缺，未把它计作相等。

### 7200 阻塞及用户决策

- 只读检查了 /ALL、/ALLIMP、/ENGINEERING、/SCHEMATICS、/SDNF，均未把 DBNO=7200 列入当前 CURD。针对性 SYS 读取确认已有 DB 声明 24575/848、名称 *SAMPLE/DESI；问题是未挂入可用 MDB，而不是磁盘无库。
- 为此给 `src/bin/mdb_project_probe.rs` 添加只读 --dbno 过滤，按 DB 元素筛选，且只输出 NAME/DBNO/PROJ/FINO/AREA/STYP，不读取或输出用户凭据字段。DBNO 为 Word 的真实存储已核验；release 编译与 --dbno 7200 实跑通过，原 --mdb 口径保留。
- 用户明确允许仅在隔离项目副本中配置验证 MDB；但 Windows PowerShell 执行策略阻止了新 ADMIN 启动器运行，ADMIN 宏尚未执行。没有修改执行策略、没有改原始 SYS/MDB。随后用户选择“先交付当前结果”，因此暂停 ADMIN/7200 后续尝试。
- 7200 的 CABLE 23584/10343、23584/10351 与四根宿主切割 SCTN 仍阻塞；不得将上述新样本当作这些原样本已通过。非零 XOFF/YGAP 同样仍待受控样本。
- 2026-10-15 20:10 复测更新：ADMIN TTY 宏通道已打通（隔离启动脚本修复，见 §8 与 `retest-20261015-2010-tty-io-admin.md`）；"隔离副本内配置验证 MDB"由"被执行策略挡住"变为"通道可用、实现方式待定"。

### 首次直接导出的源库观察（必须保留）

- 首轮曾直接在原 APS 项目启动导出；成功写出第一个完整 CABLE RVM 后，选择未挂入 MDB 的 7200 样本时会话崩溃（core.dll，0xC0000005 / fault offset 0x0074D982）。首个文件的原始 chunk 包含 END，解析有效，但整轮会话不得判成功。
- 宏没有 SAVEWORK、FLUSH 或设计编辑，但首轮后 aps250179_0001 与 aps250110_0001 的 SHA256 变化，aps7200_0001 未变。原因尚未做语义核验，不能断言只是缓存，更不能声称源库字节未改。已即时告知用户，保留 before/after 哈希；没有覆盖或回滚原文件。
- 后续全部 TTY 会话改用隔离项目副本。监测的原始三份文件与首轮后哈希保持一致。项目副本与原始系统声明均保留供继续调查。

结论：本轮交付的是原生基准和有限样本裁决；#34/#35 未全修，B/C 尚未接生产几何，E 的 SAVEWORK/TTY 增量回执/FLUSH 验收仍未完成。

## 8. 继续修复：实验性 CABLE 构建器（未通过发布门）

证据：`verification/e3d-tty-native-20261015-075723/cable-experimental-summary.json`。§7 的原生基准交付摘要保留不覆盖。

### 已实现

- 真实读取确认：CABLE.SPRE → TABITE.PRTREF → GPART.CMPR → CABD.OUTD，OUTD 是表达式而非普通 Real。复用 e3d-io 公式环境求值；原生 TTY 同时打印 OUTD=58.4mm，与构建器读取一致。为继续取证，修复属性探针按文件自报类型选择 PROP 等模板，而非把所有非 CATA 的 refno 查询当 DESI；CABD.OUTD/MINBEN 真值已成功读取。
- `../vendor/e3d-model/src/cable.rs` 已实现实验性的同一开口 CWBRAN 直段子路径裁取、顺序反转、有限对正偏移、圆截面及斜接。FRAD、CURVE/RSECT、闭合区间、跨分支 EXTGEO、非零偏移、P 点限定与镜像仍显式拒绝；不跳过坏段而拼出部分电缆。
- 全量管线和单元重建共用该构建器；CABLE 模型单元判据与构建入口同受实验开关约束。E3D_MODEL_CABLE_GEOMETRY_EXPERIMENTAL 未设或不等于 1 时不参与生产构建，开态仅供验证。最新 release CLI 关态实测 0 件、开态实测 1 件；现有刚体/单元级开关及结构/电缆级联默认均未改。
- 依据 Core3D 的终端直线连接 3mm 门，避免把重合或三毫米内的端点补成短段；用 3.0 与 3.001mm 两侧和空路径用例钉住。
- 读取集合两仓重新生成，新增 CMPR/ENDPOI/OUTD/STAPOI/YGAP；并集 144→149，DCHC=0 的生成输入从 7→9（新增 CMPR/OUTD），元数据放行数从 3934→3932。精确快照断言同步，并新增两属性不得进 metadata-only 的守卫；没有修改字典或放宽跳过判据。即使实验开关关着，读取集合仍保守扩大，可能少跳过更新，但不得漏更新。

### 仍为红的原生验收门

- 2013286723/135 从“0 个实体”变为“实验开态 1 个模型元素”，全量/单元重建体积一致；但早轮 RVM 分解为 5 个几何记录，按 3mm 终端门修订后的最新输出为 6 个，原生为 1 个。AABB 最大轴差约 0.4625mm（chord），不能用它替代表面正确性。
- 非凸面片扇形三角化的临时比较器确有缺陷：U 形面凹口会被补面，已先复现失败，再改为耳切三角化并验证凹口保留。修正比较器后，135 的双向表面采样仍有约 345.31 / 157.21mm 最大差，体积相对差约 4.45e-4；此验收仍失败，不能靠放宽容差放行。最坏位置在两个外部子路径的连接边界附近，后续须对齐 Core3D 的区间端面、连接及截面传递；尚不能断言唯一根因。
- 对两个 EXTGEO 做了独立 TTY POSS/POSE 查询，首尾位置与构建器输出在 TTY 毫米打印精度内一致；它只排除了明显端点定位错误，不证明内部路径/连接正确。
- 2013286720/575 原生空体再次导出仍为 0，但实验构建器会出实体，空体条件尚未还原。16 与 817 两个正例仍因桥架 FRAD=300/600 被明确拒绝，缺口保留且没有部分输出。以上都是默认保持关闭的原因。
- 7200 与非零偏移受控样本仍未闭合；#35 未修完，C 线生产级联与翻默认未执行。实验构建通过测试不等于原生几何已通过。

### 回归与审查

- vendor lib：371 passed / 0 failed / 3 ignored；实验开关两态的真库门、无部分输出门、非空构建回归与全量/单元一致性分别验证。忽略的真库用例必须显式选择环境运行，不计作普通单测覆盖。
- gen-model 的 model_impact 整组复跑：先因两份旧计数/名单断言失败，再同步精确断言并补两属性的安全语义测试，最终 50 passed / 0 failed；读取集合镜像守卫通过。全量 release lib 回归为 2494 passed / 0 failed / 132 ignored（原生几何验收仍红，不能以单测代替）。
- SigMap 审查改用临时 GIT_INDEX_FILE 构造只含本轮 vendor diff 的视图，没有修改真实暂存区；已实际覆盖 5 个文件。工具对内联 Rust 测试仍报四条 missing-tests 警告，不能说成零告警；实际新增单测及真库命令单独列证。未修改审查阈值或安全策略。
- 不提交/推送、不改用户的 MIME/changelog/New-LocalRelease/mdb_project_probe 在途改动。本轮仍是实验开发，不是最终收口。
- 磁盘释放后复测（证据：`verification/e3d-tty-native-20261015-075723/retest-20261015-2010-tty-io-admin.md`）：C 盘 5.43 GB；隔离 TTY 导出 `cable_250176_575` 复测 `clean_export=true`、5.0 s，Add-Type IOException 未复现；575 三次会话逐字节比对**仅 HEAD 时间戳不同**（`fc /b` 4/6 字节）、chunk census 无实体几何块 → 空体**可复现且稳定**，属数据条件而非导出抖动；CABLE 级属性假设（CEXES/CABGAP/SPACIN）经三例同构对照**排除**。ADMIN 链路执行策略阻塞已修复（仅隔离启动脚本：补 `-ExecutionPolicy Bypass` + `call set_aveva_design.bat` + 传 MDB 参数；未改机器策略），`admin_probe.mac` 已执行并回 `=24575/848`。575 空体的数据条件与"原生 TRANS"假设均**仍未定论**，不得当成已证根因。16/817 原生基准度量已回填（retest 证据 §6）；对二者生成器仍是显式拒绝，逐面对拍仍不可执行。

## 9. 2026-10-17 更新：EXTGEO 电缆几何全部 4 条原生 RVM 样本对拍通过

证据：`verification/e3d-tty-native-20261015-075723/cable-experimental-summary.json`。

### 根因与实现收口（`vendor/e3d-model/src/cable.rs`, `src/rpath.rs`, `src/cable_tray.rs`, `src/bin/gen_ams.rs`）

1. **桥架 `FRAD` 折弯与缩减段锚点基线**：
   - 非直类附着件且 `ZDIS`/`PKDI` 非零时，在 `reduced_segment`（`PPOS[2] -> PPOS[1]` 切点间缩减段）上量距（`rpath::filleted_point`）。
   - `extgeo_world_polyline` 中计算 `FRAD` 顶点进向 `dir_in` 时，从抬升前的 `raw[k - 1].1` 取前一点而非已叠加 `up * lift` 的 `locals.last()`，消除首段带高度偏移时的微倾角（修复 `/16` 的 `+1.10 mm` 与 `/817` 的 `+2.18 mm` Z 漂移）。
2. **逆序桥架遍历与正交转角抬升**：
   - `up_of(station_f, index)` 在逆序（`a > b`）且锚点位于精确桥架顶点（`|station_f - index| <= 1e-4`，如 `/817` 的 `566 RNODE`）时取 `arrive_up`，在段内锚点（如 `/817` 的 `501 RATTA`）时取所在段的 `leave_up`。
   - 无圆角的正交立管转角（`up_a · up_l == 0`，如 `/817` 的 `573 POINTR`）取两段偏移中心线的内角交点 `(up_a + up_l) * lift`。
3. **统一 `EXTGEO -> EXTGEO` 交界与 `POINTR <-> EXTGEO` 锚点圆角**：
   - 共点交接（`end.distance(span.start) <= 1.0`，如 `/16` `21 -> 22`）：按 `R = bend.max(exit.half_width)`（`150 mm`）连续倒圆并入同一折线，不拆件。
   - 共线跨接（`d_conn` 与 `first_leg` 均与 `exit.last_dir` 共线，如 `/817` `822 -> 823`）：直接顺延；在 `round_polyline_solid_with_caps` 中对 `~180°` 折返点（如 `373 HATTA`）自动拆分为两段扫掠后求并。
   - 异位跨桥架交接：朝下游目标点 `s1`（`b >= a` 取 `floor(a)+1`，`b < a` 取 `ceil(a)-1`）导弧；立管垂直出口（`/135` `140 -> 141`）偏移 `side = unit(up × last_dir) * (diameter * 0.5)` 且 `r_bend = bend`（`350.4 mm`）；水平横向跨桥架（`/817` `823 -> 824`、`824 -> 825`）按 `exit.end_frad + span.start_frad`（`900 mm`）或 `exit.half_width`（`300 mm`）导弧，并在交界处施加角平分面斜切封顶（`n = unit(out_dir + d_conn)`）。
   - `POINTR <-> EXTGEO`：支持 `MINBEN >= 0.0`（`/16` `MINBEN = 0`）及 `RNODE` 出口 2 段圆角（`/817` `825 -> 826`）。
   - 横向 `RNODE` 跨桥架空体门：当起止 `POINTR` 经 `RNODE` 横向跨越桥架且距锚点不足 `half_width + MINBEN` 时（`2013286720/575`：`167.5 mm < 150 + 66 = 216 mm`），返回 `Ok(None)`（`0 PRIM`）。
   - `gen_ams` 对 `CABLE` 输出单个 `FacetGroup`（`geoms = 1`），与原生 E3D RVM 一致。

### 4 条原生 RVM 样本最终对拍矩阵

| RefNo | 名称 | 原生 Geoms | 生成 Geoms | 体积相对差 | `native -> gen` max / p95 (mm) | `gen -> native` max / p95 (mm) | 结论 |
|---|---|---:|---:|---:|---:|---:|---|
| `2013286723/135` | `/CBLP-MCC-1` | 1 | 1 | `0.0208%` | `2.994` / `2.038` | `3.709` / `2.123` | ✅ 通过 |
| `2013286723/16` | `/CBLP-ST-210-01` | 1 | 1 | `0.0085%` | `0.271` / `0.179` | `0.305` / `0.209` | ✅ 通过 |
| `2013286723/817` | `/CBLP-LT-103-B3` | 1 | 1 | `0.1898%` | `1.144` / `0.394` | `0.681` / `0.338` | ✅ 通过 |
| `2013286720/575` | `/CBLP-LT-103-B5` | 0 | 0 | `0.0000%` | `0.000` / `0.000` | `0.000` / `0.000` | ✅ 通过（0 PRIM） |

## 10. 2026-10-17 只读现态核验（不覆盖历史执行记录）

> 核验方式：读取本计划、代码、已有验证产物及两个仓库的本地 Git 引用；**本次未复跑** Cargo、TTY、SAVEWORK/FLUSH，也未核查 GitHub issue 的线上最终评论。§1、§6–§8 是当时快照，其中“B/C 未启动”“级联默认仍关”“原生几何仍红”等结论已被 §9 和本节覆盖；旧 `cable-experimental-summary.json` 的 `remaining_gates`、`validation`、`committed_or_pushed` 等字段也未全部随最终实现刷新，不能单独当作当前状态。

| 阶段 | 本次可核实的状态 | 尚未满足或证据边界 |
|---|---|---|
| A | A0 账本产物 `verification/extgeo-complete-census.json` 记录 116 CABLE / 280 EXTGEO、108 解锚 / 172 未解；§9 和 vendor `e427eb3` 已覆盖部分路径语义。 | 此 JSON 是**旧探针快照**（`geometry_validated=false`），不能拿 172 未解推断现行构建器失败率；`nonzero_offset_samples=0`。A1 原定 7200、SAVEWORK 前后和非零 XOFF/YGAP 样本未闭合，A2 的全部形状类型、A3 的偏移公式/原生对拍也未闭合，故 A1–A3 仍不勾。 |
| B | vendor `e427eb3` 已在本地 `main` 和 `github/main`；§9 与 `verification/e3d-tty-native-20261015-075723/cable-experimental-summary.json` 记录 135/16/817 三根实体及 575 空体通过。 | `vendor/e3d-model/src/cable.rs` 的 `geometry_enabled()` 仍仅在 `E3D_MODEL_CABLE_GEOMETRY_EXPERIMENTAL=1` 时为真；未支持 CURVE/RSECT、跨 CWBRAN、非零偏移、P 点限定等。该证据目录里已找到的 `cmp-v12/v13/v14-*.json` 是早期**失败迭代**，最终四例的通过数值见汇总和 §9，但本次未定位到对应的最终逐例比较 JSON；“通过记录”不等于本次独立复算或全面放行。 |
| C | gen-model `0efb7ccaa` 已在 feat、`origin/feat/issues-36-50-34`、`origin/refactor`；`docs/plans/2026-10-17-cable-endpoint-cascade-default-on-checklist.md` 记载 30 窗、196 条端点边及 152 条 EXTGEO 路由边；`DbOption.toml` 明确设为 `true`。 | 默认开启的是 `model_cable_endpoint_cascade`（反向级联），**不是** B 的 CABLE 几何实验开关；本次未重跑 30 窗。 |
| D | `verification/sctn-neg-20261015/aps7200-new/report.json` 仍记 SJOI 346、PJOI 22 条 `negatives_skipped`；`aps250110-new/report.json` 记 SJOI 70 条。`sctn_250110_68-surface-compare.json` 仅支持一根 SCTN 的采样相符（双向最大约 0.0071 mm）。 | skipped 是**待裁决记账**，不等于这些刀全是几何缺陷；D1 的端切参数语义、D2 实现或明确记缺、D3 广泛复对拍均未完成。 |
| E | §6–§7 记录人工 SAVEWORK/FLUSH 未执行；现有 `scripts/Run-AutoVerification.ps1` 是引导而非验收结果。 | E1 宏的日志/RVM 目标仍指向 `gen-model-refactor`，与向导读取的 `gen-model` 路径不一致；向导还从 `model/records?root=...` 读取 `units.kind`，RVM 比较器引用 `scripts/rvm_verify.py`。E1/E2/E3 均待真实结果和 issue 回执，不能凭向导存在勾选。 |
| F | vendor 构建器和 gen-model C 级联均已提交；C 提交已在 `origin/refactor`，所以分支同步这一条已勾。核验时 gen-model HEAD、`origin/feat/issues-36-50-34`、`origin/refactor` 同为 `e2d817358`；本地 `refactor` 为 `c26ed4a20`，落后远端 2 个与本计划无关的 #57 提交。 | `vendor/e3d-model/changelog.md` 为空，gen-model `changelog.md` 未见本轮 EXTGEO/级联专项追记；#34/#35 最终 issue 评论未核实。此计划目录在 gen-model Git 状态中仍是**未跟踪**，证据目录也有未跟踪项；是否纳入版本管理需另行决定。 |

**收口判定：未完成。**优先闭合 A 的非零偏移与 7200 覆盖、D/E 的端切及人工验证，然后补 F 的 changelog/issue 状态；如果要将 B 由实验开关提升为生产默认，须另设覆盖与发布门，不能由 C 的级联默认开或四个样本代替。

## 11. D 线追加取证：ida-bridge 静态链路与对拍方案（2026-10-17）

> **只读**连接已在线的 `idalib-40832`（`D:/ida_scratch/plant3/Core3D.dll.i64`），使用 ida-bridge SQL 查询 `names`、`xrefs`、`pseudocode`；未改 IDB、未执行设计编辑/SAVEWORK。以下是本次核到的**静态控制流**，不是全语料几何结论。

1. `DB_Sjoi` vtable `0x10B50080` 和 `DB_Pjoi` vtable `0x10B50044`：两者的 `+8 → sub_10722680`、`+20 → sub_10721F00`、`+28 → sub_10721CC0`、`+36 → sub_107383D0` 相同（以精确 vtable 槽位 `xrefs.from_ea` 核对）。`sub_107383D0` 的 `38–66` 行在特定 mode 下调整标架，不是看到 `CUTP` 就直接生成世界系负体的证明。
2. `sub_10722D80` 第 `43–55` 行在几何选项允许时调用关节虚函数 `+8`，入参 mode 为 `2 - (a11 != 0)`；`sub_10722680` 第 `145–184` 行的目录源分支才调用 `+20/+28` 并向 CSG 插入结果，第 `186–239` 行无源时的额外分支只针对 `GMSE`(644065)、`GMSS`(919627)、`PTSS`(919825)、`NGMS`(915098)，**不是 SJOI/PJOI**。因此在这条已查到的关节负体调用链里，“无 TMPL/SPRE”不能直接推成会由 `CUTP/CUTB` 另造负体；也不能据此证明其它端切/更新路径不存在。
3. **排除一条易误认的链**：`ATT_CUTP` 的具名读取引用包含 `sub_108CA490`、`sub_108C8710` 和 `CSG_TreeBuilderFLRLAY::addCutPlanes`；`ATT_CUTB` 的具名读取引用包含 `sub_108C8670`、`sub_108C8710`、`sub_108CA490`。继续追到 `sub_108CA490` 第 `49–65` 行，它要求输入为 `FIXING`、属主为 `ENDATU`、上层为 **GENSEC**；第 `75–90` 行验证的是该 GENSEC 的 JOIS/JOIE，**不是 SCTN**。它在 `209–210` 行调 `sub_108CA220`/`sub_108C9EF0`：前者经 `sub_108CB720` 第 `10–17` 行的同一 FIXING→ENDATU→GENSEC 门，才于第 `103–107` 行写这根 GENSEC 的 `DRNS/DRNE`；后者第 `81–95` 行可能写 ENDATU 的 `POS`。所以此处 CUTP/CUTB 的证据仅支持 **GENSEC 编辑/端点更新路径**，绝不能当成 SCTN 的端切公式或新增负体；FLRLAY 的 `addCutPlanes` 同样是另一宿主路径。此处 FIXING→ENDATU→GENSEC 支路不是 SCTN；但同一 Core3D 的另一条 `RECON0 → STRCNT → JTCONN → PUTEND` 静态链已定位 SCTN 字段写入，详见本节末。PML 另有受操作与宿主门约束的联动写回。具名指针 xref 的缺席也不排除通过 hash/间接调用读取。
4. 当前生成器 `vendor/e3d-model/src/elmodl.rs` 已在 SCTN 正体调用 `section_cut_planes`，其输入见 `src/section.rs:1067` 的 DRNS/DRNE；另一条 `collect_joint_negatives` 对无目录源的 SJOI/PJOI 只记 skipped。既有 SCTN `2013286654/68` 在 skipped 包含 `SJOI 2013286654/72` 时仍与原生 RVM 采样吻合；只能裁决这个**现状样本**，不是 SAVEWORK 前后覆盖证明。
5. skipped **按使用位置记账**：`aps7200-new/report.json` 的 SJOI 346 条对应 183 个 refno（SNOD 链 183 + JOIS 80 + JOIE 83），PJOI 22 条对应 22 个 refno；`aps250110-new/report.json` 的 SJOI 70 条对应 35 个 refno（SNOD 链 35 + JOIS 10 + JOIE 25）。例如 `/72` 同时作为 `/68` 的 JOIE 和 `/70` 的 SNOD 成员入账；**不同宿主不能仅按 refno 去重判定切割义务**。

### 下一道取证门（D1 → E2 → D2/D3）

- **样本分层**：先用现成的 aps250110 `2013286654/68` + SJOI `/72` 作「现状已吻合但 skipped」控制；再在隔离项目副本选 aps7200 `23584/5902` + 无源 PJOI `/5899`、`/5901` + SJOI `/5904`（待 7200 MDB 可用）；最后选 aps250102 `2013286646/500`（`SPRE` 非零、`CUTB=8`）作**有目录源的 GENSEC 宿主反例**：其 `CREF=/495` 已核实为 GENSEC，不作为 SCTN 的 CUTB 反例，也不当 7200 无源样本。
- **同根两态采集**：在隔离副本记录关联 refno 与属主、JOIS/JOIE、CUTP/CUTB/SPRE、SCTN `POSS/POSE/DRNS/DRNE`、表现级/几何选项、SAVEWORK 前后 TTY 属性和原生 RVM；使用同标架双向表面采样、AABB、体积、端面法向核对。保留隔离副本及哈希，不在原项目执行写操作。
- **判定分支**：① 原生端面已由更新后的 SCTN 字段完全表达 → 标「已覆盖，不新增刀」并保留可审计 skipped 分类；② 原生有字段之外的新增减体 → 继续逆其 builder/局部标架，补最窄路径并重跑双语料；③ 输出或属性不稳定/不可用 → 标 `ambiguous`，不改生产几何。D1–D3/E2 继续保持未勾，直到拿到同根动态证据。

### CUTP/CUTB → DRNS/DRNE 追链复核：GENSEC 旁路，非 SCTN 公式

- ida-bridge 的 `sub_108CB720` 反编译第 `9–36` 行直接把输入限制为 `FIXING → ENDATU → GENSEC`，并以 GENSEC.JOIS/JOIE 判断起端=1、末端=2；`sub_108CA490` 第 `49–65` 行也重复同一门。故此前只看到 `sub_108CA220` 写 DRNS/DRNE 就归因于 SCTN 是**错误跨宿主推断**，已在上文 D1 与 §11 第 3 点纠正。
- `sub_108CA220` 第 `71–107` 行的写法还要有 SPINE 成员，并以起/末端标志选择 `ATT_DRNS` 或 `ATT_DRNE`；`sub_108C9EF0` 第 `38–95` 行按保存的标量差及方向投影，可移动 `ENDATU.POS`。这条可核实的是 **GENSEC 的关节编辑与端点更新路径**，不说明 SCTN 也按相同公式更新。
- SCTN 正体路径 `sub_10738C30` 的 `187/211/234/258` 行调用 `sub_10722D80`、`282` 行扫成员链、`294` 行扫属主模板；上述 `ATT_CUTP`/`ATT_CUTB` 具名指针 xref 未落在这些生成函数。静态搜索的阴性结果只界定**这批具名引用**，不能排除 hash 属性访问、PML/RECONN、其它模块或动态派发。
- **下一步定位顺序**：先从隔离工程的同根 SAVEWORK 前后属性差确定究竟改变 `POSS/POSE`、`DRNS/DRNE`、二者还是都不变；若变，再追实际写入栈/属性 hash 与触发宏，核准坐标变换；若均不变但原生 RVM 变，则查其它端切刀。没有这个判据前，不把 GENSEC 的 `CUTP/CUTB` 路径移植到 SCTN，也不把 skipped 批量消零。

### core.dll 具名引用复核：目前只定位到属性/名词注册，不是 SCTN 写回链

- 在线 `idalib-29552`（`D:/ida_scratch/plant3/core.dll.i64`）精确查询 `xrefs.to_ea`：`ATT_CUTB@0x641BE64` 的代码引用仅 `sub_567E830@0x567E8BA`，`ATT_CUTP@0x641DD08` 仅 `sub_567ED70@0x567EDFA`，`NOUN_SJOI@0x6423644` 仅 `sub_57FCDF0@0x57FCE7C`；各另有一条 `0x5E14028` 非函数数据引用。`pseudocode` 第 `24/28/34` 行显示前两函数分别构造 CUTB/CUTP 的 `DB_Attribute` 并赋给全局指针，后者构造 SJOI 的 `DB_Noun`。对应字符串 `CUTB@0x5D7DA44`、`CUTP@0x5D7DB64` 的交叉引用也各仅落在上述注册函数。
- 这些精确 xref **没有发现** core.dll 内经具名 `ATT_CUTP/ATT_CUTB` 访问的 SCTN/SJOI 几何或端点写回函数；仅覆盖该 IDB 的具名直接引用，不排除数值属性号、hash、间接调用或其它 DLL/PML 路径。先前只检索 `D:/work/plant-code/old` 根目录而未覆盖其 `e3d31-runtime/PMLLIB`，故“无 PML 源”的阴性结论已撤回；已按用户提供的位置找到实际宏，详见下节。D1–D3 仍不勾，动态对拍门不变。

### e3d31-runtime PML：SJOI 端面字段写回与 CUTB 转换旁路（只读）

- `e3d31-runtime/PMLLIB/design/objects/sectionconnectionmanager.pmlobj:1219–1243`：`.reconnect(joint)` 在 `joint.hardType == SJOI` 且 `joint.cref.hardType == GENSEC` 的**混接分支**，先 `CUTPLANE PERPENDICULAR`，取 `joint.cutPlane` 为方向，投影节段对应 `pos{end}` 到关节平面，并写 `section.pos{end}`、`section.drn{end}`；其它情形走原生 `RECONNECT RECALCULATE`。`:1278–1313` 的重载对 SJOI 可设 `posLine` 与 `joint.cutPlane`，然后调用前述 reconnect。这里写回的是 `joint.cref` 指向的 **GENSEC** 端点字段，不是 SCTN 的更新公式，也不是额外负体刀证据。
- `e3d31-runtime/PMLLIB/design/objects/stltrimcntrl.pmlobj:750–808` 在端点修剪重连时选择 JOIS/JOIE（排除 PJOI），把 `manager.connectionDirection` 写为 `CUTP ... wrt World` 后执行 `RECONNECT RECALCULATE`；`:891–919` 的另一条 forceData 路径**先写宿主节段 `DRNS/DRNE`**，再设置 joint.CUTP 并重连，异常时恢复方向与 CUTP。`design/objects/stljntlike.pmlobj:403–445` 另有反向复制：按目标 START/END 从目标节段变量 `tsctn` 的 DRNS/DRNE 赋给目标 joint.CUTP，再 `RECONN RECALC`。因此 CUTP 与端面方向可以相互伴随更新，且不同宏的先后顺序不同；不能以某次 CUTP 非零推断独立刀面，也不能把宏的赋值方向当作原生 RECONN 的普遍实现。
- `e3d31-runtime/PMLLIB/design/functions/sctlcutplane.pmlfnc:36–78` 显式按 START/END 和方向朝向写 SCTN `DRNS/DRNE`；`design/functions/sctlmitresctn.pmlfnc:115–136` 在两段之间调用它。相反，`e3d31-runtime/PMLLIB/structural/objects/strSctnGensec.pmlobj:620–679,366–389,1369–1375` 是**SCTN→GENSEC 转换**：把旧 POSS/POSE、DRNS/DRNE 复制到新 GENSEC 的 POINSP/SPINE，将非零 CUTB 的 PJOI 转为 FIXING/ENDATU，按 CUTB 位移新 GENSEC 的 POINSP，再对该新 GENSEC `RECONN`；不得据此声称原 SCTN 的 CUTB 必然生成额外负体或按 GENSEC 公式更新。上述仅为 PML 静态路径；原生命令副作用和 SAVEWORK 前后真实 RVM 仍未核实。

### 扩大 PML 范围后的操作/宿主门（只读，仍非动态证明）

- `design/functions/strmitre.pmlfnc:34–96,457–511` 接受节段、包含硬类型 SCTN 分支；计算斜接方向后，对两段各自 START/END **直接写 `DRNSTART/DRNEND`**，若端部有 JOIS/JOIE 则给所引用关节写 `CUTPLANE`（写入异常由 `handle any` 吞掉）。这是 SCTN 可达的“端面方向 ↔ 关节切平面”编辑链，但它不创建减体、不证明所有无源 SJOI 的 CUTP 数值都由该宏造成；也没有在此函数末尾看到 `RECONN`。`design/functions/sctlmitresctn.pmlfnc:48–50,130–136` 允许 SCTN/GENSEC 并调用 `sctlCutPlane`，后者仅显式写端面方向，不应误说成 CUTP 刀。
- `design/objects/stltrimcntrl.pmlobj:340–396,875–919` 的 `forcedPlineTrim` 先按待修剪线/原节段两条线构造方向并缓存原 `DRNS/DRNE`、CUTP/POSL 到 `forceData`；后续应用阶段写节段端面方向、关节 CUTP、调用 `RECONNECT RECALCULATE`，失败时还原。这是**受交互式强制 P-line 修剪条件约束**的写回路径，不是普遍端切公式。
- `supports/design/objects/MdsBracing.pmlobj:91–116` 将 JOIS/JOIE.CUTB 暂置零、重连后读 POSS/POSE，再恢复 CUTB 并重连；但它的调用方 `supports/design/functions/mdsbracings.pmlfnc:27–35` 明确只 `collect all gensec`，**不是 SCTN 证据**。同理，`supports/design/objects/mdstrimsctn.pmlobj:245–289,398–403,427–446` 虽暂置 PJOI.CUTB=0 并恢复后 `RECONN`，其 `trimSectionEnd` 在 `section.type != GENSEC` 时直接返回，不能移植为 SCTN 常规流程。`supports/design/objects/mdsConvertToGensec.pmlobj:1699–1710` 的注释还明言 PJOI→FIXING 后钢材 cutback 不同，须对**新 GENSEC**延伸 START/END；再次佐证转换路径不可跨宿主。

### PML 调用方核实：只在特定交互流程触发，不是 SAVEWORK 全局步骤

- `design/functions/strmitre.pmlfnc` 的直接 PML 调用点：`design/functions/sctlmitreboth.pmlfnc:31–73` 与 `design/forms/civtracefloor.pmlfrm:1295/1300`。前者仅当端关节为 `PJOI` 或 `FIXING`、所在 PNODE 恰有 2 成员且两成员 CREF 有效时，从各关节的 CREF 取得两根节段并调用 `strMitre`；这是**以 PNODE/端关节条件触发的斜接**，不覆盖任意无源 SJOI。`sctlMitreBoth` 的 PML 调用点为 `design/objects/stltrimcntrl.pmlobj:819`（trimSections 中的特定分支）和 `design/functions/sctlspinedismiss.pmlfnc:97/111`（WALL/GENSEC 的 spine 编辑收尾），不是 SAVEWORK 自动执行的普遍规则。`sctlMitreSCTN` 在 PMLLIB 中检出的调用点为 `design/functions/cvwalltraceloop.pmlfnc:123/130` 的墙跟踪环。
- 强制 P-line 路径入口为 `design/objects/appstlwrk.pmlobj:413` 的 **Pick (force)** 回调与 `design/commands/steelworktrimtoplinepickforce.pmlcmd:42`，二者调用 `design/functions/stlwrktrim.pmlfnc:34–43` 的 `stlWrkTrim(true,false,false,'')`，进入 `STLTRIMCNTRL.setupForce()`；`stltrimcntrl.pmlobj:216–232` 注册 EDG 端点选择与 Escape→`plineForceTidyUndo()`，`:270–312` 允许选 SCTN 或 GENSEC、须端点已连接、从 JOIS/JOIE 找对侧节段，并注册 P-line pick→`forcedPlineTrim()`；`:337–396` 才缓存 forceData；`:844–906` 在退出选择操作时经 UNDOABLE 调 `plineForceTidy()` 应用写回。故 **选端 + 已连接 + 选 P-line + 退出触发**是可见前置，不得把此宏的 DRN/CUTP 赋值当作普通 SAVEWORK/RECONN 的默认副作用。
- 以上调用点基于 `PMLLIB` 文本搜索（本次扫描 8774 文件、另有 1694 个二进制跳过），不能排除动态字符串调度、其他宏库或原生命令；与 `negatives_skipped` 的 refno 尚未建立运行时对应。原生 RECONNECT 的 SCTN 可达写回分支已在下节定位；下一证据门是复核其选项/坐标系，并在隔离同根取得属性/RVM 前后对比。未获动态证据之前不补 CUTP/CUTB 负体。

### Core3D 原生 RECONNECT 字段写回链（ida-bridge 只读；非 RVM 动态证据）

- `Core3D.dll.i64` 的 `DES_Connections::ReconnectStart@0x10501420`、`ReconnectEnd@0x10501350` 和 `ReconnectStartAndEnd@0x105014F0` 均构造 `RECO` 与 JOIS/JOIE/BOTH 并进入 `ReconnectDisconnect@0x10501250`；该函数第 `21–34` 行把字符串转 hash 后调用 `sub_1025F521`。`sub_1025F521:19–84` 标记 `connect/RECON0`：类型码 `719809=PJOI`、`719812=SJOI`、`730315=SUBJ`、`108608856=FIXING` 走关节分支，`821683=SCTN` 或 `46619692=GENSEC` 走节段分支；JOIS/JOIE 在此分别为 `912394/636832`。节段分支到 `sub_102616EA` (`connect/STRCNT`)，第 `121–125` 行将 FIXING 分给 `FXCONN@0x1026358C`，其它关节分给 **`JTCONN@0x10261B5B`**。故这是 SCTN/SJOI **可达的原生命令链**，与先前只看到的 GENSEC FIXING 编辑链不同；具体调用现场及全部异常/选项门尚未穷尽。
- `JTCONN:102–119` 按 JOIS/JOIE 选择 `POSS(919690)/POSE(644128)`、`DRNS(916114)/DRNE(640552)`；`:187–189` 在关节读取 `CUTP(861519)`、`CUTB(585957)`、`JLIN(813898)`，`:200–214` 对 PJOI/SJOI 的定位有不同分支，`:243–269` 将方向和位置做标架变换及投影，`:275–298` 在前置成功门通过时调两次 `sub_102D674D`，传入相应端点位置/方向。后者标记 `structures/PUTEND`，第 `24–45/95–137` 行查目标宿主 `TYPE`，明确包含 `821683=SCTN` 和 `46619692=GENSEC`，并经 `BPUTRA` 写相应 POS/DRN 字段（必要时转换到 owner 标架）。这给出**SCTN 原生端点可被关节重连更新的静态写回证据**，但不是每次重连都会写：还受 `JTCONN` 的成功标志、几何/连接选项和错误门约束。
- 数据码来自只读的 IDB `ida_bytes.get_dword` 和 `gen-model/all_attr_info.json` / `noun_flags.json` 映射；非猜测英文缩写。`JTCONN` 反编译中 `CUTB` 的标量读入 `v60` 目前没有在可见伪代码中找到进一步直接使用，不能由此断言 CUTB 无效，也不能据此列出位移公式。还须复核 `CUTP` 标架变换、PJOI/SJOI 分支、成功门和是否有其它几何路径；最重要的是隔离项目同根前后属性 + RVM，才能裁决已有 SCTN 端面是否完全覆盖原生端切。D1–D3 不勾，不改生产构建器。

#### JTCONN 分支与写回门补核（仅静态）

- `JTCONN:141–165` 从关节 `SPRE(643429)` 与可能的目录相关项分支；`:187–224` 在读 `CUTP/CUTB/JLIN` 后，若无有效目录定位而进入 `v20 == 211` 的**无源回退**，再依关节类型选择：`PJOI(719809)` 取 `DELP(855256)` 与属主 `NPOS(916799)` 做位置合成（`:200–209`）；`SJOI(719812)` 经其 OWNER→`pllib/PISNPS@0x103A121C`→`pllib/GSNPOS@0x103B1388` 求位置（`:211–214`），后者只在属主 `TYPE=SNOD(621505)` 时读 `ZDIS(912113)` 并投影到线（`GSNPOS:32–46`）；`SUBJ` 又是另一路。若有目录项则 `JTCONN:225–230` 走规格/目录位置求值。**这些分支不能互换，更不能把 PJOI 的 DELP/NPOS 直接套给 SJOI。**
- 方向来自 `CUTP` 的 `DGETRA`，在 `JTCONN:170–178` 以 `GSTRAM@0x103A0FC4` 取标架（GENSEC 分支还调用 `INTRAM`），`:242–269` 用矩阵乘法变换位置/方向，并在条件成立时取已存端点、求直线/切面交点或投影；`:271–298` 的 `v20` 错误门及 `BPUTI(CTYS/CTYE)` 成功位 `v18` 通过后才调用 `PUTEND` 写 POS/DRN。另一个数据库错误分支 `:379–445` 有条件地调用 `sub_102D607C`，不能据主成功分支推成“每次重连必改”。`CUTB` 仍只有本伪代码可见的读取，**尚未证实参与该位移**；标架、单位、默认方向和实际 RVM 都待核准。

#### CUTB 的局部数据流复核：读到，不等于用于 SCTN 位移

- `JTCONN@0x10261B5B` 的 `0x10262093–0x102620A4` 用地址 `0x10B267A0`（IDB 只读 dword=`585957`，字典为 CUTB）调用 `DGETR`，结果目的地是栈变量 `var_3B8`（伪代码 `v60`）。在该函数的 IDA 指令列表中，**`var_3B8` 仅出现一次：作为此次读取的输出地址**（邻近的 `var_3B4` 也无引用）；伪代码亦只出现定义与 `DGETR`，未看到将这个标量带入几何计算或 `PUTEND`。只读扫描该模块连接常量表的 `0x10B26500–0x10B27000` 范围，值 `585957` 仅有此处一项并仅由 `JTCONN@0x10262093` 引用；这只是**限定范围的阴性结果**。因此当前可证的是“JTCONN 读取 CUTB，但未见其数值在此函数内参与 SCTN 端点写回”；仍不能排除读取的错误状态副作用、其它函数/间接实现或不同操作路径，不能断言 CUTB 全局无效。
- 精确 xref 的另一条具名 CUTB 使用 `sub_108CDE20@0x108CE286`：该函数在 `:70` 要求输入 `ENDATU`，`:76–96` 要求节段含 SPINE 且至少两个 POINSP，然后对 JOIS/JOIE 与其定位运算；是 GENSEC/ENDATU 语境，不是 SCTN 常规分支。原先已查的 FIXING→ENDATU→GENSEC 编辑链亦保持独立。下一步若要确认 CUTB 在目标 SJOI/PJOI 上的真实作用，应在**隔离副本**比较同根 CUTB、POSS/POSE、DRNS/DRNE 与原生 RVM 的前后态；不要把“局部未使用”升级成普遍无效或补刀依据。

#### 具名样本现状对照（仅只读属性/既有报告；未执行 RECONNECT/SAVEWORK）

- 取证来源：`verification/joint-corpus-attrs.log` 的实际文件存在（先前按单文件调用 `search_files` 得 `files_scanned=0` 是搜索参数/文件扫描边界，**不是 refno 不存在**）；原有 `sctn-neg-20261015/{aps250110-new,aps7200-new}/report.json` 与 `elements.json`；另外复用已审源码声明只读的 `src/bin/noun_attr_dump_probe.rs` 及既有 release 可执行文件，以 `--refno` 对 APS `aps000` 原始文件只读扫描，直接在 stdout 取得宿主字段。该探针 `ReadOnlyEngine::scan_elements`/`DbSet` 只读取文件，不连 Surreal、不写设计库。数据均为**现状**，不是两态差分。
- **控制样本 aps250110**：`SCTN 2013286654/68` 属于 `TMPL /25148`，现有 `POSS=(-800,-342,8)`、`POSE=(-1784,-342,8)`、`DRNS≈(0.707107,0.707107,0)`、`DRNE≈(-0.707107,0.707107,0)`；`JOIS=0`、`JOIE=/72`。`SJOI /72` 属主为 `SNOD /71` ← **另一 SCTN `/70`**，`CUTP=(0,0,-1)`、`CUTB=0`、`SPRE=0`、`CREF=/68`；`/71.ZDIS=0`。故 `/72` 在 `/68` 上是 JOIE 的 CREF 端点，在 `/70` 上是 SNOD 成员；报告中两次 skipped 的**宿主不同**。`/68` 现有正体 `negatives_applied=0, negatives_skipped=1`，既有原生/生成 RVM 采样吻合（§7），与 `JTCONN` 可写回 SCTN 的静态链**相容**，不能证明就是该调用造成现存斜端面，更不能把原始 CUTP 向量不经标架变换直接等同 DRNE。`/70` 也记 skipped=2，不替 `/68` 作为同一切割判据。
- **aps7200 的无源 PJOI/SJOI 混接样本**：`SCTN 23584/5902` 属 `SBFR /5897`，`POSS=(13822.36,-1234.58,178.72)`、`POSE=(13822.36,-1234.58,2648.2)`，`DRNS=(0,0,1)`、`DRNE=(0,0,-1)`，`JOIS=/5899`、`JOIE=/5901`，名下 `SNOD /5903` 与 `/5905` 的 `ZDIS=2412.2/126.21`。两 PJOI 分别在 PNOD `/5898`、`/5900` 下，`CREF=/5902`、`DELP=0`、`CUTB=0`、`CUTP=(0,0,±1)`、`SPRE=0`；`SJOI /5904` 在 `/5903` 下，`CREF=/5910`、`CUTP=(1,0,0)`、`CUTB=0`、`SPRE=0`，故它同时是 `/5910.JOIS` 的对端关节，不能算作 `/5902` 的直接 JOIS。`/5902` 的生成正体 `negatives_applied=0, negatives_skipped=4`、两分量、56 三角面；报告还记录 LEVEL STRU 0 的部分 SREC/SPRO 显示过滤，**不等于该 SCTN 整体无几何**。现无这个 refno 同根原生 RVM/两态差，四条 skipped 不足以判“缺四刀”。
- **aps250102 有目录源反例需改归宿主**：`PJOI 2013286646/500` 的 `CUTB=8`、`CUTP=(0,0,1)`、`SPRE=23704/1584476`、`CREF=/495`；只读直查发现 `/495` 实为 **GENSEC**（FRMW `/490` 下、有 SPINE 成员、`JOIS=/500`、`JOIE=0`、`SPRE=2013287245/112183`），`PNOD /499.NPOS=(192,52.4,-829.47)`。因此 `/500` 可检验 PJOI→GENSEC/目录源链，但**不是 SCTN CUTB=8 的反例**；§11 上方候选措辞中“先确认相应 SCTN/宿主”现已落实为 GENSEC，禁止跨宿主归因。
- 静态映射到这三个样本：`RECON0→STRCNT→JTCONN` 可由 JOIS/JOIE 和关节类型进入；`/68` 与 `/5902` 为 `PUTEND` 的 SCTN 目标候选，`/495` 是 GENSEC 目标。实际是否走无目录回退、连接成功门、`GSTRAM` 的标架/方向及最终写入**没有调用轨迹**；`JTCONN` 的 CUTB 读取不能化作这几个样本的位移公式。D1–D3 仍不勾；下一证据门仍是先确认隔离副本安全边界，再取得同根 SAVEWORK 前后属性及原生 RVM。

#### 续查：JTCONN 的标架/成功门与既有 7200 导出状态（仍只读）

- `JTCONN@0x10261B5B` 伪代码 `:102–119` 先按传入 JOIS(912394)/JOIE(636832) 选择写回属性 POSS/POSE、DRNS/DRNE；`:121–137` 有 `DGTERR` 提前退出；`:166–185` 对关节读取 `JFRE(643150)` 并取反成局部 `v24`，调用 `GSTRAM@0x103A0FC4` 形成标架（GENSEC 分支再 `INTRAM`），若 `v27` 错误则退出。此处 `JFRE` 的属性码由 IDB dword `0x10B2678C` 和 `all_attr_info.json` 对齐；目标 SJOI/PJOI 的现有属性 dump 未直接列出 JFRE，**不能臆定运行时 v24 值**。
- `:187–230` 在标架建立后读 `CUTP` 到向量 `v79`，读 `CUTB` 到本函数未见数值后续使用的 `v60`；无目录项时需 `sub_103A3EF4` 产生特定错误码 `211` 才进入 PJOI DELP+NPOS 或 SJOI SNOD/ZDIS 回退，有目录项走另外位置求值，不能仅以 `SPRE=0` 保证某次命令选了回退。`:243–269` 先对 `v79` 做矩阵乘法并按 `v24`、SCTN/GENSEC 分支计算位置；`:271–298` 要求 `v20=0`，随后 `BPUTI` 的 `v18 & 1` 成功才调两次 `PUTEND`，非该成功门下不保证字段写回。**尚无样本运行轨迹/瞬时标架**，不能把 `/72` 的原始 `(0,0,-1)` 与 `/68.DRNE` 直接比较，更不能由 CUTB=0 推出全部端切一致。
- 既有 `verification/e3d-tty-native-20261015-075723/sctn_7200_host_cuts-20261015-080432-561/result.json` 并非可用 7200 原生基准：`done_marker=false`、`clean_export=false`、`tty_exit=1`、`parse_exit=null`；`launcher.log` 记 detached 进程异常退出，`query.log` 空。该次 `export.mac` 实际指定的是 `/5907`、`/5910`、`/8622`、`/8626`，**从未指定 `/5902`**；故这不是 `/5902` 的失败导出尝试，只是 7200 相关宿主的失败记录。不得用目录名冒充成功基准或把空 query 判作空几何；`/5902` 仍无直接原生导出。

#### 续追 GSTRAM、JFRE 与 PUTEND 坐标边界（ida-bridge/字典只读）

- `GSTRAM@0x103A0FC4:15–64` 先对源元素和目标元素各读取 `TYPE(642215)`、检查传入类型及 `IFCOMP`/`CLIMBA`，不匹配给错误 `210/211`；无错后对二者取得用于 `CSTRAM` 的位置/标架引用，再调用 `CSTRAM`，失败亦归 `211`。所以 `JTCONN` 的矩阵不是简单恒等，也不是仅由 `CUTP` 的三个原始数字确定；该步未取到 `/72→/68` 的实例矩阵，**不能做样本数值法向的等式检验**；`CSTRAM` 算法续查见下。
- `all_attr_info.json` 对 `SJOI(719812)` 和 `PJOI(719809)` 的 `JFRE(643150)` 均登记 BOOL、schema 默认 `false`；这只说明**模板默认**，属性 dump 中未见保存值，也没有运行时 `DGETL` 返回值，故不把默认值当这些样本的动态观测。`JTCONN:167–185` 将读到的 JFRE 取反给 `v24`，并在标架创建后写回 JFRE；`:247–274` 又以 `v24 & 1` 改变投影分支，逐例结果还须运行时证据。
- `PUTEND@0x102D674D:24–43,95–137` 先取目标节段 `OWNER(10206636)`、`TYPE(642215)`：若是 **SCTN** 且写 POSS/POSE，在当前 owner 与命令标架不同的条件下执行 `CSTRAM→TRAVEC`，否则保留入参位置，然后 `BPUTRA`；写 DRNS/DRNE 时若标架不同则 `CSTRAM→MVMULT`（仅旋转向量）再 `BPUTRA`。GENSEC 的 POS/DRN 则进入另有 `DFINDI`/成员定位的分支，**不与 SCTN 写法混同**。这是关节 CUTP 到 SCTN 端面字段之间存在至少两级标架处理的直接静态证据；不提供具体样本当前为何为斜面的历史因果，也不表明新增 CSG 负体。

#### core.dll 矩阵实现补核：算子明确，样本矩阵仍未知（只读）

- 在 `Core3D.dll` 中 `CSTRAM/TRAVEC/MVMULT` 是导入 `core.dll` 的 thunk；转到在线 `core.dll.i64` 原生实现。`CSTRAM@0x51E393B:16–55`：先置单位矩阵/零平移；若两 ref 相等直接返回恒等。若不等，分别对**第三参数**（源参照）与**第二参数**（目标参照）调用 `GATRAR` 取标架，目标矩阵执行 `INTRAM` 求逆，再 `CONCAT(out, inverse(target), source)`；取标架失败返回错误 `287`，而非写回。这里“源/目标”是按 `CSTRAM` 的调用参数位置和矩阵乘法定义的，不把某个 ref 在具体样本中的物理含义臆定为世界/局部。
- `INTRAM@0x526C32C:12–16` 逆转旋转并以逆旋转乘负平移；`CONCAT@0x526C67C:12–23` 计算 `R_out=R_target_inv × R_source`、`t_out=R_target_inv × t_source + t_target_inv`。`MVMULT@0x526B724:9–15` 只做 `d'=R_out d`；`TRAVEC@0x526C430:8–10` 在矩阵乘法后加平移，即 `p'=R_out p+t_out`。用于 SCTN 的 `PUTEND` 调用 `CSTRAM(out, owner, command_ref)`，所以不同参照时它把命令侧坐标变换到 SCTN owner 侧，方向不加平移；相同 ref 则不做变换。`GSTRAM` 自身又有一层取参照及 `CSTRAM`，且 `JTCONN` 有投影/重连门。
- **未闭合量**：需要样本命令发生时的实际两个 ref、其 `GATRAR` 解析出的标架矩阵、JFRE 运行时值、方向约定以及 `PUTEND` 是否被调用。当前静态公式足以排除“CUTP 原始分量直接等于 SCTN.DRNE”和“CUTB=8 自动令 SCTN 缩 8”的捷径，却不能重放 `/72` 或 `/5902` 的历史端切，也不能裁决额外负体。

#### 既有 TTY/探针运行证据盘点（只读，不启动重连）

- `verification/e3d-tty-native-20261015-075723/sctn_250110_68-20261015-080136-790/export.mac` 明确只对 `=2013286654/68` 执行 `Q CE/NAME/TYPE/POSS/POSE/DRNS/DRNE`，之后直接 `export repr`/`export =...`；**没有** `RECONNECT`、`SAVEWORK`、`Q JFRE` 或关节属性查询。相应 `query.log` 原生 TTY 文本为 `Posstart W 800mm S 342mm U 8mm`、`Posend W 1784mm S 342mm U 8mm`、`Drnstart E 45 N`、`Drnend W 45 N`，和同一现状样本只读探针的四字段及原生 RVM 的斜端面方向相容。`result.json` 显示 `tty_exit=0`、`parse_exit=0`、`done_marker=true`、`clean_export=true`：是有效**现状**导出，不是两态实验。
- 对该 TTY 证据目录下 `.mac/.log/.md` 的具名 `RECONNECT`/`RECONN `、`SAVEWORK`、`JFRE` 文本检索未命中；这只能说明**此范围的已有文本没有这些记录**，不能排除其它文件/间接执行或运行时历史。既有 `joint-corpus-attrs.log` 与 probe 属性 dump 也只给保存的字段面，不包含 `/72` 重连时的 JFRE 读取值、`GATRAR` 瞬时矩阵、`BPUTI` 成功状态或前后快照。故目前能配对的是“TTY 现状属性 ↔ 只读探针现状字段 ↔ 现状原生 RVM”，不是“RECONNECT/SAVEWORK 调用 ↔ 属性变化 ↔ RVM 差异”。

#### 扩大既有证据目录的只读检索与两项纠偏

- 对 `verification/` 下 `.mac/.log/.txt/.md/.ps1` 单文件不超过 4 MB 的文本扫描了 545 个文件：有其它项目的 SAVEWORK 宏/验收记录（如 AMS8000 scratch BRAN、增量房间 EQUI），还有 `export_isolated.ps1` 对写库命令的禁止检查；**不是目标 SCTN `/68`、`/5902` 的同根重连两态证据**。再对 `.mac/.log/.txt/.md/.json/.ps1` 单文件不超过 16 MB 的 3959 个文件做五个具名 refno 查找（排除庞大的通用 `joint-corpus-attrs.log`、`report.json`、`elements.json`）：命中的 `/68` 宏/摘要/对拍是现状基准，`/500` 是静态属性，未定位到目标 refno 的保存前后宏和双态几何。**仅限这些文件类型、大小阈值和路径，不宣称全工作区无证据。**
- `joint-corpus-attrs.log` 里 `JFRE=Bool(true)` **确实存在**：本次按当前 PJOI/SJOI 块计数分别 202/289 条；例如 PJOI `2013286654/19` 为 true。此前“目标 `/72`、`/5899`、`/5901`、`/5904`、`/500` 的 dump 未列 JFRE”只限这些具名样本，**不得扩大成全语料未保存 JFRE**；模板默认 false 不等于语料全为 false。true 的其它 refno 也不能代替目标的运行时 `DGETL` 值或证明目标已重连成功。
- 7200 候选纠偏：`sctn_7200_host_cuts` 的失败会话和较早 `export_native.mac` 都明确列 `/5907`、`/5910`、`/8622`、`/8626`，**没有 `/5902`**；故不能称已有 `/5902` 导出失败，只能说此处未找到它的直接原生 RVM。后续若选 `/5902` 作 D3 目标，需明确将其列入隔离只读原生导出方案并先审核边界。

#### 用户确认的测试运行时切换与只读启动试验（隔离副本，未触及设计写入）

- 用户明确指定测试/调试使用 `D:/work/plant-code/old/e3d31-runtime` 的 **E3D 程序及 PMLLIB**，项目数据继续使用 `verification/e3d-tty-native-20261015-075723/projects` 的**既有隔离副本**；不迁移或写入原设计库。本轮只在该任务的 `verification/e3d-tty-native-20261015-075723/export_isolated.ps1` 将 `L3_E3D_INSTALL_DIR` 改为此目录，并显式绑定 `AVEVA_DESIGN_EXE`、`pmllib`、`PMLUI`、`building_design`、`common_tools` 至同一运行时，防止其 `evars.bat` 沿用进程中已设的旧 PML 路径。通用 `scripts/e3d/run_ams_c_entrymacro.bat` 的默认运行时**未改**，仅该隔离入口的显式环境变量覆盖默认。
- 只读确认新目录的 `des.exe`、`evars.bat`、`PMLLIB`、`PMLUI`、`PMLLIB/Building_Design`、`PMLLIB/Common_Tools` 存在；既有副本完成标记及 APS/ACP 项目 evars 存在；脚本经 PowerShell Parser 语法检查通过。用户另行许可对既有隔离副本运行**只读** `sctn_250110_68` TTY 导出宏，未授权 RECONNECT/SAVEWORK/FLUSH。
- 新运行时首次只读试运行产物：`verification/e3d-tty-native-20261015-075723/sctn_250110_68-20261018-012127-943/`，`result.json` 为 `tty_exit=1`、`parse_exit=null`、`done_marker=false`、`clean_export=false`；`launcher.log` 明确 `AVEVA_DESIGN_EXE` 和 `pmllib/PMLUI/building_design/common_tools` 已指向新目录，`project=APS`、`mdb=/ALL`，detached `des.exe` 进程退出码 `3221356545`（`0xC0020001`）。没有生成 `query.log` 或 RVM，说明尚未取得有效原生几何；**不能把退出码单独解释成根因，也不能用旧运行时成功样本替新运行时验收**。失败产物保留，暂不重试或改原库；D1–D3 继续未勾。

#### 失败后的只读诊断（未重新启动）

- PowerShell 的 `Get-WinEvent` 模块在此环境不能自动加载，改用系统 `wevtutil qe Application` 只读查询最近 250 条，筛出与 `des.exe` 相关的一条 Windows Error Reporting **APPCRASH / Event ID 1001**：故障模块 `KERNELBASE.dll`、异常码 `c0020001`，与 launcher 的退出码一致。该事件只确认进程异常退出，**不是具体引发异常的代码栈/根因**；本轮未读取或改动 WER dump。
- 对指定新运行时与原先 shadow 的关键文件只读计算 SHA-256：`des.exe`、`Core3D.dll`、`Startup.dll` 一致；`evars.bat`、`set_aveva_design.bat`、`des.exe.config`、`pdmsconsole.exe` 及抽查的 `PMLLIB/design/objects/sectionconnectionmanager.pmlobj` 亦一致。**`core.dll` 不一致**（新运行时 sha256 `668783707a924c343759e99e8676fa8482d14987f4d83f10a83246001a7f5c18`，旧 shadow `7c047a8b20b85b56cf8bb8fad9db838791500a0e5370d1624f8dac4ea54c60b4`，文件大小均 50,071,544 B）。这是值得后续核对的差异，不能仅凭哈希认定它导致崩溃，也**不得通过替换厂商 DLL 来试错**。下一步须先审非写入诊断证据或取得明确复试许可；失败会话不计 D3。

#### 后续用户改定为原项目路径；仅配置，禁止自动复试

- 用户撤回“继续使用既有隔离副本/另建副本”的选择，明确改为**项目直接指向 `D:/AVEVA/Projects/E3D3.1`**，所有相关入口从 `D:/work/plant-code/old/e3d31-runtime` 启动；在单独确认对原项目只读 TTY 的安全边界前，用户本轮只允许**配置修改，不启动**。上述失败会话仍是**先前隔离副本**的历史记录，不能当作原项目试验或作为切换项目路径必然修复崩溃的依据。
- `scripts/e3d/run_ams_c_entrymacro.bat` 的默认运行时已改新目录，项目根默认本就为 `D:/AVEVA/Projects/E3D3.1`；同时在调用 `evars.bat` 前将程序及 PML 相关变量绑定到选定运行时，避免继承旧值。环境变量仍可显式覆盖默认，故不能说所有调用方**无条件**都落在新路径；对其它测试的具体影响尚未动态验证，新运行时先前的启动失败警告保留。
- 原 `verification/e3d-tty-native-20261015-075723/export_isolated.ps1` 现配置为读取**原项目**，不再复制项目，写出的项目 evars 放在证据目录并以 `original_project_evars.bat` 命名；虽然历史文件名仍含 `isolated`，脚本显式标注不再隔离，并增加 `-AllowOriginalReadOnly` 必须显式传入的启动门。新宏/结果标记改为 `ORIGINAL-READONLY` 并记录 `original_project=true`/运行时路径，以防后续产物被误标成隔离成果。**本轮未执行该脚本**。静态检查：PowerShell Parser 语法通过、原项目 APS/ACP evars 存在；未对原项目执行查询、导出或写入。D1–D3/E2 状态不变。

#### 原项目只读试验后发现原始文件变化：立即停机待裁决

- 用户随后明确批准一次仅 `-AllowOriginalReadOnly` 的 `/68` 原项目 TTY 查询/RVM 导出。启动前计算 `D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000/aps250110_0001` 的 SHA-256 为 `b7a7ea08d47891d6b9369e8eae01c6dfa9b2dc855fae5e84062f748a03ede9ff`，大小 `9,932,800` 字节。单次启动产物为 `verification/e3d-tty-native-20261015-075723/sctn_250110_68-20261018-014006-622/`；`tty_exit=1`、`parse_exit=null`、`done_marker=false`，没有 `query.log` 和 RVM，launcher 仍记录 `0xC0020001`，原生验收失败。
- **安全异常**：启动后同一原始文件大小未变但哈希变成 `bfd1cff75fb27bc69da992bfef071eb1ea71a61eb6d3f07c0a6866197ecb484d`，修改时间为 `2026-10-18 01:40:08`；同目录 `aps000` 的 240 个普通文件中有 **112 个**的 mtime 落在本次启动窗口 `01:39:50–01:40:30`。这与本次 E3D 启动时间高度吻合，但尚不能排除用户或后台进程并发活动；**不能声称对原设计库只读、也不能把变化归因于某个宏命令或明确为有效 SAVEWORK**。先前声明的“宏无写命令”不足以保证进程启动不修改项目文件。
- **处置**：不再启动、不重复哈希前态复算、不替换/回滚/覆盖任何原始项目文件；失败产物保留。需要用户决定是否先隔离/禁用原项目启动入口，并由项目管理员在现有项目会话/备份审计中调查文件变化。D1–D3/E2 仍未验收，此次失败和源库变化均不得当作几何证据。

#### 改为副本读写的预检（尚未复制）

- 用户选择将**当前事后状态**的 `D:/AVEVA/Projects/E3D3.1/{AvevaPlantSample,AvevaCatalogue}` 复制到 `D:/work/plant-code/old/e3d31-runtime/projects`，之后只对副本测试读写。预检显示目标目录目前不存在，D: 可用约 94.77 GiB；两源目录合计 2663 文件、3,770,057,192 字节（APS 1856 文件/2,340,013,123 B，ACP 807 文件/1,430,044,069 B），空间足够。复制出的项目必须标为**事后状态**，不得冒充原项目启动前基准。
- 只读进程检查同时发现**另一条 `des.exe` PID 68320 仍在运行**，程序路径 `E:/reverse/e3d/shadow_e3d31_aps_all/des.exe`，启动于 `2026-10-18 01:27:16 +08:00`，早于上述 01:40 文件变化；是否连接同一原项目未确认。这提高了并发活动的可能性，也意味着源项目在复制时可能不静止。**未停止该进程、未复制任何文件、未改全局默认项目路径**；待用户确认项目已处于可一致复制的状态，或明示接受非一致拷贝风险。
- 用户最终要求**由另一 Agent 执行 E3D TTY 测试**，本轮只在本地创建可执行的需求说明，不再做 E3D 会话验证、项目复制或读写。交接文档：[`e3d-tty-validation-handoff.md`](e3d-tty-validation-handoff.md)；列明正确宿主样本、T0/T1 属性与 RVM、原项目安全异常、旧 TTY 与当前脚本路径风险、通过/ambiguous 停止门。本文档本身不改变 D1–D3 状态。













## 12. 2026-10-18 隔离副本 TTY 动态验证（APS /ALL + gen_model_test 运行时）

> 报告：[`e3d-tty-validation-report-20261018.md`](e3d-tty-validation-report-20261018.md)；证据根 `verification/e3d-tty-native-20261015-075723/`。全程 `-tty APS` + 既有隔离副本（launcher/result/落盘三重核实，非 AMS、非原项目）；运行时 `shadow_e3d31_gen_model_test`（des.exe 3.1.9.0 / core.dll 1.3.13.0，哈希见报告 §2）。写前已建 `aps000-backup-post-t0/`（240 文件全量备份）。

### 会话结果一览（详见报告 §3）

- **T0 只读**（/68、/72、/70、/71 属性 + /68 原生 RVM）：干净成功（exit=0，双 done）。
- **/68 受控写**：首次写会话（RECONNECT RECALCULATE+SAVEWORK+FLUSH）崩溃且无 done，`aps250110_0001` 追加一页 +2048B；改用**分段 ALPHA LOG**（每段独立落盘）重试两次——`t1w3`（RECALCULATE）与 `t1w4`（END）均完整落出 seg1(pre) 后**死于 seg2=RECONNECT 段内**，定位崩溃点为 RECONNECT 命令本身。
- **/70 受控写**（`t1w5`，同拓扑对照：JOIE=下一段 SCTN 之 SNOD 上的 SJOINT）：`RECONNECT END` 正常返回、会话内 post 查询完成、`SAVEWORK` 完成（三标记齐全）；死于 seg5=FLUSH/收尾段。**原生 reconnect 在一致元素上可执行且字段幂等**——/70 全字段 pre/post/独立会话复采三方一致，SAVEWORK 后 `aps250110` 无新增页。
- **T1 只读复采**：/68、/72 全字段与 T0 逐项相同；/68 T1 RVM 与 T0 逐字节仅 HEAD 块差异（实体/顶点/面片完全一致）；/70 复采属性一致，但其 RVM 导出段崩溃未获得。
- **字节级影响**：240 文件逐一比对，唯一语义变化为 `aps250110_0001` +2048B（首次写会话追加）；其余 111 个变化文件仅 session stamp（size 不变）。

### 裁决（三态）

**SCTN/SJOI 250110 样本类：①「现有正体覆盖、无新增刀证据」**，caveat 如下：

- 原生 /68 保存态为单 FacetGroup，斜切端面已含于正体（双向表面 0.007071 mm）；写尝试前后字段与 RVM 零变化；同拓扑 /70 上原生 RECONNECT END 幂等；静态 `RECON0→STRCNT→JTCONN→PUTEND` 链只写 POS/DRN 字段、无 cutter builder。
- **显式 caveat**：/68 自身 reconnect 被 core.dll `0xC0000005 @ +0x74D982` 崩溃阻断（两次两参数形式），「/68 reconnect 也无改动」是 /70 + T0/T1 外推；「扰动状态下 reconnect 是否回写新字段」未测（需更重的受控写实验）。
- 崩溃族定位：同址 AV 命中于 未挂载库 refno 选择（aux 两探针）、/68 RECONNECT（×2）、/70 RVM 导出、/70 会话 FLUSH——属元素引用解算路径同族崩溃，可重复、非全局（纯读+查+导出稳定）。

### 对 D1–D3 的影响（保持未勾）

- **D1**：仍不勾。动态证据=「reconnect 可执行、一致态幂等」，未证「状态变化时回写」；/68 写路径崩溃未闭合。
- **D2**：仍不勾。控制样本裁决=①已覆盖（skipped 可按「原生无额外刀/端面已覆盖」单独记账），但仅限本拓扑样本类；7200 PJOI/SJOI 与有源 PJOI（/500 GENSEC）分支未做同根两态。
- **D3**：仍不勾。7200 `23584/*` 未挂载于 /ALL，本轮零证据；`/5902` 等须先列入隔离只读导出方案再复对拍。
- **E2**：推进——副本同根两态写实验已实际执行（一次成功 reconnect+savework、三次崩溃）；供裁决所用证据已入 §12 与报告，不等同 #34 全样本验收完成。

### 2026-10-19 更新：D1–D3 改勾完成

§12 列的三条不勾理由本轮全部闭合：7200 库经 `e3d-io-write` 挂载进副本 `/ALL`（绕开 ADMIN 子系统崩溃），`/5902` 等 8 宿主原生导出到手；「状态变化时回写」改为**逐类原生 RVM 对拍**取证（J-B 嵌套 SJOI 零图元、J-C PJOI 真空、SBFI mode-6 刀实切）。勾选详情见上文 Phase D 与 `2026-10-18-sctn-model-generation-fixes/plan.md` §9。
