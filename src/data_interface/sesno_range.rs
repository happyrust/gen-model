//! SesnoRangeResolver — deep module for incremental sesno range detection.
//!
//! One place for: watermark read semantics + nearest-session jump + range build.
//! Callers (init_watcher / async_watch) only supply file identity + file latest sesno.
//!
//! Prefer filtering DB types in the caller's `should_process_database` and pass
//! `skip_cata=false` from both init and watch so the two paths cannot diverge.
//!
//! Special case: when watermark is 0, DESI/CATA stay skipped (unsafe to guess
//! history). **SYS meta** (`SYST`/`DICT`/`GLB`/`GLOB`) may cold-start: range from
//! the first available sesno through `file_latest_sesno`, so never-parsed config
//! DBs can bootstrap via the same IncrementPipeline path.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::PdmsIO;

/// Meta / config DB types eligible for watermark-0 cold start (aligned with
/// [`crate::data_interface::increment_pipeline::SYS_META_DB_TYPES`]).
pub const COLD_START_DB_TYPES: &[&str] = &["SYST", "DICT", "GLB", "GLOB"];

/// Resolved incremental window for one DB file.
#[derive(Debug, Clone)]
pub struct SesnoUpdatePlan {
    pub path: PathBuf,
    pub basic_info: DbPageBasicInfo,
    /// PDMS db type from file header (`SYST` / `DESI` / …).
    pub db_type: String,
    pub range: RangeInclusive<i32>,
    pub db_latest_sesno: u32,
    pub file_latest_sesno: i32,
    /// `true` when watermark was 0 and this plan is a SYS-meta first-load window.
    pub cold_start: bool,
}

/// Independent module: watermark (dbnum) + nearest sesno → optional update range.
#[derive(Debug, Default, Clone)]
pub struct SesnoRangeResolver;

impl SesnoRangeResolver {
    pub fn new() -> Self {
        Self
    }

    /// SYS meta DBs may cold-start when watermark is absent (never fully parsed).
    #[inline]
    fn allows_cold_start(db_type: &str) -> bool {
        COLD_START_DB_TYPES.contains(&db_type)
    }

    /// Authoritative watermark for this dbnum.
    ///
    /// Delegates to [`DbnumState::applied_sesno`], which reads the single
    /// authoritative `applied_sesno` (with a one-time migration from the legacy
    /// `dbnum_watermark.sesno`, and — only when no dedicated watermark exists —
    /// the max `sesno` in `dbnum_info_table`). Per ADR-001 the running path no
    /// longer takes a cross-table max: `applied_sesno` is the only source.
    pub async fn query_watermark(dbnum: u32) -> anyhow::Result<u32> {
        let applied = crate::data_interface::dbnum_state::DbnumState::applied_sesno(dbnum).await?;
        Ok(applied.max(0) as u32)
    }

    /// Build an update plan when `file_latest_sesno > watermark`, or SYS-meta cold start.
    ///
    /// Cheap watermark pre-check first (no file open when nothing to do),
    /// then delegates to [`Self::resolve_with_header`] for the shared logic.
    pub async fn resolve(
        &self,
        path: &Path,
        project: &str,
        dbnum: u32,
        file_latest_sesno: i32,
        skip_cata: bool,
        db_type: &str,
    ) -> anyhow::Result<Option<SesnoUpdatePlan>> {
        if skip_cata && db_type == "CATA" {
            return Ok(None);
        }

        let db_latest_sesno = Self::query_watermark(dbnum).await?;
        if db_latest_sesno == 0 {
            if !Self::allows_cold_start(db_type) || file_latest_sesno <= 0 {
                return Ok(None);
            }
            // SYS meta cold start: open file and build full-window plan below.
        } else if (file_latest_sesno as u32) <= db_latest_sesno {
            return Ok(None);
        }

        let mut io = PdmsIO::new(project, path, true);
        io.open()?;
        let basic_info = io.get_page_basic_info()?;

        self.resolve_with_header(path, project, basic_info, skip_cata, db_type)
            .await
    }

    /// Convenience when caller already has `DbPageBasicInfo` (watch path).
    pub async fn resolve_with_header(
        &self,
        path: &Path,
        project: &str,
        basic_info: DbPageBasicInfo,
        skip_cata: bool,
        db_type: &str,
    ) -> anyhow::Result<Option<SesnoUpdatePlan>> {
        let dbnum = basic_info.pdms_header.db_num as u32;
        let file_latest_sesno = basic_info.latest_ses_data.sesno;
        if skip_cata && db_type == "CATA" {
            return Ok(None);
        }

        let db_latest_sesno = Self::query_watermark(dbnum).await?;

        // --- SYS meta cold start: no watermark yet, ingest from first sesno ---
        if db_latest_sesno == 0 {
            if !Self::allows_cold_start(db_type) || file_latest_sesno <= 0 {
                return Ok(None);
            }

            let mut io = PdmsIO::new(project, path, true);
            io.open()?;
            let nearest = io.get_nearest_large_sesno(1).unwrap_or(1);
            if nearest > file_latest_sesno {
                return Ok(None);
            }

            println!(
                "SesnoRangeResolver: {} cold start dbnum={}, range={}..={}",
                db_type, dbnum, nearest, file_latest_sesno
            );

            return Ok(Some(SesnoUpdatePlan {
                path: path.to_path_buf(),
                basic_info,
                db_type: db_type.to_string(),
                range: nearest..=file_latest_sesno,
                db_latest_sesno: 0,
                file_latest_sesno,
                cold_start: true,
            }));
        }

        if (file_latest_sesno as u32) <= db_latest_sesno {
            return Ok(None);
        }

        let mut io = PdmsIO::new(project, path, true);
        io.open()?;
        let nearest = io
            .get_nearest_large_sesno(db_latest_sesno as i32 + 1)
            .unwrap_or(db_latest_sesno as i32 + 1);

        if nearest > file_latest_sesno {
            return Ok(None);
        }

        Ok(Some(SesnoUpdatePlan {
            path: path.to_path_buf(),
            basic_info,
            db_type: db_type.to_string(),
            range: nearest..=file_latest_sesno,
            db_latest_sesno,
            file_latest_sesno,
            cold_start: false,
        }))
    }
}
