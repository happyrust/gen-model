//! Extract-tree file identity (ADR-028).
//!
//! Same-project `ams7355` + `ams7355_0001` are one logical dbnum: the highest
//! `_NNNN` leaf is the working file, the unsuffixed master is the parent layer.
//! Sibling extracts (`_0001` + `_0002`) and hand copies stay Duplicate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Filename form recognised as a numbered PDMS/E3D database (not sys/com/mis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractFileName {
    pub prefix: String,
    pub dbnum: u32,
    pub extract: Option<u32>,
}

/// One surviving family after collapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedFamily {
    pub project: String,
    pub dbnum: u32,
    pub leaf_path: PathBuf,
    pub parent_path: Option<PathBuf>,
}

/// Filename-derived dbnum disagrees with the 60-byte header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbnumMismatch {
    pub path: PathBuf,
    pub filename_dbnum: u32,
    pub header_dbnum: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CollapseResult {
    pub selected: Vec<SelectedFamily>,
    pub shadowed_parents: Vec<PathBuf>,
    pub duplicate_keys: HashSet<(String, u32)>,
    pub mismatches: Vec<DbnumMismatch>,
}

/// Parse `<prefix><dbnum>` or `<prefix><dbnum>_<NNNN>`. `sys`/`com`/`mis` are not
/// numbered extract families.
pub fn parse_extract_file_name(name: &str) -> Option<ExtractFileName> {
    let bytes = name.as_bytes();
    if bytes.len() <= 3 || !bytes[..3].iter().all(u8::is_ascii_alphabetic) {
        return None;
    }
    let prefix = name[..3].to_string();
    let rest = &name[3..];
    if matches!(rest.to_ascii_lowercase().as_str(), "sys" | "com" | "mis") {
        return None;
    }
    let (dbnum_str, extract) = match rest.split_once('_') {
        Some((dbnum, seq)) => {
            if seq.len() != 4 || !seq.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (dbnum, Some(seq.parse::<u32>().ok()?))
        }
        None => (rest, None),
    };
    if dbnum_str.is_empty() || !dbnum_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(ExtractFileName {
        prefix,
        dbnum: dbnum_str.parse().ok()?,
        extract,
    })
}

/// Directory sibling of an extract leaf: strip `_NNNN` → unsuffixed master name.
/// Does not check that the file exists.
pub fn parent_path_of(leaf: &Path) -> Option<PathBuf> {
    let name = leaf.file_name()?.to_str()?;
    let parsed = parse_extract_file_name(name)?;
    parsed.extract?;
    Some(leaf.with_file_name(format!("{}{}", parsed.prefix, parsed.dbnum)))
}

