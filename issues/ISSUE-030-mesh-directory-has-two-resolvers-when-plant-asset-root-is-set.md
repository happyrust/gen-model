# Issue #030: 设了 `PLANT_ASSET_ROOT` 又配了 `meshes_path` 时网格目录有两个答案——生成写一处、房间面板预热读另一处，两边各自「成功」

## 📋 Issue 信息

- **Issue ID**: #030
- **类型**: Bug 🐛（配置解析口分叉：静默失效）
- **优先级**: High 🟠（房间归属整库 degraded、无一处报错；ADR-068 -full 包外的任何自定义 `meshes_path` 部署都会踩）
- **状态**: Fixed ✅（2026-09-19，见下「修法」；live 复验见 `docs/adr/ADR-068 … D7`）
- **创建日期**: 2026-09-19
- **解决日期**: 2026-09-19
- **相关模块**：`src/options.rs`（`apply_asset_root` / `get_db_option_ext`）、`aios_core::options::DbOption::get_meshes_path`、
  `fast_model/room_resident.rs`（面板网格预热 `warm_for_install`）、`fast_model/mesh_generate.rs`（生成侧落盘）、
  `web_service/handlers.rs`（`/api/v1/meshes/{hash}`、`/assets/meshes/*`）

## 🔍 问题描述

网格目录在仓里有**两个**解析口，各认各的：

| 口 | 谁用 | 认什么 |
|---|---|---|
| `aios_core::DbOption::get_meshes_path()` | 生成侧落盘、`/api/v1/meshes/{hash}`、房间常驻模型预热面板网格、房间输入缓存、历史模型服务…… 全仓 22 处直接调 | 只认配置 `meshes_path`，缺省 `assets/meshes`（相对工作目录） |
| `options::apply_asset_root()` → `get_db_option_ext().inner.meshes_path` | `run_app` 往下传的 `db_option`（启动期房间初始化 `initialize_resident_rooms_at_startup` 拿的就是它） | `PLANT_ASSET_ROOT` 一设就改写成 `<root>\meshes`，压过配置 |

同一个进程里两处拿到的目录不同，而且**没有一处会报错**：生成照常写、预热照常「找不到文件」进保护集、初始化照常 in_sync。

### 现场（2026-09-19 f13 隔离部署，`0.1.28+g082d092d7e01`）

- `DbOption.toml`：`meshes_path = "D:/…/ams-init-1112-7997-20260919-f13/meshes"`；`start.ps1`：`PLANT_ASSET_ROOT = <ws>\assets`。
- 冷库起服务 + `--auto-initialize-new-dbnums --watch-dbnum 1112,7997`：
  - 两库初始化成功：7997 6772 根 / 0 失败、1112 18 根 / 0 失败，`applied == file == model`、`model_verdict=in_sync`；
  - 网格全部落在 `<ws>\meshes`（19749 个文件）；
  - `/health.room_build.resident`：**`status=degraded`、`panels_ready=0/230`、`problems` 230 条**，每条都是
    `dbnum=7997 panel=… stage=mesh-warm … reason=<ws>\assets\meshes\<geo_hash>.mesh: 系统找不到指定的文件。`；
  - `Watch-InitRun.ps1` 因 `room_build.resident.status != ready` 永远等不到 DONE。
- 把 `<ws>\assets\meshes` 做成指向 `<ws>\meshes` 的 junction、清库重跑：`[房间初始化] … 230 块有几何 / 网格 230/230`、
  `room=ready`、2.5 min DONE。f20 / f37b 两次部署之所以没踩到，正是因为它们一开始就做了这个 junction。

### 预期行为

同一个进程里「网格在哪个目录」只有一个答案，且与 `/assets` ServeDir 挂的 `asset_root()` 同源——浏览器端拿 `.mesh` 走
`/assets/meshes/<hash>.mesh`，生成侧写到别处，前端就拿不到。配置被环境变量压掉时要在启动日志当场点名。

### 实际行为

见「现场」。两条路径各自都成功，唯一的症状是房间面板「没有几何」，而这一格在 ADR-073 D11 的口径下**不是错误**（缺几何面板受保护），
于是连 `/health.status` 都只是 `degraded`、不 `blocked`。

