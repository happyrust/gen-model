//! 数据窗口的收口尾事务：「数据 + 水位（+ 未触根凭证前移）」那一笔原子提交。
//!
//! 与隔壁 [`model_update_pending`](crate::data_interface::model_update_pending) 同住过
//! 一个文件，命却不同：那张表是模型工作队列，ADR-065 D3 判它**整体退役**；收口尾事务属
//! **数据面**，同一条 D3 点名保留。搬出来是计划 §4 **S5a**
//! （`docs/plans/2026-09-08-external-increment-refactor-plan.md`，缺口 G1）。
//!
//! **2026-09-10（ADR-072 D1）**：一起搬出来的那半边——固定区间重放的恢复记录
//! `increment_update_attempt:{dbnum}`（`IncrementUpdateAttempt` / `prepare_attempt` /
//! `load_attempt`）——退役了。崩溃恢复不再「复用旧计划重放那一段」，改成
//! [`write_state`](crate::data_interface::write_state) 的写入标记：见 `writing{S,T}` 就回退到 S、
//! 从头重算 S→T。于是尾事务这边少了最后一句：**它不再删任何标记行**，标记由任务收尾那一笔
//! （`task_end`）盖成 `finished`。
//!
//! **2026-09-10（ADR-072 D5）**：尾事务不再写模型意图行——任务计划在内存，`RegenRoot` 之外的
//! 工作项由任务就地执行（`batch_worker::run_model_side_actions`）；空间意图行、per-root
//! attempts 与 `window_block` 也随第二条异步账一起退了。尾事务现在只剩：房间全量重建要求
//! （ADR-040）、懒生成根观察值、水位推进。
//!
//! **2026-09-15（ADR-079）**：水位之后那一步「未触根凭证前移」（P2-2，`UPDATE gen_root SET
//! source_end_sesno = T`）随 `gen_root` 表退役——「这份几何到 T 仍有效」由任务收尾盖到 T 的
//! `model_sesno` 一并承诺，库里没有任何一行要为它改；前移名单只进回执。

use aios_core::SUL_DB;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_plan::ModelUpdatePlan;

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
/// `tail`：收口尾事务体（未包装）——房间全量重建要求、懒生成根观察值、水位推进。
/// 行数有界，保持单个原子事务：这一段才是「要么全部成立、要么整体重放」的收口本体。
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
/// 直写路径由 [`finalize_attempt`] 同序执行。顺序：窗口语句批（datacenter 交付
/// 状态 + anc 重算，commit-time 语义）→ 尾事务（房间重建要求 → 懒根观察 → 水位推进）。
/// 收口条件（水位单调）在持久层事务内判定。
///
/// 计划里的工作项**不进**这一笔（ADR-072 D5）：它们随 `IncrFileSuccess::model_plan` 交回任务，
/// 由任务在内存里执行。写入标记 `dbnum_write_state:{dbnum}` 也**不在这里动**：数据尾事务盖的
/// 是 `applied_sesno`，而「本任务的写库结束了」由任务收尾那一笔说（ADR-072 D1 / W2）。
pub(crate) fn render_finalize_tail(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> FinalizeRender {
    let mut statements: Vec<String> = Vec::new();
    if let Some(lazy_roots) = plan.lazy_roots {
        statements.push(render_model_root_observation(dbnum, lazy_roots, end_sesno));
    }
    statements.push(render_watermark_advance(dbnum, end_sesno, end_sesno_time));
    FinalizeRender {
        window_batches: render_window_statement_batches(window_statements),
        tail: statements.join("\n"),
    }
}

/// Render the transaction that closes a freshly parsed baseline: watermark + the
/// confirmed-empty marker. 2026-09-14 起（ADR-077 D7）首次初始化不再走这一笔——它的 applied
/// 与模型水位、写入标记并进 `task_end::TaskEnd::data` 那一笔；这里只剩「已有基线、重复点名」
/// 的幂等收口（`manual_update::initialize_dbnum_baseline` 的尾巴）。基线的写入标记（`writing{0,T}`）
/// 同样不在这一笔里收——收它的是任务收尾（ADR-072 D1）。
pub(crate) fn render_baseline_finalize(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    confirmed_empty: bool,
) -> FinalizeRender {
    let tail = vec![
        render_watermark_advance(dbnum, end_sesno, end_sesno_time),
        render_confirmed_empty_baseline(dbnum, end_sesno, confirmed_empty),
    ];
    FinalizeRender {
        window_batches: Vec::new(),
        tail: tail.join("\n"),
    }
}

/// 空基线凭据那一句（ADR-021）：合法空库把 `confirmed_empty_baseline_sesno` 钉在这一版，
/// 非空基线清掉它。初始化收尾那一笔（`task_end::TaskEnd::data`，ADR-077 D7）与
/// [`render_baseline_finalize`] 共用这一句。
pub(crate) fn render_confirmed_empty_baseline(
    dbnum: u32,
    end_sesno: i32,
    confirmed_empty: bool,
) -> String {
    if confirmed_empty {
        format!("UPDATE dbnum_watermark:{dbnum} SET confirmed_empty_baseline_sesno = {end_sesno};")
    } else {
        format!("UPDATE dbnum_watermark:{dbnum} SET confirmed_empty_baseline_sesno = NONE;")
    }
}

/// Establish durable model work and advance the authoritative watermark
/// atomically.
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
/// `plan.credential_advance`（ADR-056 P2-2 的未触根名单）在这里**不再有动作**（ADR-079）：
/// 那些根到 T 仍有效，由任务收尾盖到 T 的 `model_sesno` 一并承诺；名单只进回执
/// （`CredentialAdvanceReceipt::from_plan`）。
pub async fn finalize_attempt(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    plan: &ModelUpdatePlan,
    window_statements: &[String],
) -> anyhow::Result<()> {
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
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);
    Ok(())
}

