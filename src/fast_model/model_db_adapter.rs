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
//! # 回执不落库（ADR-079）
//!
//! `gen_root` 表退役：一根落库成功之后没有任何一行章要盖，「这根这一版成了」记进进程内账本
//! （`model_root_ledger`），库里模型面唯一的持久事实是 `dbnum_watermark.model_sesno`。
//! 「同一根不许被两个源会话混着发布」由同一 dbnum 只有本任务一个写者与 `generation_root_lock` 守着。
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
use crate::fast_model::e3d_model_service::{f32_values, geometry_source_refno, transform_value};
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
    /// 提交之后要不要把逐根成败记进进程内的生成根账本（ADR-079）。
    ///
    /// 账本与空间树同一条纪律：它镜像的是**全局那个库**的模型面，只有写全局库的目标才该记；
    /// 自带句柄的目标（测试、旁路库）写的是别处，记进账本就是拿别的库的成败冒充本进程的。
    record_ledger: bool,
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
            record_ledger: true,
        }
    }

    /// 自带句柄的目标，介质由调用方说明。测试与将来独立开库的落库路径走这条。
    pub fn new(db: &Surreal<Any>, medium: PersistMedium) -> PersistTarget<'_> {
        PersistTarget {
            db,
            medium,
            converge_spatial: false,
            record_ledger: false,
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

    /// 这个目标写的那个库的句柄（反向传播的反查要拿它读 `tubi_relate` / `inst_relate.anc`，与随后写边的是同一个库）。
    pub fn db(&self) -> &Surreal<Any> {
        self.db
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
    /// 房间边直接用回执带来的（bake 时算好，B/T4）的根数。
    pub room_carried: usize,
    /// 房间边在 flush 时用投影记录现算（回读 `.mesh`）的根数——回执没带 / 代号不等 / 构件集不等。
    pub room_recomputed: usize,
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
    /// 这一根的边是从哪来的（回执带来的 / 这里现算的 / 没算）。
    room_source: RoomEdgeSource,
}

/// 一根在 [`room_stage`] 里房间边的来源（B/T4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoomEdgeSource {
    /// bake 时算好、随回执带来的（`RootReceipt::room`），三道闸都过了，这里只渲染。
    Carried,
    /// 这里用投影记录现算（回执没带 / 代号不等 / 构件集不等）——回读 `.mesh` 取点。
    Recomputed,
    /// 没有计算器（房间开关关 / 底层适配器直调）或这一根没有几何：不算也不写新边。
    NotComputed,
}

/// 回执带来的边能不能直接用（B/T4 三道闸，纯函数）：`carried.generation` 与此刻计算器代号相等；
/// `carried.elements` 与本根记录的构件集**相等**（少一个多一个都不行——渲染对 `claimed` 的每个构件先删后插，
/// 拿只覆盖部分构件的边去渲，没动的构件的边会被删掉而不补；嵌套根共享记录、局部发布后的整根形状都在
/// 这里被挡下）；都过了 → `Some(边)`，否则 `None`，调用方现算。
fn carried_edges_for<G: crate::fast_model::room_model::RoomInput>(
    carried: Option<&crate::fast_model::model_memory_store::CarriedRoomEdges>,
    resident_generation: &str,
    claimed: &[G],
) -> Option<Vec<crate::fast_model::room_model::ElementRoomEdge>> {
    let carried = carried?;
    if carried.generation != resident_generation {
        return None;
    }
    let elements: BTreeSet<String> = claimed
        .iter()
        .map(|record| record.element().to_string())
        .collect();
    let listed: BTreeSet<&str> = carried.elements.iter().map(String::as_str).collect();
    if elements.len() != listed.len() || !elements.iter().all(|e| listed.contains(e.as_str())) {
        return None;
    }
    Some(
        carried
            .edges
            .iter()
            .map(crate::fast_model::room_model::ElementRoomEdge::from_carried)
            .collect(),
    )
}

/// 房间关卡（ADR-073 D1 / D2）：写库**之前**逐根拿到房间归属——输入只有该根的内存投影、任务开工时
/// 冻住的计算器与回执带来的边，一句库不读。回执带了边且三道闸都过（[`carried_edges_for`]）就只渲染
/// （B/T4：bake 时已算，不再回读 `.mesh`）；否则用投影记录现算。算不出来（盒子不可用、面板网格不全）的
/// 根在这里就剔出本组、带原因记进失败名单，房间失败 = 该根失败；其余根照常往下走。与组大小无关：
/// 一组一根或一组几百根都是同一条路。`rooms` 为 `None`（房间开关关闭，或显式的底层适配器调用）时不算
/// 也不写新边，模型根仍往下持久化，只由对账那一半清掉被删来源的旧边。
fn room_stage<'a>(
    group: &'a [RootShare],
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> (Vec<LiveShare<'a>>, Vec<(RefNo, String)>) {
    let mut live = Vec::with_capacity(group.len());
    let mut failed = Vec::new();
    for share in group {
        // 每根事务携带自己的全集。祖先根失败后，随后嵌套根仍能独立写下共享记录与房间边。
        let claimed = share.records.clone();
        let (room_statements, room_edges, room_source) = match rooms {
            Some(calculator) if !claimed.is_empty() => {
                let carried = carried_edges_for(
                    share
                        .receipt
                        .as_ref()
                        .and_then(|receipt| receipt.room.as_ref()),
                    calculator.resident_generation(),
                    &claimed,
                );
                let (edges, source) = match carried {
                    Some(edges) => (edges, RoomEdgeSource::Carried),
                    None => match calculator.calculate_for_root(&claimed) {
                        Ok(edges) => (edges, RoomEdgeSource::Recomputed),
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
                    },
                };
                (
                    calculator.render_root_room_edges(&claimed, &edges),
                    edges.len(),
                    source,
                )
            }
            _ => (String::new(), 0, RoomEdgeSource::NotComputed),
        };
        live.push(LiveShare {
            share,
            claimed,
            room_statements,
            room_edges,
            room_source,
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
/// 在建（`Loading`）或建模本体失败（`Blocked`）时是 `Err`——这一轮的根都算不出房间，调用方按
/// 「房间失败 = 该根失败」记进 `failed`，下一次任务重生成。
///
/// **`Degraded` 照常冻**（ADR-073 D8–D10，2026-09-16）：模型在、带着已点名的缺口——缺几何的在册
/// 面板不进索引、逐根写边时是保护集（[`MemoryRoomCalculator::missing_panels`]），缺文件的 DESI 不在册；
/// 计算器里没有整体性的「输入齐不齐」判定。此前一块面板缺几何 / 一个 DESI 缺文件就在这里 `Err`，
/// 整批根「room calculator unavailable」判死（0.1.25 现场）。
///
/// 这里不回读库、不建模型、不换代：[`current_for_calculation`] 只取启动段装好的那份 `Arc`，
/// 计算器把面板索引建在内存里；任务期间常驻模型换代与否与这份引用无关。
///
/// [`MemoryRoomCalculator::missing_panels`]: crate::fast_model::room_model::MemoryRoomCalculator::missing_panels
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

/// **一任务一台**（ADR-073 D1「任务期间不换代」的字面形态，2026-09-17 B/T1）：dbnum 任务的执行体在
/// 开工前对齐（D12）之后冻**一次**，显式传给单元工作单 → 生成 → bake，整个任务的每一根都用它；
/// 此前是 write-behind 每次 flush 各冻一台，任务中途常驻换代（`/model/ensure` 并发对齐）就会让同一任务
/// 各批根的边不是同一代算的（P2-3 为此打的补丁从此只是富余安全网）。三态各自的处置：
///
/// - [`Self::Disabled`]：房间开关关着——模型照常生成、不写新边（与 [`frozen_room_calculator`] 的 `Ok(None)` 同义）。
/// - [`Self::Frozen`]：冻好的那台，`Arc` 让并行 bake 只读共享；`resident_generation()` 就是本任务的房间代号。
/// - [`Self::Unavailable`]：冻不出（常驻 `Loading` / `Blocked`）——本任务每一根的房间都算不出来，
///   D2「房间失败 = 该根失败」在**开工时**就定了：单元工作单入口把全部单元记 Failed、账本记同一句原因，
///   不再白跑几何等到 flush 才判死。
#[derive(Clone)]
pub enum TaskRoomCalculator {
    Disabled,
    Frozen(std::sync::Arc<crate::fast_model::room_model::MemoryRoomCalculator>),
    Unavailable(String),
}

impl TaskRoomCalculator {
    /// 冻本任务那一台：[`frozen_room_calculator`] 的三态形。开工前对齐**之后**调（钉 15）。
    pub fn freeze() -> Self {
        match frozen_room_calculator() {
            Ok(None) => Self::Disabled,
            Ok(Some(calculator)) => Self::Frozen(std::sync::Arc::new(calculator)),
            Err(error) => Self::Unavailable(format!(
                "room calculator unavailable for this task: {error:#}"
            )),
        }
    }

    /// 冻好的那台（`Disabled` / `Unavailable` 都是 `None`）。
    pub fn frozen(
        &self,
    ) -> Option<&std::sync::Arc<crate::fast_model::room_model::MemoryRoomCalculator>> {
        match self {
            Self::Frozen(calculator) => Some(calculator),
            Self::Disabled | Self::Unavailable(_) => None,
        }
    }

    /// 冻不出时的那句原因；能算（或不用算）是 `None`。
    pub fn unavailable(&self) -> Option<&str> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            Self::Disabled | Self::Frozen(_) => None,
        }
    }

    /// 本任务的房间代号 = 冻好的那台的 `resident_generation()`；`Disabled` / `Unavailable` 没有代号。
    /// 第三条触发（D13 ③）与定向准入（`room_recompute::durable_alignment` / `scope_for_task`）的「此刻常驻代号」从
    /// 这里取，不再另读一次生命周期（2026-09-19 H1）：准入链的终点与收尾定向重算用的计算器必须是同一台。
    pub fn generation(&self) -> Option<&str> {
        self.frozen().map(|rooms| rooms.resident_generation())
    }
}

impl std::fmt::Debug for TaskRoomCalculator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("Disabled"),
            Self::Frozen(calculator) => f
                .debug_tuple("Frozen")
                .field(&calculator.resident_generation())
                .finish(),
            Self::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
        }
    }
}

/// 一次整库房间边重算的账（[`recompute_dbnum_room_edges`]）。
#[derive(Debug, Clone, Default)]
pub struct RoomEdgeRecompute {
    /// 这一轮覆盖到的根数（= `covered_roots.len()` = `projected_roots + durable_roots`）。
    pub roots: usize,
    /// 这一轮**覆盖到**的根，按 refno 序：投影里有回执的根 + 调用方点名、不在投影里而从库上读回几何的根。
    /// 投影只装本进程生成过的根（初始化的模型行不进投影，ADR-077 D5；落盘形态冷重启后是空的），
    /// 不在投影里的那些由调用方拿 `DbSet@T` 的根清单点名（`library_roots`），这里回读库上的模型行补算；
    /// 调用方再拿它对 `DbSet@T` 自证覆盖面（`RoomRecomputeVerdict`）。
    pub covered_roots: Vec<RefNo>,
    /// 投影里有回执的根数。
    pub projected_roots: usize,
    /// 点名了、不在投影里、从库上读回几何的根数（含读回来一行几何都没有的）。
    pub durable_roots: usize,
    /// 从库上读回的几何条数（`inst_relate` / `tubi_relate` 行里的实例）。
    pub durable_records: usize,
    /// 回读库上几何花的时间（含在 `elapsed_ms` 里）。
    pub durable_read_ms: u128,
    /// 真重算并重写了边的根数（有几何的、算得出的；两种来源合计）。
    pub recomputed: usize,
    /// 重写出的成员边数（`room_relate`）。
    pub room_edges: usize,
    pub commits: usize,
    pub max_transaction_bytes: usize,
    /// 算不出归属的根与原因：边原样留着（陈旧可恢复），调用方按 D2 记失败。
    pub failed: Vec<(RefNo, String)>,
    /// 房间代号对齐到本台计算器的 ACK 数。
    pub acks_refreshed: usize,
    /// 面板级处置语句数（[`RootScope::Targeted`] 的 `panel_statements`：不在册清边 / 只改号改列），进第一笔。
    pub panel_statements: usize,
    pub elapsed_ms: u128,
}

