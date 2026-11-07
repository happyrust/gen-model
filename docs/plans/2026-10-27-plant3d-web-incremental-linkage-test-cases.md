# 增量更新自动化测试案例集合 · K 组：前端联动（plant3d-web）

> 日期：2026-10-27。来源：用户「继续编写完善当前的模型增量更新自动化测试案例。使用 grill-with-docs」，随后补充「需要将增量更新的文档、
> 描述保存到 ../gen-model，需要配合 e3d tty 导出 rvm 的方式去验证」。考问三轮（Q1–Q18），三轮都「全部按推荐」。
> 上位：`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md`（INC-A…J，登记表 `tests/fixtures/increment_cases.json`）。
> 本文是它的 **K 组**。那份计划与登记表在另一会话的在途改动里（2026-10-27 12:37 仍有未提交修改），K 组的登记行、计划里的
> 指向等它们提交之后再补（Q4）。
> 前端代码与测试在 `plant3d-web`；前端词条见 plant3d-web `CONTEXT.md`「即时生成数据」「数据批次」「更新提示」「重载」（2026-10-27 改）。
> 现场记录：plant3d-web `docs/verification/incremental-linkage-2026-10-26/`（L2 回放 + Chrome headless，轻量联动落地前后两轮）。

## 0. 一句话

gen-model 的数据批次落库之后，plant3d-web 只在状态栏给一条更新提示，人点「重载」才按最新记录换场景里受影响的生成根（2026-10-26
「轻量联动」）。A–J 组盯的是后端自己说的话（回执 / 水位 / 库上行），**K 组盯的是用户看到的那一份**：三层可重复的门——
L0 Vitest（进 plant3d-web CI）、L2 回放 e2e（本机 nightly，用本仓的 L2 回放夹具驱动真服务）、L3 现场 e2e（E3D TTY 驱动，发版前）。
几何裁判是 **E3D TTY 导出的 RVM**：重载之后场景里该元素每个构件的世界包围盒要和同一会话的 RVM 对上。这是对 10-25 计划 D1
的补充，只管 K 组；A–J 仍以「两端全量」为唯一几何裁判（Q5）。

## 1. 链路与覆盖

| 环节 | plant3d-web 代码 | 2026-10-27 之前 | 本次之后 |
|---|---|---|---|
| WS 任务流 | `src/composables/useGenModelV1TaskFeed.ts` | 无 | K05 |
| 信封 → 更新提示 | `src/composables/useGenModelV1UpdateNotice.ts` | 12 例（解析 / 相关性 / 状态） | K01–K04、K08（删除查询）、K10（文案）、K11 |
| 批次里删掉的根 | `genModelV1TaskChanges`（`GET /api/v1/tasks/{id}/changes`，本仓 web-service-api §4.4.1） | 无 | K08 |
| 状态栏提示条 | `src/components/model-tree/GenModelV1HealthBadge.vue` | 无 | K10 |
| 重载 | `useModelGeneration.ts` `showModelByRefno({ refresh })`、`useDbnoInstancesDtxLoader.ts` `evictDtxRefnos` | 无 | K07–K09 |
| 场景探针 | `ViewerPanel.vue` → `createGenModelV1LoadedProbe` | 无 | K11 |
| 按库水位行 | `src/composables/useModelGenerationAdmin.ts` | 无 | K06 |
| 端到端 | `e2e/incremental-linkage-gen-model-v1.spec.ts` | 无（10-26 是临时脚本，跑完就删了） | K13–K15 |

## 2. 决策（2026-10-27 考问，全部按推荐）

