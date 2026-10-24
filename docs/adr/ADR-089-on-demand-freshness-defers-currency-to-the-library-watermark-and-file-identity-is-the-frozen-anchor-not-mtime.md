# ADR-089：按需显示的新旧判据听库级承诺——回执只证「同一世代、没落后」，模型输入文件的身份是冻结锚不是 mtime

状态：**草案（待拍板）**（2026-10-24。GitHub #64 收尾时顺带看到「E3D 登录即前移 DESI 会话 → `/model/generate` 把请求的根整根重算」，
用户「继续下一步」选了「开票 + ADR 草案」；票是 GitHub #65，本文随它一起提交。D1–D6 是提议，每一条都等用户过目；
「仍开着」里的 Q1–Q5 是要拍的板。**号到提交时复核**——087 被 `.mesh` 原子重写的拟稿占用、088 已用，本文取 089。）
日期：2026-10-24
落地：**未实施**。要动的代码：`data_interface/room_read_through.rs`（`target_file_current` / `target_current`）、
`data_interface/on_demand_model.rs`（`memory_root_current` 与读穿形态的 `cache_current`）、`fast_model/e3d_model_service.rs`
（`ModelFileTarget.digest` 的记法、`model_file_identity_digest` 的调用点、`same_file_target`）、`data_interface/frozen_source.rs`
（锚的导出）；没有新表、新列、新开关，`RootReceipt` / `ModelTarget` 的 wire 形状不变。
关联：
- **修订** ADR-079 D6（按需 `ensure` 的凭证判据）：「摄入形态下仍与投影回执那一半相与」里回执那一半的**问法**——从「回执会话 == 文件最新
  ∧ mtime 摘要相同」改成「同一世代 ∧ 回执会话 ≤ 文件最新」；新旧由库级承诺那一半独自回答。库级半边的单调判据（ADR-054 实施约束 4）不动。
- **修订** ADR-066 D4 / ADR-080 实施约束 11（「设计窗之外每一格相同」）：目录项「相同」的尺子从 mtime 摘要换成冻结锚；尺子换了，判据的意思不变。
- **修订** ADR-073 房间点查的来源核对（`room_read_through::lookup` 里 `selected.source.sesno == before.design.session`）：同 D1 口径（D6）。
- **不动** ADR-079 D3（`advance` 根什么都不做，水位是承诺）——本文正是让这条承诺在按需显示上真的算数；ADR-077 D5 / token 摘要（初始化回执不进投影，
  `frozen_file_digests` 记 `sha256(commit_token_json)`）——D2 是把它的思路推到按需 / 批次那一路，不是改它；ADR-086（显示按祖先查 records，生成只调
  `/model/generate`）；ADR-082（overlay 逐出）。
- 事实来源：GitHub #65（复现步骤与数）、#64 收尾评论；`changelog.md` 2026-10-24 #64 条「顺带记下」那句「目录项摘要走的是冻结 token 摘要」
  **是错的**，本文§背景更正。

## 背景

### 两个判据、一个承诺，互相否决

按需显示 `/model/generate`（`force=false`）判「这根还新不新」有两半（`on_demand_model::ensure_exact_generation_root_inner`）：

1. **库级承诺**（ADR-079 D6，`model_coverage::root_credential_covers`）：`dbnum_watermark.model_sesno ≥ 文件最新 ∧ 库级目录落款 ≥ 各目录文件最新
   ∧ 账本里这根没悬着失败`，或者账本里这根按需生成到了不早于文件最新的一版。**单调**：水位只在本窗的根全部落库时才盖（ADR-072 D4），所以它盖到哪一版，
   那一版上每个根都已发布。
2. **投影回执**（`memory_root_current` → `room_read_through::target_current(receipt.model_target)`）：回执里的设计项与每个记了会话的目录项都要
   `latest_sesno == file.session ∧ digest_now == file.digest`（`target_file_current`），外加模板 `attlib.dat` 摘要。**等值**。

摄入形态 `cache_current = ① ∧ ②`；读穿 / 只写内存形态只问 ②（plan 2026-09-06 §8 R2：那个库在 rocksdb 里还没准备好）。

而窗口规划器（ADR-079 D3，`window_root_plan::plan_window_roots`）把没被波及、两端都是根的根放进 `advance`，**什么都不做**：「这份几何到 T 仍有效，
由任务收尾盖到 T 的 `model_sesno` 一并承诺」。回执留在 S。于是任何一窗落地之后，这个库里每个未触根都是「回执 S < 文件最新 T」，② 判假，
① 算出来的真被 `∧` 掉，按需显示整根重算——正是 ADR-079 D3 说要省掉的那笔。② 在重复回答 ① 的问题（「到最新一版还新不新」），而且回答得比 ① 差：
它只认识自己这一根、不知道中间的窗口有没有碰到它，只好把「文件前移了」当成「我过期了」。

