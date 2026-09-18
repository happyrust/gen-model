# ADR-081：最小交付单元的模型版本表从会话链按需算出，不落库、不做 diff 接口

- 状态：**已接受、已实施**（2026-09-18 plant3d-web 侧 grill 三轮 Q1–Q19 用户逐条拍板：版本对比取数迁到本仓、legacy `:3100` 链随开关退役、
  `limit` 截断由前端按 `since_sesno` 连续拉；同日 D1–D6 落地——`fast_model::unit_versions` + `GET /api/v1/model/versions`，实施记录见文末）
- 日期：2026-09-18
- 关联：
  - **沿用** ADR-079（生成根成败只活在进程里，`model_sesno` 是模型面唯一的持久事实）——本文不给模型面加第二份持久真值；
  - **沿用** `fast_model::historical_model`（会话钉死的 e3d-io 读 + `Historical(<refno>@<sesno>)` 进程内命名空间，不落库）——A/B 几何直接用它，本文不另造；
  - **复用** ADR-080 的选件层：e3d-model `plan_update(S→T)` / 增量管线的 `WindowDiff`、`classify_operation_impact`（CONTEXT「模型影响」三态）——版本表的「这一会话动了这单元没有、动的是网格还是摆放」用同一把尺子；
  - **消费方**：plant3d-web ADR 0065（`ModelVersionSource` 端口）与其执行清单 `docs/plans/2026-09-18-model-version-compare-gen-model-v1-migration-plan.md`；
  - **词汇**：CONTEXT「最小交付单元 (MDU)」「生成源版本」；plant3d-web CONTEXT「模型版本」= 单元子树在某个生成源版本下的只读状态，身份 `(dbnum, unit_refno, sesno)`。

## 背景

plant3d-web 的版本对比要回答三个问题：**这个单元有哪些版本**、**某个版本长什么样**、**A/B 差在哪**。
legacy 靠 DuckLake 的逐单元提交表（`impact_kind` 五态 + 不可变 manifest）回答前两个，第三个在前端按几何签名算。
本仓今天只回答第二个（`POST /api/v1/model/history/generate` → `query tool=instances`），第一个没有：HTTP 面全扫（~120 条路由串）
没有任何版本列表 / diff / release 路由，`tree/*` 也不带 `sesno`。而按 ADR-079，库里没有逐根 / 逐版本的制品可读——
「有哪些版本」不可能像 legacy 那样查一张表得到，只能从 dabacon 会话链算。

可用的积木都在：`db_file_header::session_chain(path)` 一次给整条链的 `sesno` + 时刻；增量管线逐会话取变更元素集（`WindowDiff`）；
`OWNER` 上溯到最近 MDU 根是选根的既有规则（CONTEXT「交付单元根」）；元素级影响归类 `classify_operation_impact` 是唯一权威。

## 决策

1. **新增 `GET /api/v1/model/versions`**：请求 = `ProjectReq` + `dbnum` + `refno`（`a/b` 或 `a_b`，同 `TreeReq` 归一）+ 可选 `since_sesno` / `limit`（默认 500，尾部截断回 `truncated: true`）。
   响应 `{ dbnum, unit_refno, unit_noun, file_latest_sesno, truncated, versions: [{ sesno, session_time, impact_kind }] }`，按 `sesno` 升序。
   错误沿 `ApiError` 码位：`400 INVALID_REFNO`、`404 REFNO_NOT_FOUND`、`422 NOT_A_DELIVERY_UNIT_ROOT`（带 `noun` 与项目当前 MDU 类型集）、`503 SOURCE_NOT_FOUND`。
   **单元根合法性由服务端按项目 `delivery_unit_types` 判**，前端不再持类型清单。
2. **版本表的算法**：`session_chain` → 逐会话变更元素集 → 每个元素 `OWNER` 上溯到最近 MDU 根，只留等于目标单元根的 → 该会话的 `impact_kind` 取最重：
   单元根自身 `Add` → `delivery`；`Deleted` → `tombstone`；任一元素 `Regen` → `mesh`；否则任一 `TransformOnly` → `placement`；全 `Skip` → `noop`。
   五态词表**沿用 legacy**（前端类型与词汇表都在用，换词无收益）；`noop` 会话**列出**（plant3d-web CONTEXT「无几何变化提交」保留版本轨迹）。
3. **不落库**：结果只进进程内缓存，键 `(dbnum, unit_refno, source_fingerprint)`，文件指纹变了整条重算；重启即丢。
   不新建逐单元版本索引表——建了就要回答重建 / 回退 / 幽灵那一整套问题（ADR-060 / 079 刚把逐根表退掉）。
