//! 按 dbnum 记「这个库的文件现在是哪一版」——路由目录的失效键（母计划不变量 7）。
//!
//! 母计划 `docs/plans/2026-09-12-refno-native-routing-and-lazy-mdb-plan.md` 不变量 7 要求「换文件、
//! 回滚和 session 前进必须能区分」；口径由 2026-09-12 对齐（跨库属性切片 §9 Q13）定下：每个 dbnum 一个
//! 单调 `u64`，旁边存上一次观察到的 `(FileIdentity, latest_sesno, base_sesno)`，身份或会话号任一变化就 +1，
//! 并说清是哪一种变化。
//!
//! 为什么不直接拿 `latest_sesno` 当 generation：同一个会话号可以出现在**另一个文件**里——目录库 reinit、
//! 外部拷贝覆盖，路径不变、文件换了，会话号恰好相同。冻住的会话号会指向另一个工程里同号的一个不相干会话，
//! 读出来的东西完全正常，只是不是你要的（`direct_store::FileIdentity` 注释里的事故）。所以身份与会话号
//! 两个都记，缺一个都分不出「换文件」。
//!
//! 为什么还要 `base_sesno`（最新会话页的 `BASESN`，e3d-io `SessionPage::tail.base_sesno`）：E3D 里 BACKTRACK 到旧会话
//! 再 SAVEWORK，会话号照涨、内容却回到旧会话那一刻，而那一页的 `REASON` 是 0——与普通保存一个样。3.1 `ams8000_0001`
//! 第 243 会话 `BASESN=239`，链上其后保留的是 242（e3d-io `tests/session_tail_real.rs`，2026-09-12）。只看会话号会把它
//! 判成「前进」，只作废 242 之后新写的东西；实际被改掉的是 239..=242 之间已经生成过的一切。所以会话号变大时还要看
//! 新页踩在谁上：踩在比上次观察到的会话号更旧的会话上，就是回滚（阶段 4 切片 §3.4 规则表）。
//!
//! 本模块**纯内存、纯函数**：不读库、不碰目录、不接任何 provider。观察值由调用方给
//! （文件身份 [`FileIdentity::of`] + 会话号与 `BASESN` 来自最新会话页），接线在母计划阶段 4/5
//! 切片（`docs/plans/2026-09-12-lazy-startup-dev-plan.md` §3.4 `MdbRouteDirectory::poll_generations`）。
//! 一次前进跨过多个会话时，中间被跳过的会话里可能藏着一次回退，新页的 `BASESN` 说不出——那要接线方沿会话链
//! 回读到上次观察到的会话号，逐页比 `BASESN`；本模块只判它拿到的那一页。

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::SystemTime;

/// 文件身份：长度 + 修改时刻。与 `direct_store::FileIdentity`、`model_source::FileStamp` 同一口径
/// （那两个是模块私有的；接线时收敛成这一份）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FileIdentity {
    pub len: u64,
    /// `None` = 文件系统给不出修改时刻；两个 `None` 互相相等，只靠长度分辨。
    #[serde(with = "system_time_millis")]
    pub modified: Option<SystemTime>,
}

impl FileIdentity {
    /// 一次 `stat`，不打开文件。
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

mod system_time_millis {
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S: serde::Serializer>(
        value: &Option<SystemTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value.and_then(|time| time.duration_since(UNIX_EPOCH).ok()) {
            Some(duration) => serializer.serialize_some(&(duration.as_millis() as u64)),
            None => serializer.serialize_none(),
        }
    }
}

/// 一次对库文件的读数：`observe` 收的就是它，账本里存的也是它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Observed {
    pub identity: FileIdentity,
    /// 最新会话页的会话号。
    pub latest_sesno: u32,
    /// 最新会话页的 `BASESN`——这一会话建立在哪个会话之上。`Some(0)` = 无（3.1 的 `Saving Meta Data`
    /// 那类系统会话）；`None` = 页上没有这一字（不足 0x7FC 字节的页），当作说不出来，不参与判定。
    pub base_sesno: Option<u32>,
}

impl Observed {
    pub fn new(identity: FileIdentity, latest_sesno: u32, base_sesno: Option<u32>) -> Self {
        Self {
            identity,
            latest_sesno,
            base_sesno,
        }
    }
}

