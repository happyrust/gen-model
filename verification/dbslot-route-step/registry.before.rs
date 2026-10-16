use std::sync::{Arc, Mutex};

use anyhow::Context;

use super::direct_store::{DirectSchema, DirectStore};
use super::mdb_membership;
use super::mdb_route_directory::MdbRouteDirectory;

/// Process-local lazy owner of the one direct read store for an MDB.
/// The store is replaced only when the route fingerprint changes, so a file
/// update invalidates that dbnum while unrelated sessions remain frozen.
#[derive(Clone)]
pub struct DirectStoreRegistry {
    project: Arc<str>,
    mdb: Arc<str>,
    state: Arc<Mutex<Option<(Vec<(u32, u64, u128)>, Vec<u32>, Arc<DirectStore>)>>>,
}

impl DirectStoreRegistry {
    pub fn new(project: impl Into<Arc<str>>, mdb: impl Into<Arc<str>>) -> Self {
        Self { project: project.into(), mdb: mdb.into(), state: Arc::new(Mutex::new(None)) }
    }

    pub fn get_blocking(&self) -> anyhow::Result<Arc<DirectStore>> {
        let option = aios_core::get_db_option();
        let membership = mdb_membership::get(&self.project, &self.mdb)
            .map(Ok)
            .unwrap_or_else(|| mdb_membership::resolve(option, &self.project, &self.mdb).map(Arc::new))?;
        let directory = MdbRouteDirectory::from_membership(membership);
        let fingerprint = directory.fingerprint();
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("direct store registry lock poisoned"))?;
        let dbnums: Vec<u32> = directory.element_databases().map(|route| route.dbnum).collect();
        if let Some((known, known_dbnums, store)) = state.as_ref().map(|(a,b,c)| (a.clone(), b.clone(), c.clone())) {
            if known == fingerprint { return Ok(store); }
            if known_dbnums == dbnums {
                for ((old_dbnum, old_len, old_mtime), (new_dbnum, new_len, new_mtime)) in known.iter().zip(fingerprint.iter()) {
                    if old_dbnum == new_dbnum && (old_len, old_mtime) != (new_len, new_mtime) {
                        store.invalidate_dbnum(*new_dbnum as i32);
                    }
                }
                *state = Some((fingerprint, dbnums, store.clone()));
                return Ok(store);
            }
        }
        let schema = Arc::new(DirectSchema::open_from_env().context("open shared direct schema")?);
        let store = Arc::new(DirectStore::new(schema, Arc::new(directory)));
        *state = Some((fingerprint, dbnums, store.clone()));
        Ok(store)
    }

    #[cfg(test)]
    pub fn is_cold(&self) -> bool { self.state.lock().unwrap().is_none() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_lazy_and_clone_shares_the_empty_state() {
        let first = DirectStoreRegistry::new(Arc::<str>::from("AMS"), Arc::<str>::from("/ALL"));
        let second = first.clone();
        assert!(first.is_cold());
        assert!(second.is_cold());
    }
}
