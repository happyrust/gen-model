//! 真库普查：**跨生成根的引用边**有多少——联动对齐计划 C1 的闸，顺带做 C5 的 TOPF 对拍。
//!
//! 为什么要它：`docs/plans/2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C1。Core3D 的
//! `change/EVALAT` 里有两类**引用类**联动（`docs/evidence/2026-09-06-ida-core3d-change-evalat-cascade.md`）：
//! 结构连接 `ADSTCH`（`CREF` ↔ `JOIS`/`JOIE`、`CFRA` ↔ `PFRE` 双向核对，从变更元素**正向读**）与孔洞
//! （PANE / 设备子件变了 → 引用它的 `HOLC` 构件，**反向**）。direct 文件窗口路径一条都没有——
//! 但「代码上没有」是不是缺口，要看真库里这些边**跨不跨生成根**：对端与变更元素同在一个根之下，
//! 整根重生成本来就覆盖；跨了根才是漏。09-04 清单 #3 / #5 都栽在「先立项、后发现语料里压不出窗口」上，
//! 这次反过来：先数，为零就不写代码。
//!
//! 只读，不连 Surreal，不写任何东西。每个 DESI 库：
//! 1. `ScanTier::Full` 全扫一遍，拿 noun / owner / **全部**引用属性：显式尾块里的 `RefNo` / `RefNoArray`，
//!    加上按 `desvir.dat` noun 模板解出的隐式区引用（`HREF` / `TREF` / `JOIS` / `JOIE` / `CREF` 这些
//!    定长槽位属性都在隐式区，只看显式尾块会把它们全漏掉；见 `extract_parsed_element_with_descriptors`）；
//! 2. 用**生产同一段遍历** [`enumerate_generation_roots_in_subtree`] 枚举根集（direct 口径），
//!    再给每个元素算「根链」（owner 链上所有是根的祖先，含自身）与「最近的根」；
//! 3. 对每条引用边 `(src, attr, dst)` 判同库 / 跨库 / 悬空，再判两个方向各两档。正向 `fwd` 问的是
//!    **src 变了，dst 的几何会不会跟着重算**：`nearest` 档 = dst 最近的根 ∈ src 的根链（嵌套根各管各的
//!    几何时的口径）；`any` 档 = 两条根链有交集（外层根整棵子树重生成时的口径）。反向 `rev` 是 dst 变了
//!    src 会不会重算，同样两档。结构连接边再核对回指（`X.JOIS == el` 等，与 `ADSTCH` 同形）；
//! 4. 会话链逐步 `diff_steps`，数**跨根边上只改了一端**的窗口——那才是 C2 / C3 的验收窗口；
//! 5. 顺带（C5）：按 core `nounlib/LISTOP` 口径判 TOPF 顶层件，数「core 单独重画、我们一个根都不归」
//!    的实例；以及所有 core 判 primitive 却没有任何根祖先的元素（BOX 直挂 ZONE 那一类）。
//! 6. ★0907 C8：电缆端点边——`STAREF` / `ENDREF` 指向 `EQUI` / `ELCONN`（core 规则 F `ELCXRF`）或指向
//!    `CWBRAN` 之下的路由节点 `HATTA` / `TATTA` / `RNODE` / `RATTA`（规则 E `CLIST`；那一端算到 `CWBRAN` 头上）。
//!    core 对这两条走的是反向查（`DB_RefTableIterator`，证据 §8.6），所以窗口统计问的是**「只改了被引用端、电缆的根没被碰」**：
//!    `cable_dst_only` = 被引用元素（或规则 E 的 `CWBRAN`）自己的记录变了；`cable_dst_root_only` = 只是它最近的根下面有东西变了
//!    （GETWORK 是先 `FNDTOP` 再 `EVALAT(top)`，所以 `EQUI` 下面一个 BOX 动了 core 也会重画接在 `EQUI` 上的电缆）。
//!    两档都要求电缆那一端最近的根**没有**任何变更——有的话整根重生成本来就覆盖。边是从链尾会话扫出来的，
//!    电缆那端在这一步的 `to` 会话里还没 `Inserted` 的（电缆后建）边当时不存在，不算窗口，单独记 `cable_dst_only_unborn`。
//!
//! 用法：
//! ```text
//! cargo run --release --bin cross_root_reference_census -- \
//!     --dir "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000" \
//!     --dir "D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000" \
//!     --json docs/evidence/2026-09-07-cross-root-reference-census.json
//! ```
//! `--template-dir` 指 `attlib.dat` 与 `desvir.dat` 所在目录（缺省读 `AIOS_E3D_TEMPLATE_DIR`，
//! 再退到 direct 模式的本机默认值）；`--no-windows` 跳过会话逐步 diff；`--max-steps N` 每库最多看 N 步；
//! `--max-files N` 调试用。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use aios_core::{RefU64, RefnoEnum};
use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::data_interface::generation_root::{
    GenerationRootKind, SubtreeElement, core_primitive_bits, core_toplevel_bit,
    enumerate_generation_roots_in_subtree, noun_is_primitive, resolve_delivery_unit_types,
};
use aios_database::noun_dict::default_noun_capabilities;
use e3d_attlib::{AttlibData, db1_dehash, db1_hash};
use e3d_io::db_element::template_file_for;
use e3d_io::engine::extract_parsed_element_with_descriptors;
use e3d_io::index::IndexCandidate;
use e3d_io::record::descriptor::{AttributeExtractionStatus, DescriptorValue};
use e3d_io::record::explicit::ExplicitValue;
use e3d_io::record::template::TemplateProvider;
use e3d_io::session::SessionSelector;
use e3d_io::{ReadOnlyEngine, RefNo, ScanTier};

/// direct 模式的本机默认模板目录（与 `direct_store.rs` 的 `TEMPLATE_DIR_DEFAULT` 同值，那个是私有常量）。
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

/// `ADSTCH` 三组核对里出现的五个属性（证据 §3）。
const STRUCTURAL_ATTRS: &[&str] = &["CREF", "JOIS", "JOIE", "CFRA", "PFRE"];

/// 电缆端点边的两个属性（core `ELCXRF` = `BACKRE{ENDREF, STAREF}`，`LALCAB::takeCables` 同两个；证据 §8.6）。
const CABLE_ENDPOINT_ATTRS: &[&str] = &["STAREF", "ENDREF"];

/// 规则 F 的被引用端 noun。
const CABLE_ENDPOINT_TARGET_NOUNS: &[&str] = &["EQUI", "ELCONN"];

/// 规则 E 的被引用端 noun（都在 `CWBRAN` 之下；`CAB_DB_BranchCableList_pseudo` 收的正是这四种成员）。
const CABLE_ROUTE_NODE_NOUNS: &[&str] = &["HATTA", "TATTA", "RNODE", "RATTA"];

/// owner 链最多走多长；超过按成环 / 坏数据处理。
const MAX_DEPTH: usize = 64;

/// noun → (数量, owner noun 分布)。
type NounOwnerCounts = BTreeMap<String, (u64, BTreeMap<String, u64>)>;

/// 一条「跨根边上只改了一端」的命中：(from sesno, to sesno, 边, 改的哪端)。
type WindowHit = (u32, u32, CrossEdge, &'static str);

/// 正向属性 → `ADSTCH` 要求对端回指的属性（证据 §3 三组；`PFRE` → `CFRA` 是第 1 组反着看，
/// core 不从这一端发起，这里一并核对只为看数据形状）。
fn back_pointers(attr: &str) -> &'static [&'static str] {
    match attr {
        "CFRA" => &["PFRE"],
        "CREF" => &["JOIS", "JOIE"],
        "JOIS" | "JOIE" => &["CREF"],
        "PFRE" => &["CFRA"],
        _ => &[],
    }
}

