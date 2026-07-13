//! IncrementPipeline — deep module for narrow incremental persist + watermark.
//!
//! Interface: `apply(ranges_map) -> IncrResult`
//! Does NOT own model refresh or MQTT sync (callers consume `IncrResult`).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use aios_core::data_center::DataCenterRecordOperate;
use aios_core::pdms_types::*;
use aios_core::SUL_DB;
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

/// Independent deep module: collect delta → Surreal persist → datacenter meta → watermark by dbnum.
#[derive(Debug, Default, Clone)]
pub struct IncrementPipeline;

impl IncrementPipeline {
    pub fn new() -> Self {
        Self
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
                result.warnings.push(format!(
                    "skip copy file: {}",
                    path.display()
                ));
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
        sesno_range: RangeInclusive<i32>,
        db_type: &str,
    ) -> anyhow::Result<(IncrFileSuccess, Vec<String>)> {
        let mut warnings = Vec::new();
        let end_sesno = *sesno_range.end();
        let dbnum = basic_info.pdms_header.db_num as u32;

        println!(
            "IncrementPipeline: {:?}, db_type={}, sesno range: {:?}",
            path, db_type, &sesno_range
        );

        let mut io = PdmsIO::new("", path.clone(), true);
        io.open()
            .map_err(|e| anyhow::anyhow!("打开 PDMS IO 失败: {}", e))?;

        let range_eles = io.collect_increment_eles(Some(sesno_range))?;
        io.update_elements_to_database(&range_eles, true).await?;

        if let Err(e) = Self::update_datacenter_version(&range_eles).await {
            warnings.push(format!(
                "datacenter_version update failed for {}: {}",
                path.display(),
                e
            ));
        }

        Self::advance_watermark_by_dbnum(dbnum, end_sesno).await?;

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

    /// Advance the dedicated watermark record `dbnum_watermark:{dbnum}`.
    ///
    /// Deliberately does NOT touch `dbnum_info_table`: its per-ref_0 records
    /// (sesno = max within that ref_0 group) are maintained by the `pe` table
    /// events; bulk-raising them here would corrupt that per-group semantic.
    async fn advance_watermark_by_dbnum(dbnum: u32, end_sesno: i32) -> anyhow::Result<()> {
        let sql = format!(
            "UPSERT dbnum_watermark:{} SET dbnum = {}, sesno = math::max([sesno?:0, {}]), updated_at = time::now();",
            dbnum, dbnum, end_sesno
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("推进水位失败 dbnum={}: {}", dbnum, e))?;
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
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init mgr");
        mgr.init_watcher().await.expect("init_watcher");
    }
}
