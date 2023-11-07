use once_cell::sync::Lazy;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::Surreal;
// use async_once_cell::Lazy;
use std::future::Future;

pub static SUL_DB: Lazy<Surreal<Client>> = Lazy::new(Surreal::init);

type H = impl Future<Output = Surreal<Client>>;
pub static SUL_DB_ASYNC: async_once_cell::Lazy<Surreal<Client>, H> =
    async_once_cell::Lazy::new(async {
        let client = Surreal::new::<Ws>("localhost:8002").await.unwrap();
        // SUL_DB.use_ns(&db_option.project_code).use_db(&db_option.project_name).await?;
        client
    });
