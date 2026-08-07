# E3D TTY + PML 驱动的增量更新 L3+V 自动化套件 · 测试计划

日期：2026-08-06（拷问共识当日定稿）
被测仓：`D:\work\plant-code\old\gen-model`（服务端）+ `D:\work\plant-code\old\plant-ui`（视觉验收端）
决策方式：grill-with-docs 拷问会（七项决策逐题拍板，纪要见 §1）

> **本文定位**：把 08-04 总纲里一直靠人工的 **L3（真实 E3D 会话端到端）与 V（plant-ui 视觉闭环）**
> 两层机器化——E3D 改数用 PML 宏无人值守投递，服务侧走真实 HTTP，证据四图自动采集。
> L0/L1/L2 不归本文（见单测台账）。

## 0. 与既有文档的关系

| 文档 | 它管什么 | 本文与它的关系 |
|---|---|---|
| `2026-08-04_data-model-queue-test-plan.md`（三阶段总纲） | L0–V 分层、G0–G4 门禁、环境铁律 | 本文是其 **G3/G4 的自动化把手**；铁律全数继承（§8） |
| `2026-08-06_model-increment-unit-test-plan.md` | L0/L1 单测 + L2 live 定靶台账 | 互补不重叠；本文只管实机 |
| `2026-07-29_test-ams-incremental-update-summary-report.md` | D-01～D-15 矩阵、§7.3 十步流程、§7.4 inspect 探针口径 | 场景与证据口径的来源；本文把其中「在 E3D 修改并保存 session」这步机器化 |
| `docs/adr/ADR-018-golden-baseline-pair-restore.md` | 金基线成对恢复的决策 | 本文 §4 是其执行剧本 |
| `2026-08-04_rvm-baseline-verification-plan.md` | RVM 几何基准导入与比对 | 本文 §6 只在几何类场景挂载它，不重抄 |
| `CONTEXT.md`「实机端到端测试」章节 | 金基线对 / 场景宏对 / 哨兵日志 / 双侧对拍四个术语 | 本文按词表用词，不另造词 |

## 1. 决策纪要（2026-08-06 拷问会，七项全部落定）

| # | 决策点 | 结论 |
|---|---|---|
| 1 | 定位 | **L3 + V 级一并编排**：E3D 改数 → 服务增量 → 库侧断言 → plant-ui 四图，四进程一套编排 |
| 2 | 范围 | **管道先行**：冒烟三连（M1 几何改 / M2 位移 / M3 删除）打通骨架，再扩首批八场景 |
| 3 | E3D 通道 | **限时 spike**：①`TTY $M/宏` 命令行直带 → ②`PDMS_NOCONSOLE=1`+stdin → ③ENTRYMACRO 回退；先通者定标，三条实测结论全部写进通道 ADR（spike 后编号 ADR-019） |
| 4 | 基线 | **金基线成对恢复**（ADR-018）：每轮整体恢复「AMS 项目副本 + Surreal 快照」一对；轮内属性场景用场景宏对，删除/新增排轮尾 |
| 5 | 编排 | **Rust runner**（`src/bin/l3_suite.rs`）场景表驱动；触发走真实服务 `POST /api/v1/update/execute`；PowerShell 只留薄入口 |
| 6 | 判据 | **库侧不变量 + E3D 双侧对拍**全场景标配；RVM 几何基准只挂几何类场景（M1 / D-08） |
| 7 | 门禁 | 合入主干前跑冒烟三连；发版/大重构前跑八场景全量；结果回写 §10 台账，**不回写视为没跑** |

## 2. 场景表

**冒烟三连**（M 系列，管道验收标准：三条全绿 = 编排骨架可用）：

