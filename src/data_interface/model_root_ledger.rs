//! 进程内的生成根账本（ADR-079）：`gen_root` 表退役之后，「这个库的哪些根这一版生成成了、
//! 哪些失败了、哪些永久放弃」只活在这里。
//!
//! 它**不是**第二份真值。库里模型面唯一的持久事实是 `dbnum_watermark.model_sesno`
//! （ADR-060：被写下的承诺，任务收尾一笔盖）；账本只是这一个进程在两次盖章之间对逐根成败的
//! 工作记录——下一窗要把失败的根并进 regen（`window_root_plan::carry_unsettled_generation`）、
//! 一致性判决要数「还在赶 / 已放弃」（`dbnum_consistency`）、`/model/ensure` 要回一句发布状态，
//! 都读它。同一根同一错误连败 [`DEAD_ROOT_STREAK`] 次判死转「已放弃」：不拦水位、不进 regen，
//! 与「已追平」并排陈述（ISSUE-029：确定性失败要有终态，不能拖着整个库的水位永远追不上）。
//! 只有这根自己的生成报错攒连败；刷盘、房间、整批报错与可重试的那几类走
//! [`record_transient_failure`]，记失败、不攒连败。判死那一窗的任务把它的旧模型撤掉
//! （`batch_worker::retire_newly_abandoned_roots`）：后面的水位担保的是「这根没有模型」，
//! 不是一份数据变化之前的几何——重启后账本里没有它，库里也就没有什么可以冒充最新。
//! 进程终止它就消失，这是有意的：重启后「欠什么」由两枚水位回答——`model_sesno`
//! 落后就按 `model_sesno → 文件最新` 重排（`startup_reconcile`），写入标记停在 `writing` 就整库
//! 重初始化（ADR-073 D3）。没有一条恢复路需要知道上一进程里哪一根曾经失败。
//!
//! 逐根的**成功**事实另有宿主：`ModelMemoryStore::RootReceipt`（按哪一版源 / 哪一份 ModelTarget
//! 生成）。账本里的 `Generated` 只是给普查数「已追平」用的计数，不参与任何放行判据。
//!
//! 写者：任务的单元执行（`batch_worker`）、初始化模型半边（`dbnum_initialize`）、按需 `ensure`
//! （`on_demand_model`）、整库清空 / 回退（`fast_delete` 的调用方）。读者：规划器、判决、HTTP。
//! 全部同步、无锁等待之外的 I/O，一把 `RwLock` 够用——每根一条小记录，AMS 全量 12 万根量级。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;

/// 同一根带着同一句错误连续失败这么多次之后判死（ISSUE-029）：`Failed` 转 `Refused`，
/// 不再拦 `model_sesno`、下一窗也不再并进 regen。「同一错误」才算连败——错误变了说明
/// 现场在动，重新数；与这根无关的失败（[`record_transient_failure`]）把连败清零。数值沿用
/// 退役前 `model_update_pending::MAX_ATTEMPTS` 的 5。
pub(crate) const DEAD_ROOT_STREAK: u32 = 5;

/// 失败根的退避（GitHub #72 / ADR-090 D1）：第一次失败后 30 s 内任务不重算它，此后每再失败一次翻倍，
/// 封顶 10 min——与房间重建的 `RebuildBackoff` 同档。不 park：一个坏根不冻住整库模型面；也不替代判死线，
/// 只是把判死之前的重试间隔拉长。只在进程内（ADR-079），重启即清零。
pub(crate) const RETRY_BACKOFF_BASE: Duration = Duration::from_secs(30);
pub(crate) const RETRY_BACKOFF_CAP: Duration = Duration::from_secs(10 * 60);

/// 连着失败 `failures` 次之后的退避时长（`failures = 0` 不退避）。
pub(crate) fn retry_backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let doublings = (failures - 1).min(16);
    RETRY_BACKOFF_BASE
        .saturating_mul(1u32 << doublings)
        .min(RETRY_BACKOFF_CAP)
}

