# rs-server 两线计划：ptset 对齐 + SurrealDB 连接改造 · 2026-10-27

两条线合成一份计划、分阶段排序。依据是同日三份只读审核：rs-server 连接逻辑侦察（Q1–Q4）、rs-server ptset 消费点清单（RECON-SURQL-PTSET）、gen-model ptset 存储审核（子代理）。所有结论带 `file:line`；行号以 2026-10-27 的工作树为准（rs-server 分支 `new_standard`，gen-model 分支 `refactor`，vendored core `vendor/old-aios-core`）。

本文件只是计划，不改代码。标注后按条修订。

## 0. 一句话

**先把测试连接治好（线 B，只动 `rs-server/src/db_mode.rs`），再在 `pe_ptset` 落地之后把 rs-server 的读侧从 `inst_relate.out.ptset` 切到 `pe_ptset`（线 A，先切 aios_core 产出层、rs-server 消费点尽量不动）。** 线 B 是线 A 验收的前提：没有一条活着的测试连接，ptset 的任何断言都是空跑。

## 1. 现状与依据

### 1.1 线 B · 连接

- `SUL_DB` 是进程级单例 `Lazy<Surreal<Any>>`（`vendor/old-aios-core/src/rs_surreal/mod.rs:66`；`:67` `SECOND_SUL_DB`、`:68` `KV_DB`）。唯一正式连接入口 `init_surreal()`（`vendor/old-aios-core/src/lib.rs:252-281`）：`connect`（`:260-263`）→ `use_ns/use_db`（`:264-267`）→ `signin`（`:268-273`）→ `define_common_functions()`（`:275-279`）。
- SDK 对同一实例第二次 `connect` 必返 `Already connected`（fork `crates/sdk/src/api/mod.rs:238`、`:258`，Lead 实测）。SDK 原生 ws 客户端内置 `router_reconnect`（`crates/sdk/src/api/engine/remote/ws/native.rs:397-448`）：杀掉 surreal 再拉起，rs-server 不重启即可继续查询（Lead 实测）。⇒ **生产侧「断线自动重连」已达成，不需要改造**，缺的是文档。
- 测试闸门 `rs-server/src/db_mode.rs`：`init_test_db`（`:121-150`）→ `CONNECT_ONCE.get_or_init(connect_test_db)`（`:149`）；`CONNECT_ONCE: tokio::sync::OnceCell<()>`（`:153`）；`connect_test_db`（`:170-221`）对 `Already connected` 直接 `return`（`:177-179`），**跳过** `:201` 的探针；其余 panic（`:181`）。`init_aios_mgr`（`:237-264`）走同一闸门（`:251`），`main.rs:168` 启动也走它。
- 根因（已判定）：router 任务由 SDK 在 `connect` 内 `tokio::spawn`，绑定在**第一个进入闸门的 `#[tokio::test]` 的 current_thread runtime** 上；该用例结束、runtime drop，router 一起没了。后续用例拿到死连接，报 `sending into a closed channel` / `receiving from an empty and closed channel`（`db_mode.rs:128`、`src/datacenter/auto_test/extraction_fixes_7997_test.rs:25-26`、`src/main.rs:618`）。这是**快失败**，不是挂起。
- 「串行 61 passed」是空跑：非 ignore 的 7 个触库用例里 6 个是观察型（`dq_query.rs:434-439`、`gy.rs:2925-2930`、`gy.rs:3025-3026` 查询已注释、`query.rs:829-835`、`heat_dissipation.rs:945` 丢弃结果、`main.rs:613-630` 只打印），在死连接上照样通过。唯一严格用例是 `dq_fixing.rs:1369-1376`（`?`）。并发跑时正是它挂住（Lead 实测 88/89 过、它一个挂）；挂住机制尚未钉死，候选 C1（ws 断开后 router 重连、在途请求无人应答且客户端无超时）与 C2（同步锁跨 `.await` 的跨 runtime 死锁），判别法见 B0。
- 其它会再次 `connect` 的路径（闸门必须保留的理由）：`lib.rs:214`（`init_test_surreal`）、`lib.rs:338`（`init_demo_test_surreal`）、`vendor/old-aios-core/src/aios_db_mgr/aios_mgr.rs:26-42`（`init_surreal_with_signin`，被 `get_surreal_db` `:79-82` 每次调用都走一遍）、`aios_mgr.rs:60`（`init_from_db_option`）、`mod.rs:82`（`connect_surdb`）、`mod.rs:98-100`（`connect_kvdb`，连的也是 `SUL_DB` 而不是 `KV_DB`）。
- `db_mode.rs:122-139` 与 `:140-148` 两套注释互相矛盾（前者说不能只连一次，后者说回到只连一次），代码是只连一次。
- 依赖就绪：`once_cell` 已在 `rs-server/Cargo.toml:52`，`tokio` full 在 `:28`。Cargo.toml 不需要动。

