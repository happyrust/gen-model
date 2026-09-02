//! 失败数据批次的落盘记录（每日 JSONL）。
//!
//! 控制台上的「[增量] 失败原因」那一行解决的是「人正站在机器前」的情形。真实现场
//! 有三个洞它补不上：
//!
//! - cmd.exe 的回滚缓冲有限，一轮冷启动的阶段日志就能把几小时前那句冲掉；
//! - 任务注册表是**进程内**的（`MAX_TASKS`，重启即清空），所以 `/api/v1/tasks`
//!   上那句权威原话活不过一次重启——而人发现问题时往往已经重启过了；
//! - `enable_log` 默认关，且它接的是 `log::` 宏，`println!` 一个字都进不去。
//!
//! 2026-08-27 的 SYST 8191 三个洞全踩了：屏幕上只剩一张照片，回执取不到，磁盘上
//! 没有任何一个文件写着「为什么」。所以失败批次必须自己往 `logs/` 落一条，形制与
//! [`super::queue_stall_diagnostics`] 的停滞记录一致（同一个目录、同样的每日切分、
//! 同样一行一条可重开的 JSON），异机复核直接拷走目录即可。
//!
//! 与相邻两本账的分界：
//!
//! | | 记什么 | 活多久 |
//! |---|---|---|
//! | `/health` 的 `batch_failures` | 连败次数与最近一句 `warnings.last()` | 进程内 |
//! | `logs/queue-stalls-*.jsonl` | 队列**姿态**（谁在等、被哪道门挡着） | 落盘 |
//! | 本模块 | 批次**为什么**失败：原话、出处、死在哪一步、分步账、全部告警 | 落盘 |
//!
//! 三者互不替代：停滞记录说得出 30999 被 meta 焊住，但说不出 8191 为什么失败。

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde_json::{Value, json};

use super::task_registry::TaskStep;

/// 落盘目录。与停滞看门狗同一个，相对进程工作目录——现场拷走 `logs/` 就是全部证据。
const DIRECTORY: &str = "logs";
const FILE_PREFIX: &str = "batch-failures-";
const FILE_SUFFIX: &str = ".jsonl";

/// 回读时最多翻几天的文件。失败是低频事件，一周足够覆盖「周五出事周一来看」。
const MAX_FILES_SCANNED: usize = 7;

/// 一次失败批次的全部可记事实。
///
/// 借用而不是拥有：调用点在 `run_one_batch` 的终态段上，那里每一样都还活着，
/// 复制一遍只是为了塞进结构体。
pub struct BatchFailure<'a> {
    pub task_id: &'a str,
    pub project: &'a str,
    pub dbnum: u32,
    pub db_type: &'a str,
    pub phase: &'a str,
    pub epoch_id: u64,
    /// 任务终态标签（`failed` / `partial`）。
    pub state: &'a str,
    pub window: (i32, i32),
    /// 窗口右端那次保存在 E3D 里的时刻。回答「这批对应我哪次 SAVEWORK」。
    pub save_time: Option<&'a str>,
    pub file_path: Option<&'a str>,
    /// 死在哪一步（`TaskEntry::current_stage`）。收集口有十几个各自具名的硬失败
    /// 出口，原话分不出它发生在收集还是写回，这一格分得出。
    pub died_at: Option<&'a str>,
    pub reason: &'a str,
    pub reason_from: &'a str,
    /// **不截断**。控制台那份只留前三条是怕冲掉阶段行，文件没有这个约束，而
    /// 净窗口的口径标注恰恰常常是判断收集口径的唯一依据。
    pub warnings: &'a [String],
    /// 同右端连败第几次；数据窗口收口了（失败只在模型侧）时为 `None`。
    pub streak: Option<u32>,
    pub elapsed_ms: u128,
    /// 落记录这一刻，该 dbnum 名下还挂着的暂存窗口（`staging::lifecycle::
    /// resource_snapshots_for` 的原样输出，与 `/health` 的 `staging_windows` 同形）。
    ///
    /// 语义是「失败之后窗口回滚了还是残留着」：空数组 = 记录时已无窗口（回滚干净，
    /// 或这一批走直写根本没建窗口）；非空 = 残留，重跑多半撞同一堵墙。面板那张
    /// 「暂存窗口」卡活在进程内、重启即清空，而人来问的时候往往已经重启过了——
    /// 这一格是它落盘的那一半（ISSUE-025 §四 4a）。
    pub staging: &'a [Value],
    /// 分步账（ISSUE-025 §一）：任务经过的每个阶段各一行 `{name, at, ms, ok}`，
    /// `TaskRegistry::finish` 已结算完最后一步。`died_at` 只答得出「死在哪」，
    /// 这本账答的是「怎么走到那儿的、哪一步慢」——「死在收集」与「收集正常、
    /// 跑了 40 分钟之后写回才死」在这一格上终于长得不一样。
    ///
    /// 空数组 = 这次执行从没报过阶段（死在建行与第一个阶段之间）；字段缺席 =
    /// 老版本记录，答不了这一问。与 `staging` 那格同一条口径。
    pub steps: &'a [TaskStep],
    /// 被环形上限（32 步）挤掉的更早步数；0 = 账是全的。挤掉了多少必须随记录
    /// 说出来，读的人才知道自己看的是尾巴还是全程。
    pub steps_dropped: u64,
}