/// 一根最近一次生成的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootOutcome {
    /// 生成并落库成功（几何可能为零——`NoRenderableGeometry` 也算成）。
    Generated,
    /// 这一版失败了，下一窗 / 下一次该库任务要重排它（拦水位）。
    Failed { error: String },
    /// 永久放弃：服务端判它解析不出合法生成根（`http_422` 一类，noun 与数据的永久性质），
    /// 或同一错误连败 [`DEAD_ROOT_STREAK`] 次判死（ISSUE-029：确定性失败要有终态）。
    /// 重试多少次都一样。不拦水位，但必须与「已追平」并排陈述（ADR-060 D5 / Q10）。
    Refused { error: String },
}

/// 账本里的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootEntry {
    pub noun: String,
    pub outcome: RootOutcome,
    /// 这一根这次是按哪一版源生成 / 失败的；`0` = 调用方不知道（按需 ensure）。
    pub source_end_sesno: i32,
    /// 同一句错误连续失败的次数（`Generated` 时恒 0）：攒到 [`DEAD_ROOT_STREAK`] 转 `Refused`。
    pub failure_streak: u32,
    /// 连着失败的次数，不分错误、不分是不是这根自己的错（`Generated` / 放弃时恒 0）：定退避档。
    pub consecutive_failures: u32,
    /// 退避到这一刻之前，任务不重算这根（`None` = 不退避）。
    pub retry_at: Option<Instant>,
}

impl RootEntry {
    pub fn is_settled(&self) -> bool {
        matches!(self.outcome, RootOutcome::Generated)
    }

    /// 还在赶：失败且不是永久放弃。退避中的根也还在赶——几何不可信、拦水位，只是这会儿不重算。
    pub fn is_chasing(&self) -> bool {
        matches!(self.outcome, RootOutcome::Failed { .. })
    }

    /// 还在赶、且退避期没过。
    pub fn backing_off(&self, now: Instant) -> bool {
        self.is_chasing() && self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    pub fn is_refused(&self) -> bool {
        matches!(self.outcome, RootOutcome::Refused { .. })
    }

    pub fn error(&self) -> Option<&str> {
        match &self.outcome {
            RootOutcome::Generated => None,
            RootOutcome::Failed { error } | RootOutcome::Refused { error } => Some(error),
        }
    }

    /// 露给 HTTP 的状态字面值，与退役前 `gen_root.status` 的取值保持同一口径。
    pub fn status_label(&self) -> &'static str {
        match self.outcome {
            RootOutcome::Generated => "Generated",
            RootOutcome::Failed { .. } => "failed",
            RootOutcome::Refused { .. } => "http_422",
        }
    }
}

/// 一个库的实时根普查：已追平 / 还在赶 / 已放弃。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerCensus {
    pub settled: u32,
    pub chasing: u32,
    pub abandoned: u32,
}

#[derive(Debug, Default)]
struct Ledger {
    roots: BTreeMap<u32, BTreeMap<RefnoEnum, RootEntry>>,
}

static LEDGER: LazyLock<RwLock<Ledger>> = LazyLock::new(|| RwLock::new(Ledger::default()));

fn read() -> std::sync::RwLockReadGuard<'static, Ledger> {
    LEDGER
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write() -> std::sync::RwLockWriteGuard<'static, Ledger> {
    LEDGER
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `24384/1` / `24384_1` 两种写法都认；解析不出来返回 `None`（调用方只记日志，账本不背锅）。
pub fn parse_root(root: &str) -> Option<RefnoEnum> {
    use std::str::FromStr;
    let normalized = root.trim().trim_start_matches('=').replace('_', "/");
    aios_core::RefU64::from_str(&normalized)
        .ok()
        .map(RefnoEnum::from)
}

