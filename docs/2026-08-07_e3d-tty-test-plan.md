# E3D TTY 实机测试完善计划：以 AMS 实库几何类型为纲

日期：2026-08-07
状态：草案（plannotator 批注中，第 2 稿）
被测通道：ADR-019 无人值守 TTY（`des.exe -tty` + `AVEVA_DESIGN_ENTRYMACRO`）

## 0. 定位：这份计划管什么

「E3D TTY 测试」= 用无人值守 TTY 通道驱动真实 E3D 改数并 SAVEWORK，再走生产增量管道，
在 Surreal 库侧做精确断言。按载体拆三层：

| 层 | 内容 | 载体 | 归属文档 |
|---|---|---|---|
| A 通道层 | launcher / driver 本身：登录、宏投递、哨兵、超时、清理、退出码 | `scripts/e3d/run_ams_c_entrymacro.bat` + `launch_detached.ps1` + `E3dDriver`（`l3_suite --check-driver`） | **本文** |
| B 矩阵层 | AMS 实库几何类型（noun）× 增量变更的逐类型验证 | `scripts/e3d/ams_model_type_cases.json` + `Run-RoomE3DE2E.ps1 -ModelTypes` + `tests/issue7_e2e_increment.rs` | **本文** |
| C 场景层 | 房间语义场景（五连）与 L3+V 全链路（M/F 系列） | `Run-RoomE3DE2E.ps1` legacy cases、`l3_suite` | `2026-08-06_room-incremental-automation-test-plan.md`、`2026-08-06_e3d-l3-automation-test-plan.md` |

C 层已有专文，本文只引用不重抄。本文的增量价值 = **矩阵以实库几何类型全集为纲收尾 +
通道失败路径补零 + 编排/门禁把手**。

## 1. AMS 实库几何类型清单（2026-08-07 实测，测试矩阵的「纲」）

实测口径：`ws://127.0.0.1:8009` · ns `1516` · db `AvevaMarineSample`
（datastore `.surreal/ams-7997-e3d-test-20260805`）。

```sql
RETURN array::sort(array::distinct(SELECT VALUE in.noun FROM inst_relate));
SELECT noun, count() AS c FROM (SELECT in.noun AS noun FROM inst_relate) GROUP BY noun;
```

**结论：实库有几何实例的类型共 41 个，与 `ams_model_type_cases.json` 名单完全一致
（零 missing / 零 stale）。** 逐类型清单（实例数为当日实测，供选靶与风险评估）：

| noun | 实例数 | dbnum | 覆盖状态 | 备注 |
|---|---:|---|---|---|
| ANCI | 137 | 8000 | pending | 无独立几何（`expect_geometry:false`） |
| ATTA | 222 | 8000 | verified | `f0a71be1` |
| BATT | 1 | 7997 | verified* | **唯一样本** |
| BEND | 819 | 8000 | verified | |
| BOX | 9903 | 7997 | verified | existing 模式（尺寸场景） |
| BRAN | 18 | 8000 | pending | 隐式管（TUBI）网格挂在 BRAN/FTUB 实例上 |
| CAP | 1 | 7999 | verified | existing 模式（issue7 血统）；**唯一样本** |
| CONE | 603 | 7997 | verified | |
| CTOR | 301 | 7997 | verified | |
| CYLI | 10654 | 7997 | verified | existing 模式（尺寸场景） |
| DAMP | 2 | 7997 | verified* | 样本仅 2 |
| DISH | 68 | 7997 | verified | |
| ELBO | 164 | 8000 | verified | |
| ELCONN | 34 | 8000 | verified | `f0a71be1` |
| EXTR | 9102 | 7997 | verified | |
| FITT | 117 | 8000 | pending | selector 选靶（首例） |
| FIXING | 788 | 8000 | pending | |
| FLOOR | 205 | 7997 | verified* | |
| FTUB | 2026 | 8000 | verified | `f0a71be1` |
| GENSEC | 398 | 8000 | pending | |
| GWALL | 62 | 7997 | verified* | 跨房搬迁（带 expected_edges） |
| NBOX | 2691 | 7997 | verified | 负体 |
| NCON | 88 | 7997 | verified | 负体 |
| NCTO | 185 | 7997 | verified | 负体 |
| NCYL | 6976 | 7997 | verified | 负体 |
| NDIS | 32 | 7997 | verified | 负体 |
| NOZZ | 11 | 7997 | verified* | 正体（dict 已澄清非负体） |
| NPYR | 305 | 7997 | verified | 负体 |
| NREV | 84 | 7997 | verified | 负体 |
| NRTO | 723 | 7997 | verified | 负体 |
| NXTR | 1727 | 7997 | verified | 负体 |
| PANE | 3182 | 7997 | pending | 面板，走 `room_recalc_panel` 分支 |
| PYRA | 782 | 7997 | verified | |
| REDU | 1 | 8000 | verified | **唯一样本** |
| REVO | 5 | 7997 | verified* | 样本仅 5 |
| RTOR | 1033 | 7997 | verified | |
| SCTN | 1014 | 7324 | pending | 7324 已入快照（见 §3.2） |
| SJOI | 212 | 7324 | pending | 同上 |
| SNOU | 337 | 7997 | verified | `f0a71be1` |
| STRT | 5 | 7997 | verified* | 样本仅 5 |
| WALL | 5 | 7997 | verified* | 样本仅 5 |

