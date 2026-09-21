//! 一个 dbnum 任务这一版攒下的副作用集合——**内存里的那本**（ADR-071 D1，计划 v3 S3/S4）。
//!
//! 今天这三个集合被写进 `incr_side_effect_pending:spatial_reconcile_{dbnum}_{sesno}`
//! （「文件轮次」那一行），再由同一个任务在收尾时读回来用。写进去又读回来的这一趟，
//! 是异步时代留下的：模型行、空间重放、房间收尾曾经分属三条路，得有一本**持久的**待办
//! 把它们串起来。ADR-071 之后三段都在同一个 dbnum 任务里同步跑完，串它们的应该是内存。
//!
//! 本模块就是那本内存账：写入口与库那一行的写入口一一对应（同一处调用同时喂两边），
//! 读出口给任务收尾。集合代数与 `render_file_spatial_reconcile_upsert` 逐条对齐——
//! 一个 refno 从 refresh 挪到 remove（或反过来）时，另一边要同时把它摘掉，否则同一根
//! 会既被刷新又被删除。
//!
//! **崩溃语义**：内存账跨不过重启。这是有意的——任务收尾那一笔没盖章，这一窗就没发生过，
//! 重启后整窗重跑、集合重新算出来（S4 把库里那一行拆掉之后，这条就是唯一语义）。过渡期
//! 库里那一行还在，任务收尾读不到内存账时回落到它（见 `room_task::finish_task_rooms`）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex};

/// 一轮任务的三个副作用集合。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskEffects {
    /// 空间索引要刷新的 refno。
    pub refresh: BTreeSet<String>,
    /// 空间索引要删除的 refno。
    pub remove: BTreeSet<String>,
    /// 房间归属要清掉的 refno（可能跨库：一块被删的板挂着别库成员的边）。
    pub room_clear: BTreeSet<String>,
    pub room_topology_changed: bool,
}

pub(crate) fn mark_room_topology_changed(dbnum: u32, sesno: i32) {
    effects().entry((dbnum, sesno)).or_default().room_topology_changed = true;
}

impl TaskEffects {
    pub fn is_empty(&self) -> bool {
        self.refresh.is_empty() && self.remove.is_empty() && self.room_clear.is_empty()
    }
}

/// 键是（dbnum, 这一轮的目标 sesno），与库里那一行的记录 id 同一把钥匙：
/// 上一轮的残留不能漏进新一轮。
static EFFECTS: LazyLock<Mutex<BTreeMap<(u32, i32), TaskEffects>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn effects() -> std::sync::MutexGuard<'static, BTreeMap<(u32, i32), TaskEffects>> {
    EFFECTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 开一轮：把这个 dbnum 的旧账全部丢掉，只留这一轮的空账。
///
/// 与库那一行的 `UPSERT … status = 'collecting'` 同一处调用。开轮时清掉**整个 dbnum**
/// 的历史键，而不只是同键那一条：上一轮如果半途没收尾，它的集合留着只会污染这一轮。
pub(crate) fn open(dbnum: u32, round_end_sesno: i32) {
    let mut effects = effects();
    effects.retain(|(owner, _), _| *owner != dbnum);
    effects.insert((dbnum, round_end_sesno), TaskEffects::default());
}

/// 往这一轮的账上追加。集合代数与 `render_file_spatial_reconcile_upsert` 一致。
pub(crate) fn record(
    dbnum: u32,
    round_end_sesno: i32,
    refresh: &[String],
    remove: &[String],
    room_clear: &[String],
) {
    let mut effects = effects();
    let entry = effects.entry((dbnum, round_end_sesno)).or_default();
    for refno in remove {
        entry.refresh.remove(refno);
    }
    for refno in refresh {
        entry.remove.remove(refno);
        entry.refresh.insert(refno.clone());
    }
    for refno in remove {
        entry.remove.insert(refno.clone());
    }
    entry.room_clear.extend(room_clear.iter().cloned());
}

/// 这一轮攒到现在的账；没有这一轮的键就是 `None`（与「攒了但是空的」分开——
/// 前者要回落到库里那一行，后者说明这一轮确实没有副作用）。
pub(crate) fn snapshot(dbnum: u32, round_end_sesno: i32) -> Option<TaskEffects> {
    effects().get(&(dbnum, round_end_sesno)).cloned()
}

/// 收尾之后销账。
pub(crate) fn forget(dbnum: u32, round_end_sesno: i32) {
    effects().remove(&(dbnum, round_end_sesno));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refnos(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|value| (*value).to_owned()).collect()
    }

    fn sorted(set: &BTreeSet<String>) -> Vec<&str> {
        set.iter().map(String::as_str).collect()
    }

    #[test]
    fn an_unopened_round_is_none_and_an_opened_empty_round_is_some() {
        open(9101, 7);
        assert_eq!(snapshot(9101, 7), Some(TaskEffects::default()));
        assert_eq!(snapshot(9101, 8), None, "别的目标不共账");
        assert!(snapshot(9101, 7).expect("opened").is_empty());
        forget(9101, 7);
        assert_eq!(snapshot(9101, 7), None);
    }

    /// 一个 refno 改主意时，另一边必须同时摘掉——同一根既刷新又删除是灾难。
    #[test]
    fn a_refno_that_changes_sides_leaves_the_other_set() {
        open(9102, 3);
        record(9102, 3, &refnos(&["1/1", "1/2"]), &[], &[]);
        record(9102, 3, &[], &refnos(&["1/2"]), &refnos(&["1/2"]));
        let effects = snapshot(9102, 3).expect("opened");
        assert_eq!(sorted(&effects.refresh), ["1/1"]);
        assert_eq!(sorted(&effects.remove), ["1/2"]);
        assert_eq!(sorted(&effects.room_clear), ["1/2"]);

        record(9102, 3, &refnos(&["1/2"]), &[], &[]);
        let effects = snapshot(9102, 3).expect("opened");
        assert_eq!(sorted(&effects.refresh), ["1/1", "1/2"]);
        assert!(effects.remove.is_empty());
        assert_eq!(
            sorted(&effects.room_clear),
            ["1/2"],
            "清边名单只增不减：边已经清了"
        );
        forget(9102, 3);
    }

    /// 上一轮没收尾就开新一轮，旧账不得漏进来。
    #[test]
    fn opening_a_new_round_drops_every_earlier_round_of_the_same_dbnum() {
        open(9103, 10);
        record(9103, 10, &refnos(&["2/1"]), &[], &refnos(&["2/1"]));
        open(9103, 11);
        assert_eq!(snapshot(9103, 10), None);
        assert_eq!(snapshot(9103, 11), Some(TaskEffects::default()));

        // 别的库不受影响。
        open(9104, 10);
        record(9104, 10, &refnos(&["3/1"]), &[], &[]);
        open(9103, 12);
        assert_eq!(
            snapshot(9104, 10).map(|effects| sorted(&effects.refresh).len()),
            Some(1)
        );
        forget(9103, 12);
        forget(9104, 10);
    }
}
