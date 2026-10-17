# `model_cable_endpoint_cascade` 翻默认开检查单（GitHub #35 Phase C）

> 日期：2026-10-17
> 适用范围：[`src/data_interface/cable_endpoint.rs`](../../src/data_interface/cable_endpoint.rs)、[`src/options.rs`](../../src/options.rs) 的 `model_cable_endpoint_cascade`（环境变量 `AIOS_MODEL_CABLE_ENDPOINT_CASCADE`）
> 上游计划：[`docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md`](./2026-09-06-core3d-cascade-parity-dev-plan.md) §3 C6 / C8
> 配套执行单：[`.planning/2026-10-15-extgeo-cable-and-sctn-joint-cut/task_plan.md`](../../.planning/2026-10-15-extgeo-cable-and-sctn-joint-cut/task_plan.md)

---

## 0. 一句话结论

之前阻挡 `model_cable_endpoint_cascade` 默认开启的**三项前置已全部闭合**，今日（2026-10-17）将该开关默认值由 `false` 翻为 **`true`**：

1. **前置 1（电缆 `EXTGEO` 路由几何落地）✅ 已完成**（`happyrust/e3d-model@e427eb3`，2026-10-17）：
   `vendor/e3d-model/src/cable.rs` 已完整实现 `CABLE → RPATH → (POINTR | EXTGEO)` 的桥架锚定段展开、正交折角双法向抬升、逆序顶点 `arrive_up`、跨桥架段间圆角（重合端点、共线折返分件、高差双弯、水平侧移与平分面斜切封顶）、`MINBEN >= 0` 直连以及 `RNODE` 侧向净空退化门，原生 E3D RVM 4 根对拍基准（`2013286720/135`、`2013286720/16`、`2013286723/817`、`2013286720/575`）体积相对误差 `0.0085% ~ 0.1898%`、表面 Hausdorff p95 `0.18 ~ 2.12 mm`。
2. **前置 2（同库 + 跨库 `STAREF` / `ENDREF` 反向级联与交接完整性）✅ 已完成**（`96c4ccd88` + 本次 Phase C1）：
   - 规则 F（`EQUI` / `ELCONN`）与规则 E（`CWBRAN` + 4 种附着点 `HATTA` / `TATTA` / `RATTA` / `RNODE`）直接触发；
   - 落在 `CWBRAN` 子树内的桥架路径输入（`RPATH` / `POINTR` / 桥架件 `CTELE`）改动时，沿 T 端 `owners` 自动上卷到所属 `CWBRAN` 根并展开整棵分支子树点查 `STAREF` / `ENDREF`；
   - 跳过 `Disposition::NoModel`（元数据门放行条目，如 `aps250514 3→4` 只改 `HCREF` / `TCREF` 的 `CWBRAN`），杜绝元数据改动误拉电缆根；
   - 跨库引用者经 `CableEndpointCascade::with_foreign_members` → `cross_db_handoff::fold_elements_to_roots` 折成电缆库 `CABLE` 根并排 `ModelOnly`。
3. **前置 3（真库两态对拍与跨库真库门）✅ 已完成**（2026-10-17）：
   30 个真库增量窗口（`aps7200_0001` 3→4、`aps250514_0001` 3→4、`aps250176_0001` 全 5 步、`aps250179_0001` 全 5 步、`zdj7200_0001` 全 9 步、`ams8000_0001` 9 个钉死窗口）开关两态 `regen` 集合**逐字相同**，`query_failures = 0`、`engine_unavailable = false`、`referrer_without_root = 0`；APS 跨库真库门 196 条端点边 + 152 条 `EXTGEO` 路由边全部命中并折回目标 `CABLE` 根。

---

## 1. 全语料边表与覆盖矩阵

数据来源：`docs/evidence/2026-09-07-cross-root-reference-census-c8.json` 与 `cable_endpoint::tests::live`。

| 规则 | 引用属性 | 引用者 → 被引用目标 | 同库跨根边数 | 跨库边数 | 规划器触发源 | 落点 |
|---|---|---|---:|---:|---|---|
| **规则 F**（`ELCXRF`） | `CABLE.STAREF` / `ENDREF` | `CABLE` → `EQUI` / `ELCONN` | **36**（`zdj7200` 6 根 × 3 副本） | **196**（APS `250176` 等 → `250152/154/156/160/162/164`） | `EQUI`、`ELCONN`（跳过 `NoModel`） | 同库并入 `touched`；跨库交接 `ForeignRootHandoff` |
| **规则 E**（`CLIST`） | `EXTGEO.STAREF` / `ENDREF` | `EXTGEO`（owner²=`CABLE`）→ `CWBRAN` 子树内 `HATTA` / `TATTA` / `RATTA` / `RNODE` | **408** | **152**（APS `250179` ↔ `250176`） | `CWBRAN`、`HATTA`、`TATTA`、`RATTA`、`RNODE` 直接触发；`CWBRAN` 下 `RPATH` / `POINTR` / `CTELE` 沿 `owners` 上卷 `CWBRAN` | 同库并入 `touched`；跨库交接 `ForeignRootHandoff` |

