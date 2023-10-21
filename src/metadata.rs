use std::{env, fs};
use std::cmp::max;
use std::io::Cursor;
use std::path::Path;
use anyhow::anyhow;
use calamine::{DataType, open_workbook, Range, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::{Error, Executor, MySql, Pool};
use sqlx::mysql::MySqlQueryResult;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::{MetadataManagerTableData, MetadataManagerTableDataExcelIndex, MetadataManagerTreeNode, MetadataManagerTreeNodeExcelIndex, ShowMetadataManagerTableData};
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

macro_rules! assign_tree_data {
    ($data:expr, $indexes:expr, $tree_data:expr,$field_name:ident) => {
        if let Some(field_index) = $indexes.$field_name {
            if let Some(field_data) = $data.get(field_index) {
                $tree_data.$field_name = field_data.clone();
            }
        }
    };
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

pub async fn create_metadata_tree_table_sql(pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {METADATA_TABLE} ("));
    sql.push_str(&format!("{} BIGINT UNSIGNED  PRIMARY KEY ,", "ID"));
    sql.push_str(&format!("{} BIGINT UNSIGNED,", "OWNER"));
    sql.push_str(&format!("{} VARCHAR(50) ,", "USER_CODE"));     // 对象类编码
    sql.push_str(&format!("{} VARCHAR(50) ,", "CHINESE_NAME"));  //中文名称
    sql.push_str(&format!("{} VARCHAR(50) ,", "ENGLISH_NAME"));   // 英文名称
    sql.push_str(&format!("{} VARCHAR(50) ,", "ENGLISH_DEFINE")); // 英文定义
    sql.push_str(&format!("{} VARCHAR(50) ,", "CHINESE_DEFINE")); // 中文定义
    sql.push_str(&format!("{} VARCHAR(50) ,", "CLASSIFY_CODE"));  // 分类编码
    sql.push_str(&format!("{} VARCHAR(50) ,", "CLASSIFY_NAME"));  // 分类名称
    sql.push_str(&format!("{} VARCHAR(50) ,", "CUSTOM_ITEM"));    // 自定义项
    sql.push_str(&format!("{} VARCHAR(100) ,", "DESCRIPTION"));   // 备注
    sql.push_str(&format!("{} TINYINT(1)  ,", "STATE"));          // 状态
    sql.push_str(&format!("{} VARCHAR(50) ", "OWNED_NAME"));     // 所有者
    sql.push_str(");");
    let mut conn = pool.clone();
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    Ok(())
}

pub async fn create_metadata_data_table_sql(pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {METADATA_DATA} ("));
    sql.push_str(&format!("{} BIGINT UNSIGNED,", "ID"));
    sql.push_str(&format!("{} VARCHAR(50),", "CODE"));            // 属性编码
    sql.push_str(&format!("{} VARCHAR(50),", "DATA_TYPE"));       // 数据类型
    sql.push_str(&format!("{} VARCHAR(50),", "DATA_CONSTRAINT")); // 数据约束
    sql.push_str(&format!("{} TINYINT(1) ,", "B_MULTI"));         // 是否多选
    sql.push_str(&format!("{} VARCHAR(50),", "ENGLISH_NAME"));    // 英文名称
    sql.push_str(&format!("{} VARCHAR(50),", "CHINESE_NAME"));    // 中文名称
    sql.push_str(&format!("{} VARCHAR(50),", "ENGLISH_DEFINE"));  // 英文定义
    sql.push_str(&format!("{} VARCHAR(50),", "CHINESE_DEFINE"));  // 中文定义
    sql.push_str(&format!("{} VARCHAR(50) ,", "UNIT"));           // 计量单位
    sql.push_str(&format!("{} VARCHAR(50) ,", "GROUPINGS"));          // 分组
    sql.push_str(&format!("{} VARCHAR(50) ,", "CUSTOM_ITEM"));     // 自定义项
    sql.push_str(&format!("{} VARCHAR(500),", "DESCRIPTION"));    // 备注
    sql.push_str(&format!("{} TINYINT(1)  ,", "STATE"));           // 状态
    sql.push_str(&format!("{} VARCHAR(50) ", "OWNED_NAME"));      // 所有者
    sql.push_str(");");
    let mut conn = pool.clone();
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&sql);
        }
    }
    Ok(())
}

pub async fn save_metadata_data(data: DashMap<u64, MetadataManagerTreeNode>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {METADATA_TABLE}(ID, OWNER,USER_CODE,CHINESE_NAME,ENGLISH_NAME,\
    ENGLISH_DEFINE,CHINESE_DEFINE,CLASSIFY_CODE,CLASSIFY_NAME,CUSTOM_ITEM,DESCRIPTION,STATE,OWNED_NAME) VALUES"));
    let b_empty = data.is_empty();
    for (_, v) in data {
        let english_define = correct_to_mysql_str(&v.english_define);
        let chinese_define = correct_to_mysql_str(&v.chinese_define);
        let classify_code = correct_to_mysql_str(&v.classify_code);
        let classify_name = correct_to_mysql_str(&v.classify_name);
        let custom_item = correct_to_mysql_str(&v.custom_item);
        let description = correct_to_mysql_str(&v.desc);
        let state = if v.state { 1 } else { 0 };

        sql.push_str(&format!("({},{},'{}','{}','{}','{english_define}','{chinese_define}','{classify_code}','{classify_name}','{custom_item}','{description}','{state}','{}') ,",
                              v.id, v.owner, v.user_code, v.chinese_name, v.english_name, v.owned_name));
    }
    if !b_empty {
        sql.remove(sql.len() - 1);
        let mut conn = pool;
        let result = conn.execute(sql.as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(sql);
                dbg!(&e);
            }
        }
    }
    Ok(())
}

