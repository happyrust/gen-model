# 内存优先的 MDB 与数据面读透 · 合并计划

- 日期：2026-09-05
- 合并自（两份就此作废，只留作落地记录与取证）：
  - `docs/plans/2026-09-05-mdb-memory-first-initialization-plan.md`（元素骨架 / 选根；A、B 两层。
    它 2026-09-08 随过期计划清理删除，见 git 历史；其 P0 实测数据与评审记录只在 git 历史里）
  - `docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md`（数据面读透 / plant-ui；C、D 两层，
    **§9 落地记录仍以那一份为准**，本文不复制它）
- 起点是同一句话的两半：「不要通过数据库、库是备份」（前者）与「不解析任何数据、开箱即用」（后者）
- 状态：**零摄入启动、数据面签名、完整文件侧属性、读透提交协调、HTTP 契约、骨架换代与
  plant-ui 双仓适配已落地**；AvevaMarineSample 的隔离 spawned-mem 冷启动、属性、CATA、ensure、
  UDA 写透和断库降级已 live，SAVEWORK、持久库重启与 plant-ui 交互联调仍待执行。

> 2026-09-05 后续决策：本文是唯一现行计划。旧的
> `2026-09-05-kv-mem-zero-ingest-read-through-plan.md` 只保留 G0–G3 的历史落地记录；
> 本文早期章节中“属性先 ensure 再查库”“CATA 不提供属性”等旧裁决已被本轮实现取代：
> 属性统一由 `POST /api/v1/element/attributes` 直接读取 e3d-io，CATA 可读但永不持久化，
> DESI 在响应之后异步写透，名称索引以持久化 `cache_epoch` 为版本戳。

---

## 0. 一句话

两份计划从两头挖同一条隧道：一头把**服务自己判断要用的东西**搬进内存（骨架 + 选根），
另一头把**库要用的东西**改成用时才写（读透 + 写透）。合并之后只有一条线：

```
                 ┌── A 元素骨架（noun/owner/归属）── 服务自己判断：树、祖先链、选根
   e3d-io ───────┤
   （文件是唯一真值）├── direct_store 按需属性 ────────── 生成
                 │
                 └── C 写透 / 读透 ──→ SurrealDB ────── plant-ui 的读面 + 备份
```

**没有「内存 → 库」的同步器。** 内存与库各自从文件来，两条路共用同一份行渲染
（`ele_ops::ElementRows`）与同一套字典位。少了这一条，下一个人就会去写那个同步器。

---

## 1. 已经站住的

### 1.1 A0 · 元素骨架（已落地）

`src/data_interface/mdb_memory_store.rs`：进程级 `MdbMemoryStore`，装 `ref0→dbnum` 归属、
`refno→owner`、`refno→noun_hash`，启动时在 `init_mdb` 之后预热，失败只告警、退回按需现建。
接线三处：`DirectTreeService::open`（不再逐库 `indexed_refnos()`）、`roots()`（不再触发
`DirectStore::indexes()` 的全树扫描 + 描述符抽取）、`ancestors()`（零解析）。

### 1.2 C0–C3 · 数据面读透（已落地）

`data_face = "read-through" | "ingest"`（默认读透，与 `store_mode` 正交，13 份夹具补 `ingest` + 护栏）；
e3d-model `GenerationOutcome.visited`；`src/data_interface/read_through.rs`（`ElementRows` 渲染 + `UPSERT`
分块，`pe.sesno` 是 last-touch 会话）；编排层 `model_refresh::generate_roots_report` 每根在 `persist_root`
**之前**写透，失败只降级不拦模型。落地形态与差别见那一份的 §9。

### 1.3 各档粒度的实测代价（`bin/mdb_memory_probe`，release，串行）

`AvevaMarineSample /ALL`：28 个 DESI、**611,345 个元素、文件合计 301.4 MiB**。

