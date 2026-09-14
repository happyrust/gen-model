//! 把进程内模型投影搬进库里的落库适配器。
//!
//! 生成侧（[`crate::fast_model::e3d_model_service`]）现在只写
//! [`crate::fast_model::model_memory_store`]，全程不认识数据库。本模块是另一头：从存储里
//! 读结构化记录，拼回 Plant UI 一直在消费的那套库行——`inst_relate` / `tubi_relate` /
//! `inst_info` / `aabb` / `trans`，外加共享网格的 `inst_geo` 与 `geo_relate`。
//!
//! **行的内容与旧发布事务逐字一致**，一条 [`ProjectedGeometry`] 携带的就是当年拼进 SQL 的
//! 同一份内容（几何身份、来源 PE、祖先链、世界盒与世界变换、mesh 身份、图元共享信息），
//! 所以这里只渲染，不回头重算几何、也不重新读盘。**变的只有行 id**，见下。
//!
//! # 行 id：一律就是记录 id，不带任何前缀（2026-09-04）
//!
//! | 行 | id |
//! | --- | --- |
//! | `inst_relate` / `inst_info` / `aabb` 世界盒 / `trans` 世界变换 | `<记录 id>` |
//! | `aabb` 共享本地盒 / `inst_geo` | `<mesh id>`——按网格做键，多个元素共用一行 |
//! | `tubi_relate` | `[pe:⟨branch⟩, 分支全局槽位]`（旧 TUBI 协议的数组 id） |
//!
//! 记录 id 本身就是身份：真实元素是 `<word0>_<word1>`，隐式管身是身份的 64 位十六进制摘要
//! （见 [`crate::fast_model::e3d_mesh_store::geometry_record_id`]），两种形状天生分得开。
//! 旧版在这之上还叠过三层前缀，本轮全撤：
//!
//! - `direct_` / `direct_world_` / `direct_shared_`：防旧生成器同表撞 id 的。今天这几张表只剩
//!   本模块一个生产写口（旧生成器的入口在非默认的 `legacy_model` feature 后面）。
//! - `direct_local_`：共享图元的**本地变换不再单开一行 `trans`**，直接以 `transform` 字段写在
//!   `geo_relate` 边上（对象形状与旧 `trans.d` 逐字相同）。`trans` 表因此回到「一个元素一行」，
//!   读侧还少一次解引用。读侧认 `transform ?? trans.d`（见 `rs-core::rs_surreal::inst`），
//!   旧行照旧能读。
//! - `derived_`：派生几何的摘要 id 不需要自证身份。**更新一律先删后插**，没有「靠 id 形状
//!   猜出还有哪些行要删」这回事。
//!
//! 因此本模块只服务**当前投影**一种命名空间：历史快照过去靠 `hist_<key>_` 前缀与
//! `snapshot_key` 字段跟当前投影挤在同一批表里，没有前缀就没法共存——而它本来也不进生产库
//! （按需快照由 [`crate::fast_model::historical_model`] 直接从存储回包 JSON）。
//!
//! 旧命名空间的悬空行由 [`render_legacy_prefix_sweep_once`] 收走：**每个库一次**，跟着这个
//! 库的第一块落库一起发出去，扫完在库上盖标记，之后不再扫。其余读口不受影响：Plant UI 走的
//! 是记录链接（`aabb.d` / `world_trans.d` / `booled_id`）与 `in` 反查，从不解析 id 形状。
//!
//! # 拼 SQL 的两条纪律
//!
//! 数据里来的文本一律走 [`sql_string`]（`noun` 是 PDMS 读回来的，摘要是上游给的）；
//! `表:⟨…⟩` 那种位置没有转义序列可用，所以走 [`bracketed_id`] **拒绝**超出机器 id 字母表的
//! 东西。裸插值一处都不留——留一处，下一个往行上加字段的人就会照抄它。
//!
//! # `gen_root` 回执少了一样东西
//!
//! revision CAS 没有跟过来（回执里没有 revision）。「同一根不许被两个源会话混着发布」那条
//! 守卫仍在，读的是库里的活行；`published_revision` 跟着 `desired_revision` 走。
//!
//! **两种落库介质，一套行。** 库可以是进程内嵌的 kv-mem（内存模式，`in_memory_db=true`），
//! 也可以是 rocksdb（默认那台 ws 连的 SurrealDB，rocksdb 在服务器侧；不带 `ws` 的 `local`
//! 构建则是进程内嵌的 `rocksdb://<project>.rdb`）。换掉的只是介质：渲染那一半拿不到介质，
//! 两种模式下写出去的行逐字相同。[`PersistTarget`] 把句柄与介质绑在一起，回执因此说得出
//! 这一批落在哪——内存模式的行随进程一起消失，那不是「落库了」。
//!
//! # 几何之外还要落的三件
//!
//! 旧发布事务除了几何行还挂着三样，这里一并接回来（P3）：
//!
//! - **房间派生面**（[`room_edges_to_clear`]）。被对账掉的来源两个方向的
//!   房间边跟着**删除那一块**走；拿到新几何的来源排一次房间重算，跟着**回执那一块**走。
//!   两笔不能对调：清边落在删除后面的块，中途失败就留下一批指着已删几何的悬空边，而下一轮
//!   对账再也看不见那些几何；排重算落在前面的块，任务一被消费就在照着只写了一半的几何算
//!   房间归属。全库落库（[`persist_dbnum`]）只清边不排重算。
//! - **空间版本号**。动过几何的每一块在同一条事务里把 `spatial_epoch` +1——它与各库水位数对
//!   一起组成快照的启动指纹，少这一下，崩溃重启会按快照复用一棵停在旧盒上的树。
//! - **进程内空间树**。提交之后**就地**收敛（ADR-072 D4）：盒直接取投影记录的
//!   `world_bounds`（[`crate::fast_model::aabb_tree::projected_tree_entry`]），交给
//!   [`crate::fast_model::aabb_tree::converge_tree_after_persist`] 换新 / 摘旧，不回读库；
//!   只有写全局库的目标才做——树镜像的是那个库。此前这一步曾按数据窗口在不在
//!   （`file_round`）分两条路：窗口内寄进 `spatial_reconcile` 意图行等 publish 阶段重放，
//!   窗口外按已提交指针回读——publish 阶段退役，两条路收成一条。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::OnceLock;

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::e3d_mesh_store::geometry_record_id;
use crate::fast_model::e3d_model_service::{ModelTarget, geometry_source_refno, transform_value};
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionScope, RootReceipt, packed_refno,
};
/// 一条 `DELETE … <->room_relate` 最多点名多少个来源（与 `delete_inst_relate_subtree` 的
/// 分块同一量级）。
const ROOM_EDGE_DELETE_CHUNK: usize = 300;

/// 这一批落库要清掉房间边的来源元素（按 refno 排序去重）。
///
/// 从 `room_publication.rs` 搬过来的（房间流程退役，t-678）：那个模块整体退了，可「删掉一条
/// 几何就要清掉它的房间边」是数据面行为——两张房间边表留着（d-757），不清就是悬空边。
/// 原来那份还带一个 `recalc` 名单去排房间待办，随房间队列一起没了，所以只剩这一个名单；
/// `RoomEffectPolicy` / `room_incremental` 两个形参在退役前就已经是空壳（渲染器里一句
/// `let _ = (policy, room_incremental);`），一并去掉。
///
/// 拿到新几何的来源不清边——重算自己先清后写，两个名单互不相交。
fn room_edges_to_clear<'a>(
    upserts: impl IntoIterator<Item = (&'a GeometryId, RefNo, &'a str)>,
    removals: &[GeometryId],
    legacy_sources: &BTreeSet<RefnoEnum>,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut kept: BTreeSet<RefnoEnum> = BTreeSet::new();
    for (geometry_id, refno, _noun) in upserts {
        if matches!(geometry_id, GeometryId::Element { .. }) {
            kept.insert(refno_from_e3d(refno));
        }
    }
    let mut cleared: BTreeSet<RefnoEnum> = legacy_sources.clone();
    for geometry_id in removals {
        if let GeometryId::Element { refno } = geometry_id {
            let refno = crate::fast_model::e3d_model_service::parse_refno(refno)
                .with_context(|| format!("removed geometry {geometry_id}"))?;
            cleared.insert(refno_from_e3d(refno));
        }
    }
    cleared.retain(|refno| !kept.contains(refno));
    Ok(cleared.into_iter().collect())
}

/// 渲染成落库事务里的清边语句。名单为空时渲染成空串。
fn render_room_edge_clear(cleared: &[RefnoEnum]) -> String {
    let mut out = String::new();
    for chunk in cleared.chunks(ROOM_EDGE_DELETE_CHUNK) {
        let keys: Vec<String> = chunk.iter().map(RefnoEnum::to_pe_key).collect();
        out.push_str(&crate::data_interface::helper::render_room_membership_delete(&keys));
        out.push('\n');
    }
    out
}

/// 落库介质。**换掉的只是介质**：两种模式下渲染出来的行逐字相同，本模块也不因此分叉——
/// 渲染那一半根本拿不到介质（看 [`render_records`] 的签名），所以它想分叉也分不出来。
/// 介质只进回执，让调用方说得出这一批落在哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistMedium {
    /// rocksdb。`external` 形态是 ws 连的那台外置 SurrealDB——rocksdb 在服务器那一侧；
    /// 不带 `ws` 的 `local` 构建走进程内嵌的 `rocksdb://<project>.rdb`。
    /// 两者的共同点、也是这一档唯一要紧的性质：**行活得过进程重启**。
    RocksDb,
    /// 进程内嵌 kv-mem（`store_mode = "mem"`）。行落在进程里：
    /// 进程一退整库就没了，外面也没有端口可连。
    Mem,
}

impl PersistMedium {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RocksDb => "rocksdb",
            Self::Mem => "mem",
        }
    }

    /// 这一批行能不能活过进程重启。内存介质活不过——回执里点名它，免得把
    /// 「落库了」当成「留下来了」。
    ///
    /// 它同时也回答「别的进程读不读得到」。原来那是两个问题（`spawned-mem` 不 durable
    /// 但读得到），自带存储两档退役之后两个问题合一，`is_reachable()` 跟着删了。
    pub fn is_durable(self) -> bool {
        matches!(self, Self::RocksDb)
    }
}

/// 落库目标：一个库句柄，外加它是什么介质。
///
/// 两样绑在一起是有意的：句柄自己说不出它连的是 kv-mem 还是 rocksdb（`Surreal<Any>` 就是
/// 为了让引擎对上层不可见），拆开传迟早会出现一份说错介质的回执。
#[derive(Debug, Clone, Copy)]
pub struct PersistTarget<'a> {
    db: &'a Surreal<Any>,
    medium: PersistMedium,
    /// 提交之后要不要把进程内空间树收敛到这批新指针上。
    ///
    /// 树（`GLOBAL_AABB_TREE`）镜像的是**全局那个库**，所以只有写全局库的目标才该动它：
    /// 自带句柄的目标写的是别处，让它去收敛全局树，就是拿另一个库的指针污染这棵树。
    converge_spatial: bool,
}

impl PersistTarget<'_> {
    /// 本进程配置指定的那个目标：全局 `SUL_DB`，介质按 `options::store_mode()` 判。
    ///
    /// 判法与 `lib.rs` 的连库分派逐档对齐：`ws` 在场时由 `store_mode` 说了算，
    /// 只有不带 `ws` 的 `local` 构建才落在进程内嵌 rocksdb 上。
    pub fn process() -> PersistTarget<'static> {
        let medium = if cfg!(feature = "ws") {
            match crate::options::store_mode() {
                crate::options::StoreMode::RocksDb => PersistMedium::RocksDb,
                crate::options::StoreMode::Mem => PersistMedium::Mem,
            }
        } else {
            PersistMedium::RocksDb
        };
        PersistTarget {
            db: &aios_core::SUL_DB,
            medium,
            converge_spatial: true,
        }
    }

    /// 自带句柄的目标，介质由调用方说明。测试与将来独立开库的落库路径走这条。
    pub fn new(db: &Surreal<Any>, medium: PersistMedium) -> PersistTarget<'_> {
        PersistTarget {
            db,
            medium,
            converge_spatial: false,
        }
    }

    /// 测试专用：自带句柄**且**提交后收敛全局空间树——给「落库后树 = 投影盒」的 `mem://`
    /// 用例用。生产上只有 [`Self::process`] 才该动树，所以不对外。
    #[cfg(test)]
    pub(crate) fn converging_for_test(
        db: &Surreal<Any>,
        medium: PersistMedium,
    ) -> PersistTarget<'_> {
        PersistTarget {
            converge_spatial: true,
            ..Self::new(db, medium)
        }
    }

    pub fn medium(&self) -> PersistMedium {
        self.medium
    }
}

/// 一根切成几块提交，每块最多这么多条几何记录。
///
/// 一整根一笔事务在大根上是个几万件的报文（7997 那两根各约 2.6 万件）；切块把单笔报文压到
/// 可预期的量级。代价是这一根不再是一个原子边界——见 [`persist_root_on`] 里那三条顺序，
/// 半途而废靠「先删后插 + 回执最后盖」自愈，中间态由 [`render_publication_fence`] 在凭证行
/// 上标出来。
pub const PERSIST_CHUNK: usize = 5_000;

/// 一次落库落了多少行、落在哪。行数口径与旧 `E3dPersistReport` 的 `upserted` / `removed`
/// 相同：数的是几何记录，不是语句条数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedRows {
    pub medium: PersistMedium,
    pub upserted: usize,
    pub removed: usize,
    pub receipts: usize,
    /// 真正发出去的事务笔数（切块之后一根不再等于一笔）。空块不发也不计。
    pub commits: usize,
}

impl PersistedRows {
    fn none(medium: PersistMedium) -> Self {
        Self {
            medium,
            upserted: 0,
            removed: 0,
            receipts: 0,
            commits: 0,
        }
    }
}

/// 让一个生成根在库里与当前投影逐条对齐，一个事务落地。
///
/// 存储是快照而不是流水，所以这里走对账：库里这根名下由 e3d-model 写过、而存储里已经没有
/// 的几何，就是要删的陈旧行。只服务当前投影——历史快照从来不进生产库，它的记录由
/// [`crate::fast_model::historical_model`] 直接回包。
pub async fn persist_root(root: RefNo) -> anyhow::Result<PersistedRows> {
    persist_root_on(PersistTarget::process(), ModelMemoryStore::global(), root).await
}

/// Queue the durable database watermark behind all root writes already handed
/// to this FIFO. The worker also reconciles any still-pending memory roots
/// before advancing `dbnum_watermark.model_sesno`, so a retry can never leave
/// the durable watermark ahead of RocksDB model rows.
pub(crate) async fn enqueue_model_watermark_write_behind(
    dbnum: u32,
    target_sesno: i32,
    target_time: Option<String>,
) -> anyhow::Result<()> {
    crate::fast_model::model_write_behind::request_settlement(dbnum, target_sesno, target_time)
        .await
}

/// 跑一次旧 `direct_*` 命名空间的清扫（[`render_legacy_prefix_sweep_once`]）。
///
/// [`persist_dbnum`] 的收尾自己会调它，这条另外露出来是给运维口：一个库已经整库重生成过
/// 了、想单独把旧行收掉，不必再整库落一遍。两道守卫在语句里，什么时候调都不会误删。
pub async fn sweep_legacy_prefixes(target: PersistTarget<'_>) -> anyhow::Result<()> {
    target
        .db
        .query(publication_transaction(&render_legacy_prefix_sweep_once()))
        .await
        .and_then(|response| response.check())
        .context("清扫旧 direct_* 命名空间")?;
    Ok(())
}

/// 把一个库当前投影里的每一根都对齐进库，一根切成若干块提交。
///
/// 整库生成之后用它收尾：生成侧只写进程内投影，这一步才让 Plant UI 看得见。逐根落而不是
/// 一整批落，是为了让一根的失败只赔掉那一根。
pub async fn persist_dbnum(dbnum: u32) -> anyhow::Result<PersistedRows> {
    persist_dbnum_in_chunks(dbnum, PERSIST_CHUNK).await
}

/// [`persist_dbnum`] 的可调块大小形态。给量报文规模的烟测用；生产走 [`PERSIST_CHUNK`]。
pub async fn persist_dbnum_in_chunks(
    dbnum: u32,
    chunk_size: usize,
) -> anyhow::Result<PersistedRows> {
    let target = PersistTarget::process();
    let store = ModelMemoryStore::global();
    let mut total = PersistedRows::none(target.medium);
    for root in store.roots(ProjectionScope::Current) {
        if root.dbno() != dbnum {
            continue;
        }
        // 全库生成只清房间边、不逐元素排重算（ADR-010 §4 收窄 1：整库以启动全量房间
        // 重建收尾，给每个元素排一次重算等于把那件事做两遍）。
        let rows = persist_root_in_chunks(target, store, root, chunk_size).await?;
        total.upserted += rows.upserted;
        total.removed += rows.removed;
        total.receipts += rows.receipts;
        total.commits += rows.commits;
    }
    // 整库重写完了才轮到旧命名空间：这一刻它才真的悬空。自带两道守卫（这个库扫过没有、
    // 旧命名空间还有没有活引用），所以放在这里不需要调用方判断什么时候该扫。
    sweep_legacy_prefixes(target).await?;
    Ok(total)
}

/// [`persist_root`] 的可注入形态：落库目标与存储都由调用方给。
pub async fn persist_root_on(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
) -> anyhow::Result<PersistedRows> {
    // 逐根那条路也写 `inst_relate` / `tubi_relate`（`/model/ensure`、未退役的文件轮次），
    // 它一动，合块那边的任务基线快照就不再等于库——作废，下一次重读。
    invalidate_geometry_baseline();
    persist_root_in_chunks(target, store, root, PERSIST_CHUNK).await
}

/// [`persist_root_on`] 的可调块大小形态。块大小只有测试才需要拨。
async fn persist_root_in_chunks(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
    chunk_size: usize,
) -> anyhow::Result<PersistedRows> {
    persist_root_chunks_impl(target, store, root, chunk_size).await
}

/// 一组最多装这么多根（[`persist_roots_batched`]）：ADR-071 D2 的跨根合块（d-932 保留），
/// 7997 六千多根不再是六千多笔事务。组的大小只是这一个常量：组内按 `tx_limits` 的字节上限合块、
/// 一组一批事务；每根的房间边紧跟它自己的模型行、同一块同一事务（ADR-073 D1 / W2），一根一组
/// 还是几百根一组都是同一条路。房间算不出来的根在写库**之前**就剔出组、单独记失败（ADR-073 D2）；
/// 写库失败的赔付边界是根：同组其它根继续独立提交。
pub const BATCHED_PERSIST_ROOTS_PER_GROUP: usize = 512;

/// 单条几何记录渲成语句之后的硬上限（与 [`persist_root_chunks_impl`] 同值）。
const PUBLICATION_STATEMENT_HARD_CAP: usize = 8 * 1024 * 1024;

