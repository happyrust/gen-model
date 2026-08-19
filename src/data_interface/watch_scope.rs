//! 增量监听限定域：`DbOption.toml` 的 `watch_dbnums` 与 `aios-database serve
//! --watch-dbnum 7998,8000`。
//!
//! 它只干一件事：把增量摄入的**数据批次**圈到指定 dbnum。不带追踪——那是
//! [`super::debug_scope`] 的活，两个开关各干各的，互不代替也互不覆盖。
//!
//! # 与 `--debug-dbnum` 的分界
//!
//! | | `watch_dbnums` / `--watch-dbnum` | `--debug-dbnum` |
//! |---|---|---|
//! | 来源 | 配置文件**或**命令行（命令行压过配置） | 只有命令行 |
//! | 附带能力 | 无 | 六个裁决点的链路追踪 |
//! | 活多久 | 配置里的能跨重启活着 | 进程一停就没了 |
//!
//! 两个都收窄范围，因此**两个都必须有自己的嗓音**：日志、回执与 `/health` 里绝不
//! 能出现「这个库被跳过了」而说不清是谁跳的。
//!
//! # 为什么配置文件里的这一个要更吵
//!
//! 这个形状与被剥夺了增量否决权的 `manual_db_nums` 一模一样，而 issue #10 就是被
//! 它坑的：7999 被配置里的手写名单挡在外面，watcher 每 30 秒发现一次增量、每次
//! 跳过，日志与「MDB 里没这个库」一字不差，于是没人看得出是自己划的。命令行参数
//! 进程一停就没了，配置文件里的能在那儿躺一个月——所以这里的护栏比
//! `--debug-dbnum` 只多不少：
//!
//! - [`excluded_reason`] 与 [`super::increment_manager::out_of_scope_reason`]、
//!   [`super::debug_scope::excluded_reason`] 三句话两两无交集，且必含
//!   [`WATCH_CONFIG_KEY`]；
//! - [`mode_notice`] 进每一份 preview / execute 回执的 `warnings`，并**点名它是从
//!   配置来的还是从命令行来的**——「一个月前谁写的」是这类静默收窄最贵的问题；
//! - 启动横幅（`run_cli` 与 Python `full_init` 两条路都有）与 `/health` 常驻一栏。

use std::sync::{LazyLock, Mutex, MutexGuard};

/// 命令行开关名。出现在每一句「这个库为什么被跳过」里。
pub const WATCH_SWITCH: &str = "--watch-dbnum";

/// 配置键名。同上。
pub const WATCH_CONFIG_KEY: &str = "watch_dbnums";

/// 限定域从哪儿来。回执与横幅必须说出来：命令行是这一次跑的意图，配置文件是
/// 可能已经躺了一个月的意图，两者的处置完全不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 来自 `DbOption.toml` 的 `watch_dbnums`。
    Config,
    /// 来自 `serve --watch-dbnum`。
    Cli,
}

impl Origin {
    /// 说给人听的来源。
    pub fn describe(self) -> String {
        match self {
            Origin::Config => format!("DbOption.toml 的 {WATCH_CONFIG_KEY}"),
            Origin::Cli => format!("命令行 {WATCH_SWITCH}"),
        }
    }
}

#[derive(Default)]
struct WatchState {
    /// `None` = 还没解析过；首次询问时从配置装载（见 [`resolved`]）。
    /// `Some((空表, _))` = 解析过且没限定，行为与本特性引入前逐位相同。
    resolved: Option<(Vec<u32>, Origin)>,
}

static WATCH: LazyLock<Mutex<WatchState>> = LazyLock::new(|| Mutex::new(WatchState::default()));

fn state() -> MutexGuard<'static, WatchState> {
    WATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 测试串行闸。**借用 [`super::debug_scope::test_guard`] 那一把**，不另开一把：