| 档 | 装什么 | 串行墙钟 | 并行下界 | 常驻 RSS | 每元素 |
|---|---|---|---|---|---|
| `index` | refno + ref0 归属 | 0.46s | 0.01s | 5.3 MiB | 9 B |
| **`header`（A0 今天）** | **加 noun + owner** | **0.72s** | 0.10s | **19.8 MiB** | **34 B** |
| `named`（D2 的候选） | 再加存储的 `NAME` | 1.13s | 0.25s | 50.4 MiB | 86 B |
| ~~`extract`~~ | ~~整张属性表~~ | 8.02s | 2.92s | **1291 MiB** | 2215 B |

`extract` 那一行是**被否决的那条路的价钱**，留作理由不是选项。**时间从来不是问题**；
分界线是内存，named 与 extract 差 26 倍。原始数据 `out/q5-20260905/*.json`。

### 1.4 已验的两条对拍

- **属主对账**：记录头自报的属主 == 属性表的 `OWNER`，**611,345 元素零分歧**（`--tier owner-parity`）。
- **行对拍**：读透渲出的 `pe` / `ATT_<noun>` / `ATT_UDA` / `pe_owner` 与 `stream_baseline` 对同一元素
  **逐字段相同，`pe.sesno` 在内**（`tests/read_through_rows_parity.rs`，真实 db8000 归档 64 样本）。
- 全树快照 37 个 SITE 根 / 402,229 节点 / 12.6s，37 与改动前记录的基线一致。
- **两边都未 live**：服务一次都没启过。

---

## 2. 四层

| 层 | 内容 | 状态 |
|---|---|---|
| **A** | 元素骨架：服务自己判断用的常驻信息 | A0 已落地；A1（`NAME`）待 D2；A2（跟文件走）待 D13 |
| **B** | 选根照 core.dll 的四道过滤 | 未做 |
| **C** | 数据面读透：零摄入 + 写透 + 按需 | C0–C3 已落地；C4–C6 未做 |
| **D** | plant-ui 适配 | 未做 |

### 2.2 B 层的判据（合并后不变，原样保留）

core 判「这个节点有没有几何」**从来不看元素，只看它 noun 的字典位**——`DB_Noun::getInt(fieldId)`
是一次内存表查找。而 noun 与 owner 恰好就是 A 层已经装好的两样。四道过滤：

1. **库级**：只吃 `DB_DB::type ∈ {1,6}` = DESI + ISOD，CATA 不进重画路。
2. **noun 级**：`重画 el ⟺ el.type ∉ {WORL,MNUM} ∧ ( gb(el) ∉ {1,2} ∨ ( gb(el)==1 ∧ gb(owner) ∉ {1,2} ) )`
3. **祖先抢占**：结果集里 owner 也在集合中的元素不发事件，只发最顶那几个。
4. **新建只建顶**：`isElementTopLevelCreate`。

落到 `generation_roots_in_subtree` 三处：祖先链取 `TYPE` 改查骨架、子树 DFS 先查位后读 `NAME`、
`gb==2` 只穿过 / `gb==1` 整棵不进（109 + 98 个 noun）。两条不能弄错：**`gb==1` 的降级只看属主
一层**（上溯多层那条是 `isGraphicsIgnoredBetween`，另一个环节，不许合成一条）；**几何粒度位是
`significant` 不是 `toplevel`**（TOPF 管的是显示属性归属，19 个调用方里 13 个是
`get/setPrimitiveOrTOPF{…}`，一个几何重算消费者都没有）。

---

## 3. 合并解决掉的四处重叠

**这一节是合并的全部价值。** 两份各自都对，摆在一起才看得出来的四件事：

### 3.1 ★ 现在有**三份** owner 链走查，而且钉的不是同一个会话

| 实现 | 读哪 | 深度上限 | 走不通时 | 钉的会话 |
|---|---|---|---|---|
| `MdbMemoryStore::ancestors` | 内存骨架 | 256 | **响亮失败** | **启动那一刻**的文件最新 |
| `read_through::ancestors_of` | `DbSet` owner 导航（池级缓存） | 128 | 静默 `break` | **这次生成**的文件最新 |
| `generation_root::collect_chain` | 泛型 lookup 闭包 | `MAX_ANCESTOR_DEPTH` | 由闭包定 | 由调用方定 |

