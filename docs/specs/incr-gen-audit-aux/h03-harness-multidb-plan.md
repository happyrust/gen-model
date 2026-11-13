# H03 多库回放 harness 改造方案

日期：2026-10-28
结论：只改 `tests/increment_replay/main.rs` 一个文件，约 **+128 / −11** 行。
- 新加字段全是 `#[serde(default)]`，新代码都挂在「空集合 / false」的分支上。
- 现有 8 条用例没有 `companions`，走的代码路径与现在逐行相同。
- 必须带上一道安全门：伙伴库工作文件在 `replay-init` 前先删。`replay-init` 对已存在的文件是就地覆写（`src/bin/db_session_fixture.rs:77`），而影子根里漏列进 `-Replayed` 的文件是指向原件的硬链接（`scripts/replay/New-ShadowProjectRoot.ps1:69-79`），不先删就会写穿到真工程。

复核方式：只读检索，未跑 `cargo`，未改任何文件。行号按 2026-10-28 工作树。

## 一、精确改动点（全在 `tests/increment_replay/main.rs`）

| # | 位置 | 改什么 |
|---|---|---|
| 1 | `:92` → `:93` | `Case` 里在 `known_red` 之后加字段 `companions`，定义见第二节。`Case` 和 `Step` 都没有 `deny_unknown_fields`（`:29`、`:136`） |
| 2 | `:99` 之后（紧跟 `SourceZip` `:95-99`） | 新增 `struct Companion` |
| 3 | `:257` → `:258` | `Step` 末尾加 `companion_to`、`expect_in_sync`、`hold_primary` 三个字段 |
| 4 | `:595` 之后（`source_args` `:573-595` 不动） | 新增三个小函数：`companion_of(case, dbnum)`、`companion_args(companion)`、`watch_list(case)` |
| 5 | `:1383-1384` 解析之后 | 加校验：伙伴库 `dbnum` 不重复、不等于主库；步里 `companion_to` 的键必须是已声明的伙伴库；`expect_in_sync` 的键必须是主库或已声明的伙伴库 |
| 6 | `:1436` 之后（主库 `replay-init` `:1424-1436` 不动） | 逐个处理伙伴库：若工作文件已存在先 `remove_file`，再 `replay-init --fixture <伙伴目录> --to <start> --out <影子根/db_file>` |
| 7 | `:1448-1452` | 把内联的 if/else 换成 `&watch_list(case)`。`derive_config`（`:511-571`）的签名不动 |
| 8 | `:1465` 之后、`seed`（`:1466-1471`）之前 | 逐个 `wait_in_sync(伙伴 dbnum, 伙伴 start, 600 s, "init 伙伴库 N")` |
| 9 | 步进循环三处 | `:1526` 前先推 `companion_to` 里的每个伙伴库；`:1526` 原来的 `replay_step(step.to, …)` 包一层 `if !step.hold_primary`；`:1527-1529` 的 `no_wait` 之后对 `expect_in_sync` 逐个 `wait_in_sync`，然后若 `hold_primary && !expect_batch_parked` 就 `continue` |
| 10 | `:2659` 之后（和 `:2660`、`:2701` 两条不需要环境的纯测试放在一起） | 新增零回归钉 |
| 11 | 模块文档 `:1-16` | 补一句伙伴库的说明（可选） |

## 二、新增字段与代码的完整定义

```rust
/// 同一个服务里一起监听、各自回放的伙伴库（INC-H03）。空 = 单库用例。
#[derive(Debug, Deserialize)]
struct Companion {
    dbnum: u32,
    /// 相对影子工程根的 DESI 文件路径（同 `Case::db_file`）。
    db_file: String,
    /// `aios-session-fixture-v1` 目录；伙伴库不接 `source_zip`。
    fixture_dir: String,
    start: u32,
}

// Case 新字段
    /// 伙伴库：自动并进监听名单，init 时各自 replay-init 并等三水位。
    #[serde(default)]
    companions: Vec<Companion>,

// Step 新字段
    /// 先把这些伙伴库推到给定会话（dbnum → to），再推主库。
    #[serde(default)]
    companion_to: BTreeMap<u32, u32>,
    /// 等这些库三水位都到给定会话（dbnum → sesno）。
    #[serde(default)]
    expect_in_sync: BTreeMap<u32, u32>,
    /// 主库这一步不动：不推文件、不等主库水位、不跑主库断言。
    /// 配 `expect_batch_parked` 时只复核主库仍 park，`to` 填主库当前的 park 右端。
    #[serde(default)]
    hold_primary: bool,
```

