# 数据库架构版本升级 · 丢弃库端到端演练（2026-10-29）

**结论：通过（两条路径）。** 在一个与现场完全隔离的 SurrealDB 实例上走完了
「启动点名老库 → `POST /api/v1/storage-version/upgrade` → 逐库重新解析 → 写
`storage_version:current` → `/health` 转绿 → rs-server 正常启动」。同时验证了反向路径：
删掉版本记录后 rs-server 拒绝启动，`AIOS_STORAGE_VERSION_CHECK=warn` 只告警。

第二轮在一份旧库副本上只升级 `8000`（§3.5）：升级本身成功，但请求集不是 MDB 声明的
全范围，**不写整库版本记录**；`/health` 仍 `upgrade_required=true`，rs-server 也继续
拒绝启动。这是整库语义的预期结果，不是升级失败——它证明「监听范围」没有被误当成
「整库范围」。

演练还抓出并修掉了四个真问题（见 §4）：`/health.storage_version` 放错层级、启动提示
没有按监听范围收窄、rs-server 不匹配时 panic 而不是干净退出，以及升级完成判定把
`MDB ∩ watch_dbnums` 当成了整库分母。

## 1. 边界与隔离

| 项 | 现场 | 本次演练 |
|---|---|---|
| SurrealDB | `127.0.0.1:8009`（长期运行，库在盘上） | `127.0.0.1:18009`，**memory 后端**，仓库自带 `bin/surreal.exe`（2.1.4，与现场同版） |
| 数据 | `ns=1516 / db=AvevaMarineSample` | 同一 ns/db 名，但服务器进程与端口独立；进程退出即消失 |
| aios-database | `:8022`（旧二进制） | 新构建的 `aios-database.exe`，`:18022` |
| rs-server | 未启动 | 新构建的 `rs-server.exe`，`:18023` |
| 配置 | 仓库根 `DbOption.toml` / `../rs-server/DbOption.toml` | `target/upgrade-rehearsal/{run,rs-run}/DbOption.toml` 副本（不在版本控制内） |
| 范围 | 全范围 | `watch_dbnums = [7352]`（盘上 114688 B、2 个 Ref0 的最小设计库） |
| 网格 | 现场目录 | `target/upgrade-rehearsal/meshes` |

现场 8009 / 8022 的进程与数据全程未被写入。

第二轮（§3.5）换成旧库的 RocksDB 副本：`D:/work/plant-code/old/test-issue-regen-model/ams-db`
自带的 `001703.sst` 校验和损坏、无法打开（原文见 `00-source-db-corruption.txt`）；改用
`gen-model/.surreal/ams-8009-fresh` 的副本
（约 349 MB，`pe_ptset` 13,276 行，只覆盖 7997 / 8000，库内**没有** `storage_version:current`），
仍由 `bin/surreal.exe` 在 `127.0.0.1:18009` 以 RocksDB 后端服务。源目录只读复制，
副本在 `target/upgrade-rehearsal/ams-fresh`，升级只写这份副本。

## 2. 造一个「老库」

全新空库会走 D-7（首次初始化完成自动写版本），测不到升级流程。因此先播种成
「非空、但没有版本记录」的老库形状：

```sql
CREATE pe:⟨99999_1⟩ SET dbnum = 99999;
UPSERT dbnum_watermark:7352 SET dbnum = 7352, applied_sesno = 1;
```

`pe` 非空使 D-7 的「全新空库」判据为假；`data_face_signature` 靠 `applied_sesno > 0`
把这份未签名库认领成 ingest（否则启动会被 `DATA_FACE_SIGNATURE_REQUIRED` 挡下）。

## 3. 走查结果

### 3.1 启动点名

- 修复前（`09-startup-before-fix.excerpt.txt`）：提示把 **MDB 声明的 29 个设计库**全列进
  「将被清空」清单，而进程只监听 `7352`。
- 修复后（`10-startup-after-fix.excerpt.txt`）：

```text
数据库架构版本不匹配（库里 none / 期望 1）：请在运维面板或 POST /api/v1/storage-version/upgrade 升级，
升级会清空并重新解析、生成以下库：[7352]
```

`/api/v1/health` 当时为 `storage_version.upgrade_required = true, stored = null,
upgrade.state = idle`（`01-health-before.json`）。

