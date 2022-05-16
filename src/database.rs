use sqlx::MySqlPool;
use crate::consts::URL;

//重新创建database
pub async fn init_database(project: &str) {
    let connection = MySqlPool::connect(URL)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();

    let result = sqlx::query(&format!("drop database if exists {project}")).execute(&mut pool).await;
    let result = sqlx::query(&format!("create database {project}")).execute(&mut pool).await;
    let result = sqlx::query(&format!("use {project}")).execute(&mut pool).await;
}

/// 创建 info 库和表
pub async fn init_info_database() {
    let connection = MySqlPool::connect(URL)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();
    sqlx::query("create database if not exists refno_infos;").execute(&mut pool).await;


    let connection = MySqlPool::connect(&format!("{URL}/refno_infos"))
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