//! 模型增量类型限定：`DbOption.toml` 的 `model_incremental_types` 与环境变量
//! `AIOS_MODEL_INCREMENTAL_TYPES=BRAN,EQUI`。
//!
//! 它只干一件事：把**模型生成的消费**圈到指定类型的生成根。这道门今天只有一个调用点——
//! 初始化路（`dbnum_initialize::model_half_inner`）按 [`admits`] 把枚举出来的根分成放行 /
//! 暂扣两堆，暂扣的一根不生成。**增量路没有这道门**：数据窗口规划出来的工作项不看名单、
//! 全部交给任务生成，所以窗口回执里也没有「本窗 N 个根暂扣」那一句——它曾经是
//! `window_plan_notice`，报的是一件实际没发生的事，2026-09-15 删掉。数据增量（窗口收集、
//! `pe` / `ATT_*` 写回、水位推进）一律照旧；`Transform`（noun 为空）、`CascadeExpand`、
//! `PostRegenAabb` 与房间任务不受限。
//!
//! # 为什么是消费门，不是计划门
//!
//! 直觉的做法是在规划器里把名单外的根从 `work_items` 里摘掉。那是错的：暂扣的根没生成，
//! 这一版的根就没有「全部落库」，任务收尾不盖 `model_sesno`（`task_end` 守着 `all_roots_ok`），
//! 下一窗 / 重启的启动核对按水位缺口把它重排（ADR-079：库里没有逐根的行可标 stale，水位不盖
//! 就是它在库里的全部说法）。摘掉工作项 = 假装这一窗没有它 = 水位照盖 = 给陈旧几何盖新章，
//! 而且此后没有任何路径会回补它。留下工作项、只是不领取，水位就守住了这个根：名单一撤就照常
//! 消费。这与 `model_incremental = false`「只停消费、不删 durable 工作」是同一条纪律。
//!
//! # 与别的限定的分界
//!
//! | | `model_incremental_types` | `watch_dbnums` | `delivery_unit_types` |
//! |---|---|---|---|
//! | 圈的是什么 | 模型消费按**类型** | 数据批次与模型工作单按**库** | 生成根**怎么选**（哪些 noun 是 MDU） |
//! | 动不动数据面 | 不动 | 动（数据批次只收域内库） | 不动 |
//! | 名单外的根 | 行留在队列里不领取 | 行留在队列里不领取 | 不存在「名单外」：它改变的是根的归并 |
//!
//! 三者互不代替：`delivery_unit_types = ["BRAN","EQUI"]` 会让 HANG 底下的变化上溯到
//! ZONE 级正常颗粒去生成，而本键让 HANG 的根照常选出、照常入队、只是不生成。
//!
//! # 有多大声
//!
//! 与 `watch_dbnums` 同一套护栏（它们的形状一样，坑也一样——配置里的名单能躺一个月）：
//! 启动横幅、`/health` 的 `model_incremental_types` 栏、每份 preview / execute 回执的
//! `warnings`（[`mode_notice`]）、初始化路暂扣根数的那一行日志、worker 空闲轮首次一句。
//! 每一句都点名 [`MODEL_TYPE_CONFIG_KEY`] 与来源。

use std::sync::{LazyLock, Mutex, MutexGuard};

/// 配置键名。出现在每一句「这个根为什么没生成」里。
pub const MODEL_TYPE_CONFIG_KEY: &str = "model_incremental_types";

/// 环境变量名。同上。
pub const MODEL_TYPE_ENV: &str = crate::options::MODEL_INCREMENTAL_TYPES_ENV;

/// 名单从哪儿来。回执与横幅必须说出来：环境变量是这一次跑的意图，配置文件是
/// 可能已经躺了一个月的意图，两者的处置完全不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 来自 `DbOption.toml` 的 `model_incremental_types`。
    Config,
    /// 来自环境变量 `AIOS_MODEL_INCREMENTAL_TYPES`。
    Env,
}

impl Origin {
    /// 说给人听的来源。
    pub fn describe(self) -> String {
        match self {
            Origin::Config => format!("DbOption.toml 的 {MODEL_TYPE_CONFIG_KEY}"),
            Origin::Env => format!("环境变量 {MODEL_TYPE_ENV}"),
        }
    }
}