| ID | 沿用编号 | 库 | 目标与操作 | 宏（scripts/e3d/） | 判据要点 | RVM |
|---|---|---|---|---|---|---|
| M1 | DG-01 | 7997 | DAMP `24381/100819` 的 `DESP NUM2` 1000→1400，生成根 BRAN `24381/100817` | `projams_damp_desp_apply.mac` / `projams_damp_desp_restore.mac` ✅ | 预览命中 1 个模型影响项；BRAN 重生成；`geo_relate` 保持 5；网格尺度与 AABB 随宽度变化；restore 后 AABB 精确回基线 | ✅ |
| M2 | D-11 | 7997 | 同一 DAMP `POS +100mm E` | `projams_incr_pos_apply.mac` / `projams_incr_pos_restore.mac` ✅ | 预览影响根 = BRAN；`world_trans`/AABB 平移 100mm；对拍 `Q POS`；水位推进 | – |
| M3 | D-03 | 7997 | 删除 VTWA `24381/107146`（BRAN `24381/107104` 子件） | `projams_incr_delete_apply.mac` ✅（无 restore，金基线兜底） | `pe.deleted` 置位；实例与 owner 边清理；BRAN 子件 46→45；V 级：树节点与几何同时消失 | – |

**首批扩展**（F 系列，冒烟三连绿后同一骨架直接加行）：

| ID | 沿用编号 | 库 | 目标与操作 | 宏 | 判据要点 |
|---|---|---|---|---|---|
| F4 | D-10 | 7997 | DAMP `24381/100819` NAME `/1CUP001VAR` → `_CODEX` 后缀 | `projams_incr_name_apply.mac` / `projams_incr_name_restore.mac` ✅ | DataOnly：零模型任务；树文字变；几何与 inst 零变化 |
| F5 | D-04 | 7997 或 8000 | 在既有 BRAN/SUPPO 下 NEW 一个简单构件（沿 GENSEC Add 已验路径） | **新写** | 新增被扫描并归并生成根；树 + 几何同时出现 |
| F6 | D-02 | 8000 | FTUB `24384/22403` 跨 BRAN 移动（session 31–32 人工先例的机器化） | **新写宏对** | 新旧两根都重生成；V 级两根都刷新 |
| F7 | D-15 | – | 幂等重跑：**编排器内建标配**，每场景 execute 后再 execute 一次 | 无宏 | 第二次 up-to-date、零新任务；repeat 图与 after 图像哈希相等 |
| F8 | R-01 | 7999 | CAP `24383/66460` `POS +100mm U`（房间归属回归，issue5/7 血统） | `issue7_cap_pos_apply.mac` / `issue7_cap_pos_restore.mac` ✅ | `room_relate` 边随位移更新且 restore 后回归；房间轮收敛后 `room_recalc.detail` 数字回落；需 `gen_spatial_tree=true` |

场景表在 runner 里是数据（`const SCENARIOS`），加场景 = 加一行 + 一对宏，不碰编排逻辑（§5）。

## 3. E3D 驱动通道 spike 剧本

已知事实（2026-08-06 实测）：`des.exe ams SYSTEM/XXXXXX /ALL TTY` 能以 Console 形态启动，
但 **stdin 重定向不被命令循环消费**（core.dll 把标准句柄接到自己 spawn 的 pdmsconsole.exe，
与 `run_ams_c_entrymacro.bat` 头注一致）。spike 按把握度顺序验证，**先通者定标**：

| # | 通道 | 做法 | 判定（三条全中才算通） | 时间盒 |
|---|---|---|---|---|
| S-CH-1 | TTY 命令行直带宏 | bat 内 evars 后 `des.exe ams SYSTEM/XXXXXX /ALL TTY $M/D:/…/spike_sentinel.mac` | ①哨兵日志三行齐 ②`QUIT` 后 des 进程 ≤60s 自退 ③退出码可采集 | 2h |
| S-CH-2 | `PDMS_NOCONSOLE=1` + stdin | 置该环境变量后起 TTY，宏从 stdin 管道喂入 | 同上 | 1h |
| S-CH-3 | ENTRYMACRO 复核 | `run_ams_c_entrymacro.bat` 跑同一哨兵宏（已验通道，只复核哨兵纪律与退出行为） | 同上 | 0.5h |

spike 哨兵宏 `spike_sentinel.mac`（三段 ALPHA LOG，防一段失败吞全部）：

