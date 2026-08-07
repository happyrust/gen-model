# 开发方案：暂存窗口祖先链解析式预载与收口上溯去持久层化（2026-08-07）

> 依据：2026-08-07 拷问会话（pchat `fable-2`，十问逐条裁定，本文 §3 即决议记录）。
> 关联：ADR-017（暂存写回，读路由规则①③、不变量 I1/I4）、
> `docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md`（T2.1/T2.5 的未竟部分与 R1 风险）、
> `docs/plans/2026-08-07-staged-transform-write-routing-fix-plan.md`（同日 P0 修复，本文是它暴露的下一层）、
> issue #16（收口硬前置缺失；工作区未提交的 DESI 收口预检与本文独立、先行合入不冲突）。
> 本文只写「改什么、按什么顺序、怎么验收」。

## 1. 一句话

把模型工作项的**祖先链设计数据（到顶，含 WORL）从 db 文件解析进暂存库**，让
ancestor / transform / datacenter 上溯全部在 kv-mem 窗口内算对；收口与 journal
从此不再依赖持久层的 `fn::find_ancestor_types` / `fn::find_ancestor_type` /
`fn::ses_date` 现场求值；写回沿用 journal 分块重放 + `applied_sesno` 水位收口，
不新增 sesno 校验。

## 2. 事实基线（2026-08-07 读码取证）

1. **mutation 预载只拷 pe 拓扑，不拷名词表行**：`staging/preload.rs`
   `apply_model_mutation_preload_from`（:110-148）只 copy `pe` + `pe_owner` +
   旧产物；祖先的 `ZONE:⟨…⟩`/`SITE:⟨…⟩` 名词表行（POS/ORI 所在）不进暂存。
2. **静默错变换**：窗口内 `get_world_transform` →
   `fn::ancestor(pe).refno.*`（rs-core `query.rs:213-223`）；名词行缺失时
   `.refno.*` 静默 NONE，`att.get_position().unwrap_or_default()`
   （rs-core `spatial.rs:284`）把缺失当 (0,0,0)——未变更祖先带真 POS 时，
   窗口内算出的世界变换**丢位移且不报错**（方案 R1「静默错模型」实锤）。
3. **regen 祖先无显式保证**：`preload_generation_root_closure`（preload.rs:173-191）
   只重解析根子树 + CATA 闭包；祖先靠 `ensure_cata_refnos_parsed` 的
   `include_owner_chain`（cata_closure.rs:609，默认开）**顺带**解析，无校验无测试。
4. **收口在持久层现场上溯**：`render_datacenter_statements`
   （increment_pipeline.rs:944-992）对非交付单元名词渲染
   `fn::find_ancestor_types(pe:X,[…])[0]`，删除渲染 `$pe.owner.owner`——
   都在收口尾事务里对持久层求值。`fn::find_ancestor_types` 缺失 = issue #16
   （整窗白跑 → 写回无限重试无声）。
5. **journal 里也有执行时函数求值**：生成的 inst_relate 字面量内联
   `zone_refno: fn::find_ancestor_type({pe},'ZONE'), dt: fn::ses_date({pe})`
   （pdms_inst.rs:298）。窗口内因祖先缺失算成 NONE（靠 CommitOnly 的
   `zone_refno = NONE` 回填擦屁股）；写回重放时在持久层重新求值——重放对
   `fn::` 的硬依赖与第 4 条同族。
6. **`fn::ancestor` 是 9 跳硬展开**（common.surql:6-25），超深静默截断；
   与子树闭包加溢出探针前的形态一致，且直写模式下今天就存在。
7. **部分解析读的是文件字节快照**：`open_db_session` →
   `parse_file_db_basic_data`（cata_closure.rs:509-512）整文件读入内存并建
   refno 索引；`parse_refnos_with_session`（:544-589）按索引读**最新**元素版本，
   无 as-of-sesno 参数。快照时点 = 开会话时刻。
8. **预载不进 journal 的既有形态**：`execute_generation_preload`（StagingOnly）
   已有，preload 测试断言 `window.journal().await.is_empty()`。

## 3. 设计决定（拷问决议，2026-08-07）

