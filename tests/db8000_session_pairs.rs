//! 阶段三：`aios-session-fixture-v1` 夹具的离线回归
//! （docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md §3）。
//!
//! 七类性质断言，**数据驱动**——对 manifest 里的每个案例跑，不硬编码任何 sesno
//! 或 refno。夹具来源两条：
//!
//! 1. 环境变量 `AIOS_SESSION_FIXTURE` 指向现成夹具目录（阶段二录出真实
//!    db8000 会话链后走这条）；
//! 2. 缺省在临时目录里从 issue-019 的 final 现场 `pack` 一份合规夹具。
//!
//! 第 2 条不是权宜：阶段一自检已证明 pack 从 issue-019 的 final 切出的台账散列
//! 与当年独立录制的逐字节相等，所以它是**真实 db8000 会话链**上的真数据，只是
//! 案例集小。真实录制到货后，把环境变量一指，同一批断言原样复用——不改测试代码
//! 就是这套设计成立与否的判据。
//!
//! **覆盖面的诚实说明**：issue-019 的两个案例都是无 restore 腿的删除，所以净变化
//! 只覆盖到 `deleted`。`cancelled`（add+restore）与 `modified`（data/transform）
//! 要等阶段二录制到货。本文件交付的是机器与删除档覆盖，不是全形态回归。
//!
//! 跑法（与 CI 逐字一致）：
//! `cargo test --locked --test db8000_session_pairs --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture`

#[path = "../src/bin/db8000_two_delete_fixture/archive.rs"]
#[allow(dead_code)]
mod issue019;

#[path = "../src/bin/db_session_fixture/format.rs"]
#[allow(dead_code)]
mod format;

#[path = "../src/bin/db_session_fixture/archive_util.rs"]
#[allow(dead_code)]
mod archive_util;

#[path = "../src/bin/db_session_fixture/session_cut.rs"]
#[allow(dead_code)]
mod session_cut;

// 与 bin 根同名声明：`pipeline` 内部的 `crate::format` 等路径在两个 crate 里都
// 解析得到，测试跑的是 bin 那一份实现本体。
#[path = "../src/bin/db_session_fixture/pipeline.rs"]
#[allow(dead_code)]
mod pipeline;

#[path = "common/issue019_recording.rs"]
#[allow(dead_code)]
mod issue019_recording;

