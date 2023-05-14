use std::cell::Ref;
use std::collections::{BTreeMap, HashMap};
use std::default::default;
use aios_core::parsed_data::CateAxisParam;
use aios_core::pdms_types::RefU64;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_with::{DisplayFromStr, serde_as};
use derive_more::{Deref, DerefMut};
use dashmap::DashMap;
use aios_core::prim_geo::category::CateBrepShape;

pub type AIOSAxisMap = BTreeMap<i32, CateAxisParam>;


///有负实体的集合信息, 返回tuple
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RefnoHasNegPosInfoTuple(
    #[serde_as(as = "DisplayFromStr")]
    pub RefU64,
    //positive
    #[serde(deserialize_with = "de_refno_from_vec_str")]
    pub Vec<RefU64>,
    //negative
    #[serde(deserialize_with = "de_refno_from_vec_str")]
    pub Vec<RefU64>,
);

// #[derive(Debug, Serialize, Deserialize, Default)]
// pub struct RefnoHasNegPosInfo {
//     #[serde(deserialize_with = "de_refno_from_vec_str")]
//     pub children: Vec<RefU64>,
//     pub nouns: Vec<String>,
//     pub cur_noun: String,
// }

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RefnoHasNegInfoTuple(
    #[serde_as(as = "DisplayFromStr")]
    pub RefU64,
    #[serde(deserialize_with = "de_refno_from_vec_str")]
    pub Vec<RefU64>,
);

///有负实体的集合信息
// #[derive(Debug, Serialize, Deserialize, Default)]
// pub struct RefnoHasNegInfo {
//     #[serde(deserialize_with = "de_refno_from_vec_str")]
//     pub children: Vec<RefU64>,
// }


fn de_refno_from_vec_str<'de, D>(deserializer: D) -> Result<Vec<RefU64>, D::Error>
    where D: Deserializer<'de> {
    let s = Vec::<String>::deserialize(deserializer)?;
    Ok(s.iter().map(|x| RefU64::from_url_refno(x).unwrap()).collect())
}

// #[derive(Debug, Serialize, Deserialize, Default)]
pub struct CateBrepShapeData{
    // pub scom_refno: RefU64,
    pub gmse_refno: RefU64,
    pub shapes: Vec<CateBrepShape>,
}

///元件库的几何Map，键值为 ele refno, 值为 (scom_refno, shapes)
pub type CateBrepShapeMap = DashMap<RefU64, Vec<CateBrepShape>>;
