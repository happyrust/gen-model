use aios_core::pdms_types::RefU64;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use std::str::FromStr;

///图数据库里存储的索引值
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphNode {
    #[serde_as(as = "DisplayFromStr")]
    #[serde(rename = "_key")]
    pub refno: RefU64,
    #[serde_as(as = "DisplayFromStr")]
    pub owner: RefU64,
    pub name: String,
    pub noun: String,
    pub order: u32,
    pub dbnum: i32,
    #[serde(default)]
    pub cata_hash: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphEdge {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleEdge {
    #[serde(rename="_key")]
    pub key: String,
    #[serde(rename="_from")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub refno: RefU64,
    #[serde(rename="_to")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub owner: RefU64,
    #[serde(default)]
    pub order: u32,
    //只有world->site部分需要加这个属性
    #[serde(default)]
    pub mdb_name: Option<String>,
    //只有world->site部分需要加这个属性
    #[serde(default)]
    pub db_type: Option<String>,
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
    pub _key: String,
    pub _from: String,
    pub _to: String,
}


pub fn ser_refno_as_ele_edge<S>(refno: &RefU64, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer{
    s.serialize_str(format!("pdms_eles/{}", refno.to_string()).as_str())
}

pub fn de_refno_as_edge<'de, D>(deserializer: D) -> Result<RefU64, D::Error>
    where
        D: Deserializer<'de>{
    if let Some(s) = String::deserialize(deserializer)?.split("/").skip(1).next(){
        Ok(RefU64::from_str(s).unwrap_or_default())
    }else{
        Ok(Default::default())
    }
}

#[serde_as]
//todo use custom serde
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PdmsMdbEdge {
    #[serde(rename="_key")]
    pub key: String,
    #[serde(rename="_from")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub mdb_refno: RefU64,
    #[serde(rename="_to")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub world_refno: RefU64,
    pub name: String,
    pub order: u32,
    pub db_num: u32,
    #[serde_as(as = "DisplayFromStr")]
    pub db_refno: RefU64,
    pub db_type: String,
}
