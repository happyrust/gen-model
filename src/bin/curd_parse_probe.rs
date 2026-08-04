//! CURD/DBLS 收集修复验收探针（gen-model-9 / ADR-006）。
//!
//! 背景：SYS 元数据里设计 MDB(如 /MHULLFWD, noun=MDB) 的 CURD（当前数据库列表）是跨块长引用列表，
//! 旧 collect_explict_data 遇不匹配块直接 break 丢块 → CURD 解析失败 → 设计 MDB/CURD 建不起来 → 空树。
//! 本探针只读解析 amssys，验证修复后：目标 MDB/DB 元素的 CURD/DBLS 能完整解析、MDB 普遍带 CURD。
//!
//! 用法：
//! cargo run --bin curd_parse_probe -- --file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\amssys"

use aios_core::RefU64;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "curd_parse_probe")]
struct Args {
    #[arg(short, long)]
    file: String,
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
}

fn dump_one(bytes: &[u8], pos: usize, refno: RefU64) {
    if pos < 4 || pos >= bytes.len() {
        println!("  [{}] pos 非法({})", refno.to_e3d_id(), pos);
        return;
    }
    match parse_pdms_db::parse::parse_raw_ele_data(&bytes[pos - 4..]) {
        Ok(ele) => {
            let merged = ele.whole_attmap.merge();
            let ty = merged.get_as_string("TYPE").unwrap_or_default();
            let name = merged.get_as_string("NAME").unwrap_or_default();
            println!(
                "  [{}] TYPE={} NAME={} explicit_cnt={}",
                refno.to_e3d_id(),
                ty.trim(),
                name.trim(),
                ele.whole_attmap.explicit_attmap.len()
            );
            for key in ["CURD", "DBLS", "DBNO", "STYP"] {
                if let Some(v) = merged.get_val(key) {
                    let s = format!("{:?}", v);
                    let short = if s.len() > 160 { &s[..160] } else { &s[..] };
                    println!(
                        "      {} = {}{}",
                        key,
                        short,
                        if s.len() > 160 { " ..." } else { "" }
                    );
                } else {
                    println!("      {} = <missing>", key);
                }
            }
        }
        Err(e) => println!("  [{}] parse err: {}", refno.to_e3d_id(), e),
    }
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
    println!("== file == {}", path.display());

    let db_basic =
        parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, &args.project)?;
    let bytes = db_basic.bytes;
    let (tbl, world) = parse_pdms_db::parse::gen_ref_type_pos_table(&bytes);
    println!(
        "== table == {} entries, world={}",
        tbl.len(),
        world.to_e3d_id()
    );

    println!("== target elements ==");
    for (r0, r1) in [
        (24575u32, 409u32),
        (24575, 1309),
        (24575, 1478),
        (24575, 1494),
    ] {
        let refno = RefU64::from_two_nums(r0, r1);
        match tbl.get(&refno) {
            Some(e) => dump_one(&bytes, e.pos, refno),
            None => println!("  [{}/{}] not in latest-session table", r0, r1),
        }
    }

    let mut mdb_total = 0usize;
    let mut mdb_with_curd = 0usize;
    let mut db_total = 0usize;
    let mut db_with_styp = 0usize;
    let mut parse_err = 0usize;
    for e in tbl.iter() {
        let pos = e.value().pos;
        if pos < 4 {
            continue;
        }
        match parse_pdms_db::parse::parse_raw_ele_data(&bytes[pos - 4..]) {
            Ok(ele) => {
                let merged = ele.whole_attmap.merge();
                match merged.get_as_string("TYPE").unwrap_or_default().trim() {
                    "MDB" => {
                        mdb_total += 1;
                        if merged.get_val("CURD").is_some() {
                            mdb_with_curd += 1;
                        }
                    }
                    "DB" => {
                        db_total += 1;
                        if merged.get_val("STYP").is_some() {
                            db_with_styp += 1;
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => parse_err += 1,
        }
    }
    println!(
        "== coverage == MDB={} with_CURD={} | DB={} with_STYP={} | parse_err={}",
        mdb_total, mdb_with_curd, db_total, db_with_styp, parse_err
    );
    Ok(())
}