/// 一次合块落库的账（ADR-071 / ADR-073 D1）。
#[derive(Debug, Clone, Default)]
pub struct BatchedPersist {
    /// 成功提交的行数账（介质随目标）。
    pub rows: Option<PersistedRows>,
    /// 没落进去的根与原因：房间算不出来（写前剔出组，原因带「room membership」）、或所在组
    /// 任一块写库失败，整根都在这里。
    pub failed: Vec<(RefNo, String)>,
    /// Typed form consumed by the write-behind state machine.  `failed` stays
    /// as the compatibility view for synchronous callers.
    pub(crate) failures: Vec<RootPersistFailure>,
    /// Durable transaction results which have not yet been accepted as a
    /// process-local ACK.  The write-behind coordinator performs that exact
    /// identity check after the adapter returns.
    pub(crate) acknowledged: Vec<ExactAckCandidate>,
    /// 实际发出的组数（≤ [`BATCHED_PERSIST_ROOTS_PER_GROUP`] 根一组）。
    pub groups: usize,
    /// 随模型行同事务写下的房间归属边数（ADR-073 D1）。
    pub room_edges: usize,
    /// 本批真正提交过的最大一笔 SurrealQL 事务字节数。
    pub max_transaction_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistFailureKind {
    Retryable,
    Blocked,
}

#[derive(Debug, Clone)]
pub(crate) struct RootPersistFailure {
    pub root: RefNo,
    pub kind: PersistFailureKind,
    pub error: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ExactAckCandidate {
    pub root: RefNo,
    pub ack: crate::fast_model::model_memory_store::RootPublicationAck,
}

impl BatchedPersist {
    fn push_failure(&mut self, root: RefNo, kind: PersistFailureKind, error: String) {
        self.failed.push((root, error.clone()));
        self.failures.push(RootPersistFailure { root, kind, error });
    }
}

/// 库里既有的一条 e3d-model 几何身份及其祖先链（对账用，一库一次取回）。
#[derive(Debug, serde::Deserialize)]
struct ExistingGeometry {
    geometry_id: GeometryId,
    /// 祖先链；历史行可能没有（`NONE`），那样的行不归任何根、也不会被当陈旧行删。
    #[serde(default)]
    anc: Option<Vec<i64>>,
}

impl Clone for ExistingGeometry {
    fn clone(&self) -> Self {
        Self {
            geometry_id: self.geometry_id.clone(),
            anc: self.anc.clone(),
        }
    }
}

impl ExistingGeometry {
    fn under(&self, packed_root: i64) -> bool {
        self.anc
            .as_deref()
            .is_some_and(|anc| anc.contains(&packed_root))
    }
}

/// 任务范围内的「库里现有几何身份」快照。
///
/// 对账读原先**每调一次 [`persist_roots_batched`] 就整库扫一遍**。7997 那一轮实测：68 组
/// 累计读约 180 万行，单笔事务从头 400 ms 涨到尾 1,280 ms——写入行数一路没变，涨的全是这个读。
/// 典型的 O(N²)：读的是「这个库**至今为止**的全部几何身份」，而写的人正是自己。
///
/// ADR-071 D1 说得明白：任务是这个 dbnum 唯一的写者，**它知道自己写了什么**。所以一个任务
/// 读一次基线，之后每组落完把自己写的那部分就地更新进来——快照始终等于「现在再读一次会得到
/// 的东西」，不是近似。
///
/// 作用域绑在**数据窗口**上（`active_data_window()`）：换窗即失效；窗口不在（`/model/ensure`
/// 那类一根一调的路）就不缓存，照旧每次读。任一组落库失败 → 整份作废（那一组可能已经提交了
/// 前几块，快照不再可信）。
struct GeometryBaseline {
    window: (u32, i32),
    dbnums: BTreeSet<u32>,
    rows: Vec<ExistingGeometry>,
}

static GEOMETRY_BASELINE: std::sync::Mutex<Option<GeometryBaseline>> = std::sync::Mutex::new(None);

fn geometry_baseline() -> std::sync::MutexGuard<'static, Option<GeometryBaseline>> {
    GEOMETRY_BASELINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 别的写者动过 `inst_relate` / `tubi_relate` 之后调：快照不再等于库。
pub(crate) fn invalidate_geometry_baseline() {
    *geometry_baseline() = None;
}

/// 取这一轮的对账依据：命中窗口且覆盖了要的库就用内存那份，否则读一次并存下。
async fn existing_geometry_for_window(
    db: &Surreal<Any>,
    dbnums: &BTreeSet<u32>,
    window: Option<(u32, i32)>,
) -> anyhow::Result<Vec<ExistingGeometry>> {
    let Some(window) = window else {
        return existing_geometry_of_dbnums_on(db, dbnums).await;
    };
    if let Some(cached) = geometry_baseline().as_ref()
        && cached.window == window
        && dbnums.is_subset(&cached.dbnums)
    {
        return Ok(cached.rows.clone());
    }
    let rows = existing_geometry_of_dbnums_on(db, dbnums).await?;
    *geometry_baseline() = Some(GeometryBaseline {
        window,
        dbnums: dbnums.clone(),
        rows: rows.clone(),
    });
    Ok(rows)
}

/// 一组落成之后把快照推到「刚写完」的状态：这些根名下的旧身份换成刚投影的那批。
///
/// 只对**做过对账**的根动手（`reconciliation_ready`）——没回执的根本来就不参与对账，
/// 它在库里的行也没被这一组动过。只传真落进去的根（房间关卡剔掉的根没动库，快照不该跟着动）。
fn advance_geometry_baseline(window: Option<(u32, i32)>, group: &[LiveShare<'_>]) {
    let Some(window) = window else {
        return;
    };
    let mut baseline = geometry_baseline();
    let Some(cached) = baseline.as_mut().filter(|cached| cached.window == window) else {
        return;
    };
    for share in group
        .iter()
        .map(|live| live.share)
        .filter(|share| share.reconciliation_ready())
    {
        let packed_root = packed_refno(share.root);
        cached.rows.retain(|row| !row.under(packed_root));
        cached
            .rows
            .extend(share.records.iter().map(|record| ExistingGeometry {
                geometry_id: record.geometry_id.clone(),
                anc: Some(record.ancestors.clone()),
            }));
    }
}

/// 一根在合块里的份：记录、回执、能不能拿它做对账。
struct RootShare {
    root: RefNo,
    records: Vec<ProjectedGeometry>,
    receipt: Option<RootReceipt>,
}

impl RootShare {
    fn load(store: &ModelMemoryStore, root: RefNo) -> anyhow::Result<Self> {
        let scope = ProjectionScope::Current;
        let records = store.records_of_root(scope, root);
        let receipt = store.receipt(scope, root);
        if let Some(receipt) = receipt.as_ref() {
            anyhow::ensure!(
                receipt.geometry_count == records.len(),
                "root {root} projection is incomplete: receipt expects {} geometries, store has {}",
                receipt.geometry_count,
                records.len()
            );
            let packed_root = packed_refno(root);
            anyhow::ensure!(
                records.iter().all(|record| {
                    record.source_sesno == receipt.source_sesno
                        && record.ancestors.contains(&packed_root)
                }),
                "root {root} projection contains records outside its receipt scope"
            );
        }
        Ok(Self {
            root,
            records,
            receipt,
        })
    }

    /// 有回执才能拿投影当「全集」去对账；没有回执的根可能只是半份或刚恢复的视图
    /// （与 [`persist_root_chunks_impl`] 的 `reconciliation_ready` 同一条规矩）。
    fn reconciliation_ready(&self) -> bool {
        self.receipt.is_some()
    }
}

/// 过了房间关卡、要进本组事务的一根（[`room_stage`] 的产物）。
///
/// 渲染用的记录是该根自己的全集。嵌套根与祖先根共有记录会重复发送、由幂等写收敛；
/// 事务成功前不得把“认领”传播给下一根。
struct LiveShare<'a> {
    share: &'a RootShare,
    /// 本根自己的完整记录，保持投影顺序。跨根重复依靠幂等写收敛；事务成功前不传播认领。
    claimed: Vec<ProjectedGeometry>,
    /// 这一根两张房间边的语句（`;\n` 收尾；没有计算器或没有几何时为空）。
    room_statements: String,
    /// 算出来的成员边数（`room_relate`）。
    room_edges: usize,
}

/// 房间关卡（ADR-073 D1 / D2）：写库**之前**逐根算房间归属——输入只有该根的内存投影与任务开工时
/// 冻住的计算器，一句库不读。算不出来（盒子不可用、面板网格不全）的根在这里就剔出本组、带原因
/// 记进失败名单，房间失败 = 该根失败；其余根照常往下走。与组大小无关：一组一根或一组几百根都是
/// 同一条路。`rooms` 为 `None`（房间开关关闭，或显式的底层适配器调用）时不算也不写新边，模型根仍
/// 往下持久化，只由对账那一半清掉被删来源的旧边。
fn room_stage<'a>(
    group: &'a [RootShare],
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> (Vec<LiveShare<'a>>, Vec<(RefNo, String)>) {
    let mut live = Vec::with_capacity(group.len());
    let mut failed = Vec::new();
    for share in group {
        // 每根事务携带自己的全集。祖先根失败后，随后嵌套根仍能独立写下共享记录与房间边。
        let claimed = share.records.clone();
        let (room_statements, room_edges) = match rooms {
            Some(calculator) if !claimed.is_empty() => {
                match calculator.calculate_for_root(&claimed) {
                    Ok(edges) => (
                        calculator.render_root_room_edges(&claimed, &edges),
                        edges.len(),
                    ),
                    Err(error) => {
                        // 这一根一行都不落；同组更晚的根始终携带自己的完整记录。
                        failed.push((
                            share.root,
                            format!(
                                "room membership of root {} could not be computed: {error:#}",
                                share.root
                            ),
                        ));
                        continue;
                    }
                }
            }
            _ => (String::new(), 0),
        };
        live.push(LiveShare {
            share,
            claimed,
            room_statements,
            room_edges,
        });
    }
    (live, failed)
}

/// 库里这些 dbnum 名下、由 e3d-model 写过的全部几何身份，**一条查询**取回（两张表各一句）。
/// 各根在客户端按祖先链分摊——比 [`projected_geometry_ids_on`] 每根一问少几千个往返。
/// 只认带 `dbnum` 列的行：没有它的历史行由启动回填（`backfill_inst_relate_anc`）补上。
async fn existing_geometry_of_dbnums_on(
    db: &Surreal<Any>,
    dbnums: &BTreeSet<u32>,
) -> anyhow::Result<Vec<ExistingGeometry>> {
    if dbnums.is_empty() {
        return Ok(Vec::new());
    }
    let dbnums = dbnums
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut response = db
        .query(format!(
            "SELECT direct_model.geometry_id AS geometry_id, anc FROM inst_relate \
             WHERE dbnum IN [{dbnums}] AND direct_model.source = 'e3d-model' \
             AND direct_model.geometry_id != NONE;\
             SELECT direct_model.geometry_id AS geometry_id, anc FROM tubi_relate \
             WHERE dbnum IN [{dbnums}] AND direct_model.source = 'e3d-model' \
             AND direct_model.geometry_id != NONE;"
        ))
        .await?
        .check()?;
    let mut rows: Vec<ExistingGeometry> = response.take(0)?;
    rows.extend(response.take::<Vec<ExistingGeometry>>(1)?);
    Ok(rows)
}

/// 多根一条的可见性栅栏（[`render_publication_fence`] 的批量形态）：`UPDATE a, b, c SET …`，
/// 每条最多点名 `per_statement` 根。
fn render_publication_fence_batch(roots: &[RefNo], per_statement: usize) -> String {
    let mut statements = String::new();
    for chunk in roots.chunks(per_statement.max(1)) {
        let targets = chunk
            .iter()
            .map(|root| format!("type::thing('gen_root', {})", sql_string(&refno_key(*root))))
            .collect::<Vec<_>>()
            .join(", ");
        statements.push_str(&format!(
            "UPDATE {targets} SET publication_status='publishing', updated_at=time::now();\n"
        ));
    }
    statements
}

/// ADR-071 D1 / D11 × ADR-073 D1：一个 dbnum 任务把本轮全部成功根的模型行合块、同步写进库，
/// 每根紧跟着它自己的两张房间归属边，同一块、同一笔事务、模型在前。
///
/// 与 [`persist_root_on`] 同一套行与同一套纪律——按记录 id 先删后插、分支重置与对账删除在首块、
/// 回执在末块（之前的块用栅栏标出中间态）、`spatial_epoch` 随动几何的块 +1、被对账掉的来源清边、
/// 混源守卫在写之前与盖章同事务各发一次——区别有三处：
///
/// 1. 一组 ≤ [`BATCHED_PERSIST_ROOTS_PER_GROUP`] 根，共享对账读取与房间预计算；持久化时每根的
///    几何行、房间边和回执合成一笔事务。软字节上限不得拆根，超过数据库硬上限则整根失败；
/// 2. 对账读一次：整库 e3d-model 几何身份一条查询取回，各根在客户端按祖先链分摊，不再每根一问；
/// 3. 没有 durable per-root queue。事务成功后返回精确 ACK 候选；同步调用方立即接受，
///    write-behind 调用方则在自己的 submission epoch 栅栏后接受，进程退出即丢。
///
/// `rooms` 是任务开工时冻出来的房间计算器（[`frozen_room_calculator`]），`None` = 房间归属开关
/// 已关闭或显式底层调用不要求房间；模型根仍照常持久化，只清被
/// 对账掉来源的旧边、不写新边。有计算器时一根的边由
/// [`MemoryRoomCalculator::calculate_for_root`] 按该根的内存投影算——**不回读库**——
/// 算不出来（盒子不可用、面板网格不全）这一根在写库之前就剔出组、整根不落库（[`room_stage`]），
/// 房间失败 = 该根失败（ADR-073 D2），同组其余根照常。
///
/// 一根事务失败只记该根失败；同组其它根照常。嵌套根与祖先根各自携带完整记录，
/// 重复 `record_id` 由先删后插的幂等语义收敛。
///
/// [`MemoryRoomCalculator::calculate_for_root`]: crate::fast_model::room_model::MemoryRoomCalculator::calculate_for_root
pub async fn persist_roots_batched(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    roots: &[RefNo],
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BatchedPersist> {
    let result = persist_roots_batched_unacked(target, store, dbnum, roots, rooms).await?;
    for candidate in &result.acknowledged {
        let _ = store.record_publication_ack(
            ProjectionScope::Current,
            candidate.root,
            candidate.ack.clone(),
        );
    }
    Ok(result)
}

/// Persist roots without mutating process-local publication ACK state.
///
/// This is the durability seam used by write-behind: a process can disappear
/// after RocksDB commits but before the coordinator accepts these candidates.
pub(crate) async fn persist_roots_batched_unacked(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    roots: &[RefNo],
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BatchedPersist> {
    let limits = crate::data_interface::tx_limits::data_tx_limits();
    persist_roots_batched_with_chunk_bytes_unacked(
        target,
        store,
        dbnum,
        roots,
        limits.max_bytes.max(64 * 1024),
        rooms,
    )
    .await
}

/// 任务开工时冻一份房间计算器（ADR-073 D1，硬约束：预加载房间模型只能是开工时冻结的不可变引用，
/// 任务期间不换代）。`Ok(None)` 只表示房间归属开关已关闭；开关开启但常驻模型
/// 不存在、在建、降级或阻塞时是 `Err`——这一轮的根都算不出房间，调用方按
/// 「房间失败 = 该根失败」记进 `failed`，下一次任务重生成。
///
/// 这里不回读库、不建模型、不换代：[`current_for_calculation`] 只取启动段装好的那份 `Arc`，
/// 计算器把面板索引建在内存里；任务期间常驻模型换代与否与这份引用无关。
///
/// [`current_for_calculation`]: crate::fast_model::room_resident::current_for_calculation
pub fn frozen_room_calculator()
-> anyhow::Result<Option<crate::fast_model::room_model::MemoryRoomCalculator>> {
    if !crate::options::room_membership() {
        return Ok(None);
    }
    let model = crate::fast_model::room_resident::current_for_calculation()?;
    let mesh_dir = aios_core::get_db_option().get_meshes_path();
    crate::fast_model::room_model::MemoryRoomCalculator::new(model, mesh_dir).map(Some)
}

/// 任务准入时冻结的 resident generation；房间开关关闭时没有这道完成要求。
pub fn required_room_generation() -> anyhow::Result<Option<String>> {
    if !crate::options::room_membership() {
        return Ok(None);
    }
    Ok(Some(
        crate::fast_model::room_resident::current_for_calculation()?
            .manifest
            .key
            .clone(),
    ))
}

/// [`persist_roots_batched`] 的显式块字节上限形态（测试把它压到 1 让每条记录自成一块）。
pub(crate) async fn persist_roots_batched_with_chunk_bytes(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    roots: &[RefNo],
    chunk_bytes: usize,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BatchedPersist> {
    let result = persist_roots_batched_with_chunk_bytes_unacked(
        target,
        store,
        dbnum,
        roots,
        chunk_bytes,
        rooms,
    )
    .await?;
    for candidate in &result.acknowledged {
        let _ = store.record_publication_ack(
            ProjectionScope::Current,
            candidate.root,
            candidate.ack.clone(),
        );
    }
    Ok(result)
}

async fn persist_roots_batched_with_chunk_bytes_unacked(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    roots: &[RefNo],
    chunk_bytes: usize,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<BatchedPersist> {
    let mut out = BatchedPersist::default();
    if roots.is_empty() {
        return Ok(out);
    }
    for root in roots {
        anyhow::ensure!(
            root.dbno() == dbnum,
            "root {root} belongs to dbnum {}, not the batch's dbnum {dbnum}",
            root.dbno()
        );
    }
    let db = target.db;
    // 只给对账基线快照当作用域键（[`GeometryBaseline`]：换窗即失效）。它**不再**分叉任何写法：
    // 清边 / 空间收敛在窗口内外一律就地做（ADR-072 D4）。
    let baseline_window = crate::data_interface::batch_worker::active_data_window();

    // 去重后的根序（同一根点两次只落一次）。
    let mut seen = BTreeSet::new();
    let roots = roots
        .iter()
        .copied()
        .filter(|root| seen.insert(packed_refno(*root)))
        .collect::<Vec<_>>();
    let mut shares = Vec::with_capacity(roots.len());
    for root in roots {
        match RootShare::load(store, root) {
            Ok(share) => shares.push(share),
            Err(error) => out.push_failure(root, PersistFailureKind::Blocked, format!("{error:#}")),
        }
    }
    if shares.is_empty() {
        return Ok(out);
    }

    // 对账读一次（只有有回执的根才拿得起自己的全集）。行上的 `dbnum` 是投影记录写下的那个，
    // 与根的 `dbno()` 在生产上同值；两者都问，夹具或历史行的口径差异不会让陈旧行漏网。
    let mut reconciliation_dbnums = BTreeSet::from([dbnum]);
    reconciliation_dbnums.extend(
        shares
            .iter()
            .flat_map(|share| share.records.iter().map(|record| record.dbnum)),
    );
    let existing = if shares.iter().any(RootShare::reconciliation_ready) {
        existing_geometry_for_window(db, &reconciliation_dbnums, baseline_window).await?
    } else {
        Vec::new()
    };

    let mut total = PersistedRows::none(target.medium);
    let mut refresh_all = Vec::<aios_core::RefnoEnum>::new();
    let mut boxes_all = Vec::<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>::new();
    let mut cleared_all = Vec::<aios_core::RefnoEnum>::new();

    for group in shares.chunks(BATCHED_PERSIST_ROOTS_PER_GROUP) {
        out.groups += 1;
        // 房间关卡在写库之前、按根（ADR-073 D1 / D2）：算不出来的根就地记失败，
        // 剩下的根才进本组的事务。
        let (live, room_failed) = room_stage(group, rooms);
        for (root, error) in room_failed {
            out.push_failure(root, PersistFailureKind::Blocked, error);
        }
        if live.is_empty() {
            continue;
        }
        // 正常路径把最多 512 根合进同一笔原子事务，避免大库退化成几万次网络往返。
        // 若 SQL 硬上限、数据事件或传输错误让整组失败，事务本身零提交；随后二分重试，
        // 直到定位到单根。于是成功路径保留合块吞吐，异常路径仍以根为赔付边界。
        match persist_group(
            db,
            &live,
            &existing,
            chunk_bytes,
            !target.medium.is_durable(),
        )
        .await
        {
            Ok(outcome) => accept_group_outcome(
                &live,
                dbnum,
                target.medium,
                rooms,
                baseline_window,
                outcome,
                &mut total,
                &mut out,
                &mut refresh_all,
                &mut boxes_all,
                &mut cleared_all,
            ),
            Err(error) if live.len() == 1 => {
                invalidate_geometry_baseline();
                out.push_failure(
                    live[0].share.root,
                    PersistFailureKind::Retryable,
                    format!("{error:#}"),
                );
            }
            Err(error) => {
                invalidate_geometry_baseline();
                log::warn!(
                    "model publication group of {} roots failed; bisecting to preserve the per-root failure boundary: {error:#}",
                    live.len()
                );
                let mut retries = VecDeque::new();
                let middle = live.len() >> 1;
                retries.push_back((0usize, middle));
                retries.push_back((middle, live.len()));
                while let Some((start, end)) = retries.pop_front() {
                    let retry = &live[start..end];
                    let current = if retry.iter().any(|root| root.share.reconciliation_ready()) {
                        existing_geometry_of_dbnums_on(db, &reconciliation_dbnums).await?
                    } else {
                        Vec::new()
                    };
                    match persist_group(
                        db,
                        retry,
                        &current,
                        chunk_bytes,
                        !target.medium.is_durable(),
                    )
                    .await
                    {
                        Ok(outcome) => accept_group_outcome(
                            retry,
                            dbnum,
                            target.medium,
                            rooms,
                            baseline_window,
                            outcome,
                            &mut total,
                            &mut out,
                            &mut refresh_all,
                            &mut boxes_all,
                            &mut cleared_all,
                        ),
                        Err(error) if retry.len() == 1 => {
                            invalidate_geometry_baseline();
                            out.push_failure(
                                retry[0].share.root,
                                PersistFailureKind::Retryable,
                                format!("{error:#}"),
                            );
                        }
                        Err(error) => {
                            invalidate_geometry_baseline();
                            log::warn!(
                                "model publication subgroup of {} roots failed; bisecting again: {error:#}",
                                retry.len()
                            );
                            let middle = start + (retry.len() >> 1);
                            retries.push_back((start, middle));
                            retries.push_back((middle, end));
                        }
                    }
                }
            }
        }
    }

    // 行已经提交，进程内空间树还停在旧盒上——就地收敛：盒是刚落库那批记录的 `world_bounds`
    // （ADR-072 D4），不回读库，也不看在不在数据窗口里。
    if target.converge_spatial && total.commits > 0 {
        crate::fast_model::aabb_tree::converge_tree_after_persist(
            &refresh_all,
            boxes_all,
            &cleared_all,
        )
        .await;
    }
    out.rows = Some(total);
    Ok(out)
}

/// 一组落完的账：行数 + 提交之后要收敛进空间树的名单与盒。
struct GroupOutcome {
    upserted: usize,
    removed: usize,
    receipts: usize,
    commits: usize,
    /// 随模型行同事务写下的房间归属边数。
    room_edges: usize,
    max_transaction_bytes: usize,
    /// 写了新几何的来源（隐式管身不算）。
    refresh: Vec<aios_core::RefnoEnum>,
    /// `refresh` 的投影世界盒（[`crate::fast_model::aabb_tree::projected_tree_entry`]）。
    boxes: Vec<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>,
    /// 被对账掉、房间边也一并清掉的来源。
    cleared: Vec<aios_core::RefnoEnum>,
}

#[allow(clippy::too_many_arguments)]
fn accept_group_outcome(
    group: &[LiveShare<'_>],
    dbnum: u32,
    medium: PersistMedium,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
    baseline_window: Option<(u32, i32)>,
    outcome: GroupOutcome,
    total: &mut PersistedRows,
    out: &mut BatchedPersist,
    refresh_all: &mut Vec<aios_core::RefnoEnum>,
    boxes_all: &mut Vec<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>,
    cleared_all: &mut Vec<aios_core::RefnoEnum>,
) {
    for root in group {
        if let Some(receipt) = root.share.receipt.as_ref() {
            out.acknowledged.push(ExactAckCandidate {
                root: root.share.root,
                ack: crate::fast_model::model_memory_store::RootPublicationAck {
                    dbnum,
                    source_sesno: receipt.source_sesno,
                    geometry_count: receipt.geometry_count,
                    manifest_hash: receipt.manifest_hash.clone(),
                    model_target: receipt.model_target.clone(),
                    room_membership: rooms.is_some(),
                    room_generation: rooms
                        .map(|calculator| calculator.resident_generation().to_string()),
                    medium,
                },
            });
        }
    }
    total.upserted += outcome.upserted;
    total.removed += outcome.removed;
    total.receipts += outcome.receipts;
    total.commits += outcome.commits;
    out.room_edges += outcome.room_edges;
    out.max_transaction_bytes = out.max_transaction_bytes.max(outcome.max_transaction_bytes);
    refresh_all.extend(outcome.refresh);
    boxes_all.extend(outcome.boxes);
    cleared_all.extend(outcome.cleared);
    advance_geometry_baseline(baseline_window, group);
}

/// 一组过了房间关卡的根：渲块 → 逐块提交。任一块失败即返回 Err，调用方把这一组记成失败。
/// 每根的顺序定死（ADR-073 D1，用户原话「放到模型计算后面」）：最终几何已在投影里 → 房间归属已在
/// [`room_stage`] 算好 → 写模型行与房间边（同一块同一事务，模型在前）→ 盖章。
async fn persist_group(
    db: &Surreal<Any>,
    group: &[LiveShare<'_>],
    existing: &[ExistingGeometry],
    chunk_bytes: usize,
    create_missing_receipts: bool,
) -> anyhow::Result<GroupOutcome> {
    // 本根要渲的完整记录；跨根重复由数据库幂等语义收敛。
    let records = group
        .iter()
        .flat_map(|live| live.claimed.iter().cloned())
        .collect::<Vec<_>>();

    // 对账：每根自己的全集 vs 库里挂在它祖先链下的身份；库里有、存储里没有的就是陈旧行。
    let mut removals = Vec::<String>::new();
    let mut removed_ids = BTreeSet::<String>::new();
    let mut removed_geometry = Vec::<GeometryId>::new();
    let mut branches = BTreeSet::<(u32, u32)>::new();
    for share in group
        .iter()
        .map(|live| live.share)
        .filter(|share| share.reconciliation_ready())
    {
        let packed_root = packed_refno(share.root);
        let projected = share
            .records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect::<BTreeSet<_>>();
        for record in share.records.iter().filter(|record| record.tube.is_some()) {
            branches.insert((record.source_refno.word0, record.source_refno.word1));
        }
        for row in existing.iter().filter(|row| row.under(packed_root)) {
            let source = geometry_source_refno(&row.geometry_id)?;
            let record_id = geometry_record_id(&row.geometry_id, source);
            if projected.contains(record_id.as_str()) || !removed_ids.insert(record_id.clone()) {
                continue;
            }
            if matches!(row.geometry_id, GeometryId::ImpliedTube { .. }) {
                branches.insert((source.word0, source.word1));
            }
            removals.push(record_id);
            removed_geometry.push(row.geometry_id.clone());
        }
    }

    let cleared_room_edges = room_edges_to_clear(
        records.iter().map(|record| {
            (
                &record.geometry_id,
                record.source_refno,
                record.noun.as_str(),
            )
        }),
        &removed_geometry,
        &BTreeSet::new(),
    )?;

    let deferred_refresh = records
        .iter()
        .filter(|record| record.tube.is_none())
        .map(|record| refno_from_e3d(record.source_refno))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    // 树条目就是这批记录的世界盒——落库那一侧写进 `aabb.d` 的同一份值，提交后直接进树。
    let tree_boxes = records
        .iter()
        .filter_map(crate::fast_model::aabb_tree::projected_tree_entry)
        .collect::<Vec<_>>();

    // 几何段：**逐根**完整渲染，不因软块上限拆分。这一根的两张房间边
    // 紧跟在它最后一段几何后面（ADR-073 D1 / W2：模型在前、房间在后，同一段 = 同一块 = 同一笔事务）；
    // 只有回执没有几何的根（删干净了）没有边可写，它的旧边由上面的清边那一半处理。
    let mut pieces = Vec::<String>::new();
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    for live in group {
        let mut piece = if live.claimed.is_empty() {
            String::new()
        } else {
            render_chunk(&live.claimed, WriteMode::Incremental, &mut tube_slots)?
        };
        piece.push_str(&live.room_statements);
        if !piece.is_empty() {
            pieces.push(piece);
        }
    }
    // 几何块：相邻的段按字节装进同一块（合块允许跨根），一段不拆——一根的行与边始终在同一块里。
    let mut geometry_chunks = Vec::<String>::new();
    for piece in pieces {
        match geometry_chunks.last_mut() {
            Some(last) if last.len() + piece.len() <= chunk_bytes => last.push_str(&piece),
            _ => geometry_chunks.push(piece),
        }
    }

    // 回执块：同一份 ModelTarget 只 `LET` 一次，几百根共用；按字节切成若干块。
    let receipted = group
        .iter()
        .map(|live| live.share)
        .filter_map(|share| share.receipt.as_ref().map(|receipt| (share.root, receipt)))
        .collect::<Vec<_>>();
    let mut receipt_chunks = Vec::<String>::new();
    {
        let mut targets = BTreeMap::<String, usize>::new();
        let mut header = String::new();
        let mut body = String::new();
        let flush = |header: &mut String, body: &mut String, out: &mut Vec<String>| {
            if !body.is_empty() {
                out.push(format!("{header}{body}"));
            }
            header.clear();
            body.clear();
        };
        for (root, receipt) in &receipted {
            let literal = receipt_model_target_literal(receipt)?;
            let expression = if literal == "NONE" {
                literal
            } else {
                let next = targets.len();
                let index = *targets.entry(literal.clone()).or_insert(next);
                if index == next {
                    header.push_str(&format!("LET $mt{index} = {literal};\n"));
                }
                format!("$mt{index}")
            };
            body.push_str(&render_receipt_with_target(
                *root,
                receipt,
                None,
                &expression,
                create_missing_receipts,
            ));
            if header.len() + body.len() > chunk_bytes {
                flush(&mut header, &mut body, &mut receipt_chunks);
                targets.clear();
            }
        }
        flush(&mut header, &mut body, &mut receipt_chunks);
    }

    // 每根不可拆的几何/房间段与全部回执合成**一笔**事务。整组超过数据库硬上限时，
    // 调用方会在发出任何写之前收到 Err 并二分重试；软块上限只帮助渲染期组装，不再制造
    // “模型已提交、回执稍后提交”的中间态。
    let mut root_statement = geometry_chunks.drain(..).collect::<String>();
    for receipt in receipt_chunks.drain(..) {
        root_statement.push_str(&receipt);
    }
    if !root_statement.is_empty() {
        geometry_chunks.push(root_statement);
    }

    // 本组随模型行同事务写下的房间边数（各根在 [`room_stage`] 里算好的那份）。
    let room_edge_count = group.iter().map(|live| live.room_edges).sum::<usize>();

    let chunk_count = geometry_chunks.len() + receipt_chunks.len();
    let has_preamble =
        !removals.is_empty() || !branches.is_empty() || !cleared_room_edges.is_empty();
    let statement_count = chunk_count.max(usize::from(has_preamble));
    if statement_count == 0 {
        return Ok(GroupOutcome {
            upserted: 0,
            removed: 0,
            receipts: 0,
            commits: 0,
            room_edges: 0,
            max_transaction_bytes: 0,
            refresh: Vec::new(),
            boxes: Vec::new(),
            cleared: Vec::new(),
        });
    }
    let fenced = statement_count > 1 && !receipted.is_empty();
    let fence_roots = receipted.iter().map(|(root, _)| *root).collect::<Vec<_>>();

    let mut commits = 0usize;
    let mut max_transaction_bytes = 0usize;
    for index in 0..statement_count {
        let mut statements = String::new();
        let is_first = index == 0;
        let is_last = index + 1 == statement_count;
        if is_first {
            // 多块时把中间态标在凭证行上。
            if fenced {
                statements.push_str(&render_publication_fence_batch(&fence_roots, 500));
            }
            for &(word0, word1) in &branches {
                append_tube_branch_reset(&mut statements, RefNo::new(word0, word1));
            }
            append_geometry_removals(
                &mut statements,
                &removals.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            // 清边跟着删几何那一块走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
            statements.push_str(&render_room_edge_clear(&cleared_room_edges));
        }
        let touches_geometry;
        if index < geometry_chunks.len() {
            statements.push_str(&geometry_chunks[index]);
            touches_geometry = true;
        } else {
            touches_geometry = is_first && (!removals.is_empty() || !branches.is_empty());
            if let Some(receipt_chunk) = receipt_chunks.get(index - geometry_chunks.len()) {
                statements.push_str(receipt_chunk);
            }
        }
        if is_last {}
        if touches_geometry {
            statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            statements.push('\n');
        }
        if statements.is_empty() {
            continue;
        }
        anyhow::ensure!(
            statements.len() <= PUBLICATION_STATEMENT_HARD_CAP,
            "root {} exceeds publication transaction hard cap ({} > {} bytes)",
            group[0].share.root,
            statements.len(),
            PUBLICATION_STATEMENT_HARD_CAP
        );
        max_transaction_bytes = max_transaction_bytes.max(statements.len());
        db.query(publication_transaction(&statements))
            .await
            .map_err(anyhow::Error::from)
            .and_then(check_publication_response)
            .with_context(|| {
                format!(
                    "persist {} projected root(s)（第 {} / {statement_count} 块）",
                    group.len(),
                    index + 1
                )
            })?;
        commits += 1;
    }

    Ok(GroupOutcome {
        upserted: records.len(),
        removed: removals.len(),
        receipts: receipted.len(),
        commits,
        room_edges: room_edge_count,
        max_transaction_bytes,
        refresh: deferred_refresh,
        boxes: tree_boxes,
        cleared: cleared_room_edges,
    })
}

/// The WS router can retain a pending response until reconnection. A deadline
/// bounds that uncertain wait; callers hold the job and inspect durable ACKs
/// before any replay. Each chunk gets its own deadline so a large root is not
/// limited by the total time of all its successful chunks.
pub(crate) async fn publication_request<T>(
    request: impl std::future::Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    publication_request_with_timeout(request, PUBLICATION_BASE_TIMEOUT).await
}

/// 单根单块请求的期限；批量请求按根数与字节数往上加，见 [`publication_timeout_for`]。
const PUBLICATION_BASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PUBLICATION_TIMEOUT_CAP: std::time::Duration = std::time::Duration::from_secs(180);

/// 一次数据库请求的期限随它的体量走：30 s 是单根的口径，32 根 / 4 MiB 的批量请求在
/// 库端排在一轮空间发布后面时，同一个 30 s 会把整批一起判成不确定（2026-09-06 run-ad
/// 16:38 就是这样停下的）。每多一根加 1 s，每 MiB SQL 加 8 s，上限 180 s。
pub(crate) fn publication_timeout_for(roots: usize, sql_bytes: usize) -> std::time::Duration {
    let per_root = std::time::Duration::from_secs(roots.saturating_sub(1) as u64);
    let per_mib = std::time::Duration::from_secs(8 * (sql_bytes / (1024 * 1024)) as u64);
    (PUBLICATION_BASE_TIMEOUT + per_root + per_mib).min(PUBLICATION_TIMEOUT_CAP)
}

pub(crate) async fn publication_request_with_timeout<T>(
    request: impl std::future::Future<Output = anyhow::Result<T>>,
    timeout: std::time::Duration,
) -> anyhow::Result<T> {
    tokio::time::timeout(timeout, request).await
        .map_err(|_| anyhow::anyhow!("model publication database request timed out; retain result and reconcile database ACK before replay"))?
}

/// 读库镜像与换进内存之间被并发提交插了队：镜像已旧，重读 [`RESTORE_ATTEMPTS`] 次仍撞上
/// 就抛这个，交给调用方推迟（收口每轮都会重新驱动）。
#[cfg(test)]
#[derive(Debug)]
pub struct ProjectionRestoreConflict {
    pub dbnum: u32,
    pub attempts: usize,
}

#[cfg(test)]
impl std::fmt::Display for ProjectionRestoreConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "model projection restore for dbnum {} lost the race with concurrent memory projection commits {} times; the database image was stale and was not swapped in",
            self.dbnum, self.attempts
        )
    }
}

#[cfg(test)]
impl std::error::Error for ProjectionRestoreConflict {}

/// 读库镜像撞上并发提交时原地重读几次。再撞就不占着收口这一轮了。
#[cfg(test)]
const RESTORE_ATTEMPTS: usize = 3;

/// Check whether a root is available in either layer without hydrating kv-mem.
/// A memory receipt means an incremental overlay exists; otherwise RocksDB is
/// queried only for availability and remains the cold-data provider.
pub async fn ensure_root_cached(root: RefNo) -> anyhow::Result<bool> {
    if !crate::options::store_mode().is_durable() {
        return Ok(ModelMemoryStore::global()
            .receipt(ProjectionScope::Current, root)
            .is_some());
    }
    ensure_root_cached_on(&aios_core::SUL_DB, ModelMemoryStore::global(), root).await
}

pub async fn ensure_root_cached_on(
    db: &Surreal<Any>,
    store: &ModelMemoryStore,
    root: RefNo,
) -> anyhow::Result<bool> {
    if store.receipt(ProjectionScope::Current, root).is_some() {
        store.note_cache_lookup(true);
        return Ok(true);
    }
    store.note_cache_lookup(false);
    store.note_cache_load_started();
    let root_id = format!("{}_{}", root.word0, root.word1);
    let result = db
        .query("SELECT VALUE publication_status = 'ready' FROM type::thing('gen_root', $root_id);")
        .bind(("root_id", root_id))
        .await
        .and_then(|response| response.check());
    store.note_cache_load_finished(result.is_ok());
    let mut response = result?;
    Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
}

/// Load exactly one complete committed root. Both geometry predicates are backed
/// by `idx_*_anc`; `gen_root` is a point lookup. An incomplete durable root fails
/// closed instead of becoming a cache entry.
#[cfg(test)]
async fn read_published_root_on(
    db: &Surreal<Any>,
    root: RefNo,
) -> anyhow::Result<Option<(Vec<ProjectedGeometry>, RootReceipt)>> {
    #[derive(serde::Deserialize)]
    struct ReceiptRow {
        source_end_sesno: u32,
        published_model_target: Option<ModelTarget>,
        published_manifest_hash: String,
        published_geometry_count: usize,
    }
    let root_id = format!("{}_{}", root.word0, root.word1);
    let mut response = db
        .query(
            "SELECT source_end_sesno,published_model_target,published_manifest_hash,\
             published_geometry_count FROM type::thing('gen_root', $root_id) \
             WHERE publication_status = 'ready';",
        )
        .bind(("root_id", root_id))
        .await?
        .check()?;
    let Some(row) = response.take::<Option<ReceiptRow>>(0)? else {
        return Ok(None);
    };
    let receipt = RootReceipt {
        source_sesno: row.source_end_sesno,
        model_target: row.published_model_target,
        manifest_hash: row.published_manifest_hash,
        geometry_count: row.published_geometry_count,
        published_via: crate::fast_model::model_memory_store::default_published_via(),
    };

    #[derive(serde::Deserialize)]
    struct GeometryRow {
        memory_projection: ProjectedGeometry,
    }
    let packed = packed_refno(root);
    let mut records = Vec::new();
    for table in ["inst_relate", "tubi_relate"] {
        let sql = root_projection_sql(table, root);
        let rows: Vec<GeometryRow> = db.query(sql).await?.check()?.take(0)?;
        records.extend(rows.into_iter().map(|row| row.memory_projection));
    }
    anyhow::ensure!(
        records.len() == receipt.geometry_count,
        "durable root {root} is incomplete: receipt expects {}, indexed rows returned {}",
        receipt.geometry_count,
        records.len()
    );
    anyhow::ensure!(
        records.iter().all(|record| {
            record.dbnum == root.dbno()
                && record.source_sesno == receipt.source_sesno
                && record.ancestors.contains(&packed)
        }),
        "durable root {root} contains rows outside its receipt scope"
    );
    Ok(Some((records, receipt)))
}

#[cfg(test)]
fn root_projection_sql(table: &str, root: RefNo) -> String {
    debug_assert!(matches!(table, "inst_relate" | "tubi_relate"));
    format!(
        "SELECT memory_projection FROM {table} WHERE anc CONTAINS {} \
         AND dbnum = {} AND memory_projection != NONE;",
        packed_refno(root),
        root.dbno()
    )
}

/// 用库里已提交的行整体换掉某个库的进程内投影。
///
/// 读镜像要翻很多页、页与页之间全是 await，而「只写内存」的按需生成同时还在往同一个库
/// 提交（`model_read_route` R2）。所以取镜像**之前**先记下提交纪元，换进去时由
/// [`ModelMemoryStore::restore_dbnum`] 校验：纪元变了就重读，不拿旧镜像盖掉那些只在内存
/// 里的记录。
#[cfg(test)]
async fn restore_projection_on(
    db: &Surreal<Any>,
    store: &ModelMemoryStore,
    dbnum: u32,
) -> anyhow::Result<()> {
    for _ in 0..RESTORE_ATTEMPTS {
        let epoch = store.commit_epoch(dbnum);
        let (records, receipts) = read_published_projection_on(db, dbnum).await?;
        if store.restore_dbnum(dbnum, records, receipts, epoch) {
            return Ok(());
        }
    }
    Err(ProjectionRestoreConflict {
        dbnum,
        attempts: RESTORE_ATTEMPTS,
    }
    .into())
}

/// Rehydrate only committed structured rows; no E3D reads and no SQL writes.
/// Primary-key ranges keep both result frames and work per page bounded.
#[cfg(test)]
async fn read_published_projection_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<(Vec<ProjectedGeometry>, Vec<(RefNo, RootReceipt)>)> {
    #[derive(serde::Deserialize)]
    struct GeometryRow {
        id: surrealdb::RecordId,
        memory_projection: ProjectedGeometry,
    }
    let mut records = Vec::new();
    for table in ["inst_relate", "tubi_relate"] {
        let mut cursor: Option<String> = None;
        loop {
            let from = cursor
                .as_ref()
                .map(|id| format!("{id}.."))
                .unwrap_or_else(|| table.to_owned());
            let index = if cursor.is_none() { "WITH NOINDEX" } else { "" };
            let after = cursor
                .as_ref()
                .map(|id| format!("AND id != {id}"))
                .unwrap_or_default();
            let sql = format!(
                "SELECT id,memory_projection FROM {from} {index} WHERE dbnum={dbnum} AND memory_projection != NONE {after} LIMIT 512;"
            );
            let rows: Vec<GeometryRow> = db.query(sql).await?.check()?.take(0)?;
            if rows.is_empty() {
                break;
            }
            let next = rows.last().unwrap().id.to_string();
            anyhow::ensure!(
                cursor.as_ref() != Some(&next),
                "projection recovery cursor did not advance"
            );
            cursor = Some(next);
            for row in rows {
                anyhow::ensure!(
                    row.memory_projection.dbnum == dbnum,
                    "projection recovery database mismatch"
                );
                records.push(row.memory_projection);
            }
        }
        let missing: Vec<surrealdb::RecordId> = db.query(format!("SELECT VALUE id FROM {table} WHERE dbnum={dbnum} AND direct_model.source='e3d-model' AND memory_projection = NONE LIMIT 1;"))
            .await?.check()?.take(0)?;
        anyhow::ensure!(
            missing.is_empty(),
            "committed projection is missing structured recovery data in {table}"
        );
    }
    #[derive(serde::Deserialize)]
    struct ReceiptRow {
        id: surrealdb::RecordId,
        source_end_sesno: u32,
        published_model_target: Option<crate::fast_model::e3d_model_service::ModelTarget>,
        published_manifest_hash: String,
        published_geometry_count: usize,
    }
    let mut receipts = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let from = cursor
            .as_ref()
            .map(|id| format!("{id}.."))
            .unwrap_or_else(|| "gen_root".to_owned());
        let index = if cursor.is_none() { "WITH NOINDEX" } else { "" };
        let after = cursor
            .as_ref()
            .map(|id| format!("AND id != {id}"))
            .unwrap_or_default();
        let rows: Vec<ReceiptRow> = db.query(format!("SELECT id,source_end_sesno,published_model_target,published_manifest_hash,published_geometry_count FROM {from} {index} WHERE dbnum={dbnum} AND publication_status='ready' {after} LIMIT 256;"))
            .await?.check()?.take(0)?;
        if rows.is_empty() {
            break;
        }
        let next = rows.last().unwrap().id.to_string();
        anyhow::ensure!(
            cursor.as_ref() != Some(&next),
            "receipt recovery cursor did not advance"
        );
        cursor = Some(next);
        for row in rows {
            let root_key = row
                .id
                .key()
                .to_string()
                .trim_matches('⟨')
                .trim_matches('⟩')
                .replace('_', "/");
            let root = crate::fast_model::e3d_model_service::parse_refno(&root_key)?;
            receipts.push((
                root,
                RootReceipt {
                    source_sesno: row.source_end_sesno,
                    model_target: row.published_model_target,
                    manifest_hash: row.published_manifest_hash,
                    geometry_count: row.published_geometry_count,
                    // 内部对账用的旧回执，不再发布，published_via 取默认即可。
                    published_via: crate::fast_model::model_memory_store::default_published_via(),
                },
            ));
        }
    }
    Ok((records, receipts))
}

async fn persist_root_chunks_impl(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
    chunk_size: usize,
) -> anyhow::Result<PersistedRows> {
    let db = target.db;
    let scope = ProjectionScope::Current;
    let records = store.records_of_root(scope, root);
    let receipt = store.receipt(scope, root);
    let reconciliation_ready = if let Some(receipt) = receipt.as_ref() {
        anyhow::ensure!(
            receipt.geometry_count == records.len(),
            "root {root} projection is incomplete: receipt expects {} geometries, store has {}",
            receipt.geometry_count,
            records.len()
        );
        let packed_root = packed_refno(root);
        anyhow::ensure!(
            records.iter().all(|record| {
                record.source_sesno == receipt.source_sesno
                    && record.ancestors.contains(&packed_root)
            }),
            "root {root} projection contains records outside its receipt scope"
        );
        true
    } else {
        false
    };
    let projected = records
        .iter()
        .map(|record| record.record_id.as_str())
        .collect::<BTreeSet<_>>();

    // 一个 BRAN 的隐式管身是整段重写的：这一轮碰到的分支先清空，再插存储里的那几段。
    let mut branches = records
        .iter()
        .filter(|record| record.tube.is_some())
        .map(|record| (record.source_refno.word0, record.source_refno.word1))
        .collect::<BTreeSet<_>>();
    if !reconciliation_ready {
        // No receipt means the store may be a partial or freshly restored view.
        // Upserts are safe, but absence is not evidence: do not reset tube
        // branches or derive database deletions from this projection.
        branches.clear();
    }
    let mut removals = Vec::new();
    let mut removed_geometry = Vec::new();
    let projected_geometry_ids = if !reconciliation_ready {
        Vec::new()
    } else {
        projected_geometry_ids_on(db, root).await?
    };
    for geometry_id in projected_geometry_ids {
        let source = geometry_source_refno(&geometry_id)?;
        let record_id = geometry_record_id(&geometry_id, source);
        if projected.contains(record_id.as_str()) {
            continue;
        }
        if matches!(geometry_id, GeometryId::ImpliedTube { .. }) {
            branches.insert((source.word0, source.word1));
        }
        removals.push(record_id);
        removed_geometry.push(geometry_id);
    }

    // 房间派生面（ADR-010 §4 / ADR-040 §3）。旧发布事务把这两笔挂在同一条事务上，
    // e3d-model 接管生成之后断了一段时间；现在从投影记录上重新折出来：
    // 拿到新几何的来源排一次房间重算，被移除的来源两个方向的房间边一并清掉。
    // `legacy_sources` 空着——那是 pre-e3d 清理路径才有的名单，落库这一侧没有。
    let cleared_room_edges = room_edges_to_clear(
        records.iter().map(|record| {
            (
                &record.geometry_id,
                record.source_refno,
                record.noun.as_str(),
            )
        }),
        &removed_geometry,
        &BTreeSet::new(),
    )?;
    // 清边跟着删除走、排重算跟着回执走，两笔不能对调：
    //
    // - 清边排在**删几何那一块**。删了行却把清边留到后面的块，中途失败就留下一批指着
    //   已删几何的房间边，而下一轮对账再也看不见那些几何（行已经没了），悬空边永远
    //   不会被清——`fn::room_relate_of` 照样把它们取出来。
    // - 排重算排在**盖章那一块**。任务一进队列就可能被消费，排在前面等于让房间重算
    //   照着只写了一半的几何算归属；排在最后，中途失败就整个不排，下一轮重新走一遍。

    let deferred_refresh = records
        .iter()
        .filter(|record| record.tube.is_none())
        .map(|record| refno_from_e3d(record.source_refno))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    // 树条目就是这批记录的世界盒——落库那一侧写进 `aabb.d` 的同一份值，提交后直接进树。
    let tree_boxes = records
        .iter()
        .filter_map(crate::fast_model::aabb_tree::projected_tree_entry)
        .collect::<Vec<_>>();

    // 大根切块提交（一块一个事务）。整根一笔在 7997 上是两笔各 2.6 万件的大报文——
    // 内存介质无所谓，ws 连出去就不一定了。
    //
    // 切开之后**这三条顺序是有讲究的**：
    //
    // - 分支重置与对账删除只进第一块。重置若排在后面的块，会把前面的块刚插进去的同分支
    //   管身删掉；管身槽位的发号表跨块延续，所以 id 也不会撞。
    // - 回执排在最后一块。一块失败就没有章，这根在窗口规划器眼里仍未发布，下一轮重生成，
    //   而下一轮的对账会把这一轮留下的半截行收干净——**先删后插 + 回执最后盖，让半途而废
    //   能自愈**，这也是敢切块的全部理由。
    // - 混源会话守卫发两次：第一块发一次是为了在写任何东西之前就失败，最后一块再发一次是
    //   为了与盖章在同一个事务里（check-then-write 不能跨事务）。
    // Keep geometry messages bounded by their rendered size, including the
    // structured restart projection. Record count alone cannot bound large roots.
    let mut chunks = Vec::new();
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    let mut pending = std::collections::VecDeque::from_iter(records.chunks(chunk_size.max(1)));
    while let Some(chunk) = pending.pop_front() {
        let mut slots = tube_slots.clone();
        let rendered = render_chunk(chunk, WriteMode::Incremental, &mut slots)?;
        if rendered.len() > 2 * 1024 * 1024 && chunk.len() > 1 {
            let (left, right) = chunk.split_at(chunk.len() >> 1);
            pending.push_front(right);
            pending.push_front(left);
            continue;
        }
        anyhow::ensure!(
            rendered.len() <= 8 * 1024 * 1024,
            "single geometry exceeds publication byte budget"
        );
        tube_slots = slots;
        chunks.push((chunk, rendered));
    }
    let chunk_count = chunks.len().max(1);
    let fenced = chunk_count > 1 && receipt.is_some();
    let mut commits = 0usize;
    for index in 0..chunk_count {
        let chunk = chunks.get(index).map(|(chunk, _)| *chunk).unwrap_or(&[]);
        let mut statements = String::new();
        if index == 0 {
            if fenced {
                statements.push_str(&render_publication_fence(root));
            }
            for &(word0, word1) in &branches {
                append_tube_branch_reset(&mut statements, RefNo::new(word0, word1));
            }
            append_geometry_removals(
                &mut statements,
                &removals.iter().map(String::as_str).collect::<Vec<_>>(),
            );
            // 清边跟着删几何那一块走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
            statements.push_str(&render_room_edge_clear(&cleared_room_edges));
        }
        if let Some((_, rendered)) = chunks.get(index) {
            statements.push_str(rendered);
        }
        if index + 1 == chunk_count {
            if let Some(receipt) = receipt.as_ref() {
                statements.push_str(&render_receipt_version(root, receipt, None)?);
            }
        }
        // 「凡是改变了树应有内容的已提交变更，都在同一事务内把 epoch +1」——落库改的
        // 正是树镜像的那些 `inst_relate.aabb` 指针。分块或重试多 bump 几次无害：这个
        // 号只与 sidecar 比相等，至多让下次启动多做一次指针重建。
        let touches_geometry =
            !chunk.is_empty() || (index == 0 && !(removals.is_empty() && branches.is_empty()));
        if touches_geometry {
            statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            statements.push('\n');
        }
        if statements.is_empty() {
            continue;
        }
        db.query(publication_transaction(&statements))
            .await
            .map_err(anyhow::Error::from)
            .and_then(check_publication_response)
            .with_context(|| {
                format!(
                    "persist projected root {root}（第 {} / {chunk_count} 块）",
                    index + 1
                )
            })?;
        commits += 1;
    }

    // 行已经提交，进程内空间树还停在旧盒上——就地收敛：盒是刚落库那批记录的 `world_bounds`
    // （ADR-072 D4），不回读库，也不看在不在数据窗口里。只渲不发（`preparing`）的那条路
    // 没有提交，`commits == 0`，树不动。
    if target.converge_spatial && commits > 0 {
        crate::fast_model::aabb_tree::converge_tree_after_persist(
            &deferred_refresh,
            tree_boxes,
            &cleared_room_edges,
        )
        .await;
    }

    Ok(PersistedRows {
        medium: target.medium,
        upserted: records.len(),
        removed: removals.len(),
        receipts: usize::from(receipt.is_some()),
        commits,
    })
}

/// 写入口径：**没有 UPDATE，也没有 UPSERT**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 初始化：库里还没有这一批，不必考虑更新，直接批量插，撞上已有 id 就跳过。
    Initial,
    /// 增量：先按 id 把这些记录的整组表示删干净，再批量插。
    ///
    /// 删这一步不是保险而是必需：`INSERT IGNORE` 撞上已有 id 会**保留旧行**，不删就等于
    /// 把上一轮的几何留在库里。
    Incremental,
}

/// 一批投影记录的库行：先删后插，每张表一条批量 `INSERT IGNORE`。
///
/// 管身的关系 id 是**分支全局**连号的（`tubi_relate:[pe:⟨branch⟩,n]`），所以同一分支的几段
/// 必须一起渲染，且渲染前要先发一条分支重置——否则上一轮的高号段留在库里。
/// [`persist_root_on`] 就是这么串的。
pub fn render_records(records: &[ProjectedGeometry], mode: WriteMode) -> anyhow::Result<String> {
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    render_chunk(records, mode, &mut tube_slots)
}

/// A rolled-back transaction reports generic failures before the statement that
/// caused it. Preserve every indexed error so the original cause is actionable.
fn check_publication_response(mut response: surrealdb::Response) -> anyhow::Result<()> {
    let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
    errors.sort_by_key(|(index, _)| *index);
    if !errors.is_empty() {
        anyhow::bail!(
            "publication statements failed: {}",
            errors
                .into_iter()
                .map(|(index, error)| format!("statement {index}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}

/// 批量请求失败时的**逐根**错误归因。
///
/// 一次批量请求 = 每根一段独立 `BEGIN … COMMIT`，而每段的顶层语句数相同：plan 里的效果
/// 语句全嵌在 `render_frozen_plan_chunk` 的 `IF $ack = NONE { … }` 块内，不改变顶层计数。
/// 所以响应里第 `i` 条语句属于批内第 `i / per_root` 根。归因不成立（响应条数不能被根数
/// 整除）时全部错误落进 `unattributed`，调用方按「不知道是谁的错」处理——绝不拿邻居的
/// `model_publish_superseded` 给本根判终态（2026-09-07 审核 H2）。
#[derive(Debug, Default, Clone)]
pub(crate) struct BatchPublicationError {
    /// 按批内序号归因到各根的语句错误，statement index 升序。
    pub per_root: Vec<Vec<String>>,
    /// 无法归因的错误；归因不成立时是全部。
    pub unattributed: Vec<String>,
    pub statements: usize,
    pub roots: usize,
}

impl BatchPublicationError {
    /// 归因到批内第 `index` 根的语句错误；`None` = 这一根的事务没有报任何错。
    pub fn root_errors(&self, index: usize) -> Option<&[String]> {
        self.per_root
            .get(index)
            .filter(|errors| !errors.is_empty())
            .map(Vec::as_slice)
    }

    /// 每条错误都落到了某一根名下。
    pub fn attributed(&self) -> bool {
        self.unattributed.is_empty() && self.per_root.len() == self.roots
    }
}

impl std::fmt::Display for BatchPublicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        for (root, errors) in self.per_root.iter().enumerate() {
            parts.extend(errors.iter().map(|error| format!("root {root}: {error}")));
        }
        parts.extend(
            self.unattributed
                .iter()
                .map(|error| format!("unattributed: {error}")),
        );
        write!(
            f,
            "publication statements failed ({} statements over {} roots): {}",
            self.statements,
            self.roots,
            parts.join("; ")
        )
    }
}

impl std::error::Error for BatchPublicationError {}

/// 批量请求的响应检查：有错就按 [`BatchPublicationError`] 逐根归因返回。
/// 单根请求继续用 [`check_publication_response`]——那里整条错误天然属于唯一的那一根。
fn check_batch_publication_response(
    mut response: surrealdb::Response,
    roots: usize,
) -> anyhow::Result<()> {
    // 先数再取：`take_errors` 会把出错的条目从响应里摘走。
    let statements = response.num_statements();
    let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort_by_key(|(index, _)| *index);
    let per_root = if roots > 0 && statements % roots == 0 {
        statements / roots
    } else {
        0
    };
    let mut error = BatchPublicationError {
        per_root: vec![Vec::new(); roots],
        unattributed: Vec::new(),
        statements,
        roots,
    };
    for (index, cause) in errors {
        let text = format!("statement {index}: {cause}");
        match per_root
            .checked_sub(1)
            .map(|_| index / per_root)
            .filter(|root| *root < roots)
        {
            Some(root) => error.per_root[root].push(text),
            None => error.unattributed.push(text),
        }
    }
    Err(anyhow::Error::new(error))
}

/// Render one complete, idempotent root replacement for the durable mirror spool.
///
/// Unlike [`persist_root_on`], this path cannot inspect the live database while
/// the source epoch is being published.  It therefore freezes both the current
/// rows and a database-side scoped delete into the job.  Replay may happen much
/// later, but it never needs to reopen an E3D file or consult the then-current
/// memory projection.
pub fn render_spooled_root(store: &ModelMemoryStore, root: RefNo) -> anyhow::Result<String> {
    let records = store.records_of_root(ProjectionScope::Current, root);
    let receipt = store
        .receipt(ProjectionScope::Current, root)
        .with_context(|| format!("model root {root} has no publication receipt"))?;
    let mut statements = render_spooled_scope_cleanup(root);
    let branches = records
        .iter()
        .filter(|record| record.tube.is_some())
        .map(|record| record.source_refno)
        .collect::<Vec<_>>();
    statements.push_str(&render_tube_branch_resets(&branches));
    statements.push_str(&render_records(&records, WriteMode::Initial)?);
    statements.push_str(&render_receipt(root, &receipt)?);
    statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    statements.push('\n');
    Ok(publication_transaction(&statements))
}

/// Render a durable tombstone for a model scope.  `affected_roots` and
/// `tube_branches` are captured from the in-memory projection before it is
/// removed, so replay cannot skip vanished rows or cross the tombstone.
pub fn render_spooled_scope_deletion(
    root: RefNo,
    affected_roots: &[RefNo],
    tube_branches: &[RefNo],
) -> String {
    let mut statements = render_spooled_scope_cleanup(root);
    statements.push_str(&render_tube_branch_resets(tube_branches));
    for affected in affected_roots {
        statements.push_str(&format!(
            "DELETE type::thing('gen_root', {});\n",
            sql_string(&refno_key(*affected))
        ));
    }
    statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    statements.push('\n');
    publication_transaction(&statements)
}

fn render_spooled_scope_cleanup(root: RefNo) -> String {
    let packed = packed_refno(root);
    let owner = sql_string(&refno_key(root));
    // Capture relation and info ids before deleting the relation rows.  All
    // predicates are evaluated by SurrealDB at replay time; replacement rows
    // are already fully rendered from the source epoch.
    format!(
        "LET $mirror_owner = type::thing('pe',{owner});\n\
         LET $mirror_rels = SELECT id, out FROM inst_relate \
         WHERE direct_model.source='e3d-model' AND (anc CONTAINS {packed} OR in = $mirror_owner);\n\
         LET $mirror_tubes = SELECT id FROM tubi_relate \
         WHERE direct_model.source='e3d-model' AND (anc CONTAINS {packed} OR in = $mirror_owner);\n\
         LET $mirror_ids = $mirror_rels.map(|$row| record::id($row.id));\n\
         LET $mirror_tube_ids = $mirror_tubes.map(|$row| record::id($row.id));\n\
         LET $mirror_infos = $mirror_rels.map(|$row| $row.out);\n\
         DELETE geo_relate WHERE in IN $mirror_infos;\n\
         DELETE inst_info WHERE id IN $mirror_infos;\n\
         DELETE inst_relate WHERE id IN $mirror_rels.id;\n\
         DELETE tubi_relate WHERE id IN $mirror_tubes.id;\n\
         DELETE aabb WHERE record::id(id) IN $mirror_ids OR record::id(id) IN $mirror_tube_ids;\n\
         DELETE trans WHERE record::id(id) IN $mirror_ids OR record::id(id) IN $mirror_tube_ids;\n"
    )
}

/// 一块记录的语句。管身槽位的表由调用方持有并跨块延续——一根切成几块提交时，
/// 每块各从 0 发号会让同一分支的后一块盖掉前一块。
fn render_chunk(
    records: &[ProjectedGeometry],
    mode: WriteMode,
    tube_slots: &mut BTreeMap<(u32, u32), usize>,
) -> anyhow::Result<String> {
    let mut statements = String::new();
    if mode == WriteMode::Incremental {
        append_geometry_removals(
            &mut statements,
            &records
                .iter()
                .map(|r| r.record_id.as_str())
                .collect::<Vec<_>>(),
        );
    }
    let mut batch = RowBatch::default();
    for record in records {
        collect_record(&mut batch, record, tube_slots)?;
    }
    statements.push_str(&batch.render());
    Ok(statements)
}

/// 按表攒好的行对象。一张表一条 `INSERT IGNORE INTO <表> [ … ]`，被引用的行（盒、变换、
/// 网格、实例信息）排在引用它们的关系之前。
#[derive(Debug, Default)]
struct RowBatch {
    aabb: Vec<String>,
    trans: Vec<String>,
    inst_geo: BTreeMap<String, String>,
    inst_info: Vec<String>,
    geo_relate: Vec<String>,
    inst_relate: Vec<String>,
    tubi_relate: Vec<String>,
}

impl RowBatch {
    fn render(&self) -> String {
        let mut statements = String::new();
        // `geo_relate` 与 `tubi_relate` 必须带 `RELATION`：普通 `INSERT` 只把 `in` / `out` 当
        // 两个字段写下去，不登记成图上的边，`out->geo_relate` 与 `pe:⟨…⟩->tubi_relate` 就都
        // 走不通。关键字顺序是 `INSERT RELATION IGNORE INTO`（语法上 RELATION 在 IGNORE 前）。
        // `inst_relate` 反过来**不能**带：烘焙实体那一行只有 `in` 没有 `out`（网格直接挂在
        // `booled_id` 上），而关系插入两端都是必填；读侧也从来是按 id 点查，不走图跳。
        let shared_meshes = self.inst_geo.values().cloned().collect::<Vec<_>>();
        for (table, rows, relation) in [
            ("aabb", &self.aabb, false),
            ("trans", &self.trans, false),
            ("inst_geo", &shared_meshes, false),
            ("inst_info", &self.inst_info, false),
            ("geo_relate", &self.geo_relate, true),
            ("inst_relate", &self.inst_relate, false),
            ("tubi_relate", &self.tubi_relate, true),
        ] {
            if rows.is_empty() {
                continue;
            }
            // `inst_geo` 与别的表一样走 `INSERT IGNORE`（plan 2026-09-06 Q4，2026-09-06 降级）：它是内容寻址的，
            // 同 id 必同内容（ADR-060 N3 的第一种前提）；此前那条 `ON DUPLICATE KEY UPDATE direct_model, aabb`
            // 是为「修正旧渲染器留下的描述符」而设——`direct_model` 是来源账、读侧不消费，`aabb` 只在
            // 标准图元行上有而同一 mesh id 不会既是烘焙又是标准图元，所以它实际改不了任何读者看得见的东西。
            // 模型面从此零 UPDATE。旧格式描述符照旧留着，与 `inst_geo` 只增不删（ADR-060 D2）同一口径。
            let keyword = if relation {
                "INSERT RELATION IGNORE INTO"
            } else {
                "INSERT IGNORE INTO"
            };
            statements.push_str(&format!(
                "{keyword} {table} [{}] RETURN NONE;\n",
                rows.join(",")
            ));
        }
        statements
    }
}

/// 把一条投影记录摊进各表的行对象里。
fn collect_record(
    batch: &mut RowBatch,
    record: &ProjectedGeometry,
    tube_slots: &mut BTreeMap<(u32, u32), usize>,
) -> anyhow::Result<()> {
    let id = record.record_id.as_str();
    let id_sql = sql_string(bracketed_id(id)?);
    let source_id = refno_key(record.source_refno);
    let source_id_sql = sql_string(&source_id);
    let geometry_json = sql_value(&serde_json::to_value(&record.geometry_id)?);
    let memory_projection = sql_value(&serde_json::to_value(record)?);
    let anc = sql_value(&json!(record.ancestors));
    let world_transform = sql_value(&transform_value(record.world_transform));
    let mesh_id = bracketed_id(record.mesh_id.as_str())?;
    let mesh_id_sql = sql_string(mesh_id);
    let noun = sql_string(&record.noun);
    let dbnum = record.dbnum;
    let source_sesno = record.source_sesno;

    let world_bounds = format!(
        "{{mins:{},maxs:{}}}",
        sql_value(&json!(record.world_bounds[0])),
        sql_value(&json!(record.world_bounds[1])),
    );
    batch.aabb.push(format!(
        "{{id:type::thing('aabb',{id_sql}),d:{world_bounds}}}"
    ));
    batch.trans.push(format!(
        "{{id:type::thing('trans',{id_sql}),d:{world_transform}}}"
    ));
    // Plant UI 的平表快路（`query_insts_flat`）读的是 `inst_relate` 行上的这两个副本：
    // 三件（`aabb_d` / `world_trans_d` / `insts_flat`）齐活的行零解引用零子查询直接成型，
    // 缺任一件就整行退回 `query_insts_slim` 现值兜底。**只买速度，不影响正确性**——
    // 实库 53,582 行上那两档解引用与子查询约 10.7s，平表读约 0.7s。
    // 管身不需要：`query_tubi_insts_by_brans` 走的是 `aabb.d` / `world_trans.d` 解引用。
    let flat_copies = format!(
        "aabb_d:{world_bounds},world_trans_d:{world_transform},memory_projection:{memory_projection}"
    );

    if let ProjectedShape::Canonical {
        key, local_bounds, ..
    } = &record.shape
    {
        let key = sql_value(&serde_json::to_value(key)?);
        batch.aabb.push(format!(
            "{{id:type::thing('aabb',{mesh_id_sql}),d:{{mins:{},maxs:{}}}}}",
            sql_value(&json!(local_bounds[0])),
            sql_value(&json!(local_bounds[1])),
        ));
        batch.inst_geo.insert(mesh_id.to_owned(), format!(
            "{{id:inst_geo:⟨{mesh_id}⟩,meshed:true,visible:true,bad:false,aabb:type::thing('aabb',{mesh_id_sql}),direct_model:{{source:'e3d-model',format:'canonical-primitive-v1',primitive_key:{key}}}}}"
        ));
    }

    if let Some(tube) = record.tube.as_ref() {
        // 隐式管身只向 Plant UI 暴露一个变换，标准图元的本地摆放在投影时就折进世界变换里
        // 了，所以这里既不写本地变换，也不写 `inst_info` / `inst_relate`。
        let (format_name, primitive_key) = match &record.shape {
            ProjectedShape::Canonical { key, .. } => (
                "canonical-primitive-v1",
                Some(sql_value(&serde_json::to_value(key)?)),
            ),
            ProjectedShape::Baked => ("baked-v2", None),
        };
        let primitive_field = primitive_key
            .map(|key| format!(",primitive_key:{key}"))
            .unwrap_or_default();
        let slot = next_tube_relation_slot(tube_slots, record.source_refno);
        if matches!(record.shape, ProjectedShape::Baked) {
            batch.inst_geo.insert(mesh_id.to_owned(), format!(
                "{{id:inst_geo:⟨{mesh_id}⟩,meshed:true,visible:true,bad:false,direct_model:{{source:'e3d-model',format:'{format_name}'{primitive_field}}}}}"
            ));
        }
        batch.tubi_relate.push(format!(
            "{{id:tubi_relate:[pe:⟨{source_id}⟩,{slot}],in:pe:⟨{source_id}⟩,out:inst_geo:⟨{mesh_id}⟩,leave:pe:⟨{}_{}⟩,arrive:pe:⟨{}_{}⟩,aabb:aabb:⟨{id}⟩,world_trans:trans:⟨{id}⟩,bore_size:0,invalid:{},anc:{anc},dbnum:{dbnum},memory_projection:{memory_projection},direct_model:{{source:'e3d-model',format:'{format_name}',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:{mesh_id_sql}{primitive_field}}}}}",
            tube.from.word0, tube.from.word1, tube.to.word0, tube.to.word1,
            tube.invalid,
        ));
        return Ok(());
    }

    match &record.shape {
        ProjectedShape::Canonical {
            key,
            local_transform,
            ..
        } => {
            let local_transform = sql_value(&transform_value(*local_transform));
            let key = sql_value(&serde_json::to_value(key)?);

            batch.inst_info.push(format!(
                "{{id:type::thing('inst_info',{id_sql}),dbnum:{dbnum},noun:{noun},direct_model:{{source:'e3d-model',sesno:{source_sesno},geometry_id:{geometry_json}}}}}"
            ));
            batch.geo_relate.push(format!(
                "{{id:geo_relate:[type::thing('inst_info',{id_sql}),inst_geo:⟨{mesh_id}⟩],in:type::thing('inst_info',{id_sql}),out:inst_geo:⟨{mesh_id}⟩,geom_refno:type::thing('pe',{source_id_sql}),transform:{local_transform},visible:true,geo_type:'Pos'}}"
            ));
            batch.inst_relate.push(format!(
                "{{id:type::thing('inst_relate',{id_sql}),in:type::thing('pe',{source_id_sql}),out:type::thing('inst_info',{id_sql}),booled_id:NONE,booled:false,bad_bool:false,solid:true,generic:{noun},dbnum:{dbnum},anc:{anc},aabb:type::thing('aabb',{id_sql}),world_trans:type::thing('trans',{id_sql}),{flat_copies},insts_flat:[{{geo_hash:{mesh_id_sql},transform:{local_transform}}}],direct_model:{{source:'e3d-model',format:'canonical-primitive-v1',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:{mesh_id_sql},primitive_key:{key}}}}}"
            ));
        }
        ProjectedShape::Baked => {
            batch.inst_relate.push(format!(
                "{{id:type::thing('inst_relate',{id_sql}),in:type::thing('pe',{source_id_sql}),booled_id:{mesh_id_sql},booled:true,bad_bool:false,solid:true,generic:{noun},dbnum:{dbnum},anc:{anc},aabb:type::thing('aabb',{id_sql}),world_trans:type::thing('trans',{id_sql}),{flat_copies},insts_flat:[{{geo_hash:{mesh_id_sql}}}],direct_model:{{source:'e3d-model',format:'baked-v2',sesno:{source_sesno},geometry_id:{geometry_json},mesh_id:{mesh_id_sql}}}}}"
            ));
        }
    }
    Ok(())
}

/// 一个 BRAN 的隐式管身整段清空。旧发布事务在插新段之前发的就是这一条。
pub fn render_tube_branch_resets(branches: &[RefNo]) -> String {
    let mut statements = String::new();
    let unique = branches
        .iter()
        .map(|branch| (branch.word0, branch.word1))
        .collect::<BTreeSet<_>>();
    for (word0, word1) in unique {
        append_tube_branch_reset(&mut statements, RefNo::new(word0, word1));
    }
    statements
}

fn append_tube_branch_reset(statements: &mut String, branch: RefNo) {
    statements.push_str(&format!(
        "DELETE pe:⟨{}_{}⟩->tubi_relate;\n",
        branch.word0, branch.word1
    ));
}

/// 从库里删掉这些记录 id 的全部表示。
pub fn render_removals(record_ids: &[String]) -> String {
    let mut statements = String::new();
    for record_id in record_ids {
        append_geometry_removal(&mut statements, record_id);
    }
    statements
}

/// 删掉一条记录在库里的**全部表示**：两种形状（烘焙 / 共享图元）的行一起删，因为一次
/// 重投影可能把同一个元素从一种换成另一种。
///
/// `inst_geo` 与共享本地盒不在其中——它们按 mesh 做键、内容寻址，别的元素可能正指着
/// 同一份，留给可达性 GC。管身的 `tubi_relate` 也不在：那是按分支整段重置的
/// （[`render_tube_branch_resets`]）。
fn append_geometry_removal(statements: &mut String, id: &str) {
    let id = sql_string(id);
    // The deployed engine scans DELETE ... WHERE even with an equality index.
    // SELECT resolves the indexed IDs first and also includes historical rows
    // inserted without RELATION, which graph traversal alone would miss.
    statements.push_str(&format!(
        "DELETE (SELECT VALUE id FROM geo_relate WHERE in=type::thing('inst_info',{id}));\n\
         DELETE type::thing('inst_info',{id});\n\
         DELETE type::thing('inst_relate',{id});\n\
         DELETE type::thing('aabb',{id});\n\
         DELETE type::thing('trans',{id});\n"
    ));
}

fn append_geometry_removals(statements: &mut String, ids: &[&str]) {
    if ids.len() <= 1 {
        if let Some(id) = ids.first() {
            append_geometry_removal(statements, id);
        }
        return;
    }
    for table in ["inst_info", "inst_relate", "aabb", "trans"] {
        let refs = ids
            .iter()
            .map(|id| format!("type::thing('{table}',{})", sql_string(id)))
            .collect::<Vec<_>>()
            .join(",");
        statements.push_str(&format!("LET $delete_{table}_ids = [{refs}];\n"));
        if table == "inst_info" {
            statements.push_str(
                "DELETE (SELECT VALUE id FROM geo_relate WHERE in IN $delete_inst_info_ids);\n",
            );
        }
        statements.push_str(&format!("DELETE $delete_{table}_ids;\n"));
    }
}

/// `route_ordinal` 只在同一对 `(from, to)` 里唯一，无关的两段常常都带 0；而旧关系 id 是
/// 分支全局的，所以每段各发一个槽位。
fn next_tube_relation_slot(slots: &mut BTreeMap<(u32, u32), usize>, source_refno: RefNo) -> usize {
    let next = slots
        .entry((source_refno.word0, source_refno.word1))
        .or_default();
    let current = *next;
    *next += 1;
    current
}

/// 一根投影完成后盖在 `gen_root` 上的章：窗口规划器认的就是
/// `publication_status='ready'` 且 `desired_revision == published_revision` 这一对。
pub fn render_receipt(root: RefNo, receipt: &RootReceipt) -> anyhow::Result<String> {
    render_receipt_version(root, receipt, None)
}
fn render_receipt_version(
    root: RefNo,
    receipt: &RootReceipt,
    frozen: Option<(u64, u32)>,
) -> anyhow::Result<String> {
    let model_target = receipt_model_target_literal(receipt)?;
    // 这份清单是整个 run 的（设计库 + 目录库的身份、会话、摘要），与根无关，而两列恒等。
    // 写成字面量就是每根把同一串发两遍：AMS 7997 那轮一份 5 952 字节、6 769 根合计 80.5 MB，
    // 占整批发布 SQL 的 26%。绑一次再引用两次，库里落的东西一字不变。
    let mut statements = String::new();
    let model_target = if model_target == "NONE" {
        model_target
    } else {
        statements.push_str(&format!("LET $mt = {model_target};\n"));
        "$mt".to_string()
    };
    statements.push_str(&render_receipt_with_target(
        root,
        receipt,
        frozen,
        &model_target,
        false,
    ));
    Ok(statements)
}

/// 回执里 `ModelTarget` 的 SurrealQL 字面量；没有目标就是 `NONE`。
fn receipt_model_target_literal(receipt: &RootReceipt) -> anyhow::Result<String> {
    Ok(receipt
        .model_target
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .map(|value| sql_value(&value))
        .unwrap_or_else(|| "NONE".to_string()))
}

/// 回执本体：`model_target` 由调用方给表达式（`NONE`、字面量或一个已 `LET` 好的变量）。
/// 跨根合块（[`persist_roots_batched`]）把同一份清单绑一次、几百根共用一个变量，就是靠这一层。
fn render_receipt_with_target(
    root: RefNo,
    receipt: &RootReceipt,
    frozen: Option<(u64, u32)>,
    model_target: &str,
    create_missing: bool,
) -> String {
    // 没有人再先写 `desired_target`（模型意图 upsert 随 pending 表退了，ADR-072 D5）：回执自己
    // 把「想要的」与「发布的」一起写成本次的目标，凭证行上两对字段始终相等。
    let target_literal = format!(
        "{{dbnum:{},source_end_sesno:{}}}",
        root.dbno(),
        receipt.source_sesno
    );
    let (revision, desired_target, published_target) = frozen
        .map(|(revision, dbnum)| {
            (
                revision.to_string(),
                "desired_target".to_string(),
                format!(
                    "{{dbnum:{dbnum},source_end_sesno:{}}}",
                    receipt.source_sesno
                ),
            )
        })
        .unwrap_or_else(|| {
            (
                "desired_revision".into(),
                target_literal.clone(),
                target_literal.clone(),
            )
        });
    let root_id = sql_string(&refno_key(root));
    let mut statements = String::new();
    let verb = if create_missing { "UPSERT" } else { "UPDATE" };
    let initialize = if create_missing {
        format!(
            "dbnum={}, desired_revision=desired_revision ?? 0, ",
            root.dbno()
        )
    } else {
        String::new()
    };
    statements.push_str(&format!(
        "{verb} type::thing('gen_root', {root_id}) SET \
         {initialize}status='Generated', publication_status='ready', \
         published_revision={revision}, desired_target={desired_target}, published_target={published_target}, \
         desired_model_target={model_target}, published_model_target={model_target}, \
         published_manifest_hash={}, published_geometry_count={}, \
         source_end_sesno={}, published_via={}, last_error=NONE, updated_at=time::now();\n",
        sql_string(&receipt.manifest_hash),
        receipt.geometry_count,
        receipt.source_sesno,
        sql_string(&receipt.published_via),
    ));
    append_root_dependency_receipt(&mut statements, root, receipt.model_target.as_ref());
    statements
}

/// 多块发布期间盖在 `gen_root` 上的可见性栅栏（P5）。
///
/// 一根切成几块之后它不再是一个原子边界：块与块之间库里躺着一个只更新了一半的根，
/// 而 `inst_relate` 的读者看不出这件事。栅栏把它写在凭证行上——`publication_status` 一旦
/// 不是 `ready`，凭证前移（`window_root_plan::render_credential_advance`）就不认这根，
/// 别的读者也能据此判断「这根正在被重写，别拿它当已发布的几何」。
///
/// 由最后一块的回执（[`render_receipt`]）解除，所以**只在会盖章的多块发布上写**：
/// 没有回执就没人把它翻回 `ready`，那时写栅栏等于把这根永久钉住；只有一块时事务本身
/// 就是原子的，中间态根本不存在。
fn render_publication_fence(root: RefNo) -> String {
    format!(
        "UPDATE type::thing('gen_root', {}) SET publication_status='publishing', \
         updated_at=time::now();\n",
        sql_string(&refno_key(root))
    )
}

// 「同一根不许被两个源会话混着发布」那道守卫（`render_publication_guard`，读凭证行上的
// `desired_target.source_end_sesno` 与回执比）随 pending 表退了（ADR-072 D5）：`desired_target`
// 原由数据尾事务的模型意图 upsert 抬到 T，现在没有人先写它；而同一 dbnum 只有本任务一个写者
// （ADR-071 D5）、同一根还有 `generation_root_lock`，守卫没有对手。

fn append_root_dependency_receipt(
    statements: &mut String,
    root: RefNo,
    target: Option<&ModelTarget>,
) {
    let Some(target) = target else {
        return;
    };
    let root_refno = sql_string(&root.to_string());
    // Dependency ids are <word0>_<word1>_<dbnum>. The trailing underscore
    // keeps root 1 distinct from root 10. A predicate DELETE scans every
    // other root's dependency inside every publication transaction.
    statements.push_str(&format!(
        "DELETE root_dependency:⟨{}_{}_⟩..⟨{}_{}_~⟩;\n",
        root.word0, root.word1, root.word0, root.word1,
    ));
    for dependency in &target.catalogue {
        statements.push_str(&format!(
            "UPSERT type::thing('root_dependency',{}) SET \
             root_refno={root_refno}, dependency_kind='CATA', dependency_dbnum={}, \
             dependency_session={}, dependency_digest={}, updated_at=time::now();\n",
            sql_string(&format!(
                "{}_{}_{}",
                root.word0, root.word1, dependency.dbnum
            )),
            dependency.dbnum,
            dependency.session,
            sql_string(&dependency.digest),
        ));
    }
}

pub fn publication_transaction(statements: &str) -> String {
    format!("BEGIN TRANSACTION;\n{statements}COMMIT TRANSACTION;\n")
}

/// 一次性清掉旧 `direct_*` 命名空间（2026-09-04 去前缀之前写下的那批行）。
///
/// 新 id 不再带前缀，所以逐条删除的语句**够不着旧行**：`inst_relate` 会被同 id UPSERT 盖掉，
/// 但它原先指向的 `inst_info:direct_<id>` / `aabb:direct_world_<id>` / `trans:direct_world_<id>`
/// 会变成没人引用的悬空行。切换之后对每个库跑一次这条即可；模型本来就是从文件重生成的，
/// 删干净不丢东西。共享网格 `inst_geo` 与盘上的 `.mesh` 不在其中——它们从来就没带过前缀，
/// 也是内容寻址的。
pub fn render_legacy_prefix_cleanup() -> String {
    let mut statements = String::new();
    statements.push_str(
        "DELETE geo_relate WHERE type::is::string(record::id(in)) \
         AND string::starts_with(<string>record::id(in), 'direct_');\n",
    );
    for table in ["inst_info", "trans"] {
        statements.push_str(&format!(
            "DELETE {table} WHERE type::is::string(record::id(id)) \
             AND string::starts_with(<string>record::id(id), 'direct_');\n"
        ));
    }
    // `aabb` 单列一条：旧共享本地盒（`aabb:direct_shared_<mesh>`）还被活着的 `inst_geo` 行
    // 指着，而 `inst_geo` 按 N2 只增不删——新一轮生成铸的是新 mesh id，旧那几行永远留在库里，
    // 它们的本地盒也就得跟着留。AMS 7997 现场是 22 个。
    statements.push_str(
        "LET $shared_boxes = (SELECT VALUE aabb FROM inst_geo WHERE aabb != NONE);\n\
         DELETE aabb WHERE type::is::string(record::id(id)) \
         AND string::starts_with(<string>record::id(id), 'direct_') \
         AND id NOTINSIDE $shared_boxes;\n",
    );
    statements
}

/// 「这个库已经扫过旧前缀了」的标记行。与 `dbnum_state` 的播种标记同表不同行。
const LEGACY_SWEEP_MARKER: &str = "queue_control:model_legacy_prefix_sweep";

/// 还指着旧命名空间的**活引用**：`(表, 字段)`。旧行只有在这五处一个不剩时才叫悬空。
///
/// 名单里**没有**另外两处指进旧命名空间的链接，各有理由，都是 2026-09-04 真库跑完之后核出来的：
///
/// - `geo_relate.in`：`in` 指着旧 `inst_info` 的那些 `geo_relate` 行**自己就是旧行**，
///   [`render_legacy_prefix_cleanup`] 第一条语句删的就是它们。把它们算成「活引用」，
///   守卫就永远拦住自己（7997 现场 7,908 行）。
/// - `inst_geo.aabb`：旧共享本地盒确实还被活着的 `inst_geo` 行指着，但 `inst_geo` 按 N2 只增
///   不删，这一条永远为真。处置不是拦住整次清扫，而是**把那几个盒子从删除里排除掉**——
///   见 [`render_legacy_prefix_cleanup`]。
const LEGACY_PREFIX_REFERENCES: [(&str, &str); 5] = [
    ("inst_relate", "out"),
    ("inst_relate", "aabb"),
    ("inst_relate", "world_trans"),
    ("tubi_relate", "aabb"),
    ("tubi_relate", "world_trans"),
];

/// [`render_legacy_prefix_cleanup`] 的自带守卫形态：**每个库一次，且只在旧命名空间真的
/// 悬空之后**。
///
/// 两道守卫，缺一不可：
///
/// 1. **标记行**。「一次」的作用域是**库**不是进程：进程里记一个 `OnceCell`，换个进程连同
///    一个库又要全表扫一遍。扫描与盖标记在同一条事务里，扫到一半崩掉就两样都不算数。
/// 2. **没有活引用**。这一条是 2026-09-04 在真库上撞出来的，不是防御性代码：切换之前写下的
///    `inst_relate` 行**用的就是不带前缀的 id**（`direct_` 只戴在 `inst_info` / `aabb` /
///    `trans` 上），所以它们还活着，而且 `out` / `aabb` / `world_trans` 三个指针一直指着旧
///    命名空间。AMS 7997 现场：22,040 行 `inst_relate` 全部指着 `aabb:direct_world_*`。那时候
///    扫，等于把整库的包围盒与世界变换连根删掉。
///
/// 旧行是**重生成一根、悬空一根**的，所以「悬空」只有整库口径才判得准。判据是
/// [`LEGACY_PREFIX_REFERENCES`] 五处引用一个不剩；判不准就不扫，下次再看——失败方向朝着
/// 「多留一批只占空间的死行」，不是「删掉还有人指着的行」。
///
/// 调用点因此挂在 [`persist_dbnum`] 的**收尾**，不在单根落库上：整库重写完那一刻才是旧命名
/// 空间真正悬空的那一刻，而单根落库顺手扫会把这一轮没轮到的根的指针一起带走。
fn render_legacy_prefix_sweep_once() -> String {
    let probes = LEGACY_PREFIX_REFERENCES
        .iter()
        .map(|(table, field)| {
            format!(
                "(SELECT VALUE id FROM {table} WHERE {field} != NONE \
                 AND type::is::string(record::id({field})) \
                 AND string::starts_with(<string>record::id({field}), 'direct_') LIMIT 1)"
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "LET $legacy_live = array::len(array::flatten([{probes}]));\n\
         IF ({LEGACY_SWEEP_MARKER}.done ?: false) != true AND $legacy_live == 0 {{ {}\
         UPSERT {LEGACY_SWEEP_MARKER} SET done=true, swept_at=time::now(); }};\n",
        render_legacy_prefix_cleanup()
    )
}

/// 库里这根名下、由 e3d-model 写过的几何身份。对账用它：库里有、存储里没有的就是陈旧行。
fn projected_geometry_ids_query(root: RefNo) -> String {
    let packed = packed_refno(root);
    format!(
        "SELECT VALUE direct_model.geometry_id FROM inst_relate \
         WHERE direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};\
         SELECT VALUE direct_model.geometry_id FROM tubi_relate \
         WHERE direct_model.source='e3d-model' \
         AND direct_model.geometry_id != NONE AND anc CONTAINS {packed};"
    )
}

async fn projected_geometry_ids_on(
    db: &Surreal<Any>,
    root: RefNo,
) -> anyhow::Result<Vec<GeometryId>> {
    let mut response = db
        .query(projected_geometry_ids_query(root))
        .await?
        .check()?;
    let mut ids: Vec<GeometryId> = response.take(0)?;
    ids.extend(response.take::<Vec<GeometryId>>(1)?);
    Ok(ids)
}

fn refno_key(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1)
}

fn sql_value(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value")
}

/// 一个 SurrealQL 字符串字面量，外层单引号一并补上。
///
/// **数据里来的文本一律走这里**，不许再写 `'{value}'`。`noun` 是从 PDMS 读出来的四字母
/// 代码，`manifest_hash` / `dependency_digest` 是上游摘要——今天它们撞上引号的概率约等于
/// 零，但这是口径不是概率：拼 SQL 的地方只要留一处裸插值，下一个往行上加字段的人就会
/// 照着它抄。转义与 [`escape_surql_str`] 同一套（反斜杠、单引号）。
fn sql_string(text: &str) -> String {
    format!("'{}'", escape_surql_str(text))
}

/// `表:⟨…⟩` 那种记录 id 里的 id 段。这里**不转义而是拒绝**：`⟨⟩` 之间没有转义序列可用，
/// 唯一能做的就是不让不合法的东西进去。
///
/// 这些 id 全是机器铸的——`<word0>_<word1>`、64 位十六进制摘要、`e3d_baked_v2_<hex>`、
/// 十进制 u64——字母表就是 `[0-9A-Za-z_]`。真出现别的字符，说明上游铸 id 的地方变了；
/// 那时早失败远好过写出一条能被从中间拆开的语句。
fn bracketed_id(id: &str) -> anyhow::Result<&str> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        anyhow::bail!("记录 id `{id}` 超出 `⟨⟩` 能安全承载的字母表（只允许字母、数字与下划线）");
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_model::primitive_instance::PrimitiveMeshKey;
    use glam::DMat4;

    use crate::fast_model::model_memory_store::{ProjectedTube, ProjectionCommit};

    const BRANCH: RefNo = RefNo::new(24383, 73948);
    /// 夹具根 `24383/*` 按 word0 解出的库号（`RefNo::dbno`）；记录上写的 `dbnum: 7997` 是
    /// 另一回事，跨根合块的对账两个都问。
    const BATCH_DBNUM: u32 = RefNo::new(24383, 1).dbno();
    /// 派生几何的记录 id 就是身份摘要，形状如此（真值是 64 位，夹具截短）。
    const TUBE_A: &str = "a1b2c3d4";
    const TUBE_B: &str = "e5f60718";

    fn baked_record() -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: "24383_73948".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/73948".into(),
            },
            source_refno: BRANCH,
            dbnum: 7997,
            source_sesno: 42,
            noun: "EQUI".into(),
            mesh_id: "e3d_baked_v2_deadbeef".into(),
            world_transform: DMat4::IDENTITY,
            world_bounds: [[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]],
            ancestors: vec![packed_refno(BRANCH), packed_refno(RefNo::new(24383, 1))],
            shape: ProjectedShape::Baked,
            tube: None,
        }
    }

    fn test_receipt(root: RefNo, geometry_count: usize) -> (RefNo, RootReceipt) {
        (
            root,
            RootReceipt {
                source_sesno: 42,
                model_target: None,
                manifest_hash: format!("test-{geometry_count}"),
                geometry_count,
                published_via: "regen".into(),
            },
        )
    }

    fn canonical_record() -> ProjectedGeometry {
        ProjectedGeometry {
            mesh_id: "123456789".into(),
            shape: ProjectedShape::Canonical {
                key: PrimitiveMeshKey::CylinderV3 { segments: 32 },
                local_transform: DMat4::IDENTITY,
                local_bounds: [[-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]],
            },
            ..baked_record()
        }
    }

    /// 共享原型行（`inst_geo:<mesh id>`）与发布顺序无关；而且它**只增不删、已有行赢**
    /// （ADR-060 N2 / N3 第一种前提；plan 2026-09-06 Q4 把此前那条修旧描述符的
    /// `ON DUPLICATE KEY UPDATE direct_model, aabb` 降成了 `INSERT IGNORE`）——
    /// 一行被手改过之后再次发布同一原型，不会被"修回来"，读侧也不消费被改的那几个字段。
    #[tokio::test]
    async fn shared_prototype_is_identical_in_both_publication_orders_and_existing_rows_win() {
        let mut tube = tube_record(0, TUBE_A);
        tube.mesh_id = canonical_record().mesh_id;
        tube.shape = canonical_record().shape;
        let mut states = Vec::new();
        for reverse in [false, true] {
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("prototype_order").use_db("test").await.unwrap();
            let mut records = vec![canonical_record(), tube.clone()];
            if reverse {
                records.reverse();
            }
            for record in &records {
                db.query(render_records(&[record.clone()], WriteMode::Initial).unwrap())
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
            let value:Vec<serde_json::Value>=db.query("SELECT <string>id AS id, <string>aabb AS aabb, meshed, visible, bad, direct_model FROM inst_geo;").await.unwrap().check().unwrap().take(0).unwrap();
            states.push(value);
            db.query("UPDATE inst_geo:⟨123456789⟩ SET visible=false,aabb=NONE,direct_model={source:'e3d-model',format:'canonical-primitive-v1'};").await.unwrap().check().unwrap();
            db.query(render_records(&[tube.clone()], WriteMode::Initial).unwrap())
                .await
                .unwrap()
                .check()
                .unwrap();
            let kept: Vec<serde_json::Value> = db
                .query(
                    "SELECT visible, aabb != NONE AS has_aabb, direct_model FROM inst_geo:⟨123456789⟩;",
                )
                .await
                .unwrap()
                .check()
                .unwrap()
                .take(0)
                .unwrap();
            assert_eq!(kept.len(), 1, "再次发布不得复制或删掉共享原型行");
            assert_eq!(kept[0]["visible"], false);
            assert_eq!(
                kept[0]["has_aabb"], false,
                "INSERT IGNORE 撞上已有 id 必须原样保留旧行：{}",
                kept[0]
            );
            assert!(
                kept[0]["direct_model"].get("primitive_key").is_none(),
                "已有行赢，不再修旧描述符：{}",
                kept[0]
            );
        }
        assert_eq!(states[0], states[1]);
        assert_eq!(states[0].len(), 1);
    }

    /// 嵌套生成根：显著节点名下的显著节点也是根，`enumerate_generation_roots_in_subtree`
    /// 会把它单独出成一根。
    const NESTED_ROOT: RefNo = RefNo::new(24383, 500);

    /// 挂在嵌套根 [`NESTED_ROOT`] 名下的一件几何。祖先链是全库的，所以它同时挂在
    /// 祖先根 24383_1 下面——两个根的对账都会看见这一行。
    fn nested_root_record() -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: "24383_600".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/600".into(),
            },
            source_refno: RefNo::new(24383, 600),
            ancestors: vec![
                packed_refno(RefNo::new(24383, 600)),
                packed_refno(NESTED_ROOT),
                packed_refno(RefNo::new(24383, 1)),
            ],
            ..baked_record()
        }
    }

    /// 另一个来源上的共享图元，用来跟 [`baked_record`] 同场出现而不撞 id。
    fn other_canonical_record() -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: "24383_500".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/500".into(),
            },
            source_refno: RefNo::new(24383, 500),
            ..canonical_record()
        }
    }

    fn tube_record(route_ordinal: u32, record_id: &str) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: record_id.into(),
            geometry_id: GeometryId::ImpliedTube {
                container_refno: "24383/73948".into(),
                route_ordinal,
                from_refno: "24383/10".into(),
                to_refno: "24383/20".into(),
            },
            tube: Some(ProjectedTube {
                route_ordinal,
                from: RefNo::new(24383, 10),
                to: RefNo::new(24383, 20),
                invalid: false,
            }),
            ..baked_record()
        }
    }

    /// 初始化那一档整段钉死：**没有 UPDATE / UPSERT**，一张表一条批量 `INSERT IGNORE`，
    /// 烘焙实体的 `booled_id` 直接指内容寻址的网格，`anc` 是打包祖先链。
    #[test]
    fn an_initial_batch_is_insert_ignore_only() {
        let sql = render_records(&[baked_record()], WriteMode::Initial).expect("render");
        let world = sql_value(&transform_value(DMat4::IDENTITY));
        let expected = format!(
            "INSERT IGNORE INTO aabb [{{id:type::thing('aabb','24383_73948'),d:{{mins:[0.0,0.0,0.0],maxs:[1.0,2.0,3.0]}}}}];\n\
             INSERT IGNORE INTO trans [{{id:type::thing('trans','24383_73948'),d:{world}}}];\n\
             INSERT IGNORE INTO inst_relate [{{id:type::thing('inst_relate','24383_73948'),in:type::thing('pe','24383_73948'),booled_id:'e3d_baked_v2_deadbeef',booled:true,bad_bool:false,solid:true,generic:'EQUI',dbnum:7997,anc:[104724187652316,104724187578369],aabb:type::thing('aabb','24383_73948'),world_trans:type::thing('trans','24383_73948'),aabb_d:{{mins:[0.0,0.0,0.0],maxs:[1.0,2.0,3.0]}},world_trans_d:{world},insts_flat:[{{geo_hash:'e3d_baked_v2_deadbeef'}}],direct_model:{{source:'e3d-model',format:'baked-v2',sesno:42,geometry_id:{{\"kind\":\"element\",\"refno\":\"24383/73948\"}},mesh_id:'e3d_baked_v2_deadbeef'}}}}];\n"
        );
        let projection = format!(
            ",memory_projection:{}",
            sql_value(&serde_json::to_value(baked_record()).unwrap())
        );
        assert_eq!(
            sql.replace(&projection, ""),
            expected.replace("];\n", "] RETURN NONE;\n")
        );
    }

    /// 增量那一档在同一批前面多一组按 id 的删除——`INSERT IGNORE` 撞上已有 id 会保留旧行，
    /// 不删就等于把上一轮的几何留在库里。
    #[test]
    fn an_incremental_batch_deletes_the_ids_first() {
        let sql = render_records(&[baked_record()], WriteMode::Incremental).expect("render");
        let expected_head = "DELETE (SELECT VALUE id FROM geo_relate WHERE in=type::thing('inst_info','24383_73948'));\n\
             DELETE type::thing('inst_info','24383_73948');\n\
             DELETE type::thing('inst_relate','24383_73948');\n\
             DELETE type::thing('aabb','24383_73948');\n\
             DELETE type::thing('trans','24383_73948');\n";
        assert!(sql.starts_with(expected_head), "{sql}");
        assert_eq!(
            &sql[expected_head.len()..],
            render_records(&[baked_record()], WriteMode::Initial).expect("render"),
            "删除之后的那一半与初始化逐字相同"
        );
    }

    /// 共享图元走的是另一条老协议：网格挂 `inst_geo`（本地盒按 mesh 做键，多个元素共用
    /// 一份），元素侧留 `inst_info + geo_relate`，本地变换在边上，`insts_flat` 里也带一份。
    #[test]
    fn a_canonical_record_keeps_the_shared_mesh_contract() {
        let sql = render_records(&[canonical_record()], WriteMode::Initial).expect("render");
        let local = sql_value(&transform_value(DMat4::IDENTITY));
        assert!(sql.contains(
            "{id:type::thing('aabb','123456789'),d:{mins:[-1.0,-1.0,0.0],maxs:[1.0,1.0,2.0]}}"
        ));
        assert_eq!(
            sql.matches("INSERT IGNORE INTO trans [").count(),
            1,
            "trans 表一个元素只占一行：本地变换写在 geo_relate 边上"
        );
        assert!(sql.contains(
            "{id:inst_geo:⟨123456789⟩,meshed:true,visible:true,bad:false,aabb:type::thing('aabb','123456789'),direct_model:{source:'e3d-model',format:'canonical-primitive-v1',primitive_key:{\"family\":\"cylinder_v3\",\"segments\":32}}}"
        ));
        assert!(sql.contains(&format!(
            "{{id:geo_relate:[type::thing('inst_info','24383_73948'),inst_geo:⟨123456789⟩],in:type::thing('inst_info','24383_73948'),out:inst_geo:⟨123456789⟩,geom_refno:type::thing('pe','24383_73948'),transform:{local},visible:true,geo_type:'Pos'}}"
        )));
        assert!(sql.contains(&format!(
            "insts_flat:[{{geo_hash:'123456789',transform:{local}}}]"
        )));
        assert!(sql.contains("booled_id:NONE,booled:false"));
        assert!(sql.contains("primitive_key:{\"family\":\"cylinder_v3\",\"segments\":32}"));
    }

    /// 管身不经 `inst_relate`：一条 `pe → inst_geo` 的关系边自带两端。
    #[test]
    fn an_implied_tube_renders_the_legacy_tubi_contract() {
        let sql = render_records(&[tube_record(0, TUBE_A)], WriteMode::Initial).expect("render");
        assert!(sql.contains("INSERT RELATION IGNORE INTO tubi_relate ["));
        assert!(sql.contains("id:tubi_relate:[pe:⟨24383_73948⟩,0]"));
        assert!(sql.contains("leave:pe:⟨24383_10⟩"));
        assert!(sql.contains("arrive:pe:⟨24383_20⟩"));
        assert!(sql.contains("out:inst_geo:⟨e3d_baked_v2_deadbeef⟩"));
        assert!(sql.contains(&format!(
            "aabb:aabb:⟨{TUBE_A}⟩,world_trans:trans:⟨{TUBE_A}⟩"
        )));
        assert!(!sql.contains("INSERT IGNORE INTO inst_relate"));
    }

    #[test]
    fn an_invalid_implied_tube_is_persisted_for_dashed_rendering() {
        let mut record = tube_record(0, TUBE_A);
        record.tube.as_mut().unwrap().invalid = true;
        let sql = render_records(&[record], WriteMode::Initial).expect("render");
        assert!(sql.contains("invalid:true"), "{sql}");
    }

    /// 关系 id 的槽位是分支全局的：同一对 `(from, to)` 的两段都带 0 也不会撞，
    /// 而且两段进同一条批量语句。
    #[test]
    fn tube_slots_are_branch_global_not_route_local() {
        let sql = render_records(
            &[tube_record(0, TUBE_A), tube_record(0, TUBE_B)],
            WriteMode::Initial,
        )
        .expect("render");
        assert!(sql.contains("id:tubi_relate:[pe:⟨24383_73948⟩,0]"));
        assert!(sql.contains("id:tubi_relate:[pe:⟨24383_73948⟩,1]"));
        assert_eq!(
            sql.matches("INSERT RELATION IGNORE INTO tubi_relate [")
                .count(),
            1
        );
    }

    /// 模型面零 UPDATE（plan 2026-09-06 Q4）：`inst_geo` 与别的表一样只 `INSERT IGNORE`，
    /// 渲染出来的整段正文里不许再有 `ON DUPLICATE KEY UPDATE`，也不许有 `UPSERT`。
    #[test]
    fn model_rows_render_without_any_update_verb() {
        let sql = render_records(
            &[baked_record(), other_canonical_record()],
            WriteMode::Incremental,
        )
        .unwrap();
        assert!(sql.contains("INSERT IGNORE INTO inst_geo ["), "{sql}");
        assert!(!sql.contains("ON DUPLICATE KEY"), "{sql}");
        assert!(!sql.contains("UPSERT "), "{sql}");
        assert!(
            !sql.contains("UPDATE "),
            "几何行渲染不许出现 UPDATE（凭证栅栏与回执另有出处）: {sql}"
        );
    }

    /// 删一条几何不碰共享网格：`inst_geo` 与盘上的 `.mesh` 都是内容寻址的，
    /// 别的元素可能正指着同一份。
    #[test]
    fn a_removal_never_deletes_a_shared_mesh() {
        let sql = render_removals(&[TUBE_A.into()]);
        assert!(sql.contains(&format!("DELETE type::thing('inst_relate','{TUBE_A}');")));
        assert!(sql.contains(&format!("DELETE type::thing('inst_info','{TUBE_A}');")));
        assert!(sql.contains(&format!("DELETE type::thing('aabb','{TUBE_A}');")));
        assert!(sql.contains(&format!("DELETE type::thing('trans','{TUBE_A}');")));
        assert!(!sql.contains("DELETE inst_geo"));
        assert!(!sql.contains(".mesh"));
    }

    /// 行 id 一律就是记录 id。这一条盯的是渲染出来的整段正文：五种旧前缀一个都不许出现，
    /// 而 `direct_model` 那个**字段**名不在此列。
    #[test]
    fn no_row_id_carries_a_prefix() {
        let sql = render_records(
            &[
                baked_record(),
                other_canonical_record(),
                tube_record(0, TUBE_A),
            ],
            WriteMode::Incremental,
        )
        .expect("render");
        for marker in [
            "'direct_",
            "⟨direct_",
            "direct_world_",
            "direct_shared_",
            "_local",
            "derived_",
            "hist_",
            "snapshot_key",
        ] {
            assert!(!sql.contains(marker), "行 id 里还留着 {marker}");
        }
        assert!(
            sql.contains("direct_model:{source:'e3d-model'"),
            "字段名照旧"
        );
    }

    #[tokio::test]
    async fn structured_projection_recovers_multiple_pages_and_empty_root_receipts() {
        let db = surreal_mem().await;
        let mut records = Vec::new();
        for i in 0..1030 {
            let r = RefNo::new(24383, 1000 + i);
            records.push(ProjectedGeometry {
                record_id: format!("24383_{}", 1000 + i),
                geometry_id: GeometryId::Element {
                    refno: format!("24383/{}", 1000 + i),
                },
                source_refno: r,
                ancestors: vec![packed_refno(BRANCH)],
                ..baked_record()
            });
        }
        records.push(tube_record(1, TUBE_A));
        records.push(tube_record(2, TUBE_B));
        db.query(render_records(&records, WriteMode::Initial).unwrap())
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query("CREATE gen_root:24383_73948 SET dbnum=7997, publication_status='ready', source_end_sesno=42, published_manifest_hash='manifest', published_geometry_count=1032; CREATE gen_root:24383_1 SET dbnum=7997, publication_status='ready', source_end_sesno=42, published_manifest_hash='empty', published_geometry_count=0;").await.unwrap().check().unwrap();
        let store = ModelMemoryStore::default();
        restore_projection_on(&db, &store, 7997).await.unwrap();
        assert_eq!(store.records(ProjectionScope::Current).len(), records.len());
        assert_eq!(store.roots(ProjectionScope::Current).len(), 2);
        assert_eq!(
            store
                .receipt(ProjectionScope::Current, BRANCH)
                .unwrap()
                .manifest_hash,
            "manifest"
        );
        let mut expected = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>();
        let mut actual = store
            .records(ProjectionScope::Current)
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
        db.query("UPDATE inst_relate SET memory_projection=NONE WHERE dbnum=7997;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(restore_projection_on(&db, &store, 7997).await.is_err());
        assert_eq!(
            store.records(ProjectionScope::Current).len(),
            records.len(),
            "failed recovery must preserve the previous image"
        );
    }

    #[tokio::test]
    async fn durable_root_probe_never_hydrates_the_incremental_overlay() {
        let db = surreal_mem().await;
        let requested_root = RefNo::new(7997, 73948);
        let other_root = RefNo::new(7997, 70000);
        let mut requested = baked_record();
        requested.ancestors = vec![packed_refno(requested_root)];
        let mut unrelated = other_canonical_record();
        unrelated.ancestors = vec![packed_refno(other_root)];
        db.query(render_records(&[requested.clone(), unrelated], WriteMode::Initial).unwrap())
            .await
            .unwrap()
            .check()
            .unwrap();
        db.query("CREATE gen_root:7997_73948 SET dbnum=7997, publication_status='ready', source_end_sesno=42, published_manifest_hash='requested', published_geometry_count=1; CREATE gen_root:7997_70000 SET dbnum=7997, publication_status='ready', source_end_sesno=42, published_manifest_hash='other', published_geometry_count=1;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let probe: Vec<Value> = db
            .query(format!(
                "SELECT anc,dbnum,memory_projection != NONE AS structured FROM inst_relate WHERE anc CONTAINS {};",
                packed_refno(requested_root)
            ))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(probe.len(), 1, "root lookup fixture: {probe:?}");

        let store = ModelMemoryStore::with_cache_limit(1024 * 1024);
        assert!(
            ensure_root_cached_on(&db, &store, requested_root)
                .await
                .unwrap()
        );
        assert!(
            store.records(ProjectionScope::Current).is_empty(),
            "a RocksDB availability probe must not hydrate kv-mem"
        );
        assert!(
            store
                .receipt(ProjectionScope::Current, requested_root)
                .is_none()
        );
        assert!(
            store
                .receipt(ProjectionScope::Current, other_root)
                .is_none()
        );
        let stats = store.cache_stats();
        assert_eq!((stats.misses, stats.loads), (1, 1));
    }

    #[tokio::test]
    async fn a_missing_durable_root_is_not_negative_cached() {
        let db = surreal_mem().await;
        let store = ModelMemoryStore::with_cache_limit(1024);
        let root = RefNo::new(7997, 999);
        assert!(!ensure_root_cached_on(&db, &store, root).await.unwrap());
        assert!(!ensure_root_cached_on(&db, &store, root).await.unwrap());
        let stats = store.cache_stats();
        assert_eq!((stats.misses, stats.loads), (2, 2));
        assert_eq!(stats.resident_roots, 0);
    }

    #[tokio::test]
    async fn root_cache_queries_are_planned_through_ancestor_indexes() {
        let db = surreal_mem().await;
        db.query(
            "DEFINE INDEX idx_inst_relate_anc ON TABLE inst_relate COLUMNS anc;\
             DEFINE INDEX idx_tubi_relate_anc ON TABLE tubi_relate COLUMNS anc;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let root = RefNo::new(7997, 1);
        for (table, index) in [
            ("inst_relate", "idx_inst_relate_anc"),
            ("tubi_relate", "idx_tubi_relate_anc"),
        ] {
            let explain = format!(
                "{} EXPLAIN FULL;",
                root_projection_sql(table, root).trim_end_matches(';')
            );
            let plan: Vec<Value> = db
                .query(explain)
                .await
                .unwrap()
                .check()
                .unwrap()
                .take(0)
                .unwrap();
            let text = serde_json::to_string(&plan).unwrap();
            assert!(
                text.contains(index),
                "root cache query must use {index}, plan: {text}"
            );
        }
    }

    #[tokio::test]
    async fn publication_error_retains_the_cause_after_rollback_errors() {
        let db = surreal_mem().await;
        let response = db.query("BEGIN TRANSACTION; CREATE publication_probe:one; THROW 'specific-publication-cause'; COMMIT TRANSACTION;")
            .await.expect("transaction response");
        let error = check_publication_response(response).expect_err("must fail");
        assert!(
            error.to_string().contains("specific-publication-cause"),
            "{error}"
        );
        assert!(error.to_string().contains("statement"), "{error}");
    }

    /// Plant UI 的平表快路要的三件（`aabb_d` / `world_trans_d` / `insts_flat`）都在
    /// `inst_relate` 行上：缺任一件那一行就整行退回 slim 现值兜底，只慢不错。
    #[tokio::test]
    async fn the_flat_fast_path_finds_all_three_inline_copies() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let sql = render_records(&[baked_record()], WriteMode::Initial).expect("render");
        db.query(publication_transaction(&sql))
            .await
            .expect("publish")
            .check()
            .expect("publish statements");

        // 与 `rs-core::rs_surreal::inst::query_insts_flat` 的投影同一形状。
        let mut response = db
            .query(
                "SELECT <string>in AS refno, anc[1] AS owner_packed, generic, \
                 aabb != NONE AS has_aabb, aabb_d, world_trans_d, insts_flat, booled_id \
                 FROM type::thing('inst_relate','24383_73948');",
            )
            .await
            .expect("flat")
            .check()
            .expect("flat statement");
        let rows = response.take::<Vec<Value>>(0).expect("flat rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row["has_aabb"], json!(true));
        assert_eq!(
            row["aabb_d"],
            json!({"mins":[0.0,0.0,0.0],"maxs":[1.0,2.0,3.0]})
        );
        assert_eq!(row["world_trans_d"], transform_value(DMat4::IDENTITY));
        assert_eq!(
            row["insts_flat"][0]["geo_hash"],
            json!("e3d_baked_v2_deadbeef")
        );
        assert_eq!(
            row["owner_packed"],
            json!(packed_refno(RefNo::new(24383, 1))),
            "owner 从 anc[1] 还原，写入侧的祖先链顺序得对得上"
        );
    }

    /// Plant UI 的实例子查询照旧取得到本地变换与 mesh 身份——本地变换现在是 `geo_relate`
    /// 边上的 `transform` 字段，读侧写成 `transform ?? trans.d`，旧的链接行同样能读。
    #[tokio::test]
    async fn the_plant_ui_instance_subquery_still_resolves_the_local_transform() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let sql = render_records(&[other_canonical_record()], WriteMode::Initial).expect("render");
        db.query(publication_transaction(&sql))
            .await
            .expect("publish")
            .check()
            .expect("publish statements");

        // 与 `rs-core::rs_surreal::inst::query_insts` 的 insts 子查询同一形状。
        let mut response = db
            .query(
                "SELECT (select (transform ?? trans.d) as transform, record::id(out) as geo_hash \
                 from out->geo_relate \
                 where visible && out.meshed && (transform ?? trans.d) != none && geo_type='Pos') \
                 AS insts FROM type::thing('inst_relate','24383_500');",
            )
            .await
            .expect("insts")
            .check()
            .expect("insts statement");
        let rows = response.take::<Vec<Value>>(0).expect("insts rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let insts = rows[0]["insts"].as_array().expect("insts array");
        assert_eq!(insts.len(), 1, "{rows:?}");
        assert_eq!(insts[0]["geo_hash"], json!("123456789"));
        assert_eq!(insts[0]["transform"], transform_value(DMat4::IDENTITY));
    }

    /// 切换之后旧命名空间的悬空行要能一次扫掉，且不碰新行。
    #[tokio::test]
    async fn the_one_shot_sweep_takes_the_old_namespace_and_leaves_the_new_rows() {
        let db = surreal_mem().await;
        db.query(
            "CREATE type::thing('inst_info','direct_24383_73948');\
             CREATE type::thing('aabb','direct_world_24383_73948');\
             CREATE type::thing('trans','direct_world_24383_73948');\
             CREATE type::thing('trans','direct_local_24383_73948');\
             CREATE type::thing('inst_info','24383_73948');\
             CREATE type::thing('aabb','24383_73948');\
             CREATE type::thing('trans','24383_73948_local');",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        db.query(publication_transaction(&render_legacy_prefix_cleanup()))
            .await
            .expect("sweep")
            .check()
            .expect("sweep statements");

        assert_eq!(count(&db, "inst_info").await, 1);
        assert_eq!(count(&db, "aabb").await, 1);
        assert_eq!(count(&db, "trans").await, 1);
    }

    fn receipt() -> RootReceipt {
        RootReceipt {
            source_sesno: 26,
            model_target: None,
            manifest_hash: "abc123".into(),
            geometry_count: 2,
            published_via: "regen".into(),
        }
    }

    /// 回执盖的就是窗口规划器认的那几列。revision 不在回执里，所以
    /// `published_revision` 直接跟着行上的 `desired_revision` 走；`desired_target` 与
    /// `published_target` 由回执一起写成本次目标（没有人再先写「想要的」，ADR-072 D5）。
    /// 混源发布守卫随之退役：同库单写者 + 根锁，守卫没有对手。
    #[test]
    fn a_receipt_settles_the_root_credential() {
        let sql = render_receipt(RefNo::new(24384, 1), &receipt()).expect("receipt");
        assert!(sql.contains("UPDATE type::thing('gen_root', '24384_1')"));
        assert!(sql.contains("publication_status='ready'"));
        assert!(sql.contains("published_revision=desired_revision"));
        assert!(
            sql.contains("desired_target={dbnum:")
                && sql.contains("published_target={dbnum:")
                && sql.matches("source_end_sesno:26}").count() == 2,
            "{sql}"
        );
        assert!(sql.contains("published_manifest_hash='abc123'"));
        assert!(sql.contains("published_geometry_count=2"));
        assert!(sql.contains("source_end_sesno=26"));
        assert!(!sql.contains("root_dependency"));
        assert!(
            !sql.contains("mixed model target publication"),
            "混源守卫已随 pending 表退役: {sql}"
        );
    }

    #[test]
    fn a_spooled_root_is_a_complete_idempotent_replacement() {
        let store = ModelMemoryStore::default();
        let root = RefNo::new(24383, 1);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        let sql = render_spooled_root(&store, root).expect("spool rendering");
        assert!(sql.starts_with("BEGIN TRANSACTION;"));
        assert!(sql.contains("LET $mirror_rels = SELECT id, out FROM inst_relate"));
        assert!(sql.contains("DELETE inst_relate WHERE id IN $mirror_rels.id"));
        assert!(sql.contains("INSERT IGNORE INTO inst_relate"));
        assert!(sql.contains("publication_status='ready'"));
        assert!(sql.ends_with("COMMIT TRANSACTION;\n"));
    }

    #[tokio::test]
    async fn a_spooled_root_replays_idempotently() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let store = ModelMemoryStore::default();
        let root = RefNo::new(24383, 1);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        let sql = render_spooled_root(&store, root).expect("spool rendering");
        for _ in 0..2 {
            db.query(&sql)
                .await
                .expect("submit spool transaction")
                .check()
                .expect("commit spool transaction");
        }
        assert_eq!(count(&db, "inst_relate").await, 1);
    }

    #[tokio::test]
    async fn a_spooled_tombstone_replays_idempotently() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let store = ModelMemoryStore::default();
        let root = RefNo::new(24383, 1);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        db.query(render_spooled_root(&store, root).unwrap())
            .await
            .unwrap()
            .check()
            .unwrap();
        let tombstone = render_spooled_scope_deletion(root, &[root], &[]);
        for _ in 0..2 {
            db.query(&tombstone).await.unwrap().check().unwrap();
        }
        assert_eq!(count(&db, "inst_relate").await, 0);
        assert_eq!(count(&db, "gen_root").await, 0);
    }

    #[tokio::test]
    async fn a_spooled_tombstone_deletes_durable_rows_without_memory_projection() {
        let db = surreal_mem().await;
        let root = RefNo::new(24383, 1);
        let packed = packed_refno(root);
        db.query(format!(
            "CREATE inst_relate:durable_only SET in=pe:durable_only, anc=[{packed}], \
             out=inst_info:durable_only, direct_model={{source:'e3d-model'}};"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let tombstone = render_spooled_scope_deletion(root, &[root], &[]);
        assert!(tombstone.contains("DELETE tubi_relate WHERE id IN $mirror_tubes.id"));
        db.query(tombstone).await.unwrap().check().unwrap();

        assert_eq!(count(&db, "inst_relate").await, 0);
    }

    /// 渲染出来的语句真能在库里跑通，落进的是旧那几张表：烘焙实体只占 `inst_relate`，
    /// 共享图元多带一组 `inst_info + geo_relate + inst_geo`，管身只走 `tubi_relate`。
    #[tokio::test]
    async fn the_rendered_rows_land_in_the_legacy_tables() {
        let db = surreal_mem().await;
        seed_pe(&db).await;

        let sql = render_records(
            &[
                baked_record(),
                other_canonical_record(),
                tube_record(0, TUBE_A),
            ],
            WriteMode::Initial,
        )
        .expect("render");
        db.query(publication_transaction(&sql))
            .await
            .expect("publish")
            .check()
            .expect("publish statements");

        assert_eq!(count(&db, "inst_relate").await, 2);
        assert_eq!(count(&db, "tubi_relate").await, 1);
        assert_eq!(count(&db, "inst_info").await, 1);
        assert_eq!(count(&db, "geo_relate").await, 1);
        assert_eq!(count(&db, "inst_geo").await, 2);
    }

    /// 同一个元素从烘焙换成共享图元，库里只剩新表示：`booled_id` 那半边先被清掉，
    /// 否则 Plant UI 会同时看见一个 booled 实体和一个共享实例。
    #[tokio::test]
    async fn switching_a_representation_leaves_only_the_new_one() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        for record in [baked_record(), canonical_record()] {
            let sql = render_records(&[record], WriteMode::Incremental).expect("render");
            db.query(publication_transaction(&sql))
                .await
                .expect("publish")
                .check()
                .expect("publish statements");
        }

        assert_eq!(count(&db, "inst_relate").await, 1);
        let mut response = db
            .query("SELECT VALUE booled FROM inst_relate;")
            .await
            .expect("booled")
            .check()
            .expect("booled statement");
        assert_eq!(
            response.take::<Vec<bool>>(0).expect("booled rows"),
            vec![false]
        );
    }

    /// 对账：库里那条存储已经没有的几何被删掉，管身分支整段重写，剩下的行照旧。
    #[tokio::test]
    async fn persisting_a_root_drops_what_the_projection_no_longer_holds() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);

        let store = ModelMemoryStore::default();
        let stale = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            ..baked_record()
        };
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), stale.clone(), tube_record(0, TUBE_A)],
                receipt: Some(test_receipt(root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let first = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(first.upserted, 3);
        assert_eq!(first.removed, 0);
        assert_eq!(first.commits, 1, "三条记录装得下一块，就只发一笔");
        assert_eq!(first.medium, PersistMedium::Mem);
        assert!(!first.medium.is_durable(), "内存模式的行活不过进程重启");
        assert_eq!(count(&db, "inst_relate").await, 2);
        assert_eq!(count(&db, "tubi_relate").await, 1);

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![stale.record_id.clone()],
                tube_branch_resets: vec![BRANCH],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        let second = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(second.upserted, 1);
        assert_eq!(second.removed, 2, "陈旧元素与上一轮的管身都要对账掉");
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(count(&db, "tubi_relate").await, 0);
    }

    /// 不完整投影没有根回执时，缺席不能被解释成孤儿。
    ///
    /// 两种走法落到这里——有人把「生成是整棵子树的」优化成「祖先根跳过嵌套根省重复计算」
    /// （core 的 R10 干的正是这件事），或者进程重启后存储是空的、只重投影了一根
    /// （`ModelMemoryStore` 是进程内的，**没有从库回灌的路径**）。
    ///
    /// 背景见 `docs/specs/core3d-partial-update-conformance.md` §8 与 R22。
    #[tokio::test]
    async fn an_unreceipted_partial_projection_does_not_delete_nested_root_rows() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let ancestor_root = RefNo::new(24383, 1);

        let full = ModelMemoryStore::default();
        full.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), nested_root_record()],
                receipt: Some(test_receipt(ancestor_root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        persist_root_on(target, &full, ancestor_root)
            .await
            .expect("persist ancestor");
        assert_eq!(count(&db, "inst_relate").await, 2);

        // 换一份只覆盖祖先根自己那一件的投影。
        let partial = ModelMemoryStore::default();
        partial.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                ..ProjectionCommit::default()
            }],
        );
        let rows = persist_root_on(target, &partial, ancestor_root)
            .await
            .expect("persist ancestor");
        assert_eq!(rows.removed, 0, "没有完整回执时不得把缺席解释成删除");
        assert_eq!(count(&db, "inst_relate").await, 2);
    }

    #[tokio::test]
    async fn reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        let target = PersistTarget::new(&db, PersistMedium::Mem);

        let full = ModelMemoryStore::default();
        full.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), nested_root_record()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &full, root).await.unwrap();

        let mismatched = ModelMemoryStore::default();
        mismatched.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        assert!(persist_root_on(target, &mismatched, root).await.is_err());
        assert_eq!(
            count(&db, "inst_relate").await,
            2,
            "failed validation is non-mutating"
        );

        let empty = ModelMemoryStore::default();
        empty.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                receipt: Some(test_receipt(root, 0)),
                ..ProjectionCommit::default()
            }],
        );
        let rows = persist_root_on(target, &empty, root).await.unwrap();
        assert_eq!(rows.removed, 2);
        assert_eq!(count(&db, "inst_relate").await, 0);
    }

    /// 切块提交的三条顺序：分支重置只在第一块（否则后一块把前一块的管身删掉）、管身槽位
    /// 跨块延续（否则同分支的 id 互撞）、回执在最后一块（一块失败就没有章）。
    /// 块大小拨到 1，三条一次全押上。
    #[tokio::test]
    async fn chunked_commits_reset_once_number_across_chunks_and_stamp_last() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);

        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    baked_record(),
                    tube_record(0, TUBE_A),
                    tube_record(0, TUBE_B),
                ],
                receipt: Some(test_receipt(root, 3)),
                ..ProjectionCommit::default()
            }],
        );

        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let rows = persist_root_in_chunks(target, &store, root, 1)
            .await
            .expect("persist");
        assert_eq!(rows.upserted, 3);
        assert_eq!(rows.receipts, 1);
        assert_eq!(rows.commits, 3, "块大小 1、三条记录，就该发三笔事务");

        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            2,
            "同分支两段分在两块里，前一块的那段不能被后一块的重置删掉"
        );
        let mut response = db
            .query("SELECT VALUE publication_status FROM type::thing('gen_root','24383_1');")
            .await
            .expect("receipt")
            .check()
            .expect("receipt statement");
        assert!(
            response
                .take::<Vec<String>>(0)
                .expect("receipt rows")
                .is_empty(),
            "夹具里没有 gen_root 行，UPDATE 是空操作——这一条只要求它不报错"
        );
    }

    async fn seed_gen_roots(db: &Surreal<Any>, roots: &[RefNo]) {
        for root in roots {
            db.query(format!(
                "CREATE type::thing('gen_root','{}_{}') SET dbnum=7997, status='Generated', \
                 publication_status='stale', desired_revision=2, published_revision=1;",
                root.word0, root.word1
            ))
            .await
            .expect("seed gen_root")
            .check()
            .expect("seed gen_root statements");
        }
    }

    async fn publication_status_of(db: &Surreal<Any>, root: RefNo) -> String {
        let mut response = db
            .query(format!(
                "SELECT VALUE publication_status FROM type::thing('gen_root','{}_{}');",
                root.word0, root.word1
            ))
            .await
            .expect("publication status")
            .check()
            .expect("publication status statement");
        response
            .take::<Vec<String>>(0)
            .expect("publication status rows")
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// 嵌套根与祖先根各自携带完整记录、每根各自盖章；重复 id 由幂等写收敛。
    /// 未提交的跨根认领会让前一根失败时后一根静默缺行，因此这里的发送账允许重复。
    #[tokio::test]
    async fn batched_persist_shares_chunks_across_roots_and_stamps_every_receipt() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let ancestor_root = RefNo::new(24383, 1);
        seed_gen_roots(&db, &[ancestor_root, NESTED_ROOT]).await;

        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), nested_root_record(), tube_record(0, TUBE_A)],
                receipt: Some(test_receipt(ancestor_root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                receipt: Some(test_receipt(NESTED_ROOT, 1)),
                ..ProjectionCommit::default()
            }],
        );

        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let batched = persist_roots_batched(
            target,
            &store,
            BATCH_DBNUM,
            &[ancestor_root, NESTED_ROOT],
            None,
        )
        .await
        .expect("batched persist");
        assert!(batched.failed.is_empty(), "{:?}", batched.failed);
        assert_eq!(batched.groups, 1);
        let rows = batched.rows.expect("rows");
        assert_eq!(rows.medium, PersistMedium::Mem);
        assert_eq!(
            rows.upserted, 4,
            "祖先根 3 条 + 嵌套根自己的 1 条；重复 id 由库内幂等收敛"
        );
        assert_eq!(rows.removed, 0);
        assert_eq!(rows.receipts, 2);
        assert_eq!(
            rows.commits, 1,
            "多根的几何、房间边与回执应合成一笔原子事务"
        );
        assert_eq!(count(&db, "inst_relate").await, 2);
        assert_eq!(count(&db, "tubi_relate").await, 1);
        assert_eq!(publication_status_of(&db, ancestor_root).await, "ready");
        assert_eq!(publication_status_of(&db, NESTED_ROOT).await, "ready");
    }

    /// 对账只读一次库：库里挂在根名下、存储里已经没有的几何在下一批里被删掉，
    /// 上一轮的管身随分支重置一起清。
    #[tokio::test]
    async fn batched_persist_reconciles_stale_rows_from_one_database_read() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        seed_gen_roots(&db, &[root]).await;

        let store = ModelMemoryStore::default();
        let stale = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            ..baked_record()
        };
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), stale.clone(), tube_record(0, TUBE_A)],
                receipt: Some(test_receipt(root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let first = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("first batch");
        assert!(first.failed.is_empty());
        assert_eq!(first.rows.expect("rows").upserted, 3);
        assert_eq!(count(&db, "inst_relate").await, 2);
        assert_eq!(count(&db, "tubi_relate").await, 1);

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![stale.record_id.clone()],
                tube_branch_resets: vec![BRANCH],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        let second = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("second batch");
        assert!(second.failed.is_empty());
        let rows = second.rows.expect("rows");
        assert_eq!(rows.upserted, 1);
        assert_eq!(rows.removed, 2, "陈旧元素与上一轮的管身都要对账掉");
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(count(&db, "tubi_relate").await, 0);
        assert_eq!(publication_status_of(&db, root).await, "ready");
    }

    /// 同一个数据窗口里连着落两批：第二批吃的是**内存里的基线快照**（对账读一个任务只做一次，
    /// 见 [`GeometryBaseline`]），对账结果必须与每批都重读一遍完全一样。
    ///
    /// 这一条钉的是 2026-09-10 那次 O(N²) 修复：7997 实测 68 组累计读 180 万行、单笔事务从
    /// 400 ms 涨到 1,280 ms。快照要是没跟着任务自己的写更新，陈旧行就会漏删——那是数据错，
    /// 不是慢。
    #[tokio::test]
    async fn a_task_reads_the_reconciliation_baseline_once_and_still_clears_stale_rows() {
        /// 同一个窗口里连落两批；`refetch = true` 时在第二批之前把快照丢掉（等价于旧行为，
        /// 每批都重读一次库），`false` 时吃上一批就地更新出来的快照。两者的终态必须一样。
        async fn two_batches(refetch: bool) -> (usize, i64, i64) {
            let db = surreal_mem().await;
            seed_pe(&db).await;
            let root = RefNo::new(24383, 1);
            seed_gen_roots(&db, &[root]).await;
            invalidate_geometry_baseline();

            let store = ModelMemoryStore::default();
            let stale = ProjectedGeometry {
                record_id: "24383_9999".into(),
                geometry_id: GeometryId::Element {
                    refno: "24383/9999".into(),
                },
                source_refno: RefNo::new(24383, 9999),
                ..baked_record()
            };
            let target = PersistTarget::new(&db, PersistMedium::Mem);

            crate::data_interface::batch_worker::with_active_data_window_for_test(
                BATCH_DBNUM,
                7,
                async {
                    store.commit(
                        ProjectionScope::Current,
                        vec![ProjectionCommit {
                            upserts: vec![baked_record(), stale.clone(), tube_record(0, TUBE_A)],
                            receipt: Some(test_receipt(root, 3)),
                            ..ProjectionCommit::default()
                        }],
                    );
                    persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
                        .await
                        .expect("first batch");

                    store.commit(
                        ProjectionScope::Current,
                        vec![ProjectionCommit {
                            removals: vec![stale.record_id.clone()],
                            tube_branch_resets: vec![BRANCH],
                            receipt: Some(test_receipt(root, 1)),
                            ..ProjectionCommit::default()
                        }],
                    );
                    if refetch {
                        invalidate_geometry_baseline();
                    }
                    let second = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
                        .await
                        .expect("second batch");
                    (
                        second.rows.expect("rows").removed,
                        count(&db, "inst_relate").await,
                        count(&db, "tubi_relate").await,
                    )
                },
            )
            .await
        }

        let cached = two_batches(false).await;
        let refetched = two_batches(true).await;
        invalidate_geometry_baseline();
        assert_eq!(
            cached, refetched,
            "对账基线收成一个任务读一次之后，终态必须与每批都重读一遍完全一样\
             （左：吃内存快照；右：每批重读）"
        );
    }

    /// ADR-072 D4（验收 ③）：合块落库提交之后，全局空间树**就地**追上投影——每条非管身记录
    /// 在树上恰有一条条目、盒逐位等于它的 `world_bounds`、noun 逐字相同；管身不进树；
    /// 被对账掉的来源从树上消失；再落一次换了盒的记录，树上换新不堆叠。全程不回读库：
    /// 树里的盒来自 `ProjectedGeometry`，库里的 `aabb.d` 只是同一份值的落盘副本。
    ///
    /// 本用例独占来源 `24383/73948`、`24383/500`、`24383/600`、`24383/9999` 在全局树上的条目，
    /// 结束时清掉，不影响同进程其它用例。
    #[tokio::test]
    async fn batched_persist_converges_the_spatial_tree_from_projected_world_bounds() {
        use aios_core::room::room::GLOBAL_AABB_TREE;

        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        seed_gen_roots(&db, &[root]).await;
        invalidate_geometry_baseline();

        let stale = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            world_bounds: [[-9.0, -9.0, -9.0], [-8.0, -8.0, -8.0]],
            ..baked_record()
        };
        let mut moved = other_canonical_record();
        moved.world_bounds = [[10.0, 20.0, 30.0], [11.0, 22.0, 33.0]];
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    baked_record(),
                    moved.clone(),
                    stale.clone(),
                    tube_record(0, TUBE_A),
                ],
                receipt: Some(test_receipt(root, 4)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::converging_for_test(&db, PersistMedium::Mem);
        let sources = [
            refno_from_e3d(BRANCH),
            refno_from_e3d(RefNo::new(24383, 500)),
            refno_from_e3d(RefNo::new(24383, 9999)),
        ];
        crate::fast_model::aabb_tree::evict_tree_entries_for_test(&sources).await;

        let first = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("first batch");
        assert!(first.failed.is_empty(), "{:?}", first.failed);

        let entries_of = |tree: &aios_core::accel_tree::acceleration_tree::AccelerationTree,
                          source: aios_core::RefnoEnum| {
            tree.iter()
                .filter(|entry| entry.refno == source.refno())
                .cloned()
                .collect::<Vec<_>>()
        };
        let expect_box = |record: &ProjectedGeometry| {
            parry3d::bounding_volume::Aabb::new(
                record.world_bounds[0].into(),
                record.world_bounds[1].into(),
            )
        };
        {
            let tree = GLOBAL_AABB_TREE.read().await;
            for record in [&baked_record(), &moved, &stale] {
                let entries = entries_of(&tree, refno_from_e3d(record.source_refno));
                assert_eq!(
                    entries.len(),
                    1,
                    "{} 在树上恰一条: {entries:?}",
                    record.record_id
                );
                assert_eq!(
                    entries[0].aabb,
                    expect_box(record),
                    "盒 = 投影 world_bounds"
                );
                assert_eq!(entries[0].noun, record.noun, "noun 逐字进树");
            }
            assert!(
                tree.iter().all(|entry| entry.refno
                    != refno_from_e3d(RefNo::new(24383, 10)).refno()
                    && entry.refno != refno_from_e3d(RefNo::new(24383, 20)).refno()),
                "隐式管身不进树"
            );
        }

        // 第二批：9999 从投影里消失（被对账掉），500 的盒换了。
        let mut moved_again = moved.clone();
        moved_again.world_bounds = [[100.0, 200.0, 300.0], [101.0, 202.0, 303.0]];
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![stale.record_id.clone()],
                upserts: vec![moved_again.clone()],
                receipt: Some(test_receipt(root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        let second = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("second batch");
        assert!(second.failed.is_empty(), "{:?}", second.failed);
        // 9999 被对账掉；夹具管身的 record_id 不是真摘要，每轮都会被当陈旧行对账掉再插回
        //（与 `batched_persist_reconciles_stale_rows_from_one_database_read` 同一口径）。
        assert_eq!(
            second.rows.expect("rows").removed,
            2,
            "9999 与夹具管身被对账掉"
        );
        assert_eq!(count(&db, "inst_relate").await, 2, "库里只剩 73948 与 500");
        {
            let tree = GLOBAL_AABB_TREE.read().await;
            assert!(
                entries_of(&tree, refno_from_e3d(RefNo::new(24383, 9999))).is_empty(),
                "被对账掉的来源必须从树上消失"
            );
            let entries = entries_of(&tree, refno_from_e3d(RefNo::new(24383, 500)));
            assert_eq!(entries.len(), 1, "换盒不堆叠: {entries:?}");
            assert_eq!(entries[0].aabb, expect_box(&moved_again), "树上是新盒");
            assert_eq!(
                entries_of(&tree, refno_from_e3d(BRANCH))[0].aabb,
                expect_box(&baked_record()),
                "没动的记录原盒不变"
            );
        }
        // 树上的盒与库里落下的 aabb.d 是同一份值。
        let mut response = db
            .query("SELECT VALUE aabb.d FROM type::thing('inst_relate', '24383_500');")
            .await
            .expect("aabb query")
            .check()
            .expect("aabb statement");
        let landed: Option<parry3d::bounding_volume::Aabb> = response.take(0).expect("aabb row");
        assert_eq!(landed, Some(expect_box(&moved_again)));

        crate::fast_model::aabb_tree::evict_tree_entries_for_test(&sources).await;
        invalidate_geometry_baseline();
    }

    /// ADR-072 D4 的源码钉（验收 ②）：`persist_roots_batched` / `persist_root_chunks_impl` 提交之后的
    /// 空间收敛**不带**数据窗口（`file_round` / `active_data_window`）条件，也不再往
    /// `spatial_reconcile` 意图行或 `task_effects` 内存账里寄名单；清边不再按窗口分叉；
    /// 收敛的盒来自投影记录（`projected_tree_entry`），不回读库。回退即红。
    #[test]
    fn spatial_convergence_after_persist_is_unconditional_and_takes_projected_boxes() {
        let source = include_str!("model_db_adapter.rs");
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        for retired in [
            "file_round.is_none()",
            "render_file_spatial_reconcile_upsert",
            "task_effects::record",
            "render_spatial_fragment",
            concat!("sync_tree_from_", "committed_pointers"),
        ] {
            assert!(
                !production.contains(retired),
                "落库路径不得再按窗口分叉 / 寄意图行（{retired}）"
            );
        }
        let batched = production
            .split_once("pub(crate) async fn persist_roots_batched_with_chunk_bytes(")
            .expect("batched persist exists")
            .1
            .split_once("struct GroupOutcome")
            .expect("group outcome follows")
            .0;
        let converge = batched
            .split_once("if target.converge_spatial && total.commits > 0")
            .expect("合块落库提交后必须收敛空间树")
            .1;
        assert!(
            converge.contains("converge_tree_after_persist(") && converge.contains("boxes_all"),
            "收敛喂的是各组攒下的投影盒: {converge}"
        );
        assert!(
            !batched.contains("active_data_window().is_none()")
                && !batched.contains("baseline_window.is_none()"),
            "数据窗口只给对账基线当作用域键，不得门控收敛: {batched}"
        );
        let group = production
            .split_once("async fn persist_group(")
            .expect("persist_group exists")
            .1
            .split_once("/// The WS router can retain a pending response until reconnection.")
            .expect("publication request helpers follow")
            .0;
        assert!(
            group.contains("filter_map(crate::fast_model::aabb_tree::projected_tree_entry)"),
            "组内记录直接折成树条目: {group}"
        );
        assert!(
            !group.contains("file_round"),
            "persist_group 不再认识数据窗口: {group}"
        );
        let per_root = production
            .split_once("async fn persist_root_chunks_impl(")
            .expect("per-root persist exists")
            .1
            .split_once("/// 写入口径：**没有 UPDATE，也没有 UPSERT**。")
            .expect("write mode follows")
            .0;
        assert!(
            per_root.contains("if target.converge_spatial && commits > 0 {")
                && per_root.contains("tree_boxes,"),
            "逐根落库提交后同样无条件按投影盒收敛: {per_root}"
        );
        assert!(
            !per_root.contains("file_round"),
            "逐根落库不再认识数据窗口: {per_root}"
        );
    }

    /// 软块上限不能拆开一个根：几何、房间边和回执必须是一笔事务。
    #[tokio::test]
    async fn batched_persist_keeps_one_root_in_one_transaction_even_above_soft_chunk_limit() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        seed_gen_roots(&db, &[root]).await;

        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    baked_record(),
                    tube_record(0, TUBE_A),
                    tube_record(0, TUBE_B),
                ],
                receipt: Some(test_receipt(root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let batched =
            persist_roots_batched_with_chunk_bytes(target, &store, BATCH_DBNUM, &[root], 1, None)
                .await
                .expect("batched persist");
        assert!(batched.failed.is_empty(), "{:?}", batched.failed);
        let rows = batched.rows.expect("rows");
        assert_eq!(rows.upserted, 3);
        assert_eq!(rows.commits, 1, "软字节上限不得拆开一个根");
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            2,
            "同分支两段分在两块里，前一块的那段不能被后一块的重置删掉"
        );
        assert_eq!(publication_status_of(&db, root).await, "ready");
    }

    /// 一批只属于一个 dbnum（任务是隔离单元）；混进别的库的根整批拒收、一行不写。
    #[tokio::test]
    async fn batched_persist_refuses_roots_of_another_dbnum() {
        let db = surreal_mem().await;
        let store = ModelMemoryStore::default();
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        let error = persist_roots_batched(
            target,
            &store,
            BATCH_DBNUM,
            &[RefNo::new(24383, 1), RefNo::new(1, 1)],
            None,
        )
        .await
        .expect_err("foreign root must be refused");
        assert!(error.to_string().contains("dbnum"), "{error}");
        assert_eq!(count(&db, "inst_relate").await, 0);
    }

    /// PDMS 的文本不许从字符串字面量里跑出来。`noun` 是库里读回来的，四字母代码撞上
    /// 引号的概率约等于零——但这是口径不是概率，裸插值只要留一处，下一个加字段的人
    /// 就会照抄。转义之后语句照旧跑得通，行上的值也一字不差。
    #[tokio::test]
    async fn a_quote_in_a_pdms_noun_never_escapes_its_string_literal() {
        let record = ProjectedGeometry {
            noun: "IT'S\\BAD".into(),
            ..baked_record()
        };
        let sql = render_records(&[record], WriteMode::Initial).expect("render");
        assert!(sql.contains(r"generic:'IT\'S\\BAD'"), "{sql}");

        let db = surreal_mem().await;
        seed_pe(&db).await;
        db.query(publication_transaction(&sql))
            .await
            .expect("publish")
            .check()
            .expect("publish statements");
        let mut response = db
            .query("SELECT VALUE generic FROM type::thing('inst_relate','24383_73948');")
            .await
            .expect("generic")
            .check()
            .expect("generic statement");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("generic rows"),
            vec!["IT'S\\BAD".to_string()]
        );
    }

    /// `⟨⟩` 之间没有转义序列可用，所以那一档只能拒绝。铸 id 的地方一旦漏出别的字符，
    /// 宁可整块渲染失败，也不要发出一条能被从中间拆开的语句。
    #[test]
    fn a_record_id_outside_the_machine_alphabet_is_refused() {
        let bad = ProjectedGeometry {
            record_id: "24383_1⟩,x:1".into(),
            ..baked_record()
        };
        let error = render_records(&[bad], WriteMode::Initial).expect_err("must refuse");
        assert!(format!("{error:#}").contains("字母表"), "{error:#}");

        let bad_mesh = ProjectedGeometry {
            mesh_id: "mesh⟩'".into(),
            ..baked_record()
        };
        assert!(render_records(&[bad_mesh], WriteMode::Initial).is_err());
    }

    /// 旧命名空间**还有人指着的时候一个字都不许删**。
    ///
    /// 这不是防御性代码：切换之前写下的 `inst_relate` 用的就是不带前缀的 id，所以它们还活着，
    /// 而 `out` / `aabb` / `world_trans` 三个指针一直指着 `direct_*`。AMS 7997 现场 22,040 行
    /// 全是这个形状——那时候扫，等于把整库的包围盒与世界变换连根删掉。
    #[tokio::test]
    async fn the_legacy_sweep_refuses_while_anything_still_points_at_the_old_namespace() {
        let db = surreal_mem().await;
        seed_legacy_namespace(&db).await;
        db.query(
            "CREATE type::thing('inst_relate','24383_73948') SET \
             aabb=type::thing('aabb','direct_world_24383_73948');",
        )
        .await
        .expect("seed live pointer")
        .check()
        .expect("seed live pointer statements");

        let target = PersistTarget::new(&db, PersistMedium::Mem);
        sweep_legacy_prefixes(target).await.expect("sweep");
        assert_eq!(count(&db, "aabb").await, 1, "还有人指着，不许删");
        assert_eq!(count(&db, "inst_info").await, 1);

        // 指针改指新命名空间（= 这一根被重生成过了），旧行这才真的悬空。
        db.query(
            "UPDATE type::thing('inst_relate','24383_73948') SET \
             aabb=type::thing('aabb','24383_73948');",
        )
        .await
        .expect("repoint")
        .check()
        .expect("repoint statements");
        sweep_legacy_prefixes(target).await.expect("sweep");
        assert_eq!(count(&db, "aabb").await, 0);
        assert_eq!(count(&db, "inst_info").await, 0);
    }

    /// 旧共享本地盒（`aabb:direct_shared_<mesh>`）**留着**：活着的 `inst_geo` 行还指着它们，
    /// 而 `inst_geo` 按 N2 只增不删——新一轮生成铸的是新 mesh id，旧那几行永远在库里。
    ///
    /// 同一条也钉住反面：旧 `geo_relate`（`in` 指着旧 `inst_info`）**不是**活引用，它自己就是
    /// 被删的那一批。把它算进守卫，清扫就永远拦住自己——7997 真库上正是这 7,908 行拦住了它。
    #[tokio::test]
    async fn the_legacy_sweep_keeps_the_shared_boxes_that_inst_geo_still_points_at() {
        let db = surreal_mem().await;
        db.query(
            "CREATE type::thing('aabb','direct_shared_123456789');\
             CREATE type::thing('aabb','direct_world_24383_73948');\
             CREATE inst_geo:⟨123456789⟩ SET aabb=type::thing('aabb','direct_shared_123456789');\
             CREATE type::thing('inst_info','direct_24383_73948');\
             CREATE type::thing('trans','direct_local_24383_73948');\
             INSERT RELATION IGNORE INTO geo_relate [{id:geo_relate:[type::thing('inst_info','direct_24383_73948'),inst_geo:⟨123456789⟩],in:type::thing('inst_info','direct_24383_73948'),out:inst_geo:⟨123456789⟩}];",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");

        let target = PersistTarget::new(&db, PersistMedium::Mem);
        sweep_legacy_prefixes(target).await.expect("sweep");

        assert_eq!(count(&db, "geo_relate").await, 0, "旧关系行不该拦住自己");
        assert_eq!(count(&db, "inst_info").await, 0);
        assert_eq!(count(&db, "trans").await, 0);
        assert_eq!(count(&db, "inst_geo").await, 1, "共享网格只增不删");
        let mut response = db
            .query("SELECT VALUE record::id(id) FROM aabb;")
            .await
            .expect("aabb")
            .check()
            .expect("aabb statement");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("aabb rows"),
            vec!["direct_shared_123456789".to_string()],
            "世界盒扫走，inst_geo 还指着的共享本地盒留着"
        );
    }

    /// 清扫**按库一次**：扫完在库上盖标记，之后再调都不再全表扫。标记没盖住的话，
    /// 每一次整库落库都要把三张表从头扫一遍。
    #[tokio::test]
    async fn the_legacy_sweep_runs_once_per_database() {
        let db = surreal_mem().await;
        seed_legacy_namespace(&db).await;
        let target = PersistTarget::new(&db, PersistMedium::Mem);

        sweep_legacy_prefixes(target).await.expect("sweep");
        assert_eq!(count(&db, "inst_info").await, 0, "旧命名空间该被扫走");

        // 再摆一行旧格式的：标记已经盖上，这一次不该再扫。
        db.query("CREATE type::thing('inst_info','direct_24383_500');")
            .await
            .expect("reseed legacy")
            .check()
            .expect("reseed legacy statements");
        sweep_legacy_prefixes(target).await.expect("sweep");
        assert_eq!(count(&db, "inst_info").await, 1, "扫过的库不再重扫");
    }

    async fn seed_legacy_namespace(db: &Surreal<Any>) {
        db.query(
            "CREATE type::thing('inst_info','direct_24383_73948');\
             CREATE type::thing('aabb','direct_world_24383_73948');",
        )
        .await
        .expect("seed legacy")
        .check()
        .expect("seed legacy statements");
    }

    /// 切块之后一根不再是原子边界，所以多块发布先在凭证行上盖一道栅栏：中途失败时
    /// `publication_status` 停在 `publishing`，凭证前移与窗口规划器都不会再把这根当成
    /// 已发布的几何。解除它的只有最后一块那张回执。
    #[tokio::test]
    async fn a_half_written_root_stays_fenced_until_the_receipt() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_root(&db).await;
        let root = RefNo::new(24383, 1);

        // 第二块数据库提交失败；预渲染验证已经完成，第一块必须保留发布栅栏。
        db.query("DEFINE EVENT reject_second_chunk ON TABLE inst_relate WHEN $after.generic = 'REJECT' THEN { THROW 'injected second chunk failure'; };").await.unwrap().check().unwrap();
        let doomed = ProjectedGeometry {
            record_id: "zzbad".into(),
            noun: "REJECT".into(),
            ..baked_record()
        };
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), doomed],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        assert!(
            persist_root_in_chunks(target, &store, root, 1)
                .await
                .is_err()
        );
        assert_eq!(publication_status(&db).await, "publishing");

        // 换成两条都渲染得出来的记录重跑，最后一块的回执把栅栏解除。
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), other_canonical_record()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_in_chunks(target, &store, root, 1)
            .await
            .expect("persist");
        assert_eq!(publication_status(&db).await, "ready");
    }

    /// 没有回执就没人把栅栏翻回 `ready`，那时盖栅栏等于把这根永久钉死。所以栅栏只跟着
    /// 回执走：没章的落库一个字都不许动凭证行。
    #[tokio::test]
    async fn a_publish_without_a_receipt_never_fences_the_credential() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_root(&db).await;
        let root = RefNo::new(24383, 1);

        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), other_canonical_record()],
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        persist_root_in_chunks(target, &store, root, 1)
            .await
            .expect("persist");
        assert_eq!(publication_status(&db).await, "stale");
    }

    /// 房间派生面与空间版本号回到发布事务上（ADR-060 P3）：被对账掉的来源两个方向的
    /// 房间边一并清掉，拿到新几何的来源排一次房间重算，动过几何的每一块顺手把
    /// `spatial_epoch` +1——少这一下，崩溃重启会按 sidecar 复用一棵停在旧盒上的树。
    #[tokio::test]
    async fn room_edges_and_the_spatial_epoch_ride_the_publication_transaction() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        let stale = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            ..baked_record()
        };

        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), stale.clone()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        persist_root_on(target, &store, root)
            .await
            .expect("persist");

        // 陈旧几何的来源在房间里有一条归属边，一条面板边。
        db.query(
            "CREATE room:⟨1⟩;\
             RELATE pe:⟨24383_9999⟩->room_relate->room:⟨1⟩;\
             RELATE pe:⟨24383_9999⟩->room_panel_relate->room:⟨1⟩;\
             RELATE pe:⟨24383_73948⟩->room_relate->room:⟨1⟩;",
        )
        .await
        .expect("seed room")
        .check()
        .expect("seed room statements");
        let epoch_before = spatial_epoch(&db).await;

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![stale.record_id.clone()],
                receipt: Some(test_receipt(root, 1)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &store, root)
            .await
            .expect("persist");

        let mut response = db
            .query("SELECT VALUE <string>in FROM room_relate;")
            .await
            .expect("room edges")
            .check()
            .expect("room edge statement");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("room edge rows"),
            vec!["pe:24383_73948".to_string()],
            "被移除的来源清边，拿到新几何的来源留着（重算自己会先清后写）"
        );
        assert_eq!(count(&db, "room_panel_relate").await, 0);
        assert!(
            spatial_epoch(&db).await > epoch_before,
            "动过几何的事务必须把 spatial_epoch 推高"
        );
        // 「留下来的那条几何要排一次房间重算」那一半随 pending 表退了（ADR-072 D5）；房间归属
        // 如今随每根的模型行同事务写（ADR-073 D1，见下面 `room_*` 一族用例）。
    }

    // ---- ADR-073 D1：逐根房间归属随模型行同事务落边 ------------------------------------

    const ROOM_ROOT_A: RefNo = RefNo::new(24383, 10);
    const ROOM_ROOT_B: RefNo = RefNo::new(24383, 20);
    const PANEL_R1: RefNo = RefNo::new(24383, 500);
    const PANEL_R2: RefNo = RefNo::new(24383, 600);

    /// 一根一件的夹具记录：根就是构件自己，盒子沿 x 从 `min` 到 `max`、y / z 落在 400–600。
    /// 行上的 `dbnum` 与根的 `dbno()` 同值（生产口径）——删干净的根对账时只剩根自己的 dbnum 可问。
    fn room_record(root: RefNo, min: f32, max: f32) -> ProjectedGeometry {
        ProjectedGeometry {
            record_id: format!("{}_{}", root.word0, root.word1),
            geometry_id: GeometryId::Element {
                refno: format!("{}/{}", root.word0, root.word1),
            },
            source_refno: root,
            dbnum: BATCH_DBNUM,
            world_bounds: [[min, 400.0, 400.0], [max, 600.0, 600.0]],
            ancestors: vec![packed_refno(root), packed_refno(RefNo::new(24383, 1))],
            ..baked_record()
        }
    }

    /// 两块面板盒：R1 占 x 0–1000，R2 占 x 500–1500（重叠段 500–1000 里的构件同属两间）。
    /// 面板几何来自内存投影，与生产同一条 `from_projection` 路；盒子只用 AABB 判定就够，
    /// 落在盒里的构件不读任何网格文件。
    fn room_calculator(
        dir: &std::path::Path,
    ) -> crate::fast_model::room_model::MemoryRoomCalculator {
        use crate::fast_model::room_model::{RoomPanelMap, RoomPanels, box_mesh_for_test};
        use crate::fast_model::room_resident::{
            MeshCache, ResidentForm, ResidentManifest, ResidentRoomModel,
        };
        box_mesh_for_test(glam::Vec3::ZERO, glam::Vec3::splat(1000.))
            .ser_to_file(&dir.join("panel_r1.mesh"))
            .unwrap();
        box_mesh_for_test(
            glam::Vec3::new(500., 0., 0.),
            glam::Vec3::new(1500., 1000., 1000.),
        )
        .ser_to_file(&dir.join("panel_r2.mesh"))
        .unwrap();
        let panel = |refno: RefNo, mesh: &str, min: [f32; 3], max: [f32; 3]| ProjectedGeometry {
            record_id: format!("{}_{}", refno.word0, refno.word1),
            geometry_id: GeometryId::Element {
                refno: format!("{}/{}", refno.word0, refno.word1),
            },
            source_refno: refno,
            noun: "PANE".into(),
            mesh_id: mesh.into(),
            world_bounds: [min, max],
            ancestors: vec![packed_refno(refno)],
            ..baked_record()
        };
        let r1: RefnoEnum = refno_from_e3d(PANEL_R1);
        let r2: RefnoEnum = refno_from_e3d(PANEL_R2);
        let map = RoomPanelMap {
            rooms: vec![
                RoomPanels {
                    room: "24383_501".into(),
                    room_num: "R1".into(),
                    panels: vec![r1],
                },
                RoomPanels {
                    room: "24383_601".into(),
                    room_num: "R2".into(),
                    panels: vec![r2],
                },
            ],
            all_panels: std::collections::HashSet::from([r1, r2]),
        };
        let model = std::sync::Arc::new(ResidentRoomModel::from_projection(
            ResidentManifest {
                form: ResidentForm::Latest,
                key: "rooms-fixture".into(),
            },
            vec![],
            map,
            &[
                panel(PANEL_R1, "panel_r1", [0.; 3], [1000.; 3]),
                panel(PANEL_R2, "panel_r2", [500., 0., 0.], [1500., 1000., 1000.]),
            ],
            std::sync::Arc::<MeshCache>::default(),
        ));
        crate::fast_model::room_model::MemoryRoomCalculator::new(model, dir.to_path_buf())
            .expect("room calculator")
    }

    /// 库里全部房间边，`面板->构件` 形态、按字典序。
    async fn room_edges(db: &Surreal<Any>) -> Vec<String> {
        #[derive(serde::Deserialize)]
        struct Edge {
            i: String,
            o: String,
        }
        let mut response = db
            .query("SELECT <string>in AS i, <string>out AS o FROM room_relate;")
            .await
            .expect("room edges")
            .check()
            .expect("room edge statement");
        let mut edges = response
            .take::<Vec<Edge>>(0)
            .expect("room edge rows")
            .into_iter()
            .map(|edge| format!("{}->{}", edge.i, edge.o))
            .collect::<Vec<_>>();
        edges.sort();
        edges
    }

    fn room_store(commits: Vec<ProjectionCommit>) -> ModelMemoryStore {
        let store = ModelMemoryStore::default();
        store.commit(ProjectionScope::Current, commits);
        store
    }

    fn root_commit(root: RefNo, records: Vec<ProjectedGeometry>) -> ProjectionCommit {
        let count = records.len();
        ProjectionCommit {
            upserts: records,
            receipt: Some(test_receipt(root, count)),
            ..ProjectionCommit::default()
        }
    }

    /// 空 kv-mem 不预建 `pe` / `gen_root` / 属性表也必须能接住一根完整发布：
    /// 模型行、房间边、数据库回执与进程 ACK 一次到位。
    #[tokio::test]
    async fn empty_kv_mem_publishes_an_indoor_root_without_attribute_ingestion() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        let store = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);

        let batched = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&rooms),
        )
        .await
        .expect("publish into empty kv-mem");

        assert!(batched.failed.is_empty(), "{:?}", batched.failed);
        assert_eq!(
            count(&db, "pe").await,
            0,
            "read-through must not ingest attributes"
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 1);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()]
        );
        assert_eq!(publication_status_of(&db, ROOM_ROOT_A).await, "ready");
        let ack = store
            .publication_ack(ProjectionScope::Current, ROOM_ROOT_A)
            .expect("complete publication ACK");
        assert_eq!(ack.source_sesno, 42);
        assert_eq!(ack.geometry_count, 1);
        assert!(ack.room_membership);
        assert_eq!(ack.room_generation.as_deref(), Some("rooms-fixture"));
        assert_eq!(ack.medium, PersistMedium::Mem);
    }

    /// ADR-073 D1 ①：一根算完即可查——A 的房间边与模型行同一块、同一笔落下、马上在库里；同组的 B
    /// 房间算不出来（盒子不可用）在写库之前就剔出组、整根不落库、进 `failed`（d-932 口径 2），
    /// A 的边与章一点不受它牵连。事务账：A 的几何 + 房间边 + 回执共一笔。
    #[tokio::test]
    async fn a_root_lands_its_room_edges_with_its_model_rows_before_the_next_root_starts() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_roots(&db, &[ROOM_ROOT_A, ROOM_ROOT_B]).await;
        let store = room_store(vec![
            root_commit(ROOM_ROOT_A, vec![room_record(ROOM_ROOT_A, 100., 200.)]),
            // 盒子带 NaN（`aabb_is_usable` 不认）：房间算不出来，这一根一行都不能落。
            root_commit(ROOM_ROOT_B, vec![room_record(ROOM_ROOT_B, f32::NAN, 200.)]),
        ]);

        let batched = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A, ROOM_ROOT_B],
            Some(&rooms),
        )
        .await
        .expect("persist");

        assert_eq!(batched.groups, 1, "两根同一组（≤ 512 根一组）");
        assert_eq!(batched.room_edges, 1);
        assert_eq!(
            batched
                .failed
                .iter()
                .map(|(root, _)| *root)
                .collect::<Vec<_>>(),
            vec![ROOM_ROOT_B]
        );
        assert!(
            batched.failed[0].1.contains("room membership"),
            "失败原因要说明是房间算不出来: {}",
            batched.failed[0].1
        );
        let rows = batched.rows.expect("rows");
        assert_eq!(rows.upserted, 1, "B 在写库之前就剔出去了，只渲 A 的行");
        assert_eq!(rows.commits, 1, "A 的几何、房间边与回执必须是一笔");
        assert_eq!(rows.receipts, 1);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()],
            "A 在 R1 里；B 没算出来就一条边都没有"
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 1);
        assert_eq!(
            count_record(&db, "inst_relate", "24383_20").await,
            0,
            "房间失败 = 该根失败：B 的模型行也不落"
        );
        assert_eq!(publication_status_of(&db, ROOM_ROOT_A).await, "ready");
        assert_eq!(
            publication_status_of(&db, ROOM_ROOT_B).await,
            "stale",
            "B 没盖章，窗口规划器眼里仍未发布"
        );
    }

    /// ADR-073 D1 ②：模型行写失败，对应的房间边不可见。库侧一条事件在 A 的模型行上 THROW，
    /// 整笔事务回滚——同一笔里的房间边一起消失。赔付边界是根：同组 B 不受 A 牵连，
    /// 当轮即可独立落下；B 再来一轮保持幂等。
    #[tokio::test]
    async fn a_failed_model_write_leaves_no_room_edge_behind() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_roots(&db, &[ROOM_ROOT_A, ROOM_ROOT_B]).await;
        db.query(
            "DEFINE TABLE inst_relate SCHEMALESS; \
             DEFINE EVENT boom ON TABLE inst_relate WHEN $after.generic = 'BOOM' THEN { THROW 'boom' };",
        )
        .await
        .expect("define event")
        .check()
        .expect("define event statements");
        let mut doomed = room_record(ROOM_ROOT_A, 100., 200.);
        doomed.noun = "BOOM".into();
        let store = room_store(vec![
            root_commit(ROOM_ROOT_A, vec![doomed]),
            root_commit(ROOM_ROOT_B, vec![room_record(ROOM_ROOT_B, 300., 400.)]),
        ]);

        let batched = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A, ROOM_ROOT_B],
            Some(&rooms),
        )
        .await
        .expect("persist");

        assert_eq!(
            batched
                .failed
                .iter()
                .map(|(root, _)| *root)
                .collect::<Vec<_>>(),
            vec![ROOM_ROOT_A],
            "根是赔付边界：A 失败不得连坐 B"
        );
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_20".to_string()],
            "A 的边随事务回滚，B 的边独立提交"
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 0);
        assert_eq!(count_record(&db, "inst_relate", "24383_20").await, 1);
        assert_eq!(publication_status_of(&db, ROOM_ROOT_A).await, "stale");
        assert_eq!(publication_status_of(&db, ROOM_ROOT_B).await, "ready");

        // B 单独再落一轮：它的行与边同一笔出现。
        let retried = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_B],
            Some(&rooms),
        )
        .await
        .expect("persist");
        assert!(retried.failed.is_empty(), "{:?}", retried.failed);
        assert_eq!(retried.room_edges, 1);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_20".to_string()]
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_20").await, 1);
        assert_eq!(publication_status_of(&db, ROOM_ROOT_B).await, "ready");
    }

    /// 一个祖先根事务失败后，不能把尚未提交的共享记录从随后嵌套根的事务里拿走。
    ///
    /// 这里故意不预建 `pe` / 属性行，等价于空 kv-mem 的读透发布目标。祖先根 A 先认领
    /// A 与嵌套根 B 共有的成员，但 A 的 BOOM 行让整根事务回滚；B 必须仍携带自己的完整
    /// 模型行与房间边并独立盖章。
    #[tokio::test]
    async fn a_failed_ancestor_cannot_steal_an_uncommitted_shared_record_from_a_nested_root() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        let ancestor_root = RefNo::new(24383, 1);
        let nested_root = RefNo::new(24383, 30);
        seed_gen_roots(&db, &[ancestor_root, nested_root]).await;
        db.query(
            "DEFINE TABLE inst_relate SCHEMALESS; \
             DEFINE EVENT boom ON TABLE inst_relate WHEN $after.generic = 'BOOM' THEN { THROW 'boom' };",
        )
        .await
        .expect("define event")
        .check()
        .expect("define event statements");

        let mut doomed = room_record(ROOM_ROOT_A, 100., 200.);
        doomed.noun = "BOOM".into();
        let shared = room_record(nested_root, 100., 200.);
        let store = room_store(vec![
            root_commit(ancestor_root, vec![doomed, shared.clone()]),
            ProjectionCommit {
                receipt: Some(test_receipt(nested_root, 1)),
                ..ProjectionCommit::default()
            },
        ]);

        let batched = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ancestor_root, nested_root],
            Some(&rooms),
        )
        .await
        .expect("persist");

        assert_eq!(
            batched
                .failed
                .iter()
                .map(|(root, _)| *root)
                .collect::<Vec<_>>(),
            vec![ancestor_root],
            "ancestor failure must not be charged to the nested root"
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 0);
        assert_eq!(
            count_record(&db, "inst_relate", "24383_30").await,
            1,
            "nested root must publish the shared row after the ancestor transaction rolls back"
        );
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_30".to_string()],
            "the shared member's room edge must be published with the nested root"
        );
        assert_eq!(publication_status_of(&db, ancestor_root).await, "stale");
        assert_eq!(publication_status_of(&db, nested_root).await, "ready");
    }

    /// ADR-073 D1 ③：更新一个根不丢其他根的结果——移动 / 删除 / 空归属 / 多房间归属四条路都走一遍，
    /// 每一步 B 的边原样不动：
    /// 1. A、B 都在 R1；2. A 移进 R1 ∩ R2 → A 两条边；3. A 移出所有房间 → A 只剩 DELETE、边清空；
    /// 4. A 的几何被删（对账）→ A 的边随删除那一块清掉。
    #[tokio::test]
    async fn updating_one_root_rewrites_its_own_room_edges_and_leaves_the_other_roots_alone() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_roots(&db, &[ROOM_ROOT_A, ROOM_ROOT_B]).await;
        async fn persist(
            db: &Surreal<Any>,
            rooms: &crate::fast_model::room_model::MemoryRoomCalculator,
            store: &ModelMemoryStore,
            roots: &[RefNo],
        ) -> BatchedPersist {
            let batched = persist_roots_batched(
                PersistTarget::new(db, PersistMedium::Mem),
                store,
                BATCH_DBNUM,
                roots,
                Some(rooms),
            )
            .await
            .expect("persist");
            assert!(batched.failed.is_empty(), "{:?}", batched.failed);
            batched
        }

        // 1. 两根都在 R1。
        let store = room_store(vec![
            root_commit(ROOM_ROOT_A, vec![room_record(ROOM_ROOT_A, 100., 200.)]),
            root_commit(ROOM_ROOT_B, vec![room_record(ROOM_ROOT_B, 300., 400.)]),
        ]);
        let first = persist(&db, &rooms, &store, &[ROOM_ROOT_A, ROOM_ROOT_B]).await;
        assert_eq!(first.room_edges, 2);
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24383_10".to_string(),
                "pe:24383_500->pe:24383_20".to_string(),
            ]
        );

        // 2. A 移进 R1 与 R2 的重叠段：两条边；B 不动。
        store.commit(
            ProjectionScope::Current,
            vec![root_commit(
                ROOM_ROOT_A,
                vec![room_record(ROOM_ROOT_A, 600., 700.)],
            )],
        );
        let moved = persist(&db, &rooms, &store, &[ROOM_ROOT_A]).await;
        assert_eq!(moved.room_edges, 2);
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24383_10".to_string(),
                "pe:24383_500->pe:24383_20".to_string(),
                "pe:24383_600->pe:24383_10".to_string(),
            ]
        );

        // 3. A 移到所有房间之外：新归属为空也要写出清除；B 不动。
        store.commit(
            ProjectionScope::Current,
            vec![root_commit(
                ROOM_ROOT_A,
                vec![room_record(ROOM_ROOT_A, 5000., 5100.)],
            )],
        );
        let outside = persist(&db, &rooms, &store, &[ROOM_ROOT_A]).await;
        assert_eq!(outside.room_edges, 0);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_20".to_string()]
        );

        // 回到 R1 再删掉几何：被对账掉的来源清边，B 仍在。
        store.commit(
            ProjectionScope::Current,
            vec![root_commit(
                ROOM_ROOT_A,
                vec![room_record(ROOM_ROOT_A, 100., 200.)],
            )],
        );
        persist(&db, &rooms, &store, &[ROOM_ROOT_A]).await;
        assert_eq!(room_edges(&db).await.len(), 2);
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec!["24383_10".into()],
                receipt: Some(test_receipt(ROOM_ROOT_A, 0)),
                ..ProjectionCommit::default()
            }],
        );
        let deleted = persist(&db, &rooms, &store, &[ROOM_ROOT_A]).await;
        assert_eq!(deleted.rows.expect("rows").removed, 1);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_20".to_string()],
            "删几何的根清掉自己的边，别的根的边原样"
        );
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 0);
    }

    /// 没有房间计算器（本进程没有房间定义）时只清被对账掉来源的旧边、不写新边，落库照常。
    #[tokio::test]
    async fn without_a_room_calculator_the_root_still_lands_and_writes_no_edges() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_roots(&db, &[ROOM_ROOT_A]).await;
        let store = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);
        let batched = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            None,
        )
        .await
        .expect("persist");
        assert!(batched.failed.is_empty());
        assert_eq!(batched.room_edges, 0);
        assert!(room_edges(&db).await.is_empty());
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 1);
    }

    /// 关闭房间归属只撤掉计算器，不得把模型根持久化也一并关掉。
    #[test]
    fn disabling_room_membership_freezes_no_calculator_without_error() {
        let _off = crate::options::RoomMembershipOverride::set(false);
        assert!(matches!(frozen_room_calculator(), Ok(None)));
    }

    /// 回归：关闭房间时模型根与 model_sesno 照常到目标版，只缺房间边；重新打开后历史
    /// 欠账不会靠模型水位自动出现，显式模型重建会为同一根补上房间边。
    ///
    /// 这里用 mem 库走生产的房间开关、逐根持久化和 TaskEnd 收尾三段；重开后的第二次
    /// `persist_roots_batched` 等价于运维端显式 `model/rebuild` 排出的根重试。
    #[tokio::test]
    async fn room_off_keeps_model_settlement_and_an_explicit_rebuild_after_reenable_fills_rooms() {
        async fn model_sesno_of(db: &Surreal<Any>, dbnum: u32) -> i32 {
            let mut response = db
                .query(format!(
                    "SELECT VALUE model_sesno FROM dbnum_watermark:{dbnum};"
                ))
                .await
                .expect("read model watermark")
                .check()
                .expect("model watermark statement");
            response
                .take::<Vec<i32>>(0)
                .expect("model watermark rows")
                .into_iter()
                .next()
                .unwrap_or_default()
        }

        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_roots(&db, &[ROOM_ROOT_A]).await;
        db.query(format!(
            "UPSERT dbnum_watermark:{BATCH_DBNUM} SET dbnum={BATCH_DBNUM}, \
             applied_sesno=42, model_sesno=41;"
        ))
        .await
        .expect("seed watermarks")
        .check()
        .expect("seed watermark statement");
        let store = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);

        let disabled_rooms = {
            let _off = crate::options::RoomMembershipOverride::set(false);
            frozen_room_calculator().expect("disabled membership is not a model error")
        };
        assert!(disabled_rooms.is_none());
        let without_rooms = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            disabled_rooms.as_ref(),
        )
        .await
        .expect("persist model while room membership is disabled");
        assert!(
            without_rooms.failed.is_empty(),
            "{:?}",
            without_rooms.failed
        );
        assert_eq!(without_rooms.room_edges, 0);

        let model_tail = crate::data_interface::task_end::commit_task_end_on(
            &db,
            &crate::data_interface::task_end::TaskEnd {
                dbnum: BATCH_DBNUM,
                target_sesno: 42,
                target_time: None,
                model: Some(crate::data_interface::task_end::TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                }),
                info: None,
                closes_task: true,
            },
        )
        .await
        .expect("close model task while rooms are disabled");
        assert_eq!(model_tail.model_sesno, Some(42));
        assert_eq!(model_sesno_of(&db, BATCH_DBNUM).await, 42);
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 1);
        assert!(room_edges(&db).await.is_empty());
        assert_eq!(publication_status_of(&db, ROOM_ROOT_A).await, "ready");

        // 重新打开只启用房间计算，不制造模型水位缺口；运维显式模型重建后才补历史边。
        {
            let _on = crate::options::RoomMembershipOverride::set(true);
            assert!(crate::options::room_membership());
        }
        let retried = persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&rooms),
        )
        .await
        .expect("explicitly rebuild root after re-enable");
        assert!(retried.failed.is_empty(), "{:?}", retried.failed);
        assert_eq!(retried.room_edges, 1);
        assert_eq!(model_sesno_of(&db, BATCH_DBNUM).await, 42);
        assert_eq!(count_record(&db, "inst_relate", "24383_10").await, 1);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()]
        );
        assert_eq!(publication_status_of(&db, ROOM_ROOT_A).await, "ready");
    }

    /// 源码钉（ADR-073 D1 / D2 / W2，d-932）：房间归属在写库**之前**、按根算（[`room_stage`]，输入只有
    /// 该根的内存投影与任务开工时冻住的计算器），算不出来的根在进组事务之前就剔掉；每根的边紧跟它
    /// 自己的几何段（`live.room_statements`），段按字节装块、段不拆、块进事务——所以「模型行已提交、
    /// 房间边稍后单独提交」这条路在源码里不存在。落库路径不回读库、不走整库房间算法
    /// （`SUL_DB` / `query_room_insts` / `cal_room_refnos`）；两个生产调用方都在任务开工时冻一次
    /// 计算器递进来。
    #[test]
    fn room_membership_is_computed_per_root_before_the_write_and_rides_the_same_transaction() {
        let source = include_str!("model_db_adapter.rs");
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        // 调用方：房间关卡排在组事务之前，算不出来的根记失败、不进 `live`。
        let batched = production
            .split_once("pub(crate) async fn persist_roots_batched_with_chunk_bytes(")
            .expect("batched persist exists")
            .1
            .split_once("struct GroupOutcome")
            .expect("group outcome follows")
            .0;
        let stage_at = batched
            .find("room_stage(group, rooms)")
            .expect("写库前先过房间关卡");
        let failed_at = batched
            .find("out.push_failure(root, PersistFailureKind::Blocked, error)")
            .expect("算不出来的根记失败");
        let group_call_at = batched
            .find("match persist_group(")
            .expect("过了关卡的根才逐根进事务");
        assert!(
            stage_at < failed_at && failed_at < group_call_at,
            "顺序：逐根算房间 → 算不出来的根剔出组 → 组事务"
        );
        // 根内：几何段渲好 → 边拼入 → 回执并入 → 单笔提交。
        let group = production
            .split_once("async fn persist_group(")
            .expect("persist_group exists")
            .1
            .split_once("/// The WS router can retain a pending response until reconnection.")
            .expect("publication request helpers follow")
            .0;
        let render_at = group
            .find("render_chunk(&live.claimed, WriteMode::Incremental")
            .expect("几何段渲染");
        let attach_at = group
            .find("piece.push_str(&live.room_statements)")
            .expect("房间边拼进这一根");
        let receipt_at = group
            .find("root_statement.push_str(&receipt)")
            .expect("回执并进同一根事务");
        let commit_at = group
            .find("db.query(publication_transaction(&statements))")
            .expect("提交");
        assert!(
            render_at < attach_at && attach_at < receipt_at && receipt_at < commit_at,
            "顺序：渲几何 → 房间边 → 回执 → 单笔提交"
        );
        // 房间关卡 + 落库路径：一句库不读，不走整间分支的算法。
        let stage = production
            .split_once("fn room_stage<'a>(")
            .expect("room_stage exists")
            .1
            .split_once("pub async fn persist_roots_batched(")
            .expect("batched persist follows")
            .0;
        for needle in ["SUL_DB", "query_room_insts(", "cal_room_refnos("] {
            for (name, span) in [
                ("room_stage", stage),
                ("batched", batched),
                ("group", group),
            ] {
                assert!(
                    !span.contains(needle),
                    "{name}: 落库路径不回读库、不走整库房间算法: {needle}"
                );
            }
        }
        for (file, source) in [
            (
                "model_refresh.rs",
                include_str!("../data_interface/model_refresh.rs"),
            ),
            (
                "batch_worker.rs",
                include_str!("../data_interface/batch_worker.rs"),
            ),
        ] {
            let production = source
                .split(concat!("#[cfg(test)]\n", "mod tests"))
                .next()
                .expect("production half");
            assert!(
                production.contains("frozen_room_calculator()"),
                "{file}: 生产调用方要在任务开工时冻一次房间计算器"
            );
        }
    }

    /// 按根对账的两边必须**同一个口径**：库那边是 `anc CONTAINS <根>`，存储那边是
    /// `records_of_root`（同一张祖先索引）。嵌套生成根 D 的行既挂在 D 下也挂在祖先根 A 下，
    /// 所以两边只要有一边收窄成「只算直属这一根的」，另一边就会把对方名下的行当孤儿删掉。
    ///
    /// 这条按两个方向各落一次：落 A 不许删 D 名下的行，落 D 也不许删 A 名下别处的行；
    /// 真孤儿（库里有、投影里没有）照删。
    ///
    /// 今天不咬人靠的是两个互不相干的事实（祖先链是全库的、生成是整棵子树的，见
    /// `docs/specs/core3d-partial-update-conformance.md` §8），任一条被「优化」掉就开始
    /// 静默删行——那时红的就是这一条。
    #[tokio::test]
    async fn reconciling_an_ancestor_root_does_not_delete_a_nested_roots_rows() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let ancestor_root = RefNo::new(24383, 1);

        let orphan = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            ..baked_record()
        };
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), nested_root_record(), orphan.clone()],
                receipt: Some(test_receipt(ancestor_root, 3)),
                ..ProjectionCommit::default()
            }],
        );
        let target = PersistTarget::new(&db, PersistMedium::Mem);
        persist_root_on(target, &store, ancestor_root)
            .await
            .expect("persist ancestor");
        assert_eq!(count(&db, "inst_relate").await, 3);

        // 只有真孤儿被对账掉：祖先根这一轮的投影里没有它了。
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                removals: vec![orphan.record_id.clone()],
                receipt: Some(test_receipt(ancestor_root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let rows = persist_root_on(target, &store, ancestor_root)
            .await
            .expect("persist ancestor");
        assert_eq!(rows.removed, 1, "只有真孤儿进 removals");
        assert_eq!(count(&db, "inst_relate").await, 2, "嵌套根名下的行还在");

        // 反过来落嵌套根：它的投影只覆盖自己名下那一件，别处的行一样不许动。
        let rows = persist_root_on(target, &store, NESTED_ROOT)
            .await
            .expect("persist nested");
        assert_eq!(rows.upserted, 1);
        assert_eq!(rows.removed, 0);
        assert_eq!(count(&db, "inst_relate").await, 2);
    }

    /// R22/R23 testbed gate: exercise reconciliation through the websocket
    /// client against the dedicated RocksDB server, inside a disposable DB.
    #[tokio::test]
    #[ignore = "manual testbed: writes only a disposable DB on ws://127.0.0.1:8019"]
    async fn live_testbed_complete_projection_reclaims_only_the_absent_primitive()
    -> anyhow::Result<()> {
        use surrealdb::opt::auth::Root;

        let endpoint =
            std::env::var("AIOS_TESTBED_WS").unwrap_or_else(|_| "ws://127.0.0.1:8019".to_string());
        let db = surrealdb::engine::any::connect(endpoint).await?;
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await?;
        let database = format!("r22_{}_{}", std::process::id(), rand::random::<u32>());
        db.use_ns("core3d_conformance").use_db(&database).await?;

        let result: anyhow::Result<()> = async {
            seed_pe(&db).await;
            let root = RefNo::new(24383, 1);
            let mut orphan = canonical_record();
            orphan.record_id = "24383_9999".into();
            orphan.geometry_id = GeometryId::Element {
                refno: "24383/9999".into(),
            };
            orphan.source_refno = RefNo::new(24383, 9999);
            orphan.ancestors = vec![packed_refno(orphan.source_refno), packed_refno(root)];
            let retained = vec![baked_record(), nested_root_record()];

            let full = ModelMemoryStore::default();
            full.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: retained
                        .iter()
                        .cloned()
                        .chain(std::iter::once(orphan.clone()))
                        .collect(),
                    receipt: Some(test_receipt(root, 3)),
                    ..ProjectionCommit::default()
                }],
            );
            let target = PersistTarget::new(&db, PersistMedium::RocksDb);
            persist_root_on(target, &full, root).await?;
            anyhow::ensure!(
                count(&db, "inst_info").await == 1,
                "orphan fixture was not published"
            );

            let partial = ModelMemoryStore::default();
            partial.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: retained.clone(),
                    ..ProjectionCommit::default()
                }],
            );
            let unreceipted = persist_root_on(target, &partial, root).await?;
            anyhow::ensure!(
                unreceipted.removed == 0,
                "unreceipted projection deleted rows"
            );
            anyhow::ensure!(
                count(&db, "inst_info").await == 1,
                "R23 guard did not preserve the absent primitive"
            );

            let complete = ModelMemoryStore::default();
            complete.commit(
                ProjectionScope::Current,
                vec![ProjectionCommit {
                    upserts: retained,
                    receipt: Some(test_receipt(root, 2)),
                    ..ProjectionCommit::default()
                }],
            );
            let reconciled = persist_root_on(target, &complete, root).await?;
            anyhow::ensure!(
                reconciled.removed == 1,
                "expected exactly one reclaimed primitive"
            );
            for table in ["inst_info", "inst_relate", "aabb", "trans", "inst_geo"] {
                anyhow::ensure!(
                    count_record(&db, table, "24383_9999").await == 0,
                    "deleted primitive record survived in {table}"
                );
            }
            let mut response = db
                .query(
                    "RETURN array::len((SELECT VALUE id FROM geo_relate \
                     WHERE in=type::thing('inst_info','24383_9999')));",
                )
                .await?
                .check()?;
            let orphan_edges = response.take::<Option<i64>>(0)?.unwrap_or_default();
            anyhow::ensure!(orphan_edges == 0, "orphan geometry edge survived");
            anyhow::ensure!(
                count(&db, "inst_relate").await == 2,
                "retained rows changed"
            );
            Ok(())
        }
        .await;

        let cleanup = db
            .query(format!("REMOVE DATABASE `{database}`;"))
            .await
            .and_then(|response| response.check())
            .map(|_| ());
        result.and(cleanup.map_err(Into::into))
    }

    async fn seed_gen_root(db: &Surreal<Any>) {
        db.query(
            "CREATE type::thing('gen_root','24383_1') SET dbnum=7997, status='Generated', \
             publication_status='stale', desired_revision=2, published_revision=1;",
        )
        .await
        .expect("seed gen_root")
        .check()
        .expect("seed gen_root statements");
    }

    async fn publication_status(db: &Surreal<Any>) -> String {
        let mut response = db
            .query("SELECT VALUE publication_status FROM type::thing('gen_root','24383_1');")
            .await
            .expect("publication status")
            .check()
            .expect("publication status statement");
        response
            .take::<Vec<String>>(0)
            .expect("publication status rows")
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    async fn spatial_epoch(db: &Surreal<Any>) -> i64 {
        let mut response = db
            .query("RETURN spatial_epoch:current.value ?: 0;")
            .await
            .expect("spatial epoch")
            .check()
            .expect("spatial epoch statement");
        response
            .take::<Option<i64>>(0)
            .expect("spatial epoch row")
            .unwrap_or_default()
    }

    async fn seed_pe(db: &Surreal<Any>) {
        db.query(
            "CREATE pe:⟨24383_73948⟩; CREATE pe:⟨24383_500⟩; CREATE pe:⟨24383_600⟩; \
             CREATE pe:⟨24383_9999⟩; CREATE pe:⟨24383_10⟩; CREATE pe:⟨24383_20⟩;",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed statements");
    }

    #[tokio::test]
    async fn memory_model_publication_does_not_require_or_copy_attributes() {
        let db = surreal_mem().await;
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), other_canonical_record()],
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            RefNo::new(24383, 1),
        )
        .await
        .expect("publish model into an empty memory database");
        assert_eq!(count(&db, "inst_relate").await, 2);
        assert_eq!(count(&db, "pe").await, 0);
        assert_eq!(count(&db, "ATT_EQUI").await, 0);
        assert_eq!(count(&db, "ATT_UDA").await, 0);
        assert_eq!(count(&db, "pe_owner").await, 0);
    }

    async fn surreal_mem() -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("legacy_layout")
            .use_db("legacy_layout")
            .await
            .expect("namespace");
        db
    }

    async fn count(db: &Surreal<Any>, table: &str) -> i64 {
        let mut response = db
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {table}));"
            ))
            .await
            .expect("count")
            .check()
            .expect("count statement");
        response
            .take::<Option<i64>>(0)
            .expect("count row")
            .unwrap_or_default()
    }

    async fn count_record(db: &Surreal<Any>, table: &str, id: &str) -> i64 {
        let mut response = db
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {table} \
                 WHERE id=type::thing('{table}','{id}')));"
            ))
            .await
            .expect("count record")
            .check()
            .expect("count record statement");
        response
            .take::<Option<i64>>(0)
            .expect("count record row")
            .unwrap_or_default()
    }
}
