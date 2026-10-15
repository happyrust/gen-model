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

    // run_rooms_rebuild publishes Ready before its per-dbnum edge loop, and holds the
    // flight guard for the whole third stage (2026-10-15: the guard owns that cell).
    *lifecycle.write().expect("local fixture lock") = ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    };
    let flight = super::start_edge_recompute_in(&progress, 2);
    progress.write().expect("local fixture lock").dbnums_done = 1;
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
    drop(flight);
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
    // 第一趟进了第三段：守卫开工（相位 / 库数 / 在飞一次就位），生命周期已翻回 `Ready`
    // （照 `run_rooms_rebuild` 的顺序）。
    *lifecycle.write().expect("local fixture lock") = ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    };
    let flight = super::start_edge_recompute_in(&progress, 3);
    progress.write().expect("local fixture lock").dbnums_done = 1;
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

    // 收工方向也得对：守卫落下（`Drop` 就是生产的收工那一步），闸会重开——不许被这一格焊死。
    drop(flight);
    assert!(
        super::try_begin_rebuild_in(&lifecycle, &progress),
        "收工即放：逐库循环跑完那一刻闸重开，换代清零过展示帧也不影响"
    );
}

/// 展示帧不是判据（2026-10-15，第三处未闭面的收口）：逐库循环 / 收尾里 panic 展开时，那一趟的
/// 展示帧会**如实停在死的地方**（`phase` 还写着 `edge-recompute`、计数停在半路）——这是
/// 「`current_stage` 是唯一说得出死在哪一步的字段」的同一条口径，也是设计。但准入闸不许把它读成
/// 「还在飞」：那一帧没人会再去改（下一趟重建正是被它挡在门外的那个），于是每一枚
/// `POST /api/v1/rooms/rebuild` 都 409「正在重建中」，直到进程重启。
/// 「在飞」只有一个所有权明确的判据：`edge_recompute_in_flight`，由守卫持有、Drop 放下。
#[test]
fn room_rebuild_regression_a_frame_frozen_by_a_panicked_loop_is_not_a_lifetime_sentence() {
    let lifecycle = RwLock::new(ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    });
    // 上一趟在第三段里炸了 / 被展开带走了：帧如实停在中途，「在飞」那一格没人立着。
    let progress = RwLock::new(ResidentProgress {
        phase: "edge-recompute".into(),
        dbnums_done: 1,
        dbnums_total: 3,
        ..ResidentProgress::default()
    });
    assert!(
        super::try_begin_rebuild_in(&lifecycle, &progress),
        "帧停在「死在哪一步」是如实、不是判据：准入只读「在飞」那一格，重建口不许被它焊死"
    );
}

/// 守卫的展开路（2026-10-15，第三处未闭面的收口）：逐库循环 / 收尾里 panic 展开时，「在飞」
/// 那一格必须跟着守卫一起落下。手工成对的那次「放」在展开的路上根本走不到，此后每一枚
/// `POST /api/v1/rooms/rebuild` 都 409「正在重建中」，直到进程重启。
#[test]
fn room_rebuild_regression_a_panicking_edge_loop_must_not_shut_the_gate_forever() {
    let lifecycle = RwLock::new(ResidentLifecycle::Ready {
        generation: "g1".into(),
        completed_at: "fixture".into(),
    });
    let progress = RwLock::new(ResidentProgress::default());
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _flight = super::start_edge_recompute_in(&progress, 2);
        // 循环跑到一半（第二个库还没算完），重算本体 panic 展开。
        progress.write().expect("local fixture lock").dbnums_done = 1;
        panic!("逐库房间边重算里 panic（库给了个坏答案）");
    }));
    assert!(unwound.is_err(), "control: panic 确实发生了");
    let frame = progress.read().expect("local fixture lock");
    assert!(
        !frame.edge_recompute_in_flight,
        "展开路径也要放下「在飞」：放开只有守卫 Drop 一条路"
    );
    assert_eq!(
        frame.phase, "edge-recompute",
        "展示帧照旧停在那一趟（如实写着死在哪一步，这不是判据）"
    );
    assert_eq!(frame.dbnums_done, 1);
    assert_eq!(frame.dbnums_total, 2);
    drop(frame);
    assert!(
        super::try_begin_rebuild_in(&lifecycle, &progress),
        "闸重开：下一枚重建请求不许再被 409 焊死"
    );
}