**不能简单让 `read_through` 改走骨架**——两者钉的会话不同。文件在服务跑起来之后前进了，
骨架给的是旧链、写透要的是新链，**替换掉就是把上一版的祖先写进这一版的 `pe` 行，而且不报错**。

裁决（D12）分两步：先把**能统一的统一**（深度上限、走不通时的语义、`WORL` 收尾的判据），
再把**不能统一的显式化**（每个走查在文档与函数名上说清自己钉的是哪个会话）。
彻底收成一份的前提是 A2（骨架跟着文件走），那是 D13。

### 3.2 骨架已经答掉了写透名单里的两件事——但 plant-ui 那一侧仍要 `pe` 行

kv-mem 那份列的零摄入坏点里，`nouns_of(anc)` 与祖先链**在 gen-model 进程内根本不需要 `pe` 行**：
骨架有全部 611,345 个元素的 noun 与 owner，19.8 MiB，一次查表。

**但写透仍然要写它们**——plant-ui 的读面是直连 SurrealDB 的（那是 A′ 的承诺，本合并不改）。
所以结论不是「少写」，是**分工写死**：

> **gen-model 进程内要 noun / owner / 归属，一律走骨架，不许查 `pe`；
> 写透只为进程外的读者（plant-ui / `/sql` 探针 / 备份）而写。**

少了这半句，下一个人会在 gen-model 里加一条「先 ensure 再查 `pe`」的路，而那条路骨架早就答完了。

### 3.3 `src/lib.rs` 启动分派：三段的先后写死

A0 的骨架预热已经在 `init_mdb` 之后落了；C5 要加读透分派。顺序固定为：

```
init_mdb（SYS 文件解成员名单）
  → 骨架预热（吃成员名单；失败只告警）
  → data_face 分派（读透则跳过 Meta → Design 摄入链，data_ready 即刻为真）
```

理由：骨架吃的是成员名单，必须在它之后；而读透分派要跳过的摄入链在它之后，
两者不换位就没有冲突。**这一段进源码钉，两边都别再自己排一次。**

### 3.4 选根的 noun 与写透的访问名单，两份名单不许互相顶替

B 层的选根从**骨架 noun 位**判「谁是根」；C 层的写透名单来自 **e3d-model DFS 的 `visited`**
（「这一根实际读过哪些元素」）。前者是判据、后者是事实，形状相近、含义不同：
拿 `visited` 去选根会漏掉没生成过的根，拿选根结果当写透名单会漏掉子树里的非根元素。

---

## 4. 决策表（合并、去重、重新编号）

### 已定

| # | 题 | 结论 | 来源 |
|---|---|---|---|
| D1 | 全属性常驻 | **不做**（「不需要全属性存储，init mbd 只是加载一些关键信息」） | 2026-09-05 批注 |
| D3 | 数据面开关 | `data_face` 二值，默认 `read-through`，与 `store_mode` 正交 | kv-mem Q1，已落地 |
| D4 | 写透范围与动词 | 访问名单 ∪ 祖先链；`UPSERT`；删除交给窗口软删 | kv-mem Q2，已落地 |
| D5 | 读透下文件事件 | 退化窗口（复用 `reconcile_startup_window`）+ refresh-if-present；`applied_sesno` 不动 | kv-mem Q3 |
| D6 | 未生成元素的属性与搜索 | 属性走 `element/ensure` 写透后照旧读库（读面零改动）；搜索只说实话 | kv-mem Q4 |
| D7 | CATA 元素属性 | 维持 ADR-058 S3：不入库，`element/ensure` 拒绝 | kv-mem Q5 |
| **D8** | **`applied_sesno` 在「库为备份」后是什么** | **读透下恒 0，是「没有导入这件事」不是幽灵水位；`initialization_required` 恒 false。ADR-021 三条判据一个字不改，因为读透形态压根不进那条路** | ★ **合并收口**：这是骨架那份列的「最贵的一格」，kv-mem 那份的 Q3 已经答了 |
| **D9** | **surql 那批读者（`common.surql` 167 处等）怎么办** | **不动**。它们读的 `pe` 由写透与按需读透填；缺行 = 还没读到，不是不存在 | ★ **合并收口**：同上 |

