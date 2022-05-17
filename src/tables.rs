use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use aios_core::pdms_types::{AttrMap, AttrVal};
use parse_pdms_db::db1_dehash;
use sqlx::{Error, MySql, MySqlPool, Pool};
use sqlx::mysql::MySqlQueryResult;
use sqlx::pool::PoolConnection;

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

pub async fn create_explicit_data_tables(connection: &mut PoolConnection<MySql>){
    // let mut connection = pool.try_acquire().unwrap();


    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "explicit_att"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} varchar(30),"#, "refno"));
    sql.push_str(&format!(r#"{} varchar(8),"#, "type"));
    sql.push_str(&format!(r#"{} bigint,"#, "owner"));
    sql.push_str(&format!(r#"{} blob"#, "data"));

    sql.push_str(");");

    let result = sqlx::query(&sql).execute(connection).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}

pub async fn create_uda_data_tables(connection: &mut PoolConnection<MySql>){
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "uda_att"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} varchar(30),"#, "refno"));   //主要是方便显示查看
    sql.push_str(&format!(r#"{} varchar(8),"#, "type"));
    sql.push_str(&format!(r#"{} bigint,"#, "owner"));
    sql.push_str(&format!(r#"{} blob"#, "data"));
    sql.push_str(");");

    let result = sqlx::query(&sql).execute(connection).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}

/// 每个dbno对应的filename
pub async fn create_dbno_filename_tables(connection: &mut PoolConnection<MySql>) {
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "dbno_filename"));
    sql.push_str(&format!(r#"{} int,"#, "dbno"));
    sql.push_str(&format!(r#"{} varchar(30),"#, "filename"));
    sql.push_str(&format!(r#"{} int "#,"version"));
    sql.push_str(");");

    let result = sqlx::query(&sql).execute(connection).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}

pub async fn create_element_tables(connection: &mut PoolConnection<MySql>){
    let mut sql = String::new();
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "pdms_elements"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} varchar(30),"#, "refno"));
    sql.push_str(&format!(r#"{} varchar(8),"#, "type"));
    sql.push_str(&format!(r#"{} bigint,"#, "owner"));
    sql.push_str(&format!(r#"{} varchar(100),"#, "name"));
    sql.push_str(&format!(r#"{} int,"#, "dbno"));
    sql.push_str(&format!(r#"{} varchar(20)"#, "project"));
    sql.push_str(");");

    let result = sqlx::query(&sql).execute(connection).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}

// #[async_std::main]
pub async fn create_implicit_tables(conn: &mut PoolConnection<MySql>, type_name: &str, att_bmap: &BTreeMap<u32, (String, AttrVal)>) {
    // let connection = MySqlPool::connect(connection_str)
    //     .await
    //     .unwrap();
    // let mut pool = connection.try_acquire().unwrap();


    let mut sql = String::new();
    let table_name = type_name.to_lowercase().replace("join", "joint");
    let table_name = table_name.replace("loop","loop_");
    //后续可以创建一个owner表
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, table_name));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY,"#, "id"));  //refno 的64位
    sql.push_str(&format!(r#"{} varchar(30),"#, "refno"));   //refno
    sql.push_str(&format!(r#"{} varchar(8),"#, "type"));
    sql.push_str(&format!(r#"{} bigint not null,"#, "owner"));
    // sql.push_str(&format!(r#"{} varchar(30),"#, "refno"));

    for (offset, (k, v)) in att_bmap {
        // let att_name = db1_dehash(k.0).to_lowercase();
        let mut att_name_full = k.to_lowercase();
        if att_name_full.as_str() == "numbdb" {
            att_name_full = "dbno".to_string();
        }
        let att_name = att_name_full.replace("desc", "desc_").replace("lock", "lock_").replace("char", "char_");

        match v {
            AttrVal::InvalidType => {}
            AttrVal::IntegerType(_) => {
                sql.push_str(&format!(r#"{} int,"#, att_name));
            }
            AttrVal::StringType(_) => {
                //根据不同类型优化一下string的大小
                sql.push_str(&format!(r#"{} varchar(20),"#, att_name));
            }
            AttrVal::DoubleType(_) => {
                sql.push_str(&format!(r#"{} bigint,"#, att_name));
            }
            AttrVal::DoubleArrayType(_) => {
                sql.push_str(&format!(r#"{} blob,"#, att_name));
            }
            AttrVal::StringArrayType(_) => {
                sql.push_str(&format!(r#"{} varchar(80),"#, att_name));  //暂时用blob来表示，至于需不需要分表，看情况
            }
            AttrVal::BoolArrayType(_) => {
                sql.push_str(&format!(r#"{} int,"#, att_name));
            }
            AttrVal::IntArrayType(_) => {
                sql.push_str(&format!(r#"{} varchar(50),"#, att_name));
            }
            AttrVal::BoolType(_) => {
                sql.push_str(&format!(r#"{} tinyint(1),"#, att_name));
            }
            AttrVal::Vec3Type(_) => {
                sql.push_str(&format!(r#"{} varchar(20),"#, att_name));
            }
            AttrVal::ElementType(_) => {
                sql.push_str(&format!(r#"{} bigint,"#, att_name));
            }
            AttrVal::WordType(_) => {
                sql.push_str(&format!(r#"{} varchar(10),"#, att_name));
            }
            AttrVal::RefU64Type(_) => {
                sql.push_str(&format!(r#"{} bigint,"#, att_name));
            }
            AttrVal::StringHashType(_) => {}
        }
        // sql.push_str(&format!(r#""{}" varchar(100) NOT NULL,"#, "name"));   //refno 的值
    }

    sql.remove(sql.len() - 1);



    sql.push_str(");");
    // dbg!(sql.as_str());
    // let mut conn = pool.try_acquire().unwrap();
    let result = sqlx::query(&sql).execute(conn).await;
    // println!("Create table character: {:?}\n", result);
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}