### 1.2 线 A · ptset

- **gen-model 当前主树（`refactor`）默认发布路径不写 ptset**：Canonical `inst_info` 只有 `{id, dbnum, noun, model_sesno}`、`inst_relate` 无 `ptset`、Baked `inst_relate` 无 `out`（`gen-model/src/fast_model/model_db_adapter.rs:2716-2728`）；e3d-model 算出的 `point_sets` 在 `gen-model/src/fast_model/e3d_model_service.rs:3768-3774` 被丢掉。旧写路径 `EleGeosInfo::gen_sur_json`（`vendor/old-aios-core/src/geometry/mod.rs:117-122`，写 `inst_info.ptset`，键为十进制字符串、值为 `CateAxisParam`）在 `legacy_model` feature 门后（`gen-model/src/fast_model/mod.rs:5-6`），默认构建不走。
- **gen-model 的新形状已定、已实做、未并**：`pe_ptset:⟨word0_word1⟩`（与 `pe` 同键）= `{ pe, root, dbnum, noun, ptset: { "<P 号>": { pt:[x,y,z], dir:[x,y,z], bore, noun } }, count, unresolved:[{number, reason}], sesno, generator, reason, written_at }`，局部系 mm（`gen-model/docs/plans/2026-10-26-ptset-split-table-plan.md:33-43`）。W2/W3 在 worktree 分支 `ptset-publication`（提交 `c963c1985` surql、`894bcec3b` 代码+测试），**未推、未并**（同文件 `:96-104`）；W3 在那条分支上把 `fn::bran_comp_len` / `fn::elbo_len` / `fn::tee_len` 改读 `fn::pe_ptset_of`（`:103`）。决策 D3：只有 BRAN / HANG 直属成员落行，NOZZ / 支架件不落、前端走 HTTP 直读（`:129`）；D5：不做补扫，老根靠逐库 `model/rebuild`（`:131`）；上线顺序 W6（`:117-121`）。
- `bore` 语义变化：旧 `CateAxisParam.pbore: f32`、未设为 `0.0`（`vendor/old-aios-core/src/parsed_data.rs:82-92`、`:119`）；新 `bore: Option<f64>`，0 收成 `None`（`vendor/e3d-model/src/catalogue_point.rs:288-289`）。P0 是构件原点与 +Z，不看目录（计划文 `:71-72`）。
- **rs-server 读侧全部钉在旧形状上**：
  - `rs-server/resource/surreal/gy_common.surql:42-58` `fn::bran_comp_len`：`:49` `let $ptset = ($pe->inst_relate.out.ptset)[0];`，`:52-53` `$ptset[$l].pt` / `$ptset[$a].pt`；`:34-37` `fn::bran_len` 间接依赖；Rust 调用 `rs-server/src/datacenter/query.rs:1203`。
  - `ptset_map` 消费点（结构体 `InstPointMap { ptset_map: BTreeMap<String, CateAxisParam> }`，`vendor/old-aios-core/src/pdms_pluggin/heat_dissipation.rs:11-15`；产出查询 `in.id->inst_relate.out.ptset[0]?:{} as ptset_map`，`vendor/old-aios-core/src/rs_surreal/geom.rs:190`、`:215`、`:238`）：`rs-server/src/datacenter/dq_bran.rs:1245, 1250, 1258-1259`（arrive/leave）、`:1425, 1438`（点 3 / 点 4 取 `three_p_bore`）；`rs-server/src/datacenter/query.rs:919, 922`；`rs-server/src/handle_api/heat_dissipation.rs:408, 412-414, 456-457, 501-502, 539-540, 589, 592, 604, 608-614, 633, 636, 652, 655`；文档镜像 `rs-server/src/datacenter/document/dq_datacenter_api.md:766, 772, 780`。
  - 其它旧形状读点在 aios_core：`rs_surreal/point.rs:42-43`、`:90-91`；`rs_surreal/query.rs:1147`、`:1258`、`:1266-1281`；点数组投影 `rs_surreal/inst.rs:178, 191, 288, 302`、`rs_surreal/geom.rs:310`（`out.ptset.d.pt`，新库恒 NONE）。
