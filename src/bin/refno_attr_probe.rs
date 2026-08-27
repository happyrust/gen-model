//! Dumps every attribute one Dabacon record yields, for one refno.
//!
//! The schema is read from a file rather than from `aios_core`, so the table
//! under test can be swapped without rebuilding and without depending on the
//! `load_file` feature being wired up in this crate's pinned revision.
//!
//! UDA attributes come back from the synchronous parse with their name still
//! set to `_UDAS`; only `hash_val` is meaningful there. `db1_dehash` turns that
//! hash back into the `:NAME` short form offline, which is what this probe
//! prints — resolving the full user-defined name needs the dictionary database.
//!
//! ```text
//! cargo run --release --bin refno_attr_probe -- \
//!   --file "D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001" \
//!   --refno 24383/73958 --schema all_attr_info.json
//! ```

use aios_core::RefU64;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::types::db_info::PdmsDatabaseInfo;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "refno_attr_probe")]
struct Args {
    /// Dabacon db file.
    #[arg(short, long)]
    file: String,
    /// `WORD0/WORD1`, as E3D prints it after `q ref`.
    #[arg(short, long)]
    refno: String,
    /// Schema file; omit to use whatever `aios_core` has embedded.
    #[arg(short, long)]
    schema: Option<String>,
    /// Snapshot of this project's UDA definitions, from
    /// `e3d-descriptor emit-uda-table`. Omit and the `:` attributes are absent,
    /// which is what the runtime shows without a Dictionary lookup.
    #[arg(short, long)]
    uda_table: Option<String>,
    #[arg(short, long, default_value = "AvevaMarineSample")]
    project: String,
    /// Instead of one refno, walk every indexed record and report which ones
    /// store UDA bytes. Answers "is the stored-value path reachable on this
    /// data at all", which one element cannot.
    #[arg(long)]
    scan_uda: bool,
    /// How many stored-UDA elements to list during a scan.
    #[arg(long, default_value_t = 10)]
    scan_examples: usize,
}

fn parse_refno(text: &str) -> anyhow::Result<RefU64> {
    let (left, right) = text
        .split_once(['/', '_'])
        .ok_or_else(|| anyhow::anyhow!("refno must look like 24383/73958"))?;
    let word0: u64 = left.trim_start_matches('=').trim().parse()?;
    let word1: u64 = right.trim().parse()?;
    Ok(RefU64((word0 << 32) | word1))
}

fn load_schema(path: &str) -> anyhow::Result<PdmsDatabaseInfo> {
    let mut info: PdmsDatabaseInfo = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    info.fill_named_map();
    Ok(info)
}

fn scan_uda(bytes: &[u8], schema: &PdmsDatabaseInfo, examples: usize) -> anyhow::Result<()> {
    let (records, _) = parse_pdms_db::parse::gen_ref_type_pos_table(bytes);
    let mut parsed = 0usize;
    let mut with_uda = 0usize;
    let mut uda_values = 0usize;
    let mut by_noun: std::collections::BTreeMap<String, usize> = Default::default();
    let mut shown = 0usize;
    for record in records.iter() {
        if record.value().pos < 4 {
            continue;
        }
        let Ok(ele) =
            parse_pdms_db::parse::parse_raw_ele_data_with_info(&bytes[record.value().pos - 4..], schema)
        else {
            continue;
        };
        parsed += 1;
        let count = ele.whole_attmap.uda_atts.len();
        if count == 0 {
            continue;
        }
        with_uda += 1;
        uda_values += count;
        *by_noun.entry(db1_dehash(ele.noun)).or_default() += 1;
        if shown < examples {
            shown += 1;
            println!("{:?} {} stores {count} UDA:", ele.refno, db1_dehash(ele.noun));
            for attr in ele.whole_attmap.uda_atts.iter() {
                println!(
                    "    hash={:<12} short={:<10} {:?}",
                    attr.hash_val,
                    db1_dehash(attr.hash_val as u32),
                    attr.value
                );
            }
        }
    }
    println!(
        "\nindexed={} parsed={parsed} with_stored_uda={with_uda} uda_values={uda_values}",
        records.len()
    );
    println!("by noun: {by_noun:?}");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let target = parse_refno(&args.refno)?;

    let owned = args.schema.as_deref().map(load_schema).transpose()?;
    let schema: &PdmsDatabaseInfo = owned
        .as_ref()
        .unwrap_or_else(|| aios_core::get_default_pdms_db_info());
    let pairs: usize = schema
        .noun_attr_info_map
        .iter()
        .map(|noun| noun.value().len())
        .sum();
    println!(
        "schema: {} ({} nouns / {pairs} pairs)",
        args.schema.as_deref().unwrap_or("<embedded in aios_core>"),
        schema.noun_attr_info_map.len()
    );

    let path = PathBuf::from(&args.file);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let db = parse_pdms_db::parse::parse_file_db_basic_data(&path, file_name, &args.project)?;

    if args.scan_uda {
        return scan_uda(&db.bytes, schema, args.scan_examples);
    }

    let entry = parse_pdms_db::refno_index::find_refno_entry(&db.bytes, target)
        .ok_or_else(|| anyhow::anyhow!("{} is not in the latest-session index", args.refno))?;
    println!(
        "record: pos={} noun_hash={} noun={}",
        entry.pos,
        entry.noun_hash,
        db1_dehash(entry.noun_hash as u32)
    );

    let ele = parse_pdms_db::parse::parse_raw_ele_data_with_info(&db.bytes[entry.pos - 4..], schema)?;
    println!(
        "element: refno={:?} owner={:?} noun={} ({}) name={:?} children={}",
        ele.refno,
        ele.owner,
        ele.noun,
        db1_dehash(ele.noun),
        ele.name,
        ele.children.len()
    );

    println!("\n--- implicit ({}) ---", ele.whole_attmap.attmap.map.len());
    for (name, value) in ele.whole_attmap.attmap.map.iter() {
        println!("  {name:<12} {value:?}");
    }

    println!(
        "\n--- explicit ({}) ---",
        ele.whole_attmap.explicit_attmap.map.len()
    );
    for (name, value) in ele.whole_attmap.explicit_attmap.map.iter() {
        println!("  {name:<12} {value:?}");
    }

    println!("\n--- uda stored on the record ({}) ---", ele.whole_attmap.uda_atts.len());
    for attr in ele.whole_attmap.uda_atts.iter() {
        println!(
            "  hash={:<12} short={:<10} parsed_name={:<8} {:?}",
            attr.hash_val,
            db1_dehash(attr.hash_val as u32),
            attr.name,
            attr.value
        );
    }

    let uda = match args.uda_table.as_deref() {
        Some(path) => aios_database::uda_table::UdaTable::load(path)?,
        None => aios_database::uda_table::UdaTable::default(),
    };
    println!(
        "\nuda table: {} ({} definitions / {} nouns)",
        args.uda_table.as_deref().unwrap_or("<none>"),
        uda.len(),
        uda.noun_count()
    );

    let view = aios_database::uda_table::full_attribute_view(&ele, schema, &uda);
    let uda_shown = view.map.keys().filter(|key| key.starts_with(':')).count();
    println!(
        "\n--- full view ({} attributes, {uda_shown} of them UDA) ---",
        view.map.len()
    );
    for (name, value) in view.map.iter() {
        println!("  {name:<20} {}", view.get_as_string(name).unwrap_or_default());
        let _ = value;
    }
    Ok(())
}