4. **A/B 几何不另造**：两侧一律 `model/history/generate {dbnum, refno, sesno}`（B 是文件最新也一样：生产投影停在 `model_sesno`，常落后于文件最新），
   现有上限（≤ 100 000 元素 / 300 s）与生命周期（不落库、重启即丢、`DELETE` 释放）原样继承。
   **用户 2026-09-18 15:4x 重申为硬约束**：「历史查询出来的、生成的模型数据，除了 mesh 文件，其他都不要存到数据库——一次性的，传递给前端的。」
   代码上这条落在 `apply_geometry_delta_on` 的分派——`Historical(_)` 只进进程内 `ModelMemoryStore`，`Current` 才走 `model_write_behind::publish`；
   mesh 在分派之前 `bake_elements` 落成盘上内容寻址的 `.mesh`（与生产共享，可达性 GC 收）；快照回执 `HistoricalModelStore` 是进程内 `RwLock<HashMap>`；
   本文的版本表（D3）也只在进程内。守卫 `historical_model::tests::historical_scope_never_reaches_the_durable_publisher` 钉住分派形状。CONTEXT 补「历史投影」。
5. **不做 diff 接口**：差异在前端按 `geo_hash + geo_index + matrix` 签名算（今天的做法）。`plan_update(A→B)` 限在单元的服务端 diff 是以后的可选项，不是前提。
6. **不给 `tree/*` 加 `sesno`**：版本对比是单元尺度的，单元子树在某 sesno 的结构从 `history/query tool=snapshot` 取；全库版本树是另一件事。

## 后果

- 版本表的代价是数据面 O(会话数)，不触几何；长链靠 `since_sesno` / `limit` 与进程内缓存兜，首次冷算可能是秒级——接受，它是人点开对比时才发生的一次性成本。
- `impact_kind` 与生成路径共用 `classify_operation_impact`：那边的判定改了，版本表的分档跟着变，不会出现「版本表说 mesh、生成说 Skip」两套说法。
- 验收口径：ams8000 任取 3 个 BRAN 根，本接口的 `sesno` 集合与 `impact_kind` 与 legacy `GET /api/model/units/{refno}/versions` 逐条对上（legacy 的 `artifact_sesno` / `manifest_path` 不比，本仓没有对应物）。

## 实施记录（2026-09-18）

- 代码：`src/fast_model/unit_versions.rs`（算法 + 缓存 + 切片，纯函数 `fold_window_impact` / `slice_table` 单测 6 条）；
  `web_service/handlers.rs::model_versions`（`spawn_blocking`，错误码 → HTTP 映射）；`web_service/mod.rs` 一条 GET 路由 + 源码守卫
  `model_versions_route_is_a_single_read_only_get`（只此一条、无 diff / release 路由）。`historical_model::{source_service, source_fingerprint}` 放开为 `pub(crate)` 复用。
- **D2 的两处实现取舍**（初稿没写、动手时定的）：
  1. 归属判断不按候选逐个 owner 上溯，而是每窗把单元子树（含根）枚举成一个 refno 集、候选查集合——一窗的候选数跟整库这一会话改了多少走
     （ams7997 实测 332 会话 18.7 万候选），子树集只跟单元自身大小走（一条 BRAN 十几个成员）。`Modified` 候选在 T 端**或** S 端子树里都算碰到（被挪出去的也是变化）。
  2. 会话钉死的 `DbSet` 不走 `E3dModelService::build_set`（每建一池重解 attlib + 挂跨库解析器，43 ms / 个），而是 attlib 解一次共享、单库 `DbSet::new` + `add_db(sesno)`；
     跨库 owner 解不到只记告警。三步实测（debug build，ams7997 BRAN 24381/145018，332 会话）：owner 上溯 + `build_set` 27.9 s → 共享 attlib 9.4 s → 子树集 **1.9 s**
     （索引差分 0.8 s / 建池 0.6 s / 归属 0.6 s）。
- **真库探针**（`#[ignore]` `live_probe_unit_versions_on_a_real_database`，`AIOS_UNIT_VERSIONS_PROBE_ROOT` / `_DBNUM` 选目标）：
  - ams7997 BRAN `24381/145018`：latest 332，**1 版** `66 delivery`（2022-11-16），17 个成员在同一窗，归入 delivery；
  - ams8000 EQUI `24384/26480`（ADR-076 S5 那台、ADR-079 已关的孤儿现场）：latest 620，**11 版** `587 delivery` → 9 × `placement`（589–602）→ `604 tombstone`，
    与 `docs/evidence/2026-09-15-adr076-increment-durable-boundary.md` 记的「新建、挪了七窗、DELETE」一致；探针里另按会话页索引交叉核对：587 在 / 586 不在，604 不在 / 603 在。6.2 s（620 会话，debug）。
