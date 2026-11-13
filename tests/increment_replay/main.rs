//! L2 回放夹具（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §3.1 / §5.2）：
//! 把录好的会话链按 SAVEWORK 逐步「就地」喂给**真的**监听器 / 调度器 / worker / 水位 / 任务表，不要 E3D。
//!
//! 每条用例（`cases/*.json`）一套进程：先在随机回环端口起一台一次性的 `bin/surreal.exe`（rocksdb 在运行目录里，
//! 与 `l3_suite` 同款），再从仓根 `DbOption.toml` 派生一份配置（`project_path` → 影子工程根、`watch_dbnums` 只圈用例
//! 的库、`auto_initialize_new_dbnums = true`、`v_ip` / `v_port` 指向那台 surreal、随机 HTTP 端口），以 `AIOS_STORE_MODE=rocksdb`
//! + `AIOS_DATA_FACE=ingest` 起本仓的 `aios-database.exe`（**摄入形态**——批次 / 水位 / 任务都在这一面；`mem` 存储走的是
//! 读透形态，没有水位，不是本夹具要测的那一层），等该库从会话 S 初始化到三水位相等，然后逐步
//! `db_session_fixture replay-step` → 等 `applied == file_latest == model == to` → 按用例断言（O2 库上行 / O3 回执与水位）。
//!
//! 前置（缺任一条整档跳过并把原因打出来，与 `direct_index_contract` 等要本机语料的门同款）：
//! - `AIOS_REPLAY_PROJECT_ROOT`：影子工程根（`scripts/replay/New-ShadowProjectRoot.ps1` 造，别的项目 junction、
//!   `ams000` 硬链接，被回放的 DESI 留空）；
//! - 仓根 `DbOption.toml`（本机开发配置，模板 / 资产路径从它来）。
//!
//! 跑法：`cargo test --test increment_replay -- --nocapture --test-threads=1`；只跑一条加 `AIOS_REPLAY_CASE=<name>`。
//! 单条用例的超时：初始化 10 min、每步 180 s。

mod assertions;
mod case;
mod harness;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::assertions::{
    assert_companion_tasks, assert_operator_surface, assert_rebuild_parity, assert_room_edges,
    assert_step, assert_task_history, check_increment_budget, compare_with_stash,
    health_and_log_problems, parse_plan_totals, records_differ, restart_parity,
};
use crate::case::{
    Case, Step, companion_args, companion_of, known_red_allowed, load_cases, unknown_keys,
    watch_list,
};
use crate::harness::{
    Env, Service, derive_config, environment, free_port, repo, run_fixture_tool, source_args,
};

