# 开发计划：部署面板启动 `aios-database` 房间模型初始化门禁与重新初始化能力

> 来源：用户 2026-11-13 需求——「部署面板启动 aios-database 时，初始化必须先把房间模型生成；正常的流程是部署启动时，房间初始化是必须过，过了才能走下一步。同时在部署面板里提供重新初始化房间模型的功能。」
> 状态：**草案（待 plannotator 批注门禁）**

---

## 1. 现状核查与问题定位

| 层面 | 现状 | 缺口 |
|---|---|---|
| **后端启动（ingest 主路径 `src/lib.rs` `run_cli`）** | `run_read_through_core`（`src/lib.rs:2518–2544`）在 `room_membership = true` 时已调用 `initialization.begin_startup_rooms()` / `settle_startup_rooms()` / `fail_startup_rooms()`，并门控 `open_model_phase()` 与 `mark_model_ready()`；而 `run_cli`（`src/lib.rs:1282`）仅调用 `prepare_room_resident_at_startup(&db_option).await`，失败时只打印告警并继续启动，**未**接入 `InitializationCoordinator` 的房间门禁 | 正常部署启动（`run_cli`）时，即使房间模型初始化失败（`Blocked`），`initialization.model_ready` 与后续流程仍可能放行 |
| **后端重建闭环（`src/web_service/handlers.rs` `run_rooms_rebuild`）** | `POST /api/v1/rooms/rebuild` 会在后台重建常驻房间模型并清边/重算，但 `run_rooms_rebuild` 结束后**未**回写 `InitializationCoordinator::global()`（`settle_startup_rooms` / `fail_startup_rooms`），也未在启动门禁解除后补开 `open_model_phase()` | 启动时房间初始化失败 → 在面板点击「重新初始化房间模型」成功后，`initialization.startup_rooms_pending` 与 `room_bootstrap` 仍停在失败态 |
| **部署向导后端（`src/web_service/setup.rs`）** | `setup_complete`（`ensure_completion_ready`）只校验 `mode == "normal"` 与核心配置完整；`update_execute` 未在入口拦截「房间初始化未通过」 | 房间模型未生成或失败时，API 仍允许直接执行 `update/execute` 或 `setup/complete` |
| **部署向导前端（`web/setup.html` 第⑤步）** | `#step5`（⑤ 初始化数据库）直接展示库列表与「执行初始化 / 完成向导」按钮，**不**轮询 `/api/v1/health` 的 `room_build.resident` / `initialization.room_bootstrap`，**不**阻塞下一步，也**没有**「重新初始化房间模型」按钮 | 部署人员看不到房间初始化状态与失败原因，未过房间初始化就能点「执行初始化」或「完成向导」 |
| **运维面板（`web/ops.html`）** | `web/ops.html` 未接入 `POST /api/v1/rooms/rebuild` | 运维侧缺少直接触发「重新初始化房间模型」的按钮 |
| **一键部署脚本（`scripts/stack/stack.ps1`）** | `Get-StorageVersionKind` / `Show-StorageVersionAdvice` 只看 `initialization.status == "discovering"` 与 `model_ready`，未识别 `room_bootstrap` / `room_build.resident` 阻塞 | `stack up` / `stack init` 遇到房间初始化阻塞时无法给出明确提示与门禁拦截 |

---

## 2. 目标流程

```text
部署启动 aios-database (normal 模式)
  │
  ▼
1. 装载空间树 + 初始化常驻房间模型 (prepare_room_resident_at_startup)
  ├── [Loading] ──▶ 面板显示「房间模型初始化中（阶段/进度）」，阻塞「执行初始化 / 完成向导 / 下一步」
  ├── [Blocked / Failed] ──▶ 面板报错（展示 last_failure / problems），阻塞下一步；
  │                          用户可点击【重新初始化房间模型】→ POST /api/v1/rooms/rebuild → 重新轮询
  └── [Ready (或允许的 Degraded)] ──▶ settle_startup_rooms() → 门禁放行！
        │
        ▼
2. 进入下一步（勾选库执行初始化 POST /api/v1/update/execute → 完成向导 POST /api/v1/setup/complete）
   （注：步骤 ⑤ 与 ops.html 均长期保留【重新初始化房间模型】按钮，随时可触发整库房间模型重建）
```

