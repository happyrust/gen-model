//! Which databases an MDB declares, resolved from the SYS database file.
//!
//! An MDB's `CURD` is the authoritative list, and it reaches across projects:
//! `AvevaMarineSample /ALL` declares six Dictionary databases, four of which
//! live under `AvevaCatalogue` and `SCB`. Picking dictionaries by scanning a
//! project directory gets this wrong in both directions at once — the first
//! attempt at it here missed two that `/ALL` declares and included one it does
//! not — and neither mistake reports anything. A UDA whose dictionary was left
//! out reads exactly like a UDA with no value.
//!
//! [`crate::data_interface::update_scope::UpdateScope`] answers the same
//! question for `STYP = DESI` through SurrealDB, which needs the SYS database
//! parsed and synced first. This reads the file, so it is available during
//! initialization — before anything has been synced — which is the point at
//! which the Dictionary set has to be known.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use aios_core::RefU64;
use aios_core::options::DbOption;
use aios_core::types::named_attvalue::NamedAttrValue;

/// The `STYP` a Dictionary database carries in the SYS record.
///
/// Not `aios_core`'s `DBType::DICT`, which is 6 and matches nothing in the AMS
/// SYS database: every dictionary `/ALL` declares reads 8. `DESI = 1` does
/// agree, which is why `UpdateScope` has never tripped over this — a query
/// asking for 6 would come back empty rather than wrong, and read as "this
/// MDB declares no dictionaries".
pub const DICT_STYP: i64 = 8;
pub const DESI_STYP: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdbDatabase {
    pub dbnum: u32,
    pub styp: i64,
    /// The SYS element's name, e.g. `*MASTER/DICT`.
    pub name: String,
    /// `None` when the declaration names a database with no file under any
    /// configured project directory. Kept rather than dropped: a declared
    /// database that is not on disk is a deployment problem worth seeing.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct MdbMembership {
    mdb: String,
    project: String,
    /// In `CURD` order. Order decides which definition wins a duplicated
    /// `UKEY`, so it is preserved rather than sorted.
    databases: Vec<MdbDatabase>,
}

