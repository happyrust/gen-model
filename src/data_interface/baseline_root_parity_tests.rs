use super::*;
use std::collections::BTreeSet;

#[tokio::test]
#[ignore = "requires AMS 8000 and MDB templates; read-only"]
async fn frozen_baseline_and_window_target_have_the_same_roots() {
    use crate::data_interface::frozen_source::FrozenSource;
    use crate::data_interface::window_root_plan::WindowRootSources;
    use crate::fast_model::e3d_model_service::E3dModelService;
    let path = std::env::var_os("AIOS_TARGET_TEST_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001".into());
    let source = FrozenSource::capture(&path).unwrap();
    let baseline = frozen_baseline_model_plan(8000, &source).await.unwrap();
    let service = E3dModelService::from_frozen_source(&source).await.unwrap();
    let session = source.target_sesno();
    let set = service.build_set(8000, Some(session)).unwrap();
    let previous = e3d_io::session::DbView::open(&path)
        .unwrap()
        .walk_sessions()
        .unwrap()
        .into_iter()
        .map(|stamp| stamp.sesno)
        .filter(|stamp| *stamp < session)
        .max()
        .unwrap();
    let base = service.build_set(8000, Some(previous)).unwrap();
    let window = WindowRootSources::collect(
        &path,
        previous,
        session,
        &base,
        &set,
        &configured_delivery_unit_types(),
        &[],
    )
    .unwrap();
    let baseline_roots: BTreeSet<_> = baseline
        .work_items
        .iter()
        .map(|item| item.target_refno.clone())
        .collect();
    let window_roots: BTreeSet<_> = window
        .roots_t
        .iter()
        .map(|root| root.root.to_pdms_str())
        .collect();
    assert!(window_roots.contains("24384/23257"));
    assert!(
        baseline_roots == window_roots,
        "baseline={} window={}; unexpected baseline roots={:?}; missing={:?}",
        baseline_roots.len(),
        window_roots.len(),
        baseline_roots
            .difference(&window_roots)
            .take(8)
            .collect::<Vec<_>>(),
        window_roots
            .difference(&baseline_roots)
            .take(8)
            .collect::<Vec<_>>()
    );
}
