# T-I-3 缺失 L2 链可行性：G03 / H03 / C05 / C06

日期：2026-10-28
结论：**四条都不能「纯靠现有夹具 + 现有 .mac」录出来。**
- G03 离得最近：宏已有，改一处快照路径 + 真机录一次，用现成字段能到 `partial`。
- H03 卡在 harness 只支持单库，另缺第二库的会话夹具。
- C05 / C06 都缺宏，要先写宏再真机录。

复核方式：只读检索，未跑 `cargo`，未改任何文件。行号按 2026-10-28 工作树。

## 一、逐条判定

### G03 PANE / FRMW —— 能录，但要先改宏、再真机录

现有三只宏：

| 宏 | 作用 | 位置 |
|---|---|---|
| `room_pane_probe.mac` | 只读探针，对象是 FRMW `24381/35842`（`/1RX-RM05-R512`）与 PANE `24381/35844` | `scripts/e3d/replay/room_pane_probe.mac:2` |
| `room_pane_apply.mac` | 对 PANE 做 `BY D 200mm` 并 `SAVEWORK` | `:8-15` |
| `room_pane_restore.mac` | `BY U 200mm` 复位 | `:6-13` |

**卡点一（必须先改）**：PANE 的 Ref0 是 `24381`，而 `24381` 是 7997 的 Ref0（`scripts/e3d/ams7997_geom_crosscheck.mac:4`），所以改动会写进 `ams7997_0001`。但两只宏的 `SYSCOM` 却拷 `ams8000_0001`（`room_pane_apply.mac:17`、`room_pane_restore.mac:15`），链名也写成 ams8000。这样快照拷错文件，字节补丁对不上。**改成拷 `ams7997_0001`，用例用 dbnum 7997**。（需真机确认：`=24381/35844` 之后 `Q DBNAME`。）

**卡点二**：harness 里没有「一条 `room_rebuild` 任务」的断言字段（`tests/increment_replay/main.rs` 里搜不到 `room_rebuild`）。「边数与全量相同」可以先用现成的 `rebuild_edges_equal`（`main.rs:63-67`）按根对拍。所以录好后用现成字段能到 `partial`；要到 `has`，得在 `tests/` 里补断言字段。

**卡点三**：7997 的初始化很重。它有 49062 行模型产物（`DbOption.toml:264-268`），规划就要 13.8 s（`increment_cases.json:62`）；init 等待上限是 600 s（`main.rs:1465`）。

### H03 两库 —— 不能录，缺 harness 能力和第二库夹具

harness 只支持一个库：
- 用例只有一组 `dbnum` / `db_file` / `fixture_dir`（`main.rs:30-49`）；
- 只对一个工作文件做 `replay-init`（`:1425-1436`）；
- 只等这一个库 `in_sync`（`:1465`）。

现有能力不够用：
- `watch_dbnums` 有（`:76-80`、`:1448-1452`）；
- `seed` 只在 init 之后跑一次（`:1466-1471`）；
- 步上没有「推进另一个库」和「断言另一个库前进」的字段。

会话夹具只有三条 8000 的链。做法见第二节。

### C05 PIPE —— 不能录，缺宏，需真机操作

仓里没有「整条 PIPE 位移」的宏。和 PIPE 相关的只有两类：
- `scripts/e3d/db8000_pipe_*` 这组：scratch 建删、改挂、加成员、换 SPRE；
- `db8000_bran_hpos_apply.mac:24-35`，只动支管头：`/C-OR-1R345-C = 24384/23257`，HPOS 从 U2900 到 U3400。

后一只同样让 HPOS 变、走 regen，可以当半个替身。但它 `SAVEWORK` 后没有 `SYSCOM` 快照，而回放链要靠这份快照做字节补丁（写法见 `replay/equi_apply.mac:3-4`、`:16`）。

登记表把 C05 记成 L3（`tests/fixtures/increment_cases.json:27`）。按 L2 录的话，`layer` 要一起改。

### C06 NOZZ —— 不能录，缺目标和宏，需真机操作

现有链的 EQUI `24384/24776` 只有 BOX 子件（`tests/fixtures/sessions/ams8000-equi-rotate-chain/manifest.json:140-154`），没有 NOZZ。