```
ALPHA LOG "D:/…/spike_a.log" OVER
$P SPIKE-ALIVE
ALPHA LOG END
ALPHA LOG "D:/…/spike_b.log" OVER
=24381/100819
Q CE
Q NAME
ALPHA LOG END
ALPHA LOG "D:/…/spike_c.log" OVER
$P SPIKE-DONE
ALPHA LOG END
QUIT
```

spike 附带测量并记录：会话冷启动耗时（TTY vs GUI），直接决定套件总时长估算。
**产物**：`docs/adr/ADR-019-e3d-unattended-driver-channel.md`（三条通道的实测证据 + 定标结论）。
runner 的驱动接口按「宏路径进、哨兵日志出」抽象（`E3dDriver`），通道可整体替换，spike 结论不锁死代码。

**spike 结论（2026-08-06 实测，已定标）**：`-tty` + `AVEVA_DESIGN_ENTRYMACRO` 采用。
只读哨兵与 runner wrapper 两轮都跑通，证据在 `output/e3d-spike/`
（`spike_a/b/c.log`、`runner-alive.log` = `L3-ALIVE`、`runner-done.log` = `L3-DONE`、
`runner-scenario.log` = 场景宏的 `Q` 输出）。落到代码上的三件事：

- **登录目标参数化**：`E3dDriver` 持有 projects_dir / 项目代号 / 账号 / MDB，经
  `L3_E3D_*` 环境变量交给 bat。同一条通道既能开在金基线工作副本上，也能
  `--project-dir` 直接开在目标项目上。
- **两级超时**：`--alive-timeout-secs`（默认 300s）盯「登录到命令循环」，
  `L3_E3D_TIMEOUT_SECONDS`（默认 1200s）盯整个宏。连不上的会话不该在每个场景上
  各烧 20 分钟——无人值守最不能忍的就是这个。
- **单独的通道验证入口**：`l3_suite --check-driver <宏>` 不起 surreal / 服务 /
  plant-ui，只回答「能不能登进去并自动执行」。探针宏
  `scripts/e3d/l3_driver_probe.mac` 只读（`Q PROJ` / `Q MDB` / `Q CE` / `Q NAME`，
  无 `SAVEWORK`、无 `QUIT`——收尾归 wrapper）。

## 4. 金基线对：制作与恢复（ADR-018 的执行剧本）

**目录约定**：

| 物件 | 工作副本（可写，每轮被恢复覆盖） | 金基线（只读，版本化） |
|---|---|---|
| E3D 项目 | `D:\AVEVA\Projects\E3D31-L3\AvevaMarineSample` | `D:\AVEVA\Projects\E3D31-L3-golden-v1\AvevaMarineSample` |
| Surreal 数据 | `.surreal/l3-suite-work` | `.surreal/l3-golden-v1` |
| 空间树文件 | 恢复时**删除** `accel_tree_AvevaMarineSample.bin*`，启动后按指针重建 | 不入金基线 |

**套件专用配置** `db_options/DbOption-l3-suite.toml`：Surreal `127.0.0.1:8048`（rocksdb 指向工作副本）、
服务 `http_api` `8028`、监控目录指向 E3D31-L3 工作副本、MDB 声明含 7997/7999/8000、
`manual_db_nums = [1112, 7997, 7999, 8000]`、ns `1516`、db `AvevaMarineSample`。
plant-ui 以 `PLANT_MODEL_API_URL=http://127.0.0.1:8028` 起。
端口独占：与日常开发（8009/8022）错开，套件启动前校验端口空闲。

**1112 必须进基线**：房间面板（PANE）在结构库 1112，`location_dbs` 早就指着它，但
`manual_db_nums` 一度只有三个设计库。该键已不管增量范围（那个只看 MDB 声明的 DESI），
可金基线制作走的正是全量生成 + 按需基线解析这条路——1112 缺席就没有面板几何，
F8 的 `room_relate` 只会算出空集，而且是「跑完了、绿了、但什么都没验到」的那种空。

