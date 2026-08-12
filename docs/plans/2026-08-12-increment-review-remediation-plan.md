# 方案：增量更新实现审查（2026-08-12）的遗留项修复计划

状态：**已落地**（2026-08-12 当日；as-built 见文末 §6）
日期：2026-08-12
牵涉仓库：gen-model（全部工作包）
关联：审查基线 = 本日增量更新子系统 PR Review（Health 72/100，1 Critical / 2 Warning / 3 Suggestion）；
`docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`（已落地，本计划 P1 补其验收）；
`docs/2026-08-12_room-scoped-drain-commit-lock-assessment.md`（房间锁评估，本计划非目标）

## 1. 背景与现状

2026-08-12 上午对增量更新子系统（`increment_pipeline` / `staging/*` /
`side_effect_pending` / `aabb_tree` / `pdms_inst` / `batch_worker` 关键段 + 当时
未提交改动）做了一轮 PR Review，发现：

| 级别 | 发现 | 现状 |
|---|---|---|
| 🔴 Critical | 直写路径空间树变更无库侧痕迹（H1/H2），崩溃后静默复用陈旧树 | **审查当日已落地修复**（见下） |
| 🟡 Warning | 「树变更 ⇒ 库侧痕迹」不变量分散在各写入点、靠约定遵守 | 部分缓解（每写点源码钉 + ADR 不变量表述），仓级缺口仍在 → P2 |
| 🟡 Warning | `export_obj` 行为重构（N 文件 → 单文件合并、anc 子树收集）无任何测试 | 未处理 → P3 |
| 🟢 Suggestion | `export_obj` 查询 `in = … OR anc CONTAINS …` 与仓内索引纪律矛盾（`in` 无索引，OR 退化全表扫） | 未处理 → P3 |
| 🟢 Suggestion | `fn::zone_u64` / `fn::site_u64` 是位置语义非名词语义，非标准层级下静默给错 | 未处理 → P4 |
| 🟢 Suggestion | 源码钉（`include_str!` 自扫描）的重构摩擦与假绿风险 | 维持现状边界，不动（非目标） |

**Critical 的现状**：审查报告发出后、本计划制定前，H1/H2 修复已按
`2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md` 落进工作区
（`helper.rs` +146 / `occ_generate.rs` +152 / ADR-010 增补 / changelog / 方案
as-built §8），已核对：代码与方案 §3b/§3c 一致（D1 bump-only、D2 取宽一格的
[变更判定→事务→树同步]、D3 按块 bump），且顺带关闭了「删除清理挤进事务与同步
之间造成幽灵条目」的交错窗口。`cargo test --lib --features http_api` 实测
**661 通过 / 0 失败**。但方案 §6 的 live 崩溃场景验收与 C3 行为级测试自记
**未做**——这是本计划 P1。

## 2. 工作包

### P0 提交切分：固化已完成的成果（0.5 人日，先做）

工作区目前捆着至少四个可分离工作流的未提交改动，Critical 修复混在其中，一次
误操作（reset / 换分支）就可能连带丢失。按依赖序切分提交：

- [x] **提交 1 — H1/H2 epoch 痕迹修复**（`fix(spatial):` 前缀）：
      `src/data_interface/helper.rs`、`src/fast_model/occ_generate.rs`、
      `docs/adr/ADR-010-*.md`、`docs/adr/ADR-017-*.md`（08-12 补记行）、
      epoch 方案文档与房间锁评估文档（新增）、changelog 08-12 段。
- [x] **提交 2 — P3 `zone_refno` 退役**（`feat(query):` 或 `chore(schema):`）：
      `pdms_inst.rs`、`increment_pipeline.rs`、`staging/{executor,lifecycle,parity,preload,replay_safe}.rs`、
      `resource/surreal/common.surql`、`fork_surreal_compat.rs`、
      P3 相关 docs、changelog 08-11 段。
- [x] **提交 3 — G-02 /health 形状钉**（`fix(web):` 或 `test(health):`）：
      `side_effect_pending.rs`、`aabb_tree.rs` 渲染半边与九键测试、
      `handlers.rs`、`options.rs` 注释。
- [x] **提交 4 — export_obj 子树合并导出**：`python/src/exec_api.rs` 及 testbed
      基建——**建议在 P3 完成后再提**，让该提交落地时查询形态与测试已一并到位。
- 边界文件注意：`changelog.md`（两个日期段）与 `aabb_tree.rs`（persist 注释属
  提交 1、渲染半边属提交 3）横跨两个工作流，需 `git add -p` 按块暂存。
- db8000 会话快照夹具（`src/bin/db_session_fixture/*`、`scripts/e3d/db8000*`、
  对应 plan 文档）是第五个进行中工作流，不属本计划范围，由其自身节奏处置。
- 完成判据：`git log` 呈独立提交，各自 `cargo check` 绿；末态 `cargo test --lib
  --features http_api` 绿。

### P1 H1/H2 修复的验收闭环（1 人日）

