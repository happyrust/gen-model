use aios_core::pdms_types::RefU64;
use serde::{Serialize, Deserialize};

///图数据库里存储的索引值
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphNode {
    pub _key: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub version: u32,
    pub dbnum: i32,
    pub cata_hash: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphEdge {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphEdgeWithKey {
    pub _key: String,
    // from -> to
    pub _from: String,
    pub _to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SSCEleGraphNode {
    pub _key: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub real_pdms_refno: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsInstanceGraphEdge {
    pub _from: String,
    pub _to: String,
}