**制作（一次性，v1）**：

1. 全停：无 des.exe、无 gen-model 服务、无 surreal 占用目标端口；
2. 复制 `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` → E3D31-L3 工作副本（连同项目 evars 结构，launcher 的 `projects_dir` 指向 E3D31-L3）；
3. 起套件栈，按 `DbOption-l3-suite` 初始化 7997/7999/8000 基线，等 pending 收敛到 0；
4. 停栈；工作副本与 `.surreal/l3-suite-work` 分别镜像为金基线 v1（设只读属性），
   记录三库的 `(file_latest_sesno, applied_sesno)` 进本文台账——**这对数字是恢复校验的判据**。

**恢复（每轮开跑前，runner 自动执行）**：

1. 全停校验（进程名 + 端口双查，有残留即拒跑）；
2. `robocopy /MIR` 双边恢复（项目 + surreal），删除空间树文件；
3. 校验：三库文件 latest sesno 与 surreal 水位 == 金基线记录值，**不成对即拒跑**——这是防「会话回退」阻断异常的硬闸。

那对数字由 `l3_suite --record-baseline` 从跑着的栈的 `/dbnums` 直接落成
`db_options/l3-golden-v1.json`（只收 `initialized` 的库），不靠手抄。

**无金基线模式**：`--project-dir` 直接开在目标项目上时没有金基线可对，此时必须同时给
`--skip-restore`（否则拒跑——`robocopy /MIR` 会把目标项目里金基线没有的东西删掉），
manifest 缺失也不再硬失败，而是在报告头把 `baseline: none` 写死。这一档只用于通道
排障与探索，**不产出可回写 §10 的轮次结论**：水位没有判据，就没有「这一轮干净地跑完了」
这回事。

**重铸条件**：场景需要新 dbnum 基线、或 E3D 项目本体升级 → 铸 v2 并回写本表。

## 5. 编排器设计（`src/bin/l3_suite.rs`）

**场景即数据**：`Scenario { id, dbnum, apply_macro, restore_macro: Option, focus_before: Option, focus_after: Option, refno, expect: Expect, rvm: bool }`，
判据枚举化（`Expect::Regen { roots } / TransformOnly { .. } / DataOnly / Deleted { .. } / Room`）。

`focus_before` / `focus_after` 是 V 级判据的落点，不是两个装饰字段：
`before.png` 按前者定位、`after.png` 按后者**重新**定位（改名场景按新名字），
`focus_after: None` 表示该节点必须**已从树上消失**，那句断言就是删除场景的 V 级判据。
单测 `every_scenario_declares_a_coherent_tree_focus` 守着这两列的自洽。

**尚未数据化的两处**（加场景时仍要碰编排逻辑，账记在这）：宏日志对拍是
`assert_macro_parity` 里按 `s.id` 的 match，M1 的 `geo_relate == 5` 是
`assert_scenario` 里的 `if s.id == "m1"`。对拍魔数（M2 的 `-6054.589`、F8 的 `5921.669`）
同时写在宏、runner 与本文三处，**金基线重铸时没有任何一处会报警**。

**每轮流程**：

```
金基线恢复(§4) → surreal 起 → 服务起(DB_OPTION_FILE=db_options/DbOption-l3-suite) → /health 就绪
→ plant-ui 起(EGUI_INSPECTION=1) → inspect tree 就绪探测
→ 逐场景串行：
   before.png（inspect tree 定位目标 → click 展开 → shot）
   E3dDriver 跑 apply 宏 → 轮询哨兵日志 + 进程退出（超时=杀 des 全家 + 场景 FAIL）
   POST /update/execute → 轮询 /tasks/{id} 终态（运行中抓 queue.png）
   库侧断言(§6) + 宏日志双侧对拍
   after.png → 幂等重跑 execute（F7 标配）→ repeat.png / 图像哈希
   有 restore 宏则再走一遍「宏→execute→断言回基线」（restore 本身就是反向增量用例）
→ teardown → output/l3-suite/<时间戳>/report.md
```

