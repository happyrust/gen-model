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
    /// 挂起：入了队、占着位、但**不派发**，等这个 dbnum 真的来一次增量再放行。
    ///
    /// 只有重扫（启动重建队列、范围刷新、共享盘补挂）排出来的行会挂起，而且只在
    /// `startup_autorun` 关着时。它兑现的是「启动不自动跑积压，增量触发了再跑」：
    /// 重扫看到的是**停机期间攒下的**会话，没有任何人在此刻要求处理它们；而一次
    /// 真实的文件事件说明有人正在这个库上干活，那才是执行的信号。
    ///
    /// 放行不需要单独一步：同 dbnum 的下一次真实触发会并进这一行（见
    /// [`enqueue`]），顺带把标记清掉，于是积压与新会话**合成一条**一次跑完。
    pub held: bool,
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
///
/// `hold` = 这次发现只是「扫出来的」而不是「有人动了这个库」，排出来的新行挂起
/// （见 [`DataBatch::held`]）。反过来，`hold == false` 的触发落在一条挂起行上时
/// **一定要把它放行**——那正是「增量触发了再执行」，而合并已经让新会话与积压
/// 变成同一条区间。放行写在 `Merged` / `AlreadyCovered` 两条分支之前：迟到的
/// 事件可能一个新会话都没带来（`AlreadyCovered`），但它同样证明有人在动这个库。
pub fn enqueue(
    queue: &mut Vec<DataBatch>,
    dbnum: u32,
    db_type: &str,
    applied_sesno: i32,
    file_latest_sesno: i32,
    hold: bool,
) -> Enqueued {
    if let Some(queued) = queue
        .iter_mut()
        .find(|b| b.dbnum == dbnum && b.state == BatchState::Queued)
    {
        if !hold {
            queued.held = false;
        }
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
        held: hold,
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
    match freeze_next_concurrent(queue, paused, true, |_| false) {
        NextDispatch::Freeze(index) => Some(index),
        NextDispatch::HeadNeedsExclusive | NextDispatch::Idle => None,
    }
}

/// 并发派发时一次出队判定的结果（ADR-011 2026-08-09 修订）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextDispatch {
    /// 冻结了下标处的排队行，可以派发。
    Freeze(usize),
    /// FIFO 首个可跑行要求独占，但还有在飞批次：不越过它派发后面的行，
    /// 等在飞批次收敛后它单独跑——独占批次的 FIFO 位置不因并发而被插队。
    HeadNeedsExclusive,
    /// 没有可派发的排队行（空队列 / 暂停 / 可跑行的 dbnum 都还在跑）。
    Idle,
}

