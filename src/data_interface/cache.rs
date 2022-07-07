use aios_core::pdms_types::{AttrMap, RefU64};
use dashmap::DashMap;
use lazy_static::lazy_static;

lazy_static!{
    pub static ref PDMS_ATT_MAP_CACHE: DashMap<RefU64, AttrMap>  = Default::default();
    pub static ref PDMS_IMPLICIT_ATT_MAP_CACHE: DashMap<RefU64, AttrMap>  = Default::default();
}