| # | 决定 |
|---|------|
| D1 | 改造对象 = **main 分支 ADR-017 暂存窗口**（非 DuckLake 分支）。 |
| D2 | **统一预载全部模型工作项**（Transform/Delete 目标 + RegenRoot 根）的祖先闭包；房间轮预载不动；惰性闭包降回本职（兜 CATA 漏边），不再承担 DESI 祖先正确性。 |
| D3 | 设计数据（`pe` + `ATT_{noun}` + `ATT_UDA` + `pe_owner`，含带产物子树路径节点）**全部从 db 文件部分解析**得到，不从 rocksdb 的 Surreal 拷贝；现行 mutation preload 的 pe+pe_owner 持久层拷贝**退役**；旧生成产物（inst_relate 等，文件里没有）仍从持久层点查拷入。写入 `INSERT IGNORE`/幂等 RELATE，**StagingOnly 不进 journal**（窗口前旧态，持久层本就有，进 journal 只白胀资源配额）。 |
| D4 | 祖先链解析**到顶（含 WORL）**，不做「按 noun 停机于 SITE」的特例。 |
| D5 | **datacenter 上溯搬进窗口**：窗口内解出 rollup 目标，收口渲染固定目标 id 的纯 UPDATE；`fn::find_ancestor_types` 从水位推进必要条件除名（issue #16 预检降为兼容守卫，去留待 W6 的 fn:: 依赖审计定）。 |
| D6 | 生成字面量的 `zone_refno`/`dt` **渲染时写死已解出的值**（journal 纯数据化，重放不再需要这两个 fn::）；`zone_refno = NONE` 回填语句本期保留不动（自然闲置，退役另立清理项）。 |
| D7 | 写回**沿用现行 journal 分块重放 + `applied_sesno` 水位尾事务**，不新增逐行/逐表 sesno 乐观校验（单写者下永真）；「预载祖先 = 窗口起点文件态，写回前持久层不可变」写入本文档作不变量。 |
| D8 | **fail-closed**：任一工作项祖先解析失败或预载后完整性验证不过 → 整批失败终态带修法，不开模型工作；惰性兜底保留作运行期最后一道网。 |
| D9 | `fn::ancestor` 9 跳上限本期**只加响亮探针**（Rust 侧验链深 ≤ 9，超了整批失败带修法）；函数扩容 + 灌库版本验证另立一项。 |
| D10 | 验收：红先单测 + parity 扩展 + 渲染单测与源码钉 + 预载验证单测 + 两口径全绿为**合入门槛**；live E2E（绝对位置断言）为**手动验收**。 |

## 4. 工作包

### W1 祖先闭包解析式预载（核心）

- 新增 `staging/ancestor_preload.rs`（或并入 preload.rs）：
  - **范围解析（只读，不碰暂存）**：沿用现行纪律——工作项目标、带产物子树节点、
    锁范围都按**窗口前持久态**解析（`mutation_roots_resolve_against_the_
    pre_window_persistent_state` 已钉，不动）；由此得出「需要祖先数据的种子集合」
    = Transform/Delete 目标 + RegenRoot 根 + 带产物子树路径节点。
  - **数据装载（解析）**：对种子集合按 refno 用文件会话迭代上溯——
    定位 refno → `parse_refnos_with_session` 解析（含 owner）→ 沿 owner 上溯
    → 直到 WORL；每个元素落 `pe` + `ATT_{noun}` + `ATT_UDA` + `pe_owner`
    （复用 `ensure_cata_refnos_parsed` 的渲染函数，保证与解析层落库形状同构），
    经 `execute_generation_preload`（StagingOnly）写入；INSERT IGNORE /
    `record::exists` 守卫，不回退本窗口解析已写的新态行。
  - **文件快照封口**：文件会话在窗口 prereq 阶段打开一次（一个窗口只付一次
    整文件读取；能复用 collect_changes 已读的字节更好，作实现期优化项）；
    打开后校验会话最新 sesno == 本批 `end_sesno`，不等 → 走既有冻结重扫/
    吸收路径或整批失败重排，**不许拿超出窗口终点的文件态当祖先旧态**（事实基线 7）。
  - **完整性验证（D8/D9）**：装载完成后逐工作项断言——祖先链在暂存可走通到
    WORL、链深 ≤ fn::ancestor 的 9 跳预算、链上每个 pe 的 `refno` 链接可解引用；
    验证用 Rust 侧迭代上溯（不经 fn::ancestor，避免用被测物验证被测物）。
    失败 → `failed_window_result` 风格终态，消息带修法。