| # | 决策点 | 结论 |
|---|---|---|
| Q1 | 范围 | 10-26 轻量联动整条链；「任务 → 增量记录」面板不在这次 |
| Q2 / Q13 | 词汇 | plant3d-web `CONTEXT.md`：重写「即时生成数据」；新增「数据批次」（与本仓词汇表同名，「增量批次」进 _Avoid_）、「更新提示」、「重载」。后两个是前端概念，本仓 `CONTEXT.md` 不动 |
| Q3 | 测出缺口怎么办 | 测试按应有行为写；小缺口当场修，要后端配合或体验取舍的写 todo、记在本文 |
| Q4 | 文档放哪 | 本文（gen-model）；登记表 / 10-25 计划的指向等在途改动提交后再补，动手前先问 |
| Q5 | RVM 管哪一段 | A–J 的 D1 不动。K 组里 RVM 是用户看到的几何的真值：重载后场景里该元素每个构件的世界包围盒与同一会话 TTY 导出的 RVM 对上（≤ 1 mm、件数相等），restore 腿同样 |
| Q6 | RVM 真值什么时候导 | 回放链：一次性导好，JSON 放在链夹具旁 `rvm-truth/<sesno>.json`，nightly 不要 E3D；现场：每一腿施加后即时导出，只在隔离副本上跑（D7） |
| Q7 | 前端测试分层 | L0 Vitest（进 CI）、L2 回放 e2e（本机 nightly）、L3 现场 e2e（发版前）；不做假 WS 的浏览器 e2e |
| Q8 | 缺口①：重载一个已删除的单元根 | 重载永远不生成；记录查询本身收齐且为空 → 撤下根与名下旧对象、提示「最新记录里已没有几何（已删除或成员已删光）」。K14 的 25 就是后一种：EQUI 还在、唯一的 BOX 被删，文案不能说「已删除」（2026-10-27 首跑前改） |
| Q9 | 缺口②：被删构件的旧对象残留 | 重载后撤下「重载前记录缓存里这根名下的构件 − 新记录」；根自己不撤 |
| Q10 | 缺口③：连着几批时前一条提示被顶掉 | 提示累积到重载 / 忽略：根取并集，按库会话范围最早起点 → 最新终点，改动项累加，跨库也并成一条 |
| Q11 | 缺口④：partial 不提示 | partial 也提示；只重载 `status = generated` 的根，失败的在悬停说明里点名、场景保留旧几何 |
| Q12 | 缺口⑤：断线 / 丢帧期间完成的批次 | 这次不修，K12 todo；做法见 §6 |
| Q14 | 回放 e2e 由谁拉后端 | 本仓 `scripts/replay/Start-LinkageReplayStack.ps1` 拉、写 stack.json；spec 只读它、自己 replay-step 并驱动浏览器 |
| Q15 | 回放步的 RVM 真值从哪来 | 隔离副本上 replay-init 到每一步再 TTY 只读导出（`scripts/e3d/Export-ReplayRvmTruth.ps1`）；不重录、不动现有夹具 |
| Q16 | 案例清单 | §3 的 K01–K15 |
| Q17 | 场景探针怎么测 | 抽成 `createGenModelV1LoadedProbe` 单测，ViewerPanel 只调用 |
| Q18 | 提交 | 先不提交，用户验收后按逻辑分笔提交；本仓只新增文件 |

## 3. 案例

判据沿用 10-25 计划 §2 的 O 编号；K 组新增 **O6：前端所见**——状态栏提示、场景里的对象、属性面板与水位行的读数。
「现状」三档：**有**（本次已写）、**todo**、**首跑待核**（已写、要本机第一次跑通才算）。

