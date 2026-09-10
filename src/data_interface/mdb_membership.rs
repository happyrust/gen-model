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
//! SYS PROJ is a character vector (ACP / SCB / APS), not an integer flag.
//! An empty vector selects the declaring project. Never substitute another
//! project merely because it contains a file with the same dbnum.
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
/// Isometric element database.  It participates in the source model exactly
/// like DESI; CATA remains catalogue-only and is deliberately excluded.
pub const ISOD_STYP: i64 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdbDatabase {
    pub dbnum: u32,
    pub styp: i64,
    /// Legacy display field: length of PROJ, never a project identifier.
    pub proj: i64,
    /// Exact SYS PROJ character vector. None means the declaring project.
    pub declared_project_code: Option<String>,
    /// The SYS element's name, e.g. `*MASTER/DICT`.
    pub name: String,
    /// `None` when the declaration names a database with no file under any
    /// configured project directory. Kept rather than dropped: a declared
    /// database that is not on disk is a deployment problem worth seeing.
    pub path: Option<PathBuf>,
    /// Which project `path` came out of.
    pub project: Option<String>,
    /// The other files carrying this dbnum: layers this project's own
    /// extract family put underneath (ADR-028), plus whatever another project
    /// numbers the same. A dbnum is unique inside a project only, so losing
    /// these silently is how a declaration binds to a foreign file unnoticed.
    pub shadowed: Vec<PathBuf>,
}

