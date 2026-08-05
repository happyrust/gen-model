# 增量更新测试计划 v3：数据 → 模型 → 任务队列（含 plant-ui）

日期：2026-08-04（**现状列于 2026-08-05 回写**，见各表；用例定义与判据未改）
被测仓：`D:\work\plant-code\old\gen-model`（服务端）+ `D:\work\plant-code\old\plant-ui`（桌面壳）
前置审核：`docs/2026-08-04_increment-update-working-tree-audit.md`（本计划的 BL/WD/QW
系列新用例全部来自它的 N1–N4 / W1–W2 结论）

> **2026-08-05 一句话现状**：Gate 0 的依赖已解开（`bba03d7a`），WD 全系列绿，
> BL-01…03 与 QW-01 的**纯函数半边**绿；**所有实库 / 实机判据一条未跑**。
> §6「不能宣称」的三句话因此原样成立。

## 0. 与既有文档的关系

| 文档 | 它还管什么 | 本文与它的关系 |
|---|---|---|
| `2026-07-27_increment-update-complete-test-plan.md` | S0–S13 阶段矩阵、L0–L4 分层、F0–F3 夹具、IU-INV 不变量 | **仍然有效**，是数据阶段的细粒度台账；本文不重抄它的逐条矩阵，只引用其 ID |
| `2026-07-25_…complete-matrix-v2.md` | core.dll 能力矩阵、noun 变化等价类（A/B/C/D 批） | 模型阶段的等价类来源 |
| `2026-07-29_test-ams-incremental-update-summary-report.md` | D-01～D-15 场景矩阵、plant-ui 验收流程（§7） | 模型阶段与视觉闭环的骨架，本文修订其环境口径（§1） |
| `plant-ui/docs/plans/queue-live-acceptance.md` | 队列视图真服务验收手册 | 队列阶段的执行手册，本文补充 8-04 修复后的新增条目 |
| **本文** | **三阶段总纲 + 审核驱动的新增用例（BL/WD/QW）+ 门禁顺序** | 执行层面以本文为准 |

分层记号统一如下（旧文档两套记号的对照）：

| 本文 | 07-27 计划 | 07-29 报告 | 含义 |
|---|---|---|---|
| L0/L1 | L0/L1 | A | 纯函数 / 源码契约，`cargo test --lib`，不连库 |
| L2 | L2 | B | 隔离实库（SurrealDB 副本），`--ignored` 定靶 |
| L3 | L3 | C | 真实 E3D 会话端到端 |
| V | —（Gate 5） | D | plant-ui before/queue/after 视觉闭环 |

**三条铁律**（历次踩坑总结，任何用例不得违反）：

1. 不动实库：一律用数据目录副本（8-04 复验的 `.surreal/site-8000-incrtest` 手法）。
2. 一份数据只起一个带 worker 的进程（worker 无条件 spawn，双消费者破坏 FIFO）。
3. 只用 `bin/surreal.exe`（2.1.4）。PATH 上的 3.x 打开过的目录会被写成
   format_version 7，2.1.4 再也打不开——`.surreal/ams-8009` 已经这样报废了。

---

## 1. 测试栈与环境

| 部件 | 入口 | 判据 |
|---|---|---|
| SurrealDB | `bin\surreal.exe start --bind 127.0.0.1:8009 rocksdb:<副本目录>` | 2.1.4；副本目录从 `.surreal/site-8000` 复制 |
| 模型服务 | `cargo run --release --features http_api`（8022） | `/api/v1/health` 的 `version`、`worker_alive`、`static_assets` |
| plant-ui | `PLANT_MODEL_API_URL=http://127.0.0.1:8022` + `EGUI_INSPECTION=1` 起 `plant-ui-app` | `inspect tree/shot/click` 探针可用 |
| 身份三元组 | 两侧 `project` / `mdb` / `namespace` 必须一致 | 不一致时界面写操作被闸门拦下（这本身是队列阶段用例 Q-11） |

环境已知坑（写进计划防重踩）：

- `.surreal/ams-8009` 与 `D:\backup-dbs\ams-8009.db` 已损坏（format_version 7），不可用；
- ~~服务端 `DbOption.toml` 与发布包 `pc/DbOption.toml` 在 8-04 复验中临时改过
  `manual_db_nums` / `mdb_name`，用完必须恢复~~ → **2026-08-05 起不必再这么做**，
  见下方定靶方式；历史记录里的临时改动仍需按证据文档末尾的原值恢复；
