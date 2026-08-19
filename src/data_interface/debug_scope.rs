//! 调试限定域与增量链路追踪（计划 `docs/plans/2026-08-17-dbnum-increment-trace-plan.md`）。
//!
//! 一个开关干两件事（D3）：把本轮**数据批次**的检查圈到指定 dbnum，并把这些库在
//! 链路六个裁决点上的中间量记下来。只能从命令行来（D2：`aios-database serve
//! --debug-dbnum 7998,8000`），进不了配置文件，因此不会有人一个月前写进去忘了拿掉。
//!
//! 为什么需要它：2026-08-17 两次 live 轮次都停在第一道断言，而两次都因为关键中间量
//! 只活在内存里、任务终态不落库、服务一拆栈就什么都不剩，无法当场定位。
//!
//! # 与 issue #10 的分界
//!
//! 这个形状与被剥夺了增量否决权的 `manual_db_nums` 长得一样。issue #10 的病**不是
//! 「能收窄」**，是收窄之后它静默：7999 被划掉，watcher 每 30 秒发现一次增量、每次
//! 跳过，日志与「MDB 里没这个库」一字不差。所以区别全部做在「有多大声」上：
//!
//! - [`excluded_reason`] 与 [`super::increment_manager::out_of_scope_reason`] 的输出
//!   无交集，且必含 `--debug-dbnum`；
//! - [`mode_notice`] 进每一份 preview / execute 回执的 `warnings`——只有 `println!`
//!   而调用方回执里看不见的报告，视同没有报告；
//! - `/health` 常驻一栏，任何人随时能看出这个服务是跛的。
//!
//! 三条都有测试钉着（D7）。

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde_json::{Value, json};

/// 环形缓存容量。一轮九场景夹具的六个点大约几百条，留一个数量级的余量。
const TRACE_CAPACITY: usize = 4096;

/// stdout 上每条追踪行的前缀。固定不变——它的用途就是从满屏
/// `read file … finished in 14ms` 里被 grep 出来。
pub const TRACE_PREFIX: &str = "AIOS-TRACE";

/// 链路上的六个裁决点（计划 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracePoint {
    /// 扫描裁决：旧水位 / 库里存的与本次观察到的文件水位 / 异常分类 / 处置。
    Scan,
    /// 入队：意图、窗口两端、入队时冻结的 `previous_observed_sesno`、入队四态。
    Enqueue,
    /// 冻结点：重扫后的真右端、是否改走首次导入。
    Freeze,
    /// 路由：基线还是增量、pe 存在性探针、空基线凭据。
    Route,
    /// 收集：口径、会话页清单、算出的并入名单。
    Collect,
    /// 终态：状态、水位旧→新、变更元素数、失败原因。
    Terminal,
}

impl TracePoint {
    pub fn as_str(self) -> &'static str {
        match self {
            TracePoint::Scan => "scan",
            TracePoint::Enqueue => "enqueue",
            TracePoint::Freeze => "freeze",
            TracePoint::Route => "route",
            TracePoint::Collect => "collect",
            TracePoint::Terminal => "terminal",
        }
    }
}

#[derive(Default)]
struct DebugState {
    /// 空 = 未启用调试限定，链路行为与本特性引入前逐位相同。
    dbnums: Vec<u32>,
    records: VecDeque<Value>,
    /// 因容量上限被挤掉的条数。缓存满了要能说出来——悄悄丢比丢本身更糟。
    dropped: u64,
}

static DEBUG: LazyLock<Mutex<DebugState>> = LazyLock::new(|| Mutex::new(DebugState::default()));

fn state() -> MutexGuard<'static, DebugState> {
    DEBUG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 测试串行闸。本模块的状态是进程级的，而 `cargo test` 默认多线程并行：一条用例
/// 装载限定域、另一条断言它是空的，谁先跑都会红。任何会碰 [`set_dbnums`] 的用例
/// （包括别的模块里的）都必须先拿它。
#[cfg(test)]
pub(crate) fn test_guard() -> MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 解析 `--debug-dbnum` 的取值（逗号分隔，允许空白）。
///
/// 认不出的值**报错**，不回落。这与 `AIOS_STARTUP_AUTORUN` 那类「认不出就退回配置
/// 值」的处置刻意相反：那些开关有一个合法的配置缺省可退，而这里一旦把 `799x`
/// 悄悄吞成「全范围」或「空集」，现场会表现为「参数写了但没生效」或「什么都没跑」，
/// 两种都要人再花一轮才发现是自己拼错了。
pub fn parse_dbnums(raw: &str) -> Result<Vec<u32>, String> {
    parse_dbnum_list(raw, "--debug-dbnum")
}

