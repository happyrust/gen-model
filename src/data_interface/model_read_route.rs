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
//! Database ⇔ 数据面是 ingest ∧ applied_sesno > 0 ∧ 该库没有数据批次任务在跑（task_registry）
//!            ∧ model_sesno == applied_sesno（镜像；ADR-071 D8「水位相等」，ADR-073 D2 收成两枚）
//! Memory   ⇔ 其余（读透形态 / 基线未落 / 该库的任务在飞 / 两枚水位不相等）
//! ```
//!
//! 「模型落后」那一档（ADR-071 D8）：库里的模型行是上一版的，而进程内投影是启动时从库读回 +
//! 本进程刚生成的**超集**——任务落库失败留痕、下一窗未触到的根，内存里都有；读内存不会比读库少。
//! 这句「超集」在两种形态下不成立（GitHub #74，2026-10-27）：冷重启后投影是空的（启动不再整表读回）；初始化直写
//! 不留投影（ADR-077 D5）。于是一根 failed 让整库 lagging，`/model/records` 对已写好的几百根也回 0 条。现在这一档
//! **按根**补（[`ModelReadRoute::allows_durable_fallback`]）：投影里有回执的根读投影，没有的回读库里 S 时刻的那份——
//! 它是整库落过的完整旧快照，比空集对；还在赶的根下一窗落地后自然换成投影那份。路由结论仍是 `Memory`，
//! 回执 `durable_fallback_roots` 点名哪几根是回读的。「任务在飞」那一档（2026-10-28）同样按根补：初始化收尾那一笔
//! 已盖 `applied == model`、`registry.finish` 还没写终态的窗口里，读者看到的是「在飞」，而库里已是完整快照、投影里又没有
//! 回执（初始化直写），不补就是整库回 0 条；模型行是一根一笔事务，在飞的写者也不会让按根读读到半根。
//! 算得了房间的进程里归属随根算、随模型块同事务落边（ADR-073 D1；2026-10-23 起不再是开关）；
//! 算不了的进程模型路由仍只按 `model_sesno` 选库，因为该水位此时只承诺几何，房间降级由
//! `/health.room_membership` 单独表达。两枚水位同一次读（镜像那一格），分两次点查会在收尾那一笔的前后各读到一半。
//!
//! - `Database`：rocksdb 为准，一个字不改（R4）。
//! - `Memory`：按需请求直接从文件生成进 `ModelMemoryStore`、由 API 供数；读者只认投影，不等库。
//!   **结果写不写库是另一回事**（下一节）：[`scope_memory_only`] 说的是「读源在内存、不等初始化队列、
//!   不进数据批次队列」，不是「不落库」。
//!
//! # 内存路由下按需生成的结果写到哪儿（2026-11-11 用户定）
//!
//! 进程内投影一定写。此外看形态与介质：
//!
//! | 形态 | 介质 | 还写到哪儿 |
//! |---|---|---|
//! | 摄入（ingest），库还没准备好 | 外部持久层（`store_mode = rocksdb`） | **也写进库**：每个发布的根登记进写后台（`model_write_behind::publish`），随后整根一笔落库——模型行，算得了房间的进程里连同房间边 |
//! | 摄入 | 内嵌 kv-mem | 只有投影（[`ModelPublicationPolicy::ProjectionOnly`]） |
//! | 读透（read-through） | kv-mem | 本进程正式的 `ProcessStore{durable:false}`：逐根同事务发布模型行、房间边 |
//!
//! （读透 + 外部持久层走的是 mirror spool，`model_refresh` 里读透那一支，不在这张表里。）
//!
//! 第一行此前写的是「只留投影、不落 rocksdb」，与现在的代码对不上——写后台登记不看路由（GitHub #146）。
//! 用户的口径：**基线没落的库，按需生成的结果也会写进库**，文档照实改，不为了对上旧文档去改代码。
//! 这些行不盖水位、不等初始化，但在外部持久层（`store_mode = rocksdb`）下已是完整按根写下的事务
//! （[`ModelReadRoute::allows_durable_fallback`]），进程冷重启后投影清空时仍允许按根回读：
//!
//! - **初始化要接住它们**。初始化直写是 `INSERT IGNORE`，旧行会赢；所以入口先把这个库的模型面清掉再建
//!   （`dbnum_initialize::clear_model_face_before_initial_on`：`pe` 圈不到的行按残留行自己的 id 圈、按位解码
//!   核对归属；清的时候又落进来的再清一遍）。先清后建不丢东西——初始化把每一根都重新生成，其间投影照常供数。
//! - **清理接口得认得它们**。这些行 `pe` 圈不到：整库模型清理（`fast_delete::wipe_dbnum_model_face`）在基线没落的
//!   库上连它们一并清，整库删除（`fast_delete::delete_dbnum_fast`）在 `pe` 删光之后补一刀；子树清理
//!   （`DELETE /model/subtree`）在基线没落且使用外部持久层时也会同步清掉库里对应范围的行。
//! - **回执里的 `durable = false` 不等于库里没有行**。它说的是还没盖库级水位（仍走写入后异步落库），
//!   外部持久层重启后读者仍可通过按根回退读回这些已落库的根。
//! - 初始化在跑的时候按需生成照常写库：写后台整根一笔、先删后插，初始化遇到已有的行让它赢，同一根在库里始终是
//!   完整的某一版；文件其间又存了新会话的话，差的那一截由下一个增量窗口重算。
//!
//! 「在飞」的来源是任务注册表（ADR-071 S5，d-300）：初始化发布 run 已退役，不再有「run 收口」这一刻；
//! 翻面（Memory → Database）就是该库的任务跑完。相反方向只在回退重建清库时发生。两处都发
//! WS 事件 `model_source_changed`（R5），不强制客户端重载——内存投影与刚落的行是同一版文件算出来的。

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
    /// 基线落了、水位没倒挂，但该库的数据批次任务还在跑（`task_registry::has_running_data_batch_for`）。
    /// 字面值沿用 `initialization_publishing`：plant-ui 已经认这个字，含义不变——「这个库正在被写」。
    /// 这一档不再挡按根回读 durable（见 [`ModelReadRoute::allows_durable_fallback`]）：模型行是一根一笔事务
    /// （`src/fast_model/model_db_adapter.rs`「一根一笔事务」），在飞的写者替换一根要么整根成、要么一行不落，按根读
    /// 永远读到完整的某一版；收尾那一笔已落、`registry.finish` 还没写终态的那段窗口里更是一行都没在写。
    InitializationPublishing,
    /// 数据到了 T、模型水位还没追上（`model_sesno < applied_sesno`，ADR-071 D8）：库里的模型行
    /// 是旧版，进程内投影更全。房间边随根写在模型块里，模型追上就是房间追上（ADR-073 D2）。
    ModelLagging,
    /// `/dbnums` 没拿到模型水位，无法证明两枚水位相等。
    ModelWatermarkUnavailable,
    /// 模型水位越过数据水位。两枚水位失配时持久层不能作为权威读源。
    WatermarksInconsistent,
}

