//! 数据库架构版本核对：库里的 `storage_version:current` ↔ 本进程的
//! `aios_core::model_storage::MODEL_STORAGE_VERSION`。
//!
//! # 它回答什么
//!
//! 「这**整个数据库**的架构，本进程读不读得懂」。读不懂的库不该被静默当成「没有数据」，
//! 而应当被点名、提示用 aios-database 一键升级：
//!
//! ```text
//! POST /api/v1/storage-version/upgrade
//! ```
//!
//! # 只有一条记录，没有逐库的副本
//!
//! 行形状是整个数据库共用的一份约定，不是某张表、也不是某个 dbnum 的属性，所以版本只有
//! 一处来源：`storage_version:current`。它要么在（且 `version` 等于本进程常量），要么不在——
//! 不存在「有的库追平了、有的没有」这种可读状态。逐库进度看升级账
//! （`storage_upgrade:current`），不看版本。
//!
//! # 只在启动时核对
//!
//! 运行期不对任何表、任何查询做版本判断；读点集没有行就是 NONE / 空集。
//!
//! # 与 `generator_reconcile` 的关键差别
//!
//! `generator_reconcile` 发现生成器变了会**自动整库重排**。这里刻意不这么做：
//! 架构升级会清空并重新解析全部旧数据，影响面远大于换生成器版本，升级时机必须由人挑。
//! 所以本模块只做两件事——启动时点名、`/health` 里报数，不排任何任务；排任务由
//! [`crate::data_interface::storage_upgrade`] 的升级入口负责。
//!
//! # 「没有 `pe_ptset` 行」不等于「老库」
//!
//! 判老库只看那一条记录：没有它 = 前版本化。一根管件还没按新版发布过、
//! 或者这件本来就没有 P 点，都会没有 `pe_ptset` 行——那是正常状态，不是老库。

use serde::Serialize;

/// 版本核对结果，直接进 `/health`（机器可读，不是散文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StorageVersionReport {
    /// 本进程期望的版本。
    pub expected: u32,
    /// 库里记录的版本；`None` = 没有 `storage_version:current` 这条记录 = 老库。
    pub stored: Option<u32>,
    /// 库里那条记录写的行形状说明（人读，原样带出来）。
    pub shape: Option<String>,
    /// 库里那条记录写的升级要求（人读，原样带出来）。
    pub rebuild: Option<String>,
}

impl StorageVersionReport {
    /// 这份库读不读得懂。
    pub fn is_caught_up(&self) -> bool {
        self.stored == Some(self.expected)
    }

    /// 可以直接发给运维的升级命令。已追平则空。
    ///
    /// 版本是整库一条记录，升级入口也只有一条；逐库重建（`model/rebuild`）不再是
    /// 版本升级手段，它只重算模型、不清旧数据。
    pub fn upgrade_hints(&self) -> Vec<String> {
        if self.is_caught_up() {
            return Vec::new();
        }
        vec!["POST /api/v1/storage-version/upgrade".to_owned()]
    }

    /// 纯函数：把库里读到的三格与期望版本拼成报告。
    pub(crate) fn from_stored(stored: Option<(u32, Option<String>, Option<String>)>) -> Self {
        let expected = aios_core::model_storage::MODEL_STORAGE_VERSION;
        match stored {
            None => Self {
                expected,
                stored: None,
                shape: None,
                rebuild: None,
            },
            Some((version, shape, rebuild)) => Self {
                expected,
                stored: Some(version),
                shape,
                rebuild,
            },
        }
    }
}

/// 库里那条记录的三格。读失败时返回 `Err`——调用方应当把这一轮当作「没核对成」，
/// 不要据此判断库落后（一次读失败不该把库点名成老库）。
pub(crate) async fn stored_version() -> anyhow::Result<Option<(u32, Option<String>, Option<String>)>>
{
    stored_version_on(&aios_core::SUL_DB).await
}