### 待定

| # | 题 | 倾向 |
|---|---|---|
| **D2** | `NAME` 进不进骨架 | 倾向加、只加它：+0.41s / +30.6 MiB，换掉树路径最后一次逐元素读属性表。**先过 name 对账**（`ScanTier::Named` 给的是文件存的 `NAME`，`node()` 读的是属性表的 `NAME`，逐元素相等才算，形状照 `--tier owner-parity`） |
| **D10** | B 层剪枝要不要门控 | 倾向不要：它是与 core 对齐，不是可选优化。但要有改前改后的逐根对拍 |
| **D11** | 库还要不要是启动硬前置 | 读透 + kv-mem 之后「连不上库就不启动」自相矛盾；但改它会碰 `clear_stale_at_process_start` 与实例锁。**两份都没答** |
| **D12** | 三份 owner 链怎么收 | 先统一口径、再显式化会话差异；彻底合一等 D13（§3.1） |
| **D13** | 骨架怎么跟着文件变 | watcher 检出新会话之后谁重建骨架、重建期间读什么。**今天骨架永远停在启动那一刻**——服务跑久了树会越来越旧，而没有任何东西会说 |

---

## 5. 里程碑（一条序列）

| 步 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| ✅ **A0** | 元素骨架 + 启动预热 + `direct_tree` 三处接线 | — | 已落地（§1.1） |
| ✅ **C0–C3** | `data_face` + `visited` + `read_through.rs` + 生成后写透 | — | 已落地（§1.2） |
| **C4** | `POST /api/v1/element/ensure` | C2 | 拒绝分支三条单测 + `mem://` 真跑（祖先链齐、`depth` 生效、超限 422） |
| **C5** | 读透形态分派：跳过摄入链、退化窗口、`initialization_required` 恒 false；**房间结构触发搬进窗口规划器（§9 R1）+ `room_rebuild_required` 随退化窗口自己落库（§9 R2）** | C0、C2、§3.3、§9 | 源码钉：读透分支全文不含 `stream_baseline` / `finalize_attempt` / `applied_sesno` 写；**`window_root_plan.rs` 的规划器与启动窗口两段不含 `panels_under_rooms` / `FROM pe`，结构触发只吃 `WindowRootSources` 那一次 `collect_window`（既有护栏 `the_window_planner_has_one_collection_arm` 不动）；`mem://` 真跑：S→T 只改一个 FRMW 的 `NAME`（命中关键字）→ 退化窗口排出名下每块 PANE 的 `RoomRecalcPanel`、零 `RegenRoot`；面板枚举失败 → `room_build:main.rebuild_required = true` 且 `StartupWindowReport.degraded` 点名** |
| **B1** | 选根照 core 四道过滤 | A0、D10 | **改前改后对同一批 SITE 逐根对拍必须逐字节相同**；剪枝省下的抽取次数入档 |
| **A1** | `NAME` 进骨架 | D2 + name 对账零分歧 | `--tier name-parity` 零分歧；`ElementRow` 护栏上限改一次且只改一次 |
| **M1** | 三份 owner 链收口 | B1、C4 | 深度上限与失败语义统一；每个走查在文档里说清自己钉哪个会话 |
| **C6** | `Verify-OutOfBox.ps1` 扩 + 09-04 计划 P5 | C4、C5 | 脚本跑通 |
| **D0–D5** | plant-ui U0–U5（Health / DESI 名单 / 属性面板 / 直管 owner / 搜索说实话 / 文档） | C4、C6 | 见 kv-mem 那份 §5 的 plant-ui 表 |
| **A2** | 骨架跟着文件走 | D13 | 文件前进后树与选根用的是新会话；重建期间的读语义说得出来 |

**顺序**：C4 → C5 是「零摄入」尚未兑现的那一半，优先；B1 与它们互不依赖，可并行；
M1 排在两边都动完之后；A2 是唯一一条会改变既有语义的，放最后。

