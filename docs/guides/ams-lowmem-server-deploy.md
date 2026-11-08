# AMS 低配 Windows 服务器部署手册

> 沉淀自 2026-11-08 腾讯云实测（Windows Server 2012 R2 / 4 vCPU / 7.9 GB RAM，
> 详见文末「实测记录」）。目标：一台没有 AVEVA E3D、内存只有个位数 GB 的
> Windows 服务器上，发布包解压即跑通「服务 + SurrealDB + 模型生成 + RVM 校验」。

## 结论先行

- **8 GB 物理内存 / 4 vCPU 实测可跑全链路**，包括 ams7333 这种单根 ~9 GB 的压力库——
  前提是 **pagefile ≥ 16 GB**（commit 上限要到 ~24 GB），且生成侧保持串行。
- 三条硬参数：`rvm-check --jobs 1`、`--min-commit-free-gb 1.5`、DbOption.toml 里的
  内存闸（`model_generation_input_max_dbnums` 等，见 §4）。
- 服务器**不需要安装 AVEVA E3D**：基线 RVM 随包携带，走 `--baseline-dir` 模式。
- 新版发布包（含 #115 修复）**自带 VC++ 运行时 DLL**，旧系统不再需要手工装 Redist。

## 1. 实测环境基线

| 项 | 实测值 | 说明 |
|---|---|---|
| OS | Windows Server 2012 R2 Datacenter | 最低兼容目标；VC++ 旧版坑见 §7 |
| CPU | AMD EPYC 7K83，4 vCPU | 生成是单进程串行，核数影响小 |
| RAM | 7.9 GB | 物理内存不是硬约束，pagefile 才是 |
| Pagefile | **16 GB 固定** | 由 1 GB 调上来；commit 上限 ~23.9 GB |
| 磁盘 | C: 剩余 ~136 GB | E3D 数据 ~8 GB + 库数据 + 产物 |
| 后端 | aios-database 0.1.44 | `rvm-check` 子命令可用 |
| SurrealDB | 2.1.4，RocksDB 介质 | `bin\surreal.exe` 随包 |

**pagefile 是唯一不能省的配置**。ams7333 单根生成峰值 ~9 GB，超过物理内存，
全靠 pagefile 扛；1 GB pagefile 时 commit 上限 ~9 GB 必撞墙。

## 2. 前置依赖清单

| 依赖 | 说明 |
|---|---|
| 发布包 | `scripts/New-LocalRelease.ps1` 产物：`aios-database.exe` + `web\` + `schema\` + `bin\surreal.exe` + `resource\surreal\` + `DbOption.toml` + `Start-AMS.ps1/.cmd` + `Start-Surreal.ps1` + `Rollback.ps1`。0.1.44 起包根与 `tools\` 各带 9 个 VC++ 运行时 DLL（#115） |
| E3D 项目数据 | `E3D2.1\` 目录（含 `AvevaMarineSample`、`AvevaCatalogue` 等项目子目录）。只要数据文件，不装 E3D 本体 |
| 基线 RVM | `baselines\ams<dbnum>_<w0>_<w1>.rvm`，每根一个。没有 E3D 的机器只能用 `--baseline-dir` 模式 |
| VC++ 2015-2022 Redist | 新包已随包 DLL（exe 同目录优先于 System32）；旧包/手工部署需装 redist 或把 DLL 放 exe 同目录，**不要动 System32** |

## 3. 部署步骤

以下以 `C:\ams-deploy` 为例；`_deploy/ams-server/` 下的 `Setup-AmsDeploy.ps1`、
`Invoke-AmsDeployTest.ps1` 是这次实测用的参考实现。

```powershell
# 1. 解包（管理员 PowerShell）
#    backend 包 → C:\ams-deploy\backend\，E3D2.1 数据 → C:\ams-deploy\E3D2.1\
7z x backend.zip -oC:\ams-deploy\backend
Expand-Archive E3D2.1.zip -DestinationPath C:\ams-deploy   # 得到 E3D2.1\

# 2. 改 DbOption.toml 的 project_path 指向真实数据目录（跑一次即可）
cd C:\ams-deploy\backend
.\Setup-AmsDeploy.ps1            # 自动找相邻的 E3D2.1；也可 -ProjectRoot 显式指定

