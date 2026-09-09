//! SideEffectCompensator — durable retry for post-watermark side effects.
//!
//! PE persist advances [`crate::data_interface::increment_pipeline::IncrementPipeline`]
//! watermarks and must not roll back. Model refresh / SYST derived sync can fail
//! afterward; this module records those jobs in Surreal and retries on drain.
//!
use aios_core::SUL_DB;
use aios_core::pdms_types::*;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::sql::Thing;

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::increment_pipeline::IncrResult;
use crate::data_interface::tidb_manager::AiosDBManager;

const TABLE: &str = "incr_side_effect_pending";
const PUBLISHED_SPATIAL_VERSION_ID: &str = "published_spatial_version:current";
const PUBLISHED_FILE_INPUT_TABLE: &str = "published_file_input";
const PUBLISHED_ROOM_BASELINE_ID: &str = "room_published_input_baseline:topology_v2";
const MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
/// File identity and completed model/spatial publication. Room completion is
/// checked separately by the room-release APIs; this proof alone does not imply it.
pub(crate) struct CompletedFileRound {
    dbnum: u32,
    end_sesno: i32,
    pub(crate) commit_token: Option<String>,
}

impl CompletedFileRound {
    pub(crate) fn dbnum(&self) -> u32 {
        self.dbnum
    }

    pub(crate) fn end_sesno(&self) -> i32 {
        self.end_sesno
    }
}

