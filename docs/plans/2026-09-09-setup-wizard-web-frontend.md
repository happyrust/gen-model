# 2026-09-09 配置向导网页前端（/setup.html）+ 设置模式

一句话：给本地发布包一个「解压 → 浏览器里一步步配置 → 初始化」的向导——内嵌单文件页
`/setup.html` + 服务端 setup API + 「设置模式」最小启动臂；配置真值不挪窝，仍是
`DbOption.toml` 文件。

## 1. 已拍决策（两轮 14 问，2026-09-09 用户全按推荐拍板）

| # | 决策 |
| --- | --- |
| Q1 | 向导管**首跑 + 日常**：首跑自动进入，日常从 `/setup.html` 随时回来 |
| Q2 | 向导 = **内嵌单文件页**（照 `ops.html` 模式：`include_str!` 内嵌 + 磁盘副本优先 + 来源响应头） |
| Q3 | 首跑未完成时首页自动跳 `/setup.html`；完成后恢复正常首页 |
| Q4 | **引导子集**五步（数据源→存储→服务→生成与监听→初始化），其余键留给手改 toml |
| Q5 | 存储步骤两档：`external`（带测试）+ `spawned-mem`（试用）；「包内持久库」本轮只给指引文案（一键拉起留 v2 单独拍） |
| Q6 | 配置生效走「**保存并重启**」端点：优雅自重启，向导页轮询等它回来；失败兜底提示手动重跑 `Start-AMS.ps1` |
| Q7 | 「初始化」内嵌**薄壳**：复用 `/dbnums` + `/update/preview` + `/update/execute`，复杂场景仍指去 plant-ui |
| Q8 | setup 端点**回环限定**：按**对端地址**判（TCP peer 非 127.0.0.1/::1 一律 403；监听地址不作判据——见 Q16 全 0 监听）（写进 ADR 当默认边界） |
| Q9 | 全链交付（服务端 + 页面 + 文档），随 `New-LocalRelease.ps1` 出 **0.1.20-full** 验收 |
| Q10 | 首跑判据 = 哨兵文件 `.first-run-pending` **+** 配置自检兜底（只认 `project_path` 缺/无效；库连不上不算） |
| Q11 | 新增**设置模式最小启动臂**，不改正常启动链的失败语义 |
| Q12 | 配置写盘用 `toml_edit` 外科手术式改键，**注释与未知键原样保留**，写前落 `DbOption.toml.bak` |
| Q13 | 初始化给**两档显式单选**：「只导数据（快）」/「导数据+生成模型与网格（慢）」，写回 `gen_model`/`gen_mesh`，界面明示耗时预期 |
| Q14 | SurrealDB 探测带版本核对：非本仓 fork 的 2.1.x **阻断下一步**（不给越过选项） |
| Q15 | （2026-09-09 用户补拍）出厂模板 `store_mode` **不用 spawned-mem**：默认 `external` + `v_ip`/`v_port` **留空**；向导 ② 步默认选中 external，spawned-mem 仍是可选档 |
| Q16 | （2026-09-09 用户补拍）默认监听**全 0**：出厂 `http_api_addr = "0.0.0.0:9099"`——应用面（plant-ui/API/assets）局域网可达，setup 面靠 Q8 的对端判据仍只许本机 |

## 2. 两个判据，各管一件事（对 Q10/Q11 的精确化）

- **设置模式（启动臂选择）= 配置无效**：`project_path` 未设置或目录不存在，**或**
  `store_mode=external` 且 `v_ip`/`v_port` 留空（出厂模板正是这个形态，Q15）→ 不连库、
  不扫描、不起 watcher/队列，只绑 HTTP（setup API + `/setup.html` + `/health` 标
  `mode:"setup"`）。**库地址填了但连不上不算无效**——那可能只是 8010 没开机，照旧走
  正常启动失败退出，别把真故障盖成向导。
- **首跑哨兵 `.first-run-pending`（跳转旗）= 向导没走完**：存在时 `/` 302 到 `/setup.html`。
  哨兵**不影响**启动臂。
- 为什么必须拆开：向导在 ④→⑤ 之间要重启一次。重启后配置已有效，必须**正常启动**（⑤初始化
  要全服务），但向导还没走完——若哨兵也决定启动臂，这一步就死锁在设置模式里。

