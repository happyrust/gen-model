//! Capture complete, read-only E3D TTY `ATTLIST` fixtures.

use std::path::{Path, PathBuf};

use aios_database::e3d_query::{E3dDriver, validate_refno};
use clap::Parser;

#[derive(Parser)]
#[command(about = "capture E3D TTY ATTLIST fixtures for e3d-io parity")]
struct Args {
    #[arg(long)]
    refno: Option<String>,
    #[arg(long)]
    out: Option<PathBuf>,
    /// Text file containing one refno per line. Blank lines and `#` comments are ignored.
    #[arg(long)]
    refno_file: Option<PathBuf>,
    #[arg(long)]
    out_dir: Option<PathBuf>,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    mdb: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let repo = std::env::current_dir()?;
    let mut driver = E3dDriver::from_env(&repo)?;
    if let Some(project) = args.project {
        driver.project = project;
    }
    if let Some(mdb) = args.mdb {
        driver.mdb = format!("/{}", mdb.trim_start_matches('/'));
    }

    match (args.refno, args.out, args.refno_file, args.out_dir) {
        (Some(refno), Some(out), None, None) => capture_one(&driver, &repo, &refno, &out),
        (None, None, Some(refno_file), Some(out_dir)) => {
            capture_batch(&driver, &repo, &refno_file, &out_dir)
        }
        _ => anyhow::bail!(
            "use either --refno <REFNO> --out <FILE>, or --refno-file <FILE> --out-dir <DIR>"
        ),
    }
}

fn capture_one(driver: &E3dDriver, repo: &Path, refno: &str, out: &Path) -> anyhow::Result<()> {
    let refno = validate_refno(refno)?;
    let raw = driver.run_source(repo, "tty-attlist-capture", &tty_source(&refno))?;
    anyhow::ensure!(!raw.contains("NOUN-NOT-FOUND"), "E3D TTY 找不到 {refno}");
    write_fixture(out, &raw)?;
    println!("TTY_ATTLIST_OK refno={refno} out={}", out.display());
    Ok(())
}

fn capture_batch(
    driver: &E3dDriver,
    repo: &Path,
    refno_file: &Path,
    out_dir: &Path,
) -> anyhow::Result<()> {
    let refnos = read_refnos(refno_file)?;
    anyhow::ensure!(!refnos.is_empty(), "{} contains no refnos", refno_file.display());
    let raw = driver.run_source(repo, "tty-attlist-batch-capture", &batch_source(&refnos))?;
    anyhow::ensure!(!raw.contains("NOUN-NOT-FOUND"), "E3D TTY batch contains missing refnos");
    std::fs::create_dir_all(out_dir)?;
    for (index, refno) in refnos.iter().enumerate() {
        let fixture = extract_batch_fixture(&raw, index)?;
        let out = out_dir.join(format!("{}.log", refno.replace('/', "-")));
        write_fixture(&out, &fixture)?;
        println!("TTY_ATTLIST_OK refno={refno} out={}", out.display());
    }
    println!("TTY_ATTLIST_BATCH_OK count={}", refnos.len());
    Ok(())
}

fn read_refnos(path: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(validate_refno)
        .collect()
}

fn write_fixture(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
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

fn batch_source(refnos: &[String]) -> String {
    refnos
        .iter()
        .enumerate()
        .map(|(index, refno)| {
            format!(
                "$P BATCH-SAMPLE-{index}-BEGIN\n{}$P BATCH-SAMPLE-{index}-END\n",
                tty_source(refno)
            )
        })
        .collect()
}

fn extract_batch_fixture(raw: &str, index: usize) -> anyhow::Result<String> {
    let begin = format!("BATCH-SAMPLE-{index}-BEGIN");
    let end = format!("BATCH-SAMPLE-{index}-END");
    let (_, tail) = raw
        .split_once(&begin)
        .ok_or_else(|| anyhow::anyhow!("batch output is missing {begin}"))?;
    let (body, _) = tail
        .split_once(&end)
        .ok_or_else(|| anyhow::anyhow!("batch output is missing {end}"))?;
    Ok(format!("MCP-BEGIN{body}MCP-END\n"))
}

#[cfg(test)]
mod tests {
    use super::{batch_source, extract_batch_fixture, tty_source};

    #[test]
    fn capture_source_has_parser_markers_and_queries_every_attlist_entry() {
        let source = tty_source("23735/3");
        assert!(source.contains("=23735/3\n"));
        assert!(source.contains("NOUN-ATTLIST-BEGIN"));
        assert!(source.contains("DO !a VALUES !attl\n$P ZZA $!a\nQ $!a\nENDDO"));
        assert!(source.contains("NOUN-ATTLIST-END"));
    }

    #[test]
    fn batch_source_marks_each_sample_and_fixture_split_is_standalone() {
        let source = batch_source(&["23735/3".into(), "24383/73930".into()]);
        assert!(source.contains("BATCH-SAMPLE-0-BEGIN\n"));
        assert!(source.contains("=23735/3\n"));
        assert!(source.contains("BATCH-SAMPLE-1-BEGIN\n"));
        assert!(source.contains("=24383/73930\n"));

        let raw = "MCP-BEGIN\nBATCH-SAMPLE-0-BEGIN\nfirst\nBATCH-SAMPLE-0-END\n\
                   BATCH-SAMPLE-1-BEGIN\nsecond\nBATCH-SAMPLE-1-END\nMCP-END\n";
        assert_eq!(
            extract_batch_fixture(raw, 1).unwrap(),
            "MCP-BEGIN\nsecond\nMCP-END\n"
        );
    }
}
