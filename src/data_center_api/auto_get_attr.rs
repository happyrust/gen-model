use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::*;
use anyhow::anyhow;
use std::collections::{BTreeMap, HashMap};
use aios_core::{AttrMap, AttrVal};

use crate::api::element::query_name;
use crate::aql_api::pdms_room::*;
use crate::consts::PUHUA_GY_MATERIAL_TABLE;
use crate::data_center_api::pipe::get_datacenter_bran_data;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use sqlx::types::Decimal;
use sqlx::{Executor, MySql, Pool, Row};

lazy_static! {
    pub static ref MATERIAL_MAP: DashMap<String, DashMap<String, String>> = {
        let mut map = DashMap::default();
        map
    };
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct DataCenterMetadataExcel {
    pub code: Option<String>,
    pub code_chinese_name: Option<String>,
    pub attr_code: Option<String>,
    pub attr_code_chinese_name: Option<String>,
    pub data_origin: Option<String>,
    pub function: Option<String>,
    pub att_type: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct DataCenterMetadata {
    pub code: String,
    pub attr_code: String,
    pub function: String,
    pub att_type: String,
}

/// 通过处理后的元数据表，根据用户填的function获取部分数据
pub async fn auto_get_datacenter_attr(
    refno: RefU64,
    aios_mgr: &AiosDBManager,
    attr: &AttrMap,
    metadata_excel_map: &HashMap<String, Vec<DataCenterMetadata>>,
) -> anyhow::Result<BTreeMap<String, DataCenterAttr>> {
    let mut map = BTreeMap::new();
    let att_type = attr.get_type();
    let Some(metadata) = metadata_excel_map.get(att_type) else {
        return Ok(map);
    };
    let values = auto_get_attr_from_metadata_excel(refno, &attr, metadata, aios_mgr).await;
    for value in values {
        map.entry(value.attribute_model_code.clone())
            .or_insert(value);
    }
    Ok(map)
}

/// 读取处理后的专业元数据表单,将可以自动获取数据的条目返回
///
/// 返回值：key : att_type  value: 三维提资需要的数据
pub(crate) fn read_data_center_metadata_excel(
    excel_path: &str,
) -> anyhow::Result<HashMap<String, Vec<DataCenterMetadata>>> {
    let mut map = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook(excel_path)?;
    let range = workbook
        .worksheet_range("对象类属性")
        .ok_or(anyhow::anyhow!("Cannot find Sheet '对象类属性'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: DataCenterMetadataExcel = result?;
        if v.att_type.is_none() || v.function.is_none() {
            continue;
        }
        let att_type = v.att_type.unwrap();
        let function = v.function.unwrap();
        let Some(code) = v.code else {
            continue;
        };
        let Some(attr_code) = v.attr_code else {
            continue;
        };
        map.entry(att_type.clone())
            .or_insert_with(Vec::new)
            .push(DataCenterMetadata {
                code,
                attr_code,
                function,
                att_type,
            });
    }
    Ok(map)
}

/// 根据处理后的元数据表单，将能自动获取数据的条目挑选出来，根据function字段,自动获取值
///
/// 返回值：DataCenterAttr:返回给数据中台的结构数据（部分)
pub(crate) async fn auto_get_attr_from_metadata_excel(
    refno: RefU64,
    attr: &AttrMap,
    metadata: &Vec<DataCenterMetadata>,
    aios_mgr: &AiosDBManager,
) -> Vec<DataCenterAttr> {
    let mut datacenter_attrs = Vec::new();
    let spre = attr.get_refu64("SPRE");
    // 找到大宗材料的编码
    let material = if let Some(spre) = spre {
        let spre_name = aios_mgr
            .get_name(spre)
            .await
            .unwrap_or(String::new())
            .split("/")
            .map(|x| x.to_string())
            .collect::<Vec<String>>();
        if spre_name.len() < 3 {
            None
        } else {
            if spre_name[2].contains(":") {
                let spre_name = spre_name[1].split(":").collect::<Vec<_>>();
                Some(spre_name[0].to_string())
            } else {
                None
            }
        }
    } else {
        None
    };
    // 找到所有要查询大宗材料表的字段
    let mut material_filed = Vec::new();
    for data in metadata {
        if !data.function.starts_with("material()") {
            continue;
        }
        let filed = data.function.split(".").collect::<Vec<_>>();
        if filed.len() < 2 {
            continue;
        }
        material_filed.push(filed[1].to_string());
    }
    // 查询该编码对应字段的数据
    let material_map = if material.is_some() && !material_filed.is_empty() {
        let material_code = material.unwrap();
        if let Ok(pool) = aios_mgr.get_puhua_pool().await {
            get_material_map_from_code(&material_code, material_filed, &pool).await
        } else {
            DashMap::new()
        }
    } else {
        DashMap::new()
    };
    // 自动获取数据
    for auto_data in metadata {
        let function = auto_data.function.split(".").collect::<Vec<_>>();
        // 长度为1，即单个特殊方法，并非从attr中直接获取得
        if function.len() == 1 {
            let value =
                auto_get_attr_from_metadata_excel_single_function(aios_mgr, refno, function[0])
                    .await
                    .unwrap_or("".to_string());
            datacenter_attrs.push(DataCenterAttr {
                attribute_model_code: auto_data.attr_code.to_string(),
                value,
            });
        } else {
            let attr_val =
                auto_get_attr_from_metadata_excel_attr_function(&function, attr, &material_map)
                    .unwrap_or(AttrVal::StringType(String::new()));
            datacenter_attrs.push(DataCenterAttr {
                attribute_model_code: auto_data.attr_code.to_string(),
                value: attr_val.get_val_as_string(),
            });
        }
    }
    datacenter_attrs
}

/// 自动获取数据中，不是从attr中获取的方法，例如 world_position room_code等
async fn auto_get_attr_from_metadata_excel_single_function(
    aios_mgr: &AiosDBManager,
    refno: RefU64,
    function: &str,
) -> Option<String> {
    match function {
        "world_position()" => {
            let Ok(Some(position)) = aios_mgr.get_world_transform(refno).await else {
                return None;
            };
            Some(
                serde_json::to_string(&position.translation).unwrap_or("[0.0,0.0,0.0]".to_string()),
            )
        }
        "orientation()" => {
            let Ok(Some(position)) = aios_mgr.get_world_transform(refno).await else {
                return None;
            };
            Some(serde_json::to_string(&position.rotation).unwrap_or("[0.0,0.0,0.0]".to_string()))
        }
        "room_code()" => {
            let Ok(database) = aios_mgr.get_arango_db().await else {
                return None;
            };
            let Ok(room_code) = query_room_name_from_refno_aql(refno, &database).await else {
                return None;
            };
            room_code
        }
        _ => None,
    }
}

/// 自动获取数据,通过function一层一层获取需要得数据
fn auto_get_attr_from_metadata_excel_attr_function(
    function: &Vec<&str>,
    attr: &AttrMap,
    material_map: &DashMap<String, String>,
) -> Option<AttrVal> {
    if function.len() < 2 {
        return None;
    }
    match function[0] {
        "attr()" => {
            // 获取attr的某个属性
            let attr_value = function[1].to_uppercase();
            attr.get_val(&attr_value).cloned()
        }
        "material()" => {
            let material_value = function[1];
            if let Some(value) = material_map.get(material_value) {
                Some(AttrVal::StringType(value.value().to_string()))
            } else {
                None
            }
        }
        "String" => match function[1] {
            "default()" => Some(AttrVal::StringType(String::default())),
            _ => None,
        },
        _ => None,
    }
}

/// 通过材料编码获取大宗材料表的数据
pub(crate) async fn get_material_map_from_code(
    code: &str,
    mut fields: Vec<String>,
    puhua_pool: &Pool<MySql>,
) -> DashMap<String, String> {
    if code.is_empty() || fields.is_empty() {
        return DashMap::default();
    }
    let mut query_map = DashMap::new();
    // 取了对应编码的数据， 但是不含某个字段的情况
    let mut cache_not_contains_filed = Vec::new();
    if let Some(map) = MATERIAL_MAP.get(code) {
        for filed in &fields {
            if !map.value().contains_key(filed) {
                cache_not_contains_filed.push(filed.to_string());
            } else {
                query_map
                    .entry(filed.to_string())
                    .or_insert(map.value().get(filed).unwrap().value().to_string());
            }
        }
        if cache_not_contains_filed.is_empty() {
            return map.value().clone();
        }
    }
    let mut query_fields = if MATERIAL_MAP.contains_key(code) {
        cache_not_contains_filed
    } else {
        fields
    };
    query_fields.push("Pressure".to_string());
    // 查询普华的材料表
    let sql = gen_query_gy_material_sql(code, &query_fields);
    if let Ok(mut puhua_conn) = puhua_pool.acquire().await {
        let Ok(query_results) = puhua_conn.fetch_all(sql.as_str()).await else {
            return DashMap::new();
        };
        for query_result in query_results {
            for filed in &query_fields {
                if filed == "Weight" {
                    let mut data = query_result
                        .try_get::<Decimal, _>(filed.as_str())
                        .unwrap_or(Decimal::new(0, 0));
                    if let Ok(_) = data.set_scale(6) {
                        query_map
                            .entry(filed.to_string())
                            .or_insert(data.to_string());
                    } else {
                        query_map
                            .entry(filed.to_string())
                            .or_insert("0.0".to_string());
                    }
                } else {
                    let data = query_result
                        .try_get::<String, _>(filed.as_str())
                        .unwrap_or("".to_string());
                    query_map.entry(filed.to_string()).or_insert(data);
                }
            }
        }
    }
    // 取出缓存中的数据
    if let Some(cache_value) = MATERIAL_MAP.get(code) {
        for cache_v in cache_value.value() {
            query_map
                .entry(cache_v.key().to_string())
                .or_insert(cache_v.value().to_string());
        }
    }
    // 将查询到的数据放到集合中
    for query_value in &query_map {
        MATERIAL_MAP
            .entry(code.to_string())
            .or_insert_with(DashMap::new)
            .entry(query_value.key().clone())
            .or_insert(query_value.value().to_string());
    }
    query_map
}

/// 获取多个编码对应的大宗材料属性
pub async fn get_material_map_from_codes(
    codes: Vec<String>,
    mut filedes: Vec<String>,
    puhua_pool: &Pool<MySql>,
) -> DashMap<String, DashMap<String, String>> {
    if codes.is_empty() || filedes.is_empty() {
        return DashMap::default();
    }
    // 取了对应编码的数据， 但是不含某个字段的情况
    let mut not_contains_filed = HashMap::new();
    for code in &codes {
        if let Some(map) = MATERIAL_MAP.get(code) {
            for filed in &filedes {
                if !map.value().contains_key(filed) {
                    not_contains_filed
                        .entry(code.clone())
                        .or_insert_with(Vec::new)
                        .push(filed.to_string());
                }
            }
        } else {
            not_contains_filed
                .entry(code.clone())
                .or_insert(filedes.clone());
        }
    }
    // 查询普华的材料表
    let mut query_map: DashMap<String, DashMap<String, String>> = DashMap::new();
    let keys = not_contains_filed
        .keys()
        .into_iter()
        .map(|x| x.to_string())
        .collect::<Vec<String>>();
    if !keys.is_empty() {
        let sql = gen_query_gy_materials_sql(keys, &filedes);
        if let Ok(mut puhua_conn) = puhua_pool.acquire().await {
            let Ok(query_results) = puhua_conn.fetch_all(sql.as_str()).await else {
                return DashMap::new();
            };
            for query_result in query_results {
                let Ok(code) = query_result.try_get::<String, _>("Code") else {
                    continue;
                };
                for filed in &filedes {
                    // Weight是Decimal类型，单独做处理
                    if filed == "Weight" {
                        let mut data = query_result
                            .try_get::<Decimal, _>(filed.as_str())
                            .unwrap_or(Decimal::new(0, 0));
                        if let Ok(_) = data.set_scale(6) {
                            query_map
                                .entry(code.to_string())
                                .or_insert_with(DashMap::new)
                                .insert(filed.clone(), data.to_string());
                        } else {
                            query_map
                                .entry(code.to_string())
                                .or_insert_with(DashMap::new)
                                .insert(filed.clone(), "0.0".to_string());
                        }
                    } else {
                        let data = query_result
                            .try_get::<String, _>(filed.as_str())
                            .unwrap_or("".to_string());
                        query_map
                            .entry(code.to_string())
                            .or_insert_with(DashMap::new)
                            .insert(filed.clone(), data.to_string());
                    }
                }
            }
        }
    }
    // 取出缓存中的数据
    for code in &codes {
        if let Some(map) = MATERIAL_MAP.get(code) {
            for v in map.value() {
                query_map
                    .entry(code.clone())
                    .or_insert_with(DashMap::new)
                    .entry(v.key().clone())
                    .or_insert(v.value().clone());
            }
        }
    }
    // 将查询的数据放到缓存中
    for query_value in &query_map {
        let value = query_value.value();
        for v in value {
            MATERIAL_MAP
                .entry(query_value.key().clone())
                .or_insert_with(DashMap::new)
                .entry(v.key().to_string())
                .or_insert(v.value().clone());
        }
    }
    query_map
}

fn gen_query_gy_materials_sql(codes: Vec<String>, fileds: &Vec<String>) -> String {
    let mut sql = String::from("SELECT ");
    for filed in fileds {
        sql.push_str(format!("{} ,", filed).as_str());
    }
    sql.remove(sql.len() - 1);
    sql.push_str(format!(" FROM {} WHERE Code  in (", PUHUA_GY_MATERIAL_TABLE,).as_str());
    for code in codes {
        sql.push_str(format!("'{}',", code).as_str());
    }
    sql.remove(sql.len() - 1);
    sql.push_str(")");
    sql
}

fn gen_query_gy_material_sql(code: &str, fileds: &Vec<String>) -> String {
    let mut sql = String::from("SELECT ");
    for filed in fileds {
        sql.push_str(format!("{} ,", filed).as_str());
    }
    sql.remove(sql.len() - 1);
    sql.push_str(format!(" FROM {} WHERE Code = '{}'", PUHUA_GY_MATERIAL_TABLE, code).as_str());
    sql
}

#[tokio::test]
async fn test_read_data_center_metadata_excel() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let path = "./resource/附录I-工艺布置管件类元数据.xlsx";
    let result = read_data_center_metadata_excel(path).unwrap();
    let refno = RefU64::from_refno_str("24383/66509")?;
    get_datacenter_bran_data(&aios_mgr, refno, &result).await?;
    Ok(())
}
