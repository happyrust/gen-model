# Agent-A 个人交接

协作轮次：ROOM-20261016-01；身份：本 Arena 会话，协调者；日期：2026-10-16。
仅 Agent-A 更新本文件。不能把本编号认作模型名称或其他会话身份。

## 接单与已读

- 任务：T-00 建档已完成；T-04 协作汇总暂缓，不再等待其他会话报告。
- 协议版本：1（协作条款暂停）；任务板修订：4；已读取最新交接文件，记录用户暂停决定。
- 状态：协作 PAUSED；T-00 DONE；T-04 DEFERRED。
- 允许写入：根 AGENTS.md、本目录公共文档与本文件；本轮不修改业务代码。

## 已做工作

- 完成 Bridge 握手并先调用 workspace_brief；读取历史证据、关键代码文件版本与 Git 只读状态。
- 用户选择临时编号、工作区文档与继续房间审核，随后确认共 2 个 Agent（含本会话）。
- 已给另一会话分配 Agent-B 与 T-01 READY，T-02/T-03 顺序等待；未代 B 确认已读或开工。
- 已建立并回读六份文档；get_changes 已检查文档差异；编辑器未报告错误/警告，此结果不等于全面 Markdown 校验。
- 没有把历史测试冒充本轮运行，没有修改业务代码或保存访问端点。

## 测试与命令

- 本轮没有运行 Cargo 测试；历史计数见 BASELINE，不作新证据。
- `git rev-parse HEAD && git status --short`：退出 0，已结束。
- 当前无自己持有的后台命令或构建预约。

## 当前决定与后续记录

- 用户决定：“先暂时不考虑协同。只记录到对应的文件。”
- 已在公共入口和任务板修订 4 记录暂停；不再分派任务、查询 B 接单或等待跨会话报告。
- B 个人文件保留为历史，不代其改写确认；以根入口与最新任务板的暂停状态为准。
- 后续按用户独立任务工作并记录到相应文件；恢复协作必须由用户明确提出。
- 本次仅更新文档，没有修改业务代码或运行构建/测试。

## 单会话续测：方位修改（2026-10-16）

- 用户随后要求继续测试模型增量，重点追踪 ORI/方位修改；协作仍暂停，未恢复 A/B 调度。
- 仅只读追踪，未改业务代码/现有测试、未调用真实更新 API。
- 实际新跑 5 组离线命令：刚体 26、影响分类 50、选根回退/容器 3，合计 79 passed / 0 failed；job j90 已结束，exit=0。
- 详细范围、源文件哈希、构建警告与未完成的旋转端到端验收见 `verification/model-orientation-increment-20261016/verification.md`。
- 前文“本轮未跑测试”仅指协作文档建档/暂停阶段；不能覆盖本节的新测试记录。
- 用户随后选择隔离合成夹具；第二阶段结果见下，不自动套用旧 live 平移脚本。

## 隔离合成夹具结果（2026-10-16）

- 新增独立测试文件 `src/fast_model/orientation_increment_regression_tests.rs`；`e3d_model_service.rs` 仅加 4 行 cfg(test) 接线，无生产业务逻辑修改。
- 6 项组件集成测试通过：父级旋转与子构件偏置、斜角紧包围盒、重放/逆旋转、混合尺寸重建分流、失效基线/缺网格、房间归属与完整合成几何烘焙对拍。
- 测试自身括号/导入错误已修正，不是生产行为红测。j139 定向 6 passed；j151 库级回归 2542 passed / 0 failed / 132 ignored，均 exit=0，均已结束。
- 第 1 阶段只读记录与“未跑全量”的历史措辞不代表当前结果；详细版本、命令及限制见方位验证文档第 7 节。
- S/T 状态、差分与根归因输入是合成适配；未覆盖真实 E3D 二进制解析、自动监听/生产队列、model_target 核对、durable/水位收尾或重启恢复，不能报真实端到端已通过。
- 未改真实模型、项目数据库或服务配置；未提交/推送，未恢复跨会话协作；其他工作线保持不动。


## 真实 S/T 夹具与新增红测（2026-10-16）

