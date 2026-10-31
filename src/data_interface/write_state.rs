//! 写入标记 `dbnum_write_state:{dbnum}` 与整库恢复（ADR-073 D3）。
//!
//! 一个 dbnum 任务把内存结果同步写进库；写库期间库里有一行 `writing{S,T}`，任务收尾那一笔
//! 控制面事务把它改成 `finished`。进程死在中途，下次启动的水化段（或同库下一次任务开工前）
//! 见到 `writing` 就把该 dbnum 排成 `Reinitialize`，在普通重建路径整库清空后从头生成。
//!
//! 它住的是 `increment_update_attempt:{dbnum}` 那一行的位置（计划 §2.4），但**不是**它的改名版：
//!
//! | | `increment_update_attempt`（退役中） | `dbnum_write_state`（本模块） |
//! |---|---|---|
//! | 存什么 | 固定区间 + 整份 `plan_json` + `commit_token` | 只有 `status / start_sesno / end_sesno / intent` |
//! | 崩溃后 | **复用旧计划**原样重放那一段 | 计划不留，整库重建 |
//! | 谁删 | 数据尾事务（`finalize_attempt`） | 任务收尾那一笔改成 `finished`，行留着 |
//!
//! 三条不变量（ADR-072）：
//!
//! - **W1 有 writing 必有出路**：任一时刻 `status = writing` 的行，要么它的任务在飞，要么下一次
//!   启动 / 开工会把整个 dbnum 重建。落实处：`batch_worker` 无论成败都以一笔收尾事务把它盖成 `finished`。
//! - **W2 finished 只由收尾写**：`status = 'finished'` 只出现在 [`render_mark_finished`]，
//!   而它只被 `task_end::render_task_end_tail` 拼进收尾那一笔。数据尾事务不碰这一行。
//! - **W3 恢复不猜局部完整性**：五种历史 intent 都折成 `Reinitialize`。

use anyhow::Context;
use serde::{Deserialize, Serialize};
use surrealdb::{Surreal, engine::any::Any};

use aios_core::SUL_DB;

use super::dbnum_state::escape_surql_str;

/// 一库一行的写入标记表。
pub const WRITE_STATE_TABLE: &str = "dbnum_write_state";

/// 写库中（任务开工时置，崩溃后由回退清掉）。
pub const STATUS_WRITING: &str = "writing";
/// 本任务的写库结束了（欠什么由三水位回答，不由这一格回答）。
pub const STATUS_FINISHED: &str = "finished";

/// 这一次写库是哪一种任务（d-283 的五种意图）。回退时决定走哪条清理路：
/// `Initialize` / `Reinitialize` 的 S 恒为 0，回退 = 整库 wipe；其余按 sesno 谓词删。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteIntent {
    Initialize,
    ApplyWindow,
    ModelOnly,
    RoomOnly,
    Reinitialize,
}

impl WriteIntent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::ApplyWindow => "apply_window",
            Self::ModelOnly => "model_only",
            Self::RoomOnly => "room_only",
            Self::Reinitialize => "reinitialize",
        }
    }

    /// 库里读回来的字符串。认不出的值按 `ApplyWindow` 处理——回退宁可多删 `sesno > S`
    /// 的痕迹，也不能因为一个不认识的意图就把这一行放着不管（W1）。
    pub fn from_str_or_apply_window(value: &str) -> Self {
        match value {
            "initialize" => Self::Initialize,
            "model_only" => Self::ModelOnly,
            "room_only" => Self::RoomOnly,
            "reinitialize" => Self::Reinitialize,
            _ => Self::ApplyWindow,
        }
    }

    /// 回退走整库 wipe（S 恒为 0）而不是按 sesno 谓词删。
    pub const fn wipes_whole_dbnum(self) -> bool {
        matches!(self, Self::Initialize | Self::Reinitialize)
    }

    /// 这一种意图落进队列时用哪个 `BatchIntent`（回退之后排 S→T 重跑）。
    pub const fn batch_intent(self) -> super::batch_queue::BatchIntent {
        match self {
            Self::Initialize | Self::Reinitialize => super::batch_queue::BatchIntent::Reinitialize,
            Self::ModelOnly => super::batch_queue::BatchIntent::ModelOnly,
            Self::ApplyWindow | Self::RoomOnly => super::batch_queue::BatchIntent::ApplyWindow,
        }
    }
}

/// ADR-073 D3: a durable `writing` row means the dbnum may contain a partially
/// applied task. Recovery never tries to infer which face is safe to keep; all
/// five historical intents restart through the whole-dbnum reinitialize path.
pub const fn recovery_batch_intent(
    _interrupted: WriteIntent,
) -> super::batch_queue::BatchIntent {
    super::batch_queue::BatchIntent::Reinitialize
}

