//! Offline SQL renderer for a captured initialization envelope. It never connects
//! to a database; the benchmark runs the output only inside cancelled transactions.
use aios_database::data_interface::model_publish_queue::FrozenRootEnvelope;
use aios_database::fast_model::model_db_adapter::{WriteMode, render_receipt, render_records};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("envelope JSON path");
    let output = std::path::PathBuf::from(args.next().expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let envelope: FrozenRootEnvelope = serde_json::from_slice(&std::fs::read(input)?)?;
    std::fs::write(
        output.join("records.surql"),
        render_records(&envelope.projection.upserts, WriteMode::Incremental)?,
    )?;
    if let Some((root, receipt)) = &envelope.projection.receipt {
        std::fs::write(
            output.join("receipt.surql"),
            render_receipt(*root, receipt)?,
        )?;
    }
    println!(
        "rendered {} geometries for {}",
        envelope.projection.upserts.len(),
        envelope.intent.root
    );
    Ok(())
}
