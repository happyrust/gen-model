# A4 前端走查自动化

计划 `docs/plans/2026-09-12-review-backend-port-plan.md` §6 A4 的走查工具：把 §4 的三条用户路径
（U1 设计 / U2 校对 / U3 PMS 嵌入）跑成可重复的脚本，采集**控制台 fallback 告警**与**每一条校审请求的
URL / 方法 / 状态码**，并对照 §1.2 契约表逐条勾。

A4 原文：「走查 U1、U2 全程无一次 mock 兜底（`VITE_REVIEW_ALLOW_MOCK_FALLBACK=false`，浏览器控制台无
fallback 告警），附件上传 multipart 成功且 `/files/review_attachments/...` 可打开」。

本目录依赖自带（`package.json` + `package-lock.json` 都在这里），不进仓库根；`node_modules/` 与产物
`out/` 都在 `.gitignore` 里。plant3d-web 仓**只读**（计划 N4），本走查不改它任何 tracked 文件。

---

## 1. 装依赖

```powershell
cd scripts\review\walkthrough
npm ci
npx playwright install chromium
```

本机 2026-09-13 实测：`~\AppData\Local\ms-playwright\chromium-1228` 已在，`playwright install chromium`
是秒回的空操作。Node 要 ≥ 20（本机 v26.1.0）。

> 本目录**不跑 cargo**。真要编后端，先在 worktree 根目录 dot-source 环境脚本：
> `. .\scripts\review\env.ps1`（它把 `CARGO_TARGET_DIR` 换成本 worktree 专用的，
> 免得和别的 worktree 抢同一个 target dir）。

## 2. 先决条件：后端与前端怎么接

### 2.1 后端

```powershell
# worktree 根目录
. .\scripts\review\env.ps1
cargo run --features http_api
# 另开一个窗口确认
curl http://localhost:8022/api/review/health
```

`[review_auth]`、`[platform_auth]`、`[review_attachments]` 三段配置见 `docs/specs/review-api.md` §1.1；
U3 还要求 `[platform_auth].review_web_base_url` 指向下面那个前端地址（计划 Q5：这一项顶替了旧端的站点
注册表 `managed_project_sites::get_site`）。

### 2.2 前端指向 :8022 的两条路子，**行为不同**

| 档位 | 怎么写 | 前端解析 | 抓到的请求 origin |
| --- | --- | --- | --- |
| `query`（默认） | URL 上带 `?backendPort=8022` | `apiBase.ts:88-90` 的 URL 参数覆盖，优先级最高 | `http://localhost:8022`（浏览器直连，靠 gen-model 的 CORS） |
| `env` | `VITE_GEN_MODEL_API_BASE_URL=http://localhost:8022` | dev 下 `apiBase.ts:68-70` 把 loopback **折成同源空串** | `http://localhost:3101`，由 Vite 的 `/api` 代理转给 :8022 |

env 档的折叠是刻意设计（局域网访问时 loopback 指的是访问者自己的电脑），不是 bug。折叠之后 Vite 的
`/api` 代理上游按 `VITE_BACKEND_PORT || 从 VITE_GEN_MODEL_API_BASE_URL 推断 || 3100` 取
（`vite.config.ts:216-220`）——**没设 `VITE_BACKEND_PORT` 又没设 env base 时它会打到旧端 3100**。
本脚本自己起 dev server 时把这几个变量全显式钉死（`config.mjs::webServerEnv`），不靠推断。

**U3 只能用 env 档**：嵌入地址是后端 `embed-url` 现拼给 PMS 的，带不上 `?backendPort=`。
用 query 档跑 U3，preflight 会直接拦下并说明原因。

## 3. 跑

```powershell
cd scripts\review\walkthrough

# ⓪ 离线自检：不碰后端、不开浏览器，验契约匹配器与控制台分类器。装完依赖先跑它。
npm run selftest

# ① 空跑：只做 preflight，不开浏览器、不改任何数据。后端没起就在这里失败。
npm run dry-run

# ② U1 + U2（A4 判据的主体）
npm run u1u2

# ③ U3（PMS 嵌入）——必须 env 档
$env:REVIEW_BACKEND_WIRING='env'; npm run u3

# ④ 三条一起
$env:REVIEW_BACKEND_WIRING='env'; npm run walkthrough
```

