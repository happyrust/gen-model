//! On-demand model generation service.
//!
//! The viewer calls this service when a requested reference has no renderable
//! model. Concurrent misses for the same delivery unit are coalesced: after
//! acquiring the unit lock the service rechecks the database, so only the first
//! request performs generation.

use aios_core::{RefnoEnum, SUL_DB};
use serde::{Deserialize, Serialize};

use crate::data_interface::generation_root::{
    configured_delivery_unit_types, is_coarse_hierarchy_noun, resolve_live_element_generation_root,
};
use crate::data_interface::manual_update::{generate_unit_model, generation_root_lock};
use crate::data_interface::model_coverage::{
    generation_root_cache_current, generation_roots_cache_current,
};
use crate::data_interface::mesh_purge::{self, ClearGeometry, MeshPurgeReport};
use crate::data_interface::model_scope_regen;
use crate::data_interface::room_read_through::SessionRule;
use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnDemandModelStatus {
    AlreadyAvailable,
    Generated,
    /// 生成跑完了却没有一条画得出来：写出的实例全都画不出来，或者本来就一条都没
    /// 写出（无子件的 BRAN、纯作层级用的 STRU）。这是这份数据的终局而不是一次
    /// 失败：重发同一个请求只会重跑一遍生成，结果一模一样。
    NoRenderableGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnDemandModelResult {
    pub requested_refno: String,
    pub generation_root: String,
    pub generation_root_noun: String,
    pub status: OnDemandModelStatus,
    pub model_available: bool,
    /// Instances the viewer can draw.
    pub model_instance_count: usize,
    /// Instances written for the subtree, drawable or not. The two differ
    /// exactly when generation produced rows the viewer drops.
    pub generated_instance_count: usize,
    /// 这个节点范围覆盖的去重后生成根数。
    #[serde(default)]
    pub generation_root_count: usize,
    /// Exact roots covered by a scope request.  For a single-root request this
    /// contains the same value as `generation_root`.
    #[serde(default)]
    pub generation_roots: Vec<String>,
    /// 本次凭最新数据完成凭证直接复用的生成根数。
    #[serde(default)]
    pub cached_root_count: usize,
    /// 本次确实执行生成的生成根数。
    #[serde(default)]
    pub generated_root_count: usize,
    /// 本次范围里报错被跳过的生成根数。范围生成逐根记错、其余根照常跑完（不再首错中止整片），
    /// 客户端拿 `failed_root_count > 0` 以 `force = false` 重发同一范围补齐缺口。
    #[serde(default)]
    pub failed_root_count: usize,
    /// 失败根点名：refno + 首错摘要。单根路径恒为空——单根失败本来就是整个请求失败，
    /// 直接走 `Err`。
    #[serde(default)]
    pub failed_roots: Vec<ScopeRootFailure>,
}

/// 范围生成里没跑出来的单根记录。`error` 是 `{error:#}` 形态的首错摘要：只进日志与详情，
/// 分型仍走请求级错误（全灭时首错原样上抛，`ensure_error` 按域分型）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRootFailure {
    /// 没生成出来的生成根。
    pub generation_root: String,
    /// 首错摘要。
    pub error: String,
    /// 重发同一请求有没有意义：根锁被占 / 刚生成完被判过期 / 初始化门还没放行——这些等一等
    /// 再发就补齐；几何真坏之类的终局失败标 false，客户端不必为它干等整段补齐窗口。
    #[serde(default)]
    pub retryable: bool,
}

impl AiosDBManager {
    /// 清掉 `requested_refno` 范围内已发布的产物——**范围清理的唯一实现**，两条入口共用：
    /// 右键「重新生成模型」（`force = true`）在逐根重算之前先调它（2026-10-10 定：重新生成 =
    /// 先删后生成），右键「清理模型」（`DELETE /model/subtree`，`handlers::model_delete_subtree`）
    /// 只调它、不生成。`roots` 是调用方从 e3d-io 骨架解出的范围内生成根（只用来拿锁）。
    ///
    /// 不清直接重算是替换不掉库里产物的：生成流水线拿着旧回执会走两条捷径——「目标没变、投影完整
    /// → 只重盖章」（`e3d_model_service::cached_root_commit`）和「清单逐位没变 → 几何一条不动」
    /// （`project_geometry_delta`）——`force` 只跳过了凭证命中，没跳过它们；根已经解不出来的旧产物
    /// （孤儿）更没人删。清掉之后每根的 `prior` 为空、回执为空，只能整根重新发布。
    ///
    /// 清的范围是整个请求子树（记录按祖先链取），与确认框那句「范围内已有的产物会被重算替换」
    /// 同一口径。形态分三路：
    /// - 「只写内存」作用域（`model_read_route` R2，读透形态下 `/model/generate` 一律在此作用域里）：
    ///   只清进程内投影、不碰 durable 权威库、不过初始化门——与同作用域下的生成路径同一口径；
    /// - 读透形态：同上，另把同一笔删除渲成墓碑进 mirror spool（它的持久面）；
    /// - 摄入形态走 [`Self::delete_model_subtree`]（库 + 投影 + 账本，过初始化门）。
    ///
    /// 三路清之前都先对范围内的生成根 `try_generation_root`（投影两路在删除入口 `projection_removal`
    /// 里拿，摄入形态在 `delete_model_subtree` 里拿），被占就按「正在后台生成」回 409、一行不删。投影删法自己只等库级锁，而同库每一次
    /// 生成全程都攥着那把锁：不先拿根锁的话，它会排队等完同库任何在飞的生成，再把别人刚发布的
    /// 那一根清掉重算。根锁清完即放，之后逐根生成各自再拿——与摄入形态同一个窗口。
    ///
    /// 代价（有意为之）：清掉到重新发布之间，这个范围在 `/model/records` 上是空的；某根重算失败时
    /// 它的旧产物也不再保留（原来 `IncompleteRoot::Discard` 会「保旧」），回执按失败报、要重试再点一次。
    ///
    /// `geometry = Purge`（`purge_meshes=true`；`force = true` 的先删一律如此）时连这片范围引用过的
    /// 几何一起清：键在删行**之前**收（行一删就再也收不出来；收不动整趟中止、一行不删），
    /// `inst_geo` 行与 `.mesh` 在删行**之后**清（反过来会先把模型还要用的几何删掉而产物还在）。
    /// 库是权威面才有权威行可收、可删；「只写内存」/ 读透只有投影那一份，只删文件。口径与
    /// 共享键的取舍见 [`mesh_purge`] 模块文档。返回那一笔账（`Keep` 时 `None`）。
    pub async fn clear_model_scope(
        &self,
        requested_refno: RefnoEnum,
        roots: &[crate::data_interface::generation_root::GenerationRoot],
        geometry: ClearGeometry,
    ) -> anyhow::Result<Option<MeshPurgeReport>> {
        let memory_only = crate::data_interface::model_read_route::memory_only_scope().is_some();
        let read_through = crate::options::data_face().is_read_through();
        // 库是权威面才有权威行可收、可删；「只写内存」/ 读透只有投影那一份（也不该在
        // `scope_memory_only` 作用域里回头去碰那个库）。
        let durable = !(memory_only || read_through);
        // 几何键在删行之前收：行一删掉，库与投影都再也说不出这片范围引用过哪些网格。
        // 收不动就整趟中止（一行不删）——半成品比不删更难收拾。
        let keys = if geometry.purges() {
            Some(mesh_purge::scope_mesh_ids(requested_refno, durable).await?)
        } else {
            None
        };
        if memory_only || read_through {
            self.clear_projected_scope(requested_refno, roots, read_through)
                .await?;
        } else {
            self.delete_model_subtree(requested_refno).await?;
        }
        // 几何排在产物行之后：反过来会先把模型还要用的几何删掉、而产物还在。
        match keys {
            Some(keys) => Ok(Some(
                mesh_purge::purge_geometry(&SUL_DB, keys, durable).await?,
            )),
            None => Ok(None),
        }
    }

