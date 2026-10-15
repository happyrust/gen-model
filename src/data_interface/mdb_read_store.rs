//! Shared, immutable HTTP read views. Refresh replaces only changed DbSet slots.
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use aios_core::{NamedAttrMap, RefU64};
use e3d_io::db_element::{DbElement, DbElementError, DbMetadata, DbSet};

use super::direct_store::{DbPin, DirectElementSnapshot, DirectSchema, DirectStore, DirectStoreError};
use super::mdb_membership::MdbMembership;
use super::mdb_route_directory::MdbRouteDirectory;
use super::refno_route::{refno_dbnum, to_refno};

pub struct MdbReadStore {
    set: Arc<DbSet>,
    directory: Arc<MdbRouteDirectory>,
    pub epoch: u64,
    // Existing explicit global-name index, outside point/attribute/tree navigation.
    indexes: DirectStore,
}

/// 树行一次取齐的字段：noun、存的名字、属主、成员原序——**不走描述符抽取**。
///
/// 展开一层只对每个成员要这四样，而它们都在已解析记录里（noun_hash / 显式 NAME /
/// 记录头 owner / 成员表）；`named_attmap` 那张全属性表是给属性面板的，为树行付
/// 一次描述符抽取换不来任何一格数据。与 `named_attmap` 的 TYPE / NAME / OWNER /
/// members 等价：`generation_root` 的 P2-1 对拍在真库上逐条比过 `element_type` /
/// `stored_name` / `member_refnos` 与 attmap 输出。
pub struct TreeNodeParts {
    pub noun: String,
    /// 显式 NAME 原文（未 trim）；没存名字是空串——回落到 noun 是调用方的事。
    pub name: String,
    pub owner: RefU64,
    /// 成员 refno，文件原序，空槽已滤——与 [`MdbReadStore::members`] 同一口径。
    pub members: Vec<RefU64>,
}

impl MdbReadStore {
    fn new(schema: Arc<DirectSchema>, directory: Arc<MdbRouteDirectory>, set: Arc<DbSet>, epoch: u64) -> Self {
        Self { set, epoch, indexes: DirectStore::new(schema, directory.clone()), directory }
    }

    pub fn dbnum_of(&self, refno: RefU64) -> Result<i32, DirectStoreError> {
        let dbnum = refno_dbnum(refno).ok_or_else(|| DirectStoreError::InvalidRefNo { refno: refno.to_string() })?;
        if !self.directory.contains(dbnum) && !self.directory.declared_without_file().any(|(id, _)| id == dbnum) {
            return Err(DirectStoreError::DbNotInMdb { dbnum: dbnum as i32, refno: refno.to_string() });
        }
        Ok(dbnum as i32)
    }

    /// Membership validation and handle construction do not perform file I/O.
    pub fn element(&self, refno: RefU64) -> Result<DbElement, DirectStoreError> {
        let dbnum = self.dbnum_of(refno)?;
        self.directory.route_dbno(dbnum as u32)
            .map_err(|_| DirectStoreError::NoFileForDbnum { dbnum })?;
        Ok(self.set.element(to_refno(refno)))
    }

    pub fn element_snapshot(&self, refno: RefU64) -> Result<DirectElementSnapshot, DirectStoreError> {
        let element = self.element(refno)?;
        let dbnum = element.db_no();
        let extraction = element.extraction().map_err(|error| domain_error(dbnum, error))?;
        let parsed = element.parsed().map_err(|error| domain_error(dbnum, error))?;
        let metadata = self.metadata(dbnum)?;
        Ok(DirectElementSnapshot {
            dbnum: dbnum as i32, db_type: metadata.db_type, file: metadata.source.file,
            sesno: metadata.source.sesno, extraction: (*extraction).clone(),
            explicit_attributes: parsed.explicit_attributes.clone(),
        })
    }

    pub fn named_attmap(&self, refno: RefU64) -> Result<NamedAttrMap, DirectStoreError> {
        let element = self.element(refno)?;
        let extraction = element.extraction().map_err(|error| domain_error(element.db_no(), error))?;
        super::direct_attmap::to_named_attmap(&extraction).map(|attrs| attrs.map)
            .map_err(|source| DirectStoreError::Convert { refno: refno.to_string(), source })
    }

