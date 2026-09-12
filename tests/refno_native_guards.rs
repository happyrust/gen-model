//! 源码护栏：跨库属性只经 RefNo 位解码路由（切片 `2026-09-12-cross-db-attribute-refno-routing-dev-plan.md` §5.2）。
//!
//! 这些断言扫的是源码文本，不跑任何库：它们守的是「不得回到全局 ref0 反查表 / 逐库试查」这条线，
//! 一旦有人把老路接回去，这里先红。每条都指向母计划 `2026-09-12-refno-native-routing-and-lazy-mdb-plan.md`
//! 的一条不变量。

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

/// 生成根定库：位解码 + MDB 目录校验 + 只开目标库；不遍历全部 DESI，没有按 word0 键的缓存。
#[test]
fn dbnum_of_root_never_probes_every_desi() {
    let code = production(MODEL_SOURCE);
    assert!(!code.contains("DBNUM_BY_REF0"));
    let body = function_body(&code, "pub fn dbnum_of_root(root: RefNo)");
    assert!(body.contains("refno_dbnum_of(root)"));
    assert!(
        !body.contains("for pin in"),
        "不得遍历 current_mdb_sources 的全部 pins"
    );
    assert_eq!(
        body.matches("ReadOnlyEngine::open(").count(),
        1,
        "只开解出的那一个库"
    );
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
