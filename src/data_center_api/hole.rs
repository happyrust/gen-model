use std::collections::HashMap;
use std::{env, fs};
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject, HoleType, ItemValue};
use aios_core::pdms_types::RefU64;
use sqlx::{Error, Executor, MySql, Pool, Row};
use sqlx::mysql::{MySqlQueryResult, MySqlRow};
use crate::consts::HOLES_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::rvm::data_api::get_num_from_str;

async fn query_hole_data(refno: RefU64, pool: &Pool<MySql>) -> Option<DataCenterInstance> {
    if let Ok(hole_type) = query_hole_type(refno, pool).await {
        let result = match hole_type {
            HoleType::STUCJ => {
                DataCenterInstance {
                    object_model_code: "1516".to_string(),
                    instance_code: "KY1801-208".to_string(),
                    attributes: gen_stucj_data(refno, pool).await,
                }
            }
            _ => { DataCenterInstance::default() }
        };
        return Some(result);
    }
    None
}

/// 查找改参考号属于哪种孔洞
async fn query_hole_type(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<HoleType> {
    let sql = gen_query_hole_type_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let h_type = result.get::<String, _>("hType");
            let material = result.get::<String, _>("SubsMaterial");
            return match h_type.as_str() {
                "K" => { Ok(HoleType::STUCJ) }
                "T" => { if material.as_str() == "Q235" { Ok(HoleType::STUCG) } else { Ok(HoleType::STUCH) } }
                "G" => { Ok(HoleType::STUCK) }
                "S" => { Ok(HoleType::STUCL) }
                "X" | "Y" => { Ok(HoleType::STUCM) }
                _ => { Ok(HoleType::Unknown) }
            };
        }
        Err(err) => { dbg!(&err); }
    }
    Ok(HoleType::Unknown)
}

async fn gen_stucj_data(refno: RefU64, pool: &Pool<MySql>) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    if let Ok(stucj_data_map) = query_stucj_data(refno, pool).await {
        for i in 0..31 {
            let name = format!("STUCJ{}", i);
            let value = stucj_data_map.get(&name);
            if value.is_none() { continue; }
            let value = value.unwrap();
            result.push(DataCenterAttr {
                attribute_model_code: name,
                value: value.clone(),
            });
        }
    }
    result
}

/// 查找stucj的数据
async fn query_stucj_data(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let sql = gen_query_hole_data_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let item_ref = result.try_get::<String, _>("ItemREF").unwrap_or("".to_string());
            let value = get_item_ref_value(item_ref, HoleType::STUCJ);
            map.entry("STUCJ1".to_string()).or_insert(AttrValue::AttrItemArray(value));
            let h_type = result.get::<String, _>("hType");
            map.entry("STUCJ2".to_string()).or_insert(AttrValue::AttrString(h_type));
            let code = result.get::<String, _>("Code");
            map.entry("STUCJ3".to_string()).or_insert(AttrValue::AttrString(code));
            let rely_item = result.get::<String, _>("RelyItem");
            map.entry("STUCJ4".to_string()).or_insert(AttrValue::AttrString(rely_item));
            let rely_item_ref = result.get::<String, _>("RelyItemREF");
            map.entry("STUCJ5".to_string()).or_insert(AttrValue::AttrString(rely_item_ref));
            let main_pipe_line = result.get::<String, _>("MainPipeline");
            map.entry("STUCJ6".to_string()).or_insert(AttrValue::AttrString(main_pipe_line));

            let position = result.get::<String, _>("Position");
            let position = get_pos_from_str(position);
            let position = if position.len() > 2 { position } else { vec![0.0, 0.0, 0.0] };
            map.entry("STUCJ7".to_string()).or_insert(AttrValue::AttrFloatArray(position));
            let ori = result.get::<String, _>("Ori");
            map.entry("STUCJ8".to_string()).or_insert(AttrValue::AttrString(ori));

            let shape = result.get::<String, _>("Shape");
            let size_height = result.get::<f32, _>("SizeHeight");
            let size_width = result.get::<f32, _>("SizeWidth");
            let mut shape_map = HashMap::new();
            match shape.as_str() {
                "CIR" => {
                    shape_map.entry("圆形孔洞".to_string()).or_insert(vec![size_height]);
                }
                "RECT" => {
                    shape_map.entry("方形孔洞".to_string()).or_insert(vec![size_width, size_height]);
                }
                _ => {}
            };
            map.entry("STUCJ10".to_string()).or_insert(AttrValue::AttrMapFloatArray(shape_map));

            let bank_height = result.try_get::<f32, _>("BankHeight").unwrap_or(0.0);
            let bank_width = result.try_get::<f32, _>("BankWidth").unwrap_or(0.0);
            map.entry("STUCJ12".to_string()).or_insert(AttrValue::AttrFloat(bank_height));
            map.entry("STUCJ13".to_string()).or_insert(AttrValue::AttrFloat(bank_width));
            if bank_height != 0.0 && bank_width != 0.0 {
                map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("Y".to_string()));
            } else {
                map.entry("STUCJ11".to_string()).or_insert(AttrValue::AttrString("N".to_string()));
            }

            let plug_type = result.get::<Option<String>, _>("PlugType");
            if let Some(plug_type) = plug_type {
                map.entry("STUCJ15".to_string()).or_insert(AttrValue::AttrString("Y".to_string()));
                map.entry("STUCJ16".to_string()).or_insert(AttrValue::AttrString(plug_type));
            } else {
                map.entry("STUCJ15".to_string()).or_insert(AttrValue::AttrString("N".to_string()));
                map.entry("STUCJ16".to_string()).or_insert(AttrValue::AttrString("".to_string()));
            }

            let b_second = result.get::<bool, _>("Second");
            map.entry("STUCJ19".to_string()).or_insert(AttrValue::AttrBool(b_second));
            let hole_work = result.get::<String, _>("HoleWork");
            map.entry("STUCJ21".to_string()).or_insert(AttrValue::AttrString(hole_work));
            let work_by = result.get::<String, _>("WorkBy");
            map.entry("STUCJ22".to_string()).or_insert(AttrValue::AttrString(work_by));
            let time = result.get::<String, _>("Time");
            map.entry("STUCJ23".to_string()).or_insert(AttrValue::AttrString(time));
            let open_item = result.get::<String, _>("OpenItem");
            map.entry("STUCJ24".to_string()).or_insert(AttrValue::AttrString(open_item));
            let note = result.get::<String, _>("Note");
            map.entry("STUCJ25".to_string()).or_insert(AttrValue::AttrString(note));
            let hole_b_pid = result.get::<f32, _>("HoleBPID");
            map.entry("STUCJ27".to_string()).or_insert(AttrValue::AttrFloat(hole_b_pid));
            let hole_b_pver = result.get::<f32, _>("HoleBPVER");
            map.entry("STUCJ28".to_string()).or_insert(AttrValue::AttrFloat(hole_b_pver));
            let rely_item_b_pid = result.get::<f32, _>("RelyItemBPID");
            map.entry("STUCJ29".to_string()).or_insert(AttrValue::AttrFloat(rely_item_b_pid));
            let rely_item_b_pver = result.get::<f32, _>("RelyItemBPVER");
            map.entry("STUCJ30".to_string()).or_insert(AttrValue::AttrFloat(rely_item_b_pver));
            let fitt_refno = result.get::<String, _>("FittRefNo");
            map.entry("STUCJ26".to_string()).or_insert(AttrValue::AttrString(fitt_refno));
            map.entry("STUCJ5".to_string()).or_insert(AttrValue::AttrString("".to_string()));
            map.entry("STUCJ18".to_string()).or_insert(AttrValue::AttrString("600".to_string()));
            map.entry("STUCJ18".to_string()).or_insert(AttrValue::AttrFloatArray(vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0]));
        }
        Err(err) => { dbg!(&err); }
    }
    Ok(map)
}