- 范围口径按 ADR-0013：**当前 MDB 声明的 DESI**，`manual_db_nums` 只是旧口径退路，
  用了要在记录里注明。

**live 测试定靶方式（2026-08-05 起）**：`aios_core` 已升到带 `DB_OPTION_FILE`
的 rev（`bba03d7a`，详见审核文档 §3 补记）。定靶用环境变量，**不要再改仓库根
`DbOption.toml`**：

```powershell
$env:DB_OPTION_FILE = "db_options/DbOption-e2e-8042"   # 不带 .toml 后缀
cargo test --lib -- --ignored --exact <测试名> --nocapture
```

缺省仍是 `DbOption`，所以服务与既有脚本不受影响。副本库 / 一次性 NS 的配置
放 `db_options/`。

## 2. 门禁顺序（Gate）

| Gate | 内容 | 退出条件 | 阻塞谁 | 现状（2026-08-05） |
|---|---|---|---|---|
| **G0 可执行性** | 升 `aios_core` 钉版到带 `DB_OPTION_FILE` 的 rev（审核 N4）；建 `db_options/` 放副本/一次性 NS 配置 | 任取一个 `live_*` 用 `$env:DB_OPTION_FILE` 定靶跑通 | 一切 L2/L3 | **依赖已升**（`bba03d7a` + 上游 `65caaef`/`1dd7fd4`，依赖图收敛到单一 `aios_core`）；**退出条件的那次定靶运行仍欠**，`db_options/` 也还没建 |
| **G1 数据阶段红转绿** | BL-01…04 + WD-01…06 落地（先红后绿） | `cargo test --lib` 全绿且新增用例逐条有「回退即红」记录 | 模型阶段的基线依赖 | **WD 全绿**（`c35e4ece`，14 条单测）；**BL-01/02/03 的 L0 半边绿**（`111186b2`）；**BL-04（SC-002）仍红**，它本来就是 L2 对拍 |
| **G2 队列健壮性** | QW-01（饿死回归，现红）+ QW-02…04 实机 | 副本库上排队批次在积压消化期间按期开跑 | 队列阶段全部实机项 | **L0 半边绿**（`4f46ebcc`：空闲轮分类 / 阻断按 dbnum 收窄 / 房间轮 10 分钟地板，各配回退即红测试）；**实机半边全部未跑** |
| **G3 模型阶段实库** | S10/S11 的 live 系列 + D-01…D-15 的 B/C 级复跑 | 每条有可复现通过记录 + 日志归档 | 视觉闭环 | **进行中**：补充锚点 DG-01（7997 DAMP DESP DirectGeometry）已通过 B/C/L2/L3；D-01…D-15 仍待复跑 |
| **G4 视觉闭环** | D-01…D-15 的 plant-ui before/queue/after | 每例三图 + 重复执行无抖动 | 发版宣称 | **进行中**：DG-01 的 plant-ui 属性刷新和三维加载已通过，但同一轮 before/queue/repeat 四图未齐，尚不计严格 V 级；D-12 另受 plant-ui B1 阻塞 |

顺序理由：G0 之前点亮实库测试 = 把「测试失败」和「夹具没配对」混在一起（07-27
计划原话，至今成立）；G1 的基线修复不先做，模型阶段在 SYS meta 库上永远带着
四条失败批次跑；G2 不先做，长积压副本上的一切队列观察都不可信。

---

## 3. 阶段一 · 数据的测试验证

**范围**：文件发现 → 扫描准入 → 水位 → 窗口 → 收集 → 折叠 → 落库 → 耐久。
断言对象只有 `pe` / `pe_owner` / `dbnum_watermark` / `dbnum_info_table` /
`increment_update_attempt`，**不看任何模型表**。

### 3.1 继承的矩阵

- `IU-S0`…`IU-S4`、`IU-S8`、`IU-S9`、`IU-S12`、`IU-S13`（07-27 计划逐条台账）；
- 跨阶段不变量 `IU-INV-01`（预览零副作用）、`IU-INV-02`（水位单调）、
  `IU-INV-03`（重放幂等，跑法在 07-27 §5）、`IU-INV-06`（收集一次）。

