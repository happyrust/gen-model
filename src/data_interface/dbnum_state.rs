//! DbnumState — authoritative per-`dbnum` incremental state (see ADR-001).
//!
//! One row per `dbnum`, physically the existing `dbnum_watermark:{dbnum}` record,
//! extended with file identity + scan-observation fields. The row carries **two**
//! watermarks with different write points (ADR-071 D4, ADR-076 D5): `applied_sesno`
//! is written by the data window's tail transaction, `model_sesno` by the task-end
//! commit (only when every root generated and serialized successfully and
//! `T <= applied_sesno`). Neither may be substituted for the other, and
//! `file_latest_sesno` is a scan observation that must never substitute for either.
//!
//! Read semantics (ADR-001 §兼容迁移):
//! 1. Prefer an already-established `applied_sesno`.
//! 2. Otherwise inherit the legacy `dbnum_watermark.sesno`.
//! 3. Until the pe-based compatibility seeding has completed once on this
//!    database (durable `queue_control:watermark_seed` marker; also covers a
//!    missing or empty watermark table), establish each dbnum from the max
//!    persisted `pe.sesno`. Fill-only, so reruns never overwrite established
//!    watermarks.
//! 4. Otherwise (only when a dedicated row is absent) fall back once to the max
//!    `sesno` in `dbnum_info_table` for this `dbnum`.
//!
//! After the state is established (a scan / advance writes `applied_sesno`), reads
//! use `applied_sesno` directly and never re-mix other sources.

use std::collections::{BTreeMap, BTreeSet};

use aios_core::SUL_DB;
use serde::{Deserialize, Serialize};

/// Authoritative per-`dbnum` state table (extends the legacy watermark record).
pub const WATERMARK_TABLE: &str = "dbnum_watermark";
/// Legacy per-`ref_0` element-statistics table, used only for one-time migration.
pub const INFO_TABLE: &str = "dbnum_info_table";

/// Serializes scan-observation writes against an in-process fast DBNUM delete.
/// Reads remain unrestricted; normal scans share the read side.
pub(crate) static DBNUM_STATE_WRITE_GATE: tokio::sync::RwLock<()> =
    tokio::sync::RwLock::const_new(());

/// 进程内水位镜像（共识 d-227：「内存里可以保持住水位，surreal 数据库相当于是一个落盘的
/// 序列化方案。在比对时，我们都可以使用内存里的缓存。」）。
///
/// 镜像里放的是**库里那一行的原样投影**（[`StateRow`] + `dbnum_info_table` 的旧水位），
/// 不是解析完的 [`DbnumState`]：解析要看 `task_registry` 里「该库任务在跑」这类进程态，
/// 它会在没有任何库写的情况下翻转，缓存解析结果就会把「初始化中」的判定钉死。事实缓存、
/// 判定现算，判定永远跟着进程态走。
///
/// **写者落库后作废那一格**（[`DbnumState::invalidate_mirrored`]），下一次读回填——
/// 作废而不是回填，是因为不少写者是 `_on(db)` 形态，写的库未必是 `SUL_DB`；作废对任何
/// 目标库都安全，代价只是多一次读。每格自己的 revision 与 singleflight 通知把版本检查和
/// 缓存发布放在同一个锁边界；正在飞的旧装载既不能回填，也不能把旧值交给本次比较。
///
/// 缺省**关**：`run_cli` 的统一水化段（ADR-071 D7 / S6）打开它
/// （[`DbnumState::enable_watermark_mirror`]），随即用 [`DbnumState::hydrate_watermark_mirror`]
/// 整表水化一次——排在全部播种之后、任何 worker / watcher 之前；之后每格只走「按需装载 +
/// 写后作废」。关着时每一条读路都是今天的直读，单测 / 集成测试不受影响；镜像自己的行为由
/// 本模块的单测显式开关覆盖。
mod mirror {
    use super::StateRow;
    use std::collections::BTreeMap;
    use std::sync::{Arc, LazyLock, Mutex};
    use tokio::sync::watch;

    /// 一格：库里那一行 + info 表旧水位（`None` 行也缓存——未登记的库每轮扫描都会被问到）。
    #[derive(Debug, Clone, Default)]
    pub(super) struct Facts {
        pub row: Option<StateRow>,
        pub info_max: Option<i32>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) struct Identity {
        pub endpoint: String,
        pub namespace: String,
        pub database: String,
        pub project: String,
    }

    #[derive(Debug)]
    struct Entry {
        facts: Option<Facts>,
        revision: u64,
        loading: bool,
        changed: Arc<watch::Sender<u64>>,
    }

    #[derive(Debug, Default)]
    struct State {
        enabled: bool,
        identity: Option<Identity>,
        entries: BTreeMap<(Identity, u32), Entry>,
    }

    static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

    pub(super) enum Acquire {
        Disabled,
        Hit(Facts),
        Wait(watch::Receiver<u64>),
        Load(Ticket),
    }

    pub(super) struct Ticket {
        key: (Identity, u32),
        revision: u64,
        changed: Arc<watch::Sender<u64>>,
        /// `publish`/`abort` 走完结算时置位；没置位就被丢的票由 `Drop` 兜底收格子。
        settled: bool,
    }

    impl Drop for Ticket {
        /// 装载票没走到 `publish`/`abort` 就被丢弃（典型场景：请求任务在
        /// `read_row` 途中被客户端超时取消）时把格子交还并唤醒等待者——否则
        /// `loading` 永真，该 dbnum 之后的所有 `acquire` 全停在 `Wait` 上挂死。
        fn drop(&mut self) {
            if self.settled {
                return;
            }
            let returned = {
                let mut state = state();
                state.entries.get_mut(&self.key).is_some_and(|entry| {
                    // revision 变了（forget/clear 换过格）或别人已结算，都不是这票的格子。
                    if entry.revision == self.revision && entry.loading {
                        entry.loading = false;
                        true
                    } else {
                        false
                    }
                })
            };
            if returned {
                self.changed.send_modify(|version| *version += 1);
            }
        }
    }

    fn state() -> std::sync::MutexGuard<'static, State> {
        STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn enabled() -> bool {
        state().enabled
    }

    pub(super) fn enable(identity: Identity) {
        let mut state = state();
        if state.identity.as_ref() != Some(&identity) {
            state.entries.clear();
        }
        state.identity = Some(identity);
        state.enabled = true;
    }

    pub(super) fn disable() {
        let mut state = state();
        state.enabled = false;
        state.identity = None;
        state.entries.clear();
    }

    pub(super) fn acquire(dbnum: u32) -> Acquire {
        let mut state = state();
        if !state.enabled {
            return Acquire::Disabled;
        }
        let Some(identity) = state.identity.clone() else {
            return Acquire::Disabled;
        };
        let key = (identity, dbnum);
        let entry = state.entries.entry(key.clone()).or_insert_with(|| {
            let (changed, _) = watch::channel(0);
            Entry {
                facts: None,
                revision: 0,
                loading: false,
                changed: Arc::new(changed),
            }
        });
        if let Some(facts) = &entry.facts {
            return Acquire::Hit(facts.clone());
        }
        if entry.loading {
            return Acquire::Wait(entry.changed.subscribe());
        }
        entry.loading = true;
        Acquire::Load(Ticket {
            key,
            revision: entry.revision,
            changed: Arc::clone(&entry.changed),
            settled: false,
        })
    }

    /// `false` 表示装载期间发生了写入/失效；调用方必须重读，不得返回旧值。
    pub(super) fn publish(mut ticket: Ticket, facts: Facts) -> bool {
        ticket.settled = true;
        let mut state = state();
        let published = state.entries.get_mut(&ticket.key).is_some_and(|entry| {
            if entry.revision != ticket.revision || !entry.loading {
                return false;
            }
            entry.facts = Some(facts);
            entry.loading = false;
            true
        });
        drop(state);
        ticket.changed.send_modify(|version| *version += 1);
        published
    }

    pub(super) fn abort(mut ticket: Ticket) {
        ticket.settled = true;
        let mut state = state();
        if let Some(entry) = state.entries.get_mut(&ticket.key) {
            if entry.revision == ticket.revision {
                entry.loading = false;
            }
        }
        drop(state);
        ticket.changed.send_modify(|version| *version += 1);
    }

    pub(super) fn forget(dbnum: u32) {
        let mut state = state();
        let Some(identity) = state.identity.clone() else {
            return;
        };
        if let Some(entry) = state.entries.get_mut(&(identity, dbnum)) {
            entry.revision = entry.revision.wrapping_add(1);
            entry.facts = None;
            entry.loading = false;
            let changed = Arc::clone(&entry.changed);
            drop(state);
            changed.send_modify(|version| *version += 1);
        }
    }

    pub(super) fn clear() {
        let mut state = state();
        let changed = state
            .entries
            .values_mut()
            .map(|entry| {
                entry.revision = entry.revision.wrapping_add(1);
                entry.facts = None;
                entry.loading = false;
                Arc::clone(&entry.changed)
            })
            .collect::<Vec<_>>();
        drop(state);
        for sender in changed {
            sender.send_modify(|version| *version += 1);
        }
    }

    /// 启动水化一次性替换快照；不会让调用方观察到半张登记表。
    pub(super) fn replace(facts: impl IntoIterator<Item = (u32, Facts)>) {
        let mut state = state();
        let Some(identity) = state.identity.clone() else {
            return;
        };
        let mut replacement = BTreeMap::new();
        for (dbnum, facts) in facts {
            let (changed, _) = watch::channel(0);
            replacement.insert(
                (identity.clone(), dbnum),
                Entry {
                    facts: Some(facts),
                    revision: 0,
                    loading: false,
                    changed: Arc::new(changed),
                },
            );
        }
        state.entries = replacement;
    }

    pub(super) fn len() -> usize {
        state()
            .entries
            .values()
            .filter(|entry| entry.facts.is_some())
            .count()
    }
}

const INCREMENT_STATE_SCHEMA: &str = r#"
DEFINE TABLE IF NOT EXISTS dbnum_watermark SCHEMALESS;
DEFINE TABLE IF NOT EXISTS dbnum_info_table SCHEMALESS;
DEFINE TABLE IF NOT EXISTS dbnum_write_state SCHEMALESS;
-- 回退到水位（ADR-072 D2）按 dbnum 谓词删 `inst_info`，而这张表此前一条索引都没有：
-- 量测里它是唯一一张落 `Iterate Table` 的模型面表（`docs/evidence/2026-09-10-rollback-predicate-index-measurement.md`
-- §2 / §5）。姐妹表 `inst_relate` / `tubi_relate` 的同名索引在 `pdms_inst::INST_RELATE_INDEX_SQL`
-- 里，这一条落在这里只因为那个文件此刻归房间段改；两处等价，都是启动序列跑一次。
DEFINE INDEX IF NOT EXISTS idx_inst_info_dbnum ON inst_info FIELDS dbnum;
DEFINE TABLE IF NOT EXISTS queue_control SCHEMALESS;
-- 强制范围重生成的未完成留痕（issue #59，`model_scope_regen`）。
DEFINE TABLE IF NOT EXISTS model_scope_regen SCHEMALESS;
"#;

const CURRENT_DATABASE_WATERMARKS: &str = "SELECT dbnum, math::max(sesno) AS sesno FROM pe \
     WHERE dbnum != NONE AND sesno != NONE GROUP BY dbnum;";
const LEGACY_INFO_WATERMARKS: &str = "SELECT dbnum, math::max(sesno) AS sesno FROM dbnum_info_table \
     WHERE dbnum != NONE AND sesno != NONE GROUP BY dbnum;";

/// File observation captured during a (read-only) scan.
///
/// Writing this must NOT touch `applied_sesno` beyond a one-time establishment
/// migration; it only refreshes the scan-observation fields and `scanned_at`.
#[derive(Debug, Clone, Default)]
pub struct FileObservation {
    pub dbnum: u32,
    /// 这个文件所属的项目（由它所在的监控目录决定，不是配置里的主项目名）。
    ///
    /// `dbnum` 在 AVEVA 里只在**项目内**唯一，而本表按裸 dbnum 做记录 id。带上项目
    /// 才能认出「这一行不是你的」：三个项目的 sys 库都是 8191，实测里 zdjsys 的
    /// 观察值就这么把 amssys 那一行的 `file_latest_sesno` 写成了 52。
    pub project: String,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_latest_sesno: i32,
    /// RFC3339 timestamp of the file's last-modified time, if known.
    pub file_modified_at: Option<String>,
}

/// Effective DBNUM state resolved from the stored record (+ one-time migration).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbnumState {
    pub dbnum: u32,
    /// 数据库内这行持久事实的版本。每个权威写者在同一事务中推进，供窗口 CAS 使用。
    #[serde(default)]
    pub state_revision: u64,
    /// 这一行归哪个项目。空串表示旧数据（本字段引入前写的），此时不做归属校验。
    #[serde(default)]
    pub owner_project: String,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_latest_sesno: i32,
    /// Effective applied watermark (migrated when necessary); 0 when uninitialized.
    pub applied_sesno: i32,
    /// `applied_sesno` 那条保存在 E3D 里的写入时刻（RFC3339），水位推进时顺手存下来的。
    ///
    /// 回退阻断卡的「已应用」那一端只能从这里取（plant-ui ADR-0019 Q6）：文件被换回
    /// 旧版本之后，那一页在当前文件里读不到了，现读读不出来。旧行没有这一列、或推进
    /// 时读不到时刻 → `None` → 界面降级成「应用时刻无记录」，**不许拿挂钟 `applied_at`
    /// 兜底**。它跟着 `applied_sesno` 同一条单调条件走，两者永远指同一条保存。
    #[serde(default)]
    pub applied_sesno_time: Option<String>,
    /// 最近一次成功基线明确确认“没有业务元素”时建立的应用水位。
    /// 只有它与当前 `applied_sesno` 相等，`pe` 零行才是合法空基线（ADR-023）。
    #[serde(default)]
    pub confirmed_empty_baseline_sesno: Option<i32>,
    /// 模型水位（ADR-060 D1）：模型已经追平到哪一版**数据窗口**。`0` = 从未追平。
    ///
    /// 与 `applied_sesno` 是两枚**同粒度**的水位，写在同一行上，一次读出。它记的不是
    /// 「模型有多新」（那是投影回执 `RootReceipt::source_sesno` 的事，按 ADR-054 走文件
    /// 刻度）而是「到哪一版数据为止，数据与模型是一致的」，因此 `model_sesno <= applied_sesno`
    /// 恒成立（M2）。落后是常态、不是告警（ADR-056 D1/N4）。
    #[serde(default)]
    pub model_sesno: i32,
    /// `model_sesno` 那条保存的写入时刻（RFC3339）。与 [`DbnumState::applied_sesno_time`]
    /// 同一把尺子：号对不上就不贴时刻，不许拿挂钟兜底。
    #[serde(default)]
    pub model_sesno_time: Option<String>,
    /// 模型水位真正写下的挂钟时刻。只在统一判决为 `in_sync` 时作为
    /// `model_caught_up_at` 对外；源保存时刻仍由 `model_sesno_time` 表达。
    #[serde(default)]
    pub model_settled_at: Option<String>,
    /// 生成根四格分类里的两格（ADR-060 D3/D5）：应有但从未生成过的根、已放弃的根
    /// （死信或 `http_422`）。两者都**不拦水位**，但都要露出来。
    ///
    /// `None` 表示**未观察**——从没开过窗的冷库就是这一档。它与 `Some(0)`（观察过、确实
    /// 一个都没有）是两件事，读侧不得把前者降级成后者（M5）。
    #[serde(default)]
    pub model_lazy_roots: Option<u32>,
    #[serde(default)]
    pub model_dead_roots: Option<u32>,
    /// 上面两个计数是在哪一版数据窗口右端观察到的；同样 `None` = 未观察。
    #[serde(default)]
    pub model_roots_observed_at: Option<i32>,
    /// 库级目录落款（ADR-079）：`CATA dbnum → 会话`，「这个库的整份模型与该目录到这一版一致」。
    /// 随 `model_sesno` 由任务收尾那一笔合并写（`task_end::render_catalogue_stamp_merge`），只在
    /// 整库重生成 / 目录级联全部命中根重算成之后才前移；目录版本核对
    /// （`model_coverage::reconcile_catalogue_versions`）读它判哪些库还落在旧目录上。
    /// 空 = 这个库还没有任何目录落款（没生成过，或旧行）——不算过期，也不参与核对。
    #[serde(default)]
    pub model_catalogue_sessions: Vec<CatalogueStampRow>,
    /// 这个库的房间边整库对齐到哪一代常驻房间模型（ADR-073 D14）：`ResidentRoomModel::input_version`，
    /// 与根 ACK 的 `room_generation` 同一个字串。随 `model_sesno` 由任务收尾那一笔写，且只有能替
    /// **整库**说话的任务才写——初始化、整库重排的 ModelOnly、本任务跑过整库房间重算（D13）；普通
    /// 增量窗口只重算被触到的根，这一格原样不动；算不了房间的进程（`room_membership()` 为 false）盖模型水位时清空。
    /// `None` = durable 侧没有房间就绪承诺（旧行、从没整库对齐过、算不了房间的进程生成的）。
    /// `e3d.room.lookup` 在进程内没有该根 ACK 时（rocksdb 直写不留投影、冷重启）拿它当就绪判据。
    #[serde(default)]
    pub model_room_generation: Option<String>,
    /// 上面那一格盖章**之后**，本库有根按哪几代常驻模型写过边（ADR-073 D13 追记 F3 落库，2026-09-19）：
    /// 刷盘一组按 `rooms` 这一代写边、而这一代 ≠ 库上 `model_room_generation` 时，随那一组的行同一笔事务记进来
    /// （`model_db_adapter::stray_room_generation_statement`）；`Aligned` / `Cleared` 盖章时清空。它替**重启后丢了
    /// 进程内 ACK** 的那几根说话：库上 == 常驻、③ 不欠、账本没段，而这一格非空 = 有根的边是另一代算的，
    /// 下一个任务欠一次（`room_recompute::DurableAlignment::owed`）。是模型水位的属性，不是第三枚水位（W3）。
    #[serde(default)]
    pub model_room_stray_generations: Vec<String>,
    /// `true` when a watermark could be resolved from any source (record, legacy
    /// field or info table). `false` means this `dbnum` has never been applied.
    pub initialized: bool,
}