- 现行 `apply_model_mutation_preload_from` 的 pe+pe_owner 拷贝段退役；
  产物拷贝（`preload_existing_generation_products_for_refnos`）保留原样。
- `batch_worker` prereq 阶段的输入从 `mutation_targets`（仅 Transform/Delete）
  扩为全部模型工作项；「闭包解析 → 持锁 → 拷贝」顺序断言同步更新。

### W2 regen 路径接线

- `ModelRefreshPolicy::generate_roots` 的暂存分支不再把祖先正确性押在
  CATA 惰性闭包顺带解析上：W1 预载先行，惰性闭包退回兜 CATA 漏边
  （行为不变，职责声明变——加注释与测试钉）。

### W3 datacenter 收口窗口内解析（D5）

- `render_datacenter_statements` 改为 resolve-then-render：上溯在渲染时完成
  （经既有读路由——暂存窗口内查暂存，直写模式查持久层，同一代码），产出
  固定目标 id 的 `update datacenter_version:… set status=…;`；删除分支的
  `$pe.owner.owner` 同改。
- 源码钉：渲染产物含 `fn::find_ancestor_types` / `$pe.owner` 即红。
- issue #16 预检（工作区未提交改动）保留合入；其降级/退役随 W6 审计结论。

### W4 生成字面量已解值渲染（D6）

- `save_instance_data` 的 inst_relate 字面量：`zone_refno` 用渲染时经读路由
  解出的固定 record id（无 ZONE 时 NONE），`dt` 用会话表解出的日期值；
  直写与暂存两种模式同一条 resolve-then-render 代码。
- `zone_refno = NONE` 回填语句保留不动。
- 源码钉：字面量含 `fn::find_ancestor_type(` / `fn::ses_date(` 即红。

### W5 测试（与各工作包同提交，先红后绿）

1. **红先单测（W1）**：暂存窗口 Transform 目标的祖先 ZONE 带真 POS 且未被本窗口
   解析触及——修复前世界变换丢该位移（静默零），修复后等于真值；同形 regen
   用例一条。
2. **parity 扩展（W1）**：`staging/parity.rs` mini 窗口加「带 POS 祖先的
   Transform」形态，写回前持久层 diff 为空、写回后 diff 恰等于 journal 终态。
3. **预载验证单测（W1，D8/D9）**：断链 / 超 9 跳 / 名词行解析失败 → 整批失败
   带修法；压线（恰 9 跳）必须通过。
4. **渲染单测与源码钉（W3/W4）**：见各工作包。
5. **全量回归**：`cargo test --lib` 与 `cargo test --lib --features http_api`
   两口径全绿。

### W6 审计与台账

- **journal/收口 fn:: 依赖审计**：W3/W4 落地后逐语句过一遍 journal 与尾事务
  还依赖哪些 `fn::`（`fn::newest_pe`、`fn::room_code` 等），产出清单入库；
  据此定 issue #16 预检的去留（仍有依赖 → 预检保留并改探针对象；清零 → 退役）。
- ADR-017「落地情况」与写回方案 §6 补记本文；`fn::ancestor` 扩容 + 灌库版本
  验证另立 issue（D9）。

## 5. 顺序与验收

1. W1 + W5.1-3 同一提交（修复与钉子不拆开）；W2 随 W1 或紧随其后；
2. W3、W4 各自独立提交，各带 W5.4 的钉子；
3. W6 收尾；
4. 合入门槛 = W5 全部 + 两口径全绿；
5. 手动验收（有 E3D 环境时）：`tests/staged_transform_e2e.rs` 加带 POS 祖先的
   靶子，断言 `inst_relate.aabb` 落在**绝对位置**（不只是"变了"）；窗口人为阻断
   时持久层零痕迹（既有探针复用）。

## 6. 风险与回滚

