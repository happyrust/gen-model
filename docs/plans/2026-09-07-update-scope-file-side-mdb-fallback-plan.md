# 方案：`UpdateScope` 库侧 MDB 表为空时回落到 SYS 文件声明

> 日期：2026-09-07
> 状态：**已实施并验收，见 §9。** §1–§7 是实施前写下的方案，一字未改地留着，好让「当时怎么想的」
> 与「实际怎么样」能对照——尤其 §5 那两个风险，验收把它们答掉了。
> 起因：YCYK-E3D 首次初始化实测（运行目录 `plant-1/runycyk1-bootstrap/`，binary `a0a076fc…`，
> namespace `ycyk_init927_20260907_133017`）。新项目第一趟只解析 SYS meta、设计库一个都不跑，
> 必须再跑第二趟才拿得到真范围。
> 涉及：`src/data_interface/update_scope.rs`（改）、`src/data_interface/mdb_membership.rs`（只读，可能加一个 accessor）、
> `src/lib.rs::run_cli`（只读，确认 `init_mdb` 的先后）、`src/data_interface/manual_update.rs`（只读，`update_scope(Option<&str>)` 这条入口）

## 0. 一句话

同一个「这个 MDB 声明了哪些 DESI」的问题，进程里有两份答案：`mdb_membership` 从 SYS **文件**读，
`UpdateScope` 从 **SurrealDB** 的 `MDB` 表查。启动时前者先答、答对了，后者随后再答一次、在新命名空间上
答的是空。入队用的是后者，于是新项目必须跑两趟。本方案只做一件事：**库侧那张表为空时，改用同进程内
已经装好的文件侧名单**——不是新增一个数据源，是让已经在内存里的那一份被用上。

## 1. 现状：两份名单

| | 文件侧 | 库侧 |
|---|---|---|
| 代码 | `mdb_membership::resolve` → `install` | `update_scope::UpdateScope::fetch` |
| 读什么 | 项目目录里文件名以 `sys` 结尾的库（`acwsys` / `amssys`），e3d-io 按最新会话钉成 `DbSet`，按 `NAME` 找到 MDB 元素读它的 `CURD` | `RETURN array::distinct(SELECT VALUE NAME FROM MDB);` + `MDB_DBNOS`（`MDB.CURD.refno` 里 `STYP IN [DESI, ISOD]` 的 `DBNO`） |
| 何时可用 | `run_cli` 里 `init_mdb` 一跑完就有，**不依赖任何同步** | 要 SYS meta 库先被摄入 `pe` / `MDB` 表 |
| 谁在用 | 字典库名单（UDA catalog）、`mdb_memory_store` 骨架、`direct_tree`、`e3d_model_service` | **入队范围**（`admits` / `declared_desi`），也就是「这一轮跑哪些设计库」 |

两者的成员判据是**同一个**：文件侧 `MdbMembership::element_databases()` 过滤
`styp ∈ {DESI_STYP=1, ISOD_STYP=6}`，库侧那条 SurrealQL 绑的 `element_db_types` 也正是
`u8::from(DBType::DESI)` 与 `mdb_membership::ISOD_STYP`。所以这不是两种口径，是同一口径的两个读法。

`mdb_membership.rs` 的模块头本来就把这件事写明白了：

> `UpdateScope` answers the same question for `STYP = DESI` through SurrealDB, which needs the SYS
> database parsed and synced first. This reads the file, so it is available during initialization —
> before anything has been synced.

分工是有意的。本方案不推翻这个分工，只补上「库侧还没有内容时，别装作什么都不知道」。

## 2. 现场：这两行差 82 毫秒

YCYK-E3D 第一趟启动（全新 namespace，`mdb_name = "SLYK"` 启动前就写在 `DbOption.toml` 里）：