- **同名函数双写风险（P0）**：rs-server 启动时 `init_surreal()` → `define_common_functions()`（`lib.rs:275`）→ 读 `resource/surreal` 目录逐文件执行（`vendor/old-aios-core/src/rs_surreal/function.rs:44-54`），会把 rs-server 自己那份 `gy_common.surql`（含 `:42` `remove function fn::bran_comp_len;` + 旧函数体）**重新定义进同一个 ns/db**；gen-model 则把它自己的 `gy_common.surql` 编进二进制启动灌入（`gen-model/src/data_interface/embedded_surql.rs:47-49`）。两份文件今天逐字相同（`:49-53` 同行同内容），`ptset-publication` 一并，谁后启动谁赢——rs-server 一重启就把新长度函数打回旧版。
- ADR-088 的 T-J-8 追记（`gen-model/docs/adr/ADR-088-….md:185-189`）描述的 `fast_model/ptset_rows.rs`、`RANGE_TABLES` 含 `pe_ptset` 在当前主树里不存在；以源码为准。

## 2. 目标与完成定义

1. `cargo test --bin rs-server`（**不带** `--test-threads=1`）稳定通过；运行日志里 0 行 `closed channel`；`--test-threads=1` 照旧通过。
2. `db_mode.rs` 只有一套解释：connect 一次、router 常驻、socket 断了 SDK 自己重连；`Already connected` 路径仍跑探针。
3. `pe_ptset` 并入 gen-model 并完成逐库 rebuild 后，rs-server 的 `fn::bran_comp_len`（经 `query.rs:1203`）与 `ptset_map` 消费点在 dbnum 7997 样本上返回非空、与 gen-model HTTP `/api/v1/element/ptset` 逐字段一致（`pt` 1e-3 mm、`dir` 1e-6、`bore` 1e-6）。
4. 两个仓库里不再有两份互相覆盖的 `gy_common.surql`（D-A2 定所有权）。
5. rs-server 至少有一条**严格断言**的 ptset 用例（可 `#[ignore]`，但不是观察型）。

## 3. 阶段与依赖

| 阶段 | 内容 | 依赖 | 归属 |
|---|---|---|---|
| P0 | B0 挂住机制判别；A0 契约冻结 + `gy_common.surql` 所有权拍板（D-A2） | 无 | rs-server 线 / Lead |
| P1 | B1 常驻 runtime、B2 闸门改形、B3 注释与文档、B4 并发验收 | P0 的 B0 结论只影响 B4 的解释，不阻塞 B1–B3 | rs-server 线 |
| P2 | A1 gen-model `ptset-publication` 并入、发版、20 库低峰串行 rebuild（gen-model 计划 W6） | gen-model 线 | gen-model 线（外部） |
| P3 | A2 aios_core 读路径切到 `pe_ptset`；A3 rs-server 消费点核对；A4 rs-server surql 副本处理 | P1（有活连接才能验）、P2（库里才有行）、A0 | rs-server 线 + vendored core |
| P4 | A5 严格测试；A6 上线核对与对拍 | P3 | rs-server 线 |

可并行：P1 与 P2 互不依赖；A2 的代码可在 P2 完成前先写（按已定形状），验收等 P2。

## 4. 工作包

### 线 B · 连接（全部落在 `rs-server/src/db_mode.rs`，vendored core 与 Cargo.toml 不动）

**B0 · 挂住机制判别（只跑、不改）**