/// `model_catalogue_sessions` 里的一条：`{ dbnum, session }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CatalogueStampRow {
    pub dbnum: u32,
    pub session: u32,
}

impl DbnumState {
    /// Select the first missing stage from the two watermarks alone, without reading
    /// model tables. 房间归属随根生成、写在模型水位的承诺里（ADR-073 D2），没有自己的一档。
    pub fn recovery_stage(&self) -> RecoveryStage {
        if self.applied_sesno < self.model_sesno || self.file_latest_sesno < self.applied_sesno {
            return RecoveryStage::InvalidWatermarks;
        }
        if !self.initialized || self.applied_sesno < self.file_latest_sesno {
            return RecoveryStage::Data;
        }
        if self.model_sesno < self.applied_sesno {
            return RecoveryStage::Model;
        }
        RecoveryStage::Complete
    }

    /// 两枚水位这一半的判据（ADR-060 D4 的前两项）：数据已建立，且模型追平到同一版。
    ///
    /// **它不是完整的 `model_in_sync`**——D4 还要求「无未安定根」，那是进程内账本
    /// （`model_root_ledger`）的实时计数，不在这一行上。露给读者的那个字段要在两者都成立时才为真；
    /// 这里只回答行内能回答的那一半，绝不替另一半打包票。
    pub fn model_caught_up(&self) -> bool {
        self.applied_sesno > 0 && self.model_sesno == self.applied_sesno
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStage {
    InvalidWatermarks,
    Data,
    Model,
    Complete,
}

/// Raw projection of the stored `dbnum_watermark:{dbnum}` record used for reads.
///
/// Only non-datetime fields are selected so deserialization stays trivial.
#[derive(Debug, Clone, Default, Deserialize)]
struct StateRow {
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default)]
    state_revision: Option<u64>,
    #[serde(default)]
    owner_project: Option<String>,
    #[serde(default)]
    db_type: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    file_latest_sesno: Option<i32>,
    /// New authoritative field; `None` when not yet established (pre-migration).
    #[serde(default)]
    applied_sesno: Option<i32>,
    /// 已应用那条保存的写入时刻。存的是 RFC3339 **字符串**（不是 datetime），所以
    /// 与本结构「只选非 datetime 字段」的前提一致；旧行没有这一列，读出来就是 `None`。
    #[serde(default)]
    applied_sesno_time: Option<String>,
    #[serde(default)]
    confirmed_empty_baseline_sesno: Option<i32>,
    /// 模型水位与它的四格计数（ADR-060）。旧行没有这几列，读出来就是 `None`——
    /// 「未观察」，不是 0。
    #[serde(default)]
    model_sesno: Option<i32>,
    #[serde(default)]
    model_sesno_time: Option<String>,
    #[serde(default)]
    model_settled_at: Option<String>,
    #[serde(default)]
    model_lazy_roots: Option<u32>,
    #[serde(default)]
    model_dead_roots: Option<u32>,
    #[serde(default)]
    model_roots_observed_at: Option<i32>,
    /// 库级目录落款（ADR-079）；旧行没有这一列 → 空。
    #[serde(default)]
    model_catalogue_sessions: Option<Vec<CatalogueStampRow>>,
    /// 整库房间边对齐到的常驻代号（ADR-073 D14）；旧行没有这一列 → `None`。
    #[serde(default)]
    model_room_generation: Option<String>,
    /// 盖章之后按哪几代写过根（ADR-073 F3 落库）；旧行没有这一列 → 空。
    #[serde(default)]
    model_room_stray_generations: Option<Vec<String>>,
    /// Legacy watermark field, kept for migration + backward-compat mirroring.
    #[serde(default)]
    sesno: Option<i32>,
}

/// 本表里模型水位那一组列，两条读路径共用一份投影，漂不了。
const MODEL_WATERMARK_COLUMNS: &str = "model_sesno, model_sesno_time, \
     IF model_settled_at != NONE THEN type::string(model_settled_at) ELSE NONE END AS model_settled_at, \
     model_lazy_roots, \
     model_dead_roots, model_roots_observed_at, model_catalogue_sessions, model_room_generation, \
     model_room_stray_generations";

#[derive(Debug, Deserialize)]
struct DatabaseWatermark {
    dbnum: u32,
    sesno: i32,
}

/// 存下来的那个时刻**只描述 `applied_sesno` 那一条保存**，所以只有生效水位确实
/// 取自这一列时才许露出来。
///
/// 生效水位可能来自旧的 `sesno` 字段或 `dbnum_info_table`（见模块头的读取顺序）——
/// 那两条路解析出来的是另一个会话号，把这一列贴上去就是拿 A 的时刻标注 B。宁可
/// 让界面走「应用时刻无记录」的降级，也不能摆一个对不上的时刻。
fn applied_time_of(row: &StateRow, effective_applied: Option<i32>) -> Option<String> {
    if effective_applied.is_none() || effective_applied != row.applied_sesno {
        return None;
    }
    row.applied_sesno_time.clone()
}

/// 模型水位那一列的同款纪律（ADR-060 D1）。
///
/// 这一枚水位没有 `applied_sesno` 那条兼容迁移链——它只由本仓写、只有一个来源，
/// 所以不必比「生效值是不是取自这一列」。要挡的只有一格：水位是 0 / 缺席时，
/// 时刻必须一起消失。留着一个孤零零的时刻，界面会拿它去标注一个并不存在的追平。
fn model_time_of(row: &StateRow) -> Option<String> {
    if row.model_sesno.unwrap_or_default() <= 0 {
        return None;
    }
    row.model_sesno_time.clone()
}

/// A file-identity anomaly for one `dbnum` (see spec §文件异常).
///
/// [`check_file_against_state`] decides `NameChanged` / `Rollback` / `TypeChanged` /
/// `PathMigrated` from a single observed file vs the stored state; `Duplicate` /
/// `Missing` are constructed by the project scanner which aggregates all files per
/// `dbnum`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileAnomaly {
    /// `file_latest_sesno < applied_sesno`: the file rolled back or was replaced.
    /// The incremental path must never touch it (the watermark never regresses
    /// inside that path); routing turns it into a reinit batch instead — the
    /// worker wipes the dbnum's data and re-parses the current file as a first
    /// import (ADR-021). Only when that wipe fails does the dbnum stay blocked.
    ///
    /// 两个序号是**判据**（比大小的那一对，也是日志与规格样例里的诊断证据）；
    /// 两个时刻是**给人看的那一对**（plant-ui ADR-0019 Q6：`最新保存 07-01 10:00
    /// 早于已应用 08-05 18:24`）。判据函数 [`check_file_against_state`] 是纯的，
    /// 手上既没有文件也没有存量时刻，所以它一律填 `None`，由
    /// [`DbnumState::classify_scan`] 在有上下文的地方补齐。
    Rollback {
        file_latest_sesno: i32,
        applied_sesno: i32,
        /// 文件里最新那条保存的写入时刻。**现读得到**——文件就在那儿，只是
        /// 阻断分支过去从不走解析那条路，所以这一格一直空着。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_latest_sesno_time: Option<String>,
        /// 已应用那条保存的写入时刻，取自水位表存下来的那一列。
        ///
        /// **只能从存量取**：回退之后 `applied_sesno` 那一页在当前文件里已经读不到了。
        /// 没有存量 → `None` → 界面降级成 `早于已应用水位（应用时刻无记录）`，
        /// 不许拿挂钟 `applied_at` 兜底。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        applied_sesno_time: Option<String>,
    },
    /// Same `dbnum` and `db_type`, path changed, watermark did not regress, and the
    /// file kept its identity: only the directory changed, or a master moved to its own
    /// extract leaf (`ams7355` → `ams7355_0001`, ADR-028). The stored path may be
    /// auto-updated.
    PathMigrated { old_path: String, new_path: String },
    /// 同一个 dbnum，现场文件名却不是登记的那一个（不是只换了目录，也不是母库换成它自己
    /// 的 extract）。库的身份是 dbnum + 约定文件名，换了名就是另一份文件：跳过它——不入队、
    /// 不清库、不改登记，连观察值也不写，等人处理。
    NameChanged { old_path: String, new_path: String },
    /// Same dbnum was observed with a different database type. Never overwrite
    /// the stored identity automatically.
    TypeChanged {
        stored_db_type: String,
        observed_db_type: String,
    },
    /// Multiple files with the same `dbnum` in the project: block, do not pick.
    Duplicate { paths: Vec<String> },
    /// A registered file is no longer present at its recorded path.
    Missing { path: String },
    /// 这个 dbnum 的登记行归另一个项目所有。
    ///
    /// `dbnum` 只在项目内唯一（三个项目的 sys 库都是 8191），而本表按裸 dbnum
    /// 做记录 id。正常情况下别的项目的运行态系统库压根进不了摄入范围
    /// （`in_scope_with`），这一条是那道门被绕过时的兜底：**连观察值
    /// 都不许写**，否则就是实测过的那种污染——行还写着 amssys 的身份，
    /// `file_latest_sesno` 却是 zdjsys 的 52。
    ForeignProject {
        stored_project: String,
        observed_project: String,
    },
}

impl FileAnomaly {
    /// 该异常是否允许**按原样增量应用**（`false` = 不许）。
    ///
    /// 只有路径迁移不阻断——它是良性搬家，登记路径跟着更新即可
    /// （QUEUE-FIELD-MAP §3「本期不执行」一格的判定，从预览里提出来复用）。
    /// 注意回退（[`Self::requires_reinit`]）虽然 `blocks() == true`，但调用方
    /// 在咨询本谓词**之前**就把它路由成整库重建批次（ADR-021）：`blocks()`
    /// 对回退真正兜住的只剩「重建（清库）失败时保持阻断」这一条退路。
    pub fn blocks(&self) -> bool {
        !matches!(self, FileAnomaly::PathMigrated { .. })
    }

    /// 说给人听的阻断理由；不阻断的异常返回 `None`。
    ///
    /// 回退那句的措辞被 `docs/specs/web-service-api.md` 的回执样例钉着，别改。
    pub fn block_reason(&self) -> Option<String> {
        match self {
            FileAnomaly::PathMigrated { .. } => None,
            FileAnomaly::NameChanged { old_path, new_path } => Some(format!(
                "文件名与登记不符（登记 {old_path} → 现场 {new_path}），已跳过该文件：库的身份是 \
                 dbnum + 约定文件名，只有换目录、或母库换成它自己的 extract 才按迁移处理"
            )),
            FileAnomaly::Rollback {
                file_latest_sesno,
                applied_sesno,
                ..
            } => Some(format!(
                "文件回退或被替换（file_latest_sesno={file_latest_sesno} < \
                 applied_sesno={applied_sesno}），已阻断"
            )),
            FileAnomaly::TypeChanged {
                stored_db_type,
                observed_db_type,
            } => Some(format!(
                "库类型变更（登记 {stored_db_type} → 现场 {observed_db_type}），已阻断"
            )),
            FileAnomaly::Duplicate { paths } => Some(format!(
                "同 dbnum 存在多个文件，已阻断: {}",
                paths.join("; ")
            )),
            FileAnomaly::Missing { path } => Some(format!("登记文件缺失，已阻断: {path}")),
            FileAnomaly::ForeignProject {
                stored_project,
                observed_project,
            } => Some(format!(
                "该 dbnum 的登记行属于项目 {stored_project}，现场文件来自 {observed_project}，\
                 已阻断（dbnum 只在项目内唯一，本库只承载主项目的系统库）"
            )),
        }
    }

    /// 是不是「这一行不归你」——这类异常连观察值都不许写。
    pub fn is_foreign_project(&self) -> bool {
        matches!(self, FileAnomaly::ForeignProject { .. })
    }

    /// 观察到的根本不是登记的那份文件：归属不符，或文件名换了。这时连观察值都不许写——
    /// 写进去的会话号、长度属于另一份文件，面板上看到的就是一个不存在的文件状态。
    pub fn observed_another_file(&self) -> bool {
        matches!(
            self,
            FileAnomaly::ForeignProject { .. } | FileAnomaly::NameChanged { .. }
        )
    }

    /// 哪些异常转「整库重建」（ADR-021）——**只有回退**。
    ///
    /// 回退的语义是清楚的：登记身份没变、文件本体被还原到更旧的会话，磁盘上
    /// 已经没有被丢弃的那段历史，重建动作（整库清空 + 按首次导入重新解析当前
    /// 文件）有唯一正确答案，因此默认自动执行、不阻断等人。其余阻断类异常动的
    /// 是「这个 dbnum 对应哪个文件」的根基（类型变更 / 同号多文件 / 归属不符 /
    /// 文件缺失 / 文件名不符），自动清库等于替人拍板挑文件，照旧阻断；`PathMigrated` 本来
    /// 就不阻断，轮不到这里。
    ///
    /// 清库动作只发生在数据批次 worker 的执行体（扫描路径只分类 + 入队重建
    /// 批次，破坏性动作受 `startup_autorun` / 队列暂停约束），见
    /// `manual_update::execute_one_dbnum` 与 `fast_delete::wipe_dbnum_for_reinit`。
    pub fn requires_reinit(&self) -> bool {
        matches!(self, FileAnomaly::Rollback { .. })
    }
}

