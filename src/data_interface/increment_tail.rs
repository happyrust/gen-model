//! 数据窗口的崩溃恢复记录（`increment_update_attempt`）与收口尾事务。
//!
//! 与隔壁 [`model_update_pending`](crate::data_interface::model_update_pending) 同住过
//! 一个文件，命却不同：那张表是模型工作队列，ADR-065 D3 判它**整体退役**；这里的两样
//! 东西属**数据面**，同一条 D3 点名**保留**——`increment_update_attempt` 是固定区间重放
//! 的恢复记录，收口尾事务是「数据 + 水位（+ 未触根凭证前移）」那一笔原子提交，两者都与
//! 模型欠账无关。
//!
//! 搬出来是计划 §4 **S5a**（`docs/plans/2026-09-08-external-increment-refactor-plan.md`，
//! 缺口 G1）：不先搬，T5 的「删 `model_update_pending` 写读两侧」要么删不动、要么把水位
//! 一起删了。**名字与行为不变**，只换住址：调用点与 `increment_pipeline` 那几条源码钉
//! 现在一律写 `increment_tail::finalize_attempt`，`model_update_pending` 不再再导出本模块
//! ——「退表」与「退模块」从此是两件分得开的事。
//!
//! 尾事务渲染仍要回头调 `model_update_pending` 的 [`render_upsert`] / [`source_time_for`] /
//! [`render_delete_revision`]：模型意图这会儿还落成 pending 行，那是 S6 / S7 掐掉写入点
//! 之后才会消失的依赖，不是本次搬家要解决的。

use aios_core::SUL_DB;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_pending::{
    render_delete_revision, render_upsert, source_time_for,
};
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction};
use crate::data_interface::window_root_plan::CredentialAdvanceOutcome;

pub const ATTEMPT_TABLE: &str = "increment_update_attempt";

/// Short-lived recovery record written before any PE mutation. A retry reuses
/// this exact range and pre-update model plan instead of recomputing ownership
/// from a possibly partially-applied database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncrementUpdateAttempt {
    pub dbnum: u32,
    pub db_type: String,
    pub file_path: String,
    pub start_sesno: i32,
    pub end_sesno: i32,
    pub plan: ModelUpdatePlan,
    #[serde(default)]
    pub commit_token: Option<String>,
    #[serde(default = "prepared_attempt_status")]
    pub status: String,
}

fn prepared_attempt_status() -> String {
    "prepared".to_string()
}

#[derive(Debug, Deserialize)]
struct AttemptRow {
    dbnum: u32,
    db_type: String,
    file_path: String,
    start_sesno: i32,
    end_sesno: i32,
    plan_json: String,
    #[serde(default)]
    commit_token: Option<String>,
    #[serde(default = "prepared_attempt_status")]
    status: String,
}

pub async fn load_attempt(dbnum: u32) -> anyhow::Result<Option<IncrementUpdateAttempt>> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT dbnum, db_type, file_path, start_sesno, end_sesno, plan_json, \
                    commit_token, status \
             FROM {ATTEMPT_TABLE}:{dbnum};"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("load increment attempt dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("load increment attempt dbnum={dbnum} statement failed: {error}")
        })?;
    let rows: Vec<AttemptRow> = response.take(0).map_err(|error| {
        anyhow::anyhow!("decode increment attempt dbnum={dbnum} failed: {error}")
    })?;
    rows.into_iter()
        .next()
        .map(|row| {
            let plan = serde_json::from_str(&row.plan_json).map_err(|error| {
                anyhow::anyhow!("decode increment attempt plan dbnum={dbnum} failed: {error}")
            })?;
            Ok(IncrementUpdateAttempt {
                dbnum: row.dbnum,
                db_type: row.db_type,
                file_path: row.file_path,
                start_sesno: row.start_sesno,
                end_sesno: row.end_sesno,
                plan,
                commit_token: row.commit_token,
                status: row.status,
            })
        })
        .transpose()
}

