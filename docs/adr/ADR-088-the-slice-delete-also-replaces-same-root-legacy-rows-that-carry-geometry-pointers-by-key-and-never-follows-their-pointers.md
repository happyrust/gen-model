# ADR-088：整片删连老生成链的同根老行一起换掉——带几何指针的 `model_sesno = NONE` 行按同键整组删，不顺它自己的指针删

状态：**已接受**（2026-10-24。W4「几何前后」在冷重启后的第一批读出库上 FTUBE 还在 2900 而 E3D 已在 U3400，用户「继续下一步修复」；
修法三选一取「整片删连老行一起圈」，不按本批 id 点删、不改初始化；镜像 spool 那一路跟上，用户「按推荐方案继续」；本文由用户点选
「补一份 ADR：整片删连老生成链的同根老行一起换掉」而写。**号到提交时复核**——087 已被 `.mesh` 原子重写的拟稿占用
（`changelog.md`「重新生成计划 S1；ADR-087 D3 拟稿」、`docs/plans/2026-10-21-model-remaining-issues-fix-plan.md`），本文取 088。）
日期：2026-10-24
落地：工作树 `D:\work\plant-code\old\gen-model`（分支 `refactor`），与同日其余工作一起**未提交**；代码全部在
`src/fast_model/model_db_adapter.rs`——`render_slice_delete`（`$slice_legacy_keys`）、`slice_sources_on`（第二句）、
`render_spooled_scope_cleanup`（`$mirror_legacy_by_anc` / `$mirror_legacy_by_owner`）与它们的测试；没有新表、新列、新开关。
关联：
- **修订** ADR-060 D4 / N3：「增量先删后插」里「先删」的作用域从「模型面自己写过的行」扩到「同根、同 id、带几何指针的老生成链行」。
  对这些 id，N3 的第二种前提（本轮先删过）此前并不成立，N1（一个元素每张表最多一行）被「同 id 的老关系行 + 模型面的同键
  `inst_info` / `aabb` / `trans` 槽位行」这一组合悄悄破了。ADR-060 §迁移 那句「`inst_relate` 会被同 id 插入接管」只在逐 id 先删的
  世界里成立，2026-09-14 之后不再是。
- **收窄** `docs/plans/2026-09-14-zone-partitioned-model-persistence-dev-plan.md` 的护栏（「`model_sesno != NONE` 是护栏：属性面独有的行
  不带这一列，谓词碰不到它们」）与验收（「属性面独有的行（`model_sesno = NONE`）在落库前后逐字不变」）：只对**没有几何指针**的行
  成立；带 `aabb` / `world_trans` 指针的同根老行在它的生成根落库时被整组换掉（D1–D3）。
- **不动** ADR-077 D4 / D5（初始化 `WriteMode::Initial` 直写、不进投影）、ADR-079（`model_sesno` 是模型面唯一的持久事实——本文只拿它
  做行的归属判据，不新增持久记录）、ADR-082（overlay 逐出 / 冷根读库——正是它让库上的真相在重启、逐出后暴露出来）、
  ADR-060 D2 / §未做（内容寻址的共享行只增不删、没有可达性 GC——老行留下的摘要键行归入同一条）。
- 同一种失效的先例：`src/fast_model/pdms_inst.rs` 顶部注释（本仓 SurrealDB fork 撞已有 id **不报错、保留旧行**，ADR-010 D13 在 8009 上
  实测；删除集没覆盖到的行「`aabb` / `world_trans` 永远停在第一次生成的值，而整条链路一声不响」）。这次是跨两条写路径的同一件事。
- 发现过程与真机数：`docs/plans/2026-10-24-increment-records-server-gaps-plan.md`「W4 顺带发现」；`changelog.md` 2026-10-24
  「顺带修掉的一条增量正确性漏洞」。

## 背景

### 三件各自成立的事，叠在一起就是一条一声不响的漏洞