要先探出一个带 NOZZ 的 8000 EQUI，再照 `equi_apply.mac` / `equi_restore.mac` 写一对宏来录。

登记表同样记成 L3（`increment_cases.json:28`）。

## 二、H03 两库的做法

### 第二库（下文叫 B）

从 `ams000` 里选一个小的 DESI 库。候选 **7998**：它在监听名单里（`DbOption.toml:269`），E03 也用过它（`increment_cases.json:40`）。它的 STYP 和体量要用真机或 `mdb_project_probe`（`src/bin/mdb_project_probe.rs:56-75`）确认。

### 造 B 的会话链（真机，在隔离副本上）

一条只登录不改的宏就够：只读的 Design 会话会给打开的库写一个只含 `DBMDAT` 的会话（`replay/equi_login.mac:1-3`）。也可以改 B 里某个元素的 `DESC` 再 `SAVEWORK`。

宏里按 `equi_apply.mac` 的写法，在 `SAVEWORK` 后加 `SYSCOM` 快照。录制与打包命令见第三节，`-Dbnum` 参数填 B，夹具输出到 `tests/fixtures/sessions/ams<B>-login-chain`。

### 怎么让一库 park、另一库继续前进

- A=8000 用现成的钉 `AIOS_TEST_FAIL_DATA_ENDS="8000:726"` 打成数据失败，它就会 park（`ams8000-equi-data-fail-park.json:10`、`:17`）。这个钉只命中 8000，别的库会话号相同也不命中（同文件 `:4`）。
- park 期间推 B 一个会话，然后断言三件事：
  - B 的 `applied_sesno` 到了 `SB+1`；
  - `/health.batch_failures.parked` 里仍有 `8000:726`；
  - A 的 `applied` 仍是 725。
- B 能前进靠两处已落地的改动：
  - **T-O-1**：park blocker 按库限定（`batch_scheduler.rs:670` / `:728` / `:1094` 的 `allows_dbnum`）；
  - **T-O-2**：有库在跑时，其它库照常入队（`increment_manager.rs:4972-5011`）。

### 影子根

```
New-ShadowProjectRoot.ps1 -Replayed AvevaMarineSample\ams000\ams8000_0001,AvevaMarineSample\ams000\ams<B>_0001
```

`-Replayed` 本来就是 `[string[]]`（`:23`、`:71`）；两个文件都在 `ams000` 的硬链接区（`:20-21`），可以安全回放。

### harness 要补的地方（都在 `tests/`）

1. 用例加伙伴库（`dbnum` / `db_file` / `fixture_dir` / `start`）。init 时也对它 `replay-init` 并等 `in_sync`，写法照 `main.rs:1425-1436` 和 `:1465`。
2. 步上加两个字段：「把伙伴库推到 n」和「等某个库的 `applied == n`」。还要允许主库 `to` 不变的步。
3. `watch_dbnums` 填 `[8000, B]`。

### 备选做法（不录 B）及不推荐的理由

做法：B 用静态的硬链接文件；init 之后用 `seed` 把 B 的 `applied` 往回钉一格，让对账补窗。

不推荐，两个原因：
- 得同时钉 model 水位，否则 `model > applied` 会被当成水位不一致；
- 补的是已经落过的会话，测不到「新会话照常前进」。

## 三、会话夹具的产生流程（命令级）

1. **隔离副本**：把 MDB 涉及的项目整拷到 `E:\reverse\e3d\replay-src\E3D3.1`，原工程不动（`changelog.md:120`）。
2. **写宏**，放在 `scripts/e3d/replay/*.mac`：
   - 恰好一个 `SAVEWORK`，没有 `QUIT` / `FINISH`，`Q REF` 和 `Q NAME` 成对；录制前会静态审查这几条（`Record-Db8000SessionChain.ps1:66-95`）；
   - `SAVEWORK` 之后用 `SYSCOM` 拷一份库文件当真快照（`replay/equi_apply.mac:3-4`、`:16`）。