`verified*` = 今晨工作区未提交的转正（BATT / DAMP / FLOOR / GWALL / NOZZ / REVO /
STRT / WALL 八个）。汇总：**33 verified / 8 pending**（pending = ANCI、BRAN、FITT、
FIXING、GENSEC、PANE、SCTN、SJOI）。

### 1.1 类型边界：pe 里有、但**不进**矩阵的类型（逐一定性，防「以为漏了」）

| 类型 | pe 存量 | dict flag（`noun_flags.json`） | 定性 | 覆盖归属 |
|---|---:|---|---|---|
| PAVE | 121,663 | 非几何（三 flag 全 false） | 船体面板附属数据，无几何实例 | 不测 |
| POGO | 14,949 | **primitive=true** | dict 认几何但未入生成路由（stage3 缺口清单在册） | `docs/plans/generation-coverage-align.md`；路由收编后经 §3.3 T-OR-4 门禁自动逼矩阵加行 |
| HANG | 7,300 | 非几何 | 交付单元容器，子件带几何 | L3 计划 F 系列（生成根口径） |
| SUPPO | 2,550 | 非几何 | 同上 | 同上 |
| CWALL | 1,850 | 非几何 | 由子件/目录几何表达 | 已有 D-06 血统用例（`2026-07-29` 总结 §5） |
| EQUI | 1,323 | 非几何 | 交付单元容器 | L3 计划 F 系列 |
| TMPL | 318 | 非几何 | 模板 | 不测 |
| TUBI | 0 | primitive=true | **隐式管**：pe 无独立元素，网格挂 BRAN/FTUB 实例 | 矩阵 FTUB/ELBO/BEND 行间接覆盖；合成轨 RF9 精确覆盖 |

「dict 认几何却无 mesh」的全集核对不归本文——那是生成覆盖对齐计划的验收
（`AIOS_GEOM_COVERAGE_AUDIT=on` 的动态审计）。本文只承诺：**该计划每收编一个新
noun 进路由，`Test-AmsModelTypeCoverage.ps1` 的 missing 检查会立刻红，逼矩阵加行**
（见 §3.3 T-OR-4）。

### 1.2 变更种类维度（矩阵当前只测「位移」，边界写死）

| 变更种类 | 现状 | 归属 |
|---|---|---|
| 相对位移（`BY U 10` / `BY D 10`） | 矩阵全类型标配 | **本文矩阵轨** |
| 尺寸/参数（XLEN、DIAM、DESP…） | BOX / CYLI（legacy 尺寸场景）、DAMP DESP（L3 M1） | legacy + L3 计划 |
| 改名 / 删除 / 新增 / 跨根搬迁 | L3 计划 F4 / M3 / F5 / F6 | L3 计划 |
| 房间归属语义 | 房间五连 + RS/RF/RL | 房间计划 |

可选扩展（P8，默认不做）：按几何家族各补一个尺寸变更行——正体 prim（已有 BOX）、
负体 prim（如 NCYL）、挤出类（EXTR 顶点）、目录参数类（已有 DAMP DESP）。做的话
case 表加 `apply_command` / `restore_command` 即可表达，不动编排逻辑。

## 2. 现状盘点与缺口

**通道（A 层）**：正常路径已定标并实跑多轮（ADR-019：冷启动至退出 2.99s、退出码 0、
哨兵三段齐、零残留）；`--check-driver` 独立入口可用；场景宏禁 `QUIT` 有单测护栏。
**失败路径零自动覆盖**（错登录、宏超时、清理定向性、退出码矩阵、密码泄漏检查）。

**矩阵（B 层）**：§1 清单，33/41，今日推进 12 个（4 已提交 + 8 未提交）。

