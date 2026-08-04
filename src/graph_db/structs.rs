use aios_core::pdms_types::RefU64;
use parry2d::simba::scalar::SupersetOf;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;
use serde_with::DisplayFromStr;
use serde_with::serde_as;
use std::collections::HashMap;
use std::fmt::format;
use std::str::FromStr;

///版本数据库里存储的索引值
#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(tag = "@type", rename = "PdmsElement")]
pub struct PdmsEleDataVersioned {
    #[serde(rename = "@id")]
    pub id: String,
    #[serde_as(as = "DisplayFromStr")]
    pub refno: RefU64,
    #[serde(serialize_with = "ser_refno_as_ref_type")]
    #[serde(skip_serializing_if = "is_zero")]
    // #[serde_as(as = "DisplayFromStr")]
    pub owner: RefU64,
    pub name: String,
    pub noun: String,
    // #[serde(default)]
    // pub order: u32,
    pub dbnum: i32,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cata_hash: Option<String>,
}

fn is_zero(refno: &RefU64) -> bool {
    refno.is_unset()
}

pub fn ser_refno_as_ref_type<S>(refno: &RefU64, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // let mut r = s.serialize_struct("PdmsElementData", 1)?;
    // r.serialize_field("@ref", &format!("PdmsElement/{}", refno.to_string()) )?;
    // r.end()
    s.serialize_str(&format!("PdmsElement/{}", refno.to_string()))
}

impl PdmsEleDataVersioned {
    //"@class": "PdmsElement",
    pub fn get_scheme() -> &'static str {
        r#"{ "@type" : "Class",
        "@id"   : "PdmsElement",
        "@key"  : { "@type": "Lexical", "@fields": ["refno"] },
        "refno"    : "xsd:string",
        "owner"    : {
            "@class": "PdmsElement",
            "@type": "Optional"
        },
        "name"    : "xsd:string",
        "noun"    : "xsd:string",
        "order"   :{
            "@class": "xsd:integer",
            "@type": "Optional"
        },
        "dbnum"    : "xsd:integer",
        "status_code": {
            "@class": "xsd:string",
            "@type": "Optional"
        },
        "cata_hash": {
            "@class": "xsd:string",
            "@type": "Optional"}
        }"#
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleData {
    #[serde_as(as = "DisplayFromStr")]
    #[serde(rename = "_key")]
    pub refno: RefU64,
    #[serde_as(as = "DisplayFromStr")]
    pub owner: RefU64,
    pub name: String,
    pub noun: String,
    #[serde(default)]
    pub order: u32,
    pub dbnum: i32,
    #[serde(default)]
    pub cata_hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleGraphEdge {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PdmsEleEdge {
    #[serde(rename = "_key")]
    pub key: String,
    #[serde(rename = "_from")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub refno: RefU64,
    #[serde(rename = "_to")]
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
    S: Serializer,
{
    s.serialize_str(format!("pdms_eles/{}", refno.to_string()).as_str())
}

pub fn de_refno_as_edge<'de, D>(deserializer: D) -> Result<RefU64, D::Error>
where
    D: Deserializer<'de>,
{
    if let Some(s) = String::deserialize(deserializer)?.split("/").skip(1).next() {
        Ok(RefU64::from_str(s).unwrap_or_default())
    } else {
        Ok(Default::default())
    }
}

#[serde_as]
//todo use custom serde
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PdmsMdbEdge {
    #[serde(rename = "_key")]
    pub key: String,
    #[serde(rename = "_from")]
    #[serde(deserialize_with = "de_refno_as_edge")]
    #[serde(serialize_with = "ser_refno_as_ele_edge")]
    pub mdb_refno: RefU64,
    #[serde(rename = "_to")]
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
