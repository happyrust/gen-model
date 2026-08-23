# Plan 021：Catalogue 相位按 MDB 声明口径收敛

**Date**: 2026-08-20
**Spec**: `specs/021-mdb-scoped-catalogue/spec.md`

## Summary

把「哪些 CATA 算 Catalogue 相位成员」的定义从「监控目录里扫得到的每一个 CATA 文件」
改成「本期 MDB 的 CURD 里 `STYP=CATA` 声明过的那些库号」——DESI 早就是这个口径，
CATA 一直漏在门外。改动落在三处：

1. **`update_scope.rs`**：同一次查询往返里多取一份 CATA 名单；`admits` 的 CATA 分支
   从无条件 `true` 改为查名单；冷启动降级三分（无 MDB / 有 MDB 无 CATA / 名字打错）。
2. **`increment_manager.rs`**：`catalogue_manifest_for_dirs` 收一个 `&UpdateScope`，
   只把声明过的 CATA 放进 `by_type`；`dependency_identities` **一条不动**；
   新增第四个范围桶，说自己的话。
3. **`manual_update.rs`**：手动 preview / execute 那份清单选择走同一道门
   （宪法 II：一条规则只有一份实现）。

**不做**的事同样重要：不碰 DICT（它在 `COLD_START_DB_TYPES` 里，MDB 名单就存在它里面）；
不碰 `dependency_identities` 与按需引用闭包；**不放宽 ADR-025 的相位屏障**。
最后一条有外部权威撑着：`DB_MDB::openMDB` 逐个 `internalOpenDB`，任何一个失败就
`internalCloseMDB` 把已开的全部关掉并返回 false——AVEVA 比本仓今天还严。

## Technical Context

- **Language/Version**: Rust edition 2024，nightly-2026-08-02
- **Primary Dependencies**: fork SurrealDB 2.1.4、`aios_core`（`DBType::CATA = 2`）、
  `pdms_io` / `parse_pdms_db`（读库头）
- **Storage**: SurrealDB；本特性只多读一次 `MDB.CURD`，不写任何表
- **Testing**: 纯函数单测（范围判定、降级三分、桶分类）+ 源码形状断言
  （定位索引不得被收窄）+ live 真机验收（四项目现场）
- **Target Platform**: Windows / PowerShell；构建口径见 AGENTS.md 的 CI 行
- **Constraints**: 禁止 `cargo clean`；工作树里另有在飞改动
  （2026-08-20 已落地的 `included_projects` 顺序兜底），按路径暂存
- **Scale/Scope**: 现场四项目、544 个候选库文件、MDB `/ALL` 声明 29 个 DESI

## Constitution Check

- **I 水位是承诺**：不碰水位。被 MDB 排除的 CATA 从此不建立完整应用水位——
  这与监听限定域下 CATA 的既有行为一字相同（CONTEXT「监听限定域」条），不是新语义。
  已经建立过水位的库被排除后水位行原样留着，**不清值、不回拨**。
- **II 一条规则只有一份实现**：这是本特性的主要宪法风险。CATA 范围判定今天有
  **三个**落点：`in_scope_with`（自动 sweep）、`catalogue_manifest_for_dirs`（清单选择）、
  `manual_update` 的 preview/execute 清单。缓解：判定本体只有 `UpdateScope::admits`
  一处，三处都调它，不各写各的；并补一条源码形状断言，禁止在清单选择里出现
  第二个手写的 CATA 名单比对。
- **III 静默失效是最高级别缺陷**：这是**最需要盯的一条**——本特性的动作就是「让一批库
  不再参与」，天然是静默的形状。三道护栏：(a) 被排除的库进独立聚合桶，说自己的话，
  与 MDB DESI 范围 / 监听限定 / 调试限定三句两两无交集（FR-005，issue #10 的教训）；
  (b) 冷启动三分，绝不把「读不出名单」和「声明了零个」说成同一句（FR-006）；
  (c) `/health` 与手动回执常驻报出本期声明的 CATA 数量（FR-008）。
