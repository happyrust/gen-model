//! B8.5 第 2 条（`docs/plans/2026-09-04-legacy-parser-retirement-endgame-plan.md`）：
//! 基线退化窗口在真实 db8000 会话链上的离线夹具回归。
//!
//! 钉的是换底座之后基线路**独有**的那几件事——逐元素的解码本身与增量路共用
//! `read_element`（源码钉 `the_baseline_and_the_net_window_share_one_enumeration` 已经
//! 守住），这里守的是它外面那层壳：
//!
//! 1. **流出来的元素集 = 目标会话的活索引**：一个幽灵都不许有（issue #10 的补链轮曾把
//!    已删子树复活，`member_prune` 因此存在；e3d-io 的目标索引在 T 上根本没有它们），
//!    一个也不许漏。正反两面分别由 `snapshot_presence` 与 issue-019 台账裁决。
//! 2. **分块不吞不重**：块上限生效、块数与 `BaselineStats::chunks` 相符、拼回去正好一份。
//! 3. **`pe.sesno` 是真会话号**（F7）：legacy 基线给链头会话的元素记 0，新口径记终稿记录
//!    所在页反查出来的 last-touch 会话，与增量路的 `Add` 同一条。
//! 4. **两个公开读口不分家**：`stream_baseline` 与单元素 API `read_element_at` 对同一个
//!    refno 给出逐字段相同的 `EleData`。
//!
//! 夹具是 issue-019 的真实 db8000 归档（sesno 24/25/26），与 `db8000_two_delete_fixture`
//! 共用一份。模板与 attlib 取自 `AIOS_E3D_TEMPLATE_DIR`（未设时用本机默认目录），
//! 与 `db8000_session_pairs` 同一条依赖。
//!
//! 跑法：`cargo test --locked --test baseline_stream_8000 -- --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod archive;

use aios_core::RefnoEnum;
use aios_core::pdms_types::RefU64;
use aios_database::data_interface::db_file_header::snapshot_presence;
use aios_database::data_interface::e3d_collector::{read_element_at, stream_baseline};
use aios_database::data_interface::ele_data::EleData;
use aios_database::data_interface::frozen_source::FrozenSource;
use archive::verify_and_extract;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;

/// 最终快照（sesno 26）在台账里的角色名。
const FINAL_ROLE: &str = "parent_deleted";

/// 故意取一个远小于库元素数的块：块边界要真的被跨过好几次，第 2 条断言才有内容。
const CHUNK_SIZE: usize = 64;

/// `read_element_at` 每调一次就重开一遍文件与模板，对整库逐个调是分钟级的。
/// 逐字段对拍取一个跨越全键序的样本；「每个元素都解得出、解出来是什么」由
/// 上面三条 + 共用 `read_element` 的源码钉负责。
const CROSS_READ_SAMPLES: usize = 24;

fn fixture_root() -> PathBuf {
    std::env::var_os("AIOS_DB8000_TWO_DELETE_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/issues/issue-019-cross-session-parent-child-delete")
        })
}

/// 一个元素的可比视图。`EleData` 没有 `PartialEq`，而且断言失败时要看得出差在哪一格，
/// 所以摊平成有序映射再比。
#[derive(Debug, PartialEq, Eq)]
struct ElementView {
    refno: String,
    owner: String,
    noun: u32,
    name: String,
    children: Vec<String>,
    implicit: BTreeMap<String, String>,
    explicit: BTreeMap<String, String>,
    uda: Vec<String>,
}

fn pdms_str(refno: RefU64) -> String {
    RefnoEnum::from(refno).to_pdms_str()
}

fn attrs(map: &aios_core::NamedAttrMap) -> BTreeMap<String, String> {
    match serde_json::to_value(map).expect("att map to json") {
        serde_json::Value::Object(fields) => fields
            .into_iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect(),
        other => BTreeMap::from([("<non-object>".to_owned(), other.to_string())]),
    }
}

fn view(ele: &EleData) -> ElementView {
    ElementView {
        refno: pdms_str(ele.refno),
        owner: pdms_str(ele.owner),
        noun: ele.noun,
        name: ele.name.clone(),
        children: ele.children.0.iter().copied().map(pdms_str).collect(),
        implicit: attrs(ele.att_map()),
        explicit: attrs(ele.explicit_attmap()),
        uda: ele.uda_atts().iter().map(|att| format!("{att:?}")).collect(),
    }
}