3. **案例清单 JSON**，默认是 `scripts/e3d/db8000_recording_cases.json`。
4. **录制**：
   - 先审宏：`powershell -File scripts\e3d\Record-Db8000SessionChain.ps1 -TargetDbFile <副本>\AvevaMarineSample\ams000\ams8000_0001 -ProjectDir <副本>\AvevaMarineSample -CheckOnly`（参数见 `:30-41`；脚本会先 `cargo build l3_suite` 和 `db_session_fixture`，`:160-161`）；
   - 再去掉 `-CheckOnly` 正式录。每条腿走 `l3_suite --check-driver`（`:116-117`），每腿之后 sesno 必须恰好 +1（用 `db_session_fixture inspect` 核对，`:60-64`）；
   - 产出 `output/db8000-recording/<时间>/recording.json`（`:224-230`）。
5. **打包与复验**：
   - 打包：`target\debug\db_session_fixture.exe pack --recording <recording.json> --dbnum <N> --out tests/fixtures/sessions/<链名>`（`:235-237`；`db_session_fixture.rs:50-65`）；
   - 复验：`db_session_fixture verify --fixture <目录>`（`:66-71`）；
   - 产出 `manifest.json`（格式 `aios-session-fixture-v1`）、zip 和 `SHA256SUMS`。
6. **用例 JSON**：写 `tests/increment_replay/cases/<链名>.json`，含 `fixture_dir` / `start` / `init` / `steps`。harness 跑的是 `replay-init --fixture … --to start --out <工作文件>`，之后逐步 `replay-step … --to n`（`main.rs:1425-1436`、`:1477-1489`）。
7. **跑用例**：
   - 先用 `New-ShadowProjectRoot.ps1 -Target <D 盘目录> -Replayed …` 造影子根；
   - 设 `$env:AIOS_REPLAY_PROJECT_ROOT=<目录>`（`main.rs:476-478`）和 `$env:AIOS_REPLAY_CASE=<链名>`；
   - 跑 `cargo test --test increment_replay -- --nocapture --test-threads=1`。

### 缺口

- `Record-Db8000SessionChain.ps1` 产出的 `recording.json` 只有 `dbnum` / `baseline_sesno` / `source` / `cases`（`:224-230`）。
- 字节补丁要的 `snapshots` 字段它不写（参见 `docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md:9`、`:258`）。
- 三条已入库的链是怎么把 `snapshots` 填进 `recording.json` 的，仓里没找到脚本或清单（`scripts/e3d/replay/` 里只有宏）。这一步得照当时的做法补。

## 四、INC 登记表的位置与 status 改法

位置：`tests/fixtures/increment_cases.json`。C05 在 `:27`，C06 在 `:28`，G03 在 `:52`，H03 在 `:57`；四条现在都是 `"status": "todo"`、`"tests": []`。

改法：`status` 改成 `"partial"`，`tests` 里写 `"replay:<用例名>"`，`stage` 保留。对应的用例 JSON 里反向写上 `"registry": ["INC-…"]`（例：`ams8000-equi-data-fail-park.json:3`）。

登记表的钉要求（`tests/increment_cases_registry.rs`）：
- `status` 只能是 `has` / `partial` / `todo` / `corpus`（`:139`）；
- `has` 和 `partial` 都必须有承载测试（`:171-177`）；
- `partial` / `todo` / `corpus` 必须有 P0–P5 的 `stage`（`:178-184`）；
- `replay:<名>` 必须对得上 `tests/increment_replay/cases/<名>.json`（`:114-121`）。

改完用 `cargo test --test increment_cases_registry` 核。

C05 / C06 按 L2 录的话，要把 `layer` 从 L3 改成 L2；也可以保持 L3，改走 `l3:<场景 id>`（`:93-96`）。

## 五、JSON 草稿（结构对齐 `ams8000-equi-fail-inject.json`）

尖括号里的是录完才知道的值。H03 草稿里的 `companions` / `companion_to` / `expect_applied` 是待加字段，现在的 harness 不认。

### G03