- **R1 解析落库形状与 Surreal 现值不同构**：ATT 渲染复用 `ensure_cata_refnos_parsed`
  的同一套函数最小化；parity 对拍兜底。
- **R2 文件快照时点**：见 W1 封口——sesno 不等即拒；漏封的后果是把窗口终点之后
  的会话态当旧态预载（read-your-future），必须有测试钉住拒绝路径。
- **R3 dt/zone_refno 双模式一致性**：直写模式的 resolve 结果必须与旧 fn:: 求值
  逐字节一致（日期格式、NONE 形态）；渲染单测覆盖两模式。
- **R4 预载体量**：祖先链行数 O(树深×工作项数)，相对子树/产物预载是噪声；
  仍计入资源状态机配额，异常增长走既有告警。
- 回滚：W1/W3/W4 各是独立提交，revert 即回到现行为；不动数据形状，无迁移。

## 7. 明确不做（本期）

- 不动房间轮预载；不扩容 `fn::ancestor`（只加探针，扩容另立项）；
- 不退役 `zone_refno = NONE` 回填（另立清理项）；
- 不新增逐行/逐表 sesno 乐观校验；不做持久层拷贝的降级回退路径（与 D3 矛盾）；
- 不改 `GEN_MODEL_DIRECT_INCREMENT` 直写紧急路径语义（W3/W4 的 resolve-then-render
  对它是同代码路过，行为等价有测试钉）。

## 8. 落地情况

### 2026-08-07，W6（fn:: 依赖审计与台账）

已落地。台账：`docs/2026-08-07_journal-fn-dependency-audit.md`（逐渲染器清单 +
已消失依赖对照 + 待立项登记）。结论与动作：

1. journal（Both/CommitOnly）语句 W4 后已全部纯数据化；收口尾事务剩唯一
   `fn::` 硬依赖 = OWNER 搬迁的 anc/zone_refno 定点重算（并行 P1 的设计）。
2. **issue #16 预检保留、探针对象改为剩余硬依赖**（`fn::anc_u64` +
   `fn::find_ancestor_type`）：旧版 common.surql（缺 P1 新增函数）现在会被
   预检正确拒绝，而不是等到含搬迁的窗口在写回里无限重试。
3. ADR-017 增补「2026-08-07 落地补记（二）」；写回方案 §6 补记本线。
4. 另立项登记（台账 §3）：`fn::ancestor` 扩容 + 灌库版本验证（D9）；
   搬迁重算的 resolve-then-render 评估（P1 线）；双跑套件旧字面量用例的
   切换时机。GitHub issue 由维护者按台账内容开立。

### 2026-08-07，W4（生成字面量已解值渲染）

已落地（`pdms_inst.rs`：`ResolvedInstMeta` + `resolve_inst_meta[_on]`；
`save_instance_data` 两处 inst_relate 字面量与 `gen_cata_geos` 三处 tubi_relate
字面量全部改写已解值；测试 5 条，两口径全量回归全绿）。要点与实现期修正：

1. **范围比方案文本宽**：除 D6 点名的 `zone_refno`/`dt`，并行 P1（层级查询优化）
   后来内联进同一批字面量的 `anc: fn::anc_u64(pe)` 与 `dbnum: pe.dbnum` 一并
   改为渲染期已解值——两条线在此合流，journal 自此纯数据。
2. **解析走「当前世界 + ses 历史回落」**：pe 链经 `active_data_db()`（暂存窗口内
   查暂存——W1 已保证生成根子树与祖先在场；直写查持久层）；`ses` 行是
   append-only 历史，暂存 miss 回落持久层点查——单写者下合成结果恰等于老字面
   量在**写回重放时**对持久层求值看到的世界（R3 由
   `resolved_literals_equal_the_retired_fn_evaluations` 在同一世界上用引擎内
   `==` 对拍钉住，格式差异归零）。
3. **失败语义**：seed 行缺失 → 空态渲染（与旧 fn:: 对缺行的求值一致）；owner
   链**断裂**（字段指向的行不存在）→ 响亮失败进重试——不烘错值进 journal；
   refno 打包值越 i64 → 拒绝（P1 边界约束，测试夹具因此不能用 4000000001
   保留段）。