**编排**：`Run-RoomE3DE2E.ps1` 已有 surreal 自起、逐案 apply→增量→restore→增量、
finally 恢复纪律；覆盖度检查器可用。但**首个失败即中止整轮**、无 report.md、
无台账约定、无幂等重跑步骤、覆盖度未挂门禁。

| # | 缺口 |
|---|---|
| G1 | 矩阵 8 类型 pending（各自障碍见 §3.2；SCTN/SJOI 经实测**无**基线障碍） |
| G2 | 通道失败路径零自动覆盖 |
| G3 | 矩阵轮无失败隔离、无汇总报告 |
| G4 | 幂等重跑判据（I-7 / RI-11 同款）未进矩阵轨 |
| G5 | 覆盖度检查器未挂门禁，manifest 与实库漂移靠人想起来跑 |
| G6 | case 表 room / panel / expected_edges 魔数在快照重铸时无报警 |
| G7 | 唯一样本类型（BATT/CAP/REDU，样本=1）restore 失败即永久损伤唯一靶子 |

## 3. 测试项

### 3.1 通道层 T-CH（负向为主）

| ID | 名称 | 做法 | 判据 | 状态 |
|---|---|---|---|---|
| T-CH-1 | 探针冒烟 | `l3_suite --check-driver scripts/e3d/l3_driver_probe.mac` | 退出码 0；日志含 `L3-PROBE-ALIVE`/`L3-PROBE-DONE` 与 `Q` 输出 | 已有，升级为每次实机轮前置 |
| T-CH-2 | launcher 前置校验退出码 | 宏不存在→2；project evars 不存在→3；`des.exe` 不存在→4（假 `L3_E3D_INSTALL_DIR`） | 退出码逐一命中，stderr 指名报错 | 新增 |
| T-CH-3 | 登录失败收摊 | 错密码（`L3_E3D_LOGIN=SYSTEM/WRONG`）+ `--alive-timeout-secs 60` | 报错含 `never reached the command loop`；本次 des/pdmsconsole 零残留；pid 文件被清 | 新增 |
| T-CH-4 | 宏超时收摊 | 死循环/长睡宏 + 短 `L3_E3D_TIMEOUT_SECONDS` | launcher 退出 124；报错含 `ran past`；本次 pid 树清理干净 | 新增 |
| T-CH-5 | 清理定向性 | T-CH-4 超时清理时另起一个无关 des 同名假进程 | 只杀本次 pid 树，对照进程存活 | 新增；自动化代价高则降级为 runbook 人工步骤并写明结论 |
| T-CH-6 | 场景宏 QUIT 纪律 | 单测 `scenario_macros_leave_session_shutdown_to_the_driver` + runner ensure | 含 QUIT 的宏拒跑 | 已覆盖 |
| T-CH-7 | 密码不入证据 | 一条脚本断言：扫描 driver.log / 生成宏 / report 无登录密码明文 | 全部产物 grep 不中 | 新增 |
| T-CH-8 | 会话耗时基准 | 每次 driver 运行把耗时记进 report | 冷启动异常偏离（阈值待定标，暂定 >60s）只警不 FAIL | 新增 |

T-CH-2 不占 E3D license；T-CH-3/4/5 各消耗一次会话，串行跑。全部不碰项目数据，
不需要金基线。

### 3.2 矩阵层 T-MT（8 个 pending 收尾，纲对 §1 清单）

| ID | noun | 障碍（2026-08-07 实测后口径） | 收尾动作 |
|---|---|---|---|
| T-MT-1 | ANCI | 无独立几何（`expect_geometry:false` 今晨刚加，未实跑） | 直接跑；判据走「无 AABB、无房间任务」分支 |
| T-MT-2 | FIXING | 无（纯没跑） | 直接跑 |
| T-MT-3 | GENSEC | 无（纯没跑；注意 GENSEC 可能自己就是生成根颗粒） | 直接跑；若 regen 根不是自身，把实际根记进 case 备注 |
| T-MT-4 | FITT | 首个用 `selector`（`CE /-RX-CCV-S2020-V1/F1`）选靶的案例 | 先 `--check-driver` 验证生成宏能选中，再进增量 |
| T-MT-5 | BRAN | 整支管移动，影响面大（18 个 BRAN 实例、隐式管网格随动） | 先 `--check-driver` 试宏并预估耗时；目标集不稳定就用 dynamic_baseline 口径 |
| T-MT-6 | PANE | 面板自身移动走 `room_recalc_panel` 分支；矩阵轨 change 固定 `element`，`change='room'` 的 target 又写死成 issue7 面板 | 编排改动：case 加 `change` 字段并把 panel target 参数化；超预算则先手工单跑 + case 表标注降级口径 |
| T-MT-7 | SCTN | ~~缺基线~~ **已排除**：`pe:15516_102` 在库、7324 水位 1365/1365、`ams000/ams7324_0001` 文件在（5.4MB） | 直接跑（`Set-CaseEnv` 的文件模板恰好命中默认路径） |
| T-MT-8 | SJOI | 同上（`pe:23708_28532` 在库） | 直接跑 |

