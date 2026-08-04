//! refno 索引 vs 扫描 语义诊断探针（ADR-005 / 决策 A 验收）。
//!
//! 索引 = 最新会话 B-tree 的「存活集」；扫描 = 全文件所有物理记录（含已删/旧会话）。
//! 本探针：
//!   ① 建两表并逐元素比对；
//!   ② 用独立单点查询 find_refno_entry 交叉验证索引建表是否自洽、更权威；
//!   ③ children_map 结构门：分别用索引表 / 扫描表构建 owner→children 树（生成管线真正遍历的结构），
//!      看方向性——索引是否 ⊇ 扫描（只找回 scan 漏掉的活元素、从不丢）。无需 SurrealDB 即可证。
//!
//! 用法：
//! ```text
//! cargo run --bin refno_index_probe -- --file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001"
//! ```

use aios_core::RefU64;
use aios_core::db::EleDataEntry;
use clap::Parser;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "refno_index_probe")]
#[command(about = "索引 vs 扫描 语义诊断 + find_refno_entry 交叉验证 + children_map 结构门")]
struct Args {
    /// db 文件路径。
    #[arg(short, long)]
    file: String,
    /// 工程名（parse 语义需要）。
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
    /// 交叉验证时对 scan-only 抽样多少个做 find_refno_entry。
    #[arg(long, default_value_t = 200)]
    sample: usize,
}

