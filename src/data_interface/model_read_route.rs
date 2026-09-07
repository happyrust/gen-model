//! 模型取数源路由：一个库的模型该从哪儿读——rocksdb 还是进程内投影。
//!
//! 出处：`docs/plans/2026-09-06-rocksdb-batch-write-and-cata-dependency-plan.md` §8（R1–R6，共识 d-29）。
//!
//! 此前 `/model/records` 按**进程介质**选源（`store_mode().is_durable()` → 读库，否则读内存），
//! `/model/ensure` 在摄入形态下先 `wait_for_dbnum`（该库的初始化发布 run 在飞就原地等）、miss 再撞
//! `require_model_generation()`。于是一个库**正在初始化**的那段时间，plant-ui 对它要么等、要么拿到错误，
//! 而进程内本来就能从文件把它的模型算出来。
//!
//! 现在只有**一个谓词**（[`route`]），读者（`/dbnums`、`/model/records`、`/model/ensure`、`/health`）只消费它、
//! 不自己拼（ADR-060 M8 同款纪律）：
//!
//! ```text
//! Database ⇔ 数据面是 ingest ∧ applied_sesno > 0 ∧ 该库没有活着的初始化发布 run
//! Memory   ⇔ 其余（读透形态 / 基线未落 / 初始化发布在飞）
//! ```
//!
//! - `Database`：rocksdb 为准，一个字不改（R4）。
//! - `Memory`：按需请求直接从文件生成进 `ModelMemoryStore`、由 API 供数；**不进 durable 队列、不落 rocksdb**
//!   （Q8：持久由初始化 run 负责，再来一个写者只会撞乐观事务）；有初始化 run 时钉它的 `source_sesno`
//!   （Q9：两个投影同版，翻面时零差异）。这一档由 [`scope_memory_only`] 的任务级作用域表达，生成链路上
//!   的三道门（`wait_for_dbnum` / `require_model_generation` / durable pending + `persist_root`）
//!   各自用 [`memory_only_for`] 判一次。
//!
//! 翻面（Memory → Database）发生在初始化 run 收口那一刻；相反方向只在回退重建清库时发生。两处都发
//! WS 事件 `model_source_changed`（R5），不强制客户端重载——内存投影与刚发布的行是同一版文件算出来的。

use serde::Serialize;

use crate::options::DataFace;

/// 一个库的模型该从哪儿读。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReadSource {
    /// 进程内投影（`ModelMemoryStore`）：rocksdb 里这个库还没准备好。
    Memory,
    /// rocksdb（经 `SUL_DB`）：这个库已经初始化过，库里的行是权威。
    Database,
}

impl ModelReadSource {
    /// `/dbnums` / `/health` / 回执上的字面值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Database => "database",
        }
    }

    /// `/model/records` 回执 `source` 一格沿用至今的字面值（plant-ui 已经认这两个）。
    pub fn records_source_label(self) -> &'static str {
        match self {
            Self::Memory => "model-memory",
            Self::Database => "model-database",
        }
    }

    pub fn is_database(self) -> bool {
        matches!(self, Self::Database)
    }
}

/// 为什么落在 `Memory`（`Database` 没有理由一栏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryReason {
    /// 读透形态：数据面不摄入，库里从来不是权威。
    ReadThrough,
    /// 基线还没落（`applied_sesno == 0`）。
    DataWatermarkUnestablished,
    /// 基线落了，但该库的初始化发布 run 还在飞（`model_publish_queue::blocks_dbnum`）。
    InitializationPublishing,
}

impl MemoryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::DataWatermarkUnestablished => "data_watermark_unestablished",
            Self::InitializationPublishing => "initialization_publishing",
        }
    }
}

/// 一个库的路由结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelReadRoute {
    pub dbnum: u32,
    pub source: ModelReadSource,
    /// `source == Memory` 时才有。
    pub reason: Option<MemoryReason>,
}

impl ModelReadRoute {
    pub fn is_database(&self) -> bool {
        self.source.is_database()
    }

    /// 回执 / 行上要摊开的几格。
    pub fn columns(&self) -> serde_json::Value {
        serde_json::json!({
            "model_source": self.source.as_str(),
            "model_source_reason": self.reason.map(MemoryReason::as_str),
        })
    }
}

/// **唯一的判据**（纯函数）。三个输入分别来自：数据面形态（`options::data_face()`）、
/// 该库的 `dbnum_watermark.applied_sesno`、该库是否有活着的初始化发布 run。
pub fn route(
    dbnum: u32,
    data_face: DataFace,
    applied_sesno: i32,
    initialization_publishing: bool,
) -> ModelReadRoute {
    let reason = if data_face.is_read_through() {
        Some(MemoryReason::ReadThrough)
    } else if applied_sesno <= 0 {
        Some(MemoryReason::DataWatermarkUnestablished)
    } else if initialization_publishing {
        Some(MemoryReason::InitializationPublishing)
    } else {
        None
    };
    ModelReadRoute {
        dbnum,
        source: if reason.is_some() {
            ModelReadSource::Memory
        } else {
            ModelReadSource::Database
        },
        reason,
    }
}

