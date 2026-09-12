//! 源码护栏：跨库属性只经 RefNo 位解码路由（切片 `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §5.2）。
//!
//! 文本断言扫的是源码，不跑任何库：它们守的是「不得回到全局 ref0 反查表 / 逐库试查」这条线，
//! 一旦有人把老路接回去，这里先红。每条都指向母计划 `2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`
//! 的一条不变量。末尾一条是**行为**护栏（§9 Q14 定的两者都要）：拿合成库文件配 `e3d_io::read_stats`
//! 计数器，验定一个根只开目标库一次、零扫描。计数器是进程全局的——本二进制里只有它一条测试碰库，
//! 差值才稳；再加碰库的测试请并进它，别另开一条。

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use aios_database::data_interface::direct_store::DbPin;
use aios_database::data_interface::model_source::{RootDbError, dbnum_of_root_in};
use e3d_io::{RefNo, read_stats};

const DIRECT_STORE: &str = include_str!("../src/data_interface/direct_store.rs");
const MODEL_SOURCE: &str = include_str!("../src/data_interface/model_source.rs");
const ELEMENT_ATTRIBUTES: &str = include_str!("../src/data_interface/element_attributes.rs");
const DIRECT_TREE: &str = include_str!("../src/data_interface/direct_tree.rs");
const REFNO_ROUTE: &str = include_str!("../src/data_interface/refno_route.rs");
const ROUTE_DIRECTORY: &str = include_str!("../src/data_interface/mdb_route_directory.rs");

/// 去掉 `#[cfg(test)]` 之后的测试模块，只留生产代码；换行统一成 `\n`，Windows 检出的
/// CRLF 副本与仓内 LF 原件在这里长得一样。
fn production(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .replace("\r\n", "\n")
}

/// 从签名到与它同缩进的闭括号：方法（四格缩进）与自由函数（零缩进）都停在自己的 `}` 上。
fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing function: {signature}"));
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &source[line_start..start];
    assert!(
        indent.chars().all(char::is_whitespace),
        "signature must start a line: {signature}"
    );
    let rest = &source[start..];
    let end = rest
        .find(&format!("\n{indent}}}\n"))
        .unwrap_or_else(|| panic!("function end not found: {signature}"));
    &rest[..end]
}

/// 不变量 1：dbnum 只由 `RefNo::dbno()` 给出，且只在 `refno_route` 里出现这一个解码式。
#[test]
fn the_bit_decode_lives_in_refno_route_and_nowhere_else() {
    assert!(production(REFNO_ROUTE).contains("refno.is_valid().then(|| refno.dbno())"));
    for (name, source) in [
        ("direct_store", DIRECT_STORE),
        ("model_source", MODEL_SOURCE),
        ("element_attributes", ELEMENT_ATTRIBUTES),
        ("direct_tree", DIRECT_TREE),
        ("mdb_route_directory", ROUTE_DIRECTORY),
    ] {
        let code = production(source);
        assert!(
            !code.contains("0x3E000") && !code.contains("& 0x1FFF"),
            "{name}: 位公式只许写在 refno_route / e3d-io RefNo::dbno 里"
        );
    }
}

/// `DirectStore` 生产代码不再问 ref0 反查表，也不再用 `word0` 冒充 dbnum。
#[test]
fn direct_store_routes_by_decoding_not_by_ref0_table() {
    let code = production(DIRECT_STORE);
    for needle in [
        "resolve_ref0(",
        "dbnum_of_ref0(",
        ".get_0()",
        "UnresolvedRef0",
    ] {
        assert!(
            !code.contains(needle),
            "direct_store.rs 生产代码不得含 `{needle}`"
        );
    }
    let dbnum_of = function_body(&code, "pub fn dbnum_of(&self, refno: RefU64)");
    assert!(
        dbnum_of.contains("refno_dbnum(refno)"),
        "dbnum_of 必须经共享解码 helper"
    );
    for variant in ["InvalidRefNo", "DbNotInMdb", "DbNumberMismatch"] {
        assert!(code.contains(variant), "错误变体 {variant} 必须存在");
    }
    let open_session = function_body(&code, "fn open_session(&self, pin: &DbPin)");
    assert!(
        open_session.contains("descriptor().db_mark"),
        "开库后必须核文件自报的库号（不变量 3）"
    );
}