| ID | 案例 | 层 | 判据 | 载体（plant3d-web） | 现状 |
|---|---|---|---|---|---|
| K01 | 信封解析：只认带 `result.batch` 的 `succeeded` / `partial`；根统一成 `a_b`；生成失败的根另记 | L0 | O3 | `useGenModelV1UpdateNotice.test.ts`「K01」 | 有 |
| K02 | 相关性：非 DESI 不提示；只登录不改（变化 0、无单元）不提示；库已装入或根在场景就提示 | L0 | O6 | 同上「K02」 | 有 |
| K03 | 提示累积：同库 / 跨库取并集，会话范围按库合并，重复信封不重复计数，重载派发并集，重载 / 忽略后清空 | L0 | O6 | 同上「K03」 | 有（修③） |
| K04 | partial：出提示；只重载生成成功的根；前一批失败、后一批成功的根移出失败名单 | L0 | O3 + O6 | 同上「K04」 | 有（修④） |
| K05 | WS 任务流：订阅即连、显式 subscribe tasks、seq 空洞计数、断线 3 s 重连、握手连败 5 次停在 down、最后一个退订回 idle、ping 25 s | L0 | O6 | `useGenModelV1TaskFeed.test.ts` | 有（顺手修一处，见 §6） |
| K06 | 按库水位行：对话框请求过 `/dbnums` 后，非本会话的 `task_finished` 800 ms 合并刷一拍；没请求过不听不刷 | L0 | O6 | `useModelGenerationAdmin.test.ts` | 有 |
| K07 | 重载路径：绕过「已在场景」短路、清这根的记录缓存、replace 装载、不生成、不飞相机；普通显示不碰缓存 | L0 | O6 | `useModelGeneration.genModelV1.test.ts`「重载（refresh）」 | 有 |
| K08 | 重载已删除的根：记录查询收齐且为空 → 撤下根与名下旧对象、不生成；没收齐 / 只退到子树查询 / 查询失败 → 场景保持原样。删掉的根从 `/tasks/{id}/changes` 找（删除不出单元回执） | L0 | O6 | 同上 + `useGenModelV1UpdateNotice.test.ts`「K08」 | 有（修①） |
| K09 | 重载后被删构件不残留；收集没收齐时一个也不撤；装载器层 `evictDtxRefnos` 隐藏对象、退出已加载、修订号 +1 | L0 | O6 | 同上 + `useDbnoInstancesDtxLoader.test.ts` | 有（修②） |
| K10 | 状态栏提示条：文案（单库会话范围 / 跨库库号 / 受影响根数 / 生成失败数）、重载按钮只在有可重载的根时出现、点重载让属性面板的 ui-attr 查询重拉、忽略、没装 VueQueryPlugin 也能挂 | L0 | O6 | `GenModelV1HealthBadge.test.ts`、通知模块「K10」 | 有 |
| K11 | 场景探针：库是否装入看当前激活的库；根是否在场景先问加载器记账、再沿属主链问 DTX（EQUI 自己没对象、`show_refno` 直装不记账） | L0 | O6 | 通知模块「K11」 | 有 |
| K12 | 断线 / 丢帧期间完成的批次的补救 | L0 | O6 | 通知模块「K12」`it.todo` | todo |
| K13 | 回放 `ams8000-equi-move-chain`：725 基线与 RVM@725 对拍 → 725→727 EQUI `BY U 500` → 提示出现、不点重载场景不动 → 重载 → 与 RVM@727 对拍、不生成、属性面板 POS z = 1900 → 打开「按库」，727→730 `BY D 500` → 水位行不点刷新自己变成 730/730/730、提示变成 …→730 → 重载 → 与 RVM@730 对拍 | L2 | O6 + RVM | `incremental-linkage-gen-model-v1.spec.ts`「K13」 | 有（2026-10-27 首跑通过，§8） |
| K14 | 回放 `db8000-two-delete`（issue-019）：24 基线与 RVM@24 对拍 → 25 删掉 EQUI 唯一的 BOX（根重算）→ 重载：记录查空且收齐，根与 BOX 撤下，与 RVM@25（无几何）对拍 → 26 删整根 → 提示「库 8000 已更新到会话 26（改 2 项）」、没有「重载」、悬停说明「受影响的构件都不在场景里」→ 单元在场景里一个对象也不剩 → 忽略；全程不生成。删掉的根还挂在场景里那条路由 L0 的 K08 覆盖 | L2 | O6 + RVM | 同上「K14」 | 有（2026-10-27 首跑通过，§8） |
| K15 | 现场：隔离副本上 E3D TTY 施加 EQUI `BY U 500`（恰好一个 SAVEWORK）→ TTY 只读导出 RVM → 等三水位齐平且安静 → 重载 → 与刚导出的 RVM 对拍；`BY D 500` 再来一遍 | L3 | O6 + RVM | 同上「K15」 | 首跑待核（10-27 按计划没跑） |

