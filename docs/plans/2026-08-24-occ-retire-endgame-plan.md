# 计划：OCC 退役收官（specs/009-retire-occ 终局）

**日期**：2026-08-24（凌晨 02:00 快照；同 worktree 另有一会话在推进，见 §0 末）
**输入**：`specs/009-retire-occ/tasks.md`、`docs/adr/ADR-030-retire-occ-tessellation.md`
（含 IDA 修订二）、`docs/adr/ADR-044-libgm-facet-caliber-and-unit-mesh-identity.md`、
`docs/evidence/2026-08-23-occ-retire-census.md`
**目标**：走完从「manifold 是唯一形状引擎」到「`occ` 退出 default / release」的最后一段，
每一步带可验收的门。

---

## 0. 现状快照（均已在 CI 口径验证；全量 `--lib` 1067 通过 / 1 预期红）

已收口：

- **WP-F 假回退**（T033–T037）：`tessellate_libgm_param` 只剩一处 `Ok(None)` 且语义固定为
  「非形状」；OCC 形状回退在 `src/fast_model/occ_generate.rs` 整段拆除。
- **WP-G 口径**（T038 / T039 / T040 / T040a / T042）：曲面段数按真实半径走
  `src/fast_model/libgm_discretise.rs` 权威规则；回转与挤出两套轮廓口径分开；
  弦高容差全库唯一 `FACET_TOL_MM`，兜底默认值已删。柱/球写死段数收成
  `unit_mesh_identity` 具名欠账，椭圆碟经向收成具名口径欠账。
- **IDA 钉参数**（T011 / T013 / T016，2026-08-24 完成）：Snout / 两种环面 /
  Pyramid / 斜端柱的参数顺序全部与实现逐位对上。**T011 顺带查出两处摆位缺陷**，
  开出 WP-K（见下）。
- **WP-H 解绑**：T043 代码已落（`src/rvm_baseline/mesh_compare.rs` 的 `gen_side` 改挂
  `manifold`，OCC 降为可选参照），CI 口径（不带 `occ`）编译已验；T044 完成
  （`src/fast_model/loop_model.rs` 断言走生产路径；浸水 STP 插件确认零调用后
  整个模块删除，业务上不再需要）。
- **护栏**：T017 源码断言落地（曲线图元不得成为形状臂）。
- **WP-I 盘点**（T045）：两库交叉验证，三类防御口子出现次数均为 0；
  单位柱段数等价类 8009=7、7997=37，**T041 裁决为本期做**。

未收口（本计划的对象）：T002 收尾、T041、T050/T051/T052（WP-K）、T021、T038a 后半、
T004 后半、T008、T025–T028、T045a、T046–T049、T031、T032。

**并行提醒**：另一会话正在同一 worktree 连续推进（今晚已完成 T011/T013/T016、
弦高容差兜底删除、浸水模块删除），主战场在 `src/fast_model/libgm_discretise.rs`、
`src/fast_model/manifold_tessellate.rs` 与 vendor snout 一带。本计划分派前
**先对一次工作区与 `specs/009-retire-occ/tasks.md`**，凡它已做的按台账为准，不重复分派。

---

## 1. 终局判据（ADR-030 决策 1 + 决策 9）

`occ` 退出 default / release（T031）之前，必须全部成立：

1. 生产路径每个会出网格的原语都能稳定生成 `PlantMesh`（WP-F/G 已给）；
2. 曲面原语段数来自真实半径的权威规则，回转走 `setNSteps` 口径（已给，
   余 T038a 经向与 T041 身份键）；
3. **摆位对齐**（WP-K 新增）：段数不对是布尔收不敛，摆位不对是构件长在别处——
   偏心 Snout 的两处缺陷要么修掉、要么以活库普查证明是纯防御；
4. 量尺子先脱离被量对象（T043 已给编译半门）；
5. **RVM 门全绿**（T046–T049，阈值不放宽，FR-010）；
6. 无 occ 的 feature 组合下 `gen_inst_meshes` 失败闭合有 CI 测试钉住（T031 自带）。

---

## Phase V — vendor 收口（串行链，最先动；snout 三件事一次推上游）

> `../vendor/old-aios-core/src/prim_geo/snout.rs` 上现在压着三件事：V1 规范化修复
> （已写好未推）、T050 摆位、T051 Y 偏移。分三次推 vendor 是三次 rev 升级 +
> 三轮全量验证，**合成一批**。