/// 生成根定库：位解码 + MDB 成员校验 + 目标库恰好一次 `element_exists` 点查；不遍历全部 DESI，
/// 没有按 word0 键的缓存，开库只在那个 helper 里出现一次（切片 §5.2 / §9 Q5）。
#[test]
fn dbnum_of_root_never_probes_every_desi() {
    let code = production(MODEL_SOURCE);
    assert!(!code.contains("DBNUM_BY_REF0"));
    let body = function_body(&code, "pub fn dbnum_of_root_in(");
    assert!(body.contains("refno_dbnum_of(root)"));
    assert!(
        !body.contains("for pin in"),
        "不得遍历 current_mdb_sources 的全部 pins"
    );
    for needle in ["ReadOnlyEngine::open(", "find_element(", "scan_elements("] {
        assert!(
            !body.contains(needle),
            "dbnum_of_root_in 不得含 `{needle}`：开库与点查只在 element_exists 里"
        );
    }
    assert_eq!(
        body.matches("element_exists(").count(),
        1,
        "恰好一次目标库点查"
    );
    assert_eq!(
        code.matches("ReadOnlyEngine::open(").count(),
        1,
        "整个 model_source 只在 element_exists 里开库"
    );
    assert!(!code.contains("scan_elements("), "定库不扫元素");
    let element_exists = function_body(&code, "fn element_exists(pin: &DbPin, root: RefNo)");
    assert!(
        element_exists.contains("descriptor().db_mark"),
        "开库后必须核文件自报的库号（不变量 3）"
    );
    assert!(
        element_exists.contains(".search_index(root)"),
        "点查走索引，不读记录页"
    );
}

/// 分母与「根归哪个库」共用一个类型集（DESI + ISOD）。
#[test]
fn design_dbnums_and_root_resolution_share_one_element_db_predicate() {
    let code = production(MODEL_SOURCE);
    let denominator = function_body(&code, "pub fn mdb_design_dbnums()");
    assert!(denominator.contains("is_element_db(&pin.db_type)"));
    assert!(
        !denominator.contains("\"DESI\""),
        "类型集不得在分母里另写一份"
    );
    let resolution = function_body(&code, "pub fn dbnum_of_root_in(");
    assert!(resolution.contains("is_element_db(&pin.db_type)"));
    let predicate = function_body(&code, "pub fn is_element_db(db_type: &str)");
    assert!(predicate.contains("\"DESI\"") && predicate.contains("\"ISOD\""));
}

/// 属性服务与树服务的定位器都从 CURD 成员清单造，不再从元素骨架抄 `ref0 → dbnum`。
#[test]
fn attribute_and_tree_services_take_their_locator_from_the_route_directory() {
    let attributes = production(ELEMENT_ATTRIBUTES);
    assert!(
        !attributes.contains("mdb_memory_store::ensure("),
        "属性服务不得再等整份骨架"
    );
    assert!(!attributes.contains("locator_parts()"));
    assert!(
        !attributes.contains("build_from_dir("),
        "依赖目录只读文件头，不再逐库扫 ref0"
    );
    assert!(attributes.contains("MdbRouteDirectory::from_membership("));

    let tree = production(DIRECT_TREE);
    assert!(!tree.contains("locator_parts()"));
    assert!(tree.contains("MdbRouteDirectory::from_membership("));
}

/// 路由目录里没有逐元素表：构造只读成员清单与文件头。
#[test]
fn the_route_directory_holds_no_per_element_maps() {
    let code = production(ROUTE_DIRECTORY);
    for needle in [
        "scan_elements(",
        "find_element(",
        "HashMap<RefNo",
        "HashMap<u32, u32>",
    ] {
        assert!(
            !code.contains(needle),
            "mdb_route_directory.rs 不得含 `{needle}`"
        );
    }
    assert!(code.contains("fn dbnum_of_ref0(&self, _ref0: u32) -> Option<u32> {\n        None"));
}

