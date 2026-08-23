# Spec 021：Catalogue 相位按 MDB 声明口径收敛

**Created**: 2026-08-20
**Status**: Proposed
**Input**: 「结合 core.dll 的分析，我们应该如何解决呢，都是指定了 MBD，然后初始化」
——现场事故是 `跨项目 CATA/DICT dbnum=7000 冲突且没有 catalogue_project_priority 选主`
把 Catalogue 相位阻断，进而把一条毫不相干的 DESI 增量（dbnum 7999，sesno 90..=93）
永久钉在队列里。

## 背景事实

四条既有事实决定了这个特性的形状，写在前面免得实现时重新发现：

1. **core.dll 里 dbno 的命名空间是 MDB，不是磁盘。**（2026-08-20 IDA 取证，
   `D:\AVEVA\Everything3D3.1\core.dll`）
   - `DB_DB::findDB(int)` 读 `0x6395048` 上唯一一张 `std::map<int, DB_DB*>`，键是裸 dbno，
     **没有任何项目维度**；
   - `DB_DB::findDB(int, int)` 的第二个键是**抽取号**而非项目：它查同一张表后沿
     `*(DB+348)` 走链表比 `*(DB+136) == a2`，而 `DB_DB::extractChildNumber` 返回的正是
     `*(子抽取+136)`（子抽取 = `*(this+352)`，`DB_DB::leafExtract` 沿 +352 递归）。
     `DB_System::findDB(int,int)` 只是转发并按同一对 `(int,int)` 缓存；
   - `DB_DB::checkDBNoInuse(int)` 枚举 `DB_System::getAllDbs()`，只要**任何**另一个合法
     库的 `DBNO` 相同就报 253 号消息。**同号在 AVEVA 里是非法的，从不被仲裁。**
     那个枚举的范围是「当前系统 / MDB 里的库」，不是「目录里扫得到的每个文件」。
2. **本仓的 CATA 从来没过 MDB 这道门。** `UpdateScope::fetch` 只
   `bind("db_type", DBType::DESI)`，根本没取过 MDB 声明的 CATA 名单；
   `UpdateScope::admits` 里那句是 `db_type == "CATA" || (db_type == "DESI" && …)`
   ——CATA 无条件放行。
3. **冲突是在清单选择阶段造出来的，比 `admits` 还早。**
   `IncrementManager::catalogue_manifest_for_dirs` 直接 `WalkDir` 全部监控目录，把扫到的
   **每一个** DICT/CATA 塞进 `by_type`，再交给 `select_catalogue_candidates` 选主。
   现场那台机器挂了四个项目（AvevaMarineSample / AvevaCatalogue / SCB / ZDJ），
   `dbnum=7000` 在其中不止一个项目里有文件，于是产生了一个 AVEVA 永远不会遇到的冲突。
4. **同一个循环里有两个互不相干的桶，这是本特性能安全落地的关键。**
   - `by_type` → 相位成员 + 选主 → **会产生阻断相位的 blocker**；
   - `dependency_identities` → `cata_closure::install_dependency_identity_manifest`
     → `InMemoryCataLocator` → 按需引用闭包（ADR-004）的定位索引，现场 544 个文件。
   两者在同一趟遍历里各自累加，**收窄前者不影响后者**。
5. **core.dll 对「成员库打不开」的处置是整体失败并回滚，不是部分放行。**
   `DB_MDB::openMDB` 逐个 `internalOpenDB` 遍历成员库，**任何一个返回 false 就
   `goto` 到 `internalCloseMDB(this)` 把已经开好的全部关掉并返回 false**；
   随后的 `internalOpenProductDBs` / `openTransDB` 失败同样跳到那一处。
   `internalOpenProductDBs` 内部也是 fail-fast：`DB_DB::openRead` 一失败就把该库的
   错误抄进 MDB 并中断循环。**AVEVA 没有「半个 MDB」这种状态。**
   与之相对，查一个**不是成员**的 dbno 只是每次查询各自失败：
   `DB_DB::findDB(int, MR_Message&)` 命不中就 `MR_Message::Reset(43, 62)` +
   `Int(1, dbno)` 并返回 NULL，没有任何全局效果。
   **两种粒度对应两件事：成员失败 = 全停；非成员查不到 = 单次查询失败。**

另有一条已经落地、本 spec 不再重复的改动（2026-08-20）：
`select_catalogue_candidates` 的 rank 已改为「`catalogue_project_priority` 点名的在前，
其余按 `included_projects` 书写顺序接着排」，所以「名单漏写一个项目」不再阻断。
本特性把那条兜底从**主要机制**降格为**最后一道**：MDB 声明过、却仍有多个项目拿得出
同号文件时才轮到它。