async fn query_stucg_data(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, AttrValue>> {
    let mut map = HashMap::new();
    let sql = gen_query_hole_data_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(result) => {
            let item_ref = result.try_get::<String, _>("ItemREF").unwrap_or("".to_string());
            let value = get_item_ref_value(item_ref, HoleType::STUCJ);
        }
        _ => {}
    }
    Ok(map)
}

fn gen_query_hole_data_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {HOLES_TABLE} WHERE refNo = '{}'", refno.to_refno_string()));
    sql
}

fn gen_query_hole_type_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT hType,SubsMaterial FROM {HOLES_TABLE} WHERE refNo = '{}'", refno.to_refno_string()));
    sql
}

fn get_item_ref_value(item_ref: String, h_type: HoleType) -> Vec<ItemValue> {
    let mut result = Vec::new();
    if item_ref.len() > 10 {
        match h_type {
            HoleType::STUCJ => {
                result.push(ItemValue::String(item_ref[..1].to_string()));
                result.push(ItemValue::String(item_ref[1..3].to_string()));
                result.push(ItemValue::String(item_ref[3..4].to_string()));
                result.push(ItemValue::String(item_ref[4..6].to_string()));
                let num = get_num_from_str(&item_ref[1..]).unwrap_or(0);
                result.push(ItemValue::Int(num));
                let len = item_ref.len();
                result.push(ItemValue::String(item_ref[len - 1..len].to_string()));
            }
            HoleType::STUCG => {
                if item_ref.len() >= 12 {
                    result.push(ItemValue::String(item_ref[..3].to_string()));
                    result.push(ItemValue::String(item_ref[3..5].to_string()));
                    result.push(ItemValue::String(item_ref[5..7].to_string()));
                    result.push(ItemValue::String(item_ref[7..11].to_string()));
                    result.push(ItemValue::String(item_ref[11..12].to_string()));
                }
            }
            HoleType::STUCH => {}
            HoleType::STUCK => {}
            HoleType::STUCL => {}
            HoleType::STUCM => {}
            HoleType::Unknown => {}
        }
    }
    result
}


pub(crate) fn get_pos_from_str(input: String) -> Vec<f32> {
    let mut result = Vec::new();
    let input_split = input.split(",").collect::<Vec<&str>>();
    for input_str in input_split {
        let data = input_str.parse::<f32>();
        if data.is_err() { continue; }
        result.push(data.unwrap());
    }
    result
}

#[test]
fn test_get_item_ref_value() {
    let item_refno = "1RSETT0003T".to_string();
    let r = get_item_ref_value(item_refno, HoleType::STUCG);
    dbg!(&r);
}

#[tokio::test]
async fn test_gen_stucj_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let refno = RefU64::from_refno_str("24383/101196").unwrap();
    let r = query_hole_data(refno, &pool).await;
    let mut file = fs::File::create("孔洞.json")?;
    if let Some(r) = r {
        let data = DataCenterProject {
            project_code: "1516".to_string(),
            owner: "KY1801-208".to_string(),
            instances: vec![r],
        };
        let data = serde_json::to_string(&data).unwrap();
        file.write_all(&data.into_bytes())?;
    }
    Ok(())
}