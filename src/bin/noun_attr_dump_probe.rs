//! 取证探针：把指定 noun 的真库元素**完整属性面**dump 出来。
//!
//! 用途：#34 的 SCTN 关节（`SJOI` / `PJOI` / `SUBJ`）与 #35 的 `EXTGEO` 段在 vendor
//! e3d-model 里都还没接——接之前先要知道它们在真实 DESI 库里长什么样：
//! 有没有 POS/ORI、挂什么参照属性（`CREF` / `STAREF` / `ENDREF` / `CATREF` / `SPRE`）、
//! 名下有没有 TMPL 成员或负体子树。这些都从属性面直接读。
//!
//! 只读文件，不连 Surreal、不写库。每文件 `ReadOnlyEngine::scan_elements` 过一遍找目标
//! refno，有目标才开 `DbSet`（attlib + DESI 模板）逐元素打：refno、owner 链（三跳）、
//! 成员 noun 统计、全部 `attribute_names()` 逐名取值（取不到标 `<err>`）。
//!
//! 用法：
//! ```text
//! cargo run --release --bin noun_attr_dump_probe -- \
//!     --dir "D:/AVEVA/Projects/E3D3.1/AvevaPlantSample/aps000" \
//!     --dir "D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000" \
//!     --nouns SJOI,PJOI,SUBJ,EXTGEO,CABLE --per-noun 3
//! ```
//! 环境：`AIOS_E3D_TEMPLATE_DIR` 或默认 `vendor/e3d-io/schema/e3d31`。

use std::collections::HashMap;
use std::path::PathBuf;

use std::sync::Arc;

use e3d_io::db_element::{DbFilePin, DbSet};
use e3d_io::{ReadOnlyEngine, RefNo, ScanTier};

fn parse_args() -> (Vec<PathBuf>, Vec<String>, Vec<RefNo>, usize) {
    let mut dirs = Vec::new();
    let mut nouns = Vec::new();
    let mut refnos = Vec::new();
    let mut per_noun = 3usize;
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
            "--refno" => {
                for text in args.next().expect("--refno 要一串逗号分隔的号").split(',') {
                    let (a, b) = text
                        .split_once('/')
                        .or_else(|| text.split_once('_'))
                        .unwrap_or_else(|| panic!("refno 要写成 word0/word1：{text}"));
                    refnos.push(RefNo::new(
                        a.trim().parse().expect("refno word0 不是数"),
                        b.trim().parse().expect("refno word1 不是数"),
                    ));
                }
            }
            "--per-noun" => {
                per_noun = args
                    .next()
                    .expect("--per-noun 要一个数")
                    .parse()
                    .expect("--per-noun 不是数")
            }
            other => panic!("不认识的参数 {other}"),
        }
    }
    (dirs, nouns, refnos, per_noun)
}

fn template_dir() -> PathBuf {
    std::env::var("AIOS_E3D_TEMPLATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/vendor/shared/e3d-io/schema/e3d31/binaries"
            )
            .into()
        })
}

fn owner_chain(mut el: e3d_io::db_element::DbElement, depth: usize) -> String {
    let mut chain = Vec::new();
    for _ in 0..depth {
        match el.owner() {
            Ok(Some(owner)) => {
                let noun = owner.element_type().unwrap_or_else(|_| "?".into());
                chain.push(format!("{} `{}`", owner.refno(), noun));
                el = owner;
            }
            _ => break,
        }
    }
    if chain.is_empty() {
        "（无 owner）".to_string()
    } else {
        chain.join(" <- ")
    }
}

