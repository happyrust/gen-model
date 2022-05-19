use aios_core::pdms_types::{AiosStr, RefU64};
use serde::{Serialize,Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EleNodeTIDB {
    pub refno: RefU64,
    pub owner: RefU64,
    // pub name_hash: AiosStrHash,
    pub name: AiosStr,
    pub noun: AiosStr,
    pub version: u32,
    // pub children_count: usize,
    pub children_count: usize,
}