pub async fn prepare_attempt(attempt: &IncrementUpdateAttempt) -> anyhow::Result<()> {
    anyhow::ensure!(
        attempt
            .commit_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty()),
        "increment attempt dbnum={} requires a durable source commit token",
        attempt.dbnum
    );
    let plan_json = escape_surql_str(&serde_json::to_string(&attempt.plan)?);
    let db_type = escape_surql_str(&attempt.db_type);
    let file_path = escape_surql_str(&attempt.file_path);
    let commit_token = attempt
        .commit_token
        .as_deref()
        .map(escape_surql_str)
        .map(|value| format!("'{value}'"))
        .unwrap_or_else(|| "NONE".to_string());
    let status = escape_surql_str(&attempt.status);
    let sql = format!(
        "UPSERT {ATTEMPT_TABLE}:{dbnum} SET dbnum = {dbnum}, \
         db_type = '{db_type}', file_path = '{file_path}', \
         start_sesno = {start_sesno}, end_sesno = {end_sesno}, \
         plan_json = '{plan_json}', commit_token = {commit_token}, status = '{status}', \
         created_at = created_at?:time::now(), updated_at = time::now();",
        dbnum = attempt.dbnum,
        start_sesno = attempt.start_sesno,
        end_sesno = attempt.end_sesno,
    );
    SUL_DB
        .query(sql)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "prepare increment attempt dbnum={} failed: {error}",
                attempt.dbnum
            )
        })?
        .check()
        .map_err(|error| {
            anyhow::anyhow!(
                "prepare increment attempt dbnum={} statement failed: {error}",
                attempt.dbnum
            )
        })?;
    Ok(())
}

pub async fn mark_attempt_commit_state_on(
    db: &Surreal<Any>,
    dbnum: u32,
    commit_token: &str,
    status: &str,
) -> anyhow::Result<()> {
    let commit_token = escape_surql_str(commit_token);
    let status = escape_surql_str(status);
    db.query(format!(
        "UPDATE {ATTEMPT_TABLE}:{dbnum} SET commit_token = '{commit_token}', \
         status = '{status}', updated_at = time::now();"
    ))
    .await
    .map_err(|error| anyhow::anyhow!("mark increment attempt dbnum={dbnum} failed: {error}"))?
    .check()
    .map_err(|error| {
        anyhow::anyhow!("mark increment attempt dbnum={dbnum} statement failed: {error}")
    })?;
    Ok(())
}

/// The monotonic watermark advance for one `dbnum`. Rendered in one place so
/// the window and baseline transactions cannot drift apart.
///
/// `end_sesno_time` 是右端那条保存在 E3D 里的**写入时刻**（RFC3339，ADR-020 那把尺子）。
/// 存它的唯一理由是回退阻断卡（plant-ui ADR-0019 Q6）：文件被换回旧版本之后，
/// `applied_sesno` 那一页在当前文件里读不到了，它的写入时刻现读不出来——水位推进的
/// 这一刻是唯一能顺手存下来的时机。
///
/// 两条硬规矩：
///
/// 1. **时刻跟着序号那条单调条件走。** 序号是 `math::max`，刻意不回退；时刻若无条件
///    赋值，一个 `end_sesno` 低于存量水位的批次会让序号不动、时刻却退回去，而阻断卡
///    恰好靠这一对说话。条件子句必须排在 `applied_sesno` 被覆盖**之前**——SurrealDB
///    的 `SET` 顺序求值，排在后面读到的就是刚写完的新值（同 `render_upsert` 的复活子句）。
/// 2. **拿不到时刻就整条子句都不写**（不是写 `NONE`），让旧行与读不到时刻的新行走同一
///    条降级路径：界面说「应用时刻无记录」，**绝不拿挂钟 `applied_at` 兜底**——回退本来
///    就是时间倒挂场景，两把尺子混用最容易骗人。
///
/// 存的是 RFC3339 **字符串**而不是 `type::datetime`，与本表其余时间列（`applied_at` /
/// `scanned_at` / `file_modified_at`）不同：那些没人读回 Rust，而这一列要原样露给界面。
/// 走 datetime 读回来会被规范化成 UTC，同一张阻断卡上「文件端」（现读，带 +08:00）与
/// 「已应用端」就会差八个小时——同一条保存两种说法，正是这轮改造要消灭的东西。
pub(crate) fn render_watermark_advance(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
) -> String {
    let time_clause = end_sesno_time
        .map(|time| {
            format!(
                "applied_sesno_time = IF {end_sesno} >= (applied_sesno ?: 0) \
                 THEN '{}' ELSE applied_sesno_time END, ",
                escape_surql_str(time)
            )
        })
        .unwrap_or_default();
    let source_hash = crate::data_interface::batch_worker::active_frozen_source()
        .ok()
        .flatten()
        .filter(|source| {
            source.dbnum() as u32 == dbnum && source.target_sesno() as i32 == end_sesno
        })
        .and_then(|source| source.commit_token_json().ok())
        .map(|token| crate::data_interface::digest::digest(token.as_bytes()));
    let source_clause = source_hash.map(|hash| format!(
        "applied_source_hash = IF {end_sesno} >= (applied_sesno ?: 0) THEN '{hash}' ELSE applied_source_hash END, "))
        .unwrap_or_else(|| format!("applied_source_hash = IF {end_sesno} > (applied_sesno ?: 0) THEN NONE ELSE applied_source_hash END, "));
    format!(
        "UPSERT dbnum_watermark:{dbnum} SET dbnum = {dbnum}, \
         {time_clause}{source_clause}\
         applied_sesno = math::max([applied_sesno?:0, {end_sesno}]), \
         sesno = math::max([sesno?:0, {end_sesno}]), \
         state_revision = (state_revision ?? 0) + 1, \
         applied_at = time::now(), updated_at = time::now();"
    )
}