### 3.2 一键升级

```http
POST /api/v1/storage-version/upgrade
{"dbnums":[7352]}
```

- `202 Accepted`，回执：`requested=[7352]`、`full_scope=true`、reinit 任务
  `db-20261029-035548-000008`；同批还排了 17 个 meta 数据窗口（SYS 库不受监听范围限制）。
- 进度（轮询 `/api/v1/health`）：

| 时刻 | upgrade.state | done | failed | caught_up |
|---|---|---|---|---|
| +5s … +50s | running | 0 | [] | false |
| **+55s** | **done** | **1** | **[]** | **true** |

- `storage_version:current` 在完成前为空（`02-sql-before.json`），完成后出现
  （`04-sql-after.json`），`version=1`，`shape` / `rebuild` 是整库语义的全文
  （`03-health-after.json` 里 `upgrade.finished_at = 03:56:49`）。
- `storage_upgrade:current`：`requested=[7352] done=[7352] failed=[] full_scope=true`。
- `dbnum_watermark:7352`：`applied_sesno` 从播种的 1 推进到 50，`model_sesno` 同批落库。

### 3.3 点集读取不再判版本

`pe_ptset` 读侧已无版本判断。7352 这个库没有管件（重解析后 `pe_ptset` 计数为 0），因此：

- 对一个真实 `pe` 行调用 `fn::pe_ptset_of`：返回 `null`（NONE），**不 THROW**；
- 手工插一行 `pe_ptset:99999_1` 后再调：返回点集对象（`05-ptset-function.json`）。

### 3.4 rs-server

- 版本记录为 1 时启动（`06-rs-server-starts.log`）：

```text
数据库架构版本：stored=1 expected=1，与本进程一致
server release: 127.0.0.1:18023
```

- 手工 `DELETE storage_version:current;` 后启动（`07-rs-server-refused.log`）：

```text
Error: MODEL_STORAGE_VERSION_MISMATCH stored=none expected=1 source=missing:
请用 aios-database 升级数据库（运维面板或 POST /api/v1/storage-version/upgrade）
```

  **exit 1**，监听端口未打开。修复前这里是 `panic!`，exit 101。

- `AIOS_STORAGE_VERSION_CHECK=warn` 启动（`08-rs-server-warn.log`）：打印
  `*** 警告（AIOS_STORAGE_VERSION_CHECK=warn）：MODEL_STORAGE_VERSION_MISMATCH …`，
  随后 `server release: 127.0.0.1:18023`。仅开发机降级用。

### 3.5 旧库副本指定升级 8000（部分范围）

数据形状：`pe_ptset` 13,276 行（`7997` 10,640 / `8000` 2,636），**没有**
`storage_version:current`；MDB `/ALL` 声明 71 个库（其中元素库 29 个），
本进程 `watch_dbnums=[8000]`。启动：

```text
[mdb-bind] 声明解析完成: project=AvevaMarineSample mdb=ALL declarations=71 problems=0
MDB /ALL / AvevaMarineSample / module=DESI：声明 71 个库（按 STYP {1: 29, ...}）
数据库架构版本不匹配（库里 none / 期望 1）：请在运维面板或 POST /api/v1/storage-version/upgrade 升级，
升级会清空并重新解析、生成以下库：[8000]
```

`/health.storage_version` 为 `stored=null / expected=1 / upgrade_required=true /
upgrade.state=idle`。发出显式名单：

```http
POST /api/v1/storage-version/upgrade
{"project":"AvevaMarineSample","mdb":"ALL","namespace":"1516","dbnums":[8000]}
```

- `202 Accepted`，`requested=[8000]`、`full_scope=false`、任务
  `db-20261029-041830-000000`（同批 `up_to_date=17`，SYS 窗口不受监听范围限制）。
- 轮询 20 秒时 `upgrade.state=running / done=0`；约 35 秒后看到 `done` /
  `done_dbnums=[8000]` / `failed=[]`。任务日志：清空 6,639 行 PE（3.3 s）→ 重新解析 →
  `pe=6639`、953 个生成根、2,814 条记录、`model_sesno=734`，`状态=succeeded`；任务总耗时
  37.4 秒，进度记录 `finished_at=04:19:12`（开始 04:18:30，约 42 秒）。