07-27 计划里 S2（窗口解析）零测试、IU-S1-05/06 缺失的状况**至今未变**，
随 G1 一并补。

### 3.2 新增：BL 系列（基线初始化，来自审核 N1/N2）

| ID | 断言 | 层 | 现状（2026-08-05 回写） |
|---|---|---|---|
| BL-01 | SYS meta 基线完整性判定把根/world 行计入口径：`PE=225, 解析=224` 的库必须初始化成功并推水位 | L0（把守卫抽成纯函数）+ L2 | **L0 绿**（`111186b2`：`baseline_parse_matches(pe, root, parsed)`）；L2 待跑 |
| BL-02 | `PE=1` 纯根库走空基线出口：推水位、计划为空、不再反复失败 | L0 + L2 | **L0 绿**（`111186b2`：`baseline_parse_confirmed_empty`）；L2 待跑 |
| BL-03 | 基线解析**失败**后，下一轮该库必须重新入队初始化，不得因 `dbnum_info_table` 残行被判 up_to_date | L2 | **L0 绿**（`111186b2`：`resolve_read_applied` 分支 + 守护测试 `an_existing_failed_state_never_inherits_the_info_table_watermark`）；L2 待跑 |
| BL-04 | SC-002：对同一磁盘现状，`GET /dbnums` 的 `initialized/blocked/excluded` 与 execute 的实际处置**逐库一致**，差异数为 0 | L2 脚本对拍 | **仍红**（8-04 实测差异 4；BL-03 的读路径已修，需重测才知道差异是否归零） |
| BL-05 | 空库（7998 形态）基线：0 元素 0 单元，水位照推（合法空基线分支不回归） | L2 | 绿（8-04 已验，固化成可重复用例） |

BL-01/02 的实现建议：把 `count / info_count / parsed_count / applied_sesno` 四个
标量的裁决抽成纯函数（仿 `baseline_needs_full_parse`），先在 L0 钉死口径，再用
副本库各跑一遍 L2。**修复与测试同一笔提交，回退即红。**
→ 已按此实现（`111186b2`），L0 口径钉在 `baseline_parse_matches` /
`baseline_parse_confirmed_empty` 两个纯函数上；**L2 那一遍是现在欠的**。

### 3.3 新增：WD 系列（监控目录解析，来自审核 W1/W2/W3）

全部 L0，落在 `project_paths.rs` 的 `#[cfg(test)]`，随未提交改动一起进版本库。

**全系列已于 `c35e4ece` 随模块同批提交并转绿**，共 14 条单测（含 4 条计划外的
去重 / 掉盘重挂用例）。逐条对应关系：

| ID | 断言 | 对应测试 |
|---|---|---|
| WD-01 | `normalize_path_input`：正斜杠 UNC → 反斜杠；剥两侧引号；非 Windows 原样 | `unc_inputs_are_normalized_and_absolute` |
| WD-02 | `is_absolute_input`：`\\host`（缺 share 段）也算绝对；`"D:/x"` 带引号算绝对；`ams` 不算 | `unc_inputs_are_normalized_and_absolute`、`absolute_and_unc_entries_are_used_as_is` |
| WD-03 | `resolve_project_root`：`project_dirs` 绝对条目直用；相对条目拼 `project_path`；`project_dirs` 比 `included_projects` 短返回 None 不 panic | `absolute_and_unc_entries_are_used_as_is`、`short_project_dirs_yields_none_instead_of_panicking` |
| WD-04 | `plan_watch_dirs` 回退分支（`included_projects` 为空）：相对 `project_dirs` 条目**也要能解析**（修 W2） | `relative_entries_resolve_when_the_project_list_is_empty`（选了「让它能解析」这条，改成 `base.join(...)`） |
| WD-05 | `path_starts_with`：`D:/AVEVA/X` vs `d:\aveva\x` 判同；前缀相等判真；`C:\ab` 不是 `C:\a` 的孩子 | `prefix_match_ignores_case_and_separator_style` |
| WD-06 | `collect_db_dirs_in`：根目录本身 `*000` 直接认；`ams1000` **不**认（修 W3）；`read_dir` 失败只丢该项目 | `a_root_that_is_itself_a_db_dir_is_used_directly`、`a_directory_that_merely_ends_in_zeros_is_not_a_db_dir`、`one_unreadable_project_does_not_erase_the_others` |

