//! Whole-tree e3d-io direct-read snapshot and optional E3D TTY member-order parity.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use aios_core::pdms_types::EleTreeNode;
use aios_core::{RefU64, RefnoEnum};
use anyhow::Context;
use clap::Parser;
use serde::Serialize;

use aios_database::data_interface::direct_tree::DirectTreeService;
use aios_database::e3d_query::{E3dDriver, parse_members};

#[derive(Parser)]
#[command(about = "e3d-io direct model tree snapshot and E3D TTY parity")]
struct Args {
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    mdb: Option<String>,
    #[arg(long, default_value = "artifacts/direct-tree-tty")]
    out: PathBuf,
    #[arg(long, default_value_t = 100_000)]
    max_nodes: usize,
    /// Limit the traversal to SITE roots belonging to one DESI database.
    #[arg(long)]
    dbnum: Option<i32>,
    /// Limit the traversal and TTY parity check to one element subtree.
    #[arg(long, conflicts_with = "dbnum")]
    root: Option<String>,
    /// Start one read-only E3D TTY session and compare Q MEMBERS for every parent.
    #[arg(long)]
    tty: bool,
}

#[derive(Serialize)]
struct DirectSnapshot {
    source: &'static str,
    project: String,
    mdb: String,
    dbnum: Option<i32>,
    node_count: usize,
    parent_count: usize,
    nodes: Vec<EleTreeNode>,
}

#[derive(Debug, Serialize)]
struct Mismatch {
    parent: String,
    direct: Vec<String>,
    tty: Vec<String>,
}

#[derive(Serialize)]
struct Verification {
    baseline_command: String,
    modified_command: String,
    direct_nodes: usize,
    parents_compared: usize,
    mismatch_count: usize,
    mismatches: Vec<Mismatch>,
    direct_exit_status: i32,
    tty_exit_status: i32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let option = aios_core::get_db_option();
    let project = args.project.unwrap_or_else(|| option.project_name.clone());
    let mdb = args.mdb.unwrap_or_else(|| option.mdb_name.clone());
    std::fs::create_dir_all(&args.out)?;

    let service = DirectTreeService::open(&project, &mdb)?;
    let root = args
        .root
        .as_deref()
        .map(str::parse::<RefU64>)
        .transpose()
        .map_err(|_| anyhow::anyhow!("--root 必须是 ref0/ref1"))?;
    let (nodes, parents) = collect(&service, args.dbnum, root, args.max_nodes)?;
    let direct = DirectSnapshot {
        source: "direct",
        project: project.clone(),
        mdb: format!("/{}", mdb.trim_start_matches('/')),
        dbnum: args.dbnum,
        node_count: nodes.len(),
        parent_count: parents.len(),
        nodes,
    };
    let direct_path = args.out.join("direct-tree.json");
    std::fs::write(&direct_path, serde_json::to_vec_pretty(&direct)?)?;
    println!(
        "DIRECT_OK nodes={} parents={} out={}",
        direct.node_count,
        direct.parent_count,
        direct_path.display()
    );

    if !args.tty {
        return Ok(());
    }

    let repo = std::env::current_dir()?;
    let body = tty_source(&parents);
    std::fs::write(args.out.join("tty-query.mac.body"), &body)?;
    let raw = E3dDriver::from_env(&repo)?.run_source(&repo, "direct-tree-parity", &body)?;
    std::fs::write(args.out.join("tty-raw.log"), &raw)?;
    let mismatches = compare_tty(&parents, &raw)?;
    let verification = Verification {
        baseline_command: "E3D TTY: =<parent>; Q MEMBERS (one read-only session)".into(),
        modified_command: "direct_tree_tty_compare --tty (e3d-io DirectStore)".into(),
        direct_nodes: direct.node_count,
        parents_compared: parents.len(),
        mismatch_count: mismatches.len(),
        mismatches,
        direct_exit_status: 0,
        tty_exit_status: 0,
    };
    let verification_path = args.out.join("verification.json");
    std::fs::write(
        &verification_path,
        serde_json::to_vec_pretty(&verification)?,
    )?;
    println!(
        "TTY_COMPARE parents={} mismatches={} out={}",
        verification.parents_compared,
        verification.mismatch_count,
        verification_path.display()
    );
    anyhow::ensure!(
        verification.mismatch_count == 0,
        "direct tree 与 E3D TTY 有 {} 个父节点不一致",
        verification.mismatch_count
    );
    Ok(())
}