- **V1（T002 收尾·上）** `../vendor/old-aios-core/src/prim_geo/snout.rs`：
  `gen_unit_shape` 的 `f32_round_3` 规范化修复提交并推上游。
  门：`rounded_equal_snout_hashes_produce_one_canonical_param` 在
  **local-deps OFF** 下转绿（它现在的红是「修复未发布」的正确报告）。
- **V2（T050 + T051，依赖 T052 的普查结论定验收强度）**
  `../vendor/old-aios-core/src/prim_geo/snout.rs`、`src/fast_model/mesh_primitives.rs`、
  `src/fast_model/manifold_tessellate.rs`：偏心 Snout 偏移改上下各摊一半
  （libgm `0x1009EA30` 顶底圈各 ±shift/2，`calcRange` 支撑函数独立佐证）；
  Y 方向偏移接通（`gm_CreateSnout` 第 4/5 参数是 xShift/yShift，本仓 `poff`
  单字段且 y 硬写 0）。**先查 `poff` 现承载的语义再动**（tasks.md T051 有注）。
  门：底/顶圈中心对称的纯函数单测 + 体积不变；属性映射单测；
  `poff != 0` 不改身份键的钉测。**T052 > 0 时另需一件真实偏心异径管过 RVM 门。**
- **V3（T041）** `../vendor/old-aios-core/src/prim_geo/cylinder.rs`、
  `../vendor/old-aios-core/src/prim_geo/sphere.rs`、`src/fast_model/pdms_inst.rs`：
  柱/球 `hash_unit_mesh_params()` 混入段数，`gen_unit_shape()` 带段数，
  `canonical_unit_param_json` 的 `CYLINDER_GEO_HASH` 特判按新键走，
  `manifold_tessellate::unit_mesh_identity` 欠账常量组整组删除（其 doc 已写明
  「T041 换键时整组删」，T039 的两条源码断言随之改判）。
  门：不同半径两柱 `geo_hash` 不同且段数各自正确；同段数两柱共享一行；
  2026-08-13 双键 `param` 回归照跑。
- **V4（升 rev）** `Cargo.toml` + `python/Cargo.toml` **两处一起**升 aios-core rev
  （f835a9da 的教训：只升一边依赖图里出双份 crate，共享类型对不上），
  `Toggle-LocalDeps -Off` 后 CI 口径 `cargo check` + `--lib` 全量。
- **V5（整库重建，需排窗口）**：改身份 = 整库重建（ADR-044 决策 6）。
  按 T045 数据预估 `.mesh` 份数 1→7（8009）/ 1→37（7997）。
  **决策点 D1**：重建窗口与库范围；V1–V4 可以先合、重建后置，但 V3 合入后到
  重建完成前，柱/球增量走新键写新行——混合期时长要一并拍板。

## Phase I — IDA 残余（与 Phase V 并行；T011/T013/T016 已由并行会话完成）

- **I1（T038a 后半）** 反编译 `GM_EDish::calcFacets`（`0x10054AB0`）的
  `knuckleRadiusToUse` / `radiusOfHub`，把 `libgm_discretise` 里刚具名的
  椭圆碟经向欠账换成权威两段分算（球冠段 + 过渡角段，`2(n_c+n_k) > 1000` 封顶）。
  门：对照表单测手算自反编译结论。**与并行会话对表后再动**。
- **I2（T021）** 反编译 `0x107318E0` 斜切延伸段 → 纯函数落
  `src/fast_model/sweep_mesh.rs`，T023 / T048 的前置。

## Phase L — 活库普查与 live 验证（需 8009；纯源码项可立即做）

- **L1（T052，普查先行）** 一条 SurrealQL 统计 `PrimLSnout` 中 `poff` 非零的行数与
  实例数，结论写回 plan 的 WP-I 一节——它决定 V2 的验收强度（纯防御 vs RVM 硬前置）。
- **L2（T026，纯源码，可立即做）** 源码断言：生产路径不调用
  Clip / Expand / Compress / SolidTree / Picture / Clash。
- **L3（T027）** 确认 `apply_cata_neg_boolean_manifold` 是目录负体唯一入口（断言化）。
- **L4（T028 复核收账）** AABB-from-mesh 与「空 pts 拒绝」已有实现与源码断言
  （`T007: manifold meshes must persist AABB/pts`），把「pts 省略策略写进回执」
  核一遍后关账，不新写代码。
- **L5（T004 后半）** live `mesh_gwall_extra_against_cwall`（布尔 p95 ≤ 180ms 门），
  台账记档；**L6（T008）** 一箱、一柱、一挤出网格非空抽检，台账一行；
  **L7（T025 live 半）** 布尔 p95 门跑通（静态半已落）。