### E3D 对文件做的事（夹具 8023，2026-10-24 实测，#65）

- 一个**只读**的 TTY 会话（打开 /ALL、`Q POS`、`QUIT`，没有 SAVEWORK）把 `ams000` 下被 MDB 打开的 **30 个库文件的 mtime 全部改写**：
  目录库 5052/5053/5054/6890/7000/73xx、`ams7741/7742`、`ams1112` 在 240 ms 内逐个刷新（21:15:04.958–05.196），设计库 7997–8000 与 `amssys` / `amscom`
  在 6 s 后刷新（21:15:11.3）。上一轮（20:45）同一形状。
- 同一个会话给 `ams8000_0001` 写了会话 701，内容只有 `DBMDAT`；watcher 批次 `db-20261024-211524-000008` 700→701：`candidates=1 … no_model=1`、
  `touched=0 regen=0 advance=953 lazy=0`、`units=0`。每次登录 / 退出都这样，这也是今天每次 SAVEWORK 都是「两个会话」的来历。
- 目录库也收只有记账元素的会话：`/health.catalogue_reconcile` 报 `5052 189→190`、`6890 279→280`、`7000 263→274`，每一窗变动元素都只有
  `DBMDAT×1 DBMETA×1`。
- 于是：21:14 两根（EQUI `24384/24776` 不读目录；BRAN `24384/23257` 读 341 行目录）都 `AlreadyAvailable`（14 / 16 ms）；21:15 只读登录一次；
  21:16 两根都 `Generated`（447 / 2158 ms，后者 1816 ms 在重解 341 行目录依赖），日志 `[模型目标核对] DESI 8000 已过期：会话 700 → 文件最新 701`。几何一个数都没变。

### 身份层的同一根病：摘要里有 mtime

`model_file_identity_digest(path, session) = sha256(规范路径, 文件长度, mtime 纳秒, 会话)`。它被三处当成「这个文件还是那个文件」的尺子：
`target_file_current`（设计项与目录项）、`same_file_target`（刚体前移 / 单元级发布的「设计窗之外每一格相同」，比目录项）、`target_current`的模板项。
E3D 每次会话改写文件头，内容没变、摘要已变。对设计项它被会话等值挡在前面，所以日志里从没出过「摘要 … → 现算 …」那一行；对目录项，
有目录依赖的根在上一次 regen 与这一窗之间必然隔着至少一次 E3D 会话（窗口就是它造的），目录项摘要必然不等，便宜路对它们预计永远回退整根
（回退理由会是 `catalogue CATA 7000: session 274 -> 274, digest … -> …`）。#64 里 EQUI 走成 `transform` 是因为它的回执 `catalogue` 为空。

**更正**：changelog #64 条说「目录项摘要走的是冻结 token 摘要」——不对。`frozen_file_digests` 只有 `from_initialization_sources` 填；`from_current` /
`from_frozen_sources`（regen、刚体、单元级三条路都建在它上）的 `model_target` 对每一项都现算 `model_file_identity_digest`。初始化那一路用 token 摘要是有意的
（`initialization_keeps_the_token_digest_so_appends_do_not_change_the_target`），和本文要做的是同一件事的两半。

### `FrozenSource` 已经有一个对的身份

`data_interface/frozen_source.rs` 的 token（`SourceCommitTokenV1`）记的是：稳定文件 id（Windows `volume serial + file index`）、`dbnum` / `db_type`、
目标会话、该会话的 **session page / index root 及二者的 sha256**（`TargetAnchorV1`）。`reopen_verified` 接受追加的会话、拒绝换文件 / 回拨 /
锚变——正是「同一世代」该有的定义，和 mtime / 长度都没关系。dabacon 里一个已提交会话的页从不改写，所以锚一旦算出就是不变量。

## 决定（草案）

### D1　回执半边只证「同一世代、没落后」，新旧听库级承诺

`target_file_current(file, latest_sesno, identity_now)` 改成
`identity_now == file.digest ∧ (!file.session_tracked() || file.session ≤ latest_sesno)`。设计项、目录项同一条。

- 「落后」（`session < latest`）**不是过期**：中间的窗口有没有碰到这根，是水位说的——水位盖到 T 就是「T 以前每一窗被碰到的根都重算过了」，回执还在 S
  的根正是没被碰到的那些（被碰到的根 regen / transform / 单元级发布都把回执推到 T；重算失败的根在账本里悬着，`root_credential_covers` 一票否决）。
