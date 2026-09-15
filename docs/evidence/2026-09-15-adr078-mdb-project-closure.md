# ADR-078 真部署实测：项目集合由 MDB 派生、目录在搜索根里按 `<code>000` 找

- 日期：2026-09-15 20:13–20:38（本机 UTC+8）
- 计划：`docs/plans/2026-09-15-mdb-project-closure-and-room-default-dev-plan.md` §5.3（结论回填在该计划 §5.4）
- ADR：`docs/adr/ADR-078-projects-come-from-the-mdb-and-project-dirs-are-found-in-search-roots.md`
- 代码：`codex/model-projection-cache@cfed5fcce` + ADR-078 本批（隔离 worktree `D:\work\plant-code\old\gen-model-adr078`）

## 0. 一句话

部署实例把 `included_projects` 整行删掉之后，MDB /ALL 声明的 40 个外部库 **40/40 自己定位**（改前 40 条落空），
用户当初点不出图的那个 ZONE（`17496/8518`）`POST /model/ensure` 直接 `Generated · 1,731 实例`；
缺项目、第二个搜索根、掉线 UNC 三态都按 ADR-078 D2/D5/D6 报。跑的过程中逮到一个定位器缺陷（目录联接被当成非目录），当场修掉并钉住。

## 1. 台子

| 项 | 值 |
| --- | --- |
| 部署 | `D:\work\plant-code\old\_runs\ams-0.1.25`（**原样那一台**，不是复制品：它的 rocksdb 店里已有 1112 / 7997 的数据与模型，省一轮初始化） |
| 二进制 | `cargo build --release --bin aios-database` 于隔离 worktree，2m57s / 2m18s（第二次带联接修复）；换进包里，旧的留 `aios-database.exe.bak-0.1.25` |
| Web | `web\setup.html` 同样换成本批的（旧的留 `web\setup.html.bak-0.1.25`） |
| 库 | 包内 surreal 2.1.x（PID 46004，15:11 起，`rocksdb:…\workspace\.surreal\ams`，766 MB）——全程没重起过 |
| 配置 | `workspace\DbOption.toml`，**删掉 `included_projects` 整行**（备份 `DbOption.toml.bak-adr078-20260915-201353`）；`room_membership = false`、`watch_dbnums = [1112, 7997]` 照旧 |
| 合成搜索根 | `D:\adr078-verify\rootA`（AvevaMarineSample / ZDJ / SCB / AvevaPlantSample 四个**目录联接**）、`rootB`（只有 AvevaCatalogue 的联接）——真盘 `D:\AVEVA\Projects\E3D3.1` 一个字节没动，8009 / 8022 上别的会话的服务照常 |
| 起法 | `D:\adr078-verify\run-case.ps1`（WMI `Win32_Process.Create` 起 `cmd /c exe serve > log`，避免调用方的 stdout 管道被后端继承而收不到 EOF） |

## 2. 用例①：真根、无名单

配置：`project_path = "D:/AVEVA/Projects/E3D3.1"`，无 `included_projects`，`project_search_paths = []`。

启动横幅（`logs\20260915-201436.stdout.log` L60 / L95）：

```text
MDB /ALL：搜索根 1；闭包 5 个项目（AMS 主→AvevaMarineSample / ZDJ→ZDJ@根#1 / SCB→SCB@根#1 /
ACP→AvevaCatalogue@根#1 / APS→AvevaPlantSample@根#1），外部库 40/40 已定位；未定位项目 0
MDB /ALL / AvevaMarineSample：dbnum 路由目录 71 条路由（元素库 29，声明了没文件 0，构造期问题 0），零元素 I/O
```

- **声明了没文件 0**——改前同一台机器上是 40 条 `声明了 dbnum … 项目目录里却没有它的文件`（ACP 34 / SCB 4 / APS 2 / ZDJ 1 里，本次 MDB 实际声明 40 个外部库）。stderr 里只剩「监听限定模式」与「队列停滞看门狗」两句。
- `/health.mdb_closure`：`status = ok`、`roots[0].reachable = true`、`projects` 5 条（AMS 31 库 / ACP 34 / SCB 4 / ZDJ 1 / APS 1）、`unresolved = []`、`problems = []`、`seen_projects` 8 条（连 `E3D3.1-slyk-project\YCYK-E3D` 那种多一层的摆法都扫到了）。
- `degraded_sections = ["dbnum_consistency"]`，**不含** `mdb_closure`。
- 用户当初点的那个元素：`POST /api/v1/model/ensure {"refno":"17496/8518"}` →
  `status Generated · model_available true · 1,731 实例 · 6 个生成根 · dbnum 1112 · model_source database · durable true · publication ready`。
- `GET /dbnums`：1112 `applied 729 / file_latest 729 / model 729 / in_sync / dead_roots 0`；7997 `184 / 184 / 184 / in_sync / 0`。

起①之前还按**原样配置**（`included_projects = ["AvevaMarineSample", "AvevaCatalogue", "SCB", "ZDJ"]` 仍在）起过一次，横幅与①逐字相同——名单确实不再参与解析。

## 3. 用例②：缺 ACP

配置：`project_path = "D:/adr078-verify/rootA"`（没有 AvevaCatalogue），`project_search_paths = []`。

