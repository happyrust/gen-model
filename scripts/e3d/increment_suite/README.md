# PML 增量更新测试套件

## 2026-11-12 起的跑法：只在私有副本上跑

现在用这一节。再往下的「AMS 原地运行入口」（`Run-AmsIncrementInPlace.ps1`）会通过 E3D TTY **写正式工程**，是 10-29 的做法，
没有明确批准不要用。

| 做什么 | 用哪个 | 说明 |
|---|---|---|
| 在 E3D 里试一份宏、跑一组宏或用例 | `Invoke-SuiteMacro.ps1` | 只认副本的**名字**（`-Copy ams` / `-Copy aps`），不认路径；副本根下要有 `.private-copy.json` 标记，E3D 报出来的工程目录也要在副本里，否则这一步判失败。`-CheckOnly` 只审不跑；`-DryRun` 不存盘试跑；实跑时 apply 和 restore 放在同一批。步的写法：`dir/x.mac[@库号]`、`export:<元素>`（只读导出 RVM，并出快照）。可以按用例号跑：`-Case <id> -WithSetup` |
| 核对、列出、合并清单 | `Get-SuiteManifest.ps1 -Mode check｜list｜json` | 主清单 `suite-manifest.json` 加 `manifest.d/*.json` 片段，合成一张表 |
| 一条命令跑一组用例（宏这一层） | `Invoke-SuiteRun.ps1` | 按来源、负责人、状态挑用例，分批跑，一批红了重做 setup 再接着跑，结果汇成 `suite-run.json` |
| 从一批的结果出回放用例的骨架 | `New-SuiteReplayCase.ps1` | 起点会话、每一步推到哪个会话、名字 → refno，E3D 导出的快照挂到对应那一步的 `rvm_compare` 上 |
| 跑回放（真的增量；不碰 E3D，不碰线上） | `Invoke-SuiteReplay.ps1` | `tests/increment_replay` 的包装：排队、按用例的工程备影子工程根、被测后端可换（`-Backend`）；同一趟不排两遍 |
| 出覆盖表（只读） | `Get-SuiteCoverage.ps1` | 变更类型 × 用例 × 各层结果：清单加片段（连片段里 `replay_windows` 登记的窗）、变更类型总表、跑宏的流水账、各趟回放的结果合成一张表，另出一份给机器读的（逐用例逐层：过 / 不过 / 已知缺口 / 没跑）。「宏」「回放」两栏看证据，其余取负责人写在片段里的话。有「不过」时退出码 1 |
| 一条命令跑全部、出总报告 | `Invoke-SuiteAll.ps1` | 编排：宏（`Invoke-SuiteRun.ps1`）、回放（知道的回放用例目录里每条用例一趟，`Invoke-SuiteReplay.ps1`），跑完重出覆盖表，再写总报告「情况 × 用例 × 各层结果」。`-CollectOnly` 一层都不跑、只收现成的证据；`-PlanOnly` 只说会跑什么、估多久。退出码：0 没有不过，1 有不过（含：回放用例在这次点的后端上红着、没声明本来就红也没有用例认领），3 / 4 见文件头。L3 要隔离后端，现在没有入口 |

本机的东西（副本在哪、E3D 装在哪、工具程序在哪、回放用例放在哪些目录 `replay.cases_dirs`、不经入口跑出来的回放结果表
`coverage.extra_replay_tsv`）都写在 `output/increment-suite/local.json` 里，不进仓库；这些脚本里没有本机路径。
同一时间只能有一个 E3D 会话；回放按本机配置的道数（`replay.lanes`）并着跑，每条道一份影子工程根。入口自己排队（先来先得），
不要绕过入口直接起 E3D 或直接跑测试程序。

### 清单片段 `manifest.d/`

新用例不改主清单，一个主题一份 `manifest.d/<负责人>-<主题>.json`：顶层 `version`、`owner`、`topic`、`copy`（在哪个副本上跑）、
`mdb`、`sites`、`setup`、`teardown`、`cases`、`chains`、`probes`。每条用例有 `id`（`<变更类型的格号>-<短名>`，例 `S6-member-reorder`）、
`situation`、`change_types`（`CT-…`，见 `docs/specs/incr-gen-audit-aux/increment-change-types.md`）、`apply` / `restore` 或 `chain`、
`db`、`expected_net`、`expect`（`rebuild` / `remove` / `untouched` / `why`）、`status`（`draft` / `macro_ok` / `verified` / `known_gap`）、
`evidence`。多出来的键原样带过去。`tests/increment_cases_registry.rs` 的 `manifest_fragments_are_well_formed` 钉着这套规则：
宏文件得在、用例号不重、格号在总表的范围里。库文件路径、本机路径不写进片段。

没有自己的宏的情况（在别的用例录下来的会话链上换起止取出来的并窗、腿内切窗）登记在片段顶层的 `replay_windows` 里：
下面每个数组里带 `id` 的项算一个「窗」。跑宏的入口不管它们；覆盖表和总报告把它们和用例一样逐层判（宏这一层记不适用），
认这几个键：`id`、`change_types`、`situation`（或 `shape`）、`status`、`checks.plan_offline`、`replay` / `checks.replay`
（里面点到名字的回放用例按实测结果挂上来）、`known_gap`（或 `gap_of_case`）。回放用例文件里的 `known_red: true`、
`_expected_red: "原因"` 表示这条本来就红：它红了，点到它的用例和窗记已知缺口，不记不过。

### 写宏的纪律（2026-11-12 实测）

- **TTY 里宏的任何一句报错，没有 `ONERROR CONTINUE` 的话会话直接以 `0xC0000005` 崩掉**，宏日志只剩 3 个字节，什么都不存。
  所以：写步什么都不带（出错就崩、库不动）；只读探针带 `ONERROR CONTINUE`，报错的那几行带着行号留在日志里。
  入口的 `-DryRun` 会替你加上 `ONERROR CONTINUE`、把 `SAVEWORK` 换成一行打印，新宏先这样试一遍。
- **`ONERROR GOLABEL /标签` 接不住报错**：报错那一句照样让会话崩掉。实测能接住的只有 `ONERROR CONTINUE`，
  在它之下可以再用 `handle any … endhandle` 分支。仓库里还在用 `ONERROR GOLABEL` 的老宏有 11 份（都在 `scripts/e3d/` 下，
  不属于本套件）：`s4_db8000_plopen_in_pane_move`、`s4_db8000_plopen_in_pane_create`、`s4_db8000_nbox_in_box_move`、
  `s4_db8000_nbox_in_box_move2`、`s4_db8000_nbox_in_box_create`、`s4_db8000_cleanup`、`s4_aps_strflt_move`、`s4_aps_strflt_fix`、
  `s4_aps_strflt_restore`、`adr066_live2_zone_103249_move_apply`、`adr066_live2_zone_103249_move_restore`。它们当时能跑通，
  是因为一句都没有报错；要重跑先改。
- **成员换序是有的**：`REORDER <成员> BEFORE <成员>` / `AFTER`，CE 在属主上；按名、按序号（`REORDER 3 BEFORE 1`）、
  按类型加序号（`REORDER BOX 3 BEFORE BOX 1`）都行。在设备的基本体上实测过，支管成员上还没有人试。
- **改类型的句子也是有的**：`CHANGETYPE TO <类型>`。BOX 改 CYLI 被拒（`Element type BOX may not be changed to type CYLI`），
  哪些类型之间许改还没探。
- **会话号怎么走**：AMS 的库一次 `SAVEWORK` 多两个会话（存盘、`QUIT` 的尾会话），同值写回多一个，导出登录在主库上再多一个；
  APS 的库一次 `SAVEWORK` 只多一个，导出不记账。写回放用例的步时按实际前进的会话数拆，不要套「每腿 +2」。
