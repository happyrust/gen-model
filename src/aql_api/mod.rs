use aios_core::AttrMap;
use aios_core::pdms_types::*;
use serde::{Serialize, Deserialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use std::str::FromStr;

pub mod attr_map;
pub mod children;
pub mod ssc_children;
pub mod query_transform;
pub mod foreign_refnos;
pub mod plin_attr;
pub mod para_value;
pub mod dtse_attr;
pub mod pdms_mesh;
pub mod pdms_room;
pub mod pdms_element;
pub mod tubi;
// pub mod hole;
// pub mod virtual_hole;
pub mod atta_pos;
pub mod lock_refnos;
pub mod vague_search;
pub mod threed_review;

/// 存放在图数据库的attr
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PdmsPLINAttrAql {
    // 参考号的 url 形式
    pub _key: String,
    pub attr: AttrMap,
}

#[derive(Debug, Default,Clone, Serialize, Deserialize)]
pub struct PdmsRefnoNameAql {
    pub refno: String,
    pub name: String,
}

#[derive(Debug, Default,Clone, Serialize, Deserialize)]
pub struct PdmsRoomNameAql {
    pub refno: String,
    pub room_name: String,
    #[serde(default)]
    pub b_rs: bool,
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PdmsOwnerNameAql {
    #[serde_as(as = "DisplayFromStr")]
    pub refno: RefU64,
    #[serde(default)]
    pub name: String,
    #[serde_as(as = "DisplayFromStr")]
    pub owner: RefU64,
    pub owner_noun: String,
    pub owner_name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PdmsSpreNameAql {
    pub refno: String,
    pub foreign_refno: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PdmsRefnoTypeAql {
    pub refno: String,
    pub noun: String,
}

#[inline]
pub fn convert_refno_vec_from_vec_string(string_vec: Vec<String>) -> Vec<RefU64> {
    let mut result = vec![];
    for v in string_vec {
        if let Ok(refno) = RefU64::from_str(&v) {
            result.push(refno);
        }
    }
    result
}

pub fn change_vec_refnos_into_vec_string(refnos: Vec<RefU64>) -> Vec<String> {
    let mut children = vec![];
    refnos.into_iter().for_each(|refno| {
        children.push(refno.to_string())
    });
    children
}