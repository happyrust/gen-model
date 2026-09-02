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
use crate::data_interface::model_update_pending::{
    ensure_regen_pending_current, generation_root_cache_current, settle_regen_work,
};
use crate::data_interface::tidb_manager::AiosDBManager;

/// Scope chunk for the written-instance probe, matching the viewer's own probe
/// in `rs-plant3-d` (`model_system.rs`).
const COUNT_CHUNK: usize = 500;

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
        crate::data_interface::model_rebuild::reject_ensure_during_rebuild(root).await?;
        let root_refno = root.to_pdms_str();
        let _guard = try_generation_root(&root_refno)?;

        // Never report a partially-written model while another generation owns
        // this root. Once the lock is ours, a completed prior run is safe to reuse.
        if !force && generation_root_cache_current(&root_refno).await? {
            let counts = instance_counts(requested_refno).await?;
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
        crate::data_interface::initialization_phase::require_model_generation()?;

        // 先落 durable pending 再生成（spec §4.5 / 2026-07-30 审计 C1）。曾经这里只
        // 读现有行的 revision：表里本来没有这个根时收口是 no-op，进程在生成中途崩溃，
        // 这次工作就没有任何持久痕迹，没有 drain 会捡它，只能靠人再点一次。落行失败
        // 直接返回错误、不跑生成——durable 语义必须先于工作本身成立。
        let pending_revision = Some(ensure_regen_pending_current(&root_refno, root_noun).await?);
        let outcome = generate_unit_model(self, &root_refno).await;
        let generation_error = outcome.as_ref().err().map(|error| format!("{error:#}"));
        if let Err(error) =
            settle_regen_work(&root_refno, pending_revision, generation_error.as_deref()).await
        {
            log::error!("收口模型 pending 失败 root={root_refno}: {error:#}");
        }
        outcome?;

        let counts = instance_counts(requested_refno).await?;
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
        let mut cached_root_count = 0usize;
        let mut generated_root_count = 0usize;
        for root in roots {
            // A scope can contain hundreds of delivery roots. Calling
            // `ensure_model_generated` for every cache hit would recount each
            // root's full instance subtree, then recount the whole requested
            // scope below. On AMS 1112 that quadratic read path exceeded the
            // HTTP worker's 120 s budget even though all 212 credentials were
            // current. A current credential is the authoritative cache hit;
            // only misses need the generation path and its per-root counts.
            if !force
                && crate::data_interface::model_update_pending::generation_root_cache_current(
                    &root.root.to_pdms_str(),
                )
                .await?
            {
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

        let counts = instance_counts(requested_refno).await?;
        let status = if counts.renderable == 0 {
            OnDemandModelStatus::NoRenderableGeometry
        } else if generated_root_count > 0 {
            OnDemandModelStatus::Generated
        } else {
            OnDemandModelStatus::AlreadyAvailable
        };
        let noun = aios_core::get_pe(requested_refno)
            .await?
            .map_or_else(|| "SCOPE".to_owned(), |pe| pe.noun);
        Ok(OnDemandModelResult {
            requested_refno: requested_refno.to_pdms_str(),
            generation_root: requested_refno.to_pdms_str(),
            generation_root_noun: noun,
            status,
            model_available: counts.renderable > 0,
            model_instance_count: counts.renderable,
            generated_instance_count: counts.written,
            generation_root_count: root_count,
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
        cached_root_count: usize::from(status != OnDemandModelStatus::Generated),
        generated_root_count: usize::from(status == OnDemandModelStatus::Generated),
    }
}

async fn instance_counts(refno: RefnoEnum) -> anyhow::Result<InstanceCounts> {
    // The model table already materializes every row's ancestor chain and has
    // an index on `anc`. Walking the PE tree here made a cache-hit ensure pay
    // for a second full hierarchy traversal before it could answer. Query the
    // generated subtree directly, and add the root's own row because `anc`
    // contains ancestors only (not self).
    let root_u64 = refno.refno().0;
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE in FROM inst_relate WHERE anc CONTAINS {root_u64};\
             SELECT VALUE in FROM {} LIMIT 1;",
            refno.to_inst_relate_key()
        ))
        .await?
        .check()?;
    let mut scope: Vec<RefnoEnum> = response.take(0)?;
    scope.extend(response.take::<Vec<RefnoEnum>>(1)?);
    scope.sort_by_key(RefnoEnum::to_string);
    scope.dedup();
    if scope.is_empty() {
        return Ok(InstanceCounts::default());
    }
    let rows = crate::data_interface::staging::query_valid_insts(&scope).await?;
    let renderable = rows
        .iter()
        .map(|row| {
            row.insts.len() + usize::from(row.pts.as_ref().is_some_and(|points| !points.is_empty()))
        })
        .sum();
    Ok(InstanceCounts {
        written: written_instance_count(&scope).await?,
        renderable,
    })
}

/// Rows `query_insts` never reports: it drops everything without an `aabb`, so
/// on its own it cannot tell "生成还没跑过" from "跑过了，只是画不出来".
async fn written_instance_count(scope: &[RefnoEnum]) -> anyhow::Result<usize> {
    let mut total = 0usize;
    for chunk in scope.chunks(COUNT_CHUNK) {
        // An empty slice makes `get_inst_relate_keys` address the whole table.
        if chunk.is_empty() {
            continue;
        }
        let inst_keys = aios_core::get_inst_relate_keys(chunk);
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::len(SELECT VALUE id FROM {inst_keys});"
            ))
            .await?;
        let count: Option<usize> = response.take(0)?;
        total += count.unwrap_or_default();
    }
    Ok(total)
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

    /// durable pending 必须先于生成成立（spec §4.5 / 2026-07-30 审计 C1）。
    ///
    /// 只读现有行（`current_regen_revision`）的话，表里本来没有这个根时收口是
    /// no-op：进程在生成中途崩溃，这次工作没有任何持久痕迹，没有 drain 会捡它。
    #[test]
    fn a_durable_pending_row_is_written_before_generation_runs() {
        let source = include_str!("on_demand_model.rs");
        let body = source
            .split_once("pub async fn ensure_model_generated(")
            .expect("ensure entrypoint must exist")
            .1
            .split_once("#[derive(Debug, Clone, PartialEq, Eq)]")
            .expect("ensure implementation must end before its error type")
            .0;
        let pending_at = body
            .find("ensure_regen_pending")
            .expect("ensure 必须先写 durable pending 行");
        let generate_at = body
            .find("generate_unit_model")
            .expect("ensure 必须执行生成");
        assert!(
            pending_at < generate_at,
            "pending 行必须写在生成之前，崩溃才有 drain 能捡的痕迹"
        );
        assert!(
            !body.contains("current_regen_revision"),
            "ensure 不得再走只读旧行的路——那条路对『表里没有行』的情形收口是 no-op"
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

    #[test]
    fn initialization_gate_precedes_pending_creation_but_follows_existing_model_reuse() {
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
        let pending = body.find("ensure_regen_pending_current(").unwrap();
        assert!(reuse < gate && gate < pending);
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
