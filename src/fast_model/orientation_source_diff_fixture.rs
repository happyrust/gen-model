//! Real synthetic binary source -> pinned S/T readers -> production diff/root
//! collection -> generation/baking parity. No installed/live project is read.
use super::orientation_increment_regression_tests::{close, rooms};
use super::*;
use crate::data_interface::generation_root::{
    enumerate_generation_roots, refno_from_e3d, resolve_delivery_unit_types,
};
use crate::data_interface::window_root_plan::{
    PersistedRoot, WindowRootSources, plan_window_roots,
};
use e3d_attlib::db1_hash;
use e3d_io::record::descriptor::DescriptorValue;
use e3d_io::record::template::TemplateProvider;
use e3d_io::record::text::TextFormat;
use e3d_io::write::{ElementWrite, RecordDraft, SessionRequest, write_element_session};
use e3d_model::element_diff::diff_element;
use e3d_model::transform::{local_placement, world_matrix};

const SOURCE_DB: u32 = 8000;
const PAGE_WORDS: usize = 1024;
const PAGE_BYTES: usize = PAGE_WORDS * 4;
const BASE: u32 = 41;
fn source_ref(n: u32) -> RefNo {
    RefNo::new(SOURCE_DB | (1 << 13), n)
}
fn put(page: &mut [u8], offset: usize, value: u32) {
    page[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn schema_file(name: &str) -> PathBuf {
    let path = e3d_io::bundled_schema_root()
        .join("e3d31/binaries")
        .join(name);
    assert!(
        path.is_file(),
        "bundled schema is required, never silently skip: {}",
        path.display()
    );
    path
}
fn template_provider() -> TemplateProvider {
    TemplateProvider::open(schema_file("desvir.dat"), Some(db1_hash("DESI")))
        .expect("bundled templates")
}
fn seed_draft(
    provider: &mut TemplateProvider,
    n: u32,
    noun: &str,
    owner: RefNo,
    pos: [f64; 3],
    size: Option<[f64; 3]>,
) -> RecordDraft {
    let template = provider.template_for(db1_hash(noun)).unwrap().unwrap();
    let mut draft = RecordDraft::from_template(source_ref(n), owner, template).unwrap();
    for (name, value) in [
        ("POS", DescriptorValue::RealArray(pos.to_vec())),
        ("ORI", DescriptorValue::RealArray(vec![0.; 3])),
    ] {
        draft
            .set_attribute(template, db1_hash(name), &value, TextFormat::Detect)
            .unwrap();
    }
    if let Some(size) = size {
        for (name, value) in ["XLEN", "YLEN", "ZLEN"].into_iter().zip(size) {
            draft
                .set_attribute(
                    template,
                    db1_hash(name),
                    &DescriptorValue::Real(value),
                    TextFormat::Detect,
                )
                .unwrap();
        }
    }
    draft
}

// Four-page reader fixtures in e3d-io establish the descriptor/session/index
// shape. Here each REAL RecordDraft occupies its own data page. Eight keys
// keep 1- and 2-candidate windows below the production 30% degradation guard.
fn create_source(dir: &Path) -> PathBuf {
    create_source_with(dir, false)
}

// `second_unit` 时 EQUI 20 也挂一只同款 BOX 21（多根同窗用例），凑数 EQUI 相应少放一个。
fn create_source_with(dir: &Path, second_unit: bool) -> PathBuf {
    let mut provider = template_provider();
    let mut parent = seed_draft(&mut provider, 10, "EQUI", RefNo::new(0, 0), [100.; 3], None);
    parent.push_member(source_ref(11));
    let child = seed_draft(
        &mut provider,
        11,
        "BOX",
        source_ref(10),
        [20., 0., 0.],
        Some([40., 20., 10.]),
    );
    let mut drafts = vec![parent, child];
    if second_unit {
        let mut parent2 = seed_draft(&mut provider, 20, "EQUI", RefNo::new(0, 0), [300.; 3], None);
        parent2.push_member(source_ref(21));
        drafts.push(parent2);
        drafts.push(seed_draft(
            &mut provider,
            21,
            "BOX",
            source_ref(20),
            [20., 0., 0.],
            Some([40., 20., 10.]),
        ));
    }
    let fillers: &[u32] = if second_unit {
        &[30, 40, 50, 60, 70]
    } else {
        &[20, 30, 40, 50, 60, 70]
    };
    for &n in fillers {
        drafts.push(seed_draft(
            &mut provider,
            n,
            "EQUI",
            RefNo::new(0, 0),
            [0.; 3],
            None,
        ));
    }
    let count = drafts.len();
    let mut pages = vec![vec![0u8; PAGE_BYTES]; 3 + count];
    for (offset, value) in [
        (0x08, SOURCE_DB),
        (0x20, db1_hash("DESI")),
        (0x28, 1),
        (0x2c, 1),
        (0x30, 1),
        (0x34, PAGE_WORDS as u32),
        (0x38, source_ref(10).word0),
        (0x3c, source_ref(10).word1),
        (0x154, 1),
    ] {
        put(&mut pages[0], offset, value);
    }
    for (offset, value) in [
        (0, 3),
        (4, u32::MAX),
        (0x0c, BASE),
        (0x10, u32::MAX),
        (0x14, (pages.len() - 1) as u32),
        (0x18, 1),
        (0x1c, 2),
        (0x20, 1),
    ] {
        put(&mut pages[1], offset, value);
    }
    let free = PAGE_WORDS - 7 - count * 4;
    for (i, value) in [5, 13_387_743, 0, 2, 2, 0, free as u32]
        .into_iter()
        .enumerate()
    {
        put(&mut pages[2], i * 4, value);
    }
    for (i, draft) in drafts.into_iter().enumerate() {
        let page = 3 + i;
        put(&mut pages[page], 0, 7);
        let placed = draft.encode(page as u32, 10, 1);
        assert!(
            40 + placed.bytes.len() < PAGE_BYTES,
            "seed record exceeds its private page"
        );
        pages[page][40..40 + placed.bytes.len()].copy_from_slice(&placed.bytes);
        for (j, value) in [
            placed.refno.word0,
            placed.refno.word1,
            page as u32,
            e3d_io::write::record::pack_offset(10, 1),
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut pages[2], (7 + i * 4 + j) * 4, value);
        }
    }
    let path = dir.join("synthetic_ori_8000_0001");
    std::fs::write(&path, pages.concat()).unwrap();
    path
}

fn pin(path: &Path, sesno: u32) -> Arc<DbSet> {
    let set = Arc::new(DbSet::with_attlib_file(schema_file("attlib.dat")).unwrap());
    set.add_db(DbFilePin {
        file: path.to_path_buf(),
        template: schema_file("desvir.dat"),
        db_type: Some("DESI".into()),
        sesno: Some(sesno),
    })
    .unwrap();
    set
}

/// 一次真实写会话里更新若干元素的若干属性（每项 `(序号, noun, [(属性名, 值)])`），一个
/// SessionRequest 提交 = S→T 的一次 SAVEWORK。
fn write_updates(path: &Path, edits: &[(u32, &str, Vec<(&str, DescriptorValue)>)]) {
    let mut provider = template_provider();
    let mut engine = e3d_io::ReadOnlyEngine::open(path).unwrap();
    let mut writes = Vec::new();
    for (n, noun, attrs) in edits {
        let element = engine
            .find_element(source_ref(*n))
            .unwrap()
            .unwrap_or_else(|| panic!("fixture element {n} missing"));
        let mut draft = RecordDraft::from_record_bytes(&element.raw_bytes).unwrap();
        let template = provider.template_for(db1_hash(noun)).unwrap().unwrap();
        for (name, value) in attrs {
            draft
                .set_attribute(template, db1_hash(name), value, TextFormat::Detect)
                .unwrap();
        }
        writes.push(ElementWrite::Update(draft));
    }
    drop(engine);
    let commit = write_element_session(
        path,
        &SessionRequest {
            user: "orientation-test",
            comment: "isolated synthetic ORI save",
            clock: None,
        },
        &writes,
        &[],
    )
    .unwrap();
    assert!(commit.written.sesno > commit.previous.sesno);
}

/// 一次真实写会话里给元素追加匿名显式属性（`(序号, hash, 文本)`）。与真库
/// 那 37 条同形状：hash 高过系统名区间（`is_uda`）、type_code 10240（存储
/// 类型 40 文本）——模板没声明、字典也答不出，是应用私有存储。
fn write_anonymous_text(path: &Path, edits: &[(u32, u32, &str)]) {
    const ANON_TEXT_TYPE: u16 = 10240;
    let mut engine = e3d_io::ReadOnlyEngine::open(path).unwrap();
    let mut writes = Vec::new();
    for (n, hash, text) in edits {
        let element = engine
            .find_element(source_ref(*n))
            .unwrap()
            .unwrap_or_else(|| panic!("fixture element {n} missing"));
        let mut draft = RecordDraft::from_record_bytes(&element.raw_bytes).unwrap();
        draft
            .add_explicit_text_ascii(*hash, ANON_TEXT_TYPE, text)
            .unwrap();
        writes.push(ElementWrite::Update(draft));
    }
    drop(engine);
    let commit = write_element_session(
        path,
        &SessionRequest {
            user: "orientation-test",
            comment: "isolated anonymous-attribute save",
            clock: None,
        },
        &writes,
        &[],
    )
    .unwrap();
    assert!(commit.written.sesno > commit.previous.sesno);
}

/// 一次真实写会话里删掉元素的匿名显式属性（`(序号, hash)`）。
fn remove_anonymous(path: &Path, edits: &[(u32, u32)]) {
    let mut engine = e3d_io::ReadOnlyEngine::open(path).unwrap();
    let mut writes = Vec::new();
    for (n, hash) in edits {
        let element = engine
            .find_element(source_ref(*n))
            .unwrap()
            .unwrap_or_else(|| panic!("fixture element {n} missing"));
        let mut draft = RecordDraft::from_record_bytes(&element.raw_bytes).unwrap();
        assert!(draft.remove_explicit(*hash), "fixture must carry {hash:#X}");
        writes.push(ElementWrite::Update(draft));
    }
    drop(engine);
    let commit = write_element_session(
        path,
        &SessionRequest {
            user: "orientation-test",
            comment: "isolated anonymous-attribute removal",
            clock: None,
        },
        &writes,
        &[],
    )
    .unwrap();
    assert!(commit.written.sesno > commit.previous.sesno);
}

/// 一次会话内同一条记录既有模板属性改动、又有匿名显式属性（混合窗）。
fn write_mixed(path: &Path, edits: &[(u32, &str, Vec<(&str, DescriptorValue)>, Vec<(u32, &str)>)]) {
    const ANON_TEXT_TYPE: u16 = 10240;
    let mut provider = template_provider();
    let mut engine = e3d_io::ReadOnlyEngine::open(path).unwrap();
    let mut writes = Vec::new();
    for (n, noun, attrs, anon) in edits {
        let element = engine
            .find_element(source_ref(*n))
            .unwrap()
            .unwrap_or_else(|| panic!("fixture element {n} missing"));
        let mut draft = RecordDraft::from_record_bytes(&element.raw_bytes).unwrap();
        let template = provider.template_for(db1_hash(noun)).unwrap().unwrap();
        for (name, value) in attrs {
            draft
                .set_attribute(template, db1_hash(name), value, TextFormat::Detect)
                .unwrap();
        }
        for (hash, text) in anon {
            draft
                .add_explicit_text_ascii(*hash, ANON_TEXT_TYPE, text)
                .unwrap();
        }
        writes.push(ElementWrite::Update(draft));
    }
    drop(engine);
    let commit = write_element_session(
        path,
        &SessionRequest {
            user: "orientation-test",
            comment: "isolated mixed-attribute save",
            clock: None,
        },
        &writes,
        &[],
    )
    .unwrap();
    assert!(commit.written.sesno > commit.previous.sesno);
}

fn save(path: &Path, ori: [f64; 3], xlen: Option<f64>) {
    let mut edits = vec![(
        10,
        "EQUI",
        vec![("ORI", DescriptorValue::RealArray(ori.to_vec()))],
    )];
    if let Some(xlen) = xlen {
        edits.push((11, "BOX", vec![("XLEN", DescriptorValue::Real(xlen))]));
    }
    write_updates(path, &edits);
}

fn collected(
    path: &Path,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
) -> (WindowRootSources, Vec<PersistedRoot>) {
    collected_at(path, base, target, BASE, BASE + 1)
}

fn collected_at(
    path: &Path,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    base_sesno: u32,
    target_sesno: u32,
) -> (WindowRootSources, Vec<PersistedRoot>) {
    let unit_types = resolve_delivery_unit_types(&[]);
    let index = scan_index(path, Some(base_sesno)).unwrap();
    let roots = enumerate_generation_roots(base, &index.roots, &unit_types).unwrap();
    let persisted: Vec<_> = roots.iter().map(PersistedRoot::from).collect();
    let sources = WindowRootSources::collect(
        path,
        base_sesno,
        target_sesno,
        base,
        target,
        &unit_types,
        &[],
    )
    .unwrap();
    (sources, persisted)
}
fn regenerate(
    set: &Arc<DbSet>,
    path: &Path,
    sesno: u32,
    meshes: &Path,
    root_n: u32,
) -> Vec<ProjectedGeometry> {
    let outcome = generate_subtree_with_cache(
        set.element(source_ref(root_n)),
        &mut CatalogueMeshCache::default(),
    )
    .unwrap();
    assert_eq!(
        outcome.elements.len(),
        1,
        "source generator must produce the real child BOX"
    );
    let baked = outcome
        .elements
        .into_iter()
        .map(|e| bake_element(e, meshes, None).unwrap())
        .collect();
    let index = scan_index(path, Some(sesno)).unwrap();
    project_geometry_delta(SOURCE_DB, sesno, baked, vec![], None, index.owners.as_ref())
        .unwrap()
        .0
        .upserts
}

#[test]
fn synthetic_orientation_source_real_binary_diff_reaches_parent_and_matches_generated_t() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0., 0., 90.], None);
    // Both readers are opened AFTER the save and select historical roots from
    // ONE file's real two-session chain: not two manually paired snapshots.
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    assert_eq!(base.pinned_sesno(SOURCE_DB), Some(BASE));
    assert_eq!(target.pinned_sesno(SOURCE_DB), Some(BASE + 1));
    let changed = diff_element(&base, &target, source_ref(10)).unwrap();
    assert_eq!(changed.attributes, vec!["ORI"]);
    assert!(!changed.opaque);
    assert!(changed.is_placement_only());
    assert_eq!(
        local_placement(&base.element(source_ref(10))).unwrap().1,
        Some([0.; 3])
    );
    assert_eq!(
        local_placement(&target.element(source_ref(10))).unwrap().1,
        Some([0., 0., 90.])
    );
    assert!(
        diff_element(&base, &target, source_ref(11))
            .unwrap()
            .is_unchanged(),
        "child record is unchanged even though its world placement moves"
    );
    let (sources, persisted) = collected(&path, &base, &target);
    assert!(sources.closure.is_complete());
    let impact = sources.impact(&persisted);
    assert_eq!(
        impact.touched,
        BTreeSet::from([refno_from_e3d(source_ref(10))])
    );
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    assert!(plan.degraded.is_none());
    assert_eq!(plan.regen.len(), 1);
    assert_eq!(plan.regen[0].root, refno_from_e3d(source_ref(10)));
    let settled = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert!(
        crate::options::model_transform_cheap_path(),
        "test requires the process cheap-path switch enabled"
    );
    assert_eq!(
        rigid.transform.len(),
        1,
        "real file collection must produce the placement anchor"
    );
    let anchors = &rigid.transform[&refno_from_e3d(source_ref(10))];
    let initial = regenerate(&base, &path, BASE, dir.path(), 10);
    let old_mesh = initial[0].mesh_id.clone();
    let before_bytes = std::fs::read(dir.path().join(format!("{old_mesh}.mesh"))).unwrap();
    let moved = republish_records(initial, anchors, BASE, BASE + 1, |r, w| {
        rigid_motion_mesh_bounds(dir.path(), r, w)
    })
    .unwrap();
    let oracle = regenerate(&target, &path, BASE + 1, dir.path(), 10);
    close(&moved[0], &oracle[0]);
    assert_eq!(moved[0].mesh_id, old_mesh);
    assert_eq!(
        std::fs::read(dir.path().join(format!("{old_mesh}.mesh"))).unwrap(),
        before_bytes
    );
    let expected_world = world_matrix(&target.element(source_ref(11))).unwrap();
    for (a, b) in moved[0]
        .world_transform
        .to_cols_array()
        .into_iter()
        .zip(expected_world.to_cols_array())
    {
        assert!((a - b).abs() < 1e-9);
    }
    let calculator = rooms(dir.path());
    assert_eq!(
        calculator.calculate_for_root(&moved).unwrap(),
        calculator.calculate_for_root(&oracle).unwrap()
    );
    assert!(calculator.calculate_for_root(&moved).unwrap().is_empty());
}

