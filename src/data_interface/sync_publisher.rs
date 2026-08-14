//! SyncPublisher — deep module for post-increment remote file sync.
//!
//! Compresses updated DB files to `.cba`, dedups via `e3d_sync`, publishes MQTT.
//! Stays outside IncrementPipeline (narrow persist boundary).

use std::path::PathBuf;
use std::sync::Arc;

use aios_core::{SUL_DB, get_db_option};
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

/// e3d_sync 去重查询（纯渲染）。
///
/// 三个插值都是外部字符串——`location` 是自由文本配置，文件名/哈希虽分别受
/// 候选白名单与十六进制约束，但进入 SurrealQL 单引号字面量的外部字符串必须
/// 统一过 `escape_surql_str`（宪法「转义」条，2026-08-13 审计 P2）：一个带
/// 引号或反斜杠的值会破坏字面量，让本次去重查询失败、文件被当作查询错误跳过。
fn render_dedup_query(location: &str, file_name: &str, file_hash: &str) -> String {
    use crate::data_interface::dbnum_state::escape_surql_str;
    format!(
        "select value <string>\
        id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
        escape_surql_str(location),
        escape_surql_str(file_name),
        escape_surql_str(file_hash)
    )
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
        self.publish_files(
            incr.successes
                .iter()
                .map(|success| (&success.path, success.dbnum)),
        )
        .await
    }

    /// Publish one already-applied file without fabricating an
    /// [`IncrFileSuccess`](crate::data_interface::increment_pipeline::IncrFileSuccess).
    /// Batch workers use this after their result DTO has already been assembled.
    pub async fn publish_file(&self, path: &PathBuf, dbnum: u32) -> SyncOutcome {
        self.publish_files(std::iter::once((path, dbnum))).await
    }

    async fn publish_files<'a>(
        &self,
        files: impl IntoIterator<Item = (&'a PathBuf, u32)>,
    ) -> SyncOutcome {
        let mut outcome = SyncOutcome::default();

        let mut notify_file_names = Vec::new();
        let mut notify_file_hashes = Vec::new();

        for (path, dbno) in files {
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

            let sql = render_dedup_query(get_db_option().location.as_str(), &file_name, &file_hash);

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
                outcome.errors.push(format!("写入 e3d_sync 失败: {}", e));
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
            outcome.skipped.extend(notify_file_names.iter().cloned());
        }

        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// e3d_sync 去重查询的三个插值都是外部字符串，必须过 escape_surql_str
    /// （2026-08-13 审计 P2）：带引号/反斜杠的值会破坏单引号字面量。回退成
    /// 裸插值时本用例必红。
    #[test]
    fn the_dedup_query_escapes_every_external_string() {
        let sql = render_dedup_query(r"loc'A\B", "ams1112_0001", "hash'X");
        assert!(
            sql.contains(r"location != 'loc\'A\\B'"),
            "location 必须转义: {sql}"
        );
        assert!(sql.contains("'ams1112_0001' in file_names"), "{sql}");
        assert!(
            sql.contains(r"'hash\'X' in file_hashes"),
            "hash 必须转义: {sql}"
        );
        assert!(
            !sql.contains(r"'loc'A\B'"),
            "裸插值的单引号会破坏字面量: {sql}"
        );
    }
}