- 按用户选择补真实差分输入：新增 `src/fast_model/orientation_source_diff_fixture.rs`，从零生成临时二进制库，用 bundled schema + 实际 record/session writer 构造 S41/T42，再经真实 DbSet pin、diff_element、索引/闭包/根归因及源几何生成对拍。未使用工程副本。
- 父 ORI 旋转、父 ORI+子尺寸两个新增测试通过；既有 6 项组件测试仍通过。
- 新增同值 ORI 保存红测复现误报：所有可见属性及属主/成员不变，仅原始头 word8/9 的成员块物理地址变化，差分器给出 opaque=true 并将父根列入 regen。属冗余重算证据，不是已证实的漏更新。
- j173 方位专项 **8 passed / 1 failed / 0 ignored**；j174 全库 **2544 passed / 1 failed / 132 ignored**，15.71s，二者 exit=101 且已结束。当前不是全绿，不能引用 j151 的历史结果替代；详细字差、版本及限制见验证文档第 8 节。
- 保留原始预期和失败，不放宽断言或忽略测试；gen-model 只增测试接线/辅助复用，vendor 仅只读定位，尚未修复。需用户确认是否扩展到依赖差分器修复。
- 真实 source→diff→root→几何对拍已补；监听/worker/model_target/durable/重启仍未验收。未改真实项目或配置、未提交/推送；协同继续暂停，只记录本文件，不分派/核查 B。

## 会话延续与记录模式确认（2026-10-16）

- 新 Arena 会话接入 Bridge（握手 + workspace_brief 正常），只读回读全部交接文档与工作区状态：25 个在途改动文件、编辑器 0 错误/0 警告、无运行中命令、无活动文件。
- 用户确认：本会话延续 Agent-A 身份；名单问题暂缓（“先不要管协同”）；协作继续暂停，本轮只做记录。
- 本轮未改业务代码、未跑构建/测试、未分派任务、未核查 B；任务板修订保持 4，不做状态流转。
- 后续等用户给出明确独立任务后再动；恢复协作须用户另行提出并先定名单。

## 红测根因只读追踪（2026-10-16）

- 用户选择继续单会话测试方向；本轮纯只读：回读方位验证文档全文（192 行，§8 红测现状），未改代码、未跑命令。
- vendor 当前不可读：`../vendor/...` 与绝对路径均被 Bridge 拒（PATH_OUTSIDE_WORKSPACE）。此前记录的 vendor 行号（element_diff.rs:230、record.rs:866-867）为历史只读定位，本轮未复核。
- 工作区内 `opaque` 消费点（src 共 7 文件 33 命中，去噪后 3 处生产门）：
  - `window_root_plan.rs:360-367 change_metadata_tier`：opaque=true 直接 None，不放行粗级联豁免。
  - `window_root_plan.rs:9060 stamp_only_with`：opaque 不算只动戳，计入护栏分子。
  - placement_only/刚体路径：opaque 且 attributes 为空 → 非 placement-only → regen。
- 结论链：成员块物理地址字噪音 → vendor 差分 opaque=true → 三门全不放行 → 父根冗余 regen。与 §8.3 一致，无新反例。
- 下一步待用户定范围：修 vendor 差分器 / gen-model 侧归一化 / 保持红测补新场景。

## vendor 读取受阻（2026-10-16）

- 用户问需看哪段：已明确两段（e3d-model element_diff.rs 原始相等门+opaque 分支；e3d-io write/record.rs 成员块重定位前后约 30 行）。
- 用户称 vendor 已加入 workspace；复核 3 种路径写法（../ 相对、正斜杠绝对、反斜杠绝对）+ tree，Bridge 仍判 PATH_OUTSIDE_WORKSPACE；brief 显示允许根仍只有 gen-model 单根。
- 未绕过读 vendor；等用户确认提供方式（粘贴代码 / 允许命令只读 / 调插件根设置）。未改代码、未跑构建。

## vendor 定位复核与修复方案（2026-10-16）