---

## 6. 不变量（合并、去重）

- **N1 骨架只装关键信息。** 属性悄悄进来就是 19.8 MiB → 1.29 GiB，而涨的那天没有任何东西会说话。
  护栏 `the_skeleton_holds_no_attributes` 钉 `ElementRow` 大小；D2 放行 `NAME` 时上限改一次，**只改这一次**。
- **N2 骨架与它的消费者钉同一个会话**（今天是启动那一刻）。跨会话混用见 §3.1。
- **N3 属主只有一个真值。** 记录头与属性表逐元素相等（611,345 零分歧）。
- **N4 选根判据只有一份。** 护栏 `direct_tree_root_enumeration_is_the_shared_traversal`。
- **N5 行形状只有一份。** 写透 / 按需 / 基线渲出的行逐字段相同；`fast_model/` 不许出现 `pe` 表写语句。
- **N6 库不是内存的镜像，两者各自从文件来。** 少了这条，下一个人会去写那个同步器。
- **N7 缓存不做前置。** 写透失败不拦模型发布，`element/ensure` 失败不拦树与三维。
- **N8 CATA 永不入库**（ADR-058 S3）。
- **N9 开销与触碰量成正比**：写透 ∝ 生成子树，刷新 ∝ 窗口变更量，按需 ∝ `depth`，骨架 ∝ 库规模**一次**。
- **N10 进程内不查 `pe` 要 noun / owner / 归属**（§3.2）。
- **N11 房间触发的判据只有一份，且不读 `pe`。** 结构触发（改名 / PANE 搬迁 / CWALL-CFLOOR 搬迁）的三条规则
  只住在 `collect_room_structural_triggers` 一处，读透与摄入两条路都调它；面板枚举走文件侧
  `room_topology`，不走 `pe` SQL（§9 R1）。房间拓扑**不进骨架**（N1 的推论）。

---

## 7. 验收（AvevaMarineSample，`watch_dbnums` 先限定 7997 / 7999）

沿用 kv-mem 那份 §6 的十条，补三条骨架侧：

11. **启动横幅**说得出骨架装了几个库 / 几个元素 / 多少毫秒；`/health` 能看到同一组数。
12. **点一个从未生成的 ZONE 的眼睛**：选根走骨架，日志记的抽取次数比改前少（B1 的收益要量得出来）。
13. **强杀后重启**：骨架重新装、耗时与启动横幅一致；不依赖库里任何一行。

再补四条房间侧（§9；硬标准仍是 ADR-010 §9「增量收敛结果 == 全量重建结果」，14 / 15 收敛后各再跑一遍
`build_room_relations` 逐边对拍必须相等）：

14. **读透下改一个 FRMW 的 `NAME`**（命中 `-RM`）→ SAVEWORK：退化窗口日志出 `RoomRecalcPanel × N`（N = 名下面板数）、
    零 `RegenRoot`；房间轮之后 `room_relate.room_num` 换新号；`applied_sesno` 仍为 0。
15. **把一块 PANE 搬到另一间 FRMW**：新旧两间的 `room_panel_relate` 都收敛，构件不同时挂两间房。
16. **spawned-mem 冷启动**：日志出「跳过启动全量房间重建（空间树已验证为空……）」，启动耗时不含房间拓扑扫描；
    `/health.room_build.rebuild_required = false`、`tree_entries = 0`（R3 的平凡盖章）。
17. **`model/ensure` 一块 PANE**：发布事务里有 `room_recalc_panel_*` 行 → 房间轮消费 → 边出现；plant-ui
    房间总览出现该房间，未生成过面板的房间不出现且界面说的是「还没生成」不是「0 间房」（R4）。

### 2026-09-05 隔离真服务记录

- 使用 `spawned-mem`、HTTP `18022`、SurrealDB `18009`、`watch_dbnums=7997,7999`；冷启动 7.87 秒，
  DESI 骨架 28 库 / 611345 元素，冷库 `pe=0`、`applied_sesno=0`。