/// Collapse same-project numbered extract families before Duplicate.
///
/// `header_dbnum` is the 60-byte header `db_no`. Unparsed names (sys/com/mis,
/// or callers passing synthetic paths like `first`) are grouped by header dbnum
/// and still Duplicate when that key appears twice.
pub fn collapse_extract_families(
    entries: impl IntoIterator<Item = (String, u32, PathBuf)>,
) -> CollapseResult {
    struct Family {
        masters: Vec<PathBuf>,
        extracts: Vec<(u32, PathBuf)>,
    }

    let mut families: HashMap<(String, u32), Family> = HashMap::new();
    let mut unparsed: Vec<(String, u32, PathBuf)> = Vec::new();
    let mut mismatches = Vec::new();
    let mut duplicate_keys = HashSet::new();

    for (project, header_dbnum, path) in entries {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        match parse_extract_file_name(name) {
            Some(parsed) if parsed.dbnum != header_dbnum => {
                mismatches.push(DbnumMismatch {
                    path,
                    filename_dbnum: parsed.dbnum,
                    header_dbnum,
                });
                duplicate_keys.insert((project, header_dbnum));
            }
            Some(parsed) => {
                let family = families.entry((project, parsed.dbnum)).or_insert(Family {
                    masters: Vec::new(),
                    extracts: Vec::new(),
                });
                match parsed.extract {
                    Some(extract) => family.extracts.push((extract, path)),
                    None => family.masters.push(path),
                }
            }
            None => unparsed.push((project, header_dbnum, path)),
        }
    }

    let mut selected = Vec::new();
    let mut shadowed_parents = Vec::new();

    for ((project, dbnum), mut family) in families {
        family.extracts.sort_by_key(|(extract, _)| *extract);
        if family.extracts.len() > 1 || family.masters.len() > 1 {
            duplicate_keys.insert((project, dbnum));
            continue;
        }
        if let Some((_, leaf_path)) = family.extracts.pop() {
            let parent_path = family.masters.pop();
            if let Some(parent) = parent_path.clone() {
                shadowed_parents.push(parent);
            }
            selected.push(SelectedFamily {
                project,
                dbnum,
                leaf_path,
                parent_path,
            });
            continue;
        }
        if let Some(leaf_path) = family.masters.pop() {
            selected.push(SelectedFamily {
                project,
                dbnum,
                leaf_path,
                parent_path: None,
            });
        }
    }

    for (project, dbnum, path) in unparsed {
        selected.push(SelectedFamily {
            project,
            dbnum,
            leaf_path: path,
            parent_path: None,
        });
    }

    let mut seen = HashSet::new();
    for family in &selected {
        let key = (family.project.clone(), family.dbnum);
        if !seen.insert(key.clone()) {
            duplicate_keys.insert(key);
        }
    }
    selected.retain(|family| !duplicate_keys.contains(&(family.project.clone(), family.dbnum)));
    shadowed_parents.retain(|path| {
        selected
            .iter()
            .any(|family| family.parent_path.as_ref() == Some(path))
    });

    CollapseResult {
        selected,
        shadowed_parents,
        duplicate_keys,
        mismatches,
    }
}