```
13:30:26.451  MDB /SLYK / YCYK-E3D / module=DESI：声明 49 个库（按 STYP {1:10, 2:24, 4:4, 7:5, 8:5, 14:1}），其中字典库 5 个：盘上 5 个，缺 0 个
13:30:26.488  MDB /SLYK / YCYK-E3D：DESI 骨架 10 个库 / 65455 个元素，装载 36 ms
13:30:26.533  [init] 库里还没有任何 MDB——存放 MDB/CURD 的 SYS meta 库尚未解析。本期只解析 SYS meta 把 /SLYK 的成员名单建起来，设计库一个都不跑；完成后再跑一次即可拿到真正的执行范围
13:30:26.533  [init] 1 个库不在 MDB /SLYK 的声明名单里，本轮不入队（本期声明 0 个 DESI）：PADD:927
```

同一个进程，同一个 `/SLYK`。第 1 行是文件侧答的，第 3 行是库侧答的。**第 3 行那句「尚未解析」在事实上
只对库成立**——它要的名单，第 1 行已经拿到了，而且顺带用它扫出了 10 个 DESI 库、65455 个元素的骨架。

`update_scope.rs` 给退化写的理由是防死结：

> 一上来就 bail 的话，新项目、或者刚换过 SurrealDB 命名空间的项目会卡成死结：想更新得先有范围，
> 想有范围得先更新。

理由本身没错，但它假定「范围只能从库里来」。`init_mdb`（`db_model.rs:392`，按它自己的注释此前是个只有
`Ok(())` 的空壳）落地之后这个前提不再成立：死结的另一头已经有出口了。

## 3. 改什么

### D1 回落只发生在**库侧完全为空**那一档

`interpret` 今天有三个出口，按 `desi` 与 `known` 分：

| `desi` | `known` | 今天 | 本方案 |
|---|---|---|---|
| 非空 | — | 正常范围 | **不变** |
| 空 | 空（库里一条 MDB 都没有） | bootstrap 退化 + 告警 | **改：先问文件侧** |
| 空 | 非空、且不含本名 | `bail!`（配置错误，名字打错） | **不变** |
| 空 | 非空、含本名 | 告警「这个 MDB 没声明 DESI」 | **不变** |

只动第二行。理由是这一档**不存在两份名单打架的可能**——库侧什么都没有，谈不上谁更新。
第四行（库里有这个 MDB、但它 CURD 里没有 DESI）刻意不动：那时库侧是有内容的，拿文件侧去覆盖它
就变成了「两个来源不一致时听文件的」，那是另一个决定，见 §4。

### D2 回落取的是**已经 install 的那一份**，不现读文件

用 `mdb_membership::get(project, mdb)`，不用 `resolve`。`get` 的文档明说它「deliberately not a lazy
resolve: reading a SYS database is not something that should happen behind an innocent-looking getter」
——这条纪律要守住：`UpdateScope::fetch` 会被看门狗事件路径每隔几秒调一次，在它背后藏一次 SYS 库全树扫
是不能接受的。

拿不到（`get` 回 `None`，即 `init_mdb` 没跑过或失败了）就**保持今天的行为**：照旧 bootstrap 退化 + 告警。
这一条不是兜底摆设——`run_cli` 里 `init_mdb` 失败只 `log::warn!` 不中止（`lib.rs:743`，且有源码钉
`startup_resolves_the_mdb_before_the_manager_is_shared_and_survives_failure` 守着这个宽容），
所以「membership 不在」是真会发生的状态。

### D3 `project` 从哪儿来，以及 MDB 名必须对得上

`mdb_membership::get(project, mdb)` 是按 **(project, mdb) 二元组**取的，而 `UpdateScope::fetch` 今天只拿到
一个 `name`。两件事要处理：

1. **project**：取 `aios_core::get_db_option().project_name`。`init_mdb` 装进去时用的就是它
   （`lib.rs:745` 传的是 `&db_option.project_name`），两边同源。
2. **MDB 名**：`manual_update::update_scope(mdb: Option<&str>)` 允许请求带一个**跟配置不同**的 MDB 名。
   `get` 按名字精确匹配，名字不同就回 `None` → 落回今天的行为，这正是想要的。
   **绝不能**退化成「拿配置那个 MDB 的名单去服务另一个 MDB 的请求」——那条注释里写得很清楚，
   正是这次改动要消灭的东西的镜像：「界面显示一个 MDB 的范围、服务端跑另一个，还不出声」。
   这一条要有单测钉死。