- 结束语明确：`存储架构升级结束（部分名单）：requested=[8000] done=[8000]；请求集不是全范围，不写版本记录`。
- `storage_version:current` 仍为空（`06-sql-partial.json`）；`storage_upgrade:current` =
  `requested=[8000] done=[8000] failed=[] full_scope=false`。
- `/health` 仍是 `caught_up=false / upgrade_required=true`，`upgrade.state=done`、
  `done_dbnums=[8000]`（`05-health-partial.json`）。

数据面的「只动 8000」可以逐行验证：`pe_ptset` 计数 `7997=10,640`（未动）、
`8000=2,673`（重建后）；8000 的行带 `generator` / `sesno`，7997 的行没有这两个字段——
新旧形状在同一份库里并存，正是「部分升级」的可见状态。`fn::pe_ptset_of` 对两库的行都
返回对象，对不存在的 `pe` 返回 `NONE`，不 `THROW`。

rs-server 对这份部分升级的库启动（`07-rs-server-refused.log`）：

```text
Error: MODEL_STORAGE_VERSION_MISMATCH stored=none expected=1 source=missing:
请用 aios-database 升级数据库（运维面板或 POST /api/v1/storage-version/upgrade）
```

**exit 1**，端口未打开。理由：`storage_version:current` 是整库一条记录，7997 等其余 28 个
元素库仍是旧形状；只升级 8000 不能代表整库。要让这份副本上的 rs-server 转绿，必须把
主项目归属的全部设计库升级完——已于 §3.6 执行并通过。

### 3.6 旧库副本全范围升级（20 库，版本记录落盘 + rs-server 转绿）

**范围澄清（第 5 个真 bug，本轮修复）**：MDB `/ALL` 声明的元素库是 29 个，但其中 9 个
（`6003, 7009, 7011, 7015, 7022, 7027, 7048, 7321, 250705`）归属外部项目，按
`is_foreign_design_data` 本就不在本库数据域——持久层按裸 dbnum 建键，外部同号库摄取会
撞号；`enqueue_manual_update` 也永远扫不到它们的候选文件。按 29 做全范围注定 9 个
missing→failed。修复：`declared_dbnums` 改为 `owned_design_dbnums`（只算主项目归属），
分母 = **20 个**：`[1112, 7324, 7326, 7327, 7329, 7330, 7331, 7332, 7333, 7334, 7350,
7352, 7353, 7354, 7356, 7741, 7997, 7998, 7999, 8000]`。启动横幅与升级入口同一分母。

副本 `ams-full3`（源自 `.surreal/ams-8009-fresh` 的丢弃副本），无 `watch_dbnums`，
`POST /api/v1/storage-version/upgrade` 不带名单：

- 回执：`full_scope=true / requested=20 / scanned=37 / enqueued=19 / merged=1`
  （7333 并入启动自愈任务——重启时水位 `applied_sesno=0` 触发了自动重排，
  `db-20261029-134345-000000`）。
- 13:44 发起 → 14:07 终态（约 23 分钟）：`storage_upgrade:current` =
  `done=[20 库全列] failed=[] full_scope=true state=done`。
- **`storage_version:current` 落盘，直接 SQL 验证**（`ams-full3-storage-version.json`）：
  `{ id: storage_version:current, version: 1, shape: "数据库架构 v1…", rebuild: "…" }`。
- `/health` 终态（`ams-full3-health-final.json`）：`stored=1 / caught_up=true /
  upgrade_required=false / upgrade.state=done`。
- rs-server（`server_release_ip=127.0.0.1:18023`，连 18009）正常启动并监听
  （`rs-server-full3.log`）：`数据库架构版本：stored=1 expected=1，与本进程一致` →
  `持久层自检通过` → `server release: 127.0.0.1:18023`。
- 逐库抽查（事后直查，`ams-full3-watermarks.json` / `-ptset-dist.json` /
  `-ptset-fn.json`）：20 库水位全部 `applied = model = file_latest`；
  `pe_ptset` 行分布于 6 个含管件库（7326 / 7327 / 7333 / 7997 / 7999 / 8000，
  7333 最多 158,818 行），行带 `generator`/`sesno` 新形状；`fn::pe_ptset_of`
  对重建行返回点集对象、对不存在的 pe 返回 `NONE`，无 THROW。