/// 两个限定域都是进程级状态，且同一批用例（`in_scope_with` / `skip_reason`）两个
/// 都要碰；各持一把锁必然出现「A 先拿监听闸再拿调试闸、B 反过来」的锁序问题。
#[cfg(test)]
pub(crate) fn test_guard() -> MutexGuard<'static, ()> {
    super::debug_scope::test_guard()
}

/// 解析 `--watch-dbnum` 的取值（逗号分隔，允许空白）。
///
/// 认不出的值**报错**，不回落——理由见
/// [`super::debug_scope::parse_dbnum_list`]。
pub fn parse_dbnums(raw: &str) -> Result<Vec<u32>, String> {
    super::debug_scope::parse_dbnum_list(raw, WATCH_SWITCH)
}

/// 装载命令行给的限定域，压过配置。只该由 `main.rs` 的命令行解析调用一次。
pub fn set_cli_dbnums(dbnums: Vec<u32>) {
    state().resolved = Some((dbnums, Origin::Cli));
}

/// 生效的限定域与它的来源。空表 = 没限定。
///
/// 命令行没给过就从配置装载一次并记住：配置本身是 `OnceLock` 缓存的，这里再缓存
/// 一层是为了让来源（[`Origin`]）与取值同生共死——回执里说「来自配置」的那一刻，
/// 判定用的必须就是那份配置值。
pub fn resolved() -> (Vec<u32>, Origin) {
    if let Some(resolved) = state().resolved.clone() {
        return resolved;
    }
    // 配置装载在锁外做：它要读文件并初始化 `options` 那边的 `OnceLock`，不该嵌在
    // 本模块的锁里制造锁序问题。装载期间若命令行那份先落了地，`get_or_insert`
    // 会保留它——命令行压过配置，这个次序不能被一次竞态翻过来。
    let configured = (crate::options::watch_dbnums(), Origin::Config);
    state().resolved.get_or_insert(configured).clone()
}

/// 生效的限定域。空表 = 全范围。
pub fn dbnums() -> Vec<u32> {
    resolved().0
}

/// 本进程是否处于监听限定模式。
pub fn active() -> bool {
    !dbnums().is_empty()
}

/// 这个 dbnum 允不允许进增量摄入。
///
/// **未限定时恒为 `true`**：没写配置、没给开关，判定就必须与本特性引入前逐位相同。
pub fn admits(dbnum: u32) -> bool {
    let dbnums = dbnums();
    dbnums.is_empty() || dbnums.contains(&dbnum)
}

/// 被监听限定挡掉的库，说给人听的理由。
///
/// 必含 [`WATCH_CONFIG_KEY`] 与来源，且与 MDB 范围判定、调试限定那两句无交集
/// ——三句话长得一样，就是 issue #10 本身。
pub fn excluded_reason(dbnum: u32) -> String {
    let (dbnums, origin) = resolved();
    format!(
        "本进程按 {WATCH_CONFIG_KEY} {} 限定监听范围（来自{}），跳过数据库 {dbnum}\
         （这是监听限定，不是 MDB 范围判定）",
        render_dbnums(&dbnums),
        origin.describe()
    )
}