impl MdbMembership {
    pub fn mdb(&self) -> &str {
        &self.mdb
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    pub fn databases(&self) -> &[MdbDatabase] {
        &self.databases
    }

    pub fn of_type(&self, styp: i64) -> impl Iterator<Item = &MdbDatabase> {
        self.databases.iter().filter(move |db| db.styp == styp)
    }

    /// Declared Dictionary databases that exist on disk, in `CURD` order —
    /// the list a UDA catalog should be built from.
    pub fn dictionary_paths(&self) -> Vec<PathBuf> {
        self.of_type(DICT_STYP)
            .filter_map(|db| db.path.clone())
            .collect()
    }

    pub fn counts_by_type(&self) -> BTreeMap<i64, usize> {
        let mut out = BTreeMap::new();
        for db in &self.databases {
            *out.entry(db.styp).or_default() += 1;
        }
        out
    }

    /// Declared databases with no file under any configured project directory.
    pub fn unresolved(&self) -> impl Iterator<Item = &MdbDatabase> {
        self.databases.iter().filter(|db| db.path.is_none())
    }
}

/// Read `<project>`'s SYS database and resolve `mdb`'s declaration.
pub fn resolve(db_option: &DbOption, project: &str, mdb: &str) -> anyhow::Result<MdbMembership> {
    let plan = crate::data_interface::project_paths::plan_watch_dirs(db_option);
    let own_dirs: Vec<PathBuf> = plan
        .projects
        .iter()
        .filter(|entry| entry.project == project)
        .flat_map(|entry| entry.db_dirs.iter().cloned())
        .collect();
    if own_dirs.is_empty() {
        anyhow::bail!("项目 {project} 没有解析出任何库目录，无从读取它的 SYS 库");
    }
    let all_dirs = plan.dirs();
    let wanted = format!("/{}", mdb.trim_start_matches('/'));

    let mut tried = Vec::new();
    for sys in sys_candidates(&own_dirs) {
        tried.push(sys.display().to_string());
        match read_declaration(&sys, project, &wanted, &all_dirs) {
            Ok(Some(databases)) => {
                return Ok(MdbMembership {
                    mdb: wanted,
                    project: project.to_string(),
                    databases,
                });
            }
            Ok(None) => continue,
            Err(error) => log::warn!("读取 {} 失败，继续找下一个：{error:#}", sys.display()),
        }
    }
    anyhow::bail!(
        "在项目 {project} 的 SYS 库里找不到名为 {wanted} 的 MDB。已尝试：{}",
        if tried.is_empty() {
            "（没有找到任何 SYS 库文件）".to_string()
        } else {
            tried.join(" / ")
        }
    )
}

/// Files whose name ends in `sys`, which is how E3D names the SYSTEM database
/// (`amssys`, `acpsys`). Sibling `*mis` / `*com` files are other things.
fn sys_candidates(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with("sys"))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// `Ok(None)` when the file parses but holds no MDB of that name — the caller
/// moves on to the next candidate rather than failing.
fn read_declaration(
    sys: &Path,
    project: &str,
    wanted: &str,
    all_dirs: &[PathBuf],
) -> anyhow::Result<Option<Vec<MdbDatabase>>> {
    let file_name = sys
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let db =
        parse_pdms_db::parse::parse_file_db_basic_data(&sys.to_path_buf(), file_name, project)?;
    let (table, _) = parse_pdms_db::parse::gen_ref_type_pos_table(&db.bytes);

    // Design and catalogue SYS databases both declare `/ALL`, and the
    // catalogue one's CURD is often nearly empty. `UpdateScope` takes the
    // longest for the same reason; picking either is otherwise a coin flip
    // that silently shrinks the scope.
    let mut best: Option<Vec<RefU64>> = None;
    for entry in table.iter() {
        let pos = entry.value().pos;
        if pos < 4 {
            continue;
        }
        let Ok(element) = parse_pdms_db::parse::parse_raw_ele_data(&db.bytes[pos - 4..]) else {
            continue;
        };
        let merged = element.whole_attmap.merge();
        if merged.get_as_string("TYPE").unwrap_or_default().trim() != "MDB"
            || merged.get_as_string("NAME").unwrap_or_default().trim() != wanted
        {
            continue;
        }
        let Some(NamedAttrValue::RefU64Array(items)) = merged.get_val("CURD") else {
            continue;
        };
        let members: Vec<RefU64> = items.iter().map(|item| item.refno()).collect();
        if best.as_ref().is_none_or(|held| members.len() > held.len()) {
            best = Some(members);
        }
    }
    let Some(members) = best else {
        return Ok(None);
    };

    let mut databases = Vec::with_capacity(members.len());
    for refno in members {
        let Some(entry) = table.get(&refno) else {
            continue;
        };
        let pos = entry.value().pos;
        drop(entry);
        if pos < 4 {
            continue;
        }
        let Ok(element) = parse_pdms_db::parse::parse_raw_ele_data(&db.bytes[pos - 4..]) else {
            continue;
        };
        let merged = element.whole_attmap.merge();
        let number = |key: &str| -> Option<i64> {
            merged
                .get_as_string(key)
                .and_then(|value| value.trim().parse::<i64>().ok())
        };
        let Some(dbnum) = number("DBNO").and_then(|value| u32::try_from(value).ok()) else {
            continue;
        };
        databases.push(MdbDatabase {
            dbnum,
            styp: number("STYP").unwrap_or(-1),
            name: merged
                .get_as_string("NAME")
                .unwrap_or_default()
                .trim()
                .to_string(),
            path: locate(all_dirs, dbnum),
        });
    }
    Ok(Some(databases))
}

/// A dbnum names its file but not its directory, and an MDB reaches across
/// projects, so every configured directory is searched.
///
/// Matching goes through the extract-family parser (ADR-028) rather than a
/// name-suffix check. The first cut here was `ends_with("{dbnum}_0001")`,
/// which is wrong twice over: dbnum 100's suffix also tails `ams8100_0001`,
/// handing dbnum 100 another database's file without a sound; and a master
/// with no `_NNNN` suffix — or an extract other than `_0001` — is a legal
/// identity for the same logical database, yet never matched at all and got
/// reported as a deployment gap.
fn locate(dirs: &[PathBuf], dbnum: u32) -> Option<PathBuf> {
    let mut masters: Vec<PathBuf> = Vec::new();
    let mut leaves: Vec<(u32, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(parsed) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
            else {
                continue;
            };
            if parsed.dbnum != dbnum {
                continue;
            }
            match parsed.extract {
                Some(number) => leaves.push((number, path)),
                None => masters.push(path),
            }
        }
    }
    // `read_dir` order is not a contract; sorting pins the pick when several
    // directories answer. Per ADR-028 the highest extract leaf is the working
    // file, and the unsuffixed master is the parent layer — it only answers
    // when no extract exists at all.
    leaves.sort();
    masters.sort();
    leaves
        .pop()
        .map(|(_, path)| path)
        .or_else(|| masters.into_iter().next())
}

type Cache = Vec<(String, String, Arc<MdbMembership>)>;

fn registry() -> &'static RwLock<Cache> {
    static REGISTRY: std::sync::OnceLock<RwLock<Cache>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Publish a resolved declaration so callers that are nowhere near
