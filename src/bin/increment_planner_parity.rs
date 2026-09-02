//! 双规划器对拍探针（P0-2，只读、不连库）。
//!
//! 同一个 `(db 文件, base → target)` 窗口，两套规划器各算一份**生成根集合**，三桶归因：
//!
//! - **gen-model 侧**（生产主路的纯函数部分）：`IncrementPipeline::collect_window`
//!   （old-pdms-io 净窗口）→ `classify_operation_impact` 分区 → `merge_net_change_details`
//!   / `propagate_deletes_to_descendants` → `build_unit_rollup`（与 `build_model_update_plan`
//!   同一份纯函数）。持久层 owner 图用 e3d-io **base 端** `DbSet` 顶替 Surreal `pe`，
//!   ADR-003 反向索引留空（本探针只看 DESI 窗口自身）。
//! - **e3d-model 侧**：`collect_window` → `plan_update`（L1 索引候选 → L2 逐元素差分 →
//!   L3 语义账 → L4 单元上卷），产出 `regenerate ∪ remove ∪ regenerate_derived` **单元**。
//!
//! 对拍口径是**覆盖**而不是「根等于根」：gen-model 的一个根重算它的整棵子树，所以
//! e3d-model 的一个单元只要有某个 gen-model 根是它的祖先或自身，生产就会重算它
//! （`covered`）；反过来，一个 gen-model 根名下若没有任何 e3d-model 单元，就是 G 多算
//! （`only_gen_model`）。两侧祖先链都从同一份 e3d-io 图上读，差异只能来自**判据**：
//! L1 候选集合不同（old-pdms-io 净窗口 vs e3d-io 索引差分）、L2 有无（e3d-model 判
//! `unchanged` / TUBI 门）、L4 口径不同（`model_impact` 三态 vs `is_model_unit` +
//! 世界系级联）。每一条 `only_*` 都要有归因，`unexplained` 为 0 才算过门。
//!
//! 第四桶 **`over_coverage`**（2026-09-02，specs/035 T207 (a)）量的是「一致」底下的多算：
//! G 根名下确有 E 单元、算 `covered`，但根不是交付单元（容器自己成了根）或根子树里的
//! 模型单元数多于被覆盖的 E 单元数——增 / 删一条支管时 G 的 `RegenRoot(PIPE)` 是 E 计划的
//! 9 倍，三桶看不见。它不计入 `unexplained`；T201 落地后 `over_coverage_units` 应为 0。
//!
//! 用法：
//! ```text
//! cargo run --bin increment_planner_parity -- \
//!   --db-file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001" \
//!   --base 255 --target 256 --json-out out\parity-8000-255-256.json
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aios_core::{RefU64, RefnoEnum};
use aios_database::data_interface::generation_root::resolve_delivery_unit_types;
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::manual_update::{
    NetChangeDetail, NetOp, OwnerNode, OwnershipSnapshot, build_owner_overlay, build_unit_rollup,
    merge_net_change_details, propagate_deletes_to_descendants, resolve_change_unit,
};
use aios_database::data_interface::model_impact::{
    OperationImpact, classify_operation_effects, classify_operation_impact, owner_change,
};
use clap::Parser;
use e3d_attlib::AttlibData;
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use e3d_model::category::{is_derived_unit, is_model_unit};
use e3d_model::element_diff::diff_element;
use e3d_model::increment::{collect_window, plan_update};
use e3d_model::ledger::ChangeKind;
use pdms_io::io::{EleOperationData, EleOperationDetail};
use serde::Serialize;

const DEFAULT_ATTLIB: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all\attlib.dat";
const DEFAULT_TEMPLATE_DIR: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";

#[derive(Parser, Debug)]
#[command(name = "increment_planner_parity")]
#[command(about = "同一窗口跑两套增量规划器，按生成根三桶归因（只读，不连库）")]
struct Args {
    /// E3D 库文件（`.dat` 主文件，例如 `ams8000_0001`）。
    #[arg(long)]
    db_file: PathBuf,
    /// 窗口 base 会话号（e3d-model 口径：两端会话；gen-model 口径自动取 base+1..=target）。
    #[arg(long)]
    base: u32,
    /// 窗口 target 会话号。
    #[arg(long)]
    target: u32,
    /// 库类型（模板文件按它选）。
    #[arg(long, default_value = "DESI")]
    db_type: String,
    #[arg(long, default_value = DEFAULT_ATTLIB)]
    attlib: PathBuf,
    #[arg(long, default_value = DEFAULT_TEMPLATE_DIR)]
    template_dir: PathBuf,
    /// 最小交付单元类型，逗号分隔；缺省 = 生产默认集（BRAN/HANG/SUPPO/EQUI）。
    #[arg(long)]
    unit_types: Option<String>,
    /// 每桶在控制台最多打印多少条明细（JSON 里是全量）。
    #[arg(long, default_value_t = 40)]
    samples: usize,
    /// 结构化结果落盘路径。
    #[arg(long)]
    json_out: Option<PathBuf>,
    /// JSON 里每个桶最多落多少条明细（汇总计数不受影响；ams1112 那种 3443 条删除不必全带）。
    #[arg(long, default_value_t = 400)]
    json_rows_cap: usize,
    /// 逐条打印两侧的原始变更（G 的操作流 / E 的索引候选 + 两端存在性），超过该条数则省略。
    #[arg(long, default_value_t = 60)]
    dump_changes_up_to: usize,
    /// 对指定 refno（逗号分隔 `a/b`）做两读法交叉核对：e3d-io 与 old-pdms-io 各自在
    /// base / target / 最新会话上能不能点查到它。给 L1 分歧裁决用。
    #[arg(long)]
    probe: Option<String>,
}