    /// 「只写内存」/ 读透那一路的范围清理，交给删除入口 `projection_removal::remove_subtree`：范围内的生成根
    /// 逐个 try-lock（没解出根就对请求节点自己拿），被占回 409、一行不删；拿齐再等库级锁、清投影。根锁清完即放。
    /// 从 [`Self::clear_model_scope`] 与读透形态的 [`Self::delete_model_subtree`] 两处进，投影删法只有这一条路。
    async fn clear_projected_scope(
        &self,
        requested_refno: RefnoEnum,
        roots: &[crate::data_interface::generation_root::GenerationRoot],
        spool_tombstone: bool,
    ) -> anyhow::Result<()> {
        use crate::data_interface::projection_removal::{self, RemovalRequest, TombstoneSink};
        projection_removal::remove_subtree(RemovalRequest {
            subtree: requested_refno,
            lock_roots: roots.iter().map(|root| root.root.to_pdms_str()).collect(),
            tombstone: if spool_tombstone {
                TombstoneSink::Spool
            } else {
                TombstoneSink::None
            },
        })
        .await?;
        Ok(())
    }

    /// Delete the generated-model data rooted at exactly `requested_refno`.
    ///
    /// Root resolution is used only to share the same concurrency locks as
    /// generation. The deletion scope remains the requested PE subtree.
    ///
    /// 摄入形态的删法（库 + 投影 + 账本，过初始化门）。读透形态没有 `pe` 可解根、也没有权威库可删，
    /// 转给 [`Self::clear_projected_scope`]（对请求节点 try-lock 后只清投影、进 spool 墓碑）——
    /// 不在这里另写一份投影删法。HTTP 入口 `DELETE /model/subtree` 不直接调这里，走
    /// [`Self::clear_model_scope`]（带骨架解出的根与读路由作用域）；这里剩下的直接调用方是增量窗口的
    /// `DeleteCleanup` 与 CLI，都是摄入形态语义。
    pub async fn delete_model_subtree(&self, requested_refno: RefnoEnum) -> anyhow::Result<()> {
        crate::data_interface::initialization_phase::require_model_generation()?;

        if crate::options::data_face().is_read_through() {
            return self.clear_projected_scope(requested_refno, &[], true).await;
        }

        let mut subtree =
            crate::data_interface::helper::collect_pe_subtree_refnos(&[requested_refno])
                .await?
                .into_iter()
                .collect::<Vec<_>>();
        subtree.sort_by_key(RefnoEnum::to_string);

        let unit_types = configured_delivery_unit_types();
        let mut lock_roots = crate::data_interface::generation_root::resolve_generation_roots_on(
            &SUL_DB,
            &subtree,
            &unit_types,
        )
        .await?
        .into_iter()
        .map(|root| root.root.to_pdms_str())
        .collect::<Vec<_>>();
        if lock_roots.is_empty() {
            lock_roots.push(requested_refno.to_pdms_str());
        }
        lock_roots.sort_unstable();
        lock_roots.dedup();

        let mut guards = Vec::with_capacity(lock_roots.len());
        for root in &lock_roots {
            guards.push(try_generation_root(root)?);
        }

        for root in &lock_roots {
            let (word0, word1) = root
                .split_once('/')
                .ok_or_else(|| anyhow::anyhow!("invalid generation root {root}"))?;
            let root = e3d_io::RefNo::new(word0.parse()?, word1.parse()?);
            crate::fast_model::e3d_model_service::delete_persisted_geometry_root(root).await?;
            let tombstone = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
                root,
                &[root],
                &[],
            );
            SUL_DB.query(tombstone).await?.check()?;
        }
        crate::data_interface::helper::delete_inst_relate_subtree(&[requested_refno], 300).await?;

