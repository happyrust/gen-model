# 方位修改触发模型增量：追踪与隔离测试记录

日期：2026-10-16。单会话工作；协作仍暂停，未重新分派任何旧任务。
用户要求：继续测试模型增量，重点是方位修改如何引起更新。

> 阶段说明：第 1–6 节为最初只读追踪记录；用户随后选择隔离合成夹具，新增测试与最新回归结果见第 7 节。此前“没有新增测试/未跑全量”等措辞仅对应第一阶段。

## 1. 第一阶段边界与结论

- 本次只读追踪代码、运行现有离线测试并记录；没有新增/修改业务源代码或测试。
- 未操作真实 E3D 模型、项目数据库、服务配置或执行 update API；真实几何旋转端到端仍未验证。
- ORI 是摆放/方位属性，影响模型，不能按业务元数据跳过；但“影响模型”不等于“必须重算网格”。
- 有效触发源应是模型元素 ORI 的实际变化并保存到源库的新会话/版本，不是相机旋转、前端临时姿态或只改派生缓存。
- 先确认项目/MDB/库处于当前服务有效范围、已有模型基线，记录源版本 S、根回执与世界矩阵。

## 2. 可复核代码链路

1. `src/data_interface/model_impact.rs:110-121`：纯变换清单为 POS/ORI。YDIR/ZDIR、POSS/POSE 等是几何定义参数，不可套用同一廉价更新结论。
2. `src/data_interface/window_root_plan.rs:926`：从真实 S/T UpdatePlan 账本取 placement_only 候选，计算锚点与属主链，再归因到受影响生成根。不能只测一个手工属性名就声称覆盖真实版本差分。
3. `src/data_interface/rigid_motion.rs:97`：已发布安定的根、有效锚点、无窗口退化、无不兼容同窗改动/级联等条件都满足才进 transform；路由/摆放派生/被属主消费的几何成员等场景要保守回退。
4. `src/options.rs:2280-2313`：开关为 model_transform_cheap_path，环境覆盖 AIOS_MODEL_TRANSFORM_CHEAP_PATH。实现当前缺省 true；首行“默认 false”是旧注释，不能据此判断运行态，也未在本次顺手修改。实际运行配置/覆盖尚未检查。
5. `src/fast_model/e3d_model_service.rs:2149`：执行体检查源基线与依赖目标，不能前移就回退；生成服务先提交内存投影，durable 发布由 worker 完成，不是函数一返回就证明全部持久化完成。
6. `src/data_interface/rigid_motion.rs:251-299`：Δ = W_T(anchor) * inverse(W_S(anchor))，world_new = Δ * world_old。刚体前移不改 mesh_id / geometry_id；旋转包围盒应从网格顶点按新世界矩阵重算，不能沿用平移的加偏移做法。已在 T 的记录重放时不能再乘一次 Δ。
7. `src/data_interface/batch_worker.rs:4432`：执行期 transform 回退的回执带 transform_fallback_reason，不应把合法 regen 回退当成“没有增量”。

## 3. 如何触发与观察（待指定隔离对象后执行）

- 对测试对象在 E3D 里实际修改 ORI，保存工作（SAVEWORK），确认源库从 S 前进到 T。
- 自动发现存在文件事件与周期重扫；`src/web_service/handlers.rs:2144` 明确手动执行相当于不等下一轮约 30 秒轮询。
- 立即触发入口：POST `/api/v1/update/execute`；预览入口：POST `/api/v1/update/preview`。没有在本次调用，不能报已入队。
- 必须先看服务 data_face：ingest 扫描并排数据批次；read-through 的 execute 则排模型/缓存核对，preview 可返回空数据批次/up_to_date，且 applied_sesno 不前进。读透下不能拿它们证明模型无变化或卡住（handlers.rs:2101-2192）。
- 观察返回任务和根回执：目标版本到 T、成功落地，kind/published_via 是 transform 或有理由的 regen；核对对应几何记录和前端显示。HTTP 202 仅表示接受，不是完成。
- 旋转可能改变房间/空间关系，应同时检查收尾与归属；水位/任务成功不能单独替代几何断言。
- 现有 `docs/evidence/2026-10-14-adr066-live2-verification-checklist.md` 是旧的 ZONE 平移计划，不是本次 ORI 旋转已验收证据；其固定对象、端口、数量不得直接套用。

## 4. 本次实际离线测试

在远端工作区运行下面五条命令，用 cmd.exe 的 && 顺序串联；实际命令 job j90 已结束，整体 exit=0。全部是本次新运行的现有测试，不是新增测试，也不是历史全量结果。

所有命令格式：`cargo test --offline --lib <filter> -- --test-threads=1`。

| filter | passed | failed | ignored | filtered out |
|---|---:|---:|---:|---:|
| data_interface::rigid_motion::tests | 26 | 0 | 0 | 2641 |
| data_interface::model_impact::tests | 50 | 0 | 0 | 2617 |
| a_placement_derived_anchor_never_qualifies_for_rigid_motion | 1 | 0 | 0 | 2666 |
| a_consumed_member_anchor_regenerates_its_owner_unit | 1 | 0 | 0 | 2666 |
| a_pure_container_anchor_still_qualifies | 1 | 0 | 0 | 2666 |

合计 79 passed / 0 failed；未跑 ignored 或全量库测试。
覆盖：属性分类、旋转调用重算包围盒分支、兄弟/嵌套锚点、幂等重放、失效基线、包围盒计算失败与保守回退等。
限制：包围盒回调与分桶测试不能代替真实源文件修改 → 差分 → 发布 → 房间收尾 → 前端的端到端链路。
构建仍有 vendor 的三个 dead_code、manifest unused cc、num-bigint-dig/wide future-incompatibility 提示，不是零构建警告。

## 5. 运行时读取的关键文件 SHA-256