## User Scenarios & Testing

### User Story 1 - 目录里多出来的目录库不该造出冲突（Priority: P1）

作为运维，我在 `DbOption.toml` 里挂了四个项目的目录（因为 AMS 的目录数据在
AvevaCatalogue 项目里，SCB / ZDJ 又各有各的用途），我希望服务只把**本期 MDB 真正声明过
的** CATA 当成 Catalogue 相位的成员。别的项目目录里恰好同号的那些文件，
既不该被拿去选主，也不该因此产生任何 blocker。

**Why this priority**: 这是事故的直接成因，也是唯一一个能把冲突**消灭在源头**而不是
事后仲裁的切片。

**Independent Test**: 纯函数测——给定「MDB 声明 CATA = {7351, 7355}」与一份含
`7000/7351/7355` 三个 dbnum、跨三个项目的候选文件清单，断言只有 7351/7355 进入
`by_type`，7000 一个 blocker 都不产生。

**Acceptance Scenarios**:

1. **Given** MDB `/ALL` 的 CURD 里 `STYP=CATA` 声明了 N 个库号，
   **When** 跑一轮清单选择，
   **Then** 只有 dbnum 落在这 N 个里的 CATA 文件进入 `by_type`，其余直接不是候选。
2. **Given** dbnum 7000 在 AvevaMarineSample 与 AvevaCatalogue 各有一个 CATA 文件、
   而 MDB 没有声明 7000，**When** 跑一轮清单选择，
   **Then** 不产生 `跨项目 CATA/DICT dbnum=7000 …` 这一类 blocker，
   Catalogue 相位不因它而阻断。
3. **Given** 同上但 MDB **声明了** 7000，**When** 跑一轮清单选择，
   **Then** 照旧走选主：按 `catalogue_project_priority` + `included_projects` 顺序选一个赢家，
   落选方进 `shadowed` 并打 `[manifest] … 被项目优先级遮蔽`。
4. **Given** 被 MDB 排除掉的那些 CATA，**When** 看日志，
   **Then** 有一条聚合行报出总数与样例，且它的措辞与 MDB 范围判定、监听限定、
   调试限定三种既有嗓音**互不混同**（沿用 `skip_reason` 的分发纪律）。

---

### User Story 2 - 按需引用闭包照样解得开（Priority: P1）

作为模型生成的使用者，当一个设计元素引用到一个 MDB 没有声明的 CATA 时，
我希望它照样能被解析出来——收窄的是「谁能拖住相位」，不是「谁能被找到」。

**Why this priority**: 这是 US1 的安全带。少了它，US1 就从「消灭伪冲突」变成
「把真实依赖弄丢」，而弄丢依赖的表现是几何静默画错，属于宪法第三条的最高级别缺陷。

**Independent Test**: 纯函数测——同一份候选清单跑一趟，断言 `by_type` 被 MDB 收窄
而 `dependency_identities` 条目数**逐条不变**（现场口径：544 个文件一个不少）。

**Acceptance Scenarios**:

1. **Given** MDB 只声明了 2 个 CATA、目录里有 544 个候选库文件，
   **When** 跑一轮清单选择，**Then** `install_dependency_identity_manifest` 收到的
   仍是 544 条，`[manifest] … 依赖身份清单：544 个文件` 这句话不变。
2. **Given** 一个设计元素的 SPRE 指向 MDB 未声明的 CATA dbnum，
   **When** 该 DESI 窗口解析，**Then** `InMemoryCataLocator` 照样定位到文件，
   元素级引用闭包照常展开。
3. **Given** 同上，**When** 看 Catalogue 相位，**Then** 该 dbnum **不是**相位成员，
   不建立完整应用水位——与监听限定域下 CATA 的既有行为一致（CONTEXT「监听限定域」条）。

---

### User Story 3 - 真成员出问题时照旧全停（Priority: P1，守成）

作为运维，当一个**MDB 真正声明过的**库因为身份问题（同项目重复、文件头不可读、
目录不可达）被阻断时，我希望服务**照旧全线停下来等人**，而不是带着半份目录数据
继续生成几何。

**Why this priority**: 这是本特性最容易被顺手改坏的地方。事故现场看起来像「屏障太狠」，
诱惑是去缩小爆炸半径；但 `DB_MDB::openMDB` 的证据说明 AVEVA 比我们**还狠**——
一个成员库打不开，它把已经开好的全部关掉、整个 MDB 开失败。既然收窄成员集之后
能阻断的只剩真成员，严格屏障就不再是误伤，而是正确行为。

