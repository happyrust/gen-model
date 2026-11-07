//! Durable hand-off between the file/memory data plane and the optional mirror.
//!
//! A job contains already-rendered SurrealQL.  Replay never re-opens an E3D
//! file, so a newer file session cannot silently change an older queued job.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use surrealdb::{Surreal, engine::any::Any};

pub const FORMAT_VERSION: u16 = 2;
pub const DEFAULT_CAPACITY_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const EXTENSION: &str = "mirror-job";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MirrorJob {
    pub format_version: u16,
    pub content_hash: String,
    pub source_epoch: u64,
    pub source_sessions: Vec<(u32, u32)>,
    pub target: String,
    pub created_unix_ms: u64,
    /// Complete idempotent transaction strings, in commit order.
    pub transactions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpoolHealth {
    pub depth: usize,
    pub bytes: u64,
    pub oldest_age_ms: Option<u64>,
    pub capacity_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct DurableJob {
    pub path: PathBuf,
    pub job: MirrorJob,
}

#[derive(Debug, Clone)]
pub struct MirrorSpool {
    root: PathBuf,
    capacity_bytes: u64,
}

impl MirrorSpool {
    pub fn for_process(namespace: &str, project: &str, mdb: &str) -> anyhow::Result<Self> {
        let run_dir = std::env::var_os("GEN_MODEL_RUN_DIR")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir().context("resolve process run directory")?);
        Self::open(
            run_dir
                .join(".gen-model")
                .join("mirror-spool")
                .join(component(namespace))
                .join(component(project))
                .join(component(mdb)),
            DEFAULT_CAPACITY_BYTES,
        )
    }

    pub fn open(root: PathBuf, capacity_bytes: u64) -> anyhow::Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("create mirror spool {}", root.display()))?;
        Ok(Self {
            root,
            capacity_bytes,
        })
    }

    pub fn persist(
        &self,
        source_epoch: u64,
        source_sessions: Vec<(u32, u32)>,
        target: impl Into<String>,
        transactions: Vec<String>,
    ) -> anyhow::Result<DurableJob> {
        anyhow::ensure!(!transactions.is_empty(), "mirror job has no transactions");
        let target = target.into();
        let content_hash = content_hash(source_epoch, &source_sessions, &target, &transactions)?;
        // The hash is the stable job identity. Attribute reads and ensure
        // retries may enqueue the same frozen rows repeatedly; one durable copy
        // is sufficient and keeps the capacity gate meaningful.
        let pending = self.pending()?;
        if let Some(existing) = pending
            .iter()
            .find(|entry| entry.job.content_hash == content_hash)
        {
            return Ok(existing.clone());
        }
        let created_unix_ms = now_ms();
        let job = MirrorJob {
            format_version: FORMAT_VERSION,
            content_hash: content_hash.clone(),
            source_epoch,
            source_sessions,
            target,
            created_unix_ms,
            transactions,
        };
        let bytes = bincode::serialize(&job).context("encode mirror spool job")?;
        let current_bytes = pending.iter().try_fold(0u64, |sum, entry| {
            Ok::<_, std::io::Error>(sum.saturating_add(fs::metadata(&entry.path)?.len()))
        })?;
        anyhow::ensure!(
            current_bytes.saturating_add(bytes.len() as u64) <= self.capacity_bytes,
            "mirror spool capacity exceeded: {} + {} > {} bytes",
            current_bytes,
            bytes.len(),
            self.capacity_bytes
        );
        let stem = format!(
            "{source_epoch:020}-{created_unix_ms:020}-{}",
            &content_hash[..16]
        );
        let final_path = self.root.join(format!("{stem}.{EXTENSION}"));
        if final_path.exists() {
            return Ok(DurableJob {
                path: final_path,
                job,
            });
        }
        let temp_path = self
            .root
            .join(format!(".{stem}.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .with_context(|| format!("create mirror spool temp {}", temp_path.display()))?;
        if let Err(error) = (|| -> anyhow::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp_path, &final_path)?;
            sync_directory_best_effort(&self.root);
            Ok(())
        })() {
            let _ = fs::remove_file(&temp_path);
            return Err(error).with_context(|| format!("commit mirror spool job {stem}"));
        }
        Ok(DurableJob {
            path: final_path,
            job,
        })
    }

    pub fn pending(&self) -> anyhow::Result<Vec<DurableJob>> {
        let mut jobs = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some(EXTENSION) {
                continue;
            }
            let bytes = fs::read(&path)
                .with_context(|| format!("read mirror spool job {}", path.display()))?;
            let job: MirrorJob = bincode::deserialize(&bytes)
                .with_context(|| format!("decode mirror spool job {}", path.display()))?;
            anyhow::ensure!(
                job.format_version == 1 || job.format_version == FORMAT_VERSION,
                "unsupported mirror spool format {} in {}",
                job.format_version,
                path.display()
            );
            anyhow::ensure!(
                job.content_hash
                    == content_hash(
                        job.source_epoch,
                        &job.source_sessions,
                        &job.target,
                        &job.transactions
                    )?,
                "mirror spool checksum mismatch in {}",
                path.display()
            );
            jobs.push(DurableJob { path, job });
        }
        jobs.sort_by_key(|entry| {
            (
                entry.job.source_epoch,
                entry.job.created_unix_ms,
                entry.path.clone(),
            )
        });
        Ok(jobs)
    }

    pub async fn replay(&self, db: &Surreal<Any>) -> anyhow::Result<usize> {
        let mut committed = 0usize;
        for pending in self.pending()? {
            anyhow::ensure!(
                pending.job.format_version == FORMAT_VERSION || pending.job.target != "element",
                "legacy element mirror job {} has no explicit PE statistics; drain it with the previous version before upgrading (job retained)",
                pending.path.display()
            );
            for transaction in &pending.job.transactions {
                let result = if pending.job.target == "element" {
                    super::pe_batch::execute_on(db, transaction).await
                } else {
                    crate::surreal_retry::execute_surreal_checked_on(
                        db,
                        transaction,
                        "mirror model transaction",
                    )
                    .await
                };
                result.with_context(|| format!("commit mirror job {}", pending.path.display()))?;
            }
            fs::remove_file(&pending.path)
                .with_context(|| format!("ack mirror job {}", pending.path.display()))?;
            sync_directory_best_effort(&self.root);
            committed += 1;
        }
        Ok(committed)
    }

    pub fn health(&self) -> anyhow::Result<SpoolHealth> {
        let pending = self.pending()?;
        let bytes = pending.iter().try_fold(0u64, |sum, entry| {
            Ok::<_, std::io::Error>(sum.saturating_add(fs::metadata(&entry.path)?.len()))
        })?;
        let oldest = pending.iter().map(|entry| entry.job.created_unix_ms).min();
        Ok(SpoolHealth {
            depth: pending.len(),
            bytes,
            oldest_age_ms: oldest.map(|created| now_ms().saturating_sub(created)),
            capacity_bytes: self.capacity_bytes,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn content_hash(
    source_epoch: u64,
    source_sessions: &[(u32, u32)],
    target: &str,
    transactions: &[String],
) -> anyhow::Result<String> {
    let canonical = bincode::serialize(&(
        FORMAT_VERSION,
        source_epoch,
        source_sessions,
        target,
        transactions,
    ))?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

fn component(raw: &str) -> String {
    let value = raw.trim().trim_start_matches('/');
    let clean = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if clean.is_empty() {
        "default".into()
    } else {
        clean
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn sync_directory_best_effort(path: &Path) {
    if let Ok(dir) = File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "gen-model-spool-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ))
    }

    #[test]
    fn persists_exact_rendered_transactions_and_recovers_in_order() {
        let root = temp_root("recover");
        let spool = MirrorSpool::open(root.clone(), 1024 * 1024).unwrap();
        spool
            .persist(
                2,
                vec![(8000, 7)],
                "pe",
                vec!["BEGIN TRANSACTION;\nUPSERT pe:2;\nCOMMIT TRANSACTION;\n".into()],
            )
            .unwrap();
        spool
            .persist(
                1,
                vec![(8000, 6)],
                "pe",
                vec!["BEGIN TRANSACTION;\nUPSERT pe:1;\nCOMMIT TRANSACTION;\n".into()],
            )
            .unwrap();
        let pending = spool.pending().unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|item| item.job.source_epoch)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(spool.health().unwrap().depth, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capacity_failure_does_not_leave_a_visible_job() {
        let root = temp_root("capacity");
        let spool = MirrorSpool::open(root.clone(), 1).unwrap();
        assert!(
            spool
                .persist(1, vec![], "model", vec!["too large".into()])
                .is_err()
        );
        assert_eq!(spool.health().unwrap().depth, 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn replay_after_commit_before_ack_does_not_double_count() {
        let root = temp_root("statistics");
        let spool = MirrorSpool::open(root.clone(), 1024 * 1024).unwrap();
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("stats").use_db("spool").await.unwrap();
        let sql = crate::data_interface::pe_batch::transaction(
            7,
            &["40_1".into()],
            "UPSERT pe:40_1 SET dbnum=7, sesno=2",
        )
        .unwrap();
        spool
            .persist(1, vec![(7, 2)], "element", vec![sql.clone()])
            .unwrap();
        crate::data_interface::pe_batch::execute_on(&db, &sql)
            .await
            .unwrap();
        assert_eq!(spool.replay(&db).await.unwrap(), 1);
        assert_eq!(spool.replay(&db).await.unwrap(), 0);
        // 重放收敛到同一行；Ref0 统计已退役（ADR-071 D6），统计表无行。
        let mut result = db
            .query("SELECT count() AS n FROM pe WHERE dbnum = 7 GROUP ALL; RETURN dbnum_info_table:40.count;")
            .await
            .unwrap();
        #[derive(serde::Deserialize)]
        struct Count {
            n: i64,
        }
        let rows: Vec<Count> = result.take(0).unwrap();
        assert_eq!(rows[0].n, 1);
        assert_eq!(result.take::<Option<i64>>(1).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_same_frozen_job_is_deduplicated_by_content_hash() {
        let root = temp_root("dedupe");
        let spool = MirrorSpool::open(root.clone(), 1024 * 1024).unwrap();
        let tx = vec!["BEGIN TRANSACTION;\nRETURN NONE;\nCOMMIT TRANSACTION;\n".into()];
        let first = spool
            .persist(7, vec![(8000, 7)], "element", tx.clone())
            .unwrap();
        let second = spool.persist(7, vec![(8000, 7)], "element", tx).unwrap();
        assert_eq!(first.path, second.path);
        assert_eq!(spool.health().unwrap().depth, 1);
        fs::remove_dir_all(root).unwrap();
    }
}
