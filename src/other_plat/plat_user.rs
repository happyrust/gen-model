use std::sync::Arc;
use aios_core::plat_user::PuHuaPlatUser;
use sqlx::{Executor, MySql, Pool, Row};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 将平台的人员（非pdms人员）保存到数据库
pub async fn save_plat_user(users: Vec<PuHuaPlatUser>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let create_table_sql = create_plat_user_aql();
    let mut conn = pool.clone().acquire().await?;
    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else { return Ok(()); };
    if users.is_empty() { return Ok(()); }
    let insert_value_sql = gen_insert_plat_user_sql(users);
    let _ = conn.execute(insert_value_sql.as_str()).await;
    Ok(())
}

/// 登录功能，判断是否存在该用户
pub async fn b_exit_user(aios_mgr: &AiosDBManager, user: &str) -> anyhow::Result<bool> {
    let sql = gen_b_exit_user_sql(user);
    let global_pool = aios_mgr.get_global_pool().await?;
    let mut conn = global_pool.acquire().await?;
    let Ok(result) = conn.fetch_one(sql.as_str()).await else { return Ok(false); };
    let b_exit = result.get::<i32, _>(0) > 0;
    Ok(b_exit)
}

fn gen_b_exit_user_sql(user: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(1) FROM PuHuaPlatUser WHERE NAME = '{}'", user));
    sql
}

/// 生成人员信息的插入语句
fn gen_insert_plat_user_sql(users: Vec<PuHuaPlatUser>) -> String {
    let mut sql = String::from("INSERT IGNORE INTO PuHuaPlatUser (id, work_num, name, depart) VALUES ");
    for user in users {
        sql.push_str(&format!("( '{}', '{}', '{}', '{}' ),", user.id, user.work_num, user.name, user.depart))
    }
    sql.remove(sql.len() - 1);
    sql
}

/// 创建普华人员表sql
fn create_plat_user_aql() -> String {
    format!("CREATE TABLE IF NOT EXISTS PuHuaPlatUser (
        id VARCHAR(255) NOT NULL,
        work_num VARCHAR(255) NOT NULL,
        name VARCHAR(255) NOT NULL,
        depart VARCHAR(255) NOT NULL
    );")
}