/// 记录窗口规划时观察到的懒生成根数。它是观察值，不是承诺，也不参与两条链放行。
pub(crate) fn render_model_root_observation(
    dbnum: u32,
    lazy_roots: u32,
    observed_at: i32,
) -> String {
    format!(
        "UPDATE dbnum_watermark:{dbnum} SET model_lazy_roots = {lazy_roots}, \
         model_roots_observed_at = {observed_at}, updated_at = time::now();"
    )
}

/// 窗口语句批的分块大小。与主数据落库的 `TX_CHUNK` 同一纪律（`increment_pipeline::
/// persist_latest_main_data`）：整窗口单事务撑爆 SurrealDB ws 通道是已记录事故。
pub(crate) const FINALIZE_WINDOW_TX_CHUNK: usize = 500;

/// 一次收口的两段渲染产物（2026-08-10 审核 P1）。
///
/// `window_batches`：本窗口的 datacenter 交付状态与 anc 定点重算语句，按
/// [`FINALIZE_WINDOW_TX_CHUNK`] 分块、每块各自包装成独立事务，**先于尾事务**按
/// 原序执行。它们曾整段塞进尾事务：正确性要求其实只有「不得在水位推进之后丢失」，
/// 而语句数 ∝ 窗口内的操作数——宽 DESI 窗口会把尾事务推到当年撑爆 ws 通道的量级，
/// 且尾事务确定性失败 = 写回无限重试 + 重启重放同一窗口再失败的跨重启活锁。
/// 拆块之后不变量依然成立：任何一块失败都发生在水位推进**之前**——水位不动、
/// 恢复记录还在，整窗口按同一区间重放，幂等的固定目标 UPDATE 重复应用只是空转。
///
/// `tail`：收口尾事务体（未包装）——durable 模型工作、空间意图、revision 收口、
/// 水位推进、attempts 清除、恢复记录删除。行数有界（∝ 工作项数），保持单个原子
/// 事务：这一段才是「要么全部成立、要么整体重放」的收口本体。
#[derive(Debug, Clone)]
pub(crate) struct FinalizeRender {
    /// 已各自包装成 `BEGIN…COMMIT` 的窗口语句批，按原序执行。
    pub window_batches: Vec<String>,
    /// 尾事务体（未包装），由执行方套上唯一的事务包装。
    pub tail: String,
}

fn render_window_statement_batches(window_statements: &[String]) -> Vec<String> {
    window_statements
        .chunks(FINALIZE_WINDOW_TX_CHUNK)
        .map(|chunk| {
            format!(
                "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
                chunk.join("\n")
            )
        })
        .collect()
}

