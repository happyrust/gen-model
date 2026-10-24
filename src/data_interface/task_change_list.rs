//! 一批（一窗）的元素级变化清单：增量记录页「变化清单」，`GET /api/v1/tasks/{id}/changes`（计划 2026-10-24 W3）。
//!
//! 来源是 e3d-model 的 L3 账本（`UpdatePlan::ledger`）——窗口规划器在 `WindowRootSources::collect` 里按 refno 折成
//! [`ChangeCandidate`]（一个元素一条：主操作、动了哪些属性、去向、两端属主链），`build_model_update_plan_from_window` 选完根之后
//! 折成 [`ElementChange`]（带上它折进了哪个生成根）按窗口 `(dbnum, S, T)` 寄存；批次执行体在窗口落地、知道任务号之后
//! 认领（[`attach_to_task`]），挂到任务上：进程内有界缓存 + `logs/task-changes-YYYY-MM-DD.jsonl` 一任务一行（与
//! [`super::task_history_log`] 同目录同形制，重启后照样翻得到）。任务行上只多一格 `changes_total`；清单不内联进
//! `/tasks`（一窗几十件乘上一页 160 条会把列表撑到几 MB，计划 D5）。
//!
//! 清单封顶 [`MAX_ITEMS`] 条，超出记 `truncated`；`total` 永远是账本上的不同 refno 数。服务端只给事实不做推断：
//! 折不到根的就没有 `root_refno`，没过元数据门的就没有 `no_model_reason`。崩溃回放的窗口（不重新规划）没有清单。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aios_core::RefnoEnum;
use chrono::Local;
use e3d_io::refno::RefNo;
use e3d_model::increment::{NoModelTier, UpdatePlan};
use e3d_model::ledger::{ChangeKind, Disposition};
use serde::{Deserialize, Serialize};

use crate::data_interface::generation_root::refno_from_e3d;
use crate::data_interface::task_registry::TaskRegistry;

/// 一任务的清单封顶。
pub const MAX_ITEMS: usize = 5000;
/// 删除 / 改挂 / 换类型的候选要读 S 端属主链（文件 I/O）才知道旧根；一窗超过这个数就不读，那些条的 `root_refno` 缺席。
const BASE_CHAIN_LOOKUPS: usize = 512;
/// 规划好、还没被执行体认领的窗口清单最多留几个。
const PENDING_WINDOWS: usize = 32;
/// 进程内缓存最多留几个任务的清单；再早的按天翻文件。
const CACHE_TASKS: usize = 64;
const DIRECTORY: &str = "logs";
const FILE_PREFIX: &str = "task-changes-";
const FILE_SUFFIX: &str = ".jsonl";

/// 一个元素这一窗的主操作。账本上一个元素可以有几条（改挂 + 属性变），这里取最「重」的那条，其余留在 `details`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    Added,
    Deleted,
    TypeChanged,
    Reparented,
    MembersChanged,
    Modified,
    Opaque,
    Unresolved,
}

impl ChangeOp {
    fn from_kind(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Created => Self::Added,
            ChangeKind::Deleted => Self::Deleted,
            ChangeKind::TypeChanged => Self::TypeChanged,
            ChangeKind::Reparented => Self::Reparented,
            ChangeKind::MembersChanged => Self::MembersChanged,
            ChangeKind::AttributesModified => Self::Modified,
            ChangeKind::OpaqueRecordChange => Self::Opaque,
            ChangeKind::Unresolved => Self::Unresolved,
        }
    }

    /// 越小越「重」：新建 / 删除说的是存亡，换类型 / 改挂说的是身份，属性只是值。
    fn rank(self) -> u8 {
        match self {
            Self::Added => 0,
            Self::Deleted => 1,
            Self::TypeChanged => 2,
            Self::Reparented => 3,
            Self::MembersChanged => 4,
            Self::Modified => 5,
            Self::Opaque => 6,
            Self::Unresolved => 7,
        }
    }

    /// 旧根只有 S 端说得出的那几种（与 `collect_rigid_motion_sources` 同一条规则）。
    fn needs_base_chain(self) -> bool {
        matches!(self, Self::Deleted | Self::Reparented | Self::TypeChanged)
    }
}