/// 逗号分隔 dbnum 名单的共享解析器。`switch` 只进错误文本：解析规则是同一份，
/// 但报错必须点名**用户实际敲的那个开关**，否则他会去改另一个参数
/// （见 [`super::watch_scope::parse_dbnums`]）。
pub fn parse_dbnum_list(raw: &str, switch: &str) -> Result<Vec<u32>, String> {
    let mut parsed = Vec::new();
    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let dbnum = token
            .parse::<u32>()
            .map_err(|_| format!("{switch} 里的 `{token}` 不是合法的 dbnum"))?;
        if !parsed.contains(&dbnum) {
            parsed.push(dbnum);
        }
    }
    if parsed.is_empty() {
        return Err(format!("{switch} 至少要给一个 dbnum"));
    }
    Ok(parsed)
}

/// 装载调试限定域。只该由 `main.rs` 的命令行解析调用一次。
pub fn set_dbnums(dbnums: Vec<u32>) {
    state().dbnums = dbnums;
}

pub fn dbnums() -> Vec<u32> {
    state().dbnums.clone()
}

/// 本进程是否处于调试限定模式。
pub fn active() -> bool {
    !state().dbnums.is_empty()
}

/// 这个 dbnum 允不允许进本轮数据批次。
///
/// **未启用时恒为 `true`**——这条是 D7 第二条护栏的全部内容：没给开关，判定就必须
/// 与本特性引入前逐位相同。
pub fn admits(dbnum: u32) -> bool {
    let dbnums = &state().dbnums;
    dbnums.is_empty() || dbnums.contains(&dbnum)
}

/// 被调试限定挡掉的库，说给人听的理由。
///
/// 必含 `--debug-dbnum`，且与 `out_of_scope_reason` 无交集（D7 第一条护栏）：两句话
/// 长得一样，就是 issue #10 本身。
pub fn excluded_reason(dbnum: u32) -> String {
    format!(
        "本进程按 --debug-dbnum {} 限定，跳过数据库 {dbnum}（这是调试限定，不是 MDB 范围判定）",
        render_dbnums(&state().dbnums)
    )
}

/// 进每一份 preview / execute 回执 `warnings` 的声明（D7 第三条护栏）。
///
/// 未启用时返回 `None`，回执一个字都不多。
pub fn mode_notice() -> Option<String> {
    let dbnums = state().dbnums.clone();
    (!dbnums.is_empty()).then(|| {
        format!(
            "本进程处于调试限定模式：数据批次只处理 --debug-dbnum {}，其余数据批次\
             （DESI、CATA 等非 SYS meta）一律跳过；SYS meta（SYST/DICT/GLB/GLOB）\
             不受限制。这不是正常运行状态。",
            render_dbnums(&dbnums)
        )
    })
}

