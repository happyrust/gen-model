# 管道 / 通风 / 桥架 / 设备：增量测试案例清单与验证矩阵（2026-09-05）

- 需求：对管道、通风、桥架、设备四类做增量测试案例（增 / 删 / 改 / 移动里面的管件），配单元测试；
  并用 RVM 与数据接口验证「E3D TTY 导出的数据」与「RVM 真值」的一致性。
- 本文包含原始盘点与实施增补；2026-09-06 的可复核实施记录见 `docs/evidence/2026-09-06-cascade-key-matrix/README.md`。原始盘点范围：`scripts/e3d/`、`tests/`、`src/data_interface/`、
  `src/rvm_baseline/`、`test-increment/`、`docs/evidence/2026-09-0{2,3,4}*`、`vendor/e3d-model/tests/`。
- 术语按 `CONTEXT.md`：生成根 / 最小交付单元（MDU）/ 净变化 / 搬迁 / 净窗口收集器 / 双侧对拍。

## 0. 一句话

2026-09-06 后续修正：两个 hold 来自隔离环境把仓库配置的 ZDJ 7600 错换为 APS 7600，恢复原目录来源后两根均正常生成；同时补齐失败元素明细，并修复提前 SAVEWORK 导致数据轮、模型计划和基线解析各自捕获不同冻结源的问题。随后修复基线/稳态选根差异、跨根迁移提交顺序、连续窗口承接旧生成任务和删除后的旧死信；初始化、未修改模型保留、重启持久性及 E/P 共 14 个 live 窗口已通过；H 基线与清理追平，但创建 RVM 对照失败，矩阵尚未完成。结果见 `docs/evidence/2026-09-06-cascade-catalogue/README.md`，下条为历史记录。

2026-09-06 live 增补：隔离持久库的设备窗口尚未通过，发现初始化发布的双错误遮蔽与追加期间目标摘要漂移，已补回归和修复；原基线另有两个真实几何失败根，不能宣称整库收敛。详见 `docs/evidence/2026-09-06-cascade-live/README.md`。

2026-09-06 增补：P/E/H 已补统一 scratch 操作链、真实窗口选根测试及窄口径 RVM 对拍，首次恢复位置精度问题由严格复核发现并重跑。桥架已补初始导入的 gen-model 选根测试（明确允许阈值触发全量回退），编辑与 RVM 仍受 TTY 访问即异常退出阻塞。live 持久层收敛仍未验收。以下资产表保留原始盘点时点，最新状态以实施记录和各次 summary/acceptance 为准。

## 1. 现有资产盘点

### 1.1 靶元素与库