前置 **T-MT-0**：跑 `Test-AmsModelTypeCoverage.ps1`，missing / stale / duplicate
必须为空——manifest 与实库几何类型集合先对齐再谈覆盖（§1 已实测通过一次，
每轮矩阵前重跑）。
终态 **T-MT-9**：`-ModelTypes all` 全矩阵一轮全绿（38 个 `relative_position`），
随后 `-RequireVerified` 过闸。

### 3.3 编排与证据 T-OR

| ID | 内容 | 判据 | 状态 |
|---|---|---|---|
| T-OR-1 | 矩阵轮失败隔离：单类型断言失败（restore 仍执行）不中止整轮，轮尾汇总 | 一轮产出全类型 PASS/FAIL 表 | **已落地 2026-08-07** |
| T-OR-2 | report.md：逐案结果、首个失败断言、宏/日志路径、逐阶段耗时 | 台账（§8）可照抄 | **已落地 2026-08-07** |
| T-OR-3 | 幂等步骤：restore 收敛后每案例追加一遍（`AIOS_ROOM_IDEMPOTENT=1`） | 生产水位守卫成立（file == applied，空区间不入队）、drain 消费 0、水位/归属边/AABB/拓扑全部不变、无本案例房间任务 | **已落地 2026-08-07** |
| T-OR-4 | 覆盖度门禁：矩阵轮收尾必跑 T-MT-0 + `-RequireVerified`；生成路由每收编新 noun，missing 变红逼矩阵加行 | 不绿不回写台账 | 待办 |
| T-OR-5 | 前置环境闸：8009 端口占用、残留 des.exe / pdmsconsole 检查，脏环境拒跑 | 与 L3 套件铁律同款 | 待办 |

T-OR-3 实现注记：幂等轮不硬调 `BatchScheduler::enqueue`——水位持平的空区间在
`batch_queue` 规则下本就不入队（生产入口的守卫拦在更早的发现阶段），硬调只会踩进
「入队判定却无排队行」的失步告警分支。测试断言的是守卫本身 + 队列消费为零 +
库侧四项快照不变。

失败隔离只隔离「断言失败」，不隔离「恢复失败」：restore 失败沿用房间计划 RI-15，
立即终止剩余破坏性场景。

### 3.4 场景层（引用，不重抄）

- 房间五连（same-room / element-out / room-rename / box-size / cyli-size）：
  房间相关改动后跑，门禁归房间计划 RG3；
- L3+V M1–M3 / F4–F8 与金基线成对恢复：归 L3 计划 §8。

## 4. 执行剧本

```powershell
# 通道健康（不起栈，约 1 分钟）
cargo build --bin l3_suite -j 1
target\debug\l3_suite.exe --check-driver scripts/e3d/l3_driver_probe.mac `
    --project-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample

# 单类型（先编测试 exe 复用，避免每案例重编）
cargo test --features http_api --test issue7_e2e_increment --no-run
scripts\Run-RoomE3DE2E.ps1 -SkipLegacyCases -ModelTypes FIXING -TestExe <上一步产物路径>

# 全矩阵
scripts\Run-RoomE3DE2E.ps1 -SkipLegacyCases -ModelTypes all -TestExe <同上>