struct Args {
    dirs: Vec<PathBuf>,
    template_dir: PathBuf,
    json: Option<PathBuf>,
    max_files: usize,
    max_steps: usize,
    windows: bool,
    examples: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        dirs: Vec::new(),
        template_dir: std::env::var_os(TEMPLATE_DIR_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT)),
        json: None,
        max_files: usize::MAX,
        max_steps: usize::MAX,
        windows: true,
        examples: 40,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dir" => args
                .dirs
                .push(PathBuf::from(it.next().expect("--dir 要一个目录"))),
            "--template-dir" => {
                args.template_dir = PathBuf::from(it.next().expect("--template-dir 要一个目录"))
            }
            "--json" => args.json = Some(PathBuf::from(it.next().expect("--json 要一个路径"))),
            "--max-files" => {
                args.max_files = it
                    .next()
                    .expect("--max-files 要一个数")
                    .parse()
                    .expect("--max-files 不是数")
            }
            "--max-steps" => {
                args.max_steps = it
                    .next()
                    .expect("--max-steps 要一个数")
                    .parse()
                    .expect("--max-steps 不是数")
            }
            "--examples" => {
                args.examples = it
                    .next()
                    .expect("--examples 要一个数")
                    .parse()
                    .expect("--examples 不是数")
            }
            "--no-windows" => args.windows = false,
            other => panic!("不认识的参数 {other}"),
        }
    }
    args
}

// ---------------------------------------------------------------------------
// 账目
// ---------------------------------------------------------------------------

/// 一类边（`src noun` × 属性 × `dst noun`）的账。
#[derive(Debug, Default, Clone, serde::Serialize)]
struct Tally {
    edges: u64,
    /// 目标在本库却不存在（删了没清引用）。
    dangling: u64,
    /// 目标在另一个 DESI 库——跨库必跨根。
    cross_db_desi: u64,
    /// 目标在非 DESI 库（CATA / PROP / …），不在本计划范围。
    cross_db_other: u64,
    /// 同库且目标存在，下面几格都只数这一部分。
    same_db: u64,
    /// src 变 → dst 会重算（dst 最近的根 ∈ src 的根链）。
    fwd_covered_nearest: u64,
    /// src 变 → dst **不会**重算（nearest 口径）。**正向缺口的数。**
    fwd_uncovered_nearest: u64,
    /// src 变 → dst 不会重算，连外层根整棵重生成也罩不住（两条根链无交集）。缺口下界。
    fwd_uncovered_any: u64,
    /// dst 变 → src 会重算（src 最近的根 ∈ dst 的根链）。
    rev_covered_nearest: u64,
    /// dst 变 → src **不会**重算。**反向缺口的数**（孔洞那条看这一格）。
    rev_uncovered_nearest: u64,
    rev_uncovered_any: u64,
    /// src 自己一个根祖先都没有（direct 路径根本不会生成它）。
    src_no_root: u64,
    dst_no_root: u64,
    /// 结构连接边：核对了回指的条数与成立的条数（`ADSTCH` 只在回指成立时联动）。
    backref_checked: u64,
    backref_ok: u64,
    /// 回指成立**且**正向 nearest 不覆盖——这才是 `ADSTCH` 会联动而我们漏掉的边。
    backref_ok_fwd_uncovered_nearest: u64,
    /// src 的 owner 链上有 `STRU`（`EVALAT` 规则 K 的前置：`CGETEL(STRU)` 成功才走 `ADSTCH`）。
    src_under_stru: u64,
    /// 上一格 ∩ 回指成立 ∩ 正向 nearest 不覆盖——严格按 core 的触发条件算出来的缺口。
    backref_ok_fwd_uncovered_under_stru: u64,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.edges += other.edges;
        self.dangling += other.dangling;
        self.cross_db_desi += other.cross_db_desi;
        self.cross_db_other += other.cross_db_other;
        self.same_db += other.same_db;
        self.fwd_covered_nearest += other.fwd_covered_nearest;
        self.fwd_uncovered_nearest += other.fwd_uncovered_nearest;
        self.fwd_uncovered_any += other.fwd_uncovered_any;
        self.rev_covered_nearest += other.rev_covered_nearest;
        self.rev_uncovered_nearest += other.rev_uncovered_nearest;
        self.rev_uncovered_any += other.rev_uncovered_any;
        self.src_no_root += other.src_no_root;
        self.dst_no_root += other.dst_no_root;
        self.backref_checked += other.backref_checked;
        self.backref_ok += other.backref_ok;
        self.backref_ok_fwd_uncovered_nearest += other.backref_ok_fwd_uncovered_nearest;
        self.src_under_stru += other.src_under_stru;
        self.backref_ok_fwd_uncovered_under_stru += other.backref_ok_fwd_uncovered_under_stru;
    }
}

/// 一个 TOPF noun 的 C5 账。
#[derive(Debug, Default, Clone, serde::Serialize)]
struct TopfStat {
    /// `LISTOP` 判真的实例数。
    instances: u64,
    /// 其中有根祖先（含自身是根）的。
    with_root: u64,
    /// **一个根都不归**的——core 单独重画、我们不生成。
    no_root: u64,
    /// 无根实例的 owner noun 分布。
    no_root_owner_nouns: BTreeMap<String, u64>,
    /// 有根实例被哪个 noun 的根罩住（最近的根）。
    nearest_root_nouns: BTreeMap<String, u64>,
}

