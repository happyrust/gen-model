//! Bounded model regeneration against an already running SurrealDB.
//!
//! This maintenance probe deliberately does not construct `AiosDBManager`, so
//! it neither scans source databases nor competes for the project instance
//! lock.  It invokes the same targeted `gen_all_geos_data` path used by model
//! refresh: replace instance rows, generate meshes, update AABBs and apply
//! Manifold booleans.

use anyhow::{Context, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Regenerate explicit model roots in an existing database")]
struct Args {
    /// Comma-separated PDMS refnos, for example 24384/22399.
    #[arg(long, value_delimiter = ',')]
    roots: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.roots.is_empty() {
        bail!("--roots must contain at least one PDMS refno");
    }
    for root in &args.roots {
        let Some((db, id)) = root.split_once('/') else {
            bail!("invalid refno {root}; expected db/id");
        };
        db.parse::<u64>()
            .with_context(|| format!("invalid refno database in {root}"))?;
        id.parse::<u64>()
            .with_context(|| format!("invalid refno element in {root}"))?;
    }

    aios_core::init_surreal()
        .await
        .context("connect existing SurrealDB")?;
    let mut option = aios_core::get_db_option().clone();
    option.gen_model = true;
    option.gen_mesh = true;
    option.replace_mesh = Some(true);
    option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
    option.debug_root_refnos = Some(args.roots.clone());

    println!("TARGETED_REGEN|roots={}|start", args.roots.join(","));
    aios_database::fast_model::gen_all_geos_data(&option)
        .await
        .context("targeted model regeneration")?;
    println!("TARGETED_REGEN|roots={}|done", args.roots.join(","));
    Ok(())
}