4. `zone_refno = NONE` 回填与启动自愈回填 `backfill_inst_relate_anc`（直打持久
   层的非 journal 路径）保留不动；`render_anc_repair_statements`（OWNER 搬迁的
   finalize 定点重算）仍用 fn::，属并行 P1 的设计，随 W6 审计。
5. 源码钉：`save_instance_data` / `gen_cata_geos` 函数体出现
   `fn::find_ancestor_type(`/`fn::ses_date(`/`fn::anc_u64(` 即红。

### 2026-08-07，W3（datacenter 收口 resolve-then-render）

已落地（`increment_pipeline.rs`：`resolve_datacenter_statements_with` 纯核 +
`load_pe_noun_owner_from_persistent` 生产 loader；渲染/等价/回退即红测试 7 条，
两口径全量回归全绿）。对方案文本的实现期修正与既定语义：

1. **上溯不走读路由，走「窗口 overlay + 持久层窗口前态」的 Rust 合成链**：
   W1 只把**模型工作项**的祖先预载进暂存，普通属性修改元素的祖先不在暂存里，
   经读路由（暂存）上溯会断链。改为 overlay（本窗口 ops 净态：
   `ModifiedElement::current_data.owner` / `added_owner`）优先、持久层窗口前态
   点查兜底（显式 `SUL_DB`，与锁域解析同一纪律）——单写者下两层合成 ==
   主数据重放后的持久层，即老 commit-time 现场上溯看到的同一个世界（等价性由
   `fixed_target_updates_hit_the_rows_the_server_side_walk_hit` 对拍钉住，含
   `type::thing` id 强转与 BRAN `$pe.owner.owner` 两个角落）。
2. **顺带除掉两颗雷**：Rust 走链上限 64，不再吃 `fn::ancestor` 9 跳静默截断；
   上溯解不出单元层归属（如 SITE 自身属性修改）从「`$pe = NONE` 塞进
   `type::thing` 的未定义行为」变成显式跳过（无交付记录可标）。
3. issue #16 预检**保留**并降级为「common.surql 灌没灌」的兼容守卫（探针注释
   已改写）：journal / 收口里仍有 `fn::` 消费者（inst_relate 字面量的
   `fn::find_ancestor_type`/`fn::ses_date`（W4 前）、OWNER 搬迁重算的
   `fn::anc_u64`）。去留随 W6 审计。

### 2026-08-07，W1 + W2

W1 与 W2 已落地（`staging/ancestor_preload.rs` + `preload.rs` 分桶改造 +
`batch_worker` prereq 接线与顺序钉 + `model_refresh` 职责声明；W5.1–5.3 测试
随行，两口径全量回归全绿）。三处对方案文本的**实现期修正**：

1. **D3 的退役范围收窄到 Transform/regen**：删除子树的 pe + `pe_owner` 持久层
   拷贝**保留**（`ModelMutationPreload::delete_hierarchy`）。被删元素已从文件
   refno 索引消失、无从解析；而删除级联的暂存子树枚举
   （`delete_inst_relate_subtree` → `collect_pe_subtree_refnos` →
   `active_data_db()`）靠这份拓扑圈出待清理的产物行——它与旧生成产物同类
   （「窗口前旧态、文件里没有」，ADR-017 读路由规则②的同一法理）。
2. **文件快照封口用逐元素校验实现**（比 §4 W1 原文的「会话最新 sesno ==
   本批 end_sesno」整文件校验更精确）：解析出的每个链上元素断言
   `att.sesno() <= end_sesno`，超出即 read-your-future 拒绝——忙文件在窗口
   执行期间落的新会话只要没触及本链就不误伤，触及了则本批失败重排、由既有
   冻结重扫/吸收路径扩窗收敛。
3. **W1 的显式预载覆盖 prereq 已知工作项**（Transform 目标 + Transform 子树
   模型节点 + 计划 RegenRoot + 本批新单元根）。本批执行中**后来**合并进工作单
   的待重试单元与级联派生根，其祖先仍由 CATA/DESI 惰性闭包顺带解析兜住
   （W2 原文「行为不变，职责声明变」的字面执行）；把它们也纳入显式预载 +
   验证属后续收紧项。
