use aios_core::AttrMap;
use aios_core::RefU64Vec;
use aios_core::pdms_data::*;
use aios_core::pdms_types::*;
use dashmap::DashMap;
use derive_more::{Deref, DerefMut};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

lazy_static! {
    pub static ref CACHED_MDB_SITE_MAP: RwLock<HashMap<RefU64, PdmsElementVec>> =
        RwLock::new(HashMap::new());
}

#[derive(Serialize, Deserialize, Deref, DerefMut, Clone, Default, Eq, Hash, PartialEq)]
pub struct RString(pub String);

impl AsRef<str> for RString {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl From<String> for RString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

// impl Into<sled::IVec> for RString {
//     fn into(self) -> sled::IVec {
//         bincode::serialize(&self).unwrap().into()
//     }
// }
//
// impl Into<sled::IVec> for &RString {
//     fn into(self) -> sled::IVec {
//         bincode::serialize(self).unwrap().into()
//     }
// }
//
// impl From<sled::IVec> for RString {
//     fn from(d: sled::IVec) -> Self {
//         bincode::deserialize(&d).unwrap()
//     }
// }