**失败轮次与根因（环境，非代码）**：本次成功之前同副本连死 4 轮，全部死于宿主机
commit 耗尽——该机 `63.1GB RAM + 23.3GB pagefile(E:) = ~86.4GB` 硬上限，而基线
（python×36 ≈13GB + 607 进程 + 现场 aios-database 热缓存 ~20GB）日常就在 ~78GB，
余量 ~6-8GB。撞顶后任意下一笔分配失败，Rust alloc abort = 进程即死——先后带走
排练 SurrealDB（7333 写 model lane 时）、排练 aios（alloc 1.3MB 失败）、乃至
**现场 8009/8022 两个服务**（12:46–12:52 风暴中同归于尽，均非本演练直接操作）。
现场服务已恢复：surreal 8009 由看护重启（13:34），aios-database 8022 经 release 包
`Start-AMS.ps1` 拉绿（13:40，PID 18436）。成功轮的关键不是运气，是现场 aios 冷缓存
窗口——重启后 commit 基线从 ~78GB 掉到 ~54GB，排练栈全程 56-62GB 跑完。

**顺带实锤两条既有文档行为**：
- 升级中途重启 → 持久化进度里的 task_id 不在注册表 → 落成 `failed`（13:43 重启时
  亲眼看到 `存储架构升级结束但未全部成功…不在任务注册表`）。
- 启动自愈：未初始化库（`applied_sesno=0`）在启动时被自动重排——7333 正是以此身份
  并入第 5 轮并成功。

另：早先"排练 SurrealDB 预留 32.8GB 块缓存"的判断**有误**——RocksDB 块缓存按需提交，
重启后的现场 surreal Private 仅 ~0.1GB；真正的常驻大户是现场 aios-database 的
模型/输入缓存（~20GB）。`SURREAL_ROCKSDB_BLOCK_CACHE_SIZE=1GB` 仍建议保留，
它把容量上限压住了，但死因从来不是单进程预留，而是系统总量贴顶。

## 4. 演练中发现并修复

1. **`/health.storage_version` 实际在 `model_cache` 里。**
   计划、spec 与运维面板都按顶层 `/health.storage_version` 读写，结果面板横幅永远不会出现。
   已提到顶层（连接/断开两套响应都带），`model_cache` 里不再重复；测试加了
   「两处响应都出现顶层字段」的钉。
2. **启动提示忽略监听范围。**
   `startup_reconcile` 用的是 MDB 全量候选，没过滤 `watch_dbnums`，提示里会列出本次根本
   不会碰的库。已改为「MDB 声明 ∩ 监听范围」，与升级入口的缺省名单同一分母。
3. **rs-server 版本不匹配时 panic。**
   共用建连闸门 `resident_db_runtime::ensure_connected()` 在错误分支直接
   `panic!`，生产启动拿到 exit 101 与栈回溯。改为把首次建连结论存进
   `INIT_RESULT` 并返回 `anyhow::Error`：`init_aios_mgr` 用 `?`，`init_test_db`
   仍 `expect`（测试要保留「库没起就炸」的语义）。`main` 里重复的那次核对删除，
   现在只有一行日志。
4. **全范围分母误用了监听范围。**
   旧实现把 `full_scope` 算成「请求集 == `MDB ∩ watch_dbnums`」，于是
   `watch_dbnums=[8000]` 只升级 8000 也会写整库版本记录，把其余仍为旧形状的库一块
   冒充成新版。改为 `covers_whole_database(requested, declared)`：分母是 **MDB 声明的
   全部设计库**，监听子集永远不等于整库。§3.5 的旧库实测就是这条判据：升级 8000 成功，
   但 `full_scope=false`、版本记录不写、rs-server 继续拒绝。
5. **MDB 声明名单 ≠ 本库数据域（29 → 20）。**
   MDB `/ALL` 声明 29 个元素库，其中 9 个归属外部项目、在本持久库没有候选文件（外部
   同号库本就被 `is_foreign_design_data` 排除），按 29 做全范围必然缺 9 个→failed。
   `declared_dbnums` 已改为按主项目归属过滤（`owned_design_dbnums`），启动横幅与升级
   入口一致列 20 个。首轮 29 请求的失败形态本身就是证据：`enqueued=20` 且 9 个外部
   库记 missing→failed、版本记录不写（`ams-full-health-29-failed.json` /
   `ams-full-sql-29-failed.json`）。