## 3. 端点表（新增；设置模式与正常模式都注册；API 全部按**对端地址**回环限定）

回环闸的判据是 `ConnectInfo` 里的 TCP 对端：仅 `127.0.0.1`/`::1` 放行，不认
`X-Forwarded-For`。`/setup.html` 页面本身不拦（无害），API 全拦——远程打开页面会在第一个
403 上收到「请在服务器本机用 `http://127.0.0.1:<port>/setup.html` 打开」的整页提示（本机经
局域网 IP 访问也会撞 403，同一句文案引导换 127.0.0.1，属可接受边角）。

| 端点 | 作用 |
| --- | --- |
| `GET /api/v1/setup/state` | `{ mode, first_run_pending, config_path, workspace_dir, version }`——向导页开场先问它定位自己在哪一步 |
| `GET /api/v1/setup/config` | 引导子集当前值 + 配置文件路径；`store_mode` 被环境变量压过时注明生效值与文件值的差异 |
| `PUT /api/v1/setup/config` | 校验 + `toml_edit` 写盘（先落 `.bak`）；只认引导子集字段；回 `{ changed: [...], restart_required: true }` |
| `POST /api/v1/setup/probe/project` | 入参 `project_path` → `{ exists, projects: [{name, db_files}] }`，供 ① 步列项目 |
| `POST /api/v1/setup/probe/store` | 入参 `v_ip/v_port` → `{ reachable, version, fork_ok }`；`fork_ok=false`（非 2.1.x）前端阻断 |
| `POST /api/v1/setup/probe/port` | 入参 `http_api_addr` → `{ verdict: free \| self \| occupied, owner_pid? }` |
| `POST /api/v1/setup/restart` | 202 → 优雅自重启（见 §5） |
| `POST /api/v1/setup/complete` | 删哨兵文件；幂等 |

初始化不加新端点：`GET /api/v1/dbnums`（`awaiting_manual_initialization` 列表 + 水位三元组）、
`POST /api/v1/update/preview`、`POST /api/v1/update/execute`、进度轮询 `/dbnums`，全是现口。

## 4. 向导五步与字段明细

| 步 | 字段 | 交互 |
| --- | --- | --- |
| ① 数据源 | `project_path`、`included_projects`、`project_name`、`mdb_name`、`module`、`project_code`、`surreal_ns` | 路径 + 【探测】→ 列项目子目录与 db 文件数 → 多选 included、单选 project_name；其余格带现值可改 |
| ② 存储 | `store_mode`、`v_ip`、`v_port` | **默认选中 external**（地址端口留空待填 + 【测试连接】：连通 + 版本硬闸）/ spawned-mem 仍可选（试用说明：数据不落盘）；旁给「包内持久库」指引文案（`Start-Surreal.ps1` 命令可复制） |
| ③ 服务 | `http_api_addr` | 端口预检（free / self / occupied+PID） |
| ④ 生成与监听 | 初始化档位（→`gen_model`/`gen_mesh`）、`watch_dbnums`、`manual_db_nums` | 两档单选明示耗时预期；dbnum 列表逗号输入，空 = 不限；末尾【保存并重启】= `PUT config` + `POST restart`，页面按新 `http_api_addr` 轮询 `/health`，回来自动续到 ⑤ |
| ⑤ 初始化（正常模式下） | — | `/dbnums` 列出待初始化行（含 `manual_db_nums` 点名的）→ 勾选 → 【预览】显示条数 → 【执行】→ 逐库进度到 in_sync（applied==model==file_latest）；失败引导去 ops.html / plant-ui；【完成】= `POST complete` 删哨兵 → 跳 `/`（plant-ui） |

## 5. 重启交接（唯一的高危面）

回 202 → 关 axum 监听 → **显式释放进程实例锁**（deny-share 句柄提前 close）→ spawn 子进程
（同 exe、`serve`、继承 env 含 `DB_OPTION_FILE`）→ `exit 0`。子进程实例锁获取加
`AIOS_RESTART_HANDOFF` 重试窗（≤10 s）兜父子竞态。跨端口轮询靠 CORS 缺省 permissive 成立；
现场若配了 origin 白名单，向导降级为「配置已保存，请手动打开新地址」。向导显示/轮询地址时把
`0.0.0.0` **映射成 `127.0.0.1`**（浏览器不可导航到全 0）。`Start-AMS.ps1` 的
`process.json` 在自重启后会过期——0.1.20 release-notes 注明，v1 不追。