/// Returns every displayed node and parent -> direct children.  SITE roots are
/// grouped under their actual WORL owner so TTY verifies the root layer too.
fn collect(
    service: &DirectTreeService,
    dbnum: Option<i32>,
    root: Option<RefU64>,
    max_nodes: usize,
) -> anyhow::Result<(Vec<EleTreeNode>, BTreeMap<RefU64, Vec<EleTreeNode>>)> {
    if let Some(root) = root {
        let ancestors = service.ancestors(root)?;
        let owner = ancestors
            .get(1)
            .map(RefnoEnum::refno)
            .context("--root 指向 WORL，模型树子树必须从 WORL 下级开始")?;
        let root_node = service
            .children(owner)?
            .into_iter()
            .find(|node| node.refno.refno() == root)
            .with_context(|| format!("--root {root} 不在 direct OWNER {owner} 的成员表中"))?;
        let mut parents = BTreeMap::new();
        let mut nodes = vec![root_node];
        let mut queue = VecDeque::from([root]);
        let mut seen = HashSet::new();
        while let Some(parent) = queue.pop_front() {
            anyhow::ensure!(seen.insert(parent), "树中重复/成环节点 {parent}");
            let children = service.children(parent)?;
            queue.extend(children.iter().map(|node| node.refno.refno()));
            nodes.extend(children.iter().cloned());
            parents.insert(parent, children);
            anyhow::ensure!(
                nodes.len() <= max_nodes,
                "模型树超过 --max-nodes={max_nodes}，当前 {}",
                nodes.len()
            );
        }
        return Ok((nodes, parents));
    }
    let roots = service
        .roots()?
        .into_iter()
        .filter(|node| {
            dbnum.is_none_or(|wanted| service.dbnum_of(node.refno.refno()).ok() == Some(wanted))
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        dbnum.is_none() || !roots.is_empty(),
        "DESI dbnum {:?} 在 MDB 中没有 SITE 根",
        dbnum
    );
    let mut parents: BTreeMap<RefU64, Vec<EleTreeNode>> = BTreeMap::new();
    if dbnum.is_none() {
        for root in &roots {
            parents
                .entry(root.owner.refno())
                .or_default()
                .push(root.clone());
        }
    }
    let mut queue: VecDeque<RefU64> = roots.iter().map(|node| node.refno.refno()).collect();
    let mut seen = HashSet::new();
    let mut nodes = roots;
    while let Some(parent) = queue.pop_front() {
        anyhow::ensure!(seen.insert(parent), "树中重复/成环节点 {parent}");
        let children = service.children(parent)?;
        for child in &children {
            queue.push_back(child.refno.refno());
        }
        nodes.extend(children.iter().cloned());
        parents.insert(parent, children);
        anyhow::ensure!(
            nodes.len() <= max_nodes,
            "模型树超过 --max-nodes={max_nodes}，当前 {}",
            nodes.len()
        );
    }
    Ok((nodes, parents))
}

fn tty_source(parents: &BTreeMap<RefU64, Vec<EleTreeNode>>) -> String {
    let mut out = String::new();
    for (index, parent) in parents.keys().enumerate() {
        let refno = RefnoEnum::from(*parent).to_pdms_str();
        out.push_str(&format!(
            "!dtselected = TRUE\n={refno}\nhandle any\n!dtselected = FALSE\nelsehandle none\nendhandle\n$P DT-{index}-BEGIN\nif (!dtselected) then\nQ MEMBERS\nendif\n$P DT-{index}-END\n"
        ));
    }
    out
}

fn marker_section<'a>(raw: &'a str, index: usize) -> anyhow::Result<&'a str> {
    let begin = format!("DT-{index}-BEGIN");
    let end = format!("DT-{index}-END");
    let start = raw
        .find(&begin)
        .with_context(|| format!("TTY 缺少 {begin}"))?
        + begin.len();
    let tail = &raw[start..];
    let finish = tail.find(&end).with_context(|| format!("TTY 缺少 {end}"))?;
    Ok(&tail[..finish])
}

fn signature(noun: &str, name: &str, refno: Option<&str>) -> String {
    match refno {
        // A reference number is the stable identity. TTY may decorate unnamed
        // members as `=refno`, while direct mode deliberately uses the noun as
        // its UI fallback label; comparing both labels would invent a mismatch.
        Some(refno) => format!("{}|{}", noun.trim(), refno),
        None => format!("{}|{}", noun.trim(), name.trim()),
    }
}

fn compare_tty(
    parents: &BTreeMap<RefU64, Vec<EleTreeNode>>,
    raw: &str,
) -> anyhow::Result<Vec<Mismatch>> {
    let mut mismatches = Vec::new();
    for (index, (parent, children)) in parents.iter().enumerate() {
        let section = marker_section(raw, index)?;
        let wrapped = format!("MCP-MEMBERS-BEGIN\n{section}\nMCP-MEMBERS-END\n");
        let tty_rows = parse_members(&wrapped)?;
        let direct: Vec<String> = children
            .iter()
            .enumerate()
            .map(|(row_index, node)| {
                let tty_refno = tty_rows.get(row_index).and_then(|row| row.refno.as_deref());
                signature(
                    &node.noun,
                    &node.name,
                    tty_refno.map(|_| node.refno.to_pdms_str()).as_deref(),
                )
            })
            .collect();
        let tty: Vec<String> = tty_rows
            .iter()
            .map(|row| signature(&row.noun, &row.value, row.refno.as_deref()))
            .collect();
        if direct != tty {
            mismatches.push(Mismatch {
                parent: RefnoEnum::from(*parent).to_pdms_str(),
                direct,
                tty,
            });
        }
    }
    Ok(mismatches)
}

#[allow(dead_code)]
fn _assert_out_is_absolute(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(path.is_absolute(), "output path must be absolute");
    Ok(())
}
