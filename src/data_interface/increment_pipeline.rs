//! IncrementPipeline — deep module for narrow incremental persist + watermark.
//!
//! Interface: `apply(ranges_map) -> IncrResult`
//! Does NOT own model refresh or MQTT sync (callers consume `IncrResult`).

use std::collections::{BTreeMap, HashSet};
use std::ops::RangeInclusive;
use std::path::PathBuf;

use aios_core::data_center::DataCenterRecordOperate;
use aios_core::pdms_types::*;
use aios_core::{RefnoEnum, SUL_DB, clear_all_caches};
use indexmap::IndexMap;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};

use crate::data_interface::sesno_range::COLD_START_DB_TYPES;

const DATACENTER_VERSION: &str = "datacenter_version";

/// Meta / config DB types: persist + watermark only; no geometry model refresh.
/// Same set as cold-start eligibility ([`COLD_START_DB_TYPES`]).
pub const SYS_META_DB_TYPES: &[&str] = COLD_START_DB_TYPES;

/// One file that completed Surreal persist + watermark advance.
#[derive(Debug, Clone)]
pub struct IncrFileSuccess {
    pub path: PathBuf,
    pub dbnum: u32,
    pub end_sesno: i32,
    /// PDMS db type (`SYST` / `DESI` / …) for downstream side-effects.
    pub db_type: String,
    /// Changed element refnos for downstream model refresh.
    pub changed_refnos: Vec<RefU64>,
    /// Full delta payload (MySQL / classified refresh). Kept for callers that need detail.
    pub range_eles: BTreeMap<u32, Vec<EleOperationData>>,
}

/// One file that failed before watermark advance.
#[derive(Debug, Clone)]
pub struct IncrFileError {
    pub path: PathBuf,
    pub error: String,
}

/// Result of [`IncrementPipeline::apply`]. Per-file isolation: failures do not stop siblings.
#[derive(Debug, Default, Clone)]
pub struct IncrResult {
    pub successes: Vec<IncrFileSuccess>,
    pub errors: Vec<IncrFileError>,
    /// Non-fatal side-channel issues (MySQL skipped here; datacenter warnings, etc.).
    pub warnings: Vec<String>,
}

impl IncrResult {
    pub fn all_changed_refnos(&self) -> Vec<RefU64> {
        self.successes
            .iter()
            .flat_map(|s| s.changed_refnos.iter().copied())
            .collect()
    }

    /// Refnos from successes that are not SYS meta DBs (eligible for mesh refresh).
    pub fn geometry_changed_refnos(&self) -> Vec<RefU64> {
        self.successes
            .iter()
            .filter(|s| !SYS_META_DB_TYPES.contains(&s.db_type.as_str()))
            .flat_map(|s| s.changed_refnos.iter().copied())
            .collect()
    }

    pub fn had_work(&self) -> bool {
        !self.successes.is_empty()
    }

    pub fn has_db_type(&self, db_type: &str) -> bool {
        self.successes.iter().any(|s| s.db_type == db_type)
    }
}

/// Wrap rendered SurrealQL statements into a single atomic transaction so a
/// per-file incremental persist is all-or-nothing (ADR-001: the applied
/// watermark must never advance on a partially-applied batch). Returns `None`
/// when there is nothing to run. Statements keep the original `;\n` separator
/// (SurrealDB tolerates the resulting empty statements).
fn wrap_in_transaction(statements: &[String]) -> Option<String> {
    if statements.is_empty() {
        return None;
    }
    Some(format!(
        "BEGIN TRANSACTION;\n{};\nCOMMIT TRANSACTION;",
        statements.join(";\n")
    ))
}

/// Independent deep module: collect delta → Surreal persist → datacenter meta → watermark by dbnum.
#[derive(Debug, Default, Clone)]
pub struct IncrementPipeline;

fn validate_prepared_attempt(
    attempt: &crate::data_interface::model_update_pending::IncrementUpdateAttempt,
    db_type: &str,
    file_path: &str,
    current_file_latest_sesno: i32,
) -> anyhow::Result<()> {
    if attempt.db_type != db_type || attempt.file_path != file_path {
        anyhow::bail!(
            "unfinished increment attempt dbnum={} belongs to type={} path={}, \
             current type={db_type} path={file_path}",
            attempt.dbnum,
            attempt.db_type,
            attempt.file_path
        );
    }
    if attempt.end_sesno > current_file_latest_sesno {
        anyhow::bail!(
            "unfinished increment attempt dbnum={} requires sesno {}..={}, \
             but current file only covers through {current_file_latest_sesno}; \
             file rollback/replacement is blocked",
            attempt.dbnum,
            attempt.start_sesno,
            attempt.end_sesno,
        );
    }
    Ok(())
}