        // 库里没有逐根凭证行可删（`gen_root` 退役，ADR-079）：这些根在进程内账本上的那一条摘掉，
        // 别让「上一版成了」的记录把刚删掉的几何当成还在。
        for root in &lock_roots {
            if let Some(refno) = crate::data_interface::model_root_ledger::parse_root(root) {
                let dbnum = crate::data_interface::generation_root::refno_to_e3d(refno).dbno();
                crate::data_interface::model_root_ledger::forget_root(dbnum, refno);
            }
        }
        Ok(())
    }

    /// Ensure that `requested_refno` has a renderable model.
    ///
    /// A missing model is normalized through the shared generation-root policy,
    /// then generated once through the same backend path used by manual updates.
    ///
    /// `force` re-runs generation on a root that has already settled. Without it
    /// a root whose generation produced nothing drawable is reported as such
    /// rather than regenerated on every request.
    pub async fn ensure_model_generated(
        &self,
        requested_refno: RefnoEnum,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        let (root, root_noun) = resolve_generation_root(requested_refno).await?;
        self.ensure_exact_generation_root(requested_refno, root, &root_noun, force)
            .await
    }

    /// Generate a root that has already been resolved by the authoritative
    /// hierarchy source. Direct mode obtains this tuple from e3d-io and must not
    /// send it back through the Surreal owner-chain resolver, which can select a
    /// different (usually coarser) root.
    async fn ensure_exact_generation_root(
        &self,
        requested_refno: RefnoEnum,
        root: RefnoEnum,
        root_noun: &str,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        // Detached producers must never lose an active transaction or frozen window.
        if super::batch_worker::active_data_window().is_some()
            || super::staging::write_context::active_staging_writes().is_some()
        {
            return self.ensure_exact_generation_root_inner(requested_refno, root, root_noun, force).await;
        }
        static FLIGHTS: std::sync::OnceLock<super::generation_flight::Flights<OnDemandModelResult>> = std::sync::OnceLock::new();
        let version = generation_source_identity(root).await?;
        let memory_scope = super::model_read_route::memory_only_scope();
        let key = format!("{}|{}|{}|{}|{}|{:?}", self.db_option.project_name,
            self.db_option.mdb_name, root.to_pdms_str(), force, version, memory_scope);
        let manager = self.clone();
        let noun = root_noun.to_string();
        let future = FLIGHTS.get_or_init(Default::default).run(key, async move {
            let work = manager.ensure_exact_generation_root_inner(root, root, &noun, force);
            match memory_scope {
                Some(dbnum) => super::model_read_route::scope_memory_only(dbnum, work).await,
                None => work.await,
            }
        });
        let mut result = tokio::time::timeout(std::time::Duration::from_secs(110), future).await
            .map_err(|_| ModelGenerationInProgress { root_refno: root.to_pdms_str() })??;
        // 两道后验失败都不是「正在后台生成」：生成已经完成，只是世界变了（源文件在生成期间前移）
        // 或判据说这根不算新。报成 `ModelGenerationInProgress` 会把这两种都藏在同一句 409 里
        // （2026-09-19 字典库会话位 0 那一桩就是这么被盖了一天）。
        anyhow::ensure!(generation_source_identity(root).await? == version,
            ModelStaleAfterGeneration {
                root_refno: root.to_pdms_str(),
                reason: "源文件版本在生成期间前移",
            });
        anyhow::ensure!(memory_root_current(root, SessionRule::NotBehind).await?,
            ModelStaleAfterGeneration {
                root_refno: root.to_pdms_str(),
                reason: "投影回执的模型目标核对不过，原因见日志 [模型目标核对]",
            });
        result.requested_refno = requested_refno.to_pdms_str();
        Ok(result)
    }

    async fn ensure_exact_generation_root_inner(
        &self,
        requested_refno: RefnoEnum,
        root: RefnoEnum,
        root_noun: &str,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        crate::data_interface::model_rebuild::reject_ensure_during_rebuild(root).await?;
        let root_refno = root.to_pdms_str();
        let _guard = try_generation_root(&root_refno)?;

        // 「只写内存」作用域（plan 2026-09-06 §8 R2）：这个库在 rocksdb 里还没准备好，模型面的
        // 状态只在进程内投影里——判「已可用」、数 renderable、都只问投影；durable pending 与
        // 初始化门一概不碰（它们都是持久层上的事）。判据本身在 `model_read_route`，这里只认
        // 作用域，不重拼。
        let memory_only = crate::data_interface::model_read_route::memory_only_scope().is_some();
        // Never report a partially-written model while another generation owns
        // this root. Once the lock is ours, a completed prior run is safe to reuse.
        // A durable credential survives restart, but /model/records serves the
        // process projection: a root only counts as cached once the projection holds it.
        // 两半各答各的（ADR-089 D1）：库级承诺（`generation_root_cache_current`：水位 / 目录落款 / 账本）答「到文件
        // 最新一版还新不新」，回执那一半只答「还是同一份文件、没落后」。此前回执半边也要会话等值，而窗口规划器的
        // `advance` 根回执留在 S（ADR-079 D3）——任何窗口落地后每个未触根都被判过期整根重算，E3D 每次登录写的 DBMDAT
        // 会话让这件事天天发生（GitHub #65）。读穿 / 只写内存形态问同一枚水位（D5）：听得到就按「没落后」判，
        // 水位行不在 / 没盖到 / 读不到就退回等值，不让任何库比从前更越权。
        let cache_current = if memory_only || crate::options::data_face().is_read_through() {
            let rule = match generation_root_cache_current(&root_refno).await {
                Ok(true) => SessionRule::NotBehind,
                Ok(false) => SessionRule::Exact,
                Err(error) => {
                    log::debug!(
                        "[按需生成] 生成根 {root_refno} 的库级承诺读不到，回执按等值判: {error:#}"
                    );
                    SessionRule::Exact
                }
            };
            memory_root_current(root, rule).await?
        } else {
            generation_root_cache_current(&root_refno).await?
                && memory_root_current(root, SessionRule::NotBehind).await?
        };
        if !force && cache_current {
            // 计数只问进程内投影（S4 之后 legacy「读 inst_relate 表数实例」那条路退役）。
            let counts = memory_counts(&[root]);
            return Ok(describe(
                requested_refno,
                root,
                root_noun,
                cached_status(counts),
                counts,
            ));
        }

        // Existing stable geometry stays readable during initialization, but a
        // miss must not create durable work or start generation before all data
        // phases (and the configured startup-wide model pass) have settled.
        // 只写内存的库不受这道门管：它既不造 durable 工作，也不是启动全量（R2）。
        if !memory_only {
            crate::data_interface::initialization_phase::require_model_generation()?;
        }

        // 不再先落 durable pending 行再生成（ADR-072 D5）：`/model/ensure` 是任务之外按根串行的
        // 第二个写者（ADR-071 D5），生成完就地合块落库、盖根凭证；进程在生成中途崩溃，这一根
        // 的凭证没盖，下一次点它（或该库任务的规划器按凭证缺口）照常重生成——没有第二本账要收。
        generate_unit_model(self, &root_refno).await?;

        let counts = memory_counts(&[root]);
        let status = post_generation_status(counts);
        Ok(describe(requested_refno, root, root_noun, status, counts))
    }

    /// 确保一个树节点覆盖的**全部**生成根都与最新已应用数据一致。
    ///
    /// WORL/SITE/ZONE 等容器本身不是生成根，但它们是合法显示范围：先展开完整
    /// PE 子树，再按统一策略解析、排序和去重生成根。每个根仍走
    /// [`Self::ensure_model_generated`]，所以锁、durable pending、完成凭证和强制重跑
    /// 语义只有一份实现。
    pub async fn ensure_model_scope_generated(
        &self,
        requested_refno: RefnoEnum,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        let subtree = crate::data_interface::helper::collect_pe_subtree_refnos(&[requested_refno])
            .await?
            .into_iter()
            .collect::<Vec<_>>();
        self.ensure_model_scope_generated_from_refnos(requested_refno, subtree, force)
            .await
    }

    /// Same scope semantics with a caller-supplied authoritative subtree. In
    /// direct mode the HTTP layer supplies this from e3d-io, so model display
    /// does not repeat the hierarchy read through SurrealDB.
    pub async fn ensure_model_scope_generated_from_refnos(
        &self,
        requested_refno: RefnoEnum,
        mut subtree: Vec<RefnoEnum>,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        subtree.sort_by_key(RefnoEnum::to_string);
        if subtree.is_empty() {
            return self.ensure_model_generated(requested_refno, force).await;
        }

        let unit_types = configured_delivery_unit_types();
        let mut roots = crate::data_interface::generation_root::resolve_generation_roots_on(
            &SUL_DB,
            &subtree,
            &unit_types,
        )
        .await?;
        self.ensure_model_scope_generated_from_roots(requested_refno, roots, force)
            .await
    }

    /// 单根失败不再中止整片（2026-10-20 不全修复）：`force` 先把范围清成空，首错上抛
    /// 会留下「已删未重生」的半截状态。现在逐根记进 `failed_roots`、其余根照常跑完；
    /// 全部失败时才把首错原样上抛（`ensure_error` 按域分型照旧）。
    ///
    /// 进程级中断（issue #59）：摄入形态下先删后生成的范围在清理之前落一行持久留痕
    /// （`model_scope_regen`），全部根落盘之后才删；进程死在中途，下次启动就绪后以
    /// `force = false` 续跑。同一范围的 `force = false` 请求（客户端补齐、人再点一次）接手
    /// 那一行，成了一样删。
    pub async fn ensure_model_scope_generated_from_roots(
        &self,
        requested_refno: RefnoEnum,
        mut roots: Vec<crate::data_interface::generation_root::GenerationRoot>,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        roots.sort_by_key(|root| root.root.to_pdms_str());
        roots.dedup_by_key(|root| root.root);
        // 只写内存 / 读透的清理不删权威库，没有要留痕的洞。
        let durable_scope = crate::data_interface::model_read_route::memory_only_scope().is_none()
            && !crate::options::data_face().is_read_through();
        let pending = match (durable_scope, force) {
            (true, true) => Some(model_scope_regen::begin(requested_refno, roots.len()).await?),
            (true, false) => model_scope_regen::adopt(requested_refno),
            (false, _) => None,
        };
        let outcome = self.regenerate_scope(requested_refno, roots, force).await;
        if let Some(run) = pending {
            model_scope_regen::settle(run, outcome.as_ref()).await;
        }
        outcome
    }

    /// [`Self::ensure_model_scope_generated_from_roots`] 的本体：`force` 先删整个请求范围，再逐根生成。
    async fn regenerate_scope(
        &self,
        requested_refno: RefnoEnum,
        roots: Vec<crate::data_interface::generation_root::GenerationRoot>,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        if force {
            // 先删后生成（见 `clear_model_scope`）。放在解根之后、逐根加锁之前：清理自己拿
            // 根锁 / 库级锁，范围内有根正在生成就按「正在后台生成」回，不与它抢。
            // 连几何一起清（2026-10-22）：马上就重新生成，这片范围还在用的 `inst_geo` 行与 `.mesh`
            // 会被写回，旧键不留着占盘；单位几何与共享键的取舍见 `mesh_purge`。
            self.clear_model_scope(requested_refno, &roots, ClearGeometry::Purge).await?;
        }
        if roots.is_empty() {
            return self.ensure_model_generated(requested_refno, force).await;
        }

        let root_count = roots.len();
        let root_refnos = roots.iter().map(|root| root.root).collect::<Vec<_>>();
        let generation_roots = roots
            .iter()
            .map(|root| root.root.to_pdms_str())
            .collect::<Vec<_>>();
        let scope_noun = roots
            .iter()
            .find(|root| root.root == requested_refno)
            .map(|root| root.noun.clone())
            .unwrap_or_else(|| "SCOPE".to_owned());
        let mut tally = ScopeRootTally::default();
        // 只写内存的库（plan 2026-09-06 §8 R2）：凭证与计数都只问进程内投影。
        let memory_only = crate::data_interface::model_read_route::memory_only_scope().is_some();
        let current_roots = if force {
            std::collections::HashSet::new()
        } else if memory_only || crate::options::data_face().is_read_through() {
            // 整批只借一次阻塞线程：一个作用域有几百个根，逐根 `spawn_blocking` 就是
            // 几百次调度往返，而每根要做的只是几次 stat。
            // 库级承诺盖到的根按「没落后」判，其余退回等值（ADR-089 D5）；水位读不到就是空集，全部等值。
            let promised = generation_roots_cache_current(&generation_roots)
                .await
                .unwrap_or_else(|error| {
                    log::debug!("[按需生成] 作用域的库级承诺读不到，回执按等值判: {error:#}");
                    std::collections::HashSet::new()
                });
            let pinned = pinned_data_window();
            let candidates = root_refnos.clone();
            tokio::task::spawn_blocking(move || {
                candidates
                    .into_iter()
                    .filter(|root| {
                        let rule = if promised.contains(&root.to_pdms_str()) {
                            SessionRule::NotBehind
                        } else {
                            SessionRule::Exact
                        };
                        memory_root_current_on_thread(*root, pinned, rule)
                    })
                    .map(|root| root.to_pdms_str())
                    .collect::<std::collections::HashSet<_>>()
            })
            .await?
        } else {
            generation_roots_cache_current(
                &roots
                    .iter()
                    .map(|root| root.root.to_pdms_str())
                    .collect::<Vec<_>>(),
            )
            .await?
        };
        for root in roots {
            // A scope can contain hundreds of delivery roots. Calling
            // `ensure_model_generated` for every cache hit would recount each
            // root's full instance subtree, then recount the whole requested
            // scope below. On AMS 1112 that quadratic read path exceeded the
            // HTTP worker's 120 s budget even though all 212 credentials were
            // current. A current credential is the authoritative cache hit;
            // only misses need the generation path and its per-root counts.
            // A durable credential survives restart, but /model/records serves the
            // process projection. Rebuild a missing resident root before calling it cached.
            let current = current_roots.contains(&root.root.to_pdms_str())
                && match memory_root_current(root.root, SessionRule::NotBehind).await {
                    Ok(current) => current,
                    // 投影核对本身垮了（阻塞线程 join 失败等）不拦整片：照常进生成路径，
                    // 生成里的同一道核对会再裁一次，真坏就记进 `failed_roots`。
                    Err(error) => {
                        log::warn!(
                            "[按需生成] 生成根 {} 的投影核对失败，照常生成: {error:#}",
                            root.root.to_pdms_str()
                        );
                        false
                    }
                };
            if current {
                tally.cached += 1;
                continue;
            }
            match self
                .ensure_exact_generation_root(root.root, root.root, &root.noun, force)
                .await
            {
                Ok(result) => tally.settled(result.status),
                Err(error) => tally.failed(requested_refno, &root, error, force),
            }
        }
        let (cached_root_count, generated_root_count, failed_roots) =
            tally.into_counts(requested_refno, root_count)?;

        let counts = memory_counts(&root_refnos);
        let status = if counts.renderable == 0 {
            OnDemandModelStatus::NoRenderableGeometry
        } else if generated_root_count > 0 {
            OnDemandModelStatus::Generated
        } else {
            OnDemandModelStatus::AlreadyAvailable
        };
        // 只写内存的库还没摄入 `pe`，noun 只能从根名单里来（与读透同一条路）。
        let noun = if memory_only || crate::options::data_face().is_read_through() {
            scope_noun
        } else {
            aios_core::get_pe(requested_refno)
                .await?
                .map_or_else(|| "SCOPE".to_owned(), |pe| pe.noun)
        };
        Ok(OnDemandModelResult {
            requested_refno: requested_refno.to_pdms_str(),
            generation_root: requested_refno.to_pdms_str(),
            generation_root_noun: noun,
            status,
            model_available: counts.renderable > 0,
            model_instance_count: counts.renderable,
            generated_instance_count: counts.written,
            generation_root_count: root_count,
            generation_roots,
            cached_root_count,
            generated_root_count,
            failed_root_count: failed_roots.len(),
            failed_roots,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGenerationInProgress {
    pub root_refno: String,
}

impl std::fmt::Display for ModelGenerationInProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "生成根 {} 正在后台生成", self.root_refno)
    }
}