use aios_core::RefnoEnum;
use aios_core::pdms_types::RefU64;
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::manual_update::{NetOp, merge_net_changes};
use format::{CaseSpec, SessionFixtureManifest};
use parse_pdms_db::paged::PagedDbSession;
use pdms_io::io::{EleOperationData, EleOperationDetail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;

/// 差分 oracle 的噪声属性白名单。
///
/// **目前是空的**，这是实测结论不是偷懒：在 issue-019 的真实会话链上，
/// 24→25→26 两个边界上受影响元素的 `att_map()` 差异为 **0 条**——删除的信号
/// 全在「存在性」与「children 列表」上（父件 children 1→0、祖父 26→25）。
/// 方案 §6 预判的 CACHID 类派生属性漂移没有出现。
///
/// 留着这个常量是因为它是**机制**：真实录制的 data/transform 案例若引入噪声，
/// 加在这里即可，不必改 oracle 结构。加条目时请连同「在哪个夹具上观察到」一起注释。
const NOISY_ATTRS: &[&str] = &[];

/// 环境变量：指向现成的 `aios-session-fixture-v1` 夹具目录。
const FIXTURE_ENV: &str = "AIOS_SESSION_FIXTURE";

/// 环境变量：把合成夹具落在指定目录而不是临时目录。
///
/// CI 用它把 `manifest.json` / `SHA256SUMS` 留在工作区，失败时当 artifact 传出来
/// ——远程红了以后光看断言文本还原不出「当时那份台账长什么样」，而台账正是
/// 历史还原对账的对照物。本地不设则一切都在临时目录，跑完不留痕。
const KEEP_ENV: &str = "AIOS_SESSION_FIXTURE_KEEP";

// ── 夹具装载 ────────────────────────────────────────────────────────────────

struct SessionPairsFixture {
    root: PathBuf,
    manifest: SessionFixtureManifest,
    final_path: PathBuf,
    final_bytes: Vec<u8>,
    chain: session_cut::SessionChain,
    /// 现切快照的落脚处，也持有解出的最终文件。
    scratch: TempDir,
    /// 合成夹具时持有它的临时目录（外部夹具时为 None）。
    _packed: Option<TempDir>,
    /// issue-019 解压产物；合成路径下 pack 读它当源。
    _issue019: Option<issue019::ExtractedFixture>,
}

impl SessionPairsFixture {
    fn load() -> Self {
        let (root, packed, extracted) = match std::env::var_os(FIXTURE_ENV) {
            Some(path) => (PathBuf::from(path), None, None),
            None => {
                let extracted =
                    issue019::verify_and_extract(&issue019_recording::issue019_fixture_root())
                        .expect("verify issue-019 fixture");
                let source = extracted
                    .path_for_role("parent_deleted")
                    .expect("issue-019 final snapshot");
                let work = tempfile::Builder::new()
                    .prefix("aios-session-pairs-fixture-")
                    .tempdir()
                    .expect("tempdir");
                let recording = work.path().join("recording.json");
                fs::write(&recording, issue019_recording::issue019_recording())
                    .expect("write synthesized recording");
                // 落盘目录由 KEEP_ENV 决定：给了就留在工作区（CI 传 artifact 用），
                // 没给就留在临时目录里，跑完自动清。
                let (out, keeper) = match std::env::var_os(KEEP_ENV) {
                    Some(dir) => (PathBuf::from(dir), None),
                    None => (work.path().join("issue-019-as-session-fixture"), Some(work)),
                };
                // force=true 只对夹具目录/空目录生效（pipeline 自己把着这道闸），
                // 所以重复跑 CI 不会误删别的东西。
                pipeline::pack(Some(&source), &recording, &out, Some(8000), true)
                    .expect("pack a verifiable fixture from the issue-019 final");
                (out, keeper, Some(extracted))
            }
        };

        let manifest: SessionFixtureManifest = serde_json::from_slice(
            &fs::read(root.join("manifest.json")).expect("read manifest.json"),
        )
        .expect("parse manifest.json");

        let scratch = tempfile::Builder::new()
            .prefix("aios-session-pairs-cuts-")
            .tempdir()
            .expect("tempdir");
        let final_path = scratch.path().join("final_db");
        archive_util::extract_single_declared_file(
            &root.join(&manifest.archive.path),
            &manifest.final_snapshot.path,
            manifest.final_snapshot.bytes,
            &manifest.final_snapshot.sha256,
            &final_path,
        )
        .expect("extract the declared final snapshot");
        let final_bytes = fs::read(&final_path).expect("read final snapshot");
        let chain = session_cut::session_chain(&final_bytes).expect("walk session chain");

        Self {
            root,
            manifest,
            final_path,
            final_bytes,
            chain,
            scratch,
            _packed: packed,
            _issue019: extracted,
        }
    }

    /// 从最终文件现切某个 sesno（已切过就复用），返回快照路径。
    fn cut(&self, sesno: u32) -> PathBuf {
        let path = self.scratch.path().join(format!("sesno-{sesno:03}"));
        if !path.exists() {
            session_cut::write_snapshot(
                &self.final_bytes,
                self.chain
                    .cut_for(sesno)
                    .unwrap_or_else(|error| panic!("会话链里没有 sesno={sesno}: {error:#}")),
                &path,
            )
            .unwrap_or_else(|error| panic!("切 sesno={sesno} 失败: {error:#}"));
        }
        path
    }

    fn collect_from(&self, path: &Path, start: u32, end: u32) -> Window {
        IncrementPipeline::collect_changes(path, start as i32..=end as i32)
            .unwrap_or_else(|error| panic!("采集 {start}..={end} 失败: {error:#}"))
    }

    /// 从最终文件采集一个窗口（回归的主视角）。
    fn collect(&self, start: u32, end: u32) -> Window {
        self.collect_from(&self.final_path, start, end)
    }

    fn cases(&self) -> &[CaseSpec] {
        &self.manifest.cases
    }
}

type Window = BTreeMap<u32, Vec<EleOperationData>>;

/// 整个测试二进制共用一份夹具：pack 与解压都不便宜，而所有断言都只读。
fn fixture() -> &'static SessionPairsFixture {
    static FIXTURE: OnceLock<SessionPairsFixture> = OnceLock::new();
    FIXTURE.get_or_init(SessionPairsFixture::load)
}

