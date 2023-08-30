use std::collections::HashMap;
use aios_core::pdms_data::NewDataOperate;
use aios_core::pdms_types::{AttrMap, RefU64, RefU64Vec};
use chrono::{Datelike, DateTime, Local, Timelike};
use parse_pdms_db::parse::WholeAttMap;
use sqlx::{Executor, MySql, Pool};
use sqlx::types::Uuid;
use serde::{Serialize, Deserialize};
use crate::data_interface::increment_manager::EleOperation;

pub const INCREMENT_DATA: &'static str = "INCREMENT_DATA";

#[derive(Debug, Serialize, Deserialize)]
pub struct IncreaseDataTiDB {
    pub refno: RefU64,
    pub data_operate: EleOperation,
    pub numbdb: i32,
    pub children: RefU64Vec,
    pub old_attr: AttrMap,
    pub new_attr: AttrMap,
    pub new_version: u32,
    pub old_version: u32,
}

impl IncreaseDataTiDB {
    /// 将增量数据保存到对应的表
    pub async fn save_increment_data(increment_datas: Vec<IncreaseDataTiDB>, session_name: String, pool: &Pool<MySql>) -> anyhow::Result<()> {
        // 将数据根据dbno分类
        let mut dbno_map = HashMap::new();
        for data in increment_datas {
            dbno_map.entry(data.numbdb).or_insert_with(Vec::new).push(data);
        }
        for (dbno, increment_data) in dbno_map {
            dbg!(&dbno);
            let Ok(_r) = create_increment_table(dbno, pool).await else { continue; };
            let sql = gen_insert_increment_sql(dbno, increment_data, &session_name);
            let mut conn = pool.acquire().await?;
            let result = conn.execute(sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(sql.as_str());
                }
            }
        }
        Ok(())
    }
}

fn gen_insert_increment_sql(dbno: i32, increment_datas: Vec<IncreaseDataTiDB>, session_name: &str) -> String {
    let mut sql = format!("INSERT INTO {dbno}_{INCREMENT_DATA}(ID,REFNO,REFNO_STR,OWNER, OPERATE, VERSION,TIME, CHILDREN,OLD_DATA,NEW_DATA,USER) VALUES");
    for increment_data in increment_datas {
        // uuid 作为图数据库和 tidb 连接的主键
        let id = Uuid::new_v4().to_string();
        let operate = increment_data.data_operate.into_tidb_num();
        let owner = increment_data.new_attr.get_owner();
        if owner.is_none() { continue; }
        let owner = owner.unwrap().0;
        let old_data = hex::encode(increment_data.old_attr.into_rkyv_compress_bytes());
        let new_data = hex::encode(increment_data.new_attr.into_rkyv_compress_bytes());
        let children = hex::encode(bincode::serialize(&increment_data.children).unwrap_or(vec![]));
        let local: DateTime<Local> = Local::now();
        let refno = increment_data.refno;
        let refno_str = refno.to_refno_string();
        let time = format!("{}-{}-{} {}:{}:{}", local.year(), local.month(), local.day(),
                           local.hour().to_string(), local.minute(), local.second());
        sql.push_str(&format!("('{}',{},'{refno_str}',{owner},{},{},'{time}',0x{},0x{},0x{},'{}') ,"
                              , id, refno, operate, increment_data.new_version, children, old_data, new_data, session_name));
    }
    sql.remove(sql.len() - 1);
    sql
}

/// 创建对应的增量记录表
pub async fn create_increment_table(dbno: i32, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_create_increment_table_sql(dbno);
    let mut conn = pool.acquire().await?;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
            dbg!(sql.as_str());
        }
    }
    Ok(())
}

/// 生成创建表的sql
fn gen_create_increment_table_sql(dbno: i32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {}_{INCREMENT_DATA} (", dbno));
    sql.push_str(&format!("{} VARCHAR(50) PRIMARY KEY ,", "ID"));
    sql.push_str(&format!("{} BIGINT ,", "REFNO"));
    sql.push_str(&format!("{} VARCHAR(30) ,", "REFNO_STR"));
    sql.push_str(&format!("{} BIGINT ,", "OWNER"));
    sql.push_str(&format!("{} SMALLINT ,", "OPERATE"));
    sql.push_str(&format!("{} INT ,", "VERSION"));
    sql.push_str(&format!("{} INT ,", "NUMBDB"));
    sql.push_str(&format!("{} VARCHAR(20) ,", "USER"));
    sql.push_str(&format!("{} BLOB ,", "CHILDREN"));
    sql.push_str(&format!("{} BLOB ,", "OLD_DATA"));
    sql.push_str(&format!("{} BLOB ,", "NEW_DATA"));
    sql.push_str(&format!("{} VARCHAR(50) ,", "TIME"));
    sql.push_str(&format!("{} VARCHAR(100) ", "DESC"));
    sql.push_str(");");
    sql
}