/// 库里那一行的解出形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteState {
    pub dbnum: u32,
    pub status: String,
    /// 本次任务的起点 S：回退删的是 `sesno > S` 的痕迹，三水位退回它。
    pub start_sesno: i32,
    /// 目标 T：回退之后按 S→T 重排。
    pub end_sesno: i32,
    pub intent: WriteIntent,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl WriteState {
    pub fn is_writing(&self) -> bool {
        self.status == STATUS_WRITING
    }
}

#[derive(Debug, Deserialize)]
struct WriteStateRow {
    dbnum: u32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    start_sesno: i32,
    #[serde(default)]
    end_sesno: i32,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    finished_at: Option<String>,
}

impl From<WriteStateRow> for WriteState {
    fn from(row: WriteStateRow) -> Self {
        Self {
            dbnum: row.dbnum,
            status: row.status,
            start_sesno: row.start_sesno,
            end_sesno: row.end_sesno,
            intent: row
                .intent
                .as_deref()
                .map(WriteIntent::from_str_or_apply_window)
                .unwrap_or(WriteIntent::ApplyWindow),
            started_at: row.started_at,
            finished_at: row.finished_at,
        }
    }
}

/// 开工那一句：第一笔写库**之前**落，`finished_at` 一并清空（同一个 dbnum 的上一次任务
/// 留下的收尾时刻不许跟着这一次走）。
pub fn render_mark_writing(
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
    intent: WriteIntent,
) -> String {
    format!(
        "UPSERT {WRITE_STATE_TABLE}:{dbnum} SET dbnum = {dbnum}, status = '{STATUS_WRITING}', \
         start_sesno = {start_sesno}, end_sesno = {end_sesno}, intent = '{}', \
         started_at = time::now(), finished_at = NONE, updated_at = time::now();",
        intent.as_str()
    )
}

/// 收尾那一句。**只许** `task_end::render_task_end_tail` 拼它（W2），与三水位同一笔事务。
///
/// `UPDATE` 而不是 `UPSERT`：没有开工那一行就谈不上「这一次写库结束了」，凭空造一行出来
/// 只会让下一次启动读到一个没有对手的 `finished`。
pub(crate) fn render_mark_finished(dbnum: u32) -> String {
    format!(
        "UPDATE {WRITE_STATE_TABLE}:{dbnum} SET status = '{STATUS_FINISHED}', \
         finished_at = time::now(), updated_at = time::now();"
    )
}

/// 回退的最后一句：标记行整条删掉，下一次任务开工重写 `writing`（§4.3-6）。
pub(crate) fn render_clear(dbnum: u32) -> String {
    format!("DELETE {WRITE_STATE_TABLE}:{dbnum};")
}

pub async fn mark_writing(
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
    intent: WriteIntent,
) -> anyhow::Result<()> {
    mark_writing_on(&SUL_DB, dbnum, start_sesno, end_sesno, intent).await
}

pub(crate) async fn mark_writing_on(
    db: &Surreal<Any>,
    dbnum: u32,
    start_sesno: i32,
    end_sesno: i32,
    intent: WriteIntent,
) -> anyhow::Result<()> {
    db.query(render_mark_writing(dbnum, start_sesno, end_sesno, intent))
        .await
        .with_context(|| format!("mark dbnum {dbnum} writing"))?
        .check()
        .with_context(|| format!("mark dbnum {dbnum} writing statement"))?;
    Ok(())
}

/// 两格时刻的读法。**必须逐格判 NONE**：`type::string(NONE)` 在这个 fork 上直接报
/// 「Expected a string but cannot convert NONE into a string」，而刚落下的 `writing` 行
/// `finished_at` 恰恰就是 NONE——不判就等于「开工之后读不出自己的标记」。
const TIME_COLUMNS: &str = "IF started_at = NONE THEN NONE ELSE type::string(started_at) END \
     AS started_at, \
     IF finished_at = NONE THEN NONE ELSE type::string(finished_at) END AS finished_at";