    pub fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> {
        let element = self.element(refno)?;
        Ok(element.member_refnos().map_err(|error| domain_error(element.db_no(), error))?
            .into_iter().filter(|member| member.is_valid())
            .map(|member| RefU64::from_two_nums(member.word0, member.word1)).collect())
    }

    /// 一个元素的树行字段，一份记录解析全出——零描述符抽取。
    ///
    /// 树接口（`DirectTreeService::node`）专用：`element_type` / `stored_name` /
    /// 记录头 owner / 成员表都挂在 `parsed` 上，池级元素缓存命中时整行是几次查表。
    pub fn tree_node_parts(&self, refno: RefU64) -> Result<TreeNodeParts, DirectStoreError> {
        let element = self.element(refno)?;
        let parsed = element.parsed().map_err(|error| domain_error(element.db_no(), error))?;
        let noun = element
            .element_type()
            .map_err(|error| domain_error(element.db_no(), error))?;
        let name = element
            .stored_name()
            .map_err(|error| domain_error(element.db_no(), error))?
            .unwrap_or_default();
        Ok(TreeNodeParts {
            noun,
            name,
            owner: RefU64::from_two_nums(parsed.owner.word0, parsed.owner.word1),
            members: parsed
                .members
                .iter()
                .filter(|member| member.is_valid())
                .map(|member| RefU64::from_two_nums(member.word0, member.word1))
                .collect(),
        })
    }

    pub fn metadata(&self, dbnum: u32) -> Result<DbMetadata, DirectStoreError> {
        self.directory.route_dbno(dbnum).map_err(|_| DirectStoreError::NoFileForDbnum { dbnum: dbnum as i32 })?;
        self.set.metadata(dbnum).map_err(|error| domain_error(dbnum, error))
    }

    pub fn opened_dbnums(&self) -> Vec<i32> { self.set.db_numbers().into_iter().map(|dbnum| dbnum as i32).collect() }
    pub fn cached_elements(&self) -> usize { self.set.cached_element_count() }

    pub fn indexes(&self, dbnum: i32) -> Result<Arc<super::direct_index::DbIndexes>, DirectStoreError> {
        let metadata = self.metadata(dbnum as u32)?;
        self.indexes.pin(DbPin { dbnum, db_type: metadata.db_type, file: metadata.source.file,
            sesno: Some(metadata.source.sesno) });
        self.indexes.indexes(dbnum)
    }
}

fn domain_error(dbnum: u32, error: DbElementError) -> DirectStoreError {
    match error {
        DbElementError::NotFound { refno, dbno, sesno } => DirectStoreError::NoSuchElement {
            refno: format!("{}/{}", refno.word0, refno.word1), dbnum: dbno as i32, sesno: Some(sesno) },
        DbElementError::DbNumberMismatch { file, expected, actual } => DirectStoreError::DbNumberMismatch {
            file, dbnum: expected as i32, actual: actual as i32 },
        DbElementError::DbNotRegistered { dbno } => DirectStoreError::NoFileForDbnum { dbnum: dbno as i32 },
        error => DirectStoreError::Session { dbnum: dbnum as i32, detail: error.to_string() },
    }
}

pub struct MdbReadProvider {
    pub membership: Arc<MdbMembership>,
    pub schema: Arc<DirectSchema>,
    directory: Arc<MdbRouteDirectory>,
    current: Mutex<Arc<MdbReadStore>>,
}

type Providers = HashMap<(String, String), Arc<MdbReadProvider>>;
fn providers() -> &'static Mutex<Providers> {
    static PROVIDERS: OnceLock<Mutex<Providers>> = OnceLock::new();
    PROVIDERS.get_or_init(Default::default)
}