| 文件 | SHA-256 |
|---|---|
| src/data_interface/model_impact.rs | 6ac7245c5638a34b60705849f9652264adcecdaa80239ebbd3e2856710550d93 |
| src/data_interface/rigid_motion.rs | f14b25cb628fd2d3a8de8295e7120a164c88b4ef2970d4357fca4cfae4375d9a |
| src/data_interface/window_root_plan.rs | 2a804efdcdb3fcb2d81a71bc36f0be50b25ea482c89dd9dd7ddd7f2e13ba76af |
| src/fast_model/e3d_model_service.rs | dbaeb6e73307791919d64f19393ed3501088635a29957b24609e90713d19fb4a |
| src/data_interface/batch_worker.rs | 8356738c2e438141c85dad0c519acf8093c8a896ce534af744850c22f25affdd |
| src/options.rs | 91714db564aec116b48b78592979af9fa87254404eb391c65a9d3a13cf79115e |
| src/web_service/handlers.rs | d7ceb3b75eed4faabdf2d82f5a1387c15f833107472ce2baf378d78116021d9c |

这些是本次读取版本，非跨会话锁；后续改动前重读。

## 6. 下一步旋转验收建议（尚未执行）

1. 在可还原的隔离对象上选非对称几何，例如长宽不同的盒/设备；记录原 ORI、POS、属主、尺寸、网格与回执。
2. 只改 ORI，围绕明确的局部轴/枢轴转 90°，不同时改尺寸/属主，保存为 T。
3. 比较增量结果与 T 端全量生成的基准：每条世界矩阵、网格身份、紧 AABB、空间/房间归属都正确；不能用“画面转了”作为唯一证据。
4. 同窗口重复提交、服务恢复或重放不应二次旋转；还原到原 ORI 必须作为新的源会话正常处理，不手改水位。
5. 再测 ORI 与尺寸同改、复杂依赖/消费成员场景，验证不能刚体前移时确实重生成，不漏更新。

待用户指定：使用合成夹具还是现有工程副本；若后者，需明确 project/MDB/dbnum/refno、旋转轴/角度、基线与还原方案。当前不据旧平移脚本推定真实写入授权。

## 7. 第二阶段：用户选择隔离合成夹具后的实际结果

用户选择 synthetic_isolated。新增 `src/fast_model/orientation_increment_regression_tests.rs`（487 行，6 项测试），仅在 `e3d_model_service.rs` 加 4 行 cfg(test) 模块接线；没有修改生产业务逻辑、真实模型、项目数据库或服务配置，也未恢复协作。

### 7.1 夹具及真实组件

- 非对称 40×20×10 盒，父级原点 (100,100,100)，子构件局部偏置 (20,0,0)。S=41，T=42；主例只改父级 Z 轴方位 0→90°。
- S/T 状态及属性差分由测试适配器提供；根集合、属主关系、touched 集和“锚点可刚体移动”事实同样是夹具输入，不是通过 E3D 二进制源文件收集。
- 实际调用 ElementDiff::is_placement_only、plan_window_roots、classify_transform_roots、republish_records、rigid_motion_mesh_bounds、ModelMemoryStore::commit 及 MemoryRoomCalculator。
- 对拍侧重新创建 T 状态的整个 Manifold 实体，独立经过生产 bake_element / project_geometry_delta，不复用增量 Δ、旧记录或旧包围盒；这是合成几何完整烘焙基准，不是 generate_subtree/目录求值源文件全量生成。
- 网格在各测试 tempfile 目录，ModelMemoryStore 和房间模型均是独立实例；不调用 global store 或真实库。

### 7.2 六项通过的行为

| 测试后缀（均带 synthetic_orientation_ 前缀） | 实际断言 |
|---|---|
| 90_degrees_matches_full_bake_and_keeps_mesh_bytes | 旋转父级带动偏置子构件；世界盒为 [90,100,95]—[110,140,105]；世界矩阵/盒对拍；原 mesh_id 与文件字节不改；内存回执到 T |
| oblique_delta_uses_tight_vertex_bounds_not_old_aabb | 30→75° 对拍，另证旋转旧世界 AABB 会产生显著过宽盒，不能替代网格顶点重算 |
| replay_does_not_double_rotate_and_restore_is_a_new_window | 重放逐记录不动、不读网格；逆旋转作为 42→43 新窗口恢复到原几何 |
| mixed_dimension_change_rejects_transform | 同改 ORI/XLEN 时 placement_only 为假、没有 transform，选根保留 regen；尺寸变化后的独立重建盒正确 |
| missing_mesh_and_stale_baseline_do_not_publish | 新于基线的记录/旋转网格缺失返回 Err；夹具在成功结果前不提交，旧内存记录/回执不变 |
| room_membership_changes_and_matches_full_geometry | 真实房间计算器：原盒与房间相交，旋转后完全离开；增量与 T 全量烘焙的边一致，逆旋转恢复旧边 |

### 7.3 新运行记录

- j129：首次 rustfmt 发现夹具迭代式括号错误，exit=1，Cargo 未运行；改成显式角点循环。
- j132：首次编译发现 Manifold 被同名内部模块遮蔽（E0603），exit=101；仅修夹具导入为外部 crate。
- 上述是新增测试自身的语法/导入问题，不是生产行为的红测或已修复生产缺陷。
- j139：`rustfmt --edition 2024 src/fast_model/orientation_increment_regression_tests.rs && cargo test --offline --lib synthetic_orientation_ -- --nocapture --test-threads=1`，exit=0；6 passed / 0 failed / 0 ignored / 2668 filtered out；测试 0.03s。
- j151：`cargo test --offline --lib`，exit=0；2542 passed / 0 failed / 132 ignored / 0 filtered out；共 2674 项，测试 15.32s。
- 全量结果对应当时整个共享工作树，包含其他工作线；与历史 2535 的差额不能全部归因于本次新增的 6 项。132 ignored 未执行、不算验收通过。
- 既有 vendor dead_code、unused cc 和 future-incompatibility 提示仍存在。
- get_problems 对新增测试和接线文件均未报告错误/警告（编辑器诊断）；get_changes 已核对接线仅 +4 行和独立新增测试文件。

### 7.4 当前测试文件版本