---

## 2. 真库门与两态对拍实测结果（2026-10-17）

### 2.1 单测与读取集合护栏

- `cargo test --lib data_interface::cable_endpoint`：**10 passed, 0 failed**（含 `rpath_and_pointr_under_cwbran_fold_to_owning_cwbran_route_source` 钉死 `CWBRAN ← RPATH ← POINTR` 上卷、`CABLE` 下 `POINTR` 隔离、`Disposition::NoModel` 过滤）。
- `cargo test --lib data_interface::model_impact::tests::the_generation_read_set`：**2 passed, 0 failed**（150 项属性读取集合与 `vendor/e3d-model` 148 项护栏镜像一致）。
- `cargo test --lib options::tests::the_design_reference_cascade_defaults_on_and_stays_switchable`：**passed**（同时钉死 `scripts/templates/DbOption.factory.toml` 的 `model_cable_endpoint_cascade = true` 与 `effective_default_on_switch(None, None)` 一致）。

### 2.2 真库两态对拍（30 窗）与跨库门

运行命令：

```cmd
set "AIOS_E3D_TEMPLATE_DIR=D:\AVEVA\Everything3D3.1\Templates" && cargo test --lib data_interface::cable_endpoint::tests::live -- --ignored --nocapture --test-threads=1
```

| 用例 / 窗口集 | 窗口数 | 点查与命中表现 | 两态 `regen` 对拍 |
|---|---:|---|---|
| `live_zdj7200_an_equi_or_cwbran_source_pulls_its_cable_roots_in` | 2 组假定源 | `EQUI 23584/10206`：`queries=2, edges=6 -> referrer_roots=6 (CABLE)`；`CWBRAN×8`：`queries=122, edges=32 -> referrer_roots=6 (CABLE)` | 100% 精确命中 6 根 `CABLE` |
| `live_aps_a_cross_db_endpoint_finds_its_cable_in_the_other_library` | 98 个 DESI 库 | 跨库端点边 **196 条**（前 8 条逐一假定 `ELCONN` 变，`queries=22` 查回 `250176` 的 `CABLE` 根）；跨库 `EXTGEO` 路由边 **152 条**（前 8 条逐一假定附着点自身及所属 `CWBRAN` 下 `RPATH` 变，`queries=1400` 查回 9 根跨库 `CABLE`） | 全部命中并经 `fold_elements_to_roots` 折回 `CABLE` 根 |
| `aps7200_0001` `3→4` | 1 | `sources=0, queries=0` | `regen_on=6 == regen_off=6` |
| `aps250514_0001` `3→4`（含 2 条只改 `HCREF/TCREF` 的 `NoModel` `CWBRAN`） | 1 | `sources=0, queries=0`（`NoModel` 过滤生效） | `regen_on=12 == regen_off=12` |
| `aps250176_0001` 全链（`1→2, 2→16, 16→17, 17→18, 18→19`） | 5 | `2→16`：`sources=79 (route=79), queries=818, edges=132, referrer_roots=16`；其余 4 步 `sources=0` | 全 5 步 `regen_on == regen_off`（`2→16` 为 `35 == 35`） |
| `aps250179_0001` 全链（`1→2, 2→11, 11→12, 12→13, 13→14, 14→15`） | 6 | `2→11`：`sources=113 (route=113), queries=1464, edges=116, referrer_roots=16`；其余 5 步 `sources=0` | 全 6 步 `regen_on == regen_off`（`2→11` 为 `32 == 32`） |
| `zdj7200_0001` 全链（`1→2` 至 `9→10`） | 9 | `4→5`：`sources=30 (route=30), queries=166, edges=64, referrer_roots=6`；其余 8 步 `sources=0` | 全 9 步 `regen_on == regen_off`（`4→5` 为 `16 == 16`） |
| `ams8000_0001` 九个钉死窗口（`255→256` 等） | 9 | `195→196`：`sources=1 (endpoint=1), tables_present=0, queries=0`；其余 8 窗 `sources=0` | 全 9 窗 `regen_on == regen_off` |

---

## 3. 配置与代码对齐清单（Phase C3）

- [x] `src/options.rs`：`model_cable_endpoint_cascade()` 改調 `effective_default_on_switch`，文档注明 2026-10-17 默认开启（GitHub #35）。
- [x] `DbOption.toml` 与 `scripts/templates/DbOption.factory.toml`：写入 `model_cable_endpoint_cascade = true` 及说明注释，单测钉死模板值与 `effective_default_on_switch(None, None)` 相等。
- [x] `src/data_interface/cable_endpoint.rs`：模块文档与测试对齐。
- [x] `docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` 与 `CONTEXT.md`：同步更新 C8 状态为 `✅ 已接线并默认开启（2026-10-17，GitHub #35）`。