fn run_case(env: &Env, case: &Case) {
    let run_dir = repo().join("output/increment-replay").join(format!(
        "{}-{}",
        case.name,
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    fs::create_dir_all(&run_dir).expect("run dir");
    eprintln!(
        "== 用例 {}（{}）运行目录 {}",
        case.name,
        case.registry.join(" "),
        run_dir.display()
    );

    // 这条用例的工程目录要在影子工程根里（缺省 AMS；别的工程要用以它为主造出来的那一份影子根）。
    // 不认识的键不参与判定：先说清楚，免得把「没判」当成「判过了」。
    if !case.unknown_keys.is_empty() {
        eprintln!(
            "== 用例 {} 里有回放程序不认识的键，这些格**没有判**（拼错了？想当注释就以下划线开头）：{}",
            case.name,
            case.unknown_keys.join("、")
        );
        let strict = std::env::var("AIOS_REPLAY_STRICT_KEYS").unwrap_or_default();
        assert!(
            matches!(strict.trim(), "" | "0" | "false"),
            "AIOS_REPLAY_STRICT_KEYS 开着：用例 {} 有不认识的键 {:?}",
            case.name,
            case.unknown_keys
        );
    }
    let project_dir_name = case
        .project
        .as_ref()
        .map_or("AvevaMarineSample", |spec| spec.name.as_str());
    assert!(
        env.project_root.join(project_dir_name).is_dir(),
        "影子工程根 {} 下没有 {project_dir_name}（用例 {} 的工程）",
        env.project_root.display(),
        case.name
    );

    // 1. 工作文件到会话 S。主库和伙伴库都先把影子根里那个路径上的东西删掉再 init（见 `clear_for_replay_init`）。
    let work = clear_for_replay_init(&env.project_root, &case.db_file);
    let source = source_args(case, &run_dir);
    let mut init = vec!["replay-init".to_string()];
    init.extend(source.iter().cloned());
    init.extend([
        "--to".to_string(),
        case.start.to_string(),
        "--out".to_string(),
        work.to_string_lossy().into_owned(),
    ]);
    let report = run_fixture_tool(&env.fixture_exe, &init);
    eprintln!("  replay-init → {report}");
    for companion in &case.companions {
        let path = clear_for_replay_init(&env.project_root, &companion.db_file);
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

    // 2. 起服务，等初始化到 S。
    let port = free_port();
    let surreal_port = free_port();
    derive_config(
        &env.base_config,
        &run_dir.join("DbOption.toml"),
        &env.project_root,
        port,
        surreal_port,
        &watch_list(case),
        case.project.as_ref(),
        &case.config_overrides,
    );
    if !case.config_overrides.is_empty() {
        eprintln!(
            "== 用例 {} 改了派生配置（不是产品缺省）：{:?}",
            case.name, case.config_overrides
        );
    }
    let mut service = Service::start(env, &run_dir, port, surreal_port, &case.service_env);
    let started = Instant::now();
    while service.get("health").is_none() {
        service.assert_alive("startup");
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "180 s 内 /health 没有起来；日志 {}",
            service.stdout_path.display()
        );
        std::thread::sleep(Duration::from_millis(1000));
    }
    service.wait_in_sync(case.dbnum, case.start, Duration::from_secs(600), "init");
    for companion in &case.companions {
        service.wait_in_sync(
            companion.dbnum,
            companion.start,
            Duration::from_secs(600),
            &format!("init 伙伴库 {}", companion.dbnum),
        );
    }
    // 前置状态（`seed`）：只在这里造得了的东西——库级目录落款只有真跑过整库生成才会写，
    // 而「落后」要一个钉在旧会话上的落款。写在 init 断言之前，断言块就在同一拍里等对账。
    for sql in &case.seed {
        let rows = service.store_rows(sql);
        eprintln!("  [seed] {sql} → {} 行", rows.len());
    }
    let mut stash: BTreeMap<(u32, String), Vec<Value>> = BTreeMap::new();
    let stash_after = |service: &Service,
                       stash: &mut BTreeMap<(u32, String), Vec<Value>>,
                       at: u32,
                       step: &Step,
                       what: &str| {
        for root in &step.stash_records {
            let (source, items) = service
                .records_items(root)
                .unwrap_or_else(|| panic!("[{what}] 根 {root} 不在索引里，存不了"));
            eprintln!(
                "  [{what}] 存下根 {root} 的 {} 行（source={source}）",
                items.len()
            );
            stash.insert((at, root.clone()), items);
        }
    };
    if let Some(init_checks) = &case.init {
        assert_step(&mut service, case, init_checks, "init");
        // `init` 里的 `stash_records` 按起点会话存：给后面「改了又改回去」的对拍当基线。
        stash_after(&service, &mut stash, case.start, init_checks, "init");
    }

    // 3. 逐步回放。
    let replay_step = |to: u32, half: Option<&str>, rollback: bool| {
        let mut args = vec!["replay-step".to_string()];
        args.extend(source.iter().cloned());
        args.extend([
            "--work".to_string(),
            work.to_string_lossy().into_owned(),
            "--to".to_string(),
            to.to_string(),
        ]);
        if let Some(half) = half {
            args.extend(["--half".to_string(), half.to_string()]);
        }
        if rollback {
            args.push("--rollback".to_string());
        }
        let report = run_fixture_tool(&env.fixture_exe, &args);
        eprintln!("  replay-step → {report}");
    };
    for step in &case.steps {
        // 步一级的 `known_red`：这一步的规格钉住了、产品还没修。整条用例的 `known_red` 是另一回事
        // （在 `load_cases` 之后统一跳过）。步一级跳过会把本条剩下的步一起跳过——步与步是串起来的，
        // 中间少一步后面的水位就对不上，所以现在没有用例这么写；留着这一格是为了将来「末尾补一刀」那种。
        if step.known_red && !known_red_allowed(&case.name) {
            eprintln!("== 跳过 {} 的这一步（known_red）：{step:?}", case.name);
            break;
        }
        // 失败注入要发生在「文件前移」之前，服务才是在这一窗的生成里失败（INC-F05 / H05，计划 §6 D6）。
        if !step.fail_roots.is_empty() {
            service.restart_with_fail_roots(Some(step.fail_roots.join(",")), "失败注入");
        } else if step.clear_fail_injection {
            service.restart_with_fail_roots(None, "清钩子");
        }
        // 步一级的前置状态：要在这一窗的文件前移之前写进库。
        for sql in &step.seed {
            let rows = service.store_rows(sql);
            eprintln!("  [seed → {}] {sql} → {} 行", step.to, rows.len());
        }
        for (&dbnum, &to) in &step.companion_to {
            let companion = companion_of(case, dbnum);
            let mut args = vec!["replay-step".to_string()];
            args.extend(companion_args(companion));
            args.extend([
                "--work".to_string(),
                env.project_root
                    .join(&companion.db_file)
                    .to_string_lossy()
                    .into_owned(),
                "--to".to_string(),
                to.to_string(),
            ]);
            let report = run_fixture_tool(&env.fixture_exe, &args);
            eprintln!("  replay-step 伙伴库 {dbnum} → {report}");
        }
        // 别的库上「因这一步多出来的任务」要和推文件之前比，所以先记下它们现有的任务号。
        let tasks_before: BTreeMap<u32, std::collections::BTreeSet<String>> = step
            .companion_tasks
            .iter()
            .map(|expect| (expect.dbnum, service.task_ids(expect.dbnum)))
            .collect();
        // 这一步的日志从这里算起（`log_must_contain`、`kill_on_log` 只看这之后的）。
        service.mark_log();
        if !step.hold_primary {
            if step.push_while_down {
                // 后端停着的时候有人保存：停 → 推文件 → 拉起，启动时要自己发现落后并补上。
                service.stop("停机期间推文件");
                replay_step(step.to, step.half.as_deref(), step.rollback);
                service.start_again("停机期间推文件");
            } else {
                replay_step(step.to, step.half.as_deref(), step.rollback);
            }
            if let Some(needle) = &step.kill_on_log {
                // 一窗做到一半杀进程：等这一步的日志里出现这一句（位置由延时钩撑开），杀掉再拉起。
                let line = service
                    .wait_for_log_after_mark(needle, Duration::from_secs(180), "kill_on_log")
                    .unwrap_or_else(|| {
                        panic!(
                            "[step → {}] 180 s 内日志里没出现「{needle}」，杀不到一半的位置；日志 {}",
                            step.to,
                            service.stdout_path.display()
                        )
                    });
                eprintln!("  [kill_on_log] {line}");
                service.restart("一窗做到一半杀进程");
            }
        }
        if step.no_wait {
            continue;
        }
        let step_timeout = Duration::from_secs(step.timeout_secs.unwrap_or(180));
        for (&dbnum, &sesno) in &step.expect_in_sync {
            let what = format!("step → 库 {dbnum} 到 {sesno}");
            service.wait_in_sync(dbnum, sesno, step_timeout, &what);
        }
        if step.hold_primary && !step.expect_batch_parked {
            // 主库不动的步：不等水位、不跑主库断言，只看后端自己的状态（`/health` 的格子、这一步的日志）。
            let what = format!("step（主库不动）→ {}", step.to);
            let problems = health_and_log_problems(&mut service, step, &what);
            assert!(
                problems.is_empty(),
                "[{what}] 断言不过：\n{}\n日志 {}",
                problems.join("\n"),
                service.stdout_path.display()
            );
            continue;
        }
        if let Some(race) = &step.race {
            // 等监听器把 to 的批次跑起来（测试钩正在冻结与复核之间等着），再把下一会话按真机的写序塞进去：
            // 先页（含对 to 会话页的就地改写——anchor 就是在这一半变的），后头。
            let needle = format!("[测试钩] 窗口右端 {}：", step.to);
            let line = service.wait_for_log_line(&needle, Duration::from_secs(120), "race");
            eprintln!("  [race] {line}");
            eprintln!(
                "  [race] 批次 {} 已冻结、正在等，现在写入 {}（先页后头）",
                step.to, race.next
            );
            replay_step(race.next, Some("pages"), false);
            replay_step(race.next, Some("header"), false);
            let what = format!("step → {} 被 {} 追上", step.to, race.next);
            service.wait_in_sync(case.dbnum, race.next, Duration::from_secs(240), &what);
            assert_step(&mut service, case, step, &what);
            stash_after(&service, &mut stash, race.next, step, &what);
            continue;
        }
        let what = if step.rollback {
            format!("step 回退 → {}", step.to)
        } else {
            format!("step → {}", step.to)
        };
        let timeout = Duration::from_secs(step.timeout_secs.unwrap_or(180));
        if let Some(stay) = step.expect_model_sesno_stay {
            // 预期失败的那一窗不能等三水位（它们不该追上来）：等任务落终态，然后只判 model_sesno 没被推。
            let deadline = Instant::now() + timeout;
            let task = loop {
                service.assert_alive(&what);
                if let Some(task) = service.latest_terminal_task(case.dbnum, step.to) {
                    break task;
                }
                assert!(
                    Instant::now() < deadline,
                    "[{what}] {timeout:?} 内 dbnum {} 右端 {} 的任务没有落终态；日志 {}",
                    case.dbnum,
                    step.to,
                    service.stdout_path.display()
                );
                std::thread::sleep(Duration::from_millis(500));
            };
            let task_id = task.get("task_id").and_then(Value::as_str).unwrap_or("");
            let task_state = task.get("state").and_then(Value::as_str).unwrap_or("");
            let row = service
                .dbnum_row(case.dbnum)
                .unwrap_or_else(|| panic!("[{what}] /dbnums 里没有 dbnum {} 这一行", case.dbnum));
            let get = |key: &str| row.get(key).and_then(Value::as_i64);
            let (applied, latest, model) = (
                get("applied_sesno"),
                get("file_latest_sesno"),
                get("model_sesno"),
            );
            assert!(
                model == Some(stay as i64),
                "[{what}] 生成失败不该推模型水位：model_sesno={model:?}，期望仍停在 {stay}（applied={applied:?} file_latest={latest:?}）；任务 {task_id}",
            );
            eprintln!(
                "  [{what}] 任务 {task_id} 落终态 {task_state}，model_sesno 停在 {stay}（applied={applied:?} file_latest={latest:?}）"
            );
            assert_step(&mut service, case, step, &what);
            continue;
        }
        if step.expect_batch_parked {
            let entry = service.wait_batch_parked(case.dbnum, step.to, timeout, &what);
            let streak = entry.get("streak").and_then(Value::as_u64).unwrap_or(0);
            let line = service.wait_for_log_line("本轮不自动重跑", timeout, &what);
            assert!(
                line.contains(&format!("dbnum={}", case.dbnum)),
                "[{what}] 停跑日志不是这个库的：{line}"
            );
            let again = service
                .batch_failure(case.dbnum)
                .unwrap_or_else(|| panic!("[{what}] 重扫已经声明不再自动重跑，连败账却消失了"));
            let streak_after = again.get("streak").and_then(Value::as_u64).unwrap_or(0);
            assert_eq!(
                streak_after, streak,
                "[{what}] 触顶后重扫仍把这个库再跑了一次：streak {streak} → {streak_after}"
            );
            let row = service
                .dbnum_row(case.dbnum)
                .unwrap_or_else(|| panic!("[{what}] /dbnums 里没有 dbnum {} 这一行", case.dbnum));
            let applied = row.get("applied_sesno").and_then(Value::as_i64);
            assert!(
                applied.is_some_and(|applied| applied < step.to as i64),
                "[{what}] 数据批次失败不该推进 applied_sesno：applied={applied:?}，窗口右端 {}",
                step.to
            );
            eprintln!("  [{what}] 重扫已跳过（{line}），streak 仍是 {streak}，applied={applied:?}");
            continue;
        }
        service.wait_in_sync(case.dbnum, step.to, timeout, &what);
        if step.expect_batch_failure_cleared {
            let left = service.batch_failure(case.dbnum);
            assert!(
                left.is_none(),
                "[{what}] 右端前进后 dbnum {} 的连败账还在：{left:?}",
                case.dbnum
            );
            eprintln!("  [{what}] 连败账已清");
        }
        // 先对拍再断言：对拍要看的是这一窗（如整库重建）落库的行，不是 `generate_prime` 补常驻之后的投影。
        if let Some(at) = step.records_equal_to_step {
            compare_with_stash(&service, &stash, at, step.records_equal_tolerance, &what);
        }
        assert_step(&mut service, case, step, &what);
        if !step.companion_tasks.is_empty() {
            assert_companion_tasks(
                &mut service,
                &step.companion_tasks,
                &tasks_before,
                &stash,
                &what,
            );
        }
        if !step.room_edges.is_empty() {
            assert_room_edges(&service, step, &what);
        }
        stash_after(&service, &mut stash, step.to, step, &what);
        if let Some(restart) = &step.restart {
            restart_parity(
                &mut service,
                case,
                step.to,
                restart,
                &format!("{what} 重启"),
            );
        }
        if step.history_root.is_some() {
            assert_task_history(&service, case, step, &what);
        }
    }
    // RI-12 与刚体前移对拍都要打一枪整库全量重建，放在全部步骤之后：它会把这几个根重算一遍，中途打会把后面的窗搅乱。
    if !case.rebuild_edges_equal.is_empty() || !case.rebuild_records_equal.is_empty() {
        assert_rebuild_parity(&mut service, case, "rebuild 对拍");
    }
    // 人工执行会扫全项目并可能把非 DESI 库入队，放在全部步骤之后，避开正在跑的窗。
    if case.probe_execute {
        assert_operator_surface(&service, case);
    }
    // 预算门要在服务进程退出、stdout 句柄关上之后采日志（和事后手工 collect 同一时刻）。
    if case.budget_gate {
        check_increment_budget(&mut service, &run_dir, &case.name);
    }
    // 服务随 Drop 按句柄杀掉；工作文件留在终态，下一条用例 init 前删掉重写（`clear_for_replay_init`）。
    drop(service);
}

/// init 前把影子根里 `relative` 这个路径上的东西删掉（不碰原件），回完整路径。
///
/// `replay-init` 对已有文件是就地覆写；影子根里凡是没列进 `-Replayed` 的库都是到真工程原件的硬链接，覆写会写穿过去。
/// 先删掉这一个链接，让 `replay-init` 写出独立的新文件（服务此时还没起、surreal 每条用例新建，没有要保的 `StableFileId`）。
/// `db_session_fixture` 自己也拒绝覆写多链接文件（TH-1），这里是第一道，那里是兑底。
///
/// 影子根里别的项目目录都是 junction：从工程根到文件的每一级目录只要有一级是重解析点（junction / 符号链接），
/// 写进去就是写真工程，删链接也救不了——直接拒绝，让人把这个库列进影子根的 `-Replayed`。
fn clear_for_replay_init(project_root: &Path, relative: &str) -> PathBuf {
    let path = project_root.join(relative);
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        let mut dir = project_root.to_path_buf();
        let parents = Path::new(relative)
            .parent()
            .into_iter()
            .flat_map(|p| p.components());
        for component in parents {
            dir.push(component);
            if let Ok(meta) = fs::symlink_metadata(&dir) {
                assert!(
                    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                    "{} 是 junction / 符号链接，往 {} 做 replay-init 等于写真工程；把这个库列进影子根的 -Replayed（scripts/replay/New-ShadowProjectRoot.ps1）",
                    dir.display(),
                    path.display()
                );
            }
        }
    }
    if path.exists() {
        fs::remove_file(&path).unwrap_or_else(|e| panic!("删 {}: {e}", path.display()));
    }
    path
}

#[test]
fn replay_cases_land_in_sync_and_match_their_expectations() {
    let env = match environment() {
        Ok(env) => env,
        Err(reason) => {
            eprintln!("skip increment_replay: {reason}");
            return;
        }
    };
    let mut cases = load_cases();
    assert!(!cases.is_empty(), "tests/increment_replay/cases 下没有用例");
    // 开发时只跑一条：`AIOS_REPLAY_CASE=<name>`（用例文件名去掉 .json）。
    if let Ok(only) = std::env::var("AIOS_REPLAY_CASE") {
        cases.retain(|case| case.name == only);
        assert!(!cases.is_empty(), "AIOS_REPLAY_CASE={only} 不对应任何用例");
    }
    for case in &cases {
        run_case(&env, case);
    }
}

/// init 前的清场只拆影子根里那一个路径：硬链接被删、原件字节不动、路径不在也不报错。不要回放环境。
#[test]
fn clearing_for_replay_init_detaches_the_link_without_touching_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original");
    fs::write(&original, b"the real project file").unwrap();
    let root = dir.path().join("shadow");
    fs::create_dir_all(root.join("AvevaMarineSample/ams000")).unwrap();
    let relative = "AvevaMarineSample/ams000/ams8000_0001";
    fs::hard_link(&original, root.join(relative)).unwrap();

    let cleared = clear_for_replay_init(&root, relative);
    assert_eq!(cleared, root.join(relative));
    assert!(!cleared.exists(), "链接该被删掉，给 replay-init 写独立文件");
    assert_eq!(fs::read(&original).unwrap(), b"the real project file");
    // 再来一次（路径已经不在）也不报错。
    assert_eq!(clear_for_replay_init(&root, relative), cleared);
}