impl MdbDatabase {
    /// Detect a declaration/path mismatch without reducing PROJ to a flag.
    pub fn off_declared_project(&self, declaring_project: &str) -> bool {
        let Some(project) = self.project.as_deref() else {
            return false;
        };
        match self.declared_project_code.as_deref() {
            None => !project
                .trim()
                .eq_ignore_ascii_case(declaring_project.trim()),
            Some(code) => !self
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
                .is_some_and(|file| file.prefix.eq_ignore_ascii_case(code)),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MdbMembership {
    mdb: String,
    project: String,
    /// In `CURD` order. Order decides which definition wins a duplicated
    /// `UKEY`, so it is preserved rather than sorted.
    databases: Vec<MdbDatabase>,
    /// Things that went wrong while resolving this list and that a person
    /// should see. They deliberately do not live only in `log::warn!`: the
    /// sandbox and plenty of deployments run `enable_log = false`, the logger
    /// is then never initialised, and not one of those lines ever appears.
    problems: Vec<String>,
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

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn of_type(&self, styp: i64) -> impl Iterator<Item = &MdbDatabase> {
        self.databases.iter().filter(move |db| db.styp == styp)
    }

    pub fn element_databases(&self) -> impl Iterator<Item = &MdbDatabase> {
        self.databases
            .iter()
            .filter(|db| matches!(db.styp, DESI_STYP | ISOD_STYP))
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

    /// A complete room reference frame requires every declared design file.
    /// An absent file may contain a room or a panel; it is not an empty database.
    pub(crate) fn require_design_files(&self) -> anyhow::Result<()> {
        let missing = self
            .unresolved()
            .filter(|db| db.styp == DESI_STYP)
            .map(|db| format!("{} ({})", db.dbnum, db.name))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            missing.is_empty(),
            "MDB {} / {} room inputs incomplete: missing DESI source files: {}",
            self.project,
            self.mdb,
            missing.join(", ")
        );
        Ok(())
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
    // Kept per project rather than flattened into one directory list: a dbnum
    // is unique inside a project only, so "which file" cannot be answered
    // before "which project".
    let projects: Vec<(String, Vec<PathBuf>)> = plan
        .projects
        .iter()
        .map(|entry| (entry.project.clone(), entry.db_dirs.clone()))
        .collect();
    let mut problems = Vec::new();
    let wanted = format!("/{}", mdb.trim_start_matches('/'));

    let mut tried = Vec::new();
    for sys in sys_candidates(&own_dirs) {
        tried.push(sys.display().to_string());
        match read_declaration(&sys, project, &wanted, &projects) {
            Ok(Some(databases)) => {
                for db in databases.iter().filter(|db| db.path.is_none()) {
                    problems.push(format!(
                        "MDB {wanted}: dbnum={} ({}) declared project {} has no matching configured source; same-dbnum foreign fallback disabled",
                        db.dbnum, db.name, db.declared_project_code.as_deref().unwrap_or(project)
                    ));
                }
                return Ok(MdbMembership {
                    mdb: wanted,
                    project: project.to_string(),
                    databases,
                    problems,
                });
            }
            Ok(None) => continue,
            Err(error) => {
                log::warn!("读取 {} 失败，继续找下一个：{error:#}", sys.display());
                problems.push(format!("读取 {} 失败，已跳过：{error:#}", sys.display()));
            }
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
    projects: &[(String, Vec<PathBuf>)],
) -> anyhow::Result<Option<Vec<MdbDatabase>>> {
    // e3d-io 读法（parse_pdms_db 退役清单 §B5）：SYS 库按最新会话钉成一个单库 `DbSet`
    // （模板 `sysvir.dat`），MDB 元素按存储的 NAME 找（`find_named` 是一次 Named 档全树扫，
    // 与旧读法整库 `gen_ref_type_pos_table` 同量级），属性经 `direct_attmap::to_named_attmap`
    // 落成与旧 `whole_attmap.merge()` 同一形状的 `NamedAttrMap`——下面对 `CURD` / `DBNO` /
    // `PROJ` / `STYP` / `NAME` 的读法逐字不变。
    let sys_db = SysDb::open(sys)?;

    // Design and catalogue SYS databases both declare `/ALL`, and the
    // catalogue one's CURD is often nearly empty. `UpdateScope` takes the
    // longest for the same reason; picking either is otherwise a coin flip
    // that silently shrinks the scope.
    let mut best: Option<Vec<RefU64>> = None;
    for refno in sys_db.named(wanted)? {
        let Some(merged) = sys_db.attrs(refno) else {
            continue;
        };
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
        // A `CURD` entry the SYS index does not hold, or a record that will not
        // decode, is skipped exactly as the legacy byte reader skipped it.
        let Some(merged) = sys_db.attrs(refno) else {
            continue;
        };
        let number = |key: &str| -> Option<i64> {
            merged
                .get_as_string(key)
                .and_then(|value| value.trim().parse::<i64>().ok())
        };
        let Some(dbnum) = number("DBNO").and_then(|value| u32::try_from(value).ok()) else {
            continue;
        };
        // Read the native character vector before NamedAttrMap can stringify
        // it as an array length. Missing/undecoded evidence is not an empty PROJ.
        let declared_project_code = sys_db.project_code(refno)?;
        // Keep the legacy numeric field as a length, not a project identifier.
        let proj = declared_project_code
            .as_ref()
            .map_or(0, |code| code.len() as i64);
        let located = locate_declared(projects, project, declared_project_code.as_deref(), dbnum)?;
        databases.push(MdbDatabase {
            dbnum,
            styp: number("STYP").unwrap_or(-1),
            proj,
            declared_project_code,
            name: merged
                .get_as_string("NAME")
                .unwrap_or_default()
                .trim()
                .to_string(),
            path: located.as_ref().map(|found| found.path.clone()),
            project: located.as_ref().map(|found| found.project.clone()),
            shadowed: located.map(|found| found.shadowed).unwrap_or_default(),
        });
    }
    Ok(Some(databases))
}

/// One SYS database file, pinned at its latest session and read through e3d-io.
struct SysDb {
    set: Arc<e3d_io::DbSet>,
    dbno: u32,
}

impl SysDb {
    fn project_code(&self, refno: RefU64) -> anyhow::Result<Option<String>> {
        let extraction = self
            .set
            .element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()))
            .extraction()?;
        let attr = extraction
            .attributes
            .iter()
            .find(|attr| attr.name == "PROJ")
            .ok_or_else(|| anyhow::anyhow!("SYS {refno}: PROJ descriptor missing"))?;
        decode_project_code(attr.value.as_ref())
    }
    fn open(sys: &Path) -> anyhow::Result<Self> {
        let schema = crate::data_interface::direct_store::DirectSchema::open_from_env()?;
        let header = crate::data_interface::db_file_header::DbFileHeader::read(sys)?;
        let set = crate::data_interface::e3d_collector::open_pinned_set(
            &schema,
            sys,
            &header.db_type,
            header.latest_sesno(),
        )?;
        Ok(Self {
            set,
            dbno: header.dbnum as u32,
        })
    }

    /// Elements whose stored `NAME` is exactly `name` (leading `/` included).
    fn named(&self, name: &str) -> anyhow::Result<Vec<RefU64>> {
        Ok(self
            .set
            .find_named(self.dbno, name)
            .map_err(|error| anyhow::anyhow!("scan SYS database names: {error}"))?
            .into_iter()
            .map(|refno| RefU64::from_two_nums(refno.word0, refno.word1))
            .collect())
    }

    /// The element's attributes in the shape the legacy `whole_attmap.merge()`
    /// produced; `None` when the SYS index has no such element or the record
    /// does not decode (the byte reader skipped both the same way).
    fn attrs(&self, refno: RefU64) -> Option<aios_core::NamedAttrMap> {
        let element = self
            .set
            .element(e3d_io::RefNo::new(refno.get_0(), refno.get_1()));
        if !element.exists().ok()? {
            return None;
        }
        let extraction = element.extraction().ok()?;
        crate::data_interface::direct_attmap::to_named_attmap(&extraction)
            .ok()
            .map(|attrs| attrs.map)
    }
}

/// Where a declared dbnum resolved to.
struct Located {
    project: String,
    path: PathBuf,
    shadowed: Vec<PathBuf>,
}

// core.dll DB_DB::setFromDbele (0x5912440) reads PROJ as a character
// vector, passing it to CHFRAN, not as a numeric foreign-project flag.
fn decode_project_code(
    value: Option<&e3d_io::record::descriptor::DescriptorValue>,
) -> anyhow::Result<Option<String>> {
    use e3d_io::record::descriptor::DescriptorValue;
    let words = match value {
        Some(DescriptorValue::RawWords(words) | DescriptorValue::WordArray(words)) => words.clone(),
        Some(DescriptorValue::IntArray(words)) => words.iter().map(|word| *word as u32).collect(),
        Some(DescriptorValue::Unset) => return Ok(None),
        other => anyhow::bail!("SYS PROJ is not a decoded character vector: {other:?}"),
    };
    if words.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        words.len() == 3
            && words
                .iter()
                .all(|word| *word <= 127 && (*word as u8).is_ascii_alphabetic()),
        "invalid SYS project code: {words:?}"
    );
    Ok(Some(
        words
            .into_iter()
            .map(|word| (word as u8 as char).to_ascii_uppercase())
            .collect(),
    ))
}

/// Bind only the declared project. A similarly numbered file in another
/// project is diagnostic evidence, never a fallback input.
fn locate_declared(
    projects: &[(String, Vec<PathBuf>)],
    declaring: &str,
    code: Option<&str>,
    dbnum: u32,
) -> anyhow::Result<Option<Located>> {
    let mut matches = Vec::new();
    let mut shadowed = Vec::new();
    for (project, dirs) in projects {
        let Some((path, rest)) = pick_within_project(dirs, dbnum) else {
            continue;
        };
        let selected = match code {
            None => project.trim().eq_ignore_ascii_case(declaring.trim()),
            Some(code) => path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(crate::data_interface::extract_family::parse_extract_file_name)
                .is_some_and(|file| file.prefix.eq_ignore_ascii_case(code)),
        };
        if selected {
            matches.push((project.clone(), path, rest));
        } else {
            shadowed.push(path);
            shadowed.extend(rest);
        }
    }
    anyhow::ensure!(
        matches.len() <= 1,
        "ambiguous MDB project binding: project={declaring}, PROJ={code:?}, dbnum={dbnum}"
    );
    Ok(matches.pop().map(|(project, path, mut rest)| {
        rest.extend(shadowed);
        Located {
            project,
            path,
            shadowed: rest,
        }
    }))
}

/// ADR-028 inside one project: the highest extract leaf is the working file,
/// the unsuffixed master is the parent layer and only answers when no extract
/// exists. Everything it did not pick comes back rather than being dropped.
///
/// Matching goes through the extract-family parser rather than a name-suffix
/// check. The first cut here was `ends_with("{dbnum}_0001")`, which is wrong
/// twice over: dbnum 100's suffix also tails `ams8100_0001`, handing dbnum 100
/// another database's file without a sound; and a master with no `_NNNN`
/// suffix — or an extract other than `_0001` — is a legal identity for the
/// same logical database, yet never matched at all and got reported as a
/// deployment gap.
fn pick_within_project(dirs: &[PathBuf], dbnum: u32) -> Option<(PathBuf, Vec<PathBuf>)> {
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
    // directories answer.
    leaves.sort();
    masters.sort();

    let mut rest: Vec<PathBuf> = Vec::new();
    let Some(top) = leaves.last().map(|(extract, _)| *extract) else {
        let mut masters = masters.into_iter();
        let winner = masters.next()?;
        rest.extend(masters);
        return Some((winner, rest));
    };
    let (working, older): (Vec<_>, Vec<_>) =
        leaves.into_iter().partition(|(extract, _)| *extract == top);
    let mut working = working.into_iter().map(|(_, path)| path);
    let winner = working
        .next()
        .expect("the top extract has at least one file");
    rest.extend(working);
    rest.extend(older.into_iter().map(|(_, path)| path));
    rest.extend(masters);
    Some((winner, rest))
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

    #[test]
    fn project_characters_are_not_a_foreign_project_flag() {
        use e3d_io::record::descriptor::DescriptorValue::RawWords;
        assert_eq!(
            decode_project_code(Some(&RawWords(vec![65, 67, 80])))
                .unwrap()
                .as_deref(),
            Some("ACP")
        );
        assert_eq!(
            decode_project_code(Some(&RawWords(vec![83, 67, 66])))
                .unwrap()
                .as_deref(),
            Some("SCB")
        );
        assert_eq!(decode_project_code(Some(&RawWords(vec![]))).unwrap(), None);
        assert!(decode_project_code(None).is_err());
        assert!(decode_project_code(Some(&RawWords(vec![3]))).is_err());
    }

    #[test]
    fn exact_project_binding_never_falls_back_to_same_dbnum() {
        let own = dir_with(&["ams7000_0001", "ams7355_0001"]);
        let other = dir_with(&["acp7000_0001"]);
        let (projects, _) = two_projects(own.path(), other.path());
        assert_eq!(
            locate_declared(&projects, "Main", None, 7000)
                .unwrap()
                .unwrap()
                .project,
            "Main"
        );
        assert_eq!(
            locate_declared(&projects, "Main", Some("ACP"), 7000)
                .unwrap()
                .unwrap()
                .project,
            "Catalogue"
        );
        assert!(
            locate_declared(&projects, "Main", Some("APS"), 7355)
                .unwrap()
                .is_none()
        );
        assert!(
            locate_declared(&projects, "Missing", None, 7000)
                .unwrap()
                .is_none()
        );
    }

    fn db(dbnum: u32, styp: i64, name: &str, on_disk: bool) -> MdbDatabase {
        MdbDatabase {
            dbnum,
            styp,
            proj: 0,
            declared_project_code: None,
            name: name.into(),
            path: on_disk.then(|| PathBuf::from(format!("/somewhere/x{dbnum}_0001"))),
            project: on_disk.then(|| "AvevaMarineSample".to_string()),
            shadowed: Vec::new(),
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
            problems: Vec::new(),
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
    fn missing_design_file_blocks_complete_room_inputs_but_empty_mdb_is_valid() {
        let mut declaration = membership();
        assert!(
            declaration.require_design_files().is_ok(),
            "missing dictionary is a separate check"
        );
        declaration
            .databases
            .push(db(9999, DESI_STYP, "*MASTER/MISSING", false));
        let error = declaration.require_design_files().unwrap_err().to_string();
        assert!(
            error.contains("9999") && error.contains("*MASTER/MISSING") && error.contains("/ALL")
        );
        declaration.databases.clear();
        assert!(
            declaration.require_design_files().is_ok(),
            "authoritative empty declaration differs from a missing file"
        );
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

        assert_eq!(
            pick_within_project(&dirs, 100),
            None,
            "8100 的文件不属于 100"
        );
        assert_eq!(
            pick_within_project(&dirs, 8100).map(|(path, _)| path),
            Some(dir.path().join("ams8100_0001"))
        );
    }

    /// An unsuffixed master and a non-`_0001` extract are both legal identities
    /// for the same logical database (ADR-028); the leaf is the working file
    /// and wins over the master. Hand copies never match at all.
    #[test]
    fn locate_accepts_masters_and_other_extracts_and_prefers_the_leaf() {
        let dir = dir_with(&["scb100", "scb100_0002", "scb100_0002 copy"]);
        let dirs = vec![dir.path().to_path_buf()];

        let (winner, shadowed) = pick_within_project(&dirs, 100).expect("解得出");
        assert_eq!(
            winner,
            dir.path().join("scb100_0002"),
            "抽取叶子是工作文件，压过无后缀主库"
        );
        assert_eq!(
            shadowed,
            vec![dir.path().join("scb100")],
            "被压在下面的主库要交出来，不能静默丢掉"
        );

        let master_only = dir_with(&["scb100"]);
        assert_eq!(
            pick_within_project(&[master_only.path().to_path_buf()], 100).map(|(path, _)| path),
            Some(master_only.path().join("scb100")),
            "没有抽取时主库本身就是答案，不该被报成缺件"
        );
    }

    fn two_projects(
        main_dir: &Path,
        other_dir: &Path,
    ) -> (Vec<(String, Vec<PathBuf>)>, Vec<String>) {
        (
            vec![
                ("Main".to_string(), vec![main_dir.to_path_buf()]),
                ("Catalogue".to_string(), vec![other_dir.to_path_buf()]),
            ],
            vec!["Main".into(), "Catalogue".into()],
        )
    }
}