#[test]
fn synthetic_orientation_source_same_value_save_is_not_a_model_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0.; 3], None);
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let before = base.element(source_ref(10)).parsed().unwrap();
    let after = target.element(source_ref(10)).parsed().unwrap();
    assert_eq!(before.noun_hash, after.noun_hash);
    assert_eq!(before.owner, after.owner);
    assert_eq!(before.members, after.members);
    assert_eq!(before.explicit_attributes, after.explicit_attributes);
    assert_eq!(before.implicit_data.len(), after.implicit_data.len());
    // Eleven header words precede attribute payload. Only the physical
    // member/explicit block addresses (words 6..=9) may relocate on this save.
    let header_diffs: Vec<_> = before
        .implicit_data
        .chunks_exact(4)
        .zip(after.implicit_data.chunks_exact(4))
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(word, (a, b))| {
            (
                word,
                u32::from_be_bytes(a.try_into().unwrap()),
                u32::from_be_bytes(b.try_into().unwrap()),
            )
        })
        .collect();
    assert!(
        !header_diffs.is_empty(),
        "the fixture must really relocate its record"
    );
    assert!(
        header_diffs
            .iter()
            .all(|(word, _, _)| (6..=9).contains(word)),
        "unexpected payload changes: {header_diffs:?}"
    );
    assert_eq!(&before.implicit_data[44..], &after.implicit_data[44..]);
    assert_eq!(
        local_placement(&base.element(source_ref(10))).unwrap(),
        local_placement(&target.element(source_ref(10))).unwrap()
    );
    let changed = diff_element(&base, &target, source_ref(10)).unwrap();
    assert!(
        changed.attributes.is_empty(),
        "no attribute value changed: {changed:?}"
    );
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    println!(
        "same-value ORI: diff={changed:?}; relocated_header_words={header_diffs:?}; \
        touched={:?}; regen={:?}",
        impact.touched,
        plan.regen
            .iter()
            .map(|r| r.root.to_pdms_str())
            .collect::<Vec<_>>()
    );
    assert!(
        changed.is_unchanged(),
        "only physical header addresses changed, not ORI or geometry: {changed:?}; {header_diffs:?}"
    );
    assert!(impact.touched.is_empty());
    assert!(plan.degraded.is_none());
    assert!(plan.regen.is_empty());
}