/// 删掉标记行。整库 wipe 那条回退路用它收口——wipe 自己不认识这张表。
pub(crate) async fn clear_marker(dbnum: u32) -> anyhow::Result<()> {
    SUL_DB
        .query(render_clear(dbnum))
        .await
        .with_context(|| format!("clear write state of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("clear write state of dbnum {dbnum} statement"))?;
    Ok(())
}

pub async fn load(dbnum: u32) -> anyhow::Result<Option<WriteState>> {
    load_on(&SUL_DB, dbnum).await
}

pub(crate) async fn load_on(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<Option<WriteState>> {
    let mut response = db
        .query(format!(
            "SELECT dbnum, status, start_sesno, end_sesno, intent, {TIME_COLUMNS} \
             FROM {WRITE_STATE_TABLE}:{dbnum};"
        ))
        .await
        .with_context(|| format!("load write state of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("load write state of dbnum {dbnum} statement"))?;
    let rows: Vec<WriteStateRow> = response
        .take(0)
        .with_context(|| format!("decode write state of dbnum {dbnum}"))?;
    Ok(rows.into_iter().next().map(WriteState::from))
}

/// 全部还停在 `writing` 的库，按 dbnum 升序。启动水化段扫的就是它（ADR-072 D2）。
pub async fn writing_states() -> anyhow::Result<Vec<WriteState>> {
    writing_states_on(&SUL_DB).await
}

pub(crate) async fn writing_states_on(db: &Surreal<Any>) -> anyhow::Result<Vec<WriteState>> {
    let mut response = db
        .query(format!(
            "SELECT dbnum, status, start_sesno, end_sesno, intent, {TIME_COLUMNS} \
             FROM {WRITE_STATE_TABLE} WHERE status = '{STATUS_WRITING}' ORDER BY dbnum;"
        ))
        .await
        .context("scan writing dbnums")?
        .check()
        .context("scan writing dbnums statement")?;
    let rows: Vec<WriteStateRow> = response.take(0).context("decode writing dbnums")?;
    Ok(rows.into_iter().map(WriteState::from).collect())
}

/// 全部库的标记行，按 dbnum 升序——`/dbnums` 每行那格 `write_state`（spec §4.7）一趟读回，
/// 不逐库点名。这是计划 §4.5 白名单里「`dbnum_write_state` 控制面信息」那一格的读法。
pub async fn load_all() -> anyhow::Result<Vec<WriteState>> {
    load_all_on(&SUL_DB).await
}

pub(crate) async fn load_all_on(db: &Surreal<Any>) -> anyhow::Result<Vec<WriteState>> {
    let mut response = db
        .query(format!(
            "SELECT dbnum, status, start_sesno, end_sesno, intent, {TIME_COLUMNS} \
             FROM {WRITE_STATE_TABLE} ORDER BY dbnum;"
        ))
        .await
        .context("load write states")?
        .check()
        .context("load write states statement")?;
    let rows: Vec<WriteStateRow> = response.take(0).context("decode write states")?;
    Ok(rows.into_iter().map(WriteState::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn database(label: &str) -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("write_state").use_db(label).await.unwrap();
        db
    }

    #[test]
    fn the_opening_statement_names_the_window_and_clears_the_previous_finish() {
        let sql = render_mark_writing(7997, 41, 43, WriteIntent::ApplyWindow);
        assert!(sql.contains("UPSERT dbnum_write_state:7997"), "{sql}");
        assert!(sql.contains("status = 'writing'"), "{sql}");
        assert!(sql.contains("start_sesno = 41"), "{sql}");
        assert!(sql.contains("end_sesno = 43"), "{sql}");
        assert!(sql.contains("intent = 'apply_window'"), "{sql}");
        assert!(
            sql.contains("finished_at = NONE"),
            "上一次的收尾时刻不许跟着这一次走: {sql}"
        );
    }

    /// W2：`finished` 只有一句，而且是 `UPDATE`——没有开工行就不该凭空造一个收尾。
    #[test]
    fn only_one_statement_writes_finished_and_it_never_creates_the_row() {
        let sql = render_mark_finished(7997);
        assert_eq!(sql.matches("status = 'finished'").count(), 1, "{sql}");
        assert!(sql.starts_with("UPDATE dbnum_write_state:7997 SET"), "{sql}");
        assert!(!sql.contains("UPSERT"), "{sql}");

        let production = include_str!("write_state.rs")
            .split_once("#[cfg(test)]")
            .expect("tests follow")
            .0;
        assert_eq!(
            production.matches("STATUS_FINISHED}'").count(),
            1,
            "本模块里只有 render_mark_finished 写得出 finished"
        );
    }

    /// T-A-5：`writing` 标记不管原本是哪一种 intent，重启后一律折成 `Reinitialize`
    /// （ADR-073 D3「恢复不猜局部完整性」）。数据尾已成功、kill 落在 `task_end` 之前时，
    /// 这一条保证那个库不会带着半截模型面继续跑。
    #[test]
    fn a_writing_marker_of_any_intent_recovers_as_a_whole_dbnum_reinitialize() {
        use crate::data_interface::batch_queue::BatchIntent;
        for intent in [
            WriteIntent::Initialize,
            WriteIntent::Reinitialize,
            WriteIntent::ApplyWindow,
            WriteIntent::ModelOnly,
            WriteIntent::RoomOnly,
        ] {
            assert_eq!(
                recovery_batch_intent(intent),
                BatchIntent::Reinitialize,
                "{intent:?} 必须折成整库重建"
            );
        }
    }

    #[test]
    fn the_intent_decides_whether_the_rollback_wipes_and_which_batch_it_requeues() {
        use crate::data_interface::batch_queue::BatchIntent;
        assert!(WriteIntent::Initialize.wipes_whole_dbnum());
        assert!(WriteIntent::Reinitialize.wipes_whole_dbnum());
        for intent in [
            WriteIntent::ApplyWindow,
            WriteIntent::ModelOnly,
            WriteIntent::RoomOnly,
        ] {
            assert!(!intent.wipes_whole_dbnum(), "{intent:?}");
        }
        assert_eq!(
            WriteIntent::ModelOnly.batch_intent(),
            BatchIntent::ModelOnly
        );
        assert_eq!(
            WriteIntent::Initialize.batch_intent(),
            BatchIntent::Reinitialize
        );
        assert_eq!(
            WriteIntent::ApplyWindow.batch_intent(),
            BatchIntent::ApplyWindow
        );
        // 认不出的意图按 apply_window：宁可多删痕迹，也不能把 writing 放着不管（W1）。
        assert_eq!(
            WriteIntent::from_str_or_apply_window("who-knows"),
            WriteIntent::ApplyWindow
        );
        for intent in [
            WriteIntent::Initialize,
            WriteIntent::ApplyWindow,
            WriteIntent::ModelOnly,
            WriteIntent::RoomOnly,
            WriteIntent::Reinitialize,
        ] {
            assert_eq!(
                WriteIntent::from_str_or_apply_window(intent.as_str()),
                intent
            );
        }
    }

    #[test]
    fn every_unfinished_write_restarts_as_a_whole_dbnum_reinitialize() {
        use crate::data_interface::batch_queue::BatchIntent;

        for intent in [
            WriteIntent::Initialize,
            WriteIntent::ApplyWindow,
            WriteIntent::ModelOnly,
            WriteIntent::RoomOnly,
            WriteIntent::Reinitialize,
        ] {
            assert_eq!(
                recovery_batch_intent(intent),
                BatchIntent::Reinitialize,
                "unfinished {intent:?} must rebuild the whole dbnum"
            );
        }
    }

    #[tokio::test]
    async fn the_marker_round_trips_and_only_writing_rows_show_up_in_the_startup_scan() {
        let db = database("round_trip").await;
        mark_writing_on(&db, 7997, 41, 43, WriteIntent::ApplyWindow)
            .await
            .unwrap();
        mark_writing_on(&db, 8000, 0, 26, WriteIntent::Initialize)
            .await
            .unwrap();

        let state = load_on(&db, 7997).await.unwrap().expect("marker row");
        assert!(state.is_writing());
        assert_eq!((state.start_sesno, state.end_sesno), (41, 43));
        assert_eq!(state.intent, WriteIntent::ApplyWindow);
        assert!(state.started_at.is_some());
        assert_eq!(state.finished_at, None);

        assert_eq!(
            writing_states_on(&db)
                .await
                .unwrap()
                .iter()
                .map(|state| (state.dbnum, state.intent))
                .collect::<Vec<_>>(),
            vec![
                (7997, WriteIntent::ApplyWindow),
                (8000, WriteIntent::Initialize)
            ]
        );

        db.query(render_mark_finished(7997))
            .await
            .unwrap()
            .check()
            .unwrap();
        let closed = load_on(&db, 7997).await.unwrap().expect("marker row");
        assert!(!closed.is_writing());
        assert_eq!(closed.status, STATUS_FINISHED);
        assert!(closed.finished_at.is_some(), "收尾时刻必须留下");
        assert_eq!(
            writing_states_on(&db)
                .await
                .unwrap()
                .iter()
                .map(|state| state.dbnum)
                .collect::<Vec<_>>(),
            vec![8000],
            "盖成 finished 的库不再进启动扫描"
        );
        // `/dbnums` 那一趟整表读：finished 与 writing 都回来，按 dbnum 升序，
        // 与逐库 `load_on` 读到的是同一行。
        let all = load_all_on(&db).await.unwrap();
        assert_eq!(
            all.iter()
                .map(|state| (state.dbnum, state.status.as_str()))
                .collect::<Vec<_>>(),
            vec![(7997, STATUS_FINISHED), (8000, STATUS_WRITING)]
        );
        assert_eq!(all[0], closed);
    }

    /// 没有开工行时 `finished` 那一句不许造行——否则一个从没写过库的 dbnum 会凭空
    /// 长出一个 `finished` 标记，下一次真崩溃时反而没人回退它。
    #[tokio::test]
    async fn finishing_a_dbnum_that_never_started_writes_nothing() {
        let db = database("no_row").await;
        db.query(render_mark_finished(7997))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(load_on(&db, 7997).await.unwrap(), None);
    }
}