impl MemoryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadThrough => "read-through",
            Self::DataWatermarkUnestablished => "data_watermark_unestablished",
            Self::InitializationPublishing => "initialization_publishing",
            Self::ModelLagging => "model_lagging",
            Self::ModelWatermarkUnavailable => "model_watermark_unavailable",
            Self::WatermarksInconsistent => "watermarks_inconsistent",
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

    /// `Memory` 路由里允许**按根**回读 durable 的三档：「基线未落（按需落库）」、「模型落后」（GitHub #74，
    /// 2026-10-27）与「任务在飞」（2026-10-28）。后两档满足 `applied_sesno > 0` 且 `model_sesno <= applied_sesno`；
    /// 「基线未落」则是按需生成通过写后台以「一根一笔事务」落入持久库的根（2026-11-11 定，GitHub #146），冷重启后
    /// 进程内投影清空，回读这些已落盘的根避免整库回 0 条而误触发重新生成。别的理由仍不许：读透的库从来不是权威；
    /// 水位缺席 / 倒挂（含在飞且倒挂）说明库行不可信。`Database` 路由本来就读库，谈不上回读。
    pub fn allows_durable_fallback(&self) -> bool {
        matches!(
            self.reason,
            Some(MemoryReason::DataWatermarkUnestablished)
                | Some(MemoryReason::ModelLagging)
                | Some(MemoryReason::InitializationPublishing)
        )
    }

    /// 回执 / 行上要摊开的几格。
    pub fn columns(&self) -> serde_json::Value {
        serde_json::json!({
            "model_source": self.source.as_str(),
            "model_source_reason": self.reason.map(MemoryReason::as_str),
        })
    }
}