fn render_dbnums(dbnums: &[u32]) -> String {
    dbnums
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// 记一条追踪。未启用、或该 dbnum 不在限定域内时是无操作，且 `fields` 闭包
/// **不会执行**——载荷构造（json 分配、协调器快照这类取证成本）只在真要记的时候
/// 发生。「未启用时零成本」不能只靠调用点自觉：json! 的实参是急切求值的，扫描点
/// 每轮全面重扫都会逐文件走到这里（2026-08-18 审核 P3）。
///
/// 同时写 stdout（JSON 行，带 [`TRACE_PREFIX`]）与进程内环形缓存（供
/// `GET /api/v1/trace` 与 `aios-database trace` 在服务拆栈前取走）。
pub fn trace(point: TracePoint, dbnum: u32, fields: impl FnOnce() -> Value) {
    {
        let guard = state();
        if guard.dbnums.is_empty() || !guard.dbnums.contains(&dbnum) {
            return;
        }
    }
    // 闭包在锁外执行：取证可能要拿别的锁（如 batch_scheduler 入队点取
    // InitializationCoordinator 快照），不许嵌在本模块的锁里制造锁序问题。
    // 代价是「判定通过 → 落缓存」之间限定域理论上可变，但限定域只在 main 启动
    // 时装载一次（测试另有串行闸），多记一条无害。
    let record = json!({
        "at": chrono::Local::now().to_rfc3339(),
        "point": point.as_str(),
        "dbnum": dbnum,
        "fields": fields(),
    });
    println!("{TRACE_PREFIX} {record}");
    let mut guard = state();
    if guard.records.len() == TRACE_CAPACITY {
        guard.records.pop_front();
        guard.dropped += 1;
    }
    guard.records.push_back(record);
}

/// 取走缓存快照。`dbnum` 为 `None` 时不筛；`limit` 为 0 时取全部。
///
/// `dropped` 必须一起交出去：读的人得知道自己看的是不是完整的一段。
pub fn snapshot(dbnum: Option<u32>, limit: usize) -> Value {
    let guard = state();
    let mut records: Vec<&Value> = guard
        .records
        .iter()
        .filter(|record| {
            dbnum.is_none_or(|wanted| {
                record.get("dbnum").and_then(Value::as_u64) == Some(u64::from(wanted))
            })
        })
        .collect();
    if limit > 0 && records.len() > limit {
        records.drain(..records.len() - limit);
    }
    json!({
        "debug_dbnums": guard.dbnums,
        "capacity": TRACE_CAPACITY,
        "dropped": guard.dropped,
        "returned": records.len(),
        "records": records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拿下串行闸并把进程级状态清空；闸随返回值的生命周期持有到用例结束。
    #[must_use]
    fn fresh() -> MutexGuard<'static, ()> {
        let lease = test_guard();
        let mut guard = state();
        guard.dbnums.clear();
        guard.records.clear();
        guard.dropped = 0;
        lease
    }

    #[test]
    fn dbnum_lists_parse_and_deduplicate() {
        assert_eq!(parse_dbnums("7998").unwrap(), vec![7998]);
        assert_eq!(parse_dbnums(" 7998 , 8000 ").unwrap(), vec![7998, 8000]);
        assert_eq!(parse_dbnums("7998,7998").unwrap(), vec![7998]);
    }

    /// 拼错的取值必须报错。回落成「全范围」会表现为「参数没生效」，回落成空集会
    /// 表现为「什么都没跑」，两种都要人再花一轮才发现是自己手误。
    #[test]
    fn unparseable_values_are_rejected_instead_of_falling_back() {
        assert!(parse_dbnums("799x").is_err());
        assert!(parse_dbnums("").is_err());
        assert!(parse_dbnums(" , ").is_err());
    }

    /// D7 护栏二：没给开关，入范围判定与本特性引入前逐位相同。
    #[test]
    fn an_empty_debug_scope_admits_everything() {
        let _lease = fresh();
        for dbnum in [0, 1, 7997, 7998, 8000, u32::MAX] {
            assert!(admits(dbnum), "未启用时 {dbnum} 必须照旧放行");
        }
        assert!(!active());
        assert_eq!(mode_notice(), None, "未启用时回执一个字都不该多");
    }

    #[test]
    fn a_debug_scope_admits_only_its_members() {
        let _lease = fresh();
        set_dbnums(vec![7998, 8000]);
        assert!(admits(7998) && admits(8000));
        assert!(!admits(7997) && !admits(8191));
        assert!(active());
        assert!(mode_notice().is_some_and(|notice| notice.contains("7998,8000")));
        set_dbnums(Vec::new());
    }

    /// D7 护栏一：调试排除的理由必须点名开关，且不能与 MDB 范围判定那句混同。
    #[test]
    fn the_exclusion_reason_names_the_switch_and_denies_being_a_scope_verdict() {
        let _lease = fresh();
        set_dbnums(vec![7998]);
        let reason = excluded_reason(7997);
        assert!(
            reason.contains("--debug-dbnum"),
            "理由必须点名开关: {reason}"
        );
        assert!(
            reason.contains("不是 MDB 范围判定"),
            "理由必须自证它不是范围判定: {reason}"
        );
        set_dbnums(Vec::new());
    }

    /// 缓存满了要挤掉最旧的并如实计数——悄悄丢比丢本身更糟。
    #[test]
    fn the_ring_evicts_the_oldest_and_counts_what_it_dropped() {
        let _lease = fresh();
        set_dbnums(vec![7998]);
        for seq in 0..TRACE_CAPACITY + 5 {
            trace(TracePoint::Scan, 7998, || json!({ "seq": seq }));
        }
        let snapshot = snapshot(Some(7998), 0);
        assert_eq!(snapshot["dropped"], 5);
        assert_eq!(snapshot["returned"], TRACE_CAPACITY);
        assert_eq!(
            snapshot["records"][0]["fields"]["seq"], 5,
            "挤掉的必须是最旧的那几条"
        );
        set_dbnums(Vec::new());
    }

    /// 限定域外的 dbnum 一条都不记：追踪目标与限定目标是同一个集合（D3）。
    /// 顺带钉惰性：域外调用连载荷闭包都不许执行。
    #[test]
    fn tracing_ignores_dbnums_outside_the_debug_scope() {
        let _lease = fresh();
        set_dbnums(vec![7998]);
        trace(TracePoint::Scan, 7997, || panic!("限定域外不许构造载荷"));
        trace(TracePoint::Scan, 7998, || json!({}));
        assert_eq!(snapshot(None, 0)["returned"], 1);
        set_dbnums(Vec::new());
    }

    /// 未启用时连缓存都不该长、载荷闭包也不许执行——默认路径上这个模块必须是
    /// 零成本的。json! 实参是急切求值的，只有闭包形态才守得住这句承诺。
    #[test]
    fn tracing_is_inert_until_the_switch_is_set() {
        let _lease = fresh();
        trace(TracePoint::Scan, 7998, || panic!("未启用时不许构造载荷"));
        assert_eq!(snapshot(None, 0)["returned"], 0);
    }
}