- 并发跑一次 `RUST_LOG=surrealdb=trace cargo test --bin rs-server`，看挂住期间有无 reconnect 日志：有 → C1（ws 断开，router 重连，在途请求丢）；无 → 看 C2。
- 挂住后抓线程栈（Windows：`dotnet-dump` / `procdump` 任一），看挂住线程停在 channel `recv` 还是 `parking_lot` 锁。
- `--test-threads=2` 把 `dq_fixing::tests::test_query_dq_circular_plate` 与其余 6 个触库用例两两配对，找出触发者。
- 产出：一段写进 B3 文档的「并发挂住根因」；若是 C1，追加决策 D-B3。

**B1 · 常驻 runtime（`#[cfg(test)]`）**

- 新增 `static TEST_DB_RUNTIME: once_cell::sync::Lazy<tokio::runtime::Runtime>`，`Builder::new_multi_thread().worker_threads(1).thread_name("sul-db").enable_all()`。静态永不 drop；不能用 current_thread（没人 `block_on` 就没人轮询）。
- `connect_test_db`（`:170`）在 `cfg!(test)` 下改为：把「`init_surreal()` + `verify_external_rocksdb` 探针」**整个 future** 交给 `TEST_DB_RUNTIME.spawn(...)`，在测试自己的 runtime 里 `await` JoinHandle；`JoinError::is_panic` 时取 payload 重新 panic，保留 `:181-197` / `:209-219` 的诊断文案。非 test 构建保持原样（`main.rs:158` 的 `#[tokio::main]` runtime 与进程同寿）。
- 禁忌：绝不在测试 runtime 内对常驻 runtime `block_on`（会 panic「Cannot start a runtime from within a runtime」）；不能只把 `connect` 包进去——`tokio::spawn` 取的是轮询时的上下文，`use_ns/signin/define_common_functions/探针` 都要在常驻 runtime 内。

**B2 · 闸门改形**

- `CONNECT_ONCE` 保留为「进程内只 connect 一次」的唯一入口（tokio OnceCell 与 runtime 无关）。
- `:177-179` 的 `Already connected` 分支从 `return` 改成「视为已连，继续执行 `:201` 的探针」；探针失败照旧 panic。这是让「别处建的连接 / 死连接」被发现的唯一机会。
- `init_aios_mgr` 的 `OPTION`（`:241`）保留（只缓存 `DbOption`），`:245-251` 注释同步。

**B3 · 注释与文档**

- 删掉 `:122-139` 与 `:140-148` 两套矛盾解释，只留一段不变量：connect 一次；router 常驻在 `TEST_DB_RUNTIME`（测试）/ 主 runtime（生产）；socket 断开由 SDK `router_reconnect` 兜住（引用 fork `native.rs:397-448` 与 Lead 的 kill/restart 实测）。
- `extraction_fixes_7997_test.rs:17-28` 的「为什么一个用例三步」说明改成「历史原因」，B1 落地后可拆回三个用例（可选，不在本计划强制）。
- 列出会再次 `connect` 的路径清单（§1.1 最后一条），标注「禁止在测试里直接调用」。

**B4 · 验收**

- `cargo test --bin rs-server` 不带 `--test-threads=1`：全部通过，输出里 `rg "closed channel"` 0 命中。
- `cargo test --bin rs-server -- --test-threads=1`：照旧通过。
- 观察型用例的打印从「查询失败: …」变成真实结果（这是 B1 生效的直接证据）。
- 若 `test_query_dq_circular_plate` 并发仍挂：按 B0 结论处理，不算 B1 失败。

### 线 A · ptset 对齐

**A0 · 契约冻结与所有权（P0，要拍板）**

- 冻结读侧契约为 gen-model 计划 §3 的行形状；rs-server 侧按 pe 键直取：`select value ptset from only type::thing('pe_ptset', record::id($pe))`（即 `fn::pe_ptset_of`），**不再有 `[0]`、不再走 `inst_relate`**。
- D-A2：`gy_common.surql` 的所有权。推荐：gen-model 是唯一出厂方，rs-server 从 `resource/surreal/` 移除 `gy_common.surql`（或让 `define_common_functions_on` 跳过它），rs-server 只消费 `fn::bran_comp_len` 等函数。备选：两份逐字相同 + CI 校验，但仍有启动顺序竞态。
- D-A3：aios_core 的点类型。推荐：新增 `PtsetPoint { pt: Vec3, dir: Option<Vec3>, bore: Option<f32>, noun: String }` 与 `From<PtsetPoint> for CateAxisParam`（`bore.unwrap_or(0.0)` → `pbore`，保住「0.0 = 未设」），`InstPointMap.ptset_map` 类型不变 ⇒ rs-server 消费点代码零改动，只核语义。