/// 用例里拼错的、还没实现的键报得出来（带路径），注释键、别名、映射里的数据键不误报。不要回放环境。
#[test]
fn unknown_case_keys_are_reported_with_their_path() {
    let text = serde_json::json!({
        "name": "typo",
        "_doc": "注释不算",
        "dbnum": 8000,
        "db_file": "AvevaMarineSample/ams000/ams8000_0001",
        "start": 1,
        "fixure_dir": "拼错了",
        "init": { "to": 1, "unit_coun": 0 },
        "steps": [
            { "to": 2, "_leg": "注释", "unit_kinds": { "24384/1": "regen" }, "warnings_not_contain": ["x"] },
            { "to": 3, "★companion_task": { "dbnum": 250104 }, "plan_totals": { "regen": 1 },
              "datacenter_deletes": [{ "refno": "24384/2", "reasn": "deleted" }] }
        ]
    });
    let case: Case = serde_json::from_value(text.clone()).expect("多出来的键不挡解析");
    let known = serde_json::to_value(&case).unwrap();
    let mut unknown = Vec::new();
    unknown_keys(&text, &known, "", &mut unknown);
    unknown.sort();
    assert_eq!(
        unknown,
        vec![
            "fixure_dir".to_string(),
            "init.unit_coun".to_string(),
            "steps[1].datacenter_deletes[0].reasn".to_string(),
            "steps[1].★companion_task".to_string(),
        ]
    );
}