- **IV 队列可消费 / 可收口 / 可复活**：不新增 action。被排除的 CATA 不再入队；
  已在队列里的行不受影响（本特性只改「下一轮排不排」）。
- **V 标识只用真值**：名单来自库里真实的 `MDB.CURD` + `DBNO`，不按文件名前缀、
  不按目录归属猜测。
- **VI 不变量由可执行的守护看住**：SC-007 列的三条各配一条回退即红的测试
  （CATA 分支改回 `true`、定位索引被一起收窄、相位屏障被放宽）。

结论：**通过**，不需要 Complexity Tracking 例外。II 条的三落点已收敛到单一判定函数，
III 条的三道护栏是 FR 级要求而非事后补的日志。

## Referenced Decisions

- **ADR-025（严格初始化阶段）**：本特性改它的第 2 条——`Catalogue` 从「included_projects
  的全部有效 CATA」改为「本期 MDB 声明的 CATA」。**需要一条 ADR-025 修订注记**，
  第 6 条与第 3 条（相位屏障）不动。`specs/006` 的 FR-005 同批对齐。
- **ADR-004（按需 CATA）**：本特性是它的正当性来源之一——被排除的 CATA 仍由按需闭包
  兜底，所以「不是相位成员」不等于「找不到」。
- **ADR-016（监控目录解析与项目数据域）**：`project_dirs` 与 `included_projects` 按下标
  一一对应，因此「靠重排 `included_projects` 换优先级」代价高，`catalogue_project_priority`
  作为可选覆盖层保留。
- **ADR-007（SYS meta 解析不受 included_files 约束）**：DICT 不进本特性范围的依据。
- **外部权威（宪法「外部权威」条）**：2026-08-20 对 `D:\AVEVA\Everything3D3.1\core.dll`
  的 IDA 取证——`DB_DB::findDB` 单键 map、`findDB(int,int)` 第二键是抽取号、
  `checkDBNoInuse` 强制 dbno 全局唯一、`DB_MDB::openMDB` 成员失败即全部回滚。
  四条结论要单独落一份 `docs/evidence/`，带地址与反编译片段。

## Project Structure

```text
specs/021-mdb-scoped-catalogue/
├── spec.md
├── plan.md
└── tasks.md

src/data_interface/update_scope.rs        # CATA 名单、admits、降级三分、declared_cata
src/data_interface/increment_manager.rs   # 清单选择过门、第四个范围桶、形状断言
src/data_interface/manual_update.rs       # 手动 preview/execute 走同一道门
src/web_service/handlers.rs               # /health 报本期声明的 CATA 数
docs/adr/ADR-025-strict-initialization-phases.md   # 第 2 条修订注记
specs/006-strict-initialization-order/spec.md      # FR-005 对齐
docs/evidence/2026-08-20-core-dll-dbno-namespace.md # 新文件：四条逆向结论
CONTEXT.md                                # 新术语「相位成员」+「严格初始化阶段」补一句
changelog.md
```

## Implementation

1. **`MDB_DBNOS` 参数名可注入**（`update_scope.rs`）。今天它写死 `$db_type`，
   一次往返里问两种类型就会撞名。改成一个 `fn mdb_dbnos(param: &str) -> String`，
   `fetch` 里拼 `$desi_type` 与 `$cata_type` 两份，三条 `RETURN` 一次发出去：
   MDB 名字列表、DESI 名单、CATA 名单。**MUST 保持一次往返**（FR-001）。

2. **`UpdateScope` 增 `cata: BTreeSet<u32>`**，`interpret` 收两份 dbnos。
   告警三分（FR-006）：
   - `known.is_empty()` → 沿用今天那句 bootstrap（只解析 SYS meta 建名单）；
   - MDB 名字不在 `known` 里 → 沿用今天那句配置错误 `bail!`；
   - MDB 在、但 CATA 名单为空 → **新的一句**，与 DESI 那句同形状不同措辞。
     两句可能同时出现，`warning` 得能装下两条（今天是 `Option<String>`，
     改 `Vec<String>` 或拼接，调用方 `scope.warning()` 的签名同步）。

