// pub mod pdms_arango;
pub mod structs;
// pub mod ssc_arango;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignEdges {
    pub _key: String,
    pub _from: String,
    pub _to: String,
    // 外键的种类, catr,gmre等
    pub foreign_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaDocument {
    pub _key: String,
    pub para: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataDocument {
    pub _key: String,
    pub dkey: String,
    pub ppro: String,
    pub dpro: String,
}