/// 不变量「不写死现场 dbnum」：生产源码里不得出现 JEU 的 8188。
#[test]
fn production_sources_do_not_hardcode_site_dbnums() {
    for (name, source) in [
        ("direct_store", DIRECT_STORE),
        ("model_source", MODEL_SOURCE),
        ("element_attributes", ELEMENT_ATTRIBUTES),
        ("direct_tree", DIRECT_TREE),
        ("refno_route", REFNO_ROUTE),
        ("mdb_route_directory", ROUTE_DIRECTORY),
    ] {
        assert!(
            !production(source).contains("8188") && !production(source).contains("8_188"),
            "{name}: 现场 dbnum 8188 只许出现在测试与基线文件里"
        );
    }
}

// ---- 行为护栏：合成库文件 + read_stats 计数器 ----

const WORDS_PER_PAGE: u32 = 64;
const PAGE_BYTES: usize = WORDS_PER_PAGE as usize * 4;
const INDEX_NOUN: u32 = 13_387_743;
const INDEX_HEADER_DWORDS: usize = 7;

/// `RefNo::dbno` 的逆：低 13 位原位，extent 放 bits 13..25，高位段左移 13。
fn word0(dbno: u32, extent: u32) -> u32 {
    (dbno & 0x1FFF) | ((extent & 0x1FFF) << 13) | ((dbno & 0x3E000) << 13)
}