| 文件 | SHA-256 |
|---|---|
| src/fast_model/orientation_increment_regression_tests.rs | 6ed7695c1f41f4e6688f1c7f9ae671915ad5863ac7903be8eee8a6715bb4cb53 |
| src/fast_model/e3d_model_service.rs（仅新增测试接线后） | 94ec69747a1f7b6e8a11a4d71ddb801b91f7ea5e904674b6447d9572780f39cb |

### 7.5 尚未覆盖，不能声称端到端已验收

- E3D 二进制文件的真实 ORI 读取/S-T 差分、索引与属主闭包收集、目录/模板依赖求值。
- 自动监听/读透版本轮询、HTTP execute、真实队列/worker、model_target 依赖核对（夹具回执 model_target=None）。
- 生产刚体根执行体的全套 guard/publish 调度；夹具串联真实组件，但不是调用整个生产 executor。混合尺寸例验证分流/重建组件，非 worker 自动执行回退。
- durable write-behind、房间边持久化及水位收尾、服务重启/崩溃恢复和前端显示。
- X/Y 轴、任意轴、更多祖孙/兄弟锚点的新增综合对拍；原有单元测试不能替代这些新场景。

结论：本阶段 6 项合成组件集成测试和库级回归通过，未发现需改生产逻辑的缺陷；真实源文件到最终持久化的端到端链路仍待独立夹具或授权工程副本验证。


## 8. 真实 S/T 源文件隔离夹具（2026-10-16；当前存在 1 项红测）

本节是第 7 节之后的新结果；第 7 节的全绿基线是历史结果，不能代表当前工作区。用户选择补真实 S/T 差分输入，不使用工程副本；协同仍暂停。

### 8.1 构造及调用链

- 新增 `src/fast_model/orientation_source_diff_fixture.rs`，449 行、3 个非 ignored 测试。临时目录从零生成二进制库，未访问/复制安装工程或项目源库。
- 仅用依赖随附 `schema/e3d31/binaries/{desvir,attlib}.dat`；缺失即失败，不静默跳过。描述页/初始会话/索引骨架由夹具构造；元素布局、默认值、POS/ORI/尺寸实际编码来自 `TemplateProvider`、`RecordDraft::from_template/set_attribute/encode`。
- 每页 4096 字节；8 个真实索引键：EQUI 父级 `16192/10`、BOX 子级 `16192/11`、6 个未修改的 EQUI。1/8、2/8 的候选比例低于生产 30% 退化阈值；不是伪造扫描统计。
- S=41；实际 `write_element_session` 追加 T=42 并提交页零。保存后才打开两个 `DbSet`，在**同一文件的历史会话链**上各自 pin S/T，断言实际 pinned session。
- 真实调用 `diff_element`、`WindowRootSources::collect`（含索引差分、`plan_update`、祖先闭包、根枚举/归因）、`impact/plan_window_roots/rigid_motion`；未手工构造 ElementDiff、候选或受影响根表。
- S/T 各自通过真实 `generate_subtree_with_cache` 从源记录生成 BOX，随后调用生产 bake/投影。刚体结果与 T 端全量源生成对拍，核对世界变换、紧包围盒、mesh 标识及磁盘网格字节不变，并用房间计算器对拍。
- 仅额外增加 4 行 cfg(test) 模块接线；旧组件夹具只将 `close`、`rooms` 两个测试辅助函数改为 `pub(super)` 供新夹具复用。未改 gen-model 的生产业务逻辑、vendor 文件、服务配置或真实项目数据。

### 8.2 三个真实源测试

| 用例 | 结果 | 实际观测 |
| --- | --- | --- |
| 父 EQUI ORI 0→90° | PASS | `attributes=[ORI]`、非 opaque、placement-only；BOX 本地记录不变而世界坐标变化；仅父生成根受影响；刚体分桶 1 根，与从 T 源生成的结果一致 |
| 父 ORI + 子 BOX XLEN 40→60 | PASS | 父差分为 ORI、子差分为 XLEN；同一根不能走刚体分桶，保留 1 根 regen；T 生成 AABB 为 `[90,90,95]–[110,150,105]` |
| ORI 0→0 的同值保存 | **FAIL（保留红测）** | 属性、类型、属主、成员及隐式属性区均未变；仅物理成员块地址变动，却得到 `opaque=true`、父根进入 regen 计划 |

### 8.3 同值保存误报的证据与边界

j173 和 j174 均输出：

```text
ElementDiff { refno: "16192/10", noun: "EQUI",
  type_changed: None, owner_changed: None, members: None,
  attributes: [], opaque: true }
relocated_header_words=[(8, 3, 11), (9, 352257, 278529)]
touched={Refno(16192_10)}; regen=["16192/10"]
```

- 红测在失败前独立断言：noun、owner、members、explicit_attributes 相同，隐式数据长度相同，`implicit_data[44..]` 相同，实际 local placement 相同；所有原始字差异只允许落在头部 6..=9，实际仅为 8/9。因此不能将失败归因为夹具偷偷改变了 ORI 或几何。
- 读取依赖确认：`../vendor/e3d-model/src/element_diff.rs:230` 将完整 `implicit_data` 纳入原始相等门；属性/属主/成员均无可见差异而记录原始内容不同后，后续分支置 opaque。`../vendor/e3d-io/src/write/record.rs:866–867` 正是重定位成员块页号和 packed offset 的写入处。
- 这是**已复现的冗余重算/性能误报**；当前证据不是漏更新，也不证明所有工程保存都会发生该行为。测试调用库 writer，不是启动 E3D UI 执行 SAVEWORK。
- 尚未修复依赖差分器；不放宽断言、不新增 ignored、不把红测改为接受误报。后续修复应仅消除已证明的物理地址噪音，并保留真实成员变化、未知负载变化及 ORI/尺寸变化的检测。

### 8.4 命令和当前结果