/// 派生配置跟着用例的工程走：不写工程，除了那六个键一个字不动；写了工程，工程名、工程码、库的命名空间、
/// MDB、元件库次序换成它的，退役的 `included_projects` 和注释掉的行不动。不要回放环境。
#[test]
fn derived_config_follows_the_case_project() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.toml");
    let text = "project_path = \"D:/formal\"\nincluded_projects = []\n\
                catalogue_project_priority = [\"AvevaMarineSample\", \"AvevaCatalogue\", \"ZDJ\"]\n\
                mdb_name = \"ALL\"\nproject_name = \"AvevaMarineSample\"\nproject_code = 1516\n\
                surreal_ns = 1516\n# project_name = \"commented\"\nhttp_api_addr = \"0.0.0.0:1\"\n\
                watch_dbnums = [1]\nauto_initialize_new_dbnums = false\nv_ip = \"10.0.0.1\"\nv_port = 1\n";
    fs::write(&base, text).unwrap();
    let root = dir.path().join("shadow");

    let plain = dir.path().join("plain.toml");
    derive_config(
        &base,
        &plain,
        &root,
        4321,
        5432,
        &[8000],
        None,
        &BTreeMap::new(),
    );
    let plain = fs::read_to_string(&plain).unwrap();
    for line in [
        "project_name = \"AvevaMarineSample\"",
        "project_code = 1516",
        "surreal_ns = 1516",
        "mdb_name = \"ALL\"",
        "catalogue_project_priority = [\"AvevaMarineSample\", \"AvevaCatalogue\", \"ZDJ\"]",
        "http_api_addr = \"127.0.0.1:4321\"",
        "watch_dbnums = [8000]",
        "auto_initialize_new_dbnums = true",
        "v_ip = \"127.0.0.1\"",
        "v_port = 5432",
    ] {
        assert!(
            plain.lines().any(|have| have == line),
            "缺 {line}：\n{plain}"
        );
    }

    let spec: Case = serde_json::from_value(serde_json::json!({
        "name": "aps-shape",
        "dbnum": 7166,
        "db_file": "AvevaPlantSample/aps000/aps7166_0001",
        "project": { "name": "AvevaPlantSample", "code": 2501, "mdb": "SDNF" },
        "start": 7,
        "steps": []
    }))
    .expect("用例解得开");
    let other = dir.path().join("other.toml");
    derive_config(
        &base,
        &other,
        &root,
        4321,
        5432,
        &[7166],
        spec.project.as_ref(),
        &BTreeMap::from([("data_tx_max_write_rows".to_string(), Value::from(50000))]),
    );
    let other = fs::read_to_string(&other).unwrap();
    assert!(
        other
            .lines()
            .any(|have| have == "data_tx_max_write_rows = 50000"),
        "用例点名的事务上限要进派生配置：\n{other}"
    );
    for line in [
        "project_name = \"AvevaPlantSample\"",
        "project_code = 2501",
        "surreal_ns = 2501",
        "mdb_name = \"SDNF\"",
        "catalogue_project_priority = [\"AvevaPlantSample\", \"AvevaCatalogue\"]",
        "included_projects = []",
        "# project_name = \"commented\"",
        "watch_dbnums = [7166]",
    ] {
        assert!(
            other.lines().any(|have| have == line),
            "缺 {line}：\n{other}"
        );
    }
    assert!(!other.contains("AvevaMarineSample"), "{other}");
}

