//! Read-only dabacon census for the OCC-retirement field gates.

use std::path::PathBuf;

use aios_database::data_interface::source_primitive_census::{
    DEFAULT_TARGET_NOUNS, census_source_root,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "occ_retire_census")]
struct Args {
    /// Directory containing the project's dabacon `*_0001` files.
    #[arg(long)]
    root: PathBuf,

    /// Source noun names to census before inst_geo normalization.
    #[arg(long, value_delimiter = ',', default_values_t = DEFAULT_TARGET_NOUNS.iter().map(|value| value.to_string()))]
    nouns: Vec<String>,

    /// JSON evidence output.
    #[arg(long)]
    out: PathBuf,

    /// Generate and validate every directly tessellatable target primitive.
    #[arg(long)]
    validate_mesh: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let census = census_source_root(&args.root, &args.nouns, args.validate_mesh)?;
    let json = serde_json::to_vec_pretty(&census)?;
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, &json)?;
    println!(
        "files={} indexed_elements={} samples={} counts={:?} out={}",
        census.files_scanned,
        census.indexed_elements,
        census.samples.len(),
        census.noun_counts,
        args.out.display()
    );
    Ok(())
}
