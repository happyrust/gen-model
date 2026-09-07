use super::E3dModelService;
use crate::data_interface::frozen_source::FrozenSource;
use std::io::Write;

#[tokio::test]
#[ignore = "requires AMS and the mismatched APS 7600 catalogue fixture"]
async fn frozen_geometry_hold_preserves_element_and_catalogue_cause() {
    let source = std::env::var_os("AIOS_TARGET_TEST_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001".into());
    let frozen = FrozenSource::capture(&source).unwrap();
    let (pins, _) = super::current_mdb_sources().unwrap();
    let dependencies = pins
        .into_iter()
        .filter(|pin| pin.dbnum != frozen.dbnum())
        .map(|pin| {
            // Reproduce the isolated harness's wrong same-number catalogue,
            // rather than depending on the caller's catalogue priority.
            let file = if pin.dbnum == 7600 {
                std::path::PathBuf::from(
                    "D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000/aps7600_0001",
                )
            } else {
                pin.file
            };
            FrozenSource::capture(&file)
                .unwrap()
                .commit_token_json()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let service = E3dModelService::from_initialization_sources(&frozen, &dependencies)
        .await
        .unwrap();
    let roots = vec!["24384/26229".to_owned()];
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let (generated, outcome) =
        tokio::join!(service.generate_frozen_roots(8000, &roots, tx), rx.recv());
    generated.unwrap();
    let error = outcome.unwrap().outcome.unwrap_err();
    for expected in ["24384/26234", "24384/26235", "ATTA", "7600", "23984/105633"] {
        assert!(error.contains(expected), "missing {expected}: {error}");
    }
}

#[tokio::test]
#[ignore = "requires the AMS 8000 file and MDB templates; modifies only a temporary copy"]
async fn initialization_target_survives_append_and_reopen_but_rejects_replacement() {
    let source = std::env::var_os("AIOS_TARGET_TEST_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001".into());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ams8000_0001");
    std::fs::copy(&source, &path).unwrap();
    let frozen = FrozenSource::capture(&path).unwrap();
    let service = E3dModelService::from_initialization_sources(&frozen, &[])
        .await
        .unwrap();
    let before = service
        .initialization_target_hash(8000, frozen.target_sesno())
        .unwrap();

    // Grow the same file without changing the frozen session/index anchors.
    // This is the relevant identity change made by a later SAVEWORK.
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0; 2048])
        .unwrap();
    frozen
        .reopen_verified()
        .expect("same pinned generation remains readable");
    assert_eq!(
        before,
        service
            .initialization_target_hash(8000, frozen.target_sesno())
            .unwrap()
    );
    let reopened = E3dModelService::from_initialization_sources(&frozen, &[])
        .await
        .unwrap();
    assert_eq!(
        before,
        reopened
            .initialization_target_hash(8000, frozen.target_sesno())
            .unwrap()
    );

    std::fs::rename(&path, dir.path().join("original.db")).unwrap();
    std::fs::copy(source, &path).unwrap();
    assert!(
        E3dModelService::from_initialization_sources(&frozen, &[])
            .await
            .is_err(),
        "a replacement at the same path is not an append to the pinned generation"
    );
}