/// 本进程对 `dbnum` 的现行路由。`applied_sesno` 走点查而不是 `dbnum_consistency` 的 20 s 普查缓存：
/// 翻面那一刻（run 收口）读者就该看到 `Database`，缓存会让它多晃 20 s。
pub async fn route_for(dbnum: u32) -> anyhow::Result<ModelReadRoute> {
    let face = crate::options::data_face();
    if face.is_read_through() {
        return Ok(route(dbnum, face, 0, false));
    }
    let applied = crate::data_interface::dbnum_state::DbnumState::applied_sesno(dbnum).await?;
    let publishing = crate::data_interface::model_publish_queue::blocks_dbnum(dbnum);
    Ok(route(dbnum, face, applied, publishing))
}

tokio::task_local! {
    static MEMORY_ONLY_DBNUM: u32;
}

/// 在这个作用域里，对 `dbnum` 的按需生成**只写进程内投影**（R2）：不等初始化队列、不进 durable
/// 队列、不落 rocksdb。只有 [`route_for`] 判成 `Memory` 的库才该进这个作用域。
pub async fn scope_memory_only<T>(dbnum: u32, future: impl std::future::Future<Output = T>) -> T {
    MEMORY_ONLY_DBNUM.scope(dbnum, future).await
}

/// 当前任务是否处在对 `dbnum` 的「只写内存」作用域里。
pub fn memory_only_for(dbnum: u32) -> bool {
    MEMORY_ONLY_DBNUM
        .try_with(|scoped| *scoped == dbnum)
        .unwrap_or(false)
}

/// 当前任务处在哪个库的「只写内存」作用域里（不在任何作用域里 = `None`）。
pub fn memory_only_scope() -> Option<u32> {
    MEMORY_ONLY_DBNUM.try_with(|scoped| *scoped).ok()
}

/// 翻面通告（R5）：`/dbnums` 行与 WS `tasks` 主题同时露出，客户端只刷那一格、不重载场景。
pub fn announce_source_changed(dbnum: u32, source: ModelReadSource) {
    println!(
        "[model-read-route] dbnum={dbnum} 模型取数源 → {}（{}）",
        source.as_str(),
        match source {
            ModelReadSource::Database => "初始化发布 run 已收口，rocksdb 为准",
            ModelReadSource::Memory => "库已清空待重建，按需请求改由进程内投影供数",
        }
    );
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "model_source_changed",
        None,
        serde_json::json!({ "dbnum": dbnum, "model_source": source.as_str() }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_needs_ingest_and_watermark_and_no_publishing_run() {
        let ok = route(8000, DataFace::Ingest, 285, false);
        assert_eq!(ok.source, ModelReadSource::Database);
        assert_eq!(ok.reason, None);
        assert!(ok.is_database());
    }

    #[test]
    fn every_memory_reason_has_its_own_name() {
        assert_eq!(
            route(8000, DataFace::ReadThrough, 285, false).reason,
            Some(MemoryReason::ReadThrough)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 0, false).reason,
            Some(MemoryReason::DataWatermarkUnestablished)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 285, true).reason,
            Some(MemoryReason::InitializationPublishing)
        );
        for reason in [
            MemoryReason::ReadThrough,
            MemoryReason::DataWatermarkUnestablished,
            MemoryReason::InitializationPublishing,
        ] {
            assert!(!reason.as_str().is_empty());
        }
    }

    /// 读透形态下水位恒 0、也没有初始化 run，但理由必须点名「读透」而不是「水位未建立」：
    /// 后者读起来像「基线还没跑」，而读透根本不做基线。
    #[test]
    fn read_through_wins_over_the_other_reasons() {
        assert_eq!(
            route(8000, DataFace::ReadThrough, 0, true).reason,
            Some(MemoryReason::ReadThrough)
        );
    }

    #[test]
    fn records_labels_are_the_two_values_plant_ui_already_accepts() {
        assert_eq!(
            ModelReadSource::Memory.records_source_label(),
            "model-memory"
        );
        assert_eq!(
            ModelReadSource::Database.records_source_label(),
            "model-database"
        );
    }

    #[tokio::test]
    async fn memory_only_scope_is_task_local_and_dbnum_bound() {
        assert!(!memory_only_for(8000));
        assert_eq!(memory_only_scope(), None);
        scope_memory_only(8000, async {
            assert!(memory_only_for(8000));
            assert!(!memory_only_for(7997));
            assert_eq!(memory_only_scope(), Some(8000));
        })
        .await;
        assert!(!memory_only_for(8000));
    }

    #[test]
    fn columns_expose_source_and_reason() {
        let value = route(8000, DataFace::Ingest, 285, true).columns();
        assert_eq!(value["model_source"], "memory");
        assert_eq!(value["model_source_reason"], "initialization_publishing");
        let value = route(8000, DataFace::Ingest, 285, false).columns();
        assert_eq!(value["model_source"], "database");
        assert!(value["model_source_reason"].is_null());
    }
}