/// [`stored_version`] 的指定库版本，供内存库用例复用同一条 SQL。
async fn stored_version_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
) -> anyhow::Result<Option<(u32, Option<String>, Option<String>)>> {
    #[derive(serde::Deserialize)]
    struct Row {
        #[serde(default)]
        version: Option<u32>,
        #[serde(default)]
        shape: Option<String>,
        #[serde(default)]
        rebuild: Option<String>,
    }
    let mut response = db
        // `SELECT *` 而不是 `SELECT a AS a, b AS b`：`rebuild` 在 SurrealDB 2.x 里是
        // 保留字（`REBUILD` 语句关键字），`rebuild AS rebuild` 会被解析器拒成
        // `Parse error: Unexpected token 'AS', expected a rebuild statement keyword`。
        // 写成 `SELECT *` 后三格原名返回，`Row` 的字段名与之逐字对应，不必起别名。
        .query(format!(
            "SELECT * FROM ONLY {};",
            aios_core::model_storage::STORAGE_VERSION_RECORD,
        ))
        .await?;
    let row: Option<Row> = response.take(0)?;
    // 记录存在但 `version` 那格缺失 = 一份读不懂的记录，按「没有版本」处理，
    // 这样它会被判成老库而不是被当成追平。
    Ok(row.and_then(|row| row.version.map(|version| (version, row.shape, row.rebuild))))
}

/// 最近一次核对的结果，供同步的 `/health` 读取。
///
/// `model_cache_health` 是同步函数，不能 `await`，所以核对结果存这里，由 [`refresh_if_stale`]
/// 在 `/health` 入口处按需刷新。没跑过核对就是 `None`——`/health` 里表现为 `null`，
/// 不冒充「已追平」。
///
/// 这里**不能**用 `OnceLock`：重建完成后版本记录才写进库，一次性的缓存会让 `/health`
/// 一直报 `caught_up: false` 直到进程重启，而「重建有没有生效」恰恰是运维要当场看见的那件事。
static LAST_REPORT: std::sync::RwLock<Option<StorageVersionReport>> = std::sync::RwLock::new(None);

fn report_lock() -> std::sync::RwLockReadGuard<'static, Option<StorageVersionReport>> {
    LAST_REPORT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn store_report(report: StorageVersionReport) {
    *LAST_REPORT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(report);
}

/// 同步读最近一次核对结果（给 `/health` 用）。
pub fn last_report() -> Option<StorageVersionReport> {
    report_lock().clone()
}

/// 要不要再去库里核一次。
///
/// 已追平就不必再问：版本号只会随代码升级前进，运行期不会倒退，一份已追平的库
/// 不会自己变回老库。没核对过（`None`）或未追平则每次都问——那正是「刚重建完，
/// 等它转绿」的那段时间。
pub(crate) fn needs_refresh(last: Option<&StorageVersionReport>) -> bool {
    last.is_none_or(|report| !report.is_caught_up())
}

/// `/health` 入口处调用：未追平时重读一次库里的版本记录。
///
/// 读失败保留旧值——一次查询失败不该把库改判成老库，也不该把已经追平的状态抹掉。
/// 只有「从没追平变成追平」这一种转变打一条 info（重建生效了，值得留一行痕迹）；
/// 其余情况由启动时的 [`report_and_log`] 负责点名，这里不重复打 warn。
pub async fn refresh_if_stale() -> Option<StorageVersionReport> {
    let previous = last_report();
    if !needs_refresh(previous.as_ref()) {
        return previous;
    }
    let Ok(stored) = stored_version().await else {
        // 读库失败：保留上一次的结论，不把这一轮当成「库落后」。
        return previous;
    };
    let report = StorageVersionReport::from_stored(stored);
    let caught_up = report.is_caught_up();
    store_report(report.clone());
    if caught_up && previous.as_ref().is_some_and(|old| !old.is_caught_up()) {
        log::info!(
            "模型面存储架构版本已追平 stored={} expected={}",
            report.expected,
            report.expected
        );
    }
    Some(report)
}

