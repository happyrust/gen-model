use aios_core::SUL_DB;
use futures::StreamExt;
use once_cell::sync::Lazy;
use tokio::sync::Mutex;
use crate::versioned_db::database::SenderSql;

static GLOBAL_SENDER: Lazy<Mutex<Option<flume::Sender<SenderSql>>>> = Lazy::new(|| Mutex::new(None));

pub async fn initialize_global_db_sender() {
    let (sender, receiver) = flume::unbounded();
    *GLOBAL_SENDER.lock().await = Some(sender);
    tokio::spawn(background_save_task(receiver));
    dbg!("Background save task started");
}

pub async fn get_global_db_sender() -> flume::Sender<SenderSql> {
    GLOBAL_SENDER.lock().await.as_ref().expect("Global db sender not initialized").clone()
}

async fn background_save_task(receiver: flume::Receiver<SenderSql>) {
    const CHUNK_SIZE: usize = 1;
    let mut all_handles = vec![];

    for i in 0..10 {
        let receiver = receiver.clone();
        #[cfg(feature = "sql")]
        let pools_clone = pool.clone(); // 假设 pool 是全局可用的

        let insert_handle = tokio::spawn(async move {
            let mut record_stream = receiver.into_stream().chunks(CHUNK_SIZE);
            while let Some(sqls) = record_stream.next().await {
                // println!("thread {i} Imported records: {}", sqls.len());
                for sql in sqls {
                    match sql {
                        SenderSql::SurrealSql(sql) => {
                            if !sql.is_empty() {
                                SUL_DB.query(sql).await.expect("insert db failed");
                            }
                        }
                        #[cfg(feature = "sql")]
                        SenderSql::MysqlSql((project, sql)) => {
                            let Some(pool) = pools_clone.get(&project) else { continue; };
                            let mut conn = pool.acquire().await.expect("get pool failed");
                            match conn.execute(sql.as_str()).await {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(e.to_string());
                                    dbg!(&sql);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
        all_handles.push(insert_handle);
    }
    // 等待所有处理任务完成（实际上这个 join 永远不会结束，因为任务会持续运行）
    futures::future::join_all(all_handles).await;
}