**Independent Test**: 纯函数测——给一个 MDB 声明过的 CATA 注入身份 blocker，
断言 Catalogue 相位不就绪、Design 不被派发（即今天的行为逐位不变）。

**Acceptance Scenarios**:

1. **Given** MDB 声明的某个 CATA 同项目内有多个文件，**When** 跑一轮清单选择，
   **Then** Catalogue 相位阻断、Design 不开始——与改动前**逐位相同**。
2. **Given** 同上，**When** 看 `/health`，**Then** `blockers` 逐条点名到 dbnum，
   人一眼看得出要去修哪个文件。
3. **Given** 一个 MDB **未**声明的 CATA 有同样的身份问题，**When** 跑一轮清单选择，
   **Then** 它压根不是成员，一个 blocker 都不产生（这正是 US1）。

---

### Edge Cases

- **SYS meta 还没解析过（冷启动）**：MDB 表压根不存在 → 与 DESI 名单今天的降级同款：
  本期只解析 SYS meta 并留一句告警，把名单的来源先建起来，下一轮才有真范围。
  **不得**把「读不出名单」等同于「声明了零个 CATA」。
- **MDB 存在、但 CURD 里确实一个 CATA 都没声明**：这是合法状态（纯设计项目）。
  Catalogue 相位成员数为 0、直接就绪，Design 照常推进，并留一句与 DESI 那句同形状的告警。
- **DICT 不在本特性范围内**：它在 `COLD_START_DB_TYPES` 里，MDB 名单本身就存在它里面，
  圈它等于自锁。DICT 的候选与选主逻辑一字不改。
- **监听限定域（`watch_dbnums`）已开**：CONTEXT 已规定此时不建立全量 Catalogue 批次，
  顺序是 Meta → Design（内部先完成该窗口所需的 CATA 引用闭包）→ Model。
  本特性 MUST NOT 改变这条路径的行为。
- **MDB 声明了某个 CATA、但目录里一个文件都找不到**：沿用「声明了却没导入」的既有
  语义（`initialization_required` 那一档），不因本特性变成静默跳过。
- **抽取家族**：MDB 声明的是裸 dbnum，一个 dbnum 下的父层 + `_NNNN` 叶子仍由
  `collapse_extract_families` 归并成一个逻辑库——这一层与 core.dll 的抽取链同构，
  本特性不碰。

## Requirements

### Functional Requirements

- **FR-001**: `UpdateScope` MUST 额外解出本期 MDB 声明的 **CATA** 库号集合，
  取值口径与 DESI 完全一致（同一句 `MDB_DBNOS`，只换 `$db_type` 为 `DBType::CATA`），
  并 MUST 在同一次查询往返里取回，不得为此多开一趟 SurrealQL。
- **FR-002**: `UpdateScope::admits` 的 CATA 分支 MUST 从「无条件 `true`」改为
  「在本期 MDB 声明的 CATA 集合里」。SYS meta（`COLD_START_DB_TYPES`）
  与 `unrestricted` 两条既有豁免 MUST 原样保留。
- **FR-003**: `catalogue_manifest_for_dirs` 累加 `by_type` 时 MUST 过同一道 CATA
  范围门。这是**必需的**而非重复防御：blocker 产生在这里，而这里今天连 `UpdateScope`
  都拿不到。
- **FR-004**: 同一趟遍历里的 `dependency_identities` MUST NOT 被这道门收窄。
  按需引用闭包的定位索引保持全量。
- **FR-005**: 被 CATA 范围门排除的库 MUST 进**自己的一个聚合桶**，说自己的话，
  与 MDB DESI 范围判定、监听限定、调试限定三句**两两无交集**
  （沿用 `skip_reason` 与三桶聚合的既有纪律，issue #10）。
- **FR-006**: 冷启动降级 MUST 三分：MDB 表为空 → bootstrap 告警（只解析 SYS meta）；
  MDB 存在但未声明 CATA → 空成员 + 告警，Design 照常推进；
  MDB 名字打错 → 照旧上抛配置错误。**MUST NOT** 把前两者合并成同一句话。
- **FR-007**: 范围缓存（`SCOPE_CACHE`、`invalidate_scope_cache`）MUST 同时覆盖
  DESI 与 CATA 两份名单——它们来自同一次查询，不得出现一份新一份旧。
- **FR-008**: `/health` 与手动 preview / execute 回执 MUST 报出本期声明的 CATA 数量，
  与既有 `declared_desi()` 同一处出口。
