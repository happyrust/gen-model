//! Initialization-only durable hand-off. The journal, never the current memory
//! projection, owns generated results until the database acknowledges them.
mod journal;
mod runtime;
#[cfg(test)]
mod tests;
pub use journal::{Journal, QueueHealth, TaskState};
pub use runtime::*;

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
pub struct RunManifest {
    pub version: u16,
    pub run_id: String,
    pub dbnum: u32,
    pub source_sesno: u32,
    pub source_token: String,
    #[serde(default)]
    pub database_incarnation: String,
    /// The database registry transaction proved that this dbnum had no model
    /// projection rows before this run was reserved. The proof belongs to the
    /// run and remains true after this run starts publishing rows.
    #[serde(default, skip_serializing_if = "is_false")]
    pub model_projection_empty_at_reservation: bool,
    pub dependency_tokens: Vec<String>,
    #[serde(default)]
    pub room_source_keys: Vec<String>,
    #[serde(default)]
    pub room_context_fingerprint: String,
    pub target_hash: String,
    pub roots: Vec<RootIntent>,
    pub created_ms: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
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

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