/// 「这个库从此不再自动重跑」——park 生效的那一刻。
///
/// 它与失败记录是两件事，所以是两个 `event`：失败记录回答「这一批为什么没成」，
/// park 记录回答「引擎从现在起不打算再试了」。后者才是真正需要人介入的那条线，
/// 而在它之前，那个库看起来只是「又失败了一次」。
///
/// 每次 park 只落一条：重扫每个对账周期都会再问一次，答案在解除之前恒为是
/// （去重靠 `batch_worker` 那边的 `first_notice`）。
pub struct BatchPark<'a> {
    pub dbnum: u32,
    pub db_type: &'a str,
    pub project: &'a str,
    pub phase: &'a str,
    pub streak: u32,
    pub max_attempts: u32,
    /// 连败期间观察到的窗口右端。它不前进，就说明没人往这个库里存新会话。
    pub end_sesno: i32,
    pub file_path: Option<&'a str>,
    /// 账本里记的最近一句失败原因（`warnings.last()` 口径，权威那句在同 dbnum
    /// 的 `batch_failure` 记录里）。
    pub last_reason: &'a str,
    pub first_at: &'a str,
    pub last_at: &'a str,
}

/// 落一条 park 记录。
pub fn record_park(park: &BatchPark<'_>) {
    let now = Local::now();
    let path = daily_path(Path::new(DIRECTORY), now);
    let line = to_park_record(park, now);
    match append_json_line(&path, &line) {
        Ok(()) => println!(
            "[增量] dbnum={} 已 park（连败 {}/{}，右端 {} 未前进）：重扫从此不再自动重跑它，\
             已落盘 {}",
            park.dbnum,
            park.streak,
            park.max_attempts,
            park.end_sesno,
            path.display()
        ),
        Err(error) => eprintln!(
            "[增量] dbnum={} park 记录落盘失败 {}: {error}",
            park.dbnum,
            path.display()
        ),
    }
}

fn to_park_record(park: &BatchPark<'_>, now: DateTime<Local>) -> Value {
    json!({
        "event": "batch_park",
        "at": now.to_rfc3339(),
        "dbnum": park.dbnum,
        "db_type": park.db_type,
        "project": park.project,
        "phase": park.phase,
        "streak": park.streak,
        "max_attempts": park.max_attempts,
        "end_sesno": park.end_sesno,
        "file_path": park.file_path,
        "last_reason": park.last_reason,
        "first_at": park.first_at,
        "last_at": park.last_at,
        // 解除路径写进记录本身，而不是只留在文档里：读这个文件的人多半是被叫来
        // 救火的，他手上只有这一个文件。三条路清的都是**重试账**，不是病因——
        // 确定性失败解开之后照样会再走一遍这五次。
        "auto_retry": false,
        "unblock": [
            "文件长出新会话（右端 sesno 前进）",
            "POST /api/v1/update/execute（显式重试，立即清零）",
            "重启进程（账本在进程内）",
        ],
        "unblock_note": "三条路清的是重试账不是病因；先修 batch_failure 记录里那句 reason，否则解开后仍会再 park 一次。",
    })
}