1. **两条写路径共用同一套 `inst_relate` id。** 老生成链（`pdms_inst`，以及给夹具库填过数据的更老的二进制）写的
   `inst_relate:<a>_<b>` 不带 `model_sesno`，带 `generic` / `has_cata_neg` / `zone_refno` / `insts_flat` 那一套属性面列；它给生成过几何
   的元素还带两个指针——`aabb` → `aabb:⟨摘要⟩`、`world_trans` → `trans:⟨摘要⟩`，`out` → 共享的 `inst_info:⟨cata_hash⟩`（旧世界这三张表
   **按几何实例做键**，ADR-060 §背景）。模型面（`model_db_adapter`）写**同一个 id**，带 `model_sesno`，盒 / 变换 / `inst_info` 用
   **同键**槽位 `aabb:<id>` / `trans:<id>` / `inst_info:<id>`（ADR-060 D1）。
2. **增量的删是谓词圈整片，护栏按列。** 2026-09-14 起有回执的根不再逐 id 点删，`render_slice_delete` 用
   `anc CONTAINSANY [<根>…] AND model_sesno != NONE` 一次圈掉六张表（`persist_group` 对这些根改走 `WriteMode::Initial`，省掉的是
   一根两千件时四个各两千项的 `type::thing()` 数组——AMS 8000 实测 16.7 s → 50.3 s 的那一段）。护栏的本意是**不误伤属性面独有的行**。
3. **插是 `INSERT IGNORE`，撞 id 保留旧行。** ADR-060 D4 定的；而且本仓 fork 的 `INSERT RELATION` 不带 `IGNORE` 撞 id 也是
   静默保留旧行（ADR-010 D13），插入这一侧没有「出声」的选项。

于是一个老生成链生成过几何的元素，在模型面接手它的根之后：整片删圈不到它的老关系行（`model_sesno = NONE`），模型面的新关系行
每一轮都被 `INSERT IGNORE` 挡掉；而它的同键槽位 `aabb:<id>` / `trans:<id>` / `inst_info:<id>` 在模型面第一轮时是**空的**，第一轮的
几何插了进去，之后每一轮——这个 id 没有带 `model_sesno` 的关系行，所以不在 `$slice_keys` 里——槽位既不被删、也插不进新值。
库上这一件就定格在模型面第一轮的几何；它在投影里是对的（新行先进 overlay，`/model/records` 覆盖优先），所以进程活着的时候看不出来，
重启或逐出（ADR-082）之后冷读答的是定格的那一套。

### 夹具 8023 上的样子

dbnum 8000（AvevaMarineSample，NS 1516）`inst_relate.model_sesno` 的分布：NONE 3412 / 650 92 / 673 2。BRAN `24384/23257` 名下 9 行，
7 行是老行（`generic = PIPE`、`out = inst_info:⟨1493143050250919266⟩`、`aabb = aabb:⟨10122273704659448252⟩`、
`world_trans = trans:⟨10695070650411381143⟩`），只有 2 行（`24384_23261` / `24384_23266`，烘焙 FTUB）带 `model_sesno`。
管道用例把 FTUBE `24384/23262` 从 U2900 抬到 U3400 再放回去，E3D 每次 SAVEWORK 都触发一批增量、每批都「成功」，
但库上 `aabb` 的 z 永远是 2900..2930。W4 的 `geometry_before` 在冷重启后那一批（`db-20261024-040215-000001`，670→671）读到的就是它：
「前」2930、「后」3430，`diff.moved = 7`——库上老行与投影新行的盒每一件都差几毫米，7 件全算「动了」。

修法的第一版只把老关系行并进删除、不删同键槽位，真机上露出第二层：`db-20261024-111914-000001`（676→677）之后 9 行全带 `model_sesno = 677`
（NONE 3412 → 3405），FTUBE 的关系行落进来了，可它指着的 `aabb:24384_23262` / `trans:24384_23262` 还是第一轮留下的 2900
（`.scratch/fixture-8023/incr2/durable_probe_after_apply5.json`）。所以是**两层**：老关系行挡住新关系行；同键槽位里留着第一轮的几何。
两层都要删，而且删的是同一套键。