fn record(dbnum: u32, root: RefnoEnum, entry: RootEntry) {
    write().roots.entry(dbnum).or_default().insert(root, entry);
}

/// 这一根这一版生成成功。
pub fn record_generated(dbnum: u32, root: RefnoEnum, noun: &str, source_end_sesno: i32) {
    record(
        dbnum,
        root,
        RootEntry {
            noun: noun.trim().to_ascii_uppercase(),
            outcome: RootOutcome::Generated,
            source_end_sesno,
            failure_streak: 0,
            consecutive_failures: 0,
            retry_at: None,
        },
    );
}

/// 这一根上一条记录之后又失败一次：连着失败的次数。
fn next_consecutive_failures(previous: Option<&RootEntry>) -> u32 {
    previous
        .filter(|entry| !entry.is_settled())
        .map_or(1, |entry| entry.consecutive_failures.saturating_add(1))
}

/// 这一根这一版失败了，错出在它自己的生成上（几何算不出、引用的目录件不在之类）：同一句错误
/// 连败 [`DEAD_ROOT_STREAK`] 次判死转 `Refused`（ISSUE-029），否则记 `Failed`（下一窗重排、拦水位）。
/// `error` 截 2000 字，与退役前 `gen_root.last_error` 同一口径。返回记完之后这根是不是已放弃。
pub fn record_failed(
    dbnum: u32,
    root: RefnoEnum,
    noun: &str,
    source_end_sesno: i32,
    error: &str,
) -> bool {
    let error = truncate(error);
    let mut ledger = write();
    let roots = ledger.roots.entry(dbnum).or_default();
    let streak = match roots.get(&root) {
        // 同一句错误接着败才累连败——`Refused` 之后同错再败仍是那个死根（继续记 `Refused`）；
        // 错误换了说明现场变了，从 1 重新数（`Refused` 也退回 `Failed`，给它一次新的机会）。
        Some(entry) => {
            let same_failure = matches!(
                &entry.outcome,
                RootOutcome::Failed { error: last } | RootOutcome::Refused { error: last }
                    if last == &error
            );
            if same_failure {
                entry.failure_streak + 1
            } else {
                1
            }
        }
        None => 1,
    };
    let refused = streak >= DEAD_ROOT_STREAK;
    let consecutive_failures = next_consecutive_failures(roots.get(&root));
    let outcome = if refused {
        RootOutcome::Refused { error }
    } else {
        RootOutcome::Failed { error }
    };
    roots.insert(
        root,
        RootEntry {
            noun: noun.trim().to_ascii_uppercase(),
            outcome,
            source_end_sesno,
            failure_streak: streak,
            consecutive_failures: if refused { 0 } else { consecutive_failures },
            retry_at: (!refused).then(|| Instant::now() + retry_backoff(consecutive_failures)),
        },
    );
    drop(ledger);
    if refused && streak == DEAD_ROOT_STREAK {
        println!(
            "dbnum={dbnum} 生成根 {root} 同一错误连败 {streak} 次，判死放弃（Refused）：\
             不再拦 model_sesno、下一窗不进 regen，账本与 /health 按「已放弃」并排陈述"
        );
    }
    refused
}

/// 这一根这一版没成，但失败不是「它自己坏了」的证据——刷盘预算用尽、房间算不出或计算器
/// 冻不出、整批生成报错、初始化门 / 依赖库入库这类可重试的情形：记 `Failed`（下一窗重排、拦水位），
/// 连败清零。已放弃的根撞上它也退回「还在赶」。一阵基础设施故障不许把整批根一起判死——
/// 判死的根会被后面窗口的水位担保过去。退避照样走（ADR-090 实施约束 2）：判不死，但也不必每一轮都撞。
pub fn record_transient_failure(
    dbnum: u32,
    root: RefnoEnum,
    noun: &str,
    source_end_sesno: i32,
    error: &str,
) {
    let mut ledger = write();
    let roots = ledger.roots.entry(dbnum).or_default();
    let consecutive_failures = next_consecutive_failures(roots.get(&root));
    roots.insert(
        root,
        RootEntry {
            noun: noun.trim().to_ascii_uppercase(),
            outcome: RootOutcome::Failed {
                error: truncate(error),
            },
            source_end_sesno,
            failure_streak: 0,
            consecutive_failures,
            retry_at: Some(Instant::now() + retry_backoff(consecutive_failures)),
        },
    );
}