## 4. RVM 真值（K13–K15）

- **口径**：与 `scripts/e3d/rvm_export_*.mac`、`adr066_live2_rvm_export_*.mac` 同一套窄口径（`repre insu off` / `repre obst off` /
  `repre tube on`、隐含管段分容器、`export system /expdri.so`），按 refno 导 `export =24384/24776`。导出宏没有 SAVEWORK。
- **导出**：在 pwsh 里 `& scripts/e3d/Export-ReplayRvmTruth.ps1 -Case <链> -Steps @(725, 727, 730) -IsolatedProjectsDir <隔离副本>`
  （`powershell -File` 会把 `-Steps 725,727,730` 压成一个数 725727730，必须用 `&` 调）。隔离副本要把 E3D3.1 下**全部**工程都拷过去
  （`robocopy` 真拷贝，2026-10-27 这份是 `_runs/linkage-truth/E3D3.1`，约 14.3 GB）：E3D 的 evars 链会无条件 call 别的工程（如
  YCYK-E3D）的 evars，只拷 AMS + 目录库时 TTY 在登录前就 exit 1。`l3_suite` 把当前目录当仓根（找 `scripts/e3d/run_ams_c_entrymacro.bat`），
  `Invoke-TtyMacro` 会先 `Push-Location` 到本仓。E3D 一登录就改写所有打开的库的
  文件头，影子工程根是硬链接 / junction 到原件的，**E3D 绝不能打开影子根**；脚本先核副本：不是原件、没有 junction / 符号链接、
  `???000` 库目录里的文件只有一个名字（`fsutil hardlink list`）。每一步：副本里被回放的库 `replay-init --to <该步>`（与回放时服务
  看到的是同一份字节）→ `l3_suite --check-driver` 跑脚本写出的导出宏 → `rvm_verify import --scope narrow --root-refno` →
  `<链夹具>/rvm-truth/<sesno>.json`（`fixture_dir` 链放在夹具目录里，`source_zip` 链放在 zip 旁）。元素在某步已不存在的不导
  （K14 的 26 用「场景里一个对象也不剩」判）。
- **K13 / K14 要的真值**：`ams8000-equi-move-chain` 725 / 727 / 730（`-Element =24384/24776`）；`db8000-two-delete` 24 / 25
  （`-Element =24384/24778`）。2026-10-27 已导好、已放进夹具目录（未提交）：

  | 链 | 步 | 元素（TTY `Q POS WRT /*`） | 带几何的成员 | 世界 AABB（mm） |
  |---|---|---|---|---|
  | `ams8000-equi-move-chain` | 725 | EQUI /1-LNR-Q005-PJ（24384/24776）W 2821 N 8004 U 1400 | `BOX 1 of EQUIPMENT /1-LNR-Q005-PJ` → 24384_24777 | [-3001.39, 7696.57, 1125.00, -2641.15, 8311.35, 1675.00] |
  | | 727 | U 1900 | 同上 | x / y 不变，z 1625.00..2175.00 |
  | | 730 | U 1400 | 同上 | 与 725 逐位相同 |
  | `db8000-two-delete` | 24 | EQUI /1-LNR-Y302-PJ（24384/24778）E 2255 N 6819 U 1400 | `BOX 1 of EQUIPMENT /1-LNR-Y302-PJ` | [1988.14, 6552.34, 1150.00, 2522.00, 7086.20, 1650.00] |
  | | 25 | EQUI 还在，BOX 已删 | 无（`geometry_count` 0） | — |

  25 那份：E3D 导 RVM 时不写没有图元的 EQUI 组，RVM 里只剩 SITE / ZONE，`rvm_verify import` 的 `--root-refno` 于是钉到了
  ZONE `/1RX03-CASE-DQ` 上（见 `meta.root_name`）。这是标签错位，不影响判据：K14 只用「没有几何」。每步证据（宏、TTY 日志、RVM、
  `check-driver.log`）在 `output/rvm-truth/<链>-<时间戳>/<步>/`（git 忽略）。
