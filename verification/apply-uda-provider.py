from pathlib import Path
import re

p = Path('src/data_interface/element_attributes.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('    catalog: Arc<UdaCatalog>,', '    catalog: Arc<super::uda_catalog_provider::UdaCatalogProvider>,', 1)
s = s.replace('let catalog = Arc::new(load_uda_catalog(&membership, &schema)?);',
              'let catalog = super::uda_catalog_provider::UdaCatalogProvider::shared(&membership, schema.clone());', 1)
s = re.sub(r'self\s*\.catalog\b', 'catalog', s)
s = s.replace('        let mut snapshot = self.store.element_snapshot(refno)?;',
              '        let mut snapshot = self.store.element_snapshot(refno)?;\n        let catalog = self.catalog.get()?;', 1)
s = s.replace('        let root_snapshot = self.store.element_snapshot(root)?;',
              '        let root_snapshot = self.store.element_snapshot(root)?;\n        let catalog = self.catalog.get()?;', 1)
a = s.index('    pub async fn refresh_cached(&self)')
b = s.index('        let started = Instant::now();', a)
s = s[:b] + '        let catalog = self.catalog.get()?;\n' + s[b:]
s = s.replace('self.offer_backup(&snapshot)', 'self.offer_backup(&snapshot, &catalog)', 1)
s = s.replace('fn offer_backup(&self, snapshot: &DirectElementSnapshot)',
              'fn offer_backup(&self, snapshot: &DirectElementSnapshot, catalog: &UdaCatalog)', 1)
a = s.index('pub(crate) fn load_uda_catalog(')
b = s.index('fn identity_attribute(', a)
s = s[:a] + s[b:]
a = s.index('fn membership_fingerprint(')
b = s.index('    for db in membership.databases() {', a)
s = s[:b] + s[b:].replace('    for db in membership.databases() {',
    '    for db in membership.databases().iter().filter(|db| db.styp != super::mdb_membership::DICT_STYP) {', 1)
p.write_bytes(s.encode('utf-8'))

p = Path('src/data_interface/mdb_memory_store.rs')
s = p.read_text(encoding='utf-8')
for line in ['use e3d_io::uda_catalog::UdaCatalog;\n',
             '    uda_catalog: Arc<UdaCatalog>,\n',
             '        let schema = super::direct_store::DirectSchema::open_from_env()?;\n',
             '        let uda_catalog = super::element_attributes::load_uda_catalog(membership, &schema)?;\n',
             '            uda_catalog: Arc::new(uda_catalog),\n',
             '            uda_catalog: Arc::new(UdaCatalog::default()),\n']:
    assert line in s, line
    s = s.replace(line, '')
a = s.index('    pub fn uda_catalog(')
b = s.index('    pub fn mdb(', a)
s = s[:a] + s[b:]
p.write_bytes(s.encode('utf-8'))

p = Path('src/data_interface/read_through.rs')
s = p.read_text(encoding='utf-8')
s = s.replace('    let snapshot = super::mdb_memory_store::ensure(&option.project_name, &option.mdb_name)?;\n    let catalog = snapshot.uda_catalog();', '''    let membership = super::mdb_membership::get(&option.project_name, &option.mdb_name)
        .map(Ok).unwrap_or_else(|| super::mdb_membership::resolve(
            option, &option.project_name, &option.mdb_name).map(Arc::new))?;
    let schema = Arc::new(super::direct_store::DirectSchema::open_from_env()?);
    let provider = super::uda_catalog_provider::UdaCatalogProvider::shared(&membership, schema);
    let (snapshot_epoch, catalog) = provider.snapshot()?;''', 1)
s = s.replace('    let snapshot_epoch = snapshot.epoch();\n', '', 1)
p.write_bytes(s.encode('utf-8'))
