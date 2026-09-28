# ADR-068：本地发布包带全资源、但不带库数据与源模型

状态：**已接受，2026-09-13 修订为 runtime 默认 embedded-mem、带 workspace 档默认 spawned-rocksdb**（完整包以跨重启持久化为默认，同时保留 kv-mem 增量 overlay。）
**2026-09-13 再修订（ADR-074）**：存储只剩 `mem` / `rocksdb` 两档、服务从不拉库，**两种包出厂都是 `mem`**（上一句里
「带 workspace 档默认 spawned-rocksdb」作废）。下文凡 `embedded-mem` 读作 `mem`；凡 `spawned-rocksdb` / `external` 读作
`rocksdb`——但 `rocksdb` 档**服务只连不拉起**，要持久的现场先跑包内 `Start-Surreal.ps1` 再在向导改选；`spawned-mem` 一档
不复存在。D6 的冒烟腿按 ADR-074「后果」一节读：B 腿默认 `mem`，C 腿由脚本起库、服务连它。
**2026-09-19 追记 D7**：网格目录只有一个解析口 `options::meshes_dir*`，`PLANT_ASSET_ROOT` 压过配置 `meshes_path`（issue #030）。
日期：2026-09-09
落地：`scripts/New-LocalRelease.ps1`（全链路组包）、`docs/release-notes/<版本>.md`（后端提炼稿）、产物 `releases/ams-local-0.1.19-full.zip`。
关联：
- **不动** 现有 `releases/ams-local-0.1.19`（程序档：exe + web，README 明说配置 / 资源 / 模型缓存 / SurrealDB 都靠 `test-worklspace\bin`）——本 ADR 记的是它之上补的「一体化全资源」那一档，不改它。
- CONTEXT.md 新增词条「本地发布包 / 依赖资源包 / 冒烟门」。

## 背景

现有 `ams-local-0.1.19`（105 MB zip）只带程序：后端 `aios-database.exe` + plant-ui Web 静态资源。README 明说运行时依赖 `test-worklspace\bin` 里的配置、资源、模型缓存与一台在 `127.0.0.1:8010` 跑着的 SurrealDB——**换机器就跑不起来**。用户要一个「解压即跑」的发布包，把「相关依赖资源」一并放进去。

难点在切一条线：哪些进包、哪些不进。资源里 `assets`（含 `meshes` 813 MB）体量大但**内容寻址、与部署环境无关**；而 SurrealDB 数据目录（`rocksdb://ams.db`）与源模型（AVEVA E3D 工程 `D:/AVEVA/Projects/E3D3.1`）既大、又与具体库 / 工程环境强绑，进包既臃肿又容易误导「换机器数据也跟着走」。

## 决定

### D1　三档分级，本次交付「全资源（-full）」

- **轻装**（约 +45 MB）：`DbOption.toml` / `config` / `resource` / `search-index` / accel 快照 / `surreal.exe` + 起库脚本；不含 `meshes` 与库数据。
- **全资源（-full）**：再加整个 `assets`（`meshes` / `fonts` / …）。**本次交付这一档。**
- **全离线**：再加 SurrealDB 数据目录，彻底自足。**留将来**，名字 `ams-local-<版本>-offline` 预留，本次不占。

### D2　-full 带什么，装哪里（Q2 全资源 / Q3=A 一体化）

进包资源装进**包根 `workspace\` 子目录**，`Start-AMS.ps1` 默认 `-Workspace` 指向包内 `workspace\`，与 `test-worklspace\bin` 同构，解压即跑：`DbOption.toml`、`config\`、`resource\`、`search-index\`、`accel_tree_*.snapshot`、整个 `assets\`（`PLANT_ASSET_ROOT` 指向它）、`bin\surreal.exe`（本仓 fork 的 2.1.x）+ `Start-Surreal.ps1`。

### D3　-full 不带什么（本 ADR 的核心边界）

- **SurrealDB 数据目录不进包**（`rocksdb://ams.db`，8010 那台的持久层）。
- **源模型不进包**（AVEVA E3D 工程目录）。
- 包内 `DbOption.toml` 不复制打包机的绝对路径：`project_path` 留空并带
  `.first-run-pending`，首跑进入配置向导。runtime 档默认 `store_mode=embedded-mem`，
  无 SurrealDB 子进程与数据库端口；带 workspace 档默认 `spawned-rocksdb@127.0.0.1:8009`，并默认开启房间归属计算。
  runtime 包根仍保留 fork 2.1.x 的 `bin\surreal.exe`，供用户显式切回 spawned 档。
- HTTP 监听与展示分开：`http_api_addr` 可用 `0.0.0.0:9099` 覆盖全部接口，
  `http_public_addr` 填当前主机可导航的局域网地址；首跑向导仍只从回环入口打开。

### D4　为什么这条线站得住（难回头的口径）

