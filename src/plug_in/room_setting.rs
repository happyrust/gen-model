use aios_core::ssc_setting::SiteData;
use sqlx::{Executor, MySql, Pool, Row};


pub async fn update_selected_room_ssc_site(sites: (Vec<SiteData>, Vec<SiteData>), pool: &Pool<MySql>) -> anyhow::Result<()> {
    //若没有selected_site,则创建表
    let create_table_sql = create_selected_room_site_sql();
    let mut conn = pool.clone();
    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else { return Ok(()); };

    let add_site = sites.0;
    let delete_site = sites.1;

    let insert_value_sql = gen_insert_selected_room_site_sql(add_site);
    let _ = conn.execute(insert_value_sql.as_str()).await;

    let delete_value_sql = delete_selected_room_site_sql(delete_site);
    let _ = conn.execute(delete_value_sql.as_str()).await;

    Ok(())
}

fn create_selected_room_site_sql() -> String {
    format!("CREATE TABLE IF NOT EXISTS room_site(
        refno VARCHAR(255) NOT NULL,
        name VARCHAR(255) NOT NULL,
        UNIQUE (refno)
    );")
}


fn gen_insert_selected_room_site_sql(sites: Vec<SiteData>) -> String {
    let mut insert_sql = String::from("INSERT IGNORE INTO room_site (refno, name) VALUES ");
    for site in sites {
        insert_sql.push_str(&format!("( '{}', '{}' ) ,", site.refno, site.name));
    }
    insert_sql.remove(insert_sql.len() - 1);
    insert_sql
}

fn delete_selected_room_site_sql(sites: Vec<SiteData>) -> String {
    let mut delete_sql = String::new();
    for site in sites {
        delete_sql.push_str(&format!("DELETE FROM room_site WHERE refno ='{}';", site.refno));
    }
    delete_sql
}


pub async fn query_room_site_table(pool: &Pool<MySql>) -> anyhow::Result<&'static str> {
    let sql = gen_query_table_sql();
    let mut conn = pool;
    if let Ok(query_results) = conn.fetch_all(sql.as_str()).await {
        if query_results.len() > 0 {
            return Ok("true");
        } else {
            return Ok("false");
        }
    }
    Ok("error")
}

fn gen_query_table_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SHOW TABLES LIKE 'room_site'"));
    sql
}

pub async fn query_selected_room_site(pool: &Pool<MySql>) -> anyhow::Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    let sql = gen_query_selected_room_site_sql();
    let mut conn = pool;
    let Ok(query_results) = conn.fetch_all(sql.as_str()).await else { return Ok(vec![]); };
    for query_result in query_results {
        let refno = query_result.get::<String, _>("refno");
        let name = query_result.get::<String, _>("name");
        result.push((refno, name));
    }
    Ok(result)
}

fn gen_query_selected_room_site_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT name,refno FROM room_site"));
    sql
}