---

## 3. 详细改造方案

### 阶段 P1：后端启动与重建门禁（`src/lib.rs`, `src/web_service/handlers.rs`, `src/web_service/setup.rs`）

1. **统一 `run_cli` 与 `run_read_through_core` 的房间启动门禁（`src/lib.rs`）**
   - 提取共享辅助函数（避免两处重复逻辑）：在 `prepare_room_resident_at_startup` 前后同步 `InitializationCoordinator::global()`：
     - 当需要房间能力时，调用 `initialization.begin_startup_rooms()`。
     - 执行 `prepare_room_resident_at_startup(&db_option).await`。
     - 检查房间常驻模型是否就绪（见 §4 决策点 D1/D2）：就绪则 `initialization.settle_startup_rooms()`；未就绪则 `initialization.fail_startup_rooms("blocked", reason)`（记录具体失败原因）。
   - `run_cli` 中若房间门禁未过，不得将 `model_ready` 放行，确保 `/api/v1/health` 与 `/api/v1/setup/state` 能真实反映阻塞状态。

2. **打通 `POST /api/v1/rooms/rebuild` 与 `InitializationCoordinator` 的状态回写（`src/web_service/handlers.rs`）**
   - `run_rooms_rebuild` 开始时若处于启动阻塞恢复场景，保持 `startup_rooms_pending` 门禁直到重建完成。
   - `resident::rebuild_resident_model()` 成功并 `record_startup_init(Ok(&outcome))` 后，按门禁策略调用 `initialization.settle_startup_rooms()`（并按需放行 `open_model_phase()`）。
   - 若 `rebuild_resident_model()` 失败，调用 `initialization.fail_startup_rooms("blocked", format!("{error:#}"))`，确保门禁保持关闭并暴露最新失败原因。

3. **接口层硬门禁（`src/web_service/setup.rs`, `src/web_service/handlers.rs`）**
   - `POST /api/v1/setup/complete`（`ensure_completion_ready`）：当房间初始化为必需且尚未通过时，返回 `409 Conflict`（或 `precondition` 错误），明确提示「房间模型初始化尚未通过，请等待完成或重新初始化房间模型」。
   - `POST /api/v1/update/execute`：当房间初始化为必需且尚未通过时，拒绝入队并返回带明确错误码的 `409`（防止先入队后逐根失败）。
   - `/api/v1/setup/state` 或 `/api/v1/health`：确保前端第⑤步能直接拿到房间模型状态（`room_build.resident.status`、`phase`、`last_failure`、`problems`、`generation`、`initialization.room_bootstrap`）。

---

### 阶段 P2：部署面板（`web/setup.html`）与运维面板（`web/ops.html`）

1. **`web/setup.html` 第⑤步「⑤ 初始化数据库」增加房间初始化门禁与重初始化按钮**
   - **前置状态卡片（房间模型门禁）**：
     - 进入 `#step5` 时立即轮询 `/api/v1/health`（与现有 `pollGen` 机制一致，不重叠发请求），实时展示：
       - `loading`：显示加载动画 + 当前阶段（`spatial-tree` / `source-snapshot` / `mesh-warm` / `edge-recompute`）与进度，提示「正在初始化房间模型，完成后才能继续」。
       - `ready`（绿色）：显示「房间模型已就绪 ✅（代号 / 面板统计）」，解锁「刷新库列表 / 预览 / 执行初始化 / 完成向导」。
       - `degraded`（按决策点 D2 处理）：展示缺口 `problems` 明细。
       - `blocked` / `failed`（红色）：展示 `last_failure.reason` / `problems`，提示「房间模型初始化未通过，无法走下一步」。
       - `disabled` / `unsupported`：按决策点 D1 展示提示或放行。
   - **门禁联动**：
     - 房间初始化未通过前，禁用 `#btn-execute`（执行初始化）、`#btn-preview`（预览）与 `#btn-finish`（完成向导），并在按钮旁/状态卡给出明确原因。
   - **「重新初始化房间模型」按钮（`#btn-rebuild-rooms`）**：
     - 无论当前是 `blocked`、`degraded` 还是 `ready`，均可点击「重新初始化房间模型」（二次确认后调用 `POST /api/v1/rooms/rebuild`）。
     - 触发后立即进入轮询状态，锁定下一步按钮直到本轮重建重新变为就绪，失败则展示错误并允许再次重试。