impl IncrementPipeline {
    pub fn new() -> Self {
        Self
    }

    /// Side-effect-free change collection for one file over a sesno range.
    ///
    /// Opens the E3D file and returns the per-`sesno` element operations WITHOUT
    /// persisting anything (no `pe` writes, no datacenter meta, no watermark
    /// advance). Shared by the apply path ([`Self::apply_one`]) and the read-only
    /// manual-update preview so the two cannot diverge.
    pub fn collect_changes(
        path: &std::path::Path,
        sesno_range: RangeInclusive<i32>,
    ) -> anyhow::Result<BTreeMap<u32, Vec<EleOperationData>>> {
        let mut io = PdmsIO::new("", path.to_path_buf(), true);
        io.open()
            .map_err(|e| anyhow::anyhow!("打开 PDMS IO 失败: {}", e))?;
        let range_eles = io.collect_increment_eles(Some(sesno_range))?;
        Ok(range_eles)
    }

    /// Apply incremental updates for the given sesno ranges.
    ///
    /// Map value: `(basic_info, sesno_range, db_type)`.
    ///
    /// - Skips copy files whose name contains `-`
    /// - On per-file failure: records error, continues
    /// - Watermark advances only after Surreal persist succeeds for that file
    /// - Watermark key is **dbnum** (dedicated `dbnum_watermark:{dbnum}` record)
    pub async fn apply(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>, String)>,
    ) -> IncrResult {
        let mut result = IncrResult::default();

        for (path, (basic_info, sesno_range, db_type)) in increment_ranges_map {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();

            if file_name.contains('-') {
                result
                    .warnings
                    .push(format!("skip copy file: {}", path.display()));
                continue;
            }

            match self
                .apply_one(&path, &basic_info, sesno_range, &db_type)
                .await
            {
                Ok((success, warnings)) => {
                    result.warnings.extend(warnings);
                    result.successes.push(success);
                }
                Err(e) => {
                    result.errors.push(IncrFileError {
                        path,
                        error: e.to_string(),
                    });
                }
            }
        }

        result
    }

    async fn apply_one(
        &self,
        path: &PathBuf,
        basic_info: &DbPageBasicInfo,
        requested_range: RangeInclusive<i32>,
        db_type: &str,
    ) -> anyhow::Result<(IncrFileSuccess, Vec<String>)> {
        let mut warnings = Vec::new();
        let dbnum = basic_info.pdms_header.db_num as u32;
        let path_text = path.to_string_lossy().into_owned();

        // A crash may leave PE chunks partially applied while the watermark is
        // intentionally unchanged. In that case the pre-update OWNER graph is
        // no longer trustworthy, so reuse the durable fixed range + model plan
        // prepared before the first write.
        let prepared = crate::data_interface::model_update_pending::load_attempt(dbnum).await?;
        let (sesno_range, model_plan, collected) = if let Some(attempt) = prepared {
            validate_prepared_attempt(&attempt, db_type, &path_text, *requested_range.end())?;
            warnings.push(format!(
                "dbnum={dbnum}: replay unfinished range {}..={} after an interrupted persist",
                attempt.start_sesno, attempt.end_sesno
            ));
            (attempt.start_sesno..=attempt.end_sesno, attempt.plan, None)
        } else {
            let range_eles = Self::collect_changes(path, requested_range.clone())?;
            let end_sesno = *requested_range.end();
            let model_plan = crate::data_interface::model_update_plan::build_model_update_plan(
                dbnum,
                end_sesno,
                db_type,
                &range_eles,
            )
            .await;
            crate::data_interface::model_update_pending::prepare_attempt(
                &crate::data_interface::model_update_pending::IncrementUpdateAttempt {
                    dbnum,
                    db_type: db_type.to_string(),
                    file_path: path_text,
                    start_sesno: *requested_range.start(),
                    end_sesno,
                    plan: model_plan.clone(),
                },
            )
            .await?;
            (requested_range, model_plan, Some(range_eles))
        };
        let end_sesno = *sesno_range.end();

        println!(
            "IncrementPipeline: {:?}, db_type={}, sesno range: {:?}",
            path, db_type, &sesno_range
        );

        // Recovery recollects the durable fixed range; a fresh attempt reuses
        // the collection that produced its pre-update model plan.
        let range_eles = match collected {
            Some(range_eles) => range_eles,
            None => Self::collect_changes(path, sesno_range)?,
        };
        let cache_refnos = Self::collect_cache_invalidation_refnos(&range_eles);
        warnings.extend(model_plan.warnings.iter().cloned());

        // 只保留最新数据：仅写入 pe 主数据（最新状态），不再写 sessions / element_changes 历史表
        //
        // Cache invalidation must run after every attempted persist, including a
        // partially failed batch: earlier Surreal statements may already have
        // changed data even though the watermark must remain unchanged.
        let persist_result = Self::persist_latest_main_data(&range_eles, dbnum as i32).await;
        let invalidated = Self::invalidate_caches(cache_refnos).await;
        if invalidated > 0 {
            println!(
                "IncrementPipeline: invalidated {invalidated} PE/attribute cache entries \
                 and world-transform caches"
            );
        }
        persist_result?;

        // ADR-003 B1-emit: 维护反向引用索引（非致命，绝不阻塞数据批次 / 水位推进）。
        // 写失败只记 warning：缺一条引用边最多漏一次级联，靠后续触及 / 全量重建自愈。
        if let Err(e) = Self::maintain_reverse_index(&range_eles).await {
            warnings.push(format!(
                "reverse-index maintain (non-fatal) {}: {}",
                path.display(),
                e
            ));
        }

        if let Err(e) = Self::update_datacenter_version(&range_eles).await {
            warnings.push(format!(
                "datacenter_version update failed for {}: {}",
                path.display(),
                e
            ));
        }

        // One final transaction establishes durable model work, advances the
        // watermark and removes the short-lived recovery record. If it fails,
        // the attempt remains and the whole fixed range is safe to replay.
        crate::data_interface::model_update_pending::finalize_attempt(
            dbnum,
            end_sesno,
            &model_plan,
        )
        .await?;

        let changed_refnos = range_eles
            .values()
            .flat_map(|vec| vec.iter())
            .map(|p| p.refno)
            .collect::<Vec<RefU64>>();

        Ok((
            IncrFileSuccess {
                path: path.clone(),
                dbnum,
                end_sesno,
                db_type: db_type.to_string(),
                changed_refnos,
                range_eles,
            },
            warnings,
        ))
    }

    /// Collect the cache keys whose database-backed values may change.
    ///
    /// Besides each changed element, include its current owner and both sides of
    /// an explicit OWNER move. This invalidates parent hierarchy/attribute reads
    /// used by the subsequent model refresh. The global world-transform caches
    /// are cleared by [`aios_core::clear_all_caches`] as part of each invalidation.
    fn collect_cache_invalidation_refnos(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> HashSet<RefnoEnum> {
        use crate::data_interface::model_impact::changed_owner_refnos;

        let mut refnos = HashSet::new();
        for operation in range_eles.values().flatten() {
            if matches!(&operation.detail, EleOperationDetail::None) {
                continue;
            }

            let changed = RefnoEnum::from(operation.refno);
            if changed.is_valid() {
                refnos.insert(changed);
            }

            refnos.extend(changed_owner_refnos(operation));

            let current_owner = match &operation.detail {
                EleOperationDetail::Add(element) => Some(RefnoEnum::from(element.owner)),
                EleOperationDetail::Modified(element) => {
                    Some(RefnoEnum::from(element.current_data.owner))
                }
                EleOperationDetail::Deleted | EleOperationDetail::None => None,
            };
            if let Some(owner) = current_owner.filter(|owner| owner.is_valid()) {
                refnos.insert(owner);
            }
        }
        refnos
    }

    /// Clear database-backed aios-core caches before any post-persist consumer
    /// (model refresh, transform update, preview, etc.) can read stale values.
    async fn invalidate_caches(refnos: HashSet<RefnoEnum>) -> usize {
        let count = refnos.len();
        for refno in refnos {
            clear_all_caches(refno).await;
        }
        count
    }

    /// Persist ONLY the latest main data (pe + attributes) for this delta.
    ///
    /// Deliberately skips the history/version tables (`sessions` /
    /// `element_changes`): we keep only the latest state, no historical
    /// versions. Mirrors step 5 of the old `update_elements_to_database`,
    /// batching `EleOperationData::to_surql` in groups of 100.
    ///
    /// Any batch write failure is propagated (ADR-001): the caller must NOT
    /// advance the watermark unless the whole batch persisted. Swallowing errors
    /// here would let `applied_sesno` run ahead of the data actually stored.
    async fn persist_latest_main_data(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
        dbnum: i32,
    ) -> anyhow::Result<()> {
        // 收集本文件本窗口的全部落库语句，作为「一个事务」原子提交：要么整体成功、
        // 要么整体回滚，绝不留下半写状态。这样 ADR-001「失败批次不推进水位、按同一
        // 窗口重试」才安全——重试永远从干净状态开始；配合 Add 改用幂等 UPSERT，
        // 彻底消除「上次半写 + 本次重试撞已存在记录反复失败 → dbnum 水位卡死」。
        let mut statements: Vec<String> = Vec::new();
        for (&sesno, elements) in range_eles {
            for element in elements {
                let id = element.refno.to_string();
                let surql = element.to_surql(&id, dbnum, sesno);
                if !surql.is_empty() {
                    statements.push(surql);
                }
            }
        }

        let total = statements.len();
        // 分块事务提交：原实现把整窗口拼成「单个事务」，大型系统库（如 amssys 冷启动
        // 168 会话 ~4000+ 元素）会撑爆 SurrealDB ws 通道上限，报「receiving from an
        // empty and closed channel」而整体失败。改为按 TX_CHUNK 条语句一块、每块自身
        // 原子提交：配合幂等 UPSERT 与「失败不推进水位、按同一窗口重试」，重试仍从可
        // 收敛状态开始，不会半写卡死。语句顺序保持不变，跨块引用与单事务同样是前向依赖。
        const TX_CHUNK: usize = 500;
        for chunk in statements.chunks(TX_CHUNK) {
            if let Some(tx_sql) = wrap_in_transaction(chunk) {
                // `.check()`：把事务内被取消/失败的语句错误上浮为 Err。原实现只 map_err
                // 传输错误、未 check 语句级错误，事务被取消时仍可能返回 Ok → 水位误推进。
                SUL_DB
                    .query(&tx_sql)
                    .await
                    .map_err(|e| anyhow::anyhow!("增量主数据落库失败(事务提交): {e}"))?
                    .check()
                    .map_err(|e| anyhow::anyhow!("增量主数据落库失败(事务内语句): {e}"))?;
            }
        }

        println!(
            "增量主数据落库完成，共 {total} 条（分块事务提交 chunk={TX_CHUNK}，仅最新状态，不写历史）"
        );
        Ok(())
    }

    async fn update_datacenter_version(
        data: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> anyhow::Result<()> {
        let unit = ["SUPPO", "BRAN", "EQUI", "ZONE"];
        for (_, data) in data {
            for d in data {
                match &d.detail {
                    EleOperationDetail::Deleted => {
                        let sql = format!(
                            "let $pe = {};\
                             let $belong_zone = if $pe.noun == 'BRAN' {{ $pe.owner.owner }} else {{ $pe.owner }};\
                             update type::thing('{}',$pe) set status = '{:?}',belong_zone = $belong_zone;",
                            d.refno.to_pe_key(),
                            DATACENTER_VERSION,
                            DataCenterRecordOperate::Delete
                        );
                        if let Err(e) = SUL_DB.query(&sql).await {
                            eprintln!("datacenter delete warn: {e}; sql={sql}");
                        }
                    }
                    EleOperationDetail::Modified(modify_data) => {
                        let sql = if unit.contains(&modify_data.noun.as_str()) {
                            format!(
                                "update {} set status = '{:?}'",
                                d.refno.to_table_key(DATACENTER_VERSION),
                                DataCenterRecordOperate::Modify
                            )
                        } else {
                            let unit_str = unit
                                .iter()
                                .map(|u| format!("'{u}'"))
                                .collect::<Vec<_>>()
                                .join(",");
                            format!(
                                "let $pe = fn::find_ancestor_types({},[{}])[0];\
                                 update type::thing('{}',$pe) set status = '{:?}';",
                                d.refno.to_pe_key(),
                                unit_str,
                                DATACENTER_VERSION,
                                DataCenterRecordOperate::Modify
                            )
                        };
                        if let Err(e) = SUL_DB.query(&sql).await {
                            eprintln!("datacenter modify warn: {e}; sql={sql}");
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// ADR-003 B1-emit: maintain the reverse-reference index (`ref_rev`) for this
    /// window. BEST-EFFORT / non-fatal by contract: the caller swallows the Err
    /// into a warning so a failure here can NEVER block the data batch or the
    /// applied watermark. Not wrapped in the main-data transaction; transport
    /// and statement errors are still surfaced as warnings so a stale index is
    /// never reported as successfully maintained.
    /// Statements are rendered by the pure
    /// [`crate::data_interface::manual_update::build_reverse_index_statements`].
    async fn maintain_reverse_index(
        range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    ) -> anyhow::Result<()> {
        let statements =
            crate::data_interface::manual_update::build_reverse_index_statements(range_eles);
        if statements.is_empty() {
            return Ok(());
        }
        const CHUNK: usize = 500;
        for chunk in statements.chunks(CHUNK) {
            let sql = chunk.join("\n");
            SUL_DB
                .query(&sql)
                .await
                .map_err(|e| anyhow::anyhow!("反向引用索引维护失败(非致命): {e}"))?
                .check()
                .map_err(|e| anyhow::anyhow!("反向引用索引语句失败(非致命): {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn cache_targets_are_deduped_and_none_operations_are_skipped() {
        let changed = RefU64((1_u64 << 32) | 42);
        let ignored = RefU64((1_u64 << 32) | 99);
        let mut range_eles = BTreeMap::new();
        range_eles.insert(
            1,
            vec![
                EleOperationData::new(changed, 1, EleOperationDetail::Deleted),
                EleOperationData::new(changed, 1, EleOperationDetail::Deleted),
                EleOperationData::new(ignored, 1, EleOperationDetail::None),
            ],
        );

        let targets = IncrementPipeline::collect_cache_invalidation_refnos(&range_eles);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains(&RefnoEnum::from(changed)));
        assert!(!targets.contains(&RefnoEnum::from(ignored)));
    }

    #[test]
    fn wrap_in_transaction_is_atomic_or_none() {
        assert_eq!(wrap_in_transaction(&[]), None);

        let sql = wrap_in_transaction(&[
            "UPSERT a:1 CONTENT {}".to_string(),
            "UPDATE pe:1 SET x = 1".to_string(),
        ])
        .expect("non-empty statements must wrap");

        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with(";\nCOMMIT TRANSACTION;"), "{sql}");
        // Both statements are inside the same transaction body.
        assert!(
            sql.contains("UPSERT a:1 CONTENT {};\nUPDATE pe:1 SET x = 1"),
            "{sql}"
        );
    }

    #[test]
    fn prepared_attempt_rejects_a_file_that_no_longer_covers_fixed_range() {
        let attempt = crate::data_interface::model_update_pending::IncrementUpdateAttempt {
            dbnum: 8191,
            db_type: "DESI".to_string(),
            file_path: "D:/project/desi".to_string(),
            start_sesno: 40,
            end_sesno: 42,
            plan: Default::default(),
        };
        let error = validate_prepared_attempt(&attempt, "DESI", "D:/project/desi", 41)
            .expect_err("rollback must be rejected");
        assert!(error.to_string().contains("only covers through 41"));
        validate_prepared_attempt(&attempt, "DESI", "D:/project/desi", 42)
            .expect("complete fixed range is replayable");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::data_interface::tidb_manager::AiosDBManager;

    /// Manual: requires local Surreal `ws://127.0.0.1:8009` + E3D project files.
    /// Example: lower `dbnum_watermark:8191` then
    /// `cargo test -p aios-database force_init_watcher_incr_once -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "manual live incr against local Surreal/E3D"]
    async fn force_init_watcher_incr_once() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let mgr = AiosDBManager::init_form_config().await.expect("init mgr");
        mgr.init_watcher().await.expect("init_watcher");
    }
}