/// 这一根永久放弃（`http_422` 一类，服务端判它解析不出合法生成根）。
pub fn record_refused(dbnum: u32, root: RefnoEnum, noun: &str, source_end_sesno: i32, error: &str) {
    record(
        dbnum,
        root,
        RootEntry {
            noun: noun.trim().to_ascii_uppercase(),
            outcome: RootOutcome::Refused {
                error: truncate(error),
            },
            source_end_sesno,
            failure_streak: 0,
            consecutive_failures: 0,
            retry_at: None,
        },
    );
}

/// 文本形式的根（`a/b`）的三个写点；解析不出来只记日志。
pub fn record_generated_str(dbnum: u32, root: &str, noun: &str, source_end_sesno: i32) {
    match parse_root(root) {
        Some(refno) => record_generated(dbnum, refno, noun, source_end_sesno),
        None => log::warn!("model root ledger: `{root}` is not an a/b refno; success not recorded"),
    }
}

pub fn record_failed_str(
    dbnum: u32,
    root: &str,
    noun: &str,
    source_end_sesno: i32,
    error: &str,
) -> bool {
    match parse_root(root) {
        Some(refno) => record_failed(dbnum, refno, noun, source_end_sesno, error),
        None => {
            log::warn!("model root ledger: `{root}` is not an a/b refno; failure not recorded");
            false
        }
    }
}

/// 这一根不再是根 / 已被删掉：从账本里摘掉。
pub fn forget_root(dbnum: u32, root: RefnoEnum) {
    let mut ledger = write();
    if let Some(roots) = ledger.roots.get_mut(&dbnum) {
        roots.remove(&root);
        if roots.is_empty() {
            ledger.roots.remove(&dbnum);
        }
    }
}

/// 把一条记录原样放回去：调用方走了一条顺手摘账本的删法（`delete_model_subtree`），删完这根还得
/// 是原来的结局——判死撤旧模型之后它仍是「已放弃」。
pub(crate) fn restore(dbnum: u32, root: RefnoEnum, entry: RootEntry) {
    record(dbnum, root, entry);
}

/// 整库清空 / 回退 / 重初始化：这个库的账本整页作废。
pub fn forget_dbnum(dbnum: u32) {
    write().roots.remove(&dbnum);
}

/// 一根的当前记录。
pub fn entry(dbnum: u32, root: RefnoEnum) -> Option<RootEntry> {
    read()
        .roots
        .get(&dbnum)
        .and_then(|roots| roots.get(&root))
        .cloned()
}

/// 这个库还欠着的根（失败且未放弃）：规划器把它们并进本窗 regen。
pub fn chasing_roots(dbnum: u32) -> BTreeSet<RefnoEnum> {
    read()
        .roots
        .get(&dbnum)
        .map(|roots| {
            roots
                .iter()
                .filter(|(_, entry)| entry.is_chasing())
                .map(|(root, _)| *root)
                .collect()
        })
        .unwrap_or_default()
}