- 用户授权命令只读 vendor。j176 定位（element_diff.rs 共 627 行，快筛 227-231、opaque 门 266-270）；j177 读 99-292+560-627；j178 读 record.rs 820-905 与模块文档 1-98；j179 取 SHA。全部 exit=0 已结束，只读，无写入。
- vendor SHA 与验证文档 §8.4 一致（1521a7a9…/a9188984…），历史定位逐字复核通过。
- 精确根因：record.rs:862-868，头字 8/9=成员块地址（page + pack_offset），记录搬迁即变；element_diff.rs:230 把完整 implicit_data 纳入 record_identical，纯搬迁落到 266-270 得 opaque=true。另发现 6/7=explicit 块地址，同理是噪音源（内容由 explicit_attributes 门覆盖）。
- 推荐方案 A（vendor 侧）：record_identical 比较 implicit_data 时排除字 6..=9，其余（0-5、10+）逐字比。安全论证：成员增删由 members 门覆盖，显式内容由 explicit_attributes 门覆盖，UDA/未解码变化仍走 opaque；TUBI 门不受影响。vendor 仓需补两测试：纯搬迁→unchanged、UDA 变化→仍 opaque。
- 否决 gen-model 侧二次确认：ElementDiff 只有 opaque 布尔，区分噪音需重读 DbSet 原始记录，重复差分器职责且改动面大。
- 约束：vendor 在 Bridge 允许根外，我只能读不能写；实施需用户动手或另授权。未改工作区代码、未跑构建/测试。

## plannatator 查找（2026-10-16）

- 用户要求先用 plannatator 制定开发计划；三处查找均无：工作区文件名/内容搜索 0 命中（约 40000 文件）；`where plannatator` 不存在（j180，exit=1）；pip/cargo 无 plan 相关包（j181）。
- 未找到该工具，等用户指明是路径/插件/拼写，或改由我手工起草计划。未改代码。

## grill 手工模拟（2026-10-16）

- 用户改指 grill-with-doc skill；工作区无 SKILL.md、无 .claude、无同名文件（40000 文件 0 命中），PATH/pip/cargo 亦无其实体。
- changelog.md:4090 证实真名是 grill-with-docs + Plannotator，走“两轮提问（Q1-Q15）→ 共识 → 批准”流程，属用户 Claude 侧 skill，我经 Bridge 无法调用。
- 处理：按其文档化流程手工模拟，grill 提问全部锚定已读证据（verification.md §8、element_diff.rs:227-231/266-270、record.rs:862-868、window_root_plan.rs:364/9060），每问给推荐项；共识后起草计划文档请用户批（代批准位）。未改代码。

## 开发计划已起草（2026-10-16）

- grill 第一轮被跳过，按 changelog 惯例“全按推荐”推进：G1 排除字 6..=9、G2 我出 patch 文本用户应用、G3 全套验证、G4 落 docs/plans。
- 旧隧道 530/1033 中断约 3 轮；用户给新端点后重连成功（同工作区，0 错误/0 警告；新窗口 changed_files 基线不同，在途文件以磁盘为准）。
- 新建 `docs/plans/2026-10-16-opaque-relocation-noise-fix-plan.md`（126 行，草案待批）：含精确 patch 文本、vendor 2 测试、R1-R5 步骤、V1-V4 验证矩阵、回滚与批准栏。诊断见本轮 problems 调用。
- 本轮除计划文档外零改动；未跑构建/测试。等用户批计划（R1 由用户开始）。

## vendor 穿透验证（2026-10-16）

- 用户建 `gen-model\vendor\shared` → `D:\work\plant-code\old\vendor` 穿透，自验 48 子仓可访问。
- 我方验证：文件工具（tree/read/search）走穿透路径仍被拒，Bridge 显式规则“不允许符号链接或 junction 路径”；
  run_command（cmd）可穿透列目录（j1，exit=0，见 e3d-model/e3d-io 等）。
- 结论：穿透只打通命令通道，文件工具仍须 Bridge 允许根治本。vendor 写操作仍未授权；计划审批待用户。

## 等允许根（2026-10-16）

- 用户选择先加 Bridge 允许根，计划实施等根加上再说。
- 本轮探一次：vendor 绝对路径仍 PATH_OUTSIDE_WORKSPACE，根尚未生效。未改任何文件，未跑构建。

## 允许根机制查明（2026-10-16）

- 用户问如何加允许根；只读查插件本体：`extension.js:116-117 root()` 取 `workspaceFolders` 第一个 file 文件夹，
  单根，无多根支持；`package.json` 无允许根配置项；`workspace.js resolveSafe` 显式拒绝路径中任何 symlink/junction。
- 结论：之前“加入 workspace”无效是因为插件只认第一个文件夹；junction 穿透对文件工具注定无效。
- 可行路：(b) 直接打开共同父目录 `D:\work\plant-code\old` 作单根（gen-model+vendor 全覆盖；换根后隧道公网 URL 会变，
  且我后续相对路径须加 `gen-model/` 前缀）；(c) 保持现状，用户亲手改 vendor；(d) 授权命令写 vendor。等用户选。未改业务文件。

