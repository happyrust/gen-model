# P 点分表（`pe_ptset`）落地计划 · 2026-10-26

用户 2026-10-26 拍板：**用分表做**（不放 `inst_info`，不放 `inst_geo`）。需求是 **SurrealQL 查询时实时算长度**（`fn::bran_comp_len` / `fn::elbo_len` / `fn::tee_len`，`resource/surreal/gy_common.surql`），
它们要的是：每个管件按 `pe` 键 O(1) 取到 ARRI / LEAV / P3 的局部坐标，**与模型同代、每件都有**。现状与问题见 `2026-10-26-ptset-storage-audit.md`（审核 A–G）；
本计划把那份审核落成改动清单，并把「算的那半」（e3d-model）一并规划。

线上现状（2026-10-26 23:4x，`0.1.44+g83aa3c250088`）：`/health.model_cache` 里**没有** `ptset_backfill` 这块——补写功能还没部署，今天线上长度函数一律算 0。

## 0. 一句话

**表不变（`pe_ptset`，与 `pe` 同键），改「什么时候写、写什么」：** P 点由 e3d-model 在生成时随 `GeneratedElement` 交出（和几何同一个 `DbSet` / 求值环境），
gen-model 在发布路径里**同步**写 `pe_ptset`，行带 `sesno` 与生成器版本；异步补写只留给老库搬运。查询函数一字不改。

## 1. 为什么是分表而不是 `inst_info`（记录决策依据，免得以后再问）

| | `inst_geo` | `inst_info` | `inst_relate`（实例行） | `pe_ptset`（分表） |
|---|---|---|---|---|
| 身份 | 内容寻址网格，全项目共用，随「清理模型」连带删（`mesh_purge.rs:1-16`） | 旧管线按 `cata_hash` 共享的形体信息，`INSERT IGNORE` 首写者赢、从不更新（`pdms_inst.rs:50`、`:1282`）；**e3d-model 烘焙件没有这行**（实例行直接带 `insts_flat` / `booled_id`，`model_record_reader.rs:268-270`） | 每个设计实例一行，发布块里写，「每个实例都要读」的热路径（`staging/mod.rs:87/99`） | 每件一行，与 `pe` 同键，只在算长度时按主键取 |
| 放 P 点的后果 | 生命周期不归它管、共享网格互踩、身份错配（审核 §8） | 烘焙件要先新造每件一行的 inst_info（与分表同形、多一跳）；旧 CATA 件共享行首写者赢，P 点吃设计属性（PBOR / 连接）或算法升级时不刷新（gen-model #73 同形）；读路两跳；清理是引用计数式（7997 BEND 24381/100848 留过两条同源关系） | 可行，但 +0.2–0.7 KB/件进热行，且要改「与 legacy 逐字节同形」纪律（`ele_ops.rs:950`）与 ADR-071 的块字节 | 失败域隔开、热行不膨胀、`DELETE … WHERE root` 一条语句清、读侧主键直取（审核 §7） |

`inst_info` 唯一的好处是 `out.ptset.d.pt AS pts` 那行投影不用改——换表也就是改一行，不值上面三条代价。

## 2. 目标与完成定义

1. 任意一个已发布的 BRAN / HANG 根，其直属管件在 `pe_ptset` 里**每件一行**（没有目录链的件也有行，`ptset: {}`、`count: 0`、`reason` 写明）；行的 `sesno` = 该根这次发布的 `source_sesno`。
2. 长度函数对 ams8000 `/M-OR-1RX-3081D`、`/C-OR-1RX-3040A`（已有 TTY 管身长度真值）的 BRAN 算出的值与 E3D 一致（≤ 0.01 mm）。
3. `model/rebuild` 或增量重生成一根后，`pe_ptset` 行在**同一次**发布里更新；根删除时行随之删。没有「有行但过期」的状态（审核 A）。
4. P 点有 E3D 真值门（下面 W1）；L2 回放末步断言 `pe_ptset` 覆盖度（审核 B）；`gy_common.surql` 有 `DEFINE TABLE` / `DEFINE INDEX`（审核 C）。
5. 线上：部署后对 20 库跑一轮 `model/rebuild`（或按 `sesno` 的补扫），`/health` 账 `rows_written` 与 `pe` 里 BRAN / HANG 直属成员数对上。

## 3. 行形状（在现状上只加不改）