- j171：`rustfmt --edition 2024 src/fast_model/orientation_source_diff_fixture.rs && cargo test --offline --lib synthetic_orientation_source_ -- --nocapture --test-threads=1`：**2 passed / 1 failed / 0 ignored**，exit=101，已结束。
- 增补负向测试的逐字定位证据后，j173：`rustfmt --edition 2024 src/fast_model/orientation_source_diff_fixture.rs && cargo test --offline --lib synthetic_orientation_ -- --nocapture --test-threads=1`：**8 passed / 1 failed / 0 ignored / 2668 filtered**，测试耗时 0.39s，exit=101，已结束；原 6 个组件测试仍全部通过。
- j174：`cargo test --offline --lib`：**2544 passed / 1 failed / 132 ignored / 0 filtered**，总计 2677，测试耗时 15.71s，exit=101，已结束。唯一失败为上述同值 ORI 红测；132 ignored 未执行，不能计为验收通过。
- 新夹具编辑器 diagnostics：0 errors / 0 warnings。相关 diff 已复查；这不替代运行结果，当前测试集仍为红色。既有 vendor dead_code、未用 cc、未来兼容性警告未在本阶段处理。

当前代码 SHA-256（历史第 7 节摘要保留不改）：

| 文件 | SHA-256 |
| --- | --- |
| src/fast_model/orientation_source_diff_fixture.rs | d64a5bf772526d855ce6d49d5c61bb29c7e60118a2653e21f521b476f3be0a7c |
| src/fast_model/orientation_increment_regression_tests.rs | efcc03e229af95ec6ba93239e63ada437ff03ecb00ea18b7e898eb1897140920 |
| src/fast_model/e3d_model_service.rs | b0f2ad173eda4b012aaf476ab0c307f1c997c1959b6c4e13f4dd41c02461a67d |
| ../vendor/e3d-model/src/element_diff.rs（只读，未改） | 1521a7a952a6a82b736ab0564878b45e24e120e6d6c49ec804c29e69d5444562 |
| ../vendor/e3d-io/src/write/record.rs（只读，未改） | a91889842b2c690825f41b93cad9ddd3cd44b41e191cc303fa5533a1895de3f4 |

### 8.5 仍不属于已验收范围

- 这不是完整 E3D 编辑器/真实项目的合法性认证；最小源库仅为当前 reader/writer/model 管线构造，未验 E3D UI 的 claim/name 表及编辑副作用。
- S 端 settled 根由夹具从实际 S 源根集构造，不是读取真实已发布回执；当前 cheap-path 选项为启用，测试未改配置或环境。
- 自动监听、HTTP execute、队列/worker、生产刚体 executor 全套 guard、model_target 依赖核对、durable publication/水位、服务重启与前端显示仍未覆盖。真实源输入和归因缺口已补上，但不能报完整端到端验收完成。
- 原始真实工程两库/重启/性能验收仍未完成。未提交或推送；不恢复协同、不分派另一代理工作。下一步需用户确认是否扩展到依赖差分器的修复。

## 9. 红测修复与转绿（2026-10-16；计划已执行，待 R5 关闭）

计划：`docs/plans/2026-10-16-opaque-relocation-noise-fix-plan.md`（已批准/执行中）。用户已切父目录根，
vendor 文件工具直写；R1–R4 由 Agent-A 执行。

### 9.1 修复内容（vendor/e3d-model）

- `src/element_diff.rs`：`record_identical` 比较 `implicit_data` 时排除字节 24..40（头字 6..=9 的两块地址），
  新增 `implicit_data_equal_ignoring_block_addresses`；+62 −1，rustfmt 干净。
- 新增 2 单测：`relocated_block_addresses_do_not_break_record_identity`（纯搬迁→相等）、
  `non_address_implicit_change_still_counts`（字 0/字 11 变化→仍不等，哨兵不削弱）。
- R1 类型确认：`ParsedElement.implicit_data: Vec<u8>`，`parse_as` 取全隐式区字节，区间 24..40 精确。

### 9.2 验证证据（本轮新跑）

| ID | 命令 | 结果 |
|---|---|---|
| V1 定向 | e3d-model `cargo test --offline --lib element_diff` | exit=0；11 passed / 0 failed（含 2 新） |
| V1 全库 | e3d-model `cargo test --offline --lib` | exit=0；373 passed / 0 failed / 4 ignored |
| V2 红测转绿 | gen-model `cargo test --offline --lib synthetic_orientation_` | exit=0；**13 passed / 0 failed**；同值保存 `opaque: false`、`touched={}`、`regen=[]` |
| V3 全库 | gen-model `cargo test --offline --lib` | 2548 passed / **1 failed（预存 MINBEN 漂移，见下）** / 132 ignored，75.60s |
| V4 | `get_problems` element_diff.rs；`get_changes` | 0 错误/0 警告；本窗口仅 3 文件 +71 −2 |

### 9.3 V3 唯一失败的归因（非本计划引入）

- `the_generation_read_set_mirrors…`：gen-model `generation_read_set.json` 与 e3d-model
  `data/attribute-read-set.json` 差一个 `MINBEN`；后者工作树在途修改来自其他工作线（我未碰该文件）。
- 归因实验证据：`git stash push` 仅藏起本人的 element_diff.rs 改动后重跑，仍然同样失败；`stash pop` 已恢复
  （62 insertions / 1 deletion，6 处 helper 引用无缺）。
- 另：V2 首次运行时曾报 fixture E0599（`.parsed()`），重跑即消失；同期夹具文件 449→734 行（他人新增 4 用例：
  子 BOX 锚点、第二窗恢复、双根同窗、X/Y/斜轴），判为并发编辑中的瞬态，不作为证据问题。

### 9.4 仍未覆盖

§8.5 各项（监听/worker/model_target/durable/重启/前端、真实工程两库/性能）依然未验收；MINBEN 漂移待所属
工作线处理。未提交/推送；协同仍暂停。

## 10. R5 关闭：MINBEN 漂移归因与最终全绿（2026-10-16 本会话）

### 10.1 全量两个失败的最终归因

- `the_generation_read_set_mirrors…`：真实漂移，非陈旧产物。电缆工作线（vendor commit `420d1f4` 及在途改动）
  在 `vendor/e3d-model/src/cable.rs:237` 新增 `get_number("MINBEN")` 读点并已重生成 e3d-model 侧产物，
  但 gen-model 侧 `generation_read_set.json` 未同步——两边产物差 `MINBEN`。