/// `records_differ` 把 `q` 与 `-q` 当同一个旋转（刚体前移对拍：便宜路与整根重算各自分解矩阵），
/// 真不同的旋转与平移照样报出来。不要回放环境。
/// 没有伙伴库的用例（现有八条）形状与加伙伴库之前完全一样：监听名单仍是「写了的名单或主库」，
/// 步上不出现伙伴库字段。
#[test]
fn cases_without_companions_keep_the_single_dbnum_shape() {
    for case in load_cases() {
        if !case.companions.is_empty() {
            continue;
        }
        let before = if case.watch_dbnums.is_empty() {
            vec![case.dbnum]
        } else {
            case.watch_dbnums.clone()
        };
        assert_eq!(watch_list(&case), before, "{}", case.name);
        assert!(
            case.init.iter().chain(&case.steps).all(|step| {
                step.companion_to.is_empty() && step.expect_in_sync.is_empty() && !step.hold_primary
            }),
            "{} 没有伙伴库却用了伙伴库字段",
            case.name
        );
    }
}

#[test]
fn records_compare_rotations_up_to_sign() {
    let record = |rotation: [f64; 4], x: f64| {
        serde_json::json!({
            "refno": "24384_24776",
            "world_trans": { "translation": [x, 0.0, 0.0], "rotation": rotation, "scale": [1.0, 1.0, 1.0] },
        })
    };
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let q = [0.0, 0.0, half, half];
    let minus_q = [0.0, 0.0, -half, -half];
    assert!(records_differ(&[record(q, 10.0)], &[record(minus_q, 10.0)], 1e-6).is_none());
    assert!(
        records_differ(
            &[record([0.0, 0.0, 1.0, 0.0], 10.0)],
            &[record([0.0, 0.0, -1.0, 0.0], 10.0)],
            1e-6
        )
        .is_none()
    );
    // 半圈附近 w 只剩浮点噪声、两边噪声的符号还相反：`Z is D` 的 EQUI 绕 U 转 90° 后就是这样
    // （ams8000-equi-rotate-chain 的 736：转后 Y is W 5 S and Z is D，q ≈ ±(0.737, -0.676, 0, 0)）。
    // 按分量给单个四元数定符号会在这里误报。
    assert!(
        records_differ(
            &[record([0.737, -0.676, 0.0, 3e-7], 10.0)],
            &[record([-0.737, 0.676, 0.0, 2e-7], 10.0)],
            1e-6
        )
        .is_none()
    );
    assert!(
        records_differ(
            &[record(q, 10.0)],
            &[record([0.0, 0.0, 0.0, 1.0], 10.0)],
            1e-6
        )
        .is_some()
    );
    assert!(records_differ(&[record(q, 10.0)], &[record(minus_q, 11.0)], 1e-6).is_some());
}