- **对拍**：场景侧取该单元名下每个有可见对象的构件（`resolveDtxRefnosByUnitRefno`）的世界包围盒并集，页面带 `dtx_recenter=0`
  取真坐标，DTX 世界单位 × 1000 = mm（`LINKAGE_SCENE_TO_MM` 可改）；RVM 侧取快照里自己有几何的成员的 `aabb_world_mm`。
  两边构件集合相等、每个包围盒六个分量都在 1 mm 内。快照要 `export_scope = narrow`，否则直接红在真值上，不放宽容差。
- **身份**：只用 TTY 导 RVM、不带 ATT，`rvm_verify import` 只能给命名根钉上 `--root-refno`；无名构件是 E3D 默认名
  `BOX 1 of EQUIPMENT /1-LNR-Q005-PJ`，import 已拆好 `noun` / `owner_desc` / `ordinal`。spec 按「属主 + 名词 + 序号」对服务的
  `GET /api/v1/tree/children` 解成 refno（属主是命名元素就查快照里已解析的名字，属主又是默认名就递归；取属主成员表里同名词的
  第 n 个），与 `rvm_baseline::resolve` 同一口径，只是走 HTTP 树。树接口的 `name` 是短名（`BOX`），不能拿默认名整串去对。
  带几何的成员解不出来就红（快照里 SITE / ZONE 的 `unresolved` 没有几何，不影响）。

## 5. 怎么跑

- **L0**（plant3d-web 仓根）：
  `npx vitest run src/composables/useGenModelV1UpdateNotice.test.ts src/composables/useGenModelV1TaskFeed.test.ts src/composables/useModelGenerationAdmin.test.ts src/composables/useModelGeneration.genModelV1.test.ts src/composables/useDbnoInstancesDtxLoader.test.ts src/components/model-tree/GenModelV1HealthBadge.test.ts`
  ；门禁仍是 `npm test`、`npm run type-check`（基线不涨）、`npm run lint`。
- **L2**：先在本仓导好真值（§4，只要一次），再
  1. 本仓：`powershell -File scripts/replay/Start-LinkageReplayStack.ps1 -Case ams8000-equi-move-chain`（只用现成二进制：
     `<TargetDir>/<release|debug>/aios-database.exe`、`db_session_fixture.exe`，缺就先 build；端口 9199 / 8199），最后一行是 stack.json。
     服务的工作目录是运行目录 `<RunRoot>/<链>-<时间戳>`，里面 junction `resource` → 本仓 `resource`：gen-model 的单实例锁
     `.gen-model.instance.lock` 落在工作目录，拿仓根当工作目录会和本机开着的 gen-model 撞锁（2026-10-27 首跑撞过一次）。
     影子工程根 `<RunRoot>/E3D3.1` 各链共用，换链前先 `-Stop` 上一套。冷启动约 40–50 s；迟迟不齐平、`/dbnums` 不应答时先看
     运行目录里的 `surreal.err`（见 §7「本机内存」）；
  2. plant3d-web：`$env:LINKAGE_REPLAY_STACK='<stack.json>'; npx playwright test e2e/incremental-linkage-gen-model-v1.spec.ts -g K13`
     （K14 同理；Playwright 自己起 Vite :3101）；
  3. 本仓：`powershell -File scripts/replay/Start-LinkageReplayStack.ps1 -Stop '<stack.json>'`（只停 stack.json 记下的两个 PID）。
  K14 换 `-Case db8000-two-delete` 再跑一遍；一套栈只跑它对应的那条（另一条自动跳过）。用例不带回放用例里的 `service_env`
  （如 H01 的冻结延迟钩），727 就是普通的一窗。
- **L3**：`Start-LinkageReplayStack.ps1 -Live -ProjectsDir <隔离副本>`（服务监听副本本身，不回放）→ plant3d-web
  `$env:LINKAGE_LIVE_STACK='<stack.json>'` 跑同一个 spec；每一腿由 spec 调 `scripts/e3d/Invoke-LinkageE3dLeg.ps1 -Leg apply|restore`。