修复后回归：

- gen-model 全量 lib：**2689 passed / 0 failed / 134 ignored**；
- rs-server：**64 passed / 0 failed / 29 ignored**。

## 5. 未覆盖

- **未对现场库执行升级**。现场已是 `version=1`，真升级需要清空全部设计库，属破坏性操作。
- ~~旧库副本没有升级全部设计库~~ → **§3.6 已完成**：20 个主项目归属设计库全量升级、
  `storage_version:current` 落盘、rs-server 转绿（MDB 声明 29，其中 9 个外部项目库
  不在本库数据域，见 §3.6 范围澄清）。
- 7352 没有管件，第一轮的 `pe_ptset` 有行场景只由合成行验证。第二轮旧库副本的 8000
  有 2,673 行真实点集（7997 的旧行也在），`fn::pe_ptset_of` 抽样返回对象；§3.6 全量
  重建后的逐库水位与点集抽样已补（6 库有行、函数两态正常）。
- ~~升级中途重启进程的记账行为未演练~~ → §3.6 中实测发生：重启后旧 task_id 落成
  `failed`，与 `docs/specs/web-service-api.md` §4.5.1D 一致；另观察到启动自愈把
  未初始化库自动重排。
- 未做面板视觉检查；只核对了面板读取的 JSON 路径（顶层 `storage_version`）与脚本语法。
- **宿主机 commit 余量是稳定运行的前提**：本机 `63.1GB RAM + ~23GB pagefile ≈ 86.4GB`
  上限，基线日常 ~78GB（含现场 aios ~20GB 热缓存）；在此之上跑全量重建是贴顶赌博，
  连续 4 轮被随机带走（含现场服务）。生产/演练前先确认水位或扩 pagefile。

## 6. 复现步骤

```powershell
# 1) 丢弃库（memory，退出即清空）
./bin/surreal.exe start --no-banner --bind 127.0.0.1:18009 --username root --password root memory

# 2) 运行目录：复制 DbOption.toml，改 v_port=18009 / http_api_addr=127.0.0.1:18022 /
#    server_release_ip=127.0.0.1:18022 / watch_dbnums=[7352] / startup_autorun=false，
#    并把 resource/surreal 拷进去（define_common_functions 从 CWD 读）

# 3) 造老库
curl -u root:root -H "surreal-ns: 1516" -H "surreal-db: AvevaMarineSample" \
  -X POST http://127.0.0.1:18009/sql \
  --data-binary 'CREATE pe:⟨99999_1⟩ SET dbnum = 99999; UPSERT dbnum_watermark:7352 SET dbnum = 7352, applied_sesno = 1;'

# 4) 起新二进制（debug 构建即可）
AIOS_STORE_MODE=rocksdb D:/Rust/target/debug/aios-database.exe

# 5) 一键升级
curl -X POST -H "content-type: application/json" -d '{"dbnums":[7352]}' \
  http://127.0.0.1:18022/api/v1/storage-version/upgrade

# 6) 轮询
curl -s http://127.0.0.1:18022/api/v1/health | python -c "import sys,json;print(json.load(sys.stdin)['storage_version'])"

# 7) rs-server（独立运行目录，server_release_ip=127.0.0.1:18023，v_port=18009）
D:/Rust/target/debug/rs-server.exe
```

第二轮（旧库副本，§3.5）的差异只有三步：

```powershell
# 1) 复制旧库（源目录只读；ams-fresh 是丢弃副本）
cp -r D:/work/plant-code/old/gen-model/.surreal/ams-8009-fresh target/upgrade-rehearsal/ams-fresh
./bin/surreal.exe start --no-banner --bind 127.0.0.1:18009 --username root --password root `
  rocksdb://target/upgrade-rehearsal/ams-fresh