```
pe_ptset:⟨word0_word1⟩ = {
  pe, root, dbnum, noun,
  ptset: { "<P 号>": { pt: [x,y,z], dir: [x,y,z], bore, noun } },   // 管件局部系 mm，现状不变
  count, unresolved: [ { number, reason } ],                      // unresolved 从「只有 P 号」改成带原因（e3d-model 本来就给：`UnresolvedCataloguePoint.reason`）
  sesno,                                                          // 新：该根这次发布的 source_sesno（不是 model_sesno，见审核 §9）
  generator,                                                      // 新：P 点求值器版本（e3d-model 的 PTSET_EVAL_VERSION 常量），算法升级时补扫能认出旧行
  reason,                                                         // 新：空点集的缘由："no catalogue chain" / "no PTRE" / "all points unresolved"；有点时 NONE
  written_at
}
```

读侧含义从此清楚：**没有行 = 这根还没发布过**；**有行但 `ptset: {}` = 这件真没有 P 点**（审核 §5 第 3 条「缺行与真没有不可区分」到此收口）。长度函数对 `{}` 照旧回 0，但 `reason` 让巡检分得清。

## 4. 工作包

### W1 · e3d-model：P 点随生成交出 + E3D 真值门（本线，**2026-10-27 已做**，见下「执行记录」）

- 原计划是 `GeneratedElement.point_set`；实做改成 **`GenerationOutcome::point_sets: Vec<MemberPointSet { refno, noun, set }>`**（没有 `owner`：那要沿 owner 链上溯，`ancestor_reads_guard` 要登记增量覆盖；落行用生成根，直属属主 `pe` 行上有），理由两条：
  ① 没出几何的件在 `GeneratedElement` 里没有位置，而恰恰是它们（没有 GMRE 的 ATTA）的 P1 / P2 定着两侧管身——「每件一行」靠元素字段做不到；
  ② gen-model 的测试里有 4 处字面构造 `GeneratedElement { .. }`，加字段就把 vendor 升级和测试改动绑在一起；`GenerationOutcome` 没人字面构造。
  求值在 `catalogue::model_catalogue_member_with_cache` 里顺手做（解到 SCOM 后 `catalogue_point::point_set_of_component`，同一个目录件、同一套环境），
  `CatalogueMemberBuild::point_set` 带着它走完几何的每条退路；`pipeline::build_catalogue_geometry`（路由成员 + 类 28 NOZZ）每件收一条，有没有几何都收；
  没有目录链 = `set.component == None`（不是缺条）；P 点集读坏只记 notes、不进 `failed`。`PTSET_EVAL_VERSION = 1`。
- `gen_ams` 多写 `point_sets.json`（`eval_version` + 逐成员，成员局部系），`elements.json` 不动（不让 5 万件的对拍文件胀 20 MB）。
- 增量路径（gen-model 逐件 `model_element`）没有这条输出，照旧用公开的 `e3d_model::catalogue_point_set(&member)`（同一句求值）。
- **真值门** `tests/ptset_real.rs` + `tests/fixtures/ptset/ams-ptset-truth-20261027.psv`：只读 TTY（`verification/civil-tty-20261021/ptset_truth.mac`，运行时 `shadow_e3d31_gen_model_test`），
  用 E3D 自己的设计级伪属性 `PPOS[n]` / `PDIR[n]` / `PPBORE[n]`（PML2 `!m.ppos[!n].wrt(!m)`；`Q PBOR n`、`NPPOI` 在 3.1 里不存在），坐标在成员自己的系。
  取样改成全在 AMS（一个 TTY 会话）：ams8000 `/M-OR-1RX-3081D`、`/C-OR-1RX-3040A`、带 VALV 的 `/Copy-of-1RCV0101-…`；ams7997 风管 `/-VES-E-4-L-2501`（136 件）、
  HANG `/1RCV013MN-SUPP-04/H1`、带 8 只 NOZZ 的 SUBE `/-RX-CCV-001RF`；ams7999 1WCC 阀门 BRAN、HANG `/R220.156/HANG-01`——215 件 1471 点。
  aps250132 阀门 / MDS FT48B 没取（APS 要另一个会话；MDS 支架件是 SCTN，没有 P 点），阀门的阿杆点由 ams7999 / ams8000 的 VALV 覆盖。
- 读取集合：没有新属性名（PTRE / NUMB / PX / PY / PZ / PTCD / DIR / PAXI / PDIS / PBOR 都在 LEAV / ARRI 路径上读过），只是每件多读几个点。

#### W1 执行记录（2026-10-27）