| 类 | 库（dbnum / 文件） | 容器 → 成员 | 已钉靶元素 |
|---|---|---|---|
| 管道 | AMS DESI **8000** `ams8000_0001` | `PIPE /1RX-345`(24384/23225) → `BRAN /C-OR-1R345-C`(24384/23257) → 7 FTUB + 2 BEND | FTUBE 4 = 24384/23262；BRAN 24384/23822（noun-bran 位移样本）；BRAN 24384/26229（planner 契约） |
| 管道 | AMS DESI **7999** `ams7999_0001` | `BRAN /1RCV0101-60.3-VMB-R23-R342/B1`(24383/73948) | VALV / ATTA 24383/73951、73956（09-04 已过 RVM 门） |
| 管道·桥架形态 A | 8000 | `ZONE /1RX03-LCT` → `PIPE /1RX-330` → `BRAN /C-IY-1R330-B`(24384/22404)：FTUB/BEND 走 ACP1000-TFVL 目录（GS-BOARD / COVER / CABLEWAY / RESERVED） | 「用管道 BRAN 建模的电缆槽」；RVM+ATT 样本在 |
| 通风 | AMS DESI **7997** `ams7997_0001` | `HVAC` → `BRAN 24381/100817` → DAMP / STRT / BEND / TRNS / PCOM… | DAMP 24381/100819（DESP/POS/NAME 三套宏）；STRT 24381/100820、DAMP 24381/100823、BATT 24381/100825（位移样本）；BRAN 24381/322（11→12 删容器窗口） |
| 桥架形态 B | AMS **251142** `ams251142_0001`（在盘：`D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\`） | `ZONE → CWAY → CWBRAN /CT_114-111/B1 → {RPATH→POINTR, CTMTRL→CTSTRA/CTBEND/…→CTRAY}` | 128 只 CTRAY 全在会话 2→9 一环里出现；链只有 1/2/9/10 四环，**没有编辑窗口** |
| 设备 | 8000 | `ZONE /1RX03-CASE-DQ` → `EQUI /1-LNR-Q005-PJ` → BOX… | scratch BOX `/CODEX_DB8000_SCRATCH`；EQUI 副本 `/CODEX_DB8000_EQ_COPY`；BOX 24384/25802 改挂窗口 45→46 |
| 设备 | 7997 | 正负体 BOX / CYLI / CONE / DISH / N*… | `ams_model_type_cases.json` 49 例（38 verified），**没有 EQUI 容器级位移例** |

> 「桥架」在这套语料里有两种形态：A 是 8000 里用管道 BRAN + 槽盒目录画出来的电缆槽（`/1RX03-LCT` 区），
> B 是 251142 里真正的电缆桥架学科（CWAY/CWBRAN/CTRAY）。图里是哪一种，决定 T 组用例走哪条链。

### 1.2 E3D TTY 宏（`scripts/e3d/`，都走 `l3_suite --check-driver`，每腿恰好一个 SAVEWORK）

| 类 | 宏 | 形态 | 状态 |
|---|---|---|---|
| 管道 | `db8000_bran_scratch_{create,move,ftub_move,delete_member,delete}.mac` | 增容器(COPY 支管) → 改容器位移 → 改成员位移(FTUB 4 +500) → 删成员 → 删容器；**每腿 SAVEWORK 前导一份窄口径 RVM** | 两条链跑过（266→271、271→276），五窗账平 |
| 管道 | `db8000_bran_ftub_move_apply/restore.mac`、`db8000_bran_hpos_apply/restore.mac` | 生产支管成员位移 / HPOS 端点 | `Test-TtyNetWindow.ps1` 与 kv-mem live 套件用例 |
| 管道 | `db8000_elbo3_offaxis_apply/restore.mac`、`issue7_cap_pos_*`、`issue5_cap_pos_*` | ELBO 离轴、CAP 位移（房间血统） | 有 |
| 通风 | `projams_damp_desp_apply/restore.mac`(DESP NUM2 1000→1400)、`projams_incr_pos_apply/restore.mac`(POS +100 E)、`projams_incr_name_apply/restore.mac`(NAME) | 改几何参数 / 改成员位移 / 数据仅 | M1 已 ✅，M2/F4 宏在 |
| 通风 | `projams_incr_delete_apply.mac` | 删 VTWA 24381/107146（BRAN 24381/107104 子件，INLINE 类），无 restore，靠文件备份 | 破坏性，用过一次 |
| 桥架 | —— | —— | **无**；只有 `docs/evidence/2026-09-04-hvac-ctray-increment/export-ctray-rvm.cmd`（addin 导 `/CT_114-111/B1`，目录里**没有产出 .rvm/.att**） |
| 设备 | `db8000_equi_add_box_apply/restore.mac`、`db8000_equi_copy_apply/restore.mac` | 增子件 / 增容器(COPY EQUI) | kv-mem 套件登记，尚未跑到 |
| 设备 | `db8000_scratch_{create,rename,move,resize,delete}*.mac` | scratch BOX 增 → 改名(数据仅) → 移 → 改尺寸 → 删 | 已录进 `db8000_session_pairs` 夹具 |
| 位移矩阵 | `Run-RoomE3DE2E.ps1 -ModelTypes <NOUN>`（`ams_model_type_cases.json`） | 逐 noun `BY U 10` + restore | 38 verified，覆盖 STRT/DAMP/BATT/FTUB/BEND/ELBO/BRAN/BOX/CYLI/NOZZ… |

### 1.3 离线增量判据（模型面：`update_ams` vs 两端 `gen_ams`）

| 工具 | 位置 | 判据 |
|---|---|---|
| `verify_db8000_bran_window.ps1` | `scripts/e3d/` | upserts 逐 GeometryId 与 full@T 相同；removals 覆盖 full@S∖full@T；未触及键 S/T 不变；可选 RVM 段 |
| `verify-window.ps1` / `verify_increment.py` | `docs/evidence/2026-09-04-pipeline-increment-atta/`、`…-hvac-ctray-increment/` | 同上，输出 `verification.json`（missing/stale upserts、missing/live removals = 0） |
| e3d-model `tests/increment_real.rs::an_increment_says_what_two_full_generations_say` | `vendor/e3d-model` | 7 个钉死窗口：插入 7999 45→46 / 修改 8000 255→256 / 删除 1112 721→722 / 级联 8000 195→196 / 改挂 8000 45→46 / **桥架 251142 2→9** / 管路分支 8000 249→250 / 删管路分支 7997 11→12 |
| e3d-model `hvac_owned_branches_derive_no_implied_tubes_but_pipe_branches_still_do` | 同上 | HVAC 属主的 BRAN 不派生隐式管身 |
| gen-model `live_ams8000_pinned_windows_land_in_the_expected_buckets` | `src/data_interface/window_root_plan.rs`（ignored，需真库） | 0902 链九窗 regen/delete/advance/lazy 分桶 |

已有离线证据：管道 scratch 五窗（09-02 / 09-03）、7999 四窗 VALV/BRAN-HPOS、**通风 7997 71→72**（DAMP 1 / STRT 4 /
BEND 3 / TRNS 2 / PCOM 1，PASS）、**桥架 251142 2→9**（CTRAY 128，PASS）。

### 1.4 RVM 真值链

| 环节 | 入口 | 状态 |
|---|---|---|
| E3D 导出（几何） | TTY 宏 `rvm_export_*.mac`（窄口径：`repre insu/obst off`、`repre tube on`、隐含管段分容器） | 11 个宏；样本 `test_data/rvm/`：`C-OR-1R345-C`、`C-IY-1R330-B`(+ATT，宽口径)、`1RCV0101-B1`、`1RX07-LCT`、`1RX03-MSUP-E0110`、`KX-CAM-S4001`、`SCTN-HW150-C80`、`GENSEC-CT01`(`/SUPPORT/FRAMES/BS/CT01/1`)、`1RS-WF03-W-C-RR001` |
| E3D 导出（RVM+ATT 成对） | CAF addin `GenModelRvmExport`（`GENMODEL_RVM_*` 环境变量，`repre lev pipe/nozz/struc/hvac`——HVAC 一档 HEAD `0884a714` 刚补） | 可用；`Test-GenModelRvmExportRepresentation.ps1` 钉住命令集 |
| 读入 / 对拍 | `rvm_verify import/compare`（`src/rvm_baseline/`，feature `rvm_verify`）、`rvm_generated_obj_compare`、e3d-model `rvm_compare --baseline … --generated …`、`gen_ams --rvm` | 09-04 起两侧同一读法（对称链），21 条 BRAN 126 件 0 差 |
| 判档 | exact / chord / off；AABB 分量 ≤ 1 mm；`only_baseline` / `only_generated` = 0 | —— |

**缺的真值**：~~通风~~（**已补**：`test_data/rvm/CUP-S-3-M-1201.rvm`，HVAC BRAN 24381/100817，8 件全 exact，
见 §3.2 H-10 与 `docs/evidence/2026-09-06-hvac-100817-increment-tty-rvm/`）、桥架形态 B（导出跑过没产出）、
设备 8000 `/1-LNR-Q005-PJ`。**HVAC 导出必须显式 `repre lev hvac 6`**——本 -tty 运行时 `Q REPRE` 缺省全 0，
不设则 STRT/BEND 的法兰片被滤掉、与 gen_ams 差 50 mm。

### 1.5 数据接口（数据面 ↔ TTY 双侧对拍）

| 工具 | 作用 |
|---|---|
| `aios_db.parse.net_window(detail=True)`（python 绑定） + `Test-TtyNetWindow.ps1` | apply / restore 两腿净窗口三态、合并窗口业务净零、`rollback_verified` |
| `e3d_tty_sample_refs` → `e3d_tty_attlist_capture` → `docs/evidence/2026-09-04-tty-ams8000-sample/verify.ps1` | TTY `ATTLIST` 值 vs e3d-io 直读逐属性对拍（8000：5131 行 / 23 个运行期边界差） |
| `direct_tree_tty_compare --tty` | `Q MEMBERS` 顺序 vs e3d-io 子树（8000：24 父 0 差） |
| `e3d_tty_cata_members_compare` | 目录成员对拍 |
| `Record-Db8000SessionChain.ps1` + `db8000_recording_cases.json` → `tests/db8000_session_pairs.rs` | 把一条 TTY 会话链录成离线夹具：逐窗净变化 / 折叠 / 快照差分互证 |
| `tests/db8000_two_delete_fixture.rs`（issue-019） | 跨会话父子删除 |

### 1.6 单元测试（纯函数层，`cargo test --lib` 即跑）

| 文件 | 覆盖 | 与四类的关系 |
|---|---|---|
| `tests/pipe26229_increment.rs` | `plan_window_roots` 契约：增支管 / 删支管 / 成员变化归并一次 / 改挂两根都 regen / 数据仅只 advance / 属主链不可读不许 advance / 未发布根 lazy | **只写了管道 BRAN** |
| `src/data_interface/model_impact.rs`（36 测试） | 属性→影响分类（SPRE regen、NAME skip、POS/ORI/DESP…）、owner 变化记两端、字典全 noun 策略总函数 | 类别无关，缺 HVAC / CT* / EQUI 子件的专项断言 |
| `src/data_interface/generation_root.rs`（22） | MDU 配置、`components_resolve_to_their_nearest_delivery_unit`、`ftub_is_never_a_normal_generation_root`、结构子件归可绘父 | 缺 HVAC 成员→BRAN、CTRAY/CTSTRA→?、EQUI 子件→EQUI 三条 |
| `src/data_interface/model_update_plan.rs`（27） | 成员位移只 regen 带隐式管身的单元、删子树塌到顶、跨会话子删父删、房间触发 | 缺 HVAC（无隐式管身）与桥架成员位移分支 |
| `src/data_interface/window_root_plan.rs`（21） | regen/delete/advance/lazy 分桶、凭证前移护栏 | 类别无关 |
| e3d-model `tests/noun_coverage.rs`、`increment_real.rs` | noun 覆盖 / 真库门 | 见 1.3 |

### 1.7 live 套件

- `test-increment/db8000-pipe-equipment-kvmem/Run.ps1`（spawned-mem + `--watch-dbnum 8000` + autorun）：4 例
  `pipe-bran-hpos / pipe-ftub-move / equipment-add-box / equipment-copy`；09-05 首轮 G0/G1 过、
  **G2 模型收敛 FAIL**（3 条 `delete_cleanup running` + 1 条 `regen_root pending`，`attempts=0`）。
- `scripts/Run-RoomE3DE2E.ps1 -ModelTypes …`、`Test-AmsModelTypeCoverage.ps1`：noun × 位移矩阵。
- `scripts/Run-LiveBatch.ps1`、`l3_suite`（M1–M3 / F4–F8 场景表，`docs/2026-08-06_e3d-l3-automation-test-plan.md`）。

## 2. 缺口清单

| # | 缺口 | 影响 |
|---|---|---|
| ~~G1~~ | ~~通风没有 RVM 真值样本~~ **已补（2026-09-06）**：`test_data/rvm/CUP-S-3-M-1201.rvm`（BRAN 24381/100817，8 件全 exact，`repre lev hvac 6`）；H-01/H-02 已跑完 ①②③④⑤ | H 组现在能做「vs E3D 几何」 |
| G2 | 桥架形态 B 没有 TTY 编辑宏、没有 RVM 产出、链上没有编辑窗口；gen-model 源码里 `CWBRAN` / `CTRAY` 只出现在 legacy `src/cata/consts.rs` 的注释与 `noun_census_probe` 里，MDU 默认集不含 CWBRAN，归根规则从未被断言 | T 组增 / 删 / 改 / 移全部要新写；先要确认 CTRAY 在 gen-model 归到哪个根 |
| G3 | 设备没有 EQUI 容器级位移用例、没有 8000 EQUI 的 RVM；子件跨 EQUI 改挂只有离线窗口（45→46），无宏 | E 组补 3 例 |
| G4 | 管道缺「增成员」（在既有 BRAN 里 NEW 一件）、「跨 BRAN 改挂管件」、「换规格件 SPRE」三种形态的宏 | P 组补 3 例 |
| ~~G5~~ | ~~单元测试层只有管道 planner 契约~~ **已补（2026-09-06）**：H/T/E 各一份 planner 契约（`tests/{hvac_bran,ctray,equi}_increment.rs`）+ 归根断言（U-H3/U-T1/U-E2 在 `generation_root.rs`）+ 影响分类（U-H2/U-T3 在 `model_impact.rs`）全绿 | —— |
| G6 | `db8000_session_pairs` 夹具只录了 BOX/EQUI 链；BRAN scratch 五腿、HVAC、CTRAY 链没进夹具 | 数据面折叠对这三类没有离线回归 |
| G7 | live 阻塞：模型 drain 尾页停滞（09-05）、direct 读视图冻结（09-03 §4，`verify_file_identity` 至今无生产调用点，是否已用别的方式修好需先确认）、数据批次 `dispatcher_not_claiming_eligible_row`（09-03 §5） | 所有 live 用例的 G2/G3 都过不去；离线三段链不受影响 |

## 3. 用例矩阵

列说明：**层** = 数据面(D：TTY 净窗口 + ATTLIST 对拍) / 模型面(M：`update_ams` vs `gen_ams`) / 真值(R：RVM) / 生产(L：live 收敛)；
**状态** ✅ 有宏且跑过 · ◐ 有宏未跑或只跑过部分层 · ✚ 要新写。恢复腿一律写完整终态，不写反向增量。

### 3.1 管道 P（8000，PIPE `/1RX-345`；scratch 支管 = `COPY /C-OR-1R345-C`）

| ID | 操作 | 靶 / 宏 | 期望（模型面） | 层 | 状态 |
|---|---|---|---|---|---|
| P-01 | 增·容器 | `db8000_bran_scratch_create.mac` | `created_subtree_roots=1`；regen 9 + derived 1；PIPE 不成根 | D M R | ✅（L ✚） |
| P-02 | 改·容器位移 | `db8000_bran_scratch_move.mac`（BY U 2000） | 9 单元 regen（位姿）；管身槽位幂等 | D M R | ✅ |
| P-03 | **移动管件**·成员位移 | `db8000_bran_scratch_ftub_move.mac`（FTUB 4 +500）/ 生产 `db8000_bran_ftub_move_apply/restore` | 只该 FTUB regen + 两侧管身变长；BRAN 是根 | D M R L | ✅（L 在 kv-mem 套件待跑） |
| P-04 | 删·成员 | `db8000_bran_scratch_delete_member.mac` | remove 1 + derived 1；消失键全部在 removals | D M R | ✅ |
| P-05 | 删·容器 | `db8000_bran_scratch_delete.mac` | remove 8 + derived retired；`DeleteCleanup` 一条 | D M | ✅（L 的 `DeleteCleanup` 执行未验） |
| P-06 | 改·路由端点 | `db8000_bran_hpos_apply/restore.mac`（HPOS U +500） | 只 BRAN 重生成，成员几何不变 | D M R L | ◐（09-05 L 停在 drain） |
| P-07 | 增·成员 | ✅ `db8000_pipe_new_member_apply.mac`（scratch A 尾部 NEW FTUB + COPY 带 SPRE） | ②added 1 + BRAN children_changed；③created 1 → BRAN A regen 1、`no_model={}`（PIPE 不成根）、upsert 逐件 = full@289；④RVM 10/10 exact（新件 26301 exact，max 0.0014 mm）；⑤删 A 库回原样 | D M R | ✅（2026-09-06 ams8000 288→290 ①–⑤ 全绿） |
| P-08 | **移动管件**·跨 BRAN 改挂 | ◐ `db8000_pipe_reparent_apply.mac`（A 的 FTUB 4 改 OWNER 到 B；配 `db8000_pipe_scratch_b_create.mac`） | 新旧两根都 regen（纯函数契约 `reparented_fitting_regenerates_both_old_and_new_branches` 已钉） | D M R | ◐（宏已写，live 待验：OWNER 直改能否被判 owner_change 需首跑确认） |
| P-09 | 改·换规格件 | ◐ `db8000_pipe_spre_swap_apply.mac`（scratch A 的 BEND 1 换 SPRE） | 该件 regen；几何随目录变（`pipe_spec_reference_regenerates…` 已钉分类） | D M R | ◐（宏已写，占位 `<ALT_SPRE>` 待填：db 13244 SPEC 109234 的同族替换件需先定位该库枚举） |
| P-10 | 改·数据仅 | ✚ scratch BRAN `NAME` 改回（复用 `db8000_scratch_rename` 形态） | 零模型工作，仅 advance | D M | ✚ |
| P-11 | 增删抵消 | 同一窗口内 P-01 + P-05 | 净 `cancelled`，无工作项 | D | ◐（`db8000_recording_cases` 里 BOX 版已有，BRAN 版 ✚） |

### 3.2 通风 H（7997，HVAC BRAN 24381/100817；scratch = `COPY` 该支管到同 HVAC）

| ID | 操作 | 靶 / 宏 | 期望 | 层 | 状态 |
|---|---|---|---|---|---|
| H-01 | 改·几何参数 | ✅ `hvac_100817_damp_desp_apply/restore.mac`（DESP NUM2 1000→1400；取代 `projams_damp_desp_*`，基线来自 @106 只读探针） | 只 BRAN 根 regen；DAMP 体积 157.9M→220.7M；**不派生管身**；RVM 8/8 exact；合并窗口净零（restore 只写 NUM2，见证据 §3） | D M R | ✅（2026-09-06 ①②③④⑤ 全平） |
| H-02 | **移动管件**·成员位移 | ✅ `hvac_100817_damp_pos_apply/restore.mac`（DAMP +100 E；取代 `projams_incr_pos_*`，基线 @106） | 根 = BRAN；AABB 平移 100（体积不变）；RVM 8/8 exact；合并窗口只剩 CACHID | D M R | ✅（2026-09-06 ①②③④⑤ 全平） |
| H-03 | 改·数据仅 | `projams_incr_name_apply/restore.mac` | DataOnly，零模型任务 | D M | ◐ F4 |
| H-04 | 删·成员 | ✚ 在 scratch HVAC 支管上删一件 STRT（不再动生产件 VTWA） | remove 1；相邻 STRT 不变 | D M R | ✚（旧 `projams_incr_delete_apply` 破坏性，弃用） |
| H-05 | 增·容器 | ✚ `NEW BRAN` + `COPY 24381/100817` 到同 HVAC | 新单元 N + 无 derived 管身 | D M R | ✚ |
| H-06 | 增·成员 | ✚ scratch 支管里 `NEW STRT` / `NEW DAMP` | 新单元 1 | D M R | ✚ |
| H-07 | 改·容器位移 | ✚ scratch 支管 `BY U 500` | 全部成员位姿变；件数不变 | D M R | ✚ |
| H-08 | **移动管件**·跨 BRAN 改挂 | ✚ DAMP 从 scratch A 挂到 B | 两根都 regen | D M R | ✚ |
| H-09 | 删·容器 | ✚ 删 scratch 支管（7997 11→12 删 BRAN 24381/322 的离线门已在） | remove N + retired | D M | ✚ |
| H-10 | 真值样本 | ✅ `rvm_export_hvac_100817.mac`（窄口径 + `repre lev hvac 6`）→ `test_data/rvm/CUP-S-3-M-1201.rvm` | `only_baseline=only_generated=0`，8 件全 exact（最大轴差 0.0004 mm） | R | ✅（G1 关闭） |

### 3.3 桥架 T（形态 B：251142，CWBRAN `/CT_114-111/B1`；形态 A 按 P 组走，靶换成 `/C-IY-1R330-B` 所在 PIPE）

| ID | 操作 | 靶 / 宏 | 期望 | 层 | 状态 |
|---|---|---|---|---|---|
| T-00 | 前置 | ✚ 确认 251142 在 MDB `/ALL` 里可写；确认 gen-model 对 CTRAY / CTSTRA / CWBRAN 的归根（单测 U-T1） | —— | — | ✚（G2） |
| T-01 | 增·一次性导入 | 链 2→9（128 CTRAY） | 每只 CTRAY 自成单元被建 | M | ✅ 离线门 |
| T-02 | 改·路径点 | ✚ `POINTR` 位移（RPATH 下） | 受影响 CTSTRA/CTBEND → 其 CTRAY 重算 | D M R | ✚ |
| T-03 | 增·成员 | ✚ `NEW CTSTRA`（CTMTRL 下）或加一个 POINTR | 新 CTRAY 单元出现 | D M R | ✚ |
| T-04 | 删·成员 | ✚ `DELETE CTSTRA` | 对应 CTRAY 消失 | D M R | ✚ |
| T-05 | 改·容器位移 | ✚ CWBRAN / CWAY `BY U 500` | 位姿 | D M R | ✚ |
| T-06 | **移动管件**·跨 CWBRAN 改挂 | ✚ CTSTRA 从 B1 挂到 B2 | 两根都 regen | D M R | ✚ |
| T-07 | 删·容器 | ✚ `DELETE CWBRAN`（scratch 副本） | 全部 CTRAY remove | D M | ✚ |
| T-08 | 真值样本 | ✚ 复跑 `export-ctray-rvm.cmd` 拿到 `/CT_114-111/B1` 的 .rvm/.att（先排查上次为何无产出） | 128 只托盘配对 | R | ✚（G2） |

### 3.4 设备 E（8000，EQUI `/1-LNR-Q005-PJ`）

| ID | 操作 | 靶 / 宏 | 期望 | 层 | 状态 |
|---|---|---|---|---|---|
| E-01 | 增·子件 | `db8000_equi_add_box_apply/restore.mac` | 新 PE + 实例；EQUI 根 regen | D M (R ✚) L | ◐（kv-mem 套件待跑） |
| E-02 | 增·容器 | `db8000_equi_copy_apply/restore.mac`（COPY EQUI） | 新根 + 全部子件 | D M (R ✚) L | ◐ |
| E-03 | 改·数据仅 / 移 / 改尺寸 / 删（子件） | `db8000_scratch_{rename,move,resize,delete}` | modified / modified / modified / deleted | D | ✅ 夹具（M R ✚） |
| E-04 | 改·容器位移 | ✚ EQUI `BY U 500`（`ams_model_type_cases` 无 EQUI 例） | 全部子件位姿变；一个根 | D M R L | ✚ |
| E-05 | **移动子件**·跨 EQUI 改挂 | ✚ BOX 从 EQUI A 挂到 EQUI B（离线窗口 8000 45→46 已钉：`24384/25802` 25801→25803） | 两根都 regen | D M R | ✚ |
| E-06 | NOZZ 位移 / 连接 | ✚ NOZZ `BY U 100`（7997 NOZZ 24381/160030 位移例已 verified）；连接属性改动 | EQUI 根 regen；相连 BRAN 是否级联按 `docs/2026-09-04_model-cascade-update-inventory.md` §1 | D M R | ✚ |
| E-07 | 删·容器 | E-02 的 restore 腿 | `DeleteCleanup` 覆盖整棵子树 | D M L | ◐ |
| E-08 | 真值样本 | ✚ `rvm_export_1_lnr_q005_pj.mac`（8000）；7997 已有 `KX-CAM-S4001.rvm`（EXTR 2 件 exact） | 配对 exact | R | ✚（G3） |

## 4. 单元测试案例清单（纯函数层，不需要 E3D / 库文件）

| ID | 落点 | 断言 |
|---|---|---|
| U-P1 | `tests/pipe26229_increment.rs`（已有 7 条） | 保持；作为 U-H1 / U-T2 / U-E1 的模板 |
| U-H1 | ✅ `tests/hvac_bran_increment.rs`（9 条，09-06 绿） | 同 U-P1 七条，根 noun=BRAN、属主 HVAC；另加「HVAC 容器自身被点名不成窗口根」「成员跨 BRAN 改挂两根都 regen」 |
| U-H2 | ✅ `model_impact.rs::hvac_component_attribute_changes_follow_the_update_contract`（09-06 绿） | DAMP/STRT/TRNS/BATT/BEND：`DESP`→DependencyCascade→Regen、`POS`/`ORI`→TransformOnly、`NAME`→Skip（背书 H-01/H-02 宏） |
| U-H3 | ✅ `generation_root.rs::hvac_components_resolve_to_their_branch_and_hvac_is_never_a_delivery_unit`（09-06 绿） | STRT / DAMP / TRNS / BATT / BEND → 最近 BRAN（DeliveryUnit）；HVAC 自身落自身 Normal、非交付单元；ZONE 不成根 |
| U-T1 | ✅ `generation_root.rs::cable_tray_members_only_share_a_root_when_cwbran_is_a_delivery_unit`（09-06 绿） | **现状（默认交付单元集）**：CTRAY **不**自成单元——CTRAY → 直接属主 CTSTRA / CTBEND，CTSTRA → CTMTRL（core 位表里 CT* 全不显著）；文件侧枚举只出 **CWBRAN**（显著，Normal 根），CWAY 不成根。托盘侧两条路互不相交；路径侧 RPATH 是点容器（noun_caps `point=true`）被跨过，POINTR / RPATH 落到 CWBRAN。**契约**：`append_delivery_unit_types = ["CWBRAN"]` 后 CT* / RPATH / POINTR 全部归 CWBRAN（DeliveryUnit），两条路合上 → T 组按「CWBRAN 为根」写 |
| U-T2 | ✅ `tests/ctray_increment.rs`（8 条，09-06 绿） | planner 契约：一次性导入两条 CWBRAN 全 regen、删 CWBRAN 塌成一条 DeleteCleanup、POINTR 位移只 regen 所属 CWBRAN、CTSTRA 跨 CWBRAN 改挂两根都 regen；另钉 U-T1 的现状 / 契约两段 |
| U-T3 | ✅ `model_impact.rs::cable_tray_route_and_material_changes_regenerate`（09-06 绿） | `POINTR`/`RPATH`（点容器）`POS`→Regen（非 Skip/非 TransformOnly）；`CTMTRL` 成员表增减→Regen（背书 T-02/T-03/T-04） |
| U-E1 | ✅ `tests/equi_increment.rs`（9 条，09-06 绿） | 子件增 / 删 / 改尺寸 / 位移都归并成 EQUI 一次 regen；COPY 成新根；跨 EQUI 改挂两根都 regen；BOX 改挂进同台 SUBE 仍一根；NAME 只 advance |
| U-E2 | ✅ `generation_root.rs::equipment_sub_elements_resolve_to_their_equipment_delivery_unit`（09-06 绿） | BOX / CYLI / NOZZ / SUBE / SUBE 下的 BOX → 最近 EQUI（DeliveryUnit）；EQUI 是默认 MDU；ZONE 不成根 |
| U-E3 | ✅ `model_update_plan.rs::equipment_sub_element_move_stays_cheap_without_implied_tubing`（09-06 绿） | 挪 NOZZ（EQUI 子件）两条判据都判「无派生几何」→ 不改判整根、只 TransformOnly+PostRegenAabb；对照 FTUB→BRAN 派生管身要整根重推 |
| U-X1 | `tests/db8000_session_pairs.rs` 夹具扩录 | `db8000_recording_cases.json` 加 BRAN scratch 五腿（净：added / modified / modified / deleted / deleted）；HVAC、CTRAY 各录一条链需要 `Record-Db8000SessionChain.ps1` 参数化 dbnum |
| U-X2 | e3d-model `tests/increment_real.rs::SAMPLES` | 钉 `ams7997_0001 71→72`（通风 DAMP/STRT/TRNS）为 `Shape::Hvac`，把 09-04 的 python 证据变成常驻门 |

## 5. 每个用例的统一验证链（TTY 数据 ↔ RVM 一致性）

```text
① TTY apply（l3_suite --check-driver <apply.mac>）             → 会话 S→T，宏内 SAVEWORK 前导出窄口径 RVM@T
② 数据面：aios_db.parse.net_window(S,T,detail=True)             → 每 refno 恰一条 Add/Modified/Deleted，等于 cases 里 expected_net
        e3d_tty_attlist_capture(受影响 refno) vs e3d-io 直读     → 逐属性 equivalent（允许 09-04 记录的三类运行期边界差）