**失败纪律**：单场景 FAIL 不中断套件；**栈死也先把已跑场景的 report.md 写出来再报错**
（早退会把已经跑绿的场景证据一起丢掉）。E3D 超时按本次 pid 树清理，连
`pdmsconsole.exe` 一起，不按映像名误杀别的会话。

> **待办（与本文口径不符）**：`DEFAULT_TIMEOUT` 目前被复用在 5 个等待点
> （apply 宏 / 数据任务 / 房间轮 / restore 宏 / restore 任务），单场景最坏约 100 分钟，
> 「每场景硬顶 20 分钟」这条还没落实成一个贯穿场景的 deadline。

**CLI**：

| 参数 | 用途 |
|---|---|
| `--scenarios m1,m2` | 场景集（按给定顺序执行） |
| `--project-dir <path>` | 驱动哪个 E3D 项目；非金基线工作副本时必须同时给 `--skip-restore` |
| `--e3d-project` / `--e3d-login` / `--e3d-mdb` | 登录目标（默认 `AMS` / `SYSTEM/XXXXXX` / `/ALL`） |
| `--alive-timeout-secs` | 登录到命令循环的上限（默认 300） |
| `--check-driver <宏>` | 只验驱动，不起栈 |
| `--record-baseline` | 把当前栈的 `/dbnums` 写成金基线 manifest 后退出 |
| `--keep-stack` / `--skip-restore` | 调试用旁路，正式轮不允许 |

**报告头**：`report.md` 首表记场景集、项目目录、MDB、基线口径、用了哪些旁路、
栈是否中途死掉——§10 台账要的信息全在那儿，回写时照抄即可。
**依赖注记**：runner 不新增 HTTP 依赖，走 `curl.exe` 子进程。
**PS 薄入口**：`scripts/Run-L3Suite.ps1` 只做参数转发（两个 bin 用同一条
`cargo build` 命令建，特征集必须一致，否则整个 lib 会白重编一遍）。

## 6. 判据细则

**库侧不变量集**（全场景，按场景类型取子集）：

| # | 不变量 |
|---|---|
| I-1 | 水位推进到 `file_latest_sesno`，失败场景水位不动 |
| I-2 | 任务终态 `succeeded`，`result` 无 error；欠账单元 = 0。**房间 action 不算欠账**：`model_update_pending` 里的 `room_recalc_panel` / `room_recalc_element` 带着触发库的 dbnum 落在同一张表里，而它们由空闲轮单独收敛（ADR-011 §8），算进来会让任何一个房间没收干净的库永远判 FAIL。快照另存 `room_pending` 一列入证据、不参与断言，房间的收敛归 I-8 |
| I-3 | `inst_relate`/`geo_relate` 计数变化方向与场景声明一致 |
| I-4 | AABB / `world_trans`：位移场景平移量、几何场景尺度变化，坐标容差 0.01mm |
| I-5 | `pe` 属性值与宏写入值一致（对拍的库侧半边） |
| I-6 | 删除场景：`pe.deleted` 置位、实例与双向边清理 |
| I-7 | 幂等：第二次 execute up-to-date、零新任务、库侧零变化 |
| I-8 | 房间场景：`room_relate` 边集合与 `room_recalc.detail` 收敛数字 |

**双侧对拍**：apply 宏在 SAVEWORK 前 `Q` 出关键真值（POS/DESP/NAME/成员数）写哨兵日志；
runner 解析 E3D 原生输出格式，与 Surreal 侧 `pe`/`inst_relate` 对拍；坐标容差 0.01mm。
抓的是「解析层把值写歪」这类 L2 永远抓不到的缺陷。

**RVM 几何基准**（只挂 M1、后续 D-08）：沿 `2026-08-04_rvm-baseline-verification-plan.md` 的导出与
`rvm_baseline::compare` 入口；RVM 导出用独立 E3D 会话，不与场景会话混跑。

## 7. V 级证据规范

