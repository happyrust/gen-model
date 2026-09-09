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
//! - **房间派生面**（[`crate::fast_model::room_publication`]）。被对账掉的来源两个方向的
//!   房间边跟着**删除那一块**走；拿到新几何的来源排一次房间重算，跟着**回执那一块**走。
//!   两笔不能对调：清边落在删除后面的块，中途失败就留下一批指着已删几何的悬空边，而下一轮
//!   对账再也看不见那些几何；排重算落在前面的块，任务一被消费就在照着只写了一半的几何算
//!   房间归属。全库落库（[`persist_dbnum`]）只清边不排重算。
//! - **空间版本号**。动过几何的每一块在同一条事务里把 `spatial_epoch` +1——那是直写路径
//!   在库侧留下的唯一痕迹，少这一下，崩溃重启会按 sidecar 复用一棵停在旧盒上的树。
//! - **进程内空间树**。提交之后按已提交指针收敛
//!   （[`crate::fast_model::aabb_tree::converge_tree_after_persist`]），只有写全局库的目标
//!   才做——树镜像的是那个库。

use std::collections::{BTreeMap, BTreeSet};

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
use crate::fast_model::room_publication::{
    RoomEffectPolicy, RoomPublicationEffects, render_room_publication_effects,
    room_publication_effects,
};

/// 落库介质。**换掉的只是介质**：两种模式下渲染出来的行逐字相同，本模块也不因此分叉——
/// 渲染那一半根本拿不到介质（看 [`render_records`] 的签名），所以它想分叉也分不出来。
/// 介质只进回执，让调用方说得出这一批落在哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistMedium {
    /// rocksdb。默认形态是 ws 连的那台外置 SurrealDB——rocksdb 在服务器那一侧；
    /// 不带 `ws` 的 `local` 构建走进程内嵌的 `rocksdb://<project>.rdb`。
    /// 两者的共同点、也是这一档唯一要紧的性质：**行活得过进程重启**。
    RocksDb,
    /// 本进程拉起的 memory 后端 SurrealDB（`store_mode = "spawned-mem"`）。
    /// 行随进程消失，**但端口是真的**——别的进程读得到这一批。
    SpawnedMem,
    /// 进程内嵌 kv-mem（`store_mode = "embedded-mem"`）。行落在进程里：
    /// 进程一退整库就没了，外面也没有端口可连。
    EmbeddedMem,
}

impl PersistMedium {
    /// 这一批行能不能活过进程重启。两档内存介质都活不过——回执里点名它，免得把
    /// 「落库了」当成「留下来了」。
    pub fn is_durable(self) -> bool {
        matches!(self, Self::RocksDb)
    }

