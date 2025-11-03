use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::pdms_types::*;
use aios_core::{AttrMap, RefnoEnum};
use aios_core::{Datetime as SurrealDatetime, RecordId};
use chrono::{DateTime, Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};
use serde_with::DisplayFromStr;
use serde_with::serde_as;
#[cfg(feature = "sql")]
use sqlx::types::Uuid;
#[cfg(feature = "sql")]
use sqlx::{Executor, MySql, Pool, Row};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;

pub const INCREMENT_DATA: &'static str = "INCREMENT_DATA";

///需要修改的模型的增量参考号数据
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncrGeoUpdateLog {
    //基本体模型修改了的参考号
    pub prim_refnos: HashSet<RefnoEnum>,
    //拉伸体模型修改了的参考号
    pub loop_owner_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub bran_hanger_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub basic_cata_refnos: HashSet<RefnoEnum>,
    //删除了的模型
    pub delete_refnos: HashSet<RefnoEnum>,
}

impl IncrGeoUpdateLog {
    #[inline]
    pub fn count(&self) -> usize {
        self.prim_refnos.len()
            + self.loop_owner_refnos.len()
            + self.basic_cata_refnos.len()
            + self.bran_hanger_refnos.len()
    }

    #[inline]
    pub fn get_all_visible_refnos(&self) -> HashSet<RefnoEnum> {
        let mut refnos = HashSet::new();
        refnos.extend(self.prim_refnos.iter());
        refnos.extend(self.loop_owner_refnos.iter());
        refnos.extend(self.basic_cata_refnos.iter());
        refnos.extend(self.bran_hanger_refnos.iter());
        refnos
    }
}

//各个db的信息记录，需要跟踪起来？

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct IncrEleUpdateLog {
    pub refno: RefnoEnum,
    pub data_operate: EleOperation,
    pub numbdb: i32,
    // pub children: RefnoEnumVec,
    pub old_attr: AttrMap,
    pub new_attr: AttrMap,
    pub new_version: u32,
    pub old_version: u32,

    //按时间戳去对比更新是否完成
    pub timestamp: SurrealDatetime,
}

impl IncrEleUpdateLog {
    /// 将增量数据保存到对应的表
    #[cfg(feature = "sql")]
    pub async fn save_increment_data_to_sql(
        increment_datas: Vec<IncrEleUpdateLog>,
        session_name: String,
        pool: &Pool<MySql>,
    ) -> anyhow::Result<()> {
        // 将数据根据dbno分类
        let mut data_map = HashMap::new();
        for data in increment_datas {
            data_map
                .entry(data.numbdb)
                .or_insert_with(Vec::new)
                .push(data);
        }
        for (dbno, increment_data) in data_map {
            let Ok(_r) = create_increment_table(dbno, pool).await else {
                continue;
            };
            let sql = gen_insert_increment_sql(dbno, increment_data, &session_name);
            let mut conn = pool;
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

#[cfg(feature = "sql")]
fn gen_insert_increment_sql(
    dbno: i32,
    increment_datas: Vec<IncrEleUpdateLog>,
    session_name: &str,
) -> String {
    let mut sql = format!(
        "INSERT IGNORE INTO {dbno}_{INCREMENT_DATA}(ID,REFNO,REFNO_STR,OWNER, OPERATE, VERSION,NUMBDB,TIME,CHILDREN,OLD_DATA,NEW_DATA,USER) VALUES"
    );
    for increment_data in increment_datas {
        // uuid 作为图数据库和 tidb 连接的主键
        let id = Uuid::new_v4().to_string();
        let operate = increment_data.data_operate.into_tidb_num();
        let mut owner = increment_data.new_attr.get_owner();
        if owner.is_unset() {
            owner = increment_data.old_attr.get_owner()
        }
        if owner.is_unset() {
            continue;
        }
        let old_data = hex::encode(increment_data.old_attr.into_rkyv_compress_bytes());
        let new_data = hex::encode(increment_data.new_attr.into_rkyv_compress_bytes());
        let children = hex::encode(bincode::serialize(&increment_data.children).unwrap_or(vec![]));
        let local: DateTime<Local> = Local::now();
        let dbnum = increment_data.numbdb;
        let refno = increment_data.refno;
        let refno_str = refno.to_string();
        let time = format!(
            "{}-{}-{} {}:{}:{}",
            local.year(),
            local.month(),
            local.day(),
            local.hour().to_string(),
            local.minute(),
            local.second()
        );
        sql.push_str(&format!(
            "('{}',{},'{refno_str}',{owner},{},{},{dbnum},'{time}',0x{},0x{},0x{},'{}') ,",
            id,
            refno.0,
            operate,
            increment_data.new_version,
            children,
            old_data,
            new_data,
            session_name
        ));
    }
    sql.remove(sql.len() - 1);
    sql
}

/// 通过uuid查询该条增删记录
#[cfg(feature = "sql")]
pub async fn query_key_data(
    key: &str,
    numbdb: i32,
    pool: &Pool<MySql>,
) -> anyhow::Result<Option<IncrEleUpdateLog>> {
    let sql = gen_query_key_data_sql(key, numbdb);
    let val = sqlx::query(&sql).fetch_one(pool).await?;
    // let id = val.get::<String, _>("ID");
    let refno = RefnoEnum(val.get::<i64, _>("REFNO") as u64);
    let operate = val.get::<i32, _>("OPERATE");
    let numb_db = val.get::<i32, _>("NUMBDB");
    let children: Vec<RefnoEnum> =
        bincode::deserialize(&val.get::<Vec<u8>, _>("CHILDREN")).unwrap_or_default();
    let old_data = AttrMap::from_rkvy_compress_bytes(&val.get::<Vec<u8>, _>("OLD_DATA"))?;
    let new_data = AttrMap::from_rkvy_compress_bytes(&val.get::<Vec<u8>, _>("NEW_DATA"))?;
    let time = val.get::<String, _>("TIME");
    Ok(Some(IncrEleUpdateLog {
        refno,
        data_operate: EleOperation::from(operate),
        numbdb: numb_db,
        children,
        old_attr: old_data,
        new_attr: new_data,
        new_version: 0,
        old_version: 0,
        timestamp: Default::default(),
    }))
}

/// 创建对应的增量记录表
#[cfg(feature = "sql")]
pub async fn create_increment_table(dbno: i32, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_create_increment_table_sql(dbno);
    let mut conn = pool;
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
#[cfg(feature = "sql")]
fn gen_create_increment_table_sql(dbno: i32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        "CREATE TABLE IF NOT EXISTS {}_{INCREMENT_DATA} (",
        dbno
    ));
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
    sql.push_str(&format!("{} VARCHAR(100) ", "DESCRIPTION"));
    sql.push_str(");");
    sql
}

#[cfg(feature = "sql")]
fn gen_query_key_data_sql(key: &str, numbdb: i32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        "SELECT * FROM {}_{INCREMENT_DATA} WHERE ID = '{}'",
        numbdb, key
    ));
    sql
}

#[cfg(feature = "sql")]
#[tokio::test]
async fn test_increment_record() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let key = "d92e74ae-1c96-42d6-9674-0b57f9dd0e5f";
    let result = query_key_data(key, 7997, &pool).await?;
    dbg!(&result);
    Ok(())
}