## 6. 文件清单

gen-model：
- `src/web_service/setup.rs`（新）：全部 setup handlers + `toml_edit` 写盘（`.bak`）+ 三探测 + 重启交接 + 回环闸
- `src/web_service/mod.rs`：setup 路由注册 + `/setup.html` 路由；抽 `setup_router()` 供两种模式复用
- `src/lib.rs`：`run_app` 顶部设置模式分支（配置无效 → `serve_setup_mode`，不往下走连库链）
- `src/options.rs`：配置文件路径解析（`DB_OPTION_FILE` 已有逻辑复用）+ 哨兵/配置无效判据 helper
- `web/setup.html`（新）：单文件向导页，`include_str!` 内嵌 + 磁盘副本优先 + `x-setup-panel-source` 头
- `Cargo.toml`：+`toml_edit`
- `scripts/templates/DbOption.factory.toml`（新）：中性出厂模板（`project_path` 空、`store_mode="external"` + `v_ip`/`v_port` 留空、gen 全关、watch 空、`http_api_addr="0.0.0.0:9099"`）
- `scripts/New-LocalRelease.ps1`：包内 workspace 换出厂模板 + 落哨兵 + 冲烟第二腿（见 §7）
- `docs/adr/ADR-069`（新）：向导内嵌单页；设置模式判据 = 配置无效（哨兵只管跳转）；配置真值是 toml 文件（HTTP 只是读写口）；非回环禁用写口
- `CONTEXT.md`：词条「配置向导 / 设置模式 / 首跑哨兵 / 引导子集」
- `docs/release-notes/0.1.20.md` + `changelog.md` 条目

plant-ui：**不动**。

## 7. 测试与验收

- 单测：回环闸判定（纯函数）；`toml_edit` 写盘保注释与未知键（fixture 往返）；三探测的校验器；
  设置模式判据真值表；路由存在源码钉（照 `mod.rs` 现有钉风格）；`setup.html` 内嵌钉（含向导标记串）。
- 冲烟两腿（`New-LocalRelease.ps1`）：
  1. 现有腿照跑：本机 workspace + spawned-mem 全启动，health 200 + 首页含 `plant_ui_app.js`；
  2. 新腿：临时目录放出厂模板 + 哨兵 → 起服务 → 期待 `/health` 带 `mode:"setup"` + `/setup.html` 200 且含向导标记。
- 手工走查：本机临时 workspace（出厂模板 + 哨兵）全流程走到 ⑤（spawned-mem 档初始化一个小库）。
- 收口：`New-LocalRelease.ps1` 出 `ams-local-0.1.20-full.zip` 过两腿冲烟。

## 8. 顺序与协调

1）`setup.rs` + `lib.rs` 分支 + 单测 → 2）`setup.html` → 3）打包脚本 + 冲烟两腿 → 4）文档
→ 5）0.1.20-full 出包验收。

协调：**不碰 `handlers.rs`**（新代码全在 `setup.rs`）；`increment_manager.rs` / `handlers.rs`
此刻有别的会话在途未提交改动，动 `mod.rs` / `lib.rs` 前先看一眼 `git status`，提交只点名自己的文件。

## 9. 风险

- 实例锁父子竞态：重试窗兜底；兜不住的表现是子进程「second process」退出，向导轮询超时后
  给手动重跑 `Start-AMS.ps1` 的文案，不静默。
- 出厂模板与真机差异：冲烟第二腿只验设置模式面；初始化面（⑤）靠本机手工走查兜。
- 端口改动 + CORS 白名单并存时跨源轮询被拦：降级文案（§5）。
- 全 0 监听下 setup 面的安全完全押在对端判据上：单测给回环闸真值表（v4/v6 回环、局域网、
  代理头伪造均拒），ADR-069 把「对外暴露 setup 面必须另加认证」写成硬边界。
