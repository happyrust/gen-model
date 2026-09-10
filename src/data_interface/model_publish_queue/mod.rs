//! 只剩根发布信封（`FrozenRootEnvelope` / `PreparedRootPublication` / `RootIntent` /
//! `render_spatial_fragment`）：`model_db_adapter` 的信封分支与 `persist_root_for_file_round`
//! 还引用着，随文件轮次那一刀（ADR-071 S3 后半）一起删，删完这个目录就没了。
//!
//! 曾经住在这里的**初始化发布 run**（journal / 消费者 / ACK / 收口 / `/health.model_publish_queue`
//! / replay / resume）已于 2026-09-10 整体退役（ADR-071 S5，共识 d-300）：初始化与增量都是一个
//! dbnum 任务，任务自己同步落库，任务与库之间没有 journal、队列或 ACK。数据窗口的内存产物与
//! 同步落库口（`IncrementWindowArtifact` / `land_increment_window`）同日搬去
//! `data_interface::window_landing`，摘要 `digest` 搬去 `data_interface::digest`（S7）。
use crate::data_interface::digest::digest;
use crate::fast_model::model_memory_store::ProjectionCommit;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u16 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootIntent {
    pub root: String,
    pub revision: u64,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrozenRootEnvelope {
    pub version: u16,
    pub job_id: String,
    pub run_id: String,
    pub dbnum: u32,
    pub source_sesno: u32,
    #[serde(default)]
    pub source_token: String,
    #[serde(default)]
    pub database_incarnation: String,
    pub intent: RootIntent,
    pub target_hash: String,
    pub projection: ProjectionCommit,
    pub created_ms: u64,
}

/// Exact SQL/effect boundaries captured before the first database mutation.
/// Replaying this plan never re-derives deletions from already changed rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedRootPublication {
    pub version: u16,
    pub envelope_hash: String,
    pub chunks: Vec<String>,
    #[serde(default)]
    pub memory_removals: Vec<String>,
    #[serde(default)]
    pub memory_tube_branch_resets: Vec<String>,
    pub upserted: usize,
    pub removed: usize,
    pub receipts: usize,
}
impl PreparedRootPublication {
    pub fn hash(&self) -> anyhow::Result<String> {
        Ok(digest(&serde_json::to_vec(self)?))
    }
}

impl FrozenRootEnvelope {
    /// Validate the complete publication scope before any database lookup or write.
    pub fn validate(&self) -> anyhow::Result<()> {
        use crate::fast_model::model_memory_store::packed_refno;
        use anyhow::{Context, ensure};
        let root = crate::fast_model::e3d_model_service::parse_refno(&self.intent.root)?;
        ensure!(
            self.version == FORMAT_VERSION,
            "unsupported frozen envelope version"
        );
        ensure!(!self.source_token.is_empty(), "missing frozen source token");
        ensure!(
            !self.database_incarnation.is_empty(),
            "missing database incarnation"
        );
        let (receipt_root, receipt) = self
            .projection
            .receipt
            .as_ref()
            .context("missing frozen root receipt")?;
        ensure!(
            *receipt_root == root,
            "frozen receipt belongs to a different root"
        );
        ensure!(
            receipt.source_sesno == self.source_sesno,
            "frozen receipt session mismatch"
        );
        let target = receipt
            .model_target
            .as_ref()
            .context("missing frozen model target")?;
        ensure!(
            target.design.dbnum == self.dbnum && target.design.session == self.source_sesno,
            "frozen model target source mismatch"
        );
        ensure!(
            digest(&serde_json::to_vec(&receipt.model_target)?) == self.target_hash,
            "frozen model target hash mismatch"
        );
        ensure!(
            receipt.geometry_count == self.projection.upserts.len(),
            "frozen receipt geometry count mismatch"
        );
        // Initial generation has no old database state. Deletions are captured
        // separately in the consumer's durable prepared publication plan.
        ensure!(
            self.projection.removals.is_empty(),
            "initial envelope contains unfrozen database removals"
        );
        let packed = packed_refno(root);
        for record in &self.projection.upserts {
            ensure!(
                record.dbnum == self.dbnum && record.source_sesno == self.source_sesno,
                "frozen geometry source mismatch"
            );
            ensure!(
                record.ancestors.contains(&packed),
                "frozen geometry outside root publication scope"
            );
        }
        for branch in &self.projection.tube_branch_resets {
            ensure!(
                *branch == root
                    || self
                        .projection
                        .upserts
                        .iter()
                        .any(|g| g.source_refno == *branch && g.ancestors.contains(&packed)),
                "tube reset outside frozen root scope"
            );
        }
        Ok(())
    }
}

/// 初始化 run 时代的空间碎片行（`model_publish_run` / `model_publish_fragment`）。只剩
/// `model_db_adapter` 的信封分支（`frozen: Some(_)`，生产已无人走到）引用；随文件轮次那一刀
/// 与 `FrozenRootEnvelope` 一起删，两张表由 S6 的一次性 `REMOVE TABLE IF EXISTS` 收尾。
pub(crate) fn render_spatial_fragment(
    e: &FrozenRootEnvelope,
    refresh: &[String],
    removed: &[String],
) -> anyhow::Result<String> {
    let run = serde_json::to_string(&e.run_id)?;
    let refresh = serde_json::to_string(refresh)?;
    let removed = serde_json::to_string(removed)?;
    let hash = serde_json::to_string(&e.target_hash)?;
    Ok(format!(
        "UPSERT type::thing('model_publish_run',{run}) SET next_sequence=(next_sequence ?? 0)+1;\n\
        LET $fragment_sequence = (SELECT VALUE next_sequence FROM type::thing('model_publish_run',{run}))[0];\n\
        CREATE type::thing('model_publish_fragment',[{run},$fragment_sequence]) SET sequence=$fragment_sequence, \
        run_id={run}, target_hash={hash}, refresh={refresh}, removed={removed};\n"
    ))
}