```rust
fn companion_of<'a>(case: &'a Case, dbnum: u32) -> &'a Companion {
    case.companions.iter().find(|c| c.dbnum == dbnum).unwrap_or_else(|| {
        panic!("用例 {} 的步里点了伙伴库 {dbnum}，但 companions 里没有它", case.name)
    })
}

fn companion_args(companion: &Companion) -> Vec<String> {
    vec![
        "--fixture".into(),
        repo().join(&companion.fixture_dir).to_string_lossy().into_owned(),
    ]
}

/// 监听名单：用例写了就用它，没写就是主库；伙伴库总在里面。
fn watch_list(case: &Case) -> Vec<u32> {
    let mut watch = if case.watch_dbnums.is_empty() {
        vec![case.dbnum]
    } else {
        case.watch_dbnums.clone()
    };
    for companion in &case.companions {
        if !watch.contains(&companion.dbnum) {
            watch.push(companion.dbnum);
        }
    }
    watch
}
```

init 段（插在 `:1436` 之后）：

```rust
    for companion in &case.companions {
        let path = env.project_root.join(&companion.db_file);
        // replay-init 就地覆写已有文件；影子根里漏列进 -Replayed 的伙伴库是到原件的硬链接，
        // 覆写会写穿到真工程。先删这一个链接（不碰原件），让 replay-init 写出独立的新文件。
        if path.exists() {
            fs::remove_file(&path).unwrap_or_else(|e| panic!("删 {}: {e}", path.display()));
        }
        let mut args = vec!["replay-init".to_string()];
        args.extend(companion_args(companion));
        args.extend([
            "--to".to_string(),
            companion.start.to_string(),
            "--out".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
        let report = run_fixture_tool(&env.fixture_exe, &args);
        eprintln!("  replay-init 伙伴库 {} → {report}", companion.dbnum);
    }
```

等伙伴库（插在 `:1465` 之后）：

```rust
    for companion in &case.companions {
        service.wait_in_sync(
            companion.dbnum,
            companion.start,
            Duration::from_secs(600),
            &format!("init 伙伴库 {}", companion.dbnum),
        );
    }
```

步进（替换 `:1526-1529`）：

```rust
        for (&dbnum, &to) in &step.companion_to {
            let companion = companion_of(case, dbnum);
            let mut args = vec!["replay-step".to_string()];
            args.extend(companion_args(companion));
            args.extend([
                "--work".to_string(),
                env.project_root.join(&companion.db_file).to_string_lossy().into_owned(),
                "--to".to_string(),
                to.to_string(),
            ]);
            let report = run_fixture_tool(&env.fixture_exe, &args);
            eprintln!("  replay-step 伙伴库 {dbnum} → {report}");
        }
        if !step.hold_primary {
            replay_step(step.to, step.half.as_deref(), step.rollback);
        }
        if step.no_wait {
            continue;
        }
        let step_timeout = Duration::from_secs(step.timeout_secs.unwrap_or(180));
        for (&dbnum, &sesno) in &step.expect_in_sync {
            service.wait_in_sync(dbnum, sesno, step_timeout, &format!("step → 库 {dbnum} 到 {sesno}"));
        }
        if step.hold_primary && !step.expect_batch_parked {
            continue;
        }
```

零回归钉：

```rust
#[test]
fn cases_without_companions_keep_the_single_dbnum_shape() {
    for case in load_cases() {
        if !case.companions.is_empty() {
            continue;
        }
        let before = if case.watch_dbnums.is_empty() { vec![case.dbnum] } else { case.watch_dbnums.clone() };
        assert_eq!(watch_list(&case), before, "{}", case.name);
        assert!(
            case.init.iter().chain(&case.steps).all(|s| s.companion_to.is_empty() && s.expect_in_sync.is_empty() && !s.hold_primary),
            "{} 没有伙伴库却用了伙伴字段",
            case.name
        );
    }
}
```

## 三、设计决策

### init 阶段：顺序循环，不并行

`replay-init` 只是切文件，很快；顺序执行日志好读，也不会在同一个运行目录里并发解 zip。

影子根要预先处理：伙伴库文件必须列进 `-Replayed`（`New-ShadowProjectRoot.ps1:23`、`:71-75`，参数本来就是数组）。上面那道先删的安全门，是防漏列时写穿到真工程。

