use std::{env, fs};
use std::cmp::max;
use std::io::Cursor;
use anyhow::anyhow;
use calamine::{DataType, open_workbook, Range, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::{Error, Executor, MySql, Pool};
use sqlx::mysql::MySqlQueryResult;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::{MetadataManagerTableData, MetadataManagerTreeNode};
use bevy::prelude::dbg;
use bevy::reflect::Array;
use regex::Regex;
use crate::consts::METADATA_DATA;

macro_rules! max {
    ($x: expr) => ($x);
    ($x: expr, $($z: expr),+) => {{
        let y = max!($($z),*);
        if $x > y {
            $x
        } else {
            y
        }
    }}
}

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

fn create_metadata_tree_table_sql() -> String {
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

fn create_metadata_data_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {METADATA_DATA} ("));
    sql.push_str(&format!("{} BIGINT UNSIGNED,", "ID"));
    sql.push_str(&format!("{} VARCHAR(50),", "CODE"));
    sql.push_str(&format!("{} VARCHAR(50),", "NAME"));
    sql.push_str(&format!("{} TINYINT(1) ,", "B_NULL"));
    sql.push_str(&format!("{} TINYINT ,", "DATA_TYPE"));
    sql.push_str(&format!("{} TINYINT ,", "UNIT"));
    sql.push_str(&format!("{} VARCHAR(100),", "DESCRIPTION"));
    sql.push_str(&format!("{} VARCHAR(50) ", "SCOPE"));
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

async fn save_metadata_table_data(data: Vec<MetadataManagerTableData>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {METADATA_DATA}(ID,CODE,NAME,B_NULL,DATA_TYPE,UNIT,DESCRIPTION,SCOPE) VALUES"));
    let b_empty = data.is_empty();
    for v in data {
        sql.push_str(&format!("( {},'{}', '{}' , {} , {} , {} , '{}' , '{}' ) ,", v.id, v.code, v.name, if v.b_null { 1 } else { 0 },
                              v.data_type, v.unit, v.desc, v.scope));
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
fn read_excel_file_to_sql(file_path: &str) -> anyhow::Result<(DashMap<u64, MetadataManagerTreeNode>, Vec<MetadataManagerTableData>)> {
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
    Ok((map, table_map))
}

pub fn read_metadata_excel_bytes(data: Vec<u8>, sheet_idx: usize) -> Vec<Vec<String>> {
    let buffer: Cursor<Vec<u8>> = Cursor::new(data);

    let mut sheets = calamine::open_workbook_auto_from_rs(buffer).unwrap();
    let first_sheet = sheets.worksheet_range_at(sheet_idx);
    let mut rows_vec = vec![];
    match first_sheet {
        Some(sheet_result) => match sheet_result {
            Ok(range) => {
                for r in range.rows() {
                    let mut r_vec = vec![];
                    for (_, cell) in r.iter().enumerate() {
                        r_vec.push(format!("{}", cell).to_string());
                    }
                    rows_vec.push(r_vec);
                }
            }
            _ => {}
        },
        _ => {}
    };
    rows_vec
}

/// 读取 excel 表格生成元数据管理树结构的数据
pub fn convert_metadata_tree_value_from_excel_bytes(mut tree_data: Vec<Vec<String>>) -> DashMap<u64, MetadataManagerTreeNode>{
    let mut tree_data_map: DashMap<u64, MetadataManagerTreeNode> = DashMap::new();
    let headers = tree_data.remove(0);
    // 找到树结构的数据位于 excel 表的哪一行
    let mut user_code_idx = None;
    let mut chinese_name_idx = None;
    let mut english_name_idx = None;
    for (idx, header) in headers.into_iter().enumerate() {
        match header.to_lowercase().as_str() {
            "user_code" => { user_code_idx = Some(idx) }
            "chinese_name" => { chinese_name_idx = Some(idx) }
            "english_name" => { english_name_idx = Some(idx) }
            _ => {}
        }
    }
    // 按表头对应数据的位置，把所有数据形成struct
    if user_code_idx.is_some() && chinese_name_idx.is_some() && english_name_idx.is_some() {
        let user_code_idx = user_code_idx.unwrap();
        let chinese_name_idx = chinese_name_idx.unwrap();
        let english_name_idx = english_name_idx.unwrap();
        let max_idx = max!(user_code_idx,chinese_name_idx,english_name_idx);
        let mut b_head = true; // 第一个默认为根节点
        for mut data in tree_data.into_iter() {
            if max_idx >= data.len() { continue; }
            let user_code_data = data[user_code_idx].clone();
            if user_code_data.is_empty() { continue; }
            let chinese_name_data = data[chinese_name_idx].clone();
            let english_name_data = data[english_name_idx].clone();
            let id = convert_str_to_hash(&user_code_data);
            let mut user_code_split = user_code_data.clone();
            user_code_split.remove(user_code_data.len() - 1);
            let owner = if b_head { 0 } else { convert_str_to_hash(&user_code_split) };
            tree_data_map.entry(id).or_insert(MetadataManagerTreeNode {
                id,
                owner,
                user_code: user_code_data,
                chinese_name: chinese_name_data,
                english_name: english_name_data,
            });
            b_head = false;
        }
    }
    tree_data_map
}

pub fn convert_metadata_table_value_from_excel_bytes(mut table_data: Vec<Vec<String>>) -> Vec<MetadataManagerTableData> {
    let mut result = Vec::new();
    let headers = table_data.remove(0);

    let mut code_idx = None;
    let mut name_idx = None;
    let mut b_null_idx = None;
    let mut data_type_idx = None;
    let mut unit_idx = None;
    let mut des_idx = None;
    let mut scope_idx = None;
    // 找到需要的数据位于表格的哪一列
    for (idx, header) in headers.into_iter().enumerate() {
        match header.to_lowercase().as_str() {
            "code" => { code_idx = Some(idx) }
            "name" => { name_idx = Some(idx) }
            "b_null" => { b_null_idx = Some(idx) }
            "data_type" => { data_type_idx = Some(idx) }
            "unit" => { unit_idx = Some(idx) }
            "description" => { des_idx = Some(idx) }
            "scope" => { scope_idx = Some(idx) }
            _ => {}
        }
    }
    if code_idx.is_some() && name_idx.is_some() && b_null_idx.is_some() && data_type_idx.is_some()
        && unit_idx.is_some() && des_idx.is_some() && scope_idx.is_some() {
        let code_idx = code_idx.unwrap();
        let name_idx = name_idx.unwrap();
        let b_null_idx = b_null_idx.unwrap();
        let data_type_idx = data_type_idx.unwrap();
        let unit_idx = unit_idx.unwrap();
        let des_idx = des_idx.unwrap();
        let scope_idx = scope_idx.unwrap();
        let max_idx = max!(code_idx,name_idx,name_idx,data_type_idx,unit_idx,des_idx,scope_idx);

        for data in table_data {
            if max_idx >= data.len() { continue; }
            let code = data[code_idx].clone();
            let id = convert_str_to_hash(&get_characters_in_str(&code));
            let name = data[name_idx].clone();
            let b_null = if data[b_null_idx] == "是" { true } else { false };
            let data_type = MetadataManagerTableData::convert_str_to_data_type(&data[data_type_idx]);
            let unit = MetadataManagerTableData::convert_str_to_unit(&data[unit_idx]);
            let desc = data[des_idx].clone();
            let scope = data[scope_idx].clone();

            result.push(MetadataManagerTableData {
                id,
                code,
                name,
                b_null,
                data_type,
                unit,
                desc,
                scope,
            })
        }
    }
    result
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
    let table_sql = create_metadata_tree_table_sql();
    let mut conn = pool.clone().acquire().await?;
    let result = conn.execute(table_sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(table_sql);
            dbg!(&e);
        }
    }
    let data_sql = create_metadata_data_table_sql();
    let mut conn = pool.clone().acquire().await?;
    let result = conn.execute(data_sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(data_sql);
            dbg!(&e);
        }
    }
    let path = "resource/元数据_测试.xlsx";
    let (data, table_data) = read_excel_file_to_sql(path)?;
    save_metadata_data(data, &pool).await?;
    save_metadata_table_data(table_data, &pool).await?;
    Ok(())
}

#[test]
fn test_read_excel_bytes_data() {
    let path = "resource/元数据_测试.xlsx";
    let data = fs::read(path).unwrap();
    let result = read_metadata_excel_bytes(data.clone(), 0);
    let map = convert_metadata_tree_value_from_excel_bytes(result.clone());
    let result = read_metadata_excel_bytes(data, 1);
    let table_data = convert_metadata_table_value_from_excel_bytes(result);
    dbg!(&table_data.len());
}

#[test]
fn test_regex() {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    let input = "abc123ab";
    if let Some(captures) = regex.captures(input) {
        dbg!(&captures[0]);
    }
}