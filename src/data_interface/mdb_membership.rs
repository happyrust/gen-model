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
//! "Across projects" is why a declaration cannot be resolved on the bare number:
//! a dbnum is unique inside a project only, and `AvevaMarineSample` and
//! `AvevaCatalogue` in one configuration both carry a 7000. The SYS record's
//! `PROJ` says whether a declaration is this project's or another's but not
//! which other one — `AvevaCatalogue`'s dictionaries and the undeployed `SCB`
//! block (`6000`–`6003`) all read 3 — so ranking, not `PROJ`, decides between
//! two projects that answer the same number.
//!
//! [`crate::data_interface::update_scope::UpdateScope`] answers the same
//! question for `STYP = DESI` through SurrealDB, which needs the SYS database
//! parsed and synced first. This reads the file, so it is available during
//! initialization — before anything has been synced — which is the point at
//! which the Dictionary set has to be known.

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// The SYS record's `PROJ`: `0` is a database of the project that declares
    /// it, anything else is another project's.
    ///
    /// It does not name that project. The SYS database holds no element type
    /// mapping the number back to a name, and one value covers several
    /// projects: in AMS's `/ALL` the `AvevaCatalogue` databases and the
    /// undeployed `SCB` block (`6000`–`6003`) all read 3. So it decides which
    /// project to look in *first* and never which path to build. Of the
    /// declarations that do resolve to a file, all 58 `PROJ = 0` sit in
    /// `AvevaMarineSample` and all 34 `PROJ = 3` in `AvevaCatalogue`.
    pub proj: i64,
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
    /// The declaration says one project and the file came out of the other.
    ///
    /// Not an error — `*MDU/CATA` (7355) declares `PROJ = 3` while
    /// `AvevaMarineSample` holds the only file — but it is the one thing worth
    /// saying out loud, because everything downstream reads the resolved path
    /// as if the declaration had pointed straight at it.
    pub fn off_declared_project(&self, declaring_project: &str) -> bool {
        let Some(project) = self.project.as_deref() else {
            return false;
        };
        let own = project
            .trim()
            .eq_ignore_ascii_case(declaring_project.trim());
        (self.proj == 0) != own
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
    // Kept per project rather than flattened into one directory list: a dbnum
    // is unique inside a project only, so "which file" cannot be answered
    // before "which project".
    let projects: Vec<(String, Vec<PathBuf>)> = plan
        .projects
        .iter()
        .map(|entry| (entry.project.clone(), entry.db_dirs.clone()))
        .collect();
    let priority = crate::options::catalogue_project_priority();
    let rank = project_rank(&db_option.included_projects, &priority);
    let mut problems: Vec<String> = priority
        .iter()
        .filter(|named| {
            !db_option
                .included_projects
                .iter()
                .any(|included| included.trim().eq_ignore_ascii_case(named.trim()))
        })
        .map(|named| {
            format!(
                "catalogue_project_priority 含未知项目 {named:?}，同号选主本轮只按 \
                 included_projects 的书写顺序排"
            )
        })
        .collect();
    let wanted = format!("/{}", mdb.trim_start_matches('/'));

    let mut tried = Vec::new();
    for sys in sys_candidates(&own_dirs) {
        tried.push(sys.display().to_string());
        match read_declaration(&sys, project, &wanted, &projects, &rank) {
            Ok(Some(databases)) => {
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
    rank: &HashMap<String, usize>,
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
        // Absent `PROJ` reads as "this project": that is what the attribute
        // means when it is written, and it keeps a SYS dialect that omits it
        // resolving against the project whose SYS database this is.
        let proj = number("PROJ").unwrap_or(0);
        let located = locate(projects, rank, project, proj, dbnum);
        databases.push(MdbDatabase {
            dbnum,
            styp: number("STYP").unwrap_or(-1),
            proj,
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

/// Where a declared dbnum resolved to.
struct Located {
    project: String,
    path: PathBuf,
    shadowed: Vec<PathBuf>,
}

/// A dbnum names its file but neither its directory nor its project, and an
/// MDB reaches across projects — so "which file" has to answer "which project"
/// first. `AvevaMarineSample` and `AvevaCatalogue` in one configuration both
/// carry a 7000.
///
/// Ranking is [`project_rank`], the same order the ingest side adjudicates
/// with, and the SYS `PROJ` presses one layer on top of it: `PROJ = 0` looks in
/// the declaring project first, anything else looks elsewhere first. `PROJ`
/// only orders, it never excludes — `*MDU/CATA` (7355) declares `PROJ = 3`
/// while `AvevaMarineSample` holds the only file, and bucketing strictly would
/// report a database that does resolve as a deployment gap.
///
/// What this replaced pooled every project's directories into one list, matched
/// on the bare number and settled ties by path string — in opposite directions
/// per branch: the extract-leaf branch took the lexicographically last path,
/// the unsuffixed-master branch the first. AMS winning 7000 was the alphabet
/// (`AvevaM` > `AvevaC`), nothing else: rename a project, hand the foreign one
/// an `_0002`, or deploy masters without extracts, and the same declaration
/// binds to another project's file without a sound. This list is what the UDA
/// dictionaries are read from, and a dictionary taken from the wrong project
/// reads exactly like a UDA with no value.
fn locate(
    projects: &[(String, Vec<PathBuf>)],
    rank: &HashMap<String, usize>,
    declaring_project: &str,
    proj: i64,
    dbnum: u32,
) -> Option<Located> {
    let mut ranked: Vec<(bool, usize, &str, PathBuf, Vec<PathBuf>)> = Vec::new();
    for (project, dirs) in projects {
        let Some((path, rest)) = pick_within_project(dirs, dbnum) else {
            continue;
        };
        let own = project
            .trim()
            .eq_ignore_ascii_case(declaring_project.trim());
        let against_declaration = if proj == 0 { !own } else { own };
        let place = rank
            .get(&project.trim().to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX);
        ranked.push((against_declaration, place, project.as_str(), path, rest));
    }
    // The project name is the last resort rather than the first, and it now
    // breaks ties in one direction for masters and leaves alike.
    ranked.sort_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)));

    let mut ranked = ranked.into_iter();
    let (_, _, project, path, mut shadowed) = ranked.next()?;
    for (_, _, _, other, rest) in ranked {
        shadowed.push(other);
        shadowed.extend(rest);
    }
    Some(Located {
        project: project.to_string(),
        path,
        shadowed,
    })
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

/// Which project outranks which when a bare dbnum answers twice.
///
/// `catalogue_project_priority` first, then every remaining `included_projects`
/// entry in the order it is written — the explicit list is an override layer,
/// so leaving a project out of it means "no opinion", not "unrankable".
///
/// This is the same order
/// [`crate::data_interface::initialization_phase::select_catalogue_candidates`]
/// adjudicates with, held to it by
/// `the_ranking_agrees_with_the_ingest_side_adjudicator`. It is a second copy
/// rather than a call because that function answers a wider question: it reads
/// the header dbnum its caller scanned, and it turns a same-project duplicate
/// into a blocker. Blocking is right when deciding what to ingest and wrong
/// here, where the same verdict would report a declared database as absent.
fn project_rank(included_projects: &[String], priority: &[String]) -> HashMap<String, usize> {
    let included: HashSet<String> = included_projects
        .iter()
        .map(|project| project.trim().to_ascii_lowercase())
        .collect();
    let mut rank = HashMap::new();
    for (index, project) in priority.iter().enumerate() {
        let key = project.trim().to_ascii_lowercase();
        if key.is_empty() || !included.contains(&key) {
            continue;
        }
        rank.insert(key, index);
    }
    // Offsets start past the explicit list so a named project always outranks
    // an unnamed one.
    for (offset, project) in included_projects.iter().enumerate() {
        let key = project.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        rank.entry(key).or_insert(priority.len() + offset);
    }
    rank
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
            proj: 0,
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

    /// The pick has to come from the project ranking, not from how the two
    /// paths happen to sort. AMS winning 7000 on the live sandbox was
    /// `AvevaM` > `AvevaC` and nothing else, so the directories here are
    /// deliberately handed over in whichever order contradicts the ranking.
    #[test]
    fn a_cross_project_collision_follows_project_rank_not_the_path_alphabet() {
        let one = dir_with(&["ams7000_0001"]);
        let two = dir_with(&["acp7000_0001"]);
        // Give `Main` the path that sorts *first*: the replaced code took the
        // last, so an alphabet-driven pick lands on `Catalogue` here.
        let (main_dir, other_dir) = if one.path() < two.path() {
            (one.path(), two.path())
        } else {
            (two.path(), one.path())
        };
        let (projects, included) = two_projects(main_dir, other_dir);
        let rank = project_rank(&included, &[]);

        let found = locate(&projects, &rank, "Main", 0, 7000).expect("同号两份也要选得出主");
        assert_eq!(found.project, "Main");
        assert!(found.path.starts_with(main_dir));
        assert_eq!(
            found.shadowed,
            vec![other_dir.join(if one.path() < two.path() {
                "acp7000_0001"
            } else {
                "ams7000_0001"
            })],
            "另一个项目的同号文件要交出来"
        );
    }

    /// The two branches used to sort in opposite directions — leaves took the
    /// last path, masters the first — so one and the same declaration flipped
    /// to the other project depending on whether the site deploys extracts.
    #[test]
    fn masters_and_leaves_answer_with_the_same_project() {
        let leaves = (dir_with(&["ams7000_0001"]), dir_with(&["acp7000_0001"]));
        let masters = (dir_with(&["ams7000"]), dir_with(&["acp7000"]));
        for (main, other) in [&leaves, &masters] {
            let (projects, included) = two_projects(main.path(), other.path());
            let rank = project_rank(&included, &[]);
            let found = locate(&projects, &rank, "Main", 0, 7000).expect("解得出");
            assert_eq!(found.project, "Main", "抽取与主库两条分支必须同一个答案");
        }
    }

    /// `PROJ = 0` means "a database of the project declaring it". It presses on
    /// top of the ranking, which is the only thing that can pull a declaration
    /// back when an explicit priority puts another project first.
    #[test]
    fn proj_orders_the_search_before_the_ranking_does() {
        let own = dir_with(&["ams7000_0001"]);
        let other = dir_with(&["acp7000_0001"]);
        let (projects, included) = two_projects(own.path(), other.path());
        let rank = project_rank(&included, &["Catalogue".into()]);

        assert_eq!(
            locate(&projects, &rank, "Main", 0, 7000)
                .expect("PROJ=0")
                .project,
            "Main"
        );
        assert_eq!(
            locate(&projects, &rank, "Main", 3, 7000)
                .expect("PROJ=3")
                .project,
            "Catalogue"
        );
    }

    /// `*MDU/CATA` (7355) declares `PROJ = 3` while `AvevaMarineSample` holds
    /// the only file. `PROJ` therefore orders the search and never restricts
    /// it: bucketing strictly would turn a database that does resolve into a
    /// reported deployment gap.
    #[test]
    fn a_foreign_declaration_still_resolves_when_only_the_declaring_project_has_it() {
        let own = dir_with(&["ams7355_0001"]);
        let empty = dir_with(&[]);
        let (projects, included) = two_projects(own.path(), empty.path());
        let rank = project_rank(&included, &[]);

        let found = locate(&projects, &rank, "Main", 3, 7355).expect("外项目声明也要解出来");
        assert_eq!(found.project, "Main");
        assert!(found.shadowed.is_empty());
    }

    /// Two answers to "which file is dbnum 7000" inside one process is the
    /// defect this ranking exists to close, so the ingest-side adjudicator is
    /// asked the same question here. `PROJ` is kept out of it — the declaring
    /// project owns neither candidate — to leave the ranking alone under test.
    #[test]
    fn the_ranking_agrees_with_the_ingest_side_adjudicator() {
        use crate::data_interface::initialization_phase::{
            CatalogueCandidate, select_catalogue_candidates,
        };

        let one = dir_with(&["ams7000_0001"]);
        let two = dir_with(&["acp7000_0001"]);
        let (projects, included) = two_projects(one.path(), two.path());
        let priority: Vec<String> = vec!["Catalogue".into()];

        let mine = locate(
            &projects,
            &project_rank(&included, &priority),
            "Elsewhere",
            0,
            7000,
        )
        .expect("解得出");
        let theirs = select_catalogue_candidates(
            [
                CatalogueCandidate {
                    project: "Main".into(),
                    dbnum: 7000,
                    path: one.path().join("ams7000_0001"),
                },
                CatalogueCandidate {
                    project: "Catalogue".into(),
                    dbnum: 7000,
                    path: two.path().join("acp7000_0001"),
                },
            ],
            &included,
            &priority,
        );
        let winner = theirs.selected.first().expect("摄入侧也要选出一个");

        assert_eq!(mine.project, winner.project);
        assert_eq!(mine.path, winner.path);
    }
}