### D4 告警换一句，别静音

今天那句「本期只解析 SYS meta……完成后再跑一次」在回落成功时就不再成立了，必须换成另一句，
说清三件事：名单这一轮取自 SYS 文件而不是库、共几个 DESI、库侧那张表还是空的（下一次 SYS meta 落库后
会自动切回库侧）。**不许无声回落**——`warning()` 的调用方会把它放进各自的 warnings，
这是「这一轮为什么跑了这些库」的唯一出处。

### D5 缓存照旧

回落出来的 `UpdateScope` 与库侧解出来的形状完全相同，`cache_store` 照常写。SYS meta 批次落库时
`batch_worker` 会调 `invalidate_scope_cache()`，下一次解析自然切回库侧，不需要为回落单开失效路径。

### 改动形状

`fetch` 目前是「只做那一趟 SurrealQL」的纯 IO 函数，`interpret` 是纯函数、单测覆盖得很密。
回落**不要塞进 `interpret`**（它现在不碰任何全局状态，塞进去就要给每个测试造 membership registry）。
建议落在 `finish` 里 `Ok` 分支的中间层，或者给 `interpret` 多一个「文件侧名单」入参、由 `finish` 现取：

```
finish(name, fetched, stale_fallback)
  Ok((known, dbnos)) if dbnos.is_empty() && known.is_empty()
      → 取 mdb_membership::get(project, &name)
        Some(m) → 用 m.element_databases() 的 dbnum 组范围，换 D4 那句 warning，cache_store
        None    → 今天的 bootstrap 退化
  其余分支逐字不变
```

后者更好：`interpret` 仍是纯函数，多出来的那个入参在测试里就是一个 `Option<Vec<u32>>`。

## 4. 不改什么

- **不改分工。** 库侧仍是常态来源，文件侧只在库侧为空那一档顶上。「两份名单都非空且不一致时听谁的」
  **本方案不回答**，也不引入任何会踩到这个问题的路径。要回答它得先说清楚：库侧反映的是「上一次摄入的
  SYS」，文件侧反映的是「此刻盘上的 SYS」，两者不同时到底哪个才是「本期执行范围」。那是另一个议题。
- **不动 `admits` 的 SYS meta 绕过。** `COLD_START_DB_TYPES` 恒放行照旧——名单的来源就在那些库里。
- **不动配置错误那条 `bail`。** 名字打错仍要报错并列出库里实际有的 MDB。
- **不动缓存的三条纪律**（暖缓存放行基础设施故障、冷缓存 fail-closed、配置错误清缓存）。
- **不碰 `manual_db_nums` / `watch_dbnums`。** 它们是另外两道门，与本方案正交。

## 5. 风险

**R1（主风险）：一趟到位之后，Meta 与 Design 会在同一次扫描里一起入队，这个顺序没被验证过。**
今天新项目的第一趟天然只有 SYS meta，Design 永远排在下一次进程里；改完之后两相同时出现在同一个
`initialization_phase` 纪元里，靠的是阶段屏障（Meta → Design）而不是「上一趟已经跑完了」。
屏障机制是现成的（YCYK 第三趟的 health 里 `phases` 同时有 `meta: {total: 8}` 与 `design: {total: 10}`
且都收敛了），但**那一趟的 SYS meta 是上一趟就落库的**，不构成对新顺序的证据。这一条必须实测，
不能推断。

**R2：DESI 基线是否真的不依赖库里的 SYS meta。** 按 ADR-053 / ADR-058，数据面 direct 读文件，
UDA catalog 走 `mdb_memory_store::load` → `element_attributes::load_uda_catalog(membership, &schema)`
——也是文件侧。所以**看起来**不依赖。但「看起来」不够，R1 的实测会顺带答掉它：如果依赖，
第一趟的 DESI 基线会带着 UDA 缺值落库，那比多跑一趟糟得多。验收里要专门对一次 UDA。

**R3：首次开机的时长变了。** 今天第一趟是秒级（只有 SYS meta），改完之后第一趟就是完整初始化。
这不是缺陷，但部署预期要跟着改——尤其 `startup_autorun = true` 的现场。