**A1 · gen-model 落地（外部依赖，不在本计划内执行）**

- `ptset-publication` 并入 `refactor`、发版、20 库低峰串行 `POST /api/v1/dbnums/{d}/model/rebuild`（gen-model 计划 W6 `:117-121`）。
- rs-server 测试库（`127.0.0.1:8009`，ns `1516`，db `AvevaMarineSample`，dbnum 7997，见 `extraction_fixes_7997_test.rs:8-9`）也要跑一次 rebuild，否则 A5 没数据。
- 本线只跟踪：`pe_ptset` 表存在、7997 的 BRAN 直属成员每件一行、`generator` = `PTSET_EVAL_VERSION`。

**A2 · aios_core 读路径切换（vendored core，与 gen-model 共用同一份，改动要同步进 old-aios-core 仓并更新 rev；见 `rs-server/Cargo.toml:14-22`、`:93-106`）**

- `rs_surreal/geom.rs:190`、`:215`、`:238` 三处产出查询：`in.id->inst_relate.out.ptset[0]?:{} as ptset_map` → `fn::pe_ptset_of(in.id) ?: {} as ptset_map`（或直接 `type::thing('pe_ptset', record::id(in.id)).ptset`），反序列化成 `BTreeMap<String, PtsetPoint>` 再转 `CateAxisParam`。`:271`、`:276` 的 `arrive.to_string()` / `leave.to_string()` 取法不变。
- `rs_surreal/point.rs:42-43`、`:90-91`：`object::values(out.ptset)` / `inst_info:{cata_hash}.ptset` → 同上按 pe 键取；`number` 字段在新形状里是键而不是值的字段，过滤 `number == ARRI/LEAV` 改成按键取。
- `rs_surreal/query.rs:1147`、`:1258`、`:1266-1281`：`inst_info.ptset` → `pe_ptset`，键 parse 成 i32 的逻辑保留。
- `rs_surreal/inst.rs:178, 191, 288, 302` 与 `geom.rs:310` 的 `out.ptset.d.pt` 点数组投影：按 gen-model D6「不动，注释写死恒 NONE」处理；rs-server 若有用到 `GeomInstQuery.pts` 的调用方，另列（本轮未发现 rs-server 直接用）。
- 不动 `pdms_pluggin/heat_dissipation.rs:11-15` 的 `InstPointMap` 形状。

**A3 · rs-server 消费点核对（预期零代码改动，逐条核语义）**

- `dq_bran.rs:1245, 1250, 1258-1259`、`query.rs:919, 922`、`heat_dissipation.rs` 的 arrive/leave 取点：键仍是 `LEAV`/`ARRI` 数值转字符串，形状不变。
- `dq_bran.rs:1425, 1438` 的点 3 / 点 4 `three_p_bore`：新形状里 `bore` 在每个点上，但 TEE 通常只有 P1/P2/P3，点 4 可能不存在 → 决策 D-A4（缺点 4 时的回退）。`pbore` 由 `bore: None` 转成 `0.0`，与旧语义一致。
- `heat_dissipation.rs:408, 604` 的 `len() > 3` 分支：`pe_ptset.count` 可直接用，但先保持 `ptset_map.len()`，只核两者一致。
- `query.rs:1203` `return fn::bran_comp_len({pe})`：函数由 gen-model 出厂（A0），rs-server 不改调用。
- 文档镜像 `dq_datacenter_api.md:766, 772, 780` 随代码同步。

**A4 · rs-server surql 副本处理（按 D-A2 执行）**