- 未做：与 legacy `/api/model/units/{refno}/versions` 的逐条对拍——legacy `:3100` 未起；ADR-045 语料的 `artifact_sesno` 本仓无对应物本就不比。留给 plant3d-web A3 端到端时补。
- **真机 HTTP（15:32 追记）**：本机 `:8022` 换成 `d47d747fd`（`codex/model-projection-cache@7bcdd60df` + cherry-pick `ef2284f12`，release）后，
  `GET /api/v1/model/versions?dbnum=8000&refno=24384/26480` 冷 **2.20 s**（623 会话：索引差分 959 ms / 建池 782 ms / 归属 514 ms，47 502 候选、命中 2）、
  缓存 **17 ms**；11 版与探针一致；400 / 404 / `since_sesno + limit` 截断各一条对上。plant3d-web dev `:3111` 两条浏览器场景（602→604 tombstone、587→602 placement）
  端到端全过，证据在 plant3d-web `docs/verification/model-version-compare-gen-model-v1-2026-09-18/`。
- **候选需求（未立，17:5x 追记）——属性历史对比**：plant3d-web 同日删掉了 legacy 的 `ModelTreeAttrDiffPanel` + `/api/model-history/*`
  （会话锚点 `anchors` / `resolve-anchor` / 元素属性快照 `snapshot`；grill 第 4 轮 Q25，用户拍板随 legacy 版本链一起删），前端此后没有「同一构件在 A / B
  两个 sesno 下属性逐项 before / after」的取数。本仓的积木都在：会话链 `db_file_header::session_chain` 给锚点，`ReadOnlyEngine::open_at(path, sesno)` +
  `element_attributes` 给某会话下的属性；缺的只是一条只读接口。与本 ADR 同一条纪律：按需算、不落库。
  **18:0x 用户拍板要做，走 `model/history/query tool=attributes`，契约先定（前端按它先接，后端等 `element_attributes.rs` 那一笔落地再实施）**：
  - 请求：`POST /api/v1/model/history/query { snapshot_key, tool: "attributes", arguments: { refno: "a/b" } }`——`snapshot_key` 就是
    版本对比 A / B 两侧已经 generate 出来的那份历史投影（`<unit_refno>@<sesno>`），`refno` 是单元子树里被选中的构件（也允许单元根自己）。
  - 响应：`{ snapshot_key, dbnum, sesno, refno: "a/b", exists: bool, noun: string | null, attributes: ElementAttribute[] }`；
    `attributes[]` 的行**与 `POST /api/v1/element/attributes` 同型**（`name / value_type / display / is_unset / editable / is_uda`），
    由同一个渲染器出——属性面板与属性历史对比对同一个属性印同一个字。该 refno 在那个 sesno 不存在（还没建 / 已删）→
    `exists: false, noun: null, attributes: []`，**不是 404**（对比面板要把「该版本不存在」当一种正常态显示）。
  - 实现：快照回执里的 `(source_file, resolved_session.sesno)` → 会话钉死的 `DbSet`（`unit_versions::SessionSets::at` 那一套：共享 attlib、
    单库 `DbSet::new + add_db(sesno)`）→ `element.extraction()` / `parsed()` → 走 `element_attributes.rs` 的渲染器（identity 行 + `render_attribute`
    + UDA / 类型默认值 materialize）。**不落库、不进生产投影、不建缓存表**（d-179）；快照 `DELETE` 后再问回 `SNAPSHOT_NOT_FOUND` 404。
  - 错误码：`INVALID_REFNO` 400、`SNAPSHOT_NOT_FOUND` 404、`REFNO_OUTSIDE_SNAPSHOT` 422（refno 不在该单元子树里——按 T / S 两端子树集判，
    与 `unit_versions` 同一把尺子）。
  - 前端（plant3d-web，已按此契约接线）：`ModelVersionGeometry.handle` = snapshot_key（适配器私有），`ModelVersionSource.attributesAt(geometry, refno)`，
    对比面板把 `attributesAt(side, refno)` 随树内差异上下文交给模型树，差异模式底部新面板逐属性对比 A / B。
  - **20:2x 实施（后端半边落地，契约一字未改）**：`element_attributes.rs` 那一笔（E3D 对齐渲染器）由 fable-5-1-6 代提入库（`6a76eefb4`）后，
    `ElementAttributeService::read` 拆成 `read`（当前时点，offer_backup）+ `render`（共用渲染器）+ `read_pinned(set, pin, refno)`（调用方给的会话钉死
    池，不 offer_backup、不落库，不存在 → `None`）；`HistoricalModelStore::attributes_at` 按回执 `(source_file, resolved_session.sesno)` 开池
    （`DbFileHeader::read` 反出 `db_type`）、`exists` / `is_descendant_of(单元根)` 判存在与归属、再 `read_pinned`；`POST model/history/query`
    的 handler 见 `tool == "attributes"` 走它（要 `arguments.refno`，缺了 `INVALID_REFNO`），`historical_error` 多映三档
    `INVALID_REFNO` 400 / `SNAPSHOT_NOT_FOUND` 404 / `REFNO_OUTSIDE_SNAPSHOT` 422。回执在契约之外多给 `complete` / `diagnostics` / `elapsed_ms`。
    入参裁决抽成 `attributes_target`，无真库也测得到三档错误码；守卫钉住「不存在是正常回执」与「`read_pinned` 不碰备份库 / 当前时点会话」。

