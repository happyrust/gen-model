use std::env;
use std::io::Write;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors_lite::Database;
use bevy_transform::prelude::Transform;
use glam::{Mat3, Quat, Vec3};
use sqlx::{MySql, Pool, Row};
use crate::ansys::SctnAnsysData;
use crate::api::element::query_name;
use crate::aql_api::children::query_children_order_aql;
use crate::aql_api::pdms_mesh::query_refno_transform;
use crate::aql_api::query_transform::query_cylinder_transform;
use crate::consts::SCTN_STANDARD;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use crate::consts::CHANNEL_STEEL_STANDARD;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

pub async fn query_single_sctn_ansys_data(refno: RefU64, aios_mgr: &AiosDBManager,database:&ArDatabase) -> anyhow::Result<Option<SctnAnsysData>> {
    // 查找pdms中 sctn 对应的属性
    if let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(refno).await {
        let sql = gen_query_sctn_pdms_data_sql(refno);
        let result = sqlx::query(&sql).fetch_one(&mut pool.clone().acquire().await?).await;
        return match result {
            Ok(v) => {
                let poss: [f32; 3] = serde_json::from_str(&v.get::<String, _>("POSS")).unwrap_or([0.0; 3]);
                let pose: [f32; 3] = serde_json::from_str(&v.get::<String, _>("POSE")).unwrap_or([0.0; 3]);
                // 将 mm 转为 m
                let poss = Vec3::from_array(poss) * Vec3::from_array([0.001; 3]);
                let pose = Vec3::from_array(pose) * Vec3::from_array([0.001; 3]);
                let transform = query_refno_transform(refno, &database).await?.unwrap_or_default();
                let quat = Mat3::from_quat(transform.rotation);
                let dir = quat.y_axis + poss;
                let spre = RefU64(v.get::<i64, _>("SPRE") as u64);
                if let Some((_, cata_pool)) = aios_mgr.get_project_pool_by_refno(spre).await {
                    let spre_name = query_name(spre, &cata_pool).await?;
                    // 通过spre找到规格
                    // 查找型钢规格
                    let ansys_data = query_sctn_standard(&spre_name, &pool).await?;
                    if ansys_data.is_none() { return Ok(None); }
                    let mut ansys_data = ansys_data.unwrap();
                    ansys_data.point = vec![poss, pose];
                    ansys_data.rotation = dir;
                    ansys_data.connect_point = vec![1, 2];
                    return Ok(Some(ansys_data));
                }
                Ok(None)
            }
            Err(err) => { Ok(None) }
        };
    }
    Ok(None)
}

pub async fn query_single_sctn_ansys_data_test(refno: RefU64, pool: &Pool<MySql>, cata_pool: &Pool<MySql>, database: &ArDatabase) -> anyhow::Result<Option<SctnAnsysData>> {
    // 查找pdms中 sctn 对应的属性
    let sql = gen_query_sctn_pdms_data_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => {
            let poss: [f32; 3] = serde_json::from_str(&v.get::<String, _>("POSS")).unwrap_or([0.0; 3]);
            let pose: [f32; 3] = serde_json::from_str(&v.get::<String, _>("POSE")).unwrap_or([0.0; 3]);
            // 将 mm 转为 m
            let poss = Vec3::from_array(poss) * Vec3::from_array([0.001; 3]);
            let pose = Vec3::from_array(pose) * Vec3::from_array([0.001; 3]);
            let transform = query_refno_transform(refno, &database).await?.unwrap_or_default();
            let quat = Mat3::from_quat(transform.rotation);
            let dir = quat.y_axis + poss;
            let spre = RefU64(v.get::<i64, _>("SPRE") as u64);
            let spre_name = query_name(spre, cata_pool).await?;
            // 通过spre找到规格
            // 查找型钢规格
            let ansys_data = query_sctn_standard(&spre_name, pool).await?;
            if ansys_data.is_none() { return Ok(None); }
            let mut ansys_data = ansys_data.unwrap();
            ansys_data.point = vec![poss, pose];
            ansys_data.rotation = dir;
            ansys_data.connect_point = vec![1, 2];
            Ok(Some(ansys_data))
        }
        Err(err) => { Ok(None) }
    };
}