- 推荐路径：删除 `rs-server/resource/surreal/gy_common.surql`；若 `define_common_functions_on` 按文件名有依赖顺序假设，补一条「rs-server 不定义 `fn::bran_*`/`fn::elbo_len`/`fn::tee_len`」的注释到 `function.rs:51` 附近。
- 同时盘点 rs-server 其它 `fn::` 与 gen-model 重名的定义（`common.surql` 41 个、`material_common.surql` 25 个），本计划只处理 ptset 相关三件；其余列入后续所有权清单。
- `rs-server/history.txt:14` 的旧查询形态删掉或加注「已退役」。

**A5 · 严格测试**

- 在 `extraction_fixes_7997_test.rs` 追加第 4 步 `check_bran_ptset_alignment`：对 `24381/101407` PIPE 的两个 BRAN 直属成员，断言 `ptset_map` 非空、arrive/leave 键存在、`pt` 为有限数；并与 gen-model HTTP `/api/v1/element/ptset`（`gen-model/docs/specs/web-service-api.md:1248-1278`）逐点比对（`number` 对键、`pt`/`dir`/`bore`）。仍 `#[ignore]`，跑法同文件 `:33`。
- 把 `dq_query.rs:430`、`gy.rs:2921`、`query.rs:826` 三个观察型用例改成「有连接即断言、无数据才打印」：至少断言调用返回 `Ok`，不再吞 `Err`（B1 生效后 `Err` 只剩真实错误）。
- 真值：ams8000 `/M-OR-1RX-3081D`、`/C-OR-1RX-3040A` 的 `fn::bran_comp_len` 对 TTY 管身长度（gen-model 计划 W5 `:115`），由 gen-model 线提供数值，rs-server 只复用。

**A6 · 上线核对**

- 顺序：gen-model 先发（A1）→ rs-server 发 A4（删副本）→ rs-server 发 A2/A3（vendored core rev 更新）。中间态允许：gen-model 新函数 + rs-server 旧读路径 = `ptset_map` 为空（与今天相同），不更糟。
- 核：rs-server 启动后 `info for db` 里 `fn::bran_comp_len` 的函数体是 gen-model 版本（证明副本没再覆盖）；抽 7997 两条 BRAN 的 `ptset_map` 非空。

## 5. 决策点

| # | 问题 | 推荐 | 影响 |
|---|---|---|---|
| D-B1 | 常驻 runtime 只在 `cfg(test)` 还是生产也用 | 只 test；生产 `#[tokio::main]` 已与进程同寿 | 生产零改动 |
| D-B2 | 观察型用例是否改严格 | 改三处（A5 第 2 条），其余保持 | 会暴露今天被吞掉的真实错误 |
| D-B3 | 若 B0 判出 C1（在途请求随重连丢、客户端无超时） | 测试侧用 `tokio::time::timeout` 包探针与查询（只在 db_mode 脚手架）；是否改 fork 的 `router_reconnect` 重放策略另立任务 | 不改 SDK 则挂住变成超时失败，可诊断 |
| D-A1 | rs-server 读 ptset 走库（`pe_ptset`）还是走 gen-model HTTP | 走库；HTTP 只作对拍与 D3 范围外元素（NOZZ / 支架）的补充 | 不引入运行时耦合 |
| D-A2 | `gy_common.surql` 所有权 | gen-model 唯一出厂，rs-server 删副本 | P0 阻塞项；不定则新函数会被 rs-server 启动覆盖 |
| D-A3 | aios_core 点类型 | 新 `PtsetPoint` + `From` 到 `CateAxisParam`，`InstPointMap` 不变 | rs-server 消费点零改动 |
| D-A4 | TEE 点 4 不存在时 `dq_bran.rs:1438` 的回退 | 先核 7997 真实数据：若 TEE 无 P4，改成「点 3 为准、点 4 缺省回落点 3」并加注 | 影响 dq 三通口径输出 |
| D-A5 | `inst.rs` / `geom.rs:310` 的 `out.ptset.d.pt` 点数组投影 | 不动，注释恒 NONE（沿 gen-model D6） | rs-server 未直接使用 |

## 6. 风险