/// 账本给的去向（e3d-model `Disposition`，原词）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDisposition {
    RolledUp,
    NoModel,
    Unresolved,
}

impl From<Disposition> for ChangeDisposition {
    fn from(value: Disposition) -> Self {
        match value {
            Disposition::RolledUp => Self::RolledUp,
            Disposition::NoModel => Self::NoModel,
            Disposition::Unresolved => Self::Unresolved,
        }
    }
}

/// e3d-model 把 `RefNo` 显示成 `word0/word1`（`word1` 按 i32 印，高位汇总号会是负数）；`ElementDiff::refno` 就是这个字串。
pub(crate) fn parse_refno_key(text: &str) -> Option<(u32, u32)> {
    let (word0, word1) = text.trim().split_once(['/', '_'])?;
    let word0 = word0.trim().parse::<u32>().ok()?;
    let word1 = word1.trim().parse::<i64>().ok()?;
    Some((word0, word1 as u32))
}

fn tier_name(tier: NoModelTier) -> &'static str {
    match tier {
        NoModelTier::MetadataOnly => "metadata_only",
        NoModelTier::UnreadAttribute => "unread_attribute",
        NoModelTier::CacheStamp => "cache_stamp",
    }
}

/// 清单里的一条：一个元素这一窗怎么了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementChange {
    /// `a/b`。
    pub refno: String,
    /// 账本上的 noun（换类型时是新类型；读不出的是 `?`）。
    pub noun: String,
    pub op: ChangeOp,
    /// 值变了的属性名（字典序）；只有属性改过的条目才有。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<String>,
    /// 这条改动折进了哪个生成根（`a/b`）：自身或 T 端属主链上第一个进 `regen` 的；删除 / 改挂 / 换类型的再沿 S 端链找
    /// 进 `delete` / `regen` 的旧根。折不到（去向 `no_model`、根没进计划、链读不动）就缺席。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_refno: Option<String>,
    /// 账本的去向；账本没登记（不该发生）就缺席。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<ChangeDisposition>,
    /// 去向 `no_model` 且是元数据门判掉的：`metadata_only` / `unread_attribute` / `cache_stamp`（e3d-model `NoModelTier`）。
    /// 门关着、或是落在没有模型归属的元素上的 `no_model`，缺席。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_model_reason: Option<String>,
    /// 账本逐条的散文（「属性修改: POS / ORI 变了」「改挂: 属主 A → B」），给人读。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// 这一窗只动了摆放（`POS` / `ORI`）——刚体前移的入口条件，不是判决。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placement_only: bool,
}

/// 一任务的清单（落盘行的形状 = 接口不分页时的形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskChanges {
    pub task_id: String,
    pub dbnum: u32,
    pub start_sesno: i32,
    pub end_sesno: i32,
    /// 账本上的不同 refno 数（不含内容没变的「虚候选」）。
    pub total: u32,
    /// `items` 被 [`MAX_ITEMS`] 截过。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    pub items: Vec<ElementChange>,
}

/// 规划期（文件侧）从账本折出来的候选：还没定根。
#[derive(Debug, Clone)]
pub(crate) struct ChangeCandidate {
    pub refno: RefNo,
    pub noun: String,
    pub op: ChangeOp,
    pub attrs: Vec<String>,
    pub details: Vec<String>,
    pub disposition: Option<ChangeDisposition>,
    pub no_model_reason: Option<&'static str>,
    pub placement_only: bool,
    /// T 端属主链（不含自身，最近的在前）。
    pub chain_t: Vec<RefnoEnum>,
    /// S 端属主链；只有删除 / 改挂 / 换类型的条目读，其余恒空。
    pub chain_s: Vec<RefnoEnum>,
}