- `synthetic_orientation_source_same_value_save_is_not_a_model_change` 那一红：**陈旧测试二进制**。
  该次全量运行的 aios-database 测试 exe 是在 `element_diff.rs` 修复（10:41）之前编译的；修复后的二进制
  下该用例连续 12 次单跑全过、本最终全量亦过。同一 exe 文件名（build out 目录哈希不变）掩盖了依赖重编。

### 10.2 本次处置

- 重跑 `python output/gen_read_set.py`：两个产物按当前源码重新生成。vendor `attribute-read-set.json` 较 HEAD
  增 `CMPR`/`ENDPOI`/`MINBEN`/`OUTD`/`STAPOI`/`YGAP`；gen-model `generation_read_set.json` 的 e3d_model 半边
  同步补上 `MINBEN`（vote 并集 149→150）。
- `src/data_interface/model_impact.rs` 三处守卫钉更新（均为「字典 DCHC=0 集合真变了」的例行点名维护）：
  `MINBEN` 入 DCHC=0 钉名单；并集条数 149→150（附 2026-10-16 注释）；放行数 `3941-9`→`3941-10`；
  `cable_property_inputs…` 覆盖名单补 `MINBEN`。

### 10.3 最终命令与结果（本轮新跑）

| 命令 | 结果 |
|---|---|
| `cargo test --offline --lib same_value_save_is_not` ×12 | 12/12 通过，`opaque: false` |
| `cargo test --offline --lib the_generation_read_set_mirrors` | 1 passed |
| `cargo test --offline --lib data_interface::model_impact::tests` | 50 passed / 0 failed |
| `cargo test --offline --lib`（全量） | **2549 passed / 0 failed / 132 ignored**，32.61s，exit=0 |

### 10.4 仍未覆盖（沿用 §8.5）

监听/HTTP execute/队列 worker/durable 水位/重启恢复、真实工程两库与 E3D UI SAVEWORK 端到端均未验；
`implicit_data` 忽略区间 24..40 仅对本夹具布局取证成立，真实库其它 noun/布局的普适性未普查。
未提交、未推送；协同仍暂停。

## 11. 真库普查：24..40 归一化的「不误伤」证据（2026-10-16 本会话，~12:20 跑完）

### 11.1 门与口径

新增 `vendor/e3d-model/tests/relocation_noise_real.rs`：沿 `ams8000_0001`（ams000 样本工程 DESI 库）
会话链逐相邻窗口跑生产 `collect_window` → 索引 `Modified` 候选 → `diff_element`，每条候选同时算两本账——
旧账（修复前逐字节 `record_identical`）与新账（含 24..40 归一化的 `is_unchanged()`）。
哨兵（硬断言）：① 逐字节相同却报变化 → 红；② opaque 但隐式差异只落在 24..40 → 红（归一化被绕过）；
③ `diff_element` 报错数必须为 0。`absorbed==0` 仅诊断输出，不判红。

### 11.2 普查结果（649 窗全链，9.10s，1 passed）

```
census: 649 windows; modified=1096 unchanged=0 absorbed_relocation=0 (in 0 windows)
        opaque=37 (in 9 windows) attr_changed=895 structural=276 diff_errors=0
```

- **误伤**：0。37 条真 opaque 逐条过哨兵，每条都有 24..40 之外（或非隐式区）的真实差异；
  没有一条 opaque 的差异只落在地址槽里。
- **救回**：0 覆盖。本语料没有任何纯搬迁重写候选——每次真实 SAVEWORK 最少也跳真实字节
  （`CACHID`/`SPAMAP` 等戳，落进 attr_changed 桶），索引 `Modified` 的 1096 条全部带真差异。
- **结论性质修正**：归一化在真数据上的证据是「不误伤」，不是「救回 opaque」。「同值保存会
  纯搬迁」的机制仍由合成夹具 + 记录格式不变式（字 6..=9 为 explicit/成员块地址槽，
  `e3d-io/src/record/mod.rs` 的 `EXPLICIT_SLOT=24`/`MEMBERS_SLOT=32`）背书；真实语料未提供
  反例也未提供正例。本门保留为新语料落地时的自动普查器——若未来某份库吸到搬迁
  （`absorbed>0`），同一组哨兵会逐条核它没盖住真 opaque。

### 11.3 顺带发现 → 已分类并修复（二/三/四轮，同日）

37 条真 opaque 分布在 9 个窗口（18→19、17→18、15→16、14→15、13→14、12→13、11→12、10→11、
9→10）。给门内加 `classify_opaque`（隐式区逐字 + 显式区逐属性配对）二轮普查后，全部同质：

- **载荷**：显式流里 `is_uda` 区间的文本属性新增/变化。hash `0x2902d6e2`（绝大多数，新增
  `Text("JS")`）与 `0x2902d6e3`（`Text("PIPERB"/"PIPERA"/"Kangwp")`），type_code 10240
  （存储类型 40 文本）。
- **伴随差异**：隐式区字 10。它不是地址槽，是格式/计数字
  `0x20000000 | member_words | (explicit_words << 14)`（`e3d-io/src/write/record.rs` 头注）：
  显式流加一条它就长大，`200bc002→200cc002` 即 explicit 47→51 字。真实元数据，正确地留在
  24..40 忽略区间之外——顺带从另一侧验证了忽略区间的边界。
- **不是 UDA 字典缺失**：门内补挂生产同款 UDA 目录（`DirectoryDbResolver` 扫 ams000+acp000，
  `dbnos_of_type("DICT")` 合并 8 本共 567 条定义）后这两个键仍答不出——它们是**应用私有
  存储属性**（值形如用户名/管线标签），模板与项目字典都不声明。生产环境同样答不出名。