/// 被测服务的 CWD 必须是运行目录，不能是仓根：服务按进程 CWD 取单实例锁（`.gen-model.instance.lock`，Windows 上
/// deny-share 打开），仓根那把锁常被本机常驻开发服务（监听 0.0.0.0:8022）持有，每条用例都会在 `stage=instance-lock`
/// 退出 1；CWD 在仓根还会把 `<stamp>_dblog.txt` / `accel_tree_*.snapshot` / `accel_tree.bin` 写进仓根，污染检出。
/// 而 `aios_core` 连库时按 CWD 相对路径 `resource/surreal` 装函数脚本，目录不在就 `Failed to define common functions …
/// (os error 3)`，所以 `Service::start` 必须先把仓根的脚本拷进运行目录再 spawn。首次启动与冷重启共用 `spawn_server`，
/// 两条路一起钉住。不要回放环境。
#[test]
fn the_service_under_test_runs_from_the_run_directory_with_staged_surql_scripts() {
    let source = include_str!("harness.rs");
    let spawn = source
        .split_once("fn spawn_server(")
        .expect("spawn_server 还在")
        .1
        .split_once("\nimpl Service {")
        .expect("spawn_server 后面紧跟 impl Service")
        .0;
    assert!(
        spawn.contains(".current_dir(run_dir)"),
        "被测服务的 CWD 要是运行目录:\n{spawn}"
    );
    assert!(
        !spawn.contains(".current_dir(repo())"),
        "被测服务不许拿仓根当 CWD（与常驻服务抢单实例锁、往仓根写 dblog / accel_tree）:\n{spawn}"
    );
    // 两段定位、不写死可见性：先切到 `impl Service {`，再在其后找 `fn start(`（加不加 pub 都行）；
    // 收口锚点 `fn restart_with_fail_roots(` 同样不带缩进与 pub。
    let start = source
        .split_once("impl Service {")
        .and_then(|(_, rest)| rest.split_once("fn start("))
        .expect("Service::start 还在")
        .1
        .split_once("fn restart_with_fail_roots(")
        .expect("start 后面紧跟 restart_with_fail_roots")
        .0;
    let stage = start
        .find("stage_surreal_scripts(")
        .expect("start 要先把 surql 脚本拷进运行目录");
    let spawn_call = start.find("spawn_server(").expect("start 调 spawn_server");
    assert!(
        stage < spawn_call,
        "脚本要在 spawn 之前就位，不然服务连库就失败:\n{start}"
    );
}

