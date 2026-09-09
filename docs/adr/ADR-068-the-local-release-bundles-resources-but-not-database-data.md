# ADR-068：本地发布包带全资源、但不带库数据与源模型

状态：**已接受**（2026-09-09 用户「全按推荐来」拍板发布追问 Q1–Q9：包里那份 exe / 全资源档 / 一体化布局 / 加 CHANGELOG / -full 命名 / 本机脚本全链路。**号到提交时复核**——ADR-060 曾撞号两次，ADR-067 亦有此注）。
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
- `DbOption.toml` **原样带走、不改写**：`store_mode` 默认 `external`、`v_port=8010`、`project_path` 绝对路径照旧。换机器时这些是「连回原环境」的坐标，不是包内自足的东西。

### D4　为什么这条线站得住（难回头的口径）

**已生成模型的三维可离线看**：模型树走文件、几何走 `assets` + accel 快照，都不吃库——所以带 `assets` + 快照就够看已生成的模型。**但属性 / 搜索 / 补齐 / 更新仍需连回原库**（`8010`）或先导入数据。这条边界写进包内 README 与 CHANGELOG，不让「换机器解压即跑」被误读成「数据也自足」。要自足到底是「全离线」档的事（D1）。

### D5　Web 不重复带 meshes

后端从 `PLANT_ASSET_ROOT = workspace\assets` 供 mesh；浏览器端 Web 经后端 `/assets` 取。故组包**剔除 `web\assets\meshes`**（现有包目录里那 636 MB / 4.8 万个文件是历史残留，官方 `release.json` 本就未收）——避免与 `workspace\assets\meshes` 重复近一倍体积。

### D6　版本纪律与冒烟门（Q8 / Q9）

- 版本号以两仓 `Cargo.toml` 为准；`releases` 里已有同名 zip 则**拒绝执行**（`-Force` 才覆盖）；CHANGELOG 找不到当版条目**只警告不中止**（gen-model changelog 按日期记，严格中止会常误伤）。`releases` 旧版不自动清理，人工删。
- 组包后**冒烟门**：把包内服务拉起来，`health` 200 + 首页含 `plant_ui_app.js` 才算过。默认用 `spawned-mem`（自带内存库、不依赖外部 8010）跑，自足可判。端口被占 / 起不来 → 跳过并在 `release.json.smoke` 与产物名旁标「未冒烟」，**不静默放行**（`-StrictSmoke` 可改为起不来即中止、不出包）。

## 后果

- 产物 `ams-local-0.1.19-full.zip` 约 394 MB（压缩），解压即跑、已生成模型三维可看；补齐 / 更新要连回原库或先导数据。
- `surreal.exe` 进包但**库是空的**——换机器要么先 `workspace\Start-Surreal.ps1` 起空库再导数据，要么连回原 `8010`。
- **fork 2.1.x 硬约束**：别用 PATH 上 `cargo install` 装的 3.x——会要求对 `.surreal` 做不可逆迁移、且与 2.1.4 客户端握不上手。包内 `bin\surreal.exe` 就是对的那个。
- `release.json` 记程序文件逐一 sha256 + 资源目录摘要（`workspace/assets` 字节数 / 文件数）+ 冒烟结果 + 整包 sha256（另写 `.zip.sha256` sidecar）。
- 「资源进包、库数据不进包」成为难回头的发布口径：将来要真自足，是「全离线」新档，不是改这一档。

## 实施约束

1. `New-LocalRelease.ps1` 全链路：构建 / 复用（`-SkipBuild` 复用已随发布验证的包内 exe/web，Q1=A）→ 组 `workspace\`（`assets` 从源直灌进 zip、不落中转副本，省磁盘）→ 组 CHANGELOG（后端取 `docs/release-notes/<版本>.md` 提炼稿、前端截 plant-ui CHANGELOG 当版段，Q4=A）→ 冒烟门 → zip + sha256。
2. 后端提炼稿：`docs/release-notes/<版本>.md`，下一版新增同名文件即可；缺则 CHANGELOG 后端节留占位并警告（D6）。
3. 一体化布局的 `Start-AMS.ps1` 默认 `-Workspace = <包内>\workspace`；`Rollback.ps1` 仍指向原安装（`test-worklspace\bin`），不回退数据库数据。