- 第一跑：1434 点里 1166 不对，位置 / 方向几乎全对，两件事：
  ① **通径**：E3D 的 `PPBORE[n]` 对 PBOR 未设（全零元组、`[0,1,0,0]`）/ 求出是 0 的点一律答 `Unset`（1471 点里 1149 个，没有一个 `0mm`），我们交 `Some(0.0)`。
  交出的点集改照 `PPBORE` 口径（`ppbore`：0 不是通径 → `None`）；路由建管的 `RoutePoint::bore` 不动（Core `LIMPOK` 比的是数）。→ `pe_ptset.ptset[n].bore` 从此是 `None` 或正数。
  ② **P0**：ZDJ `/NPHS-PIPIE-PTSE`（db 7600 `23984/100540`）给 ATTA / PCLA 定了一只 `PTCA 0`，PTCD 写 `Y`，E3D 的 `PDIR[0]` 照样答构件 +Z、`PPOS[0]` 答原点、
  `PPBORE[0]` 答 Unset（4 件）——P0 是构件自己的原点与 +Z，不看目录（`catalogue_point_local_checked_at` 开头）。
  ③ 库 7000 在 ams000 / acp000 各有一份主文件，`--project-dir` 扫描分不出正身，ams7999 1WCC 的 FLAN / TEE / OLET 整件 failed；门与 `output/s1_regress.py` 同样显式 `--catalogue ams7000_0001`。
- 第二跑：**202 件 1471 点 0 处不对**（位置 1e-3 mm / 方向 1e-6 / 通径 1e-6），覆盖面也钉住：真值里每个路由成员 / NOZZ（含 43 只没有 SPRE 的 ATTA / PCLA）都有一条、
  编号集合相同、没有一点落 `unresolved`；SUBE 下的设计基本体（CYLI / BOX / PYRA / CTOR）不在 `point_sets` 里，门按 noun 分类放过。
- TTY 细节（下次少踩坑）：`'=w0/w1'.dbref()` 字面量调方法报语法错，要先进变量；`<DBREF>.members()` 不存在，是 `.members` 属性；对未赋值变量调方法直接 0xC0000005；
  开着一个 ALPHA LOG 再开另一个也崩，先 `ALPHA LOG END`；ALPHA LOG 崩了不落盘，进度要用 FILE 对象 APPEND。

### W2 · gen-model：发布路径同步写（gen-model 线，要先定 D1）

- 在 `e3d_model_service.rs` 的 `project_geometry_delta`（`:4357`）/ `bake_element` 那一层，从 **`GenerationOutcome::point_sets`**（W1 实做的形状，每个路由成员 / NOZZ 一条，含没几何的）渲染 `pe_ptset` 的 `UPSERT`（复用 `ptset_backfill::render_root` 的行渲染，改成按成员渲染），和本根的模型行**同一个 `publication_transaction`**。增量重生成一件时用 `e3d_model::catalogue_point_set(&member)`。
  D1 二选一：(a) 进同一事务但**不进块字节**（发布块和回执照旧，ptset 语句追在同一事务尾部）；(b) 在 durable ACK 后的写后路径（`model_write_behind.rs:999` 那个点）用已有的 `point_set` 写，不再重开 `DbSet` 重算。
  推荐 (a)：同代由事务保证；`model_refresh.rs:876` 的断言禁的是「在发布里排队补写」，不是同事务写行；要补一条 ADR（ptset 的存放位置至今没有 ADR）说清「块字节不变、事务多了哪几条」。
  (b) 是最小改动（只换数据源 + 加 sesno），但仍是异步，有 ACK→写入之间的窗口。
- 删：`render_spooled_scope_deletion`（`model_db_adapter.rs:2499`）里加 `DELETE pe_ptset WHERE root = …`，和几何行同一条范围删除。
- 行多三格（`sesno` / `generator` / `reason`）；`unresolved` 带 reason。
- 异步补写（`ptset_backfill.rs`，尚未提交）**整个不要**（D5 2026-10-27 拍板）：发布路径是唯一写入口，老根靠逐库 rebuild 补齐；只留它的行渲染函数和 `/health` 账（`rows_published` / `rows_deleted`）。老库搬运（W4）是单独工具。
- `/health.model_cache.ptset_backfill` 账保留，多两格：`rows_published`（发布路径写的）、`rows_stale_found`（补扫按 sesno / generator 认出的过期行）。

### W3 · 表结构与查询（gen-model 线，只动 `resource/surreal/`）

