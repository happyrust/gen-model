use std::{env, fs};
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::pdms_types::{RefU64, UdaMajorType};
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::data_center_api::hole::get_pos_from_str;
use crate::consts::EMBED_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;

pub async fn query_embed_data(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<(String, DataCenterInstance)>> {
    let mut instances = Vec::new();
    let sql = gen_query_embed_data_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let speciality = result.try_get::<String, _>("Speciality").unwrap_or("".to_string());
            let speciality = UdaMajorType::from_chinese_description(&speciality).to_major_str();
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC1".to_string(),
                value: AttrValue::AttrString(speciality),
            });
            let code = result.try_get::<String, _>("Code").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC2".to_string(),
                value: AttrValue::AttrString(code),
            });
            let rely_item = result.try_get::<String, _>("RelyItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC3".to_string(),
                value: AttrValue::AttrString(rely_item),
            });
            let rely_item_ref = result.try_get::<String, _>("RelyItemRef").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC4".to_string(),
                value: AttrValue::AttrString(rely_item_ref),
            });
            let main_item = result.try_get::<String, _>("MainItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC6".to_string(),
                value: AttrValue::AttrString(main_item),
            });

            let position = result.try_get::<String, _>("Position").unwrap_or("".to_string());
            let position = get_pos_from_str(position);
            let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC9".to_string(),
                value: AttrValue::AttrFloatArray(position),
            });
            let ori = result.try_get::<String, _>("Ori").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC10".to_string(),
                value: AttrValue::AttrString(ori),
            });

            let work = result.try_get::<String, _>("Work").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC11".to_string(),
                value: AttrValue::AttrString(work),
            });
            let load = result.try_get::<String, _>("Load").unwrap_or("".to_string());
            let load = get_pos_from_str(load);
            let load = if load.len() > 2 { load } else { vec![0.0, 0.0, 0.0] };
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC12".to_string(),
                value: AttrValue::AttrFloatArray(load[..3].to_vec()),
            });
            let sub_material = result.try_get::<String, _>("SubsMaterial").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC14".to_string(),
                value: AttrValue::AttrString(sub_material),
            });
            let work_by = result.try_get::<String, _>("WorkBy").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC15".to_string(),
                value: AttrValue::AttrString(work_by),
            });
            let time = result.try_get::<String, _>("Time").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC16".to_string(),
                value: AttrValue::AttrString(time),
            });
            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC17".to_string(),
                value: AttrValue::AttrString(open_item),
            });
            let note = result.try_get::<String, _>("Note").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC18".to_string(),
                value: AttrValue::AttrString(note),
            });

            let fitt_id = result.try_get::<String, _>("FittID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC27".to_string(),
                value: AttrValue::AttrString(fitt_id),
            });
            let form = result.try_get::<String, _>("Form").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCCA2".to_string(),
                value: AttrValue::AttrString(form.clone()),
            });
            return match form.as_str() {
                "标准埋件(P)" => {
                    let stander_type = result.try_get::<String, _>("StanderType").unwrap_or("".to_string());
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA1".to_string(),
                        value: AttrValue::AttrString(stander_type),
                    });
                    Ok(Some(("埋件.json".to_string(), DataCenterInstance {
                        object_model_code: "1516".to_string(),
                        instance_code: "KY1801-208".to_string(),
                        attributes: instances,
                    })))
                }
                "非标准埋件(N)" => {
                    let size_length = result.try_get::<f32, _>("SizeLength").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB1".to_string(),
                        value: AttrValue::AttrFloat(size_length),
                    });
                    let size_thickness = result.try_get::<f32, _>("SizeThickness").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB2".to_string(),
                        value: AttrValue::AttrFloat(size_thickness),
                    });
                    Ok(Some(("非标准埋件.json".to_string(), DataCenterInstance {
                        object_model_code: "1516".to_string(),
                        instance_code: "KY1801-208".to_string(),
                        attributes: instances,
                    })))
                }
                _ => {
                    Ok(None)
                }
            };
        }
        _ => {}
    }
    Ok(None)
}

fn get_relay_item(relay_item: String) -> AttrValue {
    let mut result = Vec::new();
    let relay_items = relay_item.split("/").collect::<Vec<_>>();
    let relay_items = relay_items.last().unwrap();
    let relay_items = relay_items.split("-").collect::<Vec<_>>();
    for i in 0..5 {
        if let Some(item) = relay_items.get(i) {
            result.push(item.to_string());
        }
    }
    AttrValue::AttrStrArray(result)
}

fn gen_query_embed_data_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {EMBED_TABLE} WHERE RelyItemRef = '{}'", refno.to_refno_string()));
    sql
}

#[tokio::test]
async fn test_query_embed_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let refno = RefU64::from_refno_str("17496/105935").unwrap();
    let r = query_embed_data(refno, &pool).await?;
    if let Some((file_name,r)) = r {
        let mut file = fs::File::create(file_name.as_str())?;
        let data = serde_json::to_string(&r).unwrap();
        file.write_all(&data.into_bytes())?;
    }
    Ok(())
}