/// 窗口规划那一行拆成数：会话号每次录都不同，回放用例按字段比（`Step::plan_totals`）。
#[test]
fn plan_totals_line_parses_into_numbers() {
    let line = "window 1262→1264: candidates=3 (+1 ~2 -0) rolled_up=0 unchanged=1 no_model=1 \
                (metadata_only=1 unread_attribute=0 cache_stamp=0) unresolved=0 cascades=1 units regen=2 \
                remove=0 derived=0";
    let totals = parse_plan_totals(line).expect("是规划那一行");
    let get = |key: &str| totals.get(key).copied();
    assert_eq!(get("candidates"), Some(3));
    assert_eq!(
        (get("added"), get("modified"), get("deleted")),
        (Some(1), Some(2), Some(0))
    );
    assert_eq!(get("metadata_only"), Some(1));
    assert_eq!(get("cascades"), Some(1));
    assert_eq!(
        (get("regen"), get("remove"), get("derived")),
        (Some(2), Some(0), Some(0))
    );
    assert!(parse_plan_totals("别的告警，不是规划那一行").is_none());

    // 任务回执里的原样：前面拼着根一级的账，两半都有 `regen`，各记各的。
    let receipt = "dbnum=8000: dbnum=8000: window 1543→1544 roots_T=1024 roots_S=1024 touched=1 regen=1 \
                   delete=0 advance=1023 lazy=0 | window 1543→1544: candidates=2 (+0 ~1 -1) rolled_up=1 \
                   unchanged=0 no_model=1 (metadata_only=0 unread_attribute=0 cache_stamp=0) unresolved=0 \
                   cascades=0 units regen=0 remove=1 derived=0";
    let totals = parse_plan_totals(receipt).expect("是规划那一行");
    let get = |key: &str| totals.get(key).copied();
    assert_eq!((get("regen"), get("remove")), (Some(0), Some(1)));
    assert_eq!(
        (
            get("root_regen"),
            get("root_delete"),
            get("root_advance"),
            get("root_lazy")
        ),
        (Some(1), Some(0), Some(1023), Some(0))
    );
    assert_eq!((get("roots_T"), get("touched")), (Some(1024), Some(1)));
    assert_eq!((get("candidates"), get("deleted")), (Some(2), Some(1)));
    assert_eq!(get("dbnum"), None);
}