/// 启动时点名。只打日志，不排任务、不阻断启动。
///
/// `dbnums` 是这次启动会碰的库，只用于把升级会影响到的库列给人看。
/// 返回报告，方便调用方顺手塞进 `/health` 的初值。
pub(crate) async fn report_and_log(dbnums: &[u32]) -> anyhow::Result<StorageVersionReport> {
    let report = StorageVersionReport::from_stored(stored_version().await?);
    // 存给 `/health`；重复调用覆盖（升级完成后要能看见新值）。
    store_report(report.clone());
    if !report.is_caught_up() {
        log::warn!(
            "MODEL_STORAGE_VERSION_MISMATCH stored={} expected={} source=version: 数据库架构版本与本进程不一致",
            report
                .stored
                .map_or_else(|| "none".to_owned(), |v| v.to_string()),
            report.expected
        );
        if let Some(rebuild) = &report.rebuild {
            log::warn!("库里记的升级要求：{rebuild}");
        }
        for hint in report.upgrade_hints() {
            log::warn!("{hint}");
        }
        log::warn!("升级会清空并重新解析、生成以下库：{:?}", dbnums);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Option<(u32, Option<String>, Option<String>)> {
        Some((
            aios_core::model_storage::MODEL_STORAGE_VERSION,
            Some("shape".to_owned()),
            Some("rebuild".to_owned()),
        ))
    }

    #[test]
    fn a_missing_record_counts_as_stale() {
        let report = StorageVersionReport::from_stored(None);
        assert!(!report.is_caught_up());
        assert_eq!(report.stored, None);
    }

    #[test]
    fn the_current_version_is_not_stale() {
        assert!(StorageVersionReport::from_stored(current()).is_caught_up());
    }

    #[test]
    fn a_newer_version_is_also_stale() {
        // 比本进程新的库：行形状本进程同样读不懂。
        let newer = aios_core::model_storage::MODEL_STORAGE_VERSION + 1;
        let report = StorageVersionReport::from_stored(Some((newer, None, None)));
        assert!(!report.is_caught_up());
        assert_eq!(report.stored, Some(newer));
    }

    #[test]
    fn hints_are_copy_pasteable_and_only_when_stale() {
        let stale = StorageVersionReport::from_stored(None);
        assert_eq!(
            stale.upgrade_hints(),
            vec!["POST /api/v1/storage-version/upgrade".to_string()]
        );
        let caught_up = StorageVersionReport::from_stored(current());
        assert!(
            caught_up.upgrade_hints().is_empty(),
            "已追平不该再产出升级命令"
        );
    }

    #[test]
    fn the_record_notes_are_carried_through_for_humans() {
        let report = StorageVersionReport::from_stored(current());
        assert_eq!(report.shape.as_deref(), Some("shape"));
        assert_eq!(report.rebuild.as_deref(), Some("rebuild"));
    }

    /// `/health` 该不该再去库里问一次。    ///
    /// 已追平不再问（版本号只前进，运行期不会退回老库）；没核对过、未追平都要问——
    /// 「刚重建完、等它转绿」的那段时间正靠这一条。
    #[test]
    fn the_report_is_refreshed_only_while_it_is_not_caught_up() {
        assert!(needs_refresh(None), "没核对过必须先问一次");
        assert!(
            needs_refresh(Some(&StorageVersionReport::from_stored(None))),
            "没有记录 = 老库，要问"
        );
        assert!(
            needs_refresh(Some(&StorageVersionReport::from_stored(Some((
                aios_core::model_storage::MODEL_STORAGE_VERSION + 1,
                None,
                None
            ))))),
            "比本进程新的库同样未追平，要问"
        );
        assert!(
            !needs_refresh(Some(&StorageVersionReport::from_stored(current()))),
            "已追平不该再付一次库往返"
        );
    }

    /// 读那条记录的 SQL 必须能在真引擎上跑通。
    ///
    /// 这条钉是补一个真库上才暴露的缺陷：原本写的是
    /// `SELECT version AS version, shape AS shape, rebuild AS rebuild FROM ONLY …`，
    /// 而 `rebuild` 在 SurrealDB 2.x 里是保留字，解析器报
    /// `Unexpected token 'AS', expected a rebuild statement keyword`。所有纯函数单测
    /// 都是绿的——它们只喂 `from_stored`，从不经过这条 SQL。结果是启动时那句
    /// 「读不到 storage_version:current，本次跳过」被当成一次读失败吞掉，`/health`
    /// 永远 `null`，而且没有任何迹象指向语法。
    #[tokio::test]
    async fn the_version_record_is_readable_from_a_real_engine() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("storage_version_reconcile")
            .await
            .expect("select fixture db");

        // 没有记录 = 老库，且这条 SQL 本身不能报错。
        assert_eq!(
            stored_version_on(&db).await.expect("空库也要能读"),
            None,
            "没有记录时回 None，不是 Err"
        );

        db.query(format!(
            "UPSERT {} CONTENT {{ version: {}, shape: 'sh', rebuild: 'rb' }};",
            aios_core::model_storage::STORAGE_VERSION_RECORD,
            aios_core::model_storage::MODEL_STORAGE_VERSION
        ))
        .await
        .expect("seed reaches db")
        .check()
        .expect("seed statement");

        assert_eq!(
            stored_version_on(&db).await.expect("有记录时读得出来"),
            Some((
                aios_core::model_storage::MODEL_STORAGE_VERSION,
                Some("sh".to_owned()),
                Some("rb".to_owned())
            )),
            "三格都要原样带出来，包括保留字那格 rebuild"
        );
    }
}