/// 进每一份 preview / execute 回执 `warnings`、启动横幅与 `/health` 的声明。
///
/// 未限定时返回 `None`，回执一个字都不多。
pub fn mode_notice() -> Option<String> {
    let (dbnums, origin) = resolved();
    (!dbnums.is_empty()).then(|| {
        format!(
            "本进程处于监听限定模式：增量只处理 {WATCH_CONFIG_KEY} {} 里的数据批次，\
             其余数据批次（DESI、CATA 等非 SYS meta）一律跳过；\
             SYS meta（SYST/DICT/GLB/GLOB）不受限制。限定来自{}——\
             这是被显式收窄的范围，不是完整运行状态。",
            render_dbnums(&dbnums),
            origin.describe()
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

#[cfg(test)]
pub(crate) fn set_dbnums_for_tests(dbnums: Vec<u32>, origin: Origin) {
    state().resolved = Some((dbnums, origin));
}

/// 把限定域恢复成「还没解析过」。用例之间不许互相串。
#[cfg(test)]
pub(crate) fn clear_for_tests() {
    state().resolved = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拿下串行闸并清空进程级状态；闸随返回值的生命周期持有到用例结束。
    #[must_use]
    fn fresh() -> MutexGuard<'static, ()> {
        let lease = test_guard();
        clear_for_tests();
        lease
    }

    /// 命令行与配置共用一个解析器，但报错时各说各的开关名——把
    /// `--watch-dbnum 799x` 的错报成 `--debug-dbnum`，人会去改另一个参数。
    #[test]
    fn the_parser_names_this_switch_in_its_errors() {
        assert_eq!(parse_dbnums(" 7998 , 8000 ").unwrap(), vec![7998, 8000]);
        assert_eq!(parse_dbnums("7998,7998").unwrap(), vec![7998]);
        let error = parse_dbnums("799x").unwrap_err();
        assert!(error.contains(WATCH_SWITCH), "{error}");
        assert!(!error.contains("--debug-dbnum"), "{error}");
        assert!(parse_dbnums("").is_err());
    }

    /// 没配置、没给开关时，判定与本特性引入前逐位相同，且回执一个字都不多。
    #[test]
    fn an_empty_watch_scope_admits_everything() {
        let _lease = fresh();
        set_dbnums_for_tests(Vec::new(), Origin::Config);
        for dbnum in [0, 1, 7997, 7998, 8000, u32::MAX] {
            assert!(admits(dbnum), "未限定时 {dbnum} 必须照旧放行");
        }
        assert!(!active());
        assert_eq!(mode_notice(), None, "未限定时回执一个字都不该多");
        clear_for_tests();
    }

    #[test]
    fn a_watch_scope_admits_only_its_members() {
        let _lease = fresh();
        set_dbnums_for_tests(vec![7998, 8000], Origin::Config);
        assert!(admits(7998) && admits(8000));
        assert!(!admits(7997) && !admits(8191));
        assert!(active());
        clear_for_tests();
    }

    /// 命令行压过配置：配置里躺着的名单不该让「我这次只想看 8000」失效。
    #[test]
    fn the_command_line_wins_over_the_configured_list() {
        let _lease = fresh();
        set_dbnums_for_tests(vec![7998], Origin::Config);
        set_cli_dbnums(vec![8000]);
        assert!(admits(8000) && !admits(7998));
        assert_eq!(resolved().1, Origin::Cli);
        clear_for_tests();
    }

    /// 来源必须进回执与理由：配置里的名单可能是一个月前写的，命令行的是这一次的
    /// 意图，两者该做的处置完全不同。
    #[test]
    fn every_announcement_names_where_the_narrowing_came_from() {
        let _lease = fresh();
        for (origin, needle) in [
            (Origin::Config, WATCH_CONFIG_KEY),
            (Origin::Cli, WATCH_SWITCH),
        ] {
            set_dbnums_for_tests(vec![7998], origin);
            let notice = mode_notice().expect("限定生效时必须出声");
            assert!(notice.contains(needle), "回执要点名来源: {notice}");
            assert!(
                excluded_reason(7997).contains(needle),
                "跳过理由要点名来源: {}",
                excluded_reason(7997)
            );
        }
        clear_for_tests();
    }

    /// 监听限定的理由不许借用 MDB 范围判定或调试限定的措辞。
    #[test]
    fn the_watch_exclusion_has_a_voice_of_its_own() {
        let _lease = fresh();
        set_dbnums_for_tests(vec![7998], Origin::Config);
        let reason = excluded_reason(7997);
        assert!(reason.contains(WATCH_CONFIG_KEY), "{reason}");
        assert!(
            !reason.contains("不在本期执行范围"),
            "监听限定不得借用范围判定的措辞: {reason}"
        );
        assert!(
            !reason.contains("--debug-dbnum"),
            "监听限定不得借用调试限定的措辞: {reason}"
        );
        clear_for_tests();
    }
}