epoch 方案 §6 的场景表目前只有 `--lib` 一档证据，崩溃恢复的端到端行为
（正是本缺陷的触发形态）还没验过。方案 as-built 自记 C3 的行为级/ live 用例未做。

- [x] live 用例 A（`#[ignore = "manual live"]`）：
      `helper.rs::live_direct_delete_crash_before_persist_recovers_by_rebuild`——
      幽灵构件种树 → 直写删除断言恰好 bump 一次 + `drift=true` → 清树重走启动
      加载模拟崩溃重启 → `rebuilt`、幽灵条目消失、指纹追平。「杀进程」以
      「清空内存树 + 重走 `load_project_tree_verified`」等价模拟（崩溃真正丢的
      只有进程态，磁盘与库侧痕迹逐字节相同）；真杀进程归 W5 故障注入轮。
- [x] live 用例 B：`occ_generate.rs::live_direct_refresh_crash_before_persist_recovers_by_rebuild`
      ——一并钉「逐位相等的重刷不 bump」（§6 场景 5）与「树落后于库的刷新必
      bump 并追树」，崩溃恢复断言同用例 A。
- [x] /health 验证并入两条 live 用例（崩溃前 `drift=true`、恢复后 `drift=false`
      的在场断言），不再单列。
- [ ] **沙箱执行**：两条用例按 `#[ignore]` 入库，需 testbed 8019 沙箱
      （`Start-TestSurreal.ps1` + `run_full_loop.py` 出基线）后 `--ignored --exact`
      逐条跑；跑通后在 epoch 方案 §6 逐条打勾、§8 补验收记录。
- 完成判据：§6 场景表全勾；live 用例入库可复跑（用例已入库，执行待沙箱）。

### P2 树变更不变量的仓级钉（0.5 人日）

审查 Warning 的残余：每写点的源码钉护不住**新文件里的新写点**——下一个
`GLOBAL_AABB_TREE.write()` 出现在新模块时，没有任何测试会红，ADR 文本靠人记得。

- [x] 新增仓级枚举测试（`aabb_tree.rs::tree_write_sites_stay_on_the_audited_whitelist`）：
      `std::fs` 递归遍历 `CARGO_MANIFEST_DIR/src`，收集含 `GLOBAL_AABB_TREE.write()`
      的文件集合，断言恰等于白名单 `{data_interface/helper.rs, fast_model/aabb_tree.rs,
      fast_model/occ_generate.rs, fast_model/room_fixture.rs}`。断言消息写明修法。
- [x] 评估结论：**配对 API 不单独做，由
      `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md` 吸收**。
      该方案（已评审定稿）的 §5「统一空间修改协议」+ D6 锁序
      （`STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE`，源码钉住）
      正是本项要的形状收敛，且以状态机 + 快照校验的完整设计覆盖——三个写点的
      锁跨度本就各不相同（durable 全跨度 / 普通直写 [判定→事务→同步] / 删除
      [探测→事务→摘除]），闭包式配对 API 要么装不下宽跨度、要么退化成形状复读，
      单独做配不上收益。仓级钉在闭环方案落地前是过渡防线，落地后与其锁序钉并存。
- 完成判据：仓级测试在场且绿（662 通过含它）；评估结论落档（本节）。

### P3 export_obj：查询形态修正 + 测试补齐（1 人日）

- [x] 查询改形（`python/src/exec_api.rs`）：
  - 去掉 OR 的 `in = type::thing('pe', $refno)` 臂——`anc` 含自身，该臂只在
    anc 缺失时才有意义，而它让整条谓词退化为全表扫（`preload.rs:334` 记录的
    实测口径：`in` 谓词 1.57s vs 图跳 3.1ms）。
  - 主查询只留 `WHERE anc CONTAINS $refno_u64`（走 `idx_inst_relate_anc`）；
    空结果时回落 `->inst_relate` 图跳点查（与 preload 同款
    `array::flatten(SELECT VALUE ->inst_relate FROM [pe:…])` 形态），兜住
    anc 未回填的旧库。
  - `RefU64::from_str` 失败不再 `unwrap_or_default()` 静默成 0，直接报错给
    调用方（0 值谓词永不命中，现在是无声空结果的一个来源）。
- [x] `run_full_loop.py` 导出步骤补断言：`files` 数 == 1、路径存在、obj 内
      `o ` 组行数 == `exported_insts`、`triangles` > 0；无缺失 mesh（沙箱刚
      force 生成过，按硬断言处理）。
- [x] changelog 补一条 export_obj 行为变化（2026-08-12「变更」小节）。
- 完成判据：testbed 全链路含导出断言绿；导出查询不再全表扫（大库手测时长口径）。

### P4 `fn::zone_u64` / `fn::site_u64` 语义免责（0.1 人日）