- vendored core 是两家共用的同一份（`rs-server/Cargo.toml:14-22`），A2 的改动必须提交进 old-aios-core 仓并更新两家的 rev，否则「本机能过、别人 clone 不过」（`Cargo.toml:96-104` 已警告过同类问题）。
- `ptset-publication` 迟迟不并：线 A 的 P3/P4 全部悬空；线 B 不受影响，应先做。
- 旧库里可能残留 `inst_info.ptset`（`INSERT IGNORE`，`model_db_adapter.rs:2630`）：切到 `pe_ptset` 后旧行不再被读，无害；但对拍时要用 rebuild 过的库。
- `bore` 从 `0.0` 变 `None`：`From` 转换回 `0.0` 保住旧语义，但任何新代码直接读 `bore` 要按 `Option` 处理。
- 并发挂住若是 C2（锁跨 await），B1 治不了，需要单独定位到持锁点；B0 先判别就是为了不把它算在 B1 头上。
- 两个仓库各自 `DEFINE FUNCTION` 同名函数的问题不止 ptset 三件（`common.surql` / `material_common.surql` 里还有 `next_is_type`、`next_type`、`children`、`get_ipara` 等跨文件重复定义），本计划不扩大范围，只记录。

## 7. 验收命令（由执行者在本机跑；本计划不跑）

- 线 B：`cargo test --bin rs-server`；`cargo test --bin rs-server -- --test-threads=1`；`RUST_LOG=surrealdb=trace cargo test --bin rs-server dq_fixing -- --nocapture`（B0）。
- 线 A：`cargo test --bin rs-server extraction_acceptance_7997 -- --test-threads=1 --ignored --nocapture`（A5 之后不再需要 `--test-threads=1`）；对拍 `POST /api/v1/element/ptset`（gen-model）与 `select ptset from pe_ptset:⟨…⟩`。

## 8. 不做的事

- 不改 `SUL_DB` 的类型或所在层（全仓 60+ 文件直接使用）。
- 不给生产加第二个 runtime。
- 不在 rs-server 里实现 ptset 写入或补扫（gen-model D5 已否决补扫；写入口唯一在 gen-model 发布事务）。
- 不处理 `gy_test.rs` 的 16 处 `#[ignore]`、`Cargo.lock` 的 surrealdb 条目、`db_mode.rs` 之外的测试组织方式。

## 9. 2026-10-28 决策追记（本节追加，不改写上文）

用户当日指示：**删除旧的兜底存储方式；给存储架构做版本号，方便对比和升级；旧数据按重新生成处理。**

据此对 §4 线 A 的修订（A2 已按此实施，不再是待定项）：

- **D-A2 已拍板**：`gy_common.surql` 的所有权归 gen-model。rs-server 的
  `resource/surreal/gy_common.surql` **已删除**——不删的话，rs-server 每次启动经
  `define_common_functions` 逐文件灌入同一个 ns/db，会把 gen-model 的新版函数打回旧版。
- **`legacy_fallback` 已拍板：删，不回退。** `geom.rs` 的 `ptset_map_projection` 里
  `?? $pe->inst_relate.out.ptset[0]` 整段移除，只读 `pe_ptset`。
- **后续修订（2026-10-28，已批准）**：版本不再按 dbnum / 行核对，改为**整个数据库的架构版本**，
  载体只有 `storage_version:current` 一条记录；核对只在进程启动时全局做一次。读点集
  （surql / aios_core `query_*_point_map` / rs-server）不再做任何版本判断，没有行就是 NONE / 空集。
  gen-model 不匹配只点名（日志 + `/health.storage_version` + 运维面板一键升级）；rs-server 不匹配
  拒绝启动并提示用 aios-database 升级。409 映射、SurrealQL `THROW` 与 `ptset_read_is_versioned_7997`
  已随本修订删除；`ptset_alignment_7997` 保留，前置条件改为「库已升级到当前架构版本」。
- **升级不是 `model/rebuild`**：一键入口 `POST /api/v1/storage-version/upgrade` 逐库复用
  `intent=reinitialize`（清空 → 重新解析 → 重新生成），全范围全部成功才写版本记录；进度在
  `storage_upgrade:current`。单库 `model/rebuild` 只重算模型、不写版本记录。

完整论证、三条正交轴与两次修订见 **ADR-091**；收尾与升级流程见
`docs/plans/2026-10-28-storage-version-closure-plan.md`。