- 会话链夹具（`db_session_fixture`）的上限是 16 MiB。新录的链先用本机终稿（回放用例的 `source_file`），不打进仓库。

## AMS 原地运行入口

`Run-AmsIncrementInPlace.ps1` 以现有 `AvevaMarineSample/AMS//ALL` 为目标，默认不复制工程。
`preflight` 是只读检查；`capture`、`apply`、`restore` 和 `run` 会通过 E3D TTY 写入现有库，
只能在已确认目标和恢复宏的情况下使用。每次运行都会生成独立目录和 `run_id`，并保存
`preflight.json`、`baseline.json`、宏、TTY 证据及 `summary.json`。

启动项目检查由 `scripts/e3d/Test-E3dAmsStartup.ps1` 完成，统一核对项目目录、
`evarsAvevaMarineSample.bat`、`amscom/amssys`、8000/7998 库和实际 `des.exe` 路径；
它只读运行并把结果写入 `e3d-startup.json`。TTY 入口的项目名可通过
`L3_E3D_PROJECT_NAME` 覆盖，避免 evars 与项目目录被硬编码成不同工程。

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Run-AmsIncrementInPlace.ps1 `
  -Mode preflight `
  -ProjectDir 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample' `
  -DbFile 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001' `
  -Dbnum 8000 -Case equi-move
```

内置 AMS 目标为 `/1-LNR-Q005-PJ`（`equi-move`）和 `/1RX-345`（`pipe-whole`）。
需要几何证据时显式加 `-EnableRvm -RvmVerifyExe <path-to-rvm_verify.exe>`，脚本会生成
`rvm-before.json`、`rvm-after.json`、`rvm-restored.json`；RVM 导出本身不写库。

`-Case` 也接受 `suite-manifest.json` 中的 case id。由于旧套件宏多数面向隔离副本的
`/AIOS-SUITE-*` 语料，未显式提供 AMS 目标/宏时会返回 `blocked`，不会误写现有项目。
破坏性或没有 restore 宏的 case 同样必须显式处理；脚本不会把它们报告为通过。

完整案例资产由 `ams-inplace-manifest.json` 和 `Export-AmsIncrementCaseRvm.ps1` 管理。
先生成单个案例的 action/restore/RVM 宏和 `case.json`：

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Export-AmsIncrementCaseRvm.ps1 `
  -Mode plan -Case equi-move -RunId 20261029-ams01
