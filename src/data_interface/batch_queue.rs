//! 数据批次队列的合并与冻结规则（ADR-011 §5 / §6）。
//!
//! 这里只有纯逻辑：一条数据批次长什么样、一次新触发落在已有队列上会怎样、下一条该取谁。
//! 队列本身住在 `web_service` 的任务注册表里（内存，进程重启即清空），但
//! **「同一 dbnum 排队中合并、运行中冻结」是领域规则，不是 HTTP 层的事**——手动触发与
//! `async_watch` 的自动发现两条路径合流之后共用同一份判定，判定只能有一份。
//!
//! 不碰数据库，不碰 tokio，因此可以用不连库的单测钉死全部性质。

/// 队列项的两种活着的状态。终态不留在队列里，它们进任务注册表的历史。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchState {
    Queued,
    Running,
}

/// 一个 dbnum 在一个会话号区间上的一次数据应用——词表里的「数据批次」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataBatch {
    pub dbnum: u32,
    pub db_type: String,
    /// 闭区间左端，等于入队时的水位 + 1。
    pub start_sesno: i32,
    /// 闭区间右端。**排队期间它是「入队时观察到的预期上界」，不是冻结值**——
    /// ADR-011 §5 把冻结点定义为「执行真正开始之前」的那次重扫，两次触发之间
    /// 文件还在长。真冻结值由执行侧的重扫算出，再经
    /// `BatchScheduler::record_frozen_end` 回写到这里。
    pub end_sesno: i32,
    pub state: BatchState,
}

/// 一次入队的落点。调用方拿它写日志或发事件，不必自己再判一遍。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    /// 队列里没有这个 dbnum，新排一条。
    New,
    /// 并进已排队的那条，只把目标会话号推高（词表里的「并入会话」）。
    Merged,
    /// 已排队的那条已经覆盖了这次触发，什么都不用做。
    AlreadyCovered,
    /// 正在跑的那条已经冻结，另起一条排在它后面。
    BehindRunning,
}

/// 一条队列行的会话区间恒为非空：`start_sesno <= end_sesno`。
///
/// 运行中那条的右端在冻结重扫后才定死，期间水位又还没推进，因此一次落在
/// `(applied, running_end]` 里的重扫能通过上游 `discover_batch` 的水位守卫、
/// 却排出 `start = running_end + 1 > end` 的倒挂行。它不会丢数据（执行侧一律
/// 按水位重算窗口），但面板上会挂一条 `1039..=1038` 这种读不通的幽灵行。
fn covers(start_sesno: i32, end_sesno: i32) -> bool {
    start_sesno <= end_sesno
}

/// 把一次「dbnum 的文件会话号到了 `file_latest_sesno`」的发现放进队列。
///
/// `applied_sesno` 是当前水位，只在真的要新排一条时用来定左端。
///
/// 三条规则，顺序不能换：
/// 1. 同 dbnum 已有排队项 → 合并，**只推高不降低**；
/// 2. 否则同 dbnum 正在跑 → 那条已冻结，新排一条接在它的右端之后；
/// 3. 否则新排一条。
///
/// 因此同一个 dbnum 在队列里**最多占两行**（一行运行中、一行排队中），
/// 界面上要把这件事说清楚，不能让人以为是重复项。
pub fn enqueue(
    queue: &mut Vec<DataBatch>,
    dbnum: u32,
    db_type: &str,
    applied_sesno: i32,
    file_latest_sesno: i32,
) -> Enqueued {
    if let Some(queued) = queue
        .iter_mut()
        .find(|b| b.dbnum == dbnum && b.state == BatchState::Queued)
    {
        if file_latest_sesno > queued.end_sesno {
            queued.end_sesno = file_latest_sesno;
            return Enqueued::Merged;
        }
        return Enqueued::AlreadyCovered;
    }

    let running_end = queue
        .iter()
        .find(|b| b.dbnum == dbnum && b.state == BatchState::Running)
        .map(|b| b.end_sesno);

    let (start_sesno, outcome) = match running_end {
        Some(end) => (end + 1, Enqueued::BehindRunning),
        None => (applied_sesno + 1, Enqueued::New),
    };
    if !covers(start_sesno, file_latest_sesno) {
        return Enqueued::AlreadyCovered;
    }
    queue.push(DataBatch {
        dbnum,
        db_type: db_type.to_owned(),
        start_sesno,
        end_sesno: file_latest_sesno,
        state: BatchState::Queued,
    });
    outcome
}