/// 复刻 parse_db_basic_data 的 owner→children 遍历：从 world 沿 members BFS，
/// children 仅保留在给定表中的 refno。生成管线遍历的正是这棵树。
fn build_children_map(
    input: &[u8],
    table: &DashMap<RefU64, EleDataEntry>,
    world: RefU64,
) -> HashMap<RefU64, Vec<RefU64>> {
    let mut cmap: HashMap<RefU64, Vec<RefU64>> = HashMap::new();
    let (root, root_children) = if let Some(e) = table.get(&world) {
        parse_pdms_db::parse::parse_ele_children(&input[e.pos - 4..])
    } else {
        (world, Default::default())
    };
    let rc: Vec<RefU64> = root_children
        .iter()
        .filter(|&x| table.contains_key(x))
        .map(|&x| x)
        .collect();
    cmap.insert(root, rc);

    let mut pending = vec![world];
    let mut seen: HashSet<RefU64> = HashSet::new();
    while let Some(r) = pending.pop() {
        if !seen.insert(r) {
            continue;
        }
        let pos = match table.get(&r) {
            Some(e) => e.pos,
            None => continue,
        };
        let membs = parse_pdms_db::parse::parse_ele_membs(&input[pos - 4..]);
        let ch: Vec<RefU64> = membs
            .iter()
            .filter(|&x| table.contains_key(x))
            .map(|&x| x)
            .collect();
        for m in &membs {
            if !seen.contains(m) {
                pending.push(*m);
            }
        }
        cmap.insert(r, ch);
    }
    cmap
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = PathBuf::from(&args.file);
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    println!("== 文件 == {}", path.display());

    let db_basic =
        parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &args.project)?;
    let bytes = db_basic.bytes;
    println!("== bytes == {}", bytes.len());

    let t0 = Instant::now();
    let (scan_tbl, scan_world) = parse_pdms_db::parse::gen_ref_type_pos_table_scan(&bytes);
    let scan_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let indexed = parse_pdms_db::gen_ref_type_pos_table_from_index(&bytes);
    let idx_ms = t1.elapsed().as_millis();

    let Some((idx_tbl, idx_world)) = indexed else {
        println!(
            "== 结论 == 索引不可用（from_index=None），生产将回退扫描。scan={}ms scan_len={}",
            scan_ms,
            scan_tbl.len()
        );
        return Ok(());
    };

    println!(
        "== 建表 == 索引: {} 条 / {}ms   扫描: {} 条 / {}ms   world 一致={}",
        idx_tbl.len(),
        idx_ms,
        scan_tbl.len(),
        scan_ms,
        idx_world == scan_world
    );
    println!(
        "== world == scan_world={:?} 默认值={} 在扫描表内={} | idx_world={:?} 在索引表内={}",
        scan_world,
        scan_world == RefU64::default(),
        scan_tbl.contains_key(&scan_world),
        idx_world,
        idx_tbl.contains_key(&idx_world)
    );
    {
        let mut noun_hist: HashMap<u32, usize> = HashMap::new();
        for e in scan_tbl.iter() {
            *noun_hist.entry(e.value().noun_hash as u32).or_default() += 1;
        }
        let mut top: Vec<(u32, usize)> = noun_hist.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        let shown: Vec<String> = top
            .iter()
            .take(10)
            .map(|(noun, n)| format!("{}x{}", aios_core::tool::db_tool::db1_dehash(*noun as _), n))
            .collect();
        println!("== 扫描表 noun 分布(top10) == {}", shown.join("  "));
    }

    let mut missing = Vec::new();
    let mut pos_mm = Vec::new();
    let mut noun_mm = 0usize;
    for e in scan_tbl.iter() {
        let refno = *e.key();
        let s = e.value();
        match idx_tbl.get(&refno) {
            None => missing.push((refno, s.pos, s.noun_hash)),
            Some(i) => {
                if i.pos != s.pos {
                    pos_mm.push((refno, i.pos, s.pos));
                } else if i.noun_hash != s.noun_hash {
                    noun_mm += 1;
                }
            }
        }
    }
    let mut extra = Vec::new();
    for e in idx_tbl.iter() {
        if !scan_tbl.contains_key(e.key()) {
            extra.push((*e.key(), e.value().pos));
        }
    }
    println!(
        "== 分类 == scan-only(缺失于索引)={} index-only(多出)={} pos不一致={} noun不一致={}",
        missing.len(),
        extra.len(),
        pos_mm.len(),
        noun_mm
    );

    // 交叉①：scan-only 是否真不在最新会话 B-tree。
    let mut checked = 0usize;
    let mut still_in_btree = 0usize;
    for (refno, _pos, _noun) in missing.iter().take(args.sample) {
        checked += 1;
        if parse_pdms_db::find_refno_entry(&bytes, *refno).is_some() {
            still_in_btree += 1;
        }
    }
    println!(
        "== 交叉①(scan-only) == 抽样={} 其中 find_refno_entry 仍能查到={}（应为 0）",
        checked, still_in_btree
    );

    // 交叉②：pos 不一致时 B-tree 单点查询的权威 pos。
    let mut find_eq_idx = 0usize;
    let mut find_eq_scan = 0usize;
    let mut find_other = 0usize;
    for (refno, idx_pos, scan_pos) in pos_mm.iter() {
        match parse_pdms_db::find_refno_entry(&bytes, *refno).map(|e| e.pos) {
            Some(p) if p == *idx_pos => find_eq_idx += 1,
            Some(p) if p == *scan_pos => find_eq_scan += 1,
            _ => find_other += 1,
        }
    }
    if !pos_mm.is_empty() {
        println!(
            "== 交叉②(pos不一致) == find=索引 {} / find=扫描 {} / find=其它 {}（应恒=索引）",
            find_eq_idx, find_eq_scan, find_other
        );
    }

    // ③ children_map 结构门：方向性——索引是否 ⊇ 扫描。
    let ci = build_children_map(&bytes, &idx_tbl, idx_world);
    let cs = build_children_map(&bytes, &scan_tbl, scan_world);
    let mut owner_only_idx = 0usize;
    let mut owner_only_scan = 0usize; // 索引丢了 scan 有的 owner —— 危险
    let mut scan_child_dropped = 0usize; // owner 下 scan 有、索引无 —— 危险
    let mut idx_child_added = 0usize; // owner 下 索引有、scan 无 —— 改进（找回活元素）
    let all_keys: HashSet<RefU64> = ci.keys().chain(cs.keys()).copied().collect();
    for k in &all_keys {
        match (ci.get(k), cs.get(k)) {
            (Some(a), None) => {
                owner_only_idx += 1;
                idx_child_added += a.len();
            }
            (None, Some(b)) => {
                owner_only_scan += 1;
                scan_child_dropped += b.len();
            }
            (Some(a), Some(b)) => {
                let sa: HashSet<RefU64> = a.iter().copied().collect();
                let sb: HashSet<RefU64> = b.iter().copied().collect();
                idx_child_added += sa.difference(&sb).count();
                scan_child_dropped += sb.difference(&sa).count();
            }
            (None, None) => {}
        }
    }
    let children_identical = owner_only_idx == 0
        && owner_only_scan == 0
        && idx_child_added == 0
        && scan_child_dropped == 0;
    let index_is_superset = owner_only_scan == 0 && scan_child_dropped == 0;
    let gate = if children_identical {
        "完全一致"
    } else if index_is_superset {
        "索引 ⊇ 扫描（只找回 scan 漏掉的活元素，从不丢，安全）"
    } else {
        "索引丢了 scan 的元素（危险，需核查）"
    };
    println!(
        "== 结构门(children_map) == 索引树 owners={} 扫描树 owners={} | 仅索引 owner={} 仅扫描 owner={} | 索引找回 child={} 扫描被丢 child={} => {}",
        ci.len(),
        cs.len(),
        owner_only_idx,
        owner_only_scan,
        idx_child_added,
        scan_child_dropped,
        gate
    );
    for (refno, _pos) in extra.iter().take(5) {
        let noun = idx_tbl.get(refno).map(|e| e.noun_hash).unwrap_or(0);
        println!(
            "  [index-only 真实性] refno={:?} noun={:#x}（非0=真实元素记录）",
            refno, noun
        );
    }

    // 解析 index-only 元素类型名（确认旧扫描系统性漏掉的是什么 noun）。
    let db_info = aios_core::get_default_pdms_db_info();
    for (refno, pos) in extra.iter().take(3) {
        let noun = idx_tbl.get(refno).map(|e| e.noun_hash).unwrap_or(0);
        let head: String = if *pos >= 4 {
            bytes[*pos - 4..(*pos + 12).min(bytes.len())]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        let parsed = if *pos >= 4 {
            match parse_pdms_db::parse::parse_ele_data_with_info(&bytes[*pos - 4..], &db_info).await
            {
                Ok(ele) => format!("Ok type={}", ele.whole_attmap.merge().get_type_str().trim()),
                Err(e) => format!("Err={}", e),
            }
        } else {
            "pos<4".to_string()
        };
        println!(
            "  [index-only] refno={:?} noun={:#x} head=[{}] parse={}",
            refno, noun, head, parsed
        );
    }
    let internally_consistent = still_in_btree == 0 && find_other == 0 && find_eq_scan == 0;
    if children_identical && missing.is_empty() && extra.is_empty() {
        println!("== 结论 == 完全一致（干净库）：index==scan，索引更快，children_map 亦一致。");
    } else if internally_consistent && index_is_superset {
        println!(
            "== 结论 == 语义差异（决策 A 安全）：索引=最新会话存活集、与单点查询自洽；children_map 上索引 ⊇ 扫描——只找回 scan 漏掉的 {} 个活 child、从不丢（仅扫描 owner=0、扫描被丢 child=0）⇒ 切换只会让生成更完整、不会缺件。",
            idx_child_added
        );
    } else if internally_consistent {
        println!(
            "== 结论 == 索引自洽（移植正确），但 children_map 出现『索引丢了 scan 的元素』（仅扫描 owner={} 扫描被丢 child={}），需核查。",
            owner_only_scan, scan_child_dropped
        );
        std::process::exit(2);
    } else {
        println!(
            "== 结论 == 需排查：建表与单点查询不自洽（still_in_btree={} find_eq_scan={} find_other={}）。",
            still_in_btree, find_eq_scan, find_other
        );
        std::process::exit(1);
    }
    Ok(())
}