#[test]
fn synthetic_orientation_source_ori_plus_child_dimension_forces_root_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0., 0., 90.], Some(60.));
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    assert_eq!(
        diff_element(&base, &target, source_ref(10))
            .unwrap()
            .attributes,
        vec!["ORI"]
    );
    let child_diff = diff_element(&base, &target, source_ref(11)).unwrap();
    assert_eq!(child_diff.attributes, vec!["XLEN"]);
    assert!(!child_diff.is_placement_only());
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    assert!(plan.degraded.is_none());
    assert_eq!(plan.regen.len(), 1);
    let settled = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert!(
        rigid.transform.is_empty(),
        "same-root shape edit forbids rigid-only publication"
    );
    let rebuilt = regenerate(&target, &path, BASE + 1, dir.path(), 10);
    assert_eq!(
        rebuilt[0].world_bounds,
        [[90., 90., 95.], [110., 150., 105.]]
    );
}

// 只改方位、但旋转轴不局限于 Z：X / Y / 任意复合角 / 负角各跑一遍真实二进制链路。
// 每个角度一份独立源文件——同一文件上反复保存会把 T 越推越远，互不污染。
#[test]
fn synthetic_orientation_source_x_y_and_oblique_axes_match_regenerated_t() {
    for ori in [
        [90., 0., 0.],
        [0., 90., 0.],
        [30., 45., 60.],
        [0., 0., -45.],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = create_source(dir.path());
        save(&path, ori, None);
        let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
        let changed = diff_element(&base, &target, source_ref(10)).unwrap();
        assert_eq!(changed.attributes, vec!["ORI"], "ori={ori:?}");
        assert!(!changed.opaque);
        assert!(changed.is_placement_only(), "ori={ori:?}");
        let (sources, persisted) = collected(&path, &base, &target);
        let impact = sources.impact(&persisted);
        assert_eq!(
            impact.touched,
            BTreeSet::from([refno_from_e3d(source_ref(10))]),
            "ori={ori:?}"
        );
        let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
        assert!(plan.degraded.is_none(), "ori={ori:?}: {:?}", plan.degraded);
        assert_eq!(plan.regen.len(), 1, "ori={ori:?}");
        let settled = persisted.iter().map(|r| r.refno).collect();
        let rigid = sources.rigid_motion(&persisted, &settled, &impact);
        let anchors = rigid
            .transform
            .get(&refno_from_e3d(source_ref(10)))
            .unwrap_or_else(|| panic!("ori={ori:?} must keep the cheap path: {rigid:?}"));
        assert_eq!(anchors.len(), 1, "ori={ori:?}");
        assert_eq!(anchors[0].refno, refno_from_e3d(source_ref(10)));
        let initial = regenerate(&base, &path, BASE, dir.path(), 10);
        let mesh_path = dir.path().join(format!("{}.mesh", initial[0].mesh_id));
        let bytes = std::fs::read(&mesh_path).unwrap();
        let moved = republish_records(initial.clone(), anchors, BASE, BASE + 1, |r, w| {
            rigid_motion_mesh_bounds(dir.path(), r, w)
        })
        .unwrap();
        let oracle = regenerate(&target, &path, BASE + 1, dir.path(), 10);
        close(&moved[0], &oracle[0]);
        assert_ne!(
            moved[0].world_transform.to_cols_array(),
            initial[0].world_transform.to_cols_array(),
            "ori={ori:?} must actually rotate the child frame"
        );
        assert_eq!(
            std::fs::read(&mesh_path).unwrap(),
            bytes,
            "ori={ori:?}: rigid republish must not rewrite the mesh"
        );
        let calculator = rooms(dir.path());
        assert_eq!(
            calculator.calculate_for_root(&moved).unwrap(),
            calculator.calculate_for_root(&oracle).unwrap(),
            "ori={ori:?}: room edges must match the fully regenerated T"
        );
    }
}