impl std::error::Error for ModelGenerationInProgress {}

/// 按需生成**已经跑完**，后验却说这根不是最新：源文件在生成期间前移，或投影回执的模型目标
/// 核对不过。与 [`ModelGenerationInProgress`]（还有人在生成 / 等超时）是两回事——客户端同样
/// 该重试，但排障时要能一眼分开。HTTP 面照旧映射 409。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStaleAfterGeneration {
    pub root_refno: String,
    pub reason: &'static str,
}

impl std::fmt::Display for ModelStaleAfterGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "生成根 {} 刚生成完就被判成不是最新：{}", self.root_refno, self.reason)
    }
}

impl std::error::Error for ModelStaleAfterGeneration {}

pub(super) fn try_generation_root(
    root_refno: &str,
) -> Result<tokio::sync::OwnedMutexGuard<()>, ModelGenerationInProgress> {
    generation_root_lock(root_refno)
        .try_lock_owned()
        .map_err(|_| ModelGenerationInProgress {
            root_refno: root_refno.to_string(),
        })
}

/// 范围生成的逐根结局（2026-10-20 不全修复）：一根失败只点名进 `failed`、不中止整片——`force` 已先把
/// 范围清成空，首错上抛会把「已删未重生」的半截状态留给用户。`first_failure` 留住首错的 anyhow 链：
/// 全灭时原样抛回，`ensure_error` 照样分型。
#[derive(Debug, Default)]
struct ScopeRootTally {
    cached: usize,
    generated: usize,
    failed: Vec<ScopeRootFailure>,
    first_failure: Option<anyhow::Error>,
}

impl ScopeRootTally {
    fn settled(&mut self, status: OnDemandModelStatus) {
        match status {
            OnDemandModelStatus::Generated => self.generated += 1,
            OnDemandModelStatus::AlreadyAvailable | OnDemandModelStatus::NoRenderableGeometry => {
                self.cached += 1
            }
        }
    }

    /// `forced`：这一轮是强制重生成，它刚把这片范围删过，这一根此刻没有模型。它不可重试的失败
    /// 记进账本（`Failed`，同一错误连败照 ISSUE-029 判死）：下一窗并进 regen、一致性判决数它「还在赶」。
    /// 可重试的不记——根锁被占、初始化门这些与这根坏不坏无关。`force = false` 的补齐轮次（客户端
    /// 续跑、接手留痕、启动续跑）也不记：同一次点击里它们会把同一根反复重试，记了会在几秒内把
    /// 连败数抬到判死线。
    fn failed(
        &mut self,
        requested_refno: RefnoEnum,
        root: &crate::data_interface::generation_root::GenerationRoot,
        error: anyhow::Error,
        forced: bool,
    ) {
        let rendered = format!("{error:#}");
        let retryable = root_failure_is_retryable(&error);
        log::error!(
            "[按需生成] 范围 {} 内生成根 {} 失败（retryable={}），继续其余根: {}",
            requested_refno.to_pdms_str(),
            root.root.to_pdms_str(),
            retryable,
            rendered
        );
        if forced && !retryable {
            let dbnum = crate::data_interface::generation_root::refno_to_e3d(root.root).dbno();
            crate::data_interface::model_root_ledger::record_failed(
                dbnum, root.root, &root.noun, 0, &rendered,
            );
        }
        self.failed.push(ScopeRootFailure {
            generation_root: root.root.to_pdms_str(),
            error: rendered,
            retryable,
        });
        if self.first_failure.is_none() {
            self.first_failure = Some(error);
        }
    }