# 3. 起服务（Start-AMS.ps1 会先拉起包内 surreal.exe 再起 aios-database serve）
.\Start-AMS.ps1
# 健康检查
curl http://127.0.0.1:9099/api/v1/health    # → status=ok
```

`Setup-AmsDeploy.ps1` 只做一件事：把 `DbOption.toml` 的 `project_path` 改写成
`E3D2.1` 的实际路径（POSIX 分隔符）。其余配置按发布包出厂模板即可。

## 4. 低配机器的 DbOption.toml 要点

云主机内存可能远小于开发机，以下几项是实测配置里刻意压低的（完整文件见
`_deploy/ams-server/backend/DbOption.toml`）：

```toml
gen_model_batch_size = 4                    # 生成批大小，16 并发曾撞 rocksdb 写冲突
model_generation_input_max_dbnums = 4       # GenerationInput 缓存上限（每库 ~0.7–0.95 GB）
model_root_inflight_max = 4                 # 同时在飞的根数上限
manual_db_nums = [1112]                     # serve 启动时只做这几个库的基线生成
watch_dbnums = [1112]
server_release_ip = "127.0.0.1:9099"        # 公网机器只绑回环，要外网先看 §6
http_api_addr = "127.0.0.1:9099"
```

`Start-AMS.ps1` 已处理的运行时细节：`RUST_MIN_STACK=134217728`（缺省补上）、
清掉临时房间成员环境变量、rocksdb 模式下先起 `bin\surreal.exe`（数据落
`workspace\.surreal\ams`）。服务本体**不拉起** SurrealDB（ADR-074 D2）。

## 5. RVM 校验（无 E3D 基线模式）

服务器上没有 `des.exe`，`--stage export` 不可用；用随包基线直接「生成 + 比对」：

```powershell
cd C:\ams-deploy\backend\workspace    # 与服务一样按工作目录读 DbOption.toml
$env:DB_OPTION_FILE = 'C:\ams-deploy\backend\DbOption.toml'
.\..\aios-database.exe rvm-check `
    --dbnum 1112,8000,7999,7326,7327,7333 `
    --baseline-dir C:\ams-deploy\backend\baselines `
    --out output\rvm-check `<
    --jobs 1 --min-commit-free-gb 1.5 --force