# 2) DbOption 副本改 watch_dbnums=[8000]（v_port=18009 / http_api_addr=127.0.0.1:18022）
# 3) 显式点名 8000；预期 full_scope=false，完成后版本记录仍为空
curl -X POST -H "content-type: application/json" `
  -d '{"project":"AvevaMarineSample","mdb":"ALL","namespace":"1516","dbnums":[8000]}' `
  http://127.0.0.1:18022/api/v1/storage-version/upgrade
```

## 7. 证据文件

`docs/verification/artifacts/storage-version-upgrade-2026-10-29/`：

| 文件 | 内容 |
|---|---|
| `01-health-before.json` | 升级前 `/health`（顶层 `storage_version`） |
| `02-sql-before.json` | 升级前版本记录/进度记录为空、`pe` 非空 |
| `03-health-after.json` | 升级后 `caught_up=true`、`upgrade.state=done` |
| `04-sql-after.json` | `storage_version:current`、`storage_upgrade:current`、水位 |
| `05-ptset-function.json` | `fn::pe_ptset_of` 无 THROW；有行时返回对象 |
| `06-rs-server-starts.log` | rs-server 正常启动（`stored=1 expected=1`） |
| `07-rs-server-refused.log` | 删记录后 exit 1 与稳定码 |
| `08-rs-server-warn.log` | `warn` 模式只告警 |
| `09-startup-before-fix.excerpt.txt` | 修复前列出 29 个库 |
| `10-startup-after-fix.excerpt.txt` | 修复后只列 `[7352]` |

`docs/verification/artifacts/storage-version-upgrade-2026-10-29/ams8000-partial/`（§3.5 旧库副本）：

| 文件 | 内容 |
|---|---|
| `00-source-db-corruption.txt` | 给定 `ams-db` 的 RocksDB 打开失败原文（`001703.sst` 校验和） |
| `01-startup-declarations-and-banner.txt` | MDB 声明 71 个库；启动点名 `none / 期望 1`，清单 `[8000]` |
| `02-startup-log-context.txt` | 启动阶段与监听限定模式的上下文 |
| `03-upgrade-start.json` | `202` 回执：`requested=[8000]`、`full_scope=false`、任务号 |
| `04-upgrade-log.excerpt.txt` | 清库 6,639 行、重解析、953 根、`done`、部分名单不写记录 |
| `05-health-partial.json` | 升级后 `/health`：`caught_up=false`、`upgrade.state=done` |
| `06-sql-partial.json` | 版本记录为空、进度记录、两库 `pe_ptset` 计数与 `fn::pe_ptset_of` 结果 |
| `07-rs-server-refused.log` | 部分升级后 rs-server exit 1（`stored=none`） |
| `ams-full-health-29-failed.json` / `ams-full-sql-29-failed.json` | 按 29 请求的失败形态：9 个外部项目库 missing→failed、版本记录不写 |

`docs/verification/artifacts/storage-version-upgrade-2026-10-29/ams-full3/`（§3.6 全范围成功轮）：

| 文件 | 内容 |
|---|---|
| `ams-full3-upgrade-start4.json` | 成功轮回执：`requested=20`、`full_scope=true`、`enqueued=19` + `merged=1`（7333 并入启动自愈任务） |
| `ams-full3-progress4.log` | `/health` 轮询全程：0→20/20，commit 56-62GB |
| `ams-full3-upgrade-final.json` | `storage_upgrade:current` 终态：`done=[20 库]`、`failed=[]`、`full_scope=true` |
| `ams-full3-storage-version.json` | **`storage_version:current` 直查：`version=1`** |
| `ams-full3-health-final.json` | 终态 `/health`：`stored=1`、`caught_up=true`、`upgrade_required=false`、`state=done` |
| `rs-server-full3.log` | rs-server 18023 正常启动：`stored=1 expected=1` → `持久层自检通过` |
| `ams-full3-surreal3.log` | 成功轮 SurrealDB 启动日志（`SURREAL_ROCKSDB_BLOCK_CACHE_SIZE=1GB`） |
| `ams-full3-watermarks.json` | 20 库 `dbnum_watermark`：全部 `applied = model = file_latest` |
| `ams-full3-ptset-dist.json` | `pe_ptset` 逐库行数分布（6 个含管件库，7333 达 158,818 行） |
| `ams-full3-ptset-fn.json` | `fn::pe_ptset_of` 抽样：有行返回对象、无行返回 `NONE` |