/// 案例窗口：`[apply-1]` 是变更前时点，`apply..=end` 是窗口本身。
struct CaseWindow {
    id: String,
    before: u32,
    apply: u32,
    end: u32,
}

fn case_window(case: &CaseSpec) -> CaseWindow {
    CaseWindow {
        id: case.id.clone(),
        before: case.apply_sesno - 1,
        apply: case.apply_sesno,
        end: case.restore_sesno.unwrap_or(case.apply_sesno),
    }
}

// ── 共用的比较口径 ──────────────────────────────────────────────────────────

/// 操作签名：跨「从哪份文件采集」比较窗口内容时的规范形态。
fn operation_signatures(window: &Window) -> Vec<String> {
    let mut signatures: Vec<String> = window
        .iter()
        .flat_map(|(sesno, operations)| {
            operations.iter().map(move |operation| {
                let detail = match &operation.detail {
                    EleOperationDetail::Add(element) => format!("Add:{}", element.noun),
                    EleOperationDetail::Deleted => "Deleted".to_owned(),
                    EleOperationDetail::Modified(modified) => format!(
                        "Modified:{}:children={}",
                        modified.noun,
                        modified.children_changed.is_some()
                    ),
                    EleOperationDetail::None => "None".to_owned(),
                };
                format!("{sesno}:{}:{detail}", operation.refno)
            })
        })
        .collect();
    signatures.sort_unstable();
    signatures
}

fn pdms_str(refno: RefU64) -> String {
    RefnoEnum::from(refno).to_pdms_str()
}

/// 台账里的 refno 文本（`a/b` 或 `a_b`）归一到 `merge_net_changes` 的输出形态。
fn normalize(text: &str) -> String {
    pdms_str(format::parse_refno(text).unwrap_or_else(|error| panic!("非法 refno {text}: {error:#}")))
}

// ── 快照差分 oracle（性质 f）──────────────────────────────────────────────

/// 一个元素在某个快照上的可比较视图。
#[derive(Debug, PartialEq, Eq)]
struct ElementView {
    noun: u32,
    name: String,
    owner: String,
    children: Vec<String>,
    attrs: BTreeMap<String, String>,
}

fn element_view(raw: &[u8]) -> ElementView {
    let ele = parse_pdms_db::parse::parse_raw_ele_data(raw).expect("parse raw element record");
    let attrs = match serde_json::to_value(ele.att_map()).expect("att_map to json") {
        serde_json::Value::Object(map) => map
            .into_iter()
            .filter(|(key, _)| !NOISY_ATTRS.contains(&key.as_str()))
            .map(|(key, value)| (key, value.to_string()))
            .collect(),
        other => BTreeMap::from([("<non-object>".to_owned(), other.to_string())]),
    };
    ElementView {
        noun: ele.noun,
        name: ele.name.clone(),
        owner: pdms_str(ele.owner),
        children: ele.children.0.iter().copied().map(pdms_str).collect(),
        attrs,
    }
}

/// 文件层面观察到的变化类别。
#[derive(Debug, PartialEq, Eq)]
enum FileChange {
    /// 两个时点都不存在。
    Absent,
    Added,
    Deleted,
    Modified,
    /// 两个时点都存在且内容逐字段相同。
    Unchanged,
}

fn read_views(snapshot: &Path, refnos: &[RefU64]) -> BTreeMap<String, ElementView> {
    if refnos.is_empty() {
        return BTreeMap::new();
    }
    let mut db = PagedDbSession::open(snapshot)
        .unwrap_or_else(|error| panic!("打开 {}: {error:#}", snapshot.display()));
    let raw = db.read_raw_records(refnos).expect("read raw records");
    raw.into_iter()
        .map(|(refno, bytes)| (pdms_str(refno), element_view(&bytes)))
        .collect()
}

fn classify(
    before: &BTreeMap<String, ElementView>,
    after: &BTreeMap<String, ElementView>,
    refno: &str,
) -> FileChange {
    match (before.get(refno), after.get(refno)) {
        (None, None) => FileChange::Absent,
        (None, Some(_)) => FileChange::Added,
        (Some(_), None) => FileChange::Deleted,
        (Some(old), Some(new)) => {
            if old == new {
                FileChange::Unchanged
            } else {
                FileChange::Modified
            }
        }
    }
}