#[derive(Default)]
struct TypeScopeState {
    /// `None` = 还没解析过；首次询问时从配置 / 环境装载（见 [`resolved`]）。
    /// `Some((空表, _))` = 解析过且没限定，行为与本特性引入前逐位相同。
    resolved: Option<(Vec<String>, Origin)>,
}

static SCOPE: LazyLock<Mutex<TypeScopeState>> =
    LazyLock::new(|| Mutex::new(TypeScopeState::default()));

fn state() -> MutexGuard<'static, TypeScopeState> {
    SCOPE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 测试串行闸。**借用 [`super::debug_scope::test_guard`] 那一把**，理由同
/// [`super::watch_scope::test_guard`]：几个限定域都是进程级状态，同一批用例可能
/// 都要碰，各持一把锁必然出现锁序问题。
#[cfg(test)]
pub(crate) fn test_guard() -> MutexGuard<'static, ()> {
    super::debug_scope::test_guard()
}

/// 生效的名单与它的来源。空表 = 没限定。
///
/// 只装载一次并记住：配置本身是 `OnceLock` 缓存的，这里再缓存一层是为了让来源
/// （[`Origin`]）与取值同生共死——回执里说「来自配置」的那一刻，判定用的必须就是
/// 那份配置值。
pub fn resolved() -> (Vec<String>, Origin) {
    if let Some(resolved) = state().resolved.clone() {
        return resolved;
    }
    // 装载在锁外做：它要读文件并初始化 `options` 那边的 `OnceLock`，不该嵌在本模块
    // 的锁里制造锁序问题。
    let (types, from_env) = crate::options::model_incremental_types_resolved();
    let origin = if from_env {
        Origin::Env
    } else {
        Origin::Config
    };
    state().resolved.get_or_insert((types, origin)).clone()
}

/// 生效的类型名单（已 trim + 大写 + 去重）。空表 = 不限类型。
pub fn nouns() -> Vec<String> {
    resolved().0
}

/// 本进程是否处于模型类型限定模式。
pub fn active() -> bool {
    !nouns().is_empty()
}

/// 这个类型的生成根允不允许被本进程生成 / 清理。
///
/// **未限定时恒为 `true`**：没写配置、没给环境变量，判定就必须与本特性引入前逐位相同。
/// **noun 未知（空串）时也放行**：挡不准的东西不挡——静默漏做比多做一次贵得多。
pub fn admits(noun: &str) -> bool {
    admits_with(&nouns(), noun)
}

/// [`admits`] 的纯函数形态：名单显式传入，不碰进程级状态，用例不必先装载配置。
pub fn admits_with(allowed: &[String], noun: &str) -> bool {
    let noun = noun.trim();
    allowed.is_empty()
        || noun.is_empty()
        || allowed
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(noun))
}

/// 被类型限定暂扣的根，说给人听的理由。
///
/// 必含 [`MODEL_TYPE_CONFIG_KEY`] 与来源，且与监听限定、MDB 范围判定那几句无交集
/// ——三句话长得一样，就是 issue #10 本身。
pub fn held_reason(noun: &str) -> String {
    let (types, origin) = resolved();
    format!(
        "本进程按 {MODEL_TYPE_CONFIG_KEY} [{}] 限定模型增量类型（来自{}），类型 {noun} 的生成根\
         本任务不生成、留待名单放开后的下一窗（这是模型类型限定，不是监听限定，数据增量不受影响）",
        render_nouns(&types),
        origin.describe()
    )
}

/// 进每一份 preview / execute 回执 `warnings`、启动横幅与 `/health` 的声明。
///
/// 未限定时返回 `None`，回执一个字都不多。
pub fn mode_notice() -> Option<String> {
    let (types, origin) = resolved();
    (!types.is_empty()).then(|| {
        format!(
            "本进程处于模型类型限定模式：模型增量只生成 / 清理 {MODEL_TYPE_CONFIG_KEY} [{}] 里的\
             生成根类型，其余类型的生成根在本任务里暂扣不生成\
             （模型水位不盖、随下一窗重排、名单一撤即照常生成）；数据增量、Transform、\
             CascadeExpand、房间任务与按需 ensure 不受限。限定来自{}——这是被显式收窄的范围，\
             不是完整运行状态。",
            render_nouns(&types),
            origin.describe()
        )
    })
}

