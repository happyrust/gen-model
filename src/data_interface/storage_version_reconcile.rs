//! 模型存储架构版本核对：水位行的 `dbnum_watermark.model_storage_version` ↔ 本进程的
//! `aios_core::model_storage::MODEL_STORAGE_VERSION`。
//!
//! # 它回答什么
//!
//! 「这个库的模型面行形状，本进程读不读得懂」。读不懂的库不该被静默当成「没有数据」，
//! 而应当被点名、要求按当前版本重新生成：
//!
//! ```text
//! POST /api/v1/dbnums/{dbnum}/model/rebuild
//! ```
//!
//! # 与 `generator_reconcile` 的关键差别
//!
//! `generator_reconcile` 发现生成器变了会**自动整库重排**。这里刻意不这么做：
//! 存储架构升级要重算的量级与影响面都远大于换生成器版本，重建时机必须由人挑
//! （低峰、逐库、可回退）。所以本模块只做两件事——启动时点名、`/health` 里报数，
//! 不排任何任务。这与 ptset 分表计划的 D5（不做补扫，老根靠逐库 `model/rebuild`）一致。
//!
//! # 「没有 `pe_ptset` 行」不等于「老库」
//!
//! 判老库只看水位行的这一列：`None`（缺列）= 前版本化。一根管件还没按新版发布过、
//! 或者这件本来就没有 P 点，都会没有 `pe_ptset` 行——那是正常状态，不是老库。

use std::collections::BTreeMap;

use serde::Serialize;

/// 一个落后的库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaggingDbnum {
    pub dbnum: u32,
    /// 库里记的版本；`None` = 缺列 = 前版本化。
    pub stored: Option<u32>,
}

/// 版本核对结果，直接进 `/health`（机器可读，不是散文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageVersionReport {
    /// 本进程期望的版本。
    pub expected: u32,
    /// 还没追平的库，按 dbnum 升序。
    pub lagging: Vec<LaggingDbnum>,
}

impl StorageVersionReport {
    pub fn is_caught_up(&self) -> bool {
        self.lagging.is_empty()
    }

    /// 每个落后库一条可直接复制的重建命令。
    pub fn rebuild_hints(&self) -> Vec<String> {
        self.lagging
            .iter()
            .map(|db| format!("POST /api/v1/dbnums/{}/model/rebuild", db.dbnum))
            .collect()
    }
}

/// 纯函数：哪些库落后。
///
/// 只判盖过模型水位的库（`model_sesno > 0`）：从没生成过的库没有行形状可言，
/// 把它算成「落后」会在每次冷启动刷一堆噪声。
pub(crate) fn lagging_dbnums(
    watermarks: &BTreeMap<u32, (i32, Option<u32>)>,
) -> Vec<LaggingDbnum> {
    watermarks
        .iter()
        .filter(|(_, (model_sesno, stored))| {
            *model_sesno > 0
                && *stored != Some(aios_core::model_storage::MODEL_STORAGE_VERSION)
        })
        .map(|(dbnum, (_, stored))| LaggingDbnum {
            dbnum: *dbnum,
            stored: *stored,
        })
        .collect()
}

/// 读库里的版本列。读失败时返回 `Err`——调用方应当把这一轮当作「没核对成」，
/// 不要据此判断任何库落后（一次读失败不该把每个库都点名）。
pub(crate) async fn stored_versions() -> anyhow::Result<BTreeMap<u32, (i32, Option<u32>)>> {
    #[derive(serde::Deserialize)]
    struct Row {
        dbnum: u32,
        #[serde(default)]
        model_sesno: Option<i32>,
        #[serde(default)]
        model_storage_version: Option<u32>,
    }
    let mut response = aios_core::SUL_DB
        .query(
            "SELECT dbnum, model_sesno, model_storage_version FROM dbnum_watermark \
             WHERE dbnum != NONE AND model_sesno != NONE AND model_sesno > 0;",
        )
        .await?;
    let rows: Vec<Row> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.dbnum,
                (row.model_sesno.unwrap_or_default(), row.model_storage_version),
            )
        })
        .collect())
}