③ 模型面：update_ams S→T  vs  gen_ams@S / gen_ams@T             → missing/stale upserts = 0，missing/live removals = 0
④ 真值：  rvm_compare --baseline RVM@T --generated (gen_ams --rvm @T) → only_baseline/only_generated = 0，判档 exact/chord，AABB ≤ 1 mm
⑤ TTY restore → 重复 ②③④，且 ②的合并窗口业务净零（只剩 CACHID）
⑥（有 live 服务时）Run.ps1 形态：applied_sesno 追平、pending 清零、model_ready、G3 逐 GeometryId 对拍
```

①–⑤ 不依赖服务，G7 的三条阻塞挡不住它们；⑥ 在 G7 修好前只记账不判红。

## 6. 落地顺序（建议）

1. ✅ **单元测试先行**（2026-09-06 完成）：U-H1/U-T1/U-T2/U-E1/U-E2 + U-H2/U-H3/U-T3 全绿；U-T1 结论已定 T 组按「CWBRAN 为根」写。
2. ✅ **通风真值**（2026-09-06 完成）：H-10 导出 `CUP-S-3-M-1201.rvm`（8 件 exact），H-01（DESP）/ H-02（POS）
   两类几何改动跑完 ①–⑤，增量对全量逐件相同、对 E3D RVM 逐件 exact、业务可回滚。
   证据 `docs/evidence/2026-09-06-hvac-100817-increment-tty-rvm/`。
3. **管道补形态**：P-07 / P-08 / P-09 三条新宏，沿 scratch 链加腿（不动生产支管）。
4. **设备补形态**：E-04 / E-05 / E-08。
5. **桥架**：T-00 → T-08 → T-02…T-07（若图里是形态 A，则改为在 `/1RX03-LCT` 区复制一条 `C-IY-1R330-B` 走 P 组链）。
6. **夹具化**：U-X1 / U-X2，把跑过的链录成离线回归。
7. **live**：G7 三条修好后用 `test-increment/db8000-pipe-equipment-kvmem/Run.ps1` 形态补 ⑥，`cases.json` 扩到四类。

## 7. 复跑入口（现有）

```powershell
# 单元测试（纯函数层）
cargo test --offline --test pipe26229_increment --test hvac_bran_increment --test ctray_increment --test equi_increment
cargo test --offline --lib model_impact::
cargo test --offline --lib generation_root::
cargo test --offline --lib model_update_plan::
cargo test --offline --lib window_root_plan::

