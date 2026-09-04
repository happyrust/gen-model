//! Deterministically sample refnos from an e3d-io physical database index.

use std::collections::BTreeSet;
use std::path::PathBuf;

use clap::Parser;
use e3d_io::index::IndexCursor;
use e3d_io::session::{DbView, SessionSelector};
use e3d_io::RefNo;

#[derive(Parser)]
#[command(about = "sample physical E3D refnos for TTY parity capture")]
struct Args {
    #[arg(long)]
    db: PathBuf,
    #[arg(long, default_value_t = 50)]
    count: usize,
    #[arg(long)]
    session: Option<u32>,
    #[arg(long)]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(args.count > 0, "--count must be greater than zero");
    let selector = args
        .session
        .map(SessionSelector::Exact)
        .unwrap_or(SessionSelector::Latest);
    let mut view = DbView::open_at(&args.db, selector)?;
    let root = view.index_root();
    let mut refnos = Vec::new();
    for found in IndexCursor::at_root(view.pages_mut(), root).walk() {
        refnos.push(found?.refno);
    }
    let selected = equidistant_sample(&refnos, args.count);
    anyhow::ensure!(
        !selected.is_empty(),
        "{} has no live refnos",
        args.db.display()
    );

    if let Some(parent) = args.out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = format!(
        "# db={} indexed={} sampled={}\n",
        args.db.display(),
        refnos.len(),
        selected.len()
    );
    for refno in &selected {
        text.push_str(&format!("{}/{}\n", refno.word0, refno.word1));
    }
    std::fs::write(&args.out, text)?;
    println!(
        "E3D_TTY_SAMPLE_OK indexed={} sampled={} out={}",
        refnos.len(),
        selected.len(),
        args.out.display()
    );
    Ok(())
}

fn equidistant_sample(rows: &[RefNo], count: usize) -> Vec<RefNo> {
    if rows.len() <= count {
        return rows.to_vec();
    }
    let stride = rows.len() as f64 / count as f64;
    let indexes = (0..count)
        .map(|index| ((index as f64 * stride).floor() as usize).min(rows.len() - 1))
        .collect::<BTreeSet<_>>();
    indexes.into_iter().map(|index| rows[index]).collect()
}

#[cfg(test)]
mod tests {
    use super::equidistant_sample;
    use e3d_io::RefNo;

    #[test]
    fn sampling_is_deterministic_and_spans_the_sorted_index() {
        let rows = (0..10)
            .map(|word1| RefNo::new(42, word1))
            .collect::<Vec<_>>();
        let sampled = equidistant_sample(&rows, 4)
            .into_iter()
            .map(|refno| refno.word1)
            .collect::<Vec<_>>();
        assert_eq!(sampled, vec![0, 2, 5, 7]);
    }
}
