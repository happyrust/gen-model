# PML 增量更新测试套件

针对 gen-model 的**元件级增量更新**（创建 / 修改 / 删除元件）的 PML 宏套件。设计依据见
`docs/specs/incr-gen-audit-aux/pml-increment-suite-design.md`（七部分，含覆盖矩阵、自审、FAIL 修正、**TTY 真机实测**）。

**状态：已在 E3D TTY 真机跑通 31/34 个用例腿**（2026-10-28，隔离副本 db8000）。
每腿稳定 +2 会话。剩余 3 腿阻塞在 7998 库锚点未解。

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
  connect/   9 个           连接：管道增删成员 / 改挂 / NOZZ 移动 / REORDER 探针
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

- **属性级真逆**（refno 不变）：`attr/*`、`xform/*`、`geom/resize`、`geom/nbox_resize`、
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

## 未编进套件的两条

| ID | 原因 | 下一步 |
|---|---|---|
| `C0x-bran-reorder` | `REORDER` **TTY 实测 `CP: Syntax error`**，确认不存在 | 需改用别的机制触发成员换序；`connect/bran_reorder_probe.mac` 保留作证据 |
| `C0x-spre-swap` | 卡 db 13244 语料，`db8000_pipe_spre_swap_apply.mac:22` 的 `SPRE =<ALT_SPRE>` 占位未填 | 先补语料 |

## 覆盖率的诚实说明

覆盖矩阵 56 条里，**PML 能触发 24 条（43%）**：

- **a 类 16 条**：现有宏即可触发
- **b 类 8 条**：需新宏（本套件已提供）
- **c 类 32 条**：PML 触发不了 —— 文件层（库身份、会话对、回退）、工具层（`/tasks`、`/dbnums`、
  `/health` 契约）、目录库语料（Paragon、13244）、API 计时

**关键：c 类 32 条里没有一条是「元件增删改」本身。** 所以「元件级增量更新全覆盖」是成立的，
剩下 57% 本质不属于 PML 能力范围，要用 Rust 测试覆盖。

## TTY 实测推翻的静态结论

| 语句 | 静态审计判定 | TTY 实测 |
|---|---|---|
| `REORDER` | ★ 零命中 | ❌ `CP: Syntax error` — 确认不存在 |
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

**状态：模型与脚本已就位，真值一份都还没导**（要在隔离副本上跑 E3D）。套件的几何裁判和 K 组一样是
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