**修复（`element_diff::changed_attributes` 第二趟）**：抽取层与字典都答不出的显式条目按
`#%08X` 伪名（`#2902D6E2`/`#2902D6E3`）进 `attributes`——记录变了是事实，按名读的下游
读取集合门能正常裁它是不是模型输入，比整块落 opaque 精确：`placement_input_changed` 不再因
说不出名字而保守级联，opaque 只剩「连存储条目都指不出」的真未解码形态（隐式区载荷等）。

**四轮普查结果（同日）**：`census: 649 windows; modified=1096 unchanged=0 absorbed=0
opaque=0 attr_changed=932 (anonymous_named=37) structural=276 diff_errors=0`——37 条与原
opaque 一一对应转实名伪名；`element_diff` 单测 11/11；gen-model `synthetic_orientation_*`
13/13；gen-model 全量 **2550 passed / 0 failed / 132 ignored**（其中 1 项为 plant-ui 工作线
重构 `index.html` 把 `data_api_url` trim+fallback 拆成两行造成的钉串漂移，已按语义拆钉修复，
与差分改动无关）。

### 11.4 第二条链：CATA 库同口径普查（同日五轮）

门泛化出 `db_type` 参数（`open_pinned` 按 `template_file_for` 派生模板），新增
`real_catalogue_chain_noise_census` 跑 `ams7000_0001`（项目级 CATA 覆盖库）全链：

```
census ams7000_0001: 15 windows; modified=69023 unchanged=0 absorbed=0
                     opaque=0 attr_changed=69003 (anonymous_named=0) structural=24 diff_errors=0
```

- 69012 条 modified 全落在 `238→262` 一窗——跨会话号大段的目录整库重写（目录导入/重建
  类保存），逐条都点得出属性名（`anonymous_named=0`）；CATA 库不产生 DESI 那种应用私有载荷。
- 哨兵跨库型成立：记录头 11 字不变式（字 6..=9 地址槽）不是 DESI 形状；6.9 万条真实
  CATA 记录 `diff_errors=0`，伪名路径未被触发但也未误伤。
- 纯搬迁在两库语料中都不存在：每次真实保存都带真实字节差异。

### 11.5 伪名路径的合成确定性回归（同日）

此前伪名上报只有真库普查证据（37 条一一对应）。补合成钉：
`synthetic_anonymous_explicit_attribute_reports_hash_pseudo_name`
（`orientation_source_diff_fixture.rs`）——用真实写会话
（`write_anonymous_text` → `RecordDraft::add_explicit_text_ascii`，hash `0x2902D6E2`、
type_code 10240 文本）造出与真库同形状的匿名显式属性，走 pinned S/T 读者 + 生产
`diff_element`：

- `attributes == ["#2902D6E2"]`、`opaque == false`——伪名格式与上报档位钉死；
- `is_placement_only == false`（非摆放名）且 **`placement_input_changed == false`**——
  纯属性存储变化不再级联，这是相对 opaque 判决的净收益语义；
- 第二窗改值（"JS"→"Kangwp"）仍点名同一伪名，第三窗删除属性（`remove_explicit`）
  仍点名同一伪名且不落 opaque——新增、变更、删除三形态全覆盖。

同日再补一钉：`synthetic_anonymous_attribute_alongside_ori_blocks_cheap_path`——
混合窗（同记录同窗 `ORI`+匿名属性，`write_mixed` 单次会话落库）：

- `attributes == ["#2902D6E2", "ORI"]`、`is_placement_only == false`、
  `placement_input_changed == true`；
- `plan.regen == [根10]` 且 `rigid.transform.is_empty()`——未知属性可能是模型输入，
  便宜刚体路被正确拒掉，判保守重生；
- 紧接一窗匿名属性原样带着、只动 ORI：`attributes == ["ORI"]`、
  `is_placement_only == true`、`rigid.transform == 1`——**值没变的匿名属性不上报，
  不污染纯摆放判决**。

聚焦套 `synthetic_*` 15/15 通过（含原 13 项方位用例无回归）。

下游语义经 `model_impact` 核对属实：`#` 伪名不在字典 DCHC 表 →
`metadata_only_tier` 拿不到两票 → 元素本身照常上卷重生（与 opaque 同判），
差别只在不误级联与可诊断——判决保守度不降级。

### 11.6 仍未覆盖（更新后）

§10.4 的「24..40 普适性未普查」一项关闭为「真库误伤普查通过（DESI+CATA 两链）、救回侧无语料」。
伪名路径已有真库+合成双证据。
监听/HTTP execute/worker/durable/重启、真实工程两库与 E3D UI SAVEWORK 端到端未验。
`E3D_CENSUS_DB_FILE`+`E3D_CENSUS_DB_TYPE` 可把门指向任意库文件；其余库型（PADD/ISOD 等）
未普查但同一记录头不变式适用。未提交、未推送；协同仍暂停。

## 12. 2026-10-21：审核最新在途改动后的收口与生产执行体回归

本节承接第 11 节。用户要求审核最新修改，继续修到「方位修改的增量更新」正常工作。工作区仍有多条工作线未提交改动（ISSUE-029 根账本、A6 窗口计划缓存、D10 门禁等），本节只改与本目标相关且已确认的缺陷/测试，不覆盖其他工作线。

### 12.1 发现并修复

1. **窗口计划缓存的单测依赖全局表，全量并行必红**：`window_plan_cache_replaces_same_dbnum_and_bounds_total_entries` 先清空进程级 `WINDOW_PLAN_CACHE` 再断言，而合成方位夹具（dbnum=8000）在同一进程里调用 `WindowRootSources::collect`，会并发写入同一张表并触发「同库新窗替换」，把测试刚插入的键淘汰。单独跑绿、全量跑红，红的不是淘汰逻辑。
   - 修复：把淘汰规则抽成纯函数 `evict_window_plan_cache_entries(&mut HashMap, key, sets)`，插入路径与单测共用；单测在本地表上验证同库替换、容量上限与同键重插，不再碰全局状态。