- `24381/34109` 从 e3d-io 返回 51 个属性，暖态请求 10 ms 量级；实际 UDA `:3D_SJRY` 写入
  `ATT_UDA` 并补齐 `UDA:13292_152`，默认值与 unset 未写入。
- CATA `23735/3` 同接口可读且数据库零新增；DESI `ensure(depth=1)` 写入 4 个元素，CATA ensure
  返回 `422 CATALOGUE_NOT_PERSISTED`。
- 强杀隔离 SurrealDB 后，属性仍以 109 ms 成功返回；镜像提交 2 秒超时，持久 spool 保留 1 个作业，
  `/health` 在 2032 ms 返回 `degraded`、`sul_db.connected=false`，退出后 18009/18022 均释放。
- 外部隔离 RocksDB 写入后强杀并重启存储与 HTTP 服务，`/dbnums` 前后均为
  `cache_epoch=1 / cached_pe_rows=1 / applied_sesno=0`；同时补上 spool 提交指纹，确保同一内容重放
  不重复推进 epoch、新内容才推进。
- 尚未 live：SAVEWORK 会话推进及迟到请求防回滚、plant-ui 交互、房间链 14–17。

---

## 8. 风险与不做

- **§3.1 的会话错配是本合并新暴露的风险**，两份各自看都不成立。M1 之前谁都别去「顺手统一」那三份。
- **D13（骨架跟着文件走）是今天真实存在的洞**：服务跑久了树越来越旧，没有任何东西会说。
  它不是本合并引入的（A0 落地那一刻就在），但合并让它变得更要紧——B1 之后选根也吃它。
- **B1 的剪枝改的是「走不走」不是「判不判」**：逐根对拍不通过就整条回退。
- **读透下房间的结构触发今天是断的**（§9.2）：C5 只写「跳过摄入链」而不带 R1 的话，FRMW 改名与 CWALL/CFLOOR
  搬迁之后 `room_num` 永远陈旧，且 spawned-mem 下连「下次启动全量重建」这条兜底都没有。
- **live 只覆盖了隔离读透主链。** 上述记录已经覆盖冷启动、属性、UDA、CATA、ensure 与断库降级；
  读透下的房间链（14–17）与 ADR-058 D8 的 watcher 退化窗口仍未面对真实 SAVEWORK 文件事件流。
- **不做**：全属性常驻（D1）；「内存 → 库」同步器（N6）；全库名称索引（另立计划）；
  CATA 属性读面；动 ADR-058 D3 的摄入形态基线；本合并不碰 plant-ui-view3d；
  不给读透造第二条房间队列、不把房间拓扑装进骨架、不让房间读面退回 `pe`、不动 ADR-010 §1 的最终一致口径与
  `room_incremental` 开关语义（§9.5）。

---

## 9. 房间增量怎么接进这条线

2026-09-05 用户问「房间增量更新应该如何加入这个逻辑」。全部实位核过工作树；结论先说：**几何触发早已跟数据面
脱钩，读透下原样能走；断的是结构触发与 `room_build:main` 兜底**——它们只住在数据窗口那条路上，而读透的
文件事件走的是 `window_root_plan.rs`，那个文件里 `room` 一个字都没有。

### 9.1 四段链，各自钉在哪一面