impl TopfStat {
    fn add(&mut self, other: &TopfStat) {
        self.instances += other.instances;
        self.with_root += other.with_root;
        self.no_root += other.no_root;
        for (k, v) in &other.no_root_owner_nouns {
            *self.no_root_owner_nouns.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.nearest_root_nouns {
            *self.nearest_root_nouns.entry(k.clone()).or_default() += v;
        }
    }
}

/// 一条「跨根边上只改了一端」的窗口样例——C2 / C3 的候选验收窗口。
#[derive(Debug, Clone, serde::Serialize)]
struct WindowExample {
    dbnum: u32,
    file: String,
    from_sesno: u32,
    to_sesno: u32,
    src: String,
    src_noun: String,
    attr: String,
    dst: String,
    dst_noun: String,
    /// `src` = 持有属性的一端改了（正向缺口的验收窗口）；`dst` = 被引用的一端改了（反向）。
    changed_end: &'static str,
    backref_ok: bool,
    /// 只看了索引层（键的地址变了），没逐记录排除「搬家未改字节」的情况。
    index_level_only: bool,
}

#[derive(Debug, Default, serde::Serialize)]
struct WindowStats {
    /// 看了多少库的会话链。
    dbs_stepped: u64,
    steps: u64,
    /// 跨根结构连接边（回指成立）：持属性端单独改动的窗口数 / 被引用端单独改动 / 两端都改。
    structural_src_only: u64,
    structural_dst_only: u64,
    structural_both: u64,
    /// 跨根孔洞边（反向 nearest 不覆盖）：被打孔件单独改动 / 孔洞件单独改动 / 两端都改。
    hole_dst_only: u64,
    hole_src_only: u64,
    hole_both: u64,
    /// ★0907 C8 跨根电缆端点边（反向 nearest 不覆盖）：被引用端（`EQUI` / `ELCONN`，或规则 E 的 `CWBRAN` /
    /// 路由节点）**自己的记录**改了而电缆那端最近的根没被碰 → C8 该拉进来的窗口；
    cable_dst_only: u64,
    /// 被引用端自己没改、但它最近的根下面有东西改了，电缆根没被碰（GETWORK `FNDTOP` 口径下 core 也会重画电缆）。
    cable_dst_root_only: u64,
    /// 电缆那端（`CABLE` / `EXTGEO` 记录）改了、被引用端没改 —— 整根重生成本来就覆盖，不是缺口。
    cable_src_only: u64,
    /// 被引用端改了、电缆根也被碰 —— 闭包本来就有。
    cable_both: u64,
    /// 被引用端改了、电缆根没被碰，**但电缆那端（`CABLE` / `EXTGEO`）在这一步的 `to` 会话里还没建**
    /// （后面某一步才 `Inserted`）—— 边当时不存在，不是窗口；单独记一笔，免得把「电缆还没画」误当缺口。
    cable_dst_only_unborn: u64,
    /// 会话链读不出来 / diff 失败的库。
    failed: u64,
}

impl WindowStats {
    fn add(&mut self, other: &WindowStats) {
        self.dbs_stepped += other.dbs_stepped;
        self.steps += other.steps;
        self.structural_src_only += other.structural_src_only;
        self.structural_dst_only += other.structural_dst_only;
        self.structural_both += other.structural_both;
        self.hole_dst_only += other.hole_dst_only;
        self.hole_src_only += other.hole_src_only;
        self.hole_both += other.hole_both;
        self.cable_dst_only += other.cable_dst_only;
        self.cable_dst_root_only += other.cable_dst_root_only;
        self.cable_src_only += other.cable_src_only;
        self.cable_both += other.cable_both;
        self.cable_dst_only_unborn += other.cable_dst_only_unborn;
        self.failed += other.failed;
    }
}

#[derive(Debug, Default, serde::Serialize)]
struct DbReport {
    dbnum: u32,
    file: String,
    sesno: u32,
    elements: usize,
    unreadable_records: usize,
    /// 没解出 noun 模板、只看了显式尾块的元素数。
    template_missing: usize,
    /// 隐式区解出的引用边数。
    implicit_edges: usize,
    /// refno 里解出的 dbno 与 descriptor 的 dbnum 不一致的元素数（应为 0，是本探针的自检）。
    dbno_mismatch: usize,
    roots_delivery: usize,
    roots_normal: usize,
    /// 不在 DBMETA / DBMDAT 树下、却一个根祖先都没有的元素数。
    elements_without_root: usize,
    /// 全部显式引用边（不含 DBMETA 树）。
    ref_edges: usize,
    uda_ref_edges: usize,
    /// 结构连接五属性的边，按 `SRC.ATTR->DST` 键。
    structural: BTreeMap<String, Tally>,
    /// 负实体（HOLC 代理）发出的边。
    holes: BTreeMap<String, Tally>,
    /// ★0907 C8 电缆端点边（`STAREF` / `ENDREF` → `EQUI` / `ELCONN`，或 → `CWBRAN` 之下的路由节点），
    /// 按 `SRC.ATTR->DST` 键；键尾带 `[F]` / `[E]` 标规则。
    cable_endpoint: BTreeMap<String, Tally>,
    /// 全部 DESI 内引用边（同库或跨 DESI 库），按 `SRC.ATTR->DST` 键。只进合计，不逐库落 JSON
    /// （486 库 × 几百个键太占地方；要逐库看就去掉这行 `skip`）。
    #[serde(skip)]
    all_edges: BTreeMap<String, Tally>,
    topf: BTreeMap<String, TopfStat>,
    /// core 判 primitive 却没有根祖先的元素：noun → (数量, owner noun 分布)。
    primitive_without_root: NounOwnerCounts,
    windows: WindowStats,
}

// ---------------------------------------------------------------------------
// 库内数据
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Element {
    noun: u32,
    owner: RefNo,
}

#[derive(Clone, Copy)]
struct Edge {
    src: RefNo,
    attr: u32,
    is_uda: bool,
    dst: RefNo,
}

struct Scanned {
    dbnum: u32,
    sesno: u32,
    elements: HashMap<RefNo, Element>,
    edges: Vec<Edge>,
    unreadable: usize,
    /// noun 在 `desvir.dat` 里没有模板 / 模板不权威 / 解码失败的元素数——这些只看了显式尾块。
    template_missing: usize,
    /// 隐式区解出来的引用边数（显式尾块里没有的那部分）。
    implicit_edges: usize,
}

fn is_null(refno: RefNo) -> bool {
    refno.word0 == 0 && refno.word1 == 0
}

fn to_enum(refno: RefNo) -> RefnoEnum {
    RefnoEnum::from(RefU64::from_two_nums(refno.word0, refno.word1))
}

fn from_enum(refno: RefnoEnum) -> RefNo {
    let raw = refno.refno().0;
    RefNo::new((raw >> 32) as u32, raw as u32)
}

fn scan_full(
    path: &Path,
    attlib: &AttlibData,
    provider: &mut TemplateProvider,
) -> anyhow::Result<Scanned> {
    let mut engine = ReadOnlyEngine::open(path)?;
    let dbnum = engine.descriptor().db_mark;
    let sesno = engine.session().sesno;
    let mut elements = HashMap::new();
    let mut edges = Vec::new();
    let mut unreadable = 0usize;
    let mut template_missing = 0usize;
    let mut implicit_edges = 0usize;
    let mut seen_attrs: HashSet<u32> = HashSet::new();
    for scanned in engine.scan_elements(ScanTier::Full)? {
        let Ok(scanned) = scanned else {
            unreadable += 1;
            continue;
        };
        elements.insert(
            scanned.refno,
            Element {
                noun: scanned.noun_hash,
                owner: scanned.owner,
            },
        );
        let Some(parsed) = scanned.parsed else {
            unreadable += 1;
            continue;
        };
        // 显式尾块：UDA 与「有值才落盘」的引用属性。
        seen_attrs.clear();
        for attr in &parsed.explicit_attributes {
            match &attr.value {
                ExplicitValue::RefNo(dst) if !is_null(*dst) => {
                    seen_attrs.insert(attr.hash);
                    edges.push(Edge {
                        src: scanned.refno,
                        attr: attr.hash,
                        is_uda: attr.is_uda,
                        dst: *dst,
                    });
                }
                ExplicitValue::RefNoArray(dsts) => {
                    seen_attrs.insert(attr.hash);
                    for dst in dsts.iter().filter(|dst| !is_null(**dst)) {
                        edges.push(Edge {
                            src: scanned.refno,
                            attr: attr.hash,
                            is_uda: attr.is_uda,
                            dst: *dst,
                        });
                    }
                }
                _ => {}
            }
        }
        // 隐式区：按 noun 模板解定长槽位。只收**落盘的**值（`Decoded`），模板 / 字典缺省、
        // 运行时派生的伪属性都不是引用边。
        let template = match provider.template_for(parsed.noun_hash) {
            Ok(Some(template)) => template,
            _ => {
                template_missing += 1;
                continue;
            }
        };
        let Ok(extraction) =
            extract_parsed_element_with_descriptors(scanned.refno, &parsed, attlib, template)
        else {
            template_missing += 1;
            continue;
        };
        for attr in extraction.attributes {
            if seen_attrs.contains(&attr.hash)
                || !matches!(attr.status, AttributeExtractionStatus::Decoded)
            {
                continue;
            }
            match attr.value {
                Some(DescriptorValue::RefNo(dst)) if !is_null(dst) => {
                    implicit_edges += 1;
                    edges.push(Edge {
                        src: scanned.refno,
                        attr: attr.hash,
                        is_uda: false,
                        dst,
                    });
                }
                Some(DescriptorValue::RefNoArray(dsts)) => {
                    for dst in dsts.into_iter().filter(|dst| !is_null(*dst)) {
                        implicit_edges += 1;
                        edges.push(Edge {
                            src: scanned.refno,
                            attr: attr.hash,
                            is_uda: false,
                            dst,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(Scanned {
        dbnum,
        sesno,
        elements,
        edges,
        unreadable,
        template_missing,
        implicit_edges,
    })
}

/// 一个库的层级视图：owner 链、根集、逐元素的根链缓存。
struct Hierarchy<'a> {
    elements: &'a HashMap<RefNo, Element>,
    noun_names: &'a HashMap<u32, String>,
    roots: HashMap<RefNo, GenerationRootKind>,
    /// 元素 → (owner 链上的根，含自身，从近到远)；`None` = 在 DBMETA / DBMDAT 树下或链坏了。
    chain_cache: HashMap<RefNo, Option<Vec<RefNo>>>,
    meta_hashes: [u32; 2],
}

impl<'a> Hierarchy<'a> {
    fn build(
        elements: &'a HashMap<RefNo, Element>,
        noun_names: &'a HashMap<u32, String>,
        unit_types: &[String],
    ) -> anyhow::Result<Self> {
        let mut children: HashMap<RefNo, Vec<RefNo>> = HashMap::new();
        let mut tops = Vec::new();
        for (refno, element) in elements {
            if element.owner == *refno || !elements.contains_key(&element.owner) {
                tops.push(*refno);
            } else {
                children.entry(element.owner).or_default().push(*refno);
            }
        }
        tops.sort_by_key(|r| (r.word0, r.word1));
        for kids in children.values_mut() {
            kids.sort_by_key(|r| (r.word0, r.word1));
        }

        let mut roots = HashMap::new();
        for top in tops {
            let found = enumerate_generation_roots_in_subtree(to_enum(top), unit_types, |refno| {
                let refno = from_enum(refno);
                let element = elements
                    .get(&refno)
                    .ok_or_else(|| anyhow::anyhow!("{refno} not in scan"))?;
                Ok(SubtreeElement {
                    noun: noun_name(noun_names, element.noun),
                    name: String::new(),
                    members: children
                        .get(&refno)
                        .map(|kids| kids.iter().copied().map(to_enum).collect())
                        .unwrap_or_default(),
                })
            })?;
            for root in found {
                roots.insert(from_enum(root.root), root.kind);
            }
        }
        Ok(Self {
            elements,
            noun_names,
            roots,
            chain_cache: HashMap::new(),
            meta_hashes: [db1_hash("DBMETA"), db1_hash("DBMDAT")],
        })
    }

    fn noun_of(&self, refno: RefNo) -> Option<&str> {
        self.elements
            .get(&refno)
            .and_then(|element| self.noun_names.get(&element.noun).map(String::as_str))
    }

    /// owner 链（含自身），从元素到顶；`None` = 成环 / 超深 / 经过 DBMETA 树。
    fn path(&self, refno: RefNo) -> Option<Vec<RefNo>> {
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut current = refno;
        loop {
            if !seen.insert(current) || path.len() > MAX_DEPTH {
                return None;
            }
            let element = self.elements.get(&current)?;
            if self.meta_hashes.contains(&element.noun) {
                return None;
            }
            path.push(current);
            if element.owner == current || !self.elements.contains_key(&element.owner) {
                return Some(path);
            }
            current = element.owner;
        }
    }

    /// 根链：owner 链上是根的那些（含自身），从近到远。
    fn chain(&mut self, refno: RefNo) -> Option<Vec<RefNo>> {
        if let Some(cached) = self.chain_cache.get(&refno) {
            return cached.clone();
        }
        let chain = self.path(refno).map(|path| {
            path.into_iter()
                .filter(|node| self.roots.contains_key(node))
                .collect::<Vec<_>>()
        });
        self.chain_cache.insert(refno, chain.clone());
        chain
    }
}

fn noun_name(noun_names: &HashMap<u32, String>, hash: u32) -> String {
    noun_names
        .get(&hash)
        .cloned()
        .or_else(|| db1_dehash(hash))
        .unwrap_or_else(|| format!("#{hash}"))
}

fn attr_name(attr_names: &mut HashMap<u32, String>, hash: u32, is_uda: bool) -> String {
    attr_names
        .entry(hash)
        .or_insert_with(|| {
            if is_uda {
                format!("UDA:{hash}")
            } else {
                db1_dehash(hash).unwrap_or_else(|| format!("#{hash}"))
            }
        })
        .clone()
}

/// core `nounlib/LISTOP`（证据 §4）：TOPF 顶层可画件判据。
fn listop(hier: &Hierarchy<'_>, refno: RefNo, path: &[RefNo]) -> bool {
    let Some(noun) = hier.noun_of(refno) else {
        return false;
    };
    let owner_noun = path.get(1).and_then(|owner| hier.noun_of(*owner));
    if core_toplevel_bit(noun) == Some(true) {
        return noun != "WORL" && owner_noun != Some("TMPL");
    }
    if core_primitive_bits(noun).is_some_and(|(a, _)| a) {
        return match noun {
            "RPLA" | "PVOL" | "DRAW" => matches!(owner_noun, Some("SITE") | Some("ZONE")),
            "FIXING" => {
                path[1..]
                    .iter()
                    .filter_map(|ancestor| hier.noun_of(*ancestor))
                    .find(|ancestor| {
                        core_toplevel_bit(ancestor) == Some(true) || *ancestor == "FIXING"
                    })
                    == Some("STRU")
            }
            _ => false,
        };
    }
    false
}

/// 一条跨根边（窗口统计的输入）。
#[derive(Clone, Copy)]
struct CrossEdge {
    src: RefNo,
    attr: u32,
    dst: RefNo,
    structural: bool,
    backref_ok: bool,
    fwd_uncovered: bool,
    rev_uncovered: bool,
    /// ★0907 C8：电缆端点边才有。`structural` 与它互斥；两者都不是的就是孔洞边。
    cable: Option<CableEdge>,
}

/// 电缆端点边的附加信息（窗口统计要看根级的「被碰」）。
#[derive(Clone, Copy)]
struct CableEdge {
    /// `"F"` = `EQUI` / `ELCONN` 被引用；`"E"` = `CWBRAN` 之下的路由节点被引用。
    rule: &'static str,
    /// core 规则的**主语**：F 是 `dst` 自己，E 是 `dst` 的 owner `CWBRAN`（规则 E 在 `CWBRAN` 变了时触发）。
    subject: RefNo,
    /// 电缆那端（`src`）最近的生成根；`None` = 无根。
    src_root: Option<RefNo>,
    /// 被引用端最近的生成根；`None` = 无根。
    dst_root: Option<RefNo>,
}

fn main() -> anyhow::Result<()> {
    let args = parse_args();
    if args.dirs.is_empty() {
        eprintln!("用法见文件头注释：至少要一个 --dir");
        std::process::exit(2);
    }

    let started_all = Instant::now();
    let desi = db1_hash("DESI");
    let noun_names: HashMap<u32, String> = default_noun_capabilities()
        .iter()
        .map(|caps| (caps.noun_hash as u32, caps.noun_name.clone()))
        .collect();
    // HOLC 代理：字典 `positive_equivalent != 0` 的 12 个负实体 noun（NBOX NCYL …）。HOLC 位
    // （`0x92663`）本身没导出（核对表 R0：「P0 不必导 negative 表」），这是最近的可用近似；
    // `INCOMP` 还排除 `DTHOLE`，这里同样排除。
    let hole_nouns: HashSet<u32> = default_noun_capabilities()
        .iter()
        .filter(|caps| caps.positive_equivalent.is_some_and(|hash| hash != 0))
        .filter(|caps| caps.noun_name != "DTHOLE")
        .map(|caps| caps.noun_hash as u32)
        .collect();
    let structural_hashes: HashMap<u32, &str> = STRUCTURAL_ATTRS
        .iter()
        .map(|name| (db1_hash(name), *name))
        .collect();
    let stru_hash = db1_hash("STRU");
    // ★0907 C8：电缆端点边的属性与两端 noun。
    let cable_attr_hashes: HashSet<u32> = CABLE_ENDPOINT_ATTRS
        .iter()
        .map(|name| db1_hash(name))
        .collect();
    let cable_target_hashes: HashSet<u32> = CABLE_ENDPOINT_TARGET_NOUNS
        .iter()
        .map(|name| db1_hash(name))
        .collect();
    let route_node_hashes: HashSet<u32> = CABLE_ROUTE_NODE_NOUNS
        .iter()
        .map(|name| db1_hash(name))
        .collect();
    let cwbran_hash = db1_hash("CWBRAN");
    let unit_types = resolve_delivery_unit_types(&[]);
    let mut attr_names: HashMap<u32, String> = HashMap::new();

    let attlib_path = args.template_dir.join("attlib.dat");
    let attlib = AttlibData::parse_file(&attlib_path)
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", attlib_path.display()))?;
    let template_path = template_file_for(&args.template_dir, "DESI")?;
    let mut provider = TemplateProvider::open(&template_path, None)
        .map_err(|error| anyhow::anyhow!("open {}: {error}", template_path.display()))?;
    println!(
        "schema: {} + {}",
        attlib_path.display(),
        template_path.display()
    );

    // 文件清单 + 第一遍只看 descriptor：哪些是 DESI、dbnum 是多少（跨库边要靠它分 DESI / 非 DESI）。
    let mut files = Vec::new();
    for dir in &args.dirs {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut desi_files = Vec::new();
    let mut desi_dbnums: HashSet<u32> = HashSet::new();
    let mut skipped = 0usize;
    for path in &files {
        match ReadOnlyEngine::open(path) {
            Ok(engine) if engine.descriptor().db_type_hash == desi => {
                desi_dbnums.insert(engine.descriptor().db_mark);
                desi_files.push(path.clone());
            }
            _ => skipped += 1,
        }
    }
    println!(
        "{} 个文件里 {} 个 DESI 库（{} 个 dbnum），跳过 {} 个",
        files.len(),
        desi_files.len(),
        desi_dbnums.len(),
        skipped
    );

    let mut reports: Vec<DbReport> = Vec::new();
    let mut examples: Vec<WindowExample> = Vec::new();
    let mut total_structural: BTreeMap<String, Tally> = BTreeMap::new();
    let mut total_holes: BTreeMap<String, Tally> = BTreeMap::new();
    let mut total_cable: BTreeMap<String, Tally> = BTreeMap::new();
    let mut total_all: BTreeMap<String, Tally> = BTreeMap::new();
    let mut total_topf: BTreeMap<String, TopfStat> = BTreeMap::new();
    let mut total_prim_no_root: NounOwnerCounts = BTreeMap::new();
    let mut total_windows = WindowStats::default();
    let mut total_elements = 0usize;
    let mut total_without_root = 0usize;
    let mut total_ref_edges = 0usize;
    let mut total_implicit_edges = 0usize;
    let mut total_template_missing = 0usize;
    let mut scan_failed = 0usize;

    for path in desi_files.iter().take(args.max_files) {
        let started = Instant::now();
        let scanned = match scan_full(path, &attlib, &mut provider) {
            Ok(scanned) => scanned,
            Err(error) => {
                eprintln!("{}: 全扫失败，跳过：{error}", path.display());
                scan_failed += 1;
                continue;
            }
        };
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut report = DbReport {
            dbnum: scanned.dbnum,
            file: file.clone(),
            sesno: scanned.sesno,
            elements: scanned.elements.len(),
            unreadable_records: scanned.unreadable,
            template_missing: scanned.template_missing,
            implicit_edges: scanned.implicit_edges,
            ..Default::default()
        };
        report.dbno_mismatch = scanned
            .elements
            .keys()
            .filter(|refno| refno.dbno() != scanned.dbnum)
            .count();

        let mut hier = Hierarchy::build(&scanned.elements, &noun_names, &unit_types)?;
        report.roots_delivery = hier
            .roots
            .values()
            .filter(|kind| **kind == GenerationRootKind::DeliveryUnit)
            .count();
        report.roots_normal = hier.roots.len() - report.roots_delivery;

        // ---- 逐元素：无根计数、TOPF（C5）、primitive 无根 ----
        let mut refnos: Vec<RefNo> = scanned.elements.keys().copied().collect();
        refnos.sort_by_key(|r| (r.word0, r.word1));
        for refno in &refnos {
            let Some(path) = hier.path(*refno) else {
                continue; // DBMETA 树或坏链
            };
            let chain = hier.chain(*refno).unwrap_or_default();
            let nearest = chain.first().copied();
            let noun = noun_name(&noun_names, scanned.elements[refno].noun);
            let owner_noun = path
                .get(1)
                .map(|owner| noun_name(&noun_names, scanned.elements[owner].noun))
                .unwrap_or_else(|| "-".to_string());
            if nearest.is_none() {
                report.elements_without_root += 1;
                if noun_is_primitive(&noun) {
                    let entry = report
                        .primitive_without_root
                        .entry(noun.clone())
                        .or_default();
                    entry.0 += 1;
                    *entry.1.entry(owner_noun.clone()).or_default() += 1;
                }
            }
            if listop(&hier, *refno, &path) {
                let stat = report.topf.entry(noun.clone()).or_default();
                stat.instances += 1;
                match nearest {
                    Some(root) => {
                        stat.with_root += 1;
                        let root_noun = noun_name(&noun_names, scanned.elements[&root].noun);
                        *stat.nearest_root_nouns.entry(root_noun).or_default() += 1;
                    }
                    None => {
                        stat.no_root += 1;
                        *stat.no_root_owner_nouns.entry(owner_noun).or_default() += 1;
                    }
                }
            }
        }

        // ---- 结构连接五属性的值表（回指核对用）----
        let mut structural_values: HashMap<(RefNo, u32), Vec<RefNo>> = HashMap::new();
        for edge in &scanned.edges {
            if structural_hashes.contains_key(&edge.attr) {
                structural_values
                    .entry((edge.src, edge.attr))
                    .or_default()
                    .push(edge.dst);
            }
        }
        let backref_ok = |edge: &Edge| -> Option<bool> {
            let attr = structural_hashes.get(&edge.attr)?;
            let ok = back_pointers(attr).iter().any(|back| {
                structural_values
                    .get(&(edge.dst, db1_hash(back)))
                    .is_some_and(|targets| targets.contains(&edge.src))
            });
            Some(ok)
        };

        // ---- 逐边 ----
        let mut cross_edges: Vec<CrossEdge> = Vec::new();
        for edge in &scanned.edges {
            let Some(src_chain) = hier.chain(edge.src) else {
                continue; // DBMETA 树下的引用不算
            };
            report.ref_edges += 1;
            if edge.is_uda {
                report.uda_ref_edges += 1;
            }
            let src_noun = noun_name(&noun_names, scanned.elements[&edge.src].noun);
            let attr = attr_name(&mut attr_names, edge.attr, edge.is_uda);
            let structural = structural_hashes.contains_key(&edge.attr);
            let hole = hole_nouns.contains(&scanned.elements[&edge.src].noun);
            // ★0907 C8：STAREF / ENDREF 指向同库的 EQUI / ELCONN（F），或指向 CWBRAN 之下的路由节点（E；
            // `RATTA` / `RNODE` 常常不是 CWBRAN 的直接成员，沿 owner 链找最近的 CWBRAN 祖先）。
            let cable_rule: Option<(&'static str, RefNo)> =
                if cable_attr_hashes.contains(&edge.attr) && edge.dst.dbno() == scanned.dbnum {
                    scanned.elements.get(&edge.dst).and_then(|dst_element| {
                        if cable_target_hashes.contains(&dst_element.noun) {
                            Some(("F", edge.dst))
                        } else if route_node_hashes.contains(&dst_element.noun) {
                            hier.path(edge.dst).and_then(|path| {
                                path[1..]
                                    .iter()
                                    .copied()
                                    .find(|ancestor| scanned.elements[ancestor].noun == cwbran_hash)
                                    .map(|branch| ("E", branch))
                            })
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
            let under_stru = structural
                && hier.path(edge.src).is_some_and(|path| {
                    path[1..]
                        .iter()
                        .any(|ancestor| scanned.elements[ancestor].noun == stru_hash)
                });

            let mut tally = Tally {
                edges: 1,
                src_under_stru: u64::from(under_stru),
                ..Default::default()
            };
            let dst_noun;
            if edge.dst.dbno() != scanned.dbnum {
                if desi_dbnums.contains(&edge.dst.dbno()) {
                    tally.cross_db_desi = 1;
                    dst_noun = format!("@desi{}", edge.dst.dbno());
                } else {
                    tally.cross_db_other = 1;
                    dst_noun = format!("@db{}", edge.dst.dbno());
                }
            } else if let Some(dst_element) = scanned.elements.get(&edge.dst) {
                dst_noun = noun_name(&noun_names, dst_element.noun);
                let Some(dst_chain) = hier.chain(edge.dst) else {
                    continue;
                };
                tally.same_db = 1;
                let nearest_src = src_chain.first().copied();
                let nearest_dst = dst_chain.first().copied();
                let any_common = src_chain.iter().any(|root| dst_chain.contains(root));
                let fwd_covered = nearest_dst.is_some_and(|root| src_chain.contains(&root));
                let rev_covered = nearest_src.is_some_and(|root| dst_chain.contains(&root));
                if fwd_covered {
                    tally.fwd_covered_nearest = 1;
                } else {
                    tally.fwd_uncovered_nearest = 1;
                }
                if rev_covered {
                    tally.rev_covered_nearest = 1;
                } else {
                    tally.rev_uncovered_nearest = 1;
                }
                if !any_common {
                    tally.fwd_uncovered_any = 1;
                    tally.rev_uncovered_any = 1;
                }
                if nearest_src.is_none() {
                    tally.src_no_root = 1;
                }
                if nearest_dst.is_none() {
                    tally.dst_no_root = 1;
                }
                let mut ok = false;
                if let Some(backref) = backref_ok(edge) {
                    tally.backref_checked = 1;
                    if backref {
                        tally.backref_ok = 1;
                        ok = true;
                        if !fwd_covered {
                            tally.backref_ok_fwd_uncovered_nearest = 1;
                            if under_stru {
                                tally.backref_ok_fwd_uncovered_under_stru = 1;
                            }
                        }
                    }
                }
                if (structural || hole) && (!fwd_covered || !rev_covered) {
                    cross_edges.push(CrossEdge {
                        src: edge.src,
                        attr: edge.attr,
                        dst: edge.dst,
                        structural,
                        backref_ok: ok,
                        fwd_uncovered: !fwd_covered,
                        rev_uncovered: !rev_covered,
                        cable: None,
                    });
                }
                // 电缆端点边是反向级联：被引用端变了 → 电缆要重画。电缆的根落在被引用端的根链里
                // （rev_covered）时整根重生成本来就覆盖，只有 rev 不覆盖的才进窗口统计。
                if let Some((rule, subject)) = cable_rule
                    && !rev_covered
                {
                    cross_edges.push(CrossEdge {
                        src: edge.src,
                        attr: edge.attr,
                        dst: edge.dst,
                        structural: false,
                        backref_ok: false,
                        fwd_uncovered: !fwd_covered,
                        rev_uncovered: true,
                        cable: Some(CableEdge {
                            rule,
                            subject,
                            src_root: nearest_src,
                            dst_root: nearest_dst,
                        }),
                    });
                }
            } else {
                tally.dangling = 1;
                dst_noun = "?".to_string();
            }

            let key = format!("{src_noun}.{attr}->{dst_noun}");
            if structural {
                report
                    .structural
                    .entry(key.clone())
                    .or_default()
                    .add(&tally);
            }
            if hole {
                report.holes.entry(key.clone()).or_default().add(&tally);
            }
            if let Some((rule, _)) = cable_rule {
                report
                    .cable_endpoint
                    .entry(format!("{key} [{rule}]"))
                    .or_default()
                    .add(&tally);
            }
            if tally.cross_db_other == 0 {
                report.all_edges.entry(key).or_default().add(&tally);
            }
        }

        // ---- 会话逐步 diff：跨根边上只改了一端的窗口 ----
        if args.windows && !cross_edges.is_empty() {
            match step_windows(path, &cross_edges, args.max_steps, &mut hier) {
                Ok((stats, found)) => {
                    report.windows = stats;
                    for (from_sesno, to_sesno, edge, changed_end) in found {
                        if examples.len() >= args.examples {
                            break;
                        }
                        examples.push(WindowExample {
                            dbnum: scanned.dbnum,
                            file: file.clone(),
                            from_sesno,
                            to_sesno,
                            src: edge.src.to_string(),
                            src_noun: noun_name(&noun_names, scanned.elements[&edge.src].noun),
                            attr: attr_name(&mut attr_names, edge.attr, false),
                            dst: edge.dst.to_string(),
                            dst_noun: noun_name(&noun_names, scanned.elements[&edge.dst].noun),
                            changed_end,
                            backref_ok: edge.backref_ok,
                            index_level_only: true,
                        });
                    }
                }
                Err(error) => {
                    eprintln!("{file}: 会话逐步 diff 失败：{error}");
                    report.windows.failed = 1;
                }
            }
        }

        // ---- 汇总 ----
        total_elements += report.elements;
        total_without_root += report.elements_without_root;
        total_ref_edges += report.ref_edges;
        total_implicit_edges += report.implicit_edges;
        total_template_missing += report.template_missing;
        for (key, tally) in &report.structural {
            total_structural.entry(key.clone()).or_default().add(tally);
        }
        for (key, tally) in &report.holes {
            total_holes.entry(key.clone()).or_default().add(tally);
        }
        for (key, tally) in &report.cable_endpoint {
            total_cable.entry(key.clone()).or_default().add(tally);
        }
        for (key, tally) in &report.all_edges {
            total_all.entry(key.clone()).or_default().add(tally);
        }
        for (key, stat) in &report.topf {
            total_topf.entry(key.clone()).or_default().add(stat);
        }
        for (key, (count, owners)) in &report.primitive_without_root {
            let entry = total_prim_no_root.entry(key.clone()).or_default();
            entry.0 += count;
            for (owner, n) in owners {
                *entry.1.entry(owner.clone()).or_default() += n;
            }
        }
        total_windows.add(&report.windows);

        let structural_edges: u64 = report.structural.values().map(|t| t.edges).sum();
        let structural_gap: u64 = report
            .structural
            .values()
            .map(|t| t.backref_ok_fwd_uncovered_nearest)
            .sum();
        let hole_edges: u64 = report.holes.values().map(|t| t.edges).sum();
        let hole_gap: u64 = report.holes.values().map(|t| t.rev_uncovered_nearest).sum();
        let cable_edges: u64 = report.cable_endpoint.values().map(|t| t.edges).sum();
        let cable_gap: u64 = report
            .cable_endpoint
            .values()
            .map(|t| t.rev_uncovered_nearest)
            .sum();
        println!(
            "{:<24} dbnum={:<6} ses={:<5} el={:<8} roots={:<6} 无根={:<6} 引用边={:<8} 结构边={:<6} 结构缺口={:<5} 孔洞边={:<5} 孔洞缺口={:<4} 电缆边={:<5} 电缆缺口={:<4} 窗口(结构/孔洞/电缆/电缆根级)={}/{}/{}/{} {:.1}s",
            report.file,
            report.dbnum,
            report.sesno,
            report.elements,
            report.roots_delivery + report.roots_normal,
            report.elements_without_root,
            report.ref_edges,
            structural_edges,
            structural_gap,
            hole_edges,
            hole_gap,
            cable_edges,
            cable_gap,
            report.windows.structural_src_only,
            report.windows.hole_dst_only,
            report.windows.cable_dst_only,
            report.windows.cable_dst_root_only,
            started.elapsed().as_secs_f32()
        );
        reports.push(report);
    }

    // ---- 打印合计 ----
    println!(
        "\n扫了 {} 个 DESI 库（{} 个全扫失败），{} 个元素，{} 个不在任何根之下；引用边 {}（其中隐式区 {}），\
         {} 个元素没解出模板；{:.1}s",
        reports.len(),
        scan_failed,
        total_elements,
        total_without_root,
        total_ref_edges,
        total_implicit_edges,
        total_template_missing,
        started_all.elapsed().as_secs_f32()
    );
    print_tallies("结构连接边（ADSTCH 五属性）", &total_structural);
    print_tallies("孔洞边（负实体 noun 发出的引用）", &total_holes);
    print_tallies(
        "电缆端点边（STAREF / ENDREF → EQUI / ELCONN [F]，或 → CWBRAN 之下的路由节点 [E]；只算同库）",
        &total_cable,
    );

    println!("\n全部 DESI 内引用边里正向 nearest 不覆盖最多的 25 类：");
    let mut ranked: Vec<(&String, &Tally)> = total_all.iter().collect();
    ranked.sort_by(|a, b| {
        (b.1.fwd_uncovered_nearest + b.1.cross_db_desi)
            .cmp(&(a.1.fwd_uncovered_nearest + a.1.cross_db_desi))
            .then_with(|| a.0.cmp(b.0))
    });
    println!(
        "  {:<40} {:>8} {:>8} {:>9} {:>9} {:>9} {:>9}",
        "边", "条数", "同库", "正向缺口", "缺口(any)", "反向缺口", "跨DESI库"
    );
    for (key, tally) in ranked.iter().take(25) {
        println!(
            "  {:<40} {:>8} {:>8} {:>9} {:>9} {:>9} {:>9}",
            key,
            tally.edges,
            tally.same_db,
            tally.fwd_uncovered_nearest,
            tally.fwd_uncovered_any,
            tally.rev_uncovered_nearest,
            tally.cross_db_desi
        );
    }

    println!("\nTOPF（LISTOP 判真）而一个根都不归的 noun：");
    let mut topf_ranked: Vec<(&String, &TopfStat)> = total_topf
        .iter()
        .filter(|(_, stat)| stat.no_root > 0)
        .collect();
    topf_ranked.sort_by(|a, b| b.1.no_root.cmp(&a.1.no_root).then_with(|| a.0.cmp(b.0)));
    println!(
        "  {:<8} {:>8} {:>8} {:>8}  owner noun（无根那些）",
        "noun", "实例", "有根", "无根"
    );
    for (noun, stat) in &topf_ranked {
        let owners = top_entries(&stat.no_root_owner_nouns, 4);
        println!(
            "  {:<8} {:>8} {:>8} {:>8}  {}",
            noun, stat.instances, stat.with_root, stat.no_root, owners
        );
    }
    if topf_ranked.is_empty() {
        println!("  （无）");
    }

    println!("\ncore 判 primitive 却没有根祖先的元素：");
    let mut prim_ranked: Vec<_> = total_prim_no_root.iter().collect();
    prim_ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(b.0)));
    for (noun, (count, owners)) in &prim_ranked {
        println!(
            "  {:<8} {:>8}  owner: {}",
            noun,
            count,
            top_entries(owners, 4)
        );
    }
    if prim_ranked.is_empty() {
        println!("  （无）");
    }

    println!(
        "\n会话逐步 diff：{} 个库 {} 步；跨根结构边（回指成立）持属性端单独改动 {} / 被引用端单独改动 {} / 两端都改 {}；\
         跨根孔洞边 被打孔件单独改动 {} / 孔洞件单独改动 {} / 两端都改 {}；\
         跨根电缆端点边 被引用端记录改了·电缆根没被碰 {} / 被引用端的根被碰·记录没改·电缆根没被碰 {} / 只电缆端改 {} / 两端都碰 {}\
         （另 {} 处被引用端改了但电缆那端当时还没建，边不存在，不算）；失败 {} 库",
        total_windows.dbs_stepped,
        total_windows.steps,
        total_windows.structural_src_only,
        total_windows.structural_dst_only,
        total_windows.structural_both,
        total_windows.hole_dst_only,
        total_windows.hole_src_only,
        total_windows.hole_both,
        total_windows.cable_dst_only,
        total_windows.cable_dst_root_only,
        total_windows.cable_src_only,
        total_windows.cable_both,
        total_windows.cable_dst_only_unborn,
        total_windows.failed
    );
    for example in examples.iter().take(12) {
        println!(
            "  {} {}→{} {}({}) .{} -> {}({}) 改的是 {} 端 回指={}",
            example.file,
            example.from_sesno,
            example.to_sesno,
            example.src,
            example.src_noun,
            example.attr,
            example.dst,
            example.dst_noun,
            example.changed_end,
            example.backref_ok
        );
    }

    if let Some(json) = &args.json {
        let doc = serde_json::json!({
            "dirs": args.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
            "delivery_unit_types": unit_types,
            "structural_attrs": STRUCTURAL_ATTRS,
            "cable_endpoint_attrs": CABLE_ENDPOINT_ATTRS,
            "cable_endpoint_target_nouns": CABLE_ENDPOINT_TARGET_NOUNS,
            "cable_route_node_nouns": CABLE_ROUTE_NODE_NOUNS,
            "hole_nouns_proxy": hole_nouns.iter().map(|h| noun_name(&noun_names, *h)).collect::<std::collections::BTreeSet<_>>(),
            "files_seen": files.len(),
            "desi_files": desi_files.len(),
            "desi_dbnums": desi_dbnums.len(),
            "scanned": reports.len(),
            "scan_failed": scan_failed,
            "template_dir": args.template_dir.display().to_string(),
            "elements": total_elements,
            "elements_without_root": total_without_root,
            "ref_edges": total_ref_edges,
            "implicit_ref_edges": total_implicit_edges,
            "elements_without_template": total_template_missing,
            "structural": total_structural,
            "holes": total_holes,
            "cable_endpoint": total_cable,
            "all_edges": total_all,
            "topf": total_topf,
            "primitive_without_root": total_prim_no_root,
            "windows": total_windows,
            "window_examples": examples,
            "per_db": reports,
        });
        std::fs::write(json, serde_json::to_string_pretty(&doc)?)?;
        println!("\n已写出 {}", json.display());
    }
    Ok(())
}

/// 会话链逐步 diff，数跨根边上「只改了一端」的窗口。返回 (账, 样例 (from, to, edge, 改的哪端))。
///
/// 电缆端点边（★0907 C8）多看一层「根级被碰」：一步里变更键的根链并集 = 这一步整根重生成会覆盖的根；
/// 电缆那端的根在里面就不是缺口，被引用端的根在里面（而记录本身没改）就是 GETWORK `FNDTOP` 口径下
/// core 也会触发的窗口。`hier` 只用来查根链（带缓存，故 `&mut`）。
fn step_windows(
    path: &Path,
    cross_edges: &[CrossEdge],
    max_steps: usize,
    hier: &mut Hierarchy<'_>,
) -> anyhow::Result<(WindowStats, Vec<WindowHit>)> {
    let has_cable_edges = cross_edges.iter().any(|edge| edge.cable.is_some());
    let mut engine = ReadOnlyEngine::open(path)?;
    let mut sesnos: Vec<u32> = engine
        .sessions()?
        .iter()
        .map(|session| session.session_id)
        .collect();
    sesnos.sort_unstable();
    sesnos.dedup();
    let mut stats = WindowStats {
        dbs_stepped: 1,
        ..Default::default()
    };
    let mut found = Vec::new();
    if sesnos.len() < 2 {
        return Ok((stats, found));
    }
    let steps = engine.diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)?;
    // 每个键第一次 `Inserted` 在第几步：电缆边的电缆端在那一步之前不存在，被引用端那时的改动不是窗口
    // （zdj7200 2→3 改的那几台 EQUI，电缆 4→5 才建）。链头会话里就有的键不在表里 = 一直存在。只有电缆边要看。
    let mut born_at: HashMap<RefNo, usize> = HashMap::new();
    if has_cable_edges {
        for (index, step) in steps.iter().take(max_steps).enumerate() {
            for change in step.changes() {
                if let IndexCandidate::Inserted { refno, .. } = change {
                    born_at.entry(*refno).or_insert(index);
                }
            }
        }
    }
    for (index, step) in steps.iter().take(max_steps).enumerate() {
        stats.steps += 1;
        let changed: HashSet<RefNo> = step.changes().iter().map(|change| change.refno()).collect();
        if changed.is_empty() {
            continue;
        }
        // 这一步被碰到的根：每个变更键 owner 链上的全部根（与祖先闭包同口径）。只有电缆边要看。
        let touched_roots: HashSet<RefNo> = if has_cable_edges {
            changed
                .iter()
                .filter_map(|refno| hier.chain(*refno))
                .flatten()
                .collect()
        } else {
            HashSet::new()
        };
        for edge in cross_edges {
            let src_changed = changed.contains(&edge.src);
            let dst_changed = changed.contains(&edge.dst);
            if let Some(cable) = edge.cable {
                // 被引用端：F 看 dst 自己，E 看 dst（路由节点）或它的 CWBRAN。
                let subject_changed = dst_changed || changed.contains(&cable.subject);
                let subject_root_touched = cable
                    .dst_root
                    .is_some_and(|root| touched_roots.contains(&root));
                let cable_root_touched = cable
                    .src_root
                    .is_some_and(|root| touched_roots.contains(&root));
                // 电缆那端这一步的 `to` 会话里还没建 → 边当时不存在，被引用端的改动不算窗口，单独记。
                if born_at.get(&edge.src).is_some_and(|&born| born > index) {
                    if subject_changed && !cable_root_touched {
                        stats.cable_dst_only_unborn += 1;
                    }
                    continue;
                }
                if subject_changed && !cable_root_touched {
                    stats.cable_dst_only += 1;
                    let label = if cable.rule == "E" {
                        "dst[E]"
                    } else {
                        "dst[F]"
                    };
                    found.push((step.from.sesno, step.to.sesno, *edge, label));
                } else if !subject_changed && subject_root_touched && !cable_root_touched {
                    stats.cable_dst_root_only += 1;
                    let label = if cable.rule == "E" {
                        "dst-root[E]"
                    } else {
                        "dst-root[F]"
                    };
                    found.push((step.from.sesno, step.to.sesno, *edge, label));
                } else if subject_changed && cable_root_touched {
                    stats.cable_both += 1;
                } else if src_changed && !subject_changed {
                    stats.cable_src_only += 1;
                }
                continue;
            }
            if edge.structural && edge.backref_ok {
                match (src_changed, dst_changed) {
                    (true, false) => {
                        stats.structural_src_only += 1;
                        if edge.fwd_uncovered {
                            found.push((step.from.sesno, step.to.sesno, *edge, "src"));
                        }
                    }
                    (false, true) => stats.structural_dst_only += 1,
                    (true, true) => stats.structural_both += 1,
                    _ => {}
                }
            } else if !edge.structural {
                match (src_changed, dst_changed) {
                    (false, true) => {
                        stats.hole_dst_only += 1;
                        if edge.rev_uncovered {
                            found.push((step.from.sesno, step.to.sesno, *edge, "dst"));
                        }
                    }
                    (true, false) => stats.hole_src_only += 1,
                    (true, true) => stats.hole_both += 1,
                    _ => {}
                }
            }
        }
    }
    Ok((stats, found))
}

fn print_tallies(title: &str, tallies: &BTreeMap<String, Tally>) {
    println!("\n{title}：");
    if tallies.is_empty() {
        println!("  （语料里一条都没有）");
        return;
    }
    println!(
        "  {:<40} {:>7} {:>6} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10}",
        "边",
        "条数",
        "悬空",
        "跨DESI",
        "跨其它",
        "正向缺口",
        "缺口any",
        "反向缺口",
        "回指成立",
        "回指∧缺口",
        "在STRU下",
        "严格缺口"
    );
    let mut ranked: Vec<(&String, &Tally)> = tallies.iter().collect();
    ranked.sort_by(|a, b| b.1.edges.cmp(&a.1.edges).then_with(|| a.0.cmp(b.0)));
    let mut sum = Tally::default();
    for (key, tally) in ranked {
        sum.add(tally);
        println!(
            "  {:<40} {:>7} {:>6} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10}",
            key,
            tally.edges,
            tally.dangling,
            tally.cross_db_desi,
            tally.cross_db_other,
            tally.fwd_uncovered_nearest,
            tally.fwd_uncovered_any,
            tally.rev_uncovered_nearest,
            tally.backref_ok,
            tally.backref_ok_fwd_uncovered_nearest,
            tally.src_under_stru,
            tally.backref_ok_fwd_uncovered_under_stru
        );
    }
    println!(
        "  {:<40} {:>7} {:>6} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>10}",
        "合计",
        sum.edges,
        sum.dangling,
        sum.cross_db_desi,
        sum.cross_db_other,
        sum.fwd_uncovered_nearest,
        sum.fwd_uncovered_any,
        sum.rev_uncovered_nearest,
        sum.backref_ok,
        sum.backref_ok_fwd_uncovered_nearest,
        sum.src_under_stru,
        sum.backref_ok_fwd_uncovered_under_stru
    );
}

fn top_entries(map: &BTreeMap<String, u64>, n: usize) -> String {
    let mut entries: Vec<(&String, &u64)> = map.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    entries
        .iter()
        .take(n)
        .map(|(k, v)| format!("{k}×{v}"))
        .collect::<Vec<_>>()
        .join(" ")
}