/// 这个库的失败根连同 noun（启动 / ModelOnly 重排要 noun 组工作项）。
pub fn chasing_entries(dbnum: u32) -> Vec<(RefnoEnum, RootEntry)> {
    read()
        .roots
        .get(&dbnum)
        .map(|roots| {
            roots
                .iter()
                .filter(|(_, entry)| entry.is_chasing())
                .map(|(root, entry)| (*root, entry.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// 一根这会儿该不该重算的回答：[`deferral`] 给的「还在退避」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deferral {
    pub remaining: Duration,
    pub consecutive_failures: u32,
    pub error: String,
}

/// 这一根这会儿是不是还在退避（GitHub #72）：还在赶、退避期没过，且这次要算的源不比失败那一版新——
/// 窗口右端前进（`source_end_sesno` 大于失败时记下的那一版）说明数据又变了，立刻重算，与数据批次
/// 「右端前进即复活」同一条规矩。失败时没记源版本（`0`）的根只看退避期。
///
/// 退避中的根照样留在计划里、照样拦水位：只有任务的单元执行体按这一句跳过生成、单元记失败，
/// 这一窗就不会被判追平——退避只省重算，不替水位担保什么。
pub fn deferral(dbnum: u32, root: RefnoEnum, source_end_sesno: i32, now: Instant) -> Option<Deferral> {
    let ledger = read();
    let entry = ledger.roots.get(&dbnum)?.get(&root)?;
    if !entry.backing_off(now) {
        return None;
    }
    if entry.source_end_sesno > 0 && source_end_sesno > entry.source_end_sesno {
        return None;
    }
    Some(Deferral {
        remaining: entry
            .retry_at
            .map_or(Duration::ZERO, |retry_at| retry_at.saturating_duration_since(now)),
        consecutive_failures: entry.consecutive_failures,
        error: entry.error().unwrap_or_default().to_owned(),
    })
}

/// `/health.model_root_backoff`：`null` = 没有根在退避；否则 `backing_off` 是总数，`samples` 点名
/// 最快到期的几根（库号、根、还剩几秒、连败几次、上次的错）。
pub fn backoff_snapshot() -> serde_json::Value {
    let now = Instant::now();
    let ledger = read();
    let mut backing_off = Vec::new();
    for (dbnum, roots) in &ledger.roots {
        for (root, entry) in roots {
            if entry.backing_off(now) {
                let remaining = entry
                    .retry_at
                    .map_or(Duration::ZERO, |retry_at| retry_at.saturating_duration_since(now));
                backing_off.push((remaining, *dbnum, *root, entry));
            }
        }
    }
    if backing_off.is_empty() {
        return serde_json::Value::Null;
    }
    backing_off.sort_by_key(|(remaining, ..)| *remaining);
    let samples = backing_off
        .iter()
        .take(8)
        .map(|(remaining, dbnum, root, entry)| {
            serde_json::json!({
                "dbnum": dbnum,
                "root": root.to_string(),
                "retry_in_secs": remaining.as_secs(),
                "consecutive_failures": entry.consecutive_failures,
                "error": entry.error().map(|error| error.chars().take(200).collect::<String>()),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "backing_off": backing_off.len(),
        "samples": samples,
    })
}

/// 一个库的实时普查。
pub fn census(dbnum: u32) -> LedgerCensus {
    read()
        .roots
        .get(&dbnum)
        .map(|roots| census_of(roots.values()))
        .unwrap_or_default()
}

/// 全部库的实时普查（`/health` / `/dbnums` 聚合用）。
pub fn census_all() -> BTreeMap<u32, LedgerCensus> {
    read()
        .roots
        .iter()
        .map(|(dbnum, roots)| (*dbnum, census_of(roots.values())))
        .collect()
}

fn census_of<'a>(entries: impl Iterator<Item = &'a RootEntry>) -> LedgerCensus {
    let mut census = LedgerCensus::default();
    for entry in entries {
        match entry.outcome {
            RootOutcome::Generated => census.settled += 1,
            RootOutcome::Failed { .. } => census.chasing += 1,
            RootOutcome::Refused { .. } => census.abandoned += 1,
        }
    }
    census
}

fn truncate(error: &str) -> String {
    error.chars().take(2000).collect()
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    write().roots.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::RefU64;

    fn r(id: u32) -> RefnoEnum {
        RefU64::from_two_nums(24384, id).into()
    }

    /// 账本是进程全局的：测试各用自己的 dbnum，别互相踩。
    #[test]
    fn a_failed_root_chases_until_it_is_generated_and_refused_roots_do_not_chase() {
        let dbnum = 990_001;
        forget_dbnum(dbnum);
        record_failed(dbnum, r(1), " bran ", 26, "boom");
        record_refused(dbnum, r(2), "PANE", 26, "no root");
        record_generated(dbnum, r(3), "EQUI", 26);

        assert_eq!(chasing_roots(dbnum), BTreeSet::from([r(1)]));
        assert_eq!(
            census(dbnum),
            LedgerCensus {
                settled: 1,
                chasing: 1,
                abandoned: 1,
            }
        );
        let failed = entry(dbnum, r(1)).expect("entry");
        assert_eq!(failed.noun, "BRAN", "noun 归一成大写去空白");
        assert_eq!(failed.status_label(), "failed");
        assert_eq!(failed.error(), Some("boom"));
        assert_eq!(entry(dbnum, r(2)).unwrap().status_label(), "http_422");

        record_generated(dbnum, r(1), "BRAN", 27);
        assert!(chasing_roots(dbnum).is_empty(), "生成成了就不再欠");
        assert_eq!(census(dbnum).settled, 2);

        forget_root(dbnum, r(3));
        assert!(entry(dbnum, r(3)).is_none());
        forget_dbnum(dbnum);
        assert_eq!(census(dbnum), LedgerCensus::default());
        assert!(!census_all().contains_key(&dbnum));
    }

    /// ISSUE-029：同一根同一句错误连败 [`DEAD_ROOT_STREAK`] 次判死——`Failed` 转 `Refused`、
    /// 不再在赶、不拦水位；错误换了重新数；生成成功清零；判死后同错再败仍是 `Refused`。
    #[test]
    fn the_same_error_five_times_in_a_row_turns_a_root_into_refused() {
        let dbnum = 990_004;
        forget_dbnum(dbnum);
        for turn in 1..DEAD_ROOT_STREAK {
            assert!(
                !record_failed(
                    dbnum,
                    r(1),
                    "BRAN",
                    26,
                    "db 7004 会话 3 的索引里没有 15196/40431"
                ),
                "第 {turn} 次还没到判死线"
            );
            assert!(entry(dbnum, r(1)).unwrap().is_chasing());
            assert_eq!(
                entry(dbnum, r(1)).unwrap().failure_streak,
                turn,
                "连败按次累计"
            );
        }
        assert!(
            record_failed(
                dbnum,
                r(1),
                "BRAN",
                26,
                "db 7004 会话 3 的索引里没有 15196/40431"
            ),
            "第 {DEAD_ROOT_STREAK} 次判死"
        );
        let dead = entry(dbnum, r(1)).unwrap();
        assert!(dead.is_refused());
        assert!(!dead.is_chasing());
        assert!(chasing_roots(dbnum).is_empty(), "判死的根不进 regen");
        assert_eq!(census(dbnum).abandoned, 1);

        // 判死后同一句错误再败（按需 ensure 之类又触到它）：仍是那个死根，不复活回 chasing。
        assert!(record_failed(
            dbnum,
            r(1),
            "BRAN",
            26,
            "db 7004 会话 3 的索引里没有 15196/40431"
        ));
        assert!(entry(dbnum, r(1)).unwrap().is_refused());

        // 另一根：错误换了 = 现场在变，重新数。
        record_failed(dbnum, r(2), "BRAN", 26, "error-a");
        record_failed(dbnum, r(2), "BRAN", 26, "error-b");
        assert_eq!(entry(dbnum, r(2)).unwrap().failure_streak, 1);
        assert!(entry(dbnum, r(2)).unwrap().is_chasing());

        // 已放弃的根带着不同的错回来（数据被改过、重新进窗）：退回 chasing 重新计。
        assert!(!record_failed(dbnum, r(1), "BRAN", 27, "error-c"));
        assert!(entry(dbnum, r(1)).unwrap().is_chasing());
        assert_eq!(entry(dbnum, r(1)).unwrap().failure_streak, 1);

        forget_dbnum(dbnum);
    }

    /// 与这根无关的失败（刷盘、房间、整批、可重试）照样记「还在赶」、下一窗重排，但不攒连败：
    /// 再多次也判不死，夹在确定性失败中间还把连败清零；已放弃的根撞上它退回「还在赶」。
    #[test]
    fn failures_that_are_not_the_roots_fault_never_reach_the_death_line() {
        let dbnum = 990_005;
        forget_dbnum(dbnum);
        for _ in 0..DEAD_ROOT_STREAK * 2 {
            record_transient_failure(dbnum, r(1), "BRAN", 26, "房间计算器冻不出");
        }
        let entry_1 = entry(dbnum, r(1)).unwrap();
        assert!(entry_1.is_chasing());
        assert_eq!(entry_1.failure_streak, 0);
        assert_eq!(chasing_roots(dbnum), BTreeSet::from([r(1)]));

        for _ in 1..DEAD_ROOT_STREAK {
            record_failed(dbnum, r(2), "BRAN", 26, "manifold boolean failed");
        }
        record_transient_failure(dbnum, r(2), "BRAN", 26, "write-behind exhausted");
        assert!(
            !record_failed(dbnum, r(2), "BRAN", 26, "manifold boolean failed"),
            "中间夹了一次无关失败，连败从 1 重数"
        );
        assert_eq!(entry(dbnum, r(2)).unwrap().failure_streak, 1);

        for _ in 0..DEAD_ROOT_STREAK {
            record_failed(dbnum, r(3), "BRAN", 26, "manifold boolean failed");
        }
        assert!(entry(dbnum, r(3)).unwrap().is_refused());
        record_transient_failure(dbnum, r(3), "BRAN", 27, "write-behind exhausted");
        assert!(entry(dbnum, r(3)).unwrap().is_chasing());
        forget_dbnum(dbnum);
    }

    /// 判死撤旧模型走的删法会摘账本：原样记回去之后，它还是那个「已放弃」，连败与错误一字不差。
    #[test]
    fn a_restored_entry_comes_back_exactly_as_it_was() {
        let dbnum = 990_006;
        forget_dbnum(dbnum);
        for _ in 0..DEAD_ROOT_STREAK {
            record_failed(dbnum, r(1), "BRAN", 26, "manifold boolean failed");
        }
        let dead = entry(dbnum, r(1)).unwrap();
        forget_root(dbnum, r(1));
        restore(dbnum, r(1), dead.clone());
        assert_eq!(entry(dbnum, r(1)), Some(dead));
        assert_eq!(census(dbnum).abandoned, 1);
        forget_dbnum(dbnum);
    }

    #[test]
    fn text_roots_accept_both_spellings_and_ignore_garbage() {
        let dbnum = 990_002;
        forget_dbnum(dbnum);
        record_failed_str(dbnum, "24384/7", "BRAN", 1, "x");
        record_generated_str(dbnum, "=24384_8", "EQUI", 1);
        record_failed_str(dbnum, "not-a-refno", "EQUI", 1, "x");
        assert_eq!(chasing_roots(dbnum), BTreeSet::from([r(7)]));
        assert!(entry(dbnum, r(8)).unwrap().is_settled());
        assert_eq!(census(dbnum).settled + census(dbnum).chasing, 2);
        forget_dbnum(dbnum);
    }

    /// GitHub #72 / ADR-090 D1：退避 30 s 起、翻倍、封顶 10 min，与 `RebuildBackoff` 同档。
    #[test]
    fn retry_backoff_doubles_from_thirty_seconds_and_caps_at_ten_minutes() {
        assert_eq!(retry_backoff(0), Duration::ZERO);
        assert_eq!(retry_backoff(1), Duration::from_secs(30));
        assert_eq!(retry_backoff(2), Duration::from_secs(60));
        assert_eq!(retry_backoff(3), Duration::from_secs(120));
        assert_eq!(retry_backoff(5), Duration::from_secs(480));
        assert_eq!(retry_backoff(6), RETRY_BACKOFF_CAP);
        assert_eq!(retry_backoff(40), RETRY_BACKOFF_CAP);
    }

    /// 退避中的根仍在赶、仍进计划（拦水位），只是这会儿不重算；右端前进或到期即放行；
    /// 不攒连败的失败也退避；生成成功清掉退避；判死的根不退避。
    #[test]
    fn a_failed_root_backs_off_until_it_expires_or_newer_source_arrives() {
        let dbnum = 990_007;
        forget_dbnum(dbnum);
        record_failed(dbnum, r(1), "BRAN", 26, "boom");
        let now = Instant::now();
        let first = deferral(dbnum, r(1), 26, now).expect("刚失败就退避");
        assert_eq!(first.consecutive_failures, 1);
        assert_eq!(first.error, "boom");
        assert!(
            first.remaining <= RETRY_BACKOFF_BASE && first.remaining > Duration::from_secs(25),
            "{:?}",
            first.remaining
        );
        assert!(entry(dbnum, r(1)).unwrap().is_chasing(), "退避中仍在赶、拦水位");
        assert_eq!(chasing_roots(dbnum), BTreeSet::from([r(1)]), "规划器照样把它并进计划");
        assert!(
            deferral(dbnum, r(1), 27, now).is_none(),
            "右端前进 = 数据又变了，立刻重算"
        );
        assert!(
            deferral(dbnum, r(1), 26, now + RETRY_BACKOFF_BASE + Duration::from_secs(1)).is_none(),
            "到期就放行"
        );
        assert!(!backoff_snapshot().is_null(), "/health 点得出退避中的根");

        record_transient_failure(dbnum, r(1), "BRAN", 26, "write-behind exhausted");
        let second = deferral(dbnum, r(1), 26, Instant::now()).expect("无关失败也退避");
        assert_eq!(second.consecutive_failures, 2);
        assert!(second.remaining > Duration::from_secs(55), "{:?}", second.remaining);
        assert_eq!(entry(dbnum, r(1)).unwrap().failure_streak, 0, "但不攒连败");

        record_generated(dbnum, r(1), "BRAN", 27);
        assert!(deferral(dbnum, r(1), 27, Instant::now()).is_none());
        assert_eq!(entry(dbnum, r(1)).unwrap().consecutive_failures, 0);

        record_failed(dbnum, r(2), "BRAN", 0, "boom");
        assert!(
            deferral(dbnum, r(2), 99, Instant::now()).is_some(),
            "失败时没记源版本：只看退避期"
        );

        for _ in 0..DEAD_ROOT_STREAK {
            record_failed(dbnum, r(3), "BRAN", 26, "dead");
        }
        let dead = entry(dbnum, r(3)).unwrap();
        assert!(dead.is_refused());
        assert_eq!(dead.retry_at, None, "判死的根不在赶，也就不退避");
        assert!(deferral(dbnum, r(3), 26, Instant::now()).is_none());
        forget_dbnum(dbnum);
    }

    #[test]
    fn errors_are_truncated_like_the_retired_column() {
        let dbnum = 990_003;
        forget_dbnum(dbnum);
        let long = "e".repeat(5000);
        record_failed(dbnum, r(1), "BRAN", 1, &long);
        assert_eq!(entry(dbnum, r(1)).unwrap().error().unwrap().len(), 2000);
        forget_dbnum(dbnum);
    }
}
