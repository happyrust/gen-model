use std::env;
use anyhow::anyhow;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::{Error, Executor, MySql, Pool};
use sqlx::mysql::MySqlQueryResult;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::{MetadataManagerTableData, MetadataManagerTreeNode};
use bevy::prelude::dbg;
use regex::Regex;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MetadataManagerExcelTreeData {
    pub user_code: Option<String>,
    pub chinese_name: Option<String>,
    pub english_name: Option<String>,
}

impl MetadataManagerExcelTreeData {
    fn is_null(&self) -> bool {
        match self {
            MetadataManagerExcelTreeData {
                user_code: Some(_),
                chinese_name: Some(_),
                english_name: Some(_),
            } => false,
            _ => true
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MetadataManagerExcelTableData {
    pub code: Option<String>,
    pub name: Option<String>,
    pub b_null: Option<String>,
    pub data_type: Option<String>,
    pub unit: Option<String>,
    pub description: Option<String>,
    pub scope: Option<String>,
}

impl MetadataManagerExcelTableData {
    fn is_null(&self) -> bool {
        match self {
            MetadataManagerExcelTableData {
                code: Some(_),
                name: Some(_),
                b_null: Some(_),
                data_type: Some(_),
                unit: Some(_),
                description: Some(_),
                scope: Some(_)
            } => false,
            _ => true
        }
    }
}

fn create_metadata_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {METADATA_TABLE} ("));
    sql.push_str(&format!("{} BIGINT UNSIGNED  PRIMARY KEY ,", "ID"));
    sql.push_str(&format!("{} BIGINT UNSIGNED,", "OWNER"));
    sql.push_str(&format!("{} VARCHAR(50) ,", "USER_CODE"));
    sql.push_str(&format!("{} VARCHAR(50) ,", "CHINESE_NAME"));
    sql.push_str(&format!("{} VARCHAR(50) ", "ENGLISH_NAME"));
    sql.push_str(");");
    sql
}

async fn save_metadata_data(data: DashMap<u64, MetadataManagerTreeNode>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {METADATA_TABLE}(ID, OWNER,USER_CODE,CHINESE_NAME,ENGLISH_NAME) VALUES"));
    let b_empty = data.is_empty();
    for (_, v) in data {
        sql.push_str(&format!("({},{},'{}','{}','{}') ,", v.id, v.owner, v.user_code, v.chinese_name, v.english_name));
    }
    if !b_empty {
        sql.remove(sql.len() - 1);
    }
    let mut conn = pool.acquire().await?;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(sql);
            dbg!(&e);
        }
    }
    Ok(())
}

/// 将 excel 中的数据进行处理，放到sql中
fn read_excel_file_to_sql(file_path: &str) -> anyhow::Result<DashMap<u64, MetadataManagerTreeNode>> {
    let mut map = DashMap::new();
    let mut workbook: Xlsx<_> = open_workbook(file_path)?;
    // 树节点数据
    let range = workbook.worksheet_range("对象")
        .ok_or(anyhow!("Cannot find Sheet '对象'"))??;
    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    let mut b_head = true; // 第一个默认为根节点
    while let Some(result) = iter.next() {
        let v: MetadataManagerExcelTreeData = result?;
        if !v.is_null() {
            // 将 excel 的数据转化为树结构存储
            let user_code = v.user_code.unwrap();
            let id = convert_str_to_hash(&user_code);

            let mut user_code_split = user_code.clone();
            user_code_split.remove(user_code.len() - 1);
            let owner = if b_head { 0 } else { convert_str_to_hash(&user_code_split) };
            b_head = false;

            let data = MetadataManagerTreeNode {
                id,
                owner,
                user_code,
                chinese_name: v.chinese_name.unwrap(),
                english_name: v.english_name.unwrap(),
            };
            map.entry(data.id).or_insert(data);
        }
    }
    // 表格的数据
    let mut table_map = Vec::new();
    let range_two = workbook.worksheet_range("属性")
        .ok_or(anyhow!("Cannot find Sheet '属性'"))??;
    let mut iter = RangeDeserializerBuilder::new().from_range(&range_two)?;
    while let Some(result) = iter.next() {
        let v: MetadataManagerExcelTableData = result?;
        if !v.is_null() {
            let code = v.code.unwrap();
            let id = convert_str_to_hash(&get_characters_in_str(&code));
            let data_type = MetadataManagerTableData::convert_str_to_data_type(&v.data_type.unwrap());
            let unit = MetadataManagerTableData::convert_str_to_unit(&v.unit.unwrap());
            let data = MetadataManagerTableData {
                id,
                code,
                name: v.name.unwrap(),
                b_null: if v.b_null.unwrap() == "是" { true } else { false },
                data_type,
                unit,
                desc: v.description.unwrap(),
                scope: v.scope.unwrap(),
            };
            table_map.push(data);
        }
    }
    Ok(map)
}

pub fn convert_str_to_hash(input: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(input, &mut hash);
    std::hash::Hasher::finish(&hash)
}

/// 获取 字符串中的字符部分 ,遇到非字符就停止
fn get_characters_in_str(input: &str) -> String {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    if let Some(captures) = regex.captures(input) {
        return captures[0].to_string();
    }
    "".to_string()
}

#[tokio::test]
async fn test_create_metadata_table() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let table_sql = create_metadata_table_sql();
    let mut conn = pool.clone().acquire().await?;
    let result = conn.execute(table_sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(table_sql);
            dbg!(&e);
        }
    }
    let path = "resource/元数据_测试.xlsx";
    let data = read_excel_file_to_sql(path)?;
    save_metadata_data(data, &pool).await?;
    Ok(())
}

#[test]
fn test_regex() {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    let input = "abc123ab";
    if let Some(captures) = regex.captures(input) {
        dbg!(&captures[0]);
    }
}