/// 一次扫描观察的裁决：登记状态 + 异常分类。
///
/// 存在的理由是**落库口径只能有一处**。判据（`db_type` / `file_name` /
/// `file_path`）与写这几个字段的语句是同一批，谁先谁后、阻断时写不写，过去由
/// 每个调用点各自决定，于是自动路径用只写观察值的那条语句保住了证据，而手动
/// 预览与执行体照常刷新文件身份——同一个 `TypeChanged`，点一次预览就被自己
/// 抹掉，连自动路径下一轮也检不出来了。
///
/// 现在拿到裁决才能落库（[`DbnumState::record_observation`] 由裁决自己选语句），
/// 调用方剩下的自由只有「阻断了要说什么」。
#[derive(Debug, Clone)]
pub struct ScanVerdict {
    /// 本次观察之前登记的状态；`None` 表示这个 dbnum 从未登记过。
    pub prior: Option<DbnumState>,
    pub anomaly: Option<FileAnomaly>,
}

impl ScanVerdict {
    /// 权威水位（未登记时为 0）。
    pub fn applied_sesno(&self) -> i32 {
        self.prior.as_ref().map(|s| s.applied_sesno).unwrap_or(0)
    }

    pub fn confirmed_empty_baseline_sesno(&self) -> Option<i32> {
        self.prior
            .as_ref()
            .and_then(|state| state.confirmed_empty_baseline_sesno)
    }

    /// 上一次观察到的文件最新会话号（未登记时为 0）。
    pub fn previous_file_latest_sesno(&self) -> i32 {
        self.prior
            .as_ref()
            .map(|s| s.file_latest_sesno)
            .unwrap_or(0)
    }

    pub fn blocked(&self) -> bool {
        self.anomaly.as_ref().is_some_and(FileAnomaly::blocks)
    }

    /// 阻断理由；没阻断时 `None`。
    pub fn block_reason(&self) -> Option<String> {
        self.anomaly.as_ref().and_then(FileAnomaly::block_reason)
    }
}

/// Resolve the effective applied watermark from the three ordered sources.
///
/// Pure decision function (ADR-001 §兼容迁移). Priority:
/// established `applied_sesno` > legacy `dbnum_watermark.sesno` > `dbnum_info_table`
/// max. Returns `None` when nothing is known (uninitialized `dbnum`).
pub fn resolve_migrated_applied_sesno(
    existing_applied: Option<i32>,
    legacy_watermark_sesno: Option<i32>,
    info_table_max_sesno: Option<i32>,
) -> Option<i32> {
    existing_applied
        .or(legacy_watermark_sesno)
        .or(info_table_max_sesno)
}

fn resolve_read_applied(
    dedicated: Option<(Option<i32>, Option<i32>)>,
    info_table_max_sesno: Option<i32>,
) -> Option<i32> {
    match dedicated {
        Some((applied, legacy)) => resolve_migrated_applied_sesno(applied, legacy, None),
        None => resolve_migrated_applied_sesno(None, None, info_table_max_sesno),
    }
}

/// A retained initialization is proof that legacy PE/info rows may be partial.
/// Only an explicit applied watermark may be used until its attribute commit.
fn resolve_initializing_applied(
    initializing: bool,
    dedicated: Option<(Option<i32>, Option<i32>)>,
    info: Option<i32>,
) -> Option<i32> {
    if initializing {
        dedicated.and_then(|(applied, _)| applied)
    } else {
        resolve_read_applied(dedicated, info)
    }
}

fn resolve_dbnum_applied(
    dbnum: u32,
    dedicated: Option<(Option<i32>, Option<i32>)>,
    info: Option<i32>,
) -> Option<i32> {
    // 「这个库正在被写」的来源是任务注册表（ADR-071 S5，d-300），不再是发布 run：任务在跑时
    // 只认专用水位行，不拿 `dbnum_info_table` 里半截基线的 max sesno 当应用水位。
    resolve_initializing_applied(
        crate::data_interface::task_registry::TaskRegistry::global()
            .has_running_data_batch_for(dbnum),
        dedicated,
        info,
    )
}

fn render_legacy_applied_migration(protected: &[u32]) -> String {
    let ids = protected
        .iter()
        .map(|n| format!("{WATERMARK_TABLE}:{n}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "UPDATE {WATERMARK_TABLE} SET applied_sesno = sesno WHERE applied_sesno = NONE AND sesno != NONE AND id NOT IN [{ids}];"
    )
}

/// Classify one observed file for one `dbnum` against its stored state.
///
/// Returns `Some(anomaly)` when there is something to report/handle, `None` when
/// the file looks normal. A file that is not the registered one (see
/// `keeps_file_identity`) is `NameChanged` before anything else: its session numbers
/// and type say nothing about the registered file. Otherwise rollback takes precedence
/// over a path change.
pub fn check_file_against_state(
    stored_db_type: Option<&str>,
    stored_path: Option<&str>,
    applied_sesno: i32,
    observed_db_type: &str,
    observed_path: &str,
    observed_file_latest_sesno: i32,
) -> Option<FileAnomaly> {
    if let Some(stored_path) = stored_path
        && stored_path != observed_path
        && !keeps_file_identity(stored_path, observed_path)
    {
        return Some(FileAnomaly::NameChanged {
            old_path: stored_path.to_string(),
            new_path: observed_path.to_string(),
        });
    }
    if observed_file_latest_sesno < applied_sesno {
        return Some(FileAnomaly::Rollback {
            file_latest_sesno: observed_file_latest_sesno,
            applied_sesno,
            // 纯函数给不出时刻（既没有文件也没有存量），交给 `classify_scan` 补。
            file_latest_sesno_time: None,
            applied_sesno_time: None,
        });
    }
    if let (Some(stored_path), Some(stored_db_type)) = (stored_path, stored_db_type) {
        if stored_db_type != observed_db_type {
            return Some(FileAnomaly::TypeChanged {
                stored_db_type: stored_db_type.to_string(),
                observed_db_type: observed_db_type.to_string(),
            });
        }
        if stored_db_type == observed_db_type && stored_path != observed_path {
            return Some(FileAnomaly::PathMigrated {
                old_path: stored_path.to_string(),
                new_path: observed_path.to_string(),
            });
        }
    }
    None
}

/// 换了路径的文件还是不是登记的那个库。库的身份是 dbnum + 约定文件名：
///
/// - 文件名相同（不分大小写），只是目录变了——是；
/// - 母库换成它自己的 extract（同前缀、同库号，`ams7355` → `ams7355_0001`，ADR-028
///   选序号最高的叶子当工作文件）——是；
/// - 其余（换前缀、换库号、`_0001` → `_0002`、extract 退回母库）——不是。
///
/// 两种路径分隔符都认：登记路径是 Windows 写法，判据在 Linux 构建上也要跑。
fn keeps_file_identity(stored_path: &str, observed_path: &str) -> bool {
    let file_name = |path: &str| {
        path.rsplit(|c: char| c == '/' || c == '\\')
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase()
    };
    let (stored, observed) = (file_name(stored_path), file_name(observed_path));
    if stored == observed {
        return true;
    }
    use crate::data_interface::extract_family::parse_extract_file_name;
    match (
        parse_extract_file_name(&stored),
        parse_extract_file_name(&observed),
    ) {
        (Some(master), Some(leaf)) => {
            master.extract.is_none()
                && leaf.extract.is_some()
                && master.prefix == leaf.prefix
                && master.dbnum == leaf.dbnum
        }
        _ => false,
    }
}

/// Escape a string for safe embedding inside a single-quoted SurrealQL literal.
///
/// Windows paths carry backslashes, which are escape characters in SurrealQL
/// strings; escape those and single quotes.
pub fn escape_surql_str(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('\'', "\\'")
}

fn database_has_table(info: &serde_json::Value, table: &str) -> bool {
    info.get("tables")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|tables| tables.contains_key(table))
}

/// 播种完成标记：pe 兼容播种在此库上**完整**跑完过一次。
///
/// 分块 UPSERT 每 500 条一批；部分批次成功后死掉，表里已经有行，「空表判定」
/// 就分不出「跑完了」与「跑了一半」。只有一个在全部批次成功之后才落库的标记
/// 能把两者区分开。与暂停旗标（`queue_control:main`）同表不同行。
const SEED_MARKER: &str = "queue_control:watermark_seed";

/// 是否需要按当前数据库（pe）播种兼容水位。
///
/// 三个条件任意一个成立就（重）跑 pe 源：表缺失、表为空、播种完成标记缺失。
/// 前两个是显而易见的初始/崩溃状态；第三个覆盖「分块 UPSERT 部分完成后死掉」
/// 与「从没跑过带标记版本的老库」——两者从表内容上无法区分，只能统一补跑。
/// 重跑无损：回填是 `??` 填空，绝不覆盖已建立水位，代价只是该库升级后的
/// 第一次启动多一次 pe 全表聚合（有日志提示）。
///
/// 若不这样判，后果是：pe 源本要救的那类老库（`dbnum_info_table` 缺失或陈旧）
/// 半途死一次后，下次启动切到 info 源，没播上的 dbnum 以 0 水位被
/// `needs_initial_load` 判成首次导入，整库全量重解析。
fn should_seed_from_current_database(
    watermark_table_missing: bool,
    watermark_rows: usize,
    seed_marker_present: bool,
) -> bool {
    watermark_table_missing || watermark_rows == 0 || !seed_marker_present
}

/// 把播种候选分成「可以固化」与「按下不表」两拨。按下不表的只有**任务在飞**的库
/// （`protected`）——半截基线的 info 表 max sesno 不能当应用水位。此前还按
/// `count(pe) != sum(dbnum_info_table.count)` 扣一批「可疑库」，那是逐笔观测块时代的对账
/// （两个数当时都是「pe 行数含墓碑」）；ADR-071 D6 之后 `count` 是活元素数、由任务收尾序列化，
/// 拿它跟 pe 行数比不再成立，那道对账 2026-09-10 退役。
fn partition_seedable(
    watermarks: Vec<DatabaseWatermark>,
    suspect: &BTreeSet<u32>,
) -> (Vec<DatabaseWatermark>, Vec<u32>) {
    let mut seedable = Vec::with_capacity(watermarks.len());
    let mut held_back = Vec::new();
    for row in watermarks {
        if suspect.contains(&row.dbnum) {
            held_back.push(row.dbnum);
        } else {
            seedable.push(row);
        }
    }
    (seedable, held_back)
}

fn migration_watermark_source(seed_from_current_database: bool) -> (&'static str, &'static str) {
    if seed_from_current_database {
        (CURRENT_DATABASE_WATERMARKS, "现有 PE 数据")
    } else {
        (LEGACY_INFO_WATERMARKS, "旧 DBNUM 统计")
    }
}

