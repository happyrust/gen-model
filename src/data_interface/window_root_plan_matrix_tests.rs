//! Real recorded windows exercise collection, both owner graphs, and root selection.
use super::*;
use crate::data_interface::direct_store::DirectSchema;
use crate::data_interface::generation_root::resolve_delivery_unit_types;
use e3d_io::db_element::{DbFilePin, template_file_for};

#[derive(serde::Deserialize)]
struct RecordedWindow {
    label: String,
    db: std::path::PathBuf,
    base: u32,
    target: u32,
    #[serde(default)]
    append_units: Vec<String>,
    #[serde(default)]
    expected_degradation: Option<String>,
    regen: BTreeSet<String>,
    delete: BTreeSet<String>,
}

#[test]
#[ignore = "requires the recorded E3D files; missing evidence fails explicitly"]
fn recorded_matrix_windows_reach_expected_roots() {
    let manifest = std::env::var_os("AIOS_INCREMENT_MATRIX_MANIFEST")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/increment_matrix/windows.json")
        });
    let cases: Vec<RecordedWindow> = serde_json::from_slice(
        &std::fs::read(&manifest).expect("recorded window manifest must exist"),
    )
    .expect("recorded window manifest must be valid");
    assert!(!cases.is_empty(), "an empty matrix cannot pass");
    let schema = DirectSchema::open_from_env().expect("E3D template directory");
    for case in cases {
        assert!(
            case.db.is_file(),
            "{}: missing {}",
            case.label,
            case.db.display()
        );
        let open = |sesno| {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: case.db.clone(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin session");
            set
        };
        let (base, target) = (open(case.base), open(case.target));
        let units = resolve_delivery_unit_types(&case.append_units);
        let sources = WindowRootSources::collect(
            &case.db,
            case.base,
            case.target,
            &base,
            &target,
            &units,
            &[],
        )
        .unwrap_or_else(|error| panic!("{}: {error:#}", case.label));
        assert!(
            sources.closure.is_complete(),
            "{}: {:?}",
            case.label,
            sources.closure.unreadable
        );
        let index = scan_index(&case.db, Some(case.base)).expect("base index");
        let persisted: Vec<_> = enumerate_generation_roots(&base, &index.roots, &units)
            .expect("base roots")
            .into_iter()
            .map(|root| PersistedRoot {
                refno: root.root,
                noun: root.noun,
            })
            .collect();
        let plan = plan_window_roots(&sources.roots_t, &persisted, &sources.impact(&persisted));
        assert_eq!(
            plan.degraded, case.expected_degradation,
            "{}: unexpected fallback",
            case.label
        );
        let regen: BTreeSet<_> = plan.regen.iter().map(|r| r.root.to_pdms_str()).collect();
        let deleted: BTreeSet<_> = plan.delete.iter().map(|r| r.refno.to_pdms_str()).collect();
        assert_eq!(regen, case.regen, "{}: wrong regenerated roots", case.label);
        assert_eq!(deleted, case.delete, "{}: wrong deleted roots", case.label);
        println!(
            "{} {}→{}: regen={regen:?} delete={deleted:?} advance={}",
            case.label,
            case.base,
            case.target,
            plan.advance.len()
        );
    }
}