```json
{
  "name": "ams7997-room-pane-chain",
  "registry": ["INC-G03"],
  "_doc": "INC-G03：房间定义元素 PANE =24381/35844（FRMW /1RX-RM05-R512 的面板）整块下移 200 mm 再复位（replay/room_pane_apply.mac / room_pane_restore.mac，SYSCOM 快照改拷 ams7997_0001）。<S> 基线 → <S+1> SAVEWORK 下移 → <S+2> QUIT 尾 → <S+3> SAVEWORK 复位 → <S+4> QUIT 尾。期望一条 room_rebuild、边与全量相同；「一条 room_rebuild」的断言字段未有，先用 rebuild_edges_equal 对拍到 partial。",
  "dbnum": 7997,
  "db_file": "AvevaMarineSample/ams000/ams7997_0001",
  "fixture_dir": "tests/fixtures/sessions/ams7997-room-pane-chain",
  "start": "<S>",
  "service_env": {
    "AIOS_WATCH_RECONCILE_SECS": "2",
    "AIOS_WATCH_MIN_SWEEP_GAP_SECS": "1",
    "AIOS_WATCH_POLL_SECS": "1"
  },
  "rebuild_edges_equal": ["<R512 房内某根 refno>"],
  "init": { "to": "<S>" },
  "steps": [
    { "to": "<S+1>", "task_state": "succeeded", "timeout_secs": 600 },
    { "to": "<S+2>", "task_state": "succeeded", "unit_count": 0 },
    { "to": "<S+3>", "task_state": "succeeded", "timeout_secs": 600 },
    { "to": "<S+4>", "task_state": "succeeded", "unit_count": 0, "log_must_not_contain": ["[模型目标核对]"] }
  ]
}
```

### H03

```json
{
  "name": "ams8000-two-db-park-isolation",
  "registry": ["INC-H03"],
  "_doc": "INC-H03（IU-INV-04）：8000 的右端 726 用 AIOS_TEST_FAIL_DATA_ENDS 打成数据失败并 park；park 期间伙伴库 <B> 推进一个会话，applied 必须照常前进；再推 8000 到 727 清账追平。",
  "dbnum": 8000,
  "db_file": "AvevaMarineSample/ams000/ams8000_0001",
  "fixture_dir": "tests/fixtures/sessions/ams8000-equi-move-chain",
  "start": 725,
  "watch_dbnums": [8000, "<B>"],
  "companions": [
    {
      "dbnum": "<B>",
      "db_file": "AvevaMarineSample/ams000/ams<B>_0001",
      "fixture_dir": "tests/fixtures/sessions/ams<B>-login-chain",
      "start": "<SB>"
    }
  ],
  "service_env": {
    "AIOS_TEST_FAIL_DATA_ENDS": "8000:726",
    "AIOS_WATCH_RECONCILE_SECS": "2",
    "AIOS_WATCH_MIN_SWEEP_GAP_SECS": "1",
    "AIOS_WATCH_POLL_SECS": "1"
  },
  "init": {
    "to": 725,
    "records_present": ["24384/24776", "24384/23257"],
    "expect_applied": { "<B>": "<SB>" }
  },
  "steps": [
    { "to": 726, "expect_batch_parked": true, "timeout_secs": 240 },
    {
      "to": 726,
      "companion_to": { "<B>": "<SB+1>" },
      "expect_applied": { "<B>": "<SB+1>" },
      "expect_batch_parked": true,
      "timeout_secs": 300
    },
    { "to": 727, "expect_batch_failure_cleared": true, "task_state": "succeeded", "timeout_secs": 300 }
  ]
}
```

### C05 / C06

没有宏也没有目标元素，不给草稿，卡点见第一节。

## 六、假设与未确认

- PANE `24381/35844` 在 7997，是按 Ref0 推的，需要真机确认。
- PANE 下移是否恰好排出一条 `room_rebuild`，要跑了才知道。
- B 选 7998 是否合适（STYP、体量）没核。
- `db8000_bran_hpos_restore.mac`（`apply` 宏 `:20` 点名的搭档）是否在仓里，没核。
- 三条已入库链的 `snapshots` 是怎么进 `recording.json` 的，没找到。
- harness 是否允许「主库 `to` 不变」的步，没核，草稿里假设要补。

## 七、对 G3 的影响

T-I-3 的四条都不能在本机纯代码完成：

| 用例 | 阻塞 |
|---|---|
| G03 | 需改宏的快照路径 + 真机录一次 |
| H03 | 需先改 `tests/increment_replay/main.rs` 支持多库，再造第二库会话夹具，再真机录 |
| C05 | 缺宏，需真机写宏 + 录 |
| C06 | 缺目标元素与宏，需真机探 + 写宏 + 录 |

**G3 门的「H03 夹具绿」这一条因此不能在本轮达成**，需真机 E3D 操作窗口。其余两条（T-D-6 有结论、`source_end_sesno` 不恒 0）已达成。