## 追记（2026-09-18 20:4x）：构件级版本表 `GET /api/v1/element/versions`

用户要的是「查看某个构件的所有历史版本，再任选两版对比」。本 ADR 的 `model/versions` 只答得了**最小交付单元**（非单元根 422
`NOT_A_DELIVERY_UNIT_ROOT`），所以另开一条，语义与它并列、互不覆盖：

- **算法**：同一条会话链、同一份 `collect_window` 索引差分，只是把「候选 ∈ 单元子树」那一层换成 `refno == 目标`，命中后仍走
  `classify_operation_impact`（与生成路同一把尺子）。少掉逐窗建子树那一层：真库 ams8000 一条 BRAN 的成员，debug 构建冷算
  3.5 s（单元表命中缓存时），单元表自己那 5.9 s 是并排那一列的开销。
- **两列，不是一列**（用户 Q1 拍 (c)）：构件的几何可以在自身记录没变时改变（抬一根管子 → 邻居的派生管身跟着变；换 SPRE →
  形状换了而记录没动）。所以回执每一行给 `element_impact`（它自己，`null` = 没变）与 `unit_impact`（所属单元，`null` = 单元表里
  没有这一会话）两格；单元根由 owner 链上溯解出（`delivery_unit_types`），解不到就只给左列 + 一条 warning，不报错。
- **回执**：`{ dbnum, refno, noun, unit_root, unit_noun, file_latest_sesno, truncated, versions[{sesno, session_time,
  element_impact, unit_impact}], cached, elapsed_ms, stats, warnings }`。五态词表照旧（对构件而言 `delivery` = 它出现的那一版）。
  `since_sesno` / `limit` 与单元表同口径；错误码复用 `INVALID_REFNO` 400 / `REFNO_NOT_FOUND` 404 / `SESSION_NOT_FOUND` 404 /
  `COMPUTATION_TIMEOUT` 504——**这里没有 `NOT_A_DELIVERY_UNIT_ROOT`**，任意 refno 都受理，那正是它存在的理由。
- **几何对比不按构件切**：快照仍是 `单元@sesno`，前端拿回执里的 `unit_root` 去 `model/versions` + `history/generate`，对比结果再
  收窄到这个构件。按构件生成快照没有意义——管身是单元级派生物，单拎一个构件出来生成，几何对不齐。
- 代码在 `fast_model/element_versions.rs`（独立模块）。`SessionSets` 与 `unit_versions` 那份 `PinnedSets` 同形，是当时那个文件另有
  会话在改、不动它留下的一份复制；两份合一是后续的小活。

**2026-09-18 追记二：节点版本视图的两条（plant3d-web ADR 0066，共享决策 d-203）。** 同一条纪律再加两个派生量，都按需算、只活在进程内：

- `GET /api/v1/element/attribute-history?dbnum&refno[&since_sesno&limit]`——**属性变化时间线**：谁在哪一版改了什么。每行一个它
  自身记录被改过的会话，带会话页 `user` / `comment`、`kind`（created / modified / deleted）、与版本表同一词表的 `impact`，
  `changes[{name, value_type, before, after, stamp}]` 由属性面板同一个渲染器两端各出一次，`members` / `owner` 另列。
  `element/versions` 说「这一版它变没变」，这条说「改了什么、谁改的」，两条并列不覆盖。
- `GET /api/v1/node/diff-summary?dbnum&refno&a&b[&scope]`——**差异摘要**：A→B 不生成几何的一张账，候选按范围（`self` / `subtree`）
  过滤、按生成路同一把尺子判四态、按最小交付单元分组，附 `units{changed, unchanged, total, complete}` 与 `needs_confirm` /
  `estimated_projections`（阈值 `MODEL_HISTORY_CONFIRM_UNITS`）。前端只对变了的单元 `history/generate`——代价只跟「变了多少」成正比，
  这是容器节点（ZONE 下几百个单元）也能对比的前提。`A` 不早于 `B` → 400 `INVALID_VERSION_PAIR`。
- 代码在 `fast_model/attribute_history.rs` / `fast_model/node_diff_summary.rs`；`element_versions` 的 `SessionSets` /
  `modified_impact` / `resolve_unit_root` 放开为 `pub(crate)` 三处共用。