/// 现有水位行数（表不存在时 SurrealDB 对 SELECT 返回空集，得 0）。
async fn count_watermark_rows() -> anyhow::Result<usize> {
    #[derive(Deserialize)]
    struct CountRow {
        count: usize,
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM {WATERMARK_TABLE} GROUP ALL;"
        ))
        .await
        .map_err(|e| anyhow::anyhow!("统计增量水位行数失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("统计增量水位行数语句失败: {e}"))?;
    let rows: Vec<CountRow> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码增量水位行数失败: {e}"))?;
    Ok(rows.first().map(|row| row.count).unwrap_or_default())
}

/// 播种完成标记是否存在（表/行缺失时 SELECT 返回空集，得 `false`）。
async fn seed_marker_present() -> anyhow::Result<bool> {
    #[derive(Deserialize)]
    struct MarkerRow {
        #[serde(default)]
        dbnum_count: Option<i64>,
    }
    let mut response = SUL_DB
        .query(format!("SELECT dbnum_count FROM {SEED_MARKER};"))
        .await
        .map_err(|e| anyhow::anyhow!("读取播种完成标记失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取播种完成标记语句失败: {e}"))?;
    let rows: Vec<MarkerRow> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码播种完成标记失败: {e}"))?;
    Ok(!rows.is_empty())
}

/// 全部播种批次成功后落下完成标记。
///
/// `skipped_dbnums` 也记进去：跳过的库此后靠首次导入重建基线，光打一行控制台日志
/// 的话，事后没人说得清那一批为什么在重解析。
async fn write_seed_marker(
    source_name: &str,
    seeded_dbnums: usize,
    skipped_dbnums: &[u32],
) -> anyhow::Result<()> {
    let skipped = skipped_dbnums
        .iter()
        .map(|dbnum| dbnum.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPSERT {SEED_MARKER} SET source = '{source_name}', \
         dbnum_count = {seeded_dbnums}, skipped_dbnums = [{skipped}], \
         completed_at = time::now();"
    );
    SUL_DB
        .query(sql)
        .await
        .map_err(|e| anyhow::anyhow!("写播种完成标记失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("写播种完成标记语句失败: {e}"))?;
    Ok(())
}

/// 模型水位兼容播种的候选行：`applied_sesno > 0` 而 `model_sesno` **从未写过**（NONE）。
///
/// 只圈 NONE、不圈 0：0 是本仓显式写下的（D7 幽灵清零 / D8 清库归零 / 本播种的
/// [`LegacyModelWatermarkSeed::DataOnly`]），再播一次就把「已判过」改写成「追平」。
#[derive(Debug, Deserialize)]
struct LegacyModelWatermarkCandidate {
    dbnum: u32,
    applied_sesno: i32,
}

const LEGACY_MODEL_WATERMARK_CANDIDATES: &str = "SELECT dbnum, applied_sesno FROM dbnum_watermark \
     WHERE dbnum != NONE AND model_sesno = NONE AND applied_sesno != NONE AND applied_sesno > 0;";

/// 模型水位兼容播种的去向（用户 2026-09-10 拍板「模型水位默认按数据水位保持一致」；
/// ADR-060 D10；其中房间那半句随房间水位退役，接管本身保留，ADR-073 D7）。两格措辞不同，
/// 日志里不许混。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LegacyModelWatermarkSeed {
    /// 旧管线留下了模型投影（`inst_relate` / `tubi_relate`）：模型水位缺省视作与数据水位追平。
    Adopted { dbnum: u32, sesno: i32 },
    /// 数据在、模型一行没有：显式记 0，照旧由模型面生成；下次启动不再探。
    DataOnly { dbnum: u32 },
}

/// 接管：只填空（`??`），已写过的值一个不动。时刻列不贴——这一枚不是哪一条保存盖出来的，
/// 贴任何时刻都是拿 A 标注 B（ADR-060 D1 的时刻纪律）；`model_settled_at` 只是运维展示的挂钟。
fn render_legacy_model_watermark_adoption(dbnum: u32, applied_sesno: i32) -> String {
    format!(
        "UPDATE {WATERMARK_TABLE}:{dbnum} SET \
         model_sesno = model_sesno ?? {applied_sesno}, model_settled_at = time::now(), \
         state_revision = (state_revision ?? 0) + 1, updated_at = time::now() RETURN NONE;"
    )
}

/// 数据在、模型一行没有：把「从未追平」从 NONE 写成显式 0。语义不变（读侧本来就
/// `unwrap_or_default`），只为让这一行退出候选集——投影探针在大库上不便宜，不该每次启动都付。
fn render_legacy_model_watermark_data_only(dbnum: u32) -> String {
    format!(
        "UPDATE {WATERMARK_TABLE}:{dbnum} SET model_sesno = model_sesno ?? 0, \
         state_revision = (state_revision ?? 0) + 1, updated_at = time::now() RETURN NONE;"
    )
}

/// 「这个库有没有模型投影」的 SurrealQL 谓词（原 `model_completion::projection_presence_expr`，
/// 随 ADR-072 D5 搬来）：`inst_relate` / `tubi_relate` 两张表各问一次——先按行上的 `dbnum`
/// 列直查，查不到再从 `pe` 的 dbnum 索引出发沿属主看有没有挂着的投影行（历史行可能没有
/// `dbnum` 列）。两条路各保其义。
pub(crate) fn projection_presence_expr(dbnum: u32) -> String {
    ["inst_relate", "tubi_relate"]
        .into_iter()
        .map(|table| {
            format!(
                "array::len((SELECT VALUE id FROM {table} WHERE dbnum={dbnum} LIMIT 1)) > 0 OR \
         array::len((SELECT VALUE id FROM pe WHERE dbnum={dbnum} AND \
         array::len((SELECT VALUE id FROM {table} WHERE in=$parent.id LIMIT 1)) > 0 LIMIT 1)) > 0"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// 兼容播种的第二半（ADR-060 D10）：模型 / 房间水位缺省按数据水位追平。
///
/// 对象是「已有数据的库」里**旧管线已经留下模型投影**、而 `model_sesno` 从未写过的行：
/// 两枚水位填成 `applied_sesno`。数据在、模型一行没有的库显式记 0，照旧由模型面生成。
/// 只填空，重跑无损；`protected`（初始化在飞）的库不碰。
///
/// **必须排在 `init_inst_relate_indices` 之后**：投影探针（[`projection_presence_expr`]）
/// 的 `WHERE in = $parent.id` 没有 `idx_inst_relate_in` 时是逐 pe 行全表扫，大库上跑不完。
pub(crate) async fn seed_legacy_model_watermarks_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    protected: &[u32],
) -> anyhow::Result<Vec<LegacyModelWatermarkSeed>> {
    let mut response = db
        .query(LEGACY_MODEL_WATERMARK_CANDIDATES)
        .await
        .map_err(|e| anyhow::anyhow!("读取模型水位兼容播种候选失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取模型水位兼容播种候选语句失败: {e}"))?;
    let candidates: Vec<LegacyModelWatermarkCandidate> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码模型水位兼容播种候选失败: {e}"))?;
    let mut outcomes = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let dbnum = candidate.dbnum;
        if protected.contains(&dbnum) {
            continue;
        }
        let projection = projection_presence_expr(dbnum);
        let mut probe = db
            .query(format!("RETURN {projection};"))
            .await
            .map_err(|e| anyhow::anyhow!("探测 dbnum={dbnum} 的模型投影失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("探测 dbnum={dbnum} 的模型投影语句失败: {e}"))?;
        let has_projection: Option<bool> = probe
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码 dbnum={dbnum} 的模型投影探测失败: {e}"))?;
        let (sql, outcome) = if has_projection.unwrap_or(false) {
            (
                render_legacy_model_watermark_adoption(dbnum, candidate.applied_sesno),
                LegacyModelWatermarkSeed::Adopted {
                    dbnum,
                    sesno: candidate.applied_sesno,
                },
            )
        } else {
            (
                render_legacy_model_watermark_data_only(dbnum),
                LegacyModelWatermarkSeed::DataOnly { dbnum },
            )
        };
        db.query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("写 dbnum={dbnum} 的模型 / 房间兼容水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("写 dbnum={dbnum} 的模型 / 房间兼容水位语句失败: {e}"))?;
        DbnumState::invalidate_mirrored_on(db, dbnum);
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// 兼容播种的启动日志：安静的一次不出声；两格各一行、各带样本。
pub fn legacy_model_watermark_seed_lines(seeds: &[LegacyModelWatermarkSeed]) -> Vec<String> {
    fn sample(dbnums: &[String]) -> String {
        let shown = dbnums
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("、");
        if dbnums.len() > 12 {
            format!("{shown} …")
        } else {
            shown
        }
    }
    let adopted: Vec<String> = seeds
        .iter()
        .filter_map(|seed| match seed {
            LegacyModelWatermarkSeed::Adopted { dbnum, sesno } => Some(format!("{dbnum}@{sesno}")),
            LegacyModelWatermarkSeed::DataOnly { .. } => None,
        })
        .collect();
    let data_only: Vec<String> = seeds
        .iter()
        .filter_map(|seed| match seed {
            LegacyModelWatermarkSeed::DataOnly { dbnum } => Some(dbnum.to_string()),
            LegacyModelWatermarkSeed::Adopted { .. } => None,
        })
        .collect();
    let mut lines = Vec::new();
    if !adopted.is_empty() {
        lines.push(format!(
            "模型 / 房间水位兼容播种：{} 个已有模型投影的库按数据水位接管为追平（ADR-060 D10，\
             不排恢复 Task）：{}",
            adopted.len(),
            sample(&adopted)
        ));
    }
    if !data_only.is_empty() {
        lines.push(format!(
            "模型 / 房间水位兼容播种：{} 个库数据在、模型一行没有，模型水位记 0，照旧由模型面生成：{}",
            data_only.len(),
            sample(&data_only)
        ));
    }
    lines
}

impl DbnumState {
    /// [`seed_legacy_model_watermarks_on`] 打在主库上；初始化在飞的库照 applied 播种同一份名单排除。
    pub async fn seed_legacy_model_watermarks() -> anyhow::Result<Vec<LegacyModelWatermarkSeed>> {
        let protected = crate::data_interface::task_registry::TaskRegistry::global()
            .running_data_batch_dbnums();
        seed_legacy_model_watermarks_on(&SUL_DB, &protected).await
    }

    /// Ensure an old Surreal database can run the current incremental pipeline in place.
    ///
    /// Table definitions are idempotent. When the watermark table is absent — or exists but
    /// holds no rows yet (a previous run died between table creation and seeding) — this is
    /// an in-place compatibility upgrade: the durable baseline comes from the maximum `sesno`
    /// already persisted in `pe` for each `dbnum`. Watermark tables with established rows
    /// keep their own established/legacy watermarks and use `dbnum_info_table` only as the
    /// old fallback.
    pub async fn ensure_increment_state_storage() -> anyhow::Result<usize> {
        let mut response = SUL_DB
            .query("INFO FOR DB;")
            .await
            .map_err(|e| anyhow::anyhow!("检查增量状态表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("检查增量状态表语句失败: {e}"))?;
        let database_info: Option<serde_json::Value> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码数据库表信息失败: {e}"))?;
        let database_info = database_info
            .ok_or_else(|| anyhow::anyhow!("数据库表信息为空，无法安全初始化增量状态"))?;
        let watermark_table_missing = !database_has_table(&database_info, WATERMARK_TABLE);
        // 行数与完成标记必须在建表与任何写入之前取：它们共同决定播种源。
        let watermark_rows = if watermark_table_missing {
            0
        } else {
            count_watermark_rows().await?
        };
        let marker_present = seed_marker_present().await?;
        let seed_from_current_database = should_seed_from_current_database(
            watermark_table_missing,
            watermark_rows,
            marker_present,
        );

        SUL_DB
            .query(INCREMENT_STATE_SCHEMA)
            .await
            .map_err(|e| anyhow::anyhow!("初始化增量状态表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("初始化增量状态表语句失败: {e}"))?;

        let protected = crate::data_interface::task_registry::TaskRegistry::global()
            .running_data_batch_dbnums();
        SUL_DB
            .query(render_legacy_applied_migration(&protected))
            .await
            .map_err(|e| anyhow::anyhow!("迁移旧 DBNUM 水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("迁移旧 DBNUM 水位语句失败: {e}"))?;

        let (source_sql, source_name) = migration_watermark_source(seed_from_current_database);
        if seed_from_current_database {
            // 这一步在大库上是 pe 全表聚合，可能长时间无输出；不喊一声的话，
            // 现场很容易把首次兼容启动当成卡死。
            let reason = if watermark_table_missing {
                "水位表缺失"
            } else if watermark_rows == 0 {
                "水位表为空"
            } else {
                "播种完成标记缺失（上次播种中断，或首次升级到带标记的版本）"
            };
            println!(
                "增量水位播种开始：{reason}，按{source_name}（每个 dbnum 的最大 sesno）建立基线，大库上可能耗时较长…"
            );
        }
        let seed_started = std::time::Instant::now();
        let mut response = SUL_DB
            .query(source_sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取{source_name}水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取{source_name}水位语句失败: {e}"))?;
        let watermarks: Vec<DatabaseWatermark> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码{source_name}水位失败: {e}"))?;
        // 任务在飞的库不播：半截基线的 max sesno 不是应用水位。这一道对 PE 源与旧 info 表源同样适用。
        let protected: BTreeSet<u32> = protected.iter().copied().collect();
        let (watermarks, held_back) = partition_seedable(watermarks, &protected);
        if !held_back.is_empty() {
            eprintln!(
                "播种跳过 {} 个初始化未完成的 dbnum（保留完整导入路径）: {held_back:?}",
                held_back.len()
            );
        }

        for chunk in watermarks.chunks(500) {
            let sql = chunk
                .iter()
                .map(|row| {
                    format!(
                        "UPSERT {WATERMARK_TABLE}:{dbnum} SET dbnum = {dbnum}, \
                         applied_sesno = applied_sesno ?? {sesno}, \
                         sesno = sesno ?? {sesno}, updated_at = time::now();",
                        dbnum = row.dbnum,
                        sesno = row.sesno,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            SUL_DB
                .query(sql)
                .await
                .map_err(|e| anyhow::anyhow!("固化{source_name}水位失败: {e}"))?
                .check()
                .map_err(|e| anyhow::anyhow!("固化{source_name}水位语句失败: {e}"))?;
        }
        if seed_from_current_database {
            // 全部批次成功才落标记；写失败只提示不阻断——后果不过是下次启动
            // 重跑一遍 fill-only 播种，幂等无损。
            if let Err(error) = write_seed_marker(source_name, watermarks.len(), &held_back).await {
                eprintln!("写播种完成标记失败（下次启动会重新播种一遍，幂等无损）: {error:#}");
            }
        }
        // 迁移 + 播种动了哪些行不逐个记账：整个镜像作废。
        Self::invalidate_mirror();
        println!(
            "增量水位回填检查完成：源={source_name}，固化 {} 个 dbnum，跳过 {} 个，耗时 {:.1} 秒",
            watermarks.len(),
            held_back.len(),
            seed_started.elapsed().as_secs_f32()
        );
        Ok(watermarks.len())
    }

    /// List registered DB files. Used by project scans to surface files that
    /// disappeared instead of silently omitting their dbnum.
    ///
    /// 只读、**不碰镜像**。整表替换是启动水化专用的一步（[`Self::hydrate_watermark_mirror`]）：
    /// 这个函数在运行期被 `/dbnums` 轮询、项目扫描与房间恢复反复调，若每次都整表替换，就有
    /// 「SELECT 已发出 → 某任务落库并作废了那一格 → 这里再用 SELECT 之前的旧行整表盖回去」
    /// 的窗口，那一格从此陈旧，而且再没有人来作废它。
    pub async fn list_registered() -> anyhow::Result<Vec<DbnumState>> {
        Ok(Self::states_from_rows(
            Self::registered_rows_on(&SUL_DB).await?,
        ))
    }

    /// 启动统一水化段（ADR-071 D7 / S6）的第一步：把整张登记表从库读回来，**一次性**替换
    /// 进程内水位镜像，再把解析好的状态交给同一段里的后续步骤（模型投影水化、启动核对）。
    ///
    /// 镜像里每格放的与 [`Self::read`] 按需装载时**一样**：`dbnum_watermark` 那一行的原样投影
    /// + `dbnum_info_table` 的旧水位（兼容回退用）；两张表各一句聚合读，不逐库点读。
    /// 只许在没有写者的时刻调（`run_cli` 里排在任何 worker / watcher 之前）：整表替换不理会
    /// 装载途中的作废，写者一活跃就可能把刚作废的那一格盖回旧值。运行期要读全表用
    /// [`Self::list_registered`]。
    pub async fn hydrate_watermark_mirror() -> anyhow::Result<Vec<DbnumState>> {
        Self::hydrate_watermark_mirror_on(&SUL_DB).await
    }

    pub(crate) async fn hydrate_watermark_mirror_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<Vec<DbnumState>> {
        let rows = Self::registered_rows_on(db).await?;
        if mirror::enabled() {
            let info_max = Self::legacy_info_watermarks_on(db).await?;
            mirror::replace(rows.iter().filter_map(|row| {
                row.dbnum.map(|dbnum| {
                    (
                        dbnum,
                        mirror::Facts {
                            row: Some(row.clone()),
                            info_max: info_max.get(&dbnum).copied(),
                        },
                    )
                })
            }));
        }
        Ok(Self::states_from_rows(rows))
    }

    /// 整张登记表的原样行（不解析）。[`Self::list_registered`] 与
    /// [`Self::hydrate_watermark_mirror_on`] 共用这一句。
    async fn registered_rows_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<Vec<StateRow>> {
        let sql = format!(
            "SELECT dbnum, state_revision, owner_project, db_type, file_name, file_path, file_size, file_latest_sesno, \
             applied_sesno, applied_sesno_time, confirmed_empty_baseline_sesno, \
             {MODEL_WATERMARK_COLUMNS}, sesno \
             FROM {WATERMARK_TABLE};"
        );
        let mut response = db
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 注册表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 注册表语句失败: {e}"))?;
        response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码 DBNUM 注册表失败: {e}"))
    }

    /// `dbnum_info_table` 里每个库的旧水位（`math::max(sesno)`），与 [`Self::read_row`] 第二句
    /// 同一口径，只是一次聚合全表——水化时给每格镜像配上兼容回退用的 `info_max`。
    async fn legacy_info_watermarks_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<BTreeMap<u32, i32>> {
        let mut response = db
            .query(LEGACY_INFO_WATERMARKS)
            .await
            .map_err(|e| anyhow::anyhow!("读取旧 DBNUM 水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取旧 DBNUM 水位语句失败: {e}"))?;
        let rows: Vec<DatabaseWatermark> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码旧 DBNUM 水位失败: {e}"))?;
        Ok(rows.into_iter().map(|row| (row.dbnum, row.sesno)).collect())
    }

    /// 登记表原样行 → 生效状态（与 [`Self::list_registered`] 一直以来的解析一致：这里没有
    /// 逐库的 info 表回退，`initialized` 只看专用行）。
    fn states_from_rows(rows: Vec<StateRow>) -> Vec<DbnumState> {
        rows.into_iter()
            .filter_map(|row| {
                let dbnum = row.dbnum?;
                let effective =
                    resolve_dbnum_applied(dbnum, Some((row.applied_sesno, row.sesno)), None);
                let applied_sesno_time = applied_time_of(&row, effective);
                let model_sesno_time = model_time_of(&row);
                Some(DbnumState {
                    dbnum,
                    state_revision: row.state_revision.unwrap_or_default(),
                    owner_project: row.owner_project.unwrap_or_default(),
                    db_type: row.db_type.unwrap_or_default(),
                    file_name: row.file_name.unwrap_or_default(),
                    file_path: row.file_path.unwrap_or_default(),
                    file_size: row.file_size.unwrap_or_default(),
                    file_latest_sesno: row.file_latest_sesno.unwrap_or_default(),
                    applied_sesno: effective.unwrap_or_default(),
                    applied_sesno_time,
                    confirmed_empty_baseline_sesno: row.confirmed_empty_baseline_sesno,
                    model_sesno: row.model_sesno.unwrap_or_default(),
                    model_sesno_time,
                    model_settled_at: row.model_settled_at,
                    model_lazy_roots: row.model_lazy_roots,
                    model_dead_roots: row.model_dead_roots,
                    model_roots_observed_at: row.model_roots_observed_at,
                    model_catalogue_sessions: row.model_catalogue_sessions.unwrap_or_default(),
                    model_room_generation: row.model_room_generation,
                    model_room_stray_generations: row
                        .model_room_stray_generations
                        .unwrap_or_default(),
                    initialized: effective.is_some(),
                })
            })
            .collect()
    }

    /// Read the raw stored row + info-table fallback for one `dbnum`.
    async fn read_row(dbnum: u32) -> anyhow::Result<(Option<StateRow>, Option<i32>)> {
        // `sesno != NONE`：info 行可能缺 sesno 字段（历史上事件对 sesno=0 的补链
        // 伪元素写出过无 sesno 的行），混进聚合会让 math::max 整条报错，进而把
        // 「读状态」变成扫描/执行的阻断点（2026-08-06 审计，1112 / 7999 实测）。
        let sql = format!(
            "SELECT dbnum, state_revision, owner_project, db_type, file_name, file_path, file_size, file_latest_sesno, \
             applied_sesno, applied_sesno_time, confirmed_empty_baseline_sesno, \
             {MODEL_WATERMARK_COLUMNS}, sesno \
             FROM {WATERMARK_TABLE}:{dbnum};\
             RETURN math::max((SELECT VALUE sesno FROM {INFO_TABLE} \
             WHERE dbnum = {dbnum} AND sesno != NONE));"
        );
        let mut response = SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 状态失败 dbnum={dbnum}: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 状态语句失败 dbnum={dbnum}: {e}"))?;
        let rows: Vec<StateRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析 DBNUM 状态失败 dbnum={dbnum}: {e}"))?;
        let info_max: Option<i32> = response
            .take(1)
            .map_err(|e| anyhow::anyhow!("解析旧 DBNUM 水位失败 dbnum={dbnum}: {e}"))?;
        Ok((rows.into_iter().next(), info_max))
    }

    /// 事实从哪来：镜像开着且命中 → 内存；否则读库，并在没有作废穿插时回填。
    async fn facts_for(dbnum: u32) -> anyhow::Result<(Option<StateRow>, Option<i32>)> {
        loop {
            match mirror::acquire(dbnum) {
                mirror::Acquire::Disabled => return Self::read_row(dbnum).await,
                mirror::Acquire::Hit(facts) => return Ok((facts.row, facts.info_max)),
                mirror::Acquire::Wait(mut changed) => {
                    changed
                        .changed()
                        .await
                        .map_err(|_| anyhow::anyhow!("水位镜像 singleflight 通知通道关闭"))?;
                }
                mirror::Acquire::Load(ticket) => match Self::read_row(dbnum).await {
                    Ok((row, info_max)) => {
                        let facts = mirror::Facts {
                            row: row.clone(),
                            info_max,
                        };
                        if mirror::publish(ticket, facts) {
                            return Ok((row, info_max));
                        }
                        // 装载期间发生写入或清理：旧值既不入缓存，也不参与本次比较。
                    }
                    Err(error) => {
                        mirror::abort(ticket);
                        return Err(error);
                    }
                },
            }
        }
    }

    /// 打开进程内水位镜像（d-227）。`run_cli` 在启动对账之后调一次；之后所有
    /// [`DbnumState::read`] / [`DbnumState::applied_sesno`] 的比对读内存，写者落库后
    /// 调 [`DbnumState::invalidate_mirrored`] 作废那一格。前提：本进程是该库的唯一写者
    /// （`data_face_signature::validate_or_claim` 的归属门）。
    pub fn enable_watermark_mirror() {
        let option = crate::options::get_db_option_ext().inner;
        let store_mode = crate::options::store_mode();
        mirror::enable(mirror::Identity {
            endpoint: if matches!(store_mode, crate::options::StoreMode::Mem) {
                // 与 `/health.sul_db.endpoint` 同一个字面值：库在进程里，没有地址可写。
                "embedded:mem".to_owned()
            } else {
                format!("{}:{}", option.v_ip.trim(), option.v_port)
            },
            namespace: option.surreal_ns.clone(),
            database: option.project_name.clone(),
            project: option.project_name,
        });
    }

    #[cfg(test)]
    pub(crate) fn disable_watermark_mirror_for_test() {
        mirror::disable();
    }

    pub fn watermark_mirror_enabled() -> bool {
        mirror::enabled()
    }

    /// 镜像里现有几格（`/health` 观测用）。
    pub fn mirrored_dbnums() -> usize {
        mirror::len()
    }

    /// 写者落库之后调：作废这一格，下一次读回填。对任何目标库都安全（见模块说明）。
    pub fn invalidate_mirrored(dbnum: u32) {
        mirror::forget(dbnum);
    }

    /// `_on(db)` 写者共用的回填出口：测试/暂存连接不影响主库镜像，生产 `SUL_DB` 写成功
    /// 后精确失效该格。调用必须位于数据库确认成功之后。
    pub(crate) fn invalidate_mirrored_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
    ) {
        if std::ptr::eq(db, &*SUL_DB) {
            Self::invalidate_mirrored(dbnum);
        }
    }

    /// 批量 / 不知道动了哪些行的写者（播种迁移、启动对账、整表清理）调：整个镜像作废。
    pub fn invalidate_mirror() {
        mirror::clear();
    }

    /// Read the effective state for one `dbnum` (with one-time migration applied
    /// in-memory). Returns `None` only when the `dbnum` has no record at all.
    pub async fn read(dbnum: u32) -> anyhow::Result<Option<DbnumState>> {
        let (row, info_max) = Self::facts_for(dbnum).await?;
        let Some(row) = row else {
            // No dedicated record. Still surface a migrated watermark if the info
            // table knows one (legacy full-parse before the watermark existed).
            let applied = resolve_dbnum_applied(dbnum, None, info_max);
            return Ok(applied.map(|applied| DbnumState {
                dbnum,
                applied_sesno: applied,
                initialized: true,
                ..Default::default()
            }));
        };

        let applied = resolve_dbnum_applied(dbnum, Some((row.applied_sesno, row.sesno)), info_max);
        let applied_sesno_time = applied_time_of(&row, applied);
        let model_sesno_time = model_time_of(&row);
        Ok(Some(DbnumState {
            dbnum: row.dbnum.unwrap_or(dbnum),
            state_revision: row.state_revision.unwrap_or_default(),
            owner_project: row.owner_project.unwrap_or_default(),
            db_type: row.db_type.unwrap_or_default(),
            file_name: row.file_name.unwrap_or_default(),
            file_path: row.file_path.unwrap_or_default(),
            file_size: row.file_size.unwrap_or_default(),
            file_latest_sesno: row.file_latest_sesno.unwrap_or_default(),
            applied_sesno: applied.unwrap_or_default(),
            applied_sesno_time,
            confirmed_empty_baseline_sesno: row.confirmed_empty_baseline_sesno,
            model_sesno: row.model_sesno.unwrap_or_default(),
            model_sesno_time,
            model_settled_at: row.model_settled_at,
            model_lazy_roots: row.model_lazy_roots,
            model_dead_roots: row.model_dead_roots,
            model_roots_observed_at: row.model_roots_observed_at,
            model_catalogue_sessions: row.model_catalogue_sessions.unwrap_or_default(),
            model_room_generation: row.model_room_generation,
            model_room_stray_generations: row.model_room_stray_generations.unwrap_or_default(),
            initialized: applied.is_some(),
        }))
    }

    /// Authoritative applied watermark for one `dbnum` (0 when uninitialized).
    ///
    /// Read-only: never writes, so it is safe to call from preview scanning.
    pub async fn applied_sesno(dbnum: u32) -> anyhow::Result<i32> {
        let (row, info_max) = Self::facts_for(dbnum).await?;
        Ok(
            resolve_dbnum_applied(dbnum, row.map(|r| (r.applied_sesno, r.sesno)), info_max)
                .unwrap_or(0),
        )
    }

    /// 读登记身份并对一次观察下裁决（纯读，不写任何东西）。
    ///
    /// 这是全仓判定文件异常的唯一入口：读状态、比判据、分类，一次做完，谁都不用
    /// 再自己拼 [`check_file_against_state`] 的六个参数——过去四个调用点各拼一遍，
    /// 其中两个干脆忘了拼。
    ///
    /// 读失败**必须上浮**，不能吞成「从未登记」。吞掉的话 `applied_sesno` 退化成 0，
    /// 回退检不出来，`needs_initial_load` 还会判成「首次导入」——一次数据库抖动就能
    /// 让一个跑了很久的库被当成新库重新全量解析。各调用方自己决定是跳过还是失败。
    pub async fn classify_scan(obs: &FileObservation) -> anyhow::Result<ScanVerdict> {
        let prior = Self::read(obs.dbnum).await?;

        // 归属校验先于其余判据：登记行归别的项目时，后面那些「回退 / 类型变更」
        // 都是拿两个项目的文件在对比，结论没有意义（实测里 acpsys 就被判成了
        // 「回退」——判据是错的，只是结论恰好安全）。空的 owner_project 是本字段
        // 引入之前的旧行，不做校验，等它被自己的项目扫一次自然补上。
        if let Some(state) = prior.as_ref() {
            let stored = state.owner_project.trim();
            let observed = obs.project.trim();
            if !stored.is_empty() && !observed.is_empty() && !stored.eq_ignore_ascii_case(observed)
            {
                return Ok(ScanVerdict {
                    anomaly: Some(FileAnomaly::ForeignProject {
                        stored_project: stored.to_string(),
                        observed_project: observed.to_string(),
                    }),
                    prior,
                });
            }
        }

        let anomaly = check_file_against_state(
            prior
                .as_ref()
                .map(|s| s.db_type.as_str())
                .filter(|s| !s.is_empty()),
            prior
                .as_ref()
                .map(|s| s.file_path.as_str())
                .filter(|s| !s.is_empty()),
            prior.as_ref().map(|s| s.applied_sesno).unwrap_or(0),
            &obs.db_type,
            &obs.file_path,
            obs.file_latest_sesno,
        );
        // 回退卡要说的是两条保存的时刻（plant-ui ADR-0019 Q6）。判据函数给不出它们，
        // 这里补：文件端**现读一页**——文件就在那儿，过去空着只是因为阻断分支从不走
        // 解析那条路；已应用端只能取水位表存量，回退之后那一页在当前文件里已经没了。
        // 只有真判成回退才付这一页 IO，正常扫描一次也不多读。
        let anomaly = match anomaly {
            Some(FileAnomaly::Rollback {
                file_latest_sesno,
                applied_sesno,
                ..
            }) => Some(FileAnomaly::Rollback {
                file_latest_sesno,
                applied_sesno,
                file_latest_sesno_time: crate::data_interface::manual_update::session_time_rfc3339(
                    &obs.project,
                    std::path::Path::new(&obs.file_path),
                    file_latest_sesno,
                ),
                applied_sesno_time: prior
                    .as_ref()
                    .and_then(|state| state.applied_sesno_time.clone()),
            }),
            other => other,
        };
        Ok(ScanVerdict { prior, anomaly })
    }

    /// 按裁决落库这次观察：阻断只写观察值，否则连文件身份一并刷新。
    ///
    /// 这是落库观察的**唯一入口**——底下那两条语句都是私有的，模块外拿不到。
    /// 「选哪条」不能有第二个决定点：选错一次，异常就把自己抹掉，而且是静默的
    /// ——下一轮扫描一切正常，只是那个库再也不会被拦下来了。
    pub async fn record_observation(
        obs: &FileObservation,
        verdict: &ScanVerdict,
    ) -> anyhow::Result<()> {
        // 归属不符、或文件名换了（观察到的是另一份文件）时**一个字都不写**。阻断路径本来
        // 也只写观察值，但那三个字段（file_size / file_latest_sesno / scanned_at）恰恰就是
        // 被写脏的那几个：行还挂着 amssys 的身份，file_latest_sesno 却成了 zdjsys 的 52，
        // 面板上看到的是一个不存在的文件状态。
        if let Some(anomaly) = verdict
            .anomaly
            .as_ref()
            .filter(|anomaly| anomaly.observed_another_file())
        {
            if anomaly.is_foreign_project() {
                log::warn!(
                    "dbnum={} 的登记行不归项目 {} 所有，跳过落库（连观察值也不写）",
                    obs.dbnum,
                    obs.project
                );
            } else {
                log::warn!(
                    "dbnum={} 现场文件 {} 不是登记的那份文件（文件名不符），跳过落库（连观察值也不写）",
                    obs.dbnum,
                    obs.file_path
                );
            }
            return Ok(());
        }
        let _write_guard = DBNUM_STATE_WRITE_GATE.read().await;
        if verdict.blocked() {
            Self::record_blocked_observation(obs).await
        } else {
            Self::record_scan(obs).await
        }
    }

    /// 同项目同 dbnum 出现多个文件时撤销旧的可寻址身份，但保留水位、类型与观察值。
    ///
    /// 这堵住“两轮扫描”漏洞：上一轮唯一候选留下的 `file_path` 不能在本轮已判
    /// Duplicate 后继续被 CATA closure 消费。副本消失后，唯一候选的正常
    /// [`Self::record_observation`] 会重新写回完整身份；无需新表或新字段。
    pub(crate) async fn block_duplicate_file_identity(
        dbnum: u32,
        project: &str,
    ) -> anyhow::Result<()> {
        let _write_guard = DBNUM_STATE_WRITE_GATE.read().await;
        let Some(state) = Self::read(dbnum).await? else {
            return Ok(());
        };
        let owner = state.owner_project.trim();
        if !owner.is_empty() && !owner.eq_ignore_ascii_case(project.trim()) {
            anyhow::bail!(
                "dbnum={dbnum} 的登记行属于项目 {}，拒绝由项目 {project} 清除重复身份",
                state.owner_project
            );
        }
        let sql = render_duplicate_identity_block(dbnum);
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("阻断重复文件身份失败 dbnum={dbnum}: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("阻断重复文件身份语句失败 dbnum={dbnum}: {e}"))?;
        Self::invalidate_mirrored(dbnum);
        Ok(())
    }

    /// 测试夹具专用：无条件写入文件身份，绕开裁决。
    ///
    /// 只给「需要把某个身份摆进库里当前置条件」的 live 测试用（例如故意重放一个
    /// 更旧的基线，那种观察会被判成回退，走正门就写不进身份）。名字取得这么长
    /// 是故意的：生产代码里出现它，评审一眼就能看见。
    #[cfg(test)]
    pub(crate) async fn force_scan_identity_for_test(obs: &FileObservation) -> anyhow::Result<()> {
        Self::record_scan(obs).await
    }

    /// Persist a scan observation WITHOUT touching the applied watermark.
    ///
    /// Refreshes only the file-identity + observation fields and `scanned_at`
    /// (ADR-001: "预览扫描可以更新文件身份、属性、file_latest_sesno 和 scanned_at").
    /// `applied_sesno` is never written here, so preview scans leave the
    /// authoritative watermark unchanged; it is established durably only on the
    /// success path via [`Self::advance_applied`], while reads resolve it through
    /// the one-time migration in [`resolve_migrated_applied_sesno`].
    ///
    /// 私有：外面只能经 [`Self::record_observation`] 落库，由裁决决定走这条还是
    /// 只写观察值的那条。
    async fn record_scan(obs: &FileObservation) -> anyhow::Result<()> {
        let modified_expr = obs
            .file_modified_at
            .as_deref()
            .map(|s| format!("type::datetime('{}')", escape_surql_str(s)))
            .unwrap_or_else(|| "time::now()".to_string());

        let sql = format!(
            "UPSERT {WATERMARK_TABLE}:{dbnum} SET \
             dbnum = {dbnum}, owner_project = '{owner_project}', \
             db_type = '{db_type}', file_name = '{file_name}', \
             file_path = '{file_path}', file_size = {file_size}, \
             file_latest_sesno = {file_latest_sesno}, file_modified_at = {modified_expr}, \
             state_revision = (state_revision ?? 0) + 1, \
             scanned_at = time::now(), updated_at = time::now();",
            dbnum = obs.dbnum,
            owner_project = escape_surql_str(&obs.project),
            db_type = escape_surql_str(&obs.db_type),
            file_name = escape_surql_str(&obs.file_name),
            file_path = escape_surql_str(&obs.file_path),
            file_size = obs.file_size,
            file_latest_sesno = obs.file_latest_sesno,
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("记录扫描观察失败 dbnum={}: {}", obs.dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("记录扫描观察语句失败 dbnum={}: {}", obs.dbnum, e))?;
        Self::invalidate_mirrored(obs.dbnum);
        Ok(())
    }

    /// 阻断类异常下的扫描落库：**只写观察值，不写文件身份**。
    ///
    /// `db_type` / `file_name` / `file_path` 是 [`check_file_against_state`] 的判据。
    /// 判为阻断（回退 / 类型变更 / …）时若照常 UPSERT 它们，登记基准就被现场的
    /// 那个文件顶掉了——下一轮再扫，`stored_db_type` 已经等于 `observed_db_type`，
    /// 同一个异常再也检不出来，异常把自己抹掉了。
    ///
    /// 观察值（大小、文件最新会话号、扫描时刻）照写：人从面板上仍要看得见
    /// 「现场那个文件长什么样」，这也是判断阻断是否已被人工处理掉的依据。
    /// 与 `record_scan` 一样，永不触碰 `applied_sesno`（ADR-001）。
    ///
    /// 私有：外面只能经 [`Self::record_observation`] 落库。
    async fn record_blocked_observation(obs: &FileObservation) -> anyhow::Result<()> {
        let sql = format!(
            "UPSERT {WATERMARK_TABLE}:{dbnum} SET dbnum = {dbnum}, \
             file_size = {file_size}, file_latest_sesno = {file_latest_sesno}, \
             state_revision = (state_revision ?? 0) + 1, \
             scanned_at = time::now(), updated_at = time::now();",
            dbnum = obs.dbnum,
            file_size = obs.file_size,
            file_latest_sesno = obs.file_latest_sesno,
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("记录阻断观察失败 dbnum={}: {}", obs.dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("记录阻断观察语句失败 dbnum={}: {}", obs.dbnum, e))?;
        Self::invalidate_mirrored(obs.dbnum);
        Ok(())
    }

    /// Advance the applied watermark for one `dbnum` after a data batch succeeds.
    ///
    /// Monotonic (`math::max`, never regresses) and only ever called on the success
    /// path. Mirrors the legacy `sesno` field for backward compatibility.
    ///
    /// 语句本身**不在这里渲染**：生产路径（窗口收口 / 基线事务）走的是
    /// `increment_tail::render_watermark_advance`，那句注释写着「只渲染一处，
    /// 窗口与基线事务不可能漂移」——这个函数曾经是同一条 SQL 的第二份拷贝，
    /// 那句话在两者之间早就不成立了。现在它只是那一处的独立调用外壳。
    pub async fn advance_applied(
        dbnum: u32,
        end_sesno: i32,
        end_sesno_time: Option<&str>,
    ) -> anyhow::Result<()> {
        let sql = crate::data_interface::increment_tail::render_watermark_advance(
            dbnum,
            end_sesno,
            end_sesno_time,
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("推进应用水位失败 dbnum={}: {}", dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("推进应用水位语句失败 dbnum={}: {}", dbnum, e))?;
        Self::invalidate_mirrored(dbnum);
        Ok(())
    }
}

fn render_duplicate_identity_block(dbnum: u32) -> String {
    format!(
        "UPDATE {WATERMARK_TABLE}:{dbnum} SET file_name = '', file_path = '', \
         state_revision = (state_revision ?? 0) + 1, scanned_at = time::now(), updated_at = time::now();"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    static MIRROR_TEST_GUARD: Mutex<()> = Mutex::new(());

    fn mirror_identity(database: &str) -> super::mirror::Identity {
        super::mirror::Identity {
            endpoint: "127.0.0.1:8000".into(),
            namespace: "test".into(),
            database: database.into(),
            project: database.into(),
        }
    }

    fn mirrored_facts(sesno: i32) -> super::mirror::Facts {
        super::mirror::Facts {
            row: Some(super::StateRow {
                applied_sesno: Some(sesno),
                ..Default::default()
            }),
            info_max: None,
        }
    }

    fn fact_sesno(facts: super::mirror::Facts) -> i32 {
        facts.row.and_then(|row| row.applied_sesno).unwrap_or(0)
    }

    #[test]
    fn mirror_identity_and_inflight_invalidation_never_publish_stale_facts() {
        let _guard = MIRROR_TEST_GUARD.lock().unwrap();
        super::mirror::disable();
        super::mirror::enable(mirror_identity("db-a"));

        let first = match super::mirror::acquire(8190) {
            super::mirror::Acquire::Load(ticket) => ticket,
            _ => panic!("first miss must load"),
        };
        super::mirror::forget(8190);
        assert!(
            !super::mirror::publish(first, mirrored_facts(10)),
            "a read invalidated in flight must not publish"
        );
        let second = match super::mirror::acquire(8190) {
            super::mirror::Acquire::Load(ticket) => ticket,
            _ => panic!("invalidation must make the next caller reload"),
        };
        assert!(super::mirror::publish(second, mirrored_facts(11)));
        match super::mirror::acquire(8190) {
            super::mirror::Acquire::Hit(facts) => assert_eq!(fact_sesno(facts), 11),
            _ => panic!("fresh facts must be cached"),
        }

        super::mirror::enable(mirror_identity("db-b"));
        assert!(matches!(
            super::mirror::acquire(8190),
            super::mirror::Acquire::Load(_)
        ));
        super::mirror::disable();
    }

    async fn singleflight_value(dbnum: u32, loads: Arc<AtomicUsize>) -> i32 {
        loop {
            match super::mirror::acquire(dbnum) {
                super::mirror::Acquire::Disabled => panic!("mirror unexpectedly disabled"),
                super::mirror::Acquire::Hit(facts) => return fact_sesno(facts),
                super::mirror::Acquire::Wait(mut changed) => {
                    changed.changed().await.expect("loader remains alive");
                }
                super::mirror::Acquire::Load(ticket) => {
                    loads.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    if super::mirror::publish(ticket, mirrored_facts(77)) {
                        return 77;
                    }
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_hundred_same_database_misses_share_one_load() {
        let _guard = MIRROR_TEST_GUARD.lock().unwrap();
        super::mirror::disable();
        super::mirror::enable(mirror_identity("singleflight"));
        let loads = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..100 {
            tasks.push(tokio::spawn(singleflight_value(7997, Arc::clone(&loads))));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), 77);
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        super::mirror::disable();
    }

    /// 回归：装载票随任务消失（`read_row` 途中被取消，publish/abort 都没走到）时
    /// 必须把格子交还——否则 `loading` 永真，这个 dbnum 之后所有 acquire 全停在
    /// `Wait` 上挂死（目录库批量重建时请求超时被取消后整库读路径锁死的事故根因）。
    #[tokio::test(flavor = "current_thread")]
    async fn a_ticket_dropped_mid_load_returns_the_slot() {
        let _guard = MIRROR_TEST_GUARD.lock().unwrap();
        super::mirror::disable();
        super::mirror::enable(mirror_identity("drop-ticket"));

        let ticket = match super::mirror::acquire(7022) {
            super::mirror::Acquire::Load(ticket) => ticket,
            _ => panic!("first miss must load"),
        };
        drop(ticket);

        let ticket = match super::mirror::acquire(7022) {
            super::mirror::Acquire::Load(ticket) => ticket,
            _ => panic!("a dropped ticket must return the slot for the next loader"),
        };
        assert!(super::mirror::publish(ticket, mirrored_facts(9)));
        match super::mirror::acquire(7022) {
            super::mirror::Acquire::Hit(facts) => assert_eq!(fact_sesno(facts), 9),
            _ => panic!("published facts must be cached"),
        }
        super::mirror::disable();
    }

    /// 同一事故链的另一半：`Wait` 上的等待者要在票被丢弃时被唤醒并接管装载，
    /// 而不是永远挂在通知通道上。
    #[tokio::test(flavor = "current_thread")]
    async fn a_waiter_takes_over_after_the_loader_ticket_is_dropped() {
        let _guard = MIRROR_TEST_GUARD.lock().unwrap();
        super::mirror::disable();
        super::mirror::enable(mirror_identity("drop-wakes"));

        let ticket = match super::mirror::acquire(7352) {
            super::mirror::Acquire::Load(ticket) => ticket,
            _ => panic!("first miss must load"),
        };
        let waiter = tokio::spawn(singleflight_value(7352, Arc::new(AtomicUsize::new(0))));
        tokio::task::yield_now().await;
        drop(ticket);

        let value = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("waiter must wake when the loader ticket is dropped")
            .unwrap();
        assert_eq!(value, 77);
        super::mirror::disable();
    }

    /// ADR-071 D7 / S6：启动水化 = 反序列化。写库 →（进程重启）→ 水化 → 镜像里每格与库行
    /// **逐字段**相等（含 info 表旧水位的兼容回退），之后的读命中镜像、不再回库——库里悄悄改了
    /// 也读不到，直到写者作废那一格。
    #[tokio::test]
    async fn startup_hydration_mirrors_every_registered_row_field_by_field() {
        let _guard = MIRROR_TEST_GUARD.lock().unwrap();
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("hydrate").await.unwrap();
        db.query(
            "CREATE dbnum_watermark:7997 SET dbnum=7997, db_type='DESI', file_name='ams7997', \
             owner_project='AMS', state_revision=9, applied_sesno=26, model_sesno=26;\
             CREATE dbnum_watermark:8000 SET dbnum=8000, db_type='DESI', applied_sesno=12;\
             CREATE dbnum_info_table SET dbnum=8000, sesno=11;\
             CREATE dbnum_info_table SET dbnum=8000, sesno=15;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        super::mirror::disable();
        super::mirror::enable(mirror_identity("hydrate"));

        let mut states = super::DbnumState::hydrate_watermark_mirror_on(&db)
            .await
            .unwrap();
        states.sort_by_key(|state| state.dbnum);
        assert_eq!(
            states.iter().map(|state| state.dbnum).collect::<Vec<_>>(),
            vec![7997, 8000],
            "解析结果与 list_registered 同一份"
        );
        assert_eq!(super::mirror::len(), 2, "登记表有几行镜像就有几格");

        let facts = match super::mirror::acquire(7997) {
            super::mirror::Acquire::Hit(facts) => facts,
            _ => panic!("水化之后第一次读就该命中"),
        };
        let row = facts.row.expect("库里那一行原样在镜像里");
        assert_eq!(row.dbnum, Some(7997));
        assert_eq!(row.db_type.as_deref(), Some("DESI"));
        assert_eq!(row.file_name.as_deref(), Some("ams7997"));
        assert_eq!(row.owner_project.as_deref(), Some("AMS"));
        assert_eq!(row.state_revision, Some(9));
        assert_eq!(row.applied_sesno, Some(26));
        assert_eq!(row.model_sesno, Some(26));
        assert_eq!(facts.info_max, None, "没有 info 行的库回退值就是 NONE");

        let facts = match super::mirror::acquire(8000) {
            super::mirror::Acquire::Hit(facts) => facts,
            _ => panic!("水化之后第一次读就该命中"),
        };
        let row = facts.row.expect("库里那一行原样在镜像里");
        assert_eq!(row.applied_sesno, Some(12));
        assert_eq!(row.model_sesno, None, "库里没写过的列是「未观察」，不是 0");
        assert_eq!(
            facts.info_max,
            Some(15),
            "info 表旧水位与 read_row 第二句同一口径：math::max(sesno)"
        );

        // 不经写者作废的库改动，镜像看不见——这正是「比对读镜像」的含义；作废后才回库。
        db.query("UPDATE dbnum_watermark:7997 SET applied_sesno=27;")
            .await
            .unwrap()
            .check()
            .unwrap();
        match super::mirror::acquire(7997) {
            super::mirror::Acquire::Hit(facts) => assert_eq!(fact_sesno(facts), 26),
            _ => panic!("没人作废，那一格还在"),
        }
        super::mirror::forget(7997);
        assert!(matches!(
            super::mirror::acquire(7997),
            super::mirror::Acquire::Load(_)
        ));
        super::mirror::disable();
    }

    /// ADR-073 D2：库上只剩两枚水位，恢复阶段只有数据 → 模型 → 完成三档；房间归属随根生成，
    /// 写在模型水位的承诺里，没有自己的一档。
    #[test]
    fn two_watermarks_recover_only_the_missing_stage() {
        use super::{DbnumState, RecoveryStage as Stage};
        let mut state = DbnumState {
            initialized: true,
            file_latest_sesno: 12,
            applied_sesno: 12,
            model_sesno: 12,
            ..Default::default()
        };
        assert_eq!(state.recovery_stage(), Stage::Complete);
        for _ in 0..3 {
            // unchanged restart preserves the serialized completion state
            state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            assert_eq!(state.recovery_stage(), Stage::Complete);
        }
        state.model_sesno = 11;
        assert_eq!(state.recovery_stage(), Stage::Model);
        state.applied_sesno = 11;
        assert_eq!(state.recovery_stage(), Stage::Data);
        state.model_sesno = 12;
        assert_eq!(state.recovery_stage(), Stage::InvalidWatermarks);
        state.model_sesno = 11;
        state.file_latest_sesno = 10;
        assert_eq!(state.recovery_stage(), Stage::InvalidWatermarks);
        let cold = DbnumState {
            initialized: false,
            ..Default::default()
        };
        assert_eq!(cold.recovery_stage(), Stage::Data);
    }

    use super::*;

    #[test]
    fn duplicate_identity_block_removes_only_the_locator_fields() {
        let sql = render_duplicate_identity_block(7997);
        assert!(sql.contains("file_name = ''"), "{sql}");
        assert!(sql.contains("file_path = ''"), "{sql}");
        assert!(!sql.contains("applied_sesno"), "{sql}");
        assert!(!sql.contains("db_type ="), "{sql}");
    }

    #[test]
    fn startup_schema_covers_every_increment_state_table() {
        for table in [
            WATERMARK_TABLE,
            INFO_TABLE,
            crate::data_interface::write_state::WRITE_STATE_TABLE,
            "queue_control",
            crate::data_interface::model_scope_regen::SCOPE_REGEN_TABLE,
        ] {
            assert!(
                INCREMENT_STATE_SCHEMA.contains(&format!("IF NOT EXISTS {table} ")),
                "missing startup definition for {table}"
            );
        }
    }

    #[test]
    fn missing_watermark_table_uses_current_pe_sessions_for_compatibility() {
        let before = serde_json::json!({"tables": {"pe": "DEFINE TABLE pe SCHEMALESS"}});
        let after = serde_json::json!({
            "tables": {
                "pe": "DEFINE TABLE pe SCHEMALESS",
                "dbnum_watermark": "DEFINE TABLE dbnum_watermark SCHEMALESS"
            }
        });

        let missing = !database_has_table(&before, WATERMARK_TABLE);
        let existing = !database_has_table(&after, WATERMARK_TABLE);
        let seed = should_seed_from_current_database(missing, 0, false);
        let keep_legacy = should_seed_from_current_database(existing, 7, true);
        assert!(migration_watermark_source(seed).0.contains("FROM pe"));
        assert!(
            migration_watermark_source(seed)
                .0
                .contains("GROUP BY dbnum")
        );
        assert!(
            migration_watermark_source(keep_legacy)
                .0
                .contains("FROM dbnum_info_table")
        );
    }

    /// 崩溃窗口：上一轮在建表之后、播种完成之前死掉。空表（一批都没写上）与
    /// 半途表（部分批次写上了，行数 > 0 但完成标记没落）都必须继续走 pe 源；
    /// 只有「有行且标记在」的库才算播种完成、回到 info 源快路径。
    #[test]
    fn an_interrupted_seed_resumes_from_current_pe_data() {
        // 表缺失：无条件走 pe 源。
        assert!(should_seed_from_current_database(true, 0, false));
        // 表存在但一行都没有：建表后第一批就没写上。
        assert!(should_seed_from_current_database(false, 0, false));
        // 部分批次写上了（行数 > 0）但完成标记缺失：继续补跑 pe 源。
        assert!(should_seed_from_current_database(false, 500, false));
        // 有行且标记在：播种完成，走 info 源快路径。
        assert!(!should_seed_from_current_database(false, 1, true));
        assert!(!should_seed_from_current_database(false, 300, true));
        // 标记在但表被清空/重建：行数为 0 优先，重新播种（随后标记会被重写）。
        assert!(should_seed_from_current_database(false, 0, true));
        // 标记在但整张表被删了：缺表优先，重新播种。
        assert!(should_seed_from_current_database(true, 0, true));
    }

    /// 播种只按下「任务在飞」的库（半截基线的 max sesno 不是应用水位）。此前还按
    /// `count(pe) != sum(dbnum_info_table.count)` 扣一批「可疑库」并在比对失败时整轮不播——那道
    /// 对账随 ADR-071 D6 退役（`count` 现在是任务收尾序列化的活元素数，与 pe 行数本就不必相等）。
    #[test]
    fn seeding_holds_back_only_in_flight_dbnums_and_no_longer_reconciles_pe_with_info() {
        let candidates = vec![
            DatabaseWatermark {
                dbnum: 7997,
                sesno: 84,
            },
            DatabaseWatermark {
                dbnum: 8000,
                sesno: 41,
            },
            DatabaseWatermark {
                dbnum: 8191,
                sesno: 169,
            },
        ];
        let protected = BTreeSet::from([8000u32]);
        let (seedable, held_back) = partition_seedable(candidates, &protected);
        assert_eq!(
            seedable.iter().map(|row| row.dbnum).collect::<Vec<_>>(),
            vec![7997, 8191]
        );
        assert_eq!(held_back, vec![8000]);

        let source = include_str!("dbnum_state.rs");
        let body = source
            .split_once("pub async fn ensure_increment_state_storage(")
            .expect("ensure_increment_state_storage 必须存在")
            .1
            .split_once("\n    /// List registered DB files")
            .expect("它之后是 list_registered")
            .0;
        let partition_at = body
            .find("partition_seedable(watermarks, &protected)")
            .expect("播种候选按任务在飞过滤");
        let upsert_at = body
            .find("for chunk in watermarks.chunks(500)")
            .expect("固化循环必须还在");
        assert!(partition_at < upsert_at, "过滤要发生在固化之前: {body}");
        for retired in [
            "seed_suspect_dbnums(",
            "fetch_seed_integrity_counts(",
            "seed_integrity_warnings(",
            "dbnum_info_table",
        ] {
            assert!(
                !body.contains(retired),
                "播种不再拿 dbnum_info_table 对账: {retired}"
            );
        }
    }

    #[test]
    fn migration_prefers_established_applied_over_legacy_and_info() {
        // Already established → never re-mix other sources.
        assert_eq!(
            resolve_migrated_applied_sesno(Some(50), Some(99), Some(120)),
            Some(50)
        );
    }

    #[test]
    fn migration_inherits_legacy_watermark_when_not_established() {
        assert_eq!(
            resolve_migrated_applied_sesno(None, Some(99), Some(120)),
            Some(99)
        );
    }

    #[test]
    fn migration_falls_back_to_info_table_only_when_no_watermark() {
        assert_eq!(
            resolve_migrated_applied_sesno(None, None, Some(120)),
            Some(120)
        );
    }

    #[test]
    fn migration_none_when_all_sources_absent() {
        assert_eq!(resolve_migrated_applied_sesno(None, None, None), None);
    }

    #[test]
    fn an_existing_failed_state_never_inherits_the_info_table_watermark() {
        assert_eq!(resolve_read_applied(Some((None, None)), Some(120)), None);
        assert_eq!(resolve_read_applied(None, Some(120)), Some(120));
        assert_eq!(
            resolve_read_applied(Some((None, Some(99))), Some(120)),
            Some(99)
        );
    }

    #[test]
    fn migration_preserves_zero_applied() {
        // An established applied_sesno of 0 is a real value, not "absent".
        assert_eq!(
            resolve_migrated_applied_sesno(Some(0), Some(99), Some(120)),
            Some(0)
        );
    }

    /// `blocks()` 是阻断与否的唯一权威，自动路径与手动预览都读它，所以每一个变体
    /// 的取值都要在这里说死。用 `match` 而不是逐个 `assert!`：新增一种异常时这里
    /// 编译不过，作者必须显式选边，而不是让它默默落进「不阻断」。
    #[test]
    fn every_anomaly_declares_whether_it_blocks() {
        let cases = [
            FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            },
            FileAnomaly::PathMigrated {
                old_path: "/old".into(),
                new_path: "/new".into(),
            },
            FileAnomaly::NameChanged {
                old_path: "/p/ams8000_0001".into(),
                new_path: "/p/zdj8000_0001".into(),
            },
            FileAnomaly::TypeChanged {
                stored_db_type: "DESI".into(),
                observed_db_type: "CATA".into(),
            },
            FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            },
            FileAnomaly::Missing {
                path: "/gone".into(),
            },
            FileAnomaly::ForeignProject {
                stored_project: "AvevaMarineSample".into(),
                observed_project: "ZDJ".into(),
            },
        ];
        for anomaly in &cases {
            let expected = match anomaly {
                // 良性搬家：登记路径跟着更新即可。
                FileAnomaly::PathMigrated { .. } => false,
                // 其余几种都动了「这个 dbnum 对应哪个文件」这件事的根基。
                FileAnomaly::Rollback { .. }
                | FileAnomaly::NameChanged { .. }
                | FileAnomaly::TypeChanged { .. }
                | FileAnomaly::Duplicate { .. }
                | FileAnomaly::Missing { .. }
                | FileAnomaly::ForeignProject { .. } => true,
            };
            assert_eq!(anomaly.blocks(), expected, "{anomaly:?} 的阻断口径不符");
        }
    }

    /// 「哪些异常转整库重建」是策略红线（ADR-021）：自动路径、手动入队与
    /// worker 执行体都拿 [`FileAnomaly::requires_reinit`] 当唯一裁决，这里
    /// 逐变体点名（不留 `_ =>` 兜底，理由同 `blocks()` 那条）：只有回退转
    /// 重建，其余异常自动清库等于替人拍板挑文件。
    #[test]
    fn only_a_rollback_requires_reinit() {
        let cases = [
            FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            },
            FileAnomaly::PathMigrated {
                old_path: "/old".into(),
                new_path: "/new".into(),
            },
            FileAnomaly::TypeChanged {
                stored_db_type: "DESI".into(),
                observed_db_type: "SYST".into(),
            },
            FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            },
            FileAnomaly::Missing {
                path: "/gone".into(),
            },
            FileAnomaly::ForeignProject {
                stored_project: "AMS".into(),
                observed_project: "ZDJ".into(),
            },
            FileAnomaly::NameChanged {
                old_path: "/p/ams8000_0001".into(),
                new_path: "/p/ams8000_0002".into(),
            },
        ];
        for anomaly in &cases {
            let expected = match anomaly {
                FileAnomaly::Rollback { .. } => true,
                FileAnomaly::PathMigrated { .. }
                | FileAnomaly::NameChanged { .. }
                | FileAnomaly::TypeChanged { .. }
                | FileAnomaly::Duplicate { .. }
                | FileAnomaly::Missing { .. }
                | FileAnomaly::ForeignProject { .. } => false,
            };
            assert_eq!(
                anomaly.requires_reinit(),
                expected,
                "{anomaly:?} 的整库重建口径不符"
            );
        }
    }

    #[test]
    fn file_rollback_is_rejected() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/p/desi_1"),
            120,
            "DESI",
            "/p/desi_1",
            80,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            })
        );
    }

    #[test]
    fn file_rollback_takes_precedence_over_path_change() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "DESI",
            "/new/path",
            80,
        );
        assert!(matches!(anomaly, Some(FileAnomaly::Rollback { .. })));
    }

    #[test]
    fn legal_path_migration_is_detected() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "DESI",
            "/new/path",
            130,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::PathMigrated {
                old_path: "/old/path".to_string(),
                new_path: "/new/path".to_string(),
            })
        );
    }

    /// 身份是 dbnum + 约定文件名：只换目录、或母库换成它自己的 extract 才是迁移；文件名
    /// 换了就是另一份文件，先于回退判 `NameChanged`——拿另一份文件的会话号判回退，会把一份
    /// 不相干的文件清库重解析。
    #[test]
    fn a_renamed_file_is_another_file_not_a_migration() {
        let check = |stored: &str, observed: &str, sesno: i32| {
            check_file_against_state(Some("DESI"), Some(stored), 120, "DESI", observed, sesno)
        };
        let name_changed = |stored: &str, observed: &str| {
            Some(FileAnomaly::NameChanged {
                old_path: stored.to_string(),
                new_path: observed.to_string(),
            })
        };

        // 只换目录（文件名大小写不同也算同名）：迁移。
        for (stored, observed) in [
            (r"D:\ams000\AMS8000_0001", r"E:\moved\ams000\ams8000_0001"),
            ("/p/ams000/ams8000_0001", "/q/ams000/ams8000_0001"),
            // 母库 → 它自己的 extract（ADR-028）。
            (r"D:\ams000\ams7355", r"D:\ams000\ams7355_0001"),
        ] {
            assert!(
                matches!(
                    check(stored, observed, 130),
                    Some(FileAnomaly::PathMigrated { .. })
                ),
                "{stored} → {observed} 应是迁移"
            );
        }

        // 换前缀、换序号、extract 退回母库、换库号、换系统库前缀：都是另一份文件。
        for (stored, observed) in [
            (r"D:\ams000\ams8000_0001", r"D:\ams000\zdj8000_0001"),
            (r"D:\ams000\ams8000_0001", r"D:\ams000\ams8000_0002"),
            (r"D:\ams000\ams7355_0001", r"D:\ams000\ams7355"),
            (r"D:\ams000\ams8000_0001", r"D:\ams000\ams8001_0001"),
            (r"D:\ams000\amssys", r"D:\ams000\zdjsys"),
        ] {
            assert_eq!(check(stored, observed, 130), name_changed(stored, observed));
        }
        // 先于回退：另一份文件的会话号再小，也不许转成整库重建。
        assert_eq!(
            check(r"D:\ams000\ams8000_0001", r"D:\ams000\zdj8000_0001", 80),
            name_changed(r"D:\ams000\ams8000_0001", r"D:\ams000\zdj8000_0001")
        );
    }

    #[test]
    fn db_type_change_is_blocked() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "CATA",
            "/new/path",
            130,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::TypeChanged {
                stored_db_type: "DESI".to_string(),
                observed_db_type: "CATA".to_string(),
            })
        );
    }

    #[test]
    fn normal_file_reports_no_anomaly() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/p/desi_1"),
            120,
            "DESI",
            "/p/desi_1",
            130,
        );
        assert_eq!(anomaly, None);
    }

    #[test]
    fn escape_handles_windows_paths_and_quotes() {
        assert_eq!(
            escape_surql_str(r"C:\proj\d'esi"),
            r"C:\\proj\\d\'esi".to_string()
        );
    }

    /// F6 · T605（live）：扫描观察落库，但**永不**触碰应用水位。
    ///
    /// 上面那批纯函数用例已经覆盖了 Rollback / PathMigrated 的判定口径；这里补的是
    /// 落库那一半：`record_scan` 只写文件身份与观察字段，`applied_sesno` 必须纹丝不动
    /// （ADR-001）。换成更旧的会话文件时同样不得让水位倒退，只把观察值照实记下来，
    /// 由调用方拿判定结果去阻断该 dbnum。
    ///
    /// 用 `999_999_001` 这个不会出现在真实工程里的 dbnum，跑完即清理。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_record_scan_never_moves_the_applied_watermark() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let dbnum = 999_999_001u32;
        let cleanup = format!("delete {WATERMARK_TABLE}:{dbnum};");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("clear stale fixture")
            .check()
            .expect("valid pre-cleanup");

        // 先把水位建立在 50——模拟这个 dbnum 已经成功应用到第 50 个会话。
        DbnumState::advance_applied(dbnum, 50, None)
            .await
            .expect("establish watermark");

        // 扫描到文件被移动且带来更新的会话：身份字段该更新，水位不该动。
        DbnumState::record_scan(&FileObservation {
            dbnum,
            project: "TestProject".to_string(),
            db_type: "DESI".to_string(),
            file_name: "zz_t605.dbnum".to_string(),
            file_path: r"D:\zz_t605\moved\desi_1".to_string(),
            file_size: 4096,
            file_latest_sesno: 60,
            file_modified_at: None,
        })
        .await
        .expect("record moved-file scan");

        let moved = DbnumState::read(dbnum)
            .await
            .expect("read state after move")
            .expect("state exists");

        // 再扫到一个更旧的文件（回退观察）：观察值照实写，水位仍不得倒退。
        DbnumState::record_scan(&FileObservation {
            dbnum,
            project: "TestProject".to_string(),
            db_type: "DESI".to_string(),
            file_name: "zz_t605.dbnum".to_string(),
            file_path: r"D:\zz_t605\moved\desi_1".to_string(),
            file_size: 2048,
            file_latest_sesno: 10,
            file_modified_at: None,
        })
        .await
        .expect("record rolled-back scan");

        let rolled_back = DbnumState::read(dbnum)
            .await
            .expect("read state after rollback")
            .expect("state exists");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(moved.file_path, r"D:\zz_t605\moved\desi_1");
        assert_eq!(moved.file_latest_sesno, 60);
        assert_eq!(moved.applied_sesno, 50, "预览扫描不得推进 applied_sesno");

        assert_eq!(
            rolled_back.file_latest_sesno, 10,
            "观察字段应如实记录更旧的文件"
        );
        assert_eq!(
            rolled_back.applied_sesno, 50,
            "回退文件不得让 applied_sesno 倒退"
        );

        // 判定口径与落库状态一致：这一观察应被判为 Rollback，由调用方阻断该 dbnum。
        assert!(matches!(
            check_file_against_state(
                Some("DESI"),
                Some(r"D:\zz_t605\moved\desi_1"),
                rolled_back.applied_sesno,
                "DESI",
                r"D:\zz_t605\moved\desi_1",
                rolled_back.file_latest_sesno,
            ),
            Some(FileAnomaly::Rollback { .. })
        ));
    }

    /// spec 001 · US2（live）：阻断落库只写观察值，**判据字段纹丝不动**。
    ///
    /// 这是那个 bug 的核心：`record_scan` 按 dbnum UPSERT `db_type` / `file_path`，
    /// 而它们正是 `check_file_against_state` 的判据。阻断时若照常写，第二轮扫描读到的
    /// `stored_db_type` 已经等于观察值，`TypeChanged` 再也检不出来——异常把自己抹掉了。
    /// 所以这条测试的重点不在第一次的返回值，而在**第二轮还能不能检出同一个异常**。
    ///
    /// 用 `999_999_002` 这个不会出现在真实工程里的 dbnum，跑完即清理。
    /// 空库即可验证，不需要解析过的 E3D 工程。
    #[tokio::test]
    #[ignore = "manual live: requires a reachable SurrealDB at the configured endpoint"]
    async fn live_blocked_observation_keeps_the_verdict_evidence_intact() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let dbnum = 999_999_002u32;
        let cleanup = format!("delete {WATERMARK_TABLE}:{dbnum};");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("clear stale fixture")
            .check()
            .expect("valid pre-cleanup");

        // 建立登记身份：DESI，某个路径，水位 50。
        DbnumState::advance_applied(dbnum, 50, None)
            .await
            .expect("establish watermark");
        DbnumState::record_scan(&FileObservation {
            dbnum,
            project: "TestProject".to_string(),
            db_type: "DESI".to_string(),
            file_name: "zz_us2_0001".to_string(),
            file_path: r"D:\zz_us2\ams000\zz_us2_0001".to_string(),
            file_size: 4096,
            file_latest_sesno: 60,
            file_modified_at: None,
        })
        .await
        .expect("record the registered identity");

        // 现场文件换成了另一种类型的库 —— 这一观察应判 TypeChanged 且阻断。
        let observed_path = r"D:\zz_us2\cata000\zz_us2_0001".to_string();
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some(r"D:\zz_us2\ams000\zz_us2_0001"),
            50,
            "CATA",
            &observed_path,
            70,
        );
        assert!(
            anomaly.as_ref().is_some_and(FileAnomaly::blocks),
            "前提：类型变更必须是阻断类异常，实际 {anomaly:?}"
        );

        // 阻断路径的落库。
        DbnumState::record_blocked_observation(&FileObservation {
            dbnum,
            project: "TestProject".to_string(),
            db_type: "CATA".to_string(),
            file_name: "zz_us2_0001".to_string(),
            file_path: observed_path.clone(),
            file_size: 8192,
            file_latest_sesno: 70,
            file_modified_at: None,
        })
        .await
        .expect("record blocked observation");

        let after = DbnumState::read(dbnum)
            .await
            .expect("read state after blocked scan")
            .expect("state exists");

        // 第二轮：拿库里现存的登记身份再判一次，必须仍然是 TypeChanged。
        let second_round = check_file_against_state(
            Some(&after.db_type),
            Some(&after.file_path),
            after.applied_sesno,
            "CATA",
            &observed_path,
            70,
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        // 判据字段：一个都不许动。
        assert_eq!(after.db_type, "DESI", "阻断不得改写登记的库类型");
        assert_eq!(
            after.file_path, r"D:\zz_us2\ams000\zz_us2_0001",
            "阻断不得改写登记的文件路径"
        );
        assert_eq!(after.file_name, "zz_us2_0001", "阻断不得改写登记的文件名");
        // 观察值：照实更新，人要从面板上看得见现场是什么样。
        assert_eq!(after.file_size, 8192);
        assert_eq!(after.file_latest_sesno, 70);
        // 水位：永不因为一次扫描而移动（ADR-001）。
        assert_eq!(after.applied_sesno, 50);
        // 而这才是重点：异常没有把自己抹掉。
        assert!(
            matches!(second_round, Some(FileAnomaly::TypeChanged { .. })),
            "第二轮必须仍能检出同一个异常，实际 {second_round:?}"
        );
    }

    /// 手动入队与 worker 执行体都是 `if let Some(reason) = verdict.block_reason()`
    /// 才拦——所以「阻断」与「有话说」必须严格同步。一个阻断类异常若返回 `None`，
    /// 那两处会一声不吭地把它放过去，恰好是本次要修的那类洞。
    #[test]
    fn every_blocking_anomaly_says_why_and_only_those() {
        let cases = [
            FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            },
            FileAnomaly::PathMigrated {
                old_path: "/old".into(),
                new_path: "/new".into(),
            },
            FileAnomaly::TypeChanged {
                stored_db_type: "DESI".into(),
                observed_db_type: "SYST".into(),
            },
            FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            },
            FileAnomaly::Missing {
                path: "/gone".into(),
            },
            FileAnomaly::NameChanged {
                old_path: "/p/ams8000_0001".into(),
                new_path: "/p/zdj8000_0001".into(),
            },
        ];
        for anomaly in &cases {
            assert_eq!(
                anomaly.block_reason().is_some(),
                anomaly.blocks(),
                "{anomaly:?}：阻断与理由必须同时有或同时无"
            );
        }
    }

    /// 回退那句的措辞被 `docs/specs/web-service-api.md` 的回执样例钉着。
    ///
    /// 2026-08-10 起这个变体多带两个时刻（plant-ui ADR-0019 Q6），这条测试因此
    /// 顺带钉住 T7-a 的分工：**`reason` 是诊断串，一个字都不动**——它进日志、进
    /// 规格样例，sesno 本来就该活在那里；界面改用结构化的那两个时刻自己组句，
    /// 不再当 `reason` 的传声筒。时刻有没有，都不许改变这句话。
    #[test]
    fn the_rollback_reason_matches_the_published_receipt_wording() {
        let with_times = FileAnomaly::Rollback {
            file_latest_sesno: 812,
            applied_sesno: 1005,
            file_latest_sesno_time: Some("2026-07-01T10:00:00+08:00".into()),
            applied_sesno_time: Some("2026-08-05T18:24:00+08:00".into()),
        }
        .block_reason()
        .expect("回退是阻断类异常");
        assert_eq!(
            with_times,
            "文件回退或被替换（file_latest_sesno=812 < applied_sesno=1005），已阻断"
        );

        let without_times = FileAnomaly::Rollback {
            file_latest_sesno: 812,
            applied_sesno: 1005,
            file_latest_sesno_time: None,
            applied_sesno_time: None,
        }
        .block_reason()
        .expect("回退是阻断类异常");
        assert_eq!(with_times, without_times, "诊断串与时刻无关");
    }

    /// T6（plant-ui ADR-0019 Q6）：判据函数是纯的，两个时刻一律留空——它手上既
    /// 没有文件也没有存量水位，猜一个出来比空着糟得多。补齐发生在
    /// [`DbnumState::classify_scan`]：文件端现读一页，已应用端取水位表存量。
    #[test]
    fn the_pure_verdict_leaves_both_rollback_times_to_the_caller() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/p/desi_1"),
            1005,
            "DESI",
            "/p/desi_1",
            812,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::Rollback {
                file_latest_sesno: 812,
                applied_sesno: 1005,
                file_latest_sesno_time: None,
                applied_sesno_time: None,
            })
        );
    }

    /// 从未登记过的库：水位与上一次观察都取 0，且不算异常——首次导入走的是
    /// `needs_initial_load`，不是阻断。
    #[test]
    fn an_unregistered_dbnum_yields_a_clean_verdict() {
        let verdict = ScanVerdict {
            prior: None,
            anomaly: None,
        };
        assert_eq!(verdict.applied_sesno(), 0);
        assert_eq!(verdict.previous_file_latest_sesno(), 0);
        assert!(!verdict.blocked());
        assert!(verdict.block_reason().is_none());
    }

    /// 路径迁移是良性搬家：不阻断，所以调用方照常执行，而落库会走刷新身份那条
    /// 语句把登记路径更新过来。
    #[test]
    fn a_migrated_path_does_not_block_the_batch() {
        let verdict = ScanVerdict {
            prior: Some(DbnumState {
                dbnum: 8000,
                db_type: "DESI".into(),
                file_path: "/old/desi_1".into(),
                file_latest_sesno: 120,
                applied_sesno: 120,
                initialized: true,
                ..Default::default()
            }),
            anomaly: Some(FileAnomaly::PathMigrated {
                old_path: "/old/desi_1".into(),
                new_path: "/new/desi_1".into(),
            }),
        };
        assert_eq!(verdict.applied_sesno(), 120);
        assert_eq!(verdict.previous_file_latest_sesno(), 120);
        assert!(!verdict.blocked());
        assert!(verdict.block_reason().is_none());
    }
}

#[cfg(test)]
mod initializing_watermark_tests {
    use super::*;
    #[tokio::test]
    async fn partial_initialization_never_inherits_legacy_watermarks() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("partial_init").use_db("test").await.unwrap();
        db.query(
            "CREATE dbnum_watermark:7997 SET sesno=105; CREATE dbnum_watermark:8000 SET sesno=12;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(render_legacy_applied_migration(&[7997]))
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut result=db.query("SELECT applied_sesno FROM dbnum_watermark:7997; SELECT applied_sesno FROM dbnum_watermark:8000;").await.unwrap().check().unwrap();
        let held: Vec<serde_json::Value> = result.take(0).unwrap();
        let normal: Vec<serde_json::Value> = result.take(1).unwrap();
        assert!(held[0]["applied_sesno"].is_null());
        assert_eq!(normal[0]["applied_sesno"], 12);
        assert_eq!(
            resolve_initializing_applied(true, Some((None, Some(105))), Some(105)),
            None
        );
        assert_eq!(resolve_initializing_applied(true, None, Some(105)), None);
        assert_eq!(
            resolve_initializing_applied(true, Some((Some(112), Some(105))), Some(105)),
            Some(112)
        );
        assert_eq!(
            resolve_initializing_applied(false, Some((None, Some(105))), None),
            Some(105)
        );
        let (seedable, held) = partition_seedable(
            vec![
                DatabaseWatermark {
                    dbnum: 7997,
                    sesno: 105,
                },
                DatabaseWatermark {
                    dbnum: 8000,
                    sesno: 12,
                },
            ],
            &BTreeSet::from([7997]),
        );
        assert_eq!(seedable.len(), 1);
        assert_eq!(seedable[0].dbnum, 8000);
        assert_eq!(held, vec![7997]);
    }
}

/// ADR-060 D10：模型水位缺省按数据水位追平——只对「已有数据 + 已有模型投影 +
/// 模型水位从未写过」的行；数据在模型无的记 0；显式写过的值一个不动。
/// （房间那半句随房间水位退役，ADR-073 D7：接管保留，行上不再有房间格。）
#[cfg(test)]
mod legacy_model_watermark_seed_tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Row {
        #[serde(default)]
        applied_sesno: Option<i32>,
        #[serde(default)]
        model_sesno: Option<i32>,
        #[serde(default)]
        model_sesno_time: Option<String>,
    }

    async fn row(db: &surrealdb::Surreal<surrealdb::engine::any::Any>, dbnum: u32) -> Row {
        let mut response = db
            .query(format!(
                "SELECT applied_sesno, model_sesno, model_sesno_time \
                 FROM ONLY dbnum_watermark:{dbnum};"
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
        let row: Option<Row> = response.take(0).unwrap();
        row.expect("水位行还在")
    }

    #[tokio::test]
    async fn legacy_libraries_with_a_projection_adopt_the_data_watermark_and_the_rest_do_not() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("legacy_seed").use_db("test").await.unwrap();
        // 8000：旧管线投影（dbnum 直写形态）→ 接管。
        // 8001：旧管线投影（只有 in、没有 dbnum 的历史形态）→ 也接管。
        // 8002：数据在、模型一行没有 → 记 0。
        // 8003：本仓显式写过 0（D7 / D8）→ 不是候选，一个字不动。
        // 8004：数据水位未建立 → 不是候选。
        // 8005：初始化在飞（protected）→ 跳过。
        db.query(
            "CREATE pe:24381_1 SET dbnum = 8000, sesno = 26;\
             CREATE inst_relate:24381_1 SET in = pe:24381_1, dbnum = 8000;\
             CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 26;\
             CREATE pe:24382_1 SET dbnum = 8001, sesno = 30;\
             CREATE tubi_relate:24382_1 SET in = pe:24382_1;\
             CREATE dbnum_watermark:8001 SET dbnum = 8001, applied_sesno = 30;\
             CREATE pe:24383_1 SET dbnum = 8002, sesno = 12;\
             CREATE dbnum_watermark:8002 SET dbnum = 8002, applied_sesno = 12;\
             CREATE pe:24384_1 SET dbnum = 8003, sesno = 9;\
             CREATE inst_relate:24384_1 SET in = pe:24384_1, dbnum = 8003;\
             CREATE dbnum_watermark:8003 SET dbnum = 8003, applied_sesno = 9, model_sesno = 0;\
             CREATE inst_relate:24385_1 SET dbnum = 8004;\
             CREATE dbnum_watermark:8004 SET dbnum = 8004, applied_sesno = 0;\
             CREATE pe:24386_1 SET dbnum = 8005, sesno = 40;\
             CREATE inst_relate:24386_1 SET in = pe:24386_1, dbnum = 8005;\
             CREATE dbnum_watermark:8005 SET dbnum = 8005, applied_sesno = 40;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let mut seeds = seed_legacy_model_watermarks_on(&db, &[8005]).await.unwrap();
        seeds.sort();
        assert_eq!(
            seeds,
            vec![
                LegacyModelWatermarkSeed::Adopted {
                    dbnum: 8000,
                    sesno: 26
                },
                LegacyModelWatermarkSeed::Adopted {
                    dbnum: 8001,
                    sesno: 30
                },
                LegacyModelWatermarkSeed::DataOnly { dbnum: 8002 },
            ]
        );

        let adopted = row(&db, 8000).await;
        assert_eq!(adopted.model_sesno, Some(26));
        assert_eq!(
            adopted.model_sesno_time, None,
            "接管不贴时刻：没有哪一条保存盖出它"
        );
        let adopted_via_in = row(&db, 8001).await;
        assert_eq!(adopted_via_in.model_sesno, Some(30));
        let data_only = row(&db, 8002).await;
        assert_eq!(
            data_only.model_sesno,
            Some(0),
            "数据在模型无：显式记 0，退出候选集"
        );
        let explicit_zero = row(&db, 8003).await;
        assert_eq!(explicit_zero.model_sesno, Some(0));
        let never_applied = row(&db, 8004).await;
        assert_eq!(never_applied.model_sesno, None);
        let protected = row(&db, 8005).await;
        assert_eq!(protected.model_sesno, None);

        // 幂等：同样的保护名单再跑一遍没有候选，一行不动。
        assert!(
            seed_legacy_model_watermarks_on(&db, &[8005])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(row(&db, 8000).await, adopted);

        // 初始化收口、保护解除之后，那个库才轮到接管——跳过不是判定，下一轮照常来。
        assert_eq!(
            seed_legacy_model_watermarks_on(&db, &[]).await.unwrap(),
            vec![LegacyModelWatermarkSeed::Adopted {
                dbnum: 8005,
                sesno: 40
            }]
        );
        assert_eq!(row(&db, 8005).await.model_sesno, Some(40));
    }

    /// 接管语句只填空：`??` 在，没有 `math::max`、没有 `=` 覆盖；时刻列不碰；
    /// 行上没有房间那一枚可填（ADR-073 D2 / D7）。
    #[test]
    fn the_adoption_statement_is_fill_only_and_never_stamps_a_time() {
        let sql = render_legacy_model_watermark_adoption(8000, 26);
        assert!(sql.contains("model_sesno = model_sesno ?? 26"));
        assert!(!sql.contains("math::max"));
        assert!(!sql.contains("model_sesno_time"), "时刻纪律：不贴 {sql}");
        assert!(
            !sql.contains("room_"),
            "库上只剩两枚水位，接管不填房间格 {sql}"
        );

        let zero = render_legacy_model_watermark_data_only(8002);
        assert!(zero.contains("model_sesno = model_sesno ?? 0"));
        assert!(!zero.contains("room_"));
    }

    #[test]
    fn the_seed_lines_stay_silent_when_nothing_happened_and_split_the_two_cases() {
        assert!(legacy_model_watermark_seed_lines(&[]).is_empty());
        let lines = legacy_model_watermark_seed_lines(&[
            LegacyModelWatermarkSeed::Adopted {
                dbnum: 8000,
                sesno: 26,
            },
            LegacyModelWatermarkSeed::DataOnly { dbnum: 8002 },
        ]);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("1 个已有模型投影的库") && lines[0].contains("8000@26"));
        assert!(lines[1].contains("1 个库数据在、模型一行没有") && lines[1].contains("8002"));
    }
}
