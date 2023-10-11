use std::fs::File;
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_types::RefU64;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use dashmap::DashMap;
use dashmap::mapref::one::Ref;
use crate::api::element::query_name;
use crate::api::room_code::query_room_code;
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::aql_api::tubi::query_tubi_from_bran;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_ispec_from_attr, get_refno_latest_version, get_rtext_from_attr, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_data_center_tubi_attr(bran_refno: RefU64,bran_name:&str, database: &ArDatabase, aios_mgr: &AiosDBManager) -> Vec<DataCenterInstance> {
    let Ok(tubis) = query_tubi_from_bran(bran_refno, database).await else { return Vec::new(); };
    let mut instances = Vec::new();
    let bran_lstu_name = query_foreign_name_aql(bran_refno, vec!["HSTU", "HSTU"], database).await.unwrap_or(None).unwrap_or("".to_string());
    let bran_spre_material_code = get_spre_material_code(&bran_lstu_name).unwrap_or("".to_string());
    let need_query_material_code = vec![("ITEMA11".to_string(), "Code".to_string()),
                                        ("ITEMA12".to_string(), "Name".to_string()),
                                        ("ITEMA13".to_string(), "Make".to_string()),
                                        ("ITEMA14".to_string(), "Mat".to_string()),
                                        ("ITEMA15".to_string(), "MatSpec".to_string()),
                                        ("ITEMA16".to_string(), "Spec".to_string()),
                                        ("ITEMA17".to_string(), "RCCM".to_string()),
                                        ("ITEMA18".to_string(), "QAGrade".to_string()),
                                        ("ITEMAA2".to_string(), "Weight".to_string()),
                                        ("ITEMAA5".to_string(), "Diameter".to_string()),
                                        ("ITEMAA7".to_string(), "Link".to_string())];
    for (idx, tubi) in tubis.into_iter().enumerate() {
        let Some(from) = RefU64::from_arangodb_refno_str(&tubi._from) else { continue; };
        let Some(to) = RefU64::from_arangodb_refno_str(&tubi._to) else { continue; };
        let mut result = Vec::new();
        result.push(DataCenterAttr {
            attribute_model_code: "ITEM1".to_string(),
            value: AttrString(from.to_refno_string()).into(),
        });
        let item_1 = DataCenterAttr {
            attribute_model_code: "ITEMA1".to_string(),
            value: AttrString(format!("TUBI {}", idx + 1)).into(),
        };
        result.push(item_1);
        let item_2 = DataCenterAttr {
            attribute_model_code: "ITEMA2".to_string(),
            value: AttrString("TUBI".to_string()).into(),
        };
        result.push(item_2);
        let item_3 = DataCenterAttr {
            attribute_model_code: "ITEMA3".to_string(),
            value: AttrString(bran_name.to_string()).into(),
        };
        result.push(item_3);
        let item_4 = DataCenterAttr {
            attribute_model_code: "ITEMA4".to_string(),
            value: AttrString("".to_string()).into(),
        };
        result.push(item_4);
        let world_position = aios_mgr.get_world_transform(from).unwrap_or(None).unwrap_or_default();
        let item_5 = DataCenterAttr {
            attribute_model_code: "ITEMA5".to_string(),
            value: AttrVec3(world_position.translation).into(),
        };
        result.push(item_5);
        let item_8 = DataCenterAttr {
            attribute_model_code: "ITEMA8".to_string(),
            value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
        };
        result.push(item_8);
        // 上一个为 bran则取bran的hstu，其他的则取上一个元件的lstu
        let material_code = if from == bran_refno {
            bran_spre_material_code.clone()
        } else {
            let from_lstu = query_foreign_name_aql(from, vec!["LSTU", "LSTU"], database).await.unwrap_or(None).unwrap_or("".to_string());
            get_spre_material_code(&from_lstu).unwrap_or("".to_string())
        };
        let material_map = if let Ok(puhua_pool) = aios_mgr.get_puhua_pool().await {
            let query_code = need_query_material_code.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
            let material_map = get_material_map_from_code(&material_code, query_code, &puhua_pool).await;
            material_map
        } else {
            DashMap::default()
        };
        for (item_code, material_code) in &need_query_material_code {
            let material = if material_map.contains_key(material_code) {
                material_map.get(material_code).unwrap().value().clone()
            } else {
                "".to_string()
            };
            result.push(DataCenterAttr {
                attribute_model_code: item_code.to_string(),
                value: material,
            });
        }

        // 单位 mm
        let length = tubi.start_pt.distance(tubi.end_pt);
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMAA1".to_string(),
            value: AttrFloat(length).into(),
        });
        // 单位 m
        let weight_unit:f32 = if material_map.contains_key("Weight") {
            material_map.get("Weight").unwrap().value().clone().parse().unwrap_or(0.0)
        } else {
            0.0
        };
        let weight = length * weight_unit / 1000.0;
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMA19".to_string(),
            value: AttrFloat(weight).into(),
        });
        let room_code = query_room_name_from_refno_aql(from,database).await.unwrap_or(None).unwrap_or("".to_string());
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMA20".to_string(),
            value: AttrString(room_code).into(),
        });
        let attr = aios_mgr.get_attr(from).await.unwrap_or_default();
        let ispec = get_ispec_from_attr(&attr,&aios_mgr).await.unwrap_or("".to_string());
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMA21".to_string(),
            value: AttrString(ispec).into(),
        });
        let tspe = query_foreign_name_aql(bran_refno, vec!["TSPE", "TSPE"], database).await.unwrap_or(None).unwrap_or("".to_string());
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMA22".to_string(),
            value: AttrString(tspe).into(),
        });
        let r_text = get_rtext_from_attr(&attr,aios_mgr).await.unwrap_or("".to_string());
        result.push(DataCenterAttr {
            attribute_model_code: "ITEMA24".to_string(),
            value: AttrString(r_text).into(),
        });
        instances.push(DataCenterInstance {
            object_model_code: "ITEMAA".to_string(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: from.to_refno_string(),
            version: get_refno_latest_version(),
            attributes: result,
        });
    }
    instances
}

#[tokio::test]
async fn test_get_data_center_tubi_attr() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let bran_refno = RefU64::from_refno_str("24383/66761").unwrap();
    let result = get_data_center_tubi_attr(bran_refno,"/1WCC0578-21.3-NACJ-R54-R220",&database,&aios_mgr).await;
    let mut file = std::fs::File::create("tubi.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}