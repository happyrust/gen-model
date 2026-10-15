//! Real admission decision over local state; no process singleton or database.

use super::*;

#[test]
fn room_rebuild_regression_second_request_rejected_during_edge_recompute() {
    let lifecycle = RwLock::new(ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    });
    let progress = RwLock::new(ResidentProgress::default());
    assert!(super::try_begin_rebuild_in(&lifecycle, &progress));
    assert_eq!(
        progress.read().expect("local fixture lock").phase,
        "source-snapshot",
        "an admitted request starts its own progress frame"
    );
    assert!(
        !super::try_begin_rebuild_in(&lifecycle, &progress),
        "control: a duplicate request is rejected while Loading"
    );

    // run_rooms_rebuild publishes Ready before its per-dbnum edge loop.
    *lifecycle.write().expect("local fixture lock") = ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    };
    *progress.write().expect("local fixture lock") = ResidentProgress {
        phase: "edge-recompute".into(),
        dbnums_done: 1,
        dbnums_total: 2,
        ..ResidentProgress::default()
    };
    let second_admitted = super::try_begin_rebuild_in(&lifecycle, &progress);
    assert!(
        !second_admitted,
        "second rebuild must remain rejected until the first per-dbnum edge loop ends"
    );
    let in_flight = progress.read().expect("local fixture lock");
    assert_eq!(
        in_flight.phase, "edge-recompute",
        "a rejected request must not reset in-flight progress"
    );
    assert_eq!(in_flight.dbnums_done, 1);
    assert_eq!(in_flight.dbnums_total, 2);
    drop(in_flight);
    assert!(
        matches!(
            *lifecycle.read().expect("local fixture lock"),
            ResidentLifecycle::Ready { .. }
        ),
        "a rejected request must not flip the lifecycle back to Loading"
    );

    // The loop finishing is what reopens the gate; the last progress frame
    // staying at edge-recompute must not lock later rebuilds out forever.
    progress.write().expect("local fixture lock").dbnums_done = 2;
    assert!(
        super::try_begin_rebuild_in(&lifecycle, &progress),
        "once the edge loop has finished, the next rebuild is admitted again"
    );
}

/// 第三段在飞的那段时间里，**任务开工前对齐的换代**会把展示帧清零重写成别的相位
/// （`refresh_before_task` → `initialize_file_backed_at_startup` → `mark_loading("mesh-warm")`）；
/// 那一趟跑在 `Ready` 下，生命周期挡不住它（这正是 #53 那条「只看生命周期会漏守」的另一面）。
/// 准入闸若只看 `phase` 与逐库计数，换代一过闸就开：第二趟「重新生成房间」踩着第一趟的逐库循环
/// 进来，两趟交错清边、互相覆盖盖章。**「在飞」不是展示**——清零展示帧不许把这个事实抹掉。
#[test]
fn room_rebuild_regression_a_generation_swap_must_not_reopen_the_edge_loop_gate() {
    let lifecycle = RwLock::new(ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    });
    let progress = RwLock::new(ResidentProgress::default());
    assert!(super::try_begin_rebuild_in(&lifecycle, &progress));
    // 第一趟进了第三段：相位与逐库计数就位，生命周期已翻回 Ready（照 `run_rooms_rebuild` 的顺序）。
    *lifecycle.write().expect("local fixture lock") = ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    };
    *progress.write().expect("local fixture lock") = ResidentProgress {
        phase: "edge-recompute".into(),
        dbnums_done: 1,
        dbnums_total: 3,
        ..ResidentProgress::default()
    };
    assert!(
        !super::try_begin_rebuild_in(&lifecycle, &progress),
        "control: 第三段在飞时，第二趟不许进"
    );

    // 并发：任务开工前对齐发现 composite 变了 → 换代装载 → `mark_loading("mesh-warm")`
    // （`reset_progress_in` 就是它清零展示帧的那一步，这里如实照走）。
    super::reset_progress_in(&progress, "mesh-warm");
    assert_eq!(
        progress.read().expect("local fixture lock").phase,
        "mesh-warm",
        "换代确实把展示帧换走了——这一段模拟的就是它"
    );
    assert!(
        !super::try_begin_rebuild_in(&lifecycle, &progress),
        "第一趟的逐库循环还在飞：换代清零展示帧不许把准入闸打开（#53 两趟交错的老面孔）"
    );

    // 收工方向也得对：本地槽上照走生产的「开工 / 收工」那一对（`note_recompute_dbnums_total` /
    // `note_edge_recompute_finished`），闸会重开——不许被这一格焊死。
    super::note_recompute_dbnums_total_in(&progress, 3);
    assert!(
        !super::try_begin_rebuild_in(&lifecycle, &progress),
        "开工即拒（在飞那一格立起来了）"
    );
    super::note_edge_recompute_finished_in(&progress);
    assert!(
        super::try_begin_rebuild_in(&lifecycle, &progress),
        "收工即放：逐库循环跑完那一刻闸重开，换代清零过展示帧也不影响"
    );
}
