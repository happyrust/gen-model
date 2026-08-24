use std::path::{Path, PathBuf};

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn source_files(root: &Path, relative: &str, extension: &str) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root.join(relative))
        .into_iter()
        .filter_map(Result::ok)
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .collect()
}

#[test]
fn executable_sources_and_dependency_manifests_have_no_occ_backend() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for relative in ["Cargo.toml", "Cargo.lock", "python/Cargo.toml"] {
        let text = read(root, relative).to_ascii_lowercase();
        for forbidden in ["opencascade", "opencascade-sys", "occt-rs"] {
            assert!(
                !text.contains(forbidden),
                "{relative} still contains forbidden dependency {forbidden}"
            );
        }
        assert!(
            !text.lines().any(|line| {
                let line = line.trim();
                line.starts_with("occ =") || line.starts_with("occ=")
            }),
            "{relative} still declares the occ feature"
        );
    }

    let guard = root.join("src/fast_model/occ_retirement_guard.rs");
    let mut rust_files = source_files(root, "src", "rs");
    rust_files.extend(source_files(root, "python/src", "rs"));
    for path in rust_files {
        if path == guard {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        for forbidden in [
            "gen_occ_shape",
            "gen_occ_mesh",
            "apply_insts_boolean_occ",
            "apply_cata_neg_boolean_occ",
            "cfg(feature = \"occ\")",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} still contains retired API {forbidden}",
                path.display()
            );
        }
    }

    for path in source_files(root, "python", "py") {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .to_ascii_lowercase();
        assert!(
            !text.contains("add_dll_directory") && !text.contains("opencascade"),
            "{} still contains an OCCT loader",
            path.display()
        );
    }
}

#[test]
fn workflows_do_not_download_or_install_occt() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in source_files(root, ".github/workflows", "yml") {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .to_ascii_lowercase();
        for forbidden in [
            "occt_archive_url",
            "occt_archive_sha256",
            "choco install opencascade",
            "vcpkg install opencascade",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} still installs OCCT through {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn rebuild_readiness_gate_covers_every_reusable_surface_and_fails_closed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = read(root, "scripts/Test-OccRetireRebuildReadiness.ps1");
    for variant in [
        "PrimLCylinder",
        "PrimSphere",
        "PrimLSnout",
        "PrimDish",
        "PrimCTorus",
        "PrimRTorus",
    ] {
        assert!(script.contains(variant), "readiness audit misses {variant}");
    }
    for gate in [
        "$missingCaliber -eq 0",
        "$referencedMissing -eq 0",
        "$badReusable -eq 0",
        "@($pendingRows).Count -eq 0",
        "@($orphanIds).Count -eq 0",
        "@($missingMeshFiles).Count -eq 0",
        "if ($RequireReady -and -not $ready) { exit 1 }",
    ] {
        assert!(
            script.contains(gate),
            "readiness audit drops hard gate {gate}"
        );
    }
}
