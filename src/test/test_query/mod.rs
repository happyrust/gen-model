use aios_core::options::DbOption;
use config::{Config, File};
use surrealdb::engine::remote::ws::Ws;

use crate::surreal_service::SUL_DB;

pub mod test_mdb;
pub mod test_query_fuzzy;

pub mod test_query_regex;

pub mod test_basic_query;

pub mod test_query_group;

pub mod transform;


pub async fn init_test_surreal() {
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()
        .unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    SUL_DB
        .connect::<Ws>(db_option.get_version_db_conn_str())
        .with_capacity(1000)
        .await
        .unwrap();
    SUL_DB
        .use_ns(&db_option.project_code)
        .use_db(&db_option.project_name)
        .await
        .unwrap();
}
