//! SyncPublisher — deep module for post-increment remote file sync.
//!
//! Compresses updated DB files to `.cba`, dedups via `e3d_sync`, publishes MQTT.
//! Stays outside IncrementPipeline (narrow persist boundary).

use std::path::PathBuf;
use std::sync::Arc;

use aios_core::{get_db_option, SUL_DB};
use pdms_io::sync::compress::{CompressOptions, execute_compress};
use rumqttc::{AsyncClient, QoS};

use crate::data_interface::increment_pipeline::IncrResult;
use crate::mqtt_service::SyncE3dFileMsg;

/// Outcome of one publish attempt.
#[derive(Debug, Default, Clone)]
pub struct SyncOutcome {
    pub published: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

/// Independent module: archive + dedup + MQTT notify.
#[derive(Clone)]
pub struct SyncPublisher {
    mqtt_client: Arc<AsyncClient>,
}

impl SyncPublisher {
    pub fn new(mqtt_client: Arc<AsyncClient>) -> Self {
        Self { mqtt_client }
    }

    /// Ensure a single `.cba` archive exists/refreshed for `path` (used by init too).
    pub async fn ensure_archive(path: &PathBuf) -> anyhow::Result<String> {
        tokio::fs::create_dir_all("assets/archives").await?;
        tokio::fs::create_dir_all("assets/temp").await?;
        let file_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("bad path: {}", path.display()))?;
        let output: PathBuf = format!("assets/archives/{}.cba", file_name).into();
        let compress_opt = CompressOptions::new(path.clone(), output, "assets/temp");
        let hash = execute_compress(compress_opt).await?;
        Ok(hash.to_string())
    }

    /// After a successful incremental apply: compress each success file and MQTT-notify if new.
    pub async fn publish(&self, incr: &IncrResult) -> SyncOutcome {
        let mut outcome = SyncOutcome::default();

        if !incr.had_work() {
            return outcome;
        }

        let mut notify_file_names = Vec::new();
        let mut notify_file_hashes = Vec::new();

        for success in &incr.successes {
            let path = &success.path;
            if path.is_dir() {
                continue;
            }
            let file_name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(n) => n.to_owned(),
                None => {
                    outcome
                        .errors
                        .push(format!("无法获取文件名: {}", path.display()));
                    continue;
                }
            };
            let dbno = success.dbnum;

            println!("SyncPublisher: 处理同步 {}", file_name);

            let file_hash = match Self::ensure_archive(path).await {
                Ok(h) => h,
                Err(e) => {
                    outcome
                        .errors
                        .push(format!("压缩失败 {}: {}", file_name, e));
                    continue;
                }
            };

            if let Some(location_dbs) = &get_db_option().location_dbs {
                if !location_dbs.contains(&dbno) {
                    println!("数据库编号 {} 不在地区配置中，跳过推送", dbno);
                    outcome.skipped.push(file_name);
                    continue;
                }
            }

            let sql = format!(
                "select value <string>\
                id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                get_db_option().location.as_str(),
                file_name,
                &file_hash
            );

            let id = match SUL_DB.query(&sql).await {
                Ok(mut response) => response.take::<Vec<String>>(0).unwrap_or_default(),
                Err(e) => {
                    outcome
                        .errors
                        .push(format!("e3d_sync 查询失败 {}: {}", file_name, e));
                    continue;
                }
            };

            if id.is_empty() {
                println!("检测到增量更新，准备推送文件: {}", &file_name);
                notify_file_hashes.push(file_hash);
                notify_file_names.push(file_name);
            } else {
                println!("文件 {} 的哈希已存在，跳过推送", file_name);
                outcome.skipped.push(file_name);
            }
        }

        dbg!(&notify_file_names);

        #[cfg(feature = "mqtt")]
        if !notify_file_names.is_empty() {
            let published = notify_file_names.clone();
            let payload = SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);

            if let Err(e) = SUL_DB
                .query(format!(
                    "INSERT IGNORE INTO e3d_sync {} ",
                    serde_json::to_string(&payload).unwrap_or_default()
                ))
                .await
            {
                outcome
                    .errors
                    .push(format!("写入 e3d_sync 失败: {}", e));
            }

            match self
                .mqtt_client
                .clone()
                .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                .await
            {
                Ok(_) => outcome.published.extend(published),
                Err(e) => outcome.errors.push(format!("MQTT 发布失败: {}", e)),
            }
        }

        #[cfg(not(feature = "mqtt"))]
        {
            let _ = (&notify_file_names, &notify_file_hashes, &self.mqtt_client);
            outcome
                .skipped
                .extend(notify_file_names.iter().cloned());
        }

        outcome
    }
}