/// 两读法交叉核对：同一个 refno 在同一文件、同一会话上，e3d-io 与 old-pdms-io 各说什么。
fn probe_readers(
    db_file: &Path,
    base: u32,
    target: u32,
    refnos: &[RefnoEnum],
    base_graph: &IoGraph,
    target_graph: &IoGraph,
) -> anyhow::Result<()> {
    let mut io = pdms_io::io::PdmsIO::new("", db_file, true);
    io.open()
        .map_err(|e| anyhow::anyhow!("old-pdms-io 打开失败: {e}"))?;
    let latest = io.get_latest_sesno()?;
    println!("== probe == 文件最新会话 {latest}；base={base} target={target}");
    for &refno in refnos {
        let raw = refno.refno();
        let e_base = base_graph.node(refno);
        let e_target = target_graph.node(refno);
        let fmt = |node: &Option<OwnerNode>| match node {
            Some(n) => format!(
                "{}(owner={})",
                n.noun,
                n.owner
                    .map(|o| o.to_pdms_str())
                    .unwrap_or_else(|| "-".into())
            ),
            None => "absent".into(),
        };
        let p_base = io
            .search_latest_refno(raw, Some(base))
            .map(|(s, off)| format!("ses{s}@{off:#x}"));
        let p_target = io
            .search_latest_refno(raw, Some(target))
            .map(|(s, off)| format!("ses{s}@{off:#x}"));
        let p_latest = io
            .search_latest_refno(raw, None)
            .map(|(s, off)| format!("ses{s}@{off:#x}"));
        let p_noun = io
            .auto_get_raw_element(raw)
            .map(|ele| ele.att_map().get_type())
            .unwrap_or_else(|e| format!("err:{e}"));
        println!(
            "  {:<14} e3d-io: base={} target={} | old-pdms-io: base={} target={} latest={} noun@latest={}",
            refno.to_pdms_str(),
            fmt(&e_base),
            fmt(&e_target),
            p_base.unwrap_or_else(|| "absent".into()),
            p_target.unwrap_or_else(|| "absent".into()),
            p_latest.unwrap_or_else(|| "absent".into()),
            p_noun
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// refno 互转：gen-model 用 aios_core::RefnoEnum，e3d 栈用 e3d_io::RefNo。
// ---------------------------------------------------------------------------

fn to_enum(refno: RefNo) -> RefnoEnum {
    RefnoEnum::from(RefU64::from_two_nums(refno.word0, refno.word1))
}

fn to_refno(refno: RefnoEnum) -> RefNo {
    let raw = refno.refno();
    RefNo::new(raw.get_0(), raw.get_1())
}

fn parse_pdms(text: &str) -> Option<RefNo> {
    let (w0, w1) = text.trim().split_once('/')?;
    Some(RefNo::new(w0.parse().ok()?, w1.parse::<i64>().ok()? as u32))
}

// ---------------------------------------------------------------------------
// e3d-io 端的 owner 图：两侧折根都从这里取节点，保证「同一份图」。
// ---------------------------------------------------------------------------

struct IoGraph {
    set: Arc<DbSet>,
    cache: std::cell::RefCell<HashMap<RefnoEnum, Option<OwnerNode>>>,
}

impl IoGraph {
    fn new(set: Arc<DbSet>) -> Self {
        Self {
            set,
            cache: Default::default(),
        }
    }

    fn node(&self, refno: RefnoEnum) -> Option<OwnerNode> {
        if let Some(cached) = self.cache.borrow().get(&refno) {
            return cached.clone();
        }
        let el = self.set.element(to_refno(refno));
        let node = el.element_type().ok().map(|noun| {
            let owner = el
                .owner()
                .ok()
                .flatten()
                .map(|owner| owner.refno())
                .filter(|owner| owner.is_valid() && *owner != el.refno())
                .map(to_enum);
            let name = el.stored_name().ok().flatten().unwrap_or_default();
            OwnerNode { owner, noun, name }
        });
        self.cache.borrow_mut().insert(refno, node.clone());
        node
    }

    fn exists(&self, refno: RefnoEnum) -> bool {
        self.node(refno).is_some()
    }

    /// 从种子沿 owner 链一路读到根，形状对齐 `manual_update::collect_base_graph`。
    fn graph_from(
        &self,
        seeds: impl IntoIterator<Item = RefnoEnum>,
    ) -> HashMap<RefnoEnum, OwnerNode> {
        let mut out = HashMap::new();
        let mut frontier: Vec<RefnoEnum> = seeds.into_iter().filter(|r| r.is_valid()).collect();
        let mut seen: HashSet<RefnoEnum> = HashSet::new();
        while let Some(current) = frontier.pop() {
            if !seen.insert(current) {
                continue;
            }
            let Some(node) = self.node(current) else {
                continue;
            };
            if let Some(owner) = node.owner {
                frontier.push(owner);
            }
            out.insert(current, node);
        }
        out
    }

    /// 子树里（不含根）所有节点的 `(refno, noun)`，成员表原序，环保护。
    fn subtree(&self, root: RefnoEnum) -> Vec<(RefnoEnum, String)> {
        let mut out = Vec::new();
        let mut seen: HashSet<RefnoEnum> = HashSet::new();
        seen.insert(root);
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Ok(members) = self.set.element(to_refno(current)).member_refnos() else {
                continue;
            };
            for member in members {
                let member = to_enum(member);
                if !seen.insert(member) {
                    continue;
                }
                if let Some(node) = self.node(member) {
                    out.push((member, node.noun));
                }
                stack.push(member);
            }
        }
        out
    }
}

fn open_pinned(
    attlib: &Arc<AttlibData>,
    template_dir: &Path,
    db_type: &str,
    file: &Path,
    sesno: u32,
) -> anyhow::Result<Arc<DbSet>> {
    let set = Arc::new(DbSet::new(attlib.clone()));
    let template = template_file_for(template_dir, db_type)?;
    set.add_db(DbFilePin {
        file: file.to_path_buf(),
        template,
        db_type: Some(db_type.to_string()),
        sesno: Some(sesno),
    })?;
    Ok(set)
}

/// 一个生成根 / 便宜路径目标的键：`a/b` + noun。
#[derive(Debug, Clone, Serialize)]
struct RootKey {
    root: String,
    noun: String,
}

// ---------------------------------------------------------------------------
// gen-model 侧
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct GenOp {
    refno: String,
    op: &'static str,
    noun: String,
    impact: &'static str,
    attrs: Vec<String>,
    old_owner: Option<String>,
    new_owner: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct GenSide {
    /// 生产收集器（`IncrementPipeline::collect_window`）在这一窗直接报错时的原话。
    /// 非空表示 G 侧一个根都没算出来——生产上这一窗是失败批次（重试→死信→窗口阻断）。
    collect_error: Option<String>,
    ops_total: usize,
    add: usize,
    modified: usize,
    deleted: usize,
    none: usize,
    regen_refnos: usize,
    transform_refnos: usize,
    skip_refnos: usize,
    cancelled_restored_as_deleted: usize,
    deletes_propagated: usize,
    transform_rerouted_to_regen: usize,
    derived_units_under_transform: usize,
    units_total: usize,
    units_regen: usize,
    no_generation: u32,
    warnings: Vec<String>,
    /// 生成根 → 归因标签（RegenRoot 单元 / Transform 目标折根 / 派生单元）。
    #[serde(skip)]
    regen_roots: BTreeMap<String, RootAttribution>,
    #[serde(skip)]
    transform_roots: BTreeMap<String, RootAttribution>,
    /// 原始操作流（落盘取证用；与 `ops` 同内容，按 refno 串有序）。
    raw_ops: Vec<GenOp>,
    #[serde(skip)]
    ops: HashMap<RefnoEnum, GenOp>,
    #[serde(skip)]
    skip_ops: HashSet<RefnoEnum>,
    #[serde(skip)]
    no_generation_refnos: HashSet<RefnoEnum>,
}

#[derive(Debug, Default, Clone, Serialize)]
struct RootAttribution {
    noun: String,
    tags: Vec<String>,
    /// 命中这个根的窗口元素（两侧共用同一键，归因时互查）。
    refnos: BTreeSet<String>,
}

fn impact_label(impact: OperationImpact) -> &'static str {
    match impact {
        OperationImpact::Regen => "Regen",
        OperationImpact::TransformOnly => "TransformOnly",
        OperationImpact::Skip => "Skip",
    }
}

fn op_label(detail: &EleOperationDetail) -> &'static str {
    match detail {
        EleOperationDetail::Add(_) => "Add",
        EleOperationDetail::Modified(_) => "Modified",
        EleOperationDetail::Deleted => "Deleted",
        EleOperationDetail::None => "None",
    }
}

fn touch(
    map: &mut BTreeMap<String, RootAttribution>,
    key: &RootKey,
    refno: RefnoEnum,
    tag: String,
) {
    let entry = map.entry(key.root.clone()).or_default();
    entry.noun = key.noun.clone();
    entry.tags.push(tag);
    entry.refnos.insert(refno.to_pdms_str());
}

fn gen_model_side(
    db_file: &Path,
    base: u32,
    target: u32,
    base_graph: &IoGraph,
    target_graph: &IoGraph,
    unit_types: &[String],
) -> anyhow::Result<GenSide> {
    let mut side = GenSide::default();
    let started = Instant::now();
    let collected =
        match IncrementPipeline::collect_window(db_file, (base as i32 + 1)..=(target as i32)) {
            Ok(collected) => collected,
            Err(error) => {
                let message = format!("{error:#}");
                println!(
                    "[gen-model] collect_window {}..={} 失败：{message}",
                    base + 1,
                    target
                );
                side.collect_error = Some(message);
                return Ok(side);
            }
        };
    println!(
        "[gen-model] collect_window {}..={} 用时 {}ms，会话 {:?}",
        base + 1,
        target,
        started.elapsed().as_millis(),
        collected.session_sesnos
    );
    for warning in &collected.warnings {
        println!("[gen-model]   warning: {warning}");
    }
    let range_eles: &BTreeMap<u32, Vec<EleOperationData>> = &collected.range_eles;

    // ① 分区（= partition_operation_impacts 的 pub 等价物）。
    let mut regen: HashSet<RefnoEnum> = HashSet::new();
    let mut transform: HashSet<RefnoEnum> = HashSet::new();
    for op in range_eles.values().flatten() {
        let refno = RefnoEnum::from(op.refno);
        side.ops_total += 1;
        match &op.detail {
            EleOperationDetail::Add(_) => side.add += 1,
            EleOperationDetail::Modified(_) => side.modified += 1,
            EleOperationDetail::Deleted => side.deleted += 1,
            EleOperationDetail::None => side.none += 1,
        }
        let impact = classify_operation_impact(op);
        let effects = classify_operation_effects(op);
        let (old_owner, new_owner) = owner_change(op);
        let noun = match &op.detail {
            EleOperationDetail::Deleted => base_graph
                .node(refno)
                .map(|node| node.noun)
                .unwrap_or_default(),
            _ => op.get_noun_type(),
        };
        side.ops.insert(
            refno,
            GenOp {
                refno: refno.to_pdms_str(),
                op: op_label(&op.detail),
                noun,
                impact: impact_label(impact),
                attrs: effects.changed_attributes.clone(),
                old_owner: old_owner.map(|r| r.to_pdms_str()),
                new_owner: new_owner.map(|r| r.to_pdms_str()),
            },
        );
        match impact {
            OperationImpact::Regen => {
                regen.insert(refno);
            }
            OperationImpact::TransformOnly => {
                transform.insert(refno);
            }
            OperationImpact::Skip => {
                side.skip_ops.insert(refno);
            }
        }
    }

    side.raw_ops = side.ops.values().cloned().collect();
    side.raw_ops.sort_by(|a, b| a.refno.cmp(&b.refno));

    // ② 净变更 + 基线删除还原（Surreal 存在性 → base 端 e3d-io 存在性）+ 删除下传。
    let mut details: Vec<NetChangeDetail> = merge_net_change_details(range_eles);
    for detail in details.iter_mut() {
        if detail.net == NetOp::Cancelled && base_graph.exists(detail.refno) {
            detail.net = NetOp::Deleted;
            detail.model_affecting = true;
            side.cancelled_restored_as_deleted += 1;
        }
    }
    let (overlay, deleted_post) = build_owner_overlay(range_eles);
    side.deletes_propagated = propagate_deletes_to_descendants(&mut details, |refno| {
        overlay.get(&refno).and_then(|node| node.owner)
    });
    let cancelled: HashSet<RefnoEnum> = details
        .iter()
        .filter(|d| d.net == NetOp::Cancelled)
        .map(|d| d.refno)
        .collect();
    regen.retain(|r| !cancelled.contains(r));
    transform.retain(|r| !cancelled.contains(r));
    transform.retain(|r| !regen.contains(r));

    // ③ 前态 owner 图（生产从 Surreal `pe` 读；这里从 e3d-io base 端读）。
    let mut seeds: HashSet<RefnoEnum> = details.iter().map(|d| d.refno).collect();
    seeds.extend(overlay.values().filter_map(|node| node.owner));
    seeds.extend(transform.iter().copied());
    let snap = OwnershipSnapshot {
        base: base_graph.graph_from(seeds),
        overlay,
        deleted_post,
        ref_reversal: HashMap::new(),
    };

    // ④ issue #5 改判：位姿目标的生成根若是派生几何单元（BRAN/LUG/SUPC/TRUNNI）→ 整根重生成；
    //    位姿目标子树里的派生单元 → 额外排重生成（前态子树）。
    let transform_targets: Vec<RefnoEnum> = transform.iter().copied().collect();
    let mut derived_under: BTreeMap<String, String> = BTreeMap::new();
    for &refno in &transform_targets {
        if let Some(unit) = resolve_change_unit(&snap, refno, unit_types, false)
            && is_derived_unit(&unit.noun)
        {
            transform.remove(&refno);
            regen.insert(refno);
            side.transform_rerouted_to_regen += 1;
        }
        for (descendant, noun) in base_graph.subtree(refno) {
            if is_derived_unit(&noun) {
                derived_under.insert(descendant.to_pdms_str(), noun.to_ascii_uppercase());
            }
        }
    }
    side.derived_units_under_transform = derived_under.len();
    side.regen_refnos = regen.len();
    side.transform_refnos = transform.len();
    side.skip_refnos = side.skip_ops.len();

    // ⑤ 掩成只有重建类变更 model_affecting，再跑生产同款纯 rollup。
    let regen_details: Vec<NetChangeDetail> = details
        .iter()
        .copied()
        .map(|mut d| {
            d.model_affecting &= regen.contains(&d.refno);
            d
        })
        .collect();
    let (units, no_generation, warnings) = build_unit_rollup(&snap, &regen_details, unit_types);
    side.units_total = units.len();
    side.no_generation = no_generation;
    side.warnings = warnings;

    for unit in units.iter().filter(|unit| unit.will_generate) {
        let entry = side.regen_roots.entry(unit.root_refno.clone()).or_default();
        entry.noun = unit.noun.clone();
        entry.tags.push(format!(
            "RegenRoot(+{} ~{} -{} in{} out{})",
            unit.added, unit.modified, unit.deleted, unit.moved_in, unit.moved_out
        ));
    }
    side.units_regen = side.regen_roots.len();
    for (root, noun) in &derived_under {
        let entry = side.regen_roots.entry(root.clone()).or_default();
        if entry.noun.is_empty() {
            entry.noun = noun.clone();
        }
        entry.tags.push("DerivedUnderTransform".into());
    }

    // ⑥ 逐变更归因：哪个变更把哪个根拉进来（与 build_unit_rollup 的 pre/post 口径一致）。
    for detail in regen_details.iter().filter(|d| d.model_affecting) {
        let op = side.ops.get(&detail.refno);
        let describe = |state: &str| {
            format!(
                "{state}:{:?} {} ({}) attrs={}",
                detail.net,
                detail.refno.to_pdms_str(),
                op.map(|o| o.noun.as_str()).unwrap_or("?"),
                op.map(|o| o.attrs.join("/")).unwrap_or_default()
            )
        };
        let mut hit = false;
        match detail.net {
            NetOp::Added => {
                if let Some(unit) = resolve_change_unit(&snap, detail.refno, unit_types, true) {
                    touch(
                        &mut side.regen_roots,
                        &RootKey {
                            root: unit.root.to_pdms_str(),
                            noun: unit.noun,
                        },
                        detail.refno,
                        describe("post"),
                    );
                    hit = true;
                }
            }
            NetOp::Deleted => {
                if let Some(unit) = resolve_change_unit(&snap, detail.refno, unit_types, false) {
                    touch(
                        &mut side.regen_roots,
                        &RootKey {
                            root: unit.root.to_pdms_str(),
                            noun: unit.noun,
                        },
                        detail.refno,
                        describe("pre"),
                    );
                    hit = true;
                }
            }
            NetOp::Modified => {
                let pre = resolve_change_unit(&snap, detail.refno, unit_types, false);
                let post = resolve_change_unit(&snap, detail.refno, unit_types, true);
                if let Some(unit) = &post {
                    touch(
                        &mut side.regen_roots,
                        &RootKey {
                            root: unit.root.to_pdms_str(),
                            noun: unit.noun.clone(),
                        },
                        detail.refno,
                        describe("post"),
                    );
                    hit = true;
                }
                if let Some(unit) = &pre
                    && post.as_ref().map(|p| p.root) != Some(unit.root)
                {
                    touch(
                        &mut side.regen_roots,
                        &RootKey {
                            root: unit.root.to_pdms_str(),
                            noun: unit.noun.clone(),
                        },
                        detail.refno,
                        describe("pre(moved_out)"),
                    );
                    hit = true;
                }
            }
            NetOp::Cancelled => {}
        }
        if !hit {
            side.no_generation_refnos.insert(detail.refno);
        }
    }
    // 只保留 rollup 真排出的根（归因循环可能把 will_generate=false 的根也 touch 了）。
    let regen_keys: HashSet<String> = units
        .iter()
        .filter(|u| u.will_generate)
        .map(|u| u.root_refno.clone())
        .chain(derived_under.keys().cloned())
        .collect();
    side.regen_roots.retain(|root, _| regen_keys.contains(root));

    // ⑦ Transform 目标（便宜路径）：生产的工作项就是目标元素自己（刷它的整棵子树世界
    //    变换），所以这里按**目标元素**记，不折根——覆盖判定时它覆盖自己的子树。
    for &refno in &transform {
        let op = side.ops.get(&refno);
        let noun = op
            .map(|o| o.noun.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| target_graph.node(refno).map(|n| n.noun))
            .unwrap_or_else(|| "?".into());
        touch(
            &mut side.transform_roots,
            &RootKey {
                root: refno.to_pdms_str(),
                noun,
            },
            refno,
            format!(
                "Transform:{} attrs={}",
                refno.to_pdms_str(),
                op.map(|o| o.attrs.join("/")).unwrap_or_default()
            ),
        );
    }
    Ok(side)
}

// ---------------------------------------------------------------------------
// e3d-model 侧：产出的是**单元**（模型单元 / 待删单元 / 派生容器），不折根。
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum UnitKind {
    Regenerate,
    Remove,
    Derived,
}

#[derive(Debug, Clone, Serialize)]
struct UnitAttribution {
    refno: String,
    noun: String,
    kind: UnitKind,
    /// 这个单元是从 target 端（true）还是 base 端（false）的图上读的。
    post: bool,
    tags: Vec<String>,
    /// 把它拉进计划的窗口元素（L3 账目按最近单元祖先归到它头上；级联按子树归）。
    refnos: BTreeSet<String>,
}

#[derive(Debug, Default, Serialize)]
struct E3dSide {
    candidates: usize,
    inserted: usize,
    modified: usize,
    deleted: usize,
    unchanged: usize,
    rolled_up: usize,
    unresolved: usize,
    no_model: BTreeMap<String, usize>,
    units_regenerate: usize,
    units_remove: usize,
    derived_containers: usize,
    cascades: usize,
    changes_line: String,
    totals_line: String,
    #[serde(skip)]
    units: BTreeMap<String, UnitAttribution>,
    #[serde(skip)]
    unchanged_refnos: HashSet<RefnoEnum>,
    #[serde(skip)]
    candidate_refnos: HashSet<RefnoEnum>,
    #[serde(skip)]
    ledger_refnos: HashSet<RefnoEnum>,
    /// L3 记成 `OpaqueRecordChange` 的元素：记录字节变了、L2 逐属性 / 成员表 / 属主 / 类型
    /// 都比不出差别。它们引发的级联要单列归因（`E_opaque_cascade`），别混进位姿 / 改挂级联。
    #[serde(skip)]
    opaque_refnos: HashSet<RefnoEnum>,
    #[serde(skip)]
    cascade_sources: Vec<(RefnoEnum, String)>,
    /// 原始索引候选（kind, refno, noun@target|base, exists@base, exists@target），落盘取证用。
    raw_candidates: Vec<RawCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct RawCandidate {
    kind: &'static str,
    refno: String,
    noun: String,
    exists_base: bool,
    exists_target: bool,
}

/// 自身 + 全部祖先（target 图或 base 图），链序自下而上。
fn ancestors_inclusive(graph: &IoGraph, refno: RefnoEnum) -> Vec<RefnoEnum> {
    let mut chain = vec![refno];
    let mut current = refno;
    let mut seen: HashSet<RefnoEnum> = HashSet::new();
    seen.insert(refno);
    for _ in 0..128 {
        let Some(owner) = graph.node(current).and_then(|n| n.owner) else {
            break;
        };
        if !seen.insert(owner) {
            break;
        }
        chain.push(owner);
        current = owner;
    }
    chain
}

fn e3d_model_side(
    db_file: &Path,
    base: u32,
    target: u32,
    base_set: &Arc<DbSet>,
    target_set: &Arc<DbSet>,
    base_graph: &IoGraph,
    target_graph: &IoGraph,
) -> anyhow::Result<E3dSide> {
    let mut side = E3dSide::default();
    let started = Instant::now();
    let window = collect_window(db_file, base, target)?;
    println!(
        "[e3d-model] collect_window {base}→{target} 用时 {}ms，候选 {}",
        started.elapsed().as_millis(),
        window.diff.changes.len()
    );
    let started = Instant::now();
    let plan = plan_update(base_set, target_set, &window);
    println!(
        "[e3d-model] plan_update 用时 {}ms",
        started.elapsed().as_millis()
    );
    // 只查候选账：`accounts_for` 里的执行账要 `execute_plan` 之后才成立，本探针不执行。
    let classified = plan.report.rolled_up
        + plan.report.unchanged
        + plan.report.no_model.values().sum::<usize>()
        + plan.report.unresolved.len();
    anyhow::ensure!(
        classified == plan.report.candidates,
        "e3d-model 候选账不平：rolled_up {} + unchanged {} + no_model {} + unresolved {} = {classified}，候选 {}",
        plan.report.rolled_up,
        plan.report.unchanged,
        plan.report.no_model.values().sum::<usize>(),
        plan.report.unresolved.len(),
        plan.report.candidates
    );

    side.candidates = plan.report.candidates;
    side.inserted = plan.report.inserted;
    side.modified = plan.report.modified;
    side.deleted = plan.report.deleted;
    side.unchanged = plan.report.unchanged;
    side.rolled_up = plan.report.rolled_up;
    side.unresolved = plan.report.unresolved.len();
    side.no_model = plan.report.no_model.clone();
    side.units_regenerate = plan.regenerate.len();
    side.units_remove = plan.remove.len();
    side.derived_containers = plan.regenerate_derived.len();
    side.cascades = plan.report.cascades.len();
    side.changes_line = plan.report.changes_line();
    side.totals_line = plan.report.totals_line();

    // 候选全集 + L2「内容没变」的候选（plan_update 只给计数，这里逐个重算一次 L2）。
    for change in &window.diff.changes {
        let refno = to_enum(change.refno());
        side.candidate_refnos.insert(refno);
        if let IndexCandidate::Modified { refno: r, .. } = change
            && let Ok(diff) = diff_element(base_set, target_set, *r)
            && diff.is_unchanged()
        {
            side.unchanged_refnos.insert(refno);
        }
        let kind = match change {
            IndexCandidate::Inserted { .. } => "Inserted",
            IndexCandidate::Modified { .. } => "Modified",
            IndexCandidate::Deleted { .. } => "Deleted",
        };
        let noun = target_graph
            .node(refno)
            .or_else(|| base_graph.node(refno))
            .map(|n| n.noun)
            .unwrap_or_else(|| "?".into());
        side.raw_candidates.push(RawCandidate {
            kind,
            refno: refno.to_pdms_str(),
            noun,
            exists_base: base_graph.exists(refno),
            exists_target: target_graph.exists(refno),
        });
    }

    let mut add_unit = |unit: (u32, u32), kind: UnitKind| {
        let refno = to_enum(RefNo::new(unit.0, unit.1));
        let (post, noun) = match (kind, target_graph.node(refno)) {
            (UnitKind::Remove, _) => (false, base_graph.node(refno).map(|n| n.noun)),
            (_, Some(node)) => (true, Some(node.noun)),
            (_, None) => (false, base_graph.node(refno).map(|n| n.noun)),
        };
        side.units.insert(
            refno.to_pdms_str(),
            UnitAttribution {
                refno: refno.to_pdms_str(),
                noun: noun.unwrap_or_else(|| "?".into()),
                kind,
                post,
                tags: vec![format!("{kind:?}")],
                refnos: BTreeSet::new(),
            },
        );
    };
    for &unit in &plan.regenerate {
        add_unit(unit, UnitKind::Regenerate);
    }
    for &unit in &plan.remove {
        add_unit(unit, UnitKind::Remove);
    }
    for &container in &plan.regenerate_derived {
        add_unit(container, UnitKind::Derived);
    }

    // L3 账目 → 最近的单元祖先（含自身）。改挂/类型变更两端都归。
    for entry in plan.ledger.entries() {
        let refno = to_enum(entry.refno);
        side.ledger_refnos.insert(refno);
        if entry.kind == ChangeKind::OpaqueRecordChange {
            side.opaque_refnos.insert(refno);
        }
        let mut graphs: Vec<&IoGraph> = match entry.kind {
            ChangeKind::Deleted => vec![base_graph],
            ChangeKind::Reparented | ChangeKind::TypeChanged => vec![target_graph, base_graph],
            _ => vec![target_graph],
        };
        graphs.dedup_by(|a, b| std::ptr::eq(*a, *b));
        for graph in graphs {
            if let Some(unit_key) = ancestors_inclusive(graph, refno)
                .into_iter()
                .map(|a| a.to_pdms_str())
                .find(|key| side.units.contains_key(key))
            {
                let unit = side.units.get_mut(&unit_key).expect("just found");
                unit.tags.push(format!(
                    "L3:{:?} {} ({}) {}",
                    entry.kind,
                    refno.to_pdms_str(),
                    entry.noun,
                    entry.detail
                ));
                unit.refnos.insert(refno.to_pdms_str());
            }
        }
    }
    // 级联：源元素子树里的每个单元都记「被谁级联」。
    for incident in &plan.report.cascades {
        let Some(source) = parse_pdms(&incident.refno) else {
            continue;
        };
        let source = to_enum(source);
        side.cascade_sources.push((source, incident.detail.clone()));
        let source_key = source.to_pdms_str();
        for unit in side.units.values_mut() {
            let graph = if unit.post { target_graph } else { base_graph };
            let Some(unit_refno) = parse_pdms(&unit.refno).map(to_enum) else {
                continue;
            };
            if ancestors_inclusive(graph, unit_refno)
                .iter()
                .any(|a| a.to_pdms_str() == source_key)
            {
                unit.tags.push(format!(
                    "cascade:{} ({}) {}",
                    incident.refno, incident.noun, incident.detail
                ));
                unit.refnos.insert(source_key.clone());
            }
        }
    }
    Ok(side)
}

// ---------------------------------------------------------------------------
// 覆盖对拍：G 的根（RegenRoot / DerivedUnderTransform / Transform 目标）覆盖它的子树；
// E 的单元被某个 G 根覆盖 ⇔ 生产会重算它。反向：G 根名下若没有任何 E 单元 ⇒ G 多算。
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct BucketRow {
    key: String,
    noun: String,
    kind: String,
    reason: String,
    covered_by: Vec<String>,
    gen_model_tags: Vec<String>,
    e3d_model_tags: Vec<String>,
    refnos: Vec<String>,
}

/// 一个 G 根名下确有 E 单元（算 `covered`），但 G 整根重算的范围比 E 的计划大：
/// 根不是交付单元（PIPE / ZONE 一类容器自己成了根），或根子树里的模型单元数多于
/// 被它覆盖的 E 单元数。三桶口径下这是「一致」，实际是 G 多算——db8000 BRAN 链上
/// 增 / 删一条支管，G 的 `RegenRoot(PIPE)` 是 E 计划的 9 倍（specs/035 T207 (a)）。
#[derive(Debug, Serialize)]
struct OverCoverageRow {
    root: String,
    noun: String,
    kind: String,
    is_delivery_unit: bool,
    /// 名下被覆盖的 E 单元数。
    e3d_units_covered: usize,
    /// 根子树里的模型单元数（`is_model_unit` + `is_derived_unit`，与 E 的单元口径一致；
    /// 根在 target 端不存在时按 base 端数）。
    subtree_units: usize,
    /// `subtree_units − e3d_units_covered`：G 会多重算的单元数。
    excess: usize,
    reason: String,
    gen_model_tags: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct Summary {
    e3d_units: usize,
    e3d_units_covered_by_regen_root: usize,
    e3d_units_covered_only_by_transform: usize,
    only_e3d_model: usize,
    gen_roots: usize,
    gen_roots_with_e3d_unit: usize,
    only_gen_model: usize,
    /// `over_coverage` 桶里的 G 根数（不计入 `unexplained`：它是多算的度量，不是漏算）。
    over_covered_roots: usize,
    /// 这些根合计会多重算的单元数。T201 落地后应为 0。
    over_coverage_units: usize,
    unexplained: usize,
    reasons: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct Report {
    db_file: String,
    base: u32,
    target: u32,
    unit_types: Vec<String>,
    gen_model: GenSide,
    e3d_model: E3dSide,
    covered: Vec<BucketRow>,
    only_gen_model: Vec<BucketRow>,
    only_e3d_model: Vec<BucketRow>,
    over_coverage: Vec<OverCoverageRow>,
    summary: Summary,
}

/// 根子树里按 E 的单元口径数出来的模型单元数：`is_model_unit`（正体 / 路由成员）加
/// `is_derived_unit`（隐式管身的派生容器）。G 的 `RegenRoot` 整棵子树重算，这就是它
/// 真正要重建的单元数；与 `covered` 里被它覆盖的 E 单元数相减就是多算量。
fn subtree_unit_count(set: &Arc<DbSet>, root: RefnoEnum) -> usize {
    let mut count = 0usize;
    let mut frontier = vec![to_refno(root)];
    let mut seen: HashSet<RefnoEnum> = HashSet::new();
    while let Some(current) = frontier.pop() {
        if !seen.insert(to_enum(current)) {
            continue;
        }
        let el = set.element(current);
        let Ok(noun) = el.element_type() else {
            continue;
        };
        if is_model_unit(&noun) || is_derived_unit(&noun) {
            count += 1;
        }
        if let Ok(members) = el.member_refnos() {
            frontier.extend(members.into_iter().filter(|m| m.is_valid()));
        }
    }
    count
}

fn refnos_of(attribution_refnos: &BTreeSet<String>) -> Vec<RefnoEnum> {
    attribution_refnos
        .iter()
        .filter_map(|text| parse_pdms(text))
        .map(to_enum)
        .collect()
}

fn explain_only_e3d(unit: &UnitAttribution, gside: &GenSide, e3d: &E3dSide) -> String {
    if let Some(error) = &gside.collect_error {
        let head: String = error.chars().take(80).collect();
        return format!("G_collect_window_failed(生产收集器整窗报错：{head}…)");
    }
    if unit.tags.iter().any(|t| t.starts_with("cascade:")) {
        // 级联源是 `OpaqueRecordChange`（记录字节变了、L2 逐属性 / 成员表比不出差别）时
        // 单列：这不是位姿 / 改挂的世界系重建，是 E 的保守级联；同一窗 G 的收集器按
        // 解析后内容判「原样重写跳过」、0 根，是 G 对 E 多算（db8000 净空窗 266→271）。
        let opaque_source = refnos_of(&unit.refnos)
            .into_iter()
            .any(|r| e3d.opaque_refnos.contains(&r) && e3d.cascade_sources.iter().any(|(s, _)| *s == r));
        if opaque_source {
            return "E_opaque_cascade(容器记录字节变了但成员表/属性逐项相等，E 保守级联整棵子树；G 按解析后内容判无变化)".into();
        }
        return "E_cascade_world_bake(容器位姿/改挂→世界系子树重建，G 只刷变换或不刷)".into();
    }
    if unit.kind == UnitKind::Derived {
        return "E_derived_route_container(隐式管身派生单元，G 无对应根)".into();
    }
    let refnos = refnos_of(&unit.refnos);
    if refnos.is_empty() {
        return "E_unit_without_ledger_attribution(单元来自级联/派生以外的路径，探针未归因)".into();
    }
    if refnos.iter().all(|r| !gside.ops.contains_key(r)) {
        return "L1_disagreement(e3d-io 有候选，old-pdms-io 净窗口无操作)".into();
    }
    if refnos
        .iter()
        .all(|r| gside.skip_ops.contains(r) || !gside.ops.contains_key(r))
    {
        let attrs: BTreeSet<String> = refnos
            .iter()
            .filter_map(|r| gside.ops.get(r))
            .flat_map(|o| o.attrs.iter().cloned())
            .collect();
        return format!(
            "G_model_impact_skip(model_impact 判 Skip：attrs={})",
            attrs.into_iter().collect::<Vec<_>>().join("/")
        );
    }
    if refnos
        .iter()
        .any(|r| gside.no_generation_refnos.contains(r))
    {
        return "G_no_generation(rollup 解不出合法生成根)".into();
    }
    "unexplained".into()
}

fn explain_only_gen(root: &RootAttribution, gside: &GenSide, e3d: &E3dSide) -> String {
    let refnos = refnos_of(&root.refnos);
    if refnos.is_empty() {
        return "G_root_without_contributing_change(探针未归因)".into();
    }
    if root.tags.iter().any(|t| t.contains("moved_out")) {
        return "G_moved_out_old_root(ADR-009 改挂两端重生成：旧根要重发 manifest；E 按 refno 键只重建单元本身)".into();
    }
    if refnos.iter().all(|r| e3d.unchanged_refnos.contains(r)) {
        return "E_L2_unchanged(索引说记录重写，逐属性比下来内容相等)".into();
    }
    if refnos.iter().all(|r| !e3d.candidate_refnos.contains(r)) {
        return "L1_disagreement(old-pdms-io 有操作，e3d-io 索引差分无候选)".into();
    }
    if refnos
        .iter()
        .all(|r| e3d.unchanged_refnos.contains(r) || !e3d.ledger_refnos.contains(r))
    {
        return "E_L2_unchanged_or_muted(含 TUBI 门吞掉的噪音)".into();
    }
    if root.tags.iter().all(|t| t.starts_with("Transform:")) {
        return "G_transform_target_without_E_unit(位姿目标子树里没有模型单元)".into();
    }
    if refnos.iter().all(|r| {
        e3d.ledger_refnos.contains(r) && {
            let noun = gside.ops.get(r).map(|o| o.noun.as_str()).unwrap_or("");
            e3d.no_model.contains_key(noun)
        }
    }) {
        return "E_no_model(变更落在无模型归属的元素上：容器改名/NonGraphic/未建目录件)".into();
    }
    "unexplained".into()
}

fn print_rows(title: &str, rows: &[BucketRow], samples: usize) {
    println!("== {title} == {} 条", rows.len());
    for r in rows.iter().take(samples) {
        println!("  {} ({}) [{}] reason={}", r.key, r.noun, r.kind, r.reason);
        if !r.covered_by.is_empty() {
            println!("      covered_by {}", r.covered_by.join(", "));
        }
        for t in r.gen_model_tags.iter().take(4) {
            println!("      G  {t}");
        }
        for t in r.e3d_model_tags.iter().take(4) {
            println!("      E  {t}");
        }
    }
    if rows.len() > samples {
        println!("  … 其余 {} 条见 JSON", rows.len() - samples);
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.base < args.target,
        "base 必须小于 target（数字口径；链序由两侧收集器各自校验）"
    );
    let unit_types = match &args.unit_types {
        Some(text) => resolve_delivery_unit_types(
            &text
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
        ),
        None => resolve_delivery_unit_types(&[]),
    };
    println!("== 文件 == {}", args.db_file.display());
    println!(
        "== 窗口 == {} → {}   交付单元类型 {:?}",
        args.base, args.target, unit_types
    );

    let attlib =
        Arc::new(AttlibData::parse_file(&args.attlib).map_err(|error| {
            anyhow::anyhow!("attlib {} 解析失败：{error}", args.attlib.display())
        })?);
    let base_set = open_pinned(
        &attlib,
        &args.template_dir,
        &args.db_type,
        &args.db_file,
        args.base,
    )?;
    let target_set = open_pinned(
        &attlib,
        &args.template_dir,
        &args.db_type,
        &args.db_file,
        args.target,
    )?;
    let base_graph = IoGraph::new(base_set.clone());
    let target_graph = IoGraph::new(target_set.clone());

    if let Some(list) = &args.probe {
        let refnos: Vec<RefnoEnum> = list
            .split(',')
            .filter_map(parse_pdms)
            .map(to_enum)
            .collect();
        probe_readers(
            &args.db_file,
            args.base,
            args.target,
            &refnos,
            &base_graph,
            &target_graph,
        )?;
    }

    let gside = gen_model_side(
        &args.db_file,
        args.base,
        args.target,
        &base_graph,
        &target_graph,
        &unit_types,
    )?;
    let e3d = e3d_model_side(
        &args.db_file,
        args.base,
        args.target,
        &base_set,
        &target_set,
        &base_graph,
        &target_graph,
    )?;

    println!(
        "[gen-model] ops={} (Add {} / Modified {} / Deleted {} / None {}) regen_refnos={} transform_refnos={} skip={} \
         cancelled→deleted={} deletes_propagated={} transform→regen={} derived_under_transform={} units={} regen_roots={} transform_targets={} no_generation={}",
        gside.ops_total,
        gside.add,
        gside.modified,
        gside.deleted,
        gside.none,
        gside.regen_refnos,
        gside.transform_refnos,
        gside.skip_refnos,
        gside.cancelled_restored_as_deleted,
        gside.deletes_propagated,
        gside.transform_rerouted_to_regen,
        gside.derived_units_under_transform,
        gside.units_total,
        gside.regen_roots.len(),
        gside.transform_roots.len(),
        gside.no_generation
    );
    for warning in &gside.warnings {
        println!("[gen-model]   rollup warning: {warning}");
    }
    println!("[e3d-model] {}", e3d.totals_line);
    println!("[e3d-model] {}", e3d.changes_line);
    println!(
        "[e3d-model] units={} (regenerate {} / remove {} / derived {})",
        e3d.units.len(),
        e3d.units_regenerate,
        e3d.units_remove,
        e3d.derived_containers
    );
    if gside.raw_ops.len() <= args.dump_changes_up_to {
        println!("[gen-model] 操作流（old-pdms-io 净窗口）：");
        for op in &gside.raw_ops {
            println!(
                "    {:<9} {:<14} {:<7} impact={:<13} attrs={} owner={:?}→{:?} exists@base={} exists@target={}",
                op.op,
                op.refno,
                op.noun,
                op.impact,
                op.attrs.join("/"),
                op.old_owner,
                op.new_owner,
                parse_pdms(&op.refno)
                    .map(to_enum)
                    .is_some_and(|r| base_graph.exists(r)),
                parse_pdms(&op.refno)
                    .map(to_enum)
                    .is_some_and(|r| target_graph.exists(r)),
            );
        }
    }
    if e3d.raw_candidates.len() <= args.dump_changes_up_to {
        println!("[e3d-model] 索引候选（e3d-io IndexDiff）：");
        for c in &e3d.raw_candidates {
            println!(
                "    {:<9} {:<14} {:<7} exists@base={} exists@target={}",
                c.kind, c.refno, c.noun, c.exists_base, c.exists_target
            );
        }
    }

    // —— 覆盖判定 ——
    let mut summary = Summary::default();
    let mut covered = Vec::new();
    let mut only_e = Vec::new();
    let mut only_g = Vec::new();
    let mut gen_roots_hit: HashSet<String> = HashSet::new();

    summary.e3d_units = e3d.units.len();
    for unit in e3d.units.values() {
        let Some(unit_refno) = parse_pdms(&unit.refno).map(to_enum) else {
            continue;
        };
        let graph = if unit.post {
            &target_graph
        } else {
            &base_graph
        };
        let chain: Vec<String> = ancestors_inclusive(graph, unit_refno)
            .into_iter()
            .map(|r| r.to_pdms_str())
            .collect();
        let regen_hits: Vec<String> = chain
            .iter()
            .filter(|a| gside.regen_roots.contains_key(*a))
            .cloned()
            .collect();
        let transform_hits: Vec<String> = chain
            .iter()
            .filter(|a| gside.transform_roots.contains_key(*a))
            .cloned()
            .collect();
        for hit in regen_hits.iter().chain(transform_hits.iter()) {
            gen_roots_hit.insert(hit.clone());
        }
        let g_tags: Vec<String> = regen_hits
            .iter()
            .filter_map(|r| gside.regen_roots.get(r))
            .chain(
                transform_hits
                    .iter()
                    .filter_map(|r| gside.transform_roots.get(r)),
            )
            .flat_map(|a| a.tags.iter().cloned())
            .collect();
        if !regen_hits.is_empty() {
            summary.e3d_units_covered_by_regen_root += 1;
            covered.push(BucketRow {
                key: unit.refno.clone(),
                noun: unit.noun.clone(),
                kind: format!("{:?}", unit.kind),
                reason: "covered(G RegenRoot 祖先/自身)".into(),
                covered_by: regen_hits,
                gen_model_tags: g_tags,
                e3d_model_tags: unit.tags.clone(),
                refnos: unit.refnos.iter().cloned().collect(),
            });
        } else if !transform_hits.is_empty() {
            summary.e3d_units_covered_only_by_transform += 1;
            let reason = "covered_by_transform_only(G 走 Transform 便宜路径刷子树变换；E 整单元重建——世界系产物两边等价，管身除外)";
            *summary
                .reasons
                .entry(format!("covered/{reason}"))
                .or_default() += 1;
            covered.push(BucketRow {
                key: unit.refno.clone(),
                noun: unit.noun.clone(),
                kind: format!("{:?}", unit.kind),
                reason: reason.into(),
                covered_by: transform_hits,
                gen_model_tags: g_tags,
                e3d_model_tags: unit.tags.clone(),
                refnos: unit.refnos.iter().cloned().collect(),
            });
        } else {
            let reason = explain_only_e3d(unit, &gside, &e3d);
            *summary
                .reasons
                .entry(format!("only_e3d_model/{reason}"))
                .or_default() += 1;
            if reason == "unexplained" {
                summary.unexplained += 1;
            }
            only_e.push(BucketRow {
                key: unit.refno.clone(),
                noun: unit.noun.clone(),
                kind: format!("{:?}", unit.kind),
                reason,
                covered_by: Vec::new(),
                gen_model_tags: Vec::new(),
                e3d_model_tags: unit.tags.clone(),
                refnos: unit.refnos.iter().cloned().collect(),
            });
        }
    }
    summary.only_e3d_model = only_e.len();

    // G 根反向：名下没有任何 E 单元的根。
    let mut g_all: BTreeMap<String, (&RootAttribution, &str)> = BTreeMap::new();
    for (root, attribution) in &gside.regen_roots {
        g_all.insert(root.clone(), (attribution, "RegenRoot"));
    }
    for (root, attribution) in &gside.transform_roots {
        g_all
            .entry(root.clone())
            .or_insert((attribution, "Transform"));
    }
    summary.gen_roots = g_all.len();
    for (root, (attribution, kind)) in &g_all {
        if gen_roots_hit.contains(root) {
            summary.gen_roots_with_e3d_unit += 1;
            continue;
        }
        let reason = explain_only_gen(attribution, &gside, &e3d);
        *summary
            .reasons
            .entry(format!("only_gen_model/{reason}"))
            .or_default() += 1;
        if reason == "unexplained" {
            summary.unexplained += 1;
        }
        only_g.push(BucketRow {
            key: root.clone(),
            noun: attribution.noun.clone(),
            kind: (*kind).to_string(),
            reason,
            covered_by: Vec::new(),
            gen_model_tags: attribution.tags.clone(),
            e3d_model_tags: Vec::new(),
            refnos: attribution.refnos.iter().cloned().collect(),
        });
    }
    summary.only_gen_model = only_g.len();

    // G 根正向：名下有 E 单元的 RegenRoot，整根重算的范围比 E 计划大多少。
    // Transform 目标不算——它只刷子树变换，不重建单元，多覆盖没有代价。
    let mut covered_per_root: BTreeMap<String, usize> = BTreeMap::new();
    for row in &covered {
        if row.reason.starts_with("covered(") {
            for root in &row.covered_by {
                *covered_per_root.entry(root.clone()).or_default() += 1;
            }
        }
    }
    let mut over_coverage = Vec::new();
    for (root, attribution) in &gside.regen_roots {
        let Some(&e3d_units_covered) = covered_per_root.get(root) else {
            continue;
        };
        let Some(root_refno) = parse_pdms(root).map(to_enum) else {
            continue;
        };
        let set = if target_graph.exists(root_refno) {
            &target_set
        } else {
            &base_set
        };
        let subtree_units = subtree_unit_count(set, root_refno);
        let is_delivery_unit = unit_types.iter().any(|u| u == &attribution.noun);
        let excess = subtree_units.saturating_sub(e3d_units_covered);
        if excess == 0 {
            // 整根重算的范围恰好就是 E 的计划（PANE 一类「significant owner」根也在此列）。
            continue;
        }
        // 两种多算要分开看：容器自己成了根是 T201 要改掉的判据；交付单元整根重算比 E 的
        // 单元多，是 gen-model 根级 manifest 的粒度（改一个 FTUB 重算整条 BRAN），按设计。
        let reason = if !is_delivery_unit {
            format!(
                "G_root_not_delivery_unit(容器 {} 自己成了 regen 根，整棵子树 {subtree_units} 个单元重算，E 只计划 {e3d_units_covered} 个)",
                attribution.noun
            )
        } else {
            format!(
                "G_root_regenerates_more_than_E(根子树 {subtree_units} 个单元，E 计划 {e3d_units_covered} 个)"
            )
        };
        *summary
            .reasons
            .entry(format!("over_coverage/{}", reason.split('(').next().unwrap_or(&reason)))
            .or_default() += 1;
        summary.over_covered_roots += 1;
        summary.over_coverage_units += excess;
        over_coverage.push(OverCoverageRow {
            root: root.clone(),
            noun: attribution.noun.clone(),
            kind: "RegenRoot".into(),
            is_delivery_unit,
            e3d_units_covered,
            subtree_units,
            excess,
            reason,
            gen_model_tags: attribution.tags.clone(),
        });
    }

    println!();
    print_rows(
        "covered（E 单元被 G 根覆盖）",
        &covered,
        args.samples.min(12),
    );
    print_rows(
        "only_e3d_model（E 要重建、G 不会碰）",
        &only_e,
        args.samples,
    );
    print_rows(
        "only_gen_model（G 要重算、E 名下无单元）",
        &only_g,
        args.samples,
    );
    println!(
        "== over_coverage（G 根名下有 E 单元，但整根重算比 E 计划多） == {} 条",
        over_coverage.len()
    );
    for r in over_coverage.iter().take(args.samples) {
        println!(
            "  {} ({}) [{}] delivery_unit={} covered_E={} subtree_units={} excess={} reason={}",
            r.root, r.noun, r.kind, r.is_delivery_unit, r.e3d_units_covered, r.subtree_units, r.excess, r.reason
        );
        for t in r.gen_model_tags.iter().take(3) {
            println!("      G  {t}");
        }
    }
    println!();
    println!(
        "== 汇总 == E 单元={} 被 RegenRoot 覆盖={} 仅被 Transform 覆盖={} only_e3d_model={} | G 根={} 名下有 E 单元={} only_gen_model={} | over_coverage 根={} 多算单元={} | unexplained={}",
        summary.e3d_units,
        summary.e3d_units_covered_by_regen_root,
        summary.e3d_units_covered_only_by_transform,
        summary.only_e3d_model,
        summary.gen_roots,
        summary.gen_roots_with_e3d_unit,
        summary.only_gen_model,
        summary.over_covered_roots,
        summary.over_coverage_units,
        summary.unexplained
    );
    for (reason, n) in &summary.reasons {
        println!("   {n:>5}  {reason}");
    }

    if let Some(out) = &args.json_out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cap = |mut rows: Vec<BucketRow>| {
            rows.truncate(args.json_rows_cap);
            rows
        };
        let mut gside = gside;
        let mut e3d = e3d;
        gside.raw_ops.truncate(args.json_rows_cap);
        e3d.raw_candidates.truncate(args.json_rows_cap);
        let report = Report {
            db_file: args.db_file.display().to_string(),
            base: args.base,
            target: args.target,
            unit_types,
            gen_model: gside,
            e3d_model: e3d,
            covered: cap(covered),
            only_gen_model: cap(only_g),
            only_e3d_model: cap(only_e),
            over_coverage,
            summary,
        };
        std::fs::write(out, serde_json::to_string_pretty(&report)?)?;
        println!("== JSON == {}", out.display());
    }
    Ok(())
}
