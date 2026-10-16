use std::sync::{Arc, Mutex};

use anyhow::Context;

use super::direct_store::{DbPin, DirectSchema, DirectStore};
use std::collections::BTreeMap;
use std::path::PathBuf;
type RouteBindings = BTreeMap<u32, (String, String, Option<PathBuf>)>;
use super::mdb_membership;
use super::mdb_route_directory::MdbRouteDirectory;

/// Process-local lazy owner of the one direct read store for an MDB.
/// The store is replaced only when the route fingerprint changes, so a file
/// update invalidates that dbnum while unrelated sessions remain frozen.
#[derive(Clone)]
pub struct DirectStoreRegistry {
    project: Arc<str>,
    mdb: Arc<str>,
    state: Arc<Mutex<Option<(super::mdb_route_directory::RouteFingerprint, RouteBindings, Arc<DirectStore>)>>>,
}

impl DirectStoreRegistry {
    pub fn new(project: impl Into<Arc<str>>, mdb: impl Into<Arc<str>>) -> Self {
        Self { project: project.into(), mdb: mdb.into(), state: Arc::new(Mutex::new(None)) }
    }

    pub fn get_blocking(&self) -> anyhow::Result<Arc<DirectStore>> {
        let option = aios_core::get_db_option();
        let membership = Arc::new(mdb_membership::resolve(option, &self.project, &self.mdb)?);
        let directory = MdbRouteDirectory::from_membership(membership);
        let fingerprint = directory.fingerprint();
        let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("direct store registry lock poisoned"))?;
        let routes: RouteBindings = directory.element_databases().map(|r|
            (r.dbnum, (r.project.clone(), r.db_type.clone(), r.path.clone()))).collect();
        if let Some((known, known_routes, store)) = state.as_ref() {
            if *known == fingerprint && *known_routes == routes { return Ok(store.clone()); }
            let old_versions: BTreeMap<_, _> = known.iter().map(|(n,l,t,id)| (*n,(*l,*t,id))).collect();
            let new_versions: BTreeMap<_, _> = fingerprint.iter().map(|(n,l,t,id)| (*n,(*l,*t,id))).collect();
            for dbnum in known_routes.keys().chain(routes.keys()).copied().collect::<std::collections::BTreeSet<_>>() {
                if known_routes.get(&dbnum) != routes.get(&dbnum) || old_versions.get(&dbnum) != new_versions.get(&dbnum) {
                    let expected = store.route_generation(dbnum as i32);
                    let pin = routes.get(&dbnum).and_then(|(_, kind, path)| path.as_ref().map(|file| DbPin {
                        dbnum: dbnum as i32, db_type: kind.clone(), file: file.clone(), sesno: None,
                    }));
                    anyhow::ensure!(
                        store.refresh_route_if_generation(dbnum as i32, expected, pin),
                        "dbnum {dbnum} changed while refreshing its route; retry directory reconciliation"
                    );
                }
            }
            let store = store.clone();
            *state = Some((fingerprint, routes, store.clone()));
            return Ok(store);
        }
        let schema = Arc::new(DirectSchema::open_from_env().context("open shared direct schema")?);
        let store = Arc::new(DirectStore::new(schema, Arc::new(directory)));
        *state = Some((fingerprint, routes, store.clone()));
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