/// 最近一次核对的结果，供同步的 `/health` 读取。
///
/// `model_cache_health` 是同步函数，不能 `await`，所以启动时把结果存这里。
/// 没跑过核对就是 `None`——`/health` 里表现为 `null`，不冒充「已追平」。
static LAST_REPORT: std::sync::OnceLock<StorageVersionReport> = std::sync::OnceLock::new();

/// 同步读最近一次核对结果（给 `/health` 用）。
pub fn last_report() -> Option<&'static StorageVersionReport> {
    LAST_REPORT.get()
}

/// 启动时点名。只打日志，不排任务、不阻断启动。
///
/// 返回报告，方便调用方顺手塞进 `/health` 的初值。
pub(crate) async fn report_and_log() -> anyhow::Result<StorageVersionReport> {
    let watermarks = stored_versions().await?;
    let report = StorageVersionReport {
        expected: aios_core::model_storage::MODEL_STORAGE_VERSION,
        lagging: lagging_dbnums(&watermarks),
    };
    // 存给 `/health`；重复调用只保留第一次。
    let _ = LAST_REPORT.set(report.clone());
    for db in &report.lagging {
        log::warn!(
            "MODEL_STORAGE_VERSION_MISMATCH dbnum={} stored={} expected={} source=dbnum: 该库需 POST /api/v1/dbnums/{}/model/rebuild",
            db.dbnum,
            db.stored.map_or_else(|| "none".to_owned(), |v| v.to_string()),
            report.expected,
            db.dbnum
        );
    }
    if !report.is_caught_up() {
        log::warn!(
            "{} 个库的模型面存储架构版本落后于本进程（expected={}）；未自动重建，按上面的命令逐库 rebuild",
            report.lagging.len(),
            report.expected
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watermarks(rows: &[(u32, i32, Option<u32>)]) -> BTreeMap<u32, (i32, Option<u32>)> {
        rows.iter()
            .map(|(dbnum, sesno, version)| (*dbnum, (*sesno, *version)))
            .collect()
    }

    #[test]
    fn a_missing_column_counts_as_stale() {
        let report = lagging_dbnums(&watermarks(&[(7997, 394, None)]));
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].dbnum, 7997);
        assert_eq!(report[0].stored, None);
    }

    #[test]
    fn the_current_version_is_not_stale() {
        let current = aios_core::model_storage::MODEL_STORAGE_VERSION;
        assert!(lagging_dbnums(&watermarks(&[(7997, 394, Some(current))])).is_empty());
    }

    #[test]
    fn a_newer_version_is_also_stale() {
        // 比本进程新的库：行形状本进程同样读不懂。
        let newer = aios_core::model_storage::MODEL_STORAGE_VERSION + 1;
        assert_eq!(lagging_dbnums(&watermarks(&[(7997, 394, Some(newer))])).len(), 1);
    }

    #[test]
    fn a_dbnum_that_never_generated_is_not_reported() {
        // model_sesno == 0：从没生成过，没有行形状可言，不刷噪声。
        assert!(lagging_dbnums(&watermarks(&[(7997, 0, None)])).is_empty());
    }

    #[test]
    fn hints_are_copy_pasteable() {
        let report = StorageVersionReport {
            expected: 1,
            lagging: vec![
                LaggingDbnum { dbnum: 7997, stored: None },
                LaggingDbnum { dbnum: 7354, stored: Some(0) },
            ],
        };
        assert_eq!(
            report.rebuild_hints(),
            vec![
                "POST /api/v1/dbnums/7997/model/rebuild".to_string(),
                "POST /api/v1/dbnums/7354/model/rebuild".to_string(),
            ]
        );
        assert!(!report.is_caught_up());
    }
}
