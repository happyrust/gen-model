use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphNode {
    pub _key: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub version: u32,
    pub dbnum: i32,
    // pub attr_key: String,
    // pub children_count: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphEdge {
    pub _from: String,
    pub _to: String,
}