/// initialization can read it. Replaces any earlier entry for the same pair.
pub fn install(membership: MdbMembership) -> Arc<MdbMembership> {
    let shared = Arc::new(membership);
    if let Ok(mut cache) = registry().write() {
        let key = (shared.project.clone(), shared.mdb.clone());
        cache.retain(|(project, mdb, _)| (project.clone(), mdb.clone()) != key);
        cache.push((key.0, key.1, shared.clone()));
    }
    shared
}

/// The declaration `init_mdb` resolved, or `None` if it has not run for this
/// pair. Deliberately not a lazy resolve: reading a SYS database is not
/// something that should happen behind an innocent-looking getter.
pub fn get(project: &str, mdb: &str) -> Option<Arc<MdbMembership>> {
    let wanted = format!("/{}", mdb.trim_start_matches('/'));
    let cache = registry().read().ok()?;
    cache
        .iter()
        .find(|(known_project, known_mdb, _)| known_project == project && *known_mdb == wanted)
        .map(|(_, _, membership)| membership.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(dbnum: u32, styp: i64, name: &str, on_disk: bool) -> MdbDatabase {
        MdbDatabase {
            dbnum,
            styp,
            name: name.into(),
            path: on_disk.then(|| PathBuf::from(format!("/somewhere/x{dbnum}_0001"))),
        }
    }

    fn membership() -> MdbMembership {
        MdbMembership {
            mdb: "/ALL".into(),
            project: "AvevaMarineSample".into(),
            databases: vec![
                db(6002, DICT_STYP, "*MASTER/SCBDICT", true),
                db(8000, DESI_STYP, "*AMS/DESIGN", true),
                db(5100, DICT_STYP, "*CNPESTD/DICT", true),
                db(7323, DICT_STYP, "*MASTER/MDSDICT", false),
            ],
        }
    }

    /// CURD order decides which definition wins a duplicated UKEY, so the
    /// dictionary list must come back in it rather than sorted by number.
    #[test]
    fn dictionary_paths_keep_curd_order_and_skip_what_is_not_on_disk() {
        let paths = membership().dictionary_paths();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("x6002_0001"));
        assert!(paths[1].ends_with("x5100_0001"));
    }

    /// A declared database with no file is a deployment problem; dropping it
    /// silently is how it stays one.
    #[test]
    fn a_declared_database_with_no_file_is_reported_not_dropped() {
        let membership = membership();
        let missing: Vec<u32> = membership.unresolved().map(|db| db.dbnum).collect();
        assert_eq!(missing, vec![7323]);
        assert_eq!(membership.databases().len(), 4);
    }

    #[test]
    fn counts_are_grouped_by_styp() {
        assert_eq!(
            membership().counts_by_type(),
            BTreeMap::from([(DESI_STYP, 1), (DICT_STYP, 3)])
        );
    }

    #[test]
    fn lookups_normalise_the_leading_slash() {
        install(membership());
        assert!(get("AvevaMarineSample", "ALL").is_some());
        assert!(get("AvevaMarineSample", "/ALL").is_some());
        assert!(get("AvevaMarineSample", "/NOPE").is_none());
        assert!(get("OtherProject", "/ALL").is_none());
    }

    fn dir_with(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in names {
            std::fs::write(dir.path().join(name), b"").expect("touch");
        }
        dir
    }

    /// `ends_with("100_0001")` also tails `ams8100_0001`: a suffix match hands
    /// dbnum 100 another database's file and nothing reports it. The parser
    /// reads the whole number, so 100 finds nothing here and 8100 finds its own.
    #[test]
    fn locate_reads_the_whole_dbnum_not_a_name_suffix() {
        let dir = dir_with(&["ams8100_0001"]);
        let dirs = vec![dir.path().to_path_buf()];

        assert_eq!(locate(&dirs, 100), None, "8100 的文件不属于 100");
        assert_eq!(
            locate(&dirs, 8100).as_deref(),
            Some(dir.path().join("ams8100_0001").as_path())
        );
    }

    /// An unsuffixed master and a non-`_0001` extract are both legal identities
    /// for the same logical database (ADR-028); the leaf is the working file
    /// and wins over the master. Hand copies never match at all.
    #[test]
    fn locate_accepts_masters_and_other_extracts_and_prefers_the_leaf() {
        let dir = dir_with(&["scb100", "scb100_0002", "scb100_0002 copy"]);
        let dirs = vec![dir.path().to_path_buf()];

        assert_eq!(
            locate(&dirs, 100).as_deref(),
            Some(dir.path().join("scb100_0002").as_path()),
            "抽取叶子是工作文件，压过无后缀主库"
        );

        let master_only = dir_with(&["scb100"]);
        assert_eq!(
            locate(&[master_only.path().to_path_buf()], 100).as_deref(),
            Some(master_only.path().join("scb100").as_path()),
            "没有抽取时主库本身就是答案，不该被报成缺件"
        );
    }
}
