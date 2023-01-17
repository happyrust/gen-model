use serde::{Serialize, Deserialize};
use derive_more::{Deref, DerefMut};
use lazy_static::lazy_static;
use aios_core::cache::mgr::CacheMgr;
use aios_core::pdms_types::*;
use aios_core::pdms_data::*;
use aios_core::cache::refno::CachedRefBasic;
use dashmap::DashMap;

lazy_static! {
    pub static ref PDMS_ATT_MAP_CACHE: CacheMgr<AttrMap>  = CacheMgr::new("ATTR_MAP_CACHE", true);
    pub static ref PDMS_ANCESTOR_CACHE: CacheMgr<RefU64Vec>  = CacheMgr::new("ANCESTOR_CACHE",  true);
    pub static ref CACHED_REFNO_BASIC_MAP: CacheMgr<CachedRefBasic>  = CacheMgr::new("REFNO_BASIC_CACHE",  false);
    pub static ref CACHED_MDB_SITE_MAP: CacheMgr<PdmsElementVec>  = CacheMgr::new("MDB_SITE_CACHE", false);
    pub static ref CACHED_SCOM_INFO_MAP: CacheMgr<ScomInfo>  = CacheMgr::new("SCOM_INFO_CACHE",  true);
}


#[derive(Serialize, Deserialize, Deref, DerefMut, Clone, Default, Eq, Hash, PartialEq)]
pub struct AiosString(pub String);

impl Into<sled::IVec> for AiosString {
    fn into(self) -> sled::IVec {
        bincode::serialize(&self).unwrap().into()
    }
}

impl Into<sled::IVec> for &AiosString {
    fn into(self) -> sled::IVec {
        bincode::serialize(self).unwrap().into()
    }
}

impl From<sled::IVec> for AiosString {
    fn from(d: sled::IVec) -> Self {
        bincode::deserialize(&d).unwrap()
    }
}