/// `BRAN,EQUI`——回执与日志里的名单写法。
pub fn render_nouns(nouns: &[String]) -> String {
    nouns.join(",")
}

#[cfg(test)]
pub(crate) fn set_types_for_tests(types: Vec<String>, origin: Origin) {
    state().resolved = Some((types, origin));
}

/// 把限定域恢复成「还没解析过」。用例之间不许互相串。
#[cfg(test)]
pub(crate) fn clear_for_tests() {
    state().resolved = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// 拿下串行闸并清空进程级状态；闸随返回值的生命周期持有到用例结束。
    #[must_use]
    fn fresh() -> MutexGuard<'static, ()> {
        let lease = test_guard();
        clear_for_tests();
        lease
    }

    /// 没配置、没给环境变量时，判定与本特性引入前逐位相同，且回执一个字都不多。
    #[test]
    fn an_empty_type_scope_admits_everything() {
        let _lease = fresh();
        set_types_for_tests(Vec::new(), Origin::Config);
        for noun in ["", "BRAN", "HANG", "PANE", "ZONE", "whatever"] {
            assert!(admits(noun), "未限定时 {noun:?} 必须照旧放行");
        }
        assert!(!active());
        assert_eq!(mode_notice(), None, "未限定时回执一个字都不该多");
        clear_for_tests();
    }

    /// 名单只放行自己的成员；noun 大小写不敏感；**空 noun 放行**——老规划器的
    /// Transform / DeleteCleanup 行不带 noun，挡不准的不挡。
    #[test]
    fn a_type_scope_admits_only_its_members_and_never_blocks_unknown_nouns() {
        let _lease = fresh();
        set_types_for_tests(owned(&["BRAN", "EQUI"]), Origin::Config);
        assert!(admits("BRAN") && admits("EQUI") && admits("bran"));
        assert!(!admits("HANG") && !admits("SUPPO") && !admits("PANE"));
        assert!(
            admits("") && admits("  "),
            "noun 未知的行不得被类型限定挡下"
        );
        assert!(active());
        clear_for_tests();
    }

    /// 纯函数形态与进程级判定必须是同一口径——SQL 谓词那侧用的是它。
    #[test]
    fn the_pure_predicate_matches_the_process_level_one() {
        let allowed = owned(&["BRAN"]);
        assert!(admits_with(&allowed, "BRAN"));
        assert!(!admits_with(&allowed, "HANG"));
        assert!(admits_with(&allowed, ""));
        assert!(admits_with(&[], "HANG"));
    }

    /// 来源必须进回执与理由：配置里的名单可能是一个月前写的，环境变量的是这一次的
    /// 意图，两者该做的处置完全不同。
    #[test]
    fn every_announcement_names_where_the_narrowing_came_from() {
        let _lease = fresh();
        for (origin, needle) in [
            (Origin::Config, MODEL_TYPE_CONFIG_KEY),
            (Origin::Env, MODEL_TYPE_ENV),
        ] {
            set_types_for_tests(owned(&["BRAN"]), origin);
            let notice = mode_notice().expect("限定生效时必须出声");
            assert!(notice.contains(needle), "回执要点名来源: {notice}");
            assert!(notice.contains("BRAN"), "回执要点名名单: {notice}");
            let reason = held_reason("HANG");
            assert!(reason.contains(needle), "暂扣理由要点名来源: {reason}");
            assert!(reason.contains("HANG"), "暂扣理由要点名类型: {reason}");
        }
        clear_for_tests();
    }

    /// 类型限定的理由不许借用监听限定或 MDB 范围判定的措辞，也必须说清数据增量不受影响。
    #[test]
    fn the_type_holdback_has_a_voice_of_its_own() {
        let _lease = fresh();
        set_types_for_tests(owned(&["BRAN"]), Origin::Config);
        let reason = held_reason("HANG");
        assert!(reason.contains(MODEL_TYPE_CONFIG_KEY), "{reason}");
        assert!(reason.contains("数据增量不受影响"), "{reason}");
        assert!(
            !reason.contains("不在本期执行范围"),
            "类型限定不得借用范围判定的措辞: {reason}"
        );
        assert!(
            !reason.contains(super::super::watch_scope::WATCH_CONFIG_KEY),
            "类型限定不得借用监听限定的措辞: {reason}"
        );
        clear_for_tests();
    }
}