退出码：`0` 通过 / `1` 走查判失败 / `2` preflight 失败 / `3` 总超时 / `4` 参数或环境错。

### 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `REVIEW_BACKEND_URL` | `http://localhost:8022` | 后端地址 |
| `REVIEW_WEB_URL` | `http://localhost:3101` | 前端 dev server（`vite.config.ts` 写死 `strictPort: true`） |
| `REVIEW_WEB_REPO` | `D:\work\plant-code\old\plant3d-web` | 前端仓路径（只读） |
| `REVIEW_START_WEB` | `auto` | `auto` 端口没在听才起 / `never` 只用现成的 / `always` 一律自己起 |
| `REVIEW_BACKEND_WIRING` | `query` | `query` \| `env`，见 §2.2 |
| `REVIEW_STEPS` | `u1,u2,u3` | 也可用 `--steps=` |
| `REVIEW_PROJECT_ID` | `AvevaMarineSample` | |
| `REVIEW_SJ_USER` / `REVIEW_JD_USER` | `SJ` / `JD` | 两个角色的 user_id |
| `REVIEW_U2_DECISION` | `approve` | U2 最后一下走 `approve` 还是 `return`，两条都在契约表里 |
| `REVIEW_MANUAL` | `0` | `1` = 人工接管：开着浏览器只录不点，你手动走完回车 |
| `REVIEW_HEADLESS` | `1` | `0` = 有头，便于盯 |
| `REVIEW_TOTAL_TIMEOUT_MS` | `600000` | **总**超时 |
| `REVIEW_STEP_TIMEOUT_MS` | `120000` | **单步**超时（必须小于总超时，否则启动即报错） |
| `REVIEW_ACTION_TIMEOUT_MS` | `20000` | 单个页面动作超时 |
| `REVIEW_PROBE_TIMEOUT_MS` | `5000` | preflight 探活超时——要短，探不通立刻失败 |
| `REVIEW_CLEANUP` | `1` | U3 收尾是否调 `/api/review/delete` 软删 |
| `REVIEW_TASK_ID` / `REVIEW_FORM_ID` | — | 单独跑 U2 时指定上游产物 |
| `REVIEW_OUT_DIR` / `REVIEW_RUN_ID` | `./out` / 时间戳 | 产物落点 |
| `REVIEW_WEB_LOG` | `0` | `1` = 把 Vite 的 stdout 也打出来 |

## 4. 每步在断言什么

### U1 设计人员（sj）—— `steps/u1.mjs`

1. `POST /api/auth/token` 取 sj token（role 只能是 `admin|sj|jd|sh|pz`），拿到 `form_id`。
2. 打开 `/?automation_review=1`，点 ribbon「校审 → 发起编校审」，等 `window.__plant3dInitiateReviewE2E`。
3. `addMockComponent(refno)` 挂一个构件 → `submit()` 建提资单 → `getLastCreateResult()` 必须给出 `taskId`。
4. `POST /api/review/attachments` 传一张真 1×1 PNG（后端对 png/pdf/jpg 做文件头校验，随便造的字节会被 400 挡）；
   响应必须 `success:true` 且带 `attachment.url`。
5. `GET /files/review_attachments/<stored_name>` 必须 200 且字节数 > 0 —— **A4 的后半句**。
6. `POST /api/review/tasks/{id}/submit` 推到 jd。
7. `GET /api/review/tasks/{id}/workflow` 的 `history` 里必须有一行 `action` 含 `submit` —— §4 U1 的终点判据。

### U2 校对（jd）—— `steps/u2.mjs`

1. 以 jd 取 token，**沿用 U1 的 `form_id`**（不然不是同一条流）。
2. `GET /api/review/tasks?checker_id=JD` 当「待我审」。列表里没有本单**不判失败**：外部驱动模式下
   checker 可以为空、owner 由 PMS 每次 sync 显式声明（`common.rs:1059-1064`），真结论看第 7 步。
3. 「校审面板」→ `window.__plant3dReviewerE2E` → `addMockAnnotation` + `addMockMeasurement`。
4. `confirmData()` → `GET /api/review/records/by-task/{id}` 必须非空。
5. `window.__plant3dDtxE2E.addCloudAnnotationWithScreenshot()` 传云线附件。**这一条失败不阻断走查**，
   只记 `NOT RUN` —— 云线自动截图要三维画布真渲染出来，无 GPU 的无头环境常拿不到。