    /// 全灭 = 整个请求失败：与旧的「首错中止」同一口径上抛首错（根锁冲突仍是 409、初始化门仍是 409、
    /// 上游故障仍是它自己的码），失败点名跟着错误日志走。部分失败 = 200 回执点名 `failed_roots`，
    /// 客户端补齐。返回 `(命中, 生成, 失败点名)`。
    fn into_counts(
        self,
        requested_refno: RefnoEnum,
        root_count: usize,
    ) -> anyhow::Result<(usize, usize, Vec<ScopeRootFailure>)> {
        if root_count > 0 && self.failed.len() == root_count {
            return Err(self.first_failure.unwrap_or_else(|| {
                anyhow::anyhow!(
                    "范围 {} 的 {root_count} 个生成根全部失败",
                    requested_refno.to_pdms_str()
                )
            }));
        }
        Ok((self.cached, self.generated, self.failed))
    }
}

/// 重发同一请求有没有意义（`ScopeRootFailure::retryable`，也是账本记不记、启动续跑再不再试的判据）：
/// 与 HTTP 面判成 409 冲突、或带 `Retry-After` 的 503 的那几类同一口径（`handlers::ensure_error` /
/// `ApiError::from_domain`）——根锁被占、刚生成完被判过期、初始化门、房间参照模型在建、写后台不收、
/// 依赖库还在入库或读不开。
pub(crate) fn root_failure_is_retryable(error: &anyhow::Error) -> bool {
    use crate::data_interface::batch_scheduler::{DbnumIngestState, dbnum_ingest_state};
    use crate::data_interface::direct_store::DirectStoreError;

    fn caused_by<T: std::error::Error + Send + Sync + 'static>(error: &anyhow::Error) -> bool {
        error.downcast_ref::<T>().is_some()
            || error
                .chain()
                .any(|cause| cause.downcast_ref::<T>().is_some())
    }
    caused_by::<ModelGenerationInProgress>(error)
        || caused_by::<ModelStaleAfterGeneration>(error)
        || caused_by::<super::initialization_phase::InitializationNotReady>(error)
        || caused_by::<crate::fast_model::model_memory_store::OverlayRejected>(error)
        || error.chain().any(|cause| {
            cause
                .to_string()
                .starts_with(crate::fast_model::room_resident::RESIDENT_MODEL_UPDATING)
                || match cause.downcast_ref::<DirectStoreError>() {
                    Some(DirectStoreError::Session { .. }) => true,
                    Some(DirectStoreError::NoFileForDbnum { dbnum }) => {
                        !matches!(dbnum_ingest_state(*dbnum as u32), DbnumIngestState::Settled)
                    }
                    _ => false,
                }
        })
}

/// Instances found for a subtree, split by whether the viewer can draw them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct InstanceCounts {
    written: usize,
    renderable: usize,
}

/// The outcomes that need no generation run. `written > 0` means generation has
/// already been here, so a subtree with nothing drawable stays that way until
/// the underlying geometry is fixed — regenerating it just burns the same
/// minutes again.
fn cached_status(counts: InstanceCounts) -> OnDemandModelStatus {
    if counts.renderable > 0 {
        OnDemandModelStatus::AlreadyAvailable
    } else {
        OnDemandModelStatus::NoRenderableGeometry
    }
}

/// 生成跑完之后的定性。
///
/// 与 [`settled_status`] 的区别全在 `written == 0` 上：**生成之前**它只说明"还没
/// 人来生成过"，得去跑一趟；**生成之后**它说明这份数据本来就是空的（无子件的
/// BRAN、纯作层级用的 STRU）。
///
/// 这里曾经对 `written == 0` 抛 500。那违反 API 契约（§4.5）：空是数据的终局不是
/// 服务故障，报 5xx 只会让前端把"本来就空"当成故障反复重试，而底下的数据不变，
/// 重发拿到的还是同一个空结果。空与不可画二者对客户端是同一件事——没得画——所以
/// 归到同一个状态，实例数照样在 `generated_instance_count` 里如实回报。
fn post_generation_status(counts: InstanceCounts) -> OnDemandModelStatus {
    if counts.renderable > 0 {
        OnDemandModelStatus::Generated
    } else {
        OnDemandModelStatus::NoRenderableGeometry
    }
}

fn describe(
    requested_refno: RefnoEnum,
    root: RefnoEnum,
    root_noun: &str,
    status: OnDemandModelStatus,
    counts: InstanceCounts,
) -> OnDemandModelResult {
    OnDemandModelResult {
        requested_refno: requested_refno.to_pdms_str(),
        generation_root: root.to_pdms_str(),
        generation_root_noun: root_noun.to_owned(),
        status,
        model_available: counts.renderable > 0,
        model_instance_count: counts.renderable,
        generated_instance_count: counts.written,
        generation_root_count: 1,
        generation_roots: vec![root.to_pdms_str()],
        cached_root_count: usize::from(status != OnDemandModelStatus::Generated),
        generated_root_count: usize::from(status == OnDemandModelStatus::Generated),
        failed_root_count: 0,
        failed_roots: Vec::new(),
    }
}

/// Frozen-window and staging scopes are task-locals: they must be read on the calling
/// task, because they do not follow a closure onto a blocking thread.
fn pinned_data_window() -> bool {
    super::batch_worker::active_data_window().is_some()
        || super::staging::write_context::active_staging_writes().is_some()
}

/// 核对源文件身份要做文件 stat，不能占着 runtime 线程；判「窗口已钉」的那两个
/// task-local 由调用方在自己的任务上读好再传进来。`rule` 见 [`SessionRule`]（ADR-089 D1 / D5）。
async fn memory_root_current(root: RefnoEnum, rule: SessionRule) -> anyhow::Result<bool> {
    let pinned = pinned_data_window();
    Ok(
        tokio::task::spawn_blocking(move || memory_root_current_on_thread(root, pinned, rule))
            .await?,
    )
}

fn memory_root_current_on_thread(root: RefnoEnum, pinned_window: bool, rule: SessionRule) -> bool {
    crate::fast_model::e3d_model_service::parse_refno(&root.to_pdms_str())
        .ok()
        .and_then(|root| {
            crate::fast_model::model_memory_store::ModelMemoryStore::global().receipt(
                crate::fast_model::model_memory_store::ProjectionScope::Current,
                root,
            )
        })
        .is_some_and(|receipt| {
            // Frozen/task-local callers retain their pinned-window cache semantics.
            if pinned_window {
                return true;
            }
            receipt.model_target.as_ref().is_some_and(|target|
                super::room_read_through::target_current_under(target, rule).unwrap_or_else(|error| {
                    // 核对不了（文件 stat 不到、会话解不出）按过期处理，但要说出来：此前这一句被吞成
                    // 「正在后台生成」，日志里一行不剩。
                    println!(
                        "[模型目标核对] 生成根 {} 的目标核对不了，按过期处理: {error:#}",
                        root.to_pdms_str()
                    );
                    false
                }))
        })
}

async fn generation_source_identity(root: RefnoEnum) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        let native = super::generation_root::refno_to_e3d(root);
        let version = super::model_source::root_source_version(native)?;
        let source = super::frozen_source::FrozenSource::capture(version.file)?;
        source.commit_token_json()
    }).await?
}

fn memory_counts(roots: &[RefnoEnum]) -> InstanceCounts {
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let mut ids = std::collections::HashSet::new();
    for root in roots {
        let Ok(root) = crate::fast_model::e3d_model_service::parse_refno(&root.to_pdms_str())
        else {
            continue;
        };
        ids.extend(
            store
                .records_of_root(
                    crate::fast_model::model_memory_store::ProjectionScope::Current,
                    root,
                )
                .into_iter()
                .map(|record| record.record_id),
        );
    }
    InstanceCounts {
        written: ids.len(),
        renderable: ids.len(),
    }
}