- [x] `resource/surreal/common.surql` 注释补一句显式免责：位置语义的前提是链尾
      恒为 `[…, ZONE, SITE, (WORL)]`；元素不经 ZONE 直挂 SITE 之类的非标准层级
      下，返回值**不保证是 ZONE**（退役的 `fn::find_ancestor_type` 是按 noun
      判定的，两者在这类数据上口径不同）；需要名词保证的读者应 join `pe` 验
      `noun`。
- [x] `fork_surreal_compat.rs` 对应用例（`dual_anc_u64_functions_execute_and_agree`）
      的文档注释同步免责口径与钉住范围。
- 完成判据：注释在场；无行为变化，无需新测试。

## 3. 非目标

- 房间 scoped drain 挪出 `STAGED_COMMIT_SERIAL`：已有专门评估
  （`2026-08-12_room-scoped-drain-commit-lock-assessment.md`），结论「现在不动」，
  瓶颈出现时按其 §5 杠杆顺序处置，观测（`room_duration_ms`）已就位。
- 源码钉模式的整体重构：维持「行为测试优先、钉子留给真跑不了的路径」的现状边界。
- `estimate_write_rows` 对 WHERE 集合写按 1 行代理的低报：代码内已有 ponytail
  标记，资源门禁实测低报时再接执行响应计数。
- 大文件（`increment_pipeline.rs` 4.2k / `batch_worker.rs` 3.4k 行）拆分：内联
  测试与文档注释占比高，模块边界（staging/ 九个子模块）本身清晰，拆分收益配不上
  搬动风险。

## 4. 验收汇总

- `cargo test --lib --features http_api` 全绿（基线 661 + P2/P3 新增）。
- epoch 方案 §6 场景表全勾（P1）。
- testbed `run_full_loop.py` 全链路（含导出断言）绿（P3）。
- `git log` 呈按工作流切分的独立提交（P0）。

## 5. 工作量与顺序

P0 0.5 + P1 1 + P2 0.5 + P3 1 + P4 0.1 ≈ **3 人日**。

顺序即编号：P0 先固化已完成的 Critical 修复（防工作区意外），P1 补其验收闭环，
P2 加固根因，P3/P4 独立收尾（P3 完成后再提 P0 的提交 4）。P2–P4 相互无依赖，
可并行。

## 6. As-built（2026-08-12 当日落地记录）

计划制定与执行期间，同一工作区有多条并行工作流推进，落地分工与计划文本的偏差
记录如下：

- **P0**：四个提交（`cfa7f8d0` fix(spatial) / `2fcc9c32` feat(schema) /
  `3e9fa20e` test(health) / `c75afa9a` docs(room)）由并行批次完成，分组与本计划
  一致；提交 4（export_obj）按计划推迟到 P3 完成后，与 P1/P2/P4 的成果一起收尾
  提交。房间锁评估文档与 ADR-017 补记按 docs(room) 单独成提交，比计划的归组
  更干净。
- **P1**：两条 live 用例入库（`--lib` 下 ignored，编译随套件验证）。与计划文本的
  偏差：「杀进程重启」以「清空内存树 + 重走启动加载」等价模拟——崩溃真正丢失的
  只有进程态（内存树与脏标记），磁盘文件陈旧与库侧 epoch 痕迹与真实崩溃逐字节
  相同，恢复判据走同一个 `load_project_tree_verified`；真杀进程的剧本归 W5 门禁
  故障注入轮（一致性闭环方案 §8 的 fail-point 注入是其正式承接）。**沙箱执行
  待做**（需 `Start-TestSurreal.ps1` + `run_full_loop.py` 出基线后 `--ignored`
  逐条跑）。
- **P2**：仓级钉已入库并随套件全绿（662 通过）。配对 API 评估结论：不单独做，
  由 `2026-08-12-spatial-tree-consistency-closure-plan.md`（已评审定稿，吸收
  epoch trace 方案）的 §5 统一空间修改协议 + D6 锁序承接；仓级钉作为其落地前的
  过渡防线。**九键 /health 契约将随该方案作废换新形状**——G-02 形状钉（提交 3）
  的历史使命到那时移交，属预期内的契约演进而非回退。
- **P3 / P4**：按计划落地，无偏差。`cargo check`（主库 + python 绑定 crate）与
  `cargo test --lib --features http_api` 全绿。
- **P3 的同门缺陷补记**（`d75d9820`，python 绑定线的收尾提交）：审查只盘到
  `export_obj`，但 `aios_db.db.inst` 有**一模一样**的
  `in = … OR anc CONTAINS …` 全表扫谓词与 `unwrap_or_default()` 静默 0 —— 同一
  条索引纪律，两个入口，只治一个等于没治。已一并改掉，形态上多一段：`anc` 索引
  → `->inst_relate` 图跳回落 → 两条都空且库里还有未回填行时才报错。多这一段是
  因为 `db.inst` 的空结果是**合法答案**（元素真没几何），而 `export_obj` 的空
  结果本就是错误条件。P3 缺的「行为测试」也在该提交补上：
  `python/tests/test_connection_layer.py` 覆盖三段式的每一段，跑在 conftest 自起
  的一次性内存 SurrealDB 上。
