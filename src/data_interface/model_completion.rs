//! Durable task completion, including an authoritatively empty model inventory.
//! The receipt lives on the existing spatial file round and commits with its watermark.
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use super::frozen_source::FrozenSource;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompletionReceipt {
    version: u32,
    project: String,
    source_token: String,
    policy: String,
    dependency_tokens: Vec<String>,
    pub empty: bool,
}

fn policy() -> String {
    let mut types: Vec<_> = super::generation_root::configured_delivery_unit_types()
        .into_iter()
        .collect();
    types.sort();
    format!("generation-inventory-v1:{types:?}")
}

pub(crate) fn round_id(dbnum: u32, sesno: i32) -> String {
    format!("incr_side_effect_pending:spatial_reconcile_{dbnum}_{sesno}")
}

/// Split the denormalized dbnum lookup from legacy ownership traversal. Both
/// paths retain their meaning while the latter starts from the PE dbnum index.
pub(crate) fn projection_presence_expr(dbnum: u32) -> String {
    ["inst_relate", "tubi_relate"]
        .into_iter()
        .map(|table| {
            format!(
                "array::len((SELECT VALUE id FROM {table} WHERE dbnum={dbnum} LIMIT 1)) > 0 OR \
         array::len((SELECT VALUE id FROM pe WHERE dbnum={dbnum} AND \
         array::len((SELECT VALUE id FROM {table} WHERE in=$parent.id LIMIT 1)) > 0 LIMIT 1)) > 0"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

impl CompletionReceipt {
    /// Capture before work starts; validation after ACK rejects changed dependencies.
    pub fn capture(token: &str) -> anyhow::Result<Self> {
        let source = FrozenSource::from_commit_token_json(token)?.reopen_verified()?;
        let dependency_tokens = {
            let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
            pins.into_iter()
                .filter(|pin| matches!(pin.db_type.as_str(), "CATA" | "PROP"))
                .map(|pin| FrozenSource::capture(pin.file)?.commit_token_json())
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        source.reopen_verified()?;
        Ok(Self {
            version: 1,
            project: aios_core::get_db_option().project_name.clone(),
            source_token: token.to_owned(),
            policy: policy(),
            dependency_tokens,
            empty: false,
        })
    }

    pub fn source_token(&self) -> &str {
        &self.source_token
    }

    pub fn validate(&self, dbnum: u32, sesno: i32) -> anyhow::Result<()> {
        ensure!(
            self.version == 1 && self.policy == policy(),
            "completion inventory policy changed"
        );
        ensure!(
            self.project == aios_core::get_db_option().project_name,
            "completion project changed"
        );
        let source = FrozenSource::from_commit_token_json(&self.source_token)?;
        ensure!(
            source.dbnum() == dbnum as i32 && source.target_sesno() as i32 == sesno,
            "completion source target mismatch"
        );
        let (_, selected) = super::model_source::source_file_of(dbnum)?;
        ensure!(
            source.matches_path(&selected),
            "completion MDB source changed"
        );
        source.reopen_verified()?;
        let (pins, _) = crate::fast_model::e3d_model_service::current_mdb_sources()?;
        let current_dependencies: std::collections::BTreeSet<_> = pins
            .into_iter()
            .filter(|pin| matches!(pin.db_type.as_str(), "CATA" | "PROP"))
            .map(|pin| pin.dbnum)
            .collect();
        let expected_dependencies = self
            .dependency_tokens
            .iter()
            .map(|token| Ok(FrozenSource::from_commit_token_json(token)?.dbnum()))
            .collect::<anyhow::Result<std::collections::BTreeSet<_>>>()?;
        ensure!(
            current_dependencies == expected_dependencies,
            "completion dependency membership changed"
        );
        for token in &self.dependency_tokens {
            let dependency = FrozenSource::from_commit_token_json(token)?;
            let current = super::model_source::latest_source_version(dependency.dbnum() as u32)?;
            ensure!(
                dependency.matches_path(&current.file)
                    && dependency.target_sesno() == current.sesno,
                "completion dependency changed: {}",
                dependency.dbnum()
            );
            dependency.reopen_verified()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(empty: bool) -> CompletionReceipt {
        CompletionReceipt {
            version: 1,
            project: "test".into(),
            source_token: "frozen".into(),
            policy: policy(),
            dependency_tokens: vec![],
            empty,
        }
    }

    #[tokio::test]
    async fn completion_and_watermark_commit_together_and_reject_changed_evidence() {
        let cases = [
            ("", true, true),
            (
                "UPDATE incr_side_effect_pending:spatial_reconcile_8000_12 SET commit_token='new';",
                true,
                false,
            ),
            (
                "UPDATE incr_side_effect_pending:spatial_reconcile_8000_12 SET status='collecting';",
                true,
                false,
            ),
            (
                "UPDATE incr_side_effect_pending:spatial_reconcile_8000_12 SET room_released=false;",
                true,
                true,
            ),
            ("DELETE dbnum_watermark:8000;", true, false),
            (
                "CREATE model_update_pending:x SET dbnum=8000, action='delete_cleanup';",
                true,
                false,
            ),
            ("CREATE inst_relate:x SET dbnum=8000;", true, false),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Generated', source_end_sesno=12;",
                false,
                true,
            ),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Generated', source_end_sesno=11;",
                false,
                false,
            ),
            (
                "CREATE gen_root:x SET dbnum=8000, status='Failed';",
                false,
                false,
            ),
        ];
        for (change, empty, success) in cases {
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("completion").use_db("atomic").await.unwrap();
            db.query(format!("CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=0; \
                CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET dbnum=8000, end_sesno=12, \
                status='done', room_released=true, commit_token='frozen'; {change}"))
                .await.unwrap().check().unwrap();
            let result = commit_on(&db, 8000, 12, "frozen", &receipt(empty), None).await;
            assert_eq!(result.is_ok(), success, "{change}: {result:?}");
            let mut observed = db
                .query(
                    "RETURN dbnum_watermark:8000.model_sesno; \
                RETURN incr_side_effect_pending:spatial_reconcile_8000_12.model_completion;",
                )
                .await
                .unwrap()
                .check()
                .unwrap();
            let watermark: Option<i32> = observed.take(0).unwrap();
            let completion: Option<String> = observed.take(1).unwrap();
            assert_eq!(watermark == Some(12), success, "{change}");
            assert_eq!(completion.is_some(), success, "{change}");
        }
    }

    #[test]
    fn malformed_and_unversioned_receipts_never_count_as_completed() {
        for raw in ["{}", "null", "legacy", "{\"version\":99}"] {
            assert!(validated_receipt(Some(raw), 8000, 12).is_none());
        }
        assert!(validated_receipt(None, 8000, 12).is_none());
    }

    #[tokio::test]
    async fn model_checkpoint_preserves_unfinished_rooms() {
        use super::super::side_effect_pending::SideEffectCompensator;
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("completion")
            .use_db("independent_stages")
            .await
            .unwrap();
        db.query(
            "CREATE dbnum_watermark:8000 SET applied_sesno=12, model_sesno=10,
            room_sesno=8, room_input_version='previous-room-proof';
            CREATE incr_side_effect_pending:spatial_reconcile_8000_12 SET dbnum=8000, end_sesno=12,
            status='done', room_released=false, commit_token='frozen';
            CREATE model_update_pending:room SET dbnum=8000, action='room_recalc_element',
            target_refno='8000/1', attempts=9999, status='pending';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let proof = SideEffectCompensator::published_model_file_round_on(&db, 8000, 12)
            .await
            .unwrap();
        assert_eq!(proof.commit_token.as_deref(), Some("frozen"));
        assert!(
            SideEffectCompensator::completed_file_round_on(&db, 8000, 12)
                .await
                .is_err()
        );
        let probe = super::super::model_update_pending::read_model_watermark_probe(&db, 8000)
            .await
            .unwrap();
        assert_eq!(
            probe.abandoned, 0,
            "room failures are not abandoned model roots"
        );
        commit_on(&db, 8000, 12, "frozen", &receipt(true), None)
            .await
            .unwrap();
        let mut result = db
            .query(
                "RETURN dbnum_watermark:8000.model_sesno;
            RETURN dbnum_watermark:8000.room_sesno;
            RETURN dbnum_watermark:8000.room_input_version;
            RETURN incr_side_effect_pending:spatial_reconcile_8000_12.room_released;
            RETURN model_update_pending:room.attempts;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(result.take::<Option<i32>>(0).unwrap(), Some(12));
        assert_eq!(result.take::<Option<i32>>(1).unwrap(), Some(8));
        assert_eq!(
            result.take::<Option<String>>(2).unwrap().as_deref(),
            Some("previous-room-proof")
        );
        assert_eq!(result.take::<Option<bool>>(3).unwrap(), Some(false));
        assert_eq!(result.take::<Option<i32>>(4).unwrap(), Some(9999));
        db.query("UPDATE incr_side_effect_pending:spatial_reconcile_8000_12 SET status='pending';")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            SideEffectCompensator::published_model_file_round_on(&db, 8000, 12)
                .await
                .is_err()
        );
    }
}

pub(crate) fn validated_receipt(
    raw: Option<&str>,
    dbnum: u32,
    sesno: i32,
) -> Option<CompletionReceipt> {
    let receipt: CompletionReceipt = serde_json::from_str(raw?).ok()?;
    receipt.validate(dbnum, sesno).ok()?;
    Some(receipt)
}

/// Every guard is rechecked in the commit transaction. A changed file round,
/// outstanding model action or missing watermark aborts both writes.
pub(crate) async fn commit_on(
    db: &Surreal<Any>,
    dbnum: u32,
    sesno: i32,
    token: &str,
    receipt: &CompletionReceipt,
    time: Option<&str>,
) -> anyhow::Result<()> {
    let id = round_id(dbnum, sesno);
    let token = serde_json::to_string(token)?;
    let encoded = serde_json::to_string(&serde_json::to_string(receipt)?)?;
    let blocking = super::model_update_pending::blocking_root_predicate();
    let settled = super::model_update_pending::settled_root_predicate();
    let root_guard = if receipt.empty {
        format!(
            "array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) = 0 \
            AND NOT ({})",
            projection_presence_expr(dbnum)
        )
    } else {
        format!(
            "array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} AND \
            ({blocking} OR NOT ({settled}) OR source_end_sesno < {sesno}) LIMIT 1)) = 0 \
            AND array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) > 0"
        )
    };
    let advance =
        super::model_update_pending::render_model_watermark_advance(dbnum, sesno, time, 0, sesno);
    db.query(format!(
        "BEGIN TRANSACTION; \
        IF NOT ({id}.status = 'done' AND {id}.commit_token = {token}) \
            {{ THROW 'completion file round changed'; }}; \
        IF NOT (dbnum_watermark:{dbnum}.applied_sesno >= {sesno}) \
            {{ THROW 'completion data watermark missing or behind'; }}; \
        IF NOT ({root_guard}) {{ THROW 'completion root evidence changed'; }}; \
        IF array::len((SELECT VALUE id FROM model_update_pending WHERE dbnum={dbnum} \
            AND action NOT IN ['room_recalc_element','room_recalc_panel'] LIMIT 1)) > 0 \
            {{ THROW 'completion still has model work'; }}; \
        UPDATE {id} SET model_completion = {encoded}; {advance} COMMIT TRANSACTION;"
    ))
    .await
    .context("commit model completion")?
    .check()
    .context("check model completion transaction")?;
    super::dbnum_consistency::invalidate_census();
    Ok(())
}
