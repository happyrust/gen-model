//! Read-only diagnostic: verify referenced records in explicitly named sources.
use clap::Parser;
#[derive(Parser)]
struct Args {
    #[arg(long, required = true)]
    file: Vec<std::path::PathBuf>,
    #[arg(long, required = true)]
    reference: Vec<String>,
}
fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let refs = args
        .reference
        .iter()
        .map(|value| aios_database::fast_model::e3d_model_service::parse_refno(value))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut output = Vec::new();
    for path in &args.file {
        let header = aios_database::data_interface::db_file_header::DbFileHeader::read(path)?;
        let mut engine = e3d_io::ReadOnlyEngine::open_at(path, header.latest_session.sesno)?;
        let indexed = engine
            .indexed_refnos()?
            .into_iter()
            .map(|(r, _, _)| (r.word0, r.word1))
            .collect::<std::collections::BTreeSet<_>>();
        output.push(serde_json::json!({"file":path,"dbnum":header.dbnum,"sesno":header.latest_session.sesno,
            "indexed_records":indexed.len(),"references":refs.iter().map(|r| serde_json::json!({"reference":r.to_string(),"present":indexed.contains(&(r.word0,r.word1))})).collect::<Vec<_>>() }));
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