- **L8（T045a）** 两个库都没有球 / SSCL / 多面体。
  **决策点 D2**：另找含这三类（以及偏心 Snout，与 T052 合并找）的库跑现场验收，
  或在 plan 注明「未经现场验证」收口——不得默认它们跟圆柱一样安全。

## Phase R — RVM 门（依赖 T043（已给）+ WP-G/K 段数与摆位定稿）

> 段数没定稿前跑 RVM 是白跑——基准会随 V2/V3/I1 再变一次。排在 V、I 之后。

- **R1（T046）** 直墙 RVM（T019 的门）。
- **R2（T047）** 弧墙 + 360° SANN 体积门（T020 的门，FR-006）。
- **R3（T048）** 斜切墙 RVM（T022/T023 的门，依赖 I2）。
- **R4（T049）** 曲面原语抽检：柱 / 球 / PrimLSnout / 碟 / 圆环面各一，
  按新段数**重建基准**后对拍；T052 > 0 时必须含一件真实偏心异径管（T050 门 b）。
  阈值一律不放宽（FR-010）；证据进 `docs/evidence/`，live 台账同步。
  这一步同时收掉 T043 的「跑出对拍结果」后半门。

## Phase X — 摘除与收尾（全部前置绿后）

- **X1（T031）** `Cargo.toml`：default / release 特性组去掉 `occ`；
  CI 增加「无 occ 时 `gen_inst_meshes` 失败闭合」测试（bail 而非静默跳过）。
  回滚预案按 ADR-030 决策 7：只加回 `occ=true`，不恢复 OCC 布尔。
- **X2（T032）** `changelog.md`、live 台账、`cargo fmt` / `cargo check`、
  aios-core 解耦合上游（V4 已做即免）、`Toggle-LocalDeps -Off` 复核后推送。

---

## 决策点汇总（需要你拍板的）

| # | 事项 | 影响 |
|---|---|---|
| D1 | T041 整库重建的窗口与范围（V3 合入与重建是否同窗口、混合期可接受时长） | Phase V 排期 |
| D2 | T045a + T052：找含球/SSCL/多面体/偏心 Snout 的库，还是注记「未经现场验证」收口 | Phase L/R 完成定义 |
| D3 | RVM 基准重建选库（8009 副本 / 7997 副本 / 双跑） | Phase R 证据口径 |
| D4 | 双会话并行：本计划由哪个会话执行、另一个停在哪条边界 | 全程 |

## 明确不在本计划内

- **T040b**（硬边负号 = 曲面法向分组权威）：tasks.md 已注明单独开规格。
- **T024**（多段 SPINE）：无夹具，仍后置。
- l3_suite `geo` I-3 断言矛盾：不属于 009 线，另行处理。
- 净窗口 / insts-flat / 空间树等其他并行线（ADR-031/043/045）不混入。

## Constitution Check

- **响亮失败**：全程无新增静默分支；X1 自带失败闭合测试；V3 改身份走整库重建而非
  静默混键（ADR-044 决策 6）；WP-K 摆位修正必须过 RVM 或普查证伪，不许「读码即改」。
- **单一规则**：段数只剩权威规则 + 两处点名欠账，V3/I1 各销一处；容差唯一
  `FACET_TOL_MM` 且无兜底。
- **测试钉不变量**：每条带门；源码顺序断言（L2/L3）沿用仓内先例。
- **live 留痕**：L*、R* 全部进 `docs/evidence/` 与 live 台账。
- **运行环境**：全程禁 `cargo clean`；vendor 重定向只在本地验证用，推送前 `-Off`。

## 风险

1. **段数/摆位变更让 RVM 基准漂移**：R4 必须在 V2/V3 + I1 之后跑，基准重建记录
   段数来源版本，避免「新代码对旧基准」的假红/假绿。
2. **双会话同区写作**：I1 与并行会话的 T038a 周边直接重叠、V1/V2 同压一个 vendor
   文件；动工前对表（D4），否则互相冲写。
3. **混合期身份**：V3 合入后至重建完成前，新增量走新键、存量旧键——查询按
   `geo_hash` 直址不受影响，但 `.mesh` 份数巡检会看到双轨，D1 拍板时确认可接受时长。
4. **偏心 Snout 摆位修正改变现场几何**：T052 若非零，修正会让存量偏心异径管
   整体平移 `(XOFF/2, YOFF/2)`——这是**改对**，但查看者会看到构件动了位置，
   发布说明里要点名。