2. **`web/ops.html` 补充房间模型状态与「重新初始化房间模型」按钮**
   - 在运维面板工具栏/状态区展示 `room_build.resident.status` 与 `last_failure`，并提供「重新初始化房间模型」操作按钮（复用 `POST /api/v1/rooms/rebuild` + 进度轮询）。

---

### 阶段 P3：一键部署脚本联动（`scripts/stack/stack.ps1`）

- 在 `Get-StorageVersionKind` / `Show-StorageVersionAdvice` / `stack init` 前置检查中读取 `/api/v1/health` 的 `room_build.resident` 与 `initialization.room_bootstrap`：
  - 若处于 `loading`，归类为等待态并提示正在初始化房间模型。
  - 若处于 `blocked`，立即停止继续执行 `stack init` / 启动 `rs-server`，打印 `last_failure.reason` 并提示通过面板或 `POST /api/v1/rooms/rebuild` 重新初始化房间模型。

---

### 阶段 P4：自动化测试与验收

1. **Rust 单元测试 / 源码契约测试**
   - `setup.rs`：`setup_complete` 在房间模型未就绪（`Blocked` / `Loading`）时返回错误；就绪后放行。
   - `handlers.rs`：`update_execute` 在房间门禁未过时拒绝入队；`run_rooms_rebuild` 成功后解除 `InitializationCoordinator` 门禁。
   - `web/setup.html`：静态结构/逻辑断言（存在房间门禁状态区、门禁未过前禁用执行/完成按钮、存在「重新初始化房间模型」按钮并调用 `/api/v1/rooms/rebuild`）。
2. **PowerShell 脚本测试（`scripts/stack/tests/`）**
   - 补充 `Get-StorageVersionKind` 对房间阻塞/加载态的用例，运行 `pester` 验证全绿。
3. **编译与全量检查**
   - `cargo test --lib` 相关模块通过。

---

## 4. 决策点（请在 plannotator 里批注；每条第一项为推荐）

1. **D1. 什么配置下「房间初始化是必须过」？**
   - **(a) [推荐] 只要 `room_membership = true`（即当前配置开启了房间归属计算），就必须通过房间初始化；若显式配置 `room_membership = false` 或 `AIOS_SPATIAL_COMPUTE=0`（明确关闭房间体制），则门禁自动豁免并提示「已关闭房间计算」**
   - (b) 无条件强制：即使 `room_membership = false` 也必须建完房间模型才能走下一步（需同步调整相关配置/测试）。

2. **D2. `ResidentLifecycle::Degraded`（常驻模型已装好，但存在个别缺口面板 `problems`）是否算「过了」？**
   - **(a) [推荐] `Ready` 放行，`Degraded` 在部署向导默认阻塞（需用户看到缺口并可点「重新初始化房间模型」，或勾选/确认「忽略缺口继续」），`Blocked` / `Loading` 绝对阻塞**
   - (b) `Degraded` 与 `Ready` 一样直接自动放行（仅在界面显示黄色提示），仅 `Blocked` / `Loading` 阻塞。

3. **D3. 部署向导 `web/setup.html` 第⑤步触发「重新初始化房间模型」后的交互范围**
   - **(a) [推荐] 同时在 `web/setup.html`（第⑤步）和 `web/ops.html`（运维面板）都提供「重新初始化房间模型」按钮，均调用现有 `POST /api/v1/rooms/rebuild`**
   - (b) 仅在 `web/setup.html` 第⑤步提供，不动 `web/ops.html`。

4. **D4. 后端 `update_execute` 门禁范围**
   - **(a) [推荐] `POST /api/v1/update/execute` 与 `POST /api/v1/setup/complete` 双重门禁：房间未就绪时均返回 `409 Conflict` 拦截**
   - (b) 仅门禁 `POST /api/v1/setup/complete` 与前端按钮，`update_execute` 不拦截。