- 「超前」（`session > latest`，文件被回拨）**是过期**，与 ADR-054 实施约束 4 同口径（回拨是文件异常，由重建清）。
- `memory_root_current` 的形状不变：有回执 ∧ `target_current`。摄入形态的 `cache_current` 本就是 ① ∧ ②，改完之后 ② 不再重复回答 ①，两半各答各的。
- `[模型目标核对]` 日志分两种措辞：「身份变了（换文件 / 锚变）」与「回拨」；「落后且库未追平」不再是回执半边的话，由库级半边在自己的日志里说。

### D2　模型输入文件的身份 = 冻结锚

`ModelFileTarget.digest` 改记 `hex(sha256(file_id ‖ dbnum ‖ db_type ‖ session ‖ session_page_sha256 ‖ index_root_sha256))`，由
`FrozenSource::capture_at(path, session)` 算（新增一个导出，比如 `identity_digest()`；字段仍是私有的，调用方拼不出锚）。不记长度、不记 mtime。

- 追加会话、改写文件头（E3D 每次会话干的事）都不改身份；换文件（file id 变）、同号会话内容不同（锚变：另一个工程的同号会话、或 #45 那种会话重编）才改。
- `target_current` 现算的是 `capture_at(path, file.session)` 的锚，不再 stat mtime。代价是一次 `session_chain` 遍历 + 两页 sha256；锚是不变量，进程内按
  `(file_id, session)` 缓存后是 O(1)。
- 字段名不改（`digest`，serde 形状不动）。老回执里的 mtime 摘要和新算的锚永远不等 → 那根重算一次——和今天每次登录都重算比，不是退步。
- `template_attlib_digest` 不动（模板是静态文件，mtime 记法在那里是对的）。初始化的 `frozen_file_digests`（token 摘要，房间输入缓存的键）不动（ADR-077 D5：那些回执不进投影，两套摘要不会被放在一起比）。

### D3　设计窗之外的核对按锚比

`same_file_target`（刚体前移 `rigid_motion_target_at`、单元级发布生成前 / 后两道）的目录项比 `dbnum / db_type / session / digest(锚) / 路径`，意思不变：
「目录依赖含会话相同」。改写文件头不再是「变了」。目录真换了一版不在这道核对里（`hydrate_published_dependencies` 本就把目录会话按旧回执钉住），
由目录版本核对（ADR-079 D5）排工重算被命中的根。
设计项照旧只比库号 / 类型 / 路径（`same_file_path`，#64）。

### D4　`advance` 根仍什么都不做（重申 ADR-079 D3）

不在窗口落地时改写未触根的回执。`RootReceipt.source_sesno` / `model_target.design.session` 继续是「这份几何是按哪一版算的」（房间计算、
`/model/records` 的 `source_sesno` 都读它）；「到哪一版仍有效」是水位。两件事两个字段，不合并。

### D5　读穿 / 只写内存形态问同一枚水位

`ensure_exact_generation_root_inner` 在读穿 / 只写内存形态下 `cache_current = memory_root_current ∧ 库级承诺`——水位行在 mem 存储里同样由任务收尾写（ADR-074 /
ADR-083：存储模式只分 mem / rocksdb，水位写法一样），R2 当初不问它是为了不碰 durable pending 与初始化门，不是因为水位不在。水位行不存在 /
为 0 的库（从没追平过）退回今天的等值判据，不让任何库比今天更越权。

### D6　房间点查同一口径

`room_read_through::lookup` 里 `selected.source.sesno == before.design.session` 改成 `≥`，配合 D1 的 `target_current`；`RoomNotReady("model source has
changed; regenerate model")` 只在身份变了 / 回拨时报。登录之后房间点查不再把每个根都当成「来源变了」。

## 为什么不是别的做法

1. **窗口落地时把全库 `advance` 根的回执 `design.session` 推到 T**（ADR-080 D10 那条空 delta 只盖章的思路推广到整库）。每窗 O(根数) 次回执写
   （这台夹具 953，AMS 12 万）；回执从此说不清几何是按哪一版算的（D4）；而且只改会话不改摘要，mtime 那一刀照样落下，目录项也没救。
   D10 自己不在此列：它盖章的是**被碰到**的根（有候选归到它名下），那一章是说「这根我审过了」。
2. **`target_file_current` 改成「回执会话 ≤ 最新 ∧ 中间窗全部 no_model」**。要一份按库的窗口历史；而水位已经是那份历史折起来的样子（「到 T 为止被碰到的都重算了」），
   而且比「全部 no_model」宽：中间窗碰到了别的根也不关这根的事。这条路做到底就是 D1。
