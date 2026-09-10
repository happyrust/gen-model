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
}
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let header = DbFileHeader::read(&args.file)?;
    let schema = DirectSchema::open_from_env()?;
    let set = open_pinned_set(&schema, &args.file, &header.db_type, header.latest_sesno())?;
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