## 父目录根验证与 R1 消解（2026-10-16）

- 用户切父目录根并给新端点；重连成功：workspace=d:\work\plant-code\old，0 错误/0 警告；此后 gen-model 侧路径须加 `gen-model/` 前缀。
- vendor 文件工具直读已通：element_diff.rs 627 行，SHA 1521a7a9… 与历史一致；record.rs 未重读（SHA 历史一致，内容已逐字复核）。
- R1 消解（我方完成）：`ParsedElement.implicit_data: Vec<u8>`（element.rs:25），`parse_as` 取 `raw[prefix..impl_end]` 全隐式区字节；
  字 6..=9 = 字节 24..40，与红测“implicit_data[44..] 相同 + 字差异仅 8/9”吻合。计划 §3.1 patch 无需修改。
- 计划仍为草案待批；待批准后由我实施 R1-R4（vendor 直写 + vendor 测试 + 工作区验证 + §9），用户 R5 关批准。

## R1-R4 已执行（2026-10-16）

- 用户批准后实施：R1 vendor patch（element_diff.rs +62 −1，含 2 新单测，rustfmt 干净）；
  R2 vendor 验证（定向 11/11、全库 373/0/4）；R3 工作区验证（定向 13/13 红测转绿，全库 2548/1/132）；
  R4 verification.md §9（+37）与计划状态（已执行/R5 待批）。
- V3 唯一失败为预存 MINBEN 漂移（他人改 data/attribute-read-set.json 未同步 generation_read_set.json）；
  stash 对照实验证明与本改动无关，已恢复改动。他人同期把夹具扩到 734 行（+4 用例），全部通过。
- 本窗口改动 footprint：3 文件；诊断 0/0；未提交/推送；协同仍暂停。等用户 R5 关闭批准。

## R5 已关闭（2026-10-16）

- 用户批准关闭：计划状态→已执行，批准栏已勾选；MINBEN 漂移转交所属工作线，我方不碰。
- 本轮交付：vendor 修复 +62 −1、vendor 2 新单测、V1/V2 全绿、V3 仅预存失败、§9 证据、计划与交接记录齐备。
- 未提交/推送；协同仍暂停。待命等新任务。

## 合并前审核 R1：拓扑与重叠面（2026-10-16）

- 新任务：审核当前分支，最新修改要并入 refactor。当前 gen-model @ 7a21aa1de [feat/issues-36-50-34]；
  refactor = feat + 1 提交（58885893e room 收口），在 gen-model-spatial-hierarchy 工作树干净检出；origin/HEAD 即 refactor。
- 提交层面 feat 已完全含于 refactor（ahead/behind 1/0 反向）；待合并的“最新修改”= 当前工作树 86 处未提交
  （32 M + ~54 ??），工作树 vs refactor 计 +1833 −452。
- 高风险重叠：收口 5 文件在工作树全脏（generation_read_set.json、model_impact.rs、room_recompute.rs、
  handlers.rs、room_rebuild_regression_tests.rs），直合必冲突或回退收口；其中 read_set 工作树比 refactor 少 2 行，
  与 MINBEN 测试失败同源，合并前须先对齐读集。
