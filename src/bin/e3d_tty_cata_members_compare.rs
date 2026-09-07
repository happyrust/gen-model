//! Compare physical CATA member order with read-only E3D TTY `Q MEMBERS`.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use aios_database::e3d_query::{E3dDriver, parse_members};
use clap::Parser;
use e3d_attlib::{db1_dehash, db1_hash};
use e3d_io::record::element::ParsedElement;
use e3d_io::record::explicit::ExplicitValue;
use e3d_io::{ReadOnlyEngine, RefNo};
use serde::Serialize;

#[derive(Parser)]
#[command(about = "compare e3d-io CATA members with E3D TTY")]
struct Args {
    #[arg(long)]
    db: PathBuf,
    #[arg(long, default_value = "15543/0")]
    root: String,
    #[arg(long, default_value_t = 10)]
    count: usize,
    #[arg(long, default_value_t = 100)]
    candidate_parents: usize,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Clone, Serialize)]
struct DirectMember {
    refno: String,
    noun: String,
    name: String,
}

#[derive(Serialize)]
struct Mismatch {
    parent: String,
    direct: Vec<String>,
    tty: Vec<String>,
}

#[derive(Serialize)]
struct ParentSnapshot {
    parent: String,
    children: Vec<DirectMember>,
}

#[derive(Serialize)]
struct Verification {
    candidate_command: String,
    authority_command: String,
    parents_compared: usize,
    parent_refnos: Vec<String>,
    parents: Vec<ParentSnapshot>,
    direct_members: usize,
    mismatch_count: usize,
    mismatches: Vec<Mismatch>,
    candidate_exit_status: i32,
    tty_exit_status: i32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(args.count > 0, "--count must be greater than zero");
    anyhow::ensure!(
        args.candidate_parents >= args.count,
        "--candidate-parents must be at least --count"
    );
    std::fs::create_dir_all(&args.out)?;

    let root = parse_refno(&args.root)?;
    let mut engine = ReadOnlyEngine::open(&args.db)?;
    let candidates = collect_parents(&mut engine, root, args.candidate_parents)?;
    let parents = sample_parents(&candidates, args.count);
    anyhow::ensure!(
        parents.len() == args.count,
        "found only {} parent elements",
        parents.len()
    );

    let body = tty_source(&parents);
    std::fs::write(args.out.join("tty-query.mac.body"), &body)?;
    let repo = std::env::current_dir()?;
    let mut driver = E3dDriver::from_env(&repo)?;
    driver.project = "AMS".into();
    driver.mdb = "/ALL".into();
    let raw = driver.run_source(&repo, "cata-members-parity", &body)?;
    std::fs::write(args.out.join("tty-raw.log"), &raw)?;

    let mismatches = compare(&parents, &raw)?;
    let verification = Verification {
        candidate_command: format!(
            "e3d-io physical members from {} root {}",
            args.db.display(),
            args.root
        ),
        authority_command: "E3D TTY AMS /ALL: =<parent>; Q MEMBERS".into(),
        parents_compared: parents.len(),
        parent_refnos: parents.iter().map(|(refno, _)| refno.to_string()).collect(),
        parents: parents
            .iter()
            .map(|(parent, children)| ParentSnapshot {
                parent: parent.to_string(),
                children: children.clone(),
            })
            .collect(),
        direct_members: parents.iter().map(|(_, children)| children.len()).sum(),
        mismatch_count: mismatches.len(),
        mismatches,
        candidate_exit_status: 0,
        tty_exit_status: 0,
    };
    std::fs::write(
        args.out.join("verification.json"),
        serde_json::to_vec_pretty(&verification)?,
    )?;
    println!(
        "CATA_MEMBERS_TTY parents={} members={} mismatches={}",
        verification.parents_compared, verification.direct_members, verification.mismatch_count
    );
    anyhow::ensure!(verification.mismatch_count == 0, "CATA member mismatch");
    Ok(())
}

fn collect_parents(
    engine: &mut ReadOnlyEngine,
    root: RefNo,
    limit: usize,
) -> anyhow::Result<Vec<(RefNo, Vec<DirectMember>)>> {
    let mut queue = VecDeque::from([root]);
    let mut seen = HashSet::new();
    let mut parents = Vec::new();
    while let Some(refno) = queue.pop_front() {
        if !seen.insert(refno) {
            continue;
        }
        let Some(view) = engine.find_element(refno)? else {
            continue;
        };
        let parsed = ParsedElement::parse(&view.raw_bytes).map_err(anyhow::Error::msg)?;
        queue.extend(parsed.members.iter().copied());
        if parsed.members.is_empty() || parsed.noun_hash == db1_hash("WORL") {
            continue;
        }
        let children = parsed
            .members
            .iter()
            .filter_map(|child| direct_member(engine, *child).transpose())
            .collect::<anyhow::Result<Vec<_>>>()?;
        parents.push((refno, children));
        if parents.len() == limit {
            break;
        }
    }
    Ok(parents)
}

