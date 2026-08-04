//! 只读探针：用 ADR-008 的寻址模型从 `output/noun_layout.json` 重算偏移，
//! 和 `aios_core` 内嵌快照 `all_attr_info.json` 逐属性对拍。
//!
//! 这是 Rust 侧的端到端校验：前面的结论都是 Python 模型算出来的，这里用
//! 真实的 `AttrInfo` / `PdmsDatabaseInfo` 结构重做一遍，顺便验证 `to_attr_infos`
//! 产出的东西真能填回解析器吃的那个 map。
//!
//! 跑：`cargo run --bin noun_layout_probe`

use std::collections::{HashMap, HashSet};

use aios_core::get_default_pdms_db_info;
use aios_database::noun_layout::{LayoutNoun, compute_offsets};

const LAYOUT: &str = "output/noun_layout.json";

fn main() -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(LAYOUT)
        .map_err(|e| anyhow::anyhow!("{LAYOUT} 读不到（先跑 scripts/e3d 导出）: {e}"))?;
    let layout: Vec<LayoutNoun> = serde_json::from_str(&raw)?;
    println!("layout: {} nouns", layout.len());

    let snap = get_default_pdms_db_info();
    println!("snapshot: {} nouns", snap.noun_attr_info_map.len());

    // 快照用 4-6 字缩写、导出用全名，名字对不上，改按属性 hash 集合重叠度配对。
    let mut by_attr: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, t) in layout.iter().enumerate() {
        for a in &t.attrs {
            by_attr.entry(a.hash).or_default().push(i);
        }
    }

    let (mut matched, mut exact, mut partial, mut unmatched) = (0usize, 0usize, 0usize, 0usize);
    let (mut attr_hit, mut attr_miss) = (0usize, 0usize);
    let mut worst: Vec<(String, usize, usize)> = Vec::new();

    for kv in snap.noun_attr_info_map.iter() {
        let attrs = kv.value();
        if attrs.is_empty() {
            continue;
        }
        let mut votes: HashMap<usize, usize> = HashMap::new();
        for a in attrs.iter() {
            if let Some(ids) = by_attr.get(a.key()) {
                for id in ids {
                    *votes.entry(*id).or_default() += 1;
                }
            }
        }
        let Some((&best, _)) = votes
            .iter()
            .max_by_key(|(id, v)| (**v, std::cmp::Reverse(layout[**id].attrs.len())))
        else {
            unmatched += 1;
            continue;
        };
        let t = &layout[best];
        matched += 1;

        // 快照里 offset 非零的就是占槽属性（低 20 位才是字偏移）。
        let slotted: HashSet<i32> = attrs
            .iter()
            .filter(|a| a.offset & 0x000F_FFFF > 0)
            .map(|a| *a.key())
            .collect();
        if slotted.len() < 2 {
            continue;
        }
        let Ok(computed) = compute_offsets(&t.attrs, &slotted) else {
            continue;
        };

        let (mut hit, mut miss) = (0usize, 0usize);
        for (hash, off) in &computed {
            match attrs.get(hash) {
                Some(want) if want.offset == *off => hit += 1,
                Some(_) => miss += 1,
                None => miss += 1,
            }
        }
        attr_hit += hit;
        attr_miss += miss;
        if miss == 0 {
            exact += 1;
        } else {
            partial += 1;
            if worst.len() < 10 {
                worst.push((t.noun.clone(), hit, miss));
            }
        }
    }

    println!();
    println!("paired with an exported type : {matched} (unmatched {unmatched})");
    println!("every offset reproduced      : {exact}");
    println!("some offsets differ          : {partial}");
    let tot = attr_hit + attr_miss;
    println!(
        "attribute offsets            : {attr_hit} / {tot} = {:.1}%",
        100.0 * attr_hit as f64 / tot.max(1) as f64
    );
    if !worst.is_empty() {
        println!();
        println!("examples that differ (noun, hit, miss):");
        for (n, h, m) in &worst {
            println!("  {n:<14} {h:>4} {m:>4}");
        }
    }
    Ok(())
}