```

| 参数 | 低配机器上的取值 | 原因 |
|---|---|---|
| `--jobs` | **1（必须）** | ams7333 单根 ~9 GB，任何并行都会顶穿 commit 限额 |
| `--min-commit-free-gb` | 1.5（缺省即此） | 余量低于阈值杀掉该根生成子进程，记 `commit-guard`，不连坐其他根 |
| `--in-process` | **不要用** | 生成在服务进程里跑，没有内存隔离，一个根爆内存全家遭殃 |
| `--baseline-dir` | 指向 `baselines\` | 无 E3D 时唯一可行模式；文件名 `<tag>_<w0>_<w1>.rvm` |

产物：`<out>/summary.txt`（人读）+ `summary.json`（机器读）+ 每根
`<tag>/<w0>_<w1>/run.json`、`compare.json`。退出码 0 = 全部 `pass`/`empty`。
用法细节见 `docs/guides/rvm-check-batch.md`。

## 6. 网络与安全基线

实测中踩过的坑，按此收口：

- **安全组来源收窄**：3389（RDP）与 5985（WinRM）规则来源写 `/32` 客户端 IP，
  不要 `0.0.0.0/0`；排查完立即改回。
- **临时传输通道用完即关**：本次用过自建 HTTP 上传（8000 端口）传 ~1 GB 包，
  传完已停服务并建议从安全组删除该规则。大文件优先考虑压缩后走 WinRM/SMB。
- **服务只绑回环**：`127.0.0.1:9099`。要从外网看页面，先确认安全组放行，再改
  `server_release_ip` / `http_api_addr` 为 `0.0.0.0:9099` 并重启。
- **WinRM**：`Enable-PSRemoting -Force`；本机 `WSMan\TrustedHosts` 需管理员，
  脚本化调用可用 Python `pywinrm` 绕过。
- **长命令走脚本文件**：WinRM 直接传长 PowerShell 命令会撞 `filename or extension
  is too long`，把脚本写到远端再执行。

## 7. 故障排查（全部实测出现过）

| 现象 | 根因 | 解法 |
|---|---|---|
| exe 直接退，退出码 `-1073741515`（0xC0000135） | 缺 `vcruntime140_1.dll` 等 VC++ 运行时 | 把 9 个 DLL 放 exe 同目录（新包已随包）；看事件查看器确认缺哪个 |
| 生成子进程 `-1073741819`（0xC0000005），事件日志 `MSVCP140.dll` | 系统 `C:\Windows\System32\MSVCP140.dll` 版本过旧（实测 14.0.24215.1 ≈2021 版） | 同上：exe 同目录放新版 DLL，搜索顺序先于 System32，**不用改系统目录** |
| `commit-guard` / 内存分配失败 | pagefile 太小，commit 上限不够 9 GB 峰值 | pagefile 调到 ≥16 GB（`wmic pagefileset` 或系统属性） |
| serve 起来就退，日志 `os error 3` | 工作目录缺 `resource\surreal\*.surql` | 从包里 `resource\surreal` 拷到 `workspace\resource\surreal`（Start-AMS 已处理） |
| rvm-check 某根 `gen-failed` 但无崩溃 | E3D 项目码写错（应为短码如 `AMS` 而非目录名） | `--e3d-project` 用 1–8 位字母数字短码 |

## 8. 验证清单（部署完成判定）

按序执行，全部通过才算部署正常：

1. **预检**：记录 RAM / commit 限额 / pagefile / 磁盘（`Invoke-AmsDeployTest.ps1`
   的 preflight 段，或手工 `Get-CimInstance Win32_OperatingSystem`）。
2. **版本**：`aios-database.exe version`、`bin\surreal.exe version` 都能跑。
3. **服务**：`Start-AMS.ps1` 后 `GET /api/v1/health` 返回 `status=ok`，进程稳定存活。
4. **生成**：`manual_db_nums` 里的库完成基线生成，日志见 `ingest_and_generate`。
5. **RVM**：`rvm-check --baseline-dir` 跑完，`summary.txt` 全 `pass`/`empty`
   （`fail`/`kind_differ` 是模型差异不是部署问题，见 #116；`commit-guard` 是
   内存配置问题，回 §5/§7）。
6. **内存画像**：记录 `memory.csv` 峰值与各根 `gen_peak_private_mb`，写进部署记录。

## 9. 实测记录（2026-11-08，82.156.155.65）

- **部署**：backend.7z（~124 MB）+ E3D2.1.7z（~0.78 GB）传输、SHA-256 校验、解压、
  配置改写、服务启动全部一次通过；期间修掉 §7 的两个 VC++ 问题。
- **服务**：`/api/v1/health` ok，空闲私有内存 ~407 MB；空闲 commit 0.9/23.9 GB。
- **RVM 全量**（14 根，`--jobs 1 --min-commit-free-gb 1.5`，生成耗时 284 s）：
  **10 pass / 1 empty / 3 fail**。3 个 fail 为模型内容差异（only_b/off 级几十个
  构件），与部署无关，见 issue #116。
- **内存峰值**（子进程 private）：ams7333 **8951 / 8908 MB**（两次）、ams7326/7327
  ~1.4–1.6 GB、ams7999 ~1.2 GB、ams1112 ~0.5 GB、ams8000 ~0.2 GB。
  ams7333 超物理内存靠 16 GB pagefile 完成，未触发 commit-guard。

## 参考

- 发布脚本：`scripts/New-LocalRelease.ps1`
- RVM 批量用法：`docs/guides/rvm-check-batch.md`
- 参考实现：`_deploy/ams-server/backend/Setup-AmsDeploy.ps1`、
  `_deploy/ams-server/backend/Invoke-AmsDeployTest.ps1`（预检 + 采样 + 一键验证）
- 相关 issue：#115（VC++ 随包）、#116（RVM 差异）、本手册对应 #117