- 独立线：review/* 9 文件与收口无交集，可独立合；orientation 2 夹具 + verification/* 未跟踪，按归属定带不带；
  src/lib.rs +532 大改动所属线待确认。本轮只读，未动任何文件。等用户定合并口径。

## 合并前审核 R2：冲突地图与分批（2026-10-16）

- 用户选 86 处整体审核。本轮 -U0 双边区间 mapped（j21-j25，只读）。
- read_set 解法实锤：W 加 {CMPR,ENDPOI,OUTD,STAPOI,YGAP}，R 加同 5 个 + MINBEN（×2 半区）→ 合并取 refactor 版即可，
  含 W 全部并解 MINBEN 红。model_impact 测试 3 处同区（2613/2633/2846）须手工解；room_recompute 仅 1174/1221 两单行相邻，
  其余与 R 的 +158 块分区干净；handlers（728 vs 806-963 删除）与 tests（+284 在 345 vs R 改 92-145）远距，大概率自动合。
- lib.rs +532 归属：startup-step-timing（打点/dblog/耗时渲染，对应 2026-10-15-startup-step-timing-plan），单边干净。
  review 簇 9 文件（annotation/attachment/comments-edit/workflow/tests）内聚完整，单边干净。
- 分批提案：B0 取 R 版 read_set（解 MINBEN）→ B1 重叠 5 文件（3 处手工解+验证）→ B2 startup-timing（含 plan/verification 未跟踪）
  → B3 review 簇 → B4 其余（increment/mdb_closure/mbd/probe/CI/脚本/ADR/changelog 内容审后拆）→ B5 未跟踪分类带/不带。
  本轮只读，未动文件。等批分批方案。

## 合并前审核 R3：B1 内容与解法（2026-10-16）

- 分批已批。本轮 B1 双边全文 + 新跑验证（j26-j31）：audit 12/12 绿（“intentionally red”注释已过期）、
  rebuild_regression 23/23 绿，工作树 B1 面全绿。
- 解法：(1) read_set 取 R 版（超集，解 MINBEN）；(2) model_impact 取 R 版测试为底 + W 新测试 cable_property + fmt，
  W 的 `vendor/shared/e3d-io` 路径必须改回 `../vendor`（junction 只活在本工作树，refactor 侧无），W 的 S5 注释 PURP 半句
  与断言不一致、合并时去掉；(3) room_recompute 1174/1221 两处 W 体 + R 改名（去 handlers:: 前缀），R 的 +158 整块保留；
  (4) handlers 自动合（W 事务包装与 R 删除分区远）；(5) tests 文件 89-155 取 R 版改线，+284（12 audit 新测）取 W 版。
- W 的 +284 新测读 room_recompute 源码做接线断言（446/460），依赖 W 自身生产改动；R 改名不碰其断言串，解后应仍绿。
- 合后门禁：MINBEN、audit 12、rebuild 23、全库。未动文件。等批 B1 解法 / 进 B2+B3 内容审核。

## 合并前审核 R4：B2+B3 内容 verdict（2026-10-16）

- B1 解法已批。本轮 B2+B3（j33-j37，只读 + V3 全库绿作证）。
- B2 startup-timing（lib.rs 427+/105-）：新文件 helpers（打点/dblog/耗时渲染）+ run_cli 全步骤埋点；删除的 105 行全是旧
  step_started/fmt_elapsed 手工打印与旧 dblog 初始化的机械迁移，无行为删除（仅 `serve_if_configured` 一行需 R5 瞄一眼确认是重构非删除）。
  单边干净，可合；门禁：编译 + 全库（运行时代码以编译为准）。
- e3d_model_service 拆两线：我的 orientation 接线 8 行（B-ori，已验 13/13）+ ADR-085 D3 探针缓存化（latest_for + 新测）。
  B4a=ADR-085/D3/watch 线：increment_manager 重挂健康闸（refresh_health/unwatch/missing 前移 + has_watch_gaps 早退 + 顺序单测）、
  mdb_closure（has_watch_gaps/cached_plan_for + 真值表测试）、e3d_model_service D3 hunk、ADR-085 文档；内聚完整。
- B3 review 簇：鉴权收紧线（作者→作者/管理员、operator/task-write 二次校验、非 sj 节点 409、resave 非发起人 403、
  读路径 hydrate 附件、review_history→review_workflow_history 改表名）+ platform tests +33；V3 全库绿覆盖；单边干净可合。
  改表名跨文件一致性由同线全包 + 编译通过保证，合后跑 review 测试复验。
- 待 R5：B4 剩余归属（probe/cata/mbd/examples/脚本/CI/setup/mod.rs）+ ADR/changelog 内容 + B5 未跟踪分类 + lib.rs 那一行抽查。未动文件。

## 合并前审核 R5：B4+B5 verdict 与最终结论（2026-10-16）

- 本轮 j38-j52：B4 全文件双边 diff + B5 未跟踪核对 + 三扇新门。全程只读（除本记录），未动产品文件。
- serve_if_configured 清白：base 唯一调用在 lib.rs:1154，工作树 0 调用——系 B2 有意换成 start_configured
  （裸 spawn→bind 完成才返回，bind 失败就地终结启动；changelog 启动打点条目有据，/health 200 实测）。
- B4a watch 健康闸线 verdict：increment/mdb_closure/e3d(D3 hunk)/ADR-085 D3/探针 +dbno/plans/watch_root_scan/changelog
  健康闸条目——内聚完整，直接取 W。e3d_model_service 拆分行：+9 手工取"inventory 缓存行"（与 R4 B-ori 8 行同文件分区清）。
- B4b vendor/shared 穿透迁移线 verdict：CI×2（两 job 均自带建 junction 步骤）/脚本×2/examples/cata 依赖测试/
  mbd_branch_query/mbd 文档/model_impact 测试路径——直接取 W 全量；合并后须在 refactor 工作树补建
  gen-model/vendor/shared→../vendor junction（否则测试/脚本缺路径），CI 侧无需再动。
- B4c 杂项 verdict：setup.html（Plant3D 文案）/mod.rs（handlers→pub(crate)，供 mbd 端点用）/mbd pipe 文档取 W；
  MIME changelog 条目是已入库代码的迟记（base/worktree 均有 text/javascript），无动作；changelog 删了清边条目系文档写法，保留 W 版。
- B3 补齐：annotation(排序去重+obb 第 4 bundle)/review_form(requester 保护)+workflow_sync(退回回指)+platform tests(requester 钉)/
  common(空值不断链)/sync(附件水合+import 补 model_name)/attachments(遥测)：requester 语义三件套闭环，整簇直接取 W。
- B5 未跟踪分类：必带=各线 plan+verification 证据（ori 2 夹具+model-orientation+opaque 计划/B1 room 四份+startup-alignment 计划/
  startup_timing 计划/review 移植 notes/watch_root_scan 计划/incr-model-room/mbd pipe 文档+live+资源字典+q6/rigid-exclusion 相邻）；
  可带=tree-perf/watcher-route/extgeo-cable 计划；留置=agent-handoff+AGENTS.md+.archify+各行旧 verification；
  待用户定=e3d-version-20260911、acceptance-probe-runbook、根杂项 md×2、dbslot*。
- 新门：cargo check --bins --examples 绿（jobs=1；并行下两次失败系页面文件 OOM 与 rustc 栈溢出，环境性非代码错）；
  vendor cable/primitive/elmodl 三处 dead_code 系他人在途 WIP，不管。
- 最终 verdict：✅ 可合并。冲突面仅 B1 五文件（3 手工行：model_impact 测试×3 区、room_recompute 1174/1221、e3d +9 行）+
  e3d 拆分 1 文件，其余 26 跟踪文件可 fast（含 junction 条件）。收口不退化；MINBEN 由 B0 取 R 版 read_set 解。
  执行待批准：批准后按 B0→B1→B2→B3→B4→B5 在 refactor 工作树落地，每批重跑对应门禁。

## 合并执行 E1–E终：已落地 refactor 工作树，未提交（2026-10-16）

- 用户批准执行。落点 gen-model-spatial-hierarchy（refactor @ 58885893e，执行前干净）。
- 跟踪文件 31 M：27 个整拷（fc /B 逐个验，无差异）；handlers/tests 两 patch 一次 apply 绿
  （tests 补 EOF 空白 warning，无害）；model_impact 手工 2 处（fmt + cable 新测移植，路径保持
  `../vendor`、S5/读数注释取 R 版）；room_recompute 手工 12 处（W 体 + R 去前缀改名，
  4 处 `handlers::recompute_dbnum_room_edges_now`→本地短名已 grep 归零）。read_set 取 R=B0 无动作。
- 未跟踪 29 项全落（15 目录 xcopy + 14 文件 copy，曾被 xcopy F/D 交互问卡住一次，改 copy 即好）；
  留置：agent-handoff、AGENTS.md、.archify、旧 verification、e3d-version、acceptance-runbook、根杂项、dbslot*。
- junction 已建：gen-model-spatial-hierarchy\vendor\shared → D:\work\plant-code\old\vendor（mklink /J 一次成）。
- 门禁（spatial）：check --lib 绿；model_impact 51/51（含 MINBEN 与 cable 新测）；
  room_rebuild 24/24 + audit 12/12；orientation 14/14；check --bins --examples 绿；
  全库 2549 通过 + 1 失败 + 132 ignored——唯一失败 plant_ui bundle 断言经对照实验证明是
  plant-ui 兄弟目录在 V3 之后又漂了（gen-model 现跑同测同红），非合并引入，不管。
- 事故复盘：中途 D 盘被我的 RUSTFLAGS 变体重编顶到 0 字节，libtest 构建连挂三次（页文件/栈溢出/锁等待都是表象，
  真因 os 112 磁盘满）；按 mtime 清掉 11:40 后写入的 20268 文件/12.35 GB（全是我方失败产物），D 回到 12.2 GB，
  默认 flags 一次即过。另有他人 cargo（78304，vendor relocation_noise_real）在跑，曾短暂抢 D 盘 target 锁。
- 现状：31 M + 29 ?? 全部未提交，未推送。提交与否等用户定。

---

## 2026-10-26 · issues 复核 + P2 必败注入钩（Arena 侧代理，经 AstraFable 桥）

- 交付：`docs/plans/2026-10-26-open-issues-audit.md`（未跟踪，未提交）——9 个 open issue 逐条核到 commit / 测试名 / 代码行；#65 #31 #62 #63 四条结论已发 GitHub 评论，**未关任何 issue**。
- P2 §6 D6：`tests/increment_replay/main.rs` 加 `fail_roots` / `clear_fail_injection` / `expect_model_sesno_stay` + `Service::restart_with_fail_roots`（带 `AIOS_TEST_FAIL_ROOTS` 冷重启再推窗，预期失败的窗不等 `wait_in_sync`、改判 `model_sesno` 不推），新用例 `tests/increment_replay/cases/ams8000-equi-fail-inject.json`。本代理把 `INC-H05` 翻成 `has` 后，并发会话按「回放未跑绿前不翻」改回 `partial`——本代理认同，`INC-F05` 同理留 `partial`（只接了 `model_sesno` 半边，`applied_sesno` 半边要另一种注入）。
- 上述四处改动已由并发会话随 `22b7a7e0a`（H02 park-after-1 + `ams8000-equi-data-fail-park`）一并提交；`git status` 里 main.rs 的 +293 是「本代理 75 行 + 对方约 218 行」的合计，不是重复劳动。
- 修 `22b7a7e0a` 一处编译错：`tests/increment_replay/main.rs:1127` 格式串写了 `{}` 却没给实参，`cargo check --test increment_replay` 直接编不过（H02 的连败账断言因此从未被执行）→ 补 `case.dbnum`。
- L2 跑法要点（踩过）：`AIOS_REPLAY_PROJECT_ROOT` 要指到**含 `AvevaMarineSample` 的那一层**（本机是 `..\_runs\ams-replay\E3D3.1`）；指到 `..\_runs\ams-replay` 会整档 skip 且 **exit 0**，日志里的「1 passed」是跳过不是跑绿——判绿只看有没有 `skip increment_replay:` 或断言失败。
- 计划 §9 验收门 ② 原文「CI 含 L2 档并连续绿两周」与 P1 头部修正冲突，已按头部口径改写。
- 待办转交：#68（`INC-I02` 与实现全空）、#69（零进展 0 评论）、#34（默认仍 false）+ #45 语料、#61 B 类 3 条、#63 只剩 `v0.1.43` tag 与 `0.1.43+g` 核对串、#65 目录项便宜路语料、登记表 23 条未落（P3 的 `increment_budget_baseline.json` 与 `scripts/nightly-local.ps1` 仍缺）、45 个 `output/` 取证脚本未入库。
- 边界：本代理未 commit / push，`D:\AVEVA` 全程只读，桥接端点未写进任何文件。协同状态不变：仍按「暂停 / 记录 only」，本条即记录。

### 2026-10-26 续 · 必败注入回放的四轮实跑（同一代理）

- 结论已写进 `docs/plans/2026-10-26-open-issues-audit.md` 附录（7 条，含证据行）。三条最要紧的：① 注入窗是 `partial` 不是 `failed`；② 恢复窗必须调短对账间隔（默认 300 s），否则是用例在等默认对账轮；③ **模型侧生成失败不进 `DATA_BATCH_PARK_AFTER` 的连败账**，对账轮会一直重跑同一右端——harness 为此加了 `latest_terminal_task`。
- 顺带修 `22b7a7e0a` 的格式串编译错（main.rs:1127 少实参），那处断言此前从未跑过。
- 本代理当前未提交的改动：`tests/increment_replay/main.rs`（+29 −5，新加 `latest_terminal_task` 与预期失败窗改用）、`tests/increment_replay/cases/ams8000-equi-fail-inject.json`（去掉重启后不可判的 records 断言、加 `service_env` 短间隔、728 超时 360 s）、`tests/fixtures/increment_cases.json`（H05 标题改实测口径）。`cargo check --offline --test increment_replay --features http_api` 绿；live 全绿待补一次（补跑时另一会话在同一影子根 `..\_runs\ams-replay\E3D3.1` 上跑 L2，`link.exe` 1104 产物占用）。H05 / F05 保持 `partial`。
- 已按这条不对称开票：**#72「模型侧生成失败没有停跑/退避账，与数据批次 DATA_BATCH_PARK_AFTER 不对称」**（含证据行与复现命令，三个备选方案 A/B/C 等人拍板）。补跑「全绿」这一步排在本机另一会话的 L2 之后（同一影子根 `..\_runs\ams-replay\E3D3.1` 不能并发），守候脚本空闲即跑，结果落在 `.scratch/audit-l2-run9.log`。
- 更正（同日 12:5x）：`8e055d4e3` 已把这批改动并入并把 **F05 / H02 / H05 翻 `has`**（回放全绿，12:35 那次跑的）；本代理的 `latest_terminal_task` 与实跑口径都进了 HEAD。补跑的守候进程没等到空闲就自己退出了，没产生任何一次 live 跑（`.scratch/audit-l2-run9.log` 不存在）。
- 另：只读复核 #68 的 `b661e1f88` / `c6e213736`（根枚举裁剪 + `plan=` 拆段，都已在 refactor 线上）。两条零成本建议发在 issuecomment-6415852819，正文在审计文档附二：**`parity-mismatch` 只出声不判红**（release 默认不对拍；建议三条链加 `log_must_not_contain`）、**预算门看不见 `roots_*_mode`**（`increment_budget.py:47` 只读两个总数）；边角是 `ROOT_SET_CACHE` 无失效路径。#68 的代码线一个字没动，`INC-I02` 仍 `partial`（7997 无回放语料、预算门尚未重跑）。
- `changelog.md:44` 的 `e6013ef76` 已改成发布线上的 `c6e213736`（并注明原号在 `fix/issue-68`，rebase 换号）——`git merge-base --is-ancestor` 实测 `E_NOT_IN_REFACTOR`。
- 另开一份 `docs/plans/2026-10-26-ptset-storage-audit.md`：只读审核 `pe_ptset`（管件 P 点派生表）的现行存储方式，12 行现状表 + 5 条意见。最要紧两条：**行里不带 `sesno`、启动补扫又只补「零行」的根 ⇒ 「已有一行但过期」永远不重算**（文件回退 / `pe` 单变都能撞上，代价是 `bran_comp_len` 一类静默算旧值）；**`tests/` 对 ptset 0 命中**（`fork_surreal_compat.rs:1452` 还留着退役前的 `out.ptset.d.pt` 形状）。`/health.model_cache.ptset_backfill` 的 `abandoned/failures/index_ready` 观测齐但没人读；`src/data_interface/model_refresh.rs:876` 那条断言证明「不在发布事务里排队」是有意的，设计没问题，缺的是验收面。
- ptset 审核的 B/D 两条已按「只动 tests/ 与 scripts/」落地：`log_must_not_contain` 从「只看最后一次重启那份日志」扩成「扫运行目录全部 `stdout*.log`」（同修了 `[模型目标核对]` 那一针的漏射），三条链末步各加两针（`[pe_ptset] 定义 root 索引失败`、`次补写失败，放弃`），新增 `scripts/ptset_backfill_check.py`（读 `/health.model_cache.ptset_backfill` 判 abandoned / index_ready / sweep / 排队不空，`--json` 供 nightly）。验过 `cargo check --test increment_replay`（11.72s 完成）与 `cargo test --test increment_cases_registry`（3 passed）；**未跑**完整回放。细节见 `docs/plans/2026-10-26-ptset-storage-audit.md` §4。
- **`§5 存放剖析` 补了一条必须先解决的实测事实**：三个 L2 运行目录的 `stdout.log` 里 `[pe_ptset]` **0 命中**，而补扫成功日志在 `Ok` 分支上无条件打（`:246-251`）；且 `schedule_startup_sweep` 等就绪的 `while !data_ready()` 循环**没有超时也没有日志**（`sweep.status` 停在 `waiting`）⇒ L2 环境下后台补写从不落地，任何「补写账正确」的验收都取不到证据。要真验：用例末尾等到 `/health` 的 `rows_written > 0`，或直接 `python scripts\ptset_backfill_check.py --base-url … --wait 120`（它对 `sweep.status` 非 `done`/`disabled` 判 FAIL，正是这一形态）。
- 协同仍按「暂停 / 记录 only」；本条是记录，不是新工单。


