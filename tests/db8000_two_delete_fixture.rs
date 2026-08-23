//! Portable Issue #19 regression over the real dbnum=8000 session chain.
//! Production and this test both use the terminal net-window collector; the
//! retired per-session replay collector is deliberately absent from the test graph.
//!
//! Run:
//! `cargo test --test db8000_two_delete_fixture -- --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod archive;

use aios_core::RefnoEnum;
use aios_core::pdms_types::RefU64;
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::manual_update::{NetOp, merge_net_changes};
use archive::{ExtractedFixture, verify_and_extract};
use parse_pdms_db::paged::PagedDbSession;
use pdms_io::io::{EleOperationData, EleOperationDetail};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const BASELINE_SESNO: u32 = 24;
const CHILD_DELETE_SESNO: u32 = 25;
const PARENT_DELETE_SESNO: u32 = 26;

struct Db8000Fixture {
    _extracted: ExtractedFixture,
    baseline: PathBuf,
    child_deleted: PathBuf,
    parent_deleted: PathBuf,
}

impl Db8000Fixture {
    fn load() -> Self {
        let extracted =
            verify_and_extract(&fixture_root()).expect("verify and extract Issue #19 fixture");
        let baseline = extracted.path_for_role("baseline").unwrap();
        let child_deleted = extracted.path_for_role("child_deleted").unwrap();
        let parent_deleted = extracted.path_for_role("parent_deleted").unwrap();
        Self {
            _extracted: extracted,
            baseline,
            child_deleted,
            parent_deleted,
        }
    }

    fn collect(&self, start: u32, end: u32) -> BTreeMap<u32, Vec<EleOperationData>> {
        IncrementPipeline::collect_window(&self.parent_deleted, start as i32..=end as i32)
            .expect("collect db8000 terminal net window")
            .range_eles
    }

    fn collect_full(
        &self,
        start: u32,
        end: u32,
    ) -> aios_database::data_interface::increment_pipeline::CollectedWindow {
        IncrementPipeline::collect_window(&self.parent_deleted, start as i32..=end as i32)
            .expect("collect db8000 terminal net window")
    }
}

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

fn refs() -> (RefU64, RefU64, RefU64) {
    (
        RefU64::from_str("24384/24775").unwrap(),
        RefU64::from_str("24384/24778").unwrap(),
        RefU64::from_str("24384/24779").unwrap(),
    )
}

fn operation_signatures(changes: &BTreeMap<u32, Vec<EleOperationData>>) -> Vec<String> {
    let mut signatures = changes
        .iter()
        .flat_map(|(sesno, operations)| {
            operations.iter().map(move |operation| {
                let detail = match &operation.detail {
                    EleOperationDetail::Add(element) => format!("Add:{}", element.noun),
                    EleOperationDetail::Deleted => "Deleted".to_string(),
                    EleOperationDetail::Modified(modified) => format!(
                        "Modified:{}:children={}",
                        modified.noun,
                        modified.children_changed.is_some()
                    ),
                    EleOperationDetail::None => "None".to_string(),
                };
                format!("{sesno}:{}:{detail}", operation.refno)
            })
        })
        .collect::<Vec<_>>();
    signatures.sort_unstable();
    signatures
}

fn assert_operation_sessions(changes: &BTreeMap<u32, Vec<EleOperationData>>) {
    for (sesno, operations) in changes {
        assert!(
            operations.iter().all(|operation| operation.sesno == *sesno),
            "operation session must match its map partition: {sesno} -> {operations:?}"
        );
    }
}

#[test]
fn archive_contains_the_three_declared_db8000_sessions() {
    let fixture = Db8000Fixture::load();
    assert_session(&fixture.baseline, BASELINE_SESNO);
    assert_session(&fixture.child_deleted, CHILD_DELETE_SESNO);
    assert_session(&fixture.parent_deleted, PARENT_DELETE_SESNO);
}

#[test]
fn final_file_window_emits_one_terminal_operation_per_refno() {
    let fixture = Db8000Fixture::load();
    let collected = fixture.collect_full(CHILD_DELETE_SESNO, PARENT_DELETE_SESNO);
    let operations = &collected.range_eles;
    let (zone, parent, child) = refs();

    assert_eq!(collected.session_sesnos, vec![25, 26]);
    assert_operation_sessions(operations);
    assert_eq!(
        operations.keys().copied().collect::<Vec<_>>(),
        vec![PARENT_DELETE_SESNO],
        "净窗口只按右端会话发布终态操作"
    );
    assert_eq!(operations[&PARENT_DELETE_SESNO].len(), 3);
    assert!(
        operations[&PARENT_DELETE_SESNO]
            .iter()
            .any(|op| { op.refno == child && matches!(op.detail, EleOperationDetail::Deleted) })
    );
    assert!(
        operations[&PARENT_DELETE_SESNO]
            .iter()
            .any(|op| { op.refno == parent && matches!(op.detail, EleOperationDetail::Deleted) })
    );
    assert!(operations[&PARENT_DELETE_SESNO].iter().any(|op| {
        op.refno == zone
            && matches!(
                &op.detail,
                EleOperationDetail::Modified(modified)
                    if modified.noun == "ZONE" && modified.children_changed.is_some()
            )
    }));
}

#[test]
fn final_history_matches_the_session_25_point_in_time_snapshot() {
    let fixture = Db8000Fixture::load();
    let from_session_25_file = IncrementPipeline::collect_window(
        &fixture.child_deleted,
        CHILD_DELETE_SESNO as i32..=CHILD_DELETE_SESNO as i32,
    )
    .expect("collect session 25 snapshot")
    .range_eles;
    let from_final_history = fixture.collect(CHILD_DELETE_SESNO, CHILD_DELETE_SESNO);

    assert_eq!(
        operation_signatures(&from_final_history),
        operation_signatures(&from_session_25_file),
        "later parent deletion must not rewrite session 25 history"
    );
}

#[test]
fn combined_window_is_not_a_union_of_intermediate_session_operations() {
    let fixture = Db8000Fixture::load();
    let combined = fixture.collect(CHILD_DELETE_SESNO, PARENT_DELETE_SESNO);
    let mut slices = fixture.collect(CHILD_DELETE_SESNO, CHILD_DELETE_SESNO);
    slices.extend(fixture.collect(PARENT_DELETE_SESNO, PARENT_DELETE_SESNO));

    assert_ne!(
        operation_signatures(&combined),
        operation_signatures(&slices),
        "净窗口必须消除 EQUI 在中间会话的 Modified，不能退回逐会话操作并集"
    );
}

#[test]
fn window_folds_to_box_and_equi_deleted_with_zone_modified() {
    let fixture = Db8000Fixture::load();
    let collected = fixture.collect(CHILD_DELETE_SESNO, PARENT_DELETE_SESNO);
    let (zone, parent, child) = refs();

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
    assert_eq!(by_refno.len(), 3, "unexpected extra model net changes");
}
