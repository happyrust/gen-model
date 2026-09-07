//! 真库普查：给定一批 noun，数它们在真项目里的**实例数**与**名下几何量**。
//!
//! 为什么要它：`docs/evidence/2026-09-04-noun-toplevel-reconciliation.md` §2.3b 判出
//! 一批「core 的 3D 绘制侧当顶层项（TOPF）、我方却不当生成根」的 noun，
//! 判词是「要逐条过真库，看名下有多少几何」。那份对账全是 **noun 层面**的，
//! 一个真库都没查——本探针就是补这一步。
//!
//! 只读，不连 Surreal，不写任何东西。每个库一次 `ScanTier::Header` 全扫
//! （每元素一次页读，拿 noun 与 owner），据此建 owner→children 图，
//! 对每个目标 noun 的每个实例数它的子树。
//!
//! 「几何量」的代理是**子树里 core 判 `primitive` 的元素数**
//! （`generation_root::noun_is_primitive`，两位取或）——那正是生成管线会出几何的那些。
//!
//! 用法：
//! ```text
//! cargo run --release --bin noun_census_probe -- \
//!     --dir "D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000" \
//!     --nouns ASMBLY,BLOCK,CWAY,CONVEY,SPOOL,HULCON,ISTRU,ISTRUR \
//!     --json docs/evidence/2026-09-04-topf-noun-census.json
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aios_database::data_interface::generation_root::{
    is_coarse_hierarchy_noun, is_delivery_unit_noun, noun_is_primitive, noun_is_significant,
};
use e3d_io::{ReadOnlyEngine, RefNo, ScanTier};

/// 一个目标 noun 在一个库里的普查结果。
#[derive(Debug, Default, Clone, serde::Serialize)]
struct NounCensus {
    instances: usize,
    /// 全部实例子树里的元素总数（不含实例自己）。
    descendants: usize,
    /// 其中 core 判 primitive 的（= 会出几何的那些）。
    primitive_descendants: usize,
    /// 单个实例子树最大的那个（不含自己）。
    max_subtree: usize,
    /// 实例自己就是 primitive 的个数。
    self_primitive: usize,
    /// **决定性的一格**：子树里**没有被更细的生成根罩住**的元素数。
    ///
    /// 从实例往下走，碰到一个按我方判据已经是生成根的节点就停（那一支归它管）。
    /// 走到的这些才是「今天要跟着实例的祖先根一起重算」的部分。
    /// 这一格接近 0 就说明把这个 noun 补成根**只会让粒度更粗**，不是修漏。
    uncovered: usize,
    /// 其中 core 判 primitive 的（真正会出几何的那些）。
    uncovered_primitive: usize,
    /// 子树里已经是生成根的节点数（= 更细的粒度已经在了）。
    inner_roots: usize,
}

#[derive(Debug, Default, serde::Serialize)]
struct DbReport {
    dbnum: u32,
    file: String,
    sesno: u32,
    elements: usize,
    nouns: BTreeMap<String, NounCensus>,
}

fn parse_args() -> (Vec<PathBuf>, Vec<String>, Option<PathBuf>, usize) {
    let mut dirs = Vec::new();
    let mut nouns = Vec::new();
    let mut json = None;
    let mut max_files = usize::MAX;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dirs.push(PathBuf::from(args.next().expect("--dir 要一个目录"))),
            "--nouns" => {
                nouns = args
                    .next()
                    .expect("--nouns 要一串逗号分隔的 noun")
                    .split(',')
                    .map(|n| n.trim().to_ascii_uppercase())
                    .filter(|n| !n.is_empty())
                    .collect()
            }
            "--json" => json = Some(PathBuf::from(args.next().expect("--json 要一个路径"))),
            "--max-files" => {
                max_files = args
                    .next()
                    .expect("--max-files 要一个数")
                    .parse()
                    .expect("--max-files 不是数")
            }
            other => panic!("不认识的参数 {other}"),
        }
    }
    (dirs, nouns, json, max_files)
}

