//! P1 对拍尺子：legacy(pdms_io) ↔ v2(pdmsdb_engine_v2) 只读读取对拍探针。
//!
//! 出处：`docs/plans/2026-08-30-old-pdms-io-core-dll-read-gap.md` §P1。
//! 对同一 dabacon 文件、同一 target sesno，逐项对比：
//!   1. 会话链（sesno 集合、每会话页号与 index_root）；
//!   2. 索引叶条目口径：在 **v2 认定的活树页面** 上逐页双口径解码——
//!      v2 free_dwords 反推条目数 vs 旧栈零终止扫描。多读＝每页尾部的陈旧槽位
//!      （D3-1），少读＝零终止提前停或整页解码失败；同页前缀必须逐字相等，
//!      不等即步长/宽度错位（D3-2）。内部页同法对比子指针集合。
//!   3. 点查抽样（每库默认 1000 refno，幽灵键优先）：绝对字节位置全等；
//!   4. 页头声明统计：键/值宽非 2+2 的页数（D3-2）、`page_size(0x34)` ≠ 512 字
//!      的文件数（D1-1）、多 extent 文件（D1-4）。
//! 顺带产出 V2 验证项的 flag 直方图（按点查可达/不可达分层，D3-5）。
//!
//! 另有一个**观察项**：旧栈式自由全树走查（从根顺着零终止读出的子指针一路下降，
//! 无 visited 集、层级守卫与读失败跳过均镜像 `filter_index_data`）。它会钻进
//! 陈旧子树，页数与重复键计数本身就是「零终止口径读进了多少历史」的证据；
//! 该走查带页数预算，超额即停并上报，不参与键集对比。
//!
//! 两侧都是各自栈的真实解码器，不做第三份实现：
//!   - 旧栈页解码走 `pdms_io::PdmsIO::read_index_data`（deku 零终止扫描、
//!     硬编 4 dword 步长）；
//!   - v2 页解码走 `pdmsdb_engine_v2::db3::IndexPageView::from_page`
//!     （free_dwords 反推条目数、页头声明宽度），活树页面集由镜像引擎
//!     `db3/iter.rs` 规则的走查给出，并用引擎自己的 `scan_refnos_from_root`
//!     做一遍自检，两者不一致按探针缺陷上报。
//!
//! 只读，不连库。批跑：`cargo run --release --bin legacy_v2_read_parity`。

use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use aios_core::pdms_types::RefU64;
use parse_pdms_db::pdmsdb_engine_v2::db3::IndexPageView;
use parse_pdms_db::pdmsdb_engine_v2::{DbHandle, EngineOptions, EngineV2, PageId, RefNo};
use pdms_io::PdmsIO;
use serde::{Deserialize, Serialize};

const DEFAULT_DIR: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
const DEFAULT_OUT: &str = r"docs\evidence\2026-08-30-legacy-v2-read-parity-raw.jsonl";
/// 自动汇总每次复跑都重写；手写判读在 `2026-08-30-legacy-v2-read-parity.md`。
const DEFAULT_REPORT: &str = r"docs\evidence\2026-08-30-legacy-v2-read-parity-raw-summary.md";
const PAGE_BYTES: u64 = 0x800;
/// 与 e3d-io 429 库裁决语料同一口径的尺寸窗，保证数字可互相印证。
const DEFAULT_MIN_BYTES: u64 = 20 * 1024;
const DEFAULT_MAX_BYTES: u64 = 25 * 1024 * 1024;
const DEFAULT_SAMPLE: usize = 1000;
/// 幽灵键（多读键）最多抽这么多进入点查（其余名额给活键）。
const GHOST_SAMPLE_CAP: usize = 500;
const LIST_SAMPLE_CAP: usize = 12;
/// 旧栈自由走查的页访问预算：无 visited 集时 COW 共享子树可能被反复重走，
/// 预算耗尽即停（budget_exhausted=true），防止病理文件拖死批跑。
const FREE_WALK_PAGE_BUDGET: usize = 100_000;