**R4：`get` 拿到的可能是**上一个 MDB 的残留**。`install` 按 (project, mdb) 覆盖、不清空 registry，
进程内换 MDB 名时旧条目还在。D3 的精确匹配挡得住「拿 A 的名单服务 B」，但值得在单测里钉一遍。

## 6. 验收

按顺序，前一条不过就不做后一条。

1. **纯函数单测**（不连库）：库侧空 + 文件侧有名单 → 范围等于文件侧、`warning()` 是 D4 那句新的；
   库侧空 + 文件侧 `None` → 与今天逐字相同（`declared_desi()` 为 0、`admits("SYST", …)` 为真、
   `admits("DESI", …)` 为假、`warning()` 非空）；库侧非空 → 三条现有出口逐字不变
   （`an_unparsed_project_bootstraps_instead_of_deadlocking` 等现有用例全绿）。
2. **名字不匹配不串台**：registry 里装的是 `/SLYK`，请求 `/IDW` → 回落不生效，走今天的 bootstrap。
3. **实测 R1**：全新 namespace + 全新 store，`watch_dbnums` 给全部 10 个 DESI，**只跑一趟**。
   判据是这一趟同时满足：`initialization.data_ready = true`、10 个 `applied_sesno == file_latest_sesno`、
   `pe` 行数与两趟版一致（本次基线：71896 行，其中 5525=37603 / 7015=12042 / 5107=9496 / 6015=6144）、
   `gen_root` 行数一致（2438）。数字对不上就是 R1 或 R2 命中了。
4. **对 UDA**：第 3 步落库后，挑一个带 UDA 的元素，与两趟版同一元素的 `ATT_UDA` 逐字节比。
5. **AMS 不回归**：`plant-1/run7997.py` 那条链在已有 store 上重跑一次，范围与今天相同
   （库侧非空，本方案的分支根本进不去，这一步是确认没误伤）。

## 7. 回退

单路径改动，`git revert` 一笔即可；没有数据格式变化、没有新配置键、没有持久化状态。
回退之后新项目回到「跑两趟」，与今天逐字相同。

## 8. 与本方案无关但同一次实测发现的

- YCYK-E3D 有六个 MDB（`/ALL` `/IDW` `/SJY` `/651YK` `/SLYK` `/HZYK`），`DbOption-zsy.toml` 里写死的
  `SLYK` 只是其中之一，交付要用的是哪个尚未确认。
- `/SLYK` 的 10 个 DESI 里只有 4 个有内容（5525 / 7015 / 5107 / 6015），SITE01~04 是空壳。
- 4 个 BRAN 生成根因目录件缺失卡在 `generation_hold`（`db 7004 会话 3 的索引里没有 15196/40431` 一族，
  另有 `db 212 会话 5 的索引里没有 16596/35338`），且这 4 个把两个初始化发布 run 拖住不收口，
  8 个库的 `model_sesno` 因此一直是 null。**这一条与本方案无关，已单独立项：
  `issues/ISSUE-029-initialization-run-has-no-terminal-state-for-a-deterministically-failing-root.md`**
  ——目录件是真缺（`bin/refno_locator_probe` 查过整条会话链），而 run 被它们焊住是 gen-model 自己的活性缺陷。

## 9. 实施与验收（2026-09-07 下午）

改动落在 `src/data_interface/update_scope.rs` 一个文件：`interpret` 末尾加 `from_files: impl FnOnce()`
入参（**`FnOnce` 不是取好的值**——库侧非空的三个出口不该为它付任何代价）、`finish` 的 `Ok` 分支传
`|| file_side_declared(&name)`、新增 `file_side_declared`、四个新单测。别的文件一行未动，
`mdb_membership` 现有的 `get` 与 `element_databases()` 直接够用，没加 accessor。

§6 的五步逐条：