/// Advance an already-baselined `dbnum`'s watermark (idempotent re-run only).
///
/// 首次初始化的 applied 不在这里推（ADR-077 D7：并进 `dbnum_initialize` 收尾那一笔，与模型水位、
/// 写入标记同生同死）。这里只服务「库里已有行、水位也在、又被点名一次」那条路：单调 `math::max`，
/// 幂等，不动模型。
pub async fn finalize_baseline(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    confirmed_empty: bool,
) -> anyhow::Result<()> {
    finalize_baseline_on(&SUL_DB, dbnum, end_sesno, end_sesno_time, confirmed_empty).await
}

/// [`finalize_baseline`] 的显式句柄版。
pub(crate) async fn finalize_baseline_on(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    confirmed_empty: bool,
) -> anyhow::Result<()> {
    let render = render_baseline_finalize(dbnum, end_sesno, end_sesno_time, confirmed_empty);
    db.query(format!(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Watermark {
        applied_sesno: Option<i64>,
        applied_sesno_time: Option<String>,
        confirmed_empty_baseline_sesno: Option<i64>,
    }

    async fn mem_library(name: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("increment_tail")
            .use_db(name)
            .await
            .expect("use db");
        db
    }

    async fn watermark(db: &Surreal<Any>, dbnum: u32) -> Option<Watermark> {
        db.query(format!(
            "SELECT applied_sesno, applied_sesno_time, confirmed_empty_baseline_sesno \
             FROM ONLY dbnum_watermark:{dbnum};"
        ))
        .await
        .expect("read transport")
        .check()
        .expect("read watermark")
        .take(0)
        .expect("decode watermark")
    }

    /// 收口写进调用方给的那个库：窗口语句批先落，水位与右端写入时刻随尾事务推进。
    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_attempt_lands_the_window_and_the_watermark_in_the_library_it_is_given() {
        let db = mem_library("finalize_attempt").await;
        let window = vec!["UPSERT tail_probe:window SET landed = true;".to_string()];
        finalize_attempt(
            &db,
            7993,
            12,
            Some("2026-11-07T10:00:00+08:00"),
            &ModelUpdatePlan::default(),
            &window,
        )
        .await
        .expect("finalize on the given library");

        let landed: Option<bool> = db
            .query("SELECT VALUE landed FROM ONLY tail_probe:window;")
            .await
            .expect("read transport")
            .check()
            .expect("read probe")
            .take(0)
            .expect("decode probe");
        assert_eq!(landed, Some(true), "窗口语句批落在同一个库");
        assert_eq!(
            watermark(&db, 7993).await,
            Some(Watermark {
                applied_sesno: Some(12),
                applied_sesno_time: Some("2026-11-07T10:00:00+08:00".to_string()),
                confirmed_empty_baseline_sesno: None,
            })
        );
    }

    /// 基线的幂等收口同样只写给定的库：钉上空基线凭据，水位单调、低版本不回退。
    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_baseline_on_advances_the_library_it_is_given_monotonically() {
        let db = mem_library("finalize_baseline").await;
        finalize_baseline_on(&db, 7994, 25, None, true)
            .await
            .expect("baseline on the given library");
        assert_eq!(
            watermark(&db, 7994).await,
            Some(Watermark {
                applied_sesno: Some(25),
                applied_sesno_time: None,
                confirmed_empty_baseline_sesno: Some(25),
            })
        );

        finalize_baseline_on(&db, 7994, 18, None, false)
            .await
            .expect("older baseline");
        let after = watermark(&db, 7994).await.expect("row stays");
        assert_eq!(after.applied_sesno, Some(25), "低于存量的基线不回退水位");
    }
}