/// 模型从哪里读与本轮产物向哪里发布是两件事。
///
/// `ProjectionOnly` = 这一轮只以投影为准：不走同步的逐根发布事务，就绪也不等落库的 ACK。摄入形态下库还没
/// 准备好的那些库走这一档。read-through 的 process store 本身就是本进程正式的非持久发布目标，因此即使读取源是
/// [`ModelReadSource::Memory`] 也必须走 `ProcessStore`。
///
/// **`ProjectionOnly` 不等于库里没有行**（2026-11-11 改口）：外部持久层下，每个发布的根照样登记进写后台、
/// 随后整根落库——见模块文档「内存路由下按需生成的结果写到哪儿」。它曾被写成「这时数据库有唯一写者，按需生成
/// 只能留在进程内投影」，那句话与现在的代码对不上，用户也定了不照它去改代码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPublicationPolicy {
    ProjectionOnly,
    ProcessStore { durable: bool },
}

impl ModelPublicationPolicy {
    pub fn uses_process_store(self) -> bool {
        matches!(self, Self::ProcessStore { .. })
    }

    pub fn durable(self) -> bool {
        matches!(self, Self::ProcessStore { durable: true })
    }
}

/// 发布策略的纯函数形态，测试把四种读源/介质组合逐格钉住。
pub fn publication_policy_for(
    memory_only: bool,
    data_face: DataFace,
    store_mode: crate::options::StoreMode,
) -> ModelPublicationPolicy {
    if memory_only && !(data_face.is_read_through() && !store_mode.is_durable()) {
        ModelPublicationPolicy::ProjectionOnly
    } else {
        ModelPublicationPolicy::ProcessStore {
            durable: store_mode.is_durable(),
        }
    }
}

/// 当前任务的发布策略。`memory_only` 来自任务级路由作用域，不再被解释成“永不写数据库”。
pub fn publication_policy(memory_only: bool) -> ModelPublicationPolicy {
    publication_policy_for(
        memory_only,
        crate::options::data_face(),
        crate::options::store_mode(),
    )
}

