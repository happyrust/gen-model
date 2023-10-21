use aios_core::pdms_types::RefU64;
use sqlx::{Executor, MySql};
use aios_core::ssc_setting::{SelectedSiteVec, SiteVec};
use aios_core::vague_search::SearchConditionSave;
use sqlx::Pool;
use sqlx::Row;

/// 保存查询条件到数据库
pub async fn save_vague_search_condition(condition: SearchConditionSave, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let create_table_sql = create_vague_search_aql();
    let mut conn = pool.clone();
    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else { return Ok(()); };
    let insert_value_sql = gen_insert_sql(condition);
    let _ = conn.execute(insert_value_sql.as_str()).await;
    Ok(())
}


fn gen_insert_sql(condition: SearchConditionSave) -> String {
    let mut sql = String::from("INSERT IGNORE INTO search_condition (user, name,major,note,conditions) VALUES ");
    let mut con = String::new();
    for i in &condition.condition {
        con += i;
        con += ",";
    }
    sql.push_str(&format!("('{}', '{}', '{}', '{}','{}'),", condition.user, condition.name,condition.major,condition.note,con));
    sql.remove(sql.len() - 1);
    sql
}


/// 创建模糊查询条件表 search_condition sql
fn create_vague_search_aql() -> String {
    format!("CREATE TABLE IF NOT EXISTS search_condition (
        user VARCHAR(255) NOT NULL,
        name VARCHAR(255) NOT NULL,
        major VARCHAR(255) NOT NULL,
        note VARCHAR(255) NOT NULL,
        conditions VARCHAR(255) NOT NULL
    );")
}

///查询数据库中所有记录
pub fn gen_query_all_vague_search_info_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT user,name,major,note,conditions FROM search_condition"));
    sql
}