# 覆盖度
scripts\Test-AmsModelTypeCoverage.ps1 -RequireVerified
```

环境：`DB_OPTION_FILE=db_options/DbOption-issue7-e2e`、8009 由脚本自起
（datastore `.surreal/ams-7997-e3d-test-20260805`）、`GEN_MODEL_DIRECT_INCREMENT=1`、
`RUST_MIN_STACK=134217728`（脚本已内置）。

## 5. 判据（沿用既有口径，不另造）

- **库侧**：`issue7_e2e_increment` 现有断言集——noun 基线、（动态）房间边基线、
  水位推进到 `file_latest_sesno`、AABB 随位移变化 / 无独立几何者无 AABB、
  房间任务排队与 `expect_geometry` 一致、归属边与拓扑按场景收敛；
- **通道侧**：本次 launcher 退出码 0、`L3-ALIVE` / `L3-DONE` 均存在、场景日志可读
  （ADR-019 成功判据）；
- **双侧对拍**：宏内 `Q POS` 前后值 ↔ 库侧 POS / AABB，坐标容差 0.01mm（RI-13 口径）；
- **幂等**：第二次执行零工作、库侧零变化（I-7 / RI-11 口径）。

## 6. 门禁与预算

| 轮型 | 内容 | 时机 | 预算 |
|---|---|---|---|
| 通道健康 | T-CH-1 | 每次实机轮前置 | ~1 分钟 |
| 通道失败路径 | T-CH-2/3/4(/5)/7 | bat / ps1 / `E3dDriver` 代码改动后 | ~10 分钟 |
| 单类型矩阵 | 指定 noun | 生成 / 房间路径改动后定向回归 | **实测 ~2.6 分钟/类型**（2026-08-07 WALL：apply-driver 4s + apply-incr 134s + restore-driver 4s + restore-incr 11s + 幂等 2s；apply-incr 大头是目录闭包预载与根重生成） |
| 全矩阵 | T-MT-9 | case 表大改 / 发版前 | 按单类型实测折算 ≈ 2 小时量级（38 类型；跑完全矩阵后回写实测值） |
| 覆盖度 | T-MT-0 + `-RequireVerified` | 每次矩阵轮收尾 | <1 分钟 |

E3D license 单机、会话串行，全矩阵轮不进无人 CI（沿既有拷问会裁定）。

## 7. 风险与已知约束

| 风险 | 处置 |
|---|---|
| 唯一样本类型（BATT/CAP/REDU=1，DAMP=2，REVO/STRT/WALL=5）restore 失败即损伤唯一靶子 | RI-15 红线之上加一条：唯一样本类型的 restore 失败必须当轮人工介入修复并记台账，不许留到下一轮 |
| PANE 需要编排改动 | 改动超预算就先手工验证 + case 表标注；「全矩阵全绿」口径按 §8 P3 的取舍二选一写死，不许含糊 |
| case 表魔数漂移（快照重铸无报警） | 每次重铸快照必跑 T-MT-0 + 全矩阵；case 表补「定标来源」注释字段（轻量），不做自动报警 |
| 失败隔离改造引入恢复漏洞 | RI-15 红线：restore 失败立即终止整轮 |
| license 被占 / 会话残留 | T-OR-5 前置闸，脏环境拒跑 |
| 生成路由收编新 noun 后矩阵滞后 | T-OR-4：missing 检查变红即拒绝回写台账，逼加行 |

## 8. 实施顺序

| # | 内容 | 依赖 |
|---|---|---|
| P0 | 提交今晨 case 表工作区变更（8 类型 verified + ANCI flag + PANE 换靶 + GWALL 跨房边）——先落账再继续 | – |
| P1 | T-MT-1..4（ANCI / FIXING / GENSEC / FITT，无编排改动，纯执行） | P0 |
| P2 | T-MT-7/8（SCTN / SJOI，实测无障碍）+ T-MT-5（BRAN） | P0 |
| P3 | PANE 编排支持（case 加 change / panel-target 字段），或明确降级口径 | P0 |
| P4 | ~~T-OR-1/2/3（失败隔离 + report + 幂等步骤）~~ **已完成 2026-08-07**：`Run-RoomE3DE2E.ps1` 失败隔离 + `report.md` + 幂等轮；`issue7_e2e_increment.rs` 增 `AIOS_ROOM_IDEMPOTENT` 模式 | – |
| P5 | T-CH-2/3/4/7（通道失败路径） | –（与 P1–P3 不占同一份数据，可并行） |
| P6 | 全矩阵一轮全绿（T-MT-9）→ 回写台账 → `-RequireVerified` 挂闸 | P1–P4 |
| P7（可选） | 变更种类扩展：负体 / 挤出各补一个尺寸变更行（§1.2） | P6 |

P4 必须在 P6 前完成，否则全矩阵轮一个类型失败就丢整轮证据。

## 9. 轮次台账（每轮回写，不回写视为没跑）

| 日期 | 轮型 | 类型/测试项 | 结果 | 证据目录 | 首个失败判据 |
|---|---|---|---|---|---|
| 2026-08-07 上午 | 单类型×12 | ATTA/ELCONN/FTUB/SNOU（`f0a71be1`）+ BATT/DAMP/FLOOR/GWALL/NOZZ/REVO/STRT/WALL（工作区） | PASS，转 verified | output/room-e3d-e2e/…（本机） | – |
| 2026-08-07 下午 | 单类型（P4 验证轮） | WALL（新编排链路：失败隔离 + report.md + 幂等轮 T-OR-3 首验） | PASS（1/1，幂等轮零工作） | output/room-e3d-e2e/p4-live-wall | – |
