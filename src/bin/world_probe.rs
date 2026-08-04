//! WORL 定位诊断：为什么某些 DESI 库建表拿得到元素、children 遍历却只有 1 个 owner。
//!
//! `parse_db_basic_data` 的层级遍历以 `world_refno` 为根，逐层调用 `parse_ele_membs`。
//! 只要根的成员解析为空，整棵树就只剩一个伪根，最终解析出 0 个元素且不报错。
//! 本探针打印两条建表路径各自的 world 判定、noun 直方图，对各层级元素抽样调用
//! `parse_ele_membs`，并把 WORL 在文件里的**全部物理记录**逐条列出（位置 + 成员数），
//! 用来区分「库里真没有层级」「层级存在但成员没解析出来」「建表选中的 WORL 记录不带成员」。
//!
//! 用法：
//! ```text
//! cargo run --bin world_probe -- --file "D:\AVEVA\Projects\E3D3.1\TEST\TES000\TES1002_0001" --project TEST
//! ```

use aios_core::RefU64;
use aios_core::db::EleDataEntry;
use clap::Parser;
use dashmap::DashMap;
use std::collections::HashMap;
use std::path::PathBuf;

/// 与 `parse.rs` / `refno_index.rs` 保持一致的 WORL noun 哈希。
const WORLD_NOUN: i32 = 0x000B_EB83;

/// 抽样成员解析时关心的层级类型（自顶向下）。
const MEMBER_SAMPLE_TYPES: &[&str] = &[
    "WORL", "SITE", "ZONE", "STRU", "FRMW", "PIPE", "BRAN", "SBFR",
];

#[derive(Parser, Debug)]
#[command(name = "world_probe")]
#[command(about = "WORL 定位诊断：world_refno / noun 直方图 / 成员解析抽样 / WORL 全部物理记录")]
struct Args {
    /// db 文件路径。
    #[arg(short, long)]
    file: String,
    /// 工程名（parse 语义需要）。
    #[arg(short, long, default_value = "TEST")]
    project: String,
    /// noun 直方图打印前 N 项。
    #[arg(long, default_value_t = 15)]
    top: usize,
    /// WORL 物理记录最多打印多少条。
    #[arg(long, default_value_t = 40)]
    records: usize,
}

fn noun_histogram(table: &DashMap<RefU64, EleDataEntry>) -> Vec<(i32, usize)> {
    let mut counts: HashMap<i32, usize> = HashMap::new();
    for entry in table.iter() {
        *counts.entry(entry.value().noun_hash).or_default() += 1;
    }
    let mut ordered = counts.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.1.cmp(&a.1));
    ordered
}

fn sample_refno(table: &DashMap<RefU64, EleDataEntry>, noun_hash: i32) -> Option<(RefU64, usize)> {
    table
        .iter()
        .find(|entry| entry.value().noun_hash == noun_hash)
        .map(|entry| (*entry.key(), entry.value().pos))
}

async fn type_name(bytes: &[u8], pos: usize, db_info: &aios_core::PdmsDatabaseInfo) -> String {
    if pos < 4 {
        return "pos<4".to_string();
    }
    match parse_pdms_db::parse::parse_ele_data_with_info(&bytes[pos - 4..], db_info).await {
        Ok(ele) => ele.whole_attmap.merge().get_type_str().trim().to_string(),
        Err(error) => format!("<解析失败: {error}>"),
    }
}