/// 净变化在文件上应当长什么样。`Cancelled` 是「窗口内加了又删」，两端都该看不到
/// 它——但为了不把「删了又加回原样」误判成失败，也接受两端内容相同。
fn acceptable_for(net: NetOp) -> Vec<FileChange> {
    match net {
        NetOp::Added => vec![FileChange::Added],
        NetOp::Deleted => vec![FileChange::Deleted],
        NetOp::Modified => vec![FileChange::Modified],
        NetOp::Cancelled => vec![FileChange::Absent, FileChange::Unchanged],
    }
}

// ── 性质 a) + g)：档案完整性与历史还原 ──────────────────────────────────────

/// `pipeline::verify_fixture` 已经在做「zip 尺寸/SHA256 对账 → 受控解出最终文件
/// → 逐台账 sesno 现切 → 大小/散列与台账相等 → sesno + 存在性验证闸」，
/// 也就是性质 a) 与 g) 的全部内容。回归这里直接调它，不重写一遍裁决——
/// 重写就会有两套口径，而它们必须永远一致。
#[test]
fn archive_and_history_ledger_verify() {
    let fixture = fixture();
    let report = pipeline::verify_fixture(&fixture.root)
        .unwrap_or_else(|error| panic!("夹具复验失败: {error:#}"));

    assert_eq!(report.dbnum, fixture.manifest.dbnum);
    assert_eq!(report.final_sesno, fixture.manifest.final_snapshot.sesno);
    assert_eq!(report.cases, fixture.manifest.cases.len());
    assert_eq!(
        report.ledger_len,
        fixture.manifest.session_snapshots.len(),
        "台账条数应与 manifest 一致"
    );
    assert!(report.probes_checked > 0, "复验必须真的跑过存在性探针");
}

/// 反空转：夹具至少要有一个案例，且案例窗口都落在最终文件的会话链上。
/// 否则下面六条断言会「无案例可跑」地全绿。
#[test]
fn fixture_declares_usable_cases() {
    let fixture = fixture();
    assert!(
        !fixture.cases().is_empty(),
        "夹具 {} 没有任何案例",
        fixture.root.display()
    );
    for case in fixture.cases() {
        let window = case_window(case);
        for sesno in [window.before, window.apply, window.end] {
            assert!(
                fixture.chain.contains(sesno),
                "案例 {} 需要的 sesno={sesno} 不在最终文件的会话链里",
                window.id
            );
        }
    }
}

// ── 性质 b)：窗口切片 ───────────────────────────────────────────────────────

#[test]
fn window_slices_stay_inside_their_declared_sessions() {
    let fixture = fixture();
    for case in fixture.cases() {
        let window = case_window(case);
        let collected = fixture.collect(window.apply, window.end);
        assert!(
            !collected.is_empty(),
            "案例 {} 的窗口 {}..={} 一条变更都没采到——窗口声明与录制对不上",
            window.id,
            window.apply,
            window.end
        );
        for (sesno, operations) in &collected {
            assert!(
                (window.apply..=window.end).contains(sesno),
                "案例 {} 采到了窗口外的会话 {sesno}",
                window.id
            );
            assert!(
                operations.iter().all(|operation| operation.sesno == *sesno),
                "案例 {} 的 sesno={sesno} 分区里混进了别的会话: {operations:?}",
                window.id
            );
        }
    }
}

// ── 性质 c)：时点一致性 ─────────────────────────────────────────────────────

/// 从最终文件采集某个历史会话，与在该会话的切割快照上直接采集，必须逐条相同。
/// 这是「后续会话不得改写历史」的直接检验——append-only 假设一旦被破坏
/// （有人跑了 MERGE/PURGE），这条最先红。
#[test]
fn history_from_final_matches_point_in_time_snapshots() {
    let fixture = fixture();
    let mut compared = 0usize;
    for case in fixture.cases() {
        let window = case_window(case);
        for sesno in window.apply..=window.end {
            let from_final = fixture.collect(sesno, sesno);
            let cut = fixture.cut(sesno);
            let from_cut = fixture.collect_from(&cut, sesno, sesno);
            assert_eq!(
                operation_signatures(&from_final),
                operation_signatures(&from_cut),
                "案例 {} 的会话 {sesno}：最终文件与时点快照采集结果不一致",
                window.id
            );
            compared += 1;
        }
    }
    assert!(compared > 0, "没有比较过任何时点");
}

// ── 性质 d)：并集律 ─────────────────────────────────────────────────────────