| 段 | 实位 | 依赖数据面？ | 读透下 |
|---|---|---|---|
| 几何触发（重算 + 清边） | `model_db_adapter::persist_root_in_chunks` → `room_publication_effects` / `render_room_publication_effects`：Directed 排 `RoomRecalc*`、FullDatabase 只清边，与几何同事务（ADR-010 09-02 增补 / ADR-040 §3） | 否（输入是投影记录） | ✅ 原样 |
| 结构触发（ADR-010 D12 / ADR-040 §4） | `model_update_plan::build_model_update_plan`：`collect_room_structural_triggers(range_eles)` + `panels_under_rooms`（**SQL 读 `pe`**）→ `RoomRecalcPanel`；枚举失败 → `room_rebuild_required` → 尾事务 `render_room_rebuild_required` | 三重：`range_eles` 来自摄入收集器；`pe` 查询；尾事务 | ❌ **不可达**（读透跳过摄入链，老规划器不跑） |
| 消费（房间轮） | `batch_worker::room_round`：门 = `model_ready` ∧ `room_incremental` ∧ 空间树 Ready ∧ 无 pending spatial ∧ `IdleOutcome::Settled`；`drain_rooms` → `recalc_panel_membership` / `recalc_element_membership` | 半：`load_room_panel_map` direct 走文件（`room_topology`）；`query_valid_insts` 的 owner 是 `in.owner ?? anc[1]`（`staging/mod.rs`，专为零解析写的） | ✅ 能走；`model_ready` 要 `data_ready`，C5 让它即刻为真才通 |
| 启动全量重建 | `lib.rs::skip_startup_room_build` 三门 → `build_room_relations`；凭据 `room_build:main` = (spatial_epoch, tree_entries) | 否 | spawned-mem：空库 → 必跑，但 0 块面板有几何 → **白扫全部 DESI 的房间拓扑**；external + 读透：每根发布 bump epoch → 重启必全量（已知） |

**不是洞的一条**：写透排在 `persist_root` 之前是为 plant-ui 的 `in.owner`，房间侧不依赖它——`OWNER_PROJECTION`
已经兜住，写透失败不影响房间重算（N7 在房间面上同样成立）。

### 9.2 断点的准确形状

读透下文件事件 → `reconcile_startup_window(S = 凭证下限, T = 文件最新)` → `build_model_update_plan_from_window`
→ 只产 `RegenRoot / DeleteCleanup / advance / lazy`。于是：

- **FRMW 改名**（纯 `NAME`）：不改几何，`plan_update` 的判据是几何影响，不排 regen → 无 `RoomRecalcPanel` →
  `room_relate.room_num` 陈旧到下次全量重建（spawned-mem 下 = 永不）。
- **CWALL / CFLOOR 搬迁**：PANE 几何不变，同上。
- **PANE 搬迁**：可能被 `plan_update` 判成触到根而顺带重生成 → Directed 发布 → 重算。**不能指望它**——ADR-040 §4
  要求三条显式，靠几何路兜底等于把判据改回「碰巧」。
- **`room_rebuild_required`**：读透没有尾事务，这个 durable 恢复信号没有落点。

### 9.3 接法（编号挂回 §5）

**R1（C5）结构触发搬进 `WindowRootSources::collect`，文件侧、一次收集臂。**
- 在同一个 `window`（e3d-model `collect_window` 的变更集）里挑 `Modified` 且 noun ∈ {FRMW, SBFR, PANE, CWALL, CFLOOR}
  的 refno，对比 `DbSet@S` 与 `DbSet@T` 的 `NAME` / `owner()`，复用 `collect_room_structural_triggers` 的三条规则
  （输入从 `EleOperationData` 抽成 `(refno, noun, old/new name, old/new owner)` 的小形状，或加一个 adapter；
  判据本体不复制——N4 / N11）。
- `panels_under_rooms` 换源：不用 `pe` SQL，用 `room_topology::collect_room_groups(roots = renamed_rooms, hierarchy,
  lookup@T)`——它本来就是「与生成根枚举同形状的纯遍历」，hd / hh 两层口径逐字对齐，传改名的房间当 roots 即得名下面板。
- 产物 `RoomRecalcPanel` 并进 `plan.work_items`（`into_model_update_plan` 之后 extend + dedup），随
  `reconcile_startup_window` 既有的 `enqueue_plan` 落 `model_update_pending`。房间行不带 dbnum，与 `record_id` 现口径一致。
- 护栏 `the_window_planner_has_one_collection_arm`（禁第二条 `collect_window(`）不动：结构触发吃的是已有的那一次。
- 摄入形态的老规划器随后也改调同一份触发判定并删掉 `pe` SQL——可以分两步，终态只有一份。

