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
}

impl AiosDBManager {
    /// Delete the generated-model data rooted at exactly `requested_refno`.
    ///
    /// Root resolution is used only to share the same concurrency locks as
    /// generation. The deletion scope remains the requested PE subtree.
    pub async fn delete_model_subtree(&self, requested_refno: RefnoEnum) -> anyhow::Result<()> {
        crate::data_interface::initialization_phase::require_model_generation()?;

        if crate::options::data_face().is_read_through() {
            let root = crate::data_interface::generation_root::refno_to_e3d(requested_refno);
            let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
            let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
            let records = store.records_of_root(scope, root);
            let record_ids = records
                .iter()
                .map(|record| record.record_id.clone())
                .collect::<std::collections::HashSet<_>>();
            let mut affected_roots = store
                .roots(scope)
                .into_iter()
                .filter(|candidate| {
                    *candidate == root
                        || store
                            .records_of_root(scope, *candidate)
                            .iter()
                            .any(|record| record_ids.contains(&record.record_id))
                })
                .collect::<Vec<_>>();
            if affected_roots.is_empty() {
                affected_roots.push(root);
            }
            let tube_branches = records
                .iter()
                .filter(|record| record.tube.is_some())
                .map(|record| record.source_refno)
                .collect::<Vec<_>>();
            let option = aios_core::get_db_option();
            let mut sessions = records
                .iter()
                .map(|record| (record.dbnum, record.source_sesno))
                .collect::<Vec<_>>();
            sessions.extend(affected_roots.iter().filter_map(|affected| {
                store
                    .receipt(scope, *affected)
                    .map(|receipt| (affected.dbno(), receipt.source_sesno))
            }));
            if sessions.is_empty() {
                sessions.extend(
                    crate::data_interface::mdb_memory_store::get(
                        &option.project_name,
                        &option.mdb_name,
                    )
                    .into_iter()
                    .flat_map(|snapshot| snapshot.session_fingerprint())
                    .map(|(dbnum, _, sesno)| (dbnum, sesno)),
                );
            }
            sessions.sort_unstable();
            sessions.dedup();
            let source_sesno_epoch = sessions
                .iter()
                .map(|(_, sesno)| u64::from(*sesno))
                .max()
                .unwrap_or_default();
            let epoch = crate::data_interface::mdb_memory_store::get(
                &option.project_name,
                &option.mdb_name,
            )
            .map(|snapshot| snapshot.epoch())
            .unwrap_or(source_sesno_epoch);
            let transaction = crate::fast_model::model_db_adapter::render_spooled_scope_deletion(
                root,
                &affected_roots,
                &tube_branches,
            );
            crate::data_interface::mirror_spool::MirrorSpool::for_process(
                &option.surreal_ns,
                &option.project_name,
                &option.mdb_name,
            )?
            .persist(
                epoch,
                sessions,
                format!("model-tombstone:{root}"),
                vec![transaction],
            )?;

            store.commit(
                scope,
                vec![crate::fast_model::model_memory_store::ProjectionCommit {
                    tube_branch_resets: tube_branches,
                    removals: record_ids.into_iter().collect(),
                    ..Default::default()
                }],
            );
            for affected in affected_roots {
                store.invalidate_receipt(scope, affected);
            }
            return Ok(());
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

        let credentials = lock_roots
            .iter()
            .map(|root| format!("type::thing('gen_root', '{}')", root.replace('/', "_")))
            .collect::<Vec<_>>()
            .join(",");
        if !credentials.is_empty() {
            SUL_DB
                .query(format!("DELETE {credentials};"))
                .await?
                .check()?;
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
        if super::model_read_route::memory_only_scope().is_some()
            || super::batch_worker::active_data_window().is_some()
            || super::staging::write_context::active_staging_writes().is_some()
        {
            return self.ensure_exact_generation_root_inner(requested_refno, root, root_noun, force).await;
        }
        static FLIGHTS: std::sync::OnceLock<super::generation_flight::Flights<OnDemandModelResult>> = std::sync::OnceLock::new();
        let version = generation_source_identity(root).await?;
        let key = format!("{}|{}|{}|{}|{}", self.db_option.project_name,
            self.db_option.mdb_name, root.to_pdms_str(), force, version);
        let manager = self.clone();
        let noun = root_noun.to_string();
        let future = FLIGHTS.get_or_init(Default::default).run(key, async move {
            manager.ensure_exact_generation_root_inner(root, root, &noun, force).await
        });
        let mut result = tokio::time::timeout(std::time::Duration::from_secs(110), future).await
            .map_err(|_| ModelGenerationInProgress { root_refno: root.to_pdms_str() })??;
        anyhow::ensure!(generation_source_identity(root).await? == version,
            ModelGenerationInProgress { root_refno: root.to_pdms_str() });
        anyhow::ensure!(tokio::task::spawn_blocking(move || memory_root_current(root)).await?,
            ModelGenerationInProgress { root_refno: root.to_pdms_str() });
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
        let cache_current = if memory_only || crate::options::data_face().is_read_through() {
            memory_root_current(root)
        } else {
            generation_root_cache_current(&root_refno).await? && memory_root_current(root)
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
    /// does not first repeat the hierarchy read through SurrealDB.
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

    pub async fn ensure_model_scope_generated_from_roots(
        &self,
        requested_refno: RefnoEnum,
        mut roots: Vec<crate::data_interface::generation_root::GenerationRoot>,
        force: bool,
    ) -> anyhow::Result<OnDemandModelResult> {
        roots.sort_by_key(|root| root.root.to_pdms_str());
        roots.dedup_by_key(|root| root.root);
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
        let mut cached_root_count = 0usize;
        let mut generated_root_count = 0usize;
        // 只写内存的库（plan 2026-09-06 §8 R2）：凭证与计数都只问进程内投影。
        let memory_only = crate::data_interface::model_read_route::memory_only_scope().is_some();
        let current_roots = if force {
            std::collections::HashSet::new()
        } else if memory_only || crate::options::data_face().is_read_through() {
            root_refnos
                .iter()
                .copied()
                .filter(|root| memory_root_current(*root))
                .map(|root| root.to_pdms_str())
                .collect()
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
            if current_roots.contains(&root.root.to_pdms_str()) && memory_root_current(root.root) {
                cached_root_count += 1;
                continue;
            }
            let result = self
                .ensure_exact_generation_root(root.root, root.root, &root.noun, force)
                .await?;
            match result.status {
                OnDemandModelStatus::Generated => generated_root_count += 1,
                OnDemandModelStatus::AlreadyAvailable
                | OnDemandModelStatus::NoRenderableGeometry => cached_root_count += 1,
            }
        }

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

fn try_generation_root(
    root_refno: &str,
) -> Result<tokio::sync::OwnedMutexGuard<()>, ModelGenerationInProgress> {
    generation_root_lock(root_refno)
        .try_lock_owned()
        .map_err(|_| ModelGenerationInProgress {
            root_refno: root_refno.to_string(),
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
    }
}

fn memory_root_current(root: RefnoEnum) -> bool {
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
            if super::model_read_route::memory_only_scope().is_some()
                || super::batch_worker::active_data_window().is_some()
                || super::staging::write_context::active_staging_writes().is_some() {
                return true;
            }
            receipt.model_target.as_ref().is_some_and(|target|
                super::room_read_through::target_current(target).unwrap_or(false))
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
}