pub async fn save_metadata_table_data(data: Vec<MetadataManagerTableData>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {METADATA_DATA}(ID,CODE,DATA_TYPE,DATA_CONSTRAINT,B_MULTI,ENGLISH_NAME,CHINESE_NAME,ENGLISH_DEFINE,\
        CHINESE_DEFINE,UNIT,GROUPINGS,CUSTOM_ITEM,DESCRIPTION,STATE,OWNED_NAME) VALUES"));
    let b_empty = data.is_empty();
    for v in data {
        let code = correct_to_mysql_str(&v.code);
        let data_type = correct_to_mysql_str(&v.data_type);
        let data_constraint = correct_to_mysql_str(&v.data_constraint);
        let chinese_name = correct_to_mysql_str(&v.chinese_name);
        let english_name = correct_to_mysql_str(&v.english_name);
        let english_define = correct_to_mysql_str(&v.english_define);
        let chinese_define = correct_to_mysql_str(&v.chinese_define);
        let unit = correct_to_mysql_str(&v.unit);
        // 默认值都是 "[0]" 所以 "[0]" 就不存
        let groups = if v.group == "[0]" { "".to_string() } else { correct_to_mysql_str(&v.group) };
        let custom_item = correct_to_mysql_str(&v.custom_item);
        let desc = correct_to_mysql_str(&v.desc);
        let state = if v.state { 1 } else { 0 };
        let owned_name = correct_to_mysql_str(&v.owned_name);

        sql.push_str(&format!("( {},'{code}','{data_type}','{data_constraint}', '{}', '{english_name}' ,\
         '{chinese_name}','{english_define}','{chinese_define}','{unit}','{groups}','{custom_item}','{desc}' ,'{state}','{owned_name}' ),", v.id, if v.b_multi { 1 } else { 0 }, ));
    }
    if !b_empty {
        sql.remove(sql.len() - 1);
    }
    let mut conn = pool;
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
        .ok_or(anyhow::anyhow!("Cannot find Sheet '对象'"))??;
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
                english_define: "".to_string(),
                chinese_define: "".to_string(),
                classify_code: "".to_string(),
                classify_name: "".to_string(),
                custom_item: "".to_string(),
                desc: "".to_string(),
                state: false,
                owned_name: "".to_string(),
            };
            map.entry(data.id).or_insert(data);
        }
    }
    // 表格的数据
    let mut table_map = Vec::new();
    let range_two = workbook.worksheet_range("属性")
        .ok_or(anyhow::anyhow!("Cannot find Sheet '属性'"))??;
    let mut iter = RangeDeserializerBuilder::new().from_range(&range_two)?;
    while let Some(result) = iter.next() {
        let v: MetadataManagerExcelTableData = result?;
        if !v.is_null() {
            let code = v.code.unwrap();
            let id = convert_str_to_hash(&get_characters_in_str(&code));
            let data_type = v.data_type.unwrap();
            let unit = v.unit.unwrap();
            let data = MetadataManagerTableData {
                id,
                code,
                data_type,
                data_constraint: "".to_string(),
                b_multi: false,
                english_name: "".to_string(),
                chinese_name: "".to_string(),
                english_define: "".to_string(),
                chinese_define: "".to_string(),
                unit,
                group: "".to_string(),
                custom_item: "".to_string(),
                desc: v.description.unwrap(),
                state: false,
                owned_name: "".to_string(),
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
pub fn convert_metadata_tree_value_from_excel_bytes(mut tree_data: Vec<Vec<String>>, major: &str) -> DashMap<u64, MetadataManagerTreeNode> {
    let mut tree_data_map: DashMap<u64, MetadataManagerTreeNode> = DashMap::new();
    // let headers = tree_data.remove(0);
    // 数据开始的行数
    let mut data_position = 0;
    // 找到树结构的数据位于 excel 表的哪一行
    let mut indexes = MetadataManagerTreeNodeExcelIndex::default();
    for datas in &tree_data {
        if indexes.user_code.is_some() && indexes.chinese_name.is_some() { break; }
        for (idx, header) in datas.into_iter().enumerate() {
            match header.trim() {
                "对象类编码" => { indexes.user_code = Some(idx) }
                "中文名称" => { indexes.chinese_name = Some(idx) }
                "英文名称" => { indexes.english_name = Some(idx) }
                "英文定义" => { indexes.english_define = Some(idx) }
                "中文定义" => { indexes.chinese_define = Some(idx) }
                "分类编码" => { indexes.classify_code = Some(idx) }
                "分类名称" => { indexes.classify_name = Some(idx) }
                "自定义项" => { indexes.custom_item = Some(idx) }
                "备注" => { indexes.desc = Some(idx) }
                "状态" => { indexes.state = Some(idx) }
                "所有者" => { indexes.owned_name = Some(idx) }
                _ => {}
            }
        }
        data_position += 1;
    }
    // 按表头对应数据的位置，把所有数据形成struct
    if indexes.user_code.is_some() && indexes.chinese_name.is_some() {
        // 第一个默认为根节点
        let mut b_head = true;
        for data in tree_data[data_position..].into_iter() {
            let mut tree_data = MetadataManagerTreeNode::default();
            // 通过索引 将值填入 struct 的对应字段中
            assign_tree_data!(data,indexes,tree_data,user_code);
            assign_tree_data!(data,indexes,tree_data,chinese_name);
            assign_tree_data!(data,indexes,tree_data,english_name);
            assign_tree_data!(data,indexes,tree_data,english_define);
            assign_tree_data!(data,indexes,tree_data,chinese_define);
            assign_tree_data!(data,indexes,tree_data,classify_code);
            assign_tree_data!(data,indexes,tree_data,classify_name);
            assign_tree_data!(data,indexes,tree_data,custom_item);
            assign_tree_data!(data,indexes,tree_data,desc);
            assign_tree_data!(data,indexes,tree_data,owned_name);

            if let Some(state_index) = indexes.state {
                if let Some(state_data) = data.get(state_index) {
                    let state = if state_data == "有效" { true } else { false };
                    tree_data.state = state;
                }
            }

            // 用户编码加上专业代号作为 id
            let mut id_str = format!("{}{}", major.clone(), &tree_data.user_code);
            let id = convert_str_to_hash(&id_str);
            // id_str 变成 owner_str
            id_str.remove(id_str.len() - 1);
            let owner = if b_head { 1 } else { convert_str_to_hash(&id_str) };
            tree_data.id = id;
            tree_data.owner = owner;
            // 如果是头节点，需要将专业代号带上
            if b_head {
                tree_data.chinese_name = format!("{}({})",tree_data.chinese_name,major);
            }
            tree_data_map.entry(id).or_insert(tree_data);
            b_head = false;
        }
    }
    tree_data_map
}

pub fn convert_metadata_table_value_from_excel_bytes(table_data: Vec<Vec<String>>, major: &str) -> Vec<MetadataManagerTableData> {
    let mut result = Vec::new();
    // let headers = table_data.remove(0);
    // 数据开始的行数
    let mut data_position = 0;
    let mut indexes = MetadataManagerTableDataExcelIndex::default();
    // 找到需要的数据位于表格的哪一列
    for headers in &table_data {
        if indexes.code.is_some() { break; }
        for (idx, header) in headers.into_iter().enumerate() {
            match header.trim() {
                "属性编码" => { indexes.code = Some(idx) }
                "数据类型" => { indexes.data_type = Some(idx) }
                "数据约束" => { indexes.data_constraint = Some(idx) }
                "是否多选" => { indexes.b_multi = Some(idx) }
                "英文名称" => { indexes.english_name = Some(idx) }
                "中文名称" => { indexes.chinese_name = Some(idx) }
                "英文定义" => { indexes.english_define = Some(idx) }
                "中文定义" => { indexes.chinese_define = Some(idx) }
                "计量单位" => { indexes.unit = Some(idx) }
                "分组" => { indexes.group = Some(idx) }
                "自定义项" => { indexes.custom_item = Some(idx) }
                "备注" => { indexes.desc = Some(idx) }
                "状态" => { indexes.state = Some(idx) }
                "所有者" => { indexes.owned_name = Some(idx) }
                _ => {}
            }
        }
        data_position += 1;
    }

    if indexes.code.is_some() {
        for data in table_data[data_position..].into_iter() {
            let mut manager_data = MetadataManagerTableData::default();

            assign_tree_data!(data,indexes,manager_data,code);
            assign_tree_data!(data,indexes,manager_data,data_type);
            assign_tree_data!(data,indexes,manager_data,data_constraint);
            assign_tree_data!(data,indexes,manager_data,chinese_name);
            assign_tree_data!(data,indexes,manager_data,english_name);
            assign_tree_data!(data,indexes,manager_data,english_define);
            assign_tree_data!(data,indexes,manager_data,chinese_define);
            assign_tree_data!(data,indexes,manager_data,unit);
            assign_tree_data!(data,indexes,manager_data,group);
            assign_tree_data!(data,indexes,manager_data,custom_item);
            assign_tree_data!(data,indexes,manager_data,desc);
            assign_tree_data!(data,indexes,manager_data,owned_name);

            if let Some(b_multi_index) = indexes.b_multi {
                if let Some(b_multi_data) = data.get(b_multi_index) {
                    let b_multi = if b_multi_data == "Y" { true } else { false };
                    manager_data.b_multi = b_multi;
                }
            }
            if let Some(state_index) = indexes.state {
                if let Some(state_data) = data.get(state_index) {
                    let state_data = if state_data == "有效" { true } else { false };
                    manager_data.state = state_data;
                }
            }
            /// 将 code 的 字符部 hash 为 u64
            let id_str = format!("{}{}", major, get_characters_in_str(&manager_data.code));
            let id = convert_str_to_hash(&id_str);
            manager_data.id = id;

            result.push(manager_data)
        }
    }
    result
}

pub async fn replace_metadata_table_data(data: Vec<ShowMetadataManagerTableData>, pool: &Pool<MySql>) -> anyhow::Result<bool> {
    let sql = gen_replace_metadata_table_data(data);
    let mut conn = pool;
    let result = conn.execute(sql.as_str()).await;
    return match result {
        Ok(_) => { Ok(true) }
        Err(e) => {
            Ok(false)
        }
    };
}

pub fn convert_str_to_hash(input: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(input, &mut hash);
    std::hash::Hasher::finish(&hash)
}

/// 获取 字符串中的字符部分 ,遇到非字符就停止
pub fn get_characters_in_str(input: &str) -> String {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    if let Some(captures) = regex.captures(input) {
        return captures[0].to_string();
    }
    "".to_string()
}

fn gen_replace_metadata_table_data(datas: Vec<ShowMetadataManagerTableData>) -> String {
    let mut sql = String::new();
    // for data in datas {
    //     let b_multi = if data.b_multi == "Y" { 1 } else { 0 };
    //     sql.push_str(format!("update metadata_data m set m.`CODE` = '{}',
    //                         m.`NAME` = '{}',m.B_NULL = {b_multi},m.DATA_TYPE ={},m.UNIT = {},m.DESCRIPTION = '{}',
    //                         m.SCOPE = '{}' WHERE m.ID = {} and m.`CODE` = '{}'",
    //                          data.new_code, data.name, data.data_type, data.unit, data.desc, data.scope, data.id, data.old_code).as_str());
    // }
    sql
}

/// 将不符合 mysql 语法的字符进行转义
pub fn correct_to_mysql_str(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for s in input.chars() {
        match s {
            '\'' => result.push_str("\\'"),
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            _ => result.push(s),
        }
    }
    result
}

/// 根据文件名确定传入的excel文件属于哪个专业
pub fn get_metadata_filename_major(file_name: &str) -> Option<String> {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    if let Some(captures) = regex.captures(file_name) {
        return Some(captures.get(0).map_or("".to_string(), |m| { m.as_str().to_uppercase() }));
    }
    None
}

#[tokio::test]
async fn test_create_metadata_table() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let table_sql = create_metadata_tree_table_sql(&pool).await?;
    let _ = create_metadata_data_table_sql(&pool).await?;

    let path = "resource/元数据_测试.xlsx";
    let (data, table_data) = read_excel_file_to_sql(path)?;
    save_metadata_data(data, &pool).await?;
    save_metadata_table_data(table_data, &pool).await?;
    Ok(())
}

#[test]
fn test_read_excel_bytes_data() {
    let path = Path::new("resource/附录E-系统类元数据.xlsx");
    let file_name = path.file_name().unwrap().to_str().unwrap();
    let major = get_metadata_filename_major(file_name);
    let data = fs::read(path).unwrap();

    if let Some(major) = major {
        let result = read_metadata_excel_bytes(data.clone(), 0);
        let map = convert_metadata_tree_value_from_excel_bytes(result.clone(), &major);
        let result = read_metadata_excel_bytes(data, 1);
        let table_data = convert_metadata_table_value_from_excel_bytes(result, &major);
        dbg!(&table_data);
    }
}

#[test]
fn test_regex() {
    let regex = Regex::new(r"[a-zA-Z]+").unwrap();
    let input = "abc123ab";
    if let Some(captures) = regex.captures(input) {
        dbg!(&captures[0]);
    }
}

#[test]
fn test_get_metadata_filename_major() {
    let file_name = "附录E-系统类元数据.xlsx";
    let major = get_metadata_filename_major(file_name);
    dbg!(&major);
}