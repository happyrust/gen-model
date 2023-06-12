use aios_core::options::DbOption;
use crate::graph_db::pdms_arango::{ArDatabase, connect_arangodb};

pub async fn get_arangodb_conn_from_db_option(o: &DbOption) -> anyhow::Result<ArDatabase>{
    let pool = connect_arangodb(o).await?;
    let d = pool.get().await?;
    Ok(d.db(o.arangodb_database.as_str()).await?)
}