fn head_hex(bytes: &[u8], pos: usize, len: usize) -> String {
    if pos < 4 {
        return "pos<4".to_string();
    }
    bytes[pos - 4..(pos - 4 + len).min(bytes.len())]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 全文件按 4 字节对齐扫描 `[ref0][ref1][noun]` 签名，列出该 refno 的所有物理记录位置。
fn physical_records(bytes: &[u8], refno: RefU64, noun_hash: i32) -> Vec<usize> {
    let mut signature = Vec::with_capacity(12);
    signature.extend_from_slice(&refno.get_0().to_be_bytes());
    signature.extend_from_slice(&refno.get_1().to_be_bytes());
    signature.extend_from_slice(&noun_hash.to_be_bytes());

    let mut found = Vec::new();
    let mut pos = 4usize;
    while pos + signature.len() <= bytes.len() {
        if bytes[pos..pos + signature.len()] == signature[..] {
            found.push(pos);
        }
        pos += 4;
    }
    found
}

async fn report(
    label: &str,
    bytes: &[u8],
    table: &DashMap<RefU64, EleDataEntry>,
    world: RefU64,
    top: usize,
    db_info: &aios_core::PdmsDatabaseInfo,
) {
    let has_world_noun = table
        .iter()
        .any(|entry| entry.value().noun_hash == WORLD_NOUN);
    println!(
        "== {label} == 元素 {} 条 | world_refno={:?} | 表内在={} | 存在 WORLD_NOUN({:#x}) 的元素={}",
        table.len(),
        world,
        table.contains_key(&world),
        WORLD_NOUN,
        has_world_noun
    );

    // noun → 类型名，同时给成员抽样准备一张反查表。
    let mut name_of_noun: HashMap<i32, String> = HashMap::new();
    for (noun_hash, count) in noun_histogram(table) {
        let name = match sample_refno(table, noun_hash) {
            Some((_, pos)) => type_name(bytes, pos, db_info).await,
            None => "<无样本>".to_string(),
        };
        name_of_noun.insert(noun_hash, name.clone());
        if top > 0 && name_of_noun.len() <= top {
            println!("   noun={noun_hash:#x} 数量={count} 类型名={name}");
        }
    }

    println!("-- {label} 成员解析抽样 --");
    for wanted in MEMBER_SAMPLE_TYPES {
        let Some((noun_hash, _)) = name_of_noun.iter().find(|(_, name)| name == wanted) else {
            println!("   {wanted}: 本表内无此类型");
            continue;
        };
        let Some((refno, pos)) = sample_refno(table, *noun_hash) else {
            continue;
        };
        let membs = parse_pdms_db::parse::parse_ele_membs(&bytes[pos - 4..]);
        let in_table = membs.iter().filter(|m| table.contains_key(*m)).count();
        println!(
            "   {wanted}: refno={refno:?} pos={pos} 成员={} 其中在表内={} head=[{}]",
            membs.len(),
            in_table,
            head_hex(bytes, pos, 24)
        );
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let path = PathBuf::from(&args.file);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    println!("== 文件 == {}", path.display());

    let db_basic =
        parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &args.project)?;
    println!(
        "== parse_db_basic_data == world_refno={:?} refno_table={} 条 children_map={} 个 owner",
        db_basic.world_refno,
        db_basic.refno_table_map.len(),
        db_basic.children_map.len()
    );
    let bytes = db_basic.bytes;
    let world = db_basic.world_refno;
    let db_info = aios_core::get_default_pdms_db_info();

    let (scan_table, scan_world) = parse_pdms_db::parse::gen_ref_type_pos_table_scan(&bytes);
    report("扫描表", &bytes, &scan_table, scan_world, args.top, db_info).await;

    match parse_pdms_db::gen_ref_type_pos_table_from_index(&bytes) {
        Some((index_table, index_world)) => {
            report(
                "索引表",
                &bytes,
                &index_table,
                index_world,
                args.top,
                db_info,
            )
            .await;
        }
        None => println!("== 索引表 == 不可用（from_index=None），生产回退扫描"),
    }

    let occurrences = physical_records(&bytes, world, WORLD_NOUN);
    println!(
        "== WORL 物理记录 == refno={world:?} 共 {} 条（按文件位置升序，只打印前 {}）",
        occurrences.len(),
        args.records
    );
    let mut with_members = 0usize;
    let mut max_members = 0usize;
    let mut best_pos = 0usize;
    for pos in &occurrences {
        let membs = parse_pdms_db::parse::parse_ele_membs(&bytes[pos - 4..]);
        if !membs.is_empty() {
            with_members += 1;
        }
        if membs.len() > max_members {
            max_members = membs.len();
            best_pos = *pos;
        }
    }
    for pos in occurrences.iter().take(args.records) {
        let membs = parse_pdms_db::parse::parse_ele_membs(&bytes[pos - 4..]);
        println!(
            "   pos={pos} 成员={} head=[{}]",
            membs.len(),
            head_hex(&bytes, *pos, 16)
        );
    }
    println!(
        "== 小结 == WORL 物理记录 {} 条，其中带成员的 {} 条；成员最多的一条 pos={best_pos} 成员={max_members}",
        occurrences.len(),
        with_members
    );
    Ok(())
}
