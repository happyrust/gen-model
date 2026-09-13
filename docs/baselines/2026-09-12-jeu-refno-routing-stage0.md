# JEU RefNo 原生路由阶段 0 基线

- 日期：2026-09-12
- 数据文件：`2026-09-12-jeu-refno-routing-stage0.json`
- 用途：只保存改路由前的观测事实；不把缺失数据补成估算值。

## 已冻结的现场事实

2026-09-11 的 JEU `/ALL` 启动摘要确认：

- CURD 原始行 479 条，成功解码 479 个数据库，跳过 0 个；
- 其中 DESI/ISOD 数据库 417 个；
- 旧 `MdbMemoryStore` 启动骨架读取 1,221,858 个元素头；
- 该段约耗时 68 秒；
- 运行形态为 `embedded-mem + read-through`，`room_membership=false`。

这些数字描述旧实现的成本，不是阶段 1 之后的性能目标。JSON 中保留
`database_opens`、`scan_calls`、`index_point_lookups` 等字段，便于下一次在同一份
JEU 数据上用 `e3d_io::read_stats` 补齐后直接做前后差值。

## 当前没有证据的值

以下三项在交接摘要中不存在，本实现环境也没有挂载 JEU 数据，因此保持 `null`：

- 进程 RSS；
- 首个属性请求耗时；
- 首个生成根请求耗时。

原始现场日志路径同样没有随交接进入仓库。不得把约 40 MiB 的理论骨架估算写成
实测 RSS，也不得用 AMS 或合成夹具的请求耗时冒充 JEU。

**2026-09-12 追记：JEU 现场不可达。** JEU 是另一台机器上的项目，开发机访问不到；这三项
`null` 只能由现场按下面的补采协议回填，不阻塞阶段 1 及之后。本地调试与验收项目改为 AMS
（E3D 3.1 `AvevaMarineSample` / MDB `ALL`），AMS 自己的阶段 0 基线另起一份
`2026-09-12-ams-refno-routing-stage0.json`（同 schema；待采），不往这份 JEU 文件里混。

## 位解码 corpus 门（2026-09-12 本机实测，阶段 0 退出标准）

母计划阶段 0 的退出标准是「每个已读元素的 `refno.dbno()` 等于文件描述符 dbnum，零分歧」。
门在 `vendor/e3d-io/tests/refno_dbno_corpus.rs::every_installed_corpus_key_decodes_to_its_descriptor_dbno`，
语料根由环境变量 `E3D_CORPUS_ROOTS`（`;` 分隔）指定，未设置则明确 skip；本机以
`D:\AVEVA\Projects\E3D2.1;D:\AVEVA\Projects\E3D3.1` 运行（release，38 s）：

| 语料根 | 主库文件 | 元素（Header 扫描） | `dbno() != db_mark` | 最大 dbno |
|---|---|---|---|---|
| `D:\AVEVA\Projects\E3D2.1` | 883 | 16,154,107 | 0 | 251,453 |
| `D:\AVEVA\Projects\E3D3.1` | 1,064 | 22,736,906 | 0 | 251,712 |
| 合计 | 1,947 | 38,891,013 | **0** | — |

覆盖面：两版安装下全部项目（AvevaMarineSample / AvevaPlantSample / AvevaCatalogue / SCB / TEST /
ZDJ / YCYK-E3D 等）的全部 DESI / CATA / DICT / PROP / PADD / ISOD 及 SYS / COMM / MISC / GLB 主库；
多 extent 库（`ams8000_0001` 之类）每个 extent 的 word0 都解回同一 dbnum；大编号（251xxx）走高位段。
非 ignore 的三文件门（`ams8000_0001` ×2 + 仓内 `ams1112_0001`）375,392 元素同样零分歧。

注意：这条门在 2026-09-12 之前从未在本机跑通——旧实现按字节 `split_at(3)` 切文件名，
安装目录里的中文文件名（`CAESAR弹性模量表.xls` 一类）让它在对账任何库之前就 panic。
上面的数字是修掉该崩溃之后的结果，与修复前用仓外复刻逻辑跑出的数字一致。

## WORL 位置（3C spike，同日同语料）

同一 1,947 个库：每库恰好一个 `WORL`（owner 为 0）；全部 1,905 个非 SYS/COMM/MISC 库的 `WORL`
都在 `RefNo(descriptor.primary_extent_ref, 0)`，即 `(dbno | 0x2000) / 0`，0 例外；例外只有
COMM(8190) `16382/1`、MISC(8189) `16381/1`、SYST(8191) `24575/3`。每库另有 1 个无主 `MNUM`
系统元素（noun_hash 803020）。这决定了树首页不需要按 noun 扫库找 WORL；细节见执行切片
`docs/plans/2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §10.2。

## 补采协议

在同一份 JEU 金基线和同一台机器上补采时：

1. 记录二进制 build id、`DbOption.toml`、项目/MDB、store mode、data face 与
   `room_membership`。
2. 进程启动前取一次 RSS；HTTP ready、骨架完成、首属性完成、首根完成后分别取 RSS
   与单调时钟耗时。
3. 每个阶段前后调用 `e3d_io::read_stats::snapshot()`，用 `delta_from` 记录
   `database_opens / scan_calls / scanned_elements / index_point_lookups`。
4. 首属性与首根必须记录请求 RefNo、所属 dbnum、HTTP 状态和响应摘要；失败也保留耗时
   与原始错误。
5. 将准确值回填 JSON；`elapsed_ms_approx` 仍保留为旧日志摘要，不覆盖成新构建结果。

## 阶段 0 判定

这份文件只完成“已有现场事实落库”。阶段 0 的**退出标准**（位解码 corpus 零分歧）已于
2026-09-12 在本机全部 E3D 2.1/3.1 安装语料上通过（见上节），阶段 1 可以进入。三个 JEU
`null` 指标属于“改前成本记录”而非退出标准，留待现场按补采协议回填；本地的改前/改后
对比以 AMS 基线为准。