6. `comments` → `annotations`（轻量字段）→ `annotations/{id}/severity` → `annotation-states/apply`，
   逐条要 `success:true`。
7. `approve` 或 `return`（`REVIEW_U2_DECISION`）。若回 403「权限不足」，看
   `common.rs::format_owner_mismatch_error`——外部驱动模式下要改走 `POST /api/review/workflow/sync`。
8. `history` 与 `workflow` 两个端点都要能回放（行数 > 0）。

### U3 PMS 平台（S2S）—— `steps/u3.mjs`

驱动用 plant3d-web 自带的仿 PMS 页 `pms-review-simulator.html`（仓库根，Vite dev 当静态页发）。
选择器 `#tool-create` / `#review-iframe` / `#project-id-status` 与 `[data-testid="embed-bootstrap-loading"]`
出自该仓 `e2e/pms-review-simulator-{embed-project,mocked}.spec.ts`。

1. 等 `#project-id-status` 出现「项目列表 N 项」。
2. 点 `#tool-create` → `#review-iframe` 的 `src` 必须含 `user_token=`（即 `POST /api/review/embed-url` 成功）。
3. 嵌入页 `embed-bootstrap-loading` 消失，且**不能**出现「选择项目」标题——出现就说明 project 预选没生效。
4. S2S 面（凭据在请求体 `token` 字段里，不走 `review_auth`）依次打
   `workflow/sync` → `workflow/verify` → `annotations/check`，`code` 必须 200。
5. 收尾 `POST /api/review/delete` 软删（`REVIEW_CLEANUP=0` 可留着数据人工看）。

### 为什么用 window 钩子而不是 DOM 选择器

`src/review` 下全仓 grep 零 `data-testid`（2026-09-13）。但 plant3d-web 自带三个 E2E 钩子，
在 `?automation_review=1` 或 `localStorage['plant3d_automation_review']='1'` 时挂到 window 上：

| 钩子 | 出处 | 用在 |
| --- | --- | --- |
| `__plant3dInitiateReviewE2E` | `src/components/review/InitiateReviewPanel.vue:521-546` | U1 |
| `__plant3dReviewerE2E` | `src/components/review/ReviewPanel.vue:1277-1410` | U2 |
| `__plant3dDtxE2E` | `src/composables/useDtxTools.ts:3165-3208` | U2 云线 |

钩子要面板 mount 之后才挂，所以每步都是「先点开面板，再等钩子」。点面板用的是 ribbon 按钮文案
（`src/ribbon/ribbonConfig.ts:156-189`），前端改文案就同步改 `ui.mjs` 的 `RIBBON` 常量；
点不开会抛一句写明「该点哪个按钮」的错，不会静默等到超时。

**实在驱动不了就人工接管**：`REVIEW_MANUAL=1 REVIEW_HEADLESS=0` 重跑，脚本只开浏览器、只录控制台与
请求，你手动走完回车——采集与判据、产物格式完全一样。

## 5. 控制台怎么分类

A4 说「无 fallback 告警」，但 plant3d-web 里 fallback 有两种，后果完全不同，混在一起报会误导验收人。
`capture.mjs` 按源码逐字分类：

| 类别 | 匹配 | 出处 | 含义 |
| --- | --- | --- | --- |
| `mock-used` | `using development mock data` | `useUserStore.ts:691,720` | **兜底真的用上了**，页面上的数据是假的。A4 判 FAIL。 |
| `mock-disabled` | `production mock fallback is disabled`、`Local review mock mode is disabled in production` | `useUserStore.ts:694,723,1542` | 兜底按 A4 的要求关着，但后端这一次没答上。同样 FAIL，**归因在后端**。 |
| `shadow` | `[review/M2 SHADOW]` | `reviewSnapshotService.ts:96,119,153` | 影子比对告警，与 mock 兜底无关，**不判 A4**。 |
| `plain` | 其余 | — | 普通日志，原样记，不判断。 |

沿用别人已经起着的 dev server 时（`REVIEW_START_WEB=auto` 且 3101 已在听），本脚本无法从外部确认它的
`VITE_REVIEW_ALLOW_MOCK_FALLBACK` 到底是不是 `false`。这一点由分类兜住：真出现 `mock-used` 就说明没关，
报告里会直接点名。要绝对干净就 `REVIEW_START_WEB=always` 让脚本自己起。