/// 落一条失败记录，并在控制台指出它落在哪儿。
///
/// 写失败只报警不上抛：这条记录是排查用的旁路，不能因为磁盘满就把批次终态
/// 处理搅黄——那会把一个可查的失败变成一个不可查的失败。
pub fn record(failure: &BatchFailure<'_>) {
    let now = Local::now();
    let path = daily_path(Path::new(DIRECTORY), now);
    let line = to_record(failure, now);
    match append_json_line(&path, &line) {
        Ok(()) => println!(
            "[增量] 失败原因 已落盘 {}（重启不丢；网页「错误日志」卡读 /api/v1/error-log）",
            path.display()
        ),
        Err(error) => eprintln!(
            "[增量] 失败原因 落盘失败 {}: {error}——本次原因只剩控制台这一份",
            path.display()
        ),
    }
}

fn to_record(failure: &BatchFailure<'_>, now: DateTime<Local>) -> Value {
    // 这一条是不是压垮它的那一次。翻文件的人第一个要分的就是「还会自己再试」与
    // 「从此不动了」——只给一个 streak 数字，等于要他自己去记 MAX_ATTEMPTS 是几。
    let max_attempts = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
    let parked = failure.streak.is_some_and(|streak| streak >= max_attempts);
    json!({
        "event": "batch_failure",
        "at": now.to_rfc3339(),
        "task_id": failure.task_id,
        "project": failure.project,
        "dbnum": failure.dbnum,
        "db_type": failure.db_type,
        "phase": failure.phase,
        "epoch_id": failure.epoch_id,
        "state": failure.state,
        "start_sesno": failure.window.0,
        "end_sesno": failure.window.1,
        "save_time": failure.save_time,
        "file_path": failure.file_path,
        "died_at": failure.died_at,
        "reason": failure.reason,
        "reason_from": failure.reason_from,
        "warnings": failure.warnings,
        "streak": failure.streak,
        "max_attempts": max_attempts,
        "parked": parked,
        "auto_retry_left": failure
            .streak
            .map(|streak| max_attempts.saturating_sub(streak)),
        "elapsed_ms": failure.elapsed_ms,
        "staging": failure.staging,
        "steps": failure.steps,
        "steps_dropped": failure.steps_dropped,
    })
}

fn daily_path(directory: &Path, now: DateTime<Local>) -> PathBuf {
    directory.join(format!(
        "{FILE_PREFIX}{}{FILE_SUFFIX}",
        now.format("%Y-%m-%d")
    ))
}

fn append_json_line(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{record}")?;
    file.flush()
}

/// 落盘错误事件的全部来源。
///
/// 面板上那张卡问的是「这台机器上出过什么错」，而这个问题的答案分散在两个文件族里：
/// 批次失败与 park 在本模块写的那份，队列停滞在看门狗写的那份。让人自己去开两个
/// 文件、再按时刻在脑子里并成一条线，正是排查最容易漏掉半边的地方——8191 失败与
/// 30999 停滞就是同一件事的两端。
const SOURCES: [&str; 2] = [
    FILE_PREFIX,
    crate::data_interface::queue_stall_diagnostics::FILE_PREFIX,
];

/// 落盘错误事件的读取口：两个文件族并成一条时间线，最新在前。
///
/// 从磁盘读而不是从内存读，正是这个模块存在的理由：面板要能在服务重启之后仍然
/// 说得出上一次为什么失败。
///
/// `kinds` 是要的 `event` 值，**空 = 全要**。做成集合而不是单值，是因为「批次失败
/// 与 park」是一个天然的组合（`/api/v1/batch-failures` 要的正是这两种）——只给单值
/// 的话调用方得读两遍再自己按时刻并一次，那份归并逻辑就有了第二个副本。
/// `dbnum` 为 `None` 时不筛。
///
/// `project`：这本账落在**进程工作目录**下，跨配置改动活着——同一个目录先后跑过
/// 两个 `project_name` 时，`dbnum=8191` 会把上一份配置留下的记录一并端出来（库号
/// 空间只在项目内唯一，ISSUE-025 §五 5a）。`Some(名字)` 只留该项目；`None` 不筛。
/// **没有 `project` 字段的行不筛掉**：`queue_stall` 记录与老版本记录本来就不带这一
/// 格，按项目筛把它们静默滤没，「这台机器没停滞过」就成了筛出来的假话。
pub fn recent(kinds: &[&str], project: Option<&str>, dbnum: Option<u32>, limit: usize) -> Value {
    read_recent(Path::new(DIRECTORY), kinds, project, dbnum, limit)
}