```text
- ACP -> <未解析出目录> [跳过] MDB /ALL 声明的 34 个库（7043, 7044, 7010, 7027, 7021, 7026, 7022, 7046 … 共 34 个）
  属于它，搜索根里没有 acp000 库目录：在 1 个可达的搜索根里都没有（找过：D:\adr078-verify\rootA）
MDB /ALL：搜索根 1；闭包 4 个项目（AMS 主 / ZDJ@根#1 / SCB@根#1 / APS@根#1），外部库 6/40 已定位；未定位项目 1：ACP(34 库)
MDB 成员选主：MDB /ALL: dbnum=7043 (*MASTER/APPUPDDICT) 属于项目 ACP，没有绑定到文件：……；不拿别的项目里同号的文件顶替
```

- `/health.mdb_closure.status = degraded`、`unresolved = [{ code: "ACP", dbnums: 34 个全列 }]`、`degraded_sections` 含 `mdb_closure`。
- 按需生成 `POST /model/ensure {"refno":"17496/8518","force":true}` → HTTP 500，消息尾部带路由目录给出的原因：

```text
14 geometry element(s) failed: [… FIXING 17496/156982: db 7001 没注册进这个 DbSet，resolver 也给不出它的文件 …]；
路由目录给出的原因：dbnum 7001：MDB /ALL 声明了 dbnum 7001（*MASTER/STLCATA，项目 ACP），搜索根里没有 acp000
库目录或其中没有这个库的文件；把装着它的目录（本地或 \\服务器\共享）加进 project_search_paths，详见 /health.mdb_closure
```

改前现场人看到的只有前半句（`db 7001 没注册进这个 DbSet`），从那句想不到是项目没配。

## 4. 用例③a：第二个搜索根

配置：`project_path = rootA` + `project_search_paths = ["D:/adr078-verify/rootB"]`。

```text
MDB /ALL：搜索根 2；闭包 5 个项目（AMS 主→AvevaMarineSample / ZDJ@根#1 / SCB@根#1 /
ACP→AvevaCatalogue@根#2 / APS@根#1），外部库 40/40 已定位；未定位项目 0
```

`/health.mdb_closure`：`status = ok`、ACP 的 `root_index = 1`、`project_dir = D:\adr078-verify\rootB\AvevaCatalogue`、`unresolved = []`；路由目录 71 条、声明了没文件 0。

## 5. 用例③b：掉线 UNC

配置：真根 + `project_search_paths = ["//10.255.255.1/PDM/Projects"]`（不可路由的地址）。

- **28 s 就绪**，没卡死（`read_dir` 在这条路径上是 `os error 53` 快速失败，不是 15 s 超时那条路；超时那条由单测 `an_unreachable_root_is_reported_and_does_not_block_the_others` 钉）。
- `roots[1].reachable = false`、`problem = 目录不可读（共享盘掉线 / 路径写错 / 无权限）: 找不到网络路径。 (os error 53)`。
- 横幅：`MDB /ALL：搜索根 2（1 不可达：\\10.255.255.1\PDM\Projects …）；闭包 5 个项目（… 均在根#1），外部库 40/40 已定位；未定位项目 0`。
- 其余 5 个项目照常，`degraded_sections` 含 `mdb_closure`（根不可达也算缺口，见 D6）。

## 6. 跑出来的缺陷（已修在本批里）

`project_locator::subdirs` 原先这样判子目录：

```rust
let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or_else(|_| path.is_dir());
```

Windows 的目录联接（junction）与目录符号链接在 `file_type()` 里报 **symlink 不报 dir**，于是整个 `rootA` 扫成空
（`codes_seen: []`），连主项目都定位不到——②的第一版结论是「主项目未定位，无从读取它的 SYS」而不是「缺 ACP」。
现在跟一步 `path.is_dir()`：

```rust
let is_dir = match entry.file_type() {
    Ok(kind) => kind.is_dir() || (kind.is_symlink() && path.is_dir()),
    Err(_) => path.is_dir(),
};
```

新钉 `a_project_folder_behind_a_junction_is_still_scanned`（用 `mklink /J` 造联接，建不了的机器上自动跳过）。

顺带确认主项目定位不到时**不是死循环**：「模型完整性凭证检查失败 dbnum=1112/7997」重试 26 次后停，日志 23 KB 封顶，服务照常应答 `/health`。

## 7. 回归

隔离 worktree（`cfed5fcce` + 本批 + 联接修复）：

- `cargo test --lib`：**2110 passed / 0 failed / 109 ignored**（9.36 s）。
- `cargo check --all-targets`：绿（51 s）。

## 8. 台子的收尾状态

- 部署实例留在**用例①的配置**（真根、无 `included_projects`、`project_search_paths = []`）上跑着，9099 在听；末次 `ensure … force` 复核仍是 `Generated · 1,731 实例`。
- 想回 0.1.25 的旧二进制：先把 `DbOption.toml.bak-adr078-*` 还回去（旧二进制的 `aios_core::DbOption` 里 `included_projects` 是必填字段，没有 `serde(default)`，读删掉这一行的新配置会炸），再把 `aios-database.exe.bak-0.1.25` / `web\setup.html.bak-0.1.25` 换回来。
- `D:\adr078-verify\`：`rootA` / `rootB` 两组目录联接 + `run-case.ps1`（几 KB，没有数据副本），真盘没动过。