fn put(page: &mut [u8], offset: usize, value: u32) {
    page[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// 照 `vendor/e3d-io/tests/refno_native_stage0.rs::write_counter_fixture` 写一个四页的合成库：
/// 描述符页（+0x08 自报 `dbno`）、会话页、一页只有一条的索引叶、一页元素记录。
/// 返回文件路径与索引里唯一的那个元素。
fn write_synthetic_db(directory: &Path, dbno: u32) -> (PathBuf, RefNo) {
    let element = RefNo::new(word0(dbno, 1), 42);
    let mut pages = vec![vec![0_u8; PAGE_BYTES]; 4];

    put(&mut pages[0], 0x08, dbno);
    put(&mut pages[0], 0x28, 1);
    put(&mut pages[0], 0x2C, 1);
    put(&mut pages[0], 0x30, 1);
    put(&mut pages[0], 0x34, WORDS_PER_PAGE);
    put(&mut pages[0], 0x38, element.word0);

    put(&mut pages[1], 0x00, 3);
    put(&mut pages[1], 0x04, u32::MAX);
    put(&mut pages[1], 0x0C, 7);
    put(&mut pages[1], 0x1C, 2);
    put(&mut pages[1], 0x20, 1);

    put(&mut pages[2], 0x00, 5);
    for (index, value) in [INDEX_NOUN, 0, 2, 2, 0, 53].into_iter().enumerate() {
        put(&mut pages[2], (index + 1) * 4, value);
    }
    let entry = [element.word0, element.word1, 3, (20_u32 << 12) | 1];
    for (index, value) in entry.into_iter().enumerate() {
        put(&mut pages[2], (INDEX_HEADER_DWORDS + index) * 4, value);
    }

    put(&mut pages[3], 0x00, 7);
    for (index, value) in [
        11,
        element.word0,
        element.word1,
        0x000A_AA01,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        put(&mut pages[3], 40 + index * 4, value);
    }

    let path = directory.join(format!("tst{dbno}_0001"));
    let mut file = File::create(&path).expect("create synthetic E3D database");
    for page in pages {
        file.write_all(&page).expect("write synthetic page");
    }
    file.flush().expect("flush synthetic database");
    (path, element)
}

fn root_error(result: anyhow::Result<u32>) -> RootDbError {
    let error = result.expect_err("root resolution must fail");
    error
        .downcast_ref::<RootDbError>()
        .unwrap_or_else(|| panic!("not a RootDbError: {error:#}"))
        .clone()
}

/// 定一个根 = 开目标库 1 次 + 索引点查 1 次，零扫描（切片 W4 验收 / §5.2 行为断言）。
/// 元素不存在、文件自报库号不符各有各的错；不在 MDB / 非元素库 / 非法引用 / 声明了没文件，
/// 一个成功的开库都不许多。
#[test]
fn dbnum_of_root_opens_only_the_target_database_once() {
    let directory = tempfile::tempdir().expect("temporary fixture directory");
    let (desi_file, element) = write_synthetic_db(directory.path(), 1_000);
    // 头里写的是 1002，成员清单却把它登记成 1001：核头必须拦下来。
    let (lying_file, _) = write_synthetic_db(directory.path(), 1_002);
    let members = vec![
        DbPin {
            dbnum: 1_000,
            db_type: "DESI".into(),
            file: desi_file.clone(),
            sesno: None,
        },
        DbPin {
            dbnum: 1_001,
            db_type: "ISOD".into(),
            file: lying_file.clone(),
            sesno: None,
        },
        DbPin {
            dbnum: 2_000,
            db_type: "DESI".into(),
            file: directory.path().join("declared-but-never-written"),
            sesno: None,
        },
        DbPin {
            dbnum: 5_052,
            db_type: "CATA".into(),
            file: directory.path().join("catalogue-never-opened"),
            sesno: None,
        },
    ];
    let mdb = || Ok(members.clone());

    read_stats::reset();
    assert_eq!(
        dbnum_of_root_in(element, mdb).expect("root lives in DESI 1000"),
        1_000
    );
    let one_root = read_stats::snapshot();
    assert_eq!(
        (
            one_root.database_opens,
            one_root.scan_calls,
            one_root.scanned_elements,
            one_root.index_point_lookups,
        ),
        (1, 0, 0, 1),
        "{}",
        one_root.summary()
    );

    // 同一个库、索引里没有的 refno：再开一次那一个库、再查一次，报 ElementMissing 并说出会话号。
    let missing = RefNo::new(element.word0, element.word1 + 1);
    let error = dbnum_of_root_in(missing, mdb).unwrap_err();
    assert!(error.to_string().contains("1000"), "{error}");
    match root_error(Err(error)) {
        RootDbError::ElementMissing {
            root,
            dbnum: 1_000,
            file,
            sesno,
        } => {
            assert_eq!(root, missing);
            assert_eq!(file, desi_file);
            assert!(sesno > 0, "会话号来自会话页，不是 0");
        }
        other => panic!("expected ElementMissing, got {other:?}"),
    }

    // 文件自报的库号与成员清单不符：开库算一次（成功打开了），但不点查。
    let in_lying_file = RefNo::new(word0(1_001, 1), 7);
    match root_error(dbnum_of_root_in(in_lying_file, mdb)) {
        RootDbError::DbNumberMismatch {
            dbnum: 1_001,
            actual: 1_002,
            file,
        } => assert_eq!(file, lying_file),
        other => panic!("expected DbNumberMismatch, got {other:?}"),
    }
    let probed = read_stats::snapshot();
    assert_eq!(
        (
            probed.database_opens,
            probed.scan_calls,
            probed.index_point_lookups
        ),
        (3, 0, 2),
        "{}",
        probed.summary()
    );

    // 四种拒绝，一个成功的开库都不许多：不在 MDB、目录库、非法引用、声明了却没文件。
    match root_error(dbnum_of_root_in(RefNo::new(word0(3_000, 1), 1), mdb)) {
        RootDbError::DbNotInMdb { dbnum: 3_000, .. } => {}
        other => panic!("expected DbNotInMdb, got {other:?}"),
    }
    match root_error(dbnum_of_root_in(RefNo::new(word0(5_052, 1), 1), mdb)) {
        RootDbError::NotAnElementDb {
            dbnum: 5_052,
            db_type,
            ..
        } => assert_eq!(db_type, "CATA"),
        other => panic!("expected NotAnElementDb, got {other:?}"),
    }
    match root_error(dbnum_of_root_in(RefNo::new(0, 0), mdb)) {
        RootDbError::InvalidRefNo { .. } => {}
        other => panic!("expected InvalidRefNo, got {other:?}"),
    }
    match root_error(dbnum_of_root_in(RefNo::new(word0(2_000, 1), 1), mdb)) {
        RootDbError::Open { dbnum: 2_000, .. } => {}
        other => panic!("expected Open, got {other:?}"),
    }
    let refused = read_stats::snapshot();
    assert_eq!(
        (
            refused.database_opens,
            refused.scan_calls,
            refused.scanned_elements,
            refused.index_point_lookups,
        ),
        (3, 0, 0, 2),
        "{}",
        refused.summary()
    );
}