// 动的是子构件自己的 ORI（生成根没动）：Δ 锚在成员身上，凭属主链归因到单元根。
#[test]
fn synthetic_orientation_source_child_box_ori_anchors_at_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    write_updates(
        &path,
        &[(
            11,
            "BOX",
            vec![("ORI", DescriptorValue::RealArray(vec![0., 0., 90.]))],
        )],
    );
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let child_diff = diff_element(&base, &target, source_ref(11)).unwrap();
    assert_eq!(child_diff.attributes, vec!["ORI"]);
    assert!(child_diff.is_placement_only());
    assert!(
        diff_element(&base, &target, source_ref(10))
            .unwrap()
            .is_unchanged(),
        "only the member moved"
    );
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    assert_eq!(
        impact.touched,
        BTreeSet::from([refno_from_e3d(source_ref(10))]),
        "the member's ORI change rolls up to its unit root"
    );
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    assert!(plan.degraded.is_none());
    assert_eq!(plan.regen.len(), 1);
    let settled = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert!(rigid.rejected.is_empty(), "{:?}", rigid.rejected);
    let anchors = rigid
        .transform
        .get(&refno_from_e3d(source_ref(10)))
        .expect("BOX under a pure-container EQUI keeps the cheap path");
    assert_eq!(anchors.len(), 1);
    assert_eq!(
        anchors[0].refno,
        refno_from_e3d(source_ref(11)),
        "Δ is anchored at the moved member, not the unit root"
    );
    let initial = regenerate(&base, &path, BASE, dir.path(), 10);
    let moved = republish_records(initial, anchors, BASE, BASE + 1, |r, w| {
        rigid_motion_mesh_bounds(dir.path(), r, w)
    })
    .unwrap();
    let oracle = regenerate(&target, &path, BASE + 1, dir.path(), 10);
    close(&moved[0], &oracle[0]);
    let calculator = rooms(dir.path());
    assert_eq!(
        calculator.calculate_for_root(&moved).unwrap(),
        calculator.calculate_for_root(&oracle).unwrap()
    );
}