- `gy_common.surql` 加 `DEFINE TABLE pe_ptset SCHEMALESS` + `DEFINE INDEX pe_ptset_root ON pe_ptset FIELDS root`（与其它表同一处出厂）；代码里 `ensure_index()` 退为老库兑底。
- `fn::pe_ptset_of` 不改。长度函数不改。可选加 `fn::pe_ptset_state($pe)` 返回 `missing | empty(reason) | ok(count, sesno)` 给巡检和前端用。
- 三处老投影 `out.ptset.d.pt AS pts`（`staging/mod.rs:87/99`、`model_record_reader.rs:268`）保持原样（「与 legacy 逐字节同形」纪律），在 `gy_common.surql` 和这三处注释写死「`pe_ptset` 是唯一权威，`pts` 槽位恒 NONE」（审核 F）。

#### W2 / W3 执行记录（2026-10-27，gen-model worktree `gen-model-ptset`，分支 `ptset-publication`，基底 origin/refactor 83aa3c250；提交 `c963c1985`（W3 surql）、`894bcec3b`（W2 代码 + 测试 + changelog + API 文档），未推、未并）

- D1 按 (a) 做：`pe_ptset` 语句进模型行的同一笔发布事务、不进块字节——清旧行在 `persist_group` 的 prelude（与整片删同一笔），新行跟在该根最后一块几何后面（与房间边同一个位置）；`persist_root_impl`、镜像 spool（`render_spooled_root` / `render_spooled_scope_deletion`）同一套渲染。ADR 尚未补（计划里说要补一条）。
- 数据源是 W1 的 `GenerationOutcome::point_sets`（每个路由成员 / 类 28 NOZZ 一条，含没几何的），不重开 `DbSet`、不重算；增量重生成单件（单元 delta 的 `Partial` 提交）不动已有的点集条目——计划里「增量重生成一件时用 `catalogue_point_set(&member)`」没做：单元路径不重算目录链，整根的行由下一次整根重生成刷新。
- 行形状按 §3，多的三格都在：`sesno`（这根本次发布的 `source_sesno`）、`generator`（`PTSET_EVAL_VERSION`）、`reason`（没点时说明；有点是 `NONE`）；`unresolved: [{ number, reason }]`。没点的件也一行（D2）。
- 存储语义（计划没写、实做时定的）：`ProjectionCommit.point_sets: Option<Vec<_>>`，只有 `WholeRoot` 且带 `Some` 的提交才按根整批替换（`Some(空)` = 成员全没了，落库清行）；`None`（只盖章的 `cached_root_commit`、盘上旧产物）与 `Partial` 不动。存储里**没条目**的根（从库里恢复的视图、没带点集的旧产物）落库 / 镜像都不碰它的 `pe_ptset` 行，所以恢复的视图重放到镜像不会把镜像上的行删光。
- D5 落实：分支上没有任何补写 / 补扫 / `AIOS_PTSET_BACKFILL` / `/health.model_cache.ptset_backfill`（计划里「账保留、多两格」那条随 D5 一起去掉；观测只有发布结算日志的 `ptset_rows=`）。主 worktree（refactor）里未提交的补写版已按用户 2026-10-27 拍板**直接删、不留档**：`src/fast_model/ptset_backfill.rs`、`scripts/ptset_backfill_check.py` 删除；`mod.rs` / `model_write_behind.rs` / `lib.rs` / `options.rs` / `web_service/handlers.rs` / `gy_common.surql` 恢复到 HEAD；changelog 的补写条目、`web-service-api.md` 4.11.1 的「库里的副本」注、三个 L2 回放用例与 `tests/increment_replay/main.rs` 里的两根补写日志针摘掉（其余未提交改动一字未动）。审核文档 `2026-10-26-ptset-storage-audit.md` 与 agent-a 日志保留作历史。
- W3 按计划：`define table if not exists` + `define index if not exists pe_ptset_root`，`fn::pe_ptset_of`，长度函数改读它（`elbo_len` 弦长 + 夹角求弧长，`tee_len` P1–P2 + P3 垂距）；`fn::pe_ptset_state` 没加；代码里没有 `ensure_index()`（计划里那句说的是补写版的函数，随 D5 没有了）。
- 验证：单测 7 个新增（含内存 SurrealDB 端到端：原文灌 `gy_common.surql`，合块路与逐根路各跑一遍，`fn::bran_comp_len` = 2000 / `bore` = 500 / ATTA 空行带 `reason` / 再发少一件旧行清掉 / 只盖章不动行）；`cargo test --lib` 2650 passed / 0 failed / 134 ignored；`cargo test --tests` 全部集成目标通过（唯一红的是 `src/bin/direct_model_persist.rs` 自 f40e8b3d5 起就过时的存量断言，与本包无关）。W5 的 L2 回放断言与 ams8000 长度真值对比尚未做。

