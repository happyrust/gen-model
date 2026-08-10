//! Portable Issue #19 regression over the real dbnum=8000 session chain.
//!
//! Run:
//! `cargo test --test db8000_two_delete_fixture -- --ignored --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod archive;

use aios_core::RefnoEnum;
use aios_core::pdms_types::RefU64;
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::manual_update::{NetOp, merge_net_changes};
use archive::verify_and_extract;
use parse_pdms_db::paged::PagedDbSession;
use pdms_io::io::EleOperationDetail;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

fn fixture_root() -> PathBuf {
    std::env::var_os("AIOS_DB8000_TWO_DELETE_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/issues/issue-019-cross-session-parent-child-delete")
        })
}

fn assert_session(path: &Path, expected: u32) {
    let session = PagedDbSession::open(path).expect("open extracted snapshot");
    assert_eq!(session.snapshot().sesno, expected, "{}", path.display());
}

#[test]
#[ignore = "real-file regression fixture; opt in explicitly"]
fn final_file_window_preserves_child_then_parent_delete_sessions() {
    let fixture =
        verify_and_extract(&fixture_root()).expect("verify and extract Issue #19 fixture");
    let baseline = fixture.path_for_role("baseline").unwrap();
    let child_deleted = fixture.path_for_role("child_deleted").unwrap();
    let final_file = fixture.path_for_role("parent_deleted").unwrap();
    assert_session(&baseline, 24);
    assert_session(&child_deleted, 25);
    assert_session(&final_file, 26);

    let child = RefU64::from_str("24384/24779").unwrap();
    let parent = RefU64::from_str("24384/24778").unwrap();
    let zone = RefU64::from_str("24384/24775").unwrap();
    let collected = IncrementPipeline::collect_changes(&final_file, 25..=26).unwrap();

    assert_eq!(collected.keys().copied().collect::<Vec<_>>(), vec![25, 26]);
    assert_eq!(collected[&25].len(), 2, "session 25: {:?}", collected[&25]);
    assert_eq!(collected[&26].len(), 2, "session 26: {:?}", collected[&26]);
    assert!(
        collected[&25]
            .iter()
            .any(|op| { op.refno == child && matches!(op.detail, EleOperationDetail::Deleted) })
    );
    assert!(collected[&25].iter().any(|op| {
        op.refno == parent
            && matches!(
                &op.detail,
                EleOperationDetail::Modified(modified)
                    if modified.noun == "EQUI" && modified.children_changed.is_some()
            )
    }));
    assert!(
        collected[&26]
            .iter()
            .any(|op| { op.refno == parent && matches!(op.detail, EleOperationDetail::Deleted) })
    );
    assert!(collected[&26].iter().any(|op| {
        op.refno == zone
            && matches!(
                &op.detail,
                EleOperationDetail::Modified(modified)
                    if modified.noun == "ZONE" && modified.children_changed.is_some()
            )
    }));

    let by_refno = merge_net_changes(&collected)
        .into_iter()
        .map(|change| (change.refno, change.net))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        by_refno[&RefnoEnum::from(child).to_pdms_str()],
        NetOp::Deleted
    );
    assert_eq!(
        by_refno[&RefnoEnum::from(parent).to_pdms_str()],
        NetOp::Deleted
    );
    assert_eq!(
        by_refno[&RefnoEnum::from(zone).to_pdms_str()],
        NetOp::Modified
    );
}
