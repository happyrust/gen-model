use aios_core::parsed_data::CateAxisParam;
use aios_core::pdms_types::RefU64;
use aios_core::prim_geo::category::CateCsgShape;
use dashmap::DashMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DisplayFromStr, serde_as};
use std::collections::BTreeMap;
use std::str::FromStr;

pub type PlantAxisMap = BTreeMap<i32, CateAxisParam>;

///有负实体的集合信息, 返回tuple
#[serde_as]
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RefnoHasNegPosInfoTuple(
    #[serde_as(as = "DisplayFromStr")] pub RefU64,
    //positive
    #[serde(deserialize_with = "de_refno_from_vec_str")] pub Vec<RefU64>,
    //negative
    #[serde(deserialize_with = "de_refno_from_vec_str")] pub Vec<RefU64>,
);

#[serde_as]
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RefnoHasNegInfoTuple(
    #[serde_as(as = "DisplayFromStr")] pub RefU64,
    #[serde(deserialize_with = "de_refno_from_vec_str")] pub Vec<RefU64>,
);

///有负实体的集合信息
// #[derive(Debug, Serialize, Deserialize, Default)]
// pub struct RefnoHasNegInfo {
//     #[serde(deserialize_with = "de_refno_from_vec_str")]
//     pub children: Vec<RefU64>,
// }

fn de_refno_from_vec_str<'de, D>(deserializer: D) -> Result<Vec<RefU64>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = Vec::<String>::deserialize(deserializer)?;
    Ok(s.iter().map(|x| RefU64::from_str(x).unwrap()).collect())
}

// #[derive(Debug, Serialize, Deserialize, Default)]
pub struct CateCsgShapeData {
    pub gmse_refno: RefU64,
    pub shapes: Vec<CateCsgShape>,
}