/// ADR-017 T1.3：窗口收口的渲染（窗口语句批 + 尾事务体）。
///
/// 暂存路径由 `StagedExecutor::commit_to` 在 journal 重放之后、尾事务之前逐批
/// 执行 `window_batches`；直写路径由 [`finalize_attempt_on`] 同序执行——两条
/// 路径共用同一份渲染，收口内容不可能漂移。顺序：窗口语句批（datacenter 交付
/// 状态 + anc 重算，commit-time 语义）→ 尾事务（durable 模型工作 → 水位推进 →
/// 恢复记录删除）。收口条件（水位单调、revision 判真）全部在持久层事务内判定。
pub(crate) fn render_finalize_tail(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> FinalizeRender {
    render_finalize_tail_with_effects(
        dbnum,
        end_sesno,
        end_sesno_time,
        plan,
        window_statements,
        &[],
        &[],
        &[],
    )
    .expect("empty finalize effects are valid")
}

pub(crate) fn render_finalize_tail_with_effects(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
    refresh_refnos: &[String],
    remove_refnos: &[String],
    settled_regen: &[(String, u64)],
) -> anyhow::Result<FinalizeRender> {
    let mut statements: Vec<String> = plan
        .work_items
        .iter()
        .map(|item| render_upsert(item, source_time_for(item, end_sesno, end_sesno_time)))
        .collect();
    if !refresh_refnos.is_empty() || !remove_refnos.is_empty() {
        statements.push(
            crate::data_interface::side_effect_pending::SideEffectCompensator::render_spatial_reconcile_upsert(
                dbnum,
                end_sesno,
                refresh_refnos,
                remove_refnos,
            )?,
        );
        // 空间版本号与意图、水位同一事务递增：启动侧拿 sidecar 与它比相等来决定
        // 树文件还能不能信（load_project_tree_verified）。只有携带空间意图的尾
        // 事务才 bump——没动树的提交不该作废别人的文件。
        statements.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
    }
    if plan.room_rebuild_required {
        statements.push(crate::fast_model::room_model::render_room_rebuild_required(
            plan.room_rebuild_reason.as_deref(),
        ));
    }
    statements.extend(settled_regen.iter().map(|(root, revision)| {
        render_delete_revision(ModelWorkAction::RegenRoot, root, *revision)
    }));
    if let Some(lazy_roots) = plan.lazy_roots {
        statements.push(render_model_root_observation(dbnum, lazy_roots, end_sesno));
    }
    statements.push(render_watermark_advance(dbnum, end_sesno, end_sesno_time));
    statements.push(crate::data_interface::staging::attempts::render_clear_window_attempts(dbnum));
    statements.push(format!("DELETE {ATTEMPT_TABLE}:{dbnum};"));
    Ok(FinalizeRender {
        window_batches: render_window_statement_batches(window_statements),
        tail: statements.join("\n"),
    })
}

/// Render the ordered batches that close a freshly parsed baseline.
///
/// Generation work is idempotent and executes in bounded transactions before
/// the watermark tail. A failed batch therefore leaves the watermark in place;
/// replaying the baseline only upserts the same deterministic work again. A
/// baseline has no `increment_update_attempt` row, so the tail never removes
/// one that may belong to another path.
pub(crate) fn render_baseline_finalize(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    confirmed_empty: bool,
) -> FinalizeRender {
    let work_statements: Vec<String> = plan
        .work_items
        .iter()
        .map(|item| render_upsert(item, source_time_for(item, end_sesno, end_sesno_time)))
        .collect();
    let mut tail = Vec::new();
    if plan.room_rebuild_required {
        tail.push(crate::fast_model::room_model::render_room_rebuild_required(
            plan.room_rebuild_reason.as_deref(),
        ));
    }
    tail.push(render_watermark_advance(dbnum, end_sesno, end_sesno_time));
    tail.push(if confirmed_empty {
        format!("UPDATE dbnum_watermark:{dbnum} SET confirmed_empty_baseline_sesno = {end_sesno};")
    } else {
        format!("UPDATE dbnum_watermark:{dbnum} SET confirmed_empty_baseline_sesno = NONE;")
    });
    FinalizeRender {
        window_batches: render_window_statement_batches(&work_statements),
        tail: tail.join("\n"),
    }
}

/// Establish durable model work, advance the authoritative watermark, and
/// remove the recovery record — the last three atomically.
///
/// `window_statements` carries writes that must never be lost under an
/// advancing watermark — this window's `datacenter_version` status updates and
/// the OWNER-move `anc` repairs. They execute as ordered [`FinalizeRender::
/// window_batches`] **before** the tail transaction: any batch failure returns
/// before the watermark moves, so the whole fixed range replays and the
/// idempotent fixed-target UPDATEs converge. Executing them after (or without
/// gating) the watermark was the historical bug this ordering exists to
/// prevent — a lost status write was unrepairable because no later window
/// revisits an element that did not change again.
///
/// After the tail commits, `plan.credential_advance` (ADR-056 P2-2) moves the
/// untouched roots' `gen_root` credentials to `end_sesno` on a best-effort
/// basis — outside the transaction on purpose, so a model-side failure can
/// neither roll back nor block the watermark (N4).
///
/// 返回那次前移的账（名单为空则 `None`）：它只是回执材料（`DataBatchResult.credential_advance`，
/// plant-ui 计划 G3），前移失败也不是错误——账上 `advanced = 0`，日志里说了原因。
pub async fn finalize_attempt(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<Option<CredentialAdvanceOutcome>> {
    let outcome = finalize_attempt_on(
        &SUL_DB,
        dbnum,
        end_sesno,
        end_sesno_time,
        plan,
        window_statements,
    )
    .await?;
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored(dbnum);
    Ok(outcome)
}

pub(crate) async fn finalize_attempt_on(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<Option<CredentialAdvanceOutcome>> {
    let render = render_finalize_tail(dbnum, end_sesno, end_sesno_time, plan, window_statements);
    for (index, batch) in render.window_batches.iter().enumerate() {
        db.query(batch)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize increment attempt dbnum={dbnum} window batch {index} failed: {error}"
                )
            })?
            .check()
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize increment attempt dbnum={dbnum} window batch {index} \
                     statement failed: {error}"
                )
            })?;
    }
    db.query(format!(
        "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
        render.tail
    ))
    .await
    .map_err(|error| anyhow::anyhow!("finalize increment attempt dbnum={dbnum} failed: {error}"))?
    .check()
    .map_err(|error| {
        anyhow::anyhow!("finalize increment attempt dbnum={dbnum} statement failed: {error}")
    })?;

    // Unchanged roots retain their generation receipt. The enclosing Task
    // commits continuous dbnum coverage only after all changed roots publish.
    Ok(None)
}