/// [`recompute_dbnum_room_edges`] 的根范围（ADR-073 D13）。
#[derive(Debug, Clone, Copy)]
pub enum RootScope<'a> {
    /// 整库：投影里本库有回执的根 + `library_roots` 里不在投影里的根（回读库上几何）。
    All,
    /// 反向传播命中的根（`room_reverse::AffectedRoots::roots`，2026-09-17）：投影里的按集合筛、集合里不在投影里的
    /// 回读库；`library_roots` 不看（命中的根本就来自它）。`panel_statements` 是面板级的两类处置
    /// （[`render_panel_delta_statements`]：不在册清边 / 只改号改列），作 prelude 进第一笔事务——删在第一笔，
    /// 与整根发布的装块纪律同一条。
    Targeted {
        roots: &'a [RefNo],
        panel_statements: &'a str,
    },
}

/// 面板级的两类处置渲成语句（ADR-073 D13 反查，2026-09-17）：不在册的面板清它全部出边（它名下的成员，
/// `{panel}->room_relate`）与指向它的 `room_panel_relate` 入边；只改房间号的房间对交集里的面板改列
/// （`room_resident::render_renumber_statements`）。**不查空间、不算几何**——这两类与成员的几何无关。
/// 每句以 `;\n` 收尾，直接当 [`RootScope::Targeted::panel_statements`]。要反查的那一类（`recalc`）不在这里：
/// 它们的处置是命中根的逐根重算。
pub(crate) fn render_panel_delta_statements(
    delta: &crate::fast_model::room_resident::PanelDelta,
) -> String {
    let mut out = String::new();
    // 两个渲染器一个带句尾分号一个不带；这里统一成「一句一个 `;\n`」。
    let mut push = |statement: &str| {
        out.push_str(statement.trim_end().trim_end_matches(';'));
        out.push_str(";\n");
    };
    for panel in &delta.removed {
        for statement in crate::fast_model::room_model::render_unregistered_panel_statements(*panel)
        {
            push(&statement);
        }
    }
    for renumber in &delta.renumbered {
        for statement in crate::fast_model::room_resident::render_renumber_statements(renumber) {
            push(&statement);
        }
    }
    out
}

/// 用本库全部根的**现有几何**重算两张房间边（`RoomOnly`，d-819 / ADR-073 D13）。
///
/// 不重生几何、不动模型行、不动空间树：每个有回执的根拿它在投影里的记录
/// → [`MemoryRoomCalculator::calculate_for_root`] → [`MemoryRoomCalculator::render_root_room_edges`]
/// （按构件先删后插，缺几何面板的边受保护，D9），按 [`PERSIST_CHUNK`] 装块、一串事务提交
/// ——与逐根路径同一套渲染，所以「整库重算 == 从零初始化」逐边相等的硬验收（ADR-010 §9）
/// 不多一条口径。没有几何的根跳过（它的边由对账删除那一半清过了）；算不出的根边原样留、
/// 进 `failed`，同批其余根照常。提交全过后把这些根 ACK 的 `room_generation` 对齐到本台计算器
/// （只改这一格，几何身份不动），`e3d.room.lookup` 对它们重新回 `ready`。
///
/// **不在投影里的根回读库上几何**（U1 第二半，2026-09-17）：投影只装本进程生成过的根——初始化的模型行
/// 不进投影（ADR-077 D5）、落盘形态冷重启后投影是空的——`library_roots` 是调用方拿 `DbSet@T` 枚举出的
/// 本库全部根（`None` = 枚举不出来，只算投影）；其中投影里没有回执的，逐根经
/// `model_record_reader::durable_records_of_root` 读回库上的 `inst_relate` / `tubi_relate` 行、折成
/// `room_model::durable_room_inputs`，走**同一个** `calculate_for_root` / `render_root_room_edges`：
/// 语句形状与逐根落库逐字相同。读回一行几何都没有的根（从没生成过 / 没有几何）与投影里没记录的根同一
/// 处置——跳过、算覆盖到（它没有边可陈旧）。这些根没有进程内 ACK，它们的就绪由 durable 侧 D14 那一格
/// 作答（调用方按 `WholeLibrary` 盖上）。回读一根失败（库读不出）整轮 `Err`，与写库失败同一处置。
///
/// 调用时机归调用方守：本批根**刷盘之后**（durable ACK 齐了）、任务收尾之前——本任务的根都已
/// 落库；同库别的根（`/model/ensure` 按需生成的）可能仍在 write-behind 队列里，两边对同一构件都是
/// 先删后插、同一台计算器下幂等，后写的那份与先写的逐字相同。回读库上几何的那些根同理：读的是
/// 此刻已落库的行。
///
/// 回执里的 [`RoomEdgeRecompute::covered_roots`] 就是这一轮算过的全部根（投影里的 + 点名回读的）；它是不是
/// 本库的全部根，仍由调用方对 `DbSet@T` 的根清单判——这里不枚举文件。
///
/// **定向**（[`RootScope::Targeted`]，反向传播，2026-09-17）：根集换成反查命中的那几根——投影里的按集合筛、
/// 集合里不在投影里的回读库——其余一字不改：同一个判定入口、同一套渲染、同一种装块，多的只是第一笔事务前面
/// 的面板级处置语句（不在册清边 / 只改号改列）。
///
/// [`MemoryRoomCalculator::calculate_for_root`]: crate::fast_model::room_model::MemoryRoomCalculator::calculate_for_root
/// [`MemoryRoomCalculator::render_root_room_edges`]: crate::fast_model::room_model::MemoryRoomCalculator::render_root_room_edges
pub async fn recompute_dbnum_room_edges(
    target: PersistTarget<'_>,
    store: &ModelMemoryStore,
    dbnum: u32,
    rooms: &crate::fast_model::room_model::MemoryRoomCalculator,
    library_roots: Option<&[RefNo]>,
    root_scope: RootScope<'_>,
) -> anyhow::Result<RoomEdgeRecompute> {
    let started = std::time::Instant::now();
    let scope = ProjectionScope::Current;
    // 定向：命中集合（打包 refno）；整库：`None`。
    let (targeted, prelude): (Option<BTreeSet<i64>>, &str) = match root_scope {
        RootScope::All => (None, ""),
        RootScope::Targeted {
            roots,
            panel_statements,
        } => (
            Some(roots.iter().map(|root| packed_refno(*root)).collect()),
            panel_statements,
        ),
    };
    let projected: Vec<RefNo> = store
        .roots(scope)
        .into_iter()
        .filter(|root| root.dbno() == dbnum && store.receipt(scope, *root).is_some())
        .filter(|root| {
            targeted
                .as_ref()
                .is_none_or(|targeted| targeted.contains(&packed_refno(*root)))
        })
        .collect();
    let projected_set: BTreeSet<i64> = projected.iter().map(|root| packed_refno(*root)).collect();
    // 点名的、不在投影里的根：去重、只认本库的、按 refno 序。整库点名的是 `DbSet@T` 的清单，定向点名的是命中集合。
    let named_roots: &[RefNo] = match root_scope {
        RootScope::All => library_roots.unwrap_or_default(),
        RootScope::Targeted { roots, .. } => roots,
    };
    let mut durable: Vec<RefNo> = Vec::new();
    let mut named = BTreeSet::new();
    for root in named_roots {
        if root.dbno() == dbnum
            && !projected_set.contains(&packed_refno(*root))
            && named.insert(packed_refno(*root))
        {
            durable.push(*root);
        }
    }
    durable.sort_by_key(|root| packed_refno(*root));
    let mut covered_roots: Vec<RefNo> = projected.iter().chain(durable.iter()).copied().collect();
    covered_roots.sort_by_key(|root| packed_refno(*root));
    let mut out = RoomEdgeRecompute {
        roots: covered_roots.len(),
        covered_roots,
        projected_roots: projected.len(),
        durable_roots: durable.len(),
        panel_statements: prelude.matches(";\n").count(),
        ..Default::default()
    };
    let mut pieces: Vec<(usize, String)> = Vec::new();
    let mut recomputed = Vec::new();
    // 两种来源共用一段：算得出 → 渲成这一根的语句块（装块按构件数：一构件一句 DELETE、成员边一句
    // INSERT，不按几何条数）；算不出 → 进 `failed`，边原样留。
    fn recompute_root<G: crate::fast_model::room_model::RoomInput>(
        rooms: &crate::fast_model::room_model::MemoryRoomCalculator,
        root: RefNo,
        records: &[G],
        out: &mut RoomEdgeRecompute,
        pieces: &mut Vec<(usize, String)>,
        recomputed: &mut Vec<RefNo>,
    ) {
        match rooms.calculate_for_root(records) {
            Ok(edges) => {
                out.room_edges += edges.len();
                pieces.push((records.len(), rooms.render_root_room_edges(records, &edges)));
                recomputed.push(root);
            }
            Err(error) => out.failed.push((
                root,
                format!("room membership of root {root} could not be recomputed: {error:#}"),
            )),
        }
    }
    for root in &projected {
        let records = store.records_of_root(scope, *root);
        if records.is_empty() {
            continue;
        }
        recompute_root(
            rooms,
            *root,
            &records,
            &mut out,
            &mut pieces,
            &mut recomputed,
        );
    }
    let mut durable_read = Duration::ZERO;
    for root in &durable {
        let read_started = Instant::now();
        let rows = crate::fast_model::model_record_reader::durable_records_of_root(
            target.db,
            refno_from_e3d(*root),
        )
        .await
        .with_context(|| {
            format!("reading the durable geometry of root {root} for the room recompute")
        })?;
        durable_read += read_started.elapsed();
        let records = crate::fast_model::room_model::durable_room_inputs(&rows);
        out.durable_records += records.len();
        if records.is_empty() {
            continue;
        }
        recompute_root(
            rooms,
            *root,
            &records,
            &mut out,
            &mut pieces,
            &mut recomputed,
        );
    }
    out.durable_read_ms = durable_read.as_millis();
    // 面板级处置（定向才有）作 prelude 进第一笔：删在第一笔，与整根发布同一条装块纪律。
    let transactions = pack_publication_transactions(prelude, &pieces, "", false);
    for statements in &transactions {
        out.max_transaction_bytes = out.max_transaction_bytes.max(statements.len());
        commit_publication_transaction(target.db, statements, "room membership recompute").await?;
        out.commits += 1;
    }
    // 只有投影里的根有进程内 ACK 可对齐；回读库的那些根没有 ACK，就绪由 D14 那一格作答。
    for root in &recomputed {
        if let Some(mut ack) = store.publication_ack(scope, *root) {
            ack.room_membership = true;
            ack.room_generation = Some(rooms.resident_generation().to_string());
            if store.record_publication_ack(scope, *root, ack) {
                out.acks_refreshed += 1;
            }
        }
    }
    out.recomputed = recomputed.len();
    out.elapsed_ms = started.elapsed().as_millis();
    Ok(out)
}

/// 任务准入时冻结的 resident generation；房间开关关闭时没有这道完成要求。
///
/// 代号是**计算输入的摘要**（`ResidentRoomModel::input_version`，与
/// [`MemoryRoomCalculator::resident_generation`] 同一个字串），不是文件版本 composite（ADR-073 D12）：
/// 一个 DESI 前进却没动房间 / 面板时代号不变，此前的根 ACK 照旧有效。
///
/// [`MemoryRoomCalculator::resident_generation`]: crate::fast_model::room_model::MemoryRoomCalculator::resident_generation
pub fn required_room_generation() -> anyhow::Result<Option<String>> {
    if !crate::options::room_membership() {
        return Ok(None);
    }
    Ok(Some(
        crate::fast_model::room_resident::current_for_calculation()?.input_version(),
    ))
}