WD 系列跑通前，`project_paths` 改动不应提交——这是 spec 001 FR-011 纪律的延续。
（该纪律已被遵守：模块与测试同在 `c35e4ece` 一笔提交里。）

### 3.4 数据阶段的 plant-ui 面

| ID | 看什么 | 判据 | 出处 |
|---|---|---|---|
| DU-01 | 预览窗范围三分 | 「DESI 在范围 / 声明但够不着 / 排除（非 DESI 与不在 MDB 名单分开数）」三个数与 `/dbnums` 一致 | ADR-0013、CONTEXT.md「排除/够不着」 |
| DU-02 | 预览回执 mdb 回显 | `ManualUpdatePreview.mdb` 与本端配置一致，不一致必须可见 | QUEUE-FIELD-MAP §0 |
| DU-03 | 文件异常呈现 | 五种异常（回退/搬家/换类型/同号重复/缺失）中只有搬家不阻断，面板逐库可见 | `FileAnomaly::blocks()` |
| DU-04 | 预览期间有批次在跑 | 「N 个库正在应用，以上数字可能偏大」警示条出现/消失正确 | queue-live-acceptance 三-2 |

### 3.5 证据

- L0/L1：`cargo test --lib` 完整输出（**必须同时记 ignored 数**）；
- L2：测试名 + 靶实例 + 前后快照（`snapshot_db.ps1` / `compare_snapshots.ps1`）；
- BL 系列另存 `_incrtest_*.json` 风格的 preview/execute/task 三件套（沿用 8-04 手法）。

---

## 4. 阶段二 · 模型的测试验证

**范围**：影响判定 → 生成根解析 → 反向级联 → 模型重生成 → 删除清理 → 按需生成
→ 房间归属。断言对象是 `inst_relate` / `inst_info` / `geo_relate` / AABB /
空间树 / `model_update_pending` 的收敛，以及 plant-ui 的树与三维。

### 4.1 继承的矩阵

- `IU-S5`（影响判定，32 测试全绿，无需新增）、`IU-S6`（生成根）、`IU-S7`（反向级联）、
  `IU-S10`（重生成）、`IU-S11`（补偿）、`IU-S13`（CATA 闭包）；
- D-01～D-15 场景矩阵（07-29 报告 §5）：**后端 B/C 级已全部通过一轮**，
  plant-ui 的 V 级全部待补；
- 补充锚点 DG-01（7997 DAMP DESP DirectGeometry）：2026-08-05 已通过 B/C/L2/L3
  与 plant-ui 功能复测，严格 V 级仅欠同一轮四图证据；
- noun 等价类抽样按 matrix-v2 的 25 类，未抽到的只享「同类推定」（口径不变）。

### 4.2 模型阶段当前的已知阻塞

| # | 事 | 影响 | 前置 |
|---|---|---|---|
| M-B1 | 8000 库 7/29 基线留下的 2967 条模型欠账已导出清空（`_incrtest_pending_backup_8000.json`） | 副本上重放这批欠账是 QW-01 的天然长积压夹具，**别浪费** | G2 |
| M-B2 | plant-ui / 宿主没有任何 `/model/ensure` 调用点（审核 B1） | D-12 无法闭环：显示缺失模型不会触发按需生成 | A3 定案 → 接线 |
| M-B2′ | **补充（08-05 核实）**：这不是漏写，是被测试钉死的。`plant-ui/crates/plant-ui-app/src/main.rs` 的 `eye_dispatch_does_not_call_the_model_generation_api` 直接断言 `handle_cmds` 里既不含 `ensure_model` 也不含 `/api/v1/model/ensure` | 接线时**不能删掉这条测试**，要改成「眼睛图标不触发、某个显式入口才触发」，否则会退回「切可见性就偷偷生成」的老问题 | 同 M-B2 |
| M-B5 | plant-ui 也没有 `POST /update/pending-units/retry` 的调用点（服务端 A2 已就绪：202/404 + 原子 UPDATE + `wake()`） | QW-02 / Q-13 的**界面半边**做不了，死信复活只能 curl | 客户端接线 |
| M-B3 | ensure 超时/忙碌三态不一致（审核 A3）：120s 撞线的 SUPPO/风管 BRAN 冷生成实测 99–104s | D-12 接上后每次显示都可能撞到 | 契约定案（202 轮询 vs 504/409） |
| M-B4 | 「模型变更通告」无实现（审核 D2） | plant-ui「陈旧标记 / 自动重画」只能靠轮询任务终态近似 | 跨仓契约设计 |