## 决定

### D1　同根、带几何指针的老行并进整片删的键集

```sql
LET $slice_model_keys  = SELECT VALUE record::id(id) FROM inst_relate
                         WHERE anc CONTAINSANY [<根>…] AND model_sesno != NONE;
LET $slice_legacy_keys = SELECT VALUE record::id(id) FROM inst_relate
                         WHERE anc CONTAINSANY [<根>…] AND model_sesno = NONE
                           AND aabb != NONE AND world_trans != NONE;
LET $slice_keys = array::union($slice_model_keys, $slice_legacy_keys);
```

判据两条，缺一不可：

- **同根**：`anc` 里有本组一个**有回执**的生成根。有回执的根，模型面手里的是这根的全集（`RootShare::reconciliation_ready`），
  名下一件元素要么这一轮重新投影出来、要么它确实没有几何——两种情况下老行都不该再对外。没回执的根不敢推整片，老行也照旧不动。
- **带几何指针**：`aabb` 与 `world_trans` 都不是 NONE。这就是模型冷读看得见一行的条件（`aabb.d != NONE AND world_trans.d != NONE`），
  所以圈到的恰好是「读者可能当成几何读到」的那些老行，一行不多。

### D2　按同键整组删；老行自己的指针**不顺着删**

老行的键与模型面的键走同一套点删——关系行本身，加上**同键**的 `inst_info:<key>` / `aabb:<key>` / `trans:<key>` 与按 `in` 找到的
`geo_relate`。同键的那三张表是模型面自己的槽位（ADR-060 D1）：它们要是存在，就是模型面写的，装的是被挡住的那些轮次里的第一轮几何；
不删就是把它们认成新行（第二层）。

老行自己指着的 `aabb:⟨摘要⟩` / `trans:⟨摘要⟩` / `inst_info:⟨cata_hash⟩` **不删**。它们按摘要另键、在旧世界里按几何实例共用，别的根名下
还没被模型面接手的老行可能正指着同一行；顺指针删就是替邻居拔掉几何。它们留下来是不可达的死行，与 ADR-060 D2 / §未做 对内容寻址行的
态度一致：没有可达性 GC，代价只有空间。源码钉：语句里不许出现 `$slice_legacy_keys.map`、不许出现
`SELECT id, aabb, world_trans FROM inst_relate`。

### D3　没有几何指针的属性面行照旧碰不到

`generic = 'EQUI'` 之类只带属性面列、没有 `aabb` / `world_trans` 的行一字不动——2026-09-14 的护栏对它们继续成立，那条验收改写成
「**没有几何指针的**属性面行在落库前后逐字不变」。模型面从来不写这些行，读者也从来不把它们当几何。

### D4　来源名单同步：被换掉的老行的来源进清边 / 摘树

`slice_sources_on` 两句分开读（模型面的行；带指针的老行）再并——这份名单喂房间清边（`room_edges_to_clear`）与进程内空间树
（`aabb_tree::converge_tree_after_persist`）。老行被删了而它的来源不在名单里，树上就留一只幽灵盒、房间边永远指着不存在的几何。

### D5　镜像 spool / 墓碑那一路同一条规则

`render_spooled_scope_cleanup` 的作用域比整片删多一路 `in = $mirror_owner`（兜 `anc` 缺失的历史行），老行也照这个形状：
`$mirror_legacy_by_anc` / `$mirror_legacy_by_owner` 两路分开读、与模型面的两路一起 `array::union` 进 `$mirror_keys`，同一套点删，
同样不顺指针。两条写路径的清理规则不许分叉——spool 回放写出的库和在线落库写出的库必须是同一个库。

### D6　谓词形态不变：分开读、各走索引；删一律点名