### W4 · 老库搬运（gen-model 线）

- 一次性任务（命令行子命令或启动补扫的一条分支）：`inst_relate.out.ptset` / `out.ptset.d.pt` 还在的老行，归一成 §3 的形状写进 `pe_ptset`，`sesno` 写源行的、`generator` 写 `legacy`，补扫视为「可被重算覆盖」；解不出的进 `unresolved`（不猜）。不读 dabacon 文件（老库的文件常不在盘上，审核 G）。
- 线上 20 库都是新管线生成的，没有老行可搬；这一包只对外部老库有意义，可最后做。

### W5 · 验收面

- e3d-model：W1 的真值门 + 库内测试（`CataloguePointSet` 序列化形状）。
- gen-model：L2 回放三条链末步加断言「`pe` 里每个 BRAN / HANG 直属成员在 `pe_ptset` 有行且 `sesno` = 根的 `source_sesno`」（裸 SurrealQL 要走测试进程内的连接，`POST /api/v1/query` 是只读工具契约不接任意查询）；`log_must_not_contain` 的两针照留；`scripts/ptset_backfill_check.py` 加读 `rows_published` / `rows_stale_found`。
- 长度真值：对 ams8000 那两条 BRAN 的 `fn::bran_comp_len` 与 TTY 管身长度逐件比（管身长度 = 相邻两件 LEAV→ARRI 点距，已有真值）。

### W6 · 上线

1. e3d-model W1 先推（不影响线上行为：多一个字段没人读）。
2. gen-model W2 + W3 一起发版；部署后**20 库低峰串行 `POST /api/v1/dbnums/{d}/model/rebuild`**（D5 拍板：不做补扫，老根靠重建补齐；每库重建期间该库按 #74 不可见，所以逐库做）。行上的 `generator` 仍要写：巡检靠它分辨哪些库还没跑过新版 rebuild——这一条和 gen-model #73 是同一个教训。
3. 核：`/health` 账 `rows_published` 与 `pe` 里 BRAN / HANG 成员数对上；抽 ams8000 两条 BRAN 的 `bran_comp_len` 对 TTY。

## 5. 决策点

| # | 问题 | 推荐 |
|---|---|---|
| D1 | ptset 写在发布事务内（不进块字节）还是 ACK 后写后路径 | (a) 事务内；补 ADR |
| D2 | 没有目录链的管件要不要行 | 要（`ptset: {}` + `reason`），让「没行」只有一个含义 |
| D3 | 哪些元素落行 | 沿用 BRAN / HANG 直属成员；其它有目录链的（NOZZ、支架件）先不落，前端测量走 `/element/ptset` 直读 |
| D4 | `generator` 的值 | e3d-model `PTSET_EVAL_VERSION`（整数，求值规则变就 +1），不用 commit 哈希（每次换 vendor 都会把全库排一遍） |
| D5 | 异步补写是否保留 | **2026-10-27 用户拍板：不做补扫，老根靠逐库 `model/rebuild` 补齐**。发布路径写行是唯一写入口；部署后 20 库低峰串行 rebuild 一次（每库重建期间按 #74 该库不可见，所以逐库做）；以后 `PTSET_EVAL_VERSION` 加一也走同一条 SOP。代价是重算几何，换来少一套补扫代码和「行与模型同代」的无条件成立。ACK 路径的异步补写整个去掉；老库搬运（W4）仍是单独工具。 |
| D6 | `inst_relate` 的 `pts` 槽位 | 不动，注释写死恒 NONE |

## 6. 顺序与归属

W1（e3d-model，本线）→ W3（surql，小）→ W2（gen-model 发布路径，要 D1 + ADR）→ W5 → W6 → W4（老库，按需）。W2–W4 是 gen-model 线的；本线只做 W1，并把 `point_set` 的形状和 `PTSET_EVAL_VERSION` 定下来供 W2 用。

## 7. 风险

- 读取集合变化：W1 若引入新属性读，gen-model 的读取集合钉（`13b406bbe` 一类）要同步，否则 CI 测试红。
- 生成耗时：每件多解一次 PTSE（已在同一 SCOM 上，几十微秒量级）；7333 这种 3.2 万根的库要实测一次重建时长（4.6 min → ?）。
- 事务体积：一根多 0.2–0.7 KB × 成员数，大根（几百件的 BRAN）也就几百 KB，低于 `SQL_PACKET_BYTES` 门；超了按元素分包。
- E3D 真值门可能暴露 dir / bore 的差异：那是要修的，不是风险；但会把 W1 的时间拉长。
