# 2026-09-01 增量更新实现审核

目的：在动手做 refno 范围 reconcile（`docs/plans/2026-09-01-refno-scope-model-reconcile-plan.md`）
之前，把它要骑上去的增量更新链路核一遍。只读审核，未改代码、未跑服务。

## 范围与方法

深读：`increment_pipeline.rs`（收集/折叠/落库/收口主流程）、`model_update_pending.rs`
（render_upsert / 收口尾事务 / 水位 / 凭证 / seed / drain 认领与合批）、`fast_delete.rs`、
`on_demand_model.rs`、`generation_root.rs`、`sesno_range.rs`、`gen_root.surql`、
vendor `old-pdms-io/src/io.rs` 的语句渲染；对照 ADR-025 / ADR-050。
抽查：`staging/replay_safe.rs`、`e3d_model_service.rs` 的 publication 段、`lib.rs` 实例锁。
**未深审**（覆盖度声明）：`manual_update.rs` 的 unit rollup 全文、staging executor 的
journal 重放细节、房间泳道（room_*）、CATA 级联（cata_closure）。这四块如需结论要单独审。

## 总评

链路的工程纪律是成熟的：重放幂等（UPSERT CONTENT / 先删后插 / 软删 UPDATE +
`replay_safe` 白名单）、水位单调（`math::max` + 时刻跟随条件子句）、收口顺序（窗口
语句批先于尾事务，任一块失败水位不动）、死信有界（`MAX_ATTEMPTS=5`）与三条复活路径、
批锁覆盖收口、初始化门让位不烧 attempts、单进程实例锁——这些关键不变量**都有源码
内测试钉住**（多处以 `include_str!` 断言源码顺序的护栏测试）。审出的问题集中在
**凭证时效模型**上，一条重大、三条低危。

## Findings

### F1【重大 · 效率】未变根的完成凭证随水位推进整体失效

证据链（全部为现码事实）：

1. `gen_root.source_end_sesno` 全库只有三个写点：收口 settle
   （`render_delete_work`，model_update_pending.rs ≈1313–1328）与 e3d_model_service
   两处 publication CAS（≈737、≈1123）——**三者都只写「有工作项的根」**；
2. 收口尾事务只为本窗口变化根 upsert 工作项（`render_finalize_tail_with_effects`），
   未变根凭证不动；
3. 判据是等值：`generation_root_cache_current`（≈1497–1503）与 seed 的
   `gen_root_credential_is_current`（≈1567）都要求 `source_end_sesno == 当前水位`；
4. `execute_item` 的 RegenRoot 分支（≈2023）直接 `generate_roots`，**无凭证预检**；
5. unchanged-manifest 廉价路径（e3d_model_service.rs ≈1108–1116）只省**写**
   （truncate 掉几何 DELETE/UPSERT），几何计算照付。

后果：每应用一个窗口，该库全部未变根在凭证系统里变「过期」——(a) 运行期 ensure
碰到它们就整根重算；(b) 下次启动 `sync_and_seed_model_coverage` 把整库不等于当前
水位的根全部重排（`reconcile_model_coverage_at_startup`），启动风暴 ∝ 库规模 ×
窗口频率，万级根的库是小时级算力。

现场验证法（一次即可坐实）：任意有存量凭证的库应用一个小窗口后重启，看启动日志
`模型完整性扫描 dbnum=…: 当前根=N 当前凭证=M 新排队=K`——若 K ≈ N −（本窗口
变化根数），即坐实。

修复方向：即 reconcile 方案二期的 `gen_root.data_sesno`（判据从「等于水位」改
「凭证 ≥ 根子树数据版本」），须与 ADR-025 模型门联动改。

### F2【低 · 遗留】`ensure_regen_pending`（0 认领）无生产调用方但仍 pub

`source_end_sesno = 0` 的人工路径（≈1411）如今只有测试调它
（`room_live_issue7.rs`）；生产按需路径已全部走 `ensure_regen_pending_current`。
留着的风险：谁误用它，收口会把凭证写成 0 → 该根永判过期 → 每次显示都重生成。
建议：标注 `#[deprecated]` 或移进测试模块。

### F3【低 · 设计注意】drain 执行无凭证预检，与 F1 叠加时风暴全额付算力

如果在 `execute_item` / `run_regen_batch` 前加「凭证已是当前 → 跳过」的预检，
seed 风暴可以免掉大半算力；但行上没有 force 标志，直接加预检会吞掉
`/model/rebuild` 的 `force_all` 语义（seed force 的行 source_end_sesno 与凭证相等，
会被预检误跳）。要做就得先给 `model_update_pending` 行加 force 位。二期一并考虑。

### F4【信息】`root_model_source` 依赖 `pe.dbnum`，缺失即响亮失败

生成根的 pe 行没有 dbnum 时（历史行、级联根），ensure / 凭证判定直接报错
（≈1467–1472）。可接受（不是静默），但 reconcile 的扫描要按同一口径处理：
这类根归「判不了 → 落后」桶。

## 已核为成立的不变量（抽样清单，均有代码 / 测试证据）

- 收口顺序：窗口语句批（各自事务）→ 尾事务（durable 工作 → 空间意图 → settle 删行
  → 水位推进 → attempts 清 → 恢复记录删）；任一窗口批失败时水位不动、整窗口重放。
- 水位单调：`math::max` + 时刻子句排在覆盖之前；低于存量水位的批次序号时刻都不动
  （`a_batch_below_the_watermark_moves_neither…` 落库级测试）。
- 重放幂等：Add 前按固定 id 区间清边再 UPSERT CONTENT；关系先删后插；Deleted 是
  UPDATE 软删；`replay_safe` 白名单拒绝运行时选目标的写法。
- fold 只折叠同 refno **连续** Modified run，语句都是字面值、丢中间写不改后读。
- 崩溃恢复：`increment_update_attempt` 按原区间重放；文件已前进时废弃重建（并入新
  会话）并有警示；`end_sesno > 文件最新` 判回退阻断。
- 死信：攻顶 5 次成死信；复活三径（更新会话 upsert 自动清 attempts / 人工 retry 的
  原子 UPDATE（revision+1, attempts=0，只 UPDATE 不 UPSERT）/ seed force_all）。
- 并发：单进程实例锁（`lib.rs` `.gen-model.instance.lock`）；根锁覆盖生成与收口
  （护栏测试钉 lock → settle → drop 顺序）；自动轮 DeferBusy 不被占用根拖住。
- 初始化门：drain 让位不写 last_error 不烧 attempts；ensure 在缓存命中之后、落
  pending 之前查门（护栏测试钉顺序）。
- 窗口准入：水位 0 的 DESI/CATA 不猜历史（需初始化）；SYS meta 允许冷启动。
- 软删：`UPDATE pe SET deleted = true, sesno = N`，墓碑不衰减；删除的可见性由幸存
  owner 的同窗口 Modified 兜住（见 reconcile 方案「删除路径核验」节）。

## 对 reconcile 方案的影响

一期设计不变（它不动任何被审路径，只读 + 复用 `render_upsert` 入队）。F1 把二期
`data_sesno` 从「可选优化」抬到「应做」；F3 并入二期设计；F4 已写进方案兜底纪律。