#[test]
fn combined_window_equals_the_union_of_its_session_slices() {
    let fixture = fixture();
    for case in fixture.cases() {
        let window = case_window(case);
        let combined = operation_signatures(&fixture.collect(window.apply, window.end));

        let mut union: Vec<String> = (window.apply..=window.end)
            .flat_map(|sesno| operation_signatures(&fixture.collect(sesno, sesno)))
            .collect();
        union.sort_unstable();

        assert_eq!(
            combined, union,
            "案例 {} 的组合窗口与逐会话切片之并不等",
            window.id
        );
    }
}

// ── 性质 e)：净变化折叠 ─────────────────────────────────────────────────────

#[test]
fn net_folding_matches_the_declared_expectations() {
    let fixture = fixture();
    let mut asserted = 0usize;
    for case in fixture.cases() {
        let Some(expected) = case.expected.as_ref().filter(|e| !e.net_window.is_empty()) else {
            continue;
        };
        let window = case_window(case);
        let folded: BTreeMap<String, NetOp> =
            merge_net_changes(&fixture.collect(window.apply, window.end))
                .into_iter()
                .map(|change| (change.refno, change.net))
                .collect();

        for expectation in &expected.net_window {
            let refno = normalize(&expectation.refno);
            let actual = folded.get(&refno).unwrap_or_else(|| {
                panic!(
                    "案例 {} 声明 {refno} 的净变化为 {}，但折叠结果里根本没有它：{folded:?}",
                    window.id, expectation.net
                )
            });
            // `NetOp` 的 serde 是 snake_case，与台账取值同一套词表（format::NET_OPS）。
            let actual_text = serde_json::to_value(actual)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("{actual:?}"));
            assert_eq!(
                actual_text, expectation.net,
                "案例 {} 的 {refno} 净变化与台账不符",
                window.id
            );
            asserted += 1;
        }
    }
    assert!(
        asserted > 0,
        "夹具里没有任何 expected.net_window——净折叠这条断言空转了"
    );
}

// ── 性质 f)：快照差分对账 ───────────────────────────────────────────────────

/// 增量流与文件真实状态互证：把窗口净结果里的每个 refno 拿到 before/after 两份
/// 现切快照上逐字段比对，类别必须对得上。
///
/// 这条是**通用 oracle**——新案例不必手写完整期望，也能获得「增量说变了、文件
/// 就得真的变了」这层基础保障。比对口径：存在性 + noun/name/owner + children
/// 列表 + 属性表（减去 [`NOISY_ATTRS`]）。
///
/// children 必须进比对：实测 issue-019 的删除序列里，父件与祖父的属性表**一个
/// 字节都没动**，Modified 的信号全在 children 列表上（父件 1→0、祖父 26→25）。
/// 只比属性的话，这两个元素会被误判成「增量说变了但文件没变」。
///
/// 范围说明：`read_raw_records` 按显式 refno 清单读，所以只能核对「已知的
/// refno」（净结果 + 案例声明的元素），做不到全库枚举。反方向（文件变了但增量
/// 没报）不在本 oracle 覆盖内。
#[test]
fn snapshot_diff_corroborates_the_net_result() {
    let fixture = fixture();
    let mut checked = 0usize;

    for case in fixture.cases() {
        let window = case_window(case);
        let folded = merge_net_changes(&fixture.collect(window.apply, window.end));

        // 净结果里的 refno + 案例自己声明的元素，一起作为探针清单。
        let mut wanted: BTreeSet<String> =
            folded.iter().map(|change| change.refno.clone()).collect();
        for element in &case.elements {
            wanted.insert(normalize(&element.refno));
        }
        let refnos: Vec<RefU64> = wanted
            .iter()
            .map(|text| format::parse_refno(text).expect("normalized refno"))
            .collect();

        let before = read_views(&fixture.cut(window.before), &refnos);
        let after = read_views(&fixture.cut(window.end), &refnos);

        for change in &folded {
            let observed = classify(&before, &after, &change.refno);
            let acceptable = acceptable_for(change.net);
            assert!(
                acceptable.contains(&observed),
                "案例 {}：{} 的净变化是 {:?}，但 sesno {}→{} 的文件差分是 {observed:?}\
                 （可接受：{acceptable:?}）",
                window.id,
                change.refno,
                change.net,
                window.before,
                window.end
            );
            checked += 1;
        }
    }

    assert!(checked > 0, "差分 oracle 一个 refno 都没核对到");
}