fn direct_member(
    engine: &mut ReadOnlyEngine,
    refno: RefNo,
) -> anyhow::Result<Option<DirectMember>> {
    let Some(view) = engine.find_element(refno)? else {
        return Ok(None);
    };
    let parsed = ParsedElement::parse(&view.raw_bytes).map_err(anyhow::Error::msg)?;
    let name_hash = db1_hash("NAME");
    let name = parsed
        .explicit_attributes
        .iter()
        .rev()
        .find_map(|attribute| (attribute.hash == name_hash).then_some(&attribute.value))
        .and_then(|value| match value {
            ExplicitValue::Text(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("={refno}"));
    Ok(Some(DirectMember {
        refno: refno.to_string(),
        noun: db1_dehash(parsed.noun_hash).unwrap_or_else(|| "UNKNOWN".into()),
        name,
    }))
}

fn sample_parents(
    candidates: &[(RefNo, Vec<DirectMember>)],
    count: usize,
) -> Vec<(RefNo, Vec<DirectMember>)> {
    if candidates.len() <= count {
        return candidates.iter().cloned().collect();
    }
    let stride = candidates.len() as f64 / count as f64;
    (0..count)
        .map(|index| candidates[(index as f64 * stride).floor() as usize].clone())
        .collect()
}

fn tty_source(parents: &[(RefNo, Vec<DirectMember>)]) -> String {
    parents
        .iter()
        .enumerate()
        .map(|(index, (parent, _))| {
            format!(
                "!selected = TRUE\n={parent}\nhandle any\n!selected = FALSE\nelsehandle none\nendhandle\n$P CM-{index}-BEGIN\nif (!selected) then\nQ MEMBERS\nendif\n$P CM-{index}-END\n"
            )
        })
        .collect()
}

fn compare(parents: &[(RefNo, Vec<DirectMember>)], raw: &str) -> anyhow::Result<Vec<Mismatch>> {
    let mut mismatches = Vec::new();
    for (index, (parent, children)) in parents.iter().enumerate() {
        let section = marker_section(raw, index)?;
        let rows = parse_members(&format!("MCP-MEMBERS-BEGIN\n{section}\nMCP-MEMBERS-END\n"))?;
        let tty = rows
            .iter()
            .map(|row| match &row.refno {
                Some(refno) => format!("{}|{refno}", row.noun.trim()),
                None => format!("{}|{}", row.noun.trim(), row.value.trim()),
            })
            .collect::<Vec<_>>();
        let direct = children
            .iter()
            .enumerate()
            .map(|(row_index, child)| {
                match rows.get(row_index).and_then(|row| row.refno.as_ref()) {
                    Some(_) => format!("{}|{}", child.noun, child.refno),
                    None => format!("{}|{}", child.noun, child.name),
                }
            })
            .collect::<Vec<_>>();
        if direct != tty {
            mismatches.push(Mismatch {
                parent: parent.to_string(),
                direct,
                tty,
            });
        }
    }
    Ok(mismatches)
}

fn marker_section(raw: &str, index: usize) -> anyhow::Result<&str> {
    let begin = format!("CM-{index}-BEGIN");
    let end = format!("CM-{index}-END");
    let start = raw
        .find(&begin)
        .ok_or_else(|| anyhow::anyhow!("missing {begin}"))?
        + begin.len();
    let tail = &raw[start..];
    let finish = tail
        .find(&end)
        .ok_or_else(|| anyhow::anyhow!("missing {end}"))?;
    Ok(&tail[..finish])
}

fn parse_refno(value: &str) -> anyhow::Result<RefNo> {
    let (word0, word1) = value
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("--root must be WORD0/WORD1"))?;
    Ok(RefNo::new(word0.parse()?, word1.parse::<i32>()? as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_sampling_is_deterministic() {
        let candidates = (0..10)
            .map(|word1| (RefNo::new(1, word1), Vec::new()))
            .collect::<Vec<_>>();
        assert_eq!(
            sample_parents(&candidates, 4)
                .iter()
                .map(|(refno, _)| refno.word1)
                .collect::<Vec<_>>(),
            vec![0, 2, 5, 7]
        );
    }
}