/// Establish a freshly parsed `dbnum`'s model work before advancing its watermark.
///
/// A baseline full-parse writes element data but no geometry, and every later
/// incremental window only regenerates the roots that window itself touched. So
/// a watermark that advances without its generation work leaves the database
/// permanently modelless — nothing revisits a root that never changes again.
/// Work upserts run in bounded, idempotent batches before the small watermark
/// transaction. Any failure returns before the watermark moves, so a retry
/// converges on the same work and a modelless watermarked baseline remains
/// unreachable.
pub async fn finalize_baseline(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    confirmed_empty: bool,
) -> anyhow::Result<()> {
    let render = render_baseline_finalize(dbnum, end_sesno, end_sesno_time, plan, confirmed_empty);
    for (index, batch) in render.window_batches.iter().enumerate() {
        SUL_DB
            .query(batch)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize baseline dbnum={dbnum} model work batch {index} failed: {error}"
                )
            })?
            .check()
            .map_err(|error| {
                anyhow::anyhow!(
                    "finalize baseline dbnum={dbnum} model work batch {index} statement failed: {error}"
                )
            })?;
    }
    SUL_DB
        .query(format!(
            "BEGIN TRANSACTION;\n{}\nCOMMIT TRANSACTION;",
            render.tail
        ))
        .await
        .map_err(|error| anyhow::anyhow!("finalize baseline dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("finalize baseline dbnum={dbnum} statement failed: {error}")
        })?;
    Ok(())
}
