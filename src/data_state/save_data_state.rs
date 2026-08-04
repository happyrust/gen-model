#[cfg(feature = "sql")]
use aios_core::data_state::RefnoStatusInfo;
#[cfg(feature = "sql")]
use sqlx::{Executor, MySql, Pool};

#[cfg(feature = "sql")]
pub async fn save_data_state(data: Vec<RefnoStatusInfo>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let create_table_sql = create_data_state_table_sql();
    let mut conn = pool.clone().acquire().await?;

    let create_table_result = conn.execute(create_table_sql.as_str()).await;
    let Ok(_) = create_table_result else {
        return Ok(());
    };

    let insert_value_sql = gen_insert_data_state_sql(data);
    let _ = conn.execute(insert_value_sql.as_str()).await;

    Ok(())
}

#[cfg(feature = "sql")]
fn create_data_state_table_sql() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS data_status(
        refno VARCHAR(255) NOT NULL,
        status VARCHAR(255) NOT NULL,
        user VARCHAR(255) NOT NULL,
        time VARCHAR(255) NOT NULL,
        note VARCHAR(255) NOT NULL
    );"
    )
}

#[cfg(feature = "sql")]
fn gen_insert_data_state_sql(data: Vec<RefnoStatusInfo>) -> String {
    let mut insert_sql =
        String::from("INSERT IGNORE INTO data_status (refno, status,user,time,note) VALUES ");
    for i in data {
        insert_sql.push_str(&format!(
            "( '{}', '{}','{}','{}','{}' ) ,",
            i.refno, i.status, i.user, i.time, i.note
        ));
    }
    insert_sql.remove(insert_sql.len() - 1);
    insert_sql
}
