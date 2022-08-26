pub mod pdms_arango;
pub mod pdms_inst_arango;
pub mod structs;
pub mod ssc_arango;

use serde::{Serialize,Deserialize};

#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct ForeignEdges{
    pub _from:String,
    pub _to:String,
    // 外键的种类, catr,gmre等
    pub foreign_type:String,
}