```

`-Mode run` 才启动 TTY；成功后案例目录包含 `rvm/before.rvm`、`rvm/after.rvm`、
`rvm/restored.rvm` 以及对应 snapshots。TTY/runtime 或 RVM 导出失败时保留 driver 证据并
返回 `blocked`/`failed`，不能使用历史隔离副本 RVM 代替当前 AMS 结果。

针对 gen-model 的**元件级增量更新**（创建 / 修改 / 删除元件）的 PML 宏套件。设计依据见
`docs/specs/incr-gen-audit-aux/pml-increment-suite-design.md`（七部分，含覆盖矩阵、自审、FAIL 修正、**TTY 真机实测**）。

**状态：2026-10-28 已在 E3D TTY 真机跑通 34/34 个用例腿**（隔离副本 db8000 / db7998；以 `suite-manifest.json` 的 `tty_test_status` 为准）。
每腿稳定 +2 会话。原先卡在 7998 库锚点上的 3 腿已跑通。

2026-11-04 又跑了七对（`output/increment-suite-rvm-truth/20261104-112724`，先 setup 再写腿，14 条对拍都是 pass）。前四对的基线在 `common/setup_8000.mac` 末尾；负体位移和圆柱改径用的是原有 NBOX-N1、CYLI-1。setup 里把负体缺省原点写成显式 `POS E 0`。这七对不并进上面的 34。旧套件随后也在这份 setup 上重跑过，见本节末。

| 用例 | 宏 | 2026-11-04 实测 |
|---|---|---|
| D06-zone-move | `xform/zone_move_{apply,restore}.mac` | ZONE-MV `BY E 500` 后，BOX-MV1 / BOX-MV2 世界坐标各 +500 E（sesno 1515）。子件跟着 ZONE 走 |
| A02-box-pos | `geom/box_pos_{apply,restore}.mac` | BOX-PRIM 世界平移 [400,0,0]（sesno 1523） |
| A09-pos-same | `xform/pos_same_{apply,restore}.mac` | 空设备 EQ-SAME 的 POS 按原值再写，RVM 125 件不变（sesno 1530） |
| A09-xlen-same | `geom/xlen_same_{apply,restore}.mac` | BOX-A1 的 XLEN 2000 再写，EQ-A 并集不动（sesno 1536） |
| A02-pose-reparent | `attr/pose_reparent_{apply,restore}.mac` | BOX-POSE 改挂到 EQ-PB，AABB 变成 [93900,24700,-100,94100,25300,100]（sesno 1543） |
| B02-nbox-move | `geom/nbox_move_{apply,restore}.mac` | NBOX-N1 `POS E 200`，Q POS 变了，RVM 仍看不见负体（sesno 1507） |
| B01-cyli-size | `geom/cyli_size_{apply,restore}.mac` | CYLI-1 `DIAM` 1500→2100，AABB 扩到 [50950,3950,-1500,53050,6050,1500]（sesno 1499） |

2026-11-04 另跑了三对（`output/increment-suite-rvm-truth/20261104-140106`，先 setup 再写腿，6 条对拍都是 pass）。这三对不并进上面的 34，也不并进那七对。setup 在主 ZONE 下加了一台空设备 `/AIOS-SUITE-EQ-DESC`。

| 用例 | 宏 | 2026-11-04 实测 |
|---|---|---|
| A09-desc-same | `attr/desc_same_{apply,restore}.mac` | DESC 原句再写，Q DESC 前后都是 `AIOS suite desc same`，sesno +1（1634）。空设备不进 RVM |
| A09-spre-same | `pipe/spre_same_{apply,restore}.mac` | SPRE 按原引用 `=13244/108844` 再写，Q SPRE 前后都是 TBR:50，sesno +1（1640）。RVM 不变 |
| D06-zone-ori | `xform/zone_ori_{apply,restore}.mac` | ZONE-MV 写成 `ORI Y IS W AND Z IS U WRT /*`。两件 BOX 世界包围盒按实测锁定（sesno 1647），restore 回到 before（sesno 1651） |
| A09-ori-same | `xform/ori_same_{apply,restore}.mac` | EQ-PA 的 `ORI Y IS N AND Z IS U` 再写，Q ORI 前后相同，sesno +1（2088）。RVM 不变 |

清单里 `expected_net` 记的是 apply 侧：位移、改尺寸、改挂、改朝向是 `modified`，同值写回是 `unchanged`。纯换序当时没写（那时以为 `REORDER` 没有语法；2026-11-12 订正：有，见上「写宏的纪律」，用例在片段 `manifest.d/T3-ident.json`）。`GTYP` / UDA / `RULE SET` 在本仓没有已跑通的赋值句，也没写。`NEW PLOPEN` 曾让 des.exe 崩溃，开孔位移不写。

同一天把旧套件在这份 setup 上重跑。过滤跑 `20261104-164831` 里红的四条，有两处是宏写错了：改挂还原用 `FTUB LAST` 搬回了副本自己的管子，M5 留在副本上；顶点宏把世界坐标写进了 PLOO 局部 `POS`。改成按名取 `/AIOS-SUITE-BRAN-S-M5`，顶点沿局部 W 外扩 200。`20261104-174530` 上墙板、顶点、支管链、删成员、规格换件共 12 条对拍都是 pass。去掉校准后再跑 `20261104-175546`，同样 12 条 pass。链的最后一步仍重 COPY 并补名，把成员顺序补回基线。跨库还原带回的无名盒子锁定为基线 2000×1000×500。还写不了宏的类型见 `docs/specs/incr-gen-audit-aux/increment-change-types.md` 第 6 节。

## ★ 跑之前必须先读这一条

`{{WORLD_ANCHOR}}` **必须展开成两行**：

```
/1RX03-EQUI
OWNER
```

第一行把 CE 挪进目标库，第二行上跳到该库的 WORLD。

**为什么**：E3D TTY 的「当前库」由 **CE 所在库**决定，不是由 `--target-db-file` 决定。
`/ALL` MDB 的默认库是 **db 1112**，不是 db 8000。不做这个导航，会出现：

- `NEW SITE` 报成功，但 `Ref =17496/...`（17496 = db1112 的 world prefix）
- `SAVEWORK` 报成功，实际写的是 `ams1112_0001` 与 `amssys`
- `ams8000_0001` 的 sesno 纹丝不动
- 驱动器报 `macro completed without advancing the target DB session (734 -> 734)`

**只写导航不写 `OWNER` 也不够**：`COLLECT ALL SITE ... FOR CE` 是相对当前元素的子级，
CE 停在 SITE 上时返回空数组。

db8000 的身份常量：

| 量 | 值 |
|---|---|
| WORLD refno | **16192/0** |
| 元素 refno 前缀 | **24384** |
| 稳定锚点 | `/1RX03-EQUI` = `=24384/22399` |

`24384` 只是元素前缀，**不是** WORLD refno。这是最容易搞错的一处。

## 目录

```
increment_suite/
  suite-manifest.json      机器可读清单：34 个 case + 3 个探针 + 1 条强制链 + TTY 实测状态
  common/    3 个           db8000 基线（setup/teardown）+ 1 个只读探针
  create/   10 个           建元件：BOX / 子树 / 整台复制 / 支管复制 / 跨库复制
  delete/    8 个           删元件：单体 / 删容器留子件 / 同名重建 / 先子后父
  attr/      6 个           元数据：改名 / 描述 / 换属主
  xform/    12 个           变换：平移 / 三轴旋转 / 复合 / NOZZ 单移
  geom/      6 个           几何：BOX 尺寸 / NBOX 尺寸 / NBOX 增删
  connect/   9 个           连接：管道增删成员 / 改挂 / NOZZ 移动 / REORDER 反例探针
  pipe/      4 个           管道级：整管平移 / 支管头点
  room/      8 个           房间：成员移入 / PANE 移动 / PAVE 顶点 / FRMW 改名
  compound/  4 个           复合动作：改名+移动+增子件 / 两棵树同窗改
  h03/       5 个           H03 双库：7998 探针 / 基线 / 前进腿
  rvm-truth.json           RVM 验证模型：每条写腿应有的几何变化（见下「RVM 真值」节）
  Export-SuiteRvmTruth.ps1 每腿之后只读导出 RVM → rvm-truth/<sesno>.json，并对拍
  Compare-SuiteRvmTruth.ps1 两份快照按名对拍 + 按模型判定
```

共 **75 个 `.mac`**，全部通过 `Record-Db8000SessionChain.ps1:70-94` 的录制纪律检查。

## 占位符

| 占位符 | 渲染成 | 来源 |
|---|---|---|
| `{{LOG_PATH}}` | 宏同目录同名 `.log` | `fixture.rs:2571` |
| `{{WORLD_ANCHOR}}` | **两行**：`/1RX03-EQUI` + `OWNER` | 见上，TTY 实测 |

`pipe/bran_hpos_{apply,restore}.mac` 的坐标已按 TTY 实测**填死为绝对值**，无需再手填：

| 量 | 值 |
|---|---|
| 基线（setup_8000 后稳定） | `E 8551 N 14060 U 2900` |
| apply 写入 | `E 8551 N 14060 U 3100` |
| restore 写回 | `E 8551 N 14060 U 2900` |

写绝对值而非相对增量，是为了重跑幂等（第二次 apply 仍是 3100）。**改动 `setup_8000`
的 BRAN-S 坐标会让这两腿失效**，需重跑 `common/probe_8000.mac` 重新对齐。

## 三层可逆性

| 层 | 机制 | 可逆到什么程度 |
|---|---|---|
| **L-PML** | 每个 `*_apply.mac` 配一个 `*_restore.mac`，`expected_net` 在清单里声明 | 元素级逻辑原样（重建逆会换 refno） |
| **L-FILE** | `db_session_fixture replay-init --to <sesno>` | **字节级**重现任意会话时刻 |
| **L-DB** | 隔离副本 + 整库拷贝 | 整库原样 |

`restore` 分两类，看清单里 apply 侧的 `expected_net`：

- **属性级真逆**（refno 不变）：`attr/*`、`xform/*`、`geom/resize`、`geom/cyli_size`、`geom/nbox_resize`、`geom/nbox_move`、`geom/box_pos`、
  `room/*`、`compound/*`、`pipe/*`、`connect/nozz_move`、`h03/move`
  → apply `modified`、restore `cancelled`，**refno 保持**
- **重建逆**（refno 变）：`create/*` 的 restore、`delete/*_restore`、`geom/nbox_add_restore`、
  `connect/bran_add_member_restore`、`connect/bran_del_member_restore`
  → apply `cancelled`（由 restore 收掉）、restore 删/重建，**refno 会变**

注意本套件清单用的是简写字符串；仓内 `db8000_recording_cases.json` 的 `expected_net`
是 `[{"element": ..., "net": ...}]` 数组。两套清单互不参与对方流程。

**不逆的两条**（设计如此，别当 bug）：

- `delete/recreate_same_name_apply.mac` 无 restore
- `connect/bran_reparent_restore.mac` 树逆但几何不逆（成员回到末位而非原位置）

### 实测的可逆性证据（`create/box`）

```
apply   → NEW BOX /AIOS-SUITE-BOX-A2 → Ref =24384/26716
          sesno 751: EQUI /AIOS-SUITE-EQ-A
                     children_changed: old=[26716, 26560] → new=[26560]
restore → DELETE BOX（按名找到同一个 26716）
          sesno 752: {"refno": "24384_26716", "op": "deleted"}
```

**refno 完全一致，净效果 `cancelled`。**

## 强制链

`connect/bran_reparent` 四腿**必须**合成一条链：

```
create/bran_copy_apply → connect/bran_reparent_apply
                       → connect/bran_reparent_restore
                       → create/bran_copy_restore
```

单独跑 `create/bran_copy` 会 apply→restore 闭合（删掉 BRAN-NEW），后面的 reparent 必红。
**这个失败已在 TTY 实测中复现**；合成链后四腿全过，净效果 `cancelled`。

其余依赖边（A→B 表示 A 先）：

- `common/setup_8000` → 除 h03 外全部
- `h03/setup_7998` → `h03/move_*`、`create/cross_db_copy_*`
- `connect/bran_reparent` 整条闭合 → `connect/bran_del_member_*`
- `room/member_move_*` → `room/frmw_rename_*`
- `attr/*`、`xform/*`、`geom/*`、`compound/*` → `delete/*`（delete 会换 EQ-A/BOX-A1 的 refno）
- 全部 → `create/cross_db_copy_*`（它删 EQ-A）

## 双库（H03）

伙伴库选 **7998**，不选 7997：

| 库 | 原始 | zip | 6 MiB 上限 |
|---|---|---|---|
| `ams7998_0001` | 14,706,688 | **1,789,901** | ✅ |
| `ams7997_0001` | 73,498,624 | 12,896,057 | ❌ 超 2 倍 |
| `ams8000_0001` | 37,126,144 | 4,596,652 | ✅ |

上限出自 `src/bin/db_session_fixture/format.rs:25`（`MAX_ARCHIVE_BYTES = 6 * 1024 * 1024`）。

**7998 不能声明成 `companion`**：companion 必须有 `fixture_dir`，而影子根里的
`ams7998_0001` 是原件硬链接，被监听会触发从零初始化。`derive_config` 丢掉 7998 是对的。

### ★★ db7998 的锚点（已解，2026-10-28）

**先说清楚 refno 是什么。** 它不是「db_no 加某个魔数」，而是 `word0` 的三段位布局，
源码在 `vendor/e3d-io/src/refno.rs:17-30`：

```rust
pub const fn dbno(&self) -> u32 {
    (word0 & 0x1FFF) | ((word0 >> 13) & 0x3E000)   // 低 13 位 + 高位 = 库号
}
pub const fn extent(&self) -> u32 {
    (word0 >> 13) & 0x1FFF                          // 中间 13 位 = extent 序号
}
```

对 8000 以内的库号（高位为 0）等价于：

```
word0 = db_no + 0x2000 × extent      （extent 从 1 起算）
```

`extent 1 → +8192 (0x2000)`、`extent 2 → +16384 (0x4000)`、`extent 3 → +24576 (0x6000)`。

★ **容易写错的地方**：`extent_index` 从 **1** 起算，所以 `extent 1` 加的是
`0x2000 × 1 = 8192`，**不是 0**。公式里没有「加 0」的 extent。

验证（`ams7999_0001` 三个 extent，源码测试 `refno.rs:153-160` 同款）：

| extent | word0 | |
|---|---|---|
| 1 | 7999 + 0x2000 = 16191 | ✅ |
| 2 | 7999 + 0x4000 = 24383 | ✅ |
| 3 | 7999 + 0x6000 = 32575 | ✅ |

**WORLD 在 extent 1，普通元素在 extent 2。**

**大库号例外**：`word0 = db_no + 0x2000 × extent` 只对 `db_no ≤ 8191` 成立。
库号高 5 位在 `word0` 的 bit 26..30，大库号要按
`(db_no & 0x1FFF) | (extent << 13) | ((db_no & 0x3E000) << 13)` 算
（`refno.rs:143-151` 的测试用 `dbno = 251000`）。本套件涉及的库号都在 8191 以内。

WORLD 的 refno 不需要推导，它就在文件头里。
`vendor/e3d-io/src/meta/descriptor.rs:61` 写明 `+0x38 = primary_extent_ref`，
`:198` 给出 `world_refno() = RefNo(primary_extent_ref, 0)`。直接读：

| 库 | `+0x08` db_no | `+0x38` primary_extent_ref | WORLD | 元素前缀（extent 2） |
|---|---|---|---|---|
| `ams1112_0001` | 1112 | 9304 | `9304/0` | 17496 |
| `ams7997_0001` | 7997 | 16189 | `16189/0` | 24381 |
| **`ams7998_0001`** | **7998** | **16190** | **`16190/0`** | **24382** |
| `ams7999_0001` | 7999 | 16191 | `16191/0` | 24383 |
| `ams8000_0001` | 8000 | 16192 | `16192/0` | 24384 |

`16190` 与 TTY 实测成功的 `=16190/0` 逐字一致，两条独立路径互证。

**db7998 只有 1 个 extent 文件。** extent 3 的 `=32574/1` 实测报
`Undefined reference (DB number 7998)`。

证据要用**目录里有没有兄弟文件**，不能读 `+0x2C`——那是「这个文件自己是第几个 extent」，
五个库全是 1，用它推 extent 个数是循环论证。extent 是独立文件，命名
`ams1000_0001` / `ams1000_0002` / `ams1000_0004`（见 `vendor/e3d-io/src/page/io.rs:449-457`
的测试 `extent_paths_name_exactly_the_files_the_reader_reads`）。实测 `ams000` 目录里
每个库只有一个文件，**没有任何 `_0002`**。

**未验证**：`24382`（extent 2）的地址为什么能在单文件库里写成功。观察到的是
「能写、文件从 14,706,688 长到 14,901,248、没多出兄弟文件」，没读 page 1 的 extent 表，
所以这条别当结论用。能确定的是：7998 的元素只能落在 `24382` 段，容量受该文件页数限制。

**为什么之前找不到锚点**：`=24382/1`、`/2`、`/535` 全部报 `Undefined reference`，
`Q MDB` 的 37 个 SITE 里也没有 24382 前缀。真相是 **db7998 的 WORLD 下没有任何子元素**：

```
=16190/0
Q REF                                    → Ref =16190/0
VAR !c COLLECT ALL FOR CE ; Q VAR !c     → [1] '=16190/0'   ← 只有自己
```

这个库是真实的（DESI，645 会话，14.7 MB），但里面一个模型元素都没有。
**不是「锚点找不到」，是「根本没有锚点可用」**，必须自己建：

```
=16190/0
NEW SITE /AIOS-SUITE-7998        → Ref =24382/1883
```

之后全部走**名称导航**（`/AIOS-SUITE-7998-Z` 会跨库命中 db7998 的元素），
不需要 `{{WORLD_ANCHOR}}`（那是 db8000 的两行名称锚点）。

**★ 但绝不能写死 7998 元素的 refno**：实测 `=24382/1883` 在后续 setup/teardown
循环跑过之后被回收，新元素拿到 `1888`。只有 WORLD `16190/0`（文件头字段）永不改变。

### ★★ 导航到不存在的元素会让 E3D 崩溃

```
/AIOS-SUITE-7998-Z      ← 该 ZONE 不存在时
→ exit=3221225477       ← 0xC0000005 访问违例
→ 日志 3 字节（ALPHA LOG 没 flush）
→ 驱动器报 Indeterminate
```

**不是语法错、不是报错返回，是整个 E3D 进程访问违例退出。**
证据在 `check-driver-evidence.json` 的 `attempts[].driver_log` 里那一行
`detached pid=xxxx exit=3221225477`，日志本身是空的。

所以 7998 侧所有宏**必须先判存在再导航**。`create/cross_db_copy_apply.mac` 已加两道兜底
（SITE 与 ZONE 各自 `IF (!x.SIZE() EQ 0)` 后自建）。

**7998 不能声明成 `companion`**：companion 必须有 `fixture_dir`，而影子根里的
`ams7998_0001` 是原件硬链接，被监听会触发从零初始化。`derive_config` 丢掉 7998 是对的。

## 跑之前必须知道的三件事

1. **影子根里的库文件是硬链接**。`_runs/readiness-shadowroot/AvevaMarineSample/ams000/` 下的
   文件指向 `E:\AVEVA\Projects\E3D3.1` 原件，任何 `SAVEWORK` 都会写穿真工程。
   录制必须在隔离副本里做（`.scratch/make-isolated-copy.ps1` 建，实测 452 文件链接数全为 1）。
2. **三个探针无 `SAVEWORK`**（`common/probe_8000`、`h03/probe_7998`、`connect/bran_reorder_probe`），
   会被 `Record-Db8000SessionChain.ps1:74-77` 拒。设计上不进录制清单，单走
   `l3_suite --check-driver`。
3. **`bran_hpos_*` 的坐标是填死的绝对值**（`E 8551 N 14060 U 2900` ↔ `U 3100`）。
   改了 `setup_8000` 里 BRAN-S 的位置就会失配，必须重探。
4. **7998 侧宏必须判存在再导航**。直接 `/不存在的名字` 会让 E3D 访问违例崩溃，
   日志为空，只能靠 `exit=3221225477` 认出来。

## 宏崩溃时日志是空的

PML 宏在写操作那一行崩掉时，`ALPHA LOG` 的内容**不会 flush**，日志可能是 3 字节。
驱动器只能看到 `alive=True done=False exit=1` + `Indeterminate`。

排查必须看 `check-driver-evidence.json` 的 `attempts[].alive_seen` / `done_seen` /
`exit_status`，不能只看日志。

实测中因此发现三处幂等性缺陷：

| 宏 | 缺陷 | 修法 |
|---|---|---|
| `delete/child_then_parent_restore` | 直接 `NEW EQUIPMENT`，EQ-A 还在时报 `Name already used` | 先删同名再重建 |
| `create/bran_copy_apply` | 直接 `NEW BRAN`，重跑时报 `Name already used` | 先删同名 BRAN-NEW |
| `connect/nozz_move_*` | `!nozzs[1]` 越界（空数组） | 加 `IF / .SET() / .SIZE()` 判空 |

## 语料限制：db8000 没有 NOZZ

`COPY /1-LNR-Q005-PJ` 只带过来 1 个 BOX，没有 NOZZ。扫遍 db8000 全部 40+ 个 EQUI，
`FN-HIT` 计数 **0**——这个库里没有任何带 NOZZ 的生产设备。

所以 `common/setup_8000.mac` 显式 `NEW NOZZ /AIOS-SUITE-NOZZ-1`（自建元素，
符合「不动生产元素」纪律）。

## 语料来源：ams 真实项目（2026-10-28 改造）

原先房间与设备都是自造的，坐标是自己编的 `E 58000..72000` 一套。现在除 NOZZ 与 NBOX
外，语料全部来自 ams 真实项目。

| 用途 | ams 原件 | 库 | 处理 |
|---|---|---|---|
| 房间 R201/R202 | `FRMW /1RS-WF04-W-AB`（19 面墙） | db1112 | COPY 后只留第 1 面墙（2 PANE × 4 PAVE），其余 18 面 `DELETE CWALL`；R202 东移 8000 |
| 圆筒设备 | `/1-RFH-437-SN`（Ø38 × H52） | db8000 | COPY 后 `POS` 到 E52000 N5000，放大到 Ø1500 × H3000 |
| 带 NOZZ 设备 | `/1-LNR-Q005-PJ` | db8000 | 原样 COPY（只带 BOX，无 NOZZ） |
| 管道 | `/C-OR-1R345-C` 等 | db8000 | 原样 COPY |

**db8000 本身没有房间也没有管道**：`SECTION_STRU_ALL` / `SECTION_FRMW_ALL` /
`SECTION_PIPE_ALL` / `SECTION_BRAN_ALL` 全为空，100+ 个 EQUI 每个只有 1 个 BOX 子件。
真实房间在 db1112。

### 三条 COPY 硬约束（实测）

| 语句 | 结果 |
|---|---|
| `NEW STRU` 在 WORLD 下 | ❌ `Cannot create a STRU at this level` → 先 NEW SITE → NEW ZONE |
| `NEW FRMW` 在 ZONE 下 | ❌ `Cannot create a FRMW at this level` → 必须先有 STRU |
| `COPY` 进已有成员的容器 | ❌ `Cannot copy into an element which already has members` → 目标必须先是空容器 |

### ★ 真实房间的 PANE 是墙板，不是房间轮廓

`R201-P2` 实测 AABB `236 × 223 × 300 mm`。这是一块**墙板**，不是一间房的地板。
`R201-P1` 更异常，U 下界 -4255 远低于顶点最低的 6430，E/N 上界也远超四顶点范围
（E3D 把墙体延伸算进了包围盒）。

后果：`room/member_move` 要求 `EQ-RM` 属于 R201，而归属判定要求**八顶点全落在面板内**
（`room_predicate.rs:31`）。原 `BOX-RM` 是 `500³`，装不进 236×223×300 的墙板。
已缩到 `150³`，初始位置改到 R201-P2 中心 `E 8703 N 10861 U 6580`。

### ★ COPY 生产件必须用 POS 不用 BY

`BY` 是**相对原件位置**的偏移。`/1-RFH-437-SN` 原件在 `E1830 S24670 U4088`，
`BY E 52000 N 5000 U 0` 会把设备带到 `N -20420`（跑到套件区域外）。
必须写 `POS` 绝对值。

### 房间层级：PANE 挂在 CWALL 下

```
FRMW /1RS-WF05-W-AB
  └─ CWALL /1RS-WF05-W-AB-R5102
       └─ PANE /1RS06JJ0196P
            └─ PLOOP 1 → PAVERT 1..4
```

`PANE` **不是** `FRMW` 的直接子级。所以 RVM 真值的键比自造房间时代多一层 CWALL：

```AIOS-SUITE-8000-Z/AIOS-SUITE-ROOM-STRU/AIOS-SUITE-RM-R201/AIOS-SUITE-RM-R201-W1/AIOS-SUITE-RM-R201-P1
```

## 未编进套件的用例

| ID | 原因 | 状态 |
|---|---|---|
| `C0x-bran-reorder` | 2026-11-12 订正：`REORDER` **成立**（`REORDER <成员> BEFORE｜AFTER <成员>`）；当年的 `CP: Syntax error` 是少写了「挪谁」 | 不再归 (c) 类。设备基本体上的纯换序已有用例（片段 `T3-ident`：`S6-member-reorder`、`S6-unnamed-reorder`）；支管成员上的换序还没有人试，试成了再补 `connect/bran_reorder_*`。`connect/bran_reorder_probe.mac` 保留作「少写操作数会报语法错」的反例 |
| ~~`C0x-spre-swap`~~ | ~~卡 db 13244 语料~~ | **已结案**：`13244` 就是 `ams5052` 的库号，`SPEC 13244/109234 = /ACP1000-Ventilated` 可点查。已编为第 35 个用例 `E04-spre-swap` |

## 覆盖率的诚实说明

覆盖矩阵 56 条里，**PML 能触发 25 条（45%）**（2026-11-12 订正：成员换序从 c 类移到 b 类，原先是 24 条、43%）：

- **a 类 16 条**：现有宏即可触发
- **b 类 9 条**：需新宏（本套件已提供；成员换序那一条目前只在设备基本体上有宏，支管成员上未实测）
- **c 类 31 条**：PML 触发不了 —— 文件层（库身份、会话对、回退）、工具层（`/tasks`、`/dbnums`、
  `/health` 契约）、目录库语料（Paragon、13244）、API 计时

**关键：c 类 31 条里没有一条是「元件增删改」本身。** 所以「元件级增量更新全覆盖」是成立的，
剩下 55% 本质不属于 PML 能力范围，要用 Rust 测试覆盖。其中只登录、半存、回退、失败这四类（CT-W3、W4、W5、W10）
只能在回放层造，覆盖里单列「只能回放」一档，回放盖到就算盖到。

## TTY 实测推翻的静态结论

| 语句 | 静态审计判定 | TTY 实测 |
|---|---|---|
| `REORDER` | ★ 零命中 | 当时 ❌（写的是 `REORDER AFTER FTUB 3`，少了「挪谁」）；**2026-11-12 订正：✅ 成立**，句式 `REORDER <成员> BEFORE｜AFTER <成员>` |
| `CHANGETYPE TO <类型>` | 未审 | ✅ **句子成立**（2026-11-12）；BOX → CYLI 被拒，许改的类型对未探 |
| `ONERROR GOLABEL /标签` | 未审 | ❌ 接不住报错，会话照样崩（2026-11-12）；用 `ONERROR CONTINUE` |
| `HPOS BY` | ★ 零命中 | ❌ `CP: Syntax error` — 确认不存在 |
| `FTUB LAST` | ★ 零命中 | ✅ **成立**（审计判错，已回滚） |
| `PLOO n` / `PAVE n` 序号导航 | ★ 零命中 | ✅ **成立**（审计判错，已回滚） |
| `NEW NBOX ... 一行式` | ★ 零命中 | ✅ **成立**（已回滚成分行也可，两种都行） |
| PIPE 级 `BY` | ★ 零命中 | ✅ **成立** |
| `ORI .. WRT /*` 设值 | ★ 零命中 | ✅ **成立** |
| `DESC ''` | ★ 零命中 | ✅ **成立** |
| `NEW NOZZ` | 仓内零命中 | ✅ **成立** |
| `IF / .SET() / .SIZE()` | 仓内零命中 | ✅ **成立** |

## RVM 真值（验证模型）

**状态：模型与脚本已就位，真值只导过 E04-spre-swap 一份**（隔离副本，前 / 后 / 还原三份 RVM，Compare 通过；其余用例要在隔离副本上跑 E3D 才有）。套件的几何裁判和 K 组一样是
**E3D TTY 只读导出的 RVM**（`docs/plans/2026-10-27-plant3d-web-incremental-linkage-test-cases.md` §4 的窄口径）；
与 K 组不同的是这里的真值跟着现场录制走，不走 replay-init。

| 文件 | 作用 |
|---|---|
| `rvm-truth.json` | 验证模型：`legs.<宏相对路径>` 登记每条写腿相对「它跑之前那份快照」应有的变化：`added` / `removed` / `moved` / `resized` / `renamed` / `subtree`（件数差、并集平移、同形参照）/ `equals_state_before`（restore 腿与 apply 之前的状态相等）/ `aabb_blind`（按构造就看不见）/ `calibrate`（★ 未验证，只记观测不判红）/ `blocked` |
| `Export-SuiteRvmTruth.ps1` | `-Mode plan` 只列顺序；`-Mode batch` 按 manifest 逐腿 apply → 导出 → 对拍 → restore → 导出 → 对拍（强制链整条跑、`-Case` 子串过滤、`-RunSetup` 先跑 setup）；`-Mode state` 给库里当前状态补一份 |
| `Compare-SuiteRvmTruth.ps1` | 两份 `rvm_verify import` 快照按名对拍；给 `-Leg` 就按模型判 pass / calibrate / fail / skipped（exit 1 只在 fail），`-Report` 写 JSON |

关键口径：

- **粒度**：每条写腿之后整棵 `/AIOS-SUITE-8000` 导一份（`New-RvmExportMacro -QueryRef`，窄口径、无 SAVEWORK）。成员按名字对拍，
  restore 重建换 refno 不影响；根 refno 从导出日志的 `Ref =` 回读（SITE 每次 setup 都重建）。
- **键**：`rvm-truth/<sesno>.json`，sesno = 写腿 SAVEWORK 落地后、导出登录之前的 `latest_sesno`（导出登录自己会再追加一个
  DBMDAT 尾会话，`index.json` 两个值都记）。录制期间落在 `output/increment-suite-rvm-truth/<时间戳>/rvm-truth/`（git 忽略），
  套件录成 L2 夹具后整目录搬到 `<fixture_dir>/rvm-truth/`，就是 K 组 / `Export-ReplayRvmTruth.ps1` / plant3d-web spec 读的那个位置。
- **容差**：AABB 六分量 ≤ 1 mm（`rvm-truth.json` `tolerances.aabb_mm`）。
- **每腿对拍**：apply 腿与上一份比，restore 腿与对应 apply 之前的那份比（`equals_state_before`），录制当场就能看到真值不自洽。

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode plan
pwsh -NoProfile -File scripts/e3d/increment_suite/Export-SuiteRvmTruth.ps1 -Mode batch -RunSetup -IsolatedProjectsDir E:\reverse\e3d\replay-src\E3D3.1
pwsh -NoProfile -File scripts/e3d/increment_suite/Compare-SuiteRvmTruth.ps1 -Before rvm-truth/748.json -After rvm-truth/750.json -Leg xform/move_apply.mac
```

模型里诚实写明的限制（`rvm-truth.json` `limits`）：

| 限制 | 波及 | 处理 |
|---|---|---|
| AABB 对绕自身中心的 90° 旋转不敏感（BOX-A1 / BOX-N / BOX-RM 都是居原点的正方体） | `xform/rot_*`、`compound` 的旋转分量 | `aabb_blind`；要可见需把 BOX-A1 改非正方体（连带 4 处写回值），未改 |
| 空 EQUI 不进 RVM（K14 的 25 已实证） | `attr/rename`（EQ-B 空）、`child_then_parent_2` | `aabb_blind` |
| 自建 NOZZ 无 CATREF 无几何 | `connect/nozz_move` | `aabb_blind` |
| 负实体 NBOX / PANE 是否进窄口径 RVM 未验证 ★ | `geom/nbox_*`、`room/pane_move`、`room/pave_move` | `calibrate`，首轮后定 |
| INCLUDE 是保持相对 POS 还是世界位置未验证 ★ | `attr/owner`、`delete/container_keep_child` | 按「东移 3000」登记 + `calibrate` |
| BRAN 成员增删后缺省名重排序、隐含管段重算 | `connect/bran_*`、`pipe/bran_hpos` | 只认子树件数与并集，`calibrate` |
| 伙伴库 7998 的腿只改 7998，主库导出看不见（锚点已解，见上「db7998 的锚点」） | `h03/*` | batch 跳过；跑完后用 `-Mode state -Element /AIOS-SUITE-7998 -Dbnum 7998 -DbFile AvevaMarineSample\ams000\ams7998_0001` 单独补导，Compare 加 `-Root /AIOS-SUITE-7998` |
| `create/cross_db_copy_*` 一个 SAVEWORK 写两库；restore 把 BOX 以无名复制件带回，`/AIOS-SUITE-BOX-A1` 从此不存在 | `F03-cross-db-copy` | 主库导出只覆盖 8000 半边；batch 按 `rvm-truth.json` `run_last` 把它挪到最后 |

首轮录制后要做的：把 `calibrate` 项的观测值填回 `rvm-truth.json`（去掉 `calibrate`）；把 `baseline.members_calibrate` 里的 ★ 换成实测 AABB；
`summary.json` 里 fail 的腿先查宏再查模型。

## ★ 批量执行的三条硬纪律（2026-10-29 实测）

用任何自写的批量跑器跑全套时，必须满足这三条，否则会出现「宏本身没问题但 exit=1」的假失败。

### 一、链式用例必须按 legs 顺序连续跑

`suite-manifest.json` 里 `chains` 声明的腿不能各自 apply→restore 闭合。反例：
`create/bran_copy` 独立跑会在 restore 里删掉 `BRAN-NEW`，紧接着的
`connect/bran_reparent` 就找不到改挂靶，导航到不存在的元素 →
**E3D 崩溃 `exit=3221225477`（0xC0000005）**，日志留空，`sesno` 不动。

正确的顺序（manifest 已声明，跑器必须遵守）：

```
create/bran_copy_apply → connect/bran_reparent_apply
→ connect/bran_reparent_restore → create/bran_copy_restore
```

`chain` 字段有两种形态，跑器都要支持：字符串（引用 `chains[].id`）或内联的腿数组
（`A01-child-parent` 用的就是内联形态）。

### 二、伴生库用例必须路由到伴生库

`h03/move_apply` 导航 `/AIOS-SUITE-7998-EQ`，那是 **db7998** 的元素。
在 db8000 上跑必然 `0xC0000005`。带 `companion_dbnum` 字段的用例要按
`manifest.databases` 换成对应库文件。

### 三、探针宏必须带 `{{WORLD_ANCHOR}}`

不带锚点时 `CE` 停在未知位置，`COLLECT ALL <TYPE> FOR CE` 会返回空数组，
看起来像「元素不存在」。实测踩过：BRAN-S 明明在库里，探针却报
`<ARRAY> - Unset and Empty`。去掉 `FOR CE`（全库扫）或补锚点都能修。

### 四、`run_last` 必须遵守

`rvm-truth.json` 的 `run_last` 声明了必须排在全部用例之后的宏。
`create/cross_db_copy_*` 两条腿都在其中：apply 删掉 `EQ-A`/`BOX-A1`，
restore 带回的是无名复制件，`/AIOS-SUITE-BOX-A1` 从此不存在。
之后任何导航到它的用例都会崩（`delete/box_apply`、`compound/two_roots_apply`、
`pipe/spre_swap_apply` 实测全部 `0xC0000005`）。

注意 `run_last` 只列了 apply，但 **restore 同样有害**，跑器要按前缀把两条腿一起挪后。

### 五、两个库都要先跑 setup

批量跑用例腿之前，主库与伴生库的基线都得在：

```powershell
# 主库
common/setup_8000.mac   → --target-db-file ...\ams000\ams8000_0001
# 伴生库（H03 用，缺了 h03/move_* 必崩 0xC0000005）
h03/setup_7998.mac      → --target-db-file ...\ams000\ams7998_0001
```

`h03/move_apply` 导航 `/AIOS-SUITE-7998-EQ`，那是 db7998 的元素。
7998 原本是个空库（WORLD 下零元素），基线靠 `setup_7998` 建。


### 假失败与真失败的区分

| 现象 | 判据 | 结论 |
|---|---|---|
| `exit=1`、`sesno` 不变、宏日志只有 BOM | `driver.log` 尾部 `detached pid=... exit=3221225477` | 导航到不存在的元素，E3D 崩了。**先查前置是否被拆掉**，不是宏写错 |
| `exit=1`、`sesno` 变了 | 宏执行了一半 | 宏内部语法或语义错误，看 `scenario_log` |
| `exit=0` 但 `errors>0` | 日志里有 `(...)` 开头的行 | PML 报了可捕获错误，宏继续跑完了 |

## AMS 原地 RVM 资产准备

为每个 `ams-inplace-manifest.json` case 生成可交给 E3D TTY 的动作、恢复和三阶段 RVM 导出资产：

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Prepare-AmsIncrementRvmAssets.ps1 `
  -RunId ams-rvm-20261029
```

输出目录默认为 `output/ams-increment-rvm-assets/<run-id>/<case-id>/`，每个 case 包含：

```text
action.mac
restore.mac
rvm-export-before.mac
rvm-export-after.mac
rvm-export-restored.mac
rvm-export.mac                 # 兼容入口，等同 before
case.json
rvm/before.rvm                 # TTY 执行后生成
rvm/after.rvm
rvm/restored.rvm
snapshots/before.json
snapshots/after.json
snapshots/restored.json
```

资产准备阶段只生成宏和预期模型，不写 AMS 库，也不伪造 RVM 文件。`index.json` 汇总每个 case 的 `ready`/`blocked` 状态；只有 TTY 真机完成三次导出并通过 `rvm_verify import` 后，才能把 case 标记为 `passed`。对登记了 `dc_events` 的第三轮案例，这只是必要条件：RVM 只能证明几何/层级 fixture 与恢复，不能证明 `DB_UserChanges` 的 dedupe 分支，因此还需要运行时事件观测才能升级为 `passed`。

释放现有 `des.exe` 会话后，可显式执行当前标记为 `ready` 的案例并生成真实 RVM：

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Prepare-AmsIncrementRvmAssets.ps1 `
  -RunId ams-rvm-<timestamp> `
  -ExecuteReady `
  -ProjectDir 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample' `
  -E3dInstallDir 'D:\work\plant-code\old\e3d31-runtime'
```

`-ExecuteReady` 不会尝试 `blocked` 案例；每个 ready case 必须生成 `rvm/before.rvm`、`rvm/after.rvm`、`rvm/restored.rvm` 和对应快照。TTY 启动被占用、runtime 崩溃或 RVM 导出失败时，案例会保留 driver 证据并记为 `blocked`，不会使用历史或隔离副本 RVM。

当前分支矩阵（`partial-update-branch-matrix.md`）登记 120 个 AMS 案例：1 个 `ready`、84 个 `calibrate`、35 个 `blocked`。方法级覆盖报告见 `partial-update-method-coverage.json`，条件分支覆盖见 `partial-update-branch-coverage.json`（20 组分支），DB_UserChanges 事件入口覆盖见 `db-userchanges-event-coverage.json`（12 类事件）。`calibrate`/`blocked` 只表示尚未取得现有 AMS TTY 的运行证据，不得作为通过率统计。

第三轮 `DB_UserChanges` 交叉核对（`core.dll.i64`）补了 7 个案例：`P79`–`P85`，覆盖 created 祖先去重、created 祖先后删除/改属性、primary-list 加/删成员、qualifier 从 1 清空到 0、以及 `combine` 对同元素 qualifier map 的合并；并修正了“inserted 查询受删除行数门控”的旧推断。

这 7 个案例已经绑定成可导出的 AMS PML 套件：action/restore 及 `setup_macro`/`teardown_macro` 位于 `round3/`。`P79`–`P81` 在 baseline 前新建 run-owned SITE（带一个基线 BASE），`P82`–`P85` 在 baseline 前新建 run-owned PIPE/BRAN 并 COPY `/C-OR-1R345-C`、补稳定成员名。导出器先执行 setup、再采 baseline，restored RVM 导出后才执行 teardown；`{{TARGET_NAME}}` 会被替换成解析了 `<run-id>` 的目标名。这 7 个案例的根是 SITE/BRAN，baseline 走 `common/capture_root.mac`（只查 REF/NAME/TYPE/OWNE，不查 POS/ORI），避免不支持的 Q 属性让 E3D 提前退出。

`expected_change` 除了 added/removed/renamed/member_changes，还登记 `regen_roots`、`dc_events`、`forbidden`：分别是应出现的 regen 根、期望的 DB_UserChanges 事件、以及必须不出现的事件。它们会写入 `case.json`，其中的 `<run-id>` 同时被解析成具体 run。

`P79` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p79-rvm4`）：setup/apply/restore 各一次 `SAVEWORK`，三份 RVM 均已生成并通过 `rvm_verify import`；after 比 before 多出 `-OWNER` 与 `-CHILD`，restored 与 before 按名对拍为 0 差分。它仍保持 `calibrate`，因为 `dc_events` 还没有运行时观测手段，`rvm_apply_verify` 也因非平移预期被跳过；阻塞原因已收窄为 `requires-db-userchanges-event-observation`。

`P80` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p80-rvm`）：apply/restore 日志确认 `-OWNER`（ref 24384/26561）与 `-CHILD`（ref 24384/26562）创建、`-CHILD` 删除、`-OWNER` 以空容器留存，restore 找到并删除了 `-OWNER`；before/restored 各 4 成员、按名对拍为 0 差分。但 `rvm-after.rvm` 也只有 4 成员、不含留存的空 `-OWNER`。已用确定性探针（`output/probe-empty-equi/`：SITE 下一个空 EQUI 加一个带 BOX 的 EQUI，setup 后等 15 秒再导出）证实 **E3D RVM 导出会跳过没有任何几何成员的容器**：探针的 `Q MEMB`（全新会话）看得到空 EQUI，导出的 RVM（1760 字节，与 P80 同尺寸）只有 SITE/ZONE/带 BOX 的 EQUI/BOX 四条记录。因此 P80 的 after-RVM 缺口是导出器行为而非会话竞态；runner 的 `passed` 只覆盖「before==restored（在导出范围内）」，不能作为事件级通过的证据。P80 保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation;rvm-omits-empty-containers`。

`P81` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p81-rvm2`）：apply 日志确认 `-OWNER`/`-CHILD` 创建并把 `-CHILD` 改名为 `-CHILD-RENAMED`；after-RVM 含 6 成员（含 `-OWNER` 与 `-CHILD-RENAMED` 两条几何记录），restored 与 before 按名对拍为 0 差分。改名出现在 RVM 里但 created-ancestor 对属性事件的过滤不可由 RVM 观测，保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation`。

注意：runner 必须用 `-E3dInstallDir 'E:\reverse\e3d\shadow_e3d31_gen_model_test'`。脚本默认的 `D:\work\plant-code\old\e3d31-runtime\des.exe` 会在 setup TTY 崩溃（`exit=3221356545`，无 scenario 日志、sesno 不变，`20261029-p81-rvm` 首次尝试就是这样失败的）。

`P82` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p82-rvm5`）：setup COPY `/C-OR-1R345-C` 得 9 成员（M1–M9），apply 把 `-FTUB-ADD` COPY 进去，restored 与 before 按名对拍为 0 差分。注意两点：(a) apply 日志显示 `NEW FTUB` 实际插在**第 1 位**（成员列表头）而非末尾，其余 9 个成员顺位后移——是「成员集 +1」还是「带位移的插入」取决于 `DB_UserChanges` 的成员比较实现，RVM 分辨不出；(b) 修了两个套件缺陷才拿到干净证据——`New-RvmExportMacro` 原来无条件 `Q POS WRT /*`，在 BRAN 根上让 des.exe 段错误（0xC0000005），已加 `-NoPosQuery` 并由 manifest 的 `rvm_no_pos_query` 给 P82–P85 置位；runner 原来不等 detached des.exe 退出就进下一步，P82 的 rvm-before 曾因此被 apply 已提交的 FTUB-ADD 污染（弱会话隔离），已加 `Wait-TtySettled`（等 `des.exe` 数回落到 run 前基线）。P82 保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation`（ATT_MEMB vs reorder 仍需事件级证据）。前四次尝试留下的 `20261029-p82-rvm*` 站点已由探针清理（`output/probe-empty-equi/`）。

`P83` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p83-rvm`）：apply 删除 FTUB 1（M1），after-RVM 成员数不变但 M1 变成无名 `TUBE 1`（导出器把空缺处的隐含管段物化成无名记录），M2–M9 留存；restore 删整支 BRAN 重挂 `/C-OR-1R345-C` 并补名，restored 与 before 按名对拍为 0 差分。保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation`（deleted + ATT_MEMB vs reorder 仍需事件级证据；重挂 BRAN 的 refno 变化在 RVM 里按名抹平）。

`P84` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p84-rvm`）：apply 把 BEND 1 的 `SPRE` 从 `/ACP1000-Trough/ACP1000-TBR:50` 写成 `SPRE =0/0`，**TTY 证实该写法把 SPRE 清成 `Nulref`**（apply 日志 `Spref Nulref`，restore 再写回 TBR:50）——qualifier 1→0 的模型语义已验证，但 `DB_UserChanges` 是否记成 qualifier-removed（comb key 切换）仍需事件级证据。RVM 层面 SPRE 不可见，before/after/restored 均 13 成员、按名对拍 0 差分。保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation`。

`P85` 已于 2026-10-29 在 AMS TTY 跑通（run `20261029-p85-rvm`）：同一窗口内 `SPRE =13244/108852`→`TBR90:50X150`、`SPRE =13244/108851`→`TBL90:50X150` 两次写都生效（apply 日志可见最终值 `TBL90:50X150`），restore 写回 `TBR:50`；before/after/restored 均 13 成员、按名对拍 0 差分。但这只证明「两次写同一 qualifier 在一个窗口里都生效」；`combine` 是否把两条 qualifier map 项合并成一条、`DB_UserChanges` 留下几条记录，都没有运行时证据——保持 `calibrate`，阻塞原因收窄为 `requires-db-userchanges-event-observation`。

至此 P79–P85 全部在 TTY 生成了三份 RVM；状态一律 `calibrate`：`P79`–`P81`、`P83` 的内部分支（created 祖先去重/删除/属性过滤、deleted+ATT_MEMB）以及 `P82` 的 ATT_MEMB-vs-reorder、`P84`/`P85` 的 qualifier/combine 记录语义，都只能靠 `dc_events`/`DB_UserChanges` 运行时观测提升，RVM 只能提供最终态证据。要真正通过这些案例，需要补一个事件级采集器（例如导出 `DB_UserChanges` 表或挂 `dc_events` 探针）。

### P79–P85 的窗口操作流与事件采集器

七个案例的 TTY 窗口（`baseline.captured_sesno` → `apply_sesno`）已用生产收集器重放，回执在 `output/dc-events/<case>-netwindow-<base>-<target>.json` 与 `<case>-parity-*.json`。要点：`P79`/`P81` 各有两条 Add（容器+子件）且 owner 的 `MEMBERS` 单独进 Modified；`P80` 的 child 建后又删在净窗口里彻底消失（`fold_net_op` 的 `(Added,Delete)→Cancelled` 真实发生）；`P81` 的改名被 created 折叠、不出属性行；`P82`/`P83` 是成员集 ±1（`BRAN` 带 `MEMBERS`，`P83` 另有 `FTUB 26671` 的普通 `Deleted`）；`P84`/`P85` 的两次 `SPRE` 写都只留一条 `BEND` Modified（`SPAMAP,SPRE`）。净窗口是两端 `IndexDiff` 的终态，**结构上就给不出** `DB_UserChanges` 的逐次调用序列，`combine` 合并了几项看不出来。

`P83` 的 RVM 差异已改按几何判：三份快照 `members`/`geometries` 恒为 13/9，`M2`–`M9` 的 AABB 逐位相同，只有被删 `M1` 的槽位从 Box(8536.56…9393.56) 变成 Cylinder(8544.01…9387.11)。按名对拍会误报「消失 M1 / 新增 TUBE 1」。

事件级采集器落在 `tools/dc_events_probe.py`：只读 Frida hook，挂 core.dll 的 `elementCreated`/`elementDeleted`/`elementIncluded`/`elementReordered`/`attributeModified`（两个重载）/`combine`，把每次调用的 refno 写 jsonl，不调用目标函数、不替换实现、不改返回值、不写进程内存。`DB_Element` 的 refno 取 `+0x0C`/`+0x10`，已由 ida-bridge 在 `elementCreated` 的汇编里坐实（`mov eax,[edi+0Ch]` / `mov [esi+3Ch],eax`）；`__thiscall` 下 `this` 在 `ecx`，`onEnter(args)` 的 `args[0]` 才是第一个栈参数。导出名用 mangled 全名交给 loader 解析。布局校准细节见 `docs/specs/incr-gen-audit-aux/db-userchanges-layout-calibration.md`。

```powershell
python scripts/e3d/increment_suite/tools/dc_events_probe.py `
  --out output/dc-events/p84-live.jsonl --process des.exe --timeout 900
```

本轮没跑成实跑：七个 TTY 窗口跑完后 AMS 库被 `aios-database.exe`（pid 70192）独占，`ams8000_0001` 与伴生 `ams7998_0001` 都拿不到写句柄。收口顺序与判据见 `docs/specs/incr-gen-audit-aux/p79-p85-event-evidence.md`。

状态竞争 oracle 见 `partial-update-state-matrix.json`，记录 state 0/1/2/3/4/5 的顺序、抑制关系、预期 pending 集合和 draw/erase 结果。

可用以下命令重复审计方法、事件和资产完整性：

```powershell
pwsh -NoProfile -File scripts/e3d/increment_suite/Test-AmsIncrementCoverage.ps1 `
  -Assets 'output/ams-increment-rvm-assets/<run-id>' `
  -ReportPath 'output/ams-increment-rvm-assets/<run-id>/coverage-report.json'
```
