use aios_core::pdms_types::RefU64;
use sqlx::{Executor, MySql};
use aios_core::ssc_setting::{SelectedSiteVec, SiteVec};
use sqlx::Pool;
use sqlx::Row;

/// 初始化时保存所有SITE
pub async fn save_all_site(sites: SiteVec, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let create_table_sql = create_ssc_sql();
    let mut conn = pool.clone().acquire().await?;
    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else { return Ok(()); };

    let clear_table_sql = clear_ssc_sql();
    let clear_table_result = conn.execute(clear_table_sql.as_str()).await;
    let Ok(_) = clear_table_result else { return Ok(()); };

    if sites.data.is_empty() { return Ok(()); }
    let insert_value_sql = gen_insert_ssc_sql(sites);
    let _ = conn.execute(insert_value_sql.as_str()).await;
    Ok(())
}

pub async fn save_selected_site(sites: SelectedSiteVec, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let create_table_sql = create_selected_ssc_sql();
    let mut conn = pool.clone().acquire().await?;
    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else { return Ok(()); };


    let clear_table_sql = clear_selected_ssc_sql();
    let clear_table_result = conn.execute(clear_table_sql.as_str()).await;
    let Ok(_) = clear_table_result else { return Ok(()); };

    if sites.data.is_empty() { return Ok(()); }
    let insert_value_sql = gen_insert_selected_ssc_sql(sites);
    let _ = conn.execute(insert_value_sql.as_str()).await;
    Ok(())
}


fn gen_insert_ssc_sql(sites: SiteVec) -> String {
    let mut sql = String::from("INSERT IGNORE INTO AllSscData (refno, name) VALUES ");
    for site in sites.data {
        sql.push_str(&format!("( '{}', '{}' ),", site.refno, site.name))
    }
    sql.remove(sql.len() - 1);
    sql
}

fn gen_insert_selected_ssc_sql(sites: SelectedSiteVec) -> String {
    let mut sql = String::from("INSERT IGNORE INTO SelectedSscData (refno, name) VALUES ");
    for site in sites.data {
        sql.push_str(&format!("( '{}', '{}' ),", site.refno, site.name))
    }
    sql.remove(sql.len() - 1);
    sql
}

pub async fn query_all_ssc(pool: &Pool<MySql>) -> anyhow::Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let sql = gen_query_ssc_sql();
    let mut conn = pool.acquire().await?;
    let Ok(query_results) = conn.fetch_all(sql.as_str()).await else { return Ok(vec![]); };
    for query_result in query_results {
        let refno = query_result.get::<String, _>("refno");
        let name = query_result.get::<String, _>("name");
        result.push((refno, name));
    }
    Ok(result)
}

pub async fn query_selected_ssc(pool: &Pool<MySql>) -> anyhow::Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let sql = gen_query_selected_ssc_sql();
    let mut conn = pool.acquire().await?;
    let Ok(query_results) = conn.fetch_all(sql.as_str()).await else { return Ok(vec![]); };
    for query_result in query_results {
        let refno = query_result.get::<String, _>("refno");
        let name = query_result.get::<String, _>("name");
        result.push((refno, name));
    }
    Ok(result)
}

pub async fn query_table_ssc(pool: &Pool<MySql>) -> anyhow::Result<&'static str> {
    let sql = gen_query_table_sql();
    let mut conn = pool.acquire().await?;
    if let Ok(query_results) = conn.fetch_all(sql.as_str()).await {
        if query_results.len() > 0 {
            return Ok("true");
        } else {
           return Ok("false");
        }
    }
    Ok("error")
}


fn gen_query_ssc_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT name,refno FROM AllSscData"));
    sql
}

fn gen_query_selected_ssc_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT name,refno FROM SelectedSscData"));
    sql
}

///查询数据库中是否具有all_ssc_data表
fn gen_query_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SHOW TABLES LIKE 'allsscdata'"));
    sql
}


/// 创建ssc sql
fn create_ssc_sql() -> String {
    format!("CREATE TABLE IF NOT EXISTS AllSscData (
        refno VARCHAR(255) NOT NULL,
        name VARCHAR(255) NOT NULL
    );")
}

/// 创建selected ssc sql
fn create_selected_ssc_sql() -> String {
    format!("CREATE TABLE IF NOT EXISTS SelectedSscData (
        refno VARCHAR(255) NOT NULL,
        name VARCHAR(255) NOT NULL
    );")
}

fn clear_ssc_sql() -> String {
    format!("truncate table AllSscData;")
}

fn clear_selected_ssc_sql() -> String {
    format!("truncate table SelectedSscData;")
}