// 同一写会话里两个生成根各转各的：双候选同窗，两根都走刚体前移，凑数根只推水位。
#[test]
fn synthetic_orientation_source_two_roots_rotate_in_one_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source_with(dir.path(), true);
    write_updates(
        &path,
        &[
            (
                10,
                "EQUI",
                vec![("ORI", DescriptorValue::RealArray(vec![0., 0., 90.]))],
            ),
            (
                20,
                "EQUI",
                vec![("ORI", DescriptorValue::RealArray(vec![0., 90., 0.]))],
            ),
        ],
    );
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    for n in [10u32, 20] {
        let diff = diff_element(&base, &target, source_ref(n)).unwrap();
        assert_eq!(diff.attributes, vec!["ORI"], "root {n}");
        assert!(diff.is_placement_only(), "root {n}");
    }
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    assert_eq!(
        impact.touched,
        BTreeSet::from([
            refno_from_e3d(source_ref(10)),
            refno_from_e3d(source_ref(20))
        ])
    );
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    assert!(plan.degraded.is_none(), "{:?}", plan.degraded);
    assert_eq!(plan.regen.len(), 2);
    assert_eq!(plan.advance.len(), 5, "five filler units stay untouched");
    let settled = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert_eq!(rigid.transform.len(), 2);
    assert!(rigid.rejected.is_empty(), "{:?}", rigid.rejected);
    for n in [10u32, 20] {
        let anchors = &rigid.transform[&refno_from_e3d(source_ref(n))];
        assert_eq!(anchors.len(), 1, "root {n}");
        assert_eq!(anchors[0].refno, refno_from_e3d(source_ref(n)));
        let initial = regenerate(&base, &path, BASE, dir.path(), n);
        let mesh_path = dir.path().join(format!("{}.mesh", initial[0].mesh_id));
        let bytes = std::fs::read(&mesh_path).unwrap();
        let moved = republish_records(initial, anchors, BASE, BASE + 1, |r, w| {
            rigid_motion_mesh_bounds(dir.path(), r, w)
        })
        .unwrap();
        let oracle = regenerate(&target, &path, BASE + 1, dir.path(), n);
        assert_eq!(moved.len(), 1, "root {n}");
        close(&moved[0], &oracle[0]);
        assert_eq!(
            std::fs::read(&mesh_path).unwrap(),
            bytes,
            "root {n}: rigid republish must not rewrite the mesh"
        );
    }
}

// 同一文件上连续两窗：41→42 转 90°、42→43 转回 0°。第二窗的 Δ 乘在上一窗前移过的
// 记录上，终态要与 43 的全量生成一致——转回原方位后世界矩阵回到 S 端形状，不漂移。
#[test]
fn synthetic_orientation_source_second_window_restores_orientation_without_drift() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0., 0., 90.], None);
    let (initial, moved1) = {
        let (base, mid) = (pin(&path, BASE), pin(&path, BASE + 1));
        let initial = regenerate(&base, &path, BASE, dir.path(), 10);
        let (sources, persisted) = collected(&path, &base, &mid);
        let impact = sources.impact(&persisted);
        let settled: BTreeSet<_> = persisted.iter().map(|r| r.refno).collect();
        let rigid = sources.rigid_motion(&persisted, &settled, &impact);
        let anchors = rigid.transform[&refno_from_e3d(source_ref(10))].clone();
        let moved = republish_records(initial.clone(), &anchors, BASE, BASE + 1, |r, w| {
            rigid_motion_mesh_bounds(dir.path(), r, w)
        })
        .unwrap();
        (initial, moved)
    };
    assert_eq!(moved1[0].source_sesno, BASE + 1);
    save(&path, [0.; 3], None);
    // 重新 pin 中间会话：文件链现在两个会话深，历史 pin 仍须走通。
    let (base2, target2) = (pin(&path, BASE + 1), pin(&path, BASE + 2));
    assert_eq!(base2.pinned_sesno(SOURCE_DB), Some(BASE + 1));
    assert_eq!(target2.pinned_sesno(SOURCE_DB), Some(BASE + 2));
    let changed2 = diff_element(&base2, &target2, source_ref(10)).unwrap();
    assert_eq!(changed2.attributes, vec!["ORI"]);
    assert!(changed2.is_placement_only());
    let (sources2, persisted2) = collected_at(&path, &base2, &target2, BASE + 1, BASE + 2);
    let impact2 = sources2.impact(&persisted2);
    let plan2 = plan_window_roots(&sources2.roots_t, &persisted2, &impact2);
    assert!(plan2.degraded.is_none());
    let settled2 = persisted2.iter().map(|r| r.refno).collect();
    let rigid2 = sources2.rigid_motion(&persisted2, &settled2, &impact2);
    let anchors2 = rigid2.transform[&refno_from_e3d(source_ref(10))].clone();
    let moved2 = republish_records(moved1, &anchors2, BASE + 1, BASE + 2, |r, w| {
        rigid_motion_mesh_bounds(dir.path(), r, w)
    })
    .unwrap();
    let oracle = regenerate(&target2, &path, BASE + 2, dir.path(), 10);
    close(&moved2[0], &oracle[0]);
    for (a, b) in moved2[0]
        .world_transform
        .to_cols_array()
        .into_iter()
        .zip(initial[0].world_transform.to_cols_array())
    {
        assert!(
            (a - b).abs() < 1e-9,
            "rotating back must not drift: {a} vs {b}"
        );
    }
    assert_eq!(moved2[0].source_sesno, BASE + 2);
}

// 真实 SAVEWORK 只动 ORI 时也会顺带跳 `CACHID` 戳（3.1 每次图形重建 +1）。戳不是摆放输入，
// 不能把「纯方位修改」误判成混合窗、把便宜路整根踢回 regen。`SPAMAP` 同族，但本夹具的
// EQUI 模板不声明它，改由 `element_diff` 单测覆盖。
#[test]
fn synthetic_orientation_source_ori_with_placement_stamps_keeps_the_cheap_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    write_updates(
        &path,
        &[(
            10,
            "EQUI",
            vec![
                ("ORI", DescriptorValue::RealArray(vec![0., 0., 90.])),
                ("CACHID", DescriptorValue::Word { raw: 1, text: None }),
            ],
        )],
    );
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let diff = diff_element(&base, &target, source_ref(10)).unwrap();
    assert_eq!(diff.attributes, vec!["CACHID", "ORI"], "{diff:?}");
    assert!(diff.is_placement_only(), "{diff:?}");
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    let settled: BTreeSet<_> = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert_eq!(
        rigid.transform.len(),
        1,
        "ORI + placement stamps is still a pure placement window: {rigid:?}"
    );
    let anchors = &rigid.transform[&refno_from_e3d(source_ref(10))];
    let initial = regenerate(&base, &path, BASE, dir.path(), 10);
    let moved = republish_records(initial, anchors, BASE, BASE + 1, |r, w| {
        rigid_motion_mesh_bounds(dir.path(), r, w)
    })
    .unwrap();
    let oracle = regenerate(&target, &path, BASE + 1, dir.path(), 10);
    close(&moved[0], &oracle[0]);
}