3. **让 E3D 不写会话**（MDB 改 deferred / 只读打开）。不在本仓手里；文件头改写与否也不由我们定；而且带编辑的窗口对其余根是同一回事，登录只是暴露它的方式。
4. **只去掉 mtime、保留长度**。长度随每次追加会话增长，登录写的 DBMDAT 会话一样让它变；只用 file id 又看不见「同一个 inode 被原地写成另一份」。锚两者都管。
5. **把目录项从回执里拿掉、只靠库级目录落款**。便宜路靠回执里的目录项证「这根的目录输入在 S→T 没动」（ADR-066 D4），库级落款是整库一份、没有逐根的话。不动。

## 后果

- 登录 / 任何窗口之后，未触根的按需显示回到 `AlreadyAvailable`（毫秒级）；被碰到的根照旧由窗口重算。这台夹具上是 447 / 2158 ms → 14 / 16 ms。
- 便宜路（刚体 / 单元级）对有目录依赖的根有机会走成（今天还是预测，待 Q4 真机）。
- 「按哪一版算的」（回执 `source_sesno`）与「到哪一版仍有效」（水位）分开读；`/model/generate` 回执的 `root_summaries[].source_sesno` 对未触根会小于文件最新，
  这是对的，不是 bug（可选：多一格 `current_through` 把水位也写出来，供 plant-ui 显示）。
- 老回执（mtime 摘要）在升级后第一次显示时重算一次，之后不再。
- 文件被回拨到更早的会话：回执「超前」→ 过期 → 重算（与今天一样）。文件被换成另一个工程的同号会话：锚不等 → 过期（今天靠 mtime 碰巧抓到，以后靠锚）。
- 多付的钱：`capture_at` 一次会话链遍历 + 两页 sha256，缓存后 O(1)；省下的是每次登录后每根一次整根重算。

## 实施约束（落地时要钉的）

1. `target_file_current` 真值表单测：同锚、落后（`session < latest`）= 新；同锚、等值 = 新；同锚、超前 = 过期；锚不同 = 过期；字典项（会话 0）只比锚
   （现有 `dictionary_targets_are_judged_by_digest_alone` 改口）。
2. `memory_root_current` × 库级承诺的组合测试（摄入形态）：回执 S、水位 T、账本安定 → `AlreadyAvailable`；回执 S、水位 S（watcher 还没追平）→ 重算；
   账本悬着失败 → 重算。读穿形态同一张表 + 「水位行不在 → 退回等值」。
3. `same_target_outside_the_design_window`：「只改文件头 / mtime 不算变」「换 session page 算变」「目录会话前移仍拦且点名」（现有 `the_windows_verbatim_prefix_is_not_a_target_difference`
   那组里加）。
4. 源码钉（本仓惯例 `include_str!`）：`model_target` 与 `target_current` 里不再出现 `model_file_identity_digest(`，只有模板项还用它；`target_file_current`
   里没有 `== latest_sesno`。
5. `FrozenSource` 的锚导出不暴露字段（只给摘要），`reopen_verified` 的语义不动；锚缓存按 `(file_id, session)` 键，换文件自然未命中。
6. 真机验收（夹具 8023）：复跑 #65 步骤 1–3，步骤 3 两根 `AlreadyAvailable`；再跑 `equi_apply.mac`（只动 EQUI A），BRAN B 仍 `AlreadyAvailable`、A
   `kind = transform`；房间点查登录后不报 `model source has changed`。
7. plant-ui 不改（字段和形状都没动）；若加 `current_through`，另起一条。
8. changelog 条里带上对 #64 条「顺带记下」的更正。

## 仍开着（待拍板）

- **Q1** 身份用锚（D2，严、多一次会话链遍历）还是只用 `file_id + dbnum/类型 + 会话`（便宜，看不见原地改写）？推荐锚。
- **Q2** 读穿 / 只写内存形态的水位可得性（D5）要在那个形态的夹具上证一次；证不出就只落 D1–D4，读穿照旧等值。
- **Q3** 目录库的记账会话让 `catalogue_reconcile` 每轮重报同三个库（5052 / 6890 / 7000，都是 `DBMDAT×1 DBMETA×1`）——`stamp_only` 有没有把库级落款推上去？
  不在本文范围，要不要另开票。
- **Q4** 便宜路对有目录依赖的根「永远回退」是预测：选一个带 NOZZ 的 EQUI 做刚体平移，修前看回退理由是否点名 `catalogue … digest`。要写夹具工程，等用户点头再跑。
- **Q5** D6（房间点查）随本文一起落，还是单开？推荐一起（同一个判据、同一个文件）。