/// 扫一个库，返回 `(dbnum, sesno, refno→noun_hash, refno→owner)`。
fn scan(path: &Path) -> anyhow::Result<(u32, u32, HashMap<RefNo, u32>, HashMap<RefNo, RefNo>)> {
    let mut engine = ReadOnlyEngine::open(path)?;
    let dbnum = engine.descriptor().db_mark;
    let sesno = engine.session().sesno;
    let mut nouns = HashMap::new();
    let mut owners = HashMap::new();
    for scanned in engine.scan_elements(ScanTier::Header)? {
        let scanned = scanned?;
        nouns.insert(scanned.refno, scanned.noun_hash);
        owners.insert(scanned.refno, scanned.owner);
    }
    Ok((dbnum, sesno, nouns, owners))
}

fn main() -> anyhow::Result<()> {
    let (dirs, nouns, json_out, max_files) = parse_args();
    if dirs.is_empty() || nouns.is_empty() {
        eprintln!("用法见文件头注释：至少要一个 --dir 和一串 --nouns");
        std::process::exit(2);
    }

    let desi = aios_core::tool::db_tool::db1_hash("DESI");
    let targets: HashMap<u32, String> = nouns
        .iter()
        .map(|noun| (aios_core::tool::db_tool::db1_hash(noun), noun.clone()))
        .collect();

    // hash → core 判 primitive。`e3d_io` 的反哈希是私有的，而且逐元素反哈希再查表也白费；
    // 一次把字典里全部 noun 过一遍 `noun_is_primitive`，剩下只查 hash 集合。
    let primitive_hashes: HashSet<u32> = aios_database::noun_dict::default_noun_capabilities()
        .iter()
        .filter(|caps| noun_is_primitive(&caps.noun_name))
        .map(|caps| caps.noun_hash as u32)
        .collect();

    // 我方今天的选根判据，与 `enumerate_generation_roots_in_subtree` 逐字同口径：
    // 交付单元，或（非层级容器 ∧ core 判 significant）。
    let unit_types =
        aios_database::data_interface::generation_root::resolve_delivery_unit_types(&[]);
    let root_hashes: HashSet<u32> = aios_database::noun_dict::default_noun_capabilities()
        .iter()
        .filter(|caps| {
            let noun = caps.noun_name.as_str();
            is_delivery_unit_noun(noun, &unit_types)
                || (!is_coarse_hierarchy_noun(noun) && noun_is_significant(noun))
        })
        .map(|caps| caps.noun_hash as u32)
        .collect();

    let mut files = Vec::new();
    for dir in &dirs {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut reports = Vec::new();
    let mut totals: BTreeMap<String, NounCensus> = BTreeMap::new();
    let mut scanned_files = 0usize;
    let mut skipped = 0usize;

    for path in files {
        if scanned_files >= max_files {
            break;
        }
        let started = std::time::Instant::now();
        // 先看库类型再扫：CATA 也有上百 MB，扫完再丢掉是白花全扫的钱。
        // 类型看 descriptor 的 `db_type_hash`，别猜文件名。
        let db_type_ok = ReadOnlyEngine::open(&path)
            .map(|engine| engine.descriptor().db_type_hash == desi)
            .unwrap_or(false);
        if !db_type_ok {
            skipped += 1;
            continue;
        }
        let Ok((dbnum, sesno, element_nouns, owners)) = scan(&path) else {
            skipped += 1;
            continue;
        };
        scanned_files += 1;

        // owner → children，只为下面数子树。
        let mut children: HashMap<RefNo, Vec<RefNo>> = HashMap::new();
        for (refno, owner) in &owners {
            if owner != refno {
                children.entry(*owner).or_default().push(*refno);
            }
        }

        let mut report = DbReport {
            dbnum,
            file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            sesno,
            elements: element_nouns.len(),
            nouns: BTreeMap::new(),
        };

        for (refno, hash) in &element_nouns {
            let Some(name) = targets.get(hash) else {
                continue;
            };
            let entry = report.nouns.entry(name.clone()).or_default();
            entry.instances += 1;
            if noun_is_primitive(name) {
                entry.self_primitive += 1;
            }

            let mut subtree = 0usize;
            let mut primitive = 0usize;
            let mut seen = HashSet::new();
            let mut stack = children.get(refno).cloned().unwrap_or_default();
            while let Some(node) = stack.pop() {
                if !seen.insert(node) {
                    continue;
                }
                subtree += 1;
                if let Some(child_hash) = element_nouns.get(&node)
                    && primitive_hashes.contains(child_hash)
                {
                    primitive += 1;
                }
                if let Some(kids) = children.get(&node) {
                    stack.extend(kids.iter().copied());
                }
            }
            entry.descendants += subtree;
            entry.primitive_descendants += primitive;
            entry.max_subtree = entry.max_subtree.max(subtree);

            // 再走一遍，这次碰到已经是生成根的节点就停：剩下的才是「没被更细的根罩住」的。
            let mut seen = HashSet::new();
            let mut stack = children.get(refno).cloned().unwrap_or_default();
            while let Some(node) = stack.pop() {
                if !seen.insert(node) {
                    continue;
                }
                let Some(node_hash) = element_nouns.get(&node) else {
                    continue;
                };
                if root_hashes.contains(node_hash) {
                    entry.inner_roots += 1;
                    continue;
                }
                entry.uncovered += 1;
                if primitive_hashes.contains(node_hash) {
                    entry.uncovered_primitive += 1;
                }
                if let Some(kids) = children.get(&node) {
                    stack.extend(kids.iter().copied());
                }
            }
        }

        if !report.nouns.is_empty() {
            println!(
                "{:<24} dbnum={:<8} ses={:<5} elements={:<8} {:.1}s",
                report.file,
                report.dbnum,
                report.sesno,
                report.elements,
                started.elapsed().as_secs_f32()
            );
            for (name, census) in &report.nouns {
                println!(
                    "    {name:<8} 实例 {:<5} 子树 {:<7} primitive {:<7} 内层根 {:<6} \
                     未被罩住 {:<7} 其中 primitive {}",
                    census.instances,
                    census.descendants,
                    census.primitive_descendants,
                    census.inner_roots,
                    census.uncovered,
                    census.uncovered_primitive
                );
                let total = totals.entry(name.clone()).or_default();
                total.instances += census.instances;
                total.descendants += census.descendants;
                total.primitive_descendants += census.primitive_descendants;
                total.self_primitive += census.self_primitive;
                total.uncovered += census.uncovered;
                total.uncovered_primitive += census.uncovered_primitive;
                total.inner_roots += census.inner_roots;
                total.max_subtree = total.max_subtree.max(census.max_subtree);
            }
        }
        reports.push(report);
    }

    println!("\n扫了 {scanned_files} 个 DESI 库，跳过 {skipped} 个（打不开或不是 DESI）");
    println!("\n合计：");
    println!(
        "  {:<8} {:>7} {:>9} {:>12} {:>9} {:>11} {:>15}",
        "noun", "实例", "子树", "primitive", "内层根", "未被罩住", "未被罩住primitive"
    );
    for noun in &nouns {
        let census = totals.get(noun).cloned().unwrap_or_default();
        println!(
            "  {:<8} {:>7} {:>9} {:>12} {:>9} {:>11} {:>15}",
            noun,
            census.instances,
            census.descendants,
            census.primitive_descendants,
            census.inner_roots,
            census.uncovered,
            census.uncovered_primitive
        );
    }
    println!(
        "\n「未被罩住」= 从实例往下走、碰到已经是生成根的节点就停，剩下的那些。\
         \n这一格接近 0 = 补成生成根只会让粒度更粗，不是修漏。"
    );

    if let Some(path) = json_out {
        let doc = serde_json::json!({
            "dirs": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
            "nouns": nouns,
            "scanned_desi_files": scanned_files,
            "skipped_files": skipped,
            "totals": totals,
            "per_db": reports.iter().filter(|r| !r.nouns.is_empty()).collect::<Vec<_>>(),
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;
        println!("\n已写出 {}", path.display());
    }
    Ok(())
}