// 方向修改走**生产执行体**（不是只调纯函数 `republish_records`）：S 端整根投影当基线，
// 执行体自带的重放闸 / pin@T 目标核对 / 网格盒回调 / 回执盖章全跑一遍；随后同窗再跑一次
// 必须按「回执已在 T」重放，记录一个字节都不再动。
#[test]
fn synthetic_orientation_source_production_executor_republishes_then_replays() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0., 0., 90.], None);
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    let settled: BTreeSet<_> = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    let root_e3d = source_ref(10);
    let root = refno_from_e3d(root_e3d);
    let anchors = rigid
        .transform
        .get(&root)
        .expect("pure ORI window must classify the unit root as a transform")
        .clone();

    let service = E3dModelService::from_source_files(
        vec![crate::data_interface::direct_store::DbPin {
            dbnum: SOURCE_DB as i32,
            db_type: "DESI".into(),
            file: path.clone(),
            sesno: None,
        }],
        crate::data_interface::cata_closure::InMemoryCataLocator::from_parts(
            HashMap::new(),
            HashMap::new(),
        ),
        dir.path().to_path_buf(),
    )
    .unwrap();

    // S 端整根重算 → 本地投影，回执停在 S、落款是 S 的目标（与 batch_worker 的基线同形）。
    let initial = regenerate(&base, &path, BASE, dir.path(), 10);
    let (manifest, geometry_count) = manifest_hash(&initial).unwrap();
    let target_at_s = service
        .model_target(SOURCE_DB, BASE, &service.dependency_sessions)
        .unwrap();
    let store = ModelMemoryStore::default();
    store.commit(
        ProjectionScope::Current,
        vec![ProjectionCommit {
            upserts: initial.clone(),
            receipt: Some((
                root_e3d,
                RootReceipt {
                    source_sesno: BASE,
                    model_target: Some(target_at_s),
                    manifest_hash: manifest,
                    geometry_count,
                    published_via: "regen".into(),
                    room: None,
                },
            )),
            ..ProjectionCommit::default()
        }],
    );
    assert!(store.root_projection_complete(ProjectionScope::Current, root_e3d, dir.path()));

    let units = vec![RigidMotionUnit {
        root_refno: root.to_pdms_str(),
        source_start_sesno: BASE,
        anchors,
    }];
    let report = service
        .republish_roots_by_rigid_motion_on(&store, SOURCE_DB, BASE + 1, &units, |commits| {
            store.try_commit_current(commits);
            Ok(())
        })
        .unwrap();
    assert_eq!(report.completed, vec![root.to_pdms_str()], "{report:?}");
    assert!(report.fallbacks.is_empty(), "{report:?}");

    let moved = store.records_of_root(ProjectionScope::Current, root_e3d);
    let oracle = regenerate(&target, &path, BASE + 1, dir.path(), 10);
    assert_eq!(moved.len(), oracle.len());
    close(&moved[0], &oracle[0]);
    assert_eq!(moved[0].source_sesno, BASE + 1);
    let receipt = store
        .receipt(ProjectionScope::Current, root_e3d)
        .expect("executor stamps a receipt at T");
    assert_eq!(receipt.source_sesno, BASE + 1);
    assert_eq!(receipt.published_via, "transform");
    assert!(receipt.model_target.is_some());
    let settled_world = moved[0].world_transform.to_cols_array();

    // 窗口重放：回执已在 T，重放闸必须跳过而不是再乘一次 Δ。
    let replay = service
        .republish_roots_by_rigid_motion_on(&store, SOURCE_DB, BASE + 1, &units, |commits| {
            store.try_commit_current(commits);
            Ok(())
        })
        .unwrap();
    assert_eq!(replay.replayed, vec![root.to_pdms_str()], "{replay:?}");
    assert!(replay.completed.is_empty(), "{replay:?}");
    assert_eq!(
        store.records_of_root(ProjectionScope::Current, root_e3d)[0]
            .world_transform
            .to_cols_array(),
        settled_world,
        "replay must not apply Δ twice"
    );
}

// 窗口计划缓存按「文件」分域：同号跨项目副本（MDB 路由目录允许）的同一会话窗是两份
// 不同的输入。A 规划完，B 的同号同窗键上必须查不到 A；B 也走生产规划器，两份计划各归各的
// 键；执行体命中 B 的键时读到的是 B 自己的计划与 B 自己的输入。
// 「两份一定同时还在」由 `WindowPlanCache` 的本地单测确定性覆盖：进程级缓存有容量上界，
// 又与并行测试共用，在这里断言某一格一定没被容量淘汰会把别的测试的插入误读成本测试的失败。
#[test]
fn window_plan_cache_is_scoped_by_source_file_and_keeps_both_projects() {
    let dir = tempfile::tempdir().unwrap();
    let dir_a = dir.path().join("a");
    let dir_b = dir.path().join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let path_a = create_source(&dir_a);
    let path_b = create_source(&dir_b);
    save(&path_a, [0., 0., 90.], None);
    // B 也保存一次，但 ORI 原值不变：会话链同样有 41→42，模型计划应为空。
    save(&path_b, [0.; 3], None);

    let (base_a, target_a) = (pin(&path_a, BASE), pin(&path_a, BASE + 1));
    let _ = collected(&path_a, &base_a, &target_a);
    let plan_a = cached_window_plan_sets(SOURCE_DB, &path_a, BASE, BASE + 1)
        .expect("collect of A must fill its own cache key");
    assert!(!plan_a.regenerate.is_empty(), "A really rotates its root");
    assert!(
        cached_window_plan_sets(SOURCE_DB, &path_b, BASE, BASE + 1).is_none(),
        "same dbnum/session window on another file must not alias A's plan"
    );

    // B 也走生产规划器：B 的载荷落在 B 自己的键上，与 A 的计划互不覆盖。
    let (base_b, target_b) = (pin(&path_b, BASE), pin(&path_b, BASE + 1));
    collected(&path_b, &base_b, &target_b);
    let plan_b = cached_window_plan_sets(SOURCE_DB, &path_b, BASE, BASE + 1)
        .expect("collect of B must fill B's own cache key");
    assert!(
        plan_b.regenerate.is_empty(),
        "B saved its ORI unchanged: its window names no unit to rebuild"
    );
    assert_eq!(
        plan_b.key.file(),
        path_b,
        "B's cached plan is keyed by B's own file"
    );
    assert_eq!(
        plan_a.key.file(),
        path_a,
        "A's cached plan still names A's file"
    );

    let service = E3dModelService::from_source_files(
        vec![crate::data_interface::direct_store::DbPin {
            dbnum: SOURCE_DB as i32,
            db_type: "DESI".into(),
            file: path_b.clone(),
            sesno: None,
        }],
        crate::data_interface::cata_closure::InMemoryCataLocator::from_parts(
            HashMap::new(),
            HashMap::new(),
        ),
        dir.path().to_path_buf(),
    )
    .unwrap();
    let producer = UnitDeltaProducer::open(&service, SOURCE_DB, BASE, BASE + 1).unwrap();
    assert!(
        producer.plan.regenerate.is_empty(),
        "the producer must reuse B's own plan, not A's rotated one"
    );
    let (_, ori) = local_placement(&producer.target.set.element(source_ref(10))).unwrap();
    assert_eq!(
        ori,
        Some([0.; 3]),
        "the producer must read B's target, not A's rotated one"
    );
    assert_eq!(
        producer.plan.key.file(),
        path_b,
        "the hit is B's own cache entry, keyed by B's file"
    );
}