/// 任务开工前把常驻房间模型对齐到此刻的文件版本（ADR-073 D12）——**冻计算器之前**调一次。
///
/// 是探针不是重建：文件版本 composite 与装好的那份相同就原样返回；变了才按文件重建
/// （两层缓存让没变的文件只付重开校验的钱）。重建失败不让任务失败：留旧代继续算、
/// `/health.features.room` 记 `stage=refresh` 缺口。回执里 `Rebuilt` / `Failed` 打一行任务日志，
/// `Unchanged` / `NotApplicable` 不出声。任务执行体（`execute_frozen_batch_body` 的模型半边、
/// 初始化双 lane、读透侧 ensure 准入）经这里调，不直接摸 `room_resident`。
pub async fn refresh_room_reference_before_task(
    dbnum: u32,
) -> crate::fast_model::room_resident::RoomReferenceRefresh {
    let refresh = crate::fast_model::room_resident::refresh_before_task().await;
    if let Some(line) = refresh.log_line() {
        println!("dbnum={dbnum} {line}");
    }
    refresh
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
    // 提交成功的根记进进程内账本（ADR-079）——只对全局库的目标（[`PersistTarget::record_ledger`]）：
    // 账本说的是「库里有了」，旁路连接 / 测试夹具上的落库不是。
    let record_ledger = target.record_ledger;

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
            record_ledger,
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
        match persist_group(db, &live).await {
            Ok(outcome) => accept_group_outcome(
                &live,
                dbnum,
                target.medium,
                rooms,
                record_ledger,
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
                    match persist_group(db, retry).await {
                        Ok(outcome) => accept_group_outcome(
                            retry,
                            dbnum,
                            target.medium,
                            rooms,
                            record_ledger,
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
    /// 边直接用回执带来的根数 / flush 现算的根数（B/T4）。
    room_carried: usize,
    room_recomputed: usize,
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
    record_ledger: bool,
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
            let outcome = persist_group(db, std::slice::from_ref(one)).await;
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
                record_ledger,
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
    record_ledger: bool,
    outcome: GroupOutcome,
    total: &mut PersistedRows,
    out: &mut BatchedPersist,
    refresh_all: &mut Vec<aios_core::RefnoEnum>,
    boxes_all: &mut Vec<aios_core::accel_tree::acceleration_tree::RStarBoundingBox>,
    cleared_all: &mut Vec<aios_core::RefnoEnum>,
) {
    for root in group {
        if let Some(receipt) = root.share.receipt.as_ref() {
            // 这一组的每一笔都提交了：这根这一版成了，进程内账本记一条（ADR-079，`gen_root`
            // 回执的替身）。
            if record_ledger {
                record_root_generated(root.share.root, &root.share.records, receipt);
            }
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
    out.room_carried += outcome.room_carried;
    out.room_recomputed += outcome.room_recomputed;
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
/// 每根的顺序：最终几何 → 房间归属 → 同事务写模型行与房间边（回执不落库，ADR-079）。
async fn persist_group(
    db: &Surreal<Any>,
    group: &[LiveShare<'_>],
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

    // 有回执的根数：库里不再有逐根的章可盖（`gen_root` 退役，ADR-079），回执只用来在提交之后
    // 进进程内账本与 ACK（[`accept_group_outcome`]）。
    let receipted = group
        .iter()
        .filter(|live| live.share.receipt.is_some())
        .count();

    // 本组随模型行同事务写下的房间边数（各根在 [`room_stage`] 里算好的那份），以及边是带来的还是现算的。
    let room_edge_count = group.iter().map(|live| live.room_edges).sum::<usize>();
    let room_carried = group
        .iter()
        .filter(|live| live.room_source == RoomEdgeSource::Carried)
        .count();
    let room_recomputed = group
        .iter()
        .filter(|live| live.room_source == RoomEdgeSource::Recomputed)
        .count();

    // **一组一串事务，按条数装块。** 删整片与清边进第一笔；中途崩就是「有行没账」，那几根
    // 下一轮重新生成、重新整片删，自愈。完成与否不看某一笔，看水位（任务收尾只在这个库一根
    // 不剩时才推进 `dbnum_watermark.model_sesno`）。
    let mut prelude = String::new();
    // 整片删排在分支重置**之前**：它靠 `tubi_relate` 行上的 `aabb` / `world_trans` 指针去删
    // 管身的盒与变换，重置先把关系行删光的话，它读到的就是空集（管身的盒与变换按身份做键、
    // 插入又是 `INSERT IGNORE`，没删掉就等于把第一次发布的几何永久钉在那条槽位上）。
    prelude.push_str(&render_slice_delete(&sliced_roots));
    for &(word0, word1) in &branches {
        append_tube_branch_reset(&mut prelude, RefNo::new(word0, word1));
    }
    // 本轮要插的管身按身份再删一遍盒 / 变换：重置只捞得到还有关系行指着的，没人指着的
    // 孤儿（身份消失过又建回来）只有这一句够得着（见 [`render_tube_identity_sweep`]）。
    prelude.push_str(&render_tube_identity_sweep(&records)?);
    // 清边跟着删几何走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
    prelude.push_str(&render_room_edge_clear(&cleared_room_edges));
    let touches_geometry = !pieces.is_empty() || !sliced_roots.is_empty() || !branches.is_empty();
    let transactions = pack_publication_transactions(&prelude, &pieces, "", touches_geometry);
    if transactions.is_empty() {
        return Ok(GroupOutcome {
            upserted: 0,
            removed: 0,
            receipts: receipted,
            commits: 0,
            room_edges: 0,
            room_carried: 0,
            room_recomputed: 0,
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
        receipts: receipted,
        commits,
        room_edges: room_edge_count,
        room_carried,
        room_recomputed,
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
    // 库里没有逐根凭证行可问（`gen_root` 退役，ADR-079）：「可用」就是这根名下落了几何行。
    // 只问存在性、不取行（`LIMIT 1`），不往投影里灌任何东西。
    let mut response = db
        .query("SELECT VALUE id FROM inst_relate WHERE anc CONTAINS $root LIMIT 1;")
        .bind(("root", packed_refno(root)))
        .await
        .and_then(|response| response.check())?;
    Ok(!response.take::<Vec<SurThing>>(0)?.is_empty())
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

    // **一根一串事务，按 [`PERSIST_CHUNK`] 条装块。** 删整片与清边进第一笔；库里不再有逐根的章
    // 可盖（`gen_root` 退役，ADR-079），「这根这一版成了」在全部笔提交之后记进进程内账本
    // （`model_root_ledger`）。半途而废照样自愈：一笔失败账本就没有这一条，这根在窗口规划器眼里
    // 仍欠着（或重启后随 `model_sesno → T` 那一窗）重生成，下一轮的整片删把这一轮留下的半截行收干净。
    let mode = if reconciliation_ready {
        // 整片删覆盖得到的根不必再按 id 删一遍（见 `persist_group` 里同一处的说明）。
        WriteMode::Initial
    } else {
        WriteMode::Incremental
    };
    let mut tube_slots = BTreeMap::<(u32, u32), usize>::new();
    let pieces = render_record_chunks(&records, mode, &mut tube_slots)?;

    let mut prelude = String::new();
    // 整片删排在分支重置**之前**：它靠 `tubi_relate` 行上的 `aabb` / `world_trans` 指针去删
    // 管身的盒与变换，重置先把关系行删光的话，它读到的就是空集（管身的盒与变换按身份做键、
    // 插入又是 `INSERT IGNORE`，没删掉就等于把第一次发布的几何永久钉在那条槽位上）。
    prelude.push_str(&render_slice_delete(&sliced_roots));
    for &(word0, word1) in &branches {
        append_tube_branch_reset(&mut prelude, RefNo::new(word0, word1));
    }
    // 本轮要插的管身按身份再删一遍盒 / 变换：重置只捞得到还有关系行指着的，没人指着的
    // 孤儿（身份消失过又建回来）只有这一句够得着（见 [`render_tube_identity_sweep`]）。
    prelude.push_str(&render_tube_identity_sweep(&records)?);
    // 清边跟着删几何走，任务内外一律如此（ADR-072 D4：没有 publish 阶段替它补做）。
    prelude.push_str(&render_room_edge_clear(&cleared_room_edges));
    let touches_geometry = !pieces.is_empty() || !sliced_roots.is_empty() || !branches.is_empty();
    let transactions = pack_publication_transactions(&prelude, &pieces, "", touches_geometry);
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
    // 全部笔都提交了：这根这一版成了，记进进程内账本（ADR-079）。写全局库的目标才记
    // （[`PersistTarget::record_ledger`]）——账本说的是「库里有了」，旁路连接 / 测试夹具上的落库不是。
    if let Some(receipt) = receipt.as_ref()
        && target.record_ledger
    {
        record_root_generated(root, &records, receipt);
    }

    Ok(PersistedRows {
        medium: target.medium,
        upserted: records.len(),
        removed: cleared_room_edges.len(),
        receipts: usize::from(receipt.is_some()),
        commits,
    })
}

/// 一根落库成功后进账本的那一笔（ADR-079，`gen_root` 回执的替身）：`noun` 取根元素自己那条记录
/// 的 noun（容器根 / 只有管身的分支没有这样一条时留空——账本里 `Generated` 的 noun 只作展示）。
fn record_root_generated(root: RefNo, records: &[ProjectedGeometry], receipt: &RootReceipt) {
    let noun = records
        .iter()
        .find(|record| record.source_refno == root)
        .map(|record| record.noun.as_str())
        .unwrap_or_default();
    crate::data_interface::model_root_ledger::record_generated(
        root.dbno(),
        refno_from_e3d(root),
        noun,
        receipt.source_sesno as i32,
    );
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
    // 回执只作前置校验（没回执的根不是一份完整投影，不许渲成整根替换）；库里没有逐根的章
    // 可盖（`gen_root` 退役，ADR-079）。
    store
        .receipt(ProjectionScope::Current, root)
        .with_context(|| format!("model root {root} has no publication receipt"))?;
    let mut statements = render_spooled_scope_cleanup(root);
    let branches = records
        .iter()
        .filter(|record| record.tube.is_some())
        .map(|record| record.source_refno)
        .collect::<Vec<_>>();
    statements.push_str(&render_tube_branch_resets(&branches));
    statements.push_str(&render_tube_identity_sweep(&records)?);
    statements.push_str(&render_records(&records, WriteMode::Initial)?);
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
    // 被波及的根不再有库里的凭证行要删（`gen_root` 退役，ADR-079）：它们在进程内账本上的那一条
    // 由调用方 `model_root_ledger::forget_root` 摘掉。
    let _ = affected_roots;
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
///
/// **关系行与它的盒 / 变换要一起删**：管身的 `aabb` / `trans` 按**身份**做键
/// （`geometry_record_id` = `GeometryId::ImpliedTube` 的 SHA-256，不是内容摘要），
/// 而插入一律是 `INSERT IGNORE`。只删关系行的话，同一条槽位下一轮重新插出来的关系
/// 仍指向那条**没被覆盖的旧**变换——槽位还在，几何却永远停在第一次发布的那个值。
///
/// 这一条以前指望 [`render_slice_delete`] 顺着行上的指针去删，但那条整片删是
/// `SELECT … FROM tubi_relate WHERE anc CONTAINSANY …`，而**它排在本重置之后**：
/// 关系行已经被这里删光，整片删读到的是空集，三句 `DELETE $slice_tubes.map(…)`
/// 删的都是空数组。所以重置自己把指针捞出来删，不依赖语句顺序。
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
    let key = format!("{}_{}", branch.word0, branch.word1);
    // 走 `pe:⟨…⟩->tubi_relate` 而不是 `WHERE in = …`：前者吃 `in` 上的边索引，
    // 后者在部署引擎上整表扫（`rvm_baseline::compare` 的文档里记着同一条）。
    // `WHERE` 那一句挡住历史行——没有指针的行 `map` 出来是 NONE，`DELETE` 见到 NONE 整笔事务失败。
    statements.push_str(&format!(
        "LET $tube_reset_{key} = SELECT aabb, world_trans FROM pe:⟨{key}⟩->tubi_relate \
         WHERE aabb != NONE AND world_trans != NONE;\n\
         DELETE $tube_reset_{key}.map(|$row| $row.aabb);\n\
         DELETE $tube_reset_{key}.map(|$row| $row.world_trans);\n\
         DELETE pe:⟨{key}⟩->tubi_relate;\n"
    ));
}

/// 本轮要插的管身，按**身份**先把同 id 的盒与变换删一遍（`aabb:⟨id⟩` / `trans:⟨id⟩`，
/// id 就是它的记录 id）。
///
/// 分支重置只删得到**当前关系行还指着的**那些盒 / 变换。此前只删关系行的版本留下过一批
/// 没人指着的孤儿——身份消失过一轮（缝退化成 0、成员被删）、之后又建回来的那种；重置
/// 沿指针捞不到它们，`INSERT IGNORE` 撞上同 id 新几何就整行落空，而管身的身份键在几何变了
/// 之后也**不变**（e3d-model 故意如此，键的局部性）。所以「这条身份马上要插」就是
/// 「库里同 id 的旧行必须先走」的充分条件；不存在的 id `DELETE` 是空操作。
///
/// 只管管身：元素行的盒 / 变换与 `inst_relate` 同键，整片删 / 按 id 删都顺手带走。
/// 这一句进 prelude（第一笔），`render_records` 的 `Initial` 形态照旧只插不删。
fn render_tube_identity_sweep(records: &[ProjectedGeometry]) -> anyhow::Result<String> {
    let ids = records
        .iter()
        .filter(|record| record.tube.is_some())
        .map(|record| bracketed_id(record.record_id.as_str()))
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    if ids.is_empty() {
        return Ok(String::new());
    }
    let mut statements = String::new();
    for table in ["aabb", "trans"] {
        let refs = ids
            .iter()
            .map(|id| format!("{table}:⟨{id}⟩"))
            .collect::<Vec<_>>()
            .join(",");
        statements.push_str(&format!(
            "LET $tube_sweep_{table} = [{refs}];\nDELETE $tube_sweep_{table};\n"
        ));
    }
    Ok(statements)
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

// 逐根回执（`gen_root` 行上的 status / publication / revision / 目标那一组列，与它旁边那张
// 目录依赖表）随 `gen_root` 表退役（ADR-079）：库里模型面唯一的持久事实是
// `dbnum_watermark.model_sesno`（任务收尾一笔盖）；「这根这一版成了」在提交之后记进进程内账本
// （[`record_root_generated`] → `model_root_ledger`）。「同一根不许被两个源会话混着发布」由同一
// dbnum 只有本任务一个写者（ADR-071 D5）与 `generation_root_lock` 守着，没有对手。

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

    /// 分支重置必须**连盒带变换一起删**，而且删在关系行之前。
    ///
    /// 管身的 `aabb` / `trans` 按身份做键（`GeometryId::ImpliedTube` 的 SHA-256），插入又一律
    /// `INSERT IGNORE`：只删关系行的话，同一条槽位下一轮插出来的关系仍指向那条没被覆盖的旧
    /// 变换，几何永远停在第一次发布的值。2026-09-18 实测就是这样——库里那条 `trans` 被手工
    /// 改成 999mm 之后，`/model/ensure force=true` 整根重生成也没把它写回来。
    #[test]
    fn a_branch_reset_clears_the_tube_boxes_and_transforms_before_the_relations() {
        let sql = render_tube_branch_resets(&[RefNo::new(24381, 103320)]);
        let pointers = sql
            .find("DELETE $tube_reset_24381_103320.map(|$row| $row.aabb);")
            .expect(&format!("盒要顺指针删: {sql}"));
        let transforms = sql
            .find("DELETE $tube_reset_24381_103320.map(|$row| $row.world_trans);")
            .expect(&format!("变换要顺指针删: {sql}"));
        let relations = sql
            .find("DELETE pe:⟨24381_103320⟩->tubi_relate;")
            .expect(&format!("关系行照旧整段删: {sql}"));
        assert!(
            pointers < relations && transforms < relations,
            "指针要在关系行之前捞出来删，否则读到的是空集: {sql}"
        );
        assert!(
            sql.contains("FROM pe:⟨24381_103320⟩->tubi_relate"),
            "走边索引，不用 `WHERE in = …`（部署引擎上那是整表扫）: {sql}"
        );
        assert!(
            sql.contains("WHERE aabb != NONE AND world_trans != NONE"),
            "历史行没有指针，map 出 NONE 会让整笔事务失败: {sql}"
        );
    }

    /// 整片删也顺着 `tubi_relate` 行上的指针删管身的盒与变换，所以它必须排在分支重置**之前**：
    /// 重置先把关系行删光，整片删读到的就是空集，三句 `DELETE $slice_tubes.map(…)` 全落空。
    #[test]
    fn the_slice_delete_is_emitted_before_the_branch_resets() {
        // 只看生产那一半：本测试自己也写着这几个字面量，连测试一起数会多出两处。
        let production = include_str!("model_db_adapter.rs")
            .split_once("\n#[cfg(test)]\nmod tests {")
            .expect("测试模块的起点")
            .0;
        let prelude_sites = production
            .match_indices("let mut prelude = String::new();")
            .count();
        assert_eq!(prelude_sites, 2, "两条落库路径各一处 prelude");
        for (start, _) in production.match_indices("let mut prelude = String::new();") {
            let block = &production[start..];
            let slice = block
                .find("render_slice_delete(&sliced_roots)")
                .expect("prelude 必须发整片删");
            let reset = block
                .find("append_tube_branch_reset(")
                .expect("prelude 必须发分支重置");
            assert!(
                slice < reset,
                "整片删要排在分支重置之前，否则它读不到管身行上的 aabb / world_trans 指针"
            );
        }
    }

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
        let error = persist_group(&db, &live).await.err().unwrap();
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
                room: None,
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
        // 库里没有逐根凭证行（ADR-079）：可用性探针问的是这根名下有没有几何行。
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
            room: None,
        }
    }

    /// 回执不再落库（ADR-079）：一根提交成功之后进的是进程内账本，noun 取根元素自己那条记录的
    /// noun；库里模型面的行只剩几何行本身，没有任何一句 `gen_root` / `root_dependency`。
    #[test]
    fn a_receipt_settles_the_root_in_the_process_ledger_not_in_a_table() {
        use crate::data_interface::model_root_ledger as ledger;
        let root = RefNo::new(24384, 1);
        let dbnum = root.dbno();
        ledger::forget_dbnum(dbnum);
        let mut own = baked_record();
        own.source_refno = root;
        own.noun = "BRAN".into();
        record_root_generated(root, &[own], &receipt());
        let entry = ledger::entry(dbnum, refno_from_e3d(root)).expect("ledger entry");
        assert!(entry.is_settled());
        assert_eq!(entry.noun, "BRAN");
        assert_eq!(entry.source_end_sesno, 26);
        ledger::forget_dbnum(dbnum);

        let source = include_str!("model_db_adapter.rs");
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        for forbidden in [
            "type::thing('gen_root'",
            "type::thing('root_dependency'",
            "publication_status='ready'",
        ] {
            assert!(
                !production.contains(forbidden),
                "落库适配器不许再写逐根凭证行: {forbidden}"
            );
        }
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
        assert!(
            !sql.contains("gen_root"),
            "spool 里没有逐根凭证行可写（ADR-079）: {sql}"
        );
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
        assert!(!tombstone.contains("gen_root"), "{tombstone}");
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

    /// 库里全部管身行的 `world_trans.d.scale.z`——单位圆柱的长度就写在这一格。
    async fn tube_scale_z(db: &Surreal<Any>) -> Vec<f32> {
        let mut response = db
            .query("SELECT VALUE world_trans.d.scale FROM tubi_relate;")
            .await
            .expect("scale")
            .check()
            .expect("scale statement");
        response
            .take::<Vec<[f32; 3]>>(0)
            .expect("scale rows")
            .into_iter()
            .map(|scale| scale[2])
            .collect()
    }

    fn tube_of_length(length_mm: f64) -> ProjectedGeometry {
        let mut record = tube_record(0, TUBE_A);
        record.world_transform = DMat4::from_scale(glam::DVec3::new(1.0, 1.0, length_mm));
        record
    }

    /// 2026-09-18 现场（BRAN 103320 / 145128 / 159862）：HPOS 沿管轴挪了，头段管**该不该有**判对了、
    /// **多长**却还是基线那个值，停服务重启、`force` 重生成都不动。机理：管身的 `aabb` / `trans`
    /// 按身份做键（身份在几何变了之后也不变），插入是 `INSERT IGNORE`，而 HEAD 的分支重置只删关系行、
    /// 整片删又排在重置之后读到空集——第一次发布的那条 `trans` 从此没人删得到，每一轮的新变换都落空。
    /// 这里钉的就是那一根管：同一条身份、第二次发布换了长度，库里读回来必须是新长度，且旧行不留。
    #[tokio::test]
    async fn a_regenerated_tube_overwrites_its_previous_transform_under_the_same_identity() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        let store = ModelMemoryStore::default();
        let target = PersistTarget::new(&db, PersistMedium::Mem);

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), tube_of_length(144.875)],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &store, root)
            .await
            .expect("first persist");
        assert_eq!(tube_scale_z(&db).await, vec![144.875]);

        // HPOS 退了 94.875mm：容器、序号、两端都没变，身份（记录 id）一字不变，只有长度变了。
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                tube_branch_resets: vec![BRANCH],
                upserts: vec![tube_of_length(50.0)],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &store, root)
            .await
            .expect("second persist");
        assert_eq!(
            tube_scale_z(&db).await,
            vec![50.0],
            "同一条身份第二次发布，库里必须是新长度，不许被第一次发布的那条 trans 钉住"
        );
        assert_eq!(
            count(&db, "trans").await,
            2,
            "元素一条 + 管身一条：旧变换要被删掉，不许留成孤儿"
        );
        assert_eq!(count(&db, "aabb").await, 2, "盒同理");
    }

    /// 修法之前的分支重置只删关系行，库里已经留下一批**没人指着**的旧 `trans` / `aabb`
    /// （身份消失过一轮——缝退化成 0、成员被删——之后又建回来的那种，E1→E2）。重置沿指针捞不到
    /// 它们，所以本轮要插的管身还得按身份把同 id 的旧行先删一遍（[`render_tube_identity_sweep`]），
    /// 否则 `INSERT IGNORE` 撞上孤儿，建回来的管又是旧长度。
    #[tokio::test]
    async fn an_orphaned_identity_row_does_not_pin_a_returning_tube() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let root = RefNo::new(24383, 1);
        let store = ModelMemoryStore::default();
        let target = PersistTarget::new(&db, PersistMedium::Mem);

        // 孤儿：同一条身份上一世的变换，关系行早没了。
        let stale = sql_value(&transform_value(DMat4::from_scale(glam::DVec3::new(
            1.0, 1.0, 144.875,
        ))));
        db.query(format!("CREATE trans:⟨{TUBE_A}⟩ SET d = {stale};"))
            .await
            .expect("seed orphan")
            .check()
            .expect("seed orphan statement");

        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), tube_of_length(50.0)],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(
            tube_scale_z(&db).await,
            vec![50.0],
            "建回来的管身要带自己的长度，不许被没人指着的旧 trans 钉住"
        );
        assert_eq!(count(&db, "trans").await, 2);
    }

    /// spool 重放那一路同样先按身份清管身的盒 / 变换，再插 `Initial` 那批行。
    #[test]
    fn a_spooled_root_sweeps_tube_identities_before_inserting() {
        let root = RefNo::new(24383, 1);
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), tube_record(0, TUBE_A)],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let sql = render_spooled_root(&store, root).expect("render");
        let sweep = sql
            .find(&format!("LET $tube_sweep_trans = [trans:⟨{TUBE_A}⟩];\nDELETE $tube_sweep_trans;"))
            .expect(&format!("管身变换按身份先删: {sql}"));
        assert!(
            sql.contains(&format!("LET $tube_sweep_aabb = [aabb:⟨{TUBE_A}⟩];\nDELETE $tube_sweep_aabb;")),
            "盒同理: {sql}"
        );
        let insert = sql.find("INSERT IGNORE INTO trans [").expect("插变换");
        assert!(sweep < insert, "先删后插: {sql}");
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
        assert!(
            !table_exists(&db, "gen_root").await,
            "落库不再建任何逐根凭证行（ADR-079）"
        );
    }

    /// 库里有没有这张表（`INFO FOR DB` 的 tables 段）。ADR-079 之后 `gen_root` 不该被任何一笔落库建出来。
    async fn table_exists(db: &Surreal<Any>, table: &str) -> bool {
        let mut response = db
            .query("INFO FOR DB;")
            .await
            .expect("info")
            .check()
            .expect("info statement");
        let info: Option<serde_json::Value> = response.take(0).expect("decode info");
        info.as_ref()
            .and_then(|value| value.get("tables"))
            .and_then(|tables| tables.get(table))
            .is_some()
    }

    /// 嵌套根与祖先根各自携带完整记录、每根各自盖章；重复 id 由幂等写收敛。
    /// 未提交的跨根认领会让前一根失败时后一根静默缺行，因此这里的发送账允许重复。
    #[tokio::test]
    async fn batched_persist_shares_chunks_across_roots_and_stamps_every_receipt() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
        let ancestor_root = RefNo::new(24383, 1);

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
        assert_eq!(publication_status_of(&store, ancestor_root).await, "ready");
        assert_eq!(publication_status_of(&store, NESTED_ROOT).await, "ready");
    }

    /// 对账只读一次库：库里挂在根名下、存储里已经没有的几何在下一批里被删掉，
    /// 上一轮的管身随分支重置一起清。
    #[tokio::test]
    async fn batched_persist_reconciles_stale_rows_from_one_database_read() {
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
        assert_eq!(publication_status_of(&store, root).await, "ready");
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
        assert_eq!(publication_status_of(&store, root).await, "ready");
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
        assert_eq!(count(&db, "inst_relate").await, 0, "整笔回滚，一行都不留");
        assert!(
            !table_exists(&db, "gen_root").await,
            "失败的那一笔不许在库里留下任何逐根痕迹（ADR-079：库里本就没有凭证行）"
        );

        // 换成两条都渲染得出来的记录重跑：行落了，回执只进账（不落库）。
        let store = ModelMemoryStore::default();
        store.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![baked_record(), other_canonical_record()],
                receipt: Some(test_receipt(root, 2)),
                ..ProjectionCommit::default()
            }],
        );
        let rows = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(rows.receipts, 1);
        assert_eq!(count(&db, "inst_relate").await, 2);
    }

    /// 没有回执的落库只 upsert 自己那几行，账上 `receipts = 0`（不整片删、不进账本）。
    #[tokio::test]
    async fn a_publish_without_a_receipt_never_touches_the_credential() {
        let db = surreal_mem().await;
        seed_pe(&db).await;
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
        let rows = persist_root_on(target, &store, root)
            .await
            .expect("persist");
        assert_eq!(rows.receipts, 0);
        assert_eq!(count(&db, "inst_relate").await, 2);
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
        room_calculator_with(dir, 0., None)
    }

    /// 在册却没几何的第三块面板（房间 R3）：ADR-073 D9 保护集的夹具。
    const BARE_PANEL_R3: RefNo = RefNo::new(24383, 700);

    /// [`room_calculator`] 的可调版：R1 的盒子沿 x 平移 `r1_shift`（面板搬家的夹具），
    /// `bare_panel` 是在册却没几何的面板（D9 保护集的夹具）。
    fn room_calculator_with(
        dir: &std::path::Path,
        r1_shift: f32,
        bare_panel: Option<RefNo>,
    ) -> crate::fast_model::room_model::MemoryRoomCalculator {
        use crate::fast_model::room_model::{RoomPanelMap, RoomPanels, box_mesh_for_test};
        use crate::fast_model::room_resident::{
            MeshCache, ResidentForm, ResidentManifest, ResidentRoomModel,
        };
        let r1_mesh = format!("panel_r1_{}", r1_shift as i64);
        box_mesh_for_test(
            glam::Vec3::new(r1_shift, 0., 0.),
            glam::Vec3::new(r1_shift + 1000., 1000., 1000.),
        )
        .ser_to_file(&dir.join(format!("{r1_mesh}.mesh")))
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
        let mut rooms = vec![
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
        ];
        let mut all_panels = std::collections::HashSet::from([r1, r2]);
        if let Some(bare) = bare_panel {
            let r3: RefnoEnum = refno_from_e3d(bare);
            rooms.push(RoomPanels {
                room: "24383_701".into(),
                room_num: "R3".into(),
                panels: vec![r3],
            });
            all_panels.insert(r3);
        }
        let map = RoomPanelMap { rooms, all_panels };
        let model = std::sync::Arc::new(ResidentRoomModel::from_projection(
            ResidentManifest {
                form: ResidentForm::Latest,
                key: format!("rooms-fixture-{}", r1_shift as i64),
            },
            vec![],
            map,
            &[
                panel(
                    PANEL_R1,
                    &r1_mesh,
                    [r1_shift, 0., 0.],
                    [r1_shift + 1000., 1000., 1000.],
                ),
                panel(PANEL_R2, "panel_r2", [500., 0., 0.], [1500., 1000., 1000.]),
            ],
            std::sync::Arc::<MeshCache>::default(),
        ));
        crate::fast_model::room_model::MemoryRoomCalculator::new(model, dir.to_path_buf())
            .expect("room calculator")
    }

    /// B/T4 三道闸（纯函数）：代号相等 + 构件集相等 + 回执带了边 → 用带来的；代号不等 / 少一个构件 /
    /// 多一个构件 / 没带 → `None`（现算）。
    #[test]
    fn carried_edges_pass_only_when_generation_and_element_set_both_match() {
        use crate::fast_model::room_model::{RoomInput, carry_root_edges};
        let dir = tempfile::tempdir().unwrap();
        let calculator = room_calculator(dir.path());
        let records = vec![
            room_record(ROOM_ROOT_A, 100., 200.),
            room_record(RefNo::new(24383, 11), 300., 400.),
        ];
        let edges = calculator.calculate_for_root(&records).unwrap();
        assert!(!edges.is_empty(), "夹具前提：两件都在 R1 里");
        let carried = carry_root_edges(
            calculator.resident_generation(),
            records.iter().map(RoomInput::element),
            edges.clone(),
        );
        let generation = calculator.resident_generation();

        assert_eq!(
            carried_edges_for(Some(&carried), generation, &records).as_deref(),
            Some(edges.as_slice()),
            "三道闸都过 → 带来的边逐字回来"
        );
        assert!(
            carried_edges_for(None, generation, &records).is_none(),
            "没带"
        );
        assert!(
            carried_edges_for(Some(&carried), "other-generation", &records).is_none(),
            "代号不等（常驻中途换代 / 重启后旧产物）"
        );
        assert!(
            carried_edges_for(Some(&carried), generation, &records[..1]).is_none(),
            "回执覆盖的构件比本根记录多（局部发布后的整根形状）→ 现算"
        );
        let mut more = records.clone();
        more.push(room_record(RefNo::new(24383, 12), 700., 800.));
        assert!(
            carried_edges_for(Some(&carried), generation, &more).is_none(),
            "本根记录比回执覆盖的多（嵌套根共享记录）→ 现算，否则没动的构件的边会被删而不补"
        );
    }

    /// B/T4：`room_stage` 对带边的回执只渲染（`Carried`），渲出的语句与现算（`Recomputed`）**逐字相同**；
    /// 任一道闸没过就现算；没有计算器 `NotComputed`。
    #[test]
    fn room_stage_renders_carried_edges_and_recomputes_when_a_gate_fails() {
        use crate::fast_model::room_model::{RoomInput, carry_root_edges};
        let dir = tempfile::tempdir().unwrap();
        let calculator = room_calculator(dir.path());
        let records = vec![
            room_record(ROOM_ROOT_A, 100., 200.),
            room_record(RefNo::new(24383, 11), 600., 700.),
        ];
        let edges = calculator.calculate_for_root(&records).unwrap();
        let carried = carry_root_edges(
            calculator.resident_generation(),
            records.iter().map(RoomInput::element),
            edges,
        );
        let share = |room: Option<crate::fast_model::model_memory_store::CarriedRoomEdges>| {
            let mut receipt = test_receipt(ROOM_ROOT_A, records.len()).1;
            receipt.room = room;
            RootShare {
                root: ROOM_ROOT_A,
                records: records.clone(),
                receipt: Some(receipt),
            }
        };

        let carried_share = [share(Some(carried.clone()))];
        let (with, failed) = room_stage(&carried_share, Some(&calculator));
        assert!(failed.is_empty());
        assert_eq!(with[0].room_source, RoomEdgeSource::Carried);
        let bare_share = [share(None)];
        let (without, failed) = room_stage(&bare_share, Some(&calculator));
        assert!(failed.is_empty());
        assert_eq!(without[0].room_source, RoomEdgeSource::Recomputed);
        assert_eq!(
            with[0].room_statements, without[0].room_statements,
            "带来的边渲出的语句与现算逐字相同（ADR-010 §9 的前提）"
        );
        assert_eq!(with[0].room_edges, without[0].room_edges);
        assert!(
            with[0]
                .room_statements
                .contains("INSERT RELATION INTO room_relate"),
            "{}",
            with[0].room_statements
        );

        let mut stale = carried.clone();
        stale.generation = "another-generation".into();
        let stale_share = [share(Some(stale))];
        let (stale, _) = room_stage(&stale_share, Some(&calculator));
        assert_eq!(
            stale[0].room_source,
            RoomEdgeSource::Recomputed,
            "代号不等 → 现算"
        );
        let mut partial = carried.clone();
        partial.elements.pop();
        let partial_share = [share(Some(partial))];
        let (partial, _) = room_stage(&partial_share, Some(&calculator));
        assert_eq!(
            partial[0].room_source,
            RoomEdgeSource::Recomputed,
            "构件集不等 → 现算"
        );
        let no_calculator = [share(Some(carried))];
        let (none, _) = room_stage(&no_calculator, None);
        assert_eq!(none[0].room_source, RoomEdgeSource::NotComputed);
        assert!(none[0].room_statements.is_empty());
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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "ready");
        let ack = store
            .publication_ack(ProjectionScope::Current, ROOM_ROOT_A)
            .expect("complete publication ACK");
        assert_eq!(ack.source_sesno, 42);
        assert_eq!(ack.geometry_count, 1);
        assert!(ack.room_membership);
        // 代号是计算输入摘要，不是夹具的文件 composite（"rooms-fixture"）——ADR-073 D12。
        assert_eq!(
            ack.room_generation.as_deref(),
            Some(rooms.resident_generation())
        );
        assert_ne!(ack.room_generation.as_deref(), Some("rooms-fixture"));
        assert_eq!(ack.medium, PersistMedium::Mem);
    }

    /// ADR-073 D9 / 源码钉 14 (a)，`mem://` 真测：在册却没几何的面板 R3 **不阻断**——计算器照冻、根照常落库、
    /// 没有一根进 `failed`；构件在 R1 里的新边照写，指向 R3 的**存量边原样保留**（保护集），不被
    /// 「先删后插」当成「算出来是空的」抹掉。此前这一根在 `MemoryRoomCalculator::new` 就被
    /// `ensure_complete()` 拦成「room calculator unavailable」。
    #[tokio::test]
    async fn a_registered_panel_without_geometry_keeps_its_old_edges_and_blocks_no_root() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator_with(dir.path(), 0., Some(BARE_PANEL_R3));
        assert_eq!(
            rooms.missing_panels(),
            &[refno_from_e3d(BARE_PANEL_R3)],
            "夹具前提：R3 在册、没几何"
        );
        let db = surreal_mem().await;
        // 上一版留下的、指向 R3 的存量边（与生产同一形状）：本轮判不了它，得留着。
        db.query(
            "INSERT RELATION INTO room_relate [{ id: room_relate:24383_700_24383_10, \
             in: pe:24383_700, out: pe:24383_10, room_num: 'R3', inside_count: 8, center_dist: 0.0 }];",
        )
        .await
        .expect("seed stale R3 edge")
        .check()
        .expect("seed statement");
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
        .expect("publish with a bare registered panel");

        assert!(
            batched.failed.is_empty(),
            "缺几何的面板不判死任何根: {:?}",
            batched.failed
        );
        assert_eq!(batched.room_edges, 1, "R1 的那条新边照写");
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24383_10".to_string(),
                "pe:24383_700->pe:24383_10".to_string(),
            ],
            "新边写了、指向 R3 的存量边留着（陈旧可恢复，抹平不可）"
        );
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "ready");
    }

    /// ADR-073 D13 / 源码钉 18 (e)，`mem://` 真测：面板搬了家（R1 的盒子沿 x 平移 2000）→ 用**新**计算器对
    /// 本库现有几何整库重算 → 原来在 R1 里的构件 A 的边被删掉（A 自己一行几何没动、没重生）、ACK 的
    /// 房间代号换成新台；搬回来再算一次，边又回来——两个方向都是同一条路。
    #[tokio::test]
    async fn moving_a_panel_updates_the_other_roots_edges_through_the_library_recompute() {
        let dir = tempfile::tempdir().unwrap();
        let before_move = room_calculator_with(dir.path(), 0., None);
        let db = surreal_mem().await;
        let store = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&before_move),
        )
        .await
        .expect("seed A inside R1");
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()]
        );

        // 面板搬走：R1 现在占 x 2000–3000，A（x 100–200）不在里面了。
        let after_move = room_calculator_with(dir.path(), 2000., None);
        assert_ne!(
            before_move.resident_generation(),
            after_move.resident_generation(),
            "面板几何变了 = 房间代号变了（D12）"
        );
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &after_move,
            None,
            RootScope::All,
        )
        .await
        .expect("recompute after the panel moved");
        assert_eq!(
            (report.recomputed, report.room_edges, report.failed.len()),
            (1, 0, 0),
            "{report:?}"
        );
        assert!(
            room_edges(&db).await.is_empty(),
            "A 不再在任何面板里，它的旧边随重算删掉"
        );
        assert_eq!(
            store
                .publication_ack(ProjectionScope::Current, ROOM_ROOT_A)
                .expect("ACK stays")
                .room_generation
                .as_deref(),
            Some(after_move.resident_generation())
        );
        assert_eq!(
            count_record(&db, "inst_relate", "24383_10").await,
            1,
            "整库房间重算不动模型行"
        );

        // 搬回来：边回来，代号回到原来那台。
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &before_move,
            None,
            RootScope::All,
        )
        .await
        .expect("recompute after the panel moved back");
        assert_eq!((report.recomputed, report.room_edges), (1, 1), "{report:?}");
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()]
        );
    }

    /// d-819 / ADR-073 D13：整库房间边重算用**现有几何**、走逐根同一套渲染——重算后的边与逐根落库的
    /// 逐字相同（幂等，ADR-010 §9 的对照物不多一条口径）；算不出的根边原样留、进 `failed`，其余照常；
    /// 提交全过后 ACK 的房间代号对齐到本台计算器。
    #[tokio::test]
    async fn recomputing_a_librarys_room_edges_reuses_the_per_root_rendering_and_refreshes_acks() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = room_calculator(dir.path());
        let db = surreal_mem().await;
        let store = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&rooms),
        )
        .await
        .expect("seed the library through the per-root path");
        let before = room_edges(&db).await;
        assert_eq!(before, vec!["pe:24383_500->pe:24383_10".to_string()]);

        // 模拟「面板那一侧变过、A 的 ACK 还挂着旧代号」：重算后要对齐回来。
        let mut stale = store
            .publication_ack(ProjectionScope::Current, ROOM_ROOT_A)
            .expect("seeded ACK");
        stale.room_generation = Some("old-generation".into());
        assert!(store.record_publication_ack(ProjectionScope::Current, ROOM_ROOT_A, stale));
        // 再来一根算不出的（盒子 NaN，`aabb_is_usable` 不认）：边原样（它本来就没有）、进 failed。
        store.commit(
            ProjectionScope::Current,
            vec![root_commit(
                ROOM_ROOT_B,
                vec![room_record(ROOM_ROOT_B, f32::NAN, 200.)],
            )],
        );

        // 调用方点名的 DbSet@T 根清单里两根都在投影里：一根都不回读库。
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &rooms,
            Some(&[ROOM_ROOT_A, ROOM_ROOT_B]),
            RootScope::All,
        )
        .await
        .expect("recompute the library's room edges");
        assert_eq!(
            (report.roots, report.recomputed, report.room_edges),
            (2, 1, 1),
            "{report:?}"
        );
        assert_eq!(
            (
                report.projected_roots,
                report.durable_roots,
                report.durable_records
            ),
            (2, 0, 0),
            "投影里有回执的根不回读库: {report:?}"
        );
        assert_eq!(
            report.covered_roots,
            vec![ROOM_ROOT_A, ROOM_ROOT_B],
            "覆盖到的根 = 投影里有回执的根（含算不出的那根），按 refno 序——调用方拿它对 DbSet@T 自证覆盖面"
        );
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, ROOM_ROOT_B);
        assert!(
            report.failed[0].1.contains("could not be recomputed"),
            "{}",
            report.failed[0].1
        );
        assert_eq!(
            (report.commits, report.acks_refreshed),
            (1, 1),
            "{report:?}"
        );
        assert_eq!(
            room_edges(&db).await,
            before,
            "整库重算 == 逐根落库，逐边相等"
        );
        let ack = store
            .publication_ack(ProjectionScope::Current, ROOM_ROOT_A)
            .expect("ACK stays");
        assert_eq!(
            ack.room_generation.as_deref(),
            Some(rooms.resident_generation()),
            "ACK 的房间代号对齐到本台计算器"
        );
        assert!(
            store
                .publication_ack(ProjectionScope::Current, ROOM_ROOT_B)
                .is_none(),
            "算不出的根没有 ACK 可对齐"
        );
    }

    /// ADR-073 D13 / U1 第二半（2026-09-17）：**不在投影里的根回读库上几何**补算。A 经逐根路径落库（行 + 边）
    /// 之后从投影里消失（模拟初始化的模型行 / 落盘形态重启前生成的根：库里有、投影里没有），面板搬家 → 整库
    /// 重算点名 `DbSet@T = [A]` → A 的几何从 `inst_relate` 读回、用新计算器算：旧边删掉、模型行不动；搬回来
    /// 再算，边逐字回来——回读那条路与逐根落库同一形状。回读的根没有进程内 ACK 可对齐（就绪由 D14 那一格
    /// 作答）；点名里库上一行几何都没有的根算覆盖到、什么都不写；不是本库的根不认。
    #[tokio::test]
    async fn a_root_outside_the_projection_gets_its_room_edges_recomputed_from_the_durable_rows() {
        let dir = tempfile::tempdir().unwrap();
        let before_move = room_calculator_with(dir.path(), 0., None);
        let db = surreal_mem().await;
        let seeded = room_store(vec![root_commit(
            ROOM_ROOT_A,
            vec![room_record(ROOM_ROOT_A, 100., 200.)],
        )]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &seeded,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&before_move),
        )
        .await
        .expect("seed A inside R1 through the per-root path");
        let before = room_edges(&db).await;
        assert_eq!(before, vec!["pe:24383_500->pe:24383_10".to_string()]);

        // 库里有 A 的行、投影里没有 A：空投影 = 落盘形态冷重启 / 初始化的模型行（ADR-077 D5）。
        let empty = room_store(vec![]);

        // 面板搬走：R1 现在占 x 2000–3000，A（x 100–200）不在里面了。点名 DbSet@T = [A] → 回读库上几何。
        let after_move = room_calculator_with(dir.path(), 2000., None);
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &empty,
            BATCH_DBNUM,
            &after_move,
            Some(&[ROOM_ROOT_A]),
            RootScope::All,
        )
        .await
        .expect("recompute A from its durable rows");
        assert_eq!(
            (
                report.projected_roots,
                report.durable_roots,
                report.durable_records
            ),
            (0, 1, 1),
            "{report:?}"
        );
        assert_eq!(
            (
                report.roots,
                report.recomputed,
                report.room_edges,
                report.failed.len(),
                report.acks_refreshed
            ),
            (1, 1, 0, 0, 0),
            "回读的根算了、边为空、没有 ACK 可对齐: {report:?}"
        );
        assert_eq!(report.covered_roots, vec![ROOM_ROOT_A]);
        assert!(
            room_edges(&db).await.is_empty(),
            "A 不再在任何面板里，它的旧边随回读重算删掉"
        );
        assert_eq!(
            count_record(&db, "inst_relate", "24383_10").await,
            1,
            "回读重算不动模型行"
        );

        // 搬回来：边逐字回来——回读库的那条路与逐根落库同一形状。
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &empty,
            BATCH_DBNUM,
            &before_move,
            Some(&[ROOM_ROOT_A]),
            RootScope::All,
        )
        .await
        .expect("recompute A after the panel moved back");
        assert_eq!(
            (report.durable_roots, report.recomputed, report.room_edges),
            (1, 1, 1),
            "{report:?}"
        );
        assert_eq!(
            room_edges(&db).await,
            before,
            "整库重算（回读库）== 逐根落库，逐边相等"
        );

        // 点名里的 B 库上一行几何都没有（从没生成过）：算覆盖到、什么都不写；别的库的根不认。
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &empty,
            BATCH_DBNUM,
            &before_move,
            Some(&[ROOM_ROOT_B, ROOM_ROOT_A, RefNo::new(24384, 10)]),
            RootScope::All,
        )
        .await
        .expect("recompute with an ungenerated root named");
        assert_eq!(
            report.covered_roots,
            vec![ROOM_ROOT_A, ROOM_ROOT_B],
            "按 refno 序、只认本库: {report:?}"
        );
        assert_eq!(
            (
                report.durable_roots,
                report.durable_records,
                report.recomputed,
                report.room_edges
            ),
            (2, 1, 1, 1),
            "{report:?}"
        );
        assert_eq!(room_edges(&db).await, before);
    }

    /// ADR-073 D13 反查（D/3）：面板级的两类处置渲成一串 `;\n` 收尾的语句——不在册的面板清它的出边与
    /// `room_panel_relate` 入边、只改号的房间改列；要反查的那一类（`recalc`）不在这里。两个渲染器一个带句尾分号
    /// 一个不带，这里统一。
    #[test]
    fn panel_level_statements_clear_unregistered_panels_and_renumber_rooms_only() {
        use crate::fast_model::room_resident::{PanelBounds, PanelDelta, PanelMove, RoomRenumber};
        let delta = PanelDelta {
            removed: vec![refno_from_e3d(RefNo::new(24383, 777))],
            renumbered: vec![RoomRenumber {
                room: "24383_501".into(),
                old_num: "R1".into(),
                new_num: "R1X".into(),
                panels: vec![refno_from_e3d(PANEL_R1)],
            }],
            recalc: vec![PanelMove {
                panel: refno_from_e3d(PANEL_R2),
                old: None,
                new: Some(PanelBounds([[0.0; 3], [1.0; 3]])),
            }],
            geometry_changed: 1,
        };
        let sql = render_panel_delta_statements(&delta);
        assert_eq!(
            sql,
            "DELETE pe:24383_777->room_relate;\n\
             DELETE pe:24383_777<-room_panel_relate;\n\
             UPDATE (SELECT VALUE id FROM pe:24383_500->room_relate WHERE room_num = 'R1') SET room_num = 'R1X';\n\
             UPDATE (SELECT VALUE id FROM pe:24383_500<-room_panel_relate WHERE room_num = 'R1') SET room_num = 'R1X';\n"
        );
        assert!(!sql.contains("24383_600"), "要反查的面板不在面板级语句里");
        assert!(render_panel_delta_statements(&PanelDelta::default()).is_empty());
    }

    /// ADR-073 D13 反查（D/3），`mem://` 真测：**定向**重算只碰命中的根。A、B 都在 R1 里落库；面板搬家后只点名 A
    /// → A 的旧边删掉、ACK 换代，B 的边与 ACK**一字不动**（B 不在集合里，哪怕它同样陈旧——那是反查该不该命中它
    /// 的事，不是这里的）；面板级 prelude 与逐根语句同一串事务：不在册的 777 的出边与 `room_panel_relate` 入边
    /// 随第一笔一起清掉。再点名 B，B 也收敛——两次定向 == 一次整库。
    #[tokio::test]
    async fn a_targeted_recompute_rewrites_only_the_named_roots_and_runs_the_panel_statements_first()
     {
        use crate::fast_model::room_resident::PanelDelta;
        let dir = tempfile::tempdir().unwrap();
        let before_move = room_calculator_with(dir.path(), 0., None);
        let db = surreal_mem().await;
        let store = room_store(vec![
            root_commit(ROOM_ROOT_A, vec![room_record(ROOM_ROOT_A, 100., 200.)]),
            root_commit(ROOM_ROOT_B, vec![room_record(ROOM_ROOT_B, 300., 400.)]),
        ]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A, ROOM_ROOT_B],
            Some(&before_move),
        )
        .await
        .expect("seed A and B inside R1");
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24383_10".to_string(),
                "pe:24383_500->pe:24383_20".to_string(),
            ]
        );
        // 一块不在册了的面板 777 留下的旧边：指向 A 的成员边，以及房间 778 → 777 的拓扑边。
        db.query(
            "INSERT RELATION INTO room_relate [{ id: room_relate:stale_777, in: pe:⟨24383_777⟩, out: pe:⟨24383_10⟩, room_num: 'R9', inside_count: 8, center_dist: 0.0 }];\n\
             INSERT RELATION INTO room_panel_relate [{ id: room_panel_relate:778_777, in: pe:⟨24383_778⟩, out: pe:⟨24383_777⟩, room_num: 'R9' }];",
        )
        .await
        .expect("seed the stale edges of an unregistered panel")
        .check()
        .expect("seed statements");
        assert_eq!(room_edges(&db).await.len(), 3);
        assert_eq!(
            count(&db, "room_panel_relate").await,
            1,
            "逐根路径只对构件本身是面板的写 room_panel_relate；这里只有种进去的 778→777"
        );

        let after_move = room_calculator_with(dir.path(), 2000., None);
        let delta = PanelDelta {
            removed: vec![refno_from_e3d(RefNo::new(24383, 777))],
            ..PanelDelta::default()
        };
        let panel_statements = render_panel_delta_statements(&delta);
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &after_move,
            None,
            RootScope::Targeted {
                roots: &[ROOM_ROOT_A],
                panel_statements: &panel_statements,
            },
        )
        .await
        .expect("targeted recompute of A");
        assert_eq!(report.covered_roots, vec![ROOM_ROOT_A], "{report:?}");
        assert_eq!(
            (
                report.roots,
                report.projected_roots,
                report.durable_roots,
                report.recomputed,
                report.room_edges,
                report.failed.len(),
                report.panel_statements,
                report.commits,
                report.acks_refreshed
            ),
            (1, 1, 0, 1, 0, 0, 2, 1, 1),
            "{report:?}"
        );
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_20".to_string()],
            "A 的旧边删了、777 的旧边随 prelude 清了、B 的边一字不动"
        );
        assert_eq!(
            count(&db, "room_panel_relate").await,
            0,
            "778→777 那条拓扑边随 prelude 清掉"
        );
        let ack = |root: RefNo| {
            store
                .publication_ack(ProjectionScope::Current, root)
                .expect("ACK stays")
                .room_generation
        };
        assert_eq!(
            ack(ROOM_ROOT_A).as_deref(),
            Some(after_move.resident_generation())
        );
        assert_eq!(
            ack(ROOM_ROOT_B).as_deref(),
            Some(before_move.resident_generation()),
            "没点名的根 ACK 不动"
        );

        // 再点名 B：收敛到与整库重算相同的终态。
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &after_move,
            None,
            RootScope::Targeted {
                roots: &[ROOM_ROOT_B],
                panel_statements: "",
            },
        )
        .await
        .expect("targeted recompute of B");
        assert_eq!(report.covered_roots, vec![ROOM_ROOT_B]);
        assert_eq!(report.panel_statements, 0);
        assert!(room_edges(&db).await.is_empty(), "两次定向 == 一次整库");
    }

    /// ADR-073 D13 U3 b 档，`mem://` 两库端到端：面板 R1 在 A 库（7999）的任务里搬家。A 的反查（树的答案模拟：旧盒里
    /// 有 A1 与 B 库的 B1）只折出 A1、B1 进 `foreign[B]`；A 定向重算只碰 A1，B 的两条边一字不动；结论 `Targeted` 后把
    /// B1 连同这一段换代（g1 → g2）停进 B 的槽。B 的下一个任务：`Unchanged`、库上 = g1、常驻 = g2、槽里 g1 → g2 →
    /// 准入链接上、只有停放链；折成 B1 一根、只重算它——B1 的陈旧边删了、B2（只在 R2 里）的边与 ACK 一字不动；
    /// 与整库重算 B 同一终态；成功清槽。槽空（重启后）→ 整库；链断 → 整库。
    #[tokio::test]
    async fn a_panel_move_parks_the_other_librarys_hits_and_its_next_task_recomputes_only_those_roots()
     {
        use crate::data_interface::room_recompute::{RoomRecomputeScope, scope_for_task};
        use crate::data_interface::room_reverse::{
            self, ReverseLookup, RoomRoots, fold_parked, park_foreign_hits,
        };
        use crate::fast_model::room_resident::{PanelBounds, PanelDelta, PanelMove};
        use std::collections::{BTreeMap, BTreeSet};
        const LIB_B: u32 = RefNo::new(24382, 1).dbno();
        let b1 = RefNo::new(24382, 10);
        let b2 = RefNo::new(24382, 20);
        let record_in_b = |root: RefNo, min: f32, max: f32| ProjectedGeometry {
            dbnum: LIB_B,
            ancestors: vec![packed_refno(root), packed_refno(RefNo::new(24382, 1))],
            ..room_record(root, min, max)
        };

        let dir = tempfile::tempdir().unwrap();
        let before_move = room_calculator_with(dir.path(), 0., None);
        let db = surreal_mem().await;
        let store = room_store(vec![
            root_commit(ROOM_ROOT_A, vec![room_record(ROOM_ROOT_A, 100., 200.)]),
            root_commit(b1, vec![record_in_b(b1, 300., 400.)]),
            root_commit(b2, vec![record_in_b(b2, 1200., 1300.)]),
        ]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &[ROOM_ROOT_A],
            Some(&before_move),
        )
        .await
        .expect("seed A1 inside R1");
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            LIB_B,
            &[b1, b2],
            Some(&before_move),
        )
        .await
        .expect("seed B1 inside R1, B2 inside R2");
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24382_10".to_string(),
                "pe:24383_500->pe:24383_10".to_string(),
                "pe:24383_600->pe:24382_20".to_string(),
            ]
        );
        let ack = |root: RefNo| {
            store
                .publication_ack(ProjectionScope::Current, root)
                .expect("ACK stays")
                .room_generation
        };

        // R1 搬到 x 2000–3000：常驻 g1 → g2。
        let after_move = room_calculator_with(dir.path(), 2000., None);
        let g1 = before_move.resident_generation().to_string();
        let g2 = after_move.resident_generation().to_string();
        assert_ne!(g1, g2);
        let delta = PanelDelta {
            recalc: vec![PanelMove {
                panel: refno_from_e3d(PANEL_R1),
                old: Some(PanelBounds([[0., 0., 0.], [1000., 1000., 1000.]])),
                new: Some(PanelBounds([[2000., 0., 0.], [3000., 1000., 1000.]])),
            }],
            geometry_changed: 1,
            ..PanelDelta::default()
        };

        // A 的任务：反查（树的答案模拟——旧盒里有 A1 与 B1，新盒里没人）。离开的成员那一路从库上取（L1-P）：R1 此刻名下的
        // 成员边正是 A1 与 B1——与树的答案同一批，不靠旧盒也找得齐。
        let tree_hits = BTreeSet::from([packed_refno(ROOM_ROOT_A), packed_refno(b1)]);
        let members = room_reverse::durable_panel_members(&db, &delta.leaving_panels())
            .await
            .expect("members of R1");
        assert_eq!(members, tree_hits, "库上 R1 名下的成员 == 旧盒里的人");
        let ReverseLookup::Resolved(affected) = room_reverse::resolve(
            &db,
            &store,
            BATCH_DBNUM,
            &delta.query_boxes(),
            tree_hits,
            members,
            &[ROOM_ROOT_A],
        )
        .await
        .expect("reverse lookup") else {
            panic!("树就绪与否不在这一半判")
        };
        assert_eq!(affected.roots, vec![ROOM_ROOT_A]);
        assert_eq!(affected.member_hits, 2);
        assert_eq!(
            affected.foreign,
            BTreeMap::from([(
                LIB_B,
                BTreeMap::from([(
                    packed_refno(b1),
                    vec![packed_refno(b1), packed_refno(RefNo::new(24382, 1))]
                )])
            )]),
            "B1 是 B 库的：存元素带祖先链、不折根"
        );
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &after_move,
            None,
            RootScope::Targeted {
                roots: &affected.roots,
                panel_statements: "",
            },
        )
        .await
        .expect("A's targeted recompute");
        assert_eq!(report.covered_roots, vec![ROOM_ROOT_A]);
        assert_eq!(
            room_edges(&db).await,
            vec![
                "pe:24383_500->pe:24382_10".to_string(),
                "pe:24383_600->pe:24382_20".to_string(),
            ],
            "A1 的边删了；B1 的边此刻陈旧却还在——A 的任务只碰 A 的根"
        );
        // 结论 Targeted → 停放给 B（执行体在 `run_targeted_room_recompute` 的那一臂里做）。
        let lines = park_foreign_hits(BATCH_DBNUM, &g1, &g2, &affected.foreign);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains(&format!("dbnum={LIB_B} 停放 1 个元素")),
            "{lines:?}"
        );

        // B 的下一个任务：没换代、库上 g1、冻 g2、槽里 g1 → g2 → 定向，只有停放链（账本空——这里没经 install 记段）。
        let parked = room_reverse::pending(LIB_B);
        assert_eq!(parked.len(), 1);
        assert_eq!(
            (
                parked[0].from_generation.as_str(),
                parked[0].to_generation.as_str(),
                parked[0].parked_by
            ),
            (g1.as_str(), g2.as_str(), BATCH_DBNUM)
        );
        assert_eq!(
            scope_for_task(Some(&g1), Some(&g2), &[], &parked),
            RoomRecomputeScope::Targeted {
                swaps: Vec::new(),
                parked: parked.clone(),
                generation: g2.clone(),
            }
        );
        let (roots, unresolved) = fold_parked(&parked, &[b1, b2]);
        assert_eq!(
            (roots.as_slice(), unresolved.as_slice()),
            ([b1].as_slice(), [].as_slice())
        );
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            LIB_B,
            &after_move,
            None,
            RootScope::Targeted {
                roots: &roots,
                panel_statements: "",
            },
        )
        .await
        .expect("B's targeted recompute from the parked chain");
        assert_eq!(report.covered_roots, vec![b1]);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_600->pe:24382_20".to_string()],
            "B1 的陈旧边删了；B2 一字不动"
        );
        assert_eq!(ack(b1).as_deref(), Some(g2.as_str()));
        assert_eq!(ack(b2).as_deref(), Some(g1.as_str()), "没点名的根 ACK 不动");
        // 成功 → 只清消费过的那一段（执行体在 `run_room_recompute` 按结论做；快照 = 接进准入链的 `parked`，L3）。
        assert_eq!(room_reverse::clear_consumed(LIB_B, &parked), (1, 0));
        assert!(room_reverse::pending(LIB_B).is_empty());
        // 对照：整库重算 B 得到同一终态。
        recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            LIB_B,
            &after_move,
            Some(&[b1, b2]),
            RootScope::All,
        )
        .await
        .expect("whole-library recompute of B");
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_600->pe:24382_20".to_string()],
            "定向（停放链）== 整库"
        );

        // 槽空、账本空（重启后都丢了）→ 整库；链断（槽里只有 g0 → g1，接不到 g2）→ 整库。
        assert_eq!(
            scope_for_task(Some(&g1), Some(&g2), &[], &[]),
            RoomRecomputeScope::WholeLibrary
        );
        room_reverse::park(
            LIB_B,
            RoomRoots {
                from_generation: "g0".into(),
                to_generation: g1.clone(),
                elements: BTreeMap::from([(packed_refno(b2), vec![packed_refno(b2)])]),
                parked_by: BATCH_DBNUM,
                parked_at: std::time::Instant::now(),
            },
        );
        assert_eq!(
            scope_for_task(Some(&g1), Some(&g2), &[], &room_reverse::pending(LIB_B)),
            RoomRecomputeScope::WholeLibrary
        );
        room_reverse::clear(LIB_B);
    }

    /// H1 / M1（2026-09-19，ADR-073 D13 追记）`mem://` 端到端：定向路的计算器是**本任务开工冻的那台**，scope、边、盖的代号
    /// 三者同一代；任务中途常驻又换了代（`/model/ensure` 并发对齐 g2 → g3，面板 Q 又动了）不许把 scope 按 g2 定的
    /// 命中根用 g3 算、再盖 g3——那会让 g2 → g3 命中的根永久陈旧、③ 哑掉。换代段账本（M1）让 ensure 那一次重建记下的
    /// g2 → g3 也能被下一个任务接上：T2 不再整库。
    ///
    /// 夹具：A1 在 R1@0 里、A2 在 R1@2000 里、A3 在 R1@4000 里；g1 / g2 / g3 = R1 在三处的三代。库上 g1（三根都按 g1 算过）。
    /// 任务 T1：对齐 g1 → g2（P = R1 0–1000 → 2000–3000，记进账本）、冻 g2；中途 ensure 把常驻换到 g3
    /// （Q = R1 2000–3000 → 4000–5000，也记进账本）。
    /// 断言：① 准入按冻的那一代：账本 g1 → g2 + 冻 g2 → 定向、终点 g2；账本只有 g1 → g2 + 冻 g3 → 整库；账本有 g2 → g3 + 冻 g3
    /// → 两段定向；② 反查 P 命中 A1 / A2、按 **g2** 重算 → A1 的边删、A2 的边补、A3 不动；③ 盖 g2 之后库上 g2 ≠ 常驻 g3，
    /// ③ 仍欠（盖 g3 就不欠了——这正是漏算）；④ 下一个任务 T2（Unchanged、库上 g2、冻 g3、槽空、账本有 g2 → g3）→ 定向沿
    /// 账本接 Q、反查命中 A2 / A3、按 g3 算 → 只剩 R1→A3，== 整库按 g3 == 从零。
    #[tokio::test]
    async fn a_mid_task_generation_swap_keeps_the_targeted_recompute_on_the_task_calculator() {
        use crate::data_interface::room_recompute::{
            PanelSwap, RoomRecomputeScope, library_alignment_owed, scope_for_task,
        };
        use crate::data_interface::room_reverse::{self, ReverseLookup};
        use crate::fast_model::room_resident::{PanelBounds, PanelDelta, PanelMove};
        use std::collections::BTreeSet;
        let a1 = ROOM_ROOT_A;
        let a2 = RefNo::new(24383, 30);
        let a3 = RefNo::new(24383, 40);
        let library_roots = [a1, a2, a3];

        let dir = tempfile::tempdir().unwrap();
        let calc_g1 = room_calculator_with(dir.path(), 0., None);
        let calc_g2 = room_calculator_with(dir.path(), 2000., None);
        let calc_g3 = room_calculator_with(dir.path(), 4000., None);
        let g1 = calc_g1.resident_generation().to_string();
        let g2 = calc_g2.resident_generation().to_string();
        let g3 = calc_g3.resident_generation().to_string();
        assert!(g1 != g2 && g2 != g3 && g1 != g3, "三代代号两两不同");

        let db = surreal_mem().await;
        let store = room_store(vec![
            root_commit(a1, vec![room_record(a1, 100., 200.)]),
            root_commit(a2, vec![room_record(a2, 2100., 2200.)]),
            root_commit(a3, vec![room_record(a3, 4100., 4200.)]),
        ]);
        persist_roots_batched(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &library_roots,
            Some(&calc_g1),
        )
        .await
        .expect("seed the library at g1");
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_10".to_string()],
            "库上 g1：只有 A1 在 R1@0 里"
        );
        let ack = |root: RefNo| {
            store
                .publication_ack(ProjectionScope::Current, root)
                .expect("ACK stays")
                .room_generation
        };

        // T1 开工前对齐：g1 → g2，面板 P 动了——这一段进账本（`ResidentRooms::install` 记的，这里直接造）。
        let p = PanelDelta {
            recalc: vec![PanelMove {
                panel: refno_from_e3d(PANEL_R1),
                old: Some(PanelBounds([[0., 0., 0.], [1000., 1000., 1000.]])),
                new: Some(PanelBounds([[2000., 0., 0.], [3000., 1000., 1000.]])),
            }],
            geometry_changed: 1,
            ..PanelDelta::default()
        };
        let own = PanelSwap {
            from_generation: g1.clone(),
            to_generation: g2.clone(),
            panels: p.clone(),
        };
        // ① 准入按本任务冻的那一代（`durable_alignment` 收的就是它）：账本 g1 → g2、冻 g2 → 定向、终点 g2。
        let scope = scope_for_task(Some(&g1), Some(&g2), &[own.clone()], &[]);
        assert_eq!(
            scope,
            RoomRecomputeScope::Targeted {
                swaps: vec![own.clone()],
                parked: Vec::new(),
                generation: g2.clone(),
            }
        );
        assert_eq!(scope.targeted_generation(), Some(g2.as_str()));
        // 对齐装上 g2、冻计算器时常驻却已是 g3（ensure 插在两次读之间）、账本里还没有 g2 → g3：接不到冻的那一代，整库。
        assert_eq!(
            scope_for_task(Some(&g1), Some(&g3), &[own.clone()], &[]),
            RoomRecomputeScope::WholeLibrary,
            "账本只有 g1 → g2 却冻了 g3：接不到冻的那一代"
        );
        // 收尾那一步的同一道门（`run_targeted_room_recompute` 入口）：终点 g2 ≠ 中途换上的 g3，定不了。
        assert_ne!(
            scope.targeted_generation(),
            Some(calc_g3.resident_generation()),
            "此刻常驻（g3）不是准入链的终点，收尾不许拿它算"
        );
        // ensure 中途重建 g2 → g3（面板 Q 动了）也记进账本；冻的若是 g3，两段接起来定向（M1）。
        let q = PanelDelta {
            recalc: vec![PanelMove {
                panel: refno_from_e3d(PANEL_R1),
                old: Some(PanelBounds([[2000., 0., 0.], [3000., 1000., 1000.]])),
                new: Some(PanelBounds([[4000., 0., 0.], [5000., 1000., 1000.]])),
            }],
            geometry_changed: 1,
            ..PanelDelta::default()
        };
        let ensure = PanelSwap {
            from_generation: g2.clone(),
            to_generation: g3.clone(),
            panels: q.clone(),
        };
        let ledger = [own.clone(), ensure.clone()];
        assert_eq!(
            scope_for_task(Some(&g1), Some(&g3), &ledger, &[]),
            RoomRecomputeScope::Targeted {
                swaps: vec![own.clone(), ensure.clone()],
                parked: Vec::new(),
                generation: g3.clone(),
            },
            "账本里两段接起来：库上 g1 → 冻的 g3，两段都反查、按 g3 算"
        );

        // ② 反查 P（树的答案模拟：旧盒里 A1、新盒里 A2；库上 R1 名下此刻只有 A1——离开的成员那一路，L1-P）→ 只重算
        // A1 / A2，按**开工冻的 g2**。
        let tree_hits = BTreeSet::from([packed_refno(a1), packed_refno(a2)]);
        let members = room_reverse::durable_panel_members(&db, &p.leaving_panels())
            .await
            .expect("members of R1");
        assert_eq!(members, BTreeSet::from([packed_refno(a1)]));
        let ReverseLookup::Resolved(affected) = room_reverse::resolve(
            &db,
            &store,
            BATCH_DBNUM,
            &p.query_boxes(),
            tree_hits,
            members,
            &library_roots,
        )
        .await
        .expect("reverse lookup") else {
            panic!("树就绪与否不在这一半判")
        };
        assert_eq!(affected.roots, vec![a1, a2]);
        assert!(affected.unresolved.is_empty());
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &calc_g2,
            None,
            RootScope::Targeted {
                roots: &affected.roots,
                panel_statements: "",
            },
        )
        .await
        .expect("T1's targeted recompute on the task calculator");
        assert_eq!(report.covered_roots, vec![a1, a2]);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_30".to_string()],
            "按 g2：A1 离开 R1、A2 进来；A3 没点名，一字不动"
        );
        assert_eq!(ack(a1).as_deref(), Some(g2.as_str()));
        assert_eq!(ack(a2).as_deref(), Some(g2.as_str()));
        assert_eq!(ack(a3).as_deref(), Some(g1.as_str()), "没点名的根 ACK 不动");

        // ③ 结论 `Targeted(g2)` → 库上盖 g2：此刻常驻 g3 ≠ g2，③ 仍欠着——g2 → g3 那一段（A3 进 R1）由下一个任务补。
        let stamped = calc_g2.resident_generation();
        assert_eq!(stamped, g2);
        assert!(
            library_alignment_owed(Some(stamped), Some(&g3)).is_some(),
            "盖的是边真正用的那一代，③ 才看得见 g2 → g3 还欠着"
        );
        assert!(
            library_alignment_owed(Some(&g3), Some(&g3)).is_none(),
            "反例：按此刻常驻盖 g3，③ 就哑了——A3 的边永远补不上（这正是 H1）"
        );

        // ④ T2：Unchanged（g3 早已装上）、库上 g2、冻 g3、槽空、账本有 ensure 记的 g2 → g3 → 定向沿账本接 Q（M1）；
        // 账本空（重启后）才整库。
        assert_eq!(
            scope_for_task(Some(&g2), Some(&g3), &[], &[]),
            RoomRecomputeScope::WholeLibrary,
            "账本空：整库兜底"
        );
        let scope = scope_for_task(Some(&g2), Some(&g3), &ledger, &[]);
        assert_eq!(
            scope,
            RoomRecomputeScope::Targeted {
                swaps: vec![ensure.clone()],
                parked: Vec::new(),
                generation: g3.clone(),
            },
            "T2 自己没重建（Unchanged），沿 ensure 记的那一段定向"
        );
        // 反查 Q（树的答案模拟：旧盒里 A2、新盒里 A3；库上 R1 名下此刻是 A2）→ 只重算 A2 / A3，按冻的 g3。
        let tree_hits = BTreeSet::from([packed_refno(a2), packed_refno(a3)]);
        let members = room_reverse::durable_panel_members(&db, &q.leaving_panels())
            .await
            .expect("members of R1");
        assert_eq!(members, BTreeSet::from([packed_refno(a2)]));
        let ReverseLookup::Resolved(affected) = room_reverse::resolve(
            &db,
            &store,
            BATCH_DBNUM,
            &q.query_boxes(),
            tree_hits,
            members,
            &library_roots,
        )
        .await
        .expect("reverse lookup") else {
            panic!("树就绪与否不在这一半判")
        };
        assert_eq!(affected.roots, vec![a2, a3]);
        let report = recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &calc_g3,
            None,
            RootScope::Targeted {
                roots: &affected.roots,
                panel_statements: "",
            },
        )
        .await
        .expect("T2's targeted recompute along the ledger segment");
        assert_eq!(report.covered_roots, vec![a2, a3]);
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_40".to_string()],
            "g2 → g3 那一段补上：A2 离开、A3 进来；A1 没点名、一字不动"
        );
        assert_eq!(ack(a1).as_deref(), Some(g2.as_str()), "没点名的根 ACK 不动");
        assert_eq!(ack(a3).as_deref(), Some(g3.as_str()));
        assert!(
            library_alignment_owed(Some(&g3), Some(&g3)).is_none(),
            "定向算齐、盖 g3，③ 安静"
        );
        // 对照：整库按 g3 得到同一终态。
        recompute_dbnum_room_edges(
            PersistTarget::new(&db, PersistMedium::Mem),
            &store,
            BATCH_DBNUM,
            &calc_g3,
            Some(&library_roots),
            RootScope::All,
        )
        .await
        .expect("whole-library recompute on g3");
        assert_eq!(
            room_edges(&db).await,
            vec!["pe:24383_500->pe:24383_40".to_string()],
            "两段定向（T1 按 g2、T2 沿账本按 g3）== 整库按 g3"
        );
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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "ready");
        assert_eq!(
            publication_status_of(&store, ROOM_ROOT_B).await,
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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "stale");
        assert_eq!(publication_status_of(&store, ROOM_ROOT_B).await, "ready");

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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_B).await, "ready");
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
        assert_eq!(publication_status_of(&store, ancestor_root).await, "stale");
        assert_eq!(publication_status_of(&store, nested_root).await, "ready");
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
                    catalogue_sessions: None,
                    room_generation: Default::default(),
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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "ready");

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
        assert_eq!(publication_status_of(&store, ROOM_ROOT_A).await, "ready");
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
        // 根内：几何按条数切块渲好 → 边拼进这一根的末块 → 逐笔提交。回执**不落库**（ADR-079）：
        // 收尾段是空的，「这根成了」在全部笔提交之后进进程内账本（`accept_group_outcome`）。
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
        let pack_at = group
            .find("pack_publication_transactions(&prelude, &pieces, \"\", touches_geometry)")
            .expect("装块时收尾段为空：没有逐根的章要盖");
        // 提交走 `commit_publication_transaction`（整笔原子 + 冲突重试），钉它的调用处。
        let commit_at = group
            .find("commit_publication_transaction(db, statements")
            .expect("提交");
        assert!(
            render_at < attach_at && attach_at < pack_at && pack_at < commit_at,
            "顺序：渲几何 → 房间边 → 装块 → 提交"
        );
        assert!(
            !group.contains("type::thing('gen_root'") && !group.contains("receipt_body"),
            "persist_group 不再渲任何逐根回执行（ADR-079）"
        );
        let accept = production
            .split_once("fn accept_group_outcome(")
            .expect("accept_group_outcome exists")
            .1
            .split_once("\n}\n")
            .expect("accept end")
            .0;
        assert!(
            accept.contains("record_root_generated(root.share.root, &root.share.records, receipt)"),
            "提交之后逐根成败进进程内账本: {accept}"
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
        // B/T4：回执带来的边先过三道闸（`carried_edges_for`：代号相等、构件集相等、带了），过了只渲染；
        // `calculate_for_root(` 只剩兑底那一臂，渲染仍是同一台 `render_root_room_edges`。
        assert!(
            stage.contains("carried_edges_for(")
                && stage.contains("RoomEdgeSource::Carried")
                && stage.contains("RoomEdgeSource::Recomputed"),
            "room_stage 先问回执带来的边，再兑底现算: {stage}"
        );
        assert_eq!(
            stage.matches("calculate_for_root(").count(),
            1,
            "现算只剩兑底那一臂: {stage}"
        );
        assert_eq!(
            stage.matches("render_root_room_edges(").count(),
            1,
            "带来的与现算的边都经同一台渲染器: {stage}"
        );
        let gate = production
            .split_once("fn carried_edges_for<")
            .expect("三道闸")
            .1
            .split_once("\n}\n")
            .expect("gate end")
            .0;
        assert!(
            gate.contains("carried.generation != resident_generation")
                && gate.contains("elements.len() != listed.len()"),
            "闸：代号相等、构件集相等（少一个多一个都不行）: {gate}"
        );
        // 生产调用方：dbnum 任务在开工时冻一次（`TaskRoomCalculator::freeze()`，一任务一台，B/T1）；
        // 没有任务语境的 process-store 发布仍按组冻一次（`frozen_room_calculator()`）。
        for (file, source, freeze) in [
            (
                "model_refresh.rs",
                include_str!("../data_interface/model_refresh.rs"),
                "frozen_room_calculator()",
            ),
            (
                "batch_worker.rs",
                include_str!("../data_interface/batch_worker.rs"),
                "TaskRoomCalculator::freeze()",
            ),
        ] {
            let production = source
                .split(concat!("#[cfg(test)]\n", "mod tests"))
                .next()
                .expect("production half");
            assert!(
                production.contains(freeze),
                "{file}: 生产调用方要在任务开工时冻一次房间计算器（{freeze}）"
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

    /// 逐根凭证行退役（ADR-079）之后，「这根这一轮成了没成」看进程内的发布 ACK：
    /// 有 ACK 是 `ready`，没有是 `stale`——与退役前 `gen_root.publication_status` 同一口径。
    async fn publication_status_of(store: &ModelMemoryStore, root: RefNo) -> &'static str {
        if store.publication_ack(ProjectionScope::Current, root).is_some() {
            "ready"
        } else {
            "stale"
        }
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