## 6. 缺口与修复（plant3d-web，未提交）

| 缺口 | 修复 | 案例 |
|---|---|---|
| ① 重载已删除的单元根：查空后走「records 查空」自动生成（POST `model/generate` 一次），旧几何留在场景 | `refresh` 时查询失败 / 查空都不生成；记录查询本身收齐且为空才撤下（`finishRefreshOfEmptyRoot`）；删掉的根不在单元回执里，提示侧另查 `/tasks/{id}/changes` 把它们放进重载名单 | K08 / K14 |
| ② 批次删了单元里的构件：replace 只替换新范围里的 refno，被删构件的旧对象不隐藏 | 重载前 `invalidateRoot` 本来就返回这根名下的旧构件，装完拿它减新范围，差集走新的 `evictDtxRefnos` | K09 / K14 |
| ③ 同库连着两批动不同的根：提示只留最新一条，前一批的根重载不到 | `mergeGenModelV1UpdateNotice` 累积 | K03 |
| ④ partial 批次不出提示 | 认 `partial`，按单元 `status` 分出失败的根 | K04 |
| ⑤ 断线 / 丢帧期间完成的批次没有补救 | **todo**。做法：装入时按库记下「是在哪个模型水位装的」；WS 重连或 `missedEvents` 涨时拉一次 `/dbnums`，已装入库的 `model_sesno` 前移了就出一条「库 n 在断线期间更新到会话 m」的提示，重载名单取该库已装入的全部生成根（拿不到单元回执）。要先有装入水位的记账，改动较大 | K12 |
| ⑥ 重载后的 toast 盖住状态栏的更新提示（K14 首跑发现）：全局 toast 在底部居中，正好压在状态栏中段；Vuetify snackbar 在指针悬停时不计时，点完「重载」指针还在原处，重载的 toast 就出在指针底下、一直不消失，下一条提示的「重载 / ×」点不到，挪开指针才好 | **todo**（体验取舍：动的是全局 toast 位置，如抬到状态栏之上）。spec 先像人一样挪开指针、等 toast 消失再点提示条 | K14 |
| 顺手发现：最后一个订阅者退订后，浏览器异步派发的 close 把任务流状态从 idle 改成 down；退订后立刻再订阅时，旧连接迟到的 close 会把新连接的状态改成 down | `useGenModelV1TaskFeed.ts` `onclose`：无订阅者 → idle 不重连；已有新连接 → 不改状态 | K05 |

## 7. 风险与首跑待核

- **单位与轴**：场景 × 1000 = mm、E/N/U = x/y/z。K13 首跑的 725 基线已按六个分量 1 mm 对上（§8），这条不再是假设。
- **包围盒口径**：RVM 导的是图元、前端是网格；盒体一致，曲面图元可能差一个弦高。红在这里先对 `rvm_verify` 的 chord 判法，不先放宽 1 mm。
- **身份解析**：不要 ATT，默认名走服务的模型树解（§4「身份」）。它依赖 `/api/v1/tree/children` 的 `order` 与 E3D 成员表顺序一致；
  哪天解错了会表现为「场景里多 / 缺某个 refno」，不会静默放过。
- **删除的根**：「按祖先查 records」在根名下没有几何时干净地回空（K14 的 25：`load_scope_refnos ok=1 source=visible-insts count=0`），
  `/tasks/{id}/changes` 里删除的 `op` 是 `deleted`、`root_refno` 有值（K14 的 26）。K14 首跑两条都核过。
- **现场腿的会话数**：带 SAVEWORK 的一腿会再写一个只含 DBMDAT 的尾会话，只读导出那次登录也会；spec 等三水位齐平且 10 s 不变再重载。
- **端口**：栈用 9199 / 8199，与 10-26 联动验证同一对；被占就拒绝启动，不抢。
- **Playwright 录像收尾卡住**：本机 `video: 'retain-on-failure'` 时 `browserContext.close` 会一直不返回（K13 首跑本体 70 s 跑完后 runner
  挂住 6 分钟；K14 一次失败的跑在只开了 about:blank 的页上也一样，`DEBUG=pw:api` 停在 `browserContext.close started`）。spec 里
  `test.use({ video: 'off' })`，靠关键点自己截图；配置里也没有 actionTimeout，点击与截图都带 30 s 上限。
