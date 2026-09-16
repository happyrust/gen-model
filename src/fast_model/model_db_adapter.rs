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
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_io::refno::RefNo;
use e3d_model::elmodl::GeometryId;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::sql::{
    Array as SurArray, Id as SurId, Number as SurNumber, Object as SurObject, Thing as SurThing,
    Value as SurValue,
};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::generation_root::refno_from_e3d;
use crate::fast_model::e3d_mesh_store::geometry_record_id;
use crate::fast_model::e3d_model_service::{
    ModelTarget, f32_values, geometry_source_refno, transform_value,
};
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
///
/// `slice_sources` 是这一片**删之前**库里有过的来源（[`slice_sources_on`]）。整片删掉之后
/// 它们的边一律清，本次投影里还在的那些由 `upserts` 挑回来（重算先清后写，用不着这里再清
/// 一遍）。差集正好是「这一轮不再有几何的来源」。
fn room_edges_to_clear<'a>(
    upserts: impl IntoIterator<Item = (&'a GeometryId, RefNo, &'a str)>,
    slice_sources: &[RefnoEnum],
    legacy_sources: &BTreeSet<RefnoEnum>,
) -> Vec<RefnoEnum> {
    let mut kept: BTreeSet<RefnoEnum> = BTreeSet::new();
    for (geometry_id, refno, _noun) in upserts {
        if matches!(geometry_id, GeometryId::Element { .. }) {
            kept.insert(refno_from_e3d(refno));
        }
    }
    let mut cleared: BTreeSet<RefnoEnum> = legacy_sources.clone();
    cleared.extend(slice_sources.iter().cloned());
    cleared.retain(|refno| !kept.contains(refno));
    cleared.into_iter().collect()
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

/// 一次落库落了多少行、落在哪。行数口径与旧 `E3dPersistReport` 的 `upserted` 相同：
/// 数的是几何记录，不是语句条数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedRows {
    pub medium: PersistMedium,
    pub upserted: usize,
    /// **这一轮掉了几何的来源数**，不是删掉的行数。整片删走谓词，库不回报删了几行
    /// （[`render_slice_delete`]）；能数的只有「整片删之前有、这一轮投影里没有」的那些来源
    /// ——房间边与空间树条目也正是按它们摘的。
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

/// 把一个库当前投影里的每一根都对齐进库，一根一笔事务。
///
/// 整库生成之后用它收尾：生成侧只写进程内投影，这一步才让 Plant UI 看得见。逐根落而不是
/// 一整批落，是为了让一根的失败只赔掉那一根。
///
/// **持久介质上别直接调它。** 那一档 durable 行只过 write-behind 那道缝（ADR-071），后台
/// worker 一拿到根就开始刷；这时再调本函数就是第二个写者，跟它抢同一批行，热库上实测会撞
/// `read or write conflict` 把整笔推掉。要等行落下来用
/// [`wait_until_model_rows_are_durable`]，它自己会挑对那一档该走哪边。
pub async fn persist_dbnum(dbnum: u32) -> anyhow::Result<PersistedRows> {
    let target = PersistTarget::process();
    let store = ModelMemoryStore::global();
    let mut total = PersistedRows::none(target.medium);
    for root in store.roots(ProjectionScope::Current) {
        if root.dbno() != dbnum {
            continue;
        }
        // 全库生成只清房间边、不逐元素排重算（ADR-010 §4 收窄 1：整库以启动全量房间
        // 重建收尾，给每个元素排一次重算等于把那件事做两遍）。
        let rows = persist_root_on(target, store, root).await?;
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

/// [`wait_until_model_rows_are_durable`] 的结局。
#[derive(Debug, Clone, Copy)]
pub struct DurableFlush {
    pub medium: PersistMedium,
    /// 持久介质：开始等的时候还有几根没落。内存介质：本函数自己落了几根。
    pub roots: usize,
    pub waited: Duration,
}

/// 一次性工具的收尾：等这个库的模型行**真的**落进库。
///
/// 生产任务不用它——任务在收尾前用 [`model_write_behind::await_roots_durable`] 等**自己那批根**
/// （ADR-076 D2），这里等的是该库此刻压在队列里的**全部**根：一次性命令（烟测、运维口）跑完就退，
/// 进程一没后台 worker 跟着没，行只落了一半。
///
/// **持久介质上它一行都不写**，只是等那道缝把已经交出去的根刷完。别改成在这里调
/// [`persist_dbnum`]：那是第二个写者，跟后台 worker 抢同一批行，热库上实测会撞
/// `read or write conflict` 把整笔推掉。
///
/// 两档内存介质走的是另一半：`publish` 在非持久 store 上只提交内存投影、根本不登记
/// write-behind（那里没有「后来会落」这回事），所以这一档由本函数补一次 [`persist_dbnum`]
/// ——此时没有第二个写者，抢不起来。
///
/// 被判死（`Blocked`）的根不会自己好；等到没有一根还能往前走才收工，报出来的是完整名单。
/// 重试次数不设上限——工具等的是「落完」，期限只有 `timeout` 一道。
///
/// [`model_write_behind::await_roots_durable`]: crate::fast_model::model_write_behind::await_roots_durable
pub async fn wait_until_model_rows_are_durable(
    dbnum: u32,
    timeout: Duration,
) -> anyhow::Result<DurableFlush> {
    use crate::fast_model::model_write_behind::{FlushBudget, await_roots_durable, pending_roots};

    let started = Instant::now();
    let medium = PersistTarget::process().medium;
    if !crate::options::store_mode().is_durable() {
        let rows = persist_dbnum(dbnum).await?;
        return Ok(DurableFlush {
            medium,
            roots: rows.commits,
            waited: started.elapsed(),
        });
    }
    let roots = pending_roots(dbnum);
    let outcome = await_roots_durable(
        dbnum,
        &roots,
        FlushBudget {
            max_attempts_per_root: u32::MAX,
            deadline: timeout,
        },
    )
    .await;
    if !outcome.blocked.is_empty() {
        anyhow::bail!(
            "dbnum {dbnum} 有 {} 根被 write-behind 判死：{}",
            outcome.blocked.len(),
            outcome
                .blocked
                .iter()
                .map(|(root, error)| format!("{root} {error}"))
                .collect::<Vec<_>>()
                .join("；")
        );
    }
    anyhow::ensure!(
        outcome.exhausted.is_empty(),
        "等 dbnum {dbnum} 的模型行落库超过 {timeout:?}，还剩 {} 根",
        outcome.exhausted.len()
    );
    Ok(DurableFlush {
        medium,
        roots: roots.len(),
        waited: started.elapsed(),
    })
}

/// [`persist_root`] 的可注入形态：落库目标与存储都由调用方给。一根一笔事务。
pub async fn persist_root_on(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
) -> anyhow::Result<PersistedRows> {
    persist_root_impl(target, store, root).await
}

/// 一组最多装这么多根（[`persist_roots_batched`]）：ADR-071 D2 的跨根合块（d-932 保留），
/// 7997 六千多根不再是六千多笔事务。组的大小只是这一个常量：组内按 `tx_limits` 的字节上限合块、
/// 一组一批事务；每根的房间边紧跟它自己的模型行、同一块同一事务（ADR-073 D1 / W2），一根一组
/// 还是几百根一组都是同一条路。房间算不出来的根在写库**之前**就剔出组、单独记失败（ADR-073 D2）；
/// 写库失败的赔付边界是根：同组其它根继续独立提交。
pub const BATCHED_PERSIST_ROOTS_PER_GROUP: usize = 512;

/// 一笔发布事务最多装这么多条几何记录。
///
/// **按条数装块，不按字节。** 字节量随几何复杂度浮动，同一个数字在不同库上差一个量级，
/// 调的人没有参照；条数是调用方数得出来的那个量。一根装不下就发好几笔——2026-09-14 之前
/// 这里是「一根一笔、装不下整根失败」，AMS 7997 的 `16189/0` 一个人装着全部 52,373 件、
/// 渲成 66 MB，于是那个库根本初始化不了。
pub const PERSIST_CHUNK: usize = 2_000;

/// 一笔事务渲成语句之后的硬上限。**不是装块的判据**（那是 [`PERSIST_CHUNK`]），是兜底：
/// 按条数切出来的块仍然超过它，就对半劈开重渲；劈到单条还超，那一条本身有问题，报错。
pub(crate) const PUBLICATION_STATEMENT_HARD_CAP: usize = 8 * 1024 * 1024;

/// 同时在飞的根级写者数。**定死为 1**（512 根合块、单写者）；环境变量 `AIOS_MODEL_PERSIST_WRITERS`
/// 只是实验开关，> 1 时出声告警。
///
/// 2026-09-14 实测（AMS 7997，142 个 ZONE 根，52,373 件，`e3d_persist_smoke --batched`，每轮全新空库）：
///
/// | 介质 | 1 写者 | 4 写者 | 8 写者 |
/// | --- | --- | --- | --- |
/// | kv-mem 内嵌 | 16.3 s | 17.4 s | 21.9 s |
/// | RocksDB（fork surreal，ws） | 19.6 s | 47.5 s | 85.2 s |
///
/// 提交耗时合计随写者数线性放大（kv-mem 15.7 → 60.8 → 118 s；RocksDB 18.6 → 166 → 626 s）：
/// 一台 SurrealDB 的提交本来就是串行的——kv-mem 一把全局写锁，RocksDB 乐观事务在提交点互撞、
/// 再被逐笔冲突重试放大（单笔最长 27–55 s）。多写者抢不到并行，只多付锁等待与重试。
/// 并发该花在生成侧（ZONE 根让几何闸利用率 6% → 39%）和生成与落库的重叠上，不在写者数上。
/// 想让多写者有落点，得是多个 datastore / 分片，那不是这个开关能给的。
///
/// > 1 时 [`persist_roots_batched_unacked_impl`] 不再把根合成一组，而是每根自己一串事务、最多这么多根
/// 同时提交（见 [`persist_shares_concurrently`]）。根之间的行按 `anc` 天然不相交，会撞的只有
/// 每笔都带的 `spatial_epoch:current` 与内容寻址的共享 `inst_geo` / 共享本地盒——那是乐观事务的
/// 「read or write conflict」，由 [`commit_publication_transaction`] 逐笔重试吸收，不升级成整根失败。
pub fn persist_writers() -> usize {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let writers = std::env::var("AIOS_MODEL_PERSIST_WRITERS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 64);
    if writers > 1 {
        WARNED.call_once(|| {
            log::warn!(
                "AIOS_MODEL_PERSIST_WRITERS={writers}：多写者是实验开关，2026-09-14 实测在 kv-mem 与 RocksDB 上都比单写者慢（RocksDB 4 写者 2.4×、8 写者 4.4×），生产请留默认 1"
            );
        });
    }
    writers
}

/// 提交一笔发布事务（`BEGIN … COMMIT`），只对写冲突按指数退避重试。
///
/// 整笔原子：冲突时一行都没落，原样重发是安全的。别的错误（语句错、超上限、传输断）原样返回，
/// 由调用方按根归因。多写者并发时这里是唯一的撞点处理，不要在外面再包一层整根重试。
async fn commit_publication_transaction(
    db: &Surreal<Any>,
    statements: &str,
    context: &str,
) -> anyhow::Result<()> {
    let sql = publication_transaction(statements);
    crate::surreal_retry::retry_surreal_write_operation(context, || {
        // 每次尝试拷一份 `String` 进 future：借 `&str` 进去会让整条 async 链的 `Send` 推断卡在
        // 「对某个具体生命周期才成立」上（写库进程里这段跑在 spawn 出去的任务上）。
        // 一笔 2–3 MB、提交本身几百毫秒，这一拷不值一提。
        let sql = sql.clone();
        async move {
            db.query(sql)
                .await
                .map_err(anyhow::Error::from)
                .and_then(check_publication_response)
        }
    })
    .await
}

/// 把一批记录按 [`PERSIST_CHUNK`] 条切开渲染，返回 `(条数, SQL)`。
///
/// 管身槽位的发号表（`tube_slots`）跨块延续，所以劈开之后 id 不会撞；劈半重渲时先在副本上
/// 试，成了才写回去，不然半次失败的渲染会把号段吃掉。
fn render_record_chunks(
    records: &[ProjectedGeometry],
    mode: WriteMode,
    tube_slots: &mut BTreeMap<(u32, u32), usize>,
) -> anyhow::Result<Vec<(usize, String)>> {
    let mut rendered = Vec::new();
    let mut pending = VecDeque::from_iter(records.chunks(PERSIST_CHUNK.max(1)));
    while let Some(chunk) = pending.pop_front() {
        let mut slots = tube_slots.clone();
        let sql = render_chunk(chunk, mode, &mut slots)?;
        if sql.len() > PUBLICATION_STATEMENT_HARD_CAP && chunk.len() > 1 {
            let (left, right) = chunk.split_at(chunk.len() / 2);
            pending.push_front(right);
            pending.push_front(left);
            continue;
        }
        anyhow::ensure!(
            sql.len() <= PUBLICATION_STATEMENT_HARD_CAP,
            "single geometry record renders to {} bytes, over the {PUBLICATION_STATEMENT_HARD_CAP} byte transaction cap",
            sql.len()
        );
        *tube_slots = slots;
        rendered.push((chunk.len(), sql));
    }
    Ok(rendered)
}

/// 把渲好的块按条数装进一串事务：`prelude`（分支重置 / 整片删 / 清边）进第一笔，
/// `epilogue`（回执）进最后一笔。
///
/// **顺序是这套切块敢成立的全部理由**：删在第一笔、章在最后一笔，中途崩就是「有行没回执」
/// ——那根不是 [`RootShare::reconciliation_ready`]，下一轮不敢整片删、只 upsert，直到重新
/// 盖上章；而窗口规划器眼里它仍未发布，会重生成。空事务不发。
fn pack_publication_transactions(
    prelude: &str,
    pieces: &[(usize, String)],
    epilogue: &str,
    bump_spatial_epoch: bool,
) -> Vec<String> {
    let mut transactions = Vec::new();
    let mut current = String::from(prelude);
    let mut packed = 0usize;
    for (records, sql) in pieces {
        if packed > 0 && packed + records > PERSIST_CHUNK {
            transactions.push(std::mem::take(&mut current));
            packed = 0;
        }
        current.push_str(sql);
        packed += records;
    }
    current.push_str(epilogue);
    transactions.push(current);
    transactions.retain(|statements| !statements.is_empty());
    if bump_spatial_epoch {
        // 「凡是改变了树应有内容的已提交变更，都在同一事务内把 epoch +1」——落库改的正是树
        // 镜像的那些 `inst_relate.aabb` 指针，所以每一笔都要带，不能只带最后一笔。多 bump
        // 几次无害：这个号只与 sidecar 比相等，至多让下次启动多做一次指针重建。
        for statements in &mut transactions {
            statements.push_str(&crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            statements.push('\n');
        }
    }
    transactions
}

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
            // ADR-078 D3：记录上的 `source_sesno` 是「这条记录最后一次真正生成的会话」，
            // 「这根推进到哪了」由回执单独表达。单元级发布只重建变了的那几件，没动的记录仍停在
            // 更早的会话上——所以是 `<=` 不是 `==`。比回执**新**的记录仍然是错：那意味着有人把
            // 几何投进来却没盖章，这根的全集就不是回执说的那一份。
            anyhow::ensure!(
                records.iter().all(|record| {
                    record.source_sesno <= receipt.source_sesno
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

/// 一批生成根名下、模型面已经落过的来源元素（去重、按 refno 排序）。
///
/// 这**不是**对账：它不取几何身份、不与投影比身份、也不缓存。它只回答一个问题——
/// 「这一片里有哪些来源元素」，因为两件下游拿不到谓词：房间边要按来源清
/// （[`room_edges_to_clear`]），进程内空间树要按来源摘
/// （[`crate::fast_model::aabb_tree::converge_tree_after_persist`]）。**行本身不靠它删**，
/// 删整片走 [`render_slice_delete`] 的谓词，一个 id 都不用点名。
///
/// 读的大小由**这一批根**决定，不再由「库里至今攒了多少」决定。旧的整库基线读在 7997 上
/// 68 组累计约 180 万行、单笔事务从 400 ms 涨到 1,280 ms，就是因为它读的是自己刚写进去的
/// 东西——那是 O(N²)。走 `idx_inst_relate_anc`。
///
/// 管身不在其中：隐式管身的来源是 BRAN，房间边与树条目都不按它记
/// （[`room_edges_to_clear`] 同样只认 `GeometryId::Element`）。
async fn slice_sources_on(
    db: &Surreal<Any>,
    packed_roots: &[i64],
) -> anyhow::Result<Vec<RefnoEnum>> {
    if packed_roots.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<String> = db
        .query(
            "SELECT VALUE record::id(in) FROM inst_relate \
             WHERE anc CONTAINSANY $roots AND model_sesno != NONE;",
        )
        .bind(("roots", packed_roots.to_vec()))
        .await
        .context("read model slice sources")?
        .check()?
        .take(0)?;
    let mut sources = BTreeSet::new();
    for key in keys {
        let refno = crate::fast_model::e3d_model_service::parse_refno(&key)
            .with_context(|| format!("model slice source {key}"))?;
        sources.insert(refno_from_e3d(refno));
    }
    Ok(sources.into_iter().collect())
}

/// 一批生成根名下模型面**整片**的删除：不先读、不点名 id，谓词直接圈。
///
/// 谓词与 [`collect_record`] 写行时的作用域同构（`anc` 里有这个打包根、且这行是模型面写下
/// 的），所以「删整片 + 插整片」写的行数与旧的「对账出陈旧行再删」完全一样——存活的行反正
/// 要被重插一遍。换来的是那条整库对账读整个消失。
///
/// `model_sesno != NONE` 是护栏：属性面独有的行（`pdms_inst` 写的、从没被模型碰过的）不带
/// 这一列，谓词碰不到它们。
///
/// 形状上照着 [`render_spooled_scope_cleanup`]，但**删的那几句一律点名记录、不下谓词**：
/// 先把关系行的 id 与 `out` 捞进变量，再把 `aabb` / `trans` 的记录 id 直接拼出来
/// （它们与关系行同名），`DELETE $数组` 是逐条点删，不扫表。
///
/// 谓词形态在这里是个陷阱，AMS 8000 实测过：`DELETE aabb WHERE record::id(id) IN $ids`
/// 里 `record::id(id)` 是**列上的函数**，优化器整条放弃索引退回 `Iterate Table`，然后对
/// 扫到的每一行再去 2,500 项的数组里线性找一遍——两张表两句 OR，一次发布两千多件就是
/// 上千万次数组比较。同一类坑 `fast_delete` 的文档里写过（列上套 `<int>` 也会丢索引）。
fn render_slice_delete(packed_roots: &[i64]) -> String {
    if packed_roots.is_empty() {
        return String::new();
    }
    let roots = packed_roots
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // `inst_info` / `aabb` / `trans` 与关系行**同键**（`collect_record` 就是这么写的，
    // `append_geometry_removal` 也是这么删的），所以直接拼记录，不从 `out` 上取——
    // 烘焙实体那一行根本没有 `out`（网格挂在 `booled_id` 上），取出来是一串 NONE，
    // `DELETE` 见到 NONE 会整笔事务失败。
    //
    // 管身**不同键**：`tubi_relate` 的 id 是分支槽位数组 `[pe:⟨branch⟩, n]`，而它的盒与变换
    // 按几何记录 id（摘要）落在 `aabb:⟨digest⟩` / `trans:⟨digest⟩`（`collect_record` 的管身
    // 那一行）——从槽位 id 拼 `aabb:[pe, n]` 拼出来的是一个不存在的记录，删了等于没删，
    // 管身一消失盒与变换就成了孤儿行。只能顺着行上的 `aabb` / `world_trans` 指针删；护栏
    // 保证圈到的都是模型面写的行，两个指针恒在，不会出 NONE。
    //
    // `geo_relate` 是唯一一句带谓词的：它的 id 是 `[inst_info, inst_geo]` 数组、从 `inst_info`
    // 一侧拼不出来，只能按 `in` 找——走 `idx_geo_relate_in`，且照 [`append_geometry_removal`]
    // 的路子先 `SELECT` 出 id 再 `DELETE`（部署引擎上 `DELETE … WHERE` 不走索引）。
    format!(
        "LET $slice_keys = SELECT VALUE record::id(id) FROM inst_relate \
         WHERE anc CONTAINSANY [{roots}] AND model_sesno != NONE;\n\
         LET $slice_tubes = SELECT id, aabb, world_trans FROM tubi_relate \
         WHERE anc CONTAINSANY [{roots}] AND model_sesno != NONE;\n\
         LET $slice_infos = $slice_keys.map(|$key| type::thing('inst_info', $key));\n\
         DELETE (SELECT VALUE id FROM geo_relate WHERE in IN $slice_infos);\n\
         DELETE $slice_infos;\n\
         DELETE $slice_keys.map(|$key| type::thing('inst_relate', $key));\n\
         DELETE $slice_tubes.map(|$row| $row.id);\n\
         DELETE $slice_keys.map(|$key| type::thing('aabb', $key));\n\
         DELETE $slice_tubes.map(|$row| $row.aabb);\n\
         DELETE $slice_keys.map(|$key| type::thing('trans', $key));\n\
         DELETE $slice_tubes.map(|$row| $row.world_trans);\n"
    )
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
/// 2. 对账身份分页读取：窗口内复用库级基线；窗口外只查询本批 root，各根按祖先链分摊；
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
    persist_roots_batched_unacked_impl(target, store, dbnum, roots, rooms).await
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

async fn persist_roots_batched_unacked_impl(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    roots: &[RefNo],
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

    let mut total = PersistedRows::none(target.medium);
    let mut refresh_all = Vec::<aios_core::RefnoEnum>::new();
    let mut boxes_all = Vec::<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>::new();
    let mut cleared_all = Vec::<aios_core::RefnoEnum>::new();

    // 多写者（`AIOS_MODEL_PERSIST_WRITERS` > 1）：每根一个写者、并发提交，不合块、不二分。
    // 单写者走下面原来的 512 根合块 + 二分重试那一路，行为不变。
    let writers = persist_writers();
    let concurrent_groups = if writers > 1 {
        persist_shares_concurrently(
            db,
            dbnum,
            target.medium,
            rooms,
            &shares,
            writers,
            &mut total,
            &mut out,
            &mut refresh_all,
            &mut boxes_all,
            &mut cleared_all,
        )
        .await;
        &shares[shares.len()..]
    } else {
        &shares[..]
    };

    for group in concurrent_groups.chunks(BATCHED_PERSIST_ROOTS_PER_GROUP) {
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
        match persist_group(db, &live, !target.medium.is_durable()).await {
            Ok(outcome) => accept_group_outcome(
                &live,
                dbnum,
                target.medium,
                rooms,
                outcome,
                &mut total,
                &mut out,
                &mut refresh_all,
                &mut boxes_all,
                &mut cleared_all,
            ),
            Err(error) if live.len() == 1 => {
                out.push_failure(
                    live[0].share.root,
                    publication_failure_kind(&error),
                    format!("{error:#}"),
                );
            }
            Err(error) => {
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
                    match persist_group(db, retry, !target.medium.is_durable()).await {
                        Ok(outcome) => accept_group_outcome(
                            retry,
                            dbnum,
                            target.medium,
                            rooms,
                            outcome,
                            &mut total,
                            &mut out,
                            &mut refresh_all,
                            &mut boxes_all,
                            &mut cleared_all,
                        ),
                        Err(error) if retry.len() == 1 => {
                            out.push_failure(
                                retry[0].share.root,
                                publication_failure_kind(&error),
                                format!("{error:#}"),
                            );
                        }
                        Err(error) => {
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
    /// 整片删掉之后这一轮不再有几何的来源：房间边清掉、树条目摘掉。
    cleared: Vec<aios_core::RefnoEnum>,
}

/// 多写者那一路：**一根一个写者**，最多 `writers` 根同时在飞，每根自己一串事务（整片删进第一笔、
/// 回执进最后一笔，见 [`persist_group`]）。
///
/// 根之间按 `anc` 不相交，所以不需要合块也不需要二分——一根失败就是这一根的失败，赔付边界天然
/// 是根。房间关卡仍在写库之前、按根过（ADR-073 D1 / D2）。账（`out` / `total` / 收敛名单）在
/// 本任务里按完成顺序收，不跨任务共享可变状态。
///
/// 并发是同一个 tokio 任务上的 `buffer_unordered`：提交（IO）互相重叠，渲染（CPU）仍是串行
/// 穿插——写者的瓶颈在库那一侧，这一版先不把渲染搬去阻塞池。
#[allow(clippy::too_many_arguments)]
async fn persist_shares_concurrently(
    db: &Surreal<Any>,
    dbnum: u32,
    medium: PersistMedium,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
    shares: &[RootShare],
    writers: usize,
    total: &mut PersistedRows,
    out: &mut BatchedPersist,
    refresh_all: &mut Vec<aios_core::RefnoEnum>,
    boxes_all: &mut Vec<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>,
    cleared_all: &mut Vec<aios_core::RefnoEnum>,
) {
    use futures::StreamExt as _;

    let (live, room_failed) = room_stage(shares, rooms);
    for (root, error) in room_failed {
        out.push_failure(root, PersistFailureKind::Blocked, error);
    }
    if live.is_empty() {
        return;
    }
    let create_missing_receipts = !medium.is_durable();
    log::info!(
        "model publication fan-out: roots={} writers={} dbnum={dbnum}",
        live.len(),
        writers.min(live.len())
    );
    // 用 for 循环把每根的 future 先攒成 Vec，而不是 `iter().map(|one| async move …)`：
    // 闭包参数是引用时，返回的 async 块会带上高阶生命周期，整条链的 `Send` 就推不出来
    // （「implementation of Send is not general enough」）。
    let mut pending = Vec::with_capacity(live.len());
    for (index, one) in live.iter().enumerate() {
        pending.push(async move {
            let outcome =
                persist_group(db, std::slice::from_ref(one), create_missing_receipts).await;
            (index, outcome)
        });
    }
    let mut settled = futures::stream::iter(pending).buffer_unordered(writers);
    while let Some((index, outcome)) = settled.next().await {
        out.groups += 1;
        let one = &live[index..index + 1];
        match outcome {
            Ok(outcome) => accept_group_outcome(
                one,
                dbnum,
                medium,
                rooms,
                outcome,
                total,
                out,
                refresh_all,
                boxes_all,
                cleared_all,
            ),
            Err(error) => out.push_failure(
                one[0].share.root,
                publication_failure_kind(&error),
                format!("{error:#}"),
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn accept_group_outcome(
    group: &[LiveShare<'_>],
    dbnum: u32,
    medium: PersistMedium,
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
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
}

/// Typed context for failures that retrying the same local payload cannot fix.
#[derive(Debug)]
struct DeterministicPublicationFailure;

impl std::fmt::Display for DeterministicPublicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid local publication payload")
    }
}

impl std::error::Error for DeterministicPublicationFailure {}

fn publication_failure_kind(error: &anyhow::Error) -> PersistFailureKind {
    if error
        .downcast_ref::<DeterministicPublicationFailure>()
        .is_some()
    {
        PersistFailureKind::Blocked
    } else {
        PersistFailureKind::Retryable
    }
}

/// 一组过了房间关卡的根：渲一笔、发一笔。失败即返回 Err，调用方把这一组记成失败。
/// 每根的顺序：最终几何 → 房间归属 → 同事务写模型行、房间边与回执。
async fn persist_group(
    db: &Surreal<Any>,
    group: &[LiveShare<'_>],
    create_missing_receipts: bool,
) -> anyhow::Result<GroupOutcome> {
    // 本根要渲的完整记录；跨根重复由数据库幂等语义收敛。
    let records = group
        .iter()
        .flat_map(|live| live.claimed.iter().cloned())
        .collect::<Vec<_>>();

    // 有回执的根才拿得起自己的全集，也才敢把它名下整片推掉（[`RootShare::reconciliation_ready`]）。
    // 没回执的根只 upsert 自己那几行，库里既有的不动。
    let sliced_roots = group
        .iter()
        .map(|live| live.share)
        .filter(|share| share.reconciliation_ready())
        .map(|share| packed_refno(share.root))
        .collect::<Vec<_>>();

    // 一个 BRAN 的隐式管身是整段重写的：这一轮碰到的分支先清空，再插存储里的那几段。
    // 整片删已经覆盖了带 `anc` 的管身行，这一条兜的是 `anc` 缺失的历史行。
    let branches = group
        .iter()
        .map(|live| live.share)
        .filter(|share| share.reconciliation_ready())
        .flat_map(|share| share.records.iter())
        .filter(|record| record.tube.is_some())
        .map(|record| (record.source_refno.word0, record.source_refno.word1))
        .collect::<BTreeSet<_>>();

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

    // 几何段：**逐根**渲染，每根再按 [`PERSIST_CHUNK`] 条切块。这一根的两张房间边紧跟在
    // 它**最后一块**几何后面（ADR-073 D1 / W2：模型在前、房间在后；根跨多笔时边跟着它的
    // 末块走，中途崩就是没章的半截，下一轮重来）；只有回执没有几何的根（删干净了）没有边
    // 可写，它的旧边由上面的清边那一半处理。
    let mut pieces: Vec<(usize, String)> = Vec::new();
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    for live in group {
        // 整片删已经把这一根名下的行清干净了，`Incremental` 那组按 id 的删除就是纯重复——
        // 而它不便宜：一根两千多件时那是四个各两千多项的 `type::thing()` 数组，AMS 8000
        // 实测整条落库从 16.7 s 涨到 50.3 s，涨的全是这一段。没被整片删覆盖的根（没有回执，
        // 不敢推整片）照旧走 `Incremental`。
        let mode = if live.share.reconciliation_ready() {
            WriteMode::Initial
        } else {
            WriteMode::Incremental
        };
        // 渲染是本地的：这一根切不开、单条记录就超上限，后面拼什么都救不回来。这里就判掉，
        // 一次库都不碰——本地载荷有问题不该先去读一遍再发现
        // （`oversized_local_publication_is_blocked_before_database_io`）。
        let mut rendered = render_record_chunks(&live.claimed, mode, &mut tube_slots)
            .with_context(|| format!("render root {}", live.share.root))
            .context(DeterministicPublicationFailure)?;
        if !live.room_statements.is_empty() {
            match rendered.last_mut() {
                Some((_, sql)) => sql.push_str(&live.room_statements),
                None => rendered.push((0, live.room_statements.clone())),
            }
        }
        pieces.extend(rendered);
    }

    // 本地载荷到这里已经全部过关，才轮到碰库。整片删之前库里有过的来源：行不靠它删
    // （谓词删，见 [`render_slice_delete`]），它只喂房间清边与空间树摘条目。
    let slice_read_started = std::time::Instant::now();
    let slice_sources = slice_sources_on(db, &sliced_roots).await?;
    let slice_read_elapsed = slice_read_started.elapsed();
    let cleared_room_edges = room_edges_to_clear(
        records.iter().map(|record| {
            (
                &record.geometry_id,
                record.source_refno,
                record.noun.as_str(),
            )
        }),
        &slice_sources,
        &BTreeSet::new(),
    );

    // 回执：同一份 ModelTarget 只 `LET` 一次，几百根共用。
    let receipted = group
        .iter()
        .map(|live| live.share)
        .filter_map(|share| share.receipt.as_ref().map(|receipt| (share.root, receipt)))
        .collect::<Vec<_>>();
    let mut receipt_header = String::new();
    let mut receipt_body = String::new();
    {
        let mut targets = BTreeMap::<String, usize>::new();
        for (root, receipt) in &receipted {
            let literal =
                receipt_model_target_literal(receipt).context(DeterministicPublicationFailure)?;
            let expression = if literal == "NONE" {
                literal
            } else {
                let next = targets.len();
                let index = *targets.entry(literal.clone()).or_insert(next);
                if index == next {
                    receipt_header.push_str(&format!("LET $mt{index} = {literal};\n"));
                }
                format!("$mt{index}")
            };
            receipt_body.push_str(&render_receipt_with_target(
                *root,
                receipt,
                None,
                &expression,
                create_missing_receipts,
            ));
        }
    }

    // 本组随模型行同事务写下的房间边数（各根在 [`room_stage`] 里算好的那份）。
    let room_edge_count = group.iter().map(|live| live.room_edges).sum::<usize>();

    // **一组一串事务，按条数装块。** 删整片与清边进第一笔，盖章进最后一笔；中途崩就是
    // 「有行没回执」，那几根下一轮重新生成、重新整片删，自愈。完成与否不看某一笔，看水位
    // （`settle` 只在这个库一根不剩时才推进 `dbnum_watermark.model_sesno`）。
    let mut prelude = String::new();
    for &(word0, word1) in &branches {
        append_tube_branch_reset(&mut prelude, RefNo::new(word0, word1));
    }
    prelude.push_str(&render_slice_delete(&sliced_roots));
    // 清边跟着删几何走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
    prelude.push_str(&render_room_edge_clear(&cleared_room_edges));
    let touches_geometry = !pieces.is_empty() || !sliced_roots.is_empty() || !branches.is_empty();
    let epilogue = format!("{receipt_header}{receipt_body}");
    let transactions =
        pack_publication_transactions(&prelude, &pieces, &epilogue, touches_geometry);
    if transactions.is_empty() {
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
    let mut max_transaction_bytes = 0usize;
    let mut commits = 0usize;
    for (index, statements) in transactions.iter().enumerate() {
        max_transaction_bytes = max_transaction_bytes.max(statements.len());
        let commit_started = std::time::Instant::now();
        let committed =
            commit_publication_transaction(db, statements, "model publication group").await;
        // 一笔一行的落库账：验收第二条（「读与单笔事务耗时都不随已落库量增长」）就靠连续
        // 落 N 组之后比这几列的头尾。读与提交分开计时——整片删的谓词收不收敛，看的是 `read`
        // 那一列随 `n` 平不平；`read` 只在第一笔算数，后面几笔是同一次读摊下来的。
        log::info!(
            "model publication group settled: roots={} records={} sliced_roots={} slice_sources={} read={:.1}ms tx={}/{} sql_bytes={} commit={:.1}ms ok={}",
            group.len(),
            records.len(),
            sliced_roots.len(),
            slice_sources.len(),
            if index == 0 {
                slice_read_elapsed.as_secs_f64() * 1000.0
            } else {
                0.0
            },
            index + 1,
            transactions.len(),
            statements.len(),
            commit_started.elapsed().as_secs_f64() * 1000.0,
            committed.is_ok(),
        );
        committed.with_context(|| {
            format!(
                "persist {} projected root(s)（第 {} / {} 笔）",
                group.len(),
                index + 1,
                transactions.len()
            )
        })?;
        commits += 1;
    }

    Ok(GroupOutcome {
        upserted: records.len(),
        removed: cleared_room_edges.len(),
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
        return Ok(true);
    }
    let root_id = format!("{}_{}", root.word0, root.word1);
    let mut response = db
        .query("SELECT VALUE publication_status = 'ready' FROM type::thing('gen_root', $root_id);")
        .bind(("root_id", root_id))
        .await
        .and_then(|response| response.check())?;
    Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
}

async fn persist_root_impl(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    root: RefNo,
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
        // ADR-078 D3：同 [`RootShare::load`] 的那一条——记录带的是自己最后一次生成的会话，
        // 根推进到哪了看回执。单元级发布之后没动的记录会落在回执之前，比回执新的仍是错。
        anyhow::ensure!(
            records.iter().all(|record| {
                record.source_sesno <= receipt.source_sesno
                    && record.ancestors.contains(&packed_root)
            }),
            "root {root} projection contains records outside its receipt scope"
        );
        true
    } else {
        false
    };
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
    // 有回执才敢把这根名下整片推掉；没回执只 upsert 自己那几行。
    let sliced_roots = if reconciliation_ready {
        vec![packed_refno(root)]
    } else {
        Vec::new()
    };

    // 房间派生面（ADR-010 §4 / ADR-040 §3）。旧发布事务把这两笔挂在同一条事务上，
    // e3d-model 接管生成之后断了一段时间；现在从投影记录上重新折出来：
    // 拿到新几何的来源排一次房间重算，整片删掉之后不再有几何的来源两个方向的房间边一并清掉。
    // `legacy_sources` 空着——那是 pre-e3d 清理路径才有的名单，落库这一侧没有。
    let slice_read_started = std::time::Instant::now();
    let slice_sources = slice_sources_on(db, &sliced_roots).await?;
    let slice_read_elapsed = slice_read_started.elapsed();
    let cleared_room_edges = room_edges_to_clear(
        records.iter().map(|record| {
            (
                &record.geometry_id,
                record.source_refno,
                record.noun.as_str(),
            )
        }),
        &slice_sources,
        &BTreeSet::new(),
    );
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

    // **一根一串事务，按 [`PERSIST_CHUNK`] 条装块。** 删整片与清边进第一笔、盖章进最后
    // 一笔，靠「先删后插 + 回执最后盖」让半途而废自愈：一笔失败就没有章，这根在窗口规划器
    // 眼里仍未发布，下一轮重生成，而下一轮的整片删会把这一轮留下的半截行收干净。
    let mode = if reconciliation_ready {
        // 整片删覆盖得到的根不必再按 id 删一遍（见 `persist_group` 里同一处的说明）。
        WriteMode::Initial
    } else {
        WriteMode::Incremental
    };
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    let pieces = render_record_chunks(&records, mode, &mut tube_slots)?;

    let mut prelude = String::new();
    for &(word0, word1) in &branches {
        append_tube_branch_reset(&mut prelude, RefNo::new(word0, word1));
    }
    prelude.push_str(&render_slice_delete(&sliced_roots));
    // 清边跟着删几何走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
    prelude.push_str(&render_room_edge_clear(&cleared_room_edges));
    let epilogue = match receipt.as_ref() {
        Some(receipt) => render_receipt_version(root, receipt, None)?,
        None => String::new(),
    };
    let touches_geometry = !pieces.is_empty() || !sliced_roots.is_empty() || !branches.is_empty();
    let transactions =
        pack_publication_transactions(&prelude, &pieces, &epilogue, touches_geometry);
    let mut commits = 0usize;
    for (index, statements) in transactions.iter().enumerate() {
        let commit_started = std::time::Instant::now();
        let committed =
            commit_publication_transaction(db, statements, "model publication root").await;
        // 与 `persist_group` 同一张账（那里是合块那一路，这里是逐根那一路）。
        log::info!(
            "model publication group settled: roots=1 records={} sliced_roots={} slice_sources={} read={:.1}ms tx={}/{} sql_bytes={} commit={:.1}ms ok={}",
            records.len(),
            sliced_roots.len(),
            slice_sources.len(),
            if index == 0 {
                slice_read_elapsed.as_secs_f64() * 1000.0
            } else {
                0.0
            },
            index + 1,
            transactions.len(),
            statements.len(),
            commit_started.elapsed().as_secs_f64() * 1000.0,
            committed.is_ok(),
        );
        committed.with_context(|| {
            format!(
                "persist projected root {root}（第 {} / {} 笔）",
                index + 1,
                transactions.len()
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
        removed: cleared_room_edges.len(),
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

/// 镜像 spool / 墓碑那一路的整片删：一根名下模型面的全部行。
///
/// 作用域比 [`render_slice_delete`] 多一路 `in = $mirror_owner`——兜的是 `anc` 缺失的历史行
/// （根自己的几何 `in` 就是根）。两路**分开读、再 `array::union`**，不写成一条 `… OR …`：
/// 优化器碰到 `anc CONTAINS … OR in = $参数` 整条放弃索引退回 `Iterate Table`（mem 引擎
/// EXPLAIN 实测，`spooled_cleanup_predicates_are_planned_through_indexes` 钉住），拆开之后
/// 各自是一次单索引查（`idx_*_anc` / `idx_*_in`），并集在库端算、体量由这一根决定。
/// 回放时谓词由 SurrealDB 现算，替换行早在源纪元就渲好了。
///
/// **删的那几句与 [`render_slice_delete`] 同一套纪律**：一律 `DELETE $数组` 直接点名、不下谓词
/// （部署引擎上 `DELETE … WHERE` 不走索引，列上套函数更是任何引擎都扫表）；`inst_info` /
/// `aabb` / `trans` 与关系行同键直接拼；管身的盒与变换按几何摘要另键、只能顺行上的
/// `aabb` / `world_trans` 指针删——从槽位 id 拼出来的是不存在的记录，删了等于没删。
/// `geo_relate` 是唯一一句带谓词的删：先按 `in` 走 `idx_geo_relate_in` 选出 id 再删。
fn render_spooled_scope_cleanup(root: RefNo) -> String {
    let packed = packed_refno(root);
    let owner = sql_string(&refno_key(root));
    format!(
        "LET $mirror_owner = type::thing('pe',{owner});\n\
         LET $mirror_keys_by_anc = SELECT VALUE record::id(id) FROM inst_relate \
         WHERE anc CONTAINS {packed} AND model_sesno != NONE;\n\
         LET $mirror_keys_by_owner = SELECT VALUE record::id(id) FROM inst_relate \
         WHERE in = $mirror_owner AND model_sesno != NONE;\n\
         LET $mirror_keys = array::union($mirror_keys_by_anc, $mirror_keys_by_owner);\n\
         LET $mirror_tubes_by_anc = SELECT id, aabb, world_trans FROM tubi_relate \
         WHERE anc CONTAINS {packed} AND model_sesno != NONE;\n\
         LET $mirror_tubes_by_owner = SELECT id, aabb, world_trans FROM tubi_relate \
         WHERE in = $mirror_owner AND model_sesno != NONE;\n\
         LET $mirror_tubes = array::union($mirror_tubes_by_anc, $mirror_tubes_by_owner);\n\
         LET $mirror_infos = $mirror_keys.map(|$key| type::thing('inst_info', $key));\n\
         DELETE (SELECT VALUE id FROM geo_relate WHERE in IN $mirror_infos);\n\
         DELETE $mirror_infos;\n\
         DELETE $mirror_keys.map(|$key| type::thing('inst_relate', $key));\n\
         DELETE $mirror_tubes.map(|$row| $row.id);\n\
         DELETE $mirror_keys.map(|$key| type::thing('aabb', $key));\n\
         DELETE $mirror_tubes.map(|$row| $row.aabb);\n\
         DELETE $mirror_keys.map(|$key| type::thing('trans', $key));\n\
         DELETE $mirror_tubes.map(|$row| $row.world_trans);\n"
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
            // 同 id 必同内容（ADR-060 N3 的第一种前提）。此前那条 `ON DUPLICATE KEY UPDATE` 是为
            // 「修正旧渲染器留下的描述符」而设，而 `aabb` 只在标准图元行上有、同一 mesh id 不会既是
            // 烘焙又是标准图元，所以它实际改不了任何读者看得见的东西。模型面从此零 UPDATE。
            // 旧行照旧留着，与 `inst_geo` 只增不删（ADR-060 D2）同一口径。
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
    let anc = sql_value(&json!(record.ancestors));
    let world_transform = sql_value(&transform_value(record.world_transform));
    let mesh_id = bracketed_id(record.mesh_id.as_str())?;
    let mesh_id_sql = sql_string(mesh_id);
    let noun = sql_string(&record.noun);
    let dbnum = record.dbnum;
    let source_sesno = record.source_sesno;

    let world_bounds = format!(
        "{{mins:{},maxs:{}}}",
        sql_value(&f32_values(record.world_bounds[0])),
        sql_value(&f32_values(record.world_bounds[1])),
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
    let flat_copies = format!("aabb_d:{world_bounds},world_trans_d:{world_transform}");

    if let ProjectedShape::Canonical { local_bounds, .. } = &record.shape {
        batch.aabb.push(format!(
            "{{id:type::thing('aabb',{mesh_id_sql}),d:{{mins:{},maxs:{}}}}}",
            sql_value(&f32_values(local_bounds[0])),
            sql_value(&f32_values(local_bounds[1])),
        ));
        batch.inst_geo.insert(mesh_id.to_owned(), format!(
            "{{id:inst_geo:⟨{mesh_id}⟩,meshed:true,visible:true,bad:false,aabb:type::thing('aabb',{mesh_id_sql})}}"
        ));
    }

    if let Some(tube) = record.tube.as_ref() {
        // 隐式管身只向 Plant UI 暴露一个变换，标准图元的本地摆放在投影时就折进世界变换里
        // 了，所以这里既不写本地变换，也不写 `inst_info` / `inst_relate`。
        let slot = next_tube_relation_slot(tube_slots, record.source_refno);
        if matches!(record.shape, ProjectedShape::Baked) {
            batch.inst_geo.insert(
                mesh_id.to_owned(),
                format!("{{id:inst_geo:⟨{mesh_id}⟩,meshed:true,visible:true,bad:false}}"),
            );
        }
        batch.tubi_relate.push(format!(
            "{{id:tubi_relate:[pe:⟨{source_id}⟩,{slot}],in:pe:⟨{source_id}⟩,out:inst_geo:⟨{mesh_id}⟩,leave:pe:⟨{}_{}⟩,arrive:pe:⟨{}_{}⟩,aabb:aabb:⟨{id}⟩,world_trans:trans:⟨{id}⟩,bore_size:0,invalid:{},anc:{anc},dbnum:{dbnum},model_sesno:{source_sesno}}}",
            tube.from.word0, tube.from.word1, tube.to.word0, tube.to.word1,
            tube.invalid,
        ));
        return Ok(());
    }

    match &record.shape {
        ProjectedShape::Canonical {
            local_transform, ..
        } => {
            let local_transform = sql_value(&transform_value(*local_transform));

            batch.inst_info.push(format!(
                "{{id:type::thing('inst_info',{id_sql}),dbnum:{dbnum},noun:{noun},model_sesno:{source_sesno}}}"
            ));
            batch.geo_relate.push(format!(
                "{{id:geo_relate:[type::thing('inst_info',{id_sql}),inst_geo:⟨{mesh_id}⟩],in:type::thing('inst_info',{id_sql}),out:inst_geo:⟨{mesh_id}⟩,geom_refno:type::thing('pe',{source_id_sql}),transform:{local_transform},visible:true,geo_type:'Pos'}}"
            ));
            batch.inst_relate.push(format!(
                "{{id:type::thing('inst_relate',{id_sql}),in:type::thing('pe',{source_id_sql}),out:type::thing('inst_info',{id_sql}),booled_id:NONE,booled:false,bad_bool:false,solid:true,generic:{noun},dbnum:{dbnum},anc:{anc},aabb:type::thing('aabb',{id_sql}),world_trans:type::thing('trans',{id_sql}),{flat_copies},insts_flat:[{{geo_hash:{mesh_id_sql},transform:{local_transform}}}],model_sesno:{source_sesno}}}"
            ));
        }
        ProjectedShape::Baked => {
            batch.inst_relate.push(format!(
                "{{id:type::thing('inst_relate',{id_sql}),in:type::thing('pe',{source_id_sql}),booled_id:{mesh_id_sql},booled:true,bad_bool:false,solid:true,generic:{noun},dbnum:{dbnum},anc:{anc},aabb:type::thing('aabb',{id_sql}),world_trans:type::thing('trans',{id_sql}),{flat_copies},insts_flat:[{{geo_hash:{mesh_id_sql}}}],model_sesno:{source_sesno}}}"
            ));
        }
    }
    Ok(())
}

/// 同一批行的**参数形态**：不渲成 SurrealQL 文本，直接铸成 SurrealDB 的值，由调用方
/// `.bind()` 挂到 `INSERT [RELATION] IGNORE INTO <表> $rows` 上。
///
/// 表、顺序、字段与 [`render_records`] 逐项对齐，差别只有一处：**谁来解析**。文本那条路
/// 客户端先把整串 `syn::parse` 解析成 AST（`db.query(String)` 内部就做这一下）、再
/// `Display` 回字符串发出去，服务端第三次解析；参数这条路两头都不解析，值以二进制过线。
///
/// 今天只服务基准（`e3d_persist_smoke --bindbench`）。生产写入仍走 [`render_records`]：
/// 发布事务里还有整片删、回执、盖章那些语句，换形态是整条路一起换的事，不是换一个渲染器。
pub fn bind_records(records: &[ProjectedGeometry]) -> anyhow::Result<Vec<BoundTable>> {
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    let mut batch = BoundBatch::default();
    for record in records {
        collect_bound_record(&mut batch, record, &mut tube_slots)?;
    }
    Ok(batch.finish())
}

/// 一张表的一批行，对应文本那条路的一条 `INSERT … INTO <表> [ … ]`。
#[derive(Debug, Clone)]
pub struct BoundTable {
    pub table: &'static str,
    /// 要不要 `RELATION` 关键字；与 [`RowBatch::render`] 里同一张表的答案相同。
    pub relation: bool,
    pub rows: Vec<SurValue>,
}

impl BoundTable {
    /// 这一张表的那条语句（行本身走 `$<param>` 参数，不进文本）。
    pub fn statement(&self, param: &str) -> String {
        let keyword = if self.relation {
            "INSERT RELATION IGNORE INTO"
        } else {
            "INSERT IGNORE INTO"
        };
        format!("{keyword} {} ${param} RETURN NONE;\n", self.table)
    }
}

/// [`RowBatch`] 的值版：同样按表攒行，同样的表间顺序。
#[derive(Debug, Default)]
struct BoundBatch {
    aabb: Vec<SurValue>,
    trans: Vec<SurValue>,
    inst_geo: BTreeMap<String, SurValue>,
    inst_info: Vec<SurValue>,
    geo_relate: Vec<SurValue>,
    inst_relate: Vec<SurValue>,
    tubi_relate: Vec<SurValue>,
}

impl BoundBatch {
    fn finish(self) -> Vec<BoundTable> {
        let shared_meshes = self.inst_geo.into_values().collect::<Vec<_>>();
        [
            ("aabb", self.aabb, false),
            ("trans", self.trans, false),
            ("inst_geo", shared_meshes, false),
            ("inst_info", self.inst_info, false),
            ("geo_relate", self.geo_relate, true),
            ("inst_relate", self.inst_relate, false),
            ("tubi_relate", self.tubi_relate, true),
        ]
        .into_iter()
        .filter(|(_, rows, _)| !rows.is_empty())
        .map(|(table, rows, relation)| BoundTable {
            table,
            relation,
            rows,
        })
        .collect()
    }
}

/// `<表>:<id>`，id 段是字符串——与文本那条路的 `type::thing('<表>','<id>')` 同一个记录。
fn sur_thing(table: &str, id: &str) -> SurValue {
    SurValue::Thing(SurThing::from((
        table.to_owned(),
        SurId::String(id.to_owned()),
    )))
}

/// 复合键那两张表的 id（`geo_relate:[…,…]` / `tubi_relate:[…,…]`）。
fn sur_keyed_thing(table: &str, key: Vec<SurValue>) -> SurValue {
    SurValue::Thing(SurThing::from((
        table.to_owned(),
        SurId::Array(SurArray::from(key)),
    )))
}

fn sur_int(value: u32) -> SurValue {
    SurValue::Number(SurNumber::Int(i64::from(value)))
}

/// 一个 JSON 值搬进 SurrealDB 的值域。
///
/// 数值一律落 `Number::Float`。文本那条路是把同一个字面量交给 SurrealQL 词法器，**两边
/// 未必判成同一个类型**——正是基准要对出来的东西，所以这里不迁就，照 f64 直落。
fn sur_json(value: Value) -> anyhow::Result<SurValue> {
    surrealdb::sql::to_value(value).context("JSON 值搬进 SurrealDB 值域")
}

fn sur_object<const N: usize>(fields: [(&str, SurValue); N]) -> SurValue {
    SurValue::Object(SurObject::from(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<BTreeMap<String, SurValue>>(),
    ))
}

/// [`collect_record`] 的值版。改那边就得改这边——两边渲的是同一批行，基准靠逐行比对
/// 两个库的内容把跑偏的字段抓出来（`--bindbench` 的 `BINDBENCH-VERIFY` 那一行）。
fn collect_bound_record(
    batch: &mut BoundBatch,
    record: &ProjectedGeometry,
    tube_slots: &mut BTreeMap<(u32, u32), usize>,
) -> anyhow::Result<()> {
    let id = bracketed_id(record.record_id.as_str())?;
    let source_id = refno_key(record.source_refno);
    let mesh_id = bracketed_id(record.mesh_id.as_str())?;
    let noun = SurValue::from(record.noun.clone());
    let dbnum = sur_int(record.dbnum);
    let source_sesno = sur_int(record.source_sesno);
    let anc = sur_json(json!(record.ancestors))?;
    let world_transform = sur_json(transform_value(record.world_transform))?;
    let world_bounds = sur_json(json!({
        "mins": f32_values(record.world_bounds[0]),
        "maxs": f32_values(record.world_bounds[1]),
    }))?;

    batch.aabb.push(sur_object([
        ("id", sur_thing("aabb", id)),
        ("d", world_bounds.clone()),
    ]));
    batch.trans.push(sur_object([
        ("id", sur_thing("trans", id)),
        ("d", world_transform.clone()),
    ]));

    if let ProjectedShape::Canonical { local_bounds, .. } = &record.shape {
        batch.aabb.push(sur_object([
            ("id", sur_thing("aabb", mesh_id)),
            (
                "d",
                sur_json(json!({
                    "mins": f32_values(local_bounds[0]),
                    "maxs": f32_values(local_bounds[1]),
                }))?,
            ),
        ]));
        batch.inst_geo.insert(
            mesh_id.to_owned(),
            sur_object([
                ("id", sur_thing("inst_geo", mesh_id)),
                ("meshed", SurValue::Bool(true)),
                ("visible", SurValue::Bool(true)),
                ("bad", SurValue::Bool(false)),
                ("aabb", sur_thing("aabb", mesh_id)),
            ]),
        );
    }

    if let Some(tube) = record.tube.as_ref() {
        let slot = next_tube_relation_slot(tube_slots, record.source_refno);
        if matches!(record.shape, ProjectedShape::Baked) {
            batch.inst_geo.insert(
                mesh_id.to_owned(),
                sur_object([
                    ("id", sur_thing("inst_geo", mesh_id)),
                    ("meshed", SurValue::Bool(true)),
                    ("visible", SurValue::Bool(true)),
                    ("bad", SurValue::Bool(false)),
                ]),
            );
        }
        batch.tubi_relate.push(sur_object([
            (
                "id",
                sur_keyed_thing(
                    "tubi_relate",
                    vec![
                        sur_thing("pe", &source_id),
                        SurValue::Number(SurNumber::Int(slot as i64)),
                    ],
                ),
            ),
            ("in", sur_thing("pe", &source_id)),
            ("out", sur_thing("inst_geo", mesh_id)),
            ("leave", sur_thing("pe", &refno_key(tube.from))),
            ("arrive", sur_thing("pe", &refno_key(tube.to))),
            ("aabb", sur_thing("aabb", id)),
            ("world_trans", sur_thing("trans", id)),
            ("bore_size", SurValue::Number(SurNumber::Int(0))),
            ("invalid", SurValue::Bool(tube.invalid)),
            ("anc", anc),
            ("dbnum", dbnum),
            ("model_sesno", source_sesno),
        ]));
        return Ok(());
    }

    match &record.shape {
        ProjectedShape::Canonical {
            local_transform, ..
        } => {
            let local_transform = sur_json(transform_value(*local_transform))?;
            batch.inst_info.push(sur_object([
                ("id", sur_thing("inst_info", id)),
                ("dbnum", dbnum.clone()),
                ("noun", noun.clone()),
                ("model_sesno", source_sesno.clone()),
            ]));
            batch.geo_relate.push(sur_object([
                (
                    "id",
                    sur_keyed_thing(
                        "geo_relate",
                        vec![sur_thing("inst_info", id), sur_thing("inst_geo", mesh_id)],
                    ),
                ),
                ("in", sur_thing("inst_info", id)),
                ("out", sur_thing("inst_geo", mesh_id)),
                ("geom_refno", sur_thing("pe", &source_id)),
                ("transform", local_transform.clone()),
                ("visible", SurValue::Bool(true)),
                ("geo_type", SurValue::from("Pos")),
            ]));
            batch.inst_relate.push(sur_object([
                ("id", sur_thing("inst_relate", id)),
                ("in", sur_thing("pe", &source_id)),
                ("out", sur_thing("inst_info", id)),
                ("booled_id", SurValue::None),
                ("booled", SurValue::Bool(false)),
                ("bad_bool", SurValue::Bool(false)),
                ("solid", SurValue::Bool(true)),
                ("generic", noun),
                ("dbnum", dbnum),
                ("anc", anc),
                ("aabb", sur_thing("aabb", id)),
                ("world_trans", sur_thing("trans", id)),
                ("aabb_d", world_bounds),
                ("world_trans_d", world_transform),
                (
                    "insts_flat",
                    SurValue::Array(SurArray::from(vec![sur_object([
                        ("geo_hash", SurValue::from(mesh_id.to_owned())),
                        ("transform", local_transform),
                    ])])),
                ),
                ("model_sesno", source_sesno),
            ]));
        }
        ProjectedShape::Baked => {
            batch.inst_relate.push(sur_object([
                ("id", sur_thing("inst_relate", id)),
                ("in", sur_thing("pe", &source_id)),
                ("booled_id", SurValue::from(mesh_id.to_owned())),
                ("booled", SurValue::Bool(true)),
                ("bad_bool", SurValue::Bool(false)),
                ("solid", SurValue::Bool(true)),
                ("generic", noun),
                ("dbnum", dbnum),
                ("anc", anc),
                ("aabb", sur_thing("aabb", id)),
                ("world_trans", sur_thing("trans", id)),
                ("aabb_d", world_bounds),
                ("world_trans_d", world_transform),
                (
                    "insts_flat",
                    SurValue::Array(SurArray::from(vec![sur_object([(
                        "geo_hash",
                        SurValue::from(mesh_id.to_owned()),
                    )])])),
                ),
                ("model_sesno", source_sesno),
            ]));
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
pub(crate) fn receipt_model_target_literal(receipt: &RootReceipt) -> anyhow::Result<String> {
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
pub(crate) fn render_receipt_with_target(
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

// 多块发布期间盖在 `gen_root` 上的可见性栅栏（`publication_status='publishing'`，P5）随
// 切块一起退役了：一根 / 一组收成一笔事务之后，「行已落、章未盖」这个中间态不再存在，
// 没有东西需要读侧避让。`publication_status` 本身留着——`ready` / `stale` 照旧。

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

    /// 整片删只圈模型面写下的行（`model_sesno != NONE`），属性面独有的行碰不到；
    /// 多根一条 `CONTAINSANY`，不随根数展开成一堆语句。
    #[test]
    fn slice_delete_is_one_predicate_block_guarded_by_the_model_column() {
        assert!(render_slice_delete(&[]).is_empty());
        let sql = render_slice_delete(&[42, 43]);
        assert_eq!(
            sql.matches("model_sesno != NONE").count(),
            2,
            "两张关系表各一道护栏: {sql}"
        );
        assert_eq!(
            sql.matches("anc CONTAINSANY [42,43]").count(),
            2,
            "多根合成一条谓词: {sql}"
        );
        assert!(
            !sql.contains("inst_geo"),
            "内容寻址的共享网格不分区、只增不删（ADR-060 D2）: {sql}"
        );
        // 删的那几句一律点名记录。列上套函数的谓词形态会丢索引退回全表扫，
        // 然后对每一行再去几千项的数组里线性找一遍——AMS 8000 上就是这一句把
        // 一次发布从 13 s 拖到 51 s。
        assert!(
            !sql.contains("DELETE aabb WHERE") && !sql.contains("DELETE trans WHERE"),
            "aabb / trans 必须按记录点删，不许下谓词: {sql}"
        );
        for target in [
            "DELETE $slice_infos;",
            "type::thing('inst_relate', $key)",
            "type::thing('aabb', $key)",
            "type::thing('trans', $key)",
            // 管身的盒与变换按几何摘要另键，只能顺指针删，不能从槽位 id 拼。
            "DELETE $slice_tubes.map(|$row| $row.id);",
            "DELETE $slice_tubes.map(|$row| $row.aabb);",
            "DELETE $slice_tubes.map(|$row| $row.world_trans);",
        ] {
            assert!(sql.contains(target), "{target} 也要跟着整片走: {sql}");
        }
        assert!(
            !sql.contains("type::thing('aabb', record::id(") && !sql.contains("$slice_tube_keys"),
            "管身的 aabb / trans 不能从 tubi_relate 的槽位 id 拼——那不是它们的键: {sql}"
        );
    }

    /// 整片删在库里到底删了什么：六张表里这一片的**全部表示**都要没，别的一行不动。
    ///
    /// 三种行形状各放一条——标准图元（`inst_relate` + `inst_info` + `geo_relate` + 同键
    /// `aabb` / `trans`）、烘焙实体（没有 `out`，只有 `inst_relate` + 同键 `aabb` / `trans`）、
    /// 隐式管身（`tubi_relate:[pe, 槽位]`，盒与变换按几何摘要另键）；再放三样不许动的——
    /// 邻根的行、属性面独有的行（`model_sesno = NONE`）、内容寻址的 `inst_geo` 与它的本地盒。
    /// 管身那一对是这条测试的由头：按槽位 id 拼 `aabb:[pe, n]` 删的是不存在的记录，
    /// 六张表里会剩两行孤儿，只看语句形状看不出来。
    #[tokio::test]
    async fn slice_delete_removes_every_representation_of_the_slice_and_nothing_else() {
        let db = surreal_mem().await;
        let packed = packed_refno(RefNo::new(7997, 10));
        let other = packed_refno(RefNo::new(7997, 20));
        db.query(format!(
            "INSERT INTO aabb [{{id:aabb:c1,d:1}},{{id:aabb:b1,d:1}},{{id:aabb:tube1digest,d:1}},\
                              {{id:aabb:n1,d:1}},{{id:aabb:mesh1,d:1}}];\n\
             INSERT INTO trans [{{id:trans:c1,d:1}},{{id:trans:b1,d:1}},{{id:trans:tube1digest,d:1}},\
                               {{id:trans:n1,d:1}}];\n\
             INSERT INTO inst_geo [{{id:inst_geo:mesh1,aabb:aabb:mesh1}}];\n\
             INSERT INTO inst_info [{{id:inst_info:c1,model_sesno:5}},{{id:inst_info:n1,model_sesno:5}}];\n\
             INSERT RELATION INTO geo_relate [\
                {{id:geo_relate:[inst_info:c1,inst_geo:mesh1],in:inst_info:c1,out:inst_geo:mesh1}},\
                {{id:geo_relate:[inst_info:n1,inst_geo:mesh1],in:inst_info:n1,out:inst_geo:mesh1}}];\n\
             INSERT INTO inst_relate [\
                {{id:inst_relate:c1,in:pe:⟨7997_11⟩,out:inst_info:c1,anc:[{packed}],\
                  aabb:aabb:c1,world_trans:trans:c1,model_sesno:5}},\
                {{id:inst_relate:b1,in:pe:⟨7997_12⟩,booled_id:'mesh1',booled:true,anc:[{packed}],\
                  aabb:aabb:b1,world_trans:trans:b1,model_sesno:5}},\
                {{id:inst_relate:n1,in:pe:⟨7997_21⟩,out:inst_info:n1,anc:[{other}],\
                  aabb:aabb:n1,world_trans:trans:n1,model_sesno:5}},\
                {{id:inst_relate:legacy,in:pe:⟨7997_13⟩,anc:[{packed}],generic:'EQUI'}}];\n\
             INSERT RELATION INTO tubi_relate [\
                {{id:tubi_relate:[pe:⟨7997_14⟩,0],in:pe:⟨7997_14⟩,out:inst_geo:mesh1,\
                  aabb:aabb:tube1digest,world_trans:trans:tube1digest,anc:[{packed}],model_sesno:5}}];"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        db.query(publication_transaction(&render_slice_delete(&[packed])))
            .await
            .unwrap()
            .check()
            .unwrap();

        let survivors: Vec<String> = db
            .query(
                "SELECT VALUE <string>id FROM aabb, trans, inst_geo, inst_info, geo_relate, \
                 inst_relate, tubi_relate;",
            )
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        let survivors = survivors.into_iter().collect::<BTreeSet<_>>();
        let expected = [
            "aabb:n1",
            "aabb:mesh1",
            "trans:n1",
            "inst_geo:mesh1",
            "inst_info:n1",
            "geo_relate:[inst_info:n1, inst_geo:mesh1]",
            "inst_relate:n1",
            "inst_relate:legacy",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(
            survivors, expected,
            "整片删之后只许剩下邻根、属性面独有的行与内容寻址的网格"
        );
    }

    /// 整片删掉之后，本次投影里还在的来源不清房间边（重算自己先清后写），
    /// 只剩下的那些才清。
    #[test]
    fn room_edges_clear_only_the_sources_that_lost_their_geometry() {
        let kept = GeometryId::Element {
            refno: "24383/1".into(),
        };
        let kept_refno = RefNo::new(24383, 1);
        let gone = refno_from_e3d(RefNo::new(24383, 2));
        let cleared = room_edges_to_clear(
            [(&kept, kept_refno, "EQUI")],
            &[refno_from_e3d(kept_refno), gone],
            &BTreeSet::new(),
        );
        assert_eq!(cleared, vec![gone]);
    }

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

    #[tokio::test]
    async fn oversized_local_publication_is_blocked_before_database_io() {
        let mut record = baked_record();
        record.mesh_id = "x".repeat(PUBLICATION_STATEMENT_HARD_CAP + 1);
        let shares = vec![RootShare {
            root: BRANCH,
            records: vec![record],
            receipt: Some(test_receipt(BRANCH, 1).1),
        }];
        let (live, failed) = room_stage(&shares, None);
        assert!(failed.is_empty());
        let db = Surreal::<Any>::init();
        let error = persist_group(&db, &live, true).await.err().unwrap();
        assert_eq!(
            publication_failure_kind(&error),
            PersistFailureKind::Blocked
        );
        assert_eq!(
            publication_failure_kind(&anyhow::anyhow!("unknown database failure")),
            PersistFailureKind::Retryable
        );
    }

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
    /// `ON DUPLICATE KEY UPDATE` 降成了 `INSERT IGNORE`）——一行被手改过之后再次发布
    /// 同一原型，不会被"修回来"，读侧也不消费被改的那几个字段。
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
            let value:Vec<serde_json::Value>=db.query("SELECT <string>id AS id, <string>aabb AS aabb, meshed, visible, bad FROM inst_geo;").await.unwrap().check().unwrap().take(0).unwrap();
            states.push(value);
            db.query("UPDATE inst_geo:⟨123456789⟩ SET visible=false,aabb=NONE;")
                .await
                .unwrap()
                .check()
                .unwrap();
            db.query(render_records(&[tube.clone()], WriteMode::Initial).unwrap())
                .await
                .unwrap()
                .check()
                .unwrap();
            let kept: Vec<serde_json::Value> = db
                .query("SELECT visible, aabb != NONE AS has_aabb FROM inst_geo:⟨123456789⟩;")
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

    /// 值那条路（[`bind_records`]）与文本那条路渲的必须是**同一批行**：表与表序对得上，
    /// 真写进两个库之后逐行逐字段相同。`--bindbench` 量的是这两条路的耗时差，而差值只有在
    /// 「落下去的东西一模一样」这个前提下才读得出意思——这一条钉的就是那个前提。
    #[tokio::test]
    async fn bound_rows_land_exactly_what_the_rendered_text_lands() {
        let records = vec![
            canonical_record(),
            other_canonical_record(),
            nested_root_record(),
            tube_record(0, TUBE_A),
            tube_record(1, TUBE_B),
        ];
        let text = render_records(&records, WriteMode::Initial).expect("render");
        let bound = bind_records(&records).expect("bind");
        let text_tables = text
            .lines()
            .filter_map(|line| {
                let relation = line.starts_with("INSERT RELATION IGNORE INTO ");
                line.strip_prefix("INSERT RELATION IGNORE INTO ")
                    .or_else(|| line.strip_prefix("INSERT IGNORE INTO "))
                    .and_then(|rest| rest.split(' ').next())
                    .map(|table| (table, relation))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            text_tables,
            bound
                .iter()
                .map(|table| (table.table, table.relation))
                .collect::<Vec<_>>(),
            "两条路的表、表序与 RELATION 关键字必须一致"
        );

        let mut dumps = Vec::new();
        for bind in [false, true] {
            let db = surrealdb::engine::any::connect("mem://").await.unwrap();
            db.use_ns("bound_rows")
                .use_db(if bind { "bind" } else { "text" })
                .await
                .unwrap();
            if bind {
                let statements = bound
                    .iter()
                    .enumerate()
                    .map(|(index, table)| table.statement(&format!("p{index}")))
                    .collect::<String>();
                let mut query = db.query(publication_transaction(&statements));
                for (index, table) in bound.iter().enumerate() {
                    query = query.bind((
                        format!("p{index}"),
                        SurValue::Array(SurArray::from(table.rows.clone())),
                    ));
                }
                query.await.unwrap().check().unwrap();
            } else {
                db.query(publication_transaction(&text))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
            let mut tables = Vec::new();
            for table in &bound {
                // 取成 SurrealDB 自己的值（不是 JSON）：两条路的差可能只在类型上
                // （同一个 `0.1` 一边是 Float 一边是 Decimal），JSON 会把它抹平。
                let dumped = db
                    .query(format!("SELECT * FROM {} ORDER BY id;", table.table))
                    .await
                    .unwrap()
                    .check()
                    .unwrap()
                    .take::<surrealdb::Value>(0)
                    .unwrap()
                    .into_inner();
                let SurValue::Array(rows) = dumped else {
                    panic!("{} 的查询结果不是数组", table.table);
                };
                assert!(!rows.0.is_empty(), "{} 一行都没落下去", table.table);
                tables.push(rows.0);
            }
            dumps.push(tables);
        }
        for (table, (left, right)) in bound.iter().zip(dumps[0].iter().zip(&dumps[1])) {
            assert_eq!(left.len(), right.len(), "{} 行数不一致", table.table);
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                assert_eq!(
                    left, right,
                    "{} 第 {index} 行：文本与参数两条路落下去的不是同一行",
                    table.table
                );
            }
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
             INSERT IGNORE INTO inst_relate [{{id:type::thing('inst_relate','24383_73948'),in:type::thing('pe','24383_73948'),booled_id:'e3d_baked_v2_deadbeef',booled:true,bad_bool:false,solid:true,generic:'EQUI',dbnum:7997,anc:[104724187652316,104724187578369],aabb:type::thing('aabb','24383_73948'),world_trans:type::thing('trans','24383_73948'),aabb_d:{{mins:[0.0,0.0,0.0],maxs:[1.0,2.0,3.0]}},world_trans_d:{world},insts_flat:[{{geo_hash:'e3d_baked_v2_deadbeef'}}],model_sesno:42}}];\n"
        );
        assert_eq!(sql, expected.replace("];\n", "] RETURN NONE;\n"));
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
            "{id:inst_geo:⟨123456789⟩,meshed:true,visible:true,bad:false,aabb:type::thing('aabb','123456789')}"
        ));
        assert!(sql.contains(&format!(
            "{{id:geo_relate:[type::thing('inst_info','24383_73948'),inst_geo:⟨123456789⟩],in:type::thing('inst_info','24383_73948'),out:inst_geo:⟨123456789⟩,geom_refno:type::thing('pe','24383_73948'),transform:{local},visible:true,geo_type:'Pos'}}"
        )));
        assert!(sql.contains(&format!(
            "insts_flat:[{{geo_hash:'123456789',transform:{local}}}]"
        )));
        assert!(sql.contains("booled_id:NONE,booled:false"));
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

    /// 行 id 一律就是记录 id。这一条盯的是渲染出来的整段正文：五种旧前缀一个都不许出现。
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
                "SELECT anc,dbnum,model_sesno != NONE AS structured FROM inst_relate WHERE anc CONTAINS {};",
                packed_refno(requested_root)
            ))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(probe.len(), 1, "root lookup fixture: {probe:?}");

        let store = ModelMemoryStore::default();
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
    }

    #[tokio::test]
    async fn a_missing_durable_root_is_not_negative_cached() {
        let db = surreal_mem().await;
        let store = ModelMemoryStore::default();
        let root = RefNo::new(7997, 999);
        assert!(!ensure_root_cached_on(&db, &store, root).await.unwrap());
        assert!(!ensure_root_cached_on(&db, &store, root).await.unwrap());
        assert_eq!(store.current_counts(), (0, 0));
    }

    /// 一块删除语句逐句过：带谓词的 EXPLAIN 必须落在对应索引上、计划里不许出现
    /// `Iterate Table`；其余一律是 `LET` 或 `DELETE $数组` 直接点名，一个 `WHERE` 都不许有。
    /// 返回带谓词的那几条，调用方钉数量——语句多一条少一条都要露出来，免得下次往块里加一句
    /// 谓词删、没人来 EXPLAIN 它。
    ///
    /// 谓词里引用的 `$参数` 照生产的样子先 `LET` 再用（值从 `bindings` 里取），测的是
    /// 「参数数组 + IN」「参数记录 + =」这些真实形状，不是内联字面量。计划里带记录 id
    /// （`geo_relate` 那句的 `value` 是一串 `inst_info:…`）过不了 `serde_json::Value`，
    /// 所以按 `surrealdb::Value` 取、用它的 Display 当文本比对。
    async fn assert_delete_block_never_iterates_a_table(
        db: &Surreal<Any>,
        sql: &str,
        bindings: &[(&str, &str)],
    ) -> Vec<String> {
        // 按「读哪张表、按哪一列圈」定该落的索引：`anc CONTAINS…` 走 `idx_<表>_anc`，
        // `in = …` / `in IN …` 走 `idx_<表>_in`——两组都在 `INST_RELATE_INDEX_SQL` 里。
        fn expected_index(select: &str) -> String {
            let table = select
                .split(" FROM ")
                .nth(1)
                .and_then(|rest| rest.split(' ').next())
                .unwrap_or_else(|| panic!("delete block reads no table: {select}"));
            assert!(
                matches!(table, "inst_relate" | "tubi_relate" | "geo_relate"),
                "delete block reads an unexpected table {table}: {select}"
            );
            let (_, predicate) = select
                .split_once(" WHERE ")
                .unwrap_or_else(|| panic!("a predicated read without WHERE: {select}"));
            if predicate.starts_with("in = ") || predicate.starts_with("in IN ") {
                format!("idx_{table}_in")
            } else if predicate.starts_with("anc CONTAINS") {
                format!("idx_{table}_anc")
            } else {
                panic!("a predicated read that no index answers: {select}")
            }
        }

        async fn explain(db: &Surreal<Any>, select: &str, bindings: &[(&str, &str)]) -> String {
            let prelude = bindings
                .iter()
                .filter(|(name, _)| select.contains(name))
                .map(|(name, value)| format!("LET {name} = {value};\n"))
                .collect::<Vec<_>>();
            let sql = format!("{}{select} EXPLAIN FULL;", prelude.concat());
            let mut response = db.query(sql).await.unwrap().check().unwrap();
            let plan: surrealdb::Value = response.take(prelude.len()).unwrap();
            plan.to_string()
        }

        let mut predicated = Vec::new();
        for statement in sql
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.trim_end_matches(';'))
        {
            let select = if let Some((_, select)) = statement.split_once(" = SELECT ") {
                Some(format!("SELECT {select}"))
            } else {
                statement
                    .strip_prefix("DELETE (")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .map(str::to_owned)
            };
            match select {
                Some(select) => {
                    let plan = explain(db, &select, bindings).await;
                    let index = expected_index(&select);
                    assert!(
                        !plan.contains("Iterate Table") && plan.contains(&index),
                        "{statement} must be planned through {index}, plan: {plan}"
                    );
                    predicated.push(statement.to_owned());
                }
                None => {
                    assert!(
                        statement.starts_with("LET $") || statement.starts_with("DELETE $"),
                        "every other statement is a LET or a direct-target DELETE: {statement}"
                    );
                    assert!(
                        !statement.contains(" WHERE ") && !statement.contains("SELECT"),
                        "direct-target statements must not carry a predicate: {statement}"
                    );
                }
            }
        }
        predicated
    }

    /// 整片删的**每一条**语句都不许扫表——它是这次改动的全部依仗：读侧的作用域从「整库」
    /// 收成「这一片」之后，走不走索引决定它是 O(片) 还是 O(库)。回退成 `Iterate Table`
    /// 就等于把刚拆掉的 O(N²) 原样装了回来，只是从「传回客户端」换成「库端扫」。
    ///
    /// 索引用生产那份 [`INST_RELATE_INDEX_SQL`]，测的是启动序列真会建出来的那组。
    /// 带谓词的只许三条：两条 `anc CONTAINSANY` 读与 `geo_relate` 按 `in` 的那句。
    ///
    /// [`INST_RELATE_INDEX_SQL`]: crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL
    #[tokio::test]
    async fn slice_predicates_are_planned_through_ancestor_indexes() {
        let db = surreal_mem().await;
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();

        let sql = render_slice_delete(&[
            packed_refno(RefNo::new(7997, 1)),
            packed_refno(RefNo::new(7997, 2)),
        ]);
        assert_eq!(
            sql.lines().filter(|line| !line.trim().is_empty()).count(),
            11,
            "块的形状变了就来更新这条测试: {sql}"
        );
        let predicated = assert_delete_block_never_iterates_a_table(
            &db,
            &sql,
            &[("$slice_infos", "[inst_info:a, inst_info:b]")],
        )
        .await;
        assert_eq!(
            predicated.len(),
            3,
            "exactly the two anc reads and the geo_relate lookup carry predicates: {predicated:?}"
        );
        assert!(
            predicated[2].contains("FROM geo_relate WHERE in IN $slice_infos"),
            "{predicated:?}"
        );
    }

    /// 镜像 spool / 墓碑那一路的整片删（[`render_spooled_scope_cleanup`]）同一条纪律。
    /// 它的作用域多一路 `in = $mirror_owner`（兜 `anc` 缺失的历史行），必须与 `anc` 那一路
    /// **分开读**各走各的索引再并——写成一条 `anc CONTAINS … OR in = $参数` 时优化器整条退回
    /// `Iterate Table`（改写前这条测试就是这样红的），那是 spool 回放里的整表扫。
    #[tokio::test]
    async fn spooled_cleanup_predicates_are_planned_through_indexes() {
        let db = surreal_mem().await;
        db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
            .await
            .unwrap()
            .check()
            .unwrap();

        let sql = render_spooled_scope_cleanup(RefNo::new(7997, 1));
        assert_eq!(
            sql.lines().filter(|line| !line.trim().is_empty()).count(),
            16,
            "块的形状变了就来更新这条测试: {sql}"
        );
        assert!(
            !sql.contains(" OR "),
            "两路作用域不许合成一条 OR 谓词——优化器会整条退回扫表: {sql}"
        );
        let predicated = assert_delete_block_never_iterates_a_table(
            &db,
            &sql,
            &[
                ("$mirror_owner", "type::thing('pe','7997_1')"),
                ("$mirror_infos", "[inst_info:a, inst_info:b]"),
            ],
        )
        .await;
        assert_eq!(
            predicated.len(),
            5,
            "two anc reads, two owner reads and the geo_relate lookup carry predicates: {predicated:?}"
        );
        assert_eq!(
            predicated
                .iter()
                .filter(|statement| statement.contains("WHERE in = $mirror_owner"))
                .count(),
            2,
            "the spool scope must keep the anc-less fallback by owner on both relation tables: {predicated:?}"
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
        assert!(
            sql.contains("LET $mirror_keys_by_anc = SELECT VALUE record::id(id) FROM inst_relate")
        );
        assert!(sql.contains(
            "LET $mirror_keys = array::union($mirror_keys_by_anc, $mirror_keys_by_owner);"
        ));
        assert!(sql.contains("DELETE $mirror_keys.map(|$key| type::thing('inst_relate', $key));"));
        assert!(
            !sql.contains("DELETE inst_relate WHERE") && !sql.contains("DELETE aabb WHERE"),
            "spool 清理与整片删同一套纪律：点名删，不下谓词: {sql}"
        );
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
    async fn a_spooled_tombstone_deletes_durable_rows_of_the_whole_slice() {
        let db = surreal_mem().await;
        let root = RefNo::new(24383, 1);
        let packed = packed_refno(root);
        db.query(format!(
            "CREATE inst_relate:durable_only SET in=pe:durable_only, anc=[{packed}], \
             out=inst_info:durable_only, model_sesno=42;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let tombstone = render_spooled_scope_deletion(root, &[root], &[]);
        assert!(tombstone.contains("DELETE $mirror_tubes.map(|$row| $row.id);"));
        db.query(tombstone).await.unwrap().check().unwrap();

        assert_eq!(count(&db, "inst_relate").await, 0);
    }

    /// 墓碑那一路与整片删同一套点名删：三种行形状（标准图元、无 `out` 的烘焙行、按摘要另键
    /// 盒与变换的管身）连同 `anc` 缺失、只靠 `in = 根` 兜住的历史行一起没；邻根、属性面独有
    /// 的行与内容寻址的网格不动。
    #[tokio::test]
    async fn a_spooled_tombstone_removes_every_representation_of_the_root_and_nothing_else() {
        let db = surreal_mem().await;
        let root = RefNo::new(7997, 10);
        let packed = packed_refno(root);
        let other = packed_refno(RefNo::new(7997, 20));
        db.query(format!(
            "INSERT INTO aabb [{{id:aabb:c1,d:1}},{{id:aabb:b1,d:1}},{{id:aabb:tube1digest,d:1}},\
                              {{id:aabb:own,d:1}},{{id:aabb:n1,d:1}},{{id:aabb:mesh1,d:1}}];\n\
             INSERT INTO trans [{{id:trans:c1,d:1}},{{id:trans:b1,d:1}},{{id:trans:tube1digest,d:1}},\
                               {{id:trans:own,d:1}},{{id:trans:n1,d:1}}];\n\
             INSERT INTO inst_geo [{{id:inst_geo:mesh1,aabb:aabb:mesh1}}];\n\
             INSERT INTO inst_info [{{id:inst_info:c1,model_sesno:5}},{{id:inst_info:n1,model_sesno:5}}];\n\
             INSERT RELATION INTO geo_relate [\
                {{id:geo_relate:[inst_info:c1,inst_geo:mesh1],in:inst_info:c1,out:inst_geo:mesh1}},\
                {{id:geo_relate:[inst_info:n1,inst_geo:mesh1],in:inst_info:n1,out:inst_geo:mesh1}}];\n\
             INSERT INTO inst_relate [\
                {{id:inst_relate:c1,in:pe:⟨7997_11⟩,out:inst_info:c1,anc:[{packed}],\
                  aabb:aabb:c1,world_trans:trans:c1,model_sesno:5}},\
                {{id:inst_relate:b1,in:pe:⟨7997_12⟩,booled_id:'mesh1',booled:true,anc:[{packed}],\
                  aabb:aabb:b1,world_trans:trans:b1,model_sesno:5}},\
                {{id:inst_relate:own,in:pe:⟨7997_10⟩,booled_id:'mesh1',booled:true,\
                  aabb:aabb:own,world_trans:trans:own,model_sesno:5}},\
                {{id:inst_relate:n1,in:pe:⟨7997_21⟩,out:inst_info:n1,anc:[{other}],\
                  aabb:aabb:n1,world_trans:trans:n1,model_sesno:5}},\
                {{id:inst_relate:legacy,in:pe:⟨7997_13⟩,anc:[{packed}],generic:'EQUI'}}];\n\
             INSERT RELATION INTO tubi_relate [\
                {{id:tubi_relate:[pe:⟨7997_14⟩,0],in:pe:⟨7997_14⟩,out:inst_geo:mesh1,\
                  aabb:aabb:tube1digest,world_trans:trans:tube1digest,anc:[{packed}],model_sesno:5}}];"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        db.query(render_spooled_scope_deletion(root, &[root], &[]))
            .await
            .unwrap()
            .check()
            .unwrap();

        let survivors: Vec<String> = db
            .query(
                "SELECT VALUE <string>id FROM aabb, trans, inst_geo, inst_info, geo_relate, \
                 inst_relate, tubi_relate;",
            )
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        let survivors = survivors.into_iter().collect::<BTreeSet<_>>();
        let expected = [
            "aabb:n1",
            "aabb:mesh1",
            "trans:n1",
            "inst_geo:mesh1",
            "inst_info:n1",
            "geo_relate:[inst_info:n1, inst_geo:mesh1]",
            "inst_relate:n1",
            "inst_relate:legacy",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(
            survivors, expected,
            "墓碑之后只许剩下邻根、属性面独有的行与内容寻址的网格"
        );
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
        assert_eq!(
            second.removed, 1,
            "掉了几何的来源只有陈旧那一件；管身那一段的来源 BRAN 自己还有几何，不算"
        );
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            0,
            "上一轮的管身行跟着整片一起没"
        );
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

    /// ADR-078 D3 / A1：回执之下的记录**可以停在更早的会话**，比回执新的仍然是错。
    ///
    /// 单元级发布（ADR-078）只重建变了的那几件，没动的记录留在上一次真正生成它的会话上，
    /// 而回执整根前移到 T。这一条钉住那道对账从 `==` 放宽成 `<=` 之后的两端。两次都逐字核
    /// 错误文本：陈旧那次若哪天又被拦下，错误会落在「incomplete」而不是「outside its receipt
    /// scope」上，只判 `is_err()` 看不出差别。
    #[tokio::test]
    async fn a_record_may_lag_its_root_receipt_but_never_lead_it() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        let target = PersistTarget::new(&db, PersistMedium::Mem);

        // 陈旧：回执在 42，没动的那件还停在 41——单元级发布之后的常态，必须放行。
        let lagging = ModelMemoryStore::default();
        lagging.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    baked_record(),
                    ProjectedGeometry {
                        source_sesno: 41,
                        ..nested_root_record()
                    },
                ],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &lagging, root)
            .await
            .expect("落后于回执的记录是单元级发布的正常产物");
        assert_eq!(count(&db, "inst_relate").await, 2);

        // 超前：记录比回执新 = 有人把几何投进来却没盖章，这根的全集不再是回执说的那一份。
        let leading = ModelMemoryStore::default();
        leading.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    baked_record(),
                    ProjectedGeometry {
                        source_sesno: 43,
                        ..nested_root_record()
                    },
                ],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let error = persist_root_on(target, &leading, root)
            .await
            .expect_err("记录比回执新必须拦下")
            .to_string();
        assert!(
            error.contains("outside its receipt scope"),
            "拦是拦下了，但理由不对：{error}"
        );
        assert_eq!(count(&db, "inst_relate").await, 2, "被拦下的那轮不得改库");
    }

    /// 一根一笔事务：三条记录（一件几何 + 同一分支的两段管身）与回执一起进同一笔，
    /// 分支重置排在插入之前、管身槽位在这一笔里连号。切块退役之前这三条是靠「重置只进
    /// 第一块、发号表跨块延续、回执在最后一块」维持的，现在由事务本身维持。
    #[tokio::test]
    async fn one_root_is_one_transaction_with_its_tubes_and_receipt() {
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
        let rows = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(rows.upserted, 3);
        assert_eq!(rows.receipts, 1);
        assert_eq!(rows.commits, 1, "一根就是一笔事务，多少条记录都不拆");

        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            2,
            "同分支两段共用一张发号表，后一段不能盖掉前一段"
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
        assert_eq!(
            rows.removed, 1,
            "掉了几何的来源只有陈旧那一件；管身那一段的来源 BRAN 自己还有几何，不算"
        );
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            0,
            "上一轮的管身行跟着整片一起没"
        );
        assert_eq!(publication_status_of(&db, root).await, "ready");
    }

    /// 连落两批：第二批把上一批的一件几何从投影里拿掉，库里那一行必须跟着消失——
    /// **而且整个过程一次对账读都没有**。
    ///
    /// 旧写法靠「整库对账读 + 任务级快照」找出这一行，2026-09-10 那次 O(N²) 修复
    /// （7997 实测 68 组累计读 180 万行、单笔事务从 400 ms 涨到 1,280 ms）就是在给那个读
    /// 做性能抢救。现在整片删掉再整片插回，陈旧行没有机会存活，那个读也就不必存在。
    #[tokio::test]
    async fn a_second_batch_clears_the_rows_the_projection_dropped() {
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
        let target = PersistTarget::new(&db, PersistMedium::Mem);

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
        persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("second batch");
        assert_eq!(
            count(&db, "inst_relate").await,
            1,
            "投影里没有的行跟着整片一起消失，不需要先读出来点名"
        );
        assert_eq!(count(&db, "tubi_relate").await, 0);
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
        // 9999 掉了几何；夹具管身的来源 BRAN 自己还有几何，不算进这个数
        //（与 `batched_persist_reconciles_stale_rows_from_one_database_read` 同一口径）。
        assert_eq!(second.rows.expect("rows").removed, 1, "9999 掉了几何");
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
            .split_once("async fn persist_roots_batched_unacked_impl(")
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
            .split_once("async fn persist_root_impl(")
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

    /// 合块那一路同样是一笔：几何、房间边和回执不许分开提交。
    #[tokio::test]
    async fn batched_persist_keeps_one_root_in_one_transaction() {
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
        let batched = persist_roots_batched(target, &store, BATCH_DBNUM, &[root], None)
            .await
            .expect("batched persist");
        assert!(batched.failed.is_empty(), "{:?}", batched.failed);
        let rows = batched.rows.expect("rows");
        assert_eq!(rows.upserted, 3);
        assert_eq!(rows.commits, 1, "一组就是一笔事务");
        assert_eq!(count(&db, "inst_relate").await, 1);
        assert_eq!(
            count(&db, "tubi_relate").await,
            2,
            "同分支两段共用一张发号表，后一段不能盖掉前一段"
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

    /// 一根一笔事务之后就没有「写了一半」这回事了：中途失败 = 一行都没落、凭证一个字
    /// 没动。此前这里要靠 `publication_status='publishing'` 把中间态标出来给读侧避让，
    /// 现在中间态根本不存在，栅栏跟着切块一起退役。
    #[tokio::test]
    async fn a_failed_publication_leaves_neither_rows_nor_a_touched_credential() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        seed_gen_root(&db).await;
        let root = RefNo::new(24383, 1);

        // 这一笔里有一条会被库拒收，整笔事务因此回滚。
        db.query("DEFINE EVENT reject_row ON TABLE inst_relate WHEN $after.generic = 'REJECT' THEN { THROW 'injected publication failure'; };").await.unwrap().check().unwrap();
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
        assert!(persist_root_on(target, &store, root).await.is_err());
        assert_eq!(
            publication_status(&db).await,
            "stale",
            "失败的那一笔不许在凭证行上留下任何痕迹"
        );
        assert_eq!(count(&db, "inst_relate").await, 0, "整笔回滚，一行都不留");

        // 换成两条都渲染得出来的记录重跑，同一笔事务里盖章。
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), other_canonical_record()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(publication_status(&db).await, "ready");
    }

    /// 没有回执的落库一个字都不许动凭证行。
    #[tokio::test]
    async fn a_publish_without_a_receipt_never_touches_the_credential() {
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
        persist_root_on(target, &store, root)
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
                data: None,
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
    /// 自己的几何——一根按条数切成几块时跟着它的**末块**走，落在同一笔事务里，所以「这一根的模型行
    /// 已提交、它的房间边稍后单独提交」这条路在源码里不存在。落库路径不回读库、不走整库房间算法
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
            .split_once("async fn persist_roots_batched_unacked_impl(")
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
        // 根内：几何按条数切块渲好 → 边拼进这一根的末块 → 回执进收尾段 → 逐笔提交。
        let group = production
            .split_once("async fn persist_group(")
            .expect("persist_group exists")
            .1
            .split_once("/// The WS router can retain a pending response until reconnection.")
            .expect("publication request helpers follow")
            .0;
        let render_at = group
            .find("render_record_chunks(&live.claimed, mode, &mut tube_slots)")
            .expect("几何段按条数切块渲染");
        let attach_at = group
            .find("sql.push_str(&live.room_statements)")
            .expect("房间边拼进这一根的末块");
        let receipt_at = group
            .find("let epilogue = format!(\"{receipt_header}{receipt_body}\")")
            .expect("回执进收尾段");
        // 提交走 `commit_publication_transaction`（整笔原子 + 冲突重试），钉它的调用处。
        let commit_at = group
            .find("commit_publication_transaction(db, statements")
            .expect("提交");
        assert!(
            render_at < attach_at && attach_at < receipt_at && receipt_at < commit_at,
            "顺序：渲几何 → 房间边 → 回执 → 提交"
        );
        // 装块纪律：删在第一笔、章在最后一笔。反过来就是「章盖了、行还没写完」，
        // 中途崩掉的那一根会被当成已发布，再也没人回来补。
        let packer = production
            .split_once("fn pack_publication_transactions(")
            .expect("packer exists")
            .1
            .split_once("\n}\n")
            .expect("packer end")
            .0;
        let prelude_at = packer
            .find("String::from(prelude)")
            .expect("prelude 起头第一笔");
        let epilogue_at = packer
            .find("current.push_str(epilogue)")
            .expect("epilogue 收尾最后一笔");
        assert!(
            prelude_at < epilogue_at,
            "整片删进第一笔、盖章进最后一笔，顺序不能对调"
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
