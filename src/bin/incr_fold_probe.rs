//! 增量窗口重复写探针（P1「按 refno 折叠」可行性验收，只读）。
//!
//! `IncrementPipeline::collect_changes` 是无副作用的纯文件解析——不连 SurrealDB、
//! 不写任何东西——所以可以直接在真实工程文件上量出：一个 sesno 窗口内同一 refno 被
//! 重复落库多少次、折叠后语句数与 SQL 体积能降到多少。用来判断折叠这项优化值不值得
//! 做，而不是靠语句数估算。
//!
//! 用法：
//! ```text
//! cargo run --bin incr_fold_probe -- \
//!   --file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys" --to 169 --dbnum 8191
//! ```

use aios_core::RefU64;
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::model_impact::{classify_operation_effects, owner_change};
use clap::Parser;
use pdms_io::io::EleOperationDetail;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(name = "incr_fold_probe")]
#[command(about = "量化增量窗口内同一 refno 的重复落库规模（只读，不连库）")]
struct Args {
    /// db 文件路径。
    #[arg(short, long)]
    file: String,
    /// 窗口起始 sesno。
    #[arg(long, default_value_t = 1)]
    from: i32,
    /// 窗口结束 sesno。
    #[arg(long)]
    to: i32,
    /// 该文件的 dbnum，仅用于渲染 Add 分支的 pe 记录，不影响统计口径。
    #[arg(long, default_value_t = 0)]
    dbnum: i32,
}

/// 一个 refno 在窗口内的操作类型序列（按 sesno 升序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Add,
    Modified,
    Deleted,
}

fn kind_of(detail: &EleOperationDetail) -> Option<Kind> {
    match detail {
        EleOperationDetail::Add(_) => Some(Kind::Add),
        EleOperationDetail::Modified(_) => Some(Kind::Modified),
        EleOperationDetail::Deleted => Some(Kind::Deleted),
        EleOperationDetail::None => None,
    }
}

/// 保守折叠：只把「同一 refno 的连续 Modified」压成 1 条，Add / Deleted 原样保留。
/// 这是能在不改变 Add 先建记录、Deleted 后立墓碑这两条语义的前提下拿到的收益。
fn conservative_len(seq: &[Kind]) -> usize {
    let mut n = 0usize;
    let mut prev: Option<Kind> = None;
    for &k in seq {
        if k == Kind::Modified && prev == Some(Kind::Modified) {
            continue;
        }
        n += 1;
        prev = Some(k);
    }
    n
}

