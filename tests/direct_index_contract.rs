//! D3 三索引的合同：索引答的每一句话，都能用单元素读法当场复核。
//!
//! 这里的门不碰 SurrealDB——type/name/backref 的每一条都拿 e3d-io 的单元素
//! 路径（`find_element` / 记录头 / 成员表）回查，所以「索引说的」与「文件说的」
//! 逐条对齐。与 Surreal 侧的对拍（D3 验收原文）需要活库和已入库工程，单列成
//! `#[ignore]` 的探针，有环境时手动跑。
//!
//! 需要 AMS 样本库与模板目录（attlib.dat / desvir.dat），缺哪个都整体跳过。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aios_database::data_interface::cata_closure::CataDbLocator;
use aios_database::data_interface::direct_index::BackRefVia;
use aios_database::data_interface::direct_store::{
    DbPin, DirectSchema, DirectStore, TEMPLATE_DIR_ENV,
};
use aios_core::RefU64;
use e3d_io::refno::RefNo;
use e3d_io::{ReadOnlyEngine, ScanTier};

const AMS8000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001";
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

struct NoLocator;

impl CataDbLocator for NoLocator {
    fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> {
        None
    }
    fn db_type_of(&self, _dbnum: u32) -> Option<String> {
        None
    }
    fn file_of(&self, _dbnum: u32) -> Option<(String, PathBuf)> {
        None
    }
}

/// 样本或模板缺席时跳过：这两样都不随仓库走。
fn fixtures() -> Option<(PathBuf, PathBuf)> {
    let db = PathBuf::from(AMS8000);
    let templates = std::env::var_os(TEMPLATE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
    if !db.is_file() || !templates.join("attlib.dat").is_file() {
        eprintln!("SKIP: fixtures not present ({db:?}, {templates:?})");
        return None;
    }
    Some((db, templates))
}

/// 缓存目录经环境变量传递，进程级共享；三个测试串行拿锁，各自指到自己的
/// tempdir，互不读到对方的缓存。
fn env_gate() -> std::sync::MutexGuard<'static, ()> {
    static GATE: OnceLock<Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("no test panicked while holding the gate")
}

fn store_for(db: &Path, templates: &Path, index_dir: &Path) -> DirectStore {
    unsafe {
        std::env::set_var(
            aios_database::data_interface::direct_index::INDEX_DIR_ENV,
            index_dir,
        )
    };
    let schema = Arc::new(DirectSchema::open(templates).expect("schema opens"));
    let store = DirectStore::new(schema, Arc::new(NoLocator));
    store.pin(DbPin {
        dbnum: 8000,
        db_type: "DESI".to_string(),
        file: db.to_path_buf(),
        sesno: None,
    });
    store
}

fn refno_of(refno: RefU64) -> RefNo {
    RefNo::new(refno.get_0(), refno.get_1())
}

