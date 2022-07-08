use aios_core::pdms_types::{AttrMap, RefU64};
use dashmap::DashMap;
use dashmap::mapref::one::Ref;
use lazy_static::lazy_static;
use sled::IVec;
use crate::CACHE_SLED_NAME;
use crate::data_interface::defines::CachedRefBasic;
use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;
use aios_core::pdms_types::*;
use crate::defines::AiosString;



lazy_static! {
    pub static ref CACHE_DB: sled::Db  = {
       sled::open(CACHE_SLED_NAME).unwrap()
    };
    pub static ref PDMS_ATT_MAP_CACHE: CacheMgr< AttrMap>  = CacheMgr::new("ATTR_MAP_CACHE");
    pub static ref PDMS_IMPLICIT_ATT_MAP_CACHE: CacheMgr< AttrMap>  = CacheMgr::new("IMPLICIT_ATTR_MAP_CACHE");

    pub static ref CACHED_REFNO_BASIC_MAP: CacheMgr< CachedRefBasic>  = CacheMgr::new("REFNO_BASIC_CACHE");
    pub static ref CACHED_MDB_SITE_MAP: CacheMgr< PdmsElementVec>  = CacheMgr::new("MDB_SITE_CACHE");
}

#[derive(Clone)]
pub struct CacheMgr<
    T: Into<IVec> + From<IVec> + Clone + Serialize + DeserializeOwned> {
    name: String,
    tree: sled::Tree,
    map: DashMap<RefU64, T>,
}

impl<T: Into<IVec> + From<IVec> + Clone + Serialize + DeserializeOwned> CacheMgr<T>
{
    pub fn new(name: &str) -> Self {
        let tree = CACHE_DB.open_tree(name).unwrap();
        Self {
            name: name.to_string(),
            tree,
            map: Default::default(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[inline]
    pub fn get(&self, k: &RefU64) -> Option<Ref<RefU64, T>> {
        if !self.map.contains_key(k) {
            if let Ok(Some(bytes)) = self.tree.get::<IVec>(k.into()) {
                self.map.insert((*k).into(), bytes.into());
            }
        }
        self.map.get(k)
    }

    #[inline]
    pub fn load_all(&self) {
        for k in self.tree.iter() {
            if let Ok((key, value)) = k {
                self.map.insert(key.into(), value.into());
            }
        }
    }

    #[inline]
    pub fn insert(&self, k: RefU64, value: T) {
        self.map.insert(k, value.clone());
        let bytes: IVec = k.into();
        self.tree.insert(bytes, value);
    }

    #[inline]
    pub fn contains_key(&self, k: &RefU64) -> bool {
        self.map.contains_key(k)
    }
}