/// 激进折叠：窗口末态若是 Deleted，则整条序列只留墓碑；否则留「首个 Add（若有）+ 一条
/// 合并后的 Modified」。仅用于给出收益上界，语义变化需另行确认。
fn aggressive_len(seq: &[Kind]) -> usize {
    match seq.last() {
        Some(Kind::Deleted) => 1,
        _ => {
            let has_add = seq.contains(&Kind::Add);
            let has_mod = seq.contains(&Kind::Modified);
            usize::from(has_add) + usize::from(has_mod)
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = PathBuf::from(&args.file);
    println!("== 文件 == {}", path.display());
    println!("== 窗口 == {}..={}", args.from, args.to);

    let t0 = Instant::now();
    let range_eles = IncrementPipeline::collect_changes(&path, args.from..=args.to)?;
    println!("== collect_changes == {}ms", t0.elapsed().as_millis());

    // 按 refno 归集操作序列，同时按当前「逐会话回放」口径渲染一次 SQL 量真实体积。
    let mut seqs: HashMap<RefU64, Vec<Kind>> = HashMap::new();
    let mut adds = 0usize;
    let mut mods = 0usize;
    let mut dels = 0usize;
    let mut nones = 0usize;
    let mut sql_bytes = 0usize;
    let mut statements = 0usize;
    let mut owner_moves = Vec::new();
    let mut additions = Vec::new();
    let mut modifications = Vec::new();
    let mut deletions = Vec::new();

    for (&sesno, elements) in &range_eles {
        for element in elements {
            let (old_owner, new_owner) = owner_change(element);
            if old_owner.is_some() || new_owner.is_some() {
                owner_moves.push((
                    sesno,
                    element.refno,
                    element.get_noun_type(),
                    old_owner,
                    new_owner,
                ));
            }
            if let EleOperationDetail::Modified(modified) = &element.detail {
                let effects = classify_operation_effects(element);
                modifications.push((
                    sesno,
                    element.refno,
                    modified.noun.clone(),
                    modified.current_data.owner,
                    effects.changed_attributes,
                    effects.children_delta,
                ));
            }
            match kind_of(&element.detail) {
                Some(k) => {
                    match k {
                        Kind::Add => {
                            adds += 1;
                            if let EleOperationDetail::Add(added) = &element.detail {
                                additions.push((
                                    sesno,
                                    element.refno,
                                    element.get_noun_type(),
                                    added.owner,
                                ));
                            }
                        }
                        Kind::Modified => mods += 1,
                        Kind::Deleted => {
                            dels += 1;
                            deletions.push((sesno, element.refno, element.get_noun_type()));
                        }
                    }
                    seqs.entry(element.refno).or_default().push(k);
                }
                None => {
                    nones += 1;
                    continue;
                }
            }
            let id = element.refno.to_string();
            let surql = element.to_surql(&id, args.dbnum, sesno);
            if !surql.is_empty() {
                statements += 1;
                sql_bytes += surql.len();
            }
        }
    }

    let distinct = seqs.len();
    let total_ops = adds + mods + dels;
    println!(
        "== 规模 == 会话数={} 操作总数={}（Add {} / Modified {} / Deleted {} / None {}） 去重 refno={}",
        range_eles.len(),
        total_ops,
        adds,
        mods,
        dels,
        nones,
        distinct
    );
    println!(
        "== 当前口径 == 落库语句组={} SQL 体积={:.2} MB",
        statements,
        sql_bytes as f64 / 1024.0 / 1024.0
    );
    println!("== OWNER 搬迁 == {} 条", owner_moves.len());
    for (sesno, refno, noun, old_owner, new_owner) in &owner_moves {
        println!("   sesno={sesno} refno={refno} noun={noun} owner={old_owner:?} -> {new_owner:?}");
    }
    println!("== Add 明细 == {} 条", additions.len());
    for (sesno, refno, noun, owner) in &additions {
        println!("   sesno={sesno} refno={refno} noun={noun} owner={owner}");
    }
    println!("== Modified 明细 == {} 条", modifications.len());
    for (sesno, refno, noun, owner, attributes, children_delta) in &modifications {
        println!(
            "   sesno={sesno} refno={refno} noun={noun} current_owner={owner} attrs={attributes:?} children={children_delta:?}"
        );
    }
    println!("== Deleted 明细 == {} 条", deletions.len());
    for (sesno, refno, noun) in &deletions {
        println!("   sesno={sesno} refno={refno} noun={noun}");
    }

    if distinct == 0 {
        println!("== 结论 == 窗口内没有任何变更，无从折叠。");
        return Ok(());
    }

    // 重复度分布：一个 refno 在窗口里被写了几次。
    let mut hist: HashMap<usize, usize> = HashMap::new();
    let mut max_seq = 0usize;
    let mut max_refno = None;
    let mut consecutive_mod_total = 0usize;
    let mut aggressive_total = 0usize;
    for (refno, seq) in &seqs {
        *hist.entry(seq.len()).or_default() += 1;
        if seq.len() > max_seq {
            max_seq = seq.len();
            max_refno = Some(*refno);
        }
        consecutive_mod_total += conservative_len(seq);
        aggressive_total += aggressive_len(seq);
    }

    let mut buckets: Vec<(usize, usize)> = hist.into_iter().collect();
    buckets.sort_by_key(|(times, _)| *times);
    println!("== 重复度分布 ==（写入次数 → 有多少个 refno）");
    for (times, count) in buckets.iter().take(15) {
        println!("   {times:>4} 次: {count} 个 refno");
    }
    if buckets.len() > 15 {
        let tail: usize = buckets[15..].iter().map(|(_, c)| *c).sum();
        println!("   ... 其余 {} 档共 {} 个 refno", buckets.len() - 15, tail);
    }
    println!(
        "== 最热 refno == {:?} 在本窗口被写 {} 次",
        max_refno, max_seq
    );

    let dup_ratio = total_ops as f64 / distinct as f64;
    println!("== 平均重复度 == {dup_ratio:.2} 次/refno");

    let save_cons = total_ops.saturating_sub(consecutive_mod_total);
    let save_aggr = total_ops.saturating_sub(aggressive_total);
    println!(
        "== 保守折叠（仅合并连续 Modified）== {} → {} 条，省 {} 条（{:.1}%）",
        total_ops,
        consecutive_mod_total,
        save_cons,
        save_cons as f64 / total_ops.max(1) as f64 * 100.0
    );
    println!(
        "== 激进折叠（末态 Deleted 只留墓碑）== {} → {} 条，省 {} 条（{:.1}%）",
        total_ops,
        aggressive_total,
        save_aggr,
        save_aggr as f64 / total_ops.max(1) as f64 * 100.0
    );

    let verdict = if save_cons * 5 >= total_ops {
        "值得做：保守折叠即可砍掉两成以上语句。"
    } else if save_aggr * 5 >= total_ops {
        "保守折叠收益有限，收益主要来自末态 Deleted 折叠——需先确认墓碑语义。"
    } else {
        "不值得做：该窗口内几乎没有重复写，折叠省不下东西，应把精力放在别处。"
    };
    println!("== 结论 == {verdict}");
    Ok(())
}