/// 一次基线流的全部产出，摊平给下面几条断言共用。
struct StreamedBaseline {
    _extracted: archive::ExtractedFixture,
    frozen: FrozenSource,
    stats: aios_database::data_interface::e3d_collector::BaselineStats,
    /// 键序（refno 升序）保持 `stream_baseline` 交出来的顺序。
    elements: Vec<(u32, EleData)>,
    /// 每一块交出来多少个元素。
    chunk_sizes: Vec<usize>,
}

fn stream() -> StreamedBaseline {
    let extracted = verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let final_path = extracted
        .path_for_role(FINAL_ROLE)
        .expect("issue-019 final snapshot");
    let frozen = FrozenSource::capture(&final_path).expect("freeze the db8000 final snapshot");
    assert_eq!(
        frozen.dbnum() as u32,
        extracted.manifest.dbnum,
        "夹具必须是台账声明的那个 dbnum"
    );
    assert_eq!(
        frozen.target_sesno(),
        extracted.manifest.window.end_sesno,
        "冻结目标会话必须是台账窗口末会话"
    );

    let mut elements = Vec::new();
    let mut chunk_sizes = Vec::new();
    let stats = stream_baseline(&frozen, frozen.db_type(), CHUNK_SIZE, |chunk| {
        assert!(
            chunk.elements.len() <= CHUNK_SIZE,
            "块上限失效：一块交了 {} 个元素",
            chunk.elements.len()
        );
        chunk_sizes.push(chunk.elements.len());
        elements.extend(chunk.elements);
        Ok(())
    })
    .expect("stream the db8000 baseline");

    println!("{stats:?}");
    StreamedBaseline {
        _extracted: extracted,
        frozen,
        stats,
        elements,
        chunk_sizes,
    }
}

/// 断言 1 正面：流出来的每一个 refno 都真的在目标会话的活索引里（没有幽灵）。
///
/// `snapshot_presence` 是独立于收集器的另一条读法（`ReadOnlyEngine::find_element`
/// 逐个查活索引并核记录头 refno），所以它有资格给「这个 refno 在不在」下判决。
#[test]
fn every_streamed_element_is_live_in_the_target_index() {
    let streamed = stream();
    assert!(
        !streamed.elements.is_empty(),
        "夹具一个元素都没流出来，下面的断言全是空的"
    );

    let refnos: Vec<RefU64> = streamed.elements.iter().map(|(_, ele)| ele.refno).collect();
    let presence = snapshot_presence(streamed.frozen.path(), &refnos).expect("open final snapshot");
    assert_eq!(
        presence.sesno,
        streamed.frozen.target_sesno(),
        "存在性探针必须坐在同一个会话上"
    );

    let ghosts: Vec<String> = refnos
        .iter()
        .filter(|refno| !presence.contains(refno))
        .map(|refno| pdms_str(*refno))
        .collect();
    assert!(
        ghosts.is_empty(),
        "这些 refno 进了基线流却不在目标会话的活索引里（幽灵）：{ghosts:#?}"
    );
}

/// 断言 1 反面：issue-019 台账在 sesno 26 上声明存在的元素一个不少，声明已删的一个不多。
///
/// 台账是 E3D TTY 录制时逐会话记下来的期望（共识 d-6：判据只认 TTY），不是第二套解析器。
#[test]
fn the_recorded_ledger_for_the_final_session_matches_the_stream() {
    let extracted = verify_and_extract(&fixture_root()).expect("verify issue-019 fixture");
    let spec = extracted
        .manifest
        .snapshots
        .iter()
        .find(|snapshot| snapshot.role == FINAL_ROLE)
        .expect("台账里缺最终快照")
        .clone();

    let streamed = stream();
    let seen: BTreeSet<RefU64> = streamed.elements.iter().map(|(_, ele)| ele.refno).collect();

    let mut checked = 0usize;
    for element in &spec.elements {
        let refno = RefU64::from_str(&element.refno)
            .unwrap_or_else(|error| panic!("台账 refno {} 解不出来: {error}", element.refno));
        assert_eq!(
            seen.contains(&refno),
            element.present,
            "sesno {} 上 {}（{}）的存在性与台账不符",
            spec.sesno,
            element.refno,
            element.noun
        );
        checked += 1;
    }
    assert!(checked >= 3, "台账探针只跑了 {checked} 条");
}