**已生成模型的三维可离线看**：模型树走文件、几何走 `assets` + accel 快照，都不吃持久库——所以带 `assets` + 快照就够看已生成的模型。runtime 的 `embedded-mem` 与 workspace 档的 `spawned-mem` 都派生为 read-through，属性直接从用户指定的 E3D 文件读取；内存数据随进程退出消失。浏览器默认服务供数，树、属性、搜索与模型统一走当前页面同源 API，不要求直连 Surreal。房间兼容页仍使用直连库的保留读面：embedded 下客户端必须根据 `mirror.endpoint=embedded:mem` 明确拒绝，不能误连配置里遗留的 TCP 端点。需要这些兼容页或外部 SQL/探针时改 `spawned-mem`；要保留摄入数据，改 `spawned-rocksdb` 或 `external`。

### D5　Web 不重复带 meshes

后端从 `PLANT_ASSET_ROOT = workspace\assets` 供 mesh；浏览器端 Web 经后端 `/assets` 取。故组包**剔除 `web\assets\meshes`**（现有包目录里那 636 MB / 4.8 万个文件是历史残留，官方 `release.json` 本就未收）——避免与 `workspace\assets\meshes` 重复近一倍体积。

### D6　版本纪律与冒烟门（Q8 / Q9）

- 版本号以两仓 `Cargo.toml` 为准；`releases` 里已有同名 zip 则**拒绝执行**（`-Force` 才覆盖）；CHANGELOG 找不到当版条目**只警告不中止**（gen-model changelog 按日期记，严格中止会常误伤）。`releases` 旧版不自动清理，人工删。
- runtime 冒烟两腿：出厂配置进入设置模式；本机真配置以 `embedded-mem` 完整启动，
  要求首页可用、`/health.sul_db.medium=embedded-mem` 且没有 Surreal 子进程。
- 带 workspace 档仍跑三腿：设置模式、默认 `spawned-rocksdb`、隔离的 `spawned-rocksdb@18010`。
  失败写进 `release.json.smoke`，`-StrictSmoke` 下中止出包。

### D7　网格目录只有一个解析口；`PLANT_ASSET_ROOT` 压过配置里的 `meshes_path`（2026-09-19 追记，issue #030）

D5 说「后端从 `PLANT_ASSET_ROOT = workspace\assets` 供 mesh」，落地时却只落了一半：`options::apply_asset_root` 把
`PLANT_ASSET_ROOT\meshes` 写进 `DbOptionExt.inner` 那一份**克隆**的 `meshes_path`，而 `aios_core::get_db_option()` 是没有 setter 的
全局原件，全仓 22 处 `get_meshes_path()` 直调（生成侧落盘、`/api/v1/meshes/{hash}`、房间常驻模型预热面板网格、房间输入缓存、
历史模型服务……）都只认配置里的 `meshes_path`。发布包不配 `meshes_path`、工作目录又恰是 `workspace\`，两口径碰巧落同一个目录；
配了 `meshes_path` 的现场两边就分家——2026-09-19 f13 隔离部署实测：网格写到 `<ws>\meshes`，房间常驻模型到 `<ws>\assets\meshes`
找面板网格，**230 块面板 `stage=mesh-warm` 全缺、房间 degraded，而两库初始化照样 in_sync**。两条路径各自都「成功」，与 ADR-075 D6
记的校审附件目录是同一种静默失效，修法也同一种：**一个函数，不是两处各算各的**。

- **解析口**：`options::meshes_dir_of(&DbOption)`（纯解析）/ `meshes_dir_for(&DbOption)` / `meshes_dir()`（顺手建目录，沿用
  `get_meshes_path` 的语义）。顺序：`PLANT_ASSET_ROOT`（非空白）→ `<root>\meshes`；否则配置 `meshes_path`；否则
  `asset_root()/meshes`。第一级与 `/assets` ServeDir 认的是同一枚环境变量、同一条优先级：浏览器端取 `.mesh` 走
  `/assets/meshes/<hash>.mesh`，挂的是 `asset_root()`，生成侧写到别处前端就拿不到——所以是环境变量压配置，不是反过来。
- **`apply_asset_root` 退成写回**：它只把解析口的答案写进 `DbOptionExt.inner.meshes_path`，让读那一格的人（`run_app` 往下传的
  `db_option`、向导回显）看到的与 `meshes_dir()` 是同一个值。
- **两道钉**：`clippy.toml` `disallowed-methods` 禁 `aios_core::options::DbOption::get_meshes_path`——lib 顶上的
  `#![allow(warnings)]` 会把 warn 级 clippy 全放行，所以紧跟一句 `#![deny(clippy::disallowed_methods)]` 把这一条单独抬到 deny
  （负向探针实测：塞一句直调，`cargo clippy --release --lib` 以 error 拦下；拿掉后 exit 0）；
  `options::tests::meshes_dir_is_the_only_resolver` 扫 `src/`，`cargo test` 侧再守一遍。