// 规划器与执行体是两个 service：缓存里的两端集合只能配打开它们的那张依赖会话表用——集合懒开目录库时
// 按那张表钉会话、往那张表里记，回执的 `ModelTarget` 也从它算。没绑表的输入不复用；绑表只认这两个
// 集合本身。用本地缓存实例：全局缓存与并行测试共用，断言某一格还在会把别的测试的插入误读成失败。
#[test]
fn cached_window_inputs_are_reused_only_with_the_planner_dependency_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    save(&path, [0., 0., 90.], None);
    let service = || {
        E3dModelService::from_source_files(
            vec![crate::data_interface::direct_store::DbPin {
                dbnum: SOURCE_DB as i32,
                db_type: "DESI".into(),
                file: path.clone(),
                sesno: None,
            }],
            crate::data_interface::cata_closure::InMemoryCataLocator::from_parts(
                HashMap::new(),
                HashMap::new(),
            ),
            dir.path().to_path_buf(),
        )
        .unwrap()
    };
    let (planner, executor) = (service(), service());
    let base = planner.build_set(SOURCE_DB, Some(BASE)).unwrap();
    let target = planner.build_set(SOURCE_DB, Some(BASE + 1)).unwrap();
    let inputs = CachedWindowInputs::planned(
        base.clone(),
        Arc::new(scan_index(&path, Some(BASE)).unwrap()),
        target.clone(),
        Arc::new(scan_index(&path, Some(BASE + 1)).unwrap()),
    );
    assert!(
        inputs.generation_inputs().is_none(),
        "planner inputs without their dependency table must not be reused"
    );

    let key = WindowPlanKey::new(SOURCE_DB, &path, BASE, BASE + 1);
    let mut cache = WindowPlanCache::new();
    assert!(
        cache
            .insert(Arc::new(WindowPlanSets {
                key: key.clone(),
                regenerate: BTreeSet::new(),
                remove: BTreeSet::new(),
                regenerate_derived: BTreeSet::new(),
                derived_stale: BTreeSet::new(),
                full_rebuild: None,
                unreadable: 0,
                dispositions: Vec::new(),
                inputs: Some(inputs),
            }))
            .is_empty()
    );

    let other_target = executor.build_set(SOURCE_DB, Some(BASE + 1)).unwrap();
    assert!(
        cache
            .bind_dependencies(&key, &base, &other_target, &planner.dependency_sessions)
            .is_none(),
        "a table binds only to the very sets its service opened"
    );
    assert!(
        cache
            .get(&key)
            .unwrap()
            .inputs
            .as_ref()
            .unwrap()
            .generation_inputs()
            .is_none()
    );

    assert!(
        cache
            .bind_dependencies(&key, &base, &target, &planner.dependency_sessions)
            .is_some()
    );
    let bound = cache.get(&key).unwrap();
    let (reused_base, reused_target) = bound
        .inputs
        .as_ref()
        .unwrap()
        .generation_inputs()
        .expect("bound planner inputs are reusable");
    assert!(Arc::ptr_eq(&reused_target.set, &target));
    for reused in [&reused_base, &reused_target] {
        assert!(
            Arc::ptr_eq(&reused.dependency_sessions, &planner.dependency_sessions),
            "the producer must read the table the planner's sets record into"
        );
        assert!(!Arc::ptr_eq(
            &reused.dependency_sessions,
            &executor.dependency_sessions
        ));
    }
}

