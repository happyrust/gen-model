use super::source_for_file;
use crate::data_interface::frozen_source::FrozenSource;
use std::io::Write;

#[test]
#[ignore = "requires AMS 8000; writes only a temporary copy"]
fn baseline_read_and_model_plan_reuse_the_file_round_token_after_append() {
    let source = std::env::var_os("AIOS_TARGET_TEST_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001".into());
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ams8000_0001");
    std::fs::copy(&source, &path).unwrap();
    let active = FrozenSource::capture(&path).unwrap();
    let token = active.commit_token_json().unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0; 2048])
        .unwrap();
    active.reopen_verified().unwrap();
    for target in [None, Some(active.target_sesno())] {
        let input = source_for_file(&path, target, Some(&active)).unwrap();
        assert_eq!(
            input.commit_token_json().unwrap(),
            token,
            "baseline data, model manifest and watermark must carry one token"
        );
    }
    assert!(source_for_file(&path, Some(active.target_sesno() + 1), Some(&active)).is_err());

    let other = temp.path().join("other8000");
    std::fs::copy(&source, &other).unwrap();
    assert!(
        source_for_file(&other, None, Some(&active))
            .unwrap()
            .matches_path(&other)
    );
    std::fs::rename(&path, temp.path().join("original8000")).unwrap();
    std::fs::copy(&source, &path).unwrap();
    assert!(
        source_for_file(&path, None, Some(&active)).is_err(),
        "same-path replacement must fail instead of recapturing a new file"
    );
}
