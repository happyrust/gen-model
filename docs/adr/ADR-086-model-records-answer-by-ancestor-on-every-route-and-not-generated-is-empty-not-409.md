# ADR-086：`/model/records` 在每条路由上都按祖先回答；没生成过是空结果，不是 409

状态：**已接受**（2026-10-09 grill-with-docs，plant-ui ADR-0030 的服务端半边。用户：「显示节点的模型，默认都是使用查询
inst_relate 的 anc 索引」「模型生成就是直接调用模型生成的接口，不需要额外的接口」。本文第一稿提过的 `/model/roots`
新接口与按根报状态已撤。**号到提交时复核**。）
日期：2026-10-09
落地：分支 `codex/model-roots-and-record-status`（分支名沿用第一稿，不改），工作树 `D:\work\plant-code\old\.scratch\dqo\gen-model-refactor-model-roots`
——放在 `.scratch\dqo\` 下是因为 `handlers.rs` 用 `include_bytes!("../../../plant-ui/web/public/pkg/plant_ui_app*")` 内嵌 plant-ui 的
wasm 包，主 plant-ui 工作树里那套文件已被另一会话改名（未提交），旁边放一个 `plant-ui` 目录联接（→ `plant-ui-release`）才编得过。
关联：
- plant-ui ADR-0030：客户端显示动作不再调 `model/ensure`，只按祖先查 records。
- `handlers.rs` 的 `model_records`（内存路由 `store.receipt(root)` → 409 `not_generated` 那一段）、
  `fast_model/model_record_reader.rs` 的 `durable_scope_reads`（`anc CONTAINS <根>`，任何祖先都行）、投影 overlay 的
  `overlay_records_of_root`（只按精确生成根取）。

## 背景

`/model/records` 今天两条路由两种脾气：rocksdb 路由走 `durable_scope_reads`，给任何祖先都能把子树里已生成的实例查出来；
内存投影路由先查 `store.receipt(root)`，目标不是精确生成根（ZONE 永远不是）就 409 `not_generated`。
客户端此前靠先 ensure 拿回生成根清单来迁就后者；plant-ui ADR-0030 之后显示不再 ensure，点下去的树节点就是查询范围，
内存路由必须也能按祖先回答。另外，「没生成过」在这条接口上是一个错误码，和「服务挂了」「库解不出」混在一起，
客户端分不出来。

## 决定

### D1　内存投影路由也按祖先回答

`model_records` 对走内存路由的库：以请求里的每个根为**祖先**，从投影里取 `anc` 含它的全部实例（含精确等于它的）；
不再要求该根有回执。rocksdb 路由不变。两条路由对同一个祖先给出同一口径的结果。

实现上按节点与生成根的相对位置分三条路，三条路的 `owner` 都是生成根：节点自己有回执 → 精确根（原路）；节点在根之上
（ZONE / SITE）→ `ModelMemoryStore::receipt_roots_under` 展成名下盖过章的根逐根取；节点在根之下（BRAN 里的一个 VALV，
模型树上任何一行都能点眼睛）→ `receipt_root_above` 找到头上那根，`overlay_records_under_node` 按祖先链只留它名下的记录。
三条都落空就是空结果。

### D2　没生成过是空结果

请求里的根在投影 / 库里一个实例都没有时，`records` 为空、HTTP 200，`sources` 照旧按库给。409 只留给真正的冲突
（该库正被整库任务占用，`conflict`）；库解不出、身份不对等照旧 4xx。`not_generated` 这个错误码退役。

### D3　回执形状不变

不新增字段，不报每根状态，不带凭证。客户端要知道「新不新」靠人，要「生成」靠右键调 `/model/generate`。

### D4　`/model/ensure` 改名 `/model/generate`，旧路径留一个版本的别名

用户：「现有 /model/ensure 不要使用 ensure，而是改成模型生成的接口，generate 即可」。路由新增 `POST /api/v1/model/generate`，
请求体、回执、状态名（`AlreadyAvailable / Generated / NoRenderableGeometry`）与 `force` 语义都不变——它本来就是生成接口，
改的是名字说的事：客户端调它就是要生成，不是「确保有」。`/api/v1/model/ensure` 作为别名指向同一个 handler，
保留一个版本，每次命中打一行 `deprecated: use /api/v1/model/generate`，下一版删。文档、日志、`[model.worker.*]` 的
措辞里 ensure 换成 generate。

## 为什么不是别的做法

- **新增 `/model/roots` 只解根不生成 + records 按根报状态与凭证**（本文第一稿）：用户否了——生成接口已经会解根，
  显示有 `anc` 索引，两者都不需要第三个东西。
- **保持内存路由只认精确根，让客户端先 ensure 拿根清单**：就是 ADR-0009 的形状，显示会改模型库、会撞 120 s。
- **records 对未生成根继续 409、客户端把 409 当空**：plant-ui 过渡期确实这么兼容（ADR-0030 后果 6），但错误码表达
  「没有」是错的，而且和 `conflict` 同一个状态码，日志里分不开。

## 后果

1. 内存投影路由对大祖先（整 SITE）是一次投影内按 `anc` 过滤的遍历，没有生成成本；投影本来就在内存里，按根索引之外
   加一道按 `anc` 的过滤即可，不要求重建索引结构（实现时若投影记录不带 `anc`，就补上——写回执时有）。
2. **行为变化**：未生成根 409 → 200 空。实施前 `rg not_generated` 核 plant3d-web 与其他消费者有没有把 409 当分支用；
   有的话同一次改掉，并在 release note 点名。
3. 老客户端（仍先 ensure 再 records 的 plant-ui）不受影响：它们带来的根有回执，D1 的结果与原来相同。

## 实施约束

handler 真跑要一份 `AppState`（本仓 handler 测试一贯是源码钉 + 存储层单测），所以约束落在两层：

1. 内存路由按祖先回答：对一个只生成了子树里部分 BRAN 的 ZONE 问，展出的就是那些 BRAN、按打包 refno 升序；根以下的一个件
   找到它头上的根；一根都没有就是空（存储层测试 `receipt_roots_under_an_ancestor_lists_the_committed_roots_below_it`）。
   handler 里必须经 `receipt_roots_under` / `receipt_root_above` / `overlay_records_under_node` 三条路、不许再出现
   `not_generated`（源码钉 `model_records_answer_by_ancestor_and_never_say_not_generated`）。
2. 没生成过回 200 空：同上两条测试——展开为空时 handler 没有任何早退分支，走到分页就是空数组；`conflict` 仍 409（既有测试不动）。
3. 两条路由同口径：rocksdb 路由的 `durable_scope_reads` 是 `anc CONTAINS <节点>`，内存路由三条路的判据都是记录的祖先链
   （`by_ancestor` 就是同一份索引的内存形态）。没有连库的对拍用例，真机对拍记进 plant-ui 那边的 evidence。
4. 回执形状不变：既有 records 反序列化测试不动。
5. `/model/generate` 路由在、`/model/ensure` 别名在且转发不改参数（`the_ensure_path_is_an_alias_of_generate`、
   `the_ensure_handler_is_a_deprecated_alias_of_generate`）。