- **出声**：配置里的 `meshes_path` 被压掉时 `run_app` / `run_cli` 启动各打一次
  `⚠ DbOption.toml 里的 meshes_path = … 被环境变量 PLANT_ASSET_ROOT 压过：网格目录按 … 生效`（`options::meshes_dir_notice`），
  与 ADR-083 D2 的 store_mode 点名同一条纪律；两边同一个目录（含 junction）或没配 `meshes_path` 时不出声。
- **后果**：设了 `PLANT_ASSET_ROOT` 且 `meshes_path` 指向别处的现场，生成侧落盘从配置目录改到 `<root>\meshes`——此前写的那个目录
  前端与房间本就读不到，老网格留在原地、缺的按需重生。发布包与未设 `PLANT_ASSET_ROOT` 的开发跑法位置不变（`asset_root()` 在 exe
  同级 / 上一级没有 `assets` 时仍回 `assets`）。`lib.rs::instance_lock_skip_reason` 那句「先让两份配置的 `meshes_path` 分家」照旧成立，
  只是「分家」现在指解析口的答案，不是文件里那一行。
- **live**（2026-09-19 21:51，`0.1.28+g76a4dd480f26.…dirty`，f13 同一份配置、同一个 `start.ps1`、拆掉 `assets\meshes → meshes`
  的 junction、清库重跑）：启动 stderr 第一行就是那句 ⚠（`meshes_path = "…/f13/meshes"` 被压过 → `…\f13\assets\meshes`）；
  `[房间初始化] published 文件 [] → 215 间房 / 230 块在册面板 / 230 块有几何 / 0 块无几何 / 网格 230/230 键 0.6 MB`，`常驻房间模型：开，ready（代 72979202d292）`；
  `Watch-InitRun` 21:52:09 → 21:54:45 DONE：7997 6772 根 / 0 失败 / 房间边 66135、1112 18 根 / 0 失败 / 房间边 11716，两库 `applied == file == model`、
  `in_sync`、`room_alignment.current=true`；`/health` `status=ok`、`resident.panels_ready=230/230`、`mesh_warm_failed=0`。
  网格 19749 个文件**全部**在 `<ws>\assets\meshes`（不是 junction），配置指的 `<ws>\meshes` 保持 0 个文件；
  `GET /assets/meshes/<面板 geo_hash>.mesh` → 200（5772 字节）。库上 `room_relate` 1112 = 5858 / 7997 = 58396、`room_panel_relate` = 229，
  `e3d.room.lookup` `17496/72903 → ready_nonempty [R144/R101]`、`24381/105161 → [R232/R332]`、`17496/121765 → ready_empty`——
  与修前带 junction 那一跑（同日 20:57）逐字相同。修前不带 junction 的对照（同日 20:45）：`0 块有几何 / 230 块无几何（预热失败 230）`、
  `room=degraded`、守望永不 DONE。

## 后果

- 产物解压后双击 `Start-AMS.cmd`；首次自动进入向导。runtime 配置 E3D 项目后以内嵌
  kv-mem 启动，不会因机器上已有 SurrealDB 端口而拒绝启动。
- `surreal.exe` 仍随包并做 2.x 版本与 SHA-256 校验，但在 runtime 默认路径上不启动；
  它服务于显式切换 spawned-mem / spawned-rocksdb。内存库不持久；持久需求用 spawned-rocksdb/external。
- **fork 2.1.x 硬约束**：别用 PATH 上 `cargo install` 装的 3.x——会要求对 `.surreal` 做不可逆迁移、且与 2.1.4 客户端握不上手。包内 `bin\surreal.exe` 就是对的那个。
- `release.json` 记程序文件逐一 sha256 + 资源目录摘要（`workspace/assets` 字节数 / 文件数）+ 冒烟结果 + 整包 sha256（另写 `.zip.sha256` sidecar）。
- 「资源进包、库数据不进包」成为难回头的发布口径：将来要真自足，是「全离线」新档，不是改这一档。

## 实施约束

1. `New-LocalRelease.ps1` 全链路：构建 / 复用（`-SkipBuild` 复用已随发布验证的包内 exe/web，Q1=A）→ 组 `workspace\`（`assets` 从源直灌进 zip、不落中转副本，省磁盘）→ 组 CHANGELOG（后端取 `docs/release-notes/<版本>.md` 提炼稿、前端截 plant-ui CHANGELOG 当版段，Q4=A）→ 冒烟门 → zip + sha256。
2. 后端提炼稿：`docs/release-notes/<版本>.md`，下一版新增同名文件即可；缺则 CHANGELOG 后端节留占位并警告（D6）。
3. 一体化布局的 `Start-AMS.ps1` 默认 `-Workspace = <包内>\workspace`；`Rollback.ps1` 仍指向原安装（`test-worklspace\bin`），不回退数据库数据。
