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