3. **`admits` 的 CATA 分支**（`update_scope.rs`）：
   ```rust
   db_type == "CATA" && self.cata.contains(&dbnum)
       || db_type == "DESI" && self.desi.contains(&dbnum)
   ```
   `COLD_START_DB_TYPES` 与 `unrestricted` 两条早退**原样保留**。
   新增 `declared_cata()` 访问器（与 `declared_desi()` 对称）。

4. **`for_tests` 的兼容处理**：新增 `for_tests_with_catalogue(mdb, desi, cata)`；
   旧的 `for_tests(mdb, desi)` 保留但 CATA 传空表。
   **注意**：这会让既有测试里的 CATA 从「恒放行」变成「恒不放行」。
   逐个检查 `increment_manager.rs` 现有 4 处 `for_tests` 调用点，
   凡断言涉及 CATA 的改用新构造器；不涉及的原样留着。

5. **清单选择过门**（`increment_manager.rs::catalogue_manifest_for_dirs`）：
   签名增 `scope: &UpdateScope`（调用点 `sweep_dirs` 第 3116 行手上已经有 `scope`，
   第 3107 行刚用过 `scope.warning()`）。在 `by_type` 累加处加判断：
   ```rust
   // CATA 过 MDB 声明门；DICT 是 SYS meta，永不受限。
   if db_type == "CATA" && !scope.admits("CATA", info.db_no) {
       mdb_excluded_cata.push(format!("CATA:{}", info.db_no));
       continue;
   }
   ```
   **这一句必须在 `by_type.entry(...)` 之前、在 `dependency_identities.push(...)`
   之后**——两者的先后就是 US2 的全部内容，配一条源码顺序断言钉死（宪法 II 的先例做法）。

6. **第四个范围桶**（`increment_manager.rs`）：`mdb_excluded_cata` 与既有
   `out_of_scope` / `watch_excluded` / `debug_excluded` 三桶并列，循环外聚合成一句，
   措辞必须点名「MDB `{mdb}` 的 CURD 里没有声明这些目录库」并说明
   「它们仍可被按需引用闭包定位」。补一条与
   `the_sweep_keeps_watch_exclusions_out_of_the_other_two_buckets` 同型的断言，
   把四个桶两两隔开。

7. **手动路径同门**（`manual_update.rs` 约 3959–4019 行那份清单选择）：
   同样在喂 `select_catalogue_candidates` 之前过 `scope.admits("CATA", …)`。
   那里 `scope` 已经由 `self.update_scope(mdb).await?` 解出来了。

8. **`/health` 出口**（`handlers.rs`）：在既有 `initialization` 或 scope 相关字段旁
   报 `declared_cata` 数量；手动 preview / execute 回执同批带上（FR-008）。

9. **文档**：ADR-025 第 2 条加修订注记；`specs/006` FR-005 对齐；
   `docs/evidence/2026-08-20-core-dll-dbno-namespace.md` 写四条逆向结论
   （每条带函数名、地址、关键反编译片段）；CONTEXT.md 立「相位成员」词条并在
   「严格初始化阶段」条里补一句 CATA 的口径；changelog 一条。

10. **顺序**：先落三条会红的回归（CATA 分支、定位索引不被收窄的源码顺序断言、
    冷启动三分），再实现。`cargo fmt` → 定向 `cargo test --lib data_interface::update_scope`
    与 `data_interface::increment_manager` → CI 口径全量单测 → 真机。
    真机按 SC-001（blockers 空、到 model_ready）→ SC-002（544 不变）
    → SC-003（关掉 `catalogue_project_priority` 仍成立）→ SC-005（几何对拍）顺序走，
    **SC-006 的冷启动那条要用一次性空命名空间，不要拿现场库试**。