/// 并发口径的 FIFO 出队并冻结（ADR-011 2026-08-09 修订）。
///
/// 四条规则：
/// 1. **同 dbnum 恒串行**：dbnum 已有运行中行的排队行（BehindRunning 排出来的）
///    直接跳过——它不与自己并发，轮不到它时后面的行可以先走；
/// 2. **挂起行不派发**：跳过，而且**不算队首**——挂起的语义是「这个库没人动，先
///    别管它」，让它挡住后面真有人在动的库就本末倒置了（对比规则 3 的独占，那才
///    是「必须保住 FIFO 位置」）；
/// 3. **独占批次不被插队**：FIFO 首个可跑行若 `is_exclusive`，只有在飞为空时才冻结它；
///    否则返回 [`NextDispatch::HeadNeedsExclusive`]，不派发它后面的任何行；
/// 4. `paused` 只挡出队，语义与 [`freeze_next`] 相同。
///
/// 单 worker（在飞恒空、无独占判定）下与 [`freeze_next`] 完全等价。
pub fn freeze_next_concurrent(
    queue: &mut [DataBatch],
    paused: bool,
    in_flight_empty: bool,
    is_exclusive: impl Fn(&DataBatch) -> bool,
) -> NextDispatch {
    if paused {
        return NextDispatch::Idle;
    }
    let running_dbnums: Vec<u32> = queue
        .iter()
        .filter(|b| b.state == BatchState::Running)
        .map(|b| b.dbnum)
        .collect();
    for index in 0..queue.len() {
        if queue[index].state != BatchState::Queued {
            continue;
        }
        if queue[index].held {
            continue;
        }
        if running_dbnums.contains(&queue[index].dbnum) {
            continue;
        }
        if is_exclusive(&queue[index]) {
            if !in_flight_empty {
                return NextDispatch::HeadNeedsExclusive;
            }
        }
        queue[index].state = BatchState::Running;
        return NextDispatch::Freeze(index);
    }
    NextDispatch::Idle
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
            held: false,
        }
    }

    fn held(dbnum: u32, start: i32, end: i32) -> DataBatch {
        DataBatch {
            held: true,
            ..queued(dbnum, start, end)
        }
    }

    /// 「有人真的动了这个库」那种触发：watch 事件与人工执行都走这条口径，
    /// 下面绝大多数性质与挂起无关，用它免得每处都拖一个 `false`。
    /// 重扫那条（挂起）由本模块末尾几个专门的测试覆盖。
    fn live_trigger(
        queue: &mut Vec<DataBatch>,
        dbnum: u32,
        db_type: &str,
        applied_sesno: i32,
        file_latest_sesno: i32,
    ) -> Enqueued {
        enqueue(
            queue,
            dbnum,
            db_type,
            applied_sesno,
            file_latest_sesno,
            false,
        )
    }

    /// 重扫发现：入队但挂起。
    fn sweep(
        queue: &mut Vec<DataBatch>,
        dbnum: u32,
        db_type: &str,
        applied_sesno: i32,
        file_latest_sesno: i32,
    ) -> Enqueued {
        enqueue(
            queue,
            dbnum,
            db_type,
            applied_sesno,
            file_latest_sesno,
            true,
        )
    }

    #[test]
    fn a_new_dbnum_starts_one_row_from_the_watermark() {
        let mut queue = Vec::new();
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 1023, 1034),
            Enqueued::New
        );
        assert_eq!(queue, vec![queued(7997, 1024, 1034)]);
    }

    #[test]
    fn repeated_saves_merge_into_the_queued_row() {
        let mut queue = vec![queued(7997, 1024, 1034)];
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 1023, 1041),
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
            live_trigger(&mut queue, 7997, "DESI", 1023, 1030),
            Enqueued::AlreadyCovered
        );
        assert_eq!(queue[0].end_sesno, 1041, "水位只前进不后退，目标也一样");
    }

    #[test]
    fn a_running_batch_is_frozen_so_the_new_sessions_queue_behind_it() {
        let mut queue = vec![queued(7997, 1024, 1038)];
        freeze_next(&mut queue, false).expect("有一条排队项");
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 1023, 1041),
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
            live_trigger(&mut queue, 7997, "DESI", 1023, 1030),
            Enqueued::AlreadyCovered
        );
        assert_eq!(queue.len(), 1, "不该多排一条读不通的幽灵行");
    }

    #[test]
    fn a_watermark_already_covering_the_file_never_queues_an_empty_row() {
        let mut queue = Vec::new();
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 1034, 1034),
            Enqueued::AlreadyCovered
        );
        assert!(queue.is_empty(), "start=1035 > end=1034，空区间不入队");
    }

    #[test]
    fn one_dbnum_occupies_at_most_two_rows() {
        let mut queue = vec![queued(7997, 1024, 1038)];
        freeze_next(&mut queue, false).unwrap();
        live_trigger(&mut queue, 7997, "DESI", 1023, 1041);
        for target in [1044, 1050, 1051] {
            live_trigger(&mut queue, 7997, "DESI", 1023, target);
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
            live_trigger(&mut queue, 7997, "DESI", 1023, 1041),
            Enqueued::Merged,
            "暂停挡的是出队，不是入队；水位差摆在那儿，活迟早要干"
        );
    }

    #[test]
    fn different_dbnums_never_merge() {
        let mut queue = Vec::new();
        live_trigger(&mut queue, 7997, "DESI", 0, 10);
        assert_eq!(live_trigger(&mut queue, 8000, "DESI", 0, 20), Enqueued::New);
        assert_eq!(queue.len(), 2);
    }

    /// 并发派发规则 1：同 dbnum 恒串行。7997 在跑时它的 BehindRunning 行必须被
    /// 跳过，让 8000 先走；7997 收口后那行才轮得到。
    #[test]
    fn concurrent_dispatch_never_runs_one_dbnum_twice() {
        let mut queue = vec![queued(7997, 1024, 1038), queued(8000, 812, 830)];
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Freeze(0)
        );
        // 7997 在跑期间新保存排出 BehindRunning 行。
        live_trigger(&mut queue, 7997, "DESI", 1023, 1041);
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, false, |_| false),
            NextDispatch::Freeze(1),
            "7997 的后继行必须被跳过，8000 先走"
        );
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, false, |_| false),
            NextDispatch::Idle,
            "唯一剩下的排队行属于在跑的 dbnum，本轮无事可派"
        );
    }

    /// 并发派发规则 2：独占批次保住 FIFO 位置。排在队首的独占行在飞非空时不冻结，
    /// 也不放行它后面的行；在飞排空后单独跑。
    #[test]
    fn an_exclusive_head_waits_for_the_pool_and_is_never_overtaken() {
        let is_syst = |b: &DataBatch| b.db_type == "SYST";
        let mut queue = vec![
            DataBatch {
                dbnum: 8191,
                db_type: "SYST".to_owned(),
                start_sesno: 5,
                end_sesno: 9,
                state: BatchState::Queued,
                held: false,
            },
            queued(7997, 1024, 1038),
        ];
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, false, is_syst),
            NextDispatch::HeadNeedsExclusive,
            "独占行在飞非空时既不冻结自己也不放行后面的 DESI 行"
        );
        assert_eq!(queue[0].state, BatchState::Queued, "独占行原地等待");
        assert_eq!(queue[1].state, BatchState::Queued, "后面的行不得越过它");
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, is_syst),
            NextDispatch::Freeze(0),
            "在飞排空后独占行按原 FIFO 位置出队"
        );
    }

    /// 单 worker 口径不变：`freeze_next` 与并发判定在「在飞恒空、无独占」下等价。
    #[test]
    fn serial_freeze_next_is_the_degenerate_case_of_concurrent_dispatch() {
        let mut serial = vec![queued(7997, 1, 5), queued(8000, 1, 3)];
        let mut concurrent = serial.clone();
        assert_eq!(freeze_next(&mut serial, false), Some(0));
        assert_eq!(
            freeze_next_concurrent(&mut concurrent, false, true, |_| false),
            NextDispatch::Freeze(0)
        );
        assert_eq!(serial, concurrent);
    }

    /// 重扫排出来的行入队、占位、可见，但不派发。
    ///
    /// 「入队」这半边不能省：队列是重启后从水位重建出来的那份账，人要能看见
    /// 停机期间攒了多少活；不派发的只是执行。
    #[test]
    fn a_swept_row_is_queued_but_never_dispatched() {
        let mut queue = Vec::new();
        assert_eq!(sweep(&mut queue, 7997, "DESI", 102, 132), Enqueued::New);
        assert_eq!(queue, vec![held(7997, 103, 132)]);
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Idle,
            "挂起行不出队"
        );
        assert_eq!(queue[0].state, BatchState::Queued, "也不该被改成运行中");
    }

    /// 这一条就是「增量触发了再去执行」：真实触发把积压放行，并与新会话合成一条。
    ///
    /// 端点必须是 `103..=133` 而不是 `133..=133`——积压不能被跳过，否则水位与
    /// 文件之间那 30 个会话就永远没人应用。
    #[test]
    fn a_real_trigger_releases_the_backlog_and_merges_it_into_one_run() {
        let mut queue = Vec::new();
        sweep(&mut queue, 7997, "DESI", 102, 132);
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 102, 133),
            Enqueued::Merged
        );
        assert_eq!(queue, vec![queued(7997, 103, 133)]);
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Freeze(0)
        );
    }

    /// 一个新会话都没带来的真实触发（迟到的事件、只动 mtime 的保存）同样放行。
    ///
    /// 判据是「有没有人在动这个库」，不是「这次带没带新会话」。写成只有 `Merged`
    /// 才放行的话，一次 `AlreadyCovered` 事件会让积压继续挂着，而现场看到的是
    /// 「我明明保存了，它还是不动」。
    #[test]
    fn a_trigger_without_new_sessions_still_releases_the_hold() {
        let mut queue = Vec::new();
        sweep(&mut queue, 7997, "DESI", 102, 132);
        assert_eq!(
            live_trigger(&mut queue, 7997, "DESI", 102, 132),
            Enqueued::AlreadyCovered
        );
        assert!(!queue[0].held, "迟到的事件也证明有人在动这个库");
    }

    /// 放行是**按 dbnum**的：一个库被动了，不代表其余的库该跟着开跑。
    #[test]
    fn releasing_one_dbnum_leaves_the_others_held() {
        let mut queue = Vec::new();
        sweep(&mut queue, 7997, "DESI", 102, 132);
        sweep(&mut queue, 8000, "DESI", 34, 35);
        live_trigger(&mut queue, 7997, "DESI", 102, 133);
        assert_eq!(queue[1], held(8000, 35, 35), "8000 没人动，继续挂着");
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Freeze(0),
            "被放行的 7997 出队"
        );
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Idle,
            "只剩挂起的 8000，无事可派"
        );
    }

    /// 挂起行不占队首：它不许挡住后面真有人在动的库。
    ///
    /// 与独占行（规则 3）刻意相反——独占要保住 FIFO 位置，挂起则是「这个库压根
    /// 不参与本轮排队」。写成 `HeadNeedsExclusive` 那种「停在这里」的语义，一个
    /// 启动挂起的 7997 就能把之后所有真实增量全堵死。
    #[test]
    fn a_held_head_never_blocks_the_live_rows_behind_it() {
        let mut queue = Vec::new();
        sweep(&mut queue, 7997, "DESI", 102, 132);
        live_trigger(&mut queue, 8000, "DESI", 34, 40);
        assert_eq!(
            freeze_next_concurrent(&mut queue, false, true, |_| false),
            NextDispatch::Freeze(1),
            "队首挂着的 7997 要被跳过，8000 直接走"
        );
    }

    /// 反方向不成立：重扫不会把一条已经放行的行重新挂起。
    ///
    /// 否则一次范围刷新重扫（每次 SYS meta 落库都会来一发）就能把人工刚点下去的
    /// 执行按回去，而回执已经告诉人「已入队」。
    #[test]
    fn a_later_sweep_cannot_re_hold_a_released_row() {
        let mut queue = Vec::new();
        live_trigger(&mut queue, 7997, "DESI", 102, 132);
        sweep(&mut queue, 7997, "DESI", 102, 140);
        assert!(!queue[0].held, "放行是单向的");
        assert_eq!(queue[0].end_sesno, 140, "但目标照样被推高");
    }
}