这张表和契约匹配器是整份报告的地基，错了会让报告「看起来全绿」（宪法 III 的静默失效），
所以 `selftest.mjs` 单独钉它们，`npm run selftest` 不依赖后端。2026-09-13 做过两次变异验证，
两次都真的红了：把 `mock-used` 的匹配串改掉 → 2 条断言失败；把匹配器「具体路径压过占位符」
的排序反过来 → `GET /api/review/tasks/returned` 被 `/api/review/tasks/{id}` 抢走，1 条失败。

## 6. 产物

`out/<runId>/`（`runId` 默认是 ISO 时间戳）：

| 文件 | 内容 |
| --- | --- |
| `report.md` | 人读版：A4 四条判据、分步结论、控制台分类计数、失败请求表 |
| `report.json` | 机器读版，含完整配置快照与 preflight 明细 |
| `contract-checklist.md` | **计划 §1.2 逐条勾**：方法 / 路径 / 本次应打 / 命中数 / 状态码 / 判定 |
| `console.jsonl` | 全部控制台消息，每条带 `kind` 分类与 `step` |
| `requests.jsonl` | 全部校审域请求：方法、URL、状态码、契约条目、`origin_class` |
| `screenshots/` | 每步截图，文件名形如 `u1-04-task-created.png` |

`contract-checklist.md` 的判定：

- `PASS` —— 打到了且全是 2xx/3xx
- `FAIL` —— 打到了但有 4xx/5xx 或请求本身失败
- `MISS` —— 本次步骤按 `contract.mjs` 应当打到它，却一次都没打到（**链路断了**）
- `NOT EXERCISED` —— 在册但本次不必然触发（前端未调用，或要额外操作），不算失败

`requests.jsonl` 的 `origin_class`：`backend-direct`（query 档，直连 :8022）/ `via-dev-proxy`
（env 档，经 :3101 代理）/ `unexpected`（**打到了别的后端，多半是误落回旧端 :3100**）/ `failed`。
`unexpected` 非零直接判 FAIL。

## 7. 走不动时先看哪里

| 现象 | 多半是 |
| --- | --- |
| `preflight FAIL backend-reachable` | gen-model 没起，或 `REVIEW_BACKEND_URL` 指错 |
| `preflight FAIL route-registered` | 后端起着但校审路由没注册；先跑 `cargo test --features http_api --lib web_service::review` 看护栏 |
| `preflight FAIL u3-wiring` | U3 用了 query 档，改 `REVIEW_BACKEND_WIRING=env` |
| `等不到自动化钩子 window.__plant3d…` | 面板没点开 / URL 少了 `automation_review=1` / 前端版本不含该钩子 |
| `点不开「…」面板` | ribbon 文案变了，改 `ui.mjs` 的 `RIBBON` |
| `origin_class=unexpected` | 接线错了，请求落到 :3100 之类；看 §2.2 |
| 大量 `mock-disabled` | 后端在但校审接口在报错，看 `requests.jsonl` 的状态码 |
| 大量 `mock-used` | dev server 没带 `VITE_REVIEW_ALLOW_MOCK_FALLBACK=false`，A4 不成立 |

## 8. 参照与边界

- 上游参照（**只读**，未改动）：旧端 `plant-model-gen` 的 `scripts/pms-browser-flow.ts`
  （Playwright 打 PMS 登录页的最小样例）与 `shells/run_pms_workflow_mock_page.sh`
  （embed-url → tasks → records → comments → attachments → workflow/sync 的完整请求体形状，
  本目录的请求体照它对的）；plant3d-web 的 `e2e/pms-review-simulator-*.spec.ts`（U3 的选择器来源）。
- 本目录不改后端、不改计划文档与 `docs/specs/review-api.md`、不改 plant3d-web 任何 tracked 文件、
  不在仓库根新增 node 依赖。
- 脚本自己起的 dev server 结束时一定 kill（Windows 上走 `taskkill /t /f`，因为
  `npm run dev` 是 cmd → node 两层，`SIGTERM` 只杀得掉外层、会留下 vite 占着 3101）；
  沿用别人起的则不动它。