/// 断言 2：分块不吞不重——块数与统计相符，元素总数 = 各块之和 = 去重后的个数，
/// 且除最后一块外每块都装满。
#[test]
fn chunking_neither_drops_nor_duplicates_an_element() {
    let streamed = stream();
    let total: usize = streamed.chunk_sizes.iter().sum();

    assert_eq!(
        streamed.chunk_sizes.len(),
        streamed.stats.chunks,
        "sink 被调的次数必须等于 BaselineStats::chunks"
    );
    assert_eq!(
        total,
        streamed.elements.len(),
        "各块之和必须等于流出来的元素总数"
    );
    assert_eq!(
        streamed.stats.elements + streamed.stats.worlds,
        total,
        "统计的元素数（含 WORL）必须等于交给 sink 的个数"
    );
    assert_eq!(
        streamed.stats.elements + streamed.stats.worlds + streamed.stats.ignored,
        streamed.stats.indexed,
        "索引条目 = 交出去的 + 白名单跳过的，没有第三个去处"
    );

    let unique: BTreeSet<RefU64> = streamed.elements.iter().map(|(_, ele)| ele.refno).collect();
    assert_eq!(unique.len(), total, "同一个 refno 被交了两次");

    assert!(
        streamed.chunk_sizes.len() > 1,
        "块大小 {CHUNK_SIZE} 没能把这个库切成多块（共 {total} 个元素），\
         这条断言等于没跑——把 CHUNK_SIZE 调小"
    );
    let (last, full) = streamed.chunk_sizes.split_last().expect("上面已断言非空");
    assert!(
        full.iter().all(|size| *size == CHUNK_SIZE),
        "除最后一块外每块都该装满 {CHUNK_SIZE}：{:?}",
        streamed.chunk_sizes
    );
    assert!(*last <= CHUNK_SIZE && *last > 0);
}

/// 断言 3（F7）：`pe.sesno` 写的是真会话号。
///
/// legacy 基线的 `legacy_session_ranges` 把链头会话做成 `0xFFFFFFFF..pgno` 的空区间，
/// 落在那些页上的元素 sesno 记 **0**；新口径是终稿记录所在页反查出来的 last-touch 会话，
/// 与增量路的 `Add` 同一条。零会话号从此不该再出现。
#[test]
fn every_element_carries_a_real_session_number() {
    let streamed = stream();
    let target = streamed.frozen.target_sesno();

    let zeros: Vec<String> = streamed
        .elements
        .iter()
        .filter(|(sesno, _)| *sesno == 0)
        .map(|(_, ele)| pdms_str(ele.refno))
        .collect();
    assert!(
        zeros.is_empty(),
        "这些元素拿到了 legacy 的链头 0 会话号（F7 的口径没换过来）：{zeros:#?}"
    );

    let beyond: Vec<String> = streamed
        .elements
        .iter()
        .filter(|(sesno, _)| *sesno > target)
        .map(|(sesno, ele)| format!("{}@{sesno}", pdms_str(ele.refno)))
        .collect();
    assert!(
        beyond.is_empty(),
        "冻结目标是 {target}，这些元素却拿到了更晚的会话号：{beyond:#?}"
    );

    let distinct: BTreeSet<u32> = streamed.elements.iter().map(|(sesno, _)| *sesno).collect();
    println!("sesno 分布: {distinct:?}");
}

/// 断言 4：`stream_baseline` 与单元素公开读口 `read_element_at` 对同一个 refno 逐字段同。
///
/// 取样而不是全量：`read_element_at` 每次都重开文件与模板（它是给取证脚本用的一次性读口），
/// 对整库逐个调是分钟级。样本按等距抽，跨越整个键序与全部块边界。
#[test]
fn the_single_element_read_api_agrees_with_the_baseline_stream() {
    let streamed = stream();
    let total = streamed.elements.len();
    let step = total.div_ceil(CROSS_READ_SAMPLES).max(1);

    let mut compared = 0usize;
    for (_, ele) in streamed.elements.iter().step_by(step) {
        let direct = read_element_at(&streamed.frozen, ele.refno)
            .unwrap_or_else(|error| panic!("单读 {} 失败: {error:#}", pdms_str(ele.refno)));
        assert_eq!(
            view(&direct),
            view(ele),
            "{} 在两个读口上不一致",
            pdms_str(ele.refno)
        );
        compared += 1;
    }
    assert!(
        compared >= 2,
        "只对拍了 {compared} 个元素（共 {total} 个），样本太小"
    );
}