/// 一个 dbnum 当前的世代与它背后的观察值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DbGeneration {
    /// 从 1 起单调递增；每次 [`GenerationChange`] +1。0 不会出现——没观察过的 dbnum 在账本里没有条目。
    pub generation: u64,
    pub identity: FileIdentity,
    pub latest_sesno: u32,
    /// 见 [`Observed::base_sesno`]。
    pub base_sesno: Option<u32>,
}

impl DbGeneration {
    /// 这一世代背后的读数，交给 [`classify`] 当「上一次」。
    pub fn observed(&self) -> Observed {
        Observed {
            identity: self.identity,
            latest_sesno: self.latest_sesno,
            base_sesno: self.base_sesno,
        }
    }

    fn from_observed(generation: u64, observed: Observed) -> Self {
        Self {
            generation,
            identity: observed.identity,
            latest_sesno: observed.latest_sesno,
            base_sesno: observed.base_sesno,
        }
    }
}

/// 两次观察之间发生了什么。三种互斥；判据以会话号为先，身份只在会话号不变时说话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationChange {
    /// 会话号变大，且新会话不是踩在比上次更旧的会话上：正常 `SAVEWORK`。文件身份通常也变，但那不是判据。
    SessionAdvanced,
    /// 内容回到了更旧的会话，两种形状：会话号变小（文件被整份恢复 / 换成旧备份）；会话号变大但新页的
    /// `BASESN` 低于上次观察到的会话号（BACKTRACK 后 SAVEWORK）。已生成的 roots 全部按新会话重生成，
    /// 不能拿旧凭证当覆盖。
    RolledBack,
    /// 会话号不变、文件身份变了：路径上换了另一个文件。冻住的 `sesno` 从此指向另一个文件里的同号会话，
    /// 必须清空该 dbnum 的一切缓存与 pin。
    FileReplaced,
}

impl fmt::Display for GenerationChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SessionAdvanced => "session_advanced",
            Self::RolledBack => "rolled_back",
            Self::FileReplaced => "file_replaced",
        })
    }
}

/// 一次观察的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// 第一次见到这个 dbnum：世代 1，不算变化（没有「从哪来」）。
    First(DbGeneration),
    /// 身份与会话号都没变。
    Unchanged(DbGeneration),
    /// 变了：`from` 是上一版，`to` 已经 +1。
    Changed {
        from: DbGeneration,
        to: DbGeneration,
        kind: GenerationChange,
    },
}

impl Observation {
    /// 观察后的当前世代，不管变没变。
    pub fn current(&self) -> DbGeneration {
        match self {
            Self::First(current) | Self::Unchanged(current) => *current,
            Self::Changed { to, .. } => *to,
        }
    }