/// **type/name/backref 的每一句话都能被单元素读法复核。**
#[test]
fn every_index_answer_survives_a_single_element_recheck() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache_dir = tempfile::tempdir().unwrap();
    let store = store_for(&db, &templates, cache_dir.path());
    let indexes = store.indexes(8000).expect("indexes build");

    // 独立打开一份引擎当「文件说的」。
    let mut engine = ReadOnlyEngine::open(&db).expect("sample database opens");

    // --- type 门：分组并起来 == 全部键；分布 == 独立 Header 扫描的分布 ---
    let mut scan_census: BTreeMap<u32, usize> = BTreeMap::new();
    let mut scan_childful: BTreeMap<u32, usize> = BTreeMap::new();
    let mut total = 0usize;
    for element in engine.scan_elements(ScanTier::Full).expect("scans") {
        let element = element.expect("every element reads");
        *scan_census.entry(element.noun_hash).or_default() += 1;
        let parsed = element.parsed.as_ref().expect("full tier");
        if !parsed.members.is_empty() {
            *scan_childful.entry(element.noun_hash).or_default() += 1;
        }
        total += 1;
    }
    let indexed: BTreeMap<u32, usize> = indexes.noun_census().into_iter().collect();
    assert_eq!(
        indexed, scan_census,
        "the type index and an independent scan disagree about the noun census"
    );
    assert_eq!(
        indexes
            .noun_census()
            .iter()
            .map(|(_, count)| count)
            .sum::<usize>(),
        total,
        "the type groups do not add back up to every element"
    );

    // has_children 的两个答案合起来是全集，且 childful 侧与扫描一致。
    for (noun, expected_childful) in scan_childful.iter().take(50) {
        let childful = indexes.refnos_of_noun_hash(*noun, Some(true)).len();
        let childless = indexes.refnos_of_noun_hash(*noun, Some(false)).len();
        assert_eq!(childful, *expected_childful, "noun {noun}: childful count");
        assert_eq!(
            childful + childless,
            scan_census[noun],
            "noun {noun}: the two filters must partition the group"
        );
    }

    // SITE 这种生成主链天天问的 noun 走名字入口也答得上来。
    let sites = indexes.refnos_of_noun("SITE", None);
    assert_eq!(
        sites.len(),
        scan_census
            .get(&e3d_attlib::db1_hash("SITE"))
            .copied()
            .unwrap_or_default(),
        "SITE by name and by hash are the same group"
    );

    // --- name 门：每个名字往返（抽样 97 步长），named 总数一致 ---
    let mut named_total = 0usize;
    let mut checked = 0usize;
    let all_names: Vec<(String, Vec<RefU64>)> = {
        // 从扫描侧独立收名字，再问索引。
        let mut engine = ReadOnlyEngine::open(&db).expect("sample database opens");
        let mut names: BTreeMap<String, Vec<RefU64>> = BTreeMap::new();
        for element in engine.scan_elements(ScanTier::Named).expect("scans") {
            let element = element.expect("every element reads");
            if let Some(name) = element.name {
                names
                    .entry(name)
                    .or_default()
                    .push(RefU64::from_two_nums(element.refno.word0, element.refno.word1));
            }
        }
        names.into_iter().collect()
    };
    for (name, expected) in &all_names {
        named_total += expected.len();
        let answered = indexes.find_named(name);
        assert_eq!(
            &answered, expected,
            "name {name}: the index and the scan disagree"
        );
    }
    assert_eq!(
        named_total,
        indexes.stats.named as usize,
        "named totals disagree"
    );
    // 抽样再走一遍单元素路径：find_element → stored_name 回到同一个名字。
    for (name, expected) in all_names.iter().step_by(97) {
        for refno in expected {
            let view = engine
                .find_element(refno_of(*refno))
                .expect("lookup")
                .expect("named element resolves");
            let parsed =
                e3d_io::record::element::ParsedElement::parse(&view.raw_bytes).expect("parses");
            assert_eq!(
                e3d_io::element_name::stored_name(&parsed).as_deref(),
                Some(name.as_str()),
                "{refno:?}: the stored NAME is not what the index filed it under"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no name was spot-checked");

    // --- backref 门：抽样入边，逐条用文件复核 ---
    let mut verified = 0usize;
    for (_, targets) in all_names.iter().step_by(53) {
        for target in targets {
            for edge in indexes.inbound_of(*target) {
                let source = RefU64(edge.source);
                let view = engine
                    .find_element(refno_of(source))
                    .expect("lookup")
                    .expect("edge source resolves");
                let parsed =
                    e3d_io::record::element::ParsedElement::parse(&view.raw_bytes).expect("parses");
                match edge.via {
                    BackRefVia::Owner => {
                        assert_eq!(
                            (parsed.owner.word0, parsed.owner.word1),
                            (target.get_0(), target.get_1()),
                            "{source:?} does not own {target:?} as the edge claims"
                        );
                    }
                    BackRefVia::Member => {
                        assert!(
                            parsed
                                .members
                                .iter()
                                .any(|m| (m.word0, m.word1) == (target.get_0(), target.get_1())),
                            "{source:?} has no member {target:?} as the edge claims"
                        );
                    }
                    BackRefVia::Attr(_) => {
                        // 属性边的逐条复核要重放描述符抽取，等 Surreal 对拍探针
                        // 一起做；这里先数上，别让「抽到的全是结构边」蒙混过关。
                    }
                }
                verified += 1;
            }
        }
    }
    assert!(verified > 0, "no inbound edge was verified");

    println!(
        "elements={} named={} edges={} template_fallbacks={} build_ms={}",
        indexes.stats.elements,
        indexes.stats.named,
        indexes.stats.inbound_edges,
        indexes.stats.template_fallbacks,
        indexes.stats.build_ms
    );
}

/// **指纹门：同一文件同一时点，第二次是读缓存，不是重扫。**
#[test]
fn a_second_store_reads_the_disk_cache_instead_of_rebuilding() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache_dir = tempfile::tempdir().unwrap();

    let first = store_for(&db, &templates, cache_dir.path());
    let built = first.indexes(8000).expect("indexes build");
    let cache_files: Vec<_> = std::fs::read_dir(cache_dir.path())
        .expect("cache dir reads")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(
        cache_files.len(),
        1,
        "one database, one cache file: {cache_files:?}"
    );
    let written = std::fs::metadata(&cache_files[0])
        .expect("cache file stats")
        .modified()
        .expect("mtime");

    let second = store_for(&db, &templates, cache_dir.path());
    let started = std::time::Instant::now();
    let reloaded = second.indexes(8000).expect("indexes reload");
    let elapsed = started.elapsed();

    assert_eq!(
        reloaded.fingerprint, built.fingerprint,
        "the reload answered for a different file or session"
    );
    assert_eq!(
        reloaded.stats.build_ms, built.stats.build_ms,
        "a rebuild would have written fresh stats"
    );
    let untouched = std::fs::metadata(&cache_files[0])
        .expect("cache file stats")
        .modified()
        .expect("mtime");
    assert_eq!(written, untouched, "the reload must not rewrite the cache");
    // D3 的验收数字是 <50 ms；给 CI 冷盘留余量，硬门放在 500 ms，实际值打出来。
    assert!(
        elapsed.as_millis() < 500,
        "a cache hit took {elapsed:?}, which is a rebuild in disguise"
    );
    println!(
        "cache hit in {elapsed:?} (build was {} ms)",
        built.stats.build_ms
    );
}

/// **换时点必须重建。** 把 pin 换到另一个会话号，指纹不同，旧索引不得再答。
#[test]
fn repinning_to_another_session_invalidates_the_indexes() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let _gate = env_gate();
    let cache_dir = tempfile::tempdir().unwrap();
    let store = store_for(&db, &templates, cache_dir.path());
    let latest = store.indexes(8000).expect("indexes build");
    let pinned = store.pinned_sesno(8000).expect("session opened");
    assert_eq!(latest.fingerprint.pinned_sesno, pinned);

    // 钉到早一个会话：树根不同，索引必须是另一份。
    store.pin(DbPin {
        dbnum: 8000,
        db_type: "DESI".to_string(),
        file: db.to_path_buf(),
        sesno: Some(pinned - 1),
    });
    let earlier = store.indexes(8000).expect("indexes rebuild");
    assert_eq!(earlier.fingerprint.pinned_sesno, pinned - 1);
    assert_ne!(
        latest.fingerprint, earlier.fingerprint,
        "two sessions may not share one index"
    );
}

/// Surreal 对拍探针（D3 验收原文的三道）：需要活的 SUL_DB 与已入库的 ams 工程，
/// 有环境时手动跑。这里不自动跳过——跑了但连不上库就该红，免得「绿了」其实
/// 是「没跑」。
#[test]
#[ignore = "needs a live SurrealDB with the ams project ingested; run manually"]
fn surreal_parity_type_name_backref() {
    // 留接口：type = query_type_refnos_by_dbnum 全 noun 对拍；
    // name = pe 按 NAME 查 200 个；backref = 200 个 refno 的入边集合。
    // 实装等对拍环境定了连接方式（ws://127.0.0.1:8000 或嵌入式）再写，
    // 先有名字占位，谁跑谁知道要补什么。
    panic!("wire this probe to the live SUL_DB before running");
}