老行那一句单独一条 `LET`，不与模型面那句合成 `… AND (model_sesno != NONE OR (…))`——与 `durable_records_sql` / spool 清理同一条经验：
带 `OR` 的谓词在部署引擎上退回过整表扫，而每一句单独写才每一句都 EXPLAIN 得出来。删的那几句照旧 `DELETE $数组` 点名记录、不下谓词
（`DELETE … WHERE record::id(id) IN …` 丢索引的那个坑，`render_slice_delete` 文档里记着）。

### D7　管身与初始化这次不改

- `tubi_relate` 的谓词仍是 `model_sesno != NONE`：默认构建下它没有第二个写者（2026-09-14 计划「`model_sesno` 先保留」那一段），
  而且管身的盒 / 变换按摘要另键、没有同键槽位可被占——第二层在它身上不存在。真出现老管身行再按本文的形状补，不预先写。
- 初始化那条写路（ADR-077 D4：`WriteMode::Initial`、不经整片删）不动。夹具 8023 的库是老生成链填的、之后只走过增量，
  初始化撞上老行是什么样没有验过，记在「仍开着」。

## 为什么不是别的做法

- **按本批要写的 id 点删**（ADR-060 D4 的原形）：能修，但正是 `persist_group` 给有回执的根改走 `Initial` 省掉的那笔账
  （16.7 s → 50.3 s）；而且它只覆盖本批写的 id——这一轮不再产出几何的元素，老行照样留着，就是 `pdms_inst.rs` 顶部注释写的那种漏法。
- **护栏整个摘掉**（谓词只剩 `anc CONTAINSANY`）：连没有几何指针的属性面行一起删——`has_cata_neg` / `zone_refno` / `insts_flat`
  那一套模型面从不重写，删了就没了，2026-09-14 那条验收整条作废。
- **一次性清扫 / 迁移老行**（照 ADR-060 §迁移 的 `direct_` 前缀清扫再来一遍）：那种清扫需要整库口径的「悬空」判据——
  ADR-060 在 7997 上学到「重生成一根、悬空一根」，只有整库重写完那一刻判得准——还要标记行、要决定哪些老行以后会被重生成。
  整片删在**恰好该判的那一刻**（这根有回执、全集在手）就知道答案，不需要第二条写路径，也没有「扫了一半」的状态。
- **让插入出声**（去掉 `IGNORE`、或改 UPSERT）：fork 的 `INSERT RELATION` 撞 id 不带 `IGNORE` 也静默保留旧行（ADR-010 D13）；
  模型面零 UPDATE / UPSERT 是 ADR-060 D4 / ADR-064 D6 定的口径，不为这件事开回去。
- **顺老行指针删**：见 D2——摘要键行可能被别的根的老行共用。
- **只删老关系行、不删同键槽位**（真机上试过的第一版）：第二层——新关系行落进来了却指着第一轮的孤儿盒，
  `durable_probe_after_apply5.json` 里 FTUBE 仍是 2900。

## 后果

1. **行为变化，一根一次。** 部署后每根第一次有回执地落库时，名下带几何指针的老行被模型面行换掉（8000：那一批 NONE 3412 → 3405，
   正是 BRAN `24384/23257` 的 7 行）；从此库上的模型行跟着增量走——`db-20261024-112859-000004`（680→681）起库上 FTUBE 在 3400..3430
   （`durable_probe_after_apply6.json`），冷重启后 `/model/records` 直接答 3400（`cold_records_after_fix_24384_23257.json`）。
   那一批的 `geometry_before` 读的是老几何，`geometry_diff.moved` 可能等于盒与老世界差过 0.5 mm 的全部构件（冷批 670→671 是 7）；
   之后才是真实增量（`db-20261024-113210-000001`，682→683：`moved 1`）。
2. **代价。** 每组多一句走 `idx_inst_relate_anc` 的读，与模型面那句同价；删语句数不变，键数组多出的是老行数——换掉之后它们就是模型面行，
   第二次起不再多。EXPLAIN 钉住：整片删块 13 句 / 4 条带谓词的读全走索引；spool 清理块 18 句 / 7 条。