**R2（C5）`room_rebuild_required` 的落点。** 退化窗口本来就是「结果自己落库」（`reconcile_startup_window` 注释第 3 条）：
枚举失败时紧跟 `enqueue_plan` 单发一条 `render_room_rebuild_required`（幂等 UPSERT），并写进
`StartupWindowReport.degraded`。语义不变——只有成功的全量重建清它。spawned-mem 下它随进程消失，而那形态下一次
启动本来就是「没有凭据 → 跑」。

**R3（独立，✅ 2026-09-05 已落地）启动全量重建加一道门。** `skip_startup_room_build` 的门序改成
止血口 → `startup_autorun` → **空树平凡**（`vacuous_room_build_reason`：只认 `ReadyEmpty`，纯函数、只读进程态）→ 库侧对账。
空树时 Skip「空间树已验证为空（库里没有可用几何指针）：没有已发布几何可算，全量重建无事可做——读透 + 空库的常态，
房间归属由发布事务排的重算收敛」，**并照样盖章**（`room_model::stamp_vacuous_room_build`，树非空拒绝）：凭据说的是
「上次成功重建看到的世界」，空世界也是一个世界，不盖的话 `/health.room_build` 在 spawned-mem 下永远是「没有凭据」、
下次启动的对账也没有比较对象；随后每次几何发布都 bump epoch，重启失配照样判「跑」。省掉的是 spawned-mem 每次冷启动
对全部 DESI 做房间拓扑扫描 + 逐块 `NoGeometry`（此前 `ensure_room_tree_coverage` 对 ReadyEmpty 放行 (0, 0)，
所以是白跑不是错）。护栏：`the_four_gates_keep_their_precedence`（取代三门那条）、
`only_a_verified_empty_tree_makes_the_full_rebuild_vacuous`（八态真值表）、`the_vacuous_gate_stamps_instead_of_rebuilding`。

**R4（D 层 plant-ui）房间读面说实话。** `plant-ui-data/src/room.rs`：`overview_sql` 读 `pe WHERE noun = 'FRMW'`、成员名
读 `pe`、`element_edges_sql` 走 `pe_owner`。读透下这些行只在**生成过面板的房间**才有（祖先链写透保证 FRMW / CWALL 在）。
结论：房间总览 = 已生成过面板的房间；查空不是「0 间房」而是「还没生成」（与 U4 同口径）。可选：用 C4 的
`element/ensure`（FRMW，`depth = 2`）先把房间目录拉出来——只出目录，不出边。

**R5（记进 M1）第四份会话走查。** `load_room_panel_map_from_files` 走 `E3dModelService::from_current().design_sources()`
= 消费时点的文件最新，而排任务的发布事务钉的是生成会话 T，两者可能不同——§3.1 同类。房间是最终一致的派生面
（ADR-010 §1），可接受，但要进 M1「每个走查说清自己钉哪个会话」的名单。D13 骨架陈旧**不**影响房间（房间拓扑不吃骨架）
——所以 R1 用 `DbSet@T`，不改吃骨架 owner 表，否则 B1 之后会吃到 D13。

**R6（单列）`persist_dbnum` 的 FullDatabase 策略在读透 + spawned-mem 下失去依托。** 它只清边，靠「启动全量房间重建
收尾」（ADR-010 §4 收窄 1）——而 spawned-mem 重启 = 空库，`model/rebuild` 全库之后房间一直空着。倾向：全库落完调一次
`build_room_relations`（有覆盖率门），不把 FullDatabase 改成 Directed（收窄 1 的理由仍成立：逐元素排重算 = 每个元素一次）。

### 9.4 顺序

R1 / R2 是 C5 的一部分（已写进 §5 的 C5 行）；R3 独立可先做；R4 归 D0–D5；R5 记 M1；R6 单列。验收见 §7 第 14–17 条。

### 9.5 不做

不给读透造第二条房间队列；不把房间拓扑装进骨架（N1）；不让房间读面退回 `pe`；不改 ADR-010 §1 的最终一致口径；
不动 `room_incremental` 开关语义（关着只摘 `RoomRecalc*`，清边与 epoch 照旧——ADR-040 §3）。