/// 新录的链用的那几格解得开：本机终稿 `source_file`、别的工程 `project`、步一级的 `seed`，以及落到件上的期望
/// （不在单元里的根、warnings、规划各项、几何计数与样本、交付删除记录）。
#[test]
fn cases_accept_a_local_final_file_and_unit_level_expectations() {
    let case: Case = serde_json::from_value(serde_json::json!({
        "name": "shape-only",
        "dbnum": 7998,
        "db_file": "AvevaMarineSample/ams000/ams7998_0001",
        "source_file": "output/increment-suite/finals/ams7998_0001",
        "project": { "name": "AvevaPlantSample", "code": 2501 },
        "start": 10,
        "steps": [{
            "to": 11,
            "seed": ["RETURN 1"],
            "units_absent": ["24382/9"],
            "warnings_contain": ["window"],
            "warnings_absent": ["not handed off"],
            "plan_totals": { "regen": 1, "cascades": 0 },
            "geometry_counts": { "24382/5": { "added": 1, "removed": 1 } },
            "geometry_sample": { "24382/5": ["24382/6", "24382_7"] },
            "companion_tasks": [
                { "dbnum": 8000, "intent_contains": "model", "units_exactly": ["24384/1"], "aabb_min_shift": { "24384/1": [0.0, 0.0, 25.4] } },
                { "dbnum": 8000, "absent": true }
            ],
            "datacenter_deletes": [
                { "refno": "24382/7", "reason": "deleted", "sesno": 11 },
                { "refno": "24382/8", "present": false }
            ]
        }]
    }))
    .expect("用例解得开");
    assert_eq!(
        case.source_file.as_deref(),
        Some("output/increment-suite/finals/ams7998_0001")
    );
    let project = case.project.as_ref().expect("写了工程");
    assert_eq!(
        (project.name.as_str(), project.code, project.mdb.as_str()),
        ("AvevaPlantSample", 2501, "ALL")
    );
    assert!(project.catalogue_project_priority.is_empty());
    let step = &case.steps[0];
    assert_eq!(step.geometry_sample["24382/5"].len(), 2);
    assert_eq!(step.companion_tasks.len(), 2);
    assert_eq!(
        step.companion_tasks[0].aabb_min_shift["24384/1"],
        [0.0, 0.0, 25.4]
    );
    assert!(step.companion_tasks[1].absent && !step.companion_tasks[0].absent);
    assert_eq!(step.seed.len(), 1);
    assert_eq!(step.plan_totals.get("regen"), Some(&1));
    assert_eq!(step.geometry_counts["24382/5"].added, Some(1));
    assert_eq!(step.geometry_counts["24382/5"].moved, None);
    assert!(step.datacenter_deletes[0].present);
    assert!(!step.datacenter_deletes[1].present);
    assert_eq!(step.datacenter_deletes[0].sesno, Some(11));
}