服务起来后两库会并发初始化，因为派生配置强制 `auto_initialize_new_dbnums = true`（`main.rs:538-540`）。

### in_sync 超时：每库各 600 s，顺序等

最坏情况是累加。因为两库是并发初始化的，等完主库时伙伴库多半已经就绪，第二次等待会立刻返回。

如果伙伴库是 7997 这类重库，可以给 `Companion` 加一格 `init_timeout_secs`；本方案没加。

### 步进：伙伴库先推，主库后推

只推伙伴、主库不动的步用 `hold_primary`。配上 `expect_batch_parked` 时会落进现有的 park 分支（`:1592-1621`），复核三件事：主库仍 park；`streak` 不涨；主库 `applied` 仍小于 `to`。

### `watch_dbnums` 自动并入伙伴库

由 `watch_list` 处理，用例不用手写。

### 字段命名

`expect_applied` 改叫 **`expect_in_sync`**：它直接复用 `wait_in_sync`，判的是三水位（`main.rs:963-991`），不只是 `applied`。只看 `applied` 要另写一个等待函数。

## 四、H03 用例写法

尖括号里是录完才知道的值。

```json
{
  "name": "ams8000-two-db-park-isolation",
  "registry": ["INC-H03"],
  "dbnum": 8000,
  "db_file": "AvevaMarineSample/ams000/ams8000_0001",
  "fixture_dir": "tests/fixtures/sessions/ams8000-equi-move-chain",
  "start": 725,
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
  "init": { "to": 725, "records_present": ["24384/24776", "24384/23257"] },
  "steps": [
    { "to": 726, "expect_batch_parked": true, "timeout_secs": 240 },
    {
      "to": 726,
      "hold_primary": true,
      "companion_to": { "<B>": "<SB+1>" },
      "expect_in_sync": { "<B>": "<SB+1>" },
      "expect_batch_parked": true,
      "timeout_secs": 300
    },
    { "to": 727, "expect_batch_failure_cleared": true, "task_state": "succeeded", "timeout_secs": 300 }
  ]
}
```

## 五、零回归的理由

- **反序列化**：新字段全是 `#[serde(default)]`。`companions` 默认空 `Vec`，两个 map 默认空，`hold_primary` 默认 `false`。现有 8 个 JSON 一字不改也能解开。
- **执行路径**：对没有伙伴库的用例，每处新代码都是空操作：
  - 改动点 6、8 和步内的两个循环，循环次数都是 0；
  - `if !hold_primary` 恒为真，所以 `replay_step` 照旧执行；
  - `hold_primary && …` 恒为假，照旧走下去；
  - 校验对空集合自然通过；
  - `watch_list` 对没有伙伴库的用例，等于 `:1448-1452` 原来那个表达式，由改动点 10 的钉钉住。
- **不碰的地方**：现有语句不重排；原来的 timeout（`:1553`）不移动，伙伴库用自己的 `step_timeout`；`source_args`、`derive_config`、`wait_in_sync` 的签名都不变。
- **验证分两档**：
  - 没有影子根的机器上，跑 `cargo test --test increment_replay`，只会跑纯测试和新钉（主用例在 `:2639-2644` 跳过）；
  - 真机上再跑一次全量 8 条 L2。

## 六、预估改动行数

约 **+128 / −11**（`tests/increment_replay/main.rs`）：

| 项 | 行数 |
|---|---|
| `Companion` | 11 |
| `Case` 字段 | 3 |
| `Step` 字段 | 11 |
| 三个小函数 | 26 |
| 解析校验 | 约 14 |
| init `replay-init` | 约 17 |
| init 等待 | 8 |
| 监听名单 | −10/+1 |
| 步进 | 约 +22/−1 |
| 零回归钉 | 约 16 |

不在本方案里的事（属 T-I-3）：H03 用例 JSON、伙伴库会话夹具、登记表 `:57` 行。

## 七、假设与未确认

- `serde_json` 把 JSON 对象的字符串键（`"7998"`）解成 `BTreeMap<u32, u32>`：认为支持，但没编译验证。
- `wait_for_log_line`（park 分支用它等「本轮不自动重跑」）是扫全日志还是只看新增，没核。两种都能过：这句每次重扫都会打（`ams8000-equi-data-fail-park.json:4`）。
- 伙伴库 B 选哪个、它的夹具和初始化耗时都还没有，属 T-I-3。
- 只给伙伴库加了先删门；主库仍依赖影子根的 `-Replayed` 默认值（`:23`），这是有意的，为了保持零回归。
