use aios_core::pdms_types::{AttrMap, PdmsElement, RefU64};
use serde::{Serialize, Deserialize};

pub mod children;
pub mod ssc_children;
pub mod query_transform;
pub mod foreign_refnos;
pub mod plin_attr;
pub mod para_value;
pub mod dtse_attr;
pub mod pdms_mesh;
pub mod pdms_room;
pub mod tubi;
pub mod virtual_hole_value;

/// 存放在图数据库的attr
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PdmsPLINAttrAql {
    // 参考号的 url 形式
    pub _key: String,
    pub attr: AttrMap,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PdmsRefnoNameAql {
    pub refno: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct PdmsRefnoTypeAql {
    pub refno: String,
    pub noun: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct PdmsElementAql {
    pub refno: String,
    pub owner: String,
    pub name: String,
    pub noun: String,
    pub version: u32,
    pub children_count: usize,
}

impl PdmsElementAql {
    pub fn change_to_pdms_element(self) -> Option<PdmsElement> {
        if let Some(refno) = RefU64::from_url_refno(&self.refno) {
            if RefU64::from_url_refno(&self.owner).is_none() { return None; }
            return Some(PdmsElement {
                refno: refno.to_refno_string(),
                owner: RefU64::from_url_refno(&self.owner).unwrap(),
                name: self.name,
                noun: self.noun,
                version: self.version,
                children_count: self.children_count,
            });
        }
        None
    }
}

/// todo 需要放到 RefU64的 成员方法中
pub fn convert_refno_vec_from_vec_string(string_vec: Vec<String>) -> Vec<RefU64> {
    let mut result = vec![];
    for v in string_vec {
        if let Some(refno) = RefU64::from_url_refno(&v) {
            result.push(refno);
        }
    }
    result
}

pub fn change_vec_refnos_into_vec_string(refnos: Vec<RefU64>) -> Vec<String> {
    let mut children = vec![];
    refnos.into_iter().for_each(|refno| {
        children.push(RefU64::to_url_refno(&refno))
    });
    children
}