/// 规划器折出来的一窗候选表 + 账上的不同 refno 总数（候选表按 refno 序截到 [`MAX_ITEMS`]）。
#[derive(Debug, Clone, Default)]
pub(crate) struct WindowChanges {
    pub total: u32,
    pub candidates: Vec<ChangeCandidate>,
}

/// `AttributesModified` 那条的 `detail` 是 e3d-model 按 `"{attrs.join(" / ")} 变了"` 渲的（`ledger.rs::record`），
/// 这里反解出属性名；形状不对就空——宁可少说。
fn attrs_from_detail(detail: &str) -> Vec<String> {
    let Some(names) = detail.strip_suffix(" 变了") else {
        return Vec::new();
    };
    names
        .split(" / ")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 把一窗的账本折成候选表。`owners` 是 T 端索引的属主表；`base_chain` 读 S 端属主链（不含自身）；`tiers` 是元数据门
/// 对每个候选的判词（门关着就是空表）。纯函数，不碰库。
pub(crate) fn collect_candidates(
    plan: &UpdatePlan,
    owners: &BTreeMap<(u32, u32), RefNo>,
    base_chain: impl Fn(RefNo) -> anyhow::Result<Vec<RefnoEnum>>,
    tiers: &BTreeMap<(u32, u32), NoModelTier>,
) -> WindowChanges {
    let dispositions: BTreeMap<(u32, u32), Disposition> = plan
        .ledger
        .dispositions()
        .iter()
        .map(|(key, disposition)| (*key, *disposition))
        .collect();
    let placement_only = plan.ledger.placement_only();
    let mut grouped: BTreeMap<(u32, u32), ChangeCandidate> = BTreeMap::new();
    for entry in plan.ledger.entries() {
        let key = (entry.refno.word0, entry.refno.word1);
        let op = ChangeOp::from_kind(entry.kind);
        let slot = grouped.entry(key).or_insert_with(|| ChangeCandidate {
            refno: entry.refno,
            noun: entry.noun.clone(),
            op,
            attrs: Vec::new(),
            details: Vec::new(),
            disposition: dispositions.get(&key).copied().map(ChangeDisposition::from),
            no_model_reason: tiers.get(&key).copied().map(tier_name),
            placement_only: placement_only.contains(&key),
            chain_t: Vec::new(),
            chain_s: Vec::new(),
        });
        if op.rank() < slot.op.rank() {
            slot.op = op;
            slot.noun = entry.noun.clone();
        }
        if entry.kind == ChangeKind::AttributesModified {
            for attr in attrs_from_detail(&entry.detail) {
                if !slot.attrs.contains(&attr) {
                    slot.attrs.push(attr);
                }
            }
        }
        slot.details
            .push(format!("{}: {}", entry.kind.label(), entry.detail));
    }
    let total = grouped.len() as u32;
    let mut base_lookups = 0usize;
    let candidates = grouped
        .into_values()
        .take(MAX_ITEMS)
        .map(|mut candidate| {
            candidate.attrs.sort();
            candidate.chain_t =
                crate::data_interface::window_root_plan::owner_chain_t(owners, candidate.refno);
            if candidate.op.needs_base_chain() && base_lookups < BASE_CHAIN_LOOKUPS {
                base_lookups += 1;
                candidate.chain_s = base_chain(candidate.refno).unwrap_or_default();
            }
            candidate
        })
        .collect();
    WindowChanges { total, candidates }
}

/// 选完根之后把候选折成清单条目：`regen` / `delete` 是这一窗的两个根桶。
pub(crate) fn resolve(
    candidates: &[ChangeCandidate],
    regen: &BTreeSet<RefnoEnum>,
    delete: &BTreeSet<RefnoEnum>,
) -> Vec<ElementChange> {
    candidates
        .iter()
        .map(|candidate| {
            let own = refno_from_e3d(candidate.refno);
            let in_regen = std::iter::once(own)
                .chain(candidate.chain_t.iter().copied())
                .find(|refno| regen.contains(refno));
            let root = in_regen.or_else(|| {
                candidate.op.needs_base_chain().then(|| {
                    std::iter::once(own)
                        .chain(candidate.chain_s.iter().copied())
                        .find(|refno| delete.contains(refno) || regen.contains(refno))
                })?
            });
            ElementChange {
                refno: own.to_pdms_str(),
                noun: candidate.noun.clone(),
                op: candidate.op,
                attrs: candidate.attrs.clone(),
                root_refno: root.map(|refno| refno.to_pdms_str()),
                disposition: candidate.disposition,
                no_model_reason: candidate.no_model_reason.map(str::to_owned),
                details: candidate.details.clone(),
                placement_only: candidate.placement_only,
            }
        })
        .collect()
}

/// 规划好、等执行体认领的窗口清单：`(dbnum, S, T)` → 清单。有界，最早的先挤掉。
static PENDING: Mutex<VecDeque<((u32, u32, u32), (u32, Vec<ElementChange>))>> =
    Mutex::new(VecDeque::new());

fn pending()
-> std::sync::MutexGuard<'static, VecDeque<((u32, u32, u32), (u32, Vec<ElementChange>))>> {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 规划器寄存一窗的清单。同一窗重新规划（扩窗重来）就覆盖。
pub(crate) fn deposit(
    dbnum: u32,
    base_sesno: u32,
    target_sesno: u32,
    total: u32,
    items: Vec<ElementChange>,
) {
    let key = (dbnum, base_sesno, target_sesno);
    let mut pending = pending();
    pending.retain(|(existing, _)| *existing != key);
    pending.push_back((key, (total, items)));
    while pending.len() > PENDING_WINDOWS {
        pending.pop_front();
    }
}

/// 执行体认领：先按精确窗口，其次同库同右端（左端被水位挖过的窗）。认领即移除。
pub(crate) fn claim(
    dbnum: u32,
    base_sesno: u32,
    target_sesno: u32,
) -> Option<(u32, Vec<ElementChange>)> {
    let mut pending = pending();
    let position = pending
        .iter()
        .position(|(key, _)| *key == (dbnum, base_sesno, target_sesno))
        .or_else(|| {
            pending
                .iter()
                .position(|(key, _)| key.0 == dbnum && key.2 == target_sesno)
        })?;
    pending.remove(position).map(|(_, changes)| changes)
}

/// 批次执行体在窗口落地后调：认领这一窗的清单、挂到任务上、落盘。没有清单（非 DESI / 回放 / 老计划）就什么都不做。
pub(crate) fn attach_to_task(
    registry: &TaskRegistry,
    task_id: &str,
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
) {
    let base_sesno = u32::try_from(start_sesno.saturating_sub(1)).unwrap_or(0);
    let Ok(target_sesno) = u32::try_from(end_sesno) else {
        return;
    };
    let Some((total, items)) = claim(dbnum, base_sesno, target_sesno) else {
        return;
    };
    let changes = TaskChanges {
        task_id: task_id.to_owned(),
        dbnum,
        start_sesno,
        end_sesno,
        total,
        truncated: (total as usize) > items.len(),
        items,
    };
    registry.set_changes_total(task_id, total);
    ChangeStore::global().record(changes);
}

/// 清单的进程内缓存 + 落盘。
pub struct ChangeStore {
    directory: PathBuf,
    cache: Mutex<(VecDeque<String>, HashMap<String, Arc<TaskChanges>>)>,
}

static STORE: OnceLock<ChangeStore> = OnceLock::new();

impl ChangeStore {
    pub fn global() -> &'static ChangeStore {
        STORE.get_or_init(|| ChangeStore::open(Path::new(DIRECTORY)))
    }

    pub fn open(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
            cache: Mutex::new((VecDeque::new(), HashMap::new())),
        }
    }

    fn cache(
        &self,
    ) -> std::sync::MutexGuard<'_, (VecDeque<String>, HashMap<String, Arc<TaskChanges>>)> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 记一任务的清单：追加到当天文件，再进缓存。单测里不写文件。
    pub fn record(&self, changes: TaskChanges) {
        if !cfg!(test) {
            let path = daily_path(&self.directory, Local::now());
            if let Err(error) = append_json_line(&path, &changes) {
                eprintln!(
                    "[变化清单] {} 落盘失败 {}: {error}——这份只活在进程内",
                    changes.task_id,
                    path.display()
                );
            }
        }
        self.insert(changes);
    }

    fn insert(&self, changes: TaskChanges) {
        let mut cache = self.cache();
        let id = changes.task_id.clone();
        cache.0.retain(|existing| *existing != id);
        cache.0.push_back(id.clone());
        cache.1.insert(id, Arc::new(changes));
        while cache.0.len() > CACHE_TASKS {
            if let Some(evicted) = cache.0.pop_front() {
                cache.1.remove(&evicted);
            }
        }
    }

    /// 取一任务的清单：先缓存，再按任务入队那天的文件找（`created_at` 前 10 位是日期）。这一天没有就翻前一天——
    /// 落盘按认领那一刻的日期切文件，跨午夜排队的任务在后一天。
    pub fn get(&self, task_id: &str, created_at: &str) -> Option<Arc<TaskChanges>> {
        if let Some(found) = self.cache().1.get(task_id).cloned() {
            return Some(found);
        }
        let day = created_at.get(..10)?;
        let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
        for offset in 0..2 {
            let path = self.directory.join(format!(
                "{FILE_PREFIX}{}{FILE_SUFFIX}",
                (date + chrono::Duration::days(offset)).format("%Y-%m-%d")
            ));
            if let Some(found) = find_in_file(&path, task_id) {
                let found = Arc::new(found);
                self.insert((*found).clone());
                return Some(found);
            }
        }
        None
    }
}