2. **缓存输入缺文件身份，同号跨项目副本可能拿错文件**：缓存键只有 `(dbnum, S, T)`，而 MDB 路由目录允许同号 dbnum 在不同项目下各有文件、会话号可能相同；A6 把两端 `DbSet`/`SourceIndex` 放进缓存后，执行体可能用另一个项目的输入切本窗子计划。
   - 修复：`CachedWindowInputs` 增加 `file`；`UnitDeltaProducer::open` 命中缓存时必须核 `service.source_file(dbnum) == inputs.file`，不一致就当作未命中重算并回填。规划器（`WindowRootSources::collect`）与兜底路径都写入该格。
3. **真实保存的 `CACHID` 戳没有进夹具**：E3D 3.1 每次图形重建都会把 `CACHID` +1，真实「只改 ORI」的 SAVEWORK 往往同时跳 `CACHID`；夹具若只写 ORI，就测不到戳是否被正确忽略。
   - 新增 `synthetic_orientation_source_ori_with_placement_stamps_keeps_the_cheap_path`：同一真实写会话改 `ORI` + `CACHID`，断言 `attributes == ["CACHID","ORI"]`、`is_placement_only()` 为真、刚体分桶仍为 1，并与 T 端全量生成对拍。`SPAMAP` 在本夹具的 EQUI 模板未声明，同族语义由 `element_diff` 既有单测覆盖。
4. **方位链路缺「生产执行体」级回归**：此前真实二进制夹具只直接调纯函数 `republish_records`，没有覆盖执行体的重放闸、pin@T 目标核对、网格盒回调与回执盖章。
   - 新增 `synthetic_orientation_source_production_executor_republishes_then_replays`：真实 S/T 源文件 → `WindowRootSources::collect` 判 transform → S 端整根重算灌本地投影并盖 S 回执 → `republish_roots_by_rigid_motion_on`（真执行体）→ 与 T 端全量生成逐记录对拍；再同窗重跑一次，断言 `replayed` 且世界矩阵不再乘第二次 Δ。

### 12.2 本轮新跑命令与结果

| 命令 | 结果 |
|---|---|
| `cargo test --offline --lib synthetic_orientation_ -- --test-threads=1` | exit=0；**15 passed / 0 failed**（原 13 + 新增生产执行体 1 + ORI/CACHID 戳同窗 1） |
| `cargo test --offline --lib window_plan_cache_replaces -- --test-threads=1` | exit=0；1 passed / 0 failed |
| `cargo test --offline --lib` | exit=0；**2562 passed / 0 failed / 133 ignored**，12.99s |
| `cargo check --offline --lib` | exit=0（仅既有 vendor `instantiate_cylinder` dead_code、manifest unused `cc`、future-incompatibility 提示） |
| `rustfmt --edition 2024 --check` 三个改动文件 | exit=0 |

### 12.3 仍未覆盖（沿用第 8.5 / 10.4 节）

- 真实 E3D UI `SAVEWORK` → 文件监听 → HTTP execute → 队列/worker 的完整链路；本轮仍用真实 writer/reader 的合成工程，不是已安装工程副本。
- 读透 + durable 模式下两条便宜路都按设计回退整根 regen；该回退路径的正确性由既有全量测试覆盖，但未在真实服务上做方位修改实测。
- durable 落库、任务收尾水位、服务重启恢复与前端显示未在本轮验证。
- 未提交、未推送；未操作真实项目数据库或服务配置。

## 13. 2026-10-21：文件身份并入窗口计划缓存键

本节承接 12.1 第 2 条。12.1 的修法是「命中后再核 `inputs.file`」；本轮把文件身份直接并进键，
并补一条走真实二进制 + 生产规划器的行为测试。

### 13.1 改动

- `WindowPlanCacheKey`：`(dbnum, base_sesno, target_sesno)` → `(dbnum, file, base_sesno, target_sesno)`。
  淘汰规则随之按文件分域：**同库同文件**的新窗替换该文件的旧窗（旧的 `DbSet` / `SourceIndex` 句柄不滞留），
  不同文件（同号跨项目副本）各留一份，谁也不能把对方顶掉。容量上限仍是
  `MAX_WINDOW_PLAN_CACHE_ENTRIES = 8`，超限按 `(S, T)` 最小者淘汰。
- 淘汰循环取最旧键改为 `cloned()`：键含 `PathBuf`，不再 `Copy`。
- `inputs.file` 命中复核保留（见 13.2 最后一条）；第 14 节把文件身份收进载荷键后，这一格与该复核一并删除。
- 行为测试 `window_plan_cache_is_scoped_by_source_file_and_rejects_mismatched_inputs`
  （`orientation_source_diff_fixture.rs`，真实 S/T 二进制文件 + 生产 `WindowRootSources::collect`
  + 生产 `UnitDeltaProducer::open`）三段：
  1. A 规划完，B 的同号同窗键上查不到 A 的计划；
  2. B 也走生产规划器后，A、B 两份计划并存（B 的规划不淘汰 A），B 的缓存输入是 B 自己的文件；
  3. 手动把 A 的计划塞到 B 的键下（模拟路径别名 / 旧键 / 未来把 `inputs` 塞错键的调用方），
     执行体必须拒绝它、按 B 重算出空计划，并把 B 的键回填成 B 的输入。
- 纯函数淘汰单测补一条同号跨文件并存的断言（`file_a` / `file_b` 同 dbnum 同窗）。

### 13.2 评估：为什么不只做「命中时核对」

两个方案都能得到正确结果，差别在复用率与覆盖面：

| | 只在命中时核对（12.1 的临时修法） | 文件进键（现行） |
|---|---|---|
| 正确性 | 不一致就重算，正确 | 不一致的键根本查不到，正确 |
| 跨项目复用 | A、B 同号同窗互相覆盖 / 顶掉；每窗都 miss，300.5s 的重算开销照付 | A、B 各留一份，规划完 A 再规划 B 不丢 A |
| 覆盖面 | 只对 `inputs: Some` 的条目有效；没有 inputs 的条目仍会被接受 | 键层面隔离，与 `inputs` 是否有值无关 |
| 代价 | 每次命中一次 `PathBuf` 比较 | 键变大；容量按文件分配，>8 个活跃文件窗口时仍淘汰最旧 `(S,T)` |