## 🔬 根本原因

`apply_asset_root` 只改写了 `DbOptionExt.inner` 那一份**克隆**；`aios_core::get_db_option()` 是 `OnceCell` 的全局原件，没有 setter，
覆盖层进不去。凡是直接拿全局原件调 `get_meshes_path()` 的地方——也就是全仓绝大多数——都看不见 `PLANT_ASSET_ROOT`。
这与 ADR-075 D6 记的校审附件目录问题同构：写死 `assets/review_attachments` 那一侧与 `asset_root()` 那一侧各成功。

## 🛠️ 修法（本次已落）

- `src/options.rs`：网格目录只剩一个解析口 `meshes_dir_of(&DbOption)`（纯解析）/ `meshes_dir_for(&DbOption)` / `meshes_dir()`
  （顺手建目录，沿用 `get_meshes_path` 的语义）。顺序：`PLANT_ASSET_ROOT`（非空白）→ `<root>\meshes`；否则配置 `meshes_path`；
  否则 `asset_root()/meshes`。`apply_asset_root` 只是把这个答案写回 `meshes_path`，不再另有算法。
- 全仓 22 处 `get_meshes_path()` 直调改走解析口；`clippy.toml` `disallowed-methods` 禁掉 `DbOption::get_meshes_path`
  （lib 顶上 `#![allow(warnings)]` 之后补 `#![deny(clippy::disallowed_methods)]`，否则这条在 lib 里是空挡——负向探针实测拦得住），
  `options::tests::meshes_dir_is_the_only_resolver` 扫 `src/` 再守一遍。
- 配置里的 `meshes_path` 被压掉时 `run_app` / `run_cli` 启动各打一次
  `⚠ DbOption.toml 里的 meshes_path = … 被环境变量 PLANT_ASSET_ROOT 压过：网格目录按 … 生效`（`options::meshes_dir_notice`，
  与 ADR-083 D2 的 store_mode 点名同一条纪律）。

### 风险

- 设了 `PLANT_ASSET_ROOT` 且配置 `meshes_path` 指向别处的现场，**生成侧落盘位置会变**（从配置目录改到 `<root>\meshes`）：这正是要的
  ——此前它写的那个目录前端与房间根本读不到；老网格留在原地不动，缺的按需重生。发布包（`Start-AMS.ps1`，不配 `meshes_path`）与
  `PLANT_ASSET_ROOT` 未设的开发跑法位置不变。
- 没设 `PLANT_ASSET_ROOT`、也没配 `meshes_path` 时兜底从字面 `assets/meshes` 改为 `asset_root().join("meshes")`：`asset_root()`
  在 exe 同级 / 上一级没有 `assets` 目录时仍回 `assets`，开发跑法一字不变；发布包形态下反而与 ServeDir 对齐。

## 🧪 验证

- 单测：`options::tests::{meshes_dir_prefers_asset_root_then_configured_path_then_fallback, meshes_dir_notice_only_when_the_configured_path_is_overridden,
  apply_asset_root_materializes_the_resolver_answer, meshes_dir_is_the_only_resolver}`、
  `web_service::handlers::tests::mesh_glb_handler_gates_suffix_and_hash_before_touching_the_disk`（改口成钉 `crate::options::meshes_dir()`）。
- live：f13 同一份 `DbOption.toml` + 同一个 `start.ps1`、**拆掉 junction**、清库重跑——启动日志出现那句 ⚠ 点名，网格落到
  `<ws>\assets\meshes`，`[房间初始化] … 230/230`、`room=ready`、`Watch-InitRun` DONE。结果记在 ADR-068 D7。

## 🔗 相关

- ADR-068 D5（「后端从 `PLANT_ASSET_ROOT = workspace\assets` 供 mesh」）→ 本次追记 D7
- ADR-075 D6 / S5（同构的「一个函数而不是两处字面量」）
- ADR-073 D11（缺几何面板受保护、不判死——正因为如此这条缺陷才无声）

## 🏷️ 标签

bug config meshes room PLANT_ASSET_ROOT silent-failure

---

**创建者**: fable-5-1-13
**最后更新**: 2026-09-19