- **本机内存**：整机提交内存吃紧时 surreal 会因分配失败退出（`surreal.err`：`memory allocation of … bytes failed`），服务随后挂住、
  `/dbnums` 不应答。2026-10-27 第一次起 K14 栈就是这样（系统日志同一分钟有「虚拟内存不足」）。第二次起的栈在齐平后几分钟内
  服务与 surreal 都无声消失（无崩溃事件、无日志，按被外部结束处理）。本机同时还跑着别的会话，跑前确认 stack.json 里两个 PID 还在。
- **登记表**：K 组还没进 `tests/fixtures/increment_cases.json`（登记表的核对 `increment_cases_registry` 只认 cargo / l3 / replay 三种载体，K 组的载体在 plant3d-web，
  要先定一种 `web:` 前缀怎么核），等在途改动提交后一起定。

## 8. 首跑记录（2026-10-27，本机）

环境：plant3d-web 工作区（未提交），Playwright 自起 Vite :3101，Chrome headless（`channel: 'chrome'`）；gen-model release
`aios-database.exe`（10-26 20:38）与 `db_session_fixture.exe`，栈端口 9199 / 8199，影子工程根 `_runs/plantui-linkage/E3D3.1`。

| 案例 | 结果 | 要点 |
|---|---|---|
| K13 | 1 passed（1.0–1.1 min，按最终 spec 连跑三次都过） | 725 基线与 RVM@725 六分量 1 mm 内对上；726–727 合成一批（1 s，`24384/24776 transform`），提示「库 8000 已更新到会话 726→727（改 2 项，场景内 1 根受影响）」，不点重载场景不动；重载后与 RVM@727 对上、没有 `model/generate`、属性面板 `POS -2821.27…, 8003.96…, 1400 → …, 1900`；「按库」行 727/727/727，728–730 合成一批后行自己变 730/730/730，提示「…728→730（改 2 项，场景内 1 根受影响）」；重载后与 RVM@730 对上；无页面错误 |
| K14 | 1 passed（1.1–1.2 min，按最终 spec 跑过） | 24 基线与 RVM@24 对上；25 批次单元 `24384/24778 regen`，`/changes` 里 BOX 24384/24779 `deleted`；重载时记录查空且收齐 → 撤下 2 个 refno（1 个有对象），场景与 RVM@25（无几何）一致；26 批次无单元回执，`/changes` 里 EQUI `deleted`；提示「库 8000 已更新到会话 26（改 2 项）」、没有「重载」、悬停说明「受影响的构件都不在场景里，不用重载」；忽略后提示消失；全程无生成 |
| K15 | 没跑 | 按计划留到下次（要在隔离副本上真跑 E3D SAVEWORK） |

首跑里边跑边修的（都未提交）：

- spec：按 `data-ribbon-tab` / `data-command` 开模型生成对话框；对话框是模态的，重载前先关；点击 / 截图带 30 s 上限；关录像；点提示条前挪开指针、
  等 toast 消失（§6 ⑥）；POS 等面板渲染出来再读、改成必判；补 `[linkage] notice` / `POS` 日志。
- plant3d-web：重载查空撤下时的文案改成「最新记录里已没有几何（已删除或成员已删光）」（Q8）。
- 本仓：栈脚本把服务工作目录挪到运行目录（§5）；`Invoke-TtyMacro` 先 `Push-Location` 到仓根（§4）。

环境问题（不是被测代码）：录像收尾卡住 context 关闭、第一次起 K14 栈 surreal 内存分配失败、第二次起的栈无声消失，见 §7。
各次运行的终态截图在运行目录里（`k13-after-730-reload.png`、`k14-after-26-dismiss.png`）。