    /// 别的进程读不读得到这一批。`embedded-mem` 是唯一的「读不到」——
    /// 它与 [`Self::is_durable`] 问的不是同一件事：`spawned-mem` 不 durable，但读得到。
    pub fn is_reachable(self) -> bool {
        !matches!(self, Self::EmbeddedMem)
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
                // 自带落盘档归 RocksDb：这枚枚举要紧的是「活得过重启」，不是「在不在外面」。
                crate::options::StoreMode::External | crate::options::StoreMode::SpawnedRocks => {
                    PersistMedium::RocksDb
                }
                crate::options::StoreMode::SpawnedMem => PersistMedium::SpawnedMem,
                crate::options::StoreMode::EmbeddedMem => PersistMedium::EmbeddedMem,
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
        let rows = persist_root_in_chunks(
            target,
            store,
            root,
            chunk_size,
            RoomEffectPolicy::FullDatabase,
        )
        .await?;
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
    persist_root_in_chunks(
        target,
        store,
        root,
        PERSIST_CHUNK,
        RoomEffectPolicy::Directed,
    )
    .await
}

pub(crate) async fn persist_root_for_file_round(
    root: RefNo,
    dbnum: u32,
    round_end_sesno: i32,
    source_token: &str,
) -> anyhow::Result<PersistedRows> {
    persist_root_for_file_round_on(
        PersistTarget::process(),
        ModelMemoryStore::global(),
        root,
        dbnum,
        round_end_sesno,
        source_token,
    )
    .await
}

async fn persist_root_for_file_round_on(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
    dbnum: u32,
    round_end_sesno: i32,
    source_token: &str,
) -> anyhow::Result<PersistedRows> {
    crate::data_interface::batch_worker::with_model_publication_window(
        dbnum,
        round_end_sesno,
        source_token,
        persist_root_in_chunks(
            target,
            store,
            root,
            PERSIST_CHUNK,
            RoomEffectPolicy::Directed,
        ),
    )
    .await
}

/// [`persist_root_on`] 的可调块大小形态。块大小只有测试才需要拨。
async fn persist_root_in_chunks(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
    chunk_size: usize,
    room_policy: RoomEffectPolicy,
) -> anyhow::Result<PersistedRows> {
    persist_root_chunks_impl(
        target,
        store,
        root,
        chunk_size,
        room_policy,
        None,
        None,
        None,
        false,
    )
    .await
}

/// Consume an immutable journal payload. This store is private to this call;
/// another producer can never change its records while SQL is being rendered.
pub async fn persist_frozen_on(
    target: PersistTarget<'_>,
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
) -> anyhow::Result<PersistedRows> {
    let plan = prepare_frozen_on(target, envelope).await?;
    persist_prepared_frozen_on(target, envelope, &plan).await
}

#[derive(Default)]
struct PreparedPublicationParts {
    chunks: Vec<String>,
    memory_removals: Vec<String>,
    memory_tube_branch_resets: Vec<String>,
}

pub async fn prepare_frozen_on(
    target: PersistTarget<'_>,
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
) -> anyhow::Result<crate::data_interface::model_publish_queue::PreparedRootPublication> {
    Ok(prepare_frozen_profiled_on(target, envelope).await?.0)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FrozenPrepareProfile {
    pub reconcile_queries: u64,
    pub reconcile_read_micros: u64,
    pub render_plan_micros: u64,
}

/// Prepare one immutable publication plan and expose timing without putting
/// diagnostic fields into the durable plan hash.
pub async fn prepare_frozen_profiled_on(
    target: PersistTarget<'_>,
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
) -> anyhow::Result<(
    crate::data_interface::model_publish_queue::PreparedRootPublication,
    FrozenPrepareProfile,
)> {
    prepare_frozen_profiled_for_baseline_on(target, envelope, false).await
}

/// Use a durable run-level proof to omit per-root reconciliation reads when
/// the projection was empty before the initialization run was reserved.
pub async fn prepare_frozen_profiled_for_baseline_on(
    target: PersistTarget<'_>,
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
    model_projection_empty_at_reservation: bool,
) -> anyhow::Result<(
    crate::data_interface::model_publish_queue::PreparedRootPublication,
    FrozenPrepareProfile,
)> {
    let prepare_started = std::time::Instant::now();
    envelope.validate()?;
    let root = crate::fast_model::e3d_model_service::parse_refno(&envelope.intent.root)?;
    let store = ModelMemoryStore::default();
    store.commit(ProjectionScope::Current, vec![envelope.projection.clone()]);
    let mut prepared = PreparedPublicationParts::default();
    let mut reconcile_read = std::time::Duration::ZERO;
    let result = crate::data_interface::batch_worker::with_model_publication_window(
        envelope.dbnum,
        envelope.source_sesno as i32,
        &envelope.source_token,
        persist_root_chunks_impl(
            target,
            &store,
            root,
            PERSIST_CHUNK,
            RoomEffectPolicy::FullDatabase,
            Some(envelope),
            Some(&mut prepared),
            Some(&mut reconcile_read),
            model_projection_empty_at_reservation,
        ),
    )
    .await?;
    anyhow::ensure!(
        result.receipts == 1,
        "frozen publication must have its exact root receipt"
    );
    anyhow::ensure!(
        prepared.chunks.iter().all(|s| s.len() <= 8 * 1024 * 1024),
        "frozen SQL chunk exceeds 8 MiB; retained for operator repair"
    );
    let plan = crate::data_interface::model_publish_queue::PreparedRootPublication {
        version: crate::data_interface::model_publish_queue::FORMAT_VERSION,
        envelope_hash: crate::data_interface::model_publish_queue::digest(&serde_json::to_vec(
            envelope,
        )?),
        chunks: prepared.chunks,
        memory_removals: prepared.memory_removals,
        memory_tube_branch_resets: prepared.memory_tube_branch_resets,
        upserted: result.upserted,
        removed: result.removed,
        receipts: result.receipts,
    };
    let total = prepare_started.elapsed();
    let profile = FrozenPrepareProfile {
        reconcile_queries: u64::from(!model_projection_empty_at_reservation),
        reconcile_read_micros: reconcile_read.as_micros() as u64,
        render_plan_micros: total.saturating_sub(reconcile_read).as_micros() as u64,
    };
    Ok((plan, profile))
}

pub async fn persist_prepared_frozen_on(
    target: PersistTarget<'_>,
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
    plan: &crate::data_interface::model_publish_queue::PreparedRootPublication,
) -> anyhow::Result<PersistedRows> {
    envelope.validate()?;
    anyhow::ensure!(
        plan.version == crate::data_interface::model_publish_queue::FORMAT_VERSION
            && plan.receipts == 1
            && !plan.chunks.is_empty(),
        "invalid frozen publication plan"
    );
    anyhow::ensure!(
        plan.envelope_hash
            == crate::data_interface::model_publish_queue::digest(&serde_json::to_vec(envelope)?),
        "prepared publication belongs to a different envelope"
    );
    let hash = plan.hash()?;
    for (index, statements) in plan.chunks.iter().enumerate() {
        let sql = render_frozen_plan_chunk(envelope, index, plan.chunks.len(), &hash, statements);
        let timeout = publication_timeout_for(1, sql.len());
        publication_request_with_timeout(
            async {
                target
                    .db
                    .query(sql)
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(check_publication_response)
            },
            timeout,
        )
        .await
        .with_context(|| {
            format!(
                "persist frozen root {} chunk {}/{}",
                envelope.intent.root,
                index + 1,
                plan.chunks.len()
            )
        })?;
    }
    commit_prepared_frozen_projection(envelope, plan)?;
    Ok(PersistedRows {
        medium: target.medium,
        upserted: plan.upserted,
        removed: plan.removed,
        receipts: plan.receipts,
        commits: plan.chunks.len(),
    })
}

/// Apply only the process-local half of an already committed publication.
/// Recovery and batched publication use the database ACK as authority, so they
/// must never issue the SQL effects a second time just to restore this view.
pub(crate) fn commit_prepared_frozen_projection(
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
    plan: &crate::data_interface::model_publish_queue::PreparedRootPublication,
) -> anyhow::Result<()> {
    let mut projection = envelope.projection.clone();
    projection
        .removals
        .extend(plan.memory_removals.iter().cloned());
    for branch in &plan.memory_tube_branch_resets {
        projection
            .tube_branch_resets
            .push(crate::fast_model::e3d_model_service::parse_refno(branch)?);
    }
    ModelMemoryStore::global().commit(ProjectionScope::Current, vec![projection]);
    Ok(())
}

/// Submit several one-chunk roots in one database request while retaining one
/// independent transaction and ACK per root. A failed transaction does not
/// roll back its neighbours; the caller reconciles every ACK before assigning
/// per-root terminal states.
pub(crate) async fn persist_prepared_frozen_batch_on(
    target: PersistTarget<'_>,
    publications: &[(
        &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
        &crate::data_interface::model_publish_queue::PreparedRootPublication,
    )],
) -> anyhow::Result<Vec<PersistedRows>> {
    anyhow::ensure!(!publications.is_empty(), "empty frozen publication batch");
    let mut sql = String::new();
    for (envelope, plan) in publications {
        envelope.validate()?;
        anyhow::ensure!(
            plan.version == crate::data_interface::model_publish_queue::FORMAT_VERSION
                && plan.receipts == 1
                && plan.chunks.len() == 1,
            "batched frozen publication requires exactly one valid chunk"
        );
        anyhow::ensure!(
            plan.envelope_hash
                == crate::data_interface::model_publish_queue::digest(&serde_json::to_vec(
                    envelope
                )?),
            "prepared publication belongs to a different envelope"
        );
        let hash = plan.hash()?;
        sql.push_str(&render_frozen_plan_chunk(
            envelope,
            0,
            1,
            &hash,
            &plan.chunks[0],
        ));
    }
    let timeout = publication_timeout_for(publications.len(), sql.len());
    let roots = publications.len();
    publication_request_with_timeout(
        async {
            target
                .db
                .query(sql)
                .await
                .map_err(anyhow::Error::from)
                .and_then(|response| check_batch_publication_response(response, roots))
        },
        timeout,
    )
    .await?;
    let mut outcomes = Vec::with_capacity(publications.len());
    for (envelope, plan) in publications {
        commit_prepared_frozen_projection(envelope, plan)?;
        outcomes.push(PersistedRows {
            medium: target.medium,
            upserted: plan.upserted,
            removed: plan.removed,
            receipts: plan.receipts,
            commits: 1,
        });
    }
    Ok(outcomes)
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
#[derive(Debug)]
pub struct ProjectionRestoreConflict {
    pub dbnum: u32,
    pub attempts: usize,
}

impl std::fmt::Display for ProjectionRestoreConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "model projection restore for dbnum {} lost the race with concurrent memory projection commits {} times; the database image was stale and was not swapped in",
            self.dbnum, self.attempts
        )
    }
}