fn read_recent(
    directory: &Path,
    kinds: &[&str],
    project: Option<&str>,
    dbnum: Option<u32>,
    limit: usize,
) -> Value {
    let sources = SOURCES
        .iter()
        .map(|prefix| {
            directory
                .join(format!("{prefix}*{FILE_SUFFIX}"))
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();
    let mut files = match daily_files(directory) {
        Ok(files) => files,
        Err(error) => {
            return json!({
                "records": [],
                "sources": sources,
                // 「读不到」与「没有出过错」必须分得清：后者是好消息，前者是这一页
                // 正在瞎猜。目录不存在也算读不到——它只在第一次落盘时才被创建。
                "error": format!("{error}"),
            });
        }
    };
    // 文件名带日期，字典序即时间序。每族只翻最近几天，坏库连着几周的记录不该让
    // 一次面板刷新去读整个目录。
    files.sort();
    files.reverse();
    files.truncate(MAX_FILES_SCANNED * SOURCES.len());

    let mut records: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for path in files {
        match read_one(&path, kinds, project, dbnum) {
            Ok(mut day) => records.append(&mut day),
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    // 两个文件族各自有序，合起来不是——必须真排一次，否则「队列停滞」会整段压在
    // 「批次失败」上面或下面，而这条时间线的全部价值就在于两者交替出现的次序。
    records.sort_by(|left, right| at_of(right).cmp(at_of(left)));
    if limit > 0 {
        records.truncate(limit);
    }

    // 用了哪个筛子必须随回执一起回去（`null` = 没筛）：「这个库没失败过」与
    // 「被项目筛掉了」在一张空列表上长得一模一样，本文件通篇在防的就是这件事。
    let mut payload = json!({ "records": records, "sources": sources, "project_filter": project });
    if !errors.is_empty()
        && let Some(object) = payload.as_object_mut()
    {
        object.insert("error".into(), json!(errors.join("; ")));
    }
    payload
}

/// 排序键。RFC3339 在同一时区下字典序即时间序，而这些记录全由本进程用
/// `Local::now()` 写出，偏移一致。缺 `at` 的行排到最后而不是崩掉。
fn at_of(record: &Value) -> &str {
    record.get("at").and_then(Value::as_str).unwrap_or("")
}

fn daily_files(directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(FILE_SUFFIX) && SOURCES.iter().any(|prefix| name.starts_with(prefix)) {
            files.push(path);
        }
    }
    Ok(files)
}

/// 读一个文件里符合筛选的记录。
///
/// 认不出的行**跳过而不是整份放弃**：进程被强杀会留下半行，为了一行残句丢掉
/// 那一天全部证据是本末倒置。
fn read_one(
    path: &Path,
    kinds: &[&str],
    project: Option<&str>,
    dbnum: Option<u32>,
) -> std::io::Result<Vec<Value>> {
    let file = fs::File::open(path)?;
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let kind_matches = kinds.is_empty()
            || value
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| kinds.contains(&event));
        // 只筛「带 project 且对不上」的行：没有这一格的（queue_stall、老版本记录）
        // 归不了属，留下让人看见，比替它猜一个项目再滤掉诚实。
        let project_matches = project.is_none_or(|wanted| {
            value
                .get("project")
                .and_then(Value::as_str)
                .is_none_or(|recorded| recorded == wanted)
        });
        let dbnum_matches = dbnum.is_none_or(|wanted| {
            value.get("dbnum").and_then(Value::as_u64) == Some(u64::from(wanted))
        });
        if kind_matches && project_matches && dbnum_matches {
            records.push(value);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aios-batch-failure-{name}-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn failure<'a>(task_id: &'a str, dbnum: u32, reason: &'a str) -> BatchFailure<'a> {
        BatchFailure {
            task_id,
            project: "JEU",
            dbnum,
            db_type: "SYST",
            phase: "meta",
            epoch_id: 1,
            state: "failed",
            window: (36, 37),
            save_time: Some("2026-08-25T04:08:03+00:00"),
            file_path: Some(r"D:\release\9001\project\JEU\JEU000\jeusys"),
            died_at: Some("collect_window"),
            reason,
            reason_from: "result.batch.message",
            warnings: &[],
            streak: Some(1),
            elapsed_ms: 3449,
            staging: &[],
            steps: &[],
            steps_dropped: 0,
        }
    }

    /// 一条记录必须自带回答「哪个库、哪个窗口、死在哪一步、为什么」的全部四格。
    ///
    /// 少任何一格这份文件就退化成又一句「失败了」：8191 那次屏幕上有的正是前三格，
    /// 缺的就是第四格，而只有四格齐了才不用回头再问一次现场。
    #[test]
    fn one_record_answers_which_window_died_where_and_why() {
        let record = to_record(
            &failure("db-20260827-114844-000000", 8191, "读取增量数据失败: boom"),
            Local::now(),
        );
        assert_eq!(record["dbnum"], 8191);
        assert_eq!(record["start_sesno"], 36);
        assert_eq!(record["end_sesno"], 37);
        assert_eq!(record["died_at"], "collect_window");
        assert_eq!(record["reason"], "读取增量数据失败: boom");
        assert_eq!(record["reason_from"], "result.batch.message");
        assert_eq!(record["phase"], "meta");
        // 保存时刻回答「这批对应我哪一次 SAVEWORK」——它与挂钟 `at` 差了两天时，
        // 人才看得出这是冷启动重扫捡起来的旧窗口，而不是刚才那次保存。
        assert_eq!(record["save_time"], "2026-08-25T04:08:03+00:00");
    }

    /// 「还会自己再试」与「从此不动了」必须在记录里一眼分得开。
    ///
    /// 只给一个 `streak` 数字等于要读文件的人自己记住 `MAX_ATTEMPTS` 是几；而这两
    /// 种状态的处置完全不同——前者等下一个对账周期就行，后者不动手就永远停在那儿。
    #[test]
    fn a_record_says_whether_the_engine_will_still_retry_on_its_own() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;

        let mut third = failure("t", 8191, "boom");
        third.streak = Some(3);
        let record = to_record(&third, Local::now());
        assert_eq!(record["parked"], false);
        assert_eq!(record["max_attempts"], cap);
        assert_eq!(record["auto_retry_left"], cap - 3);

        let mut last = failure("t", 8191, "boom");
        last.streak = Some(cap);
        let record = to_record(&last, Local::now());
        assert_eq!(record["parked"], true, "压垮它的那一次要自己说出来");
        assert_eq!(record["auto_retry_left"], 0);

        // 数据窗口收口了、失败只在模型侧：没有连败账，也就谈不上 park。
        let mut model_side = failure("t", 8191, "boom");
        model_side.streak = None;
        let record = to_record(&model_side, Local::now());
        assert_eq!(record["parked"], false);
        assert!(
            record["auto_retry_left"].is_null(),
            "没有账就不该编一个数出来"
        );
    }

    /// park 记录是独立的一条 `event`，而且自带解除路径。
    ///
    /// 它与失败记录回答的不是同一个问题：失败记录说「这一批为什么没成」，park 记录
    /// 说「引擎从现在起不打算再试了」——后者才是唯一必须人介入的状态。读这个文件的
    /// 人多半是被叫来救火的，手上只有这一个文件，所以怎么解除得写在记录里。
    #[test]
    fn the_park_record_is_its_own_event_and_carries_the_way_out() {
        let record = to_park_record(
            &BatchPark {
                dbnum: 8191,
                db_type: "SYST",
                project: "JEU",
                phase: "meta",
                streak: 5,
                max_attempts: 5,
                end_sesno: 37,
                file_path: Some(r"D:\release\9001\project\JEU\JEU000\jeusys"),
                last_reason: "读取增量数据失败: boom",
                first_at: "2026-08-27T11:48:47+08:00",
                last_at: "2026-08-27T12:08:47+08:00",
            },
            Local::now(),
        );
        assert_eq!(record["event"], "batch_park");
        assert_eq!(record["dbnum"], 8191);
        assert_eq!(record["streak"], 5);
        assert_eq!(record["end_sesno"], 37, "右端不前进正是 park 成立的前提");
        assert_eq!(record["auto_retry"], false);
        assert_eq!(record["unblock"].as_array().map(Vec::len), Some(3));
        // 解除 ≠ 修好：三条路清的都是重试账。不写这句，park 一解人就以为好了。
        assert!(
            record["unblock_note"]
                .as_str()
                .is_some_and(|note| note.contains("不是病因")),
            "{record}"
        );
    }

    /// 控制台截断到三条，文件一条都不许少：净窗口的口径标注常常是判断收集口径的
    /// 唯一依据，而它恰好是最长、最先被截掉的那种。
    #[test]
    fn the_file_keeps_every_warning_the_console_had_to_drop() {
        let warnings = (0..9).map(|i| format!("w{i}")).collect::<Vec<_>>();
        let mut base = failure("t", 8191, "boom");
        base.warnings = &warnings;
        let record = to_record(&base, Local::now());
        assert_eq!(record["warnings"].as_array().map(Vec::len), Some(9));
    }

    /// 写进去的能原样读回来，且最新在前——面板第一屏要的就是最近那条。
    #[test]
    fn records_round_trip_newest_first() {
        let dir = fixture("round-trip");
        let base_at = Local::now();
        let path = daily_path(&dir, base_at);
        for (index, reason) in ["第一次", "第二次", "第三次"].iter().enumerate() {
            let mut one = failure("t", 8191, reason);
            one.streak = Some(index as u32 + 1);
            let at = base_at + chrono::Duration::seconds(index as i64);
            append_json_line(&path, &to_record(&one, at)).expect("append");
        }
        let last = base_at + chrono::Duration::seconds(9);
        append_json_line(&path, &to_record(&failure("o", 30999, "别的库"), last)).expect("append");

        let all = read_recent(&dir, &[], None, None, 0);
        assert_eq!(all["records"].as_array().map(Vec::len), Some(4));
        assert_eq!(all["records"][0]["dbnum"], 30999, "最新的在最前");
        assert!(all.get("error").is_none(), "读成功不该有 error: {all}");

        let one = read_recent(&dir, &[], None, Some(8191), 2);
        assert_eq!(one["records"].as_array().map(Vec::len), Some(2));
        assert_eq!(one["records"][0]["reason"], "第三次");
        assert_eq!(one["records"][1]["reason"], "第二次");

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 项目筛只滤「带 project 且对不上」的行；没这一格的行（queue_stall、老版本
    /// 记录）必须留下，回执要说出用了哪个筛子。
    ///
    /// 这本账落在进程工作目录下、跨配置改动活着：同一个目录先后跑过两个项目时，
    /// `dbnum=8191` 会把上一份配置的记录一并端出来——库号空间只在项目内唯一
    /// （ISSUE-025 §五 5a）。而 stall 记录本来就不带 project，按项目筛把它静默滤没，
    /// 「这台机器没停滞过」就成了筛出来的假话。
    #[test]
    fn the_project_filter_keeps_own_and_unattributed_rows() {
        let dir = fixture("project-filter");
        let base_at = Local::now();
        let path = daily_path(&dir, base_at);
        append_json_line(
            &path,
            &to_record(&failure("own", 8191, "本项目的"), base_at),
        )
        .expect("append own");
        let mut foreign = failure("foreign", 8191, "上一份配置留下的");
        foreign.project = "ACP";
        append_json_line(
            &path,
            &to_record(&foreign, base_at + chrono::Duration::seconds(1)),
        )
        .expect("append foreign");
        append_json_line(
            &path,
            &json!({
                "event": "queue_stall",
                "at": (base_at + chrono::Duration::seconds(2)).to_rfc3339(),
                "dbnum": 30999,
                "reasons": ["blocked_by_phase:meta"],
            }),
        )
        .expect("append stall");

        let own = read_recent(&dir, &[], Some("JEU"), None, 0);
        assert_eq!(
            own["project_filter"], "JEU",
            "用了哪个筛子要随回执回去: {own}"
        );
        let records = own["records"].as_array().expect("records");
        assert_eq!(records.len(), 2, "本项目 + 无归属，别的项目滤掉: {own}");
        assert_eq!(
            records[0]["event"], "queue_stall",
            "不带 project 的行不许静默消失"
        );
        assert_eq!(records[1]["reason"], "本项目的");

        let all = read_recent(&dir, &[], None, None, 0);
        assert_eq!(all["records"].as_array().map(Vec::len), Some(3));
        assert!(all["project_filter"].is_null(), "没筛就说没筛: {all}");

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 失败记录带走整本分步账（ISSUE-025 §一）。
    ///
    /// 阶段行原本全是 `println!`，控制台一滚就没：「死在收集」与「收集正常、跑了
    /// 40 分钟之后写回才死」在记录里长得一模一样。这一格要能分开它们——每步带名字、
    /// 进入时刻、毫秒数、走完没有；挤掉过几步也得照实说，读的人才知道自己看的是
    /// 尾巴还是全程。空数组与字段缺席是两句话，同 `staging` 那格的口径。
    #[test]
    fn a_record_carries_the_step_ledger_with_durations() {
        let steps = [
            TaskStep {
                name: "identity_check".into(),
                at: "2026-08-27T11:48:44+08:00".into(),
                ms: 120,
                ok: true,
            },
            TaskStep {
                name: "collect_window".into(),
                at: "2026-08-27T11:48:44+08:00".into(),
                ms: 41_233,
                ok: true,
            },
            TaskStep {
                name: "stage_apply".into(),
                at: "2026-08-27T11:49:25+08:00".into(),
                ms: 8,
                ok: false,
            },
        ];
        let mut with = failure("t", 8191, "boom");
        with.steps = &steps;
        with.steps_dropped = 2;
        let record = to_record(&with, Local::now());
        assert_eq!(record["steps"].as_array().map(Vec::len), Some(3));
        assert_eq!(record["steps"][1]["name"], "collect_window");
        assert_eq!(record["steps"][1]["ms"], 41_233);
        assert_eq!(record["steps"][1]["ok"], true);
        assert_eq!(
            record["steps"][2]["ok"], false,
            "死在手里的那步要自己说出来"
        );
        assert_eq!(record["steps_dropped"], 2, "挤掉了多少不许瞒");

        let bare = to_record(&failure("t", 8191, "boom"), Local::now());
        assert!(
            bare["steps"].as_array().is_some_and(Vec::is_empty),
            "没报过阶段要写成空数组，不是把这一格藏起来: {bare}"
        );
        assert_eq!(bare["steps_dropped"], 0);
    }

    /// 失败记录带走落盘那一刻的暂存窗口残留（ISSUE-025 §四 4a 的记录一半）。
    ///
    /// 面板那张「暂存窗口」卡活在进程内、重启即清空，而 8191 现场恰恰是重启之后来
    /// 问「`staging_8191_1` 回滚了没有」。空数组与字段缺席是两句话：`[]` = 记录时
    /// 已无窗口挂着（回滚干净或走直写），缺席 = 老版本记录压根答不了这一问。
    #[test]
    fn a_record_carries_the_staging_leftovers_at_write_time() {
        let leftovers = [json!({
            "dbnum": 8191,
            "window_id": 1,
            "label": "staging_8191_1",
            "start_sesno": 36,
            "end_sesno": 37,
            "state": "writeback_stalled",
            "writeback_error": "写回卡住的原话",
            "band": "warn",
        })];
        let mut with = failure("t", 8191, "boom");
        with.staging = &leftovers;
        let record = to_record(&with, Local::now());
        assert_eq!(record["staging"][0]["label"], "staging_8191_1");
        assert_eq!(record["staging"][0]["state"], "writeback_stalled");
        assert_eq!(record["staging"][0]["start_sesno"], 36);
        assert_eq!(record["staging"][0]["end_sesno"], 37);

        let clean = to_record(&failure("t", 8191, "boom"), Local::now());
        assert!(
            clean["staging"].as_array().is_some_and(Vec::is_empty),
            "记录时没有残留要写成空数组，不是把这一格藏起来: {clean}"
        );
    }

    /// 两个文件族并成**一条**按时刻排的线，并且按 `event` 筛得动。
    ///
    /// 8191 失败与 30999 停滞是同一件事的两端：批次失败写在本模块那份文件里，队列
    /// 停滞写在看门狗那份里。只按文件先后拼接，两族会各自成块，而这条时间线的全部
    /// 价值恰恰在于它们交替出现的次序——那才看得出「谁先倒、谁跟着饿死」。
    #[test]
    fn both_log_families_merge_into_one_ordered_timeline() {
        let dir = fixture("merge");
        let base_at = Local::now();
        let stalls = dir.join(format!(
            "{}{}{FILE_SUFFIX}",
            crate::data_interface::queue_stall_diagnostics::FILE_PREFIX,
            base_at.format("%Y-%m-%d")
        ));

        // 失败在前，停滞在后：现实里的次序正是这样，8191 倒下之后 30999 才饿死。
        append_json_line(
            &daily_path(&dir, base_at),
            &to_record(&failure("t", 8191, "boom"), base_at),
        )
        .expect("append failure");
        append_json_line(
            &stalls,
            &json!({
                "event": "queue_stall",
                "at": (base_at + chrono::Duration::seconds(60)).to_rfc3339(),
                "dbnum": 30999,
                "reasons": ["blocked_by_phase:meta"],
            }),
        )
        .expect("append stall");

        let all = read_recent(&dir, &[], None, None, 0);
        assert_eq!(all["records"].as_array().map(Vec::len), Some(2));
        assert_eq!(all["records"][0]["event"], "queue_stall", "晚的排前面");
        assert_eq!(all["records"][1]["event"], "batch_failure");
        assert_eq!(
            all["sources"].as_array().map(Vec::len),
            Some(2),
            "两个来源都要报出来: {all}"
        );

        assert_eq!(
            read_recent(&dir, &["batch_failure"], None, None, 0)["records"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "按 event 筛"
        );
        assert_eq!(
            read_recent(&dir, &[], None, Some(30999), 0)["records"][0]["event"],
            "queue_stall",
            "按 dbnum 筛要能跨文件族"
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 半行残句（进程被强杀）只丢它自己，不许带走那一天其余的证据。
    #[test]
    fn a_torn_line_does_not_discard_the_rest_of_the_day() {
        let dir = fixture("torn");
        let now = Local::now();
        let path = daily_path(&dir, now);
        append_json_line(&path, &to_record(&failure("t", 8191, "完整的"), now)).expect("append");
        fs::create_dir_all(&dir).expect("dir");
        let mut file = OpenOptions::new().append(true).open(&path).expect("open");
        writeln!(file, "{{\"event\":\"batch_fail").expect("torn line");
        drop(file);

        let read = read_recent(&dir, &[], None, None, 0);
        assert_eq!(read["records"].as_array().map(Vec::len), Some(1));
        assert_eq!(read["records"][0]["reason"], "完整的");

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    /// 目录还不存在（本进程一次都没失败过）与「读不出来」在面板上是两句话，
    /// 但都必须带 `error`——把读失败画成「没有失败记录」是这一页最会骗人的形态。
    #[test]
    fn an_unreadable_directory_is_reported_not_rendered_as_no_failures() {
        let read = read_recent(&fixture("absent"), &[], None, None, 0);
        assert_eq!(read["records"].as_array().map(Vec::len), Some(0));
        assert!(read["error"].is_string(), "{read}");
        let sources = read["sources"].as_array().expect("读不到也要说清去哪儿找");
        assert!(
            sources
                .iter()
                .filter_map(Value::as_str)
                .any(|source| source.contains("batch-failures-")),
            "{read}"
        );
        assert!(
            sources
                .iter()
                .filter_map(Value::as_str)
                .any(|source| source.contains("queue-stalls-")),
            "{read}"
        );
    }
}
