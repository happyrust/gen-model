//! UDA definitions have a DICT lifecycle, independent of DESI element caches.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::Context;
use e3d_io::uda_catalog::UdaCatalog;

use super::direct_store::DirectSchema;
use super::mdb_membership::MdbMembership;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DictionaryIdentity {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

struct CachedCatalog {
    identities: Vec<DictionaryIdentity>,
    catalog: Arc<UdaCatalog>,
    epoch: u64,
}

pub struct UdaCatalogProvider {
    paths: Vec<PathBuf>,
    schema: Arc<DirectSchema>,
    cached: Mutex<Option<CachedCatalog>>,
}

impl UdaCatalogProvider {
    /// CURD order is part of the key: later DICT entries have merge precedence.
    /// Sharing survives an attribute service reload caused by a DESI SAVEWORK.
    pub fn shared(membership: &MdbMembership, schema: Arc<DirectSchema>) -> Arc<Self> {
        type Key = (PathBuf, Vec<PathBuf>);
        static PROVIDERS: OnceLock<Mutex<HashMap<Key, Arc<UdaCatalogProvider>>>> = OnceLock::new();
        let paths = membership.dictionary_paths();
        let key = (schema.template_dir().to_owned(), paths.clone());
        let mut providers = PROVIDERS.get_or_init(Default::default).lock().unwrap();
        providers
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Self {
                    paths,
                    schema,
                    cached: Mutex::new(None),
                })
            })
            .clone()
    }

    pub fn get(&self) -> anyhow::Result<Arc<UdaCatalog>> {
        Ok(self.snapshot()?.1)
    }

    /// The returned epoch describes exactly the definitions returned with it.
    pub fn snapshot(&self) -> anyhow::Result<(u64, Arc<UdaCatalog>)> {
        self.load_if_changed(|| {
            let dicvir = self.schema.template_dir().join("dicvir.dat");
            let mut merged = UdaCatalog::default();
            for dictionary in &self.paths {
                let catalog = UdaCatalog::read(dictionary, &dicvir, self.schema.attlib()).map_err(
                    |error| anyhow::anyhow!("read DICT {}: {error}", dictionary.display()),
                )?;
                merged.merge_in_order(catalog);
            }
            Ok(merged)
        })
    }

    fn identities(&self) -> anyhow::Result<Vec<DictionaryIdentity>> {
        self.paths
            .iter()
            .map(|path| {
                let metadata = std::fs::metadata(path)
                    .with_context(|| format!("stat DICT {}", path.display()))?;
                Ok(DictionaryIdentity {
                    path: path.clone(),
                    len: metadata.len(),
                    modified: metadata.modified()?,
                })
            })
            .collect()
    }

    fn load_if_changed(
        &self,
        load: impl FnOnce() -> anyhow::Result<UdaCatalog>,
    ) -> anyhow::Result<(u64, Arc<UdaCatalog>)> {
        // Serialize check + load + publication. An error leaves the previous
        // snapshot intact for existing readers, but never serves it as current.
        let mut slot = self
            .cached
            .lock()
            .map_err(|_| anyhow::anyhow!("UDA cache lock poisoned"))?;
        let before = self.identities()?;
        if let Some(cached) = slot.as_ref().filter(|cached| cached.identities == before) {
            return Ok((cached.epoch, cached.catalog.clone()));
        }
        let catalog = Arc::new(load()?);
        anyhow::ensure!(
            self.identities()? == before,
            "DICT changed while loading UDA definitions"
        );
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        before.hash(&mut hash);
        let epoch = hash.finish();
        *slot = Some(CachedCatalog {
            identities: before,
            catalog: catalog.clone(),
            epoch,
        });
        Ok((epoch, catalog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn provider(paths: Vec<PathBuf>) -> Arc<UdaCatalogProvider> {
        Arc::new(UdaCatalogProvider {
            paths,
            schema: Arc::new(DirectSchema::open_from_env().unwrap()),
            cached: Mutex::new(None),
        })
    }

    #[test]
    fn concurrent_first_read_loads_once_and_desi_changes_do_not_reload_dict() {
        let temp = tempfile::tempdir().unwrap();
        let dictionary = temp.path().join("dict");
        let design = temp.path().join("desi");
        std::fs::write(&dictionary, b"first dictionary").unwrap();
        std::fs::write(&design, b"first design").unwrap();
        let provider = provider(vec![dictionary.clone()]);
        let loads = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let provider = provider.clone();
                let loads = loads.clone();
                std::thread::spawn(move || {
                    provider
                        .load_if_changed(|| {
                            loads.fetch_add(1, Ordering::SeqCst);
                            Ok(UdaCatalog::default())
                        })
                        .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(
            results
                .iter()
                .all(|(_, catalog)| Arc::ptr_eq(catalog, &results[0].1))
        );
        std::fs::write(&design, b"a new design session").unwrap();
        let unchanged = provider
            .load_if_changed(|| panic!("DESI must not reload UDA"))
            .unwrap();
        assert!(Arc::ptr_eq(&unchanged.1, &results[0].1));
        std::fs::write(&dictionary, b"second, longer dictionary").unwrap();
        let changed = provider
            .load_if_changed(|| Ok(UdaCatalog::default()))
            .unwrap();
        assert_ne!(changed.0, unchanged.0);
        assert!(!Arc::ptr_eq(&changed.1, &unchanged.1));
        std::fs::write(&dictionary, b"first dictionary").unwrap();
        let restored = provider
            .load_if_changed(|| Ok(UdaCatalog::default()))
            .unwrap();
        assert!(!Arc::ptr_eq(&restored.1, &changed.1));
    }

    #[test]
    fn failed_or_changing_dictionary_is_not_published_or_returned_as_current() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("dict");
        std::fs::write(&path, b"original").unwrap();
        let provider = provider(vec![path.clone()]);
        let old = provider
            .load_if_changed(|| Ok(UdaCatalog::default()))
            .unwrap();
        std::fs::write(&path, b"changed dictionary").unwrap();
        assert!(
            provider
                .load_if_changed(|| anyhow::bail!("decode failed"))
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &provider.cached.lock().unwrap().as_ref().unwrap().catalog,
            &old.1
        ));
        assert!(
            provider
                .load_if_changed(|| {
                    std::fs::write(&path, b"changed again while reading").unwrap();
                    Ok(UdaCatalog::default())
                })
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &provider.cached.lock().unwrap().as_ref().unwrap().catalog,
            &old.1
        ));
        std::fs::remove_file(&path).unwrap();
        assert!(
            provider
                .load_if_changed(|| panic!("missing file must fail before load"))
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires local AMS source files; mutates only temporary DICT copies"]
    fn ams_dictionary_copy_reload_failure_and_restore() {
        let option = aios_core::get_db_option();
        let membership =
            super::super::mdb_membership::resolve(option, "AvevaMarineSample", "ALL").unwrap();
        let originals = membership.dictionary_paths();
        assert!(!originals.is_empty());
        let temp = tempfile::tempdir().unwrap();
        let copies: Vec<_> = originals
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let copy = temp.path().join(format!("dict-{index}"));
                std::fs::copy(source, &copy).unwrap();
                copy
            })
            .collect();
        let provider = provider(copies.clone());
        e3d_io::read_stats::reset();
        let original = provider.get().unwrap();
        let definitions = original.definitions().len();
        assert!(definitions > 0);
        let first_cost = e3d_io::read_stats::snapshot();
        let unchanged = provider.get().unwrap();
        assert!(Arc::ptr_eq(&original, &unchanged));
        assert_eq!(
            e3d_io::read_stats::snapshot().database_opens,
            first_cost.database_opens
        );
        std::fs::write(&copies[0], b"broken dictionary").unwrap();
        assert!(provider.get().is_err());
        assert_eq!(original.definitions().len(), definitions);
        std::fs::copy(&originals[0], &copies[0]).unwrap();
        // Force a distinct identity even on coarse timestamp filesystems.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&copies[0])
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        let restored = provider.get().unwrap();
        assert!(!Arc::ptr_eq(&original, &restored));
        assert_eq!(restored.definitions().len(), definitions);
        let cost = e3d_io::read_stats::snapshot();
        assert_eq!(cost.scan_calls, 0);
        eprintln!(
            "AMS DICT copies={} definitions={} original_preserved=true restored=true costs={}",
            copies.len(),
            definitions,
            cost.summary()
        );
    }
}
