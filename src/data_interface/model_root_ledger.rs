//! 进程内的生成根账本（ADR-079）：`gen_root` 表退役之后，「这个库的哪些根这一版生成成了、
//! 哪些失败了、哪些永久放弃」只活在这里。
//!
//! 它**不是**第二份真值。库里模型面唯一的持久事实是 `dbnum_watermark.model_sesno`
//! （ADR-060：被写下的承诺，任务收尾一笔盖）；账本只是这一个进程在两次盖章之间对逐根成败的
//! 工作记录——下一窗要把失败的根并进 regen（`window_root_plan::carry_unsettled_generation`）、
//! 一致性判决要数「还在赶 / 已放弃」（`dbnum_consistency`）、`/model/ensure` 要回一句发布状态，
//! 都读它。同一根同一错误连败 [`DEAD_ROOT_STREAK`] 次判死转「已放弃」：不拦水位、不进 regen，
//! 与「已追平」并排陈述（ISSUE-029：确定性失败要有终态，不能拖着整个库的水位永远追不上）。
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

use aios_core::RefnoEnum;

/// 同一根带着同一句错误连续失败这么多次之后判死（ISSUE-029）：`Failed` 转 `Refused`，
/// 不再拦 `model_sesno`、下一窗也不再并进 regen。「同一错误」才算连败——错误变了说明
/// 现场在动，重新数；数值沿用退役前 `model_update_pending::MAX_ATTEMPTS` 的 5。
pub(crate) const DEAD_ROOT_STREAK: u32 = 5;

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
}

impl RootEntry {
    pub fn is_settled(&self) -> bool {
        matches!(self.outcome, RootOutcome::Generated)
    }

    /// 还在赶：失败且不是永久放弃。
    pub fn is_chasing(&self) -> bool {
        matches!(self.outcome, RootOutcome::Failed { .. })
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
        },
    );
}

/// 这一根这一版失败了：同一错误连败 [`DEAD_ROOT_STREAK`] 次判死转 `Refused`（ISSUE-029），
/// 否则记 `Failed`（下一窗重排、拦水位）。`error` 截 2000 字，与退役前 `gen_root.last_error`
/// 同一口径。返回这根记完之后是不是已放弃（`Refused`）——调用方拿它决定拦不拦水位。
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

/// 这根（文本 `a/b`）现在是不是已放弃（`Refused`）：判死的根不拦水位、不进 regen，
/// 收尾与规划各拿它当「不算失败」的那一格。解析不出 `root` 按没放弃算——那种根本来就
/// 进不了账本，拦不拦由调用方自己的判据说。
pub fn root_refused(dbnum: u32, root: &str) -> bool {
    parse_root(root)
        .and_then(|refno| entry(dbnum, refno))
        .is_some_and(|entry| entry.is_refused())
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
        assert!(root_refused(dbnum, "24384/1"));

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