/// 解析不出生成根的两种缘由。调用方（HTTP 层）据此分型：客户端对着容器该做的事
/// （展开一层、逐个 ensure）与对着一个不存在的 refno 该做的事完全不同，混在一个
/// `internal` 里它无从判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvableRoot {
    /// 库里没有这个构件。
    NotFound,
    /// WORL / SITE / ZONE 这类容器，按契约恒被拒绝做生成根。
    Container,
    /// 构件在、也不是容器，但生成根策略仍给不出根。
    NoRoot,
}

impl std::fmt::Display for UnresolvableRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("构件不存在"),
            Self::Container => f.write_str("容器不能做生成根，请展开一层后对子节点逐个 ensure"),
            Self::NoRoot => f.write_str("构件向上找不到任何合法生成根"),
        }
    }
}

impl std::error::Error for UnresolvableRoot {}

async fn resolve_generation_root(
    requested_refno: RefnoEnum,
) -> anyhow::Result<(RefnoEnum, String)> {
    let unit_types = configured_delivery_unit_types();
    if let Some(root) = resolve_live_element_generation_root(requested_refno, &unit_types).await? {
        return Ok((root.root, root.noun));
    }
    Err(
        anyhow::Error::new(unresolvable_reason(requested_refno).await).context(format!(
            "构件 {} 无法解析生成根",
            requested_refno.to_pdms_str()
        )),
    )
}