fn parse_ref_rev_payload(changed_refnos: &[String]) -> anyhow::Result<Vec<RefnoEnum>> {
    anyhow::ensure!(
        !changed_refnos.is_empty(),
        "ref_rev_maintain changed_refnos 为空"
    );
    changed_refnos
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let value = raw.trim();
            anyhow::ensure!(!value.is_empty(), "changed_refnos[{index}] 为空");
            let parsed = value.parse::<RefU64>().map_err(|error| {
                anyhow::anyhow!("changed_refnos[{index}]={value:?} 非法: {error}")
            })?;
            anyhow::ensure!(parsed.0 != 0, "changed_refnos[{index}] 不得是零 refno");
            Ok(RefnoEnum::from(parsed))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectKind {
    SystDerived,
    /// 某个窗口的反向引用索引没维护上，需要按引用者定点重建（ADR-003）。
    RefRevMaintain,
    /// 水位提交后必须完成的空间树刷新/删除与文件持久化。
    SpatialReconcile,
    /// **已退役**（ADR-064 Q2，2026-09-07）：曾是「某个 DESI 窗口触及的生成根，其 CATA 引用闭包
    /// 水位之后解析进 Surreal，只为 `ref_rev` 与 UI 目录属性」（ADR-056 D8-A）。现在目录链依赖由
    /// `cata_dependency::persist_cata_dependencies` 排在 DESI 块与水位**之前**落行（`pe.dependency = true`），
    /// 这一档没有活可干。变体留着只为认得旧库里已经排进表的行：drain 到它直接收口，不再解析、不再写。
    CataRefRev,
}

impl SideEffectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::SystDerived => "syst_derived",
            Self::RefRevMaintain => "ref_rev_maintain",
            Self::SpatialReconcile => "spatial_reconcile",
            Self::CataRefRev => "cata_ref_rev",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingJob {
    pub id: Thing,
    pub kind: String,
    pub dbnum: u32,
    pub end_sesno: i32,
    pub db_type: String,
    #[serde(default)]
    pub changed_refnos: Vec<String>,
    #[serde(default)]
    pub refresh_refnos: Vec<String>,
    #[serde(default)]
    pub remove_refnos: Vec<String>,
    /// Room edges that must be cleared only after this file's spatial view is
    /// published. Kept here so a crash cannot turn a deferred delete into a
    /// permanent dangling edge.
    #[serde(default)]
    pub room_clear_refnos: Vec<String>,
    #[serde(default)]
    pub model_publish_roots: Vec<String>,
    #[serde(default)]
    pub deferred_model_actions: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub commit_token: Option<String>,
    #[serde(default)]
    pub room_released: bool,
    #[serde(default)]
    pub published_version: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct PublishedFileRound {
    pub dbnum: u32,
    pub end_sesno: i32,
    pub room_clear_refnos: Vec<String>,
}

/// One database file in the composite input consumed by room topology. The
/// token identifies both the physical file generation and its exact session;
/// rows are replaced only by a successful spatial publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PublishedFileInput {
    pub dbnum: u32,
    pub db_type: String,
    pub end_sesno: i32,
    pub commit_token: String,
    #[serde(default)]
    pub published_version: u64,
}

/// Identity of the composite frozen input represented by a spatial snapshot.
///
/// `published_version` is the version after every currently committed spatial
/// publication intent has been acknowledged.  While a crash window still has
/// pending intents, callers derive the same future value by replaying those
/// intents over the last committed manifest.  This makes the snapshot written
/// before acknowledgement comparable with the manifest both before and after
/// a partial acknowledgement restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedInputManifestStamp {
    pub published_version: u64,
    pub file_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeferredFileModelAction {
    pub action: String,
    pub target_refno: String,
    pub noun: String,
}

/// Independent module: enqueue / complete / drain side-effect jobs.
#[derive(Debug, Default, Clone)]
pub struct SideEffectCompensator;

impl SideEffectCompensator {
    fn record_id(kind: SideEffectKind, dbnum: u32, end_sesno: i32) -> String {
        format!("{}:{}_{}_{}", TABLE, kind.as_str(), dbnum, end_sesno)
    }

    /// Establish the upgrade baseline before the first file round becomes
    /// private. All current DESI files are captured under the spatial serial
    /// lock, so a later room drain never fills a missing row from a live,
    /// partially applied source.
    pub(crate) async fn ensure_published_file_baseline(
        active_dbnum: u32,
        active_previous_sesno: u32,
    ) -> anyhow::Result<()> {
        let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
        crate::fast_model::spatial_state::ensure_spatial_ready()
            .map_err(|error| anyhow::anyhow!("空间发布基线尚不可建立: {error:#}"))?;
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {PUBLISHED_ROOM_BASELINE_ID} LIMIT 1)) > 0; \
                 RETURN array::len((SELECT VALUE id FROM {TABLE} \
                   WHERE kind = 'spatial_reconcile' AND status IN ['collecting', 'pending'] LIMIT 1)) > 0;"
            ))
            .await?
            .check()?;
        if response.take::<Option<bool>>(0)?.unwrap_or(false) {
            return Ok(());
        }
        anyhow::ensure!(
            !response.take::<Option<bool>>(1)?.unwrap_or(false),
            "房间发布输入基线尚未建立，但存在未完成文件发布；拒绝从可能半发布的实时模型补基线"
        );

        let committed = Self::published_file_inputs().await?;
        if !committed.is_empty() {
            for input in &committed {
                let source =
                    crate::data_interface::frozen_source::FrozenSource::from_commit_token_json(
                        &input.commit_token,
                    )?;
                crate::fast_model::room_topology_source::prepare_producer_source(&source).await?;
            }
            let mut statements = committed
                .iter()
                .map(|input| {
                    crate::fast_model::room_publication::render_stage_published_room_input(
                        input.dbnum,
                        &input.commit_token,
                    )
                })
                .collect::<Vec<_>>();
            statements.push(
                crate::fast_model::room_topology::render_stage_published_room_topology(&committed)
                    .await?,
            );
            statements.push(format!(
                "UPSERT {PUBLISHED_ROOM_BASELINE_ID} SET established_at = time::now();"
            ));
            SUL_DB
                .query(format!(
                    "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
                    statements.join(" ")
                ))
                .await?
                .check()?;
            return Ok(());
        }

        let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
        let sources = service.design_sources()?;
        let published_version = Self::published_spatial_version().await?;
        let mut statements = Vec::with_capacity(sources.len());
        let mut baseline_inputs = Vec::with_capacity(sources.len());
        for (dbnum, path, sesno) in sources {
            if dbnum == active_dbnum && active_previous_sesno == 0 {
                // A file with no previously applied state has no old published
                // room input. It enters the manifest only after this round
                // successfully publishes.
                continue;
            }
            let baseline_sesno = if dbnum == active_dbnum {
                active_previous_sesno
            } else {
                sesno
            };
            let frozen = crate::data_interface::frozen_source::FrozenSource::capture_at(
                &path,
                baseline_sesno,
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "建立房间发布基线时冻结 dbnum={dbnum} {}@{baseline_sesno} 失败: {error:#}",
                    path.display()
                )
            })?;
            let token_raw = frozen.commit_token_json()?;
            crate::fast_model::room_topology_source::prepare_producer_source(&frozen).await?;
            statements.push(
                crate::fast_model::room_publication::render_stage_published_room_input(
                    dbnum, &token_raw,
                ),
            );
            statements.push(Self::render_published_file_input_upsert(
                dbnum,
                frozen.db_type(),
                baseline_sesno as i32,
                &token_raw,
                published_version,
            ));
            baseline_inputs.push(PublishedFileInput {
                dbnum,
                db_type: frozen.db_type().to_string(),
                end_sesno: baseline_sesno as i32,
                commit_token: token_raw,
                published_version,
            });
        }
        if !baseline_inputs.is_empty() {
            statements.push(
                crate::fast_model::room_topology::render_stage_published_room_topology(
                    &baseline_inputs,
                )
                .await?,
            );
        }
        statements.push(format!(
            "UPSERT {PUBLISHED_ROOM_BASELINE_ID} SET established_at = time::now();"
        ));
        let sql = format!(
            "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
            statements.join(" ")
        );
        SUL_DB.query(sql).await?.check()?;
        // The first file-input baseline changes the manifest even when the
        // spatial tree is empty. Persist that header while this publication
        // lock still excludes private model writes and later file rounds.
        anyhow::ensure!(
            crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty_locked_for_publication(&[])
                .await?,
            "published input baseline could not persist its spatial snapshot"
        );
        Ok(())
    }

    pub(crate) async fn published_file_inputs() -> anyhow::Result<Vec<PublishedFileInput>> {
        Self::published_file_inputs_on(&SUL_DB).await
    }

    /// [`Self::published_file_inputs`] 的显式连接版——`mem://` 门直接喂。
    pub(crate) async fn published_file_inputs_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<Vec<PublishedFileInput>> {
        let mut response = db
            .query(format!(
                "SELECT dbnum, db_type, end_sesno, commit_token, published_version \
                 FROM {PUBLISHED_FILE_INPUT_TABLE} ORDER BY dbnum ASC;"
            ))
            .await?
            .check()?;
        response
            .take::<Vec<PublishedFileInput>>(0)
            .map_err(anyhow::Error::from)
    }

    /// 一行清单的写法——基线与启动房间初始化（`room_resident::initialize_at_startup`，
    /// 只替**不归任何文件轮次管**的文件前进）共用这一个定义；文件轮次自己的推进走
    /// `apply_spatial_publications_to_manifest` 那条路，不经这里。
    pub(crate) fn render_published_file_input_upsert(
        dbnum: u32,
        db_type: &str,
        end_sesno: i32,
        commit_token: &str,
        published_version: u64,
    ) -> String {
        let token = escape_surql_str(commit_token);
        let db_type = escape_surql_str(db_type);
        format!(
            "UPSERT {PUBLISHED_FILE_INPUT_TABLE}:{dbnum} SET dbnum = {dbnum}, \
             db_type = '{db_type}', end_sesno = {end_sesno}, commit_token = '{token}', \
             published_version = {published_version}, updated_at = time::now();"
        )
    }

    /// 有没有一个文件轮次正在发布（`spatial_reconcile` 在 `collecting` / `pending`）。
    /// 基线与启动房间初始化都拿它当「实时行可能半发布」的信号：有就不写清单。
    pub(crate) async fn has_inflight_file_round_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<bool> {
        let mut response = db
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {TABLE} \
                   WHERE kind = 'spatial_reconcile' AND status IN ['collecting', 'pending'] LIMIT 1)) > 0;"
            ))
            .await?
            .check()?;
        Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
    }

    fn apply_spatial_publications_to_manifest(
        mut inputs: Vec<PublishedFileInput>,
        mut published_version: u64,
        jobs: &[PendingJob],
    ) -> PublishedInputManifestStamp {
        use sha2::{Digest, Sha256};
        use std::collections::BTreeMap;

        let mut by_dbnum = inputs
            .drain(..)
            .map(|input| (input.dbnum, input))
            .collect::<BTreeMap<_, _>>();
        for job in jobs {
            published_version += 1;
            if let Some(commit_token) = &job.commit_token {
                by_dbnum.insert(
                    job.dbnum,
                    PublishedFileInput {
                        dbnum: job.dbnum,
                        db_type: job.db_type.clone(),
                        end_sesno: job.end_sesno,
                        commit_token: commit_token.clone(),
                        published_version,
                    },
                );
            }
        }

        // Length-prefix every variable field.  The digest therefore has one
        // unambiguous canonical byte representation and does not depend on JSON
        // map ordering or separator escaping.
        let mut digest = Sha256::new();
        for input in by_dbnum.values() {
            digest.update(input.dbnum.to_le_bytes());
            digest.update((input.db_type.len() as u64).to_le_bytes());
            digest.update(input.db_type.as_bytes());
            digest.update(input.end_sesno.to_le_bytes());
            digest.update((input.commit_token.len() as u64).to_le_bytes());
            digest.update(input.commit_token.as_bytes());
            digest.update(input.published_version.to_le_bytes());
        }
        PublishedInputManifestStamp {
            published_version,
            file_count: by_dbnum.len() as u64,
            sha256: hex::encode(digest.finalize()),
        }
    }

    pub(crate) async fn current_published_input_manifest()
    -> anyhow::Result<PublishedInputManifestStamp> {
        let inputs = Self::published_file_inputs().await?;
        let published_version = Self::published_spatial_version().await?;
        Ok(Self::apply_spatial_publications_to_manifest(
            inputs,
            published_version,
            &[],
        ))
    }

    pub(crate) async fn candidate_published_input_manifest(
        jobs: &[PendingJob],
    ) -> anyhow::Result<PublishedInputManifestStamp> {
        let inputs = Self::published_file_inputs().await?;
        let published_version = Self::published_spatial_version().await?;
        Ok(Self::apply_spatial_publications_to_manifest(
            inputs,
            published_version,
            jobs,
        ))
    }

    pub(crate) async fn candidate_published_file_inputs(
        jobs: &[PendingJob],
    ) -> anyhow::Result<Vec<PublishedFileInput>> {
        use std::collections::BTreeMap;

        let mut published_version = Self::published_spatial_version().await?;
        let mut by_dbnum = Self::published_file_inputs()
            .await?
            .into_iter()
            .map(|input| (input.dbnum, input))
            .collect::<BTreeMap<_, _>>();
        for job in jobs {
            published_version += 1;
            if let Some(commit_token) = &job.commit_token {
                by_dbnum.insert(
                    job.dbnum,
                    PublishedFileInput {
                        dbnum: job.dbnum,
                        db_type: job.db_type.clone(),
                        end_sesno: job.end_sesno,
                        commit_token: commit_token.clone(),
                        published_version,
                    },
                );
            }
        }
        Ok(by_dbnum.into_values().collect())
    }

    /// Return every manifest identity that a safe pre-ack snapshot may carry.
    ///
    /// A reconcile takes a stable prefix of the ordered pending rows. A newer
    /// file tail may append another row while the tree is being persisted, so
    /// startup accepts the committed manifest plus any ordered pending prefix.
    /// It still rejects an arbitrary digest, and replay applies the entire
    /// remaining journal before consumers are released.
    pub(crate) async fn acceptable_published_input_manifests()
    -> anyhow::Result<Vec<PublishedInputManifestStamp>> {
        let inputs = Self::published_file_inputs().await?;
        let published_version = Self::published_spatial_version().await?;
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'pending' ORDER BY updated_at ASC;"
            ))
            .await?
            .check()?;
        let jobs = response.take::<Vec<PendingJob>>(0)?;
        Ok((0..=jobs.len())
            .map(|count| {
                Self::apply_spatial_publications_to_manifest(
                    inputs.clone(),
                    published_version,
                    &jobs[..count],
                )
            })
            .collect())
    }

    /// Recover the frozen endpoint of a file round that survived a process
    /// restart. The source token, rather than the current file header, remains
    /// authoritative until that round publishes or is explicitly replaced.
    pub(crate) async fn recover_collecting_file_round(
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<Option<(i32, String)>> {
        Self::recover_collecting_file_round_on(&SUL_DB, dbnum, end_sesno).await
    }

    async fn recover_collecting_file_round_on(
        db: &Surreal<Any>,
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<Option<(i32, String)>> {
        #[derive(Deserialize)]
        struct RecoveryRow {
            end_sesno: i32,
            commit_token: Option<String>,
        }
        let mut response = db
            .query(format!(
                "SELECT end_sesno, commit_token FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND dbnum = {dbnum} AND end_sesno = {end_sesno} \
                 AND status = 'collecting' LIMIT 1;"
            ))
            .await?
            .check()?;
        let row = response.take::<Option<RecoveryRow>>(0)?;
        row.map(|row| {
            let token = row.commit_token.ok_or_else(|| {
                anyhow::anyhow!(
                    "collecting file round dbnum={} end={} has no frozen source token",
                    dbnum,
                    row.end_sesno
                )
            })?;
            Ok((row.end_sesno, token))
        })
        .transpose()
    }

    fn render_replace_collecting_file_round(
        dbnum: u32,
        old_end_sesno: i32,
        new_end_sesno: i32,
        db_type: &str,
        commit_token: &str,
        reason: &str,
    ) -> String {
        let old_id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, old_end_sesno);
        let new_id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, new_end_sesno);
        let db_type = escape_surql_str(db_type);
        let commit_token = escape_surql_str(commit_token);
        let reason = escape_surql_str(reason);
        let supersede = if old_id == new_id {
            String::new()
        } else {
            format!(
                "UPDATE {old_id} SET status = 'superseded', replaced_by = {new_id}, \
                 last_error = '{reason}', updated_at = time::now() \
                 WHERE status = 'collecting';"
            )
        };
        format!(
            "BEGIN TRANSACTION; {supersede} \
             UPSERT {new_id} SET kind = 'spatial_reconcile', dbnum = {dbnum}, \
               end_sesno = {new_end_sesno}, db_type = '{db_type}', \
               changed_refnos = [], refresh_refnos = [], remove_refnos = [], \
               room_clear_refnos = [], model_publish_roots = [], deferred_model_actions = [], \
               commit_token = '{commit_token}', status = 'collecting', room_released = false, \
               attempts = 0, last_error = NONE, replacement_of = {old_id}, \
               replacement_reason = '{reason}', \
               replacement_revision = (replacement_revision?:0) + 1, updated_at = time::now(); \
             COMMIT TRANSACTION;"
        )
    }

    /// The frozen generation can no longer be reopened. Retire its unpublished
    /// candidate and durably seed an empty replacement round; the worker then
    /// queues a `Reinitialize` batch which must rebuild the complete file.
    pub(crate) async fn replace_collecting_file_round(
        dbnum: u32,
        old_end_sesno: i32,
        source: &crate::data_interface::frozen_source::FrozenSource,
        reason: &str,
    ) -> anyhow::Result<()> {
        let sql = Self::render_replace_collecting_file_round(
            dbnum,
            old_end_sesno,
            source.target_sesno() as i32,
            source.db_type(),
            &source.commit_token_json()?,
            reason,
        );
        SUL_DB.query(sql).await?.check()?;
        Ok(())
    }

    /// Render one durable post-commit spatial intent for the window tail transaction.
    pub(crate) fn render_spatial_reconcile_upsert(
        dbnum: u32,
        end_sesno: i32,
        refresh_refnos: &[String],
        remove_refnos: &[String],
    ) -> anyhow::Result<String> {
        if let Some((active_dbnum, round_end)) =
            crate::data_interface::batch_worker::active_data_window()
            && active_dbnum == dbnum
        {
            return Self::render_file_spatial_reconcile_upsert(
                dbnum,
                round_end,
                refresh_refnos,
                remove_refnos,
                &[],
            );
        }
        let refresh = refresh_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let remove = remove_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(refno) = refresh.intersection(&remove).next() {
            anyhow::bail!("空间任务 refno {refno} 同时 refresh/remove");
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let refresh_json = serde_json::to_string(&refresh.into_iter().collect::<Vec<_>>())?;
        let remove_json = serde_json::to_string(&remove.into_iter().collect::<Vec<_>>())?;
        Ok(format!(
            "UPSERT {id} SET kind = 'spatial_reconcile', dbnum = {dbnum}, \
             end_sesno = {end_sesno}, db_type = 'DESI', changed_refnos = [], \
             refresh_refnos = {refresh_json}, remove_refnos = {remove_json}, \
             status = 'pending', attempts = attempts?:0, last_error = NONE, \
             updated_at = time::now();"
        ))
    }

    /// Merge one committed model/AABB chunk into its file round without making
    /// it visible to the spatial replay worker. The first old state is the
    /// currently published tree; later chunks collapse to the final intent.
    pub(crate) fn render_file_spatial_reconcile_upsert(
        dbnum: u32,
        round_end_sesno: i32,
        refresh_refnos: &[String],
        remove_refnos: &[String],
        room_clear_refnos: &[String],
    ) -> anyhow::Result<String> {
        let refresh = refresh_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let remove = remove_refnos
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(refno) = refresh.intersection(&remove).next() {
            anyhow::bail!("空间任务 refno {refno} 同时 refresh/remove");
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let refresh_json = serde_json::to_string(&refresh.into_iter().collect::<Vec<_>>())?;
        let remove_json = serde_json::to_string(&remove.into_iter().collect::<Vec<_>>())?;
        let clear_json = serde_json::to_string(
            &room_clear_refnos
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>(),
        )?;
        let commit_token = crate::data_interface::batch_worker::active_frozen_source()?
            .map(|source| source.commit_token_json())
            .transpose()?
            .map(|token| format!("'{}'", escape_surql_str(&token)))
            .unwrap_or_else(|| "NONE".into());
        Ok(format!(
            "UPSERT {id} SET kind = 'spatial_reconcile', dbnum = {dbnum}, \
             end_sesno = {round_end_sesno}, db_type = 'DESI', changed_refnos = [], \
             refresh_refnos = array::union(array::difference(refresh_refnos?:[], {remove_json}), {refresh_json}), \
             remove_refnos = array::union(array::difference(remove_refnos?:[], {refresh_json}), {remove_json}), \
             room_clear_refnos = array::union(room_clear_refnos?:[], {clear_json}), \
             model_publish_roots = model_publish_roots?:[], \
             deferred_model_actions = deferred_model_actions?:[], \
             commit_token = {commit_token}, status = 'collecting', \
             room_released = false, attempts = attempts?:0, last_error = NONE, updated_at = time::now();"
        ))
    }

    pub(crate) fn render_file_model_publish_root(
        dbnum: u32,
        round_end_sesno: i32,
        root: &str,
    ) -> anyhow::Result<String> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let root = serde_json::to_string(root)?;
        Ok(format!(
            "UPDATE {id} SET model_publish_roots = array::union(model_publish_roots?:[], [{root}]), \
             updated_at = time::now() WHERE status = 'collecting';"
        ))
    }

    pub(crate) async fn file_round_model_publish_roots(
        dbnum: u32,
        round_end_sesno: i32,
    ) -> anyhow::Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Row {
            #[serde(default)]
            model_publish_roots: Vec<String>,
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let mut response = SUL_DB
            .query(format!("SELECT model_publish_roots FROM {id};"))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<Row>>(0)?
            .into_iter()
            .flat_map(|row| row.model_publish_roots)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub(crate) fn render_deferred_file_model_action(
        dbnum: u32,
        round_end_sesno: i32,
        action: DeferredFileModelAction,
    ) -> anyhow::Result<String> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let encoded = serde_json::to_string(&serde_json::to_string(&action)?)?;
        Ok(format!(
            "UPDATE {id} SET deferred_model_actions = array::union(deferred_model_actions?:[], [{encoded}]), \
             updated_at = time::now() WHERE status = 'collecting';"
        ))
    }

    pub(crate) async fn file_round_deferred_model_actions(
        dbnum: u32,
        round_end_sesno: i32,
    ) -> anyhow::Result<Vec<DeferredFileModelAction>> {
        #[derive(Deserialize)]
        struct Row {
            #[serde(default)]
            deferred_model_actions: Vec<String>,
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let mut response = SUL_DB
            .query(format!("SELECT deferred_model_actions FROM {id};"))
            .await?
            .check()?;
        response
            .take::<Vec<Row>>(0)?
            .into_iter()
            .flat_map(|row| row.deferred_model_actions)
            .map(|encoded| serde_json::from_str(&encoded).map_err(anyhow::Error::from))
            .collect()
    }

    pub(crate) async fn file_round_commit_token(
        dbnum: u32,
        round_end_sesno: i32,
    ) -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Row {
            commit_token: Option<String>,
        }
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let mut response = SUL_DB
            .query(format!(
                "SELECT commit_token FROM {id} WHERE status = 'collecting';"
            ))
            .await?
            .check()?;
        response
            .take::<Option<Row>>(0)?
            .and_then(|row| row.commit_token)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "collecting file round dbnum={dbnum} end={round_end_sesno} has no commit token"
                )
            })
    }

    pub(crate) async fn file_round(
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<Option<PendingJob>> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let mut response = SUL_DB
            .query(format!("SELECT * FROM {id};"))
            .await?
            .check()?;
        Ok(response.take::<Vec<PendingJob>>(0)?.into_iter().next())
    }

    /// Turn exactly one completed file round into replayable work. Collecting
    /// rows from incomplete files remain invisible to normal spatial draining.
    pub(crate) async fn promote_file_round(
        dbnum: u32,
        round_end_sesno: i32,
    ) -> anyhow::Result<Vec<String>> {
        Self::promote_file_round_on(&SUL_DB, dbnum, round_end_sesno).await
    }

    pub(crate) async fn promote_file_round_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        round_end_sesno: i32,
    ) -> anyhow::Result<Vec<String>> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, round_end_sesno);
        let rows: Vec<PendingJob> = db
            .query(format!("SELECT * FROM {id};"))
            .await?
            .check()?
            .take(0)?;
        let round = rows
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("file round missing before promotion"))?;
        if round.status != "collecting" {
            return Ok(Vec::new());
        }
        let token = round
            .commit_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("file round has no exact source token"))?;
        let key = crate::fast_model::room_topology_source::source_key(token);
        let token = serde_json::to_string(token)?;
        let topology_guard = if round.db_type.eq_ignore_ascii_case("DESI") {
            format!(
                "LET $topology=(SELECT * FROM room_topology_source:⟨{key}⟩)[0]; \
                IF $topology IS NONE OR $topology.complete != true OR $topology.source_token != {token} \
                {{ THROW 'missing_exact_room_topology_source'; }};"
            )
        } else {
            String::new()
        };
        db.query(format!("BEGIN TRANSACTION; LET $round=(SELECT * FROM {id})[0]; \
            IF $round.commit_token != {token} OR $round.status != 'collecting' {{ THROW 'file_round_promotion_changed'; }}; \
            {topology_guard} UPDATE {id} SET status='pending',updated_at=time::now() RETURN NONE; COMMIT TRANSACTION;"))
            .await?.check()?;
        Ok(round
            .room_clear_refnos
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    /// Spatial replay is already durable at this point; these rows describe the
    /// second half of publication that a crash may have interrupted.
    pub(crate) async fn published_file_rounds_awaiting_rooms()
    -> anyhow::Result<Vec<PublishedFileRound>> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'done' AND (room_released?:false) = false ORDER BY updated_at ASC;"
            ))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<PendingJob>>(0)?
            .into_iter()
            .map(|row| PublishedFileRound {
                dbnum: row.dbnum,
                end_sesno: row.end_sesno,
                room_clear_refnos: row.room_clear_refnos,
            })
            .collect())
    }

    pub(crate) async fn mark_file_rooms_released(
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        Self::mark_file_rooms_released_on(&SUL_DB, dbnum, end_sesno).await
    }

    /// During startup, project-wide room rebuilding owns all room work. A file
    /// round therefore completes after spatial publication even when legacy
    /// room rows still name that round; the project rebuild clears only rows
    /// older than its cutoff after it has successfully published all rooms.
    pub(crate) async fn mark_file_rooms_released_for_project_rebuild(
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        Self::mark_file_rooms_released_for_project_rebuild_on(&SUL_DB, dbnum, end_sesno).await
    }

    async fn mark_file_rooms_released_for_project_rebuild_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let mut response = db
            .query(format!(
                "UPDATE {id} SET room_released = true, updated_at = time::now() \
                 WHERE status = 'done' RETURN VALUE id;"
            ))
            .await?
            .check()?;
        let released: Vec<Thing> = response.take(0)?;
        anyhow::ensure!(
            released.len() == 1,
            "file round {dbnum}/{end_sesno} is not spatially done"
        );
        Self::completed_file_round_on(db, dbnum, end_sesno).await
    }

    pub(crate) async fn mark_file_rooms_released_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let round = escape_surql_str(&format!("{dbnum}:{end_sesno}"));
        let mut response = db
            .query(format!(
                "UPDATE {id} SET room_released = true, updated_at = time::now() \
                 WHERE status = 'done' AND array::len((SELECT VALUE id FROM model_update_pending \
                 WHERE file_rounds CONTAINS '{round}' AND action IN \
                 ['room_recalc_panel', 'room_recalc_element'])) = 0 RETURN VALUE id;"
            ))
            .await?
            .check()?;
        let released: Vec<Thing> = response.take(0)?;
        anyhow::ensure!(
            released.len() == 1,
            "file round {dbnum}/{end_sesno} is not spatially done or still has room work"
        );
        Self::completed_file_round_on(db, dbnum, end_sesno).await
    }

    pub(crate) async fn completed_file_round(
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        Self::completed_file_round_on(&SUL_DB, dbnum, end_sesno).await
    }

    pub(crate) async fn completed_file_round_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        Self::file_publication_proof_on(db, dbnum, end_sesno, true).await
    }

    pub(crate) async fn published_model_file_round_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        end_sesno: i32,
    ) -> anyhow::Result<CompletedFileRound> {
        Self::file_publication_proof_on(db, dbnum, end_sesno, false).await
    }

    async fn file_publication_proof_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        dbnum: u32,
        end_sesno: i32,
        require_rooms: bool,
    ) -> anyhow::Result<CompletedFileRound> {
        let id = Self::record_id(SideEffectKind::SpatialReconcile, dbnum, end_sesno);
        let room_guard = if require_rooms {
            " AND room_released = true"
        } else {
            ""
        };
        #[derive(Deserialize)]
        struct Row {
            commit_token: Option<String>,
        }
        let mut response = db
            .query(format!(
                "SELECT commit_token FROM {id} WHERE status = 'done'{room_guard};"
            ))
            .await?
            .check()?;
        let mut rows: Vec<Row> = response.take(0)?;
        anyhow::ensure!(
            rows.len() == 1,
            "file round {dbnum}/{end_sesno} has no completion proof"
        );
        Ok(CompletedFileRound {
            dbnum,
            end_sesno,
            commit_token: rows.pop().unwrap().commit_token,
        })
    }

    /// After PE+watermark success: enqueue only legacy non-model side effects.
    /// Model work is now persisted by `IncrementPipeline` before its watermark.
    ///
    /// One row per SYST file. A single row keyed by the FIRST success's `dbnum`
    /// paired with the LARGEST `end_sesno` across all of them used to stand in
    /// for the batch — with two SYST databases in one batch that pair described
    /// neither file, so the row could not be traced back to what produced it.
    pub async fn enqueue_from_incr(_mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        for success in Self::syst_successes(incr) {
            Self::upsert_pending(
                SideEffectKind::SystDerived,
                success.dbnum,
                success.end_sesno,
                &success.db_type,
                &[],
            )
            .await?;
        }
        Ok(())
    }

    fn syst_successes(
        incr: &IncrResult,
    ) -> impl Iterator<Item = &crate::data_interface::increment_pipeline::IncrFileSuccess> {
        incr.successes.iter().filter(|s| s.db_type == "SYST")
    }

    /// 单个 SYST 批次落库后登记一条派生同步任务（数据批次 worker 用）。
    ///
    /// 与 [`Self::enqueue_from_incr`] 是同一张表、同一种行 id——worker 一次只
    /// 执行一个文件，没有 `IncrResult` 可给，就按批次直接记。
    pub async fn enqueue_syst(dbnum: u32, end_sesno: i32, db_type: &str) -> anyhow::Result<()> {
        Self::upsert_pending(SideEffectKind::SystDerived, dbnum, end_sesno, db_type, &[]).await
    }

    /// 反向引用索引没维护上 → 记一条定点重建任务。
    ///
    /// 按 ADR-003，`ref_rev` 是「关联模型也要更新」的权威来源：缺一条边就是某个设计
    /// 实例静默不重生成。这件事过去只留一句 warning，而那句话既没人读、也没有任何东西
    /// 会自动触发重建，只能等有人手工跑全量。既然补偿队列就在旁边，让它走同一条重试
    /// 通道、同一个 `MAX_ATTEMPTS`。
    ///
    /// 带上引用者名单而不是会话区间：重建从库里的当前状态算（PE 主数据早于本步落库），
    /// 不必回头再解析一遍文件。
    pub async fn enqueue_ref_rev(
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        referrers: &[RefU64],
    ) -> anyhow::Result<()> {
        if referrers.is_empty() {
            return Ok(());
        }
        Self::upsert_pending(
            SideEffectKind::RefRevMaintain,
            dbnum,
            end_sesno,
            db_type,
            referrers,
        )
        .await
    }

    /// 入队语句的纯渲染，返回 `(记录 id, SurrealQL)`。
    ///
    /// `attempts = attempts?:0` 是这条语句的要害。同一个 `(kind, dbnum, end_sesno)`
    /// 会被重复 UPSERT——两个入队点按写法路由二选一，失败重试还会再来——保住已有的
    /// 重试计数才让 [`MAX_ATTEMPTS`] 有意义；清零的话一条永远失败的作业每来一个批次
    /// 就复活一次，死信这一档永远立不起来。
    fn render_upsert_pending(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        changed_refnos: &[RefU64],
    ) -> (String, String) {
        let id = Self::record_id(kind, dbnum, end_sesno);
        let refno_json = serde_json::to_string(
            &changed_refnos
                .iter()
                .map(|r| r.to_pdms_str())
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".into());
        let db_type = escape_surql_str(db_type);
        let sql = format!(
            "UPSERT {id} SET \
             kind = '{}', dbnum = {dbnum}, end_sesno = {end_sesno}, \
             db_type = '{db_type}', changed_refnos = {refno_json}, \
             status = 'pending', attempts = attempts?:0, \
             updated_at = time::now();",
            kind.as_str(),
        );
        (id, sql)
    }

    async fn upsert_pending(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        db_type: &str,
        changed_refnos: &[RefU64],
    ) -> anyhow::Result<()> {
        let (id, sql) =
            Self::render_upsert_pending(kind, dbnum, end_sesno, db_type, changed_refnos);
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("enqueue {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("enqueue {id} statement failed: {e}"))?;
        println!("SideEffectCompensator: enqueued {id}");
        Ok(())
    }

    fn render_mark_done(kind: SideEffectKind, dbnum: u32, end_sesno: i32) -> String {
        let id = Self::record_id(kind, dbnum, end_sesno);
        if kind == SideEffectKind::SpatialReconcile {
            format!(
                "BEGIN TRANSACTION; \
                 UPSERT {PUBLISHED_SPATIAL_VERSION_ID} SET value = (value?:0) + 1, updated_at = time::now(); \
                 LET $published = (SELECT VALUE `value` FROM {PUBLISHED_SPATIAL_VERSION_ID})[0]; \
                 LET $round = (SELECT commit_token, db_type FROM {id})[0]; \
                 IF $round.commit_token != NONE {{ \
                   UPSERT {PUBLISHED_FILE_INPUT_TABLE}:{dbnum} SET dbnum = {dbnum}, \
                     db_type = $round.db_type, end_sesno = {end_sesno}, \
                     commit_token = $round.commit_token, published_version = $published, \
                     updated_at = time::now(); \
                 }}; \
                 UPDATE {id} SET status = 'done', published_version = $published, \
                    last_error = NONE, updated_at = time::now(); \
                 COMMIT TRANSACTION;"
            )
        } else {
            format!("UPDATE {id} SET status = 'done', last_error = NONE, updated_at = time::now();")
        }
    }

    pub async fn mark_done(kind: SideEffectKind, dbnum: u32, end_sesno: i32) -> anyhow::Result<()> {
        let id = Self::record_id(kind, dbnum, end_sesno);
        let sql = Self::render_mark_done(kind, dbnum, end_sesno);
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("mark_done {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("mark_done {id} statement failed: {e}"))?;
        Ok(())
    }

    pub(crate) async fn published_spatial_version() -> anyhow::Result<u64> {
        #[derive(Deserialize)]
        struct Row {
            #[serde(default)]
            value: u64,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT `value` FROM {PUBLISHED_SPATIAL_VERSION_ID};"
            ))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<Row>>(0)?
            .into_iter()
            .next()
            .map_or(0, |row| row.value))
    }

    pub async fn mark_failed(
        kind: SideEffectKind,
        dbnum: u32,
        end_sesno: i32,
        err: &str,
    ) -> anyhow::Result<()> {
        let id = Self::record_id(kind, dbnum, end_sesno);
        // 错误信息常含 Windows 路径的反斜杠，需与单引号一起转义，否则会破坏
        // SurrealQL 字符串字面量导致本次 mark_failed 失败（attempts 不自增、
        // last_error 不落库）。复用 dbnum_state 的统一转义。
        let escaped = escape_surql_str(err);
        let sql = format!(
            "UPDATE {id} SET status = 'failed', attempts = (attempts?:0) + 1, \
             last_error = '{escaped}', updated_at = time::now();"
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("mark_failed {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("mark_failed {id} statement failed: {e}"))?;
        Ok(())
    }

    /// Complete every committed spatial intent before the worker admits another data batch.
    ///
    /// **空间收敛不重试**（2026-09-06 用户拍板：出错只进错误日志、方便排查，不做错误
    /// 重试）。此前这条路刻意无视 [`MAX_ATTEMPTS`]——理由是「放弃一条就会发布一个空间
    /// 状态永远追不平的水位」——代价是一条**确定性**失败的行（rocksdb 空库首启时 COMM
    /// 8190 的文件轮次，`current MDB has no DESI dbnum 8190`）让出队门永远关着、余下
    /// 全部数据批次挂死。现在一次失败就把该行标成 `dead`、原因落
    /// `logs/batch-failures-*.jsonl`（`event = side_effect_dead_letter`），后续轮次的选行
    /// 谓词看不见它，出队门与房间轮照常放行；修好原因的人把行放回 `pending` 即重放。
    ///
    /// 逐行收敛而不是整批一起：一条毒行不该把同批别的文件也拖进死信。
    ///
    /// 公开入口负责取空间串行锁（锁序 `DATA_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    /// → GLOBAL_AABB_TREE`：worker 提交路径与派发门先持前者再进来，Python
    /// `spatial.reconcile` 直接进来——这把锁把此前不设防的 Python 并发收敛也串行化了）。
    pub async fn reconcile_spatial_pending(_mgr: &AiosDBManager) -> anyhow::Result<usize> {
        let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
        Self::reconcile_spatial_pending_locked().await
    }

    /// 已持空间串行锁的收敛主体（启动装载的立即重放、revalidator 复检等持锁方用）。
    ///
    /// 只取 `pending`：`failed`（老版本留下的重试中间态）与 `dead` 都是终态，不再进候选集。
    /// 返回值是本轮收敛成功的行数；只要有行进了死信就返回 `Err`（调用方要知道这一轮
    /// 没有全部收敛），但死信行已经从候选集里消失，下一轮不会再因它失败。
    pub(crate) async fn reconcile_spatial_pending_locked() -> anyhow::Result<usize> {
        let query = format!(
            "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
             AND status = 'pending' ORDER BY updated_at ASC;"
        );
        let pending: Vec<PendingJob> =
            crate::surreal_retry::retry_sul_db_transport("读取待收敛空间任务", || {
                let query = query.clone();
                async move {
                    let mut response = SUL_DB.query(query).await?.check()?;
                    Ok(response.take(0)?)
                }
            })
            .await?;
        if pending.is_empty() {
            Self::promote_state_after_replay().await;
            return Ok(0);
        }

        let mut done = 0usize;
        let mut dead_letters: Vec<String> = Vec::new();
        for job in &pending {
            let jobs = std::slice::from_ref(job);
            let outcome = async {
                let candidate_inputs = Self::candidate_published_file_inputs(&jobs).await?;
                crate::fast_model::room_topology::ensure_published_room_topology(
                    &candidate_inputs,
                )
                .await?;
                let mut deferred =
                    crate::data_interface::staging::write_context::DeferredSpatialMutations::default();
                for entry in jobs {
                    for raw in &entry.refresh_refnos {
                        let refno = raw
                            .parse::<RefU64>()
                            .map(RefnoEnum::from)
                            .map_err(|_| anyhow::anyhow!("invalid spatial refresh refno {raw}"))?;
                        deferred.remove.remove(&refno);
                        deferred.refresh.insert(refno);
                    }
                    for raw in &entry.remove_refnos {
                        let refno = raw
                            .parse::<RefU64>()
                            .map(RefnoEnum::from)
                            .map_err(|_| anyhow::anyhow!("invalid spatial remove refno {raw}"))?;
                        deferred.refresh.remove(&refno);
                        deferred.remove.insert(refno);
                    }
                }
                crate::fast_model::aabb_tree::apply_deferred_spatial_mutations(deferred).await?;
                // 崩溃窗口 ②（一致性闭环方案 §8）：树已更新、快照未发布。pending 行
                // 还在（销账在发布之后），重启按 ReplayRequired 重放收敛。
                crate::fast_model::spatial_state::failpoint("spatial_after_tree_sync");
                // 文件发布总要写 V3 快照：AABB 即使未变，发布版本与冻结输入摘要也变了。
                // 落盘仍走状态门，DegradedBlocked 等不可信状态不能覆盖最后好快照。
                crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty_locked_for_publication(&jobs)
                    .await
                    .map(|_| ())
            }
            .await;
            match outcome {
                Ok(()) => {
                    // 崩溃窗口 ④（一致性闭环方案 §8）：快照已发布、pending 未销账。重启时
                    // 指纹相等 + pending 在场 → ReplayRequired，重放幂等追认后销账。
                    crate::fast_model::spatial_state::failpoint("spatial_after_publish_before_ack");
                    crate::surreal_retry::retry_sul_db_transport(
                        "确认空间收敛任务完成",
                        || {
                            Self::mark_done(
                                SideEffectKind::SpatialReconcile,
                                job.dbnum,
                                job.end_sesno,
                            )
                        },
                    )
                    .await?;
                    done += 1;
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    Self::dead_letter_spatial_job(job, &message).await;
                    dead_letters.push(format!(
                        "{}（dbnum={} db_type={} end_sesno={}）: {message}",
                        job.id, job.dbnum, job.db_type, job.end_sesno
                    ));
                }
            }
        }
        // 死信不挡状态晋升：它已经是「这条意图永远不会自己收敛」的终态，让消费者继续
        // 等它等于把整个进程一起锁死——那正是不重试要换掉的东西。
        Self::promote_state_after_replay().await;
        if !dead_letters.is_empty() {
            anyhow::bail!(
                "{} 个空间收敛任务确定性失败，已进死信、不再重试（原因已落 logs/batch-failures-*.jsonl，\
                 修好后 UPDATE 该行 status='pending' 重放）: {}",
                dead_letters.len(),
                dead_letters.join("; ")
            );
        }
        Ok(done)
    }

    /// 把一条空间收敛行送进死信：原因先落错误日志，再把行标成 `dead`。
    ///
    /// 两步都不上抛——这里已经在处理失败，再失败一次也只能报出来；落盘写不进就靠
    /// 控制台那行，行标不上就靠下一轮再撞一次（那一次会再落一条记录、再试一次标记）。
    async fn dead_letter_spatial_job(job: &PendingJob, reason: &str) {
        let attempts = job.attempts.saturating_add(1);
        crate::data_interface::batch_failure_log::record_side_effect_dead_letter(
            &crate::data_interface::batch_failure_log::SideEffectDeadLetter {
                id: &job.id.to_string(),
                kind: &job.kind,
                dbnum: job.dbnum,
                db_type: &job.db_type,
                end_sesno: job.end_sesno,
                commit_token: job.commit_token.as_deref(),
                refresh_refnos: job.refresh_refnos.len(),
                remove_refnos: job.remove_refnos.len(),
                room_clear_refnos: job.room_clear_refnos.len(),
                attempts,
                reason,
            },
        );
        if let Err(error) = Self::mark_dead(&job.id, reason).await {
            eprintln!(
                "[副作用] {} 标记死信失败（下一轮会再撞一次同样的错并再试一次标记）: {error:#}",
                job.id
            );
        }
    }

    /// 终态 `dead`：任何选行谓词（收敛、pending 探针、计数、可接受清单）都不再看见它。
    /// 与 [`Self::mark_failed`] 的区别就在这一个词：`failed` 是重试中间态，`dead` 不是。
    async fn mark_dead(id: &Thing, reason: &str) -> anyhow::Result<()> {
        let sql = format!(
            "UPDATE {id} SET status = 'dead', attempts = (attempts?:0) + 1, \
             last_error = '{}', updated_at = time::now();",
            escape_surql_str(reason)
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("mark_dead {id} failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("mark_dead {id} statement failed: {e}"))?;
        Ok(())
    }

    /// 重放收敛成功后的状态晋升（方案 D5）：启动把状态置为 `ReplayRequired` 时，
    /// 一次成功的收敛意味着「树已追平全部已提交意图」，按树条目数晋升
    /// Ready / ReadyEmpty。其他状态不动——重建/复检各自管理自己的迁移，
    /// 常态 Ready 下的收敛也无需反复改写状态。
    async fn promote_state_after_replay() {
        use crate::fast_model::spatial_state::{self, SpatialTreeState};
        if spatial_state::current_state() == SpatialTreeState::ReplayRequired {
            let entries = aios_core::room::room::GLOBAL_AABB_TREE.read().await.size();
            spatial_state::set_ready_by_entries(entries);
            println!("空间意图重放收敛完成，空间树进入可消费状态（{entries} 条）");
        }
    }

    /// 还有没有已提交、但空间树尚未收敛的意图。
    ///
    /// 「空间树是不是陈旧的」这件事只有库里说了算：进程内的失败标志跨不过重启，
    /// 而未收敛的意图恰恰是崩溃后最该被认出来的那一类。
    ///
    /// 只数 `pending`：`dead`（以及老版本留下的 `failed`）是终态，不再是「尚未收敛的意图」——
    /// 它们在 `/health.spatial_reconcile.dead_letters` 里点名，等人处置，不挡出队与房间。
    pub async fn has_pending_spatial_work() -> anyhow::Result<bool> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'pending' LIMIT 1;"
            ))
            .await?
            .check()?;
        Ok(!response.take::<Vec<Thing>>(0)?.is_empty())
    }

    /// A collecting row is an unpublished file round. It must never be replayed,
    /// but its presence tells startup that a physical pointer/epoch mismatch is
    /// expected and the last good snapshot remains the published view.
    pub async fn has_collecting_file_rounds() -> anyhow::Result<bool> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'collecting' LIMIT 1;"
            ))
            .await?
            .check()?;
        Ok(!response.take::<Vec<Thing>>(0)?.is_empty())
    }

    /// 仍停在 `collecting` 的文件轮次本身（按 dbnum / 冻结右端升序）。
    ///
    /// 与 [`Self::has_collecting_file_rounds`] 同一谓词，只是要行而不是布尔：空闲轮拿它
    /// 逐个复核「当初卡住这一轮的失败单元是不是已经被重试消化掉了」。没有这一步，
    /// 一根确定性之外的偶发失败就能把该库的文件轮次永久钉在 `collecting`——完成屏障
    /// 只在数据批次那一刻判一次，重试成功之后没有第二个判点（审核 F-C）。
    pub(crate) async fn collecting_file_rounds() -> anyhow::Result<Vec<PendingJob>> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'collecting' ORDER BY dbnum ASC, end_sesno ASC;"
            ))
            .await?
            .check()?;
        Ok(response.take::<Vec<PendingJob>>(0)?)
    }

    /// 待收敛空间意图的条数（/health `spatial_tree.pending` 的同源镜像；
    /// 权威口径仍是 `spatial_reconcile.pending`，两处同一查询谓词）。
    pub async fn count_pending_spatial_work() -> anyhow::Result<usize> {
        #[derive(Deserialize)]
        struct Row {
            count: i64,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT count() FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 AND status = 'pending' GROUP ALL;"
            ))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<Row>>(0)?
            .first()
            .map(|row| row.count.max(0) as usize)
            .unwrap_or(0))
    }

    pub async fn spatial_reconcile_status() -> anyhow::Result<serde_json::Value> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind = 'spatial_reconcile' \
                 ORDER BY updated_at DESC LIMIT 200;"
            ))
            .await?
            .check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        // 每个仍在 collecting 的轮次问一句「差什么才能发布」。空闲轮用的是同一个判据，
        // 所以 /health 上写的就是它下一轮会做的判断，而不是另一套近似说法。读不出来
        // 时把原因当成阻塞理由写进去——/health 不因为这一格失败而整块报错。
        let mut holds = std::collections::BTreeMap::new();
        for job in jobs.iter().filter(|job| job.status == "collecting") {
            let key = (job.dbnum, job.end_sesno);
            if holds.contains_key(&key) {
                continue;
            }
            let hold =
                match crate::data_interface::model_update_pending::read_model_watermark_probe(
                    &SUL_DB, job.dbnum,
                )
                .await
                {
                    Ok(probe) => crate::data_interface::batch_worker::file_round_readiness(
                        &probe,
                        job.end_sesno,
                    )
                    .describe(),
                    Err(error) => Some(format!("完成屏障判据读取失败: {error:#}")),
                };
            holds.insert(key, hold);
        }
        Ok(Self::render_spatial_reconcile_status(&jobs, &holds))
    }

    /// /health `spatial_reconcile` 的纯渲染半边。
    ///
    /// Existing queue keys remain stable; file-round publication fields extend
    /// the shape with the current phase, frozen target and published version.
    ///
    /// `pending` = 还会被收敛的行；`dead_letters` = 进了死信的行（`dead`，以及老版本留下的
    /// `failed`）——不重试、等人处置，原因在 `logs/batch-failures-*.jsonl`
    /// （`event = side_effect_dead_letter`）。`stalled` 只在有死信时立起。
    ///
    /// `holds` 是「这一轮差什么才能发布」的现场，按 `(dbnum, end_sesno)` 索引，落在
    /// `target_sessions[].blocked` 上。收集期的轮次在 /health 上原本只有一个数字，
    /// 一根根卡住时看不出是谁在卡；`None` = 判据已满足，只是还没轮到发布。
    pub(crate) fn render_spatial_reconcile_status(
        jobs: &[PendingJob],
        holds: &std::collections::BTreeMap<(u32, i32), Option<String>>,
    ) -> serde_json::Value {
        let pending = jobs
            .iter()
            .filter(|job| job.status == "pending")
            .collect::<Vec<_>>();
        let dead = jobs
            .iter()
            .filter(|job| matches!(job.status.as_str(), "dead" | "failed"))
            .collect::<Vec<_>>();
        let collecting = jobs
            .iter()
            .filter(|job| job.status == "collecting")
            .collect::<Vec<_>>();
        serde_json::json!({
            "pending": pending.len(),
            "dead_letters": dead.len(),
            "retries": pending.iter().chain(dead.iter()).map(|job| job.attempts as u64).sum::<u64>(),
            "last_error": jobs.iter().find_map(|job| job.last_error.as_deref()),
            "stalled": !dead.is_empty(),
            "file_rounds": collecting.len(),
            "phase": if !pending.is_empty() { "spatial_publishing" }
                else if !collecting.is_empty() { "model_building" }
                else if jobs.iter().any(|job| job.status == "done" && !job.room_released) { "room_retry" }
                else { "complete" },
            "target_sessions": collecting.iter().map(|job| serde_json::json!({
                "dbnum": job.dbnum,
                "end_sesno": job.end_sesno,
                "blocked": holds.get(&(job.dbnum, job.end_sesno)).cloned().flatten(),
            })).collect::<Vec<_>>(),
            "published_version": jobs.iter().map(|job| job.published_version).max().unwrap_or(0),
        })
    }

    /// 读库失败时 /health 的降级形状：与成功形状**同键**，`stalled` 保守置真。
    ///
    /// 这个降级此前手搓在 handler 里，与成功形状只靠肉眼保持一致——现在同源于
    /// 本模块，形状测试把两个分支一起钉住。
    pub fn spatial_reconcile_error_status(error: &anyhow::Error) -> serde_json::Value {
        serde_json::json!({
            "pending": 0,
            "dead_letters": 0,
            "retries": 0,
            "last_error": format!("读取空间收敛状态失败: {error:#}"),
            "stalled": true,
            "file_rounds": serde_json::Value::Null,
            "phase": "unknown",
            "target_sessions": serde_json::Value::Null,
            "published_version": serde_json::Value::Null,
        })
    }

    /// 可 drain 的两类副作用（SystDerived / RefRevMaintain）的队列状态
    /// （/health `side_effect_pending`，P2-4）。
    ///
    /// 与 spatial 分开：spatial 由 [`Self::reconcile_spatial_pending`] 单独收敛，**不重试**，
    /// 一次失败即 `dead`（死信在 `/health.spatial_reconcile.dead_letters` 报）；这两类走
    /// [`Self::drain`]，attempts 到顶就被 drain 的上限（[`MAX_ATTEMPTS`]）挡在候选集之外
    /// 成为死信。此前 /health 只报 spatial 四键，这两类死信在接口上无处可见、也没有
    /// 复活出口（P2-4）。
    pub async fn side_effect_status() -> anyhow::Result<serde_json::Value> {
        let mut response = SUL_DB
            .query(format!(
                "SELECT * FROM {TABLE} WHERE kind != 'spatial_reconcile' \
                 AND status IN ['pending', 'failed'] ORDER BY updated_at DESC;"
            ))
            .await?
            .check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        Ok(Self::render_side_effect_status(&jobs))
    }

    /// Dead non-spatial side effects are terminal for automatic retry but not
    /// successful initialization.  The model-ready barrier uses this probe so
    /// exhausted SYST/ref-reversal work remains visible as unsettled.
    pub async fn has_dead_work() -> anyhow::Result<bool> {
        let mut response = SUL_DB
            .query(format!(
                "RETURN array::len((SELECT VALUE id FROM {TABLE} \
                 WHERE kind != 'spatial_reconcile' AND status IN ['pending', 'failed'] \
                 AND (attempts?:0) >= {MAX_ATTEMPTS} LIMIT 1)) > 0;"
            ))
            .await?
            .check()?;
        Ok(response.take::<Option<bool>>(0)?.unwrap_or(false))
    }

    /// /health `side_effect_pending` 的纯渲染半边（形状由单测钉住）。
    ///
    /// `pending` = 仍在重试预算内（drain 会取）；`dead_letters` = attempts 到顶
    /// （drain 取不到，需 [`Self::revive_dead_letters`] 人工复活）；`by_kind` 给出
    /// 两类可 drain 副作用各自的计数。供货 `jobs` 按 updated_at DESC，`last_error`
    /// 因此取最近一条。`stalled` 只在出现死信时立起。
    pub(crate) fn render_side_effect_status(jobs: &[PendingJob]) -> serde_json::Value {
        let kind_counts = |kind: &str| -> serde_json::Value {
            let dead = jobs
                .iter()
                .filter(|job| job.kind == kind && job.attempts >= MAX_ATTEMPTS)
                .count();
            let pending = jobs
                .iter()
                .filter(|job| job.kind == kind && job.attempts < MAX_ATTEMPTS)
                .count();
            serde_json::json!({ "pending": pending, "dead_letters": dead })
        };
        let dead_letters = jobs
            .iter()
            .filter(|job| job.attempts >= MAX_ATTEMPTS)
            .count();
        let pending = jobs.len() - dead_letters;
        serde_json::json!({
            "pending": pending,
            "dead_letters": dead_letters,
            "retries": jobs.iter().map(|job| job.attempts as u64).sum::<u64>(),
            "by_kind": {
                "syst_derived": kind_counts(SideEffectKind::SystDerived.as_str()),
                "ref_rev_maintain": kind_counts(SideEffectKind::RefRevMaintain.as_str()),
            },
            "last_error": jobs.iter().find_map(|job| job.last_error.as_deref()),
            "stalled": dead_letters > 0,
        })
    }

    /// 读库失败时 /health 的降级形状：与成功形状**同键**，`stalled` 保守置真。
    ///
    /// 与 spatial 同一纪律（形状测试把成功与降级两个分支一起钉住），不在 handler
    /// 里手搓 JSON。
    pub fn side_effect_error_status(error: &anyhow::Error) -> serde_json::Value {
        serde_json::json!({
            "pending": 0,
            "dead_letters": 0,
            "retries": 0,
            "by_kind": {
                "syst_derived": { "pending": 0, "dead_letters": 0 },
                "ref_rev_maintain": { "pending": 0, "dead_letters": 0 },
            },
            "last_error": format!("读取副作用补偿状态失败: {error:#}"),
            "stalled": true,
        })
    }

    async fn mark_abandoned(id: &Thing, reason: &str) -> anyhow::Result<()> {
        let sql = format!(
            "UPDATE {id} SET status = 'abandoned', last_error = '{}', updated_at = time::now();",
            escape_surql_str(reason)
        );
        SUL_DB.query(sql).await?.check()?;
        Ok(())
    }

    /// Replay pending/failed jobs (attempts < MAX_ATTEMPTS). Safe to call at init.
    ///
    /// Every job runs on its own: a failure — including a failure to write the
    /// job's own bookkeeping — is collected and reported at the end rather than
    /// aborting the round. Propagating it from inside the loop meant one flaky
    /// `UPDATE` skipped every job queued behind it, which is the same defect
    /// `model_update_pending::run_one` was rewritten to avoid. Returning `Err`
    /// once the round is over also matches that queue's contract, so a job that
    /// keeps failing surfaces in the caller's warnings instead of only in stdout.
    pub async fn drain(mgr: &AiosDBManager) -> anyhow::Result<usize> {
        let sql = format!(
            "SELECT * FROM {TABLE} WHERE status IN ['pending', 'failed'] \
             AND kind != 'spatial_reconcile' \
             AND (attempts?:0) < {MAX_ATTEMPTS} ORDER BY updated_at ASC;"
        );
        let mut response = SUL_DB.query(sql).await?.check()?;
        let jobs: Vec<PendingJob> = response.take(0)?;
        if jobs.is_empty() {
            return Ok(0);
        }

        println!(
            "SideEffectCompensator: draining {} pending side-effect job(s)",
            jobs.len()
        );
        let mut done = 0usize;
        let mut failures: Vec<String> = Vec::new();

        for job in jobs {
            let kind = match job.kind.as_str() {
                "syst_derived" => SideEffectKind::SystDerived,
                "ref_rev_maintain" => SideEffectKind::RefRevMaintain,
                "cata_ref_rev" => SideEffectKind::CataRefRev,
                other => {
                    let reason = format!("unsupported legacy side-effect kind: {other}");
                    if let Err(error) = Self::mark_abandoned(&job.id, &reason).await {
                        failures.push(format!("abandon {} failed: {error:#}", job.id));
                    } else {
                        println!("SideEffectCompensator: abandoned {} ({other})", job.id);
                    }
                    continue;
                }
            };

            let result = match kind {
                SideEffectKind::RefRevMaintain => {
                    match parse_ref_rev_payload(&job.changed_refnos) {
                        Ok(referrers) => {
                            crate::data_interface::manual_update::repair_reverse_index_for(
                                &referrers,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    }
                }
                // 连不上 AiosDBMgr 是这个作业自己的失败，不是整轮的失败：
                // 这里必须留在 async 块里，`?` 一旦逃出去就会掐掉后面所有作业。
                SideEffectKind::SystDerived => {
                    async {
                        let aios_mgr =
                            aios_core::aios_db_mgr::aios_mgr::AiosDBMgr::init_from_db_option()
                                .await?;
                        crate::team_data::sync_team_data(&aios_mgr).await
                    }
                    .await
                }
                SideEffectKind::SpatialReconcile => {
                    unreachable!("spatial jobs are drained before dequeue")
                }
                // 已退役（ADR-064 Q2）：旧库里排进表的行只收口，不再解析、不再写——它要落的
                // 目录行早已由 `persist_cata_dependencies` 在那个窗口的水位之前落下。
                SideEffectKind::CataRefRev => {
                    println!(
                        "SideEffectCompensator: cata_ref_rev 已退役（ADR-064 Q2），dbnum={} sesno={} 直接收口",
                        job.dbnum, job.end_sesno
                    );
                    Ok(())
                }
            };

            let outcome = match result {
                Ok(()) => Self::mark_done(kind, job.dbnum, job.end_sesno).await,
                Err(error) => Err(error),
            };

            match outcome {
                Ok(()) => {
                    done += 1;
                    println!(
                        "SideEffectCompensator: done {:?} dbnum={} sesno={}",
                        kind, job.dbnum, job.end_sesno
                    );
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    println!(
                        "SideEffectCompensator: retry failed {:?} dbnum={}: {msg}",
                        kind, job.dbnum
                    );
                    let _ = Self::mark_failed(kind, job.dbnum, job.end_sesno, &msg).await;
                    failures.push(format!("{:?} dbnum={}: {msg}", kind, job.dbnum));
                }
            }
        }

        if !failures.is_empty() {
            anyhow::bail!(
                "{} side-effect job(s) failed after {done} completed: {}",
                failures.len(),
                failures.join("; ")
            );
        }
        Ok(done)
    }

    /// 人工复活 SystDerived / RefRevMaintain 死信（attempts 到顶被 [`Self::drain`]
    /// 的上限挡在候选集之外）——队列三出路（ADR-011 队列纪律）里的「可复活」那一条。
    ///
    /// spatial 无视上限、永远不会死信，故不在此复活。复活 = attempts 清零 + 清
    /// last_error + 回到 `pending`，下一轮 drain 重新取到；**唤醒 worker 由调用方负责**
    /// （HTTP 端点里 `BatchScheduler::wake()`——复活绕过入队通道，worker 的 Notify
    /// 没人碰过，不叫醒它这些行要等兜底轮询）。返回复活的行数。
    pub async fn revive_dead_letters() -> anyhow::Result<usize> {
        let mut response = SUL_DB
            .query(Self::render_revive_dead_letters())
            .await
            .map_err(|e| anyhow::anyhow!("revive side-effect dead letters failed: {e}"))?
            .check()
            .map_err(|e| {
                anyhow::anyhow!("revive side-effect dead letters statement failed: {e}")
            })?;
        let revived: Vec<PendingJob> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("decode revived side-effect dead letters failed: {e}"))?;
        Ok(revived.len())
    }

    /// 复活死信的 UPDATE（纯渲染）：只碰**可 drain 的两类**到顶死信（attempts >=
    /// [`MAX_ATTEMPTS`] 且 status ∈ pending/failed），spatial 不动（它无视上限、
    /// 自有重放语义）。
    fn render_revive_dead_letters() -> String {
        format!(
            "UPDATE {TABLE} SET attempts = 0, last_error = NONE, status = 'pending', \
             updated_at = time::now() WHERE kind != 'spatial_reconcile' \
             AND status IN ['pending', 'failed'] AND (attempts?:0) >= {MAX_ATTEMPTS} \
             RETURN AFTER;"
        )
    }

    /// 清扫已完成的非空间副作用行——队列三出路里的「可收口」终态清理。
    ///
    /// SystDerived / RefRevMaintain 成功后 [`Self::mark_done`] 置 `status = 'done'`
    /// 却从不删行，日积月累。由空闲轮每轮调一次（幂等：删完再删是空操作）。
    /// spatial 的 done 行**不在此清扫**——它的重放/销账语义（崩溃窗口 ②/④，pending
    /// 在场即 ReplayRequired）自成一套，误删会掩盖重启该重放的意图。返回删除行数。
    pub async fn sweep_done() -> anyhow::Result<usize> {
        Self::sweep_done_on(&SUL_DB).await
    }

    async fn sweep_done_on(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> anyhow::Result<usize> {
        let mut response = db
            .query(Self::render_sweep_done())
            .await
            .map_err(|e| anyhow::anyhow!("sweep done side-effect rows failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("sweep done side-effect rows statement failed: {e}"))?;
        let count: Option<usize> = response
            .take(2)
            .map_err(|e| anyhow::anyhow!("decode swept side-effect row count failed: {e}"))?;
        Ok(count.unwrap_or(0))
    }

    /// 清扫 done 行的 DELETE（纯渲染）：只让数据库返回标量计数，不反序列化 schemaless 行。
    fn render_sweep_done() -> String {
        format!(
            "LET $done = SELECT VALUE id FROM {TABLE} WHERE kind != 'spatial_reconcile' AND status = 'done';\
             DELETE $done RETURN NONE;\
             RETURN count($done);"
        )
    }

    // `complete_syst_jobs` / `fail_syst_jobs` 随 `execute_incr_update` 退役：
    // 合流后 SYST 派生只走本补偿队列（enqueue_syst → drain 逐作业 mark_done /
    // mark_failed），不再有「先同步跑一遍、成了再回头销行」的旁路。
}

#[cfg(test)]
mod tests {
    use super::{
        PendingJob, PublishedFileInput, SideEffectCompensator, SideEffectKind,
        parse_ref_rev_payload,
    };
    use crate::data_interface::table_parity::{fresh_mem_db, init_schema_on};

    fn manifest_job(dbnum: u32, end_sesno: i32, token: &str) -> PendingJob {
        PendingJob {
            id: surrealdb::sql::Thing::from((
                "incr_side_effect_pending",
                dbnum.to_string().as_str(),
            )),
            kind: "spatial_reconcile".into(),
            dbnum,
            end_sesno,
            db_type: "DESI".into(),
            changed_refnos: vec![],
            refresh_refnos: vec![],
            remove_refnos: vec![],
            room_clear_refnos: vec![],
            model_publish_roots: vec![],
            deferred_model_actions: vec![],
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            commit_token: Some(token.into()),
            room_released: false,
            published_version: 0,
        }
    }

    #[test]
    fn manifest_stamp_is_stable_across_partial_publication_acknowledgement() {
        let old = PublishedFileInput {
            dbnum: 8000,
            db_type: "DESI".into(),
            end_sesno: 10,
            commit_token: "a-old".into(),
            published_version: 4,
        };
        let jobs = [
            manifest_job(8000, 20, "a-new"),
            manifest_job(8001, 30, "b-new"),
        ];
        let before_ack =
            SideEffectCompensator::apply_spatial_publications_to_manifest(vec![old], 4, &jobs);

        // Simulate the first mark_done transaction committing before a crash.
        let a_committed = PublishedFileInput {
            dbnum: 8000,
            db_type: "DESI".into(),
            end_sesno: 20,
            commit_token: "a-new".into(),
            published_version: 5,
        };
        let after_partial_ack = SideEffectCompensator::apply_spatial_publications_to_manifest(
            vec![a_committed],
            5,
            &jobs[1..],
        );

        assert_eq!(before_ack, after_partial_ack);
        assert_eq!(before_ack.published_version, 6);
        assert_eq!(before_ack.file_count, 2);
        assert_eq!(before_ack.sha256.len(), 64);
    }

    #[test]
    fn empty_published_manifest_baseline_captures_every_design_file() {
        let source = include_str!("side_effect_pending.rs");
        let body = source
            .split_once("pub(crate) async fn ensure_published_file_baseline(")
            .expect("baseline function exists")
            .1
            .split_once("pub(crate) async fn published_file_inputs(")
            .expect("published input reader follows")
            .0;
        assert!(body.contains("for (dbnum, path, sesno) in sources"));
        assert!(
            !body.contains("include_other_files"),
            "清单为空就是升级边界，不能按初始化 epoch 漏掉其他文件: {body}"
        );
        assert!(
            !body.contains("dbnum != active_dbnum"),
            "首次基线必须冻结所有已有设计文件: {body}"
        );
    }

    #[tokio::test]
    async fn spatial_completion_publishes_the_file_token_in_the_same_transaction() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("published_file_input")
            .await
            .unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_8000_26 SET \
             kind = 'spatial_reconcile', dbnum = 8000, end_sesno = 26, \
             db_type = 'DESI', commit_token = 'frozen-token', status = 'pending';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(SideEffectCompensator::render_mark_done(
            SideEffectKind::SpatialReconcile,
            8000,
            26,
        ))
        .await
        .unwrap()
        .check()
        .expect("publication transaction is valid SurrealQL");

        #[derive(serde::Deserialize)]
        struct FileRow {
            commit_token: String,
            published_version: u64,
        }
        let mut response = db
            .query("SELECT commit_token, published_version FROM published_file_input:8000;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let row = response
            .take::<Option<FileRow>>(0)
            .unwrap()
            .expect("published file row");
        assert_eq!(row.commit_token, "frozen-token");
        assert_eq!(row.published_version, 1);
    }

    #[tokio::test]
    async fn failed_a_and_completed_b_leave_a_old_plus_b_new_in_the_manifest() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("cross_file_publish")
            .await
            .unwrap();
        db.query(
            "CREATE published_file_input:8000 SET dbnum = 8000, db_type = 'DESI', \
               end_sesno = 10, commit_token = 'a-old', published_version = 1; \
             CREATE incr_side_effect_pending:spatial_reconcile_8000_20 SET \
               kind = 'spatial_reconcile', dbnum = 8000, end_sesno = 20, \
               db_type = 'DESI', commit_token = 'a-new', status = 'collecting'; \
             CREATE incr_side_effect_pending:spatial_reconcile_8001_30 SET \
               kind = 'spatial_reconcile', dbnum = 8001, end_sesno = 30, \
               db_type = 'DESI', commit_token = 'b-new', status = 'pending';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(SideEffectCompensator::render_mark_done(
            SideEffectKind::SpatialReconcile,
            8001,
            30,
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        #[derive(serde::Deserialize)]
        struct FileRow {
            dbnum: u32,
            commit_token: String,
        }
        let mut response = db
            .query(
                "SELECT dbnum, commit_token FROM published_file_input \
                 WHERE dbnum IN [8000, 8001] ORDER BY dbnum ASC;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows = response.take::<Vec<FileRow>>(0).unwrap();
        assert_eq!(
            rows.into_iter()
                .map(|row| (row.dbnum, row.commit_token))
                .collect::<Vec<_>>(),
            vec![(8000, "a-old".into()), (8001, "b-new".into())]
        );
    }

    #[tokio::test]
    async fn replacing_an_invalid_source_retires_and_clears_the_old_candidate() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("replace_file_round")
            .await
            .unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_8000_20 SET \
               kind = 'spatial_reconcile', dbnum = 8000, end_sesno = 20, \
               db_type = 'DESI', commit_token = 'old-token', status = 'collecting', \
               refresh_refnos = ['1/2'], room_clear_refnos = ['1/3'], \
               model_publish_roots = ['1/1'];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db.query(SideEffectCompensator::render_replace_collecting_file_round(
            8000,
            20,
            30,
            "DESI",
            "new-token",
            "anchor changed",
        ))
        .await
        .unwrap()
        .check()
        .expect("replacement transaction is valid SurrealQL");

        #[derive(serde::Deserialize)]
        struct RoundRow {
            end_sesno: i32,
            status: String,
            commit_token: String,
            #[serde(default)]
            refresh_refnos: Vec<String>,
            #[serde(default)]
            model_publish_roots: Vec<String>,
        }
        let mut response = db
            .query(
                "SELECT end_sesno, status, commit_token, refresh_refnos, model_publish_roots \
                 FROM [incr_side_effect_pending:spatial_reconcile_8000_20, \
                       incr_side_effect_pending:spatial_reconcile_8000_30] \
                 ORDER BY end_sesno ASC;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows = response.take::<Vec<RoundRow>>(0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].end_sesno, rows[1].end_sesno), (20, 30));
        assert_eq!(rows[0].status, "superseded");
        assert_eq!(rows[0].commit_token, "old-token");
        assert_eq!(rows[1].status, "collecting");
        assert_eq!(rows[1].commit_token, "new-token");
        assert!(rows[1].refresh_refnos.is_empty());
        assert!(rows[1].model_publish_roots.is_empty());
    }

    #[test]
    fn ref_rev_payload_is_nonempty_and_all_or_nothing() {
        assert!(parse_ref_rev_payload(&[]).is_err());
        assert!(parse_ref_rev_payload(&["".into()]).is_err());
        assert!(parse_ref_rev_payload(&["24381/1".into(), "broken".into()]).is_err());
        assert_eq!(
            parse_ref_rev_payload(&["24381/1".into(), "24381/2".into()])
                .expect("valid payload")
                .len(),
            2
        );
    }

    /// SYST 派生入队：行 id 只由 `(kind, dbnum, end_sesno)` 决定，重复入队保住
    /// 已有 attempts，一个窗口右端一行。
    ///
    /// 这三条各自挡着一种失效。id 不稳定，两个入队点（直写路径与暂存提交尾）就
    /// 记成两行、TEAM 同步白跑一遍；attempts 被清零，一条永远失败的派生作业每来
    /// 一个批次复活一次，`MAX_ATTEMPTS` 与 /health 的死信计数一起失效；右端不进
    /// id，一次初始化里的多个 SYST 窗口会互相覆盖，只剩最后一个留下账。
    #[test]
    fn syst_enqueue_is_idempotent_per_window_and_keeps_its_retry_budget() {
        let (id, sql) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::SystDerived,
            8191,
            169,
            "SYST",
            &[],
        );
        assert_eq!(id, "incr_side_effect_pending:syst_derived_8191_169");
        assert!(sql.starts_with(&format!("UPSERT {id} SET")), "{sql}");
        assert!(
            sql.contains("attempts = attempts?:0"),
            "重复入队必须保住重试计数: {sql}"
        );
        assert!(sql.contains("status = 'pending'"), "{sql}");
        assert!(
            sql.contains("changed_refnos = []"),
            "SYST 派生按批次记账，不带引用者名单: {sql}"
        );

        let (next_window, _) = SideEffectCompensator::render_upsert_pending(
            SideEffectKind::SystDerived,
            8191,
            170,
            "SYST",
            &[],
        );
        assert_ne!(id, next_window, "换一个窗口右端就是另一行");
    }

    #[test]
    fn spatial_reconcile_row_is_deterministic_and_keeps_final_net_mutation() {
        let sql = SideEffectCompensator::render_spatial_reconcile_upsert(
            24381,
            42,
            &["16777216/1".to_string(), "16777216/2".to_string()],
            &["16777216/3".to_string()],
        )
        .expect("spatial reconcile SQL");

        assert!(sql.contains("incr_side_effect_pending:spatial_reconcile_24381_42"));
        assert!(sql.contains("kind = 'spatial_reconcile'"));
        assert!(sql.contains("refresh_refnos = [\"16777216/1\",\"16777216/2\"]"));
        assert!(sql.contains("remove_refnos = [\"16777216/3\"]"));
        assert_eq!(
            SideEffectKind::SpatialReconcile.as_str(),
            "spatial_reconcile"
        );
    }

    /// ADR-064 Q2（2026-09-07）：`cata_ref_rev` 补偿已退役——数据窗口不再入队（目录链依赖由
    /// `persist_cata_dependencies` 排在水位之前落行），drain 对旧库里已经排进表的行**只收口**：
    /// 仍在候选集里（不被当成 unsupported legacy kind 抛弃、也不被 `kind != …` 滤掉）、有自己的臂、
    /// 臂里不再解析闭包、不再写目录行。
    #[test]
    fn cata_ref_rev_is_retired_and_legacy_rows_are_only_settled() {
        let source = include_str!("side_effect_pending.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        assert!(
            !production.contains("fn enqueue_cata_ref_rev("),
            "补偿入队口已退役，不得复活"
        );
        let drain = production
            .split_once("pub async fn drain(")
            .expect("drain must exist")
            .1
            .split_once("pub async fn revive_dead_letters(")
            .expect("revive must follow drain")
            .0;
        assert!(drain.contains("kind != 'spatial_reconcile'"), "{drain}");
        assert!(!drain.contains("kind != 'cata_ref_rev'"), "{drain}");
        assert!(
            drain.contains("\"cata_ref_rev\" => SideEffectKind::CataRefRev"),
            "旧行仍要认得，才能收口而不是 abandoned"
        );
        let arm = drain
            .split_once("SideEffectKind::CataRefRev =>")
            .expect("cata_ref_rev 要有自己的臂")
            .1
            .split_once("};")
            .expect("arm boundary")
            .0;
        assert!(
            !arm.contains("preload_cata_for_roots") && !arm.contains("ensure_cata_refnos_parsed"),
            "退役后的臂不得再解析闭包 / 写目录行: {arm}"
        );
        assert!(arm.contains("Ok(())"), "旧行按成功收口: {arm}");

        let pipeline = include_str!("increment_pipeline.rs")
            .split_once("#[cfg(test)]")
            .expect("pipeline test boundary")
            .0;
        assert!(
            !pipeline.contains("enqueue_cata_ref_rev("),
            "数据窗口收口后不得再登记 CATA 落库补偿"
        );

        let status = SideEffectCompensator::render_side_effect_status(&[]);
        assert!(
            status["by_kind"].get("cata_ref_rev").is_none(),
            "/health 不再宣告已退役的种类：{status}"
        );
    }

    #[test]
    fn spatial_reconcile_rejects_conflicting_refno() {
        let duplicate = "16777216/1".to_string();
        let error = SideEffectCompensator::render_spatial_reconcile_upsert(
            24381,
            42,
            std::slice::from_ref(&duplicate),
            std::slice::from_ref(&duplicate),
        )
        .expect_err("same refno cannot be refreshed and removed");
        assert!(error.to_string().contains("同时 refresh/remove"));
    }

    /// /health `spatial_reconcile` 的四键契约（台账缺口 G-02，W1.4 验收）。
    ///
    /// 成功与读库降级两个分支必须同键；`stalled` 只在出现死信（`dead` / 老版本的
    /// `failed`）时立起——空间收敛不重试，一次失败即死信；
    /// `last_error` 取最近一条（查询按 updated_at DESC 供货，渲染取第一个非空）。
    #[test]
    fn spatial_reconcile_status_keeps_its_four_key_shape_in_both_branches() {
        use super::{PendingJob, TABLE};

        let keys = [
            "pending",
            "dead_letters",
            "retries",
            "last_error",
            "stalled",
            "file_rounds",
            "phase",
            "target_sessions",
            "published_version",
        ];
        let no_holds = std::collections::BTreeMap::new();
        let empty = SideEffectCompensator::render_spatial_reconcile_status(&[], &no_holds);
        let object = empty.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {empty}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {empty}");
        }
        assert_eq!(empty["pending"], 0);
        assert_eq!(empty["dead_letters"], 0);
        assert_eq!(empty["retries"], 0);
        assert_eq!(empty["last_error"], serde_json::Value::Null);
        assert_eq!(empty["stalled"], false);

        let job = |status: &str, attempts: u32, last_error: Option<&str>| PendingJob {
            id: surrealdb::sql::Thing::from((TABLE, "spatial_reconcile_8000_26")),
            room_clear_refnos: vec![],
            model_publish_roots: vec![],
            deferred_model_actions: vec![],
            kind: "spatial_reconcile".into(),
            dbnum: 8000,
            end_sesno: 26,
            db_type: "DESI".into(),
            changed_refnos: vec![],
            refresh_refnos: vec![],
            remove_refnos: vec![],
            status: status.into(),
            attempts,
            last_error: last_error.map(str::to_owned),
            commit_token: None,
            room_released: false,
            published_version: 0,
        };
        // 一条 `dead`（或老版本留下的 `failed`）就是死信：`stalled` 立起、不算进
        // pending；原因仍从 last_error 露出来。
        let stalled = SideEffectCompensator::render_spatial_reconcile_status(
            &[
                job("dead", 1, Some("current MDB has no DESI dbnum 8190")),
                job("failed", 3, None),
                job("pending", 0, None),
            ],
            &no_holds,
        );
        assert_eq!(stalled["pending"], 1);
        assert_eq!(stalled["dead_letters"], 2);
        assert_eq!(stalled["retries"], 4);
        assert_eq!(stalled["last_error"], "current MDB has no DESI dbnum 8190");
        assert_eq!(stalled["stalled"], true, "有死信必须报 stalled: {stalled}");

        let waiting = SideEffectCompensator::render_spatial_reconcile_status(
            &[job("pending", 0, None)],
            &no_holds,
        );
        assert_eq!(waiting["pending"], 1);
        assert_eq!(waiting["dead_letters"], 0);
        assert_eq!(
            waiting["stalled"], false,
            "只有待收敛、没有死信不算 stalled: {waiting}"
        );

        let degraded =
            SideEffectCompensator::spatial_reconcile_error_status(&anyhow::anyhow!("boom"));
        let degraded_object = degraded.as_object().expect("降级形状必须是对象");
        assert_eq!(
            degraded_object.len(),
            keys.len(),
            "降级分支不许缩键: {degraded}"
        );
        for key in keys {
            assert!(degraded_object.contains_key(key), "缺键 {key}: {degraded}");
        }
        assert_eq!(degraded["stalled"], true);
        assert!(
            degraded["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("boom")
        );
    }

    /// 文件发布即使 AABB 未变也要落快照，因为 V3 头还承载发布版本与冻结输入摘要；
    /// 全程持空间串行锁，且不得退回绕过发布证明的旧入口。
    ///
    /// 无条件 `persist_aabb_tree()` 的销毁性边界：树加载失败以空树启动的进程里，
    /// 删除清理照样寄存 remove 意图、尾事务照写 spatial 行——收敛轮对空树摘除
    /// 零条之后，会把**空树**序列化覆盖 `accel_tree_{project}.bin`，销毁上一次
    /// 攒下的全量成果。文件发布专用入口的快照发布门挡住这条路。真实落盘
    /// 写 cwd 文件，单测不能实跑，只能钉源码。
    #[test]
    fn reconcile_persists_the_file_publication_manifest() {
        let source = include_str!("side_effect_pending.rs");
        let wrapper = source
            .split_once("pub async fn reconcile_spatial_pending(")
            .expect("reconcile_spatial_pending must exist")
            .1
            .split_once("pub(crate) async fn reconcile_spatial_pending_locked(")
            .expect("locked variant must follow")
            .0;
        assert!(
            wrapper.contains("lock_spatial_serial().await"),
            "公开收敛入口必须先取空间串行锁: {wrapper}"
        );
        let body = source
            .split_once("pub(crate) async fn reconcile_spatial_pending_locked(")
            .expect("reconcile body must exist")
            .1
            .split_once("pub async fn has_pending_spatial_work(")
            .expect("reconcile 之后是 pending 探针")
            .0;
        assert!(
            body.contains("persist_aabb_tree_if_dirty_locked_for_publication(&jobs)"),
            "收敛必须把同一批文件任务写入快照发布证明: {body}"
        );
        let topology_at = body
            .find("ensure_published_room_topology")
            .expect("空间清单切换前必须准备完整房间拓扑");
        let snapshot_at = body
            .find("persist_aabb_tree_if_dirty_locked_for_publication(&jobs)")
            .expect("必须发布空间快照");
        assert!(
            topology_at < snapshot_at,
            "拓扑候选必须在同一发布锁内先于空间快照准备完成: {body}"
        );
        assert!(
            !body.contains(concat!("persist_aabb_tree", "()")),
            "无条件全量落盘会让空树覆盖项目树文件: {body}"
        );
        // 状态晋升必须发生在销账（mark_done）之后：pending 还没销就把消费者放行，
        // 门禁窗口内的房间轮会拿「已知陈旧」的树改写归属。
        let done_at = body.rfind("mark_done").expect("必须销账");
        let promote_at = body
            .rfind("promote_state_after_replay()")
            .expect("成功路径必须晋升状态");
        assert!(done_at < promote_at, "状态晋升必须在销账之后: {body}");
    }

    /// P2-4：/health `side_effect_pending` 必须报出 SystDerived / RefRevMaintain 的
    /// 死信计数，且成功与读库降级两个分支同键。
    ///
    /// 此前 /health 只报 spatial 四键，这两类走 drain 的副作用一旦 attempts 到顶就
    /// 被 drain 上限挡在候选集外成死信，接口上无处可见。这条钉住新增的可观测口径：
    /// 回退（删 dead_letters/by_kind 键、或把 spatial 混进来）即红。
    #[test]
    fn side_effect_status_exposes_dead_letters_and_keeps_its_shape() {
        use super::{MAX_ATTEMPTS, PendingJob, TABLE};

        let keys = [
            "pending",
            "dead_letters",
            "retries",
            "by_kind",
            "last_error",
            "stalled",
        ];
        let empty = SideEffectCompensator::render_side_effect_status(&[]);
        let object = empty.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {empty}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {empty}");
        }
        assert_eq!(empty["dead_letters"], 0);
        assert_eq!(empty["stalled"], false);

        let job = |kind: &str, attempts: u32, last_error: Option<&str>| PendingJob {
            id: surrealdb::sql::Thing::from((TABLE, format!("{kind}_8000_26").as_str())),
            room_clear_refnos: vec![],
            model_publish_roots: vec![],
            deferred_model_actions: vec![],
            kind: kind.into(),
            dbnum: 8000,
            end_sesno: 26,
            db_type: "SYST".into(),
            changed_refnos: vec![],
            refresh_refnos: vec![],
            remove_refnos: vec![],
            status: "failed".into(),
            attempts,
            last_error: last_error.map(str::to_owned),
            commit_token: None,
            room_released: false,
            published_version: 0,
        };
        // syst_derived 到顶 = 死信；ref_rev_maintain 还在预算内 = pending。
        let status = SideEffectCompensator::render_side_effect_status(&[
            job("syst_derived", MAX_ATTEMPTS, Some("SYST 同步失败")),
            job("ref_rev_maintain", 2, None),
        ]);
        assert_eq!(
            status["dead_letters"], 1,
            "到顶的 syst_derived 必须计死信: {status}"
        );
        assert_eq!(status["pending"], 1, "预算内的行仍是 pending: {status}");
        assert_eq!(status["retries"], u64::from(MAX_ATTEMPTS + 2));
        assert_eq!(status["stalled"], true, "有死信必须 stalled: {status}");
        assert_eq!(status["last_error"], "SYST 同步失败");
        assert_eq!(status["by_kind"]["syst_derived"]["dead_letters"], 1);
        assert_eq!(status["by_kind"]["syst_derived"]["pending"], 0);
        assert_eq!(status["by_kind"]["ref_rev_maintain"]["dead_letters"], 0);
        assert_eq!(status["by_kind"]["ref_rev_maintain"]["pending"], 1);

        // 读库降级分支必须同键，stalled 保守置真。
        let degraded = SideEffectCompensator::side_effect_error_status(&anyhow::anyhow!("boom"));
        let degraded_object = degraded.as_object().expect("降级形状必须是对象");
        assert_eq!(
            degraded_object.len(),
            keys.len(),
            "降级分支不许缩键: {degraded}"
        );
        for key in keys {
            assert!(degraded_object.contains_key(key), "缺键 {key}: {degraded}");
        }
        assert_eq!(degraded["stalled"], true);
        assert!(
            degraded["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("boom")
        );
    }

    #[test]
    fn side_effect_dead_letters_remain_unsettled_for_initialization() {
        let source = include_str!("side_effect_pending.rs");
        let body = source
            .split_once("pub async fn has_dead_work()")
            .expect("dead-work probe exists")
            .1
            .split_once("/// /health")
            .expect("dead-work probe end exists")
            .0;
        assert!(body.contains("kind != 'spatial_reconcile'"));
        assert!(body.contains("(attempts?:0) >= {MAX_ATTEMPTS}"));
    }

    /// P2-4「可复活」：复活只碰**可 drain 两类**的到顶死信，把 attempts 清零回到
    /// pending，并放过 spatial（它无视上限、自有重放语义）。
    #[test]
    fn reviving_dead_letters_resets_attempts_and_spares_spatial() {
        use super::MAX_ATTEMPTS;
        let sql = SideEffectCompensator::render_revive_dead_letters();
        assert!(sql.contains("attempts = 0"), "复活必须清零 attempts: {sql}");
        assert!(
            sql.contains("status = 'pending'"),
            "复活必须回到 pending: {sql}"
        );
        assert!(
            sql.contains("last_error = NONE"),
            "复活必须清 last_error: {sql}"
        );
        assert!(
            sql.contains("kind != 'spatial_reconcile'"),
            "spatial 不走上限、不在复活范围: {sql}"
        );
        assert!(
            sql.contains(&format!("(attempts?:0) >= {MAX_ATTEMPTS}")),
            "只复活到顶死信，不误动仍在重试预算内的行: {sql}"
        );
    }

    /// P2-4「可收口」：done 行清扫只删非空间的终态行，且幂等。
    ///
    /// spatial 的 done 行不在此清扫——pending 在场是它崩溃重放的信号，混进来会误删。
    #[test]
    fn sweeping_done_removes_only_terminal_non_spatial_rows() {
        let sql = SideEffectCompensator::render_sweep_done();
        assert!(
            sql.contains("DELETE $done RETURN NONE"),
            "必须是删除语句: {sql}"
        );
        assert!(sql.contains("RETURN count($done)"), "只返回标量计数: {sql}");
        assert!(sql.contains("status = 'done'"), "只删终态行: {sql}");
        assert!(
            sql.contains("kind != 'spatial_reconcile'"),
            "空间收敛的 done 行有自己的重放/销账语义，不在此清扫: {sql}"
        );
    }

    #[tokio::test]
    async fn sweep_done_counts_without_decoding_schemaless_rows() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("sweep_done_count").await.unwrap();
        db.query(
            "CREATE incr_side_effect_pending:one SET kind='syst_derived', status='done', arbitrary=type::thing('pe','1_2');\
             CREATE incr_side_effect_pending:two SET kind='ref_rev_maintain', status='done', arbitrary=time::now();\
             CREATE incr_side_effect_pending:keep SET kind='spatial_reconcile', status='done';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert_eq!(SideEffectCompensator::sweep_done_on(&db).await.unwrap(), 2);
        #[derive(serde::Deserialize)]
        struct CountRow {
            count: usize,
        }
        let mut response = db
            .query("SELECT count() AS count FROM incr_side_effect_pending GROUP ALL;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            response.take::<Option<CountRow>>(0).unwrap().unwrap().count,
            1
        );
    }

    #[tokio::test]
    async fn file_round_proof_requires_spatial_done_and_no_associated_room_work() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test").use_db("file_round_proof").await.unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_8000_42 SET kind='spatial_reconcile', \
             dbnum=8000, end_sesno=42, status='done', room_released=false;\
             CREATE model_update_pending:room_recalc_element_1_2 SET action='room_recalc_element', \
             target_refno='1/2', status='pending', file_rounds=['8000:42'];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert!(
            SideEffectCompensator::mark_file_rooms_released_on(&db, 8000, 42)
                .await
                .is_err()
        );
        db.query("DELETE model_update_pending:room_recalc_element_1_2;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let proof = SideEffectCompensator::mark_file_rooms_released_on(&db, 8000, 42)
            .await
            .unwrap();
        assert_eq!((proof.dbnum(), proof.end_sesno()), (8000, 42));
        assert_eq!(
            SideEffectCompensator::completed_file_round_on(&db, 8000, 42)
                .await
                .unwrap(),
            proof
        );
    }

    #[tokio::test]
    async fn project_room_rebuild_owns_legacy_room_work_after_spatial_done() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("test")
            .use_db("project_room_rebuild_proof")
            .await
            .unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_7997_155 SET kind='spatial_reconcile', \
             dbnum=7997, end_sesno=155, status='done', room_released=false;\
             CREATE model_update_pending:legacy SET action='room_recalc_element', \
             target_refno='1/2', status='pending', file_rounds=['7997:155'];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let proof =
            SideEffectCompensator::mark_file_rooms_released_for_project_rebuild_on(&db, 7997, 155)
                .await
                .unwrap();
        assert_eq!((proof.dbnum(), proof.end_sesno()), (7997, 155));
        assert_eq!(
            SideEffectCompensator::completed_file_round_on(&db, 7997, 155)
                .await
                .unwrap(),
            proof
        );
    }

    #[tokio::test]
    async fn recovery_token_must_match_the_task_right_edge() {
        let db = fresh_mem_db("side_effect_pending", "exact_recovery_round")
            .await
            .unwrap();
        init_schema_on(&db).await.unwrap();
        db.query(
            "CREATE incr_side_effect_pending:spatial_reconcile_8000_434 SET \
                 kind='spatial_reconcile', dbnum=8000, end_sesno=434, \
                 status='collecting', commit_token='old'; \
             CREATE incr_side_effect_pending:spatial_reconcile_8000_438 SET \
                 kind='spatial_reconcile', dbnum=8000, end_sesno=438, \
                 status='collecting', commit_token='current';",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        assert_eq!(
            SideEffectCompensator::recover_collecting_file_round_on(&db, 8000, 438)
                .await
                .unwrap(),
            Some((438, "current".to_owned()))
        );
        assert_eq!(
            SideEffectCompensator::recover_collecting_file_round_on(&db, 8000, 440)
                .await
                .unwrap(),
            None
        );
    }
}
