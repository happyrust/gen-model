use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use aios_core::pdms_types::{AttrInfo, AttrMap, AttrVal};
use dashmap::DashMap;
use parse_pdms_db::db1_dehash;
use serde_json::from_str;
use sqlx::{Error, MySql, MySqlPool, Pool};
use sqlx::mysql::MySqlQueryResult;
use sqlx::pool::PoolConnection;
use crate::helper::{qualified_column_name, qualified_table_name};

// #[derive(Iden)]
enum Character {
    Table,
    Id,
    Refno,
    Uuid,
    Character,
    FontSize,
    Meta,
    Decimal,
    BigDecimal,
    Created,
}

pub fn gen_create_explicit_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "explicit_att"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "refno"));
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "type"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "owner"));
    sql.push_str(&format!(r#"{} blob"#, "data"));
    sql.push_str(");");

    sql
}

pub fn gen_create_uda_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "uda_att"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "refno"));   //主要是方便显示查看
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "type"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "owner"));
    sql.push_str(&format!(r#"{} blob"#, "data"));
    sql.push_str(");");

    sql
}

/// 每个dbno对应的filename
#[inline]
pub fn gen_create_dbno_filename_tables_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "dbno_filename"));
    sql.push_str(&format!(r#"{} INT PRIMARY KEY ,"#, "dbno"));
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "filename"));
    sql.push_str(&format!(r#"{} INT, "#, "version"));
    sql.push_str(&format!(r#"{} VARCHAR(30) ,"#, "project"));
    sql.push_str(&format!(r#"{} VARCHAR(10) "#, "db_type"));
    sql.push_str(");");
    sql
}

#[inline]
pub fn gen_create_element_tables_sql() -> String {
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "pdms_elements"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "refno"));
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "type"));
    sql.push_str(&format!(r#"{} BIGINT,"#, "owner"));
    sql.push_str(&format!(r#"{} VARCHAR(100),"#, "name"));
    sql.push_str(&format!(r#"{} INT ,"#, "dbno"));
    sql.push_str(&format!(r#"{} INT "#, "order_num"));
    sql.push_str(");");

    sql
}

pub fn gen_create_attr_info_tables_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "attr_info"));
    sql.push_str(&format!(r#"{} int primary key ,"#, "type_hash"));
    sql.push_str(&format!(r#"{} varchar(8) ,"#, "type"));
    sql.push_str(&format!(r#"{} blob "#, "info"));
    sql.push_str(");");
    sql
}

pub fn gen_create_project_mdb_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "project_mdb"));
    sql.push_str(&format!(r#"{} varchar(20) ,"#, "mdb_name"));
    sql.push_str(&format!(r#"{} varchar(10) ,"#, "db_type"));
    sql.push_str(&format!(r#"{} blob "#,"data"));
    sql.push_str(");");
    sql
}

#[inline]
pub fn gen_create_implicit_tables_sql(type_name: &str, att_bmap: &BTreeMap<u32, (String, AttrVal)>) -> String {
    let mut sql = String::new();
    let table_name = qualified_table_name(type_name);
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, table_name));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} VARCHAR(30),"#, "refno"));   //refno
    sql.push_str(&format!(r#"{} VARCHAR(8),"#, "type"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL,"#, "owner"));
    // sql.push_str(&format!(r#"{} VARCHAR(30),"#, "refno"));

    for (offset, (k, v)) in att_bmap {
        // let att_name = db1_dehash(k.0).to_lowercase();
        let mut att_name_full = k.to_lowercase();
        if att_name_full.as_str() == "numbdb" {
            att_name_full = "dbno".to_string();
        }
        let att_name = qualified_column_name(att_name_full.as_str());

        match v {
            AttrVal::InvalidType => {}
            AttrVal::IntegerType(_) => {
                sql.push_str(&format!(r#"{} INT,"#, att_name));
            }
            AttrVal::StringType(_) => {
                //根据不同类型优化一下string的大小
                sql.push_str(&format!(r#"{} VARCHAR(20),"#, att_name));
            }
            AttrVal::DoubleType(_) => {
                sql.push_str(&format!(r#"{} DOUBLE,"#, att_name));
            }
            AttrVal::DoubleArrayType(_) => {
                sql.push_str(&format!(r#"{} BLOB,"#, att_name));
            }
            AttrVal::StringArrayType(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(80),"#, att_name));  //暂时用blob来表示，至于需不需要分表，看情况
            }
            AttrVal::BoolArrayType(_) => {
                sql.push_str(&format!(r#"{} INT,"#, att_name));
            }
            AttrVal::IntArrayType(_) | AttrVal::RefU64Array(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(50),"#, att_name));
            }
            AttrVal::BoolType(_) => {
                sql.push_str(&format!(r#"{} TINYINT(1),"#, att_name));
            }
            AttrVal::Vec3Type(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(20),"#, att_name));
            }
            AttrVal::ElementType(_) => {
                sql.push_str(&format!(r#"{} BIGINT,"#, att_name));
            }
            AttrVal::WordType(_) => {
                sql.push_str(&format!(r#"{} VARCHAR(10),"#, att_name));
            }
            AttrVal::RefU64Type(_) => {
                sql.push_str(&format!(r#"{} BIGINT,"#, att_name));
            }
            AttrVal::StringHashType(_) => {}
            _ => {}
        }
    }

    sql.remove(sql.len() - 1);
    sql.push_str(");");

    sql
}