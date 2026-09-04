//! Capture one complete, read-only E3D TTY `ATTLIST` fixture.

use std::path::PathBuf;

use aios_database::e3d_query::{E3dDriver, validate_refno};
use clap::Parser;

#[derive(Parser)]
#[command(about = "capture one E3D TTY ATTLIST for e3d-io parity")]
struct Args {
    #[arg(long)]
    refno: String,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    mdb: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let refno = validate_refno(&args.refno)?;
    let repo = std::env::current_dir()?;
    let mut driver = E3dDriver::from_env(&repo)?;
    if let Some(project) = args.project {
        driver.project = project;
    }
    if let Some(mdb) = args.mdb {
        driver.mdb = format!("/{}", mdb.trim_start_matches('/'));
    }

    let raw = driver.run_source(&repo, "tty-attlist-capture", &tty_source(&refno))?;
    anyhow::ensure!(!raw.contains("NOUN-NOT-FOUND"), "E3D TTY 找不到 {refno}");
    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.out, raw)?;
    println!("TTY_ATTLIST_OK refno={refno} out={}", args.out.display());
    Ok(())
}

fn tty_source(refno: &str) -> String {
    format!(
        "!selected = TRUE\n={refno}\nhandle any\n!selected = FALSE\n$P NOUN-NOT-FOUND\nelsehandle none\nendhandle\n\
         if (!selected) then\n$P NOUN-ALL-ATTRIBUTES-BEGIN\nQ CE\nQ REF\nQ TYPE\n\
         $P NOUN-ATTLIST-BEGIN\nVAR !attl ATTLIST\nDO !a VALUES !attl\n\
         $P ZZA $!a\nQ $!a\nENDDO\n$P NOUN-ATTLIST-END\n\
         $P NOUN-ALL-ATTRIBUTES-END\nendif\n"
    )
}

#[cfg(test)]
mod tests {
    use super::tty_source;

    #[test]
    fn capture_source_has_parser_markers_and_queries_every_attlist_entry() {
        let source = tty_source("23735/3");
        assert!(source.contains("=23735/3\n"));
        assert!(source.contains("NOUN-ATTLIST-BEGIN"));
        assert!(source.contains("DO !a VALUES !attl\n$P ZZA $!a\nQ $!a\nENDDO"));
        assert!(source.contains("NOUN-ATTLIST-END"));
    }
}