fn dump_element(set: &Arc<DbSet>, refno: RefNo, noun: &str) {
    let el = set.element(refno);
    println!("  {} `{}`", refno, noun);
    println!("    owner: {}", owner_chain(el.clone(), 3));
    match el.members() {
        Ok(members) => {
            let mut counts: BTreeMapWrap = BTreeMapWrap::default();
            for member in members {
                let m_noun = member.element_type().unwrap_or_else(|_| "?".into());
                *counts.0.entry(m_noun).or_insert(0usize) += 1;
            }
            let member_nouns = counts
                .0
                .iter()
                .map(|(n, c)| format!("{n}×{c}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "    members[{}]: {}",
                counts.0.values().sum::<usize>(),
                member_nouns
            );
        }
        Err(error) => println!("    members: <{error}>"),
    }
    match el.attribute_names() {
        Ok(names) => {
            for name in names {
                match el.attribute(&name) {
                    Ok(Some(value)) => println!("    {name} = {value:?}"),
                    Ok(None) => {}
                    Err(error) => println!("    {name} = <{error}>"),
                }
            }
        }
        Err(error) => println!("    attributes: <{error}>"),
    }
}

#[derive(Default)]
struct BTreeMapWrap(std::collections::BTreeMap<String, usize>);

fn main() -> anyhow::Result<()> {
    let (dirs, nouns, refnos, per_noun) = parse_args();
    if dirs.is_empty() || (nouns.is_empty() && refnos.is_empty()) {
        eprintln!("用法见文件头注释：至少要一个 --dir 和一串 --nouns（或 --refno 直查）");
        std::process::exit(2);
    }
    let desi = aios_core::tool::db_tool::db1_hash("DESI");
    let targets: HashMap<u32, String> = nouns
        .iter()
        .map(|noun| (aios_core::tool::db_tool::db1_hash(noun), noun.clone()))
        .collect();
    let mut wanted: std::collections::HashSet<RefNo> = refnos.iter().copied().collect();
    let attlib = template_dir().join("attlib.dat");
    let template_dir = template_dir();
    let cata = aios_core::tool::db_tool::db1_hash("CATA");

    let mut remaining: HashMap<String, usize> =
        nouns.iter().map(|n| (n.clone(), per_noun)).collect();

    for dir in &dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        for path in files {
            if wanted.is_empty() && remaining.values().all(|left| *left == 0) {
                break;
            }
            let Ok(mut engine) = ReadOnlyEngine::open(&path) else {
                continue;
            };
            // noun 普查看 DESI，refno 直查不限库型（目录链会落到 CATA）；
            // noun 普查带 CATA：目录侧 noun（SCOM/CABCOR/…）只在 CATA 里。
            if wanted.is_empty()
                && engine.descriptor().db_type_hash != desi
                && engine.descriptor().db_type_hash != cata
            {
                continue;
            }
            let found: Vec<(RefNo, Option<&String>)> = match engine.scan_elements(ScanTier::Header)
            {
                Ok(scan) => scan
                    .filter_map(|scanned| scanned.ok())
                    .filter_map(|scanned| {
                        if wanted.contains(&scanned.refno) {
                            Some((scanned.refno, None))
                        } else {
                            targets
                                .get(&scanned.noun_hash)
                                .map(|n| (scanned.refno, Some(n)))
                        }
                    })
                    .collect(),
                Err(_) => continue,
            };
            if found.is_empty() {
                continue;
            }
            let db_type = engine
                .descriptor()
                .db_type_name()
                .ok_or_else(|| anyhow::anyhow!("{} 的库类型无法识别", path.display()))?;
            let set = Arc::new(DbSet::with_attlib_file(&attlib)?);
            set.add_db(DbFilePin {
                file: path.clone(),
                template: e3d_io::db_element::template_file_for(&template_dir, &db_type)?,
                db_type: Some(db_type),
                sesno: None,
            })?;
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            println!("=== {name} ===");
            for (refno, noun) in found {
                let noun = match noun {
                    Some(noun) => {
                        let left = remaining.get_mut(noun.as_str()).unwrap();
                        if *left == 0 {
                            continue;
                        }
                        *left -= 1;
                        noun.clone()
                    }
                    None => set
                        .element(refno)
                        .element_type()
                        .unwrap_or_else(|_| "?".into()),
                };
                dump_element(&set, refno, &noun);
                wanted.remove(&refno);
            }
        }
    }
    let leftover: Vec<String> = remaining
        .iter()
        .filter(|(_, left)| **left > 0)
        .map(|(noun, left)| format!("{noun} 还差 {left} 个样本"))
        .collect();
    if !leftover.is_empty() {
        println!("\n未采满：{}", leftover.join("；"));
    }
    if !wanted.is_empty() {
        let mut missing: Vec<_> = wanted.iter().map(ToString::to_string).collect();
        missing.sort();
        anyhow::bail!("指定 refno 未找到：{}", missing.join(", "));
    }
    Ok(())
}