### 4.3 模型阶段用例（在 D 矩阵之上的增量）

| ID | 断言 | 层 | 现状 |
|---|---|---|---|
| MG-01 | 8000 库 31–34 窗口重放：2 个 BRAN 交付单元、ZONE 归并 `/1RX03-LCT`、`inst_relate` 0→51、pending 收口 0 | L2/L3 | 绿（8-04 已验，作为回归锚点固化） |
| MG-02 | ensure 崩溃恢复：`ensure_regen_pending` 落行后杀进程，重启空闲轮把该根捡回来生成 | L2 | 待跑（C1 修复的实机半边） |
| MG-03 | 死信复活端点：把一个根 attempts 推到 5，`POST pending-units/retry` 后 202、行归零、下一轮 drain 真执行 | L2 | 待跑（A2 修复的实机半边） |
| MG-04 | 合批收口失败不给成功根记失败（C2）：批量生成成功 + 人为断开收口，行留在表里、attempts 不涨、下一轮幂等收口 | L2 | 待跑（L0 守护已绿） |
| MG-05 | SCOM.GMRE 从存储属性解析（不依赖 legacy `->GMRE` 边）：BEND `24384/22456` | L2 | 已写（`#[ignore]`），等 G0 定靶 |
| MG-06 | 房间归属增量：AABB 变更集触发、整间/元素两分支、同轮吸收封闭性 | L0 绿 + L2 | L2 待跑（ADR-010 D12 残余缺口记为已知） |
| DG-01 | 7997 DAMP `24381/100819`：`DESP` 宽度 1000→1400，BRAN `24381/100817` 重生成；属性、网格尺度、AABB 自动刷新；恢复后精确回基线 | L2/L3 + V | **B/C/L2/L3 与 plant-ui 功能绿**；严格 V 级欠同一轮 before/queue/repeat 四图 |

### 4.4 视觉闭环（V 级，plant-ui）

流程固定十步（07-29 §7.3，不重抄），关键口径：

- 每例四件证据：`Dxx-before.png` / `Dxx-queue.png` / `Dxx-after.png` /
  `Dxx-repeat.png`（或 after 图像哈希相等记录）；
- **after 必须是队列完成后自动刷新的画面**，重启前端或手动全量重载得到的不算；
- DataOnly 场景的正确结果是「树文字变、几何不变、队列无生成单元」；
  删除场景「树节点与几何同时消失」；跨根移动「新旧两根都刷新」；
- 截图一律 `inspect shot`（探针不抢焦点），坐标从同一次 `inspect tree` 取，
  不固化到文档；
- 旧版 rs-plant3-d 截图只作历史基线，不抵扣 plant-ui 验收（07-29 口径不变）。

D-12（缺模型首次显示）在 M-B2 修复前标「客户端接线阻塞」，不允许用
「服务端接口通过」顶替（07-29 §7.5 原话，仍然有效）。

2026-08-05 新增 DG-01 DirectGeometry 回归锚点：session 91 执行和 plant-ui
自动刷新通过，session 92 恢复基线；after 证据见
`output/plant-ui-increment/D12-session91-direct-geometry-visible-offscreen.png`。由于同一轮
before/queue/repeat 四图未齐，按本节口径暂不标成严格 V 级完成。

---

## 5. 阶段三 · 任务队列的测试验证

**范围**：入队 / 合并 / 冻结 / 暂停 / 恢复 / 饿死 / 死信 / 房间泳道 / 断线降级，
以及 plant-ui 队列面板的全部呈现。执行手册以
`plant-ui/docs/plans/queue-live-acceptance.md` 为底，以下是 8-04 之后的增量与修订。

### 5.1 服务端（curl，客户端不在场也能做完）

继承 queue-live-acceptance §二的 7 条 + 排队/合并/冻结实况 4 步，新增：

