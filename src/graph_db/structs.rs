use aios_core::pdms_types::RefU64;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphNode {
    pub _key: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub version: u32,
    pub dbnum: i32,
    // pub has_instance: String,
    // pub attr_key: String,
    // pub children_count: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct VirtualHoleGraphNode {
    pub _key: String,
    pub intelld: i32,
    pub code: String,
    pub relyitem: String,
    pub mainitem: String,
    pub speciality: String,
    pub position: String,
    pub holework: String,
    pub workby: String,
    pub time: String,
    pub shape: String,
    pub ori: String,
    pub itemref: String,

    pub mainitemref: String,
    pub openitem: String,
    pub plugtype: String,
    pub sizeheigh: f32,
    pub sizewidth: f32,
    pub bankwidth: f32,
    pub bankheight: f32,
    pub hotdis: String,
    pub heatthick: f32,
    pub refno: String,
    pub fittrefno: String,
    pub subsmeterial: String,
    pub substhickness: f32,
    pub icreate: i32,
    pub substype: String,
    pub extentlength1: f32,
    pub extentlength2: f32,
    pub second: i32,
    pub rehole: i32,
    pub note: String,
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