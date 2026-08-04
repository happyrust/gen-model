//! 只读探针：拿 ADR-008 的寻址模型重建一份 `PdmsDatabaseInfo`，用它去解析真库元素，
//! 和用 `aios_core` 内嵌快照解析的结果**逐属性比值**。
//!
//! 前面的对拍都停在 offset 层面；这里才是端到端：偏移算对不代表值读对（还有
//! f32 启发式、表达式属性、BOOL 取位等一堆分支）。
//!
//! 跑：`cargo run --bin noun_layout_parse_probe -- <db文件> <项目名>`

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use aios_core::get_default_pdms_db_info;
use aios_core::types::db_info::PdmsDatabaseInfo;
use aios_database::noun_layout::{LayoutNoun, to_attr_infos};
use dashmap::DashMap;

const LAYOUT: &str = "output/noun_layout.json";

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let file = args.next().unwrap_or_else(|| {
        r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams251181_0001".to_string()
    });
    let project = args.next().unwrap_or_else(|| "ams".to_string());

    let layout: Vec<LayoutNoun> = serde_json::from_str(&std::fs::read_to_string(LAYOUT)?)?;
    let snap = get_default_pdms_db_info();

    // 快照用缩写、导出用全名，按属性 hash 重叠度配对。
    let mut by_attr: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, t) in layout.iter().enumerate() {
        for a in &t.attrs {
            by_attr.entry(a.hash).or_default().push(i);
        }
    }

    // 用快照的占槽集 + 导出的有序表重建偏移表。占槽集只能沿用快照——它无法
    // 从字典判定（ADR-008），所以这个探针验的是“寻址模型”而非“占槽判据”。
    let rebuilt = PdmsDatabaseInfo::default();
    let mut built = 0usize;
    for kv in snap.noun_attr_info_map.iter() {
        let noun_hash = *kv.key();
        let attrs = kv.value();
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
            continue;
        };
        let slotted: HashSet<i32> = attrs
            .iter()
            .filter(|a| a.offset & 0x000F_FFFF > 0)
            .map(|a| *a.key())
            .collect();
        let Ok(infos) = to_attr_infos(&layout[best], &slotted) else {
            continue;
        };
        if infos.is_empty() {
            continue;
        }
        let inner: DashMap<i32, aios_core::pdms_types::AttrInfo> = DashMap::new();
        for i in infos {
            inner.insert(i.hash, i);
        }
        rebuilt.noun_attr_info_map.insert(noun_hash, inner);
        built += 1;
    }
    let mut rebuilt = rebuilt;
    rebuilt.fill_named_map();
    println!("rebuilt info: {built} nouns");

    let path = PathBuf::from(&file);
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let db_basic = parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &project)?;
    let bytes = db_basic.bytes;
    let (tbl, _world) = parse_pdms_db::parse::gen_ref_type_pos_table(&bytes);
    println!("elements in latest session: {}", tbl.len());

    let (mut both_ok, mut only_snap, mut only_new, mut neither) = (0usize, 0usize, 0usize, 0usize);
    let (mut same, mut diff) = (0usize, 0usize);
    let mut diff_by_attr: HashMap<String, usize> = HashMap::new();
    let mut samples: Vec<String> = Vec::new();

    for e in tbl.iter() {
        let pos = e.value().pos;
        if pos < 4 {
            continue;
        }
        let slice = &bytes[pos - 4..];
        let a = parse_pdms_db::parse::parse_raw_ele_data_with_info(slice, &snap);
        let b = parse_pdms_db::parse::parse_raw_ele_data_with_info(slice, &rebuilt);
        match (a, b) {
            (Ok(x), Ok(y)) => {
                both_ok += 1;
                for (k, v) in x.whole_attmap.attmap.iter() {
                    match y.whole_attmap.attmap.get(k) {
                        Some(w) if format!("{v:?}") == format!("{w:?}") => same += 1,
                        Some(w) => {
                            diff += 1;
                            *diff_by_attr.entry(k.to_string()).or_default() += 1;
                            if samples.len() < 10 {
                                samples.push(format!("  {k}: snap={v:?} new={w:?}"));
                            }
                        }
                        None => {
                            diff += 1;
                            *diff_by_attr.entry(format!("{k} (missing)")).or_default() += 1;
                        }
                    }
                }
            }
            (Ok(_), Err(_)) => only_snap += 1,
            (Err(_), Ok(_)) => only_new += 1,
            (Err(_), Err(_)) => neither += 1,
        }
    }

    println!();
    println!(
        "parsed by both: {both_ok} | only snapshot: {only_snap} | only rebuilt: {only_new} | neither: {neither}"
    );
    let tot = same + diff;
    println!(
        "implicit attribute values identical: {same} / {tot} = {:.2}%",
        100.0 * same as f64 / tot.max(1) as f64
    );
    if !diff_by_attr.is_empty() {
        let mut v: Vec<_> = diff_by_attr.into_iter().collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        println!();
        println!("attributes that differ (top 12):");
        for (k, n) in v.into_iter().take(12) {
            println!("  {k:<20} {n}");
        }
        println!();
        for s in samples {
            println!("{s}");
        }
    }
    Ok(())
}