    pub fn change(&self) -> Option<GenerationChange> {
        match self {
            Self::Changed { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

/// 把一次 `(旧, 新)` 观察分成三类。`None` = 没变。规则表（阶段 4 切片 §3.4）：
///
/// | 观察到 | 判定 |
/// |---|---|
/// | 会话号变小 | 回滚（文件恢复） |
/// | 会话号变大，新页 `base_sesno`（非 0）< 上次会话号 | 回滚（BACKTRACK 后 SAVEWORK） |
/// | 会话号变大，`base_sesno` ≥ 上次会话号、为 0 或页上没有 | 前进 |
/// | 会话号不变，身份变 | 换文件 |
/// | 都不变 | 无变化 |
///
/// 会话号先说话，身份只在会话号不变时说话；`base_sesno` 只在会话号变大时说话。
pub fn classify(previous: Observed, current: Observed) -> Option<GenerationChange> {
    match current.latest_sesno.cmp(&previous.latest_sesno) {
        std::cmp::Ordering::Greater => {
            let based_below_last_seen = matches!(
                current.base_sesno,
                Some(base) if base != 0 && base < previous.latest_sesno
            );
            Some(if based_below_last_seen {
                GenerationChange::RolledBack
            } else {
                GenerationChange::SessionAdvanced
            })
        }
        std::cmp::Ordering::Less => Some(GenerationChange::RolledBack),
        std::cmp::Ordering::Equal if current.identity != previous.identity => {
            Some(GenerationChange::FileReplaced)
        }
        std::cmp::Ordering::Equal => None,
    }
}

/// 全部 dbnum 的世代账本。按 dbnum 键，互不影响：改 8000 不动 7998（跨库属性切片 §9 Q10 的验收）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationLedger {
    entries: BTreeMap<u32, DbGeneration>,
}

impl GenerationLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次只有身份与会话号的观察（页上的 `BASESN` 没读或没有）。
    ///
    /// 走的是同一张规则表，只是第二行（会话号变大但踩在更旧的会话上 = 回滚）永远判不出来——
    /// 拿得到最新会话页的接线方应改用 [`Self::observe_session`] 把 `BASESN` 一起交进来。
    pub fn observe(
        &mut self,
        dbnum: u32,
        identity: FileIdentity,
        latest_sesno: u32,
    ) -> Observation {
        self.observe_session(dbnum, Observed::new(identity, latest_sesno, None))
    }

    /// 记一次观察。第一次见 → 世代 1；之后按 [`classify`] 决定要不要 +1。
    pub fn observe_session(&mut self, dbnum: u32, observed: Observed) -> Observation {
        match self.entries.get(&dbnum).copied() {
            None => {
                let first = DbGeneration::from_observed(1, observed);
                self.entries.insert(dbnum, first);
                Observation::First(first)
            }
            Some(previous) => match classify(previous.observed(), observed) {
                None => Observation::Unchanged(previous),
                Some(kind) => {
                    let next = DbGeneration::from_observed(previous.generation + 1, observed);
                    self.entries.insert(dbnum, next);
                    Observation::Changed {
                        from: previous,
                        to: next,
                        kind,
                    }
                }
            },
        }
    }

    pub fn get(&self, dbnum: u32) -> Option<DbGeneration> {
        self.entries.get(&dbnum).copied()
    }

    /// 某库当前世代号；没观察过 → `None`（不是 0——0 会被当成「比 1 旧」）。
    pub fn generation_of(&self, dbnum: u32) -> Option<u64> {
        self.get(dbnum).map(|entry| entry.generation)
    }

    /// 全部世代之和：`/health.core.snapshot_epoch` 的新来源（阶段 4 切片 §8-3）。任何一个库变了它就变；
    /// 两个库同时变一次与一个库变两次得到同一个数——它只回答「变了没」，不回答「谁变了」，谁变了看
    /// [`Self::observe`] 的返回。
    pub fn epoch(&self) -> u64 {
        self.entries.values().map(|entry| entry.generation).sum()
    }

    /// 从账本里去掉一个 dbnum（MDB 成员名单不再声明它）。下次再见按第一次算。
    pub fn forget(&mut self, dbnum: u32) -> Option<DbGeneration> {
        self.entries.remove(&dbnum)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &DbGeneration)> {
        self.entries.iter().map(|(dbnum, entry)| (*dbnum, entry))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 最近一次按 dbnum 的定向刷新（阶段 4 切片 §3.4 (d)）：谁变了、怎么变的、动了几根、结果如何。
/// 进程内只留最后一笔，给 `/health.core.last_targeted_refresh`（L6 接线）与验收脚本
/// 「8000 刷了、7998 没动」那一条看。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TargetedRefresh {
    pub dbnum: u32,
    pub db_type: String,
    pub kind: GenerationChange,
    pub from: DbGeneration,
    pub to: DbGeneration,
    /// 该库此前已发布的根数。
    pub roots_seen: usize,
    /// 新会话索引里还在、已排去重生成的根数。
    pub roots_regenerated: usize,
    /// 新会话索引里没了、已删子树的根数。
    pub roots_retired: usize,
    /// 只作废了这个库的发布回执数。
    pub receipts_invalidated: usize,
    /// `"ok"`，或哪一步没做成（下一次 ensure 仍会按新会话重比，不会静默）。
    pub outcome: String,
    pub at_unix_ms: u64,
}

static LAST_TARGETED_REFRESH: std::sync::RwLock<Option<TargetedRefresh>> =
    std::sync::RwLock::new(None);

/// 记下最近一笔定向刷新（覆盖上一笔）。
pub fn record_targeted_refresh(refresh: TargetedRefresh) {
    if let Ok(mut slot) = LAST_TARGETED_REFRESH.write() {
        *slot = Some(refresh);
    }
}

/// 最近一笔定向刷新；进程起来后还没有任何库变过 → `None`。
pub fn last_targeted_refresh() -> Option<TargetedRefresh> {
    LAST_TARGETED_REFRESH
        .read()
        .ok()
        .and_then(|slot| slot.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn identity(len: u64, secs: u64) -> FileIdentity {
        FileIdentity {
            len,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    /// 一次读数，页上没说 `BASESN`（短页）——只看身份与会话号的那几条规则用它。
    fn at(identity: FileIdentity, latest_sesno: u32) -> Observed {
        Observed::new(identity, latest_sesno, None)
    }

    /// 一次读数，最新会话页说了它踩在哪个会话上。
    fn based(identity: FileIdentity, latest_sesno: u32, base_sesno: u32) -> Observed {
        Observed::new(identity, latest_sesno, Some(base_sesno))
    }

    #[test]
    fn first_observation_is_generation_one_and_not_a_change() {
        let mut ledger = GenerationLedger::new();
        let seen = ledger.observe_session(8000, at(identity(4096, 100), 37));
        assert_eq!(
            seen,
            Observation::First(DbGeneration {
                generation: 1,
                identity: identity(4096, 100),
                latest_sesno: 37,
                base_sesno: None,
            })
        );
        assert_eq!(seen.change(), None);
        assert_eq!(ledger.generation_of(8000), Some(1));
        assert_eq!(
            ledger.generation_of(7998),
            None,
            "没观察过就是 None，不是 0"
        );
    }

    #[test]
    fn same_identity_and_session_is_unchanged() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        let seen = ledger.observe_session(8000, at(identity(4096, 100), 37));
        assert!(matches!(seen, Observation::Unchanged(current) if current.generation == 1));
        assert_eq!(ledger.epoch(), 1);
    }

    /// 正常 SAVEWORK：文件长了、时刻变了、会话号 +1 —— 判「前进」，身份变化不参与判定。
    #[test]
    fn a_newer_session_is_an_advance_whatever_the_identity_did() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        let seen = ledger.observe_session(8000, at(identity(6144, 200), 38));
        match seen {
            Observation::Changed { from, to, kind } => {
                assert_eq!(kind, GenerationChange::SessionAdvanced);
                assert_eq!(from.generation, 1);
                assert_eq!(to.generation, 2);
                assert_eq!(to.latest_sesno, 38);
                assert_eq!(to.identity, identity(6144, 200));
            }
            other => panic!("expected SessionAdvanced, got {other:?}"),
        }
        // 会话号变大而身份没变（理论上不会发生）也仍是前进，不是换文件。
        let seen = ledger.observe_session(8000, at(identity(6144, 200), 39));
        assert_eq!(seen.change(), Some(GenerationChange::SessionAdvanced));
        assert_eq!(ledger.generation_of(8000), Some(3));
    }

    /// 恢复旧备份：会话号变小。已生成的 roots 不能拿旧凭证当覆盖。
    #[test]
    fn an_older_session_is_a_rollback() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(identity(6144, 200), 38));
        let seen = ledger.observe_session(8000, at(identity(4096, 100), 37));
        assert_eq!(seen.change(), Some(GenerationChange::RolledBack));
        assert_eq!(seen.current().generation, 2);
        assert_eq!(seen.current().latest_sesno, 37);
    }

    /// BACKTRACK 后 SAVEWORK：会话号照涨，新页却踩在比上次观察到的会话更旧的会话上——3.1 `ams8000_0001`
    /// 的第 243 会话（`BASESN=239`，其后保留 242，`REASON=0`）。只看会话号会判成前进，这里必须是回滚。
    #[test]
    fn a_newer_session_based_below_the_last_seen_one_is_a_rollback() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, based(identity(4096, 100), 242, 241));
        let seen = ledger.observe_session(8000, based(identity(6144, 200), 243, 239));
        match seen {
            Observation::Changed { from, to, kind } => {
                assert_eq!(kind, GenerationChange::RolledBack);
                assert_eq!((from.latest_sesno, from.base_sesno), (242, Some(241)));
                assert_eq!((to.latest_sesno, to.base_sesno), (243, Some(239)));
                assert_eq!(to.generation, 2);
            }
            other => panic!("expected RolledBack, got {other:?}"),
        }

        // 踩在上次看到的那一个上（直叠）或更新的上（中间跨了没看见的会话）都是前进；
        // 上次是 239、这次 243 踩 239，对本账本而言就是从 239 往前走。
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, based(identity(4096, 100), 239, 238));
        assert_eq!(
            ledger
                .observe_session(8000, based(identity(6144, 200), 243, 239))
                .change(),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            ledger
                .observe_session(8000, based(identity(7000, 300), 250, 247))
                .change(),
            Some(GenerationChange::SessionAdvanced)
        );
    }

    /// 不带 BASESN 的 `observe` 与 `observe_session(.., None)` 是同一条路：`mdb_route_directory::poll_generations`
    /// 现在走它，看不见 BACKTRACK 后的那种回滚——那是接线方换成 `observe_session` 的理由，不是账本的判错。
    #[test]
    fn observe_without_a_base_is_observe_session_with_none() {
        let mut ledger = GenerationLedger::new();
        let first = ledger.observe(8000, identity(4096, 100), 242);
        assert_eq!(first.current().base_sesno, None);
        let seen = ledger.observe(8000, identity(6144, 200), 243);
        assert_eq!(
            seen.change(),
            Some(GenerationChange::SessionAdvanced),
            "没有 BASESN 就只能按会话号判前进"
        );
        let mut with_base = GenerationLedger::new();
        with_base.observe_session(8000, at(identity(4096, 100), 242));
        assert_eq!(
            with_base.observe_session(8000, at(identity(6144, 200), 243)),
            seen
        );
    }

    /// 3.1 的 `Saving Meta Data` 会话 `BASESN=0`（踩在谁上没说），短页干脆没有这一字（`None`）：
    /// 两种都不参与判定，会话号变大就是前进。
    #[test]
    fn a_newer_session_based_on_nothing_is_an_advance() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, based(identity(4096, 100), 443, 442));
        assert_eq!(
            ledger
                .observe_session(8000, based(identity(6144, 200), 444, 0))
                .change(),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            ledger
                .observe_session(8000, at(identity(7000, 300), 445))
                .change(),
            Some(GenerationChange::SessionAdvanced)
        );
        // 会话号变小时 BASESN 说什么都不改判：文件被整份恢复。
        assert_eq!(
            ledger
                .observe_session(8000, based(identity(1000, 10), 400, 399))
                .change(),
            Some(GenerationChange::RolledBack)
        );
    }

    /// 路径不变、文件换了、会话号恰好相同：这是 `latest_sesno` 单独当键分不出来的那一种。
    #[test]
    fn same_session_in_another_file_is_a_replacement() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        let by_length = ledger.observe_session(8000, at(identity(8192, 100), 37));
        assert_eq!(by_length.change(), Some(GenerationChange::FileReplaced));
        let by_mtime = ledger.observe_session(8000, at(identity(8192, 300), 37));
        assert_eq!(by_mtime.change(), Some(GenerationChange::FileReplaced));
        assert_eq!(ledger.generation_of(8000), Some(3));
    }

    /// `modified` 拿不到时（`None`）只靠长度：两个 `None` 相等，长度不变就是没变。
    #[test]
    fn missing_mtime_falls_back_to_length_only() {
        let no_mtime = |len| FileIdentity {
            len,
            modified: None,
        };
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(no_mtime(4096), 37));
        assert!(matches!(
            ledger.observe_session(8000, at(no_mtime(4096), 37)),
            Observation::Unchanged(_)
        ));
        assert_eq!(
            ledger
                .observe_session(8000, at(no_mtime(4097), 37))
                .change(),
            Some(GenerationChange::FileReplaced)
        );
    }

    /// 跨库属性切片 §9 Q10 的验收形状：8000 保存了，7998 的世代一动不动。
    #[test]
    fn dbnums_are_independent() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(7998, at(identity(1000, 10), 5));
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        let before_7998 = ledger.get(7998).expect("7998 observed");

        ledger.observe_session(8000, at(identity(6144, 200), 38));
        ledger.observe_session(8000, at(identity(6144, 200), 38));
        ledger.observe_session(8000, at(identity(6000, 250), 38));

        assert_eq!(ledger.get(7998), Some(before_7998));
        assert_eq!(ledger.generation_of(8000), Some(3));
        assert_eq!(ledger.epoch(), 1 + 3);
        let dbnums: Vec<u32> = ledger.iter().map(|(dbnum, _)| dbnum).collect();
        assert_eq!(dbnums, vec![7998, 8000], "按 dbnum 排序，稳定可比");
    }

    #[test]
    fn epoch_moves_whenever_any_generation_moves() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(7998, at(identity(1000, 10), 5));
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        let epoch = ledger.epoch();
        ledger.observe_session(7998, at(identity(1000, 10), 5));
        assert_eq!(ledger.epoch(), epoch, "没变化 epoch 不动");
        ledger.observe_session(7998, at(identity(1200, 20), 6));
        assert_eq!(ledger.epoch(), epoch + 1);
    }

    #[test]
    fn forgetting_a_dbnum_restarts_it_from_generation_one() {
        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(identity(4096, 100), 37));
        ledger.observe_session(8000, at(identity(6144, 200), 38));
        assert_eq!(ledger.forget(8000).map(|entry| entry.generation), Some(2));
        assert!(ledger.is_empty());
        assert!(matches!(
            ledger.observe_session(8000, at(identity(6144, 200), 38)),
            Observation::First(_)
        ));
    }

    #[test]
    fn classify_is_the_single_rule_table() {
        let a = identity(1, 1);
        let b = identity(2, 2);
        assert_eq!(classify(at(a, 5), at(a, 5)), None);
        assert_eq!(
            classify(at(a, 5), at(b, 5)),
            Some(GenerationChange::FileReplaced)
        );
        assert_eq!(
            classify(at(a, 5), at(a, 6)),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            classify(at(a, 5), at(b, 6)),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            classify(at(a, 5), at(a, 4)),
            Some(GenerationChange::RolledBack)
        );
        assert_eq!(
            classify(at(a, 5), at(b, 4)),
            Some(GenerationChange::RolledBack)
        );
        // BASESN 只在会话号变大时说话：低于上次 = 回滚；等于 / 高于 / 0 / 没说 = 前进。
        assert_eq!(
            classify(at(a, 242), based(b, 243, 239)),
            Some(GenerationChange::RolledBack)
        );
        assert_eq!(
            classify(at(a, 242), based(b, 243, 242)),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            classify(at(a, 242), based(b, 250, 247)),
            Some(GenerationChange::SessionAdvanced)
        );
        assert_eq!(
            classify(at(a, 242), based(b, 243, 0)),
            Some(GenerationChange::SessionAdvanced)
        );
        // 会话号不变或变小时，BASESN 不改判。
        assert_eq!(classify(at(a, 5), based(a, 5, 1)), None);
        assert_eq!(
            classify(at(a, 5), based(b, 5, 1)),
            Some(GenerationChange::FileReplaced)
        );
        assert_eq!(
            classify(based(a, 5, 4), based(a, 4, 3)),
            Some(GenerationChange::RolledBack)
        );
    }

    /// 真文件：追加字节后身份变化；`FileIdentity::of` 只 stat 不打开。
    #[test]
    fn file_identity_tracks_a_real_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("ams8000_0001");
        std::fs::write(&path, [0_u8; 2048]).expect("write fixture");
        let before = FileIdentity::of(&path).expect("stat before");
        assert_eq!(before.len, 2048);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("reopen for append");
        std::io::Write::write_all(&mut file, &[0_u8; 2048]).expect("append a page");
        drop(file);
        let after = FileIdentity::of(&path).expect("stat after");
        assert_eq!(after.len, 4096);
        assert_ne!(before, after);

        let mut ledger = GenerationLedger::new();
        ledger.observe_session(8000, at(before, 37));
        assert_eq!(
            ledger.observe_session(8000, at(after, 37)).change(),
            Some(GenerationChange::FileReplaced),
            "同会话号、文件变了 = 换文件"
        );
    }

    #[test]
    fn serializes_for_health() {
        let entry = DbGeneration {
            generation: 3,
            identity: identity(4096, 100),
            latest_sesno: 38,
            base_sesno: Some(37),
        };
        let value = serde_json::to_value(entry).expect("serialize");
        assert_eq!(value["generation"], 3);
        assert_eq!(value["latest_sesno"], 38);
        assert_eq!(value["base_sesno"], 37);
        assert_eq!(value["identity"]["len"], 4096);
        assert_eq!(value["identity"]["modified"], 100_000);
        assert_eq!(
            serde_json::to_value(GenerationChange::FileReplaced).expect("serialize kind"),
            "file_replaced"
        );
        assert_eq!(GenerationChange::RolledBack.to_string(), "rolled_back");
    }
}
