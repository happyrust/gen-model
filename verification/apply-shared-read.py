from pathlib import Path

p = Path('src/data_interface/element_attributes.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('DirectSchema, DirectStore, DirectStoreError', 'DirectSchema, DirectStoreError')
s = s.replace('use super::mdb_route_directory::MdbRouteDirectory;', 'use super::mdb_read_store::{MdbReadProvider, MdbReadStore};')
s = s.replace('    store: Arc<DirectStore>,', '    store: Arc<MdbReadStore>,\n    reader: Arc<MdbReadProvider>,')
a = s.index('    /// MDB 非 DICT')
b = s.index('    snapshot_epoch: u64,', a)
s = s[:a] + '    /// Process-local read-view epoch.\n' + s[b:]
a = s.index('        let membership =', s.index('    fn open(project:'))
b = s.index('        let catalog =', a)
s = s[:a] + '''        let reader = MdbReadProvider::shared(project, mdb)?;
        let membership = reader.membership.clone();
        let schema = reader.schema.clone();
        let store = reader.view()?;
        let snapshot_epoch = store.epoch;
''' + s[b:]
s = s.replace('        let file_fingerprint = membership_fingerprint(&membership)?;\n', '')
s = s.replace('        let store = Arc::new(DirectStore::new(schema, Arc::new(directory)));\n', '')
s = s.replace('                store,\n                catalog,', '                store,\n                reader,\n                catalog,', 1)
s = s.replace('                file_fingerprint,\n                snapshot_epoch: file_fingerprint,', '                snapshot_epoch,', 1)
a = s.index('    /// 非 DICT 成员变化')
b = s.index('    pub fn read(', a)
s = s[:a] + '''    fn is_current(&self) -> anyhow::Result<bool> {
        Ok(Arc::ptr_eq(&self.store, &self.reader.view()?))
    }

''' + s[b:]
a = s.index('fn membership_fingerprint(')
b = s.index('fn identity_attribute(', a)
s = s[:a] + s[b:]
p.write_bytes(s.encode('utf-8'))

p = Path('src/data_interface/direct_tree.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('use super::direct_store::{DbPin, DirectSchema, DirectStore};',
              'use super::direct_store::{DirectStore, DirectStoreError};\nuse super::mdb_read_store::{MdbReadProvider, MdbReadStore};')
s = s.replace('use super::mdb_membership::{ISOD_STYP, MdbMembership};', 'use super::mdb_membership::MdbMembership;')
s = s.replace('use super::mdb_route_directory::MdbRouteDirectory;\n', '')
s = s.replace('    identities: Vec<(std::path::PathBuf, super::db_generation::FileIdentity)>,\n', '')
a = s.index('    fn open(membership:')
b = s.index('    pub fn databases(', a)
s = s[:a] + '''    fn open(membership: &MdbMembership, store: &MdbReadStore) -> anyhow::Result<Self> {
        let mut result = Self { databases: Vec::new(), worlds: HashMap::new(), ref0s: HashMap::new(), problems: membership.problems().to_vec() };
        for db in membership.element_databases() {
            if db.path.is_none() { continue; }
            let metadata = store.metadata(db.dbnum)?;
            result.worlds.insert(db.dbnum, RefU64::from_two_nums(metadata.world.word0, metadata.world.word1));
            result.ref0s.insert(db.dbnum, metadata.extent_ref0s);
            result.databases.push(LoadedDb { dbnum: db.dbnum, db_type: metadata.db_type,
                file: metadata.source.file, sesno: metadata.source.sesno, elements: 0 });
        }
        Ok(result)
    }
''' + s[b:]
s = s.replace('    store: Arc<DirectStore>,\n    metadata: Arc<TreeMetadata>,',
'''    store: Arc<MdbReadStore>,
    reader: Arc<MdbReadProvider>,
    metadata: std::sync::Mutex<Option<Arc<TreeMetadata>>>,''')
a = s.index('        let option =', s.index('    pub fn open(project:'))
b = s.index('    pub fn project(', a)
s = s[:a] + '''        let reader = MdbReadProvider::shared(project, mdb)?;
        let store = reader.view()?;
        Ok(Self {
            project: project.to_string(), mdb: format!("/{}", mdb.trim_start_matches('/')),
            membership: reader.membership.clone(), store, reader,
            metadata: std::sync::Mutex::new(None),
        })
    }

    pub fn metadata(&self) -> anyhow::Result<Arc<TreeMetadata>> {
        let mut slot = self.metadata.lock().map_err(|_| anyhow::anyhow!("tree metadata lock poisoned"))?;
        if let Some(metadata) = slot.as_ref() { return Ok(metadata.clone()); }
        let metadata = Arc::new(TreeMetadata::open(&self.membership, &self.store)?);
        *slot = Some(metadata.clone());
        Ok(metadata)
    }

    fn is_current(&self) -> bool {
        self.reader.view().is_ok_and(|view| Arc::ptr_eq(&self.store, &view))
    }

''' + s[b:]
a = s.index('    /// WORL 从骨架查')
b = s.index('    pub fn roots(', a)
s = s[:a] + '    /// Enumerating every SITE explicitly needs each library WORLD; ancestors/children do not.\n' + s[b:]
s = s.replace('    pub fn roots(&self) -> anyhow::Result<Vec<EleTreeNode>> {\n',
              '    pub fn roots(&self) -> anyhow::Result<Vec<EleTreeNode>> {\n        let metadata = self.metadata()?;\n', 1)
s = s.replace('self.metadata.worlds', 'metadata.worlds')
s = s.replace('            let dbnum = db.dbnum as i32;\n', '')
s = s.replace('self.store.members_in(dbnum, world)?', 'self.store.members(world)?')
s = s.replace('generation_roots_in_subtree_on(&self.store,', 'generation_roots_in_subtree_on(self.store.as_ref(),')
a = s.index('pub fn generation_roots_in_subtree_on(')
s = s[:a] + '''pub trait TreeStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError>;
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError>;
}
impl TreeStore for DirectStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> { DirectStore::named_attmap(self, refno) }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> { DirectStore::members(self, refno) }
}
impl TreeStore for MdbReadStore {
    fn named_attmap(&self, refno: RefU64) -> Result<aios_core::NamedAttrMap, DirectStoreError> { MdbReadStore::named_attmap(self, refno) }
    fn members(&self, refno: RefU64) -> Result<Vec<RefU64>, DirectStoreError> { MdbReadStore::members(self, refno) }
}

''' + s[a:]
s = s.replace('    store: &DirectStore,', '    store: &dyn TreeStore,', 1)
# Obsolete comments described the deleted skeleton reload path.
a = s.index('    /// 骨架装不上')
b = s.index('    pub async fn get(', a)
s = s[:a] + s[b:]
p.write_bytes(s.encode('utf-8'))

p = Path('src/web_service/handlers.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('let skeleton = service.metadata();', 'let skeleton = service.metadata().map_err(ApiError::from_domain)?;')
a = s.index('    let (mut opened, attribute_elements) =')
b = s.index('    let source_reads =', a)
s = s[:a] + '''    let (mut opened, cached_elements) = crate::data_interface::mdb_read_store::cache_stats(
        &state.identity.project, &state.identity.mdb);
    opened.sort_unstable();
    core["open_db_count"] = json!(opened.len());
    core["open_dbnums"] = json!(opened);
    core["cached_elements"] = json!(cached_elements);
''' + s[b:]
p.write_bytes(s.encode('utf-8'))