结论：**文件身份该进键**。12.1 的「命中后核对」在键不含文件时是必需的，但它把「别的项目的计划」
留在同一个格子里，两个项目同时活跃时会持续互相顶掉——正确但不省，且会把对方的计划丢掉。
进键后 `inputs.file` 核对在生产路径上已不可达（键与 `inputs.file` 同源写入），当时保留它作为
对「键命中 ≠ 输入同一份文件」的零成本断言；同日第 14 节进一步把这份重复删掉，身份只留键一处。

### 13.3 残余风险（未改，记录）

- **路径拼写即身份**：键比较原始 `PathBuf` 字节。不同副本路径不同 → 正确隔离；同一文件以不同拼写
  （相对/绝对、`\` 与 `/`、`..`、符号链接）进入 lookup → **miss 而不是错命中**，只损失复用。
  当前规划器与执行体都经 `E3dModelService::source_file(dbnum)` 的同一条 pin 解析取路径，拼写一致；
  不要再引入第二条路径来源。没有加 `canonicalize`：它要一次系统调用、在网络盘上可能失败，
  而现在的唯一来源已经足够。
- **同路径换文件**（备份覆盖 / 就地替换副本，dbnum 与会话号都相同）仍在键外，会命中旧计划。
  概率低但后果是静默用旧输入。要堵可在键里加 `(len, mtime)`：规划→执行之间不写文件，不会破坏
  预期命中；跨窗本来就因 `(S,T)` 不同而 miss。本轮按「不做、记为可选硬化」处理。
- **写入用 `index.dbnum`、查询用调用方 `dbnum`**：两者由同一文件解析，当前一致；若将来不一致，
  表现是持续性 miss，不会错命中。

### 13.4 验证

| 命令 | 结果 |
|---|---|
| `cargo test --offline --lib window_plan_cache -- --test-threads=1` | exit=0；2 passed / 0 failed |
| `cargo test --offline --lib` | exit=0；**2563 passed / 0 failed / 133 ignored**，12.93s（并行默认线程数，含全局缓存写入）；第 14 节收口后同一命令 **2565 passed** |
| `rustfmt --edition 2024` 两个改动文件 | exit=0 |

仍未验收：多项目同进程服务上真实 SAVEWORK 的缓存命中 / 隔离实测，以及同路径换文件场景；
本节证据是合成双工程 + 真实二进制读写。未提交、未推送。

## 14. 2026-10-21：分域键收成一个类型，淘汰规则只留一处

### 14.1 改动

- 新增 `WindowPlanKey`（字段私有，只能经 `new(dbnum, &Path, S, T)` 构造）：四个分量就是分域
  规则本身，调用方不再拼 `(u32, PathBuf, u32, u32)` 元组。
- `WindowPlanSets` 自带 `key`；唯一构造入口 `window_plan_sets(...)` 把 `plan_update` 的产出与键
  绑在一起。插入入口 `insert_window_plan_sets(sets)` 只收载荷、键由载荷派生——「键是 B、载荷是 A」
  不可表达。规划器不再逐字段克隆构造载荷，原 20 行 struct literal 删除。
- 新增 `WindowPlanCache`（`entries` + `CAPACITY`）：`insert` 一处负责同文件换窗、容量淘汰与同键
  替换，被移出的 `Arc` 返回给调用方在锁外释放。原自由函数 `evict_window_plan_cache_entries`
  与直接暴露的全局 `HashMap` 一并删除。
- `CachedWindowInputs.file` 与 `UnitDeltaProducer::open` 的 `inputs.file` 复核删除：文件身份只剩
  `WindowPlanKey` 一处。原复核是「键不含文件」时代的补丁；现在同一份 `file` 在
  `window_plan_sets` 里同时写进键与载荷，第三份重复反而会漂。行为无变化（键含文件后该复核已不可达）。
- 单测改为对本地 `WindowPlanCache` 实例断言四条规则（同文件换窗并交还旧载荷、同号跨文件并存、
  容量上界、同键重插）；真实二进制夹具只断言「B 的同号同窗查不到 A、B 的载荷键是 B 的文件、
  执行体命中 B 的计划与 B 的输入」。夹具不再断言全局缓存里某一格一定还在：进程级缓存有容量上界
  且与并行测试共用，实测全量并行时会把别的测试的插入误读成本测试失败（红过一次，已按此收紧）。

### 14.2 接口

| 用途 | 之前 | 现在 |
|---|---|---|
| 查 | `cached_window_plan_sets(dbnum, &file, S, T)` | 不变（内部构造 `WindowPlanKey`） |
| 构造载荷 | 就地 struct literal / `cache_window_plan_sets(..)` | `window_plan_sets(dbnum, &file, S, T, &plan, inputs)` |
| 插入 | `insert_window_plan_cache(dbnum, &file, S, T, sets)` | `insert_window_plan_sets(sets)`（键从载荷来） |
| 淘汰 | 自由函数 + 裸 `HashMap` | `WindowPlanCache::insert` + `CAPACITY` |
| 文件身份 | 键 + `inputs.file` 两处 + 命中复核 | 只在 `WindowPlanKey` |

### 14.3 验证

| 命令 | 结果 |
|---|---|
| `cargo test --offline --lib window_plan_cache -- --test-threads=1` | exit=0；2 passed / 0 failed |
| `cargo test --offline --lib` ×3（默认并行） | exit=0；**2565 passed / 0 failed / 133 ignored**，12.80s / 12.92s / 15.01s（三次全绿，专门验证不再受全局缓存并行淘汰影响） |
| `cargo check --offline --lib` | exit=0 |
| `rustfmt --edition 2024 --check` 三个文件 | exit=0 |

### 14.4 没变的限制

- 路径拼写仍是身份（见 13.3）；同路径换文件仍不覆盖。
- 规划器写 `index.dbnum`、执行体查调用方 `dbnum`，不一致时表现为 miss（13.3 第三条）。
- 未提交、未推送；真实双工程 SAVEWORK 未跑。
