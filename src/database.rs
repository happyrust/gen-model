use aios_core::pdms_types::{AttrMap, AttrVal};
use parse_pdms_db::local_db::DbOption;
use sqlx::{MySql, MySqlPool, Pool};
use sqlx::mysql::MySqlArguments;
use sqlx::pool::PoolConnection;
use crate::consts::URL;

pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}




#[inline]
pub fn get_connect_url(ip: &str, user: &str, pwd: &str, project: &str, port: &str) -> String {
    format!("mysql://{user}:{pwd}@{ip}:{port}/{project}")
}

pub async fn get_tidb_pool(connection_str: &str) -> Pool<MySql> {
    let connection = MySqlPool::connect(connection_str)
        .await
        .unwrap();
    connection
    // connection.try_acquire().unwrap()
}

//重新创建database
pub async fn init_database(project: &str, url: &str) {
    let connection = MySqlPool::connect(url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();

    let result = sqlx::query(&format!("drop database if exists {project}")).execute(&mut pool).await;
    let result = sqlx::query(&format!("create database {project}")).execute(&mut pool).await;
    // let result = sqlx::query(&format!("use {project}")).execute(&mut pool).await;
}

/// 创建 info 库和表
pub async fn init_info_database(url: &str) {
    let connection = MySqlPool::connect(&url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();
    sqlx::query("create database if not exists refno_infos;").execute(&mut pool).await;

    dbg!(url);
    let connection = MySqlPool::connect(&format!("{url}/refno_infos"))
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, "refno_infos"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL ,"#, "ref0"));  //refno.get_0() //ref_0 可能会重，先不设 primary key
    sql.push_str(&format!(r#"{} varchar(20)"#, "project"));

    sql.push_str(");");
    let result = sqlx::query(&sql).execute(&mut pool).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}