/// FIFO 出队并冻结：取最早入队的那条排队项，转 `Running`，返回它的下标。
///
/// 冻结点与现状严丝合缝——`merged_sesnos` 兑现的正是「执行真正开始之前」的那次重扫，
/// 跑到一半新存的会话本来就并不进去（ADR-011 §5）。
///
/// `paused` 只挡出队，**不碰正在跑的那条**：服务端没有中止接口，界面上那句
/// 「不再出队，这一批会跑完为止」就是这条实现的兑现（ADR-011 §9）。
pub fn freeze_next(queue: &mut [DataBatch], paused: bool) -> Option<usize> {
    if paused {
        return None;
    }
    let index = queue.iter().position(|b| b.state == BatchState::Queued)?;
    queue[index].state = BatchState::Running;
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(dbnum: u32, start: i32, end: i32) -> DataBatch {
        DataBatch {
            dbnum,
            db_type: "DESI".to_owned(),
            start_sesno: start,
            end_sesno: end,
            state: BatchState::Queued,
        }
    }

    #[test]
    fn a_new_dbnum_starts_one_row_from_the_watermark() {
        let mut queue = Vec::new();
        assert_eq!(enqueue(&mut queue, 7997, "DESI", 1023, 1034), Enqueued::New);
        assert_eq!(queue, vec![queued(7997, 1024, 1034)]);
    }

    #[test]
    fn repeated_saves_merge_into_the_queued_row() {
        let mut queue = vec![queued(7997, 1024, 1034)];
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1023, 1041),
            Enqueued::Merged
        );
        assert_eq!(queue.len(), 1, "合并不该多排一行");
        assert_eq!(queue[0].end_sesno, 1041);
        assert_eq!(queue[0].start_sesno, 1024, "左端是入队时定的，合并不动它");
    }

    #[test]
    fn merging_never_lowers_the_target() {
        let mut queue = vec![queued(7997, 1024, 1041)];
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1023, 1030),
            Enqueued::AlreadyCovered
        );
        assert_eq!(queue[0].end_sesno, 1041, "水位只前进不后退，目标也一样");
    }

    #[test]
    fn a_running_batch_is_frozen_so_the_new_sessions_queue_behind_it() {
        let mut queue = vec![queued(7997, 1024, 1038)];
        freeze_next(&mut queue, false).expect("有一条排队项");
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1023, 1041),
            Enqueued::BehindRunning
        );
        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue[0].end_sesno, 1038,
            "入队合并不再动运行中那条；真正的冻结值由执行侧重扫经 record_frozen_end 回写"
        );
        assert_eq!(
            queue[1],
            queued(7997, 1039, 1041),
            "新的一条从运行中那条的右端之后接上，不重叠"
        );
    }

    #[test]
    fn a_rescan_that_does_not_pass_the_running_end_never_queues_an_inverted_row() {
        // 水位还停在 1023（运行中那条尚未收口），所以 file_latest=1030 能通过
        // discover_batch 的水位守卫走到这里。若不拦，排出的就是 1039..=1030。
        let mut queue = vec![queued(7997, 1024, 1038)];
        freeze_next(&mut queue, false).expect("有一条排队项");
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1023, 1030),
            Enqueued::AlreadyCovered
        );
        assert_eq!(queue.len(), 1, "不该多排一条读不通的幽灵行");
    }

    #[test]
    fn a_watermark_already_covering_the_file_never_queues_an_empty_row() {
        let mut queue = Vec::new();
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1034, 1034),
            Enqueued::AlreadyCovered
        );
        assert!(queue.is_empty(), "start=1035 > end=1034，空区间不入队");
    }

    #[test]
    fn one_dbnum_occupies_at_most_two_rows() {
        let mut queue = vec![queued(7997, 1024, 1038)];
        freeze_next(&mut queue, false).unwrap();
        enqueue(&mut queue, 7997, "DESI", 1023, 1041);
        for target in [1044, 1050, 1051] {
            enqueue(&mut queue, 7997, "DESI", 1023, target);
        }
        assert_eq!(queue.len(), 2, "再密集的保存也只塌成运行中 + 排队中两行");
        assert_eq!(queue[1].end_sesno, 1051);
    }

    #[test]
    fn freezing_takes_the_oldest_queued_row_first() {
        let mut queue = vec![queued(7997, 1, 5), queued(8000, 1, 3), queued(7999, 1, 9)];
        assert_eq!(freeze_next(&mut queue, false), Some(0));
        assert_eq!(
            freeze_next(&mut queue, false),
            Some(1),
            "跳过已经在跑的那条"
        );
        assert_eq!(queue[0].state, BatchState::Running);
        assert_eq!(queue[2].state, BatchState::Queued, "第三条还没轮到");
    }

    #[test]
    fn nothing_to_freeze_when_every_row_is_running() {
        let mut queue = vec![queued(7997, 1, 5)];
        freeze_next(&mut queue, false).unwrap();
        assert_eq!(freeze_next(&mut queue, false), None);
    }

    #[test]
    fn pausing_stops_dequeuing_but_leaves_the_running_row_alone() {
        let mut queue = vec![queued(7997, 1024, 1038), queued(8000, 812, 830)];
        freeze_next(&mut queue, false).unwrap();
        assert_eq!(freeze_next(&mut queue, true), None, "暂停期间不再出队");
        assert_eq!(
            queue[0].state,
            BatchState::Running,
            "正在跑的那条会跑完为止——服务端没有中止接口"
        );
        assert_eq!(queue[1].state, BatchState::Queued);
        assert_eq!(freeze_next(&mut queue, false), Some(1), "恢复后接着出队");
    }

    #[test]
    fn a_paused_queue_still_accepts_new_work() {
        let mut queue = vec![queued(7997, 1024, 1038)];
        assert_eq!(
            enqueue(&mut queue, 7997, "DESI", 1023, 1041),
            Enqueued::Merged,
            "暂停挡的是出队，不是入队；水位差摆在那儿，活迟早要干"
        );
    }

    #[test]
    fn different_dbnums_never_merge() {
        let mut queue = Vec::new();
        enqueue(&mut queue, 7997, "DESI", 0, 10);
        assert_eq!(enqueue(&mut queue, 8000, "DESI", 0, 20), Enqueued::New);
        assert_eq!(queue.len(), 2);
    }
}