fn daily_path(directory: &Path, now: chrono::DateTime<Local>) -> PathBuf {
    directory.join(format!(
        "{FILE_PREFIX}{}{FILE_SUFFIX}",
        now.format("%Y-%m-%d")
    ))
}

fn append_json_line(path: &Path, record: &TaskChanges) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(record)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    file.flush()
}

/// 在一天的文件里找一任务：先按子串筛行再解，同 id 后写为准；残行跳过。
fn find_in_file(path: &Path, task_id: &str) -> Option<TaskChanges> {
    let file = fs::File::open(path).ok()?;
    let needle = format!("\"task_id\":\"{task_id}\"");
    let mut found = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains(&needle) {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<TaskChanges>(&line) {
            if parsed.task_id == task_id {
                found = Some(parsed);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_model::element_diff::ElementDiff;
    use e3d_model::ledger::ChangeLedger;

    fn diff(noun: &str, attrs: &[&str]) -> ElementDiff {
        ElementDiff {
            refno: String::new(),
            noun: noun.into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: attrs.iter().map(|attr| attr.to_string()).collect(),
            opaque: false,
        }
    }

    fn plan_with(ledger: ChangeLedger) -> UpdatePlan {
        UpdatePlan {
            regenerate: BTreeSet::new(),
            remove: BTreeSet::new(),
            regenerate_derived: BTreeSet::new(),
            derived_stale: BTreeSet::new(),
            ledger,
            full_rebuild: None,
            report: Default::default(),
        }
    }

    fn r(word1: u32) -> RefnoEnum {
        refno_from_e3d(RefNo::new(24384, word1))
    }

    /// 账本里一个元素几条账折成一条：主操作取最重的、属性名从散文里反解、去向 / 门档 / 只动摆放都带上；
    /// 根按属主链折：改了的折到 T 端链上进 regen 的根，删了的折到 S 端链上进 delete 的根，no_model 的没有根。
    #[test]
    fn a_window_ledger_folds_into_one_change_per_element_with_its_root() {
        let mut ledger = ChangeLedger::new();
        // FTUBE 动了 POS（只动摆放），属 BRAN 23257。
        ledger.record(RefNo::new(24384, 23262), &diff("FTUB", &["POS"]));
        ledger.dispose(RefNo::new(24384, 23262), Disposition::RolledUp);
        // 一个元素两条账：改挂 + 属性变——主操作是改挂。
        ledger.record(
            RefNo::new(24384, 30),
            &ElementDiff {
                owner_changed: Some((Some("24384/10".into()), Some("24384/23257".into()))),
                ..diff("VALV", &["NAME", "DESC"])
            },
        );
        ledger.dispose(RefNo::new(24384, 30), Disposition::RolledUp);
        // 删了的：只有 S 端链说得出旧根 40。
        ledger.deleted(
            RefNo::new(24384, 41),
            "BOX".into(),
            Some(RefNo::new(24384, 40)),
        );
        ledger.dispose(RefNo::new(24384, 41), Disposition::RolledUp);
        // 只改了名字的：元数据门放行，没有根。
        ledger.record(RefNo::new(24384, 50), &diff("ZONE", &["NAME"]));
        ledger.dispose(RefNo::new(24384, 50), Disposition::NoModel);
        ledger.finish();
        let plan = plan_with(ledger);

        let owners: BTreeMap<(u32, u32), RefNo> = [
            ((24384, 23262), RefNo::new(24384, 23257)),
            ((24384, 23257), RefNo::new(24384, 22400)),
            ((24384, 30), RefNo::new(24384, 23257)),
            ((24384, 50), RefNo::new(24384, 22400)),
        ]
        .into_iter()
        .collect();
        let tiers: BTreeMap<(u32, u32), NoModelTier> = [((24384, 50), NoModelTier::MetadataOnly)]
            .into_iter()
            .collect();
        let window = collect_candidates(
            &plan,
            &owners,
            |refno| {
                Ok(if refno == RefNo::new(24384, 41) {
                    vec![r(40), r(22400)]
                } else {
                    Vec::new()
                })
            },
            &tiers,
        );
        assert_eq!(window.total, 4);
        let regen: BTreeSet<RefnoEnum> = [r(23257)].into_iter().collect();
        let delete: BTreeSet<RefnoEnum> = [r(40)].into_iter().collect();
        let changes = resolve(&window.candidates, &regen, &delete);
        let by_refno: BTreeMap<&str, &ElementChange> = changes
            .iter()
            .map(|change| (change.refno.as_str(), change))
            .collect();

        let ftube = by_refno["24384/23262"];
        assert_eq!(ftube.op, ChangeOp::Modified);
        assert_eq!(ftube.attrs, vec!["POS"]);
        assert_eq!(ftube.root_refno.as_deref(), Some("24384/23257"));
        assert_eq!(ftube.disposition, Some(ChangeDisposition::RolledUp));
        assert!(ftube.placement_only);
        assert_eq!(ftube.no_model_reason, None);

        let valve = by_refno["24384/30"];
        assert_eq!(valve.op, ChangeOp::Reparented, "改挂比属性变重");
        assert_eq!(valve.attrs, vec!["DESC", "NAME"]);
        assert_eq!(valve.details.len(), 2);
        assert_eq!(valve.root_refno.as_deref(), Some("24384/23257"));

        let gone = by_refno["24384/41"];
        assert_eq!(gone.op, ChangeOp::Deleted);
        assert_eq!(
            gone.root_refno.as_deref(),
            Some("24384/40"),
            "旧根沿 S 端链找"
        );

        let renamed = by_refno["24384/50"];
        assert_eq!(renamed.disposition, Some(ChangeDisposition::NoModel));
        assert_eq!(renamed.no_model_reason.as_deref(), Some("metadata_only"));
        assert_eq!(renamed.root_refno, None, "没折进任何根就不猜");
    }

    #[test]
    fn refno_keys_parse_both_separators_and_negative_word1() {
        assert_eq!(parse_refno_key("24384/23262"), Some((24384, 23262)));
        assert_eq!(parse_refno_key("24384_23262"), Some((24384, 23262)));
        assert_eq!(parse_refno_key("24384/-1"), Some((24384, u32::MAX)));
        assert_eq!(parse_refno_key("junk"), None);
    }

    /// 寄存 / 认领：精确窗口优先，其次同库同右端；认领即移除；超过上限挤掉最早的。
    #[test]
    fn deposited_windows_are_claimed_once_by_window_or_right_edge() {
        let item = |refno: &str| ElementChange {
            refno: refno.into(),
            noun: "FTUB".into(),
            op: ChangeOp::Modified,
            attrs: vec![],
            root_refno: None,
            disposition: None,
            no_model_reason: None,
            details: vec![],
            placement_only: false,
        };
        deposit(9001, 10, 11, 1, vec![item("1/1")]);
        deposit(9001, 11, 12, 1, vec![item("1/2")]);
        assert_eq!(
            claim(9001, 10, 11).map(|(_, items)| items[0].refno.clone()),
            Some("1/1".into())
        );
        assert!(claim(9001, 10, 11).is_none(), "认领即移除");
        // 左端被挖过的窗：按右端认。
        assert_eq!(
            claim(9001, 9, 12).map(|(_, items)| items[0].refno.clone()),
            Some("1/2".into())
        );
        for sesno in 0..(PENDING_WINDOWS as u32 + 5) {
            deposit(9002, sesno, sesno + 1, 0, Vec::new());
        }
        assert!(claim(9002, 0, 1).is_none(), "最早的被挤掉");
        assert!(claim(9002, 10, 11).is_some());
    }

    /// 落盘一任务一行，重开按天找得回来；残行跳过；缓存有界。
    #[test]
    fn a_recorded_change_list_is_found_again_by_task_and_day() {
        let dir = std::env::temp_dir().join(format!("task-changes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("task-changes-2026-10-24.jsonl");
        let changes = TaskChanges {
            task_id: "db-20261024-000001".into(),
            dbnum: 8000,
            start_sesno: 676,
            end_sesno: 677,
            total: 1,
            truncated: false,
            items: vec![ElementChange {
                refno: "24384/23262".into(),
                noun: "FTUB".into(),
                op: ChangeOp::Modified,
                attrs: vec!["POS".into()],
                root_refno: Some("24384/23257".into()),
                disposition: Some(ChangeDisposition::RolledUp),
                no_model_reason: None,
                details: vec!["属性修改: POS 变了".into()],
                placement_only: true,
            }],
        };
        append_json_line(&path, &changes).unwrap();
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "{{\"task_id\":\"db-20261024-000001\",\"torn").unwrap();
        }
        let store = ChangeStore::open(&dir);
        let found = store
            .get("db-20261024-000001", "2026-10-24T03:54:45+08:00")
            .expect("found in that day's file");
        assert_eq!(*found, changes);
        assert!(
            store
                .get("db-20261024-000009", "2026-10-24T03:54:45+08:00")
                .is_none()
        );
        assert!(
            ChangeStore::open(&dir)
                .get("db-20261024-000001", "2026-10-01T03:54:45+08:00")
                .is_none(),
            "别的天没有（冷缓存）"
        );
        assert!(
            store
                .get("db-20261024-000001", "2026-10-01T03:54:45+08:00")
                .is_some(),
            "缓存里有就不再翻文件"
        );
        for n in 0..(CACHE_TASKS + 3) {
            store.insert(TaskChanges {
                task_id: format!("t{n}"),
                ..changes.clone()
            });
        }
        assert_eq!(store.cache().0.len(), CACHE_TASKS);
        let _ = fs::remove_dir_all(&dir);
    }
}