/// **唯一的判据**（纯函数）。四个输入分别来自：数据面形态（`options::data_face()`）、
/// 该库的 `dbnum_watermark.applied_sesno` / `model_sesno`（镜像，同一次读）、
/// 该库是否有数据批次任务在跑。
///
/// 两枚水位必须严格相等；模型水位落后或异常超前时，持久层都不能冒充权威读源。
///
/// 判据顺序：读透 → 基线未落 → **倒挂**（`model_sesno > applied_sesno`）→ 在飞 → 落后 → `Database`。倒挂排在
/// 在飞之前（2026-10-28）：「在飞且倒挂」要归到 `WatermarksInconsistent`——那是库行不可信、不许按根回读的一档；
/// 在飞排在落后之前照旧（在飞时模型水位本来就还没盖，说「正在被写」才是读者要的那句），这两档都允许按根回读
/// （[`ModelReadRoute::allows_durable_fallback`]）。
pub fn route(
    dbnum: u32,
    data_face: DataFace,
    applied_sesno: i32,
    model_sesno: i32,
    initialization_publishing: bool,
) -> ModelReadRoute {
    let reason = if data_face.is_read_through() {
        Some(MemoryReason::ReadThrough)
    } else if applied_sesno <= 0 {
        Some(MemoryReason::DataWatermarkUnestablished)
    } else if model_sesno > applied_sesno {
        Some(MemoryReason::WatermarksInconsistent)
    } else if initialization_publishing {
        Some(MemoryReason::InitializationPublishing)
    } else if model_sesno < applied_sesno {
        Some(MemoryReason::ModelLagging)
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

/// `/dbnums` 普查可能拿不到模型水位；缺席不是相等，必须 fail closed 到内存读源。
pub fn route_with_optional_model_watermark(
    dbnum: u32,
    data_face: DataFace,
    applied_sesno: i32,
    model_sesno: Option<i32>,
    initialization_publishing: bool,
) -> ModelReadRoute {
    match model_sesno {
        Some(model_sesno) => route(
            dbnum,
            data_face,
            applied_sesno,
            model_sesno,
            initialization_publishing,
        ),
        None if data_face.is_read_through() => route(
            dbnum,
            data_face,
            applied_sesno,
            0,
            initialization_publishing,
        ),
        None if applied_sesno <= 0 => route(
            dbnum,
            data_face,
            applied_sesno,
            0,
            initialization_publishing,
        ),
        None if initialization_publishing => route(dbnum, data_face, applied_sesno, 0, true),
        None => ModelReadRoute {
            dbnum,
            source: ModelReadSource::Memory,
            reason: Some(MemoryReason::ModelWatermarkUnavailable),
        },
    }
}

/// 本进程对 `dbnum` 的现行路由。`applied_sesno` 走点查而不是 `dbnum_consistency` 的 20 s 普查缓存：
/// 翻面那一刻（任务跑完）读者就该看到 `Database`，缓存会让它多晃 20 s。
pub async fn route_for(dbnum: u32) -> anyhow::Result<ModelReadRoute> {
    let face = crate::options::data_face();
    if face.is_read_through() {
        return Ok(route(dbnum, face, 0, 0, false));
    }
    // 两枚水位同一次读（镜像那一格）：分几次点查会在任务收尾那一笔的前后各读到一半。
    let (applied, model) = crate::data_interface::dbnum_state::DbnumState::read(dbnum)
        .await?
        .map(|state| (state.applied_sesno, state.model_sesno))
        .unwrap_or_default();
    let in_flight = crate::data_interface::task_registry::TaskRegistry::global()
        .has_running_data_batch_for(dbnum);
    Ok(route(dbnum, face, applied, model, in_flight))
}

tokio::task_local! {
    static MEMORY_ONLY_DBNUM: u32;
}

/// 在这个作用域里，对 `dbnum` 的按需生成按**内存路由**走（R2）：读者只认进程内投影，不等初始化队列、
/// 不进数据批次队列、不盖水位。只有 [`route_for`] 判成 `Memory` 的库才该进这个作用域。
///
/// 名字里的 `memory_only` 说的是读源，不是写目标：外部持久层下产物照样经写后台落库（模块文档
/// 「内存路由下按需生成的结果写到哪儿」，2026-11-11 用户定）。代码与注释里沿用的「只写内存」作用域指的就是它。
pub async fn scope_memory_only<T>(dbnum: u32, future: impl std::future::Future<Output = T>) -> T {
    MEMORY_ONLY_DBNUM.scope(dbnum, future).await
}

/// 当前任务是否处在对 `dbnum` 的「只写内存」（内存路由）作用域里。
pub fn memory_only_for(dbnum: u32) -> bool {
    MEMORY_ONLY_DBNUM
        .try_with(|scoped| *scoped == dbnum)
        .unwrap_or(false)
}

/// 当前任务处在哪个库的「只写内存」（内存路由）作用域里（不在任何作用域里 = `None`）。
pub fn memory_only_scope() -> Option<u32> {
    MEMORY_ONLY_DBNUM.try_with(|scoped| *scoped).ok()
}

/// 翻面通告（R5）：`/dbnums` 行与 WS `tasks` 主题同时露出，客户端只刷那一格、不重载场景。
pub fn announce_source_changed(dbnum: u32, source: ModelReadSource) {
    println!(
        "[model-read-route] dbnum={dbnum} 模型取数源 → {}（{}）",
        source.as_str(),
        match source {
            ModelReadSource::Database => "该库的任务已跑完，rocksdb 为准",
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

    /// ADR-071 D8（ADR-073 D2 收成两枚）：`Database` ⇔ ingest ∧ 基线已落 ∧ 无任务在飞 ∧
    /// **两水位相等**。
    #[test]
    fn database_needs_ingest_watermark_no_task_in_flight_and_two_equal_watermarks() {
        let ok = route(8000, DataFace::Ingest, 285, 285, false);
        assert_eq!(ok.source, ModelReadSource::Database);
        assert_eq!(ok.reason, None);
        assert!(ok.is_database());
        // 模型水位跑到数据前面同样违反「两水位相等」：异常库行不能对外冒充权威。
        assert_eq!(
            route(8000, DataFace::Ingest, 285, 300, false).source,
            ModelReadSource::Memory
        );
    }

    #[test]
    fn every_memory_reason_has_its_own_name() {
        assert_eq!(
            route(8000, DataFace::ReadThrough, 285, 285, false).reason,
            Some(MemoryReason::ReadThrough)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 0, 0, false).reason,
            Some(MemoryReason::DataWatermarkUnestablished)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 285, 285, true).reason,
            Some(MemoryReason::InitializationPublishing)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 285, 284, false).reason,
            Some(MemoryReason::ModelLagging)
        );
        assert_eq!(
            route(8000, DataFace::Ingest, 285, 300, false).reason,
            Some(MemoryReason::WatermarksInconsistent)
        );
        let mut names = std::collections::BTreeSet::new();
        for reason in [
            MemoryReason::ReadThrough,
            MemoryReason::DataWatermarkUnestablished,
            MemoryReason::InitializationPublishing,
            MemoryReason::ModelLagging,
            MemoryReason::ModelWatermarkUnavailable,
            MemoryReason::WatermarksInconsistent,
        ] {
            assert!(!reason.as_str().is_empty());
            assert!(
                names.insert(reason.as_str()),
                "理由字面值不许撞: {}",
                reason.as_str()
            );
        }
    }

    /// ADR-073 实施约束 2：理由字面值里没有房间那一档——房间边随根写在模型块里，
    /// 「房间落后」不再是 `/dbnums` / `/model/records` 上可见的独立理由。
    #[test]
    fn there_is_no_room_reason_anymore() {
        let source = include_str!("model_read_route.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        assert!(
            !production.contains(concat!("room_", "lagging")),
            "{production}"
        );
        assert!(
            !production.contains(concat!("Room", "Lagging")),
            "{production}"
        );
        assert!(
            !production.contains(concat!("room_", "sesno")),
            "{production}"
        );
    }

    /// 读透形态下水位恒 0、也没有任务在飞，但理由必须点名「读透」而不是「水位未建立」：
    /// 后者读起来像「基线还没跑」，而读透根本不做基线。
    #[test]
    fn read_through_wins_over_the_other_reasons() {
        assert_eq!(
            route(8000, DataFace::ReadThrough, 0, 0, true).reason,
            Some(MemoryReason::ReadThrough)
        );
    }

    /// 理由的优先序：任务在飞压过模型落后——在飞时模型水位本来就还没盖，说「落后」是废话；
    /// 说「正在被写」才是读者要的那句。
    #[test]
    fn a_task_in_flight_is_named_before_a_lagging_model() {
        assert_eq!(
            route(8000, DataFace::Ingest, 285, 200, true).reason,
            Some(MemoryReason::InitializationPublishing)
        );
    }

    /// GitHub #74 + 2026-10-28 + GitHub #146：「基线未落（按需落库）」、「模型落后」与「任务在飞」三档
    /// 都允许按根回读 durable；读透 / 水位缺席 / 倒挂（含在飞且倒挂）都不许，`Database` 路由本来就读库。
    #[test]
    fn a_complete_durable_snapshot_falls_back_per_root_even_while_a_task_is_in_flight() {
        // 在飞 + 相等：初始化收尾那一笔已落、registry.finish 还没写终态的窗口。
        let in_flight_equal = route(8000, DataFace::Ingest, 285, 285, true);
        assert_eq!(
            in_flight_equal.reason,
            Some(MemoryReason::InitializationPublishing)
        );
        assert!(in_flight_equal.allows_durable_fallback());
        // 在飞 + 落后。
        assert!(route(8000, DataFace::Ingest, 285, 284, true).allows_durable_fallback());
        // 不在飞 + 落后（#74 原有的那一档）。
        assert!(route(8000, DataFace::Ingest, 285, 284, false).allows_durable_fallback());
        // 基线未落：按需生成结果在 durable 介质会写库，冷重启后按根回读持久库。
        assert!(route(8000, DataFace::Ingest, 0, 0, false).allows_durable_fallback());
        assert!(route(8000, DataFace::Ingest, 0, 0, true).allows_durable_fallback());
        // 在飞 + 倒挂：倒挂先于在飞，归 WatermarksInconsistent，不许回读。
        let in_flight_inconsistent = route(8000, DataFace::Ingest, 285, 300, true);
        assert_eq!(
            in_flight_inconsistent.reason,
            Some(MemoryReason::WatermarksInconsistent)
        );
        assert!(!in_flight_inconsistent.allows_durable_fallback());
        // 不在飞 + 相等：Database 路由，本来就读库。
        let database = route(8000, DataFace::Ingest, 285, 285, false);
        assert!(database.is_database());
        assert!(!database.allows_durable_fallback());
        assert!(!route(8000, DataFace::ReadThrough, 285, 284, false).allows_durable_fallback());
        assert!(!route(8000, DataFace::ReadThrough, 285, 284, true).allows_durable_fallback());
        assert!(!route(8000, DataFace::Ingest, 285, 300, false).allows_durable_fallback());
        assert!(
            !route_with_optional_model_watermark(8000, DataFace::Ingest, 285, None, false)
                .allows_durable_fallback()
        );
    }

    /// 2026-10-28：`route()` 里倒挂那一支必须排在在飞之前、在飞排在落后之前——否则「在飞且倒挂」会被叫成
    /// initialization_publishing 并按新口径放行回读，而倒挂正是「库行不可信」的那一档。按文本位置比，不靠行号。
    #[test]
    fn inconsistent_watermarks_are_judged_before_a_task_in_flight() {
        let source = include_str!("model_read_route.rs");
        let body = source
            .split_once(concat!("pub fn ", "route("))
            .expect("route 未找到")
            .1
            .split_once(concat!("pub fn ", "route_with_optional_model_watermark("))
            .expect("route 之后应当是 route_with_optional_model_watermark")
            .0;
        let inconsistent = body
            .find("else if model_sesno > applied_sesno")
            .expect("倒挂那一支还在");
        let in_flight = body
            .find("else if initialization_publishing")
            .expect("在飞那一支还在");
        let lagging = body
            .find("else if model_sesno < applied_sesno")
            .expect("落后那一支还在");
        assert!(
            inconsistent < in_flight && in_flight < lagging,
            "判据顺序必须是 倒挂 → 在飞 → 落后: {body}"
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

    #[test]
    fn publication_policy_separates_read_source_from_process_store_target() {
        use crate::options::StoreMode;

        assert_eq!(
            publication_policy_for(false, DataFace::Ingest, StoreMode::RocksDb),
            ModelPublicationPolicy::ProcessStore { durable: true }
        );
        assert_eq!(
            publication_policy_for(true, DataFace::Ingest, StoreMode::RocksDb),
            ModelPublicationPolicy::ProjectionOnly,
            "an ingest fallback is judged by the projection alone: no synchronous per-root publication, \
             no waiting for a durable ACK (its rows still reach RocksDB through the write-behind)"
        );
        assert_eq!(
            publication_policy_for(true, DataFace::ReadThrough, StoreMode::Mem),
            ModelPublicationPolicy::ProcessStore { durable: false }
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
        let value = route(8000, DataFace::Ingest, 285, 285, true).columns();
        assert_eq!(value["model_source"], "memory");
        assert_eq!(value["model_source_reason"], "initialization_publishing");
        let value = route(8000, DataFace::Ingest, 285, 285, false).columns();
        assert_eq!(value["model_source"], "database");
        assert!(value["model_source_reason"].is_null());
        let value = route(8000, DataFace::Ingest, 285, 280, false).columns();
        assert_eq!(value["model_source"], "memory");
        assert_eq!(value["model_source_reason"], "model_lagging");
    }
}