| ID | 命令/操作 | 必须看到 | 现状 |
|---|---|---|---|
| QW-01 | 副本库灌入 M-B1 的 2967 条欠账 → 空闲轮开始消化 → 向范围内库存新会话入队 | **排队批次在可说明的上限内开跑**（数字随修复方案定，如每片 N 根间让位一次）；不得等积压全部跑完 | **L0 已绿 / 实机待跑**（`4f46ebcc` 改了三处：空闲轮自分类 Settled/MoreWork/Failed 使失败不再自唤醒成热循环、`drain_where` 的阻断范围从全局收窄到按 dbnum、房间轮加 10 分钟地板；各配一条回退即红的测试。**本条的实机判据尚未在副本库上验过**） |
| QW-02 | `POST /update/pending-units/retry`（存在的行 / 不存在的行） | 202+复活行 / 404 不凭空建行；复活后 worker 立即被唤醒 | 待实机（L0 已绿） |
| QW-03 | `POST /queue/pause` → 重启服务 → `/health` | `queue_paused=true` 活过重启；resume 恢复 | 继承（手册 #6），未跑过 |
| QW-04 | 房间轮收敛到 0 后读最近一条 `room_recalc` 任务 | `detail` 是 `{panels:0, elements:0, dead_letters:N}`，不再是开跑前数字 | 待实机（B2 修复的实机半边） |
| QW-05 | `/health` | 新增 `static_assets` 字段在；`ref0_affiliation_conflicts` 仍缺（审核 A4 半边）记已知 | 待实机 |
| QW-06 | 批次 panic（可用坏根注入） | 任务 `result.error` 有那句话；**面板能不能看到**按审核 B5 现状记录 | 待实机 |

### 5.2 plant-ui 队列面板

继承 queue-live-acceptance §三的 11 条 + §五的十二条口径，重点复验三处：

| ID | 看什么 | 判据 |
|---|---|---|
| Q-10′ | 房间泳道 | 收敛后泳道回落，不再永久「N 块面板待重算」、不再 30 分钟假饥饿变红（QW-04 的界面侧） |
| Q-12 | 「部分完成 · 欠 N 个单元」 | `pending-units` 反序列化失败时那格**整格消失**而非摆 0——用一条构造的坏行验诚实降级 |
| Q-13 | 死信可见性 | 泳道 `dead_letters` 数与 `pending-units` 里 attempts≥5 的行数一致；复活（QW-02）后数字回落 |

### 5.3 六个行状态留证

排队中 / 应用中 / 生成中 / 已完成 / 部分完成 / 失败——至少拍到五个（手册口径），
「生成中」以第一条单元事件（轮询侧 `total_units` 出现）为界。

---

## 6. 通过标准与不能宣称

单用例通过标准继承 07-29 §7.6 七条（范围正确 / 终态正确 / 数据正确 / 树自动刷新 /
三维自动刷新 / 证据齐全 / 重复执行幂等），逐条不重抄。

**不能宣称**（合并两份旧计划，一条不减）：

- `cargo test --lib` 全绿 ≠ 增量更新可用——报告必须同时给 ignored 数
  （2026-08-05 实测 **336 passed / 0 failed / 60 ignored**；旧文写的 58 已过期）；
- 等价类抽样 ≠ 全覆盖；`changeType` 等价类是变化处理分类，不是几何生成分类；
- 数据成功不能顶替视觉成功；旧版截图不抵扣 plant-ui 验收；
- 带静默早退的扫描类测试没有反空转计数断言，绿的不算绿；
- **BL-01…04 / QW-01 转绿之前，不得宣称「SYS meta 可初始化」「面板口径与执行
  一致」「长积压下手动增量可用」**——这三句今天都是不成立的。
  **2026-08-05 补充**：这三条的**纯函数半边**已绿（见各表），但本条约束
  **一字不改地继续成立**——它们要的是实库判据，L0 转绿不构成宣称资格。

## 7. 证据归档

| 层 | 留存 | 位置 |
|---|---|---|
| L0/L1 | `cargo test --lib` 完整输出（passed/failed/ignored 三个数） | `output/logs/<日期>_<主题>.log` |
| L2 | 测试名 + 靶实例 + 前后快照 + preview/execute/task JSON 三件套 | 同上 + `docs/evidence/` |
| L3 | 上述 + E3D 会话号 + refno/noun/owner/AABB/world_trans 前后 JSON | `docs/evidence/` |
| V | before/queue/after/repeat 四图 + 相机与树展开状态说明 | `output/plant-ui-increment/` |

每一轮跑完，回写本文各表的「现状」列；BL/WD/QW 系列转绿后，在
`2026-08-04_increment-update-working-tree-audit.md` 对应条目补「已收口」标记。