async fn query_sctn_standard(spre_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<SctnAnsysData>> {
    let spre_name_split = spre_name.split("/").collect::<Vec<_>>();
    if spre_name_split.len() < 2 { return Ok(None); }
    let spre_name_split = spre_name_split.last().unwrap();
    // 槽钢
    return if spre_name_split.starts_with("[") {
        let standard = &spre_name_split[1..];
        let sql = gen_query_channel_sctn_standard_sql(standard);
        let query_standard_result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
        if query_standard_result.is_err() { return Ok(None); }
        let query_standard_result = query_standard_result.unwrap();
        let h = query_standard_result.get::<f32, _>("H");
        let b = query_standard_result.get::<f32, _>("B");
        let d = query_standard_result.get::<f32, _>("D");
        let t = query_standard_result.get::<f32, _>("T");
        Ok(Some(SctnAnsysData {
            point: vec![],
            rotation: Default::default(),
            connect_point: vec![],
            w1: b,
            w2: b,
            w3: h,
            t1: t,
            t2: t,
            t3: d,
            b_channel_steel: true,
        }))
    } else {
        // 工字钢
        let standard = get_spre_standard(spre_name_split);
        if standard.is_none() { return Ok(None); }
        let standard = standard.unwrap();
        let query_standard_sql = gen_query_sctn_standard_sql(&standard);
        let query_standard_result = sqlx::query(&query_standard_sql).fetch_one(&mut pool.acquire().await?).await;
        if query_standard_result.is_err() { return Ok(None); }
        let query_standard_result = query_standard_result.unwrap();
        let h = query_standard_result.get::<f32, _>("H");
        let b = query_standard_result.get::<f32, _>("B");
        Ok(Some(SctnAnsysData {
            point: vec![],
            rotation: Default::default(),
            connect_point: vec![],
            w1: b,
            w2: b,
            w3: h,
            t1: standard.t1,
            t2: standard.t2,
            t3: standard.t2,
            b_channel_steel: false,
        }))
    };
}

fn gen_query_sctn_pdms_data_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT POSS, POSE , SPRE FROM SCTN WHERE ID = {}", refno.0));
    sql
}

fn gen_query_channel_sctn_standard_sql(size: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT H,B,D,T FROM {CHANNEL_STEEL_STANDARD} WHERE SIZE = '{}'", size));
    sql
}

fn gen_query_sctn_standard_sql(standard: &SctnSpreStandard) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT H,B FROM {SCTN_STANDARD} WHERE CLASS = '{}' AND SIZE = '{}' AND T1 = {} AND T2 = {}", standard.class, standard.size, standard.t1, standard.t2));
    sql
}

struct SctnSpreStandard {
    pub class: String,
    pub size: String,
    pub t1: f32,
    pub t2: f32,
}

fn get_spre_standard(input: &str) -> Option<SctnSpreStandard> {
    let input_split = input.split("X").collect::<Vec<_>>();
    if input_split.len() < 4 { return None; }

    let letter_re = regex::Regex::new(r"([a-zA-Z]+)").unwrap();
    let number_re = regex::Regex::new(r"(\d+)").unwrap();
    let class = letter_re.find(input_split[0]).map(|m| m.as_str()).unwrap_or("");
    let number = number_re.find(input_split[0]).map(|m| m.as_str()).unwrap_or("").parse::<i32>();
    if number.is_err() { return None; }
    let number = number.unwrap();
    let size = format!("{}X{}", number, input_split[1]);
    let t1 = input_split[2].parse::<f32>();
    let t2 = input_split[3].parse::<f32>();
    if t1.is_err() || t2.is_err() { return None; }
    let t1 = t1.unwrap();
    let t2 = t2.unwrap();

    Some(SctnSpreStandard {
        class: class.to_string(),
        size,
        t1,
        t2,
    })
}

#[tokio::test]
async fn test_query_single_sctn_ansys_data() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let cata_pool = AiosDBManager::get_db_pool(&url, "zdj").await?;
    let refno = RefU64::from_refno_str("24383/69687").unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let children = query_children_order_aql(&database, refno).await?;
    let mut sctns = Vec::new();
    for child in children {
        let sctn = query_single_sctn_ansys_data_test(child.refno, &pool, &cata_pool, &database).await?;
        if sctn.is_none() { continue; }
        sctns.push(sctn.unwrap());
    }
    let data = SctnAnsysData::create_many_sctn_ansys_file(sctns);
    let mut file = std::fs::File::create("sctn_ansys.txt").unwrap();
    file.write_all(&data).unwrap();
    Ok(())
}