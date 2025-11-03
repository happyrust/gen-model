use std::collections::HashMap;
use std::env;
use std::hash::Hash;
use sqlx::{MySql, Pool, Executor, Error, Row, Column};
use sqlx::mysql::MySqlRow;
use crate::data_interface::tidb_manager::AiosDBManager;

pub async fn create_and_insert_profession_table(map: Vec<HashMap<String, String>>, form_name: &str, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut conn = pool;
    let sql = gen_create_profession_table_sql(&map[0], form_name);
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
            dbg!(sql.as_str());
        }
    }
    gen_insert_profession_to_db_sql(map.clone(), form_name, pool).await?;
    Ok(())
}

pub fn gen_create_profession_table_sql(map: &HashMap<String, String>, form_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("CREATE TABLE IF NOT EXISTS {} (", form_name));
    for key in map.keys() {
        sql.push_str(&format!("{} varchar(100),", key));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(");");
    sql
}

pub async fn gen_insert_profession_to_db_sql(map: Vec<HashMap<String, String>>, form_name: &str, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut project_conn = pool;
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {} ( ", form_name));
    let mut key_vec = vec![];
    for keys in map[0].clone().keys() {
        key_vec.push(keys.clone());
        sql.push_str(&format!("{} ,", keys));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(") Values");
    let mut value_sql = String::new();
    let mut i = 0;
    let map_len = &map.len();
    for vals in map {
        value_sql.push_str("(");
        for key in &key_vec {
            if let Some(val) = vals.get(key) {
                value_sql.push_str(&format!("'{}' ,", val));
            }
        }
        value_sql.remove(value_sql.len() - 1);
        value_sql.push_str("),");
        i += 1;
        if i == 500 || i == *map_len {
            value_sql.remove(value_sql.len() - 1);
            value_sql.push_str(";");
            let result = project_conn.execute(format!("{}{}", sql, value_sql).as_str()).await;
            match result {
                Ok(_) => {}
                Err(_) => {
                    dbg!(format!("{}{}", sql, value_sql));
                }
            }
        }
    }
    Ok(())
}

pub async fn select_profession_form_data_page(table: &str, page: u32, page_count: u32, pool: &Pool<MySql>) -> anyhow::Result<Vec<HashMap<String, String>>> {
    let mut r = vec![];
    let count = page * page_count;
    let sql = format!("SELECT * FROM {} ORDER BY ID LIMIT {} , {}", table, count, page_count);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    match result {
        Ok(vals) => {
            for val in vals {
                let mut map = HashMap::new();
                let names = val.columns();
                for name in names {
                    let filed = name.name();
                    let v = val.get::<String, _>(filed);
                    map.insert(filed.to_string(), v);
                }
                r.push(map);
            }
        }
        Err(_) => {}
    }
    Ok(r)
}

#[tokio::test]
async fn test_gen_insert_to_db_sql() -> anyhow::Result<()> {
    let mut val = vec![];
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let mut map = HashMap::new();
    map.insert("a".to_string(), "b".to_string());
    map.insert("c".to_string(), "d".to_string());
    val.push(map);
    let mut map = HashMap::new();
    map.insert("a".to_string(), "f".to_string());
    map.insert("c".to_string(), "h".to_string());
    val.push(map);
    let sql = create_and_insert_profession_table(val, "test", &pool).await;
    Ok(())
}

#[tokio::test]
async fn test_select_profession_form_data_page() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "Sample").await?;
    let v = select_profession_form_data_page("EXPLICIT_ATT", 0, 10, &pool).await.unwrap();
    for i in v {
        println!("i={:?}", i);
    }
    Ok(())
}