impl std::error::Error for ProjectionRestoreConflict {}

/// 读库镜像撞上并发提交时原地重读几次。再撞就不占着收口这一轮了。
const RESTORE_ATTEMPTS: usize = 3;

/// 用库里已提交的行整体换掉某个库的进程内投影。
///
/// 读镜像要翻很多页、页与页之间全是 await，而「只写内存」的按需生成同时还在往同一个库
/// 提交（`model_read_route` R2）。所以取镜像**之前**先记下提交纪元，换进去时由
/// [`ModelMemoryStore::restore_dbnum`] 校验：纪元变了就重读，不拿旧镜像盖掉那些只在内存
/// 里的记录。
pub async fn restore_projection_on(
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
    room_policy: RoomEffectPolicy,
    frozen: Option<&crate::data_interface::model_publish_queue::FrozenRootEnvelope>,
    mut preparing: Option<&mut PreparedPublicationParts>,
    mut reconcile_timing: Option<&mut std::time::Duration>,
    skip_reconcile_query: bool,
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
    let projected_geometry_ids = if skip_reconcile_query || !reconciliation_ready {
        Vec::new()
    } else {
        let reconcile_started = std::time::Instant::now();
        let ids = projected_geometry_ids_on(db, root).await?;
        if let Some(elapsed) = reconcile_timing.as_mut() {
            **elapsed += reconcile_started.elapsed();
        }
        ids
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
    let effects = room_publication_effects(
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
    let room_incremental = crate::options::room_incremental();
    // 清边跟着删除走、排重算跟着回执走，两笔不能对调：
    //
    // - 清边排在**删几何那一块**。删了行却把清边留到后面的块，中途失败就留下一批指着
    //   已删几何的房间边，而下一轮对账再也看不见那些几何（行已经没了），悬空边永远
    //   不会被清——`fn::room_relate_of` 照样把它们取出来。
    // - 排重算排在**盖章那一块**。任务一进队列就可能被消费，排在前面等于让房间重算
    //   照着只写了一半的几何算归属；排在最后，中途失败就整个不排，下一轮重新走一遍。
    let clearing = RoomPublicationEffects {
        cleared: effects.cleared.clone(),
        recalc: Vec::new(),
    };
    let recalculation = RoomPublicationEffects {
        cleared: Vec::new(),
        recalc: effects.recalc.clone(),
    };
    let file_round = crate::data_interface::batch_worker::active_data_window();
    let deferred_refresh = records
        .iter()
        .filter(|record| record.tube.is_none())
        .map(|record| refno_from_e3d(record.source_refno))
        .collect::<BTreeSet<_>>()
        .into_iter()
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
    if let Some(prepared) = preparing.as_mut() {
        prepared.memory_removals = removals.clone();
        prepared.memory_tube_branch_resets =
            branches.iter().map(|(a, b)| format!("{a}/{b}")).collect();
    }
    // Keep geometry messages bounded by their rendered size, including the
    // structured restart projection. Record count alone cannot bound large roots.
    let mut chunks = Vec::new();
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    let mut pending = std::collections::VecDeque::from_iter(records.chunks(chunk_size.max(1)));
    while let Some(chunk) = pending.pop_front() {
        let mut slots = tube_slots.clone();
        let rendered = render_chunk(chunk, WriteMode::Incremental, &mut slots)?;
        if rendered.len() > 2 * 1024 * 1024 && chunk.len() > 1 {
            let (left, right) = chunk.split_at(chunk.len() / 2);
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
        if let Some(receipt) = receipt.as_ref() {
            statements.push_str(&render_publication_guard(root, receipt));
        }
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
            if file_round.is_none() || frozen.is_some() {
                statements.push_str(&render_room_publication_effects(
                    &clearing,
                    room_policy,
                    room_incremental,
                ));
            }
        }
        if let Some((_, rendered)) = chunks.get(index) {
            statements.push_str(rendered);
        }
        if index + 1 == chunk_count {
            statements.push_str(&render_room_publication_effects(
                &recalculation,
                room_policy,
                room_incremental,
            ));
            if let Some((dbnum, round_end_sesno)) = file_round {
                let refresh = deferred_refresh
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let removed = effects
                    .cleared
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                if let Some(envelope) = frozen {
                    statements.push_str(
                        &crate::data_interface::model_publish_queue::render_spatial_fragment(
                            envelope, &refresh, &removed,
                        )?,
                    );
                } else {
                    statements.push_str(&crate::data_interface::side_effect_pending::SideEffectCompensator::render_file_spatial_reconcile_upsert(
                        dbnum,
                        round_end_sesno,
                        &refresh,
                        &removed,
                        &removed,
                    )?);
                }
                statements.push('\n');
            }
            if let Some(receipt) = receipt.as_ref() {
                statements.push_str(&render_receipt_version(
                    root,
                    receipt,
                    frozen.map(|e| (e.intent.revision, e.dbnum)),
                )?);
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
        if let Some(envelope) = frozen {
            if index + 1 == chunk_count {
                statements.push_str(&render_frozen_tail(envelope));
            }
        }
        if let Some(prepared) = preparing.as_mut() {
            prepared.chunks.push(statements);
            continue;
        }
        let transaction = if let Some(envelope) = frozen {
            render_frozen_chunk(envelope, index, index + 1 == chunk_count, &statements)
        } else {
            publication_transaction(&statements)
        };
        db.query(transaction)
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

    // 行已经提交，进程内空间树还停在旧盒上——收敛在事务之外，因为它读的是**已提交**
    // 的指针（`inst_relate.aabb`），事务里读不到自己还没提交的那一半。
    if target.converge_spatial && commits > 0 && file_round.is_none() {
        crate::fast_model::aabb_tree::converge_tree_after_persist(
            &deferred_refresh,
            &effects.cleared,
        )
        .await
        .with_context(|| format!("落库后收敛空间树（根 {root}）"))?;
    }

    Ok(PersistedRows {
        medium: target.medium,
        upserted: records.len(),
        removed: removals.len(),
        receipts: usize::from(receipt.is_some()),
        commits,
    })
}

/// Each chunk's acknowledgement and effects share a transaction. Exact job
/// identity remains mandatory even when the design session did not change.
pub(crate) fn render_frozen_chunk(
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
    chunk: usize,
    last: bool,
    statements: &str,
) -> String {
    let effects = if last {
        format!("{statements}\n{}", render_frozen_tail(envelope))
    } else {
        statements.to_owned()
    };
    render_frozen_plan_chunk(
        envelope,
        chunk,
        chunk + if last { 1 } else { 2 },
        "test-plan",
        &effects,
    )
}

fn render_frozen_tail(
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
) -> String {
    format!(
        "DELETE model_update_pending:regen_root_{} WHERE revision={} AND init_run_id={} AND init_token={} RETURN NONE;\n",
        envelope.intent.root.replace('/', "_"),
        envelope.intent.revision,
        sql_string(&envelope.run_id),
        sql_string(&envelope.intent.token)
    )
}

fn render_frozen_plan_chunk(
    envelope: &crate::data_interface::model_publish_queue::FrozenRootEnvelope,
    chunk: usize,
    count: usize,
    plan_hash: &str,
    statements: &str,
) -> String {
    let last = chunk + 1 == count;
    let chunk_hash = crate::data_interface::model_publish_queue::digest(statements.as_bytes());
    let source_hash =
        crate::data_interface::model_publish_queue::digest(envelope.source_token.as_bytes());
    let dbnum = envelope.dbnum;
    let incarnation = sql_string(&envelope.database_incarnation);
    let root_id = sql_string(&refno_key(
        crate::fast_model::e3d_model_service::parse_refno(&envelope.intent.root)
            .expect("validated frozen root"),
    ));
    let pending = format!(
        "model_update_pending:regen_root_{}",
        envelope.intent.root.replace('/', "_")
    );
    let run = sql_string(&envelope.run_id);
    let token = sql_string(&envelope.intent.token);
    let hash = sql_string(&envelope.target_hash);
    let marker = format!("model_publish_ack:⟨{}_{}⟩", envelope.job_id, chunk);
    let revision = envelope.intent.revision;
    let session = envelope.source_sesno;
    publication_transaction(&format!(
        "LET $ack = (SELECT * FROM {marker} LIMIT 1)[0];\n\
         LET $watermark = (SELECT applied_sesno, applied_source_hash FROM dbnum_watermark:{dbnum})[0];\n\
         LET $applied = $watermark.applied_sesno;\n\
         IF (SELECT VALUE incarnation FROM model_publish_database:current)[0] != {incarnation} {{ THROW 'model_publish_database_mismatch'; }};\n\
         IF $watermark.applied_source_hash != '{source_hash}' {{ THROW 'model_publish_source_mismatch'; }};\n\
         LET $root = (SELECT desired_revision, init_target_hash FROM type::thing('gen_root',{root_id}) LIMIT 1)[0];\n\
         IF $applied != {session} {{ THROW 'model_publish_applied_barrier'; }};\n\
         IF $root.desired_revision != {revision} {{ THROW 'model_publish_superseded'; }};\n\
         IF $ack = NONE {{\n\
           LET $work = (SELECT revision, init_run_id, init_token, source_end_sesno, init_target_hash, dbnum, init_source_hash FROM {pending} LIMIT 1)[0];\n\
           IF $work = NONE OR $work.revision != {revision} OR $work.init_run_id != {run} \
               OR $work.init_token != {token} OR $work.source_end_sesno != {session} \
               OR $work.init_target_hash != {hash} OR $work.dbnum != {} \
               OR $work.init_source_hash != '{source_hash}' \
              OR ($root.init_target_hash IS NOT NONE AND $root.init_target_hash != {hash}) \
              {{ THROW 'model_publish_superseded'; }};\n\
           {statements}\n\
            CREATE {marker} SET job_id='{}', run_id={run}, target_hash={hash}, revision={revision}, source_sesno={session}, final={last}, \
              token={token}, dbnum={dbnum}, database_incarnation={incarnation}, source_hash='{source_hash}', plan_hash='{plan_hash}', chunk_hash='{chunk_hash}', chunk_count={count}, renderer_version=2;\n\
         }} ELSE {{\n\
           IF $ack.target_hash != {hash} OR $ack.revision != {revision} OR $ack.run_id != {run} \
              OR $ack.source_sesno != {session} OR $ack.final != {last} OR $ack.token != {token} OR $ack.dbnum != {dbnum} \
              OR $ack.database_incarnation != {incarnation} OR $ack.source_hash != '{source_hash}' OR $ack.plan_hash != '{plan_hash}' OR $ack.chunk_hash != '{chunk_hash}' \
              OR $ack.chunk_count != {count} OR $ack.renderer_version != 2 \
              {{ THROW 'model_publish_ack_identity_mismatch'; }};\n\
          }};\n",
        envelope.dbnum, envelope.job_id
    ))
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
    let mut statements = render_publication_guard(root, &receipt);
    statements.push_str(&render_spooled_scope_cleanup(root));
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
    let (revision, published_target) = frozen
        .map(|(revision, dbnum)| {
            (
                revision.to_string(),
                format!(
                    "{{dbnum:{dbnum},source_end_sesno:{}}}",
                    receipt.source_sesno
                ),
            )
        })
        .unwrap_or_else(|| ("desired_revision".into(), "desired_target".into()));
    let root_id = sql_string(&refno_key(root));
    let model_target = receipt
        .model_target
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?
        .map(|value| sql_value(&value))
        .unwrap_or_else(|| "NONE".to_string());
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
    statements.push_str(&format!(
        "UPDATE type::thing('gen_root', {root_id}) SET \
         status='Generated', publication_status='ready', \
         published_revision={revision}, published_target={published_target}, \
         desired_model_target={model_target}, published_model_target={model_target}, \
         published_manifest_hash={}, published_geometry_count={}, \
         source_end_sesno={}, published_via={}, last_error=NONE, updated_at=time::now();\n",
        sql_string(&receipt.manifest_hash),
        receipt.geometry_count,
        receipt.source_sesno,
        sql_string(&receipt.published_via),
    ));
    append_root_dependency_receipt(&mut statements, root, receipt.model_target.as_ref());
    Ok(statements)
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

/// 旧发布事务的 revision CAS 没有跟过来（回执里没有 revision），但「同一根不许被两个源
/// 会话混着发布」这条还在：判据从凭证行上的 `desired_target.source_end_sesno` 读，
/// 与事务里的写在同一条边界上。
fn render_publication_guard(root: RefNo, receipt: &RootReceipt) -> String {
    let root_id = sql_string(&refno_key(root));
    let source_sesno = receipt.source_sesno;
    format!(
        "LET $publish_root = (SELECT * FROM type::thing('gen_root', {root_id}) LIMIT 1)[0];\n\
         IF $publish_root != NONE AND ($publish_root.desired_target.source_end_sesno?:0) > 0 AND ($publish_root.desired_target.source_end_sesno?:0) != {source_sesno} {{ THROW 'mixed model target publication'; }};\n"
    )
}

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

    fn valid_frozen_envelope() -> crate::data_interface::model_publish_queue::FrozenRootEnvelope {
        let model_target = crate::fast_model::e3d_model_service::ModelTarget {
            project: "test".into(),
            design: crate::fast_model::e3d_model_service::ModelFileTarget {
                dbnum: 7997,
                db_type: "DESI".into(),
                file: "frozen".into(),
                session: 26,
                digest: "digest".into(),
            },
            catalogue: vec![],
            template_attlib_digest: "template".into(),
            generator_fingerprint: "generator".into(),
            tessellation_profile: "profile".into(),
        };
        let target_hash = crate::data_interface::model_publish_queue::digest(
            &serde_json::to_vec(&Some(model_target.clone())).unwrap(),
        );
        let mut record = baked_record();
        record.source_sesno = 26;
        crate::data_interface::model_publish_queue::FrozenRootEnvelope {
            version: crate::data_interface::model_publish_queue::FORMAT_VERSION,
            job_id: "profile-job".into(),
            run_id: "profile-run".into(),
            dbnum: 7997,
            source_sesno: 26,
            source_token: "frozen-source".into(),
            database_incarnation: "database-1".into(),
            intent: crate::data_interface::model_publish_queue::RootIntent {
                root: BRANCH.to_string(),
                revision: 2,
                token: "root-token".into(),
            },
            target_hash,
            projection: ProjectionCommit {
                upserts: vec![record],
                receipt: Some((
                    BRANCH,
                    RootReceipt {
                        source_sesno: 26,
                        model_target: Some(model_target),
                        manifest_hash: "manifest".into(),
                        geometry_count: 1,
                        published_via: "regen".into(),
                    },
                )),
                ..ProjectionCommit::default()
            },
            created_ms: 0,
        }
    }

    #[tokio::test]
    async fn empty_baseline_proof_skips_the_query_without_changing_the_plan() {
        let envelope = valid_frozen_envelope();
        let queried_db = surreal_mem().await;
        let skipped_db = surreal_mem().await;
        let (queried, queried_profile) = prepare_frozen_profiled_for_baseline_on(
            PersistTarget::new(&queried_db, PersistMedium::EmbeddedMem),
            &envelope,
            false,
        )
        .await
        .unwrap();
        let (skipped, skipped_profile) = prepare_frozen_profiled_for_baseline_on(
            PersistTarget::new(&skipped_db, PersistMedium::EmbeddedMem),
            &envelope,
            true,
        )
        .await
        .unwrap();
        assert_eq!(queried.hash().unwrap(), skipped.hash().unwrap());
        let sql = skipped.chunks.join("\n");
        assert!(
            !sql.contains("room_recalc_element") && !sql.contains("room_recalc_panel"),
            "初始化发布由项目级全量房间计算收尾，不应制造逐构件待办"
        );
        assert_eq!(queried_profile.reconcile_queries, 1);
        assert_eq!(skipped_profile.reconcile_queries, 0);
        assert_eq!(skipped_profile.reconcile_read_micros, 0);
    }

    /// 回执盖的就是窗口规划器认的那几列。revision 不在回执里，所以
    /// `published_revision` 直接跟着行上的 `desired_revision` 走。
    #[test]
    fn a_receipt_settles_the_root_credential() {
        let sql = render_receipt(RefNo::new(24384, 1), &receipt()).expect("receipt");
        assert!(sql.contains("UPDATE type::thing('gen_root', '24384_1')"));
        assert!(sql.contains("publication_status='ready'"));
        assert!(sql.contains("published_revision=desired_revision"));
        assert!(sql.contains("published_manifest_hash='abc123'"));
        assert!(sql.contains("published_geometry_count=2"));
        assert!(sql.contains("source_end_sesno=26"));
        assert!(!sql.contains("root_dependency"));

        let guard = render_publication_guard(RefNo::new(24384, 1), &receipt());
        assert!(guard.contains("THROW 'mixed model target publication'"));
        assert!(guard.contains("!= 26"));
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
        let first = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(first.upserted, 3);
        assert_eq!(first.removed, 0);
        assert_eq!(first.commits, 1, "三条记录装得下一块，就只发一笔");
        assert_eq!(first.medium, PersistMedium::EmbeddedMem);
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);

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

        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
        let rows = persist_root_in_chunks(target, &store, root, 1, RoomEffectPolicy::Directed)
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

        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
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

        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);

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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
        assert!(
            persist_root_in_chunks(target, &store, root, 1, RoomEffectPolicy::Directed)
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
        persist_root_in_chunks(target, &store, root, 1, RoomEffectPolicy::Directed)
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
        persist_root_in_chunks(target, &store, root, 1, RoomEffectPolicy::Directed)
            .await
            .expect("persist");
        assert_eq!(publication_status(&db).await, "stale");
    }

    /// 房间派生面与空间版本号回到发布事务上（ADR-060 P3）：被对账掉的来源两个方向的
    /// 房间边一并清掉，拿到新几何的来源排一次房间重算，动过几何的每一块顺手把
    /// `spatial_epoch` +1——少这一下，崩溃重启会按 sidecar 复用一棵停在旧盒上的树。
    #[tokio::test]
    async fn room_edges_and_the_spatial_epoch_ride_the_publication_transaction() {
        let _room = crate::options::RoomIncrementalOverride::set(true);
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
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
        // 第一轮两条几何各排过一次重算；清空之后再看第二轮自己排了谁。
        db.query(format!(
            "DELETE {};",
            crate::data_interface::model_update_pending::TABLE
        ))
        .await
        .expect("clear queue")
        .check()
        .expect("clear queue statement");

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

        let mut response = db
            .query(format!(
                "SELECT VALUE target_refno FROM {} WHERE action='room_recalc_element';",
                crate::data_interface::model_update_pending::TABLE
            ))
            .await
            .expect("recalc")
            .check()
            .expect("recalc statement");
        assert_eq!(
            response.take::<Vec<String>>(0).expect("recalc rows"),
            vec!["24383/73948".to_string()],
            "留下来的那条几何要排一次房间重算"
        );
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
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
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

    #[tokio::test]
    async fn directed_persist_for_file_round_tags_room_work_as_collecting() {
        let _room = crate::options::RoomIncrementalOverride::set(true);
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_root(&db).await;
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record()],
                receipt: Some(test_receipt(BRANCH, 1)),
                ..Default::default()
            }],
        );

        let source_token = crate::data_interface::frozen_source::FrozenSource::synthetic(
            "fixture.db",
            7997,
            "DESI",
            42,
        )
        .commit_token_json()
        .unwrap();
        persist_root_for_file_round_on(
            PersistTarget::new(&db, PersistMedium::EmbeddedMem),
            &store,
            BRANCH,
            7997,
            42,
            &source_token,
        )
        .await
        .unwrap();

        #[derive(serde::Deserialize)]
        struct RoomRow {
            status: String,
            #[serde(default)]
            file_rounds: Vec<String>,
        }
        let mut response = db
            .query(
                "SELECT status, file_rounds FROM model_update_pending:room_recalc_element_24383_73948;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let row = response
            .take::<Option<RoomRow>>(0)
            .unwrap()
            .expect("directed room row");
        assert_eq!(row.status, "collecting");
        assert_eq!(row.file_rounds, ["7997:42"]);
    }

    #[tokio::test]
    async fn frozen_file_round_clears_room_edges_in_first_chunk_before_later_failure() {
        let _room = crate::options::RoomIncrementalOverride::set(true);
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_root(&db).await;
        let root = RefNo::new(24383, 1);
        let stale = ProjectedGeometry {
            record_id: "24383_9999".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/9999".into(),
            },
            source_refno: RefNo::new(24383, 9999),
            ..baked_record()
        };
        db.query(publication_transaction(
            &render_records(&[stale], WriteMode::Incremental).unwrap(),
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query("CREATE room:one; RELATE pe:24383_9999->room_relate->room:one; RELATE pe:24383_9999->room_panel_relate->room:one;").await.unwrap().check().unwrap();
        let store = ModelMemoryStore::default();
        let mut first = baked_record();
        first.source_sesno = 26;
        let mut second = other_canonical_record();
        second.source_sesno = 26;
        let projection = ProjectionCommit {
            upserts: vec![first, second],
            receipt: Some((root, receipt())),
            ..Default::default()
        };
        store.commit(ProjectionScope::Current, vec![projection.clone()]);
        let envelope = crate::data_interface::model_publish_queue::FrozenRootEnvelope {
            version: crate::data_interface::model_publish_queue::FORMAT_VERSION,
            job_id: "test-job".into(),
            run_id: "test-run".into(),
            dbnum: 7997,
            source_sesno: 26,
            source_token: "frozen".into(),
            database_incarnation: "database".into(),
            intent: crate::data_interface::model_publish_queue::RootIntent {
                root: root.to_string(),
                revision: 2,
                token: "token".into(),
            },
            target_hash: "target".into(),
            created_ms: 0,
            projection,
        };
        let target = PersistTarget::new(&db, PersistMedium::EmbeddedMem);
        let mut prepared = PreparedPublicationParts::default();
        crate::data_interface::batch_worker::with_model_publication_window(
            7997,
            26,
            "frozen",
            persist_root_chunks_impl(
                target,
                &store,
                root,
                1,
                RoomEffectPolicy::Directed,
                Some(&envelope),
                Some(&mut prepared),
                None,
                false,
            ),
        )
        .await
        .unwrap();
        assert_eq!(prepared.chunks.len(), 2);
        let before = spatial_epoch(&db).await;
        db.query(publication_transaction(&prepared.chunks[0]))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(count(&db, "room_relate").await, 0);
        assert_eq!(count(&db, "room_panel_relate").await, 0);
        assert!(spatial_epoch(&db).await > before);
        assert_eq!(publication_status(&db).await, "publishing");
        // Failure of the receipt block must retain first-block cleanup and fence.
        let second = format!(
            "THROW 'injected later block failure'; {}",
            prepared.chunks[1]
        );
        assert!(
            db.query(publication_transaction(&second))
                .await
                .unwrap()
                .check()
                .is_err()
        );
        assert_eq!(publication_status(&db).await, "publishing");
        assert_eq!(count(&db, "room_relate").await, 0);
        assert_eq!(count(&db, "model_publish_fragment").await, 0);
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
            PersistTarget::new(&db, PersistMedium::EmbeddedMem),
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