- **FR-009**: 监听限定域生效时的既有路径 MUST 逐位不变（不建立全量 Catalogue 批次）。
- **FR-010**: ADR-025 的相位屏障语义 MUST NOT 被本特性放宽。真成员的身份阻断照旧
  阻断相位与其后继相位——`DB_MDB::openMDB` 的 fail-fast + `internalCloseMDB` 回滚
  是外部权威（宪法「外部权威」条）。本特性改的是**谁算成员**，不是**成员失败怎么办**。
- **FR-011**: 本特性 MUST NOT 收窄或改动 DICT 的候选与选主逻辑。
- **FR-012**: 每条改动 MUST 配一条「回退到旧写法就会红」的纯函数回归测试（宪法 VI）。

### Key Entities

- **MDB 声明的 CATA 集合**：`(select value DBNO from CURD.refno where STYP = 2)`
  的结果。它是「哪些目录库参与本期 Catalogue 相位」的**唯一定义**，
  与「哪些目录库能被引用闭包找到」是两回事。
- **相位成员（Phase Member）**：会被排进数据批次、必须在下一相位开始前收口、
  且其阻断会影响相位就绪的库。本特性收窄的就是 CATA 的这个集合。
- **依赖身份清单（Dependency Identity Manifest）**：`dbnum → (project, db_type, path)`
  的全量定位索引，喂 `InMemoryCataLocator`。本特性 MUST NOT 收窄它。
- **范围桶（Scope Bucket）**：一句「这些库本轮没进来，因为 X」的聚合日志。
  现有三个（MDB DESI 范围 / 监听限定 / 调试限定），本特性新增第四个（MDB CATA 范围）。

## Success Criteria

- **SC-001**: 现场那台四项目机器上，`dbnum=7000` 不再产生任何 blocker，
  `/health` 的 `initialization.blockers` 为空，`status` 到 `model_ready`。
- **SC-002**: 同一台机器上 `[manifest] … 依赖身份清单：N 个文件` 的 N 与改动前**相同**。
- **SC-003**: 关掉 `catalogue_project_priority` 整键，SC-001 仍然成立
  ——证明冲突是被消灭而不是被仲裁掉的。
- **SC-004**: MDB 声明的每一个 CATA 都仍进入 Catalogue 相位并建立完整应用水位；
  一个都不少（逐 dbnum 对拍）。
- **SC-005**: 一个引用了「MDB 未声明的 CATA」的设计元素，其几何生成结果与改动前
  **逐字节相同**（取一个已知含跨项目目录引用的生成根做对拍）。
- **SC-006**: 冷启动（空 SurrealDB 命名空间）跑一轮，日志里出现的是 bootstrap 那句话，
  **不是**「MDB 未声明 CATA」那句；第二轮跑出真范围。
- **SC-006b**: MDB 声明过的某个 CATA 注入身份 blocker 时，Catalogue 阻断、Design 不开始
  ——与改动前逐位相同（严格屏障没有被顺手放宽）。
- **SC-007**: 把 FR-002 的 CATA 分支改回无条件 `true`、把 FR-004 的定位索引也一起收窄、
  或放宽 FR-010 的相位屏障，三者各自对应一条回归测试变红。

## Assumptions

- MDB 的 CURD 用 `STYP` 区分库类型，`DBType::CATA = 2`（`aios_core::rs_surreal::mdb`）。
  取 CATA 名单与取 DESI 名单是同一句 SQL 换一个绑定值。
- 本期 MDB 是从**主项目**的 SYS meta 里读出来的；它声明的库号即是本命名空间的权威成员，
  这正是 core.dll `DB_System::getAllDbs()` 的对应物。
- 跨项目同号在收窄之后**仍可能发生**（MDB 声明了 7000，而两个项目都有 7000 文件）。
  那时才轮到已经落地的项目顺序兜底。本 spec 不改那条规则。
- 本特性**不**把状态层改成 `(project, dbnum)` 键——ADR-025 已否决，改动面覆盖水位、
  队列、PE 聚合与清库语义。
- 「缩小相位屏障爆炸半径」这条路**已被显式否决**，理由是外部权威：
  `DB_MDB::openMDB` 对成员库失败的处置是 fail-fast + `internalCloseMDB` 全部回滚，
  AVEVA 比本仓今天**更严**。事故现场之所以看起来像屏障太狠，是因为把一个非成员
  （目录扫描artifact）算成了成员；成员集修对之后，屏障就是正确行为。
  真要放宽，得先改 ADR-025 并给出与 core.dll 不一致的正当理由，不在本特性范围内。
- `catalogue_project_priority` 保留为可选覆盖层。ADR-016 让 `project_dirs` 与
  `included_projects` 按下标一一对应，所以「靠重排 `included_projects` 换优先级」
  的代价高于保留这个键。