impl MdbReadProvider {
    pub fn shared(project: &str, mdb: &str) -> anyhow::Result<Arc<Self>> {
        let key = (project.to_string(), mdb.trim_start_matches('/').to_string());
        let mut providers = providers().lock().map_err(|_| anyhow::anyhow!("MDB provider lock poisoned"))?;
        if let Some(provider) = providers.get(&key) { return Ok(provider.clone()); }
        let option = aios_core::get_db_option();
        let membership = super::mdb_membership::get(project, mdb).map(Ok)
            .unwrap_or_else(|| super::mdb_membership::resolve(option, project, mdb).map(Arc::new))?;
        let schema = Arc::new(DirectSchema::open_from_env()?);
        let mut directory = MdbRouteDirectory::from_membership(&membership, schema.template_dir());
        // 依赖目录来自 MDB 闭包里的每个项目（ADR-078 D4），不再是 `included_projects`。
        for project in super::project_paths::project_names(option) {
            if let Some(root) = super::project_paths::resolve_project_root(option, &project) {
                directory.add_dependency_dir(&project, &root);
            }
        }
        anyhow::ensure!(!directory.is_empty(), "MDB {mdb} has no readable routes");
        let provider = Self::assemble(membership, schema, Arc::new(directory));
        providers.insert(key, provider.clone());
        Ok(provider)
    }

    /// Test seam: a provider over an explicit route directory, registered under `(project, mdb)`
    /// so `shared` / `invalidate_cached` / `cache_stats` resolve to it. Reads no SYS database and
    /// no `DbOption`, scans no dependency directory. Replaces an earlier provider under the same
    /// key; callers own the key. `tests/shared_read_view_invalidation.rs` is the consumer.
    #[doc(hidden)]
    pub fn install_for_tests(project: &str, mdb: &str, membership: Arc<MdbMembership>,
        schema: Arc<DirectSchema>, directory: MdbRouteDirectory) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(!directory.is_empty(), "MDB {mdb} has no readable routes");
        let provider = Self::assemble(membership, schema, Arc::new(directory));
        providers().lock().map_err(|_| anyhow::anyhow!("MDB provider lock poisoned"))?
            .insert((project.to_string(), mdb.trim_start_matches('/').to_string()), provider.clone());
        Ok(provider)
    }

    /// Epoch 1 over an empty pooled set: the first `view()` opens nothing until an element is read.
    fn assemble(membership: Arc<MdbMembership>, schema: Arc<DirectSchema>, directory: Arc<MdbRouteDirectory>) -> Arc<Self> {
        let set = Arc::new(DbSet::with_resolver(Arc::new(schema.attlib().clone()), Box::new(directory.as_ref().clone())).with_element_cache());
        let current = Arc::new(MdbReadStore::new(schema.clone(), directory.clone(), set, 1));
        Arc::new(Self { membership, schema, directory, current: Mutex::new(current) })
    }

    pub fn view(&self) -> anyhow::Result<Arc<MdbReadStore>> {
        let mut current = self.current.lock().map_err(|_| anyhow::anyhow!("MDB view lock poisoned"))?;
        let changed: HashSet<_> = current.set.opened_sources()?.into_iter()
            .filter(|(_, source)| !source.is_current().unwrap_or(false)).map(|(dbnum, _)| dbnum).collect();
        self.replace_changed(&mut current, &changed)?;
        Ok(current.clone())
    }

    fn replace_changed(&self, current: &mut Arc<MdbReadStore>, changed: &HashSet<u32>) -> anyhow::Result<()> {
        if changed.is_empty() { return Ok(()); }
        let set = Arc::new(current.set.fork_without(changed, Box::new(self.directory.as_ref().clone()))?);
        *current = Arc::new(MdbReadStore::new(self.schema.clone(), self.directory.clone(), set, current.epoch + 1));
        Ok(())
    }
}

pub fn invalidate_cached(project: &str, mdb: &str, dbnum: u32) -> anyhow::Result<()> {
    let provider = providers().lock().map_err(|_| anyhow::anyhow!("MDB provider lock poisoned"))?
        .get(&(project.to_string(), mdb.trim_start_matches('/').to_string())).cloned();
    if let Some(provider) = provider {
        let mut current = provider.current.lock().map_err(|_| anyhow::anyhow!("MDB view lock poisoned"))?;
        provider.replace_changed(&mut current, &HashSet::from([dbnum]))?;
    }
    Ok(())
}

/// Observability only. Health must not create a provider or check source files.
pub fn cache_stats(project: &str, mdb: &str) -> (Vec<i32>, usize) {
    let provider = providers().lock().ok().and_then(|providers|
        providers.get(&(project.to_string(), mdb.trim_start_matches('/').to_string())).cloned());
    provider.and_then(|provider| provider.current.lock().ok().map(|view|
        (view.opened_dbnums(), view.cached_elements()))).unwrap_or_default()
}