3. **死行。** 被换掉的老行指着的摘要键 `aabb` / `trans` / `inst_info` 与它们的 `geo_relate` 留在库里不可达，不回收（ADR-060 §未做 同一条）。
4. **房间与空间树。** 被换掉的元素旧房间边随落库事务清掉、按新几何重算；树条目按来源摘掉再按新盒进树（D4）。
5. **回退覆盖面变大。** 这些元素的关系行从此带 `model_sesno`，按水位回退（`fast_delete`，`model_sesno > S`）才圈得到它们——此前老关系行对回退不可见。
6. **文档口径。** 2026-09-14 计划的护栏与验收按本文「关联」收窄；`render_slice_delete` / `render_spooled_scope_cleanup` 的文档注释已写入例外。

## 实施约束

全部在 `model_db_adapter.rs` 的测试里：

1. **语句形状**（`slice_delete_is_one_predicate_block_guarded_by_the_model_column`）：`anc CONTAINSANY [42,43]` 出现 3 次
   （模型面 `inst_relate`、老行 `inst_relate`、`tubi_relate`），`model_sesno != NONE` 2 次；`$slice_legacy_keys` 那一句逐字钉住、
   `array::union` 进 `$slice_keys`；不许出现 `$slice_legacy_keys.map`、`SELECT id, aabb, world_trans FROM inst_relate`、`inst_geo`。
2. **删了什么、留了什么**（`slice_delete_removes_every_representation_of_the_slice_and_nothing_else`，内存 SurrealDB）：
   夹具里同根老行 `inst_relate:oldgeo`（指着 `aabb:olddigest` / `trans:olddigest` / `inst_info:oldshared`）加同键槽位
   `aabb:oldgeo` / `trans:oldgeo` / `inst_info:oldgeo`；整片删之后老关系行与三个同键槽位必须走，三个摘要键行必须在，
   没指针的 `inst_relate:legacy`、邻根的全部行、`inst_geo:mesh1` / `aabb:mesh1` 必须在——幸存集合**逐项相等**。
3. **来源名单**（`slice_sources_include_legacy_geometry_rows_under_the_root`）：模型面的 7997/11 与老行的 7997/15 都是来源，
   没指针的 7997/13 与邻根的 7997/21 不是。
4. **索引**（`slice_predicates_are_planned_through_ancestor_indexes`）：13 句、4 条带谓词的读没有一条 `Iterate Table`，`predicated[1]` 就是老行那句；
   spool 这边 `spooled_cleanup_predicates_are_planned_through_indexes`（18 句 / 7 条）。
5. **spool 同形**（`a_spooled_root_is_a_complete_idempotent_replacement`）：四路 `array::union` 逐字钉住；不许 `$mirror_legacy_by_*.map`；
   只许点名删。
6. **真机**（不入库）：夹具 8023 / dbnum 8000，E3D 宏 `driver_pipe_apply.mac` / `driver_pipe_restore.mac`，
   批次 `db-20261024-111914-000001`（676→677，老行换掉）、`db-20261024-112859-000004`（680→681，库上 3400）、
   `db-20261024-113210-000001`（682→683，`moved 1`）；探针 `.scratch/fixture-8023/incr2/durable_probe_after_apply{4,5,6}.json`、
   `cold_records_after_fix_24384_23257.json`。

## 仍开着

- 初始化（ADR-077 D4）撞上带几何指针的老行是什么样，没验；按 D7 不改。要是也被挡住，第一次增量碰到那根时由 D1 收掉。
- 老行留下的摘要键死行没有回收（ADR-060 §未做）。
- `model_sesno` 仍兼两份工（护栏 + 回退水位，2026-09-14 计划）。哪天摘它，D1 的判据「`model_sesno = NONE` 且带指针」得换成新的归属列
  重新表述，不能只删那一列。
- `tubi_relate` 没有老行这一路（D7）。