/// Count refnos present in the parent index but missing from the leaf.
pub fn parent_gap_refno_count(leaf: &Path, parent: &Path) -> anyhow::Result<usize> {
    let leaf_index = parse_pdms_db::parse::parse_file_db_index_data(&leaf.to_path_buf())?;
    let parent_index = parse_pdms_db::parse::parse_file_db_index_data(&parent.to_path_buf())?;
    Ok(parent_index
        .refno_table_map
        .iter()
        .filter(|entry| !leaf_index.refno_table_map.contains_key(entry.key()))
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::dbnum_state::{FileAnomaly, check_file_against_state};

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn parse_master_extract_and_sys() {
        let master = parse_extract_file_name("ams7355").expect("master");
        assert_eq!(master.prefix, "ams");
        assert_eq!(master.dbnum, 7355);
        assert_eq!(master.extract, None);

        let leaf = parse_extract_file_name("ams7355_0001").expect("leaf");
        assert_eq!(leaf.dbnum, 7355);
        assert_eq!(leaf.extract, Some(1));

        let wide = parse_extract_file_name("acp250705_0001").expect("six-digit");
        assert_eq!(wide.dbnum, 250705);
        assert_eq!(wide.extract, Some(1));

        assert!(parse_extract_file_name("amssys").is_none());
        assert!(parse_extract_file_name("ams1112_0001 copy").is_none());
    }

    #[test]
    fn parent_path_strips_extract_suffix() {
        assert_eq!(
            parent_path_of(Path::new(r"D:\ams000\ams7355_0001")),
            Some(PathBuf::from(r"D:\ams000\ams7355"))
        );
        assert_eq!(parent_path_of(Path::new(r"D:\ams000\ams7355")), None);
    }

    #[test]
    fn ams7355_parent_and_leaf_collapse_to_the_leaf() {
        let result = collapse_extract_families([
            ("AMS".into(), 7355, p("ams000/ams7355")),
            ("AMS".into(), 7355, p("ams000/ams7355_0001")),
        ]);
        assert!(
            result.duplicate_keys.is_empty(),
            "{:?}",
            result.duplicate_keys
        );
        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected[0].leaf_path, p("ams000/ams7355_0001"));
        assert_eq!(result.selected[0].parent_path, Some(p("ams000/ams7355")));
        assert_eq!(result.shadowed_parents, vec![p("ams000/ams7355")]);
    }

    #[test]
    fn leaf_only_stays_the_candidate() {
        let result = collapse_extract_families([("AMS".into(), 7322, p("ams000/ams7322_0001"))]);
        assert!(result.duplicate_keys.is_empty());
        assert_eq!(result.selected[0].leaf_path, p("ams000/ams7322_0001"));
        assert_eq!(result.selected[0].parent_path, None);
        assert!(result.shadowed_parents.is_empty());
    }

    #[test]
    fn sibling_extracts_are_duplicate() {
        let result = collapse_extract_families([
            ("AMS".into(), 9990, p("ams000/ams9990_0001")),
            ("AMS".into(), 9990, p("ams000/ams9990_0002")),
        ]);
        assert_eq!(result.duplicate_keys, HashSet::from([("AMS".into(), 9990)]));
        assert!(result.selected.is_empty());
    }

    #[test]
    fn a_copy_name_next_to_a_legal_extract_is_still_duplicate() {
        let result = collapse_extract_families([
            ("AMS".into(), 1112, p("ams000/ams1112_0001")),
            ("AMS".into(), 1112, p("ams000/ams1112_0001 copy")),
        ]);
        assert_eq!(result.duplicate_keys, HashSet::from([("AMS".into(), 1112)]));
    }

    #[test]
    fn filename_dbnum_mismatch_blocks() {
        let result = collapse_extract_families([("AMS".into(), 8000, p("ams000/ams7355_0001"))]);
        assert_eq!(result.mismatches.len(), 1);
        assert_eq!(result.mismatches[0].filename_dbnum, 7355);
        assert_eq!(result.mismatches[0].header_dbnum, 8000);
        assert_eq!(result.duplicate_keys, HashSet::from([("AMS".into(), 8000)]));
        assert!(result.selected.is_empty());
    }

    #[test]
    fn switching_master_to_leaf_with_sesno_regression_is_reinit() {
        let anomaly = check_file_against_state(
            Some("CATA"),
            Some(r"D:\ams000\ams7355"),
            13,
            "CATA",
            r"D:\ams000\ams7355_0001",
            12,
        );
        assert!(
            matches!(anomaly, Some(FileAnomaly::Rollback { .. })),
            "{anomaly:?}"
        );
    }

    #[test]
    fn switching_master_to_leaf_without_sesno_regression_is_path_migrated() {
        let anomaly = check_file_against_state(
            Some("CATA"),
            Some(r"D:\ams000\ams7355"),
            13,
            "CATA",
            r"D:\ams000\ams7355_0001",
            15,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::PathMigrated {
                old_path: r"D:\ams000\ams7355".into(),
                new_path: r"D:\ams000\ams7355_0001".into(),
            })
        );
    }

    #[test]
    #[ignore = "reads the real AMS 7355 master/extract pair"]
    fn live_extract_tree_ams7355_refno_sets() {
        let dir = Path::new(r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000");
        let parent = dir.join("ams7355");
        let leaf = dir.join("ams7355_0001");
        if !parent.is_file() || !leaf.is_file() {
            eprintln!("skip: AMS 7355 pair not on this machine");
            return;
        }
        let leaf_index = parse_pdms_db::parse::parse_file_db_index_data(&leaf).expect("leaf index");
        let parent_index =
            parse_pdms_db::parse::parse_file_db_index_data(&parent).expect("parent index");
        let gap = parent_index
            .refno_table_map
            .iter()
            .filter(|entry| !leaf_index.refno_table_map.contains_key(entry.key()))
            .count();
        eprintln!(
            "ams7355 refnos={} ses_pgno={} | ams7355_0001 refnos={} ses_pgno={} | parent_only={}",
            parent_index.refno_table_map.len(),
            parent_index.ses_pgno,
            leaf_index.refno_table_map.len(),
            leaf_index.ses_pgno,
            gap
        );
        assert!(
            !leaf_index.refno_table_map.is_empty() && !parent_index.refno_table_map.is_empty(),
            "both indexes must contain refnos"
        );
    }
}