// ---------------------------------------------------------------- 输出结构

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionReport {
    old_count: usize,
    v2_count: usize,
    /// 旧栈有、v2 没有的 sesno。
    only_old: Vec<i64>,
    /// v2 有、旧栈没有的 sesno。
    only_v2: Vec<i64>,
    /// 共同 sesno 里会话页号不等的。
    page_mismatch: Vec<i64>,
    /// 共同 sesno 里 index_root 不等的。
    root_mismatch: Vec<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct V2WalkReport {
    entries: usize,
    distinct_keys: usize,
    duplicate_keys: usize,
    pages_visited: usize,
    leaf_pages: usize,
    /// 解码失败被当空页跳过的页数（镜像引擎行为，但这里必须计数）。
    undecodable_pages: usize,
}

/// 旧栈式自由全树走查（观察项，不参与键集对比）。
#[derive(Debug, Default, Serialize, Deserialize)]
struct FreeWalkReport {
    entries: usize,
    distinct_keys: usize,
    duplicate_keys: usize,
    pages_visited: usize,
    undecodable_pages: usize,
    /// 子页层级未下降被跳过的条目数（镜像 filter_index_data 守卫）。
    level_anomalies: usize,
    budget_exhausted: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EnumReport {
    /// 活叶页尾部多读的条目数（零终止越过 free_dwords 界限，D3-1）。
    over_read_entries: usize,
    /// 多读条目里，键在全库活键集之外的（纯幽灵键）。
    ghost_keys: usize,
    /// 多读条目里，键与别处活键重复的（陈旧副本，搜索去重层的用武之地）。
    stale_duplicates_of_live: usize,
    /// 少读：零终止提前停掉的活条目数（含整页解码失败折算）。
    under_read_entries: usize,
    /// 旧栈整页解码失败的活叶页数（零终止扫描无终止符时整页报废，
    /// 该页所有活键对旧栈完全不可见）。
    old_undecodable_live_leaf_pages: usize,
    /// 同页前缀逐字不等的条目数（应为 0；非 0 即 D3-2 步长错位实锤）。
    prefix_mismatch_entries: usize,
    /// 内部页：旧栈多读的子指针数（指向陈旧子树的入口）。
    stale_child_pointers: usize,
    /// 内部页：旧栈少读的子指针数。
    missing_child_pointers: usize,
    ghost_sample: Vec<EntrySample>,
    under_read_sample: Vec<EntrySample>,
    prefix_mismatch_sample: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EntrySample {
    refno: String,
    pgno: u32,
    offset_words: u32,
    flag: u16,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HeaderDeclReport {
    /// 文件头 0x34 的页大小（4 字节字计）。512 之外的值是 D1-1 的实锤。
    page_size_words: u32,
    stored_page_count: u32,
    extent_count: usize,
    /// v2 走查途中的页头声明直方图：(是否叶, key_dwords, value_dwords) → 页数。
    widths: BTreeMap<String, usize>,
    /// 叶页里声明宽度不在 {0,2}×{0,2} 的页数（D3-2：旧栈硬编 4 步长会错位）。
    leaf_nondefault_decl: usize,
    internal_nondefault_decl: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LookupReport {
    sampled: usize,
    both_hit_agree: usize,
    both_hit_pos_mismatch: usize,
    old_only_hit: usize,
    v2_only_hit: usize,
    both_miss: usize,
    /// 抽中的幽灵键里旧栈点查命中的个数——幽灵能被点查够到，才会漏进上层。
    ghost_reachable_by_old: usize,
    ghost_sampled: usize,
    /// v2 命中但落在非主 extent 上的次数（D1-4 观测）。
    v2_hit_off_main_extent: usize,
    old_only_hit_sample: Vec<String>,
    v2_only_hit_sample: Vec<String>,
    pos_mismatch_sample: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FlagReport {
    /// v2 口径全部活叶条目的 flag 直方图。
    v2_all: BTreeMap<u16, u64>,
    /// 多读条目（陈旧槽位）的 flag 直方图。
    over_read_only: BTreeMap<u16, u64>,
    /// 抽样键上按旧栈点查可达/不可达分层：flag → (可达数, 不可达数)。
    old_lookup_by_flag: BTreeMap<u16, (u64, u64)>,
    /// 同上，按 v2 点查分层。
    v2_lookup_by_flag: BTreeMap<u16, (u64, u64)>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileReport {
    file: String,
    bytes: u64,
    dbnum: i32,
    target_sesno: Option<i64>,
    old_latest_sesno: Option<i64>,
    v2_latest_sesno: Option<i64>,
    /// target sesno 下两栈 index_root 是否一致。
    target_root_equal: Option<bool>,
    sessions: SessionReport,
    v2_walk: V2WalkReport,
    free_walk: FreeWalkReport,
    enumeration: EnumReport,
    header_decl: HeaderDeclReport,
    lookup: LookupReport,
    flags: FlagReport,
    /// 我的 v2 带 flag 走查与引擎 scan_refnos_from_root 的自检差异数，必须为 0。
    selfcheck_mismatch: usize,
    error: Option<String>,
    elapsed_ms: u64,
}

impl FileReport {
    fn stub(path: &Path) -> Self {
        Self {
            file: path.to_string_lossy().into_owned(),
            bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            dbnum: 0,
            target_sesno: None,
            old_latest_sesno: None,
            v2_latest_sesno: None,
            target_root_equal: None,
            sessions: SessionReport::default(),
            v2_walk: V2WalkReport::default(),
            free_walk: FreeWalkReport::default(),
            enumeration: EnumReport::default(),
            header_decl: HeaderDeclReport::default(),
            lookup: LookupReport::default(),
            flags: FlagReport::default(),
            selfcheck_mismatch: 0,
            error: None,
            elapsed_ms: 0,
        }
    }
}

// ---------------------------------------------------------------- 参数

struct Args {
    dirs: Vec<PathBuf>,
    limit: Option<usize>,
    sample: usize,
    out: PathBuf,
    report: PathBuf,
    resume: bool,
    min_bytes: u64,
    max_bytes: u64,
}

fn parse_args() -> Args {
    let mut args = Args {
        dirs: Vec::new(),
        limit: None,
        sample: DEFAULT_SAMPLE,
        out: PathBuf::from(DEFAULT_OUT),
        report: PathBuf::from(DEFAULT_REPORT),
        resume: false,
        min_bytes: DEFAULT_MIN_BYTES,
        max_bytes: DEFAULT_MAX_BYTES,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut take = |name: &str| it.next().unwrap_or_else(|| panic!("{name} 需要一个参数值"));
        match flag.as_str() {
            "--dir" => args.dirs.push(PathBuf::from(take("--dir"))),
            "--limit" => args.limit = Some(take("--limit").parse().expect("--limit 要整数")),
            "--sample" => args.sample = take("--sample").parse().expect("--sample 要整数"),
            "--out" => args.out = PathBuf::from(take("--out")),
            "--report" => args.report = PathBuf::from(take("--report")),
            "--resume" => args.resume = true,
            "--min-bytes" => {
                args.min_bytes = take("--min-bytes").parse().expect("--min-bytes 要整数")
            }
            "--max-bytes" => {
                args.max_bytes = take("--max-bytes").parse().expect("--max-bytes 要整数")
            }
            other => panic!("未知参数 {other}"),
        }
    }
    if args.dirs.is_empty() {
        args.dirs.push(PathBuf::from(DEFAULT_DIR));
    }
    args
}

/// 与 e3d-io 裁决语料同一条过滤规则：`<字母开头>…_<4 位数字>`，尺寸在窗内。
fn corpus_files(dirs: &[PathBuf], min_bytes: u64, max_bytes: u64) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in dirs {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("语料目录读取失败 {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.expect("目录项可读");
            let meta = entry.metadata().expect("目录项元数据可读");
            let name = entry.file_name().to_string_lossy().into_owned();
            let sampled = name.len() > 5
                && name.as_bytes()[name.len() - 5] == b'_'
                && name[name.len() - 4..].bytes().all(|b| b.is_ascii_digit())
                && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic());
            if !meta.is_file() || !sampled {
                continue;
            }
            if meta.len() < min_bytes || meta.len() > max_bytes {
                continue;
            }
            files.push(entry.path());
        }
    }
    files.sort();
    files
}

// ---------------------------------------------------------------- v2 活树走查

type EntryTuple = (u32, u32, u32, u32, u16); // (r0, r1, pgno, offset_words, flag)

const START_MARKER: u32 = 0x8000_0001;

fn is_marker(r0: u32, r1: u32) -> bool {
    r0 == START_MARKER && r1 == START_MARKER
}

/// v2 视角下活树的一页：原始条目序列（含起始标记，供逐字前缀对比）。
struct V2Page {
    page: PageId,
    level: u32,
    /// 原始顺序条目（含起始标记）。
    raw_entries: Vec<EntryTuple>,
}

/// 镜像引擎 db3/iter.rs 的规则（全局 visited 去重、解码失败当空页、
/// 内部页每个条目都下降、ext 继承自所在页），保留 flag 与页清单。
/// 与引擎 `scan_refnos_from_root` 的输出做过自检对拍（见 selfcheck）。
fn v2_walk(
    handle: &DbHandle,
    root: PageId,
    decl: &mut HeaderDeclReport,
) -> anyhow::Result<(Vec<V2Page>, V2WalkReport)> {
    let mut report = V2WalkReport::default();
    let mut pages = Vec::new();
    let mut visited: HashSet<(u32, u32)> = HashSet::new();
    let mut stack = vec![root];
    while let Some(page_id) = stack.pop() {
        if !visited.insert((page_id.ext_no, page_id.page_no)) {
            continue;
        }
        // 引擎 descend 里 I/O 错误是硬失败、解码错误当空页；镜像同一姿势。
        let bytes = handle.read_page(page_id)?;
        let Ok(view) = IndexPageView::from_page(&bytes) else {
            report.undecodable_pages += 1;
            continue;
        };
        report.pages_visited += 1;
        let is_leaf = view.level == 0;
        *decl
            .widths
            .entry(format!(
                "{}k{}v{}",
                if is_leaf { "leaf-" } else { "int-" },
                view.key_dwords,
                view.value_dwords
            ))
            .or_default() += 1;
        let nondefault = !matches!(view.key_dwords, 0 | 2) || !matches!(view.value_dwords, 0 | 2);
        if is_leaf {
            report.leaf_pages += 1;
            if nondefault {
                decl.leaf_nondefault_decl += 1;
            }
        } else {
            if nondefault {
                decl.internal_nondefault_decl += 1;
            }
            for entry in view.entries.iter().rev() {
                stack.push(PageId {
                    ext_no: page_id.ext_no,
                    page_no: entry.page_no,
                });
            }
        }
        let raw_entries = view
            .entries
            .iter()
            .map(|e| {
                (
                    e.refno.hi(),
                    e.refno.lo(),
                    e.page_no,
                    e.offset_words,
                    e.flag,
                )
            })
            .collect::<Vec<_>>();
        if is_leaf {
            report.entries += raw_entries.iter().filter(|e| !is_marker(e.0, e.1)).count();
        }
        pages.push(V2Page {
            page: page_id,
            level: view.level,
            raw_entries,
        });
    }
    let mut distinct: HashSet<(u32, u32)> = HashSet::new();
    for page in &pages {
        if page.level == 0 {
            for e in &page.raw_entries {
                if !is_marker(e.0, e.1) {
                    distinct.insert((e.0, e.1));
                }
            }
        }
    }
    report.distinct_keys = distinct.len();
    report.duplicate_keys = report.entries - report.distinct_keys;
    Ok((pages, report))
}

// ---------------------------------------------------------------- 旧栈自由走查（观察项）

fn free_walk(io: &mut PdmsIO, root: u32) -> FreeWalkReport {
    let mut report = FreeWalkReport::default();
    let mut distinct: HashSet<(u32, u32)> = HashSet::new();
    // 镜像 filter_index_data：层级守卫、读失败静默跳过（这里跳过但计数）、
    // 无 visited 集。深度由层级严格下降兜底，广度由页预算兜底。
    let mut stack: Vec<(u32, i64)> = vec![(root, i64::MAX)];
    while let Some((pgno, parent_level)) = stack.pop() {
        if report.pages_visited >= FREE_WALK_PAGE_BUDGET {
            report.budget_exhausted = true;
            break;
        }
        let page = match io.read_index_data(pgno) {
            Ok(page) => page,
            Err(_) => {
                report.undecodable_pages += 1;
                continue;
            }
        };
        let level = page.level as i64;
        if level >= parent_level {
            report.level_anomalies += 1;
            continue;
        }
        report.pages_visited += 1;
        if page.level == 0 {
            for loc in &page.refno_locs {
                if loc.is_start_page() {
                    continue;
                }
                report.entries += 1;
                distinct.insert((loc.refno_0, loc.refno_1));
            }
        } else {
            // 起始标记也是子指针，旧栈搜索会下降它，这里同样下降。
            for loc in page.refno_locs.iter().rev() {
                stack.push((loc.pgno, level));
            }
        }
    }
    report.distinct_keys = distinct.len();
    report.duplicate_keys = report.entries - report.distinct_keys;
    report
}

// ---------------------------------------------------------------- 单文件

fn refno_str(r0: u32, r1: u32) -> String {
    format!("={r0}/{r1}")
}

fn sample_entry(e: &EntryTuple) -> EntrySample {
    EntrySample {
        refno: refno_str(e.0, e.1),
        pgno: e.2,
        offset_words: e.3,
        flag: e.4,
    }
}

fn read_raw_header(path: &Path) -> anyhow::Result<(u32, u32)> {
    let mut file = std::fs::File::open(path)?;
    let mut head = [0u8; 0x40];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let be = |at: usize| u32::from_be_bytes(head[at..at + 4].try_into().unwrap());
    Ok((be(0x34), be(0x38)))
}

fn process_file(path: &Path, sample_n: usize) -> FileReport {
    let started = Instant::now();
    let mut report = FileReport::stub(path);
    if let Err(error) = process_file_inner(path, sample_n, &mut report) {
        report.error = Some(format!("{error:#}"));
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report
}

fn process_file_inner(path: &Path, sample_n: usize, report: &mut FileReport) -> anyhow::Result<()> {
    // ---- 文件头原始字段（旧栈根本不读 0x34/0x38，这里绕开两栈直接取证）
    let (page_size_words, stored_page_count) = read_raw_header(path)?;
    report.header_decl.page_size_words = page_size_words;
    report.header_decl.stored_page_count = stored_page_count;

    // ---- 两栈各自打开
    let mut old = PdmsIO::new("parity", path, true);
    old.open()
        .map_err(|e| anyhow::anyhow!("旧栈打开失败: {e:#}"))?;
    report.dbnum = old.dbnum;

    let options = EngineOptions {
        // 页大小探测器在 490 库里有 17 个误判案例（见 paged.rs 顶注），给死 hint。
        page_size_bytes_hint: Some(PAGE_BYTES as usize),
        ..EngineOptions::default()
    };
    let handle =
        EngineV2::open_read(path, options).map_err(|e| anyhow::anyhow!("v2 打开失败: {e}"))?;
    report.header_decl.extent_count = handle.extent_count();

    // ---- 1. 会话链
    let mut old_sessions: BTreeMap<i64, (u32, u32)> = BTreeMap::new();
    let old_sesnos: Vec<i32> = old.sesno_pgno_map.keys().copied().collect();
    for sesno in old_sesnos {
        let pgno = old.sesno_pgno_map[&sesno];
        let root = old
            .read_ses_data(pgno)
            .map(|d| d.index_root_pageno)
            .unwrap_or(0);
        old_sessions.insert(sesno as i64, (pgno, root));
    }
    let mut v2_sessions: BTreeMap<i64, (u32, u32)> = BTreeMap::new();
    let mut v2_roots: BTreeMap<i64, PageId> = BTreeMap::new();
    for session in handle.sessions() {
        v2_sessions.insert(
            session.sesno as i64,
            (session.page.page_no, session.index_root.page_no),
        );
        v2_roots.insert(session.sesno as i64, session.index_root);
    }
    report.sessions.old_count = old_sessions.len();
    report.sessions.v2_count = v2_sessions.len();
    for (&sesno, &(page, root)) in &old_sessions {
        match v2_sessions.get(&sesno) {
            None => report.sessions.only_old.push(sesno),
            Some(&(v2_page, v2_root)) => {
                if v2_page != page {
                    report.sessions.page_mismatch.push(sesno);
                }
                if v2_root != root {
                    report.sessions.root_mismatch.push(sesno);
                }
            }
        }
    }
    for &sesno in v2_sessions.keys() {
        if !old_sessions.contains_key(&sesno) {
            report.sessions.only_v2.push(sesno);
        }
    }
    report.old_latest_sesno = old_sessions.keys().next_back().copied();
    report.v2_latest_sesno = v2_sessions.keys().next_back().copied();

    // ---- target sesno：两栈都认识的最大会话
    let common_latest = old_sessions
        .keys()
        .rev()
        .find(|sesno| v2_sessions.contains_key(sesno))
        .copied();
    let Some(target_sesno) = common_latest else {
        anyhow::bail!(
            "两栈没有共同 sesno（旧 {:?} / v2 {:?}）",
            report.old_latest_sesno,
            report.v2_latest_sesno
        );
    };
    report.target_sesno = Some(target_sesno);
    let old_root = old_sessions[&target_sesno].1;
    let v2_root = v2_roots[&target_sesno];
    report.target_root_equal = Some(old_root == v2_root.page_no);

    // ---- 2. 活树逐页双口径解码
    let (v2_pages, v2_walk_report) = v2_walk(&handle, v2_root, &mut report.header_decl)?;
    report.v2_walk = v2_walk_report;

    // v2 自检：带 flag 走查必须与引擎自己的枚举一致
    {
        let mut engine_set: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        handle
            .scan_refnos_from_root(v2_root, |entry| {
                engine_set
                    .entry((entry.refno.hi(), entry.refno.lo()))
                    .or_insert((entry.loc.page_no, entry.loc.byte_offset));
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!("引擎枚举失败: {e}"))?;
        let mut mine: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        for page in &v2_pages {
            if page.level != 0 {
                continue;
            }
            for e in &page.raw_entries {
                if !is_marker(e.0, e.1) {
                    mine.entry((e.0, e.1)).or_insert((e.2, e.3 * 2));
                }
            }
        }
        report.selfcheck_mismatch += mine.len().abs_diff(engine_set.len());
        for (key, value) in &mine {
            if engine_set.get(key) != Some(value) {
                report.selfcheck_mismatch += 1;
            }
        }
    }

    // 全库活键集（幽灵判定用）
    let mut live_keys: BTreeMap<(u32, u32), EntryTuple> = BTreeMap::new();
    for page in &v2_pages {
        if page.level != 0 {
            continue;
        }
        for e in &page.raw_entries {
            if !is_marker(e.0, e.1) {
                live_keys.entry((e.0, e.1)).or_insert(*e);
                *report.flags.v2_all.entry(e.4).or_default() += 1;
            }
        }
    }

    let mut ghost_keys: Vec<(u32, u32)> = Vec::new();
    let mut ghost_flags: BTreeMap<(u32, u32), u16> = BTreeMap::new();
    for v2_page in &v2_pages {
        let old_page = match old.read_index_data(v2_page.page.page_no) {
            Ok(page) => page,
            Err(_) => {
                // 零终止扫描无终止符（或页损坏）→ 旧栈整页报废。
                if v2_page.level == 0 {
                    report.enumeration.old_undecodable_live_leaf_pages += 1;
                    let live = v2_page
                        .raw_entries
                        .iter()
                        .filter(|e| !is_marker(e.0, e.1))
                        .count();
                    report.enumeration.under_read_entries += live;
                    for e in v2_page.raw_entries.iter().take(LIST_SAMPLE_CAP) {
                        if report.enumeration.under_read_sample.len() < LIST_SAMPLE_CAP {
                            report.enumeration.under_read_sample.push(sample_entry(e));
                        }
                    }
                }
                continue;
            }
        };
        let old_raw: Vec<EntryTuple> = old_page
            .refno_locs
            .iter()
            .map(|l| (l.refno_0, l.refno_1, l.pgno, l.offset, l.flag))
            .collect();
        let v2_raw = &v2_page.raw_entries;
        let shared = old_raw.len().min(v2_raw.len());
        for i in 0..shared {
            if old_raw[i] != v2_raw[i] {
                report.enumeration.prefix_mismatch_entries += 1;
                if report.enumeration.prefix_mismatch_sample.len() < LIST_SAMPLE_CAP {
                    report.enumeration.prefix_mismatch_sample.push(format!(
                        "pg 0x{:X}[{i}]: old={:?} v2={:?}",
                        v2_page.page.page_no, old_raw[i], v2_raw[i]
                    ));
                }
            }
        }
        if v2_page.level == 0 {
            if old_raw.len() > v2_raw.len() {
                for e in &old_raw[v2_raw.len()..] {
                    if is_marker(e.0, e.1) {
                        continue;
                    }
                    report.enumeration.over_read_entries += 1;
                    *report.flags.over_read_only.entry(e.4).or_default() += 1;
                    if live_keys.contains_key(&(e.0, e.1)) {
                        report.enumeration.stale_duplicates_of_live += 1;
                    } else {
                        report.enumeration.ghost_keys += 1;
                        ghost_keys.push((e.0, e.1));
                        ghost_flags.entry((e.0, e.1)).or_insert(e.4);
                        if report.enumeration.ghost_sample.len() < LIST_SAMPLE_CAP {
                            report.enumeration.ghost_sample.push(sample_entry(e));
                        }
                    }
                }
            } else if old_raw.len() < v2_raw.len() {
                for e in &v2_raw[old_raw.len()..] {
                    if is_marker(e.0, e.1) {
                        continue;
                    }
                    report.enumeration.under_read_entries += 1;
                    if report.enumeration.under_read_sample.len() < LIST_SAMPLE_CAP {
                        report.enumeration.under_read_sample.push(sample_entry(e));
                    }
                }
            }
        } else {
            let old_children: HashSet<u32> = old_raw.iter().map(|e| e.2).collect();
            let v2_children: HashSet<u32> = v2_raw.iter().map(|e| e.2).collect();
            report.enumeration.stale_child_pointers +=
                old_children.difference(&v2_children).count();
            report.enumeration.missing_child_pointers +=
                v2_children.difference(&old_children).count();
        }
    }

    // ---- 旧栈自由走查（观察项）
    report.free_walk = free_walk(&mut old, old_root);

    // ---- 3. 点查抽样：幽灵键优先（封顶），活键均匀取样补足
    ghost_keys.sort_unstable();
    ghost_keys.dedup();
    let mut sampled: Vec<((u32, u32), bool)> = Vec::with_capacity(sample_n);
    for key in ghost_keys.iter().take(GHOST_SAMPLE_CAP.min(sample_n)) {
        sampled.push((*key, true));
    }
    let live_list: Vec<(u32, u32)> = live_keys.keys().copied().collect();
    let remain = sample_n.saturating_sub(sampled.len());
    if remain > 0 && !live_list.is_empty() {
        let step = (live_list.len() / remain).max(1);
        for key in live_list.iter().step_by(step).take(remain) {
            sampled.push((*key, false));
        }
    }
    let main_ext = v2_root.ext_no;
    for &((r0, r1), is_ghost) in &sampled {
        let refno = RefU64::from_two_nums(r0, r1);
        let old_hit = old.search_latest_refno(refno, Some(target_sesno as u32));
        let v2_hit = handle
            .find_refno_from_root(v2_root, RefNo::from_parts(r0, r1))
            .map_err(|e| anyhow::anyhow!("v2 点查失败 {r0}/{r1}: {e}"))?;
        report.lookup.sampled += 1;
        if is_ghost {
            report.lookup.ghost_sampled += 1;
            if old_hit.is_some() {
                report.lookup.ghost_reachable_by_old += 1;
            }
        }

        // 活键用活条目的 flag；幽灵键用陈旧条目自己的 flag（D3-5 分层要看的
        // 正是「什么 flag 的陈旧槽位能被点查够到」）。
        let flag = live_keys
            .get(&(r0, r1))
            .map(|e| e.4)
            .or_else(|| ghost_flags.get(&(r0, r1)).copied())
            .unwrap_or(u16::MAX);
        {
            let slot = report.flags.old_lookup_by_flag.entry(flag).or_default();
            if old_hit.is_some() {
                slot.0 += 1;
            } else {
                slot.1 += 1;
            }
            let slot = report.flags.v2_lookup_by_flag.entry(flag).or_default();
            if v2_hit.is_some() {
                slot.0 += 1;
            } else {
                slot.1 += 1;
            }
        }

        match (old_hit, v2_hit) {
            (Some((_, old_abs)), Some(loc)) => {
                if loc.ext_no != main_ext {
                    report.lookup.v2_hit_off_main_extent += 1;
                }
                let v2_abs = loc.page_no as u64 * PAGE_BYTES + loc.byte_offset as u64;
                if old_abs == v2_abs {
                    report.lookup.both_hit_agree += 1;
                } else {
                    report.lookup.both_hit_pos_mismatch += 1;
                    if report.lookup.pos_mismatch_sample.len() < LIST_SAMPLE_CAP {
                        report.lookup.pos_mismatch_sample.push(format!(
                            "{}: old_abs=0x{old_abs:X} v2_abs=0x{v2_abs:X}",
                            refno_str(r0, r1)
                        ));
                    }
                }
            }
            (Some(_), None) => {
                report.lookup.old_only_hit += 1;
                if report.lookup.old_only_hit_sample.len() < LIST_SAMPLE_CAP {
                    report.lookup.old_only_hit_sample.push(refno_str(r0, r1));
                }
            }
            (None, Some(_)) => {
                report.lookup.v2_only_hit += 1;
                if report.lookup.v2_only_hit_sample.len() < LIST_SAMPLE_CAP {
                    report.lookup.v2_only_hit_sample.push(refno_str(r0, r1));
                }
            }
            (None, None) => report.lookup.both_miss += 1,
        }
    }

    Ok(())
}

// ---------------------------------------------------------------- 汇总

#[derive(Default)]
struct Totals {
    files: usize,
    errors: Vec<String>,
    selfcheck_bad: Vec<String>,
    v2_keys: u64,
    over_read_entries: u64,
    ghost_keys: u64,
    stale_duplicates: u64,
    files_with_over_read: usize,
    under_read_entries: u64,
    files_with_under_read: Vec<String>,
    old_dead_leaf_pages: u64,
    files_with_dead_leaf: Vec<String>,
    prefix_mismatch: u64,
    files_with_prefix_mismatch: Vec<String>,
    stale_child_pointers: u64,
    files_with_stale_children: usize,
    missing_child_pointers: u64,
    session_diverged: Vec<String>,
    target_root_unequal: Vec<String>,
    page_size_not_512: Vec<String>,
    multi_extent: Vec<String>,
    leaf_nondefault_decl: u64,
    internal_nondefault_decl: u64,
    widths: BTreeMap<String, usize>,
    leaf_pages: u64,
    lookup_sampled: u64,
    lookup_agree: u64,
    lookup_pos_mismatch: Vec<String>,
    lookup_old_only: Vec<String>,
    lookup_v2_only: Vec<String>,
    lookup_both_miss: u64,
    ghost_sampled: u64,
    ghost_reachable_by_old: u64,
    v2_flags: BTreeMap<u16, u64>,
    over_read_flags: BTreeMap<u16, u64>,
    old_lookup_by_flag: BTreeMap<u16, (u64, u64)>,
    v2_lookup_by_flag: BTreeMap<u16, (u64, u64)>,
    free_walk_pages: u64,
    free_walk_dup_keys: u64,
    free_walk_undecodable: u64,
    free_walk_level_anomalies: u64,
    free_walk_budget_exhausted: Vec<String>,
    over_read_top: Vec<(u64, String)>,
}

fn fold(totals: &mut Totals, r: &FileReport) {
    let name = Path::new(&r.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| r.file.clone());
    totals.files += 1;
    if let Some(error) = &r.error {
        totals.errors.push(format!("{name}: {error}"));
        return;
    }
    if r.selfcheck_mismatch > 0 {
        totals
            .selfcheck_bad
            .push(format!("{name}: {}", r.selfcheck_mismatch));
    }
    totals.v2_keys += r.v2_walk.distinct_keys as u64;
    totals.over_read_entries += r.enumeration.over_read_entries as u64;
    totals.ghost_keys += r.enumeration.ghost_keys as u64;
    totals.stale_duplicates += r.enumeration.stale_duplicates_of_live as u64;
    if r.enumeration.over_read_entries > 0 {
        totals.files_with_over_read += 1;
        totals
            .over_read_top
            .push((r.enumeration.over_read_entries as u64, name.clone()));
    }
    totals.under_read_entries += r.enumeration.under_read_entries as u64;
    if r.enumeration.under_read_entries > 0 {
        totals.files_with_under_read.push(name.clone());
    }
    totals.old_dead_leaf_pages += r.enumeration.old_undecodable_live_leaf_pages as u64;
    if r.enumeration.old_undecodable_live_leaf_pages > 0 {
        totals.files_with_dead_leaf.push(name.clone());
    }
    totals.prefix_mismatch += r.enumeration.prefix_mismatch_entries as u64;
    if r.enumeration.prefix_mismatch_entries > 0 {
        totals.files_with_prefix_mismatch.push(name.clone());
    }
    totals.stale_child_pointers += r.enumeration.stale_child_pointers as u64;
    if r.enumeration.stale_child_pointers > 0 {
        totals.files_with_stale_children += 1;
    }
    totals.missing_child_pointers += r.enumeration.missing_child_pointers as u64;
    let s = &r.sessions;
    if !s.only_old.is_empty()
        || !s.only_v2.is_empty()
        || !s.page_mismatch.is_empty()
        || !s.root_mismatch.is_empty()
    {
        totals.session_diverged.push(format!(
            "{name}: only_old={:?} only_v2={:?} page={:?} root={:?}",
            s.only_old, s.only_v2, s.page_mismatch, s.root_mismatch
        ));
    }
    if r.target_root_equal == Some(false) {
        totals.target_root_unequal.push(name.clone());
    }
    if r.header_decl.page_size_words != 512 {
        totals
            .page_size_not_512
            .push(format!("{name}: 0x34={}", r.header_decl.page_size_words));
    }
    if r.header_decl.extent_count > 1 {
        totals
            .multi_extent
            .push(format!("{name}: extents={}", r.header_decl.extent_count));
    }
    totals.leaf_nondefault_decl += r.header_decl.leaf_nondefault_decl as u64;
    totals.internal_nondefault_decl += r.header_decl.internal_nondefault_decl as u64;
    for (k, v) in &r.header_decl.widths {
        *totals.widths.entry(k.clone()).or_default() += v;
    }
    totals.leaf_pages += r.v2_walk.leaf_pages as u64;
    totals.lookup_sampled += r.lookup.sampled as u64;
    totals.lookup_agree += r.lookup.both_hit_agree as u64;
    if r.lookup.both_hit_pos_mismatch > 0 {
        totals.lookup_pos_mismatch.push(format!(
            "{name}: {} 处 {:?}",
            r.lookup.both_hit_pos_mismatch, r.lookup.pos_mismatch_sample
        ));
    }
    if r.lookup.old_only_hit > 0 {
        totals.lookup_old_only.push(format!(
            "{name}: {} 个 {:?}",
            r.lookup.old_only_hit, r.lookup.old_only_hit_sample
        ));
    }
    if r.lookup.v2_only_hit > 0 {
        totals.lookup_v2_only.push(format!(
            "{name}: {} 个 {:?}",
            r.lookup.v2_only_hit, r.lookup.v2_only_hit_sample
        ));
    }
    totals.lookup_both_miss += r.lookup.both_miss as u64;
    totals.ghost_sampled += r.lookup.ghost_sampled as u64;
    totals.ghost_reachable_by_old += r.lookup.ghost_reachable_by_old as u64;
    for (k, v) in &r.flags.v2_all {
        *totals.v2_flags.entry(*k).or_default() += v;
    }
    for (k, v) in &r.flags.over_read_only {
        *totals.over_read_flags.entry(*k).or_default() += v;
    }
    for (k, (hit, miss)) in &r.flags.old_lookup_by_flag {
        let slot = totals.old_lookup_by_flag.entry(*k).or_default();
        slot.0 += hit;
        slot.1 += miss;
    }
    for (k, (hit, miss)) in &r.flags.v2_lookup_by_flag {
        let slot = totals.v2_lookup_by_flag.entry(*k).or_default();
        slot.0 += hit;
        slot.1 += miss;
    }
    totals.free_walk_pages += r.free_walk.pages_visited as u64;
    totals.free_walk_dup_keys += r.free_walk.duplicate_keys as u64;
    totals.free_walk_undecodable += r.free_walk.undecodable_pages as u64;
    totals.free_walk_level_anomalies += r.free_walk.level_anomalies as u64;
    if r.free_walk.budget_exhausted {
        totals.free_walk_budget_exhausted.push(name.clone());
    }
}

fn list_block(title: &str, items: &[String], cap: usize) -> String {
    let mut out = format!("- {title}: **{}**\n", items.len());
    for item in items.iter().take(cap) {
        out.push_str(&format!("  - {item}\n"));
    }
    if items.len() > cap {
        out.push_str(&format!("  - …（其余 {} 条见 JSONL）\n", items.len() - cap));
    }
    out
}

fn write_report(path: &Path, totals: &mut Totals, elapsed_s: u64, args: &Args) -> String {
    totals.over_read_top.sort_by(|a, b| b.0.cmp(&a.0));
    let mut md = String::new();
    md.push_str(&format!(
        "## 探针原始汇总（legacy_v2_read_parity 自动生成，勿手改本节数字）\n\n\
         - 语料:{:?} 尺寸窗 [{}, {}] 字节;文件 **{}** 个,错误 **{}** 个,耗时 {}s\n\
         - 抽样点查:每库 ≤{},合计 {}\n\n",
        args.dirs,
        args.min_bytes,
        args.max_bytes,
        totals.files,
        totals.errors.len(),
        elapsed_s,
        args.sample,
        totals.lookup_sampled,
    ));
    md.push_str("### 自检\n\n");
    md.push_str(&list_block(
        "v2 带 flag 走查 ↔ 引擎枚举不一致（应为 0）",
        &totals.selfcheck_bad,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block("文件级错误", &totals.errors, LIST_SAMPLE_CAP));
    md.push_str("\n### 1 · 会话链\n\n");
    md.push_str(&list_block(
        "会话链有分歧的文件",
        &totals.session_diverged,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "target sesno 下两栈 index_root 不等的文件",
        &totals.target_root_unequal,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&format!(
        "\n### 2 · 活树逐页双口径解码（D3-1 / D3-2）\n\n\
         - v2 口径活键合计 **{}**（活叶页 **{}**）\n\
         - 旧栈**多读**:**{}** 条（纯幽灵键 **{}**,陈旧活键副本 **{}**）,分布在 **{}** 个文件\n\
         - 旧栈**少读**:**{}** 条,文件数 **{}**;其中旧栈整页报废的活叶页 **{}**（文件数 {}）\n\
         - 同页前缀逐字不等:**{}** 条（应为 0,非 0 即 D3-2 错位）,文件数 **{}**\n\
         - 内部页子指针:旧栈多读 **{}**（{} 个文件,陈旧子树入口）,少读 **{}**\n",
        totals.v2_keys,
        totals.leaf_pages,
        totals.over_read_entries,
        totals.ghost_keys,
        totals.stale_duplicates,
        totals.files_with_over_read,
        totals.under_read_entries,
        totals.files_with_under_read.len(),
        totals.old_dead_leaf_pages,
        totals.files_with_dead_leaf.len(),
        totals.prefix_mismatch,
        totals.files_with_prefix_mismatch.len(),
        totals.stale_child_pointers,
        totals.files_with_stale_children,
        totals.missing_child_pointers,
    ));
    md.push_str("- 多读最重的文件（top10）:\n");
    for (count, name) in totals.over_read_top.iter().take(10) {
        md.push_str(&format!("  - {name}: {count}\n"));
    }
    md.push_str(&list_block(
        "出现少读的文件",
        &totals.files_with_under_read,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "出现整页报废的文件",
        &totals.files_with_dead_leaf,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "出现前缀错位的文件",
        &totals.files_with_prefix_mismatch,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&format!(
        "\n### 2b · 旧栈自由走查（观察项,零终止口径顺陈旧指针能走多远）\n\n\
         - 途经页 **{}**,重复键 **{}**,解码失败页 **{}**,层级异常 **{}**\n",
        totals.free_walk_pages,
        totals.free_walk_dup_keys,
        totals.free_walk_undecodable,
        totals.free_walk_level_anomalies,
    ));
    md.push_str(&list_block(
        "预算耗尽被截断的文件",
        &totals.free_walk_budget_exhausted,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&format!(
        "\n### 3 · 点查抽样\n\n\
         - 抽样 **{}**:两栈同中且位置全等 **{}**,双双未中 **{}**\n\
         - 幽灵键抽样 **{}**,其中旧栈点查可达 **{}**（可达即会漏进上层）\n",
        totals.lookup_sampled,
        totals.lookup_agree,
        totals.lookup_both_miss,
        totals.ghost_sampled,
        totals.ghost_reachable_by_old,
    ));
    md.push_str(&list_block(
        "两栈同中但位置不等",
        &totals.lookup_pos_mismatch,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "仅旧栈命中",
        &totals.lookup_old_only,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "仅 v2 命中",
        &totals.lookup_v2_only,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&format!(
        "\n### 4 · 页头声明（D3-2 / D1-1 / D1-4）\n\n\
         - v2 走查途经叶页 **{}**;叶页声明宽度非 {{0,2}}×{{0,2}}:**{}**;\
           内部页非默认声明:**{}**\n\
         - 页头声明直方图:{:?}\n",
        totals.leaf_pages,
        totals.leaf_nondefault_decl,
        totals.internal_nondefault_decl,
        totals.widths,
    ));
    md.push_str(&list_block(
        "page_size(0x34) ≠ 512 字的文件",
        &totals.page_size_not_512,
        LIST_SAMPLE_CAP,
    ));
    md.push_str(&list_block(
        "多 extent 文件",
        &totals.multi_extent,
        LIST_SAMPLE_CAP,
    ));
    md.push_str("\n### 5 · flag 直方图（D3-5 / V2 验证项）\n\n");
    md.push_str(&format!(
        "- v2 全量活叶条目 flag 分布:{:?}\n",
        totals.v2_flags
    ));
    md.push_str(&format!(
        "- 多读条目（陈旧槽位）flag 分布:{:?}\n",
        totals.over_read_flags
    ));
    md.push_str(&format!(
        "- 抽样键按旧栈点查分层 flag → (可达, 不可达):{:?}\n",
        totals.old_lookup_by_flag
    ));
    md.push_str(&format!(
        "- 抽样键按 v2 点查分层 flag → (可达, 不可达):{:?}\n",
        totals.v2_lookup_by_flag
    ));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("报告目录可建");
    }
    std::fs::write(path, &md).expect("报告可写");
    md
}

// ---------------------------------------------------------------- main

fn main() {
    let args = parse_args();
    let started = Instant::now();
    let mut files = corpus_files(&args.dirs, args.min_bytes, args.max_bytes);
    if let Some(limit) = args.limit {
        files.truncate(limit);
    }
    eprintln!("语料 {} 个文件", files.len());

    let mut done: HashSet<String> = HashSet::new();
    if args.resume && args.out.exists() {
        let text = std::fs::read_to_string(&args.out).expect("续跑读取 JSONL");
        for line in text.lines() {
            if let Ok(prev) = serde_json::from_str::<FileReport>(line) {
                done.insert(prev.file);
            }
        }
        eprintln!("续跑:已完成 {} 个", done.len());
    }
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent).expect("输出目录可建");
    }
    let out_file = std::fs::OpenOptions::new()
        .create(true)
        .append(args.resume)
        .truncate(!args.resume)
        .write(true)
        .open(&args.out)
        .expect("JSONL 可写");
    let writer = Mutex::new(std::io::BufWriter::new(out_file));

    let pending: Vec<&PathBuf> = files
        .iter()
        .filter(|p| !done.contains(&p.to_string_lossy().into_owned()))
        .collect();

    use rayon::prelude::*;
    let progress = std::sync::atomic::AtomicUsize::new(0);
    pending.par_iter().for_each(|path| {
        let report =
            std::panic::catch_unwind(|| process_file(path, args.sample)).unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".into());
                let mut report = FileReport::stub(path);
                report.error = Some(format!("panic: {message}"));
                report
            });
        let line = serde_json::to_string(&report).expect("序列化 FileReport");
        {
            let mut writer = writer.lock().expect("writer 锁");
            writeln!(writer, "{line}").expect("写 JSONL 行");
            writer.flush().expect("刷 JSONL");
        }
        let n = progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n % 25 == 0 || n == pending.len() {
            eprintln!("[{n}/{}] {}", pending.len(), path.display());
        }
    });

    // 汇总从 JSONL 全量读回，续跑也能得到完整报告。
    let mut totals = Totals::default();
    let text = std::fs::read_to_string(&args.out).expect("回读 JSONL");
    for line in text.lines() {
        match serde_json::from_str::<FileReport>(line) {
            Ok(report) => fold(&mut totals, &report),
            Err(error) => eprintln!("JSONL 行解析失败（跳过并计入错误）: {error}"),
        }
    }
    let md = write_report(
        &args.report,
        &mut totals,
        started.elapsed().as_secs(),
        &args,
    );
    println!("{md}");
    eprintln!(
        "完成:{} 文件,JSONL={},报告={}",
        totals.files,
        args.out.display(),
        args.report.display()
    );
}