**1–2 纯函数单测（过）**：`cargo test --release --lib update_scope` → **13 passed / 0 failed / 2 ignored**。
新增四个：库侧空 + 文件侧有 → 用文件侧；文件侧 `None` → 与今天逐字相同；文件侧空表 → 同上（空表不算回答）；
`a_non_empty_store_never_consults_the_file_side` 把「只动一档」钉死——它给的 `from_files` 是个 `panic!`，
库侧非空的三个出口只要问一次文件侧就红。九个既有用例一个没改。

**3 一趟到位（过，R1 答掉）**：全新 store + 全新 namespace，`watch_dbnums` 给全部 10 个 DESI，**只跑一趟**
（运行目录 `plant-1/runycyk2-onepass/`，端口 18095/18096，binary `cff3bbf4…`）。第一次扫描就把
8 个 SYS meta 与 10 个 DESI 一起排进队列，日志上是这一句：

```
14:00:44.853 [init] 库侧 MDB 表还是空的（SYS meta 尚未摄入），本轮执行范围改取自 SYS 库文件里 /SLYK 的声明：10 个设计库。SYS meta 一落库即自动切回库侧名单
```

阶段屏障成立：`phases` 收在 `design {total:10, pending:0, running:0, failed:0, blocked:0}` /
`meta {total:8, 同上}`。**这一句只出现了一次**——7 秒后的 `[scope-refresh]` 轮已经从库侧解出同一份名单
（10 个 dbnum 全判「已覆盖」），D5 的「自动切回」是实测过的，不是推断。

**3 的数字判据（过）**：与两趟版（`runycyk1-bootstrap`）逐项对齐，`plant-1/compare_runs.py`：

| | 两趟 | 一趟 |
|---|---|---|
| `pe` / 按 dbnum 分布 | 71896 | 一致 |
| `pe_owner` 边 | 83506 | 一致 |
| 依赖入库的 `pe`（`dependency = true`） | 5168 | 一致 |
| `gen_root` / 按 `publication_status` | 2438 | 一致 |
| `inst_relate` / `aabb` / `trans` / `inst_info` / `geo_relate` / `tubi_relate` / `inst_geo` | — | 全部一致 |
| 10 个 `applied_sesno` / `model_sesno` / `file_latest_sesno` | — | 全部一致 |
| `cata_dependency_state` | — | 一致（`updated_at` 等墙钟字段剔除后） |

**4 属性载荷（过，R2 答掉）**：这个项目 `ATT_UDA` 两边都是 0 行——**它压根没有 UDA**，所以「对一次 UDA」
在这个语料上是空判据，不能算数。改对**按 noun 的属性行整行**（ADR-064：元素行落 `pe` / `<NOUN>` / `ATT_UDA`，
没有 UDA 时属性就全在 `<NOUN>` 表里）：`BRAN`(1363) / `ELBO`(2228) / `SCTN`(1201) / `PIPE`(637) /
`EQUI`(122) / `ZONE`(101) / `SITE`(19) / `GENSEC`(11) / `TUBI`(0)，共 5682 行**整行取 sha256 比对，全等**。
比计数强的地方在这儿：计数对得上而每个值都错的情况，整行比对抓得住。

**5 不回归（过）**：`cargo test --release --lib -- --test-threads 1 --skip damaged_existing_content_addressed_file_is_rejected`
→ **1748 passed / 0 failed / 115 ignored**。没跑活 AMS 一轮：库侧非空那条路本方案根本进不去，
第 1 步那个 `panic!` 闭包已经在纯函数层把它钉死，而重跑 7997 要动用户正在跑的服务。

### 顺带撞上、与本方案无关的两件事

- `fast_model::e3d_mesh_store::tests::damaged_existing_content_addressed_file_is_rejected`
  **硬崩测试二进制**（`STATUS_ACCESS_VIOLATION` / `0xc0000005`）。**单独跑那一个模块也崩**，
  所以与本方案无关（`update_scope` 根本不在那条路上）。是新回归还是本机 mesh 目录的状态所致，没追。
  它是上面那条 `--skip` 的原因。
- `data_interface::project_paths` 那 12 个用例**只在并行下红**（`--test-threads 2/4` 全红、
  单独跑 16/16 绿、`--test-threads 1` 全绿）。典型的进程级共享状态互踩，同样与本方案无关。