- 四件套：`<场景>-before.png / -queue.png / -after.png / -repeat.png`（或 after 哈希相等记录）；
- 相机与树展开状态由 inspect 探针固定（同一次 `inspect tree` 取坐标，不固化到文档）；
- **after 必须是队列完成后自动刷新的画面**——重启前端或手动全量重载得到的不算（总纲原话）；
- DataOnly 判「树文字变、几何不变、队列无生成单元」；删除判「树 + 几何同消失」；跨根判「两根都刷新」；
- 树侧判据已进 runner（`focus_before` / `focus_after`）：删除场景断言节点**找不到了**，
  新增场景断言新节点**出现了**，改名场景按**新名字**定位。四张图不再只采不判；

> **待办（V 级仍有缺口）**：`before.png` 不参与任何断言，唯一涉及图的判据是
> `after.png == repeat.png` 的**字节相等**。方向是反的——R4 那个「完成后不自动刷新」的
> 缺陷反而让两张图更容易相等，一个彻底冻住的界面必然 PASS。而且字节相等比「图像哈希
> 相等」严得多，任何计时器 / 悬浮态都会误报。这一条要么换成结构化判据（比较 inspect
> tree 快照而非像素），要么明确降级为「仅存证」。
- 探针实测基线：2026-08-06 `inspect shot` 已修复为后台 161–204ms 出图，套件不需要窗口前台。

## 8. 运行剧本与门禁

| 轮 | 场景 | 时机 | 预算 |
|---|---|---|---|
| 冒烟三连 | M1–M3 | 合入主干前手动跑，不绿不合 | ≤ 60 分钟 |
| 全量 | M1–M3 + F4–F8 | 发版 / 大重构前 | ≤ 3 小时 |

**铁律**（总纲三条全数继承 + 本文新增一条）：

1. 不动实库：套件只碰 E3D31-L3 副本与 `.surreal/l3-suite-work`；
2. 一份数据只起一个带 worker 的进程；
3. 只用 `bin/surreal.exe`（2.1.4）；
4. **套件运行期独占整套栈**：开跑前校验专用端口空闲、无残留 des.exe，脏环境拒跑。

## 9. 风险与已知约束

| # | 风险 | 处置 |
|---|---|---|
| R1 | TTY 两条快速出路都不通 | ENTRYMACRO 回退已验证，套件照常只是会话启动慢；不做 pdmsconsole 管道逆向（拷问会已裁定） |
| R2 | E3D 启动弹窗 / license 被占 | spike 时记录规避手法进 ADR-019；套件开跑前查 license 空闲 |
| R3 | 恢复不成对 → 会话回退阻断 | §4 恢复校验硬闸，不成对拒跑 |
| R4 | plant-ui 队列完成后自动刷新的既有缺陷（M-B 系列） | D-12 类场景不入首批；after 图拍不到自动刷新按 FAIL 记录并挂 plant-ui issue，不在本套件里绕 |
| R5 | apply 宏 SAVEWORK 前死掉 | 无 SAVEWORK 即无新 session，金基线重跑即可；SAVEWORK 后死属正常增量路径 |
| R6 | E3D license 单机、套件串行 | 接受；不进无人 CI（拷问会已裁定） |

## 10. 里程碑与台账

**里程碑**：

| 周 | 目标 |
|---|---|
| W1 | 通道 spike 定标（ADR-019）+ 金基线 v1 制作 + runner 骨架 + M1 单场景走通 |
| W2 | 冒烟三连全绿 → 启用合入门禁；开写 F5/F6 新宏 |
| W3 | 八场景全绿 → 启用发版门禁 |

**轮次台账**（每轮回写，不回写视为没跑）：

| 日期 | 轮型 | 通道 | 金基线版本 | 结果（场景 PASS/FAIL 明细） | 证据目录 |
|---|---|---|---|---|---|
|  |  |  |  |  |  |

回写规则：FAIL 场景在备注列写第一个失败判据（I-x 编号）与证据路径；
金基线重铸、场景表增删随轮回写 §2/§4 对应表格。
