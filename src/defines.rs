use serde::{Serialize, Deserialize};
use derive_more::{Deref, DerefMut};
use lazy_static::lazy_static;
use aios_core::cache::mgr::{BytesTrait, CacheMgr};
use aios_core::pdms_types::*;
use aios_core::pdms_data::*;
use aios_core::cache::refno::CachedRefBasic;
use dashmap::DashMap;
use aios_core::AttrMap;
use tokio::sync::RwLock;
use std::collections::HashMap;
use aios_core::RefU64Vec;

lazy_static! {
    pub static ref PDMS_ATT_MAP_CACHE: CacheMgr<NamedAttrMap>  = CacheMgr::new("ATTR_MAP_CACHE", false);
    pub static ref PDMS_ANCESTOR_CACHE: CacheMgr<RefU64Vec>  = CacheMgr::new("ANCESTOR_CACHE",  false);
    pub static ref CACHED_REFNO_BASIC_MAP: CacheMgr<CachedRefBasic>  = CacheMgr::new("REFNO_BASIC_CACHE",  false);
    pub static ref CACHED_MDB_SITE_MAP : RwLock<HashMap<RefU64, PdmsElementVec>> = RwLock::new(HashMap::new());
    pub static ref CACHED_PLIN_MAP: CacheMgr<RString>  = CacheMgr::new("PLIN_CACHE", false);
    pub static ref CACHED_SCOM_INFO_MAP: CacheMgr<ScomInfo>  = CacheMgr::new("SCOM_INFO_CACHE",  true);
}




#[derive(Serialize, Deserialize, Deref, DerefMut, Clone, Default, Eq, Hash, PartialEq)]
pub struct RString(pub String);

impl AsRef<str> for RString {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl BytesTrait for RString {
}

impl From<String> for RString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Into<sled::IVec> for RString {
    fn into(self) -> sled::IVec {
        bincode::serialize(&self).unwrap().into()
    }
}

impl Into<sled::IVec> for &RString {
    fn into(self) -> sled::IVec {
        bincode::serialize(self).unwrap().into()
    }
}

impl From<sled::IVec> for RString {
    fn from(d: sled::IVec) -> Self {
        bincode::deserialize(&d).unwrap()
    }
}