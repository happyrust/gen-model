//! SesnoRangeResolver — deep module for incremental sesno range detection.
//!
//! One place for: watermark read semantics + nearest-session jump + range build.
//! Callers (init_watcher / async_watch) only supply file identity + file latest sesno.
//!
//! Prefer filtering DB types in the caller's scope gate (`in_scope_with`) and pass
//! `skip_cata=false` from every production call site so the paths cannot diverge
//! (today both callers live in `manual_update.rs`: preview and execute; the
//! consistency is pinned by `production_call_sites_pass_skip_cata_false`).
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

/// 准入裁决（纯函数 [`SesnoRangeResolver::admission`] 的输出）：这次要不要开文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    /// 无事可做：不开文件、不跳会话。
    Skip,
    /// 打开文件跳最近会话；`cold_start` 标记「水位 0 的 SYS-meta 首载」窗口。
    Open { cold_start: bool },
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

    /// 准入真值表（纯函数，两个 resolve 入口共用的唯一裁决）：
    ///
    /// 1. `skip_cata` + CATA → Skip（调用方要求跳过目录库时优先于一切判定；
    ///    入口处另有同义的快速短路，为的是跳过件不打水位查询那一趟 DB）；
    /// 2. 水位 0：DESI/CATA 一律 Skip（不猜历史），SYS meta
    ///    （[`COLD_START_DB_TYPES`]）且文件有会话（`file_latest_sesno > 0`）
    ///    → 冷启动窗；
    /// 3. 水位在位：`file_latest_sesno` 未超过水位 → Skip，否则正常窗。
    fn admission(
        watermark: u32,
        file_latest_sesno: i32,
        db_type: &str,
        skip_cata: bool,
    ) -> Admission {
        if skip_cata && db_type == "CATA" {
            return Admission::Skip;
        }
        if watermark == 0 {
            if !Self::allows_cold_start(db_type) || file_latest_sesno <= 0 {
                return Admission::Skip;
            }
            return Admission::Open { cold_start: true };
        }
        if (file_latest_sesno as u32) <= watermark {
            return Admission::Skip;
        }
        Admission::Open { cold_start: false }
    }

    /// 跳会话的起点（纯函数）：冷启动从 1（取第一个可用会话），正常从水位 + 1。
    fn seek_from(watermark: u32, cold_start: bool) -> i32 {
        if cold_start { 1 } else { watermark as i32 + 1 }
    }

    /// 窗口算术（纯函数）：左端 = 跳出来的最近会话号，右端 = 文件最新会话。
    /// `nearest` 超过右端 → 无窗——文件在预检与打开之间被换过、或会话链里
    /// 已无更新内容，安静放弃，与老形态的双重守卫同一结局。
    fn window(nearest: i32, file_latest_sesno: i32) -> Option<RangeInclusive<i32>> {
        (nearest <= file_latest_sesno).then(|| nearest..=file_latest_sesno)
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
    /// Cheap watermark pre-check first (no file open when nothing to do), then
    /// one open serves both the header read and the nearest-session jump
    /// （2026-08-10 审核 P2-2：此前这里开一次读 header、`resolve_with_header`
    /// 又开一次跳会话号——watcher 热路径每个文件白开一遍）。
    pub async fn resolve(
        &self,
        path: &Path,
        project: &str,
        dbnum: u32,
        file_latest_sesno: i32,
        skip_cata: bool,
        db_type: &str,
    ) -> anyhow::Result<Option<SesnoUpdatePlan>> {
        // 快速短路：被跳过的 CATA 连水位查询那趟 DB 都不打（语义由 admission 兜底）。
        if skip_cata && db_type == "CATA" {
            return Ok(None);
        }

        let db_latest_sesno = Self::query_watermark(dbnum).await?;
        let cold_start =
            match Self::admission(db_latest_sesno, file_latest_sesno, db_type, skip_cata) {
                Admission::Skip => return Ok(None),
                Admission::Open { cold_start } => cold_start,
            };

        let mut io = PdmsIO::new(project, path, true);
        io.open()?;
        let basic_info = io.get_page_basic_info()?;

        Ok(Self::build_plan(
            &mut io,
            path,
            basic_info,
            db_type,
            db_latest_sesno,
            cold_start,
        ))
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
        // 快速短路：同 `resolve`，跳过件不打水位查询。
        if skip_cata && db_type == "CATA" {
            return Ok(None);
        }

        let db_latest_sesno = Self::query_watermark(dbnum).await?;
        let cold_start =
            match Self::admission(db_latest_sesno, file_latest_sesno, db_type, skip_cata) {
                Admission::Skip => return Ok(None),
                Admission::Open { cold_start } => cold_start,
            };

        let mut io = PdmsIO::new(project, path, true);
        io.open()?;
        Ok(Self::build_plan(
            &mut io,
            path,
            basic_info,
            db_type,
            db_latest_sesno,
            cold_start,
        ))
    }

    /// 在**已打开**的文件上跳最近会话号并组装计划。水位、CATA 门与冷启动资格
    /// 都已由 [`Self::admission`] 判定；这里用 header 的 `file_latest_sesno`
    /// 复核右端（[`Self::window`]）——文件在预检与打开之间被换过时，跳出来的
    /// `nearest` 超过右端即安静放弃，与老形态的双重守卫同一结局。
    fn build_plan(
        io: &mut PdmsIO,
        path: &Path,
        basic_info: DbPageBasicInfo,
        db_type: &str,
        db_latest_sesno: u32,
        cold_start: bool,
    ) -> Option<SesnoUpdatePlan> {
        let dbnum = basic_info.pdms_header.db_num as u32;
        let file_latest_sesno = basic_info.latest_ses_data.sesno;

        let seek = Self::seek_from(db_latest_sesno, cold_start);
        let nearest = io.get_nearest_large_sesno(seek).unwrap_or(seek);
        let range = Self::window(nearest, file_latest_sesno)?;

        if cold_start {
            println!(
                "SesnoRangeResolver: {} cold start dbnum={}, range={}..={}",
                db_type, dbnum, nearest, file_latest_sesno
            );
        }

        Some(SesnoUpdatePlan {
            path: path.to_path_buf(),
            basic_info,
            db_type: db_type.to_string(),
            range,
            db_latest_sesno,
            file_latest_sesno,
            cold_start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IU-S2-01/02/03（资格半边）：准入真值表。窗口取错则后面每个阶段都在处理
    /// 错误的数据，这张表是整条增量链路「读哪段」的唯一裁决。
    #[test]
    fn admission_truth_table() {
        use Admission::{Open, Skip};
        let admit = SesnoRangeResolver::admission;

        // IU-S2-01：水位已追平/超过文件 → Skip（等于号也要拦：同号即无新会话）。
        assert_eq!(admit(101, 101, "DESI", false), Skip);
        assert_eq!(admit(101, 100, "DESI", false), Skip);
        // 文件领先水位 → 正常窗。
        assert_eq!(admit(100, 101, "DESI", false), Open { cold_start: false });

        // skip_cata 门：只拦 CATA、优先于一切判定；非 CATA 不受影响。
        assert_eq!(admit(100, 101, "CATA", true), Skip);
        assert_eq!(admit(100, 101, "CATA", false), Open { cold_start: false });
        assert_eq!(admit(100, 101, "DESI", true), Open { cold_start: false });

        // IU-S2-02：水位 0 时 DESI/CATA 一律 Skip——不猜历史，首载走基线不走增量。
        for db_type in ["DESI", "CATA"] {
            assert_eq!(admit(0, 50, db_type, false), Skip, "{db_type} 不许冷启动");
        }

        // IU-S2-03（资格半边）：水位 0 的 SYS meta 允许冷启动，但文件必须真有
        // 会话（file_latest_sesno > 0），空文件/坏 header 不开。
        for db_type in COLD_START_DB_TYPES {
            assert_eq!(
                admit(0, 50, db_type, false),
                Open { cold_start: true },
                "{db_type} 应可冷启动"
            );
            assert_eq!(admit(0, 0, db_type, false), Skip, "{db_type} 空文件");
            assert_eq!(admit(0, -1, db_type, false), Skip, "{db_type} 负号哨兵");
        }
        // 反空转：冷启动名单非空，上面的循环真的跑过。
        assert!(!COLD_START_DB_TYPES.is_empty());
    }

    /// IU-S2-03（窗形半边）/ IU-S2-04：窗口算术与跳会话起点。
    #[test]
    fn window_and_seek_arithmetic() {
        // 冷启动从 1 起跳（取第一个可用会话），正常从水位 + 1。
        assert_eq!(SesnoRangeResolver::seek_from(0, true), 1);
        assert_eq!(SesnoRangeResolver::seek_from(100, false), 101);

        // 正常窗与单会话窗（nearest == latest 是合法的一格窗）。
        assert_eq!(SesnoRangeResolver::window(101, 105), Some(101..=105));
        assert_eq!(SesnoRangeResolver::window(105, 105), Some(105..=105));
        // IU-S2-04：跳出来的会话号超过右端 → 无窗（文件在预检与打开之间被换过，
        // 或链里已无更新内容），冷启动与正常两分支共用这一个裁决点。
        assert_eq!(SesnoRangeResolver::window(106, 105), None);
        assert_eq!(SesnoRangeResolver::window(2, 1), None);
    }

    /// IU-S2-01 的「不开文件」半边（L1 源码钉）：两个 resolve 入口的顺序必须是
    /// 「CATA 快速短路 → 水位查询 → admission 裁决 → 才 PdmsIO::new」。裁决挪到
    /// 开文件之后，Skip 件就白开一遍文件；快速短路挪到水位查询之后，跳过的
    /// CATA 就白打一趟 DB。回退即红。
    #[test]
    fn admission_precedes_file_open_on_both_entry_points() {
        let source = include_str!("sesno_range.rs");
        let entries = [
            (
                concat!("pub async fn ", "resolve("),
                "/// Convenience when caller already has",
            ),
            (
                concat!("pub async fn ", "resolve_with_header("),
                concat!("fn ", "build_plan("),
            ),
        ];
        for (open_marker, close_marker) in entries {
            let body = source
                .split_once(open_marker)
                .unwrap_or_else(|| panic!("入口 {open_marker} 必须存在"))
                .1
                .split_once(close_marker)
                .unwrap_or_else(|| panic!("入口 {open_marker} 之后应有 {close_marker}"))
                .0;
            let cata_at = body
                .find(r#"skip_cata && db_type == "CATA""#)
                .expect("CATA 快速短路缺失");
            let watermark_at = body.find("query_watermark(").expect("水位查询缺失");
            let admission_at = body.find("Self::admission(").expect("准入裁决缺失");
            let open_at = body.find("PdmsIO::new(").expect("开文件缺失");
            assert!(
                cata_at < watermark_at,
                "{open_marker}: 跳过的 CATA 不得打水位查询: {body}"
            );
            assert!(
                watermark_at < admission_at && admission_at < open_at,
                "{open_marker}: 裁决必须先于开文件: {body}"
            );
        }
    }

    /// IU-S2-05（按今日调用面落地）：生产调用点的 `skip_cata` 必须全部为 false
    /// ——CATA 过滤归调用方的 scope 门（`in_scope_with`），不在窗口解析层分叉。
    /// 新增调用点要么保持 false，要么带着理由更新本钉与模块文档。
    #[test]
    fn production_call_sites_pass_skip_cata_false() {
        let source = include_str!("manual_update.rs");
        let needle = concat!("SesnoRangeResolver::new()");
        let mut sites = 0;
        let mut cursor = 0;
        while let Some(offset) = source[cursor..].find(needle) {
            let start = cursor + offset;
            let rest = &source[start..];
            // 400 字节窗口，向后走到字符边界（周边是中文注释，硬切会劈开多字节字符）。
            let mut end = rest.len().min(400);
            while !rest.is_char_boundary(end) {
                end += 1;
            }
            let window = &rest[..end];
            assert!(
                window.contains(".resolve("),
                "SesnoRangeResolver::new() 之后应紧跟 .resolve(: {window}"
            );
            assert!(
                window.contains("false,"),
                "生产调用点必须传 skip_cata=false: {window}"
            );
            sites += 1;
            cursor = start + 1;
        }
        assert_eq!(
            sites, 2,
            "当前应恰有预览与执行两个调用点；数量变化说明有人新增/挪走了窗口解析入口，检查其 skip_cata 取值后更新本钉"
        );
    }
}