# 管道 scratch 链一腿（TTY 写入，一个 SAVEWORK）
D:\Rust\target\debug\l3_suite.exe --check-driver scripts/e3d/db8000_bran_scratch_create.mac `
  --target-db-file D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001 `
  --aios-project AvevaMarineSample --project-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample --output output/db8000-bran-scratch/<ts>/create-driver

# 三方核对（模型面 + RVM）
pwsh -File scripts/e3d/verify_db8000_bran_window.ps1 ...     # 判据同 output/db8000-bran-scratch/verify-window-0903.ps1

# 通风 HVAC BRAN 24381/100817 真值 + H-01/H-02（2026-09-06；每腿一个 SAVEWORK，先确认无其它 E3D 会话）
D:\Rust\target\debug\l3_suite.exe --check-driver scripts/e3d/rvm_export_hvac_100817.mac `
  --project-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample --e3d-project AMS --e3d-mdb /ALL --output <run>/rvm
D:\Rust\target\debug\l3_suite.exe --check-driver scripts/e3d/hvac_100817_damp_desp_apply.mac `
  --target-db-file D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001 `
  --aios-project AvevaMarineSample --project-dir D:\AVEVA\Projects\E3D3.1\AvevaMarineSample --output <run>/h01-apply
powershell -File docs/evidence/2026-09-06-hvac-100817-increment-tty-rvm/verify-window.ps1 -RunDir <run> -Leg <名> -Base <S> -Target <T> -RvmFile <该腿 .rvm>

# 通风 / 桥架离线门
python docs/evidence/2026-09-04-hvac-ctray-increment/verify_increment.py
GATE_ONLY=ams7997_0001:71:72 cargo test --offline --test increment_real an_increment_says_what_two_full_generations_say -- --exact   # 在 vendor/e3d-model，需先钉 Sample

# TTY 数据面对拍
powershell -File scripts/e3d/Test-TtyNetWindow.ps1 -L3Exe D:\Rust\target\debug\l3_suite.exe
powershell -File docs/evidence/2026-09-04-tty-ams8000-sample/verify.ps1

# live（spawned-mem）
pwsh ./test-increment/db8000-pipe-equipment-kvmem/Run.ps1 -Preflight -Case all
```