// ---------- 真实库取证（环境变量门控，默认跳过） ----------
//
// `AIOS_CENSUS_DB_FILE` 指向一个真实 DESI 库文件（如 ams8000_0001）时，沿会话链逐
// 相邻窗口跑生产 `collect_window` → `diff_element`，统计「字节变了但判 unchanged」的
// 搬迁吸收量与剩余 opaque，并对每条 opaque 校验差异落在 24..40 之外（反证：若一条
// opaque 的隐式差异只在 24..40 内，归一化就没生效）。只读文件，不写、不起服务。
// 可选：`AIOS_CENSUS_WINDOWS=N` 只查最近 N 窗；`AIOS_CENSUS_BASE`/`AIOS_CENSUS_TARGET`
// 指定单窗。
#[test]
fn real_db_relocation_census_only_when_env_points_at_a_desi_file() {
    let Some(path) = std::env::var_os("AIOS_CENSUS_DB_FILE").map(PathBuf::from) else {
        eprintln!("AIOS_CENSUS_DB_FILE unset; real-db census skipped");
        return;
    };
    assert!(path.is_file(), "census db missing: {}", path.display());
    let windows: Vec<(u32, u32)> = match (
        std::env::var("AIOS_CENSUS_BASE"),
        std::env::var("AIOS_CENSUS_TARGET"),
    ) {
        (Ok(b), Ok(t)) => vec![(b.parse().unwrap(), t.parse().unwrap())],
        _ => {
            let mut view = e3d_io::session::DbView::open(&path).expect("open census db");
            let sessions = view.walk_sessions().expect("walk session chain");
            assert!(sessions.len() >= 2, "db has <2 sessions");
            let mut pairs: Vec<(u32, u32)> = sessions
                .windows(2)
                .map(|w| (w[1].sesno, w[0].sesno))
                .collect();
            if let Some(n) = std::env::var("AIOS_CENSUS_WINDOWS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
            {
                pairs.truncate(n);
            }
            pairs
        }
    };
    let (mut candidates, mut unchanged, mut absorbed, mut opaque) =
        (0usize, 0usize, 0usize, 0usize);
    let (mut attr_changed, mut structural, mut errors) = (0usize, 0usize, 0usize);
    for (base_sesno, target_sesno) in &windows {
        let (base, target) = (pin(&path, *base_sesno), pin(&path, *target_sesno));
        let window = e3d_model::increment::collect_window(&path, *base_sesno, *target_sesno)
            .unwrap_or_else(|e| panic!("collect_window {base_sesno}->{target_sesno}: {e}"));
        let mut w = (0usize, 0usize, 0usize, 0usize);
        for change in &window.diff.changes {
            let e3d_io::index::IndexCandidate::Modified { refno, .. } = change else {
                continue;
            };
            candidates += 1;
            w.0 += 1;
            let diff = match diff_element(&base, &target, *refno) {
                Ok(d) => d,
                Err(e) => {
                    errors += 1;
                    eprintln!("  diff_element {refno} failed: {e}");
                    continue;
                }
            };
            let (before, after) = (
                base.element(*refno).parsed().unwrap(),
                target.element(*refno).parsed().unwrap(),
            );
            let raw_identical = before.implicit_data == after.implicit_data
                && before.explicit_attributes == after.explicit_attributes
                && before.members == after.members
                && before.owner == after.owner
                && before.noun_hash == after.noun_hash;
            assert!(
                !raw_identical || diff.is_unchanged(),
                "{base_sesno}->{target_sesno} {refno}: bytes identical but diff reports a change: {diff:?}"
            );
            if diff.is_unchanged() {
                unchanged += 1;
                w.1 += 1;
                if !raw_identical {
                    absorbed += 1;
                    w.2 += 1;
                }
            } else if diff.opaque {
                opaque += 1;
                w.3 += 1;
                // 反证哨兵：opaque 必须确有 24..40 之外（或非隐式区）的证据。
                let diffs: Vec<usize> = before
                    .implicit_data
                    .iter()
                    .zip(after.implicit_data.iter())
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .map(|(i, _)| i)
                    .collect();
                assert!(
                    before.implicit_data.len() != after.implicit_data.len()
                        || diffs.iter().any(|i| !(24..40).contains(i))
                        || before.explicit_attributes != after.explicit_attributes
                        || before.members != after.members
                        || before.owner != after.owner
                        || before.noun_hash != after.noun_hash,
                    "{base_sesno}->{target_sesno} {refno}: opaque with diffs only in 24..40 — normalization failed"
                );
            } else {
                if diff.type_changed.is_some()
                    || diff.owner_changed.is_some()
                    || diff.members.is_some()
                {
                    structural += 1;
                }
                if !diff.attributes.is_empty() {
                    attr_changed += 1;
                }
            }
        }
        if w.0 > 0 {
            println!(
                "window {base_sesno}->{target_sesno}: modified={} unchanged={} absorbed_relocation={} opaque={}",
                w.0, w.1, w.2, w.3
            );
        }
    }
    println!(
        "census totals over {} windows: modified={} unchanged={} absorbed_relocation={} opaque={} attr_changed={} structural={} errors={}",
        windows.len(),
        candidates,
        unchanged,
        absorbed,
        opaque,
        attr_changed,
        structural,
        errors
    );
    assert_eq!(errors, 0, "diff_element errors on real candidates");
}

/// 匿名显式属性（模板没声明、字典答不出的 hash）经真实写会话落库后，
/// `diff_element` 按 `#%08X` 伪名点名——不再落 opaque，也不再保守级联：
/// 纯属性存储变化挪不动世界矩阵。与 ams8000 那 37 条同一形状。
#[test]
fn synthetic_anonymous_explicit_attribute_reports_hash_pseudo_name() {
    const ANON_HASH: u32 = 0x2902_D6E2;
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    write_anonymous_text(&path, &[(10, ANON_HASH, "JS")]);
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let diff = diff_element(&base, &target, source_ref(10)).unwrap();
    assert_eq!(diff.attributes, vec!["#2902D6E2"]);
    assert!(!diff.opaque);
    assert!(!diff.is_placement_only());
    assert!(
        !diff.placement_input_changed(),
        "纯属性存储变化挪不动世界矩阵，不该级联"
    );
    // 改值同样点名；删掉也点名——before 有 after 无也是变化。
    write_anonymous_text(&path, &[(10, ANON_HASH, "Kangwp")]);
    let (next_base, next_target) = (pin(&path, BASE + 1), pin(&path, BASE + 2));
    let diff = diff_element(&next_base, &next_target, source_ref(10)).unwrap();
    assert_eq!(diff.attributes, vec!["#2902D6E2"]);
    remove_anonymous(&path, &[(10, ANON_HASH)]);
    let (third_base, third_target) = (pin(&path, BASE + 2), pin(&path, BASE + 3));
    let diff = diff_element(&third_base, &third_target, source_ref(10)).unwrap();
    assert_eq!(diff.attributes, vec!["#2902D6E2"]);
    assert!(!diff.opaque);
}

/// 混合窗（ORI + 匿名属性同记录同窗）：`is_placement_only` 为假 → 根必须走
/// 重生而非便宜刚体路（匿名属性可能是模型输入，保守是设计判决）；但
/// `placement_input_changed` 仍真，级联维度不丢。随后一窗只动 ORI 而匿名
/// 属性原样带着——值没变就不上报，纯摆放判决不被污染。
#[test]
fn synthetic_anonymous_attribute_alongside_ori_blocks_cheap_path() {
    const ANON_HASH: u32 = 0x2902_D6E2;
    let dir = tempfile::tempdir().unwrap();
    let path = create_source(dir.path());
    write_mixed(
        &path,
        &[(
            10,
            "EQUI",
            vec![("ORI", DescriptorValue::RealArray(vec![0., 0., 90.]))],
            vec![(ANON_HASH, "JS")],
        )],
    );
    let (base, target) = (pin(&path, BASE), pin(&path, BASE + 1));
    let diff = diff_element(&base, &target, source_ref(10)).unwrap();
    assert_eq!(diff.attributes, vec!["#2902D6E2", "ORI"]);
    assert!(!diff.is_placement_only());
    assert!(diff.placement_input_changed());
    let (sources, persisted) = collected(&path, &base, &target);
    let impact = sources.impact(&persisted);
    let plan = plan_window_roots(&sources.roots_t, &persisted, &impact);
    assert_eq!(plan.regen.len(), 1);
    assert_eq!(plan.regen[0].root, refno_from_e3d(source_ref(10)));
    let settled = persisted.iter().map(|r| r.refno).collect();
    let rigid = sources.rigid_motion(&persisted, &settled, &impact);
    assert!(
        rigid.transform.is_empty(),
        "ORI+匿名属性是混合窗：未知属性可能是模型输入，必须重生不走刚体"
    );
    // 第二窗：匿名属性原样带着 + 只动 ORI → 仍纯摆放，便宜路照常放行。
    save(&path, [0., 0., 45.], None);
    let (base2, target2) = (pin(&path, BASE + 1), pin(&path, BASE + 2));
    let diff2 = diff_element(&base2, &target2, source_ref(10)).unwrap();
    assert_eq!(diff2.attributes, vec!["ORI"]);
    assert!(diff2.is_placement_only());
    let (sources2, persisted2) = collected_at(&path, &base2, &target2, BASE + 1, BASE + 2);
    let impact2 = sources2.impact(&persisted2);
    let settled2 = persisted2.iter().map(|r| r.refno).collect();
    let rigid2 = sources2.rigid_motion(&persisted2, &settled2, &impact2);
    assert_eq!(
        rigid2.transform.len(),
        1,
        "值没变的匿名属性不上报，纯摆放窗照常走刚体"
    );
}
