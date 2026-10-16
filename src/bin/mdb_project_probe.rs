//! Read-only SYS declaration evidence; retain descriptor values before projection.
use aios_database::data_interface::{
    db_file_header::DbFileHeader, direct_store::DirectSchema, e3d_collector::open_pinned_set,
};
use clap::Parser;
#[derive(Parser)]
struct Args {
    #[arg(long)]
    file: std::path::PathBuf,
    #[arg(long, default_value = "/ALL")]
    mdb: String,
    #[arg(long)]
    dbno: Option<u32>,
}
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let header = DbFileHeader::read(&args.file)?;
    let schema = DirectSchema::open_from_env()?;
    let set = open_pinned_set(&schema, &args.file, &header.db_type, header.latest_sesno())?;
    if let Some(wanted) = args.dbno {
        let mut engine = e3d_io::ReadOnlyEngine::open(&args.file)?;
        let hash = aios_core::tool::db_tool::db1_hash("DB");
        let mut found = 0;
        for scanned in engine.scan_elements(e3d_io::ScanTier::Header)? {
            let scanned = scanned?;
            if scanned.noun_hash != hash {
                continue;
            }
            let element = set.element(scanned.refno);
            let dbno = match element.attribute("DBNO")? {
                Some(e3d_io::record::descriptor::DescriptorValue::Word { raw, .. }) => raw,
                Some(e3d_io::record::descriptor::DescriptorValue::Int(value)) if value >= 0 => {
                    value as u32
                }
                other => anyhow::bail!("{} 的 DBNO 不是库号：{other:?}", scanned.refno),
            };
            if dbno != wanted {
                continue;
            }
            found += 1;
            println!("REF {}", scanned.refno);
            for field in element.extraction()?.attributes.iter().filter(|a| {
                matches!(
                    a.name.as_str(),
                    "NAME" | "DBNO" | "PROJ" | "FINO" | "AREA" | "STYP"
                )
            }) {
                println!("{field:?}");
            }
        }
        if found == 0 {
            anyhow::bail!("SYS 中没有 DBNO={wanted} 的 DB 声明");
        }
        return Ok(());
    }
    for root in set.find_named(header.dbnum as u32, &args.mdb)? {
        let extraction = set.element(root).extraction()?;
        for attr in extraction.attributes.iter().filter(|a| a.name == "CURD") {
            if let Some(e3d_io::record::descriptor::DescriptorValue::RefNoArray(refs)) = &attr.value
            {
                for reference in refs {
                    let db = set.element(*reference).extraction()?;
                    println!("REF {}", reference);
                    for field in db.attributes.iter().filter(|a| {
                        matches!(
                            a.name.as_str(),
                            "NAME" | "DBNO" | "PROJ" | "FINO" | "AREA" | "STYP"
                        )
                    }) {
                        println!("{field:?}");
                    }
                }
            }
        }
    }
    Ok(())
}
