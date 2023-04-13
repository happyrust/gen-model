use std::{env, fs};
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject, ItemValue};
use aios_core::pdms_types::{RefU64, UdaMajorType};
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::data_center_api::hole::{convert_time_to_vec, get_pos_from_str};
use crate::consts::EMBED_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;

pub async fn create_embed_data(pool: &Pool<MySql>) -> anyhow::Result<Option<DataCenterProject>> {
    let mut instances = Vec::new();
    for i in 7..30 {
        let instance = query_embed_data(i, pool).await?;
        if let Some(instance) = instance {
            instances.push(instance);
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances,
    };
    Ok(Some(project))
}

async fn query_embed_data(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Option<DataCenterInstance>> {
    let mut instances = Vec::new();
    let sql = gen_query_embed_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC1".to_string(),
                value: AttrValue::AttrStrArray(vec!["Tes".to_string(), "Te".to_string(), "Te".to_string(), "Test".to_string(), "T".to_string()]).into(),
            });
            let code = result.try_get::<String, _>("Code").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC2".to_string(),
                value: AttrValue::AttrString(code).into(),
            });
            let rely_item = result.try_get::<String, _>("RelyItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC3".to_string(),
                value: AttrValue::AttrString(rely_item).into(),
            });
            let rely_item_ref = result.try_get::<String, _>("RelyItemRef").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC4".to_string(),
                value: AttrValue::AttrString(rely_item_ref).into(),
            });

            let main_item = result.try_get::<String, _>("MainItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC6".to_string(),
                value: AttrValue::AttrString(main_item).into(),
            });

            let mut stucc_7 = Vec::new();
            stucc_7.push(ItemValue::String("T".to_string()));
            stucc_7.push(ItemValue::String("Te".to_string()));
            stucc_7.push(ItemValue::String("Test".to_string()));
            stucc_7.push(ItemValue::Int(1));
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC7".to_string(),
                value: AttrValue::AttrItemArray(stucc_7).into(),
            });

            let ori = result.try_get::<String, _>("Ori").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC10".to_string(),
                value: AttrValue::AttrString(ori).into(),
            });

            let work = result.try_get::<String, _>("Work").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC11".to_string(),
                value: AttrValue::AttrString(work).into(),
            });
            let load = result.try_get::<String, _>("Load").unwrap_or("".to_string());
            let load = get_pos_from_str(load);
            let load = if load.len() > 5 { load } else { vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC12".to_string(),
                value: AttrValue::AttrFloatArray(load.to_vec()).into(),
            });

            let sub_material = result.try_get::<String, _>("SubsMaterial").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC14".to_string(),
                value: AttrValue::AttrString(sub_material).into(),
            });
            let work_by = result.try_get::<String, _>("WorkBy").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC15".to_string(),
                value: AttrValue::AttrString(work_by).into(),
            });
            let time = result.try_get::<String, _>("Time").unwrap_or("".to_string());
            let time = time.replace("/", "-");
            let time = convert_time_to_vec(&time);
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC16".to_string(),
                value: AttrValue::AttrStrArray(time).into(),
            });

            let open_item = result.try_get::<String, _>("OpenItem").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC17".to_string(),
                value: AttrValue::AttrString(open_item).into(),
            });
            let note = result.try_get::<String, _>("Note").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC18".to_string(),
                value: AttrValue::AttrString(note).into(),
            });
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC19".to_string(),
                value: AttrValue::AttrString("Test".to_string()).into(),
            });

            let fitt_id = result.try_get::<String,_>("FittID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC27".to_string(),
                value: AttrValue::AttrString(fitt_id).into(),
            });
            let embed_bpid = result.try_get::<String,_>("EmbedBPID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC28".to_string(),
                value: AttrValue::AttrString(embed_bpid).into(),
            });
            let embed_b_pver = result.try_get::<String,_>("EmbedBPVER").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC29".to_string(),
                value: AttrValue::AttrString(embed_b_pver).into(),
            });
            let rely_item_bpid  = result.try_get::<String,_>("RelyItemBPID").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC30".to_string(),
                value: AttrValue::AttrString(rely_item_bpid).into(),
            });
            let rely_item_bpver = result.try_get::<String,_>("RelyItemBPVER").unwrap_or("".to_string());
            instances.push(DataCenterAttr {
                attribute_model_code: "STUCC31".to_string(),
                value: AttrValue::AttrString(rely_item_bpver).into(),
            });

            let form = result.try_get::<String, _>("Form").unwrap_or("".to_string());

            return match form.as_str() {
                "标准埋件(P)" => {
                    let stander_type = result.try_get::<String, _>("StanderType").unwrap_or("".to_string());
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA1".to_string(),
                        value: AttrValue::AttrString(stander_type).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA2".to_string(),
                        value: AttrValue::AttrFloat(0.0).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA3".to_string(),
                        value: AttrValue::AttrString("预埋板".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCA4".to_string(),
                        value: AttrValue::AttrFloat(0.0).into(),
                    });
                    Ok(Some(DataCenterInstance {
                        object_model_code: "STUCCA".to_string(),
                        project_code: "1516".to_string(),
                        instance_code: format!("STUCCA{}", id),
                        version: "A版".to_string(),
                        attributes: instances,
                    }))
                }
                "非标准埋件(N)" => {
                    let size_length = result.try_get::<f32, _>("SizeLength").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB1".to_string(),
                        value: AttrValue::AttrFloat(size_length).into(),
                    });
                    let size_thickness = result.try_get::<f32, _>("SizeThickness").unwrap_or(0.0);
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB2".to_string(),
                        value: AttrValue::AttrFloat(size_thickness).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB3".to_string(),
                        value: AttrValue::AttrString("非标预埋板".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB4".to_string(),
                        value: AttrValue::AttrString("Test".to_string()).into(),
                    });
                    instances.push(DataCenterAttr {
                        attribute_model_code: "STUCCB5".to_string(),
                        value: AttrValue::AttrFloat(0.0).into(),
                    });
                    Ok(Some(DataCenterInstance {
                        object_model_code: "STUCCB".to_string(),
                        project_code: "1516".to_string(),
                        instance_code: format!("STCUCCB{}", id),
                        version: "A版".to_string(),
                        attributes: instances,
                    }))
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

fn gen_query_embed_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {EMBED_TABLE} WHERE IntelId = {}", id));
    sql
}

#[tokio::test]
async fn test_query_embed_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let r = create_embed_data(&pool).await?;
    if let Some(r) = r {
        let mut file = fs::File::create("埋件.json")?;
        let data = serde_json::to_string(&r).unwrap();
        file.write_all(&data.into_bytes())?;
    }
    Ok(())
}