/// 分型只在失败之后跑，正常路径不多这一次查询。查询本身出错时按「不存在」报——
/// 同一条链路刚刚才读过这个构件，这时候读不到，最可能的解释就是它不在。
async fn unresolvable_reason(refno: RefnoEnum) -> UnresolvableRoot {
    match aios_core::get_pe(refno).await {
        Ok(Some(pe)) if is_coarse_hierarchy_noun(&pe.noun) => UnresolvableRoot::Container,
        Ok(Some(_)) => UnresolvableRoot::NoRoot,
        _ => UnresolvableRoot::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_status_is_stable_for_client_protocol() {
        let result = OnDemandModelResult {
            requested_refno: "16192/1".into(),
            generation_root: "16192/2".into(),
            generation_root_noun: "EQUI".into(),
            status: OnDemandModelStatus::Generated,
            model_available: true,
            model_instance_count: 3,
            generated_instance_count: 3,
            generation_root_count: 1,
            generation_roots: vec!["16192/2".into()],
            cached_root_count: 0,
            generated_root_count: 1,
            failed_root_count: 0,
            failed_roots: vec![],
        };
        let json = serde_json::to_string(&result).unwrap();
        let roundtrip: OnDemandModelResult = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, result);
    }

    #[tokio::test]
    async fn active_generation_root_is_rejected_instead_of_queued() {
        let lock = generation_root_lock("test/active-root");
        let _guard = lock.lock().await;
        assert!(matches!(
            try_generation_root("test/active-root"),
            Err(ModelGenerationInProgress { .. })
        ));
    }

    #[test]
    fn subtree_delete_locks_covered_generation_roots_but_deletes_the_requested_scope() {
        let source = include_str!("on_demand_model.rs");
        let body = source
            .split_once("pub async fn delete_model_subtree(")
            .expect("delete model subtree service must exist")
            .1
            .split_once("pub async fn ensure_model_generated(")
            .expect("ensure service must follow delete service")
            .0;

        let collect_at = body
            .find("collect_pe_subtree_refnos(&[requested_refno])")
            .expect("delete must inspect the exact requested subtree for lock coverage");
        let resolve_at = body
            .find("resolve_generation_roots_on(")
            .expect("delete must resolve every covered generation root");
        let lock_at = body
            .find("try_generation_root")
            .expect("delete must reject roots with active generation");
        let delete_at = body
            .find("delete_inst_relate_subtree(&[requested_refno], 300)")
            .expect("delete scope must remain the requested refno subtree");

        assert!(
            collect_at < resolve_at && resolve_at < lock_at && lock_at < delete_at,
            "scope discovery, root locking, and exact deletion must stay ordered: {body}"
        );
    }

    /// 右键「重新生成模型」= 先删后生成（2026-10-10 定）：`force` 在逐根加锁之前先清整个请求范围；
    /// 「只写内存」/ 读透走投影删法、不过初始化门、不碰 durable 权威库；摄入形态走带门的
    /// `delete_model_subtree`。不先清的话执行层会拿旧回执走「只重盖章」/「几何不动」的捷径。
    #[test]
    fn forced_regeneration_clears_the_requested_scope_before_generating() {
        let source = include_str!("on_demand_model.rs");
        let scope = source
            .split_once("pub async fn ensure_model_scope_generated_from_roots(")
            .expect("direct scope entry exists")
            .1
            .split_once("pub struct ModelGenerationInProgress")
            .expect("direct scope entry end exists")
            .0;
        let clear_at = scope
            .find("self.clear_model_scope(requested_refno, &roots, ClearGeometry::Purge).await?")
            .expect("force must clear the requested scope first, geometry included");
        let fallback_at = scope
            .find("ensure_model_generated(requested_refno, force)")
            .expect("single-root fallback exists");
        let generate_at = scope
            .find("ensure_exact_generation_root(")
            .expect("roots are generated after the clear");
        assert!(
            clear_at < fallback_at && clear_at < generate_at,
            "the clear must precede both the fallback and the per-root loop: {scope}"
        );

        let clear = source
            .split_once("pub async fn clear_model_scope(")
            .expect("clear helper exists")
            .1
            .split_once("pub async fn delete_model_subtree(")
            .expect("ingest delete follows the clear helpers")
            .0;
        assert!(clear.contains("memory_only_scope()"), "{clear}");
        assert!(clear.contains("is_read_through()"), "{clear}");
        assert!(
            clear.contains(".clear_projected_scope(requested_refno, roots, read_through)"),
            "memory-only / read-through clear the projection: {clear}"
        );
        assert!(clear.contains("self.delete_model_subtree(requested_refno)"), "{clear}");
        assert!(
            !clear.contains("require_model_generation()"),
            "只写内存作用域不过初始化门，与同作用域下的生成路径同一口径: {clear}"
        );

        let removal = include_str!("projection_removal.rs");
        assert!(
            !removal.contains("SUL_DB"),
            "投影删法不碰 durable 权威库: {removal}"
        );
    }

    /// 范围清理只有一份实现（2026-10-10 清理模型计划 G1）：`DELETE /model/subtree` 与 `force = true` 都从
    /// `clear_model_scope` 进；投影那一路只有 `clear_projected_scope`（先 try-lock 再删），摄入形态的
    /// `delete_model_subtree` 读透分支也转给它——此前那一支不先对生成根 try-lock，spec §4.5.1 写的
    /// 「范围内有根正在生成回 409」在读透形态不成立。
    #[test]
    fn scope_cleanup_has_exactly_one_implementation() {
        let source = include_str!("on_demand_model.rs");
        let production = source.split_once("#[cfg(test)]").expect("tests follow").0;
        assert_eq!(
            production
                .matches("projection_removal::remove_subtree(")
                .count(),
            1,
            "投影删法只从 clear_projected_scope 进删除入口: {production}"
        );
        assert_eq!(
            production.matches("try_generation_root(root)").count(),
            1,
            "这里只剩摄入形态的 delete_model_subtree 自己拿根锁；投影删法的根锁归删除入口: {production}"
        );
        let delete = production
            .split_once("pub async fn delete_model_subtree(")
            .expect("delete exists")
            .1
            .split_once("pub async fn ensure_model_generated(")
            .expect("ensure follows")
            .0;
        assert!(
            delete.contains("return self.clear_projected_scope(requested_refno, &[], true).await;"),
            "读透分支转给 clear_projected_scope: {delete}"
        );
        assert!(
            !delete.contains("projection_removal::"),
            "delete_model_subtree 不直接调删除入口，读透分支经 clear_projected_scope 进: {delete}"
        );
    }

    #[test]
    fn availability_is_checked_only_after_the_generation_root_is_owned() {
        let source = include_str!("on_demand_model.rs");
        let body = source
            .split_once("pub async fn ensure_model_generated(")
            .expect("ensure entrypoint must exist")
            .1
            .split_once("#[derive(Debug, Clone, PartialEq, Eq)]")
            .expect("ensure implementation must end before its error type")
            .0;
        let lock_at = body
            .find("try_generation_root")
            .expect("ensure must own the generation root");
        let availability_at = body
            .find("generation_root_cache_current")
            .expect("ensure must validate the latest model completion credential");
        assert!(
            lock_at < availability_at,
            "a concurrent partial write must return busy instead of looking complete"
        );
    }

    /// A subtree that generated rows but nothing drawable must settle instead of
    /// asking for another generation run: that is the case the viewer hits on
    /// every single show of an EQUI whose only child is an ELCONN.
    #[test]
    fn generated_but_undrawable_settles_without_regenerating() {
        assert_eq!(
            cached_status(InstanceCounts {
                written: 1,
                renderable: 0
            }),
            OnDemandModelStatus::NoRenderableGeometry
        );
        assert_eq!(
            cached_status(InstanceCounts {
                written: 4,
                renderable: 2
            }),
            OnDemandModelStatus::AlreadyAvailable
        );
        assert_eq!(
            cached_status(InstanceCounts::default()),
            OnDemandModelStatus::NoRenderableGeometry
        );
    }

    /// 同一个 `written == 0`，在生成前后含义相反：生成前是「还没跑过，去跑」，
    /// 生成后是「跑过了，这份数据就是空的」。这里曾经把后者当成 500 内部错误，
    /// 7997 全量 sweep 里 22 个 STRU 根因此被反复当作服务故障重试。
    #[test]
    fn empty_unit_after_generation_is_terminal_not_an_error() {
        assert_eq!(
            cached_status(InstanceCounts::default()),
            OnDemandModelStatus::NoRenderableGeometry
        );
        assert_eq!(
            post_generation_status(InstanceCounts::default()),
            OnDemandModelStatus::NoRenderableGeometry
        );
        assert_eq!(
            post_generation_status(InstanceCounts {
                written: 3,
                renderable: 0
            }),
            OnDemandModelStatus::NoRenderableGeometry
        );
        assert_eq!(
            post_generation_status(InstanceCounts {
                written: 3,
                renderable: 3
            }),
            OnDemandModelStatus::Generated
        );
    }

    /// HTTP 层靠 `downcast_ref` 分型，所以缘由必须活着穿过 `anyhow` 链——
    /// 把 `Error::new(..).context(..)` 写成 `anyhow!(..)` 就会把它丢掉，
    /// 而那样丢了也照样编译、照样返回 500，没人会发现。
    #[test]
    fn unresolvable_reason_survives_the_anyhow_chain() {
        let error = anyhow::Error::new(UnresolvableRoot::Container)
            .context("构件 24384/22400 无法解析生成根");
        assert_eq!(
            error.downcast_ref::<UnresolvableRoot>(),
            Some(&UnresolvableRoot::Container)
        );
        let rendered = format!("{error:#}");
        assert!(rendered.contains("24384/22400"), "{rendered}");
        assert!(rendered.contains("展开一层"), "{rendered}");
    }

    /// `NoRenderableGeometry` is not "available": the viewer has nothing to draw
    /// and must not report the element as shown.
    #[test]
    fn undrawable_result_is_not_reported_as_available() {
        let result = describe(
            RefnoEnum::from("24384/24882"),
            RefnoEnum::from("24384/24882"),
            "EQUI",
            OnDemandModelStatus::NoRenderableGeometry,
            InstanceCounts {
                written: 1,
                renderable: 0,
            },
        );
        assert!(!result.model_available);
        assert_eq!(result.model_instance_count, 0);
        assert_eq!(result.generated_instance_count, 1);
    }

    /// Without project overrides the on-demand path must still recover the
    /// delivery units the viewer asks for most often.
    #[test]
    fn default_units_include_required_types() {
        let units = crate::data_interface::generation_root::resolve_delivery_unit_types(&[]);
        for required in ["BRAN", "HANG", "SUPPO", "EQUI"] {
            assert!(units.iter().any(|unit| unit == required));
        }
    }

    /// Live recovery probe for the configured local SurrealDB/E3D dataset.
    ///
    /// Prepare a target without `inst_relate`, set
    /// `AIOS_ON_DEMAND_TEST_REFNO=24384/24777`, then run this ignored test.
    #[tokio::test]
    #[ignore = "requires the configured local SurrealDB and parsed E3D project"]
    async fn live_generates_a_missing_model() {
        let refno =
            std::env::var("AIOS_ON_DEMAND_TEST_REFNO").expect("set AIOS_ON_DEMAND_TEST_REFNO");
        let refno = RefnoEnum::from(refno.as_str());
        assert!(refno.is_valid(), "invalid test refno");

        aios_core::init_test_surreal()
            .await
            .expect("connect to local SurrealDB");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("initialize local database manager");
        let result = manager
            .ensure_model_generated(refno, false)
            .await
            .expect("generate missing model");

        assert!(result.model_available);
        assert!(result.model_instance_count > 0);
        if let Ok(expected) = std::env::var("AIOS_ON_DEMAND_EXPECT_ROOT_NOUN") {
            assert_eq!(result.generation_root_noun, expected);
        }
        if let Ok(expected) = std::env::var("AIOS_ON_DEMAND_EXPECT_ROOT") {
            assert_eq!(result.generation_root, expected);
        }
    }

    /// 初始化门排在「复用已有模型」之后、生成之前；生成前不再落 durable pending 行
    /// （ADR-072 D5），生成完也没有 pending 行要收口。
    #[test]
    fn initialization_gate_precedes_generation_but_follows_existing_model_reuse() {
        let source = include_str!("on_demand_model.rs");
        let body = source
            .split_once("pub async fn ensure_model_generated(")
            .expect("ensure_model_generated exists")
            .1
            .split_once("pub struct ModelGenerationInProgress")
            .expect("ensure_model_generated end exists")
            .0;
        let reuse = body.find("generation_root_cache_current").unwrap();
        let gate = body.find("require_model_generation()").unwrap();
        let generate = body.find("generate_unit_model(self, &root_refno)").unwrap();
        assert!(reuse < gate && gate < generate);
        for retired in ["ensure_regen_pending_current", "settle_regen_work"] {
            assert!(!body.contains(retired), "pending 行不得回来 `{retired}`");
        }
    }

    #[test]
    fn direct_scope_does_not_reresolve_an_e3dio_generation_root_through_db() {
        let source = include_str!("on_demand_model.rs");
        let body = source
            .split_once("pub async fn ensure_model_scope_generated_from_roots(")
            .expect("direct scope entry exists")
            .1
            .split_once("pub struct ModelGenerationInProgress")
            .expect("direct scope entry end exists")
            .0;
        assert!(body.contains("ensure_exact_generation_root("));
        assert!(!body.contains("ensure_model_generated(root.root"));
    }

    /// 测试用生成根都落在 dbnum 6911（`word0 < 0x2000` 时 dbnum 就是 `word0`），不与别的测试共用账本页。
    fn scope_root(id: u32, noun: &str) -> crate::data_interface::generation_root::GenerationRoot {
        crate::data_interface::generation_root::GenerationRoot {
            root: aios_core::RefU64::from_two_nums(6911, id).into(),
            noun: noun.into(),
            name: String::new(),
            kind: crate::data_interface::generation_root::GenerationRootKind::DeliveryUnit,
        }
    }

    fn in_progress(root: &str) -> anyhow::Error {
        anyhow::Error::new(ModelGenerationInProgress {
            root_refno: root.into(),
        })
    }

    /// 范围里一根失败不拦其余根：部分失败仍是成功回执，失败根逐个点名、各带可不可重试。
    #[test]
    fn a_partially_failed_scope_still_answers_and_names_the_failed_roots() {
        let requested = RefnoEnum::from("6911/1");
        let mut tally = ScopeRootTally::default();
        tally.settled(OnDemandModelStatus::Generated);
        tally.settled(OnDemandModelStatus::NoRenderableGeometry);
        tally.failed(
            requested,
            &scope_root(11, "BRAN"),
            anyhow::anyhow!("几何坏了"),
            false,
        );
        tally.failed(
            requested,
            &scope_root(12, "EQUI"),
            in_progress("6911/12").context("生成根 6911/12"),
            false,
        );
        let (cached, generated, failed) = tally
            .into_counts(requested, 4)
            .expect("部分失败仍回成功回执");
        assert_eq!((cached, generated), (1, 1));
        assert_eq!(
            failed
                .iter()
                .map(|failure| (failure.generation_root.as_str(), failure.retryable))
                .collect::<Vec<_>>(),
            vec![("6911/11", false), ("6911/12", true)]
        );
        assert_eq!(failed[0].error, "几何坏了");
    }

    /// 全灭 = 整个请求失败，首错原样上抛：HTTP 面靠 `downcast_ref` 分型（根锁冲突仍是 409）。
    /// 全是命中、或一根都不用跑，都不是「全灭」。
    #[test]
    fn an_all_failed_scope_rethrows_the_first_failure_with_its_type() {
        let requested = RefnoEnum::from("6911/2");
        let mut tally = ScopeRootTally::default();
        tally.failed(
            requested,
            &scope_root(21, "BRAN"),
            in_progress("6911/21"),
            false,
        );
        tally.failed(
            requested,
            &scope_root(22, "BRAN"),
            anyhow::anyhow!("几何坏了"),
            false,
        );
        let error = tally
            .into_counts(requested, 2)
            .expect_err("全部失败 = 请求失败");
        assert_eq!(
            error
                .downcast_ref::<ModelGenerationInProgress>()
                .map(|busy| busy.root_refno.as_str()),
            Some("6911/21")
        );

        let mut cached = ScopeRootTally::default();
        cached.settled(OnDemandModelStatus::AlreadyAvailable);
        assert_eq!(cached.into_counts(requested, 1).unwrap(), (1, 0, vec![]));
        assert_eq!(
            ScopeRootTally::default().into_counts(requested, 0).unwrap(),
            (0, 0, vec![])
        );
    }

    /// 强制重生成那一轮里，不可重试的失败进账本（下一窗并进 regen、一致性判决数它「还在赶」）；
    /// 可重试的不进；`force = false` 的补齐轮次一根也不记——客户端续跑会把同一根反复重试，
    /// 记了就是虚抬 ISSUE-029 连败。
    #[test]
    fn only_non_retryable_failures_of_a_forced_round_enter_the_root_ledger() {
        use crate::data_interface::model_root_ledger as ledger;
        let dbnum =
            crate::data_interface::generation_root::refno_to_e3d(scope_root(31, "BRAN").root)
                .dbno();
        assert_eq!(dbnum, 6911);
        ledger::forget_dbnum(dbnum);
        let requested = RefnoEnum::from("6911/3");
        let mut tally = ScopeRootTally::default();
        tally.failed(
            requested,
            &scope_root(31, " bran "),
            anyhow::anyhow!("几何坏了"),
            true,
        );
        tally.failed(
            requested,
            &scope_root(32, "BRAN"),
            in_progress("6911/32"),
            true,
        );
        tally.failed(
            requested,
            &scope_root(33, "BRAN"),
            anyhow::anyhow!("几何坏了"),
            false,
        );

        let entry =
            ledger::entry(dbnum, scope_root(31, "BRAN").root).expect("不可重试的失败进账本");
        assert!(entry.is_chasing());
        assert_eq!(entry.noun, "BRAN");
        assert_eq!(entry.source_end_sesno, 0, "按需生成不知道源版本");
        assert_eq!(entry.error(), Some("几何坏了"));
        assert!(
            ledger::entry(dbnum, scope_root(32, "BRAN").root).is_none(),
            "可重试的不进账本"
        );
        assert!(
            ledger::entry(dbnum, scope_root(33, "BRAN").root).is_none(),
            "force = false 的补齐轮次不记"
        );
        ledger::forget_dbnum(dbnum);
    }

    /// 可重试 = HTTP 面那几类「等一等再来」，套了几层 context 也认得出；其余都是终局失败。
    #[test]
    fn retryable_root_failures_are_the_http_retry_classes() {
        use crate::data_interface::direct_store::DirectStoreError;
        let retryable = [
            in_progress("6911/1").context("生成根 6911/1"),
            anyhow::Error::new(ModelStaleAfterGeneration {
                root_refno: "6911/1".into(),
                reason: "源文件版本在生成期间前移",
            }),
            anyhow::Error::new(super::super::initialization_phase::InitializationNotReady {
                snapshot: super::super::initialization_phase::InitializationCoordinator::global()
                    .snapshot(),
            }),
            anyhow::Error::new(crate::fast_model::model_memory_store::OverlayRejected)
                .context("publish"),
            anyhow::anyhow!(
                "{}: room resident model is not ready (loading)",
                crate::fast_model::room_resident::RESIDENT_MODEL_UPDATING
            )
            .context("房间归属"),
            anyhow::Error::new(DirectStoreError::Session {
                dbnum: 7004,
                detail: "locked".into(),
            })
            .context("读目录库"),
        ];
        for error in &retryable {
            assert!(root_failure_is_retryable(error), "{error:#}");
        }
        let terminal = [
            anyhow::anyhow!("几何坏了"),
            anyhow::Error::new(DirectStoreError::DbNumberMismatch {
                dbnum: 7004,
                actual: 7005,
                file: "x.db".into(),
            }),
            anyhow::Error::new(UnresolvableRoot::NoRoot).context("构件 6911/1 无法解析生成根"),
        ];
        for error in &terminal {
            assert!(!root_failure_is_retryable(error), "{error:#}");
        }
    }

    /// issue #59 根因 2：先删后生成的范围在清理**之前**落持久留痕，整轮跑完才收口；只写内存 / 读透不落。
    #[test]
    fn forced_regeneration_records_the_scope_before_clearing_and_settles_after_the_run() {
        let source = include_str!("on_demand_model.rs");
        let entry = source
            .split_once("pub async fn ensure_model_scope_generated_from_roots(")
            .expect("direct scope entry exists")
            .1
            .split_once("async fn regenerate_scope(")
            .expect("the scope body follows its entry")
            .0;
        let durable = entry
            .find("memory_only_scope().is_none()")
            .expect("只写内存不落留痕");
        assert!(
            entry.contains("!crate::options::data_face().is_read_through()"),
            "读透不落留痕: {entry}"
        );
        let begin = entry
            .find("model_scope_regen::begin(requested_refno, roots.len()).await?")
            .expect("强制重生成先落留痕");
        let run = entry
            .find(".regenerate_scope(requested_refno, roots, force)")
            .expect("清理与生成都在本体里");
        let settle = entry
            .find("model_scope_regen::settle(run, outcome.as_ref())")
            .expect("整轮跑完才收口");
        assert!(durable < begin && begin < run && run < settle, "{entry}");
        assert!(
            entry.contains("model_scope_regen::adopt(requested_refno)"),
            "force = false 接手未完成的留痕: {entry}"
        );
        assert!(
            !entry.contains("clear_model_scope("),
            "清理只在本体里、留痕落下之后: {entry}"
        );
    }
}
