//! 任务收尾的一笔控制面事务（ADR-071 D3 / D6，计划 v3 S2）。
//!
//! 一个 dbnum 任务在自己的目标 `T` 上把数据、模型两段跑完之后（房间归属随根算、随模型块
//! 同事务落边，ADR-073 D1，收尾没有房间段），把「这一版的承诺」**一次**写进库：模型水位、
//! 每个 Ref0 的元素统计、`state_revision`。事实全部来自任务自己手里的内存结果——一条都不
//! 回查库，这正是它替掉的三处的代价所在：
//!
//! | 换掉的 | 它原来的代价 |
//! |---|---|
//! | `model_watermark::complete_model_task_on` → `read_model_watermark_probe` | 每次 10 条语句、两趟 `gen_root` 全库扫（7997 实测 avg 5.5 s） |
//! | `room_completion::PreparedRoomResult::commit_on` 的水位半边 | 房间提交与水位耦在一笔 CAS 里，房间先于模型盖章（d-73） |
//! | `dbnum_stats` 的逐笔 ±1 观测块 | 每笔元素写都带 `$before_` / `$after_` 读回与一次 UPSERT |
//!
//! **谁负责什么**：本模块只负责「把任务交上来的事实写成一笔事务」。事实对不对是调用方的事
//! ——尤其 [`TaskEnd::info`]，它是**目标索引的全量快照**，只有当任务确实把那一版的数据全部
//! 落库了才能给；给了就按绝对值覆盖（ADR-071 D6：统计从内存序列化，不再逐笔加减）。
//!
//! 水位那一句仍留着 ADR-060 的三条纪律：单调 `math::max`、时刻子句排在赋值之前、
//! `WHERE (applied_sesno ?: 0) >= T` 是数据水位的第二道闸（M2）。闸挡下来不是错误，
//! 回执里 [`TaskEndReceipt::blocked_by_data`] 说明这一次没盖章。

use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

use super::dbnum_state::{DbnumState, INFO_TABLE, WATERMARK_TABLE, escape_surql_str};

/// 本任务模型阶段的结果。`all_roots_ok` 里的「成功」含该根的房间归属：某根房间算不出来
/// 或写不进去 = 该根本轮未完成（ADR-073 D2），模型水位不越过。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskModelOutcome {
    /// 这一版要生成的根全部落库成功；失败时保持模型水位。
    pub all_roots_ok: bool,
    /// 永久放弃的根数（写进 `model_dead_roots`，供 `/dbnums` 观测）。
    pub dead_roots: usize,
}

/// 目标索引里一个 Ref0 的元素统计（`count` = 该 Ref0 在 `T` 上的**活**元素数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ref0Stats {
    pub ref0: u32,
    pub count: i64,
    pub max_ref1: u32,
    pub sesno: i32,
}

/// 目标索引的 Ref0 全量快照。库标识跟着快照走——它们只有统计块用得上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ref0Snapshot {
    pub file_name: String,
    pub db_type: String,
    /// 空 = 该库在 `T` 上没有活元素（现有行全部清零）。
    pub rows: Vec<Ref0Stats>,
}

/// 一个 dbnum 任务收尾时要落的全部控制面事实。
#[derive(Debug, Clone)]
pub(crate) struct TaskEnd {
    pub dbnum: u32,
    /// 本任务冻结的目标会话号。
    pub target_sesno: i32,
    /// 目标那一版的保存时刻。号对不上就别传——宁可让界面说「无记录」，
    /// 也不拿 A 的时刻标注 B（ADR-060 M1）。
    pub target_time: Option<String>,
    /// `None` = 本任务没跑模型（纯数据批次），模型水位与根观测都不动。
    pub model: Option<TaskModelOutcome>,
    /// `None` = 本任务没有权威的目标索引快照（例如 ModelOnly 任务一个元素都没动），
    /// 此时 `dbnum_info_table` 一行都不碰。
    pub info: Option<Ref0Snapshot>,
    /// 这一笔是不是本任务的**收尾**那一笔（ADR-072 D1 / W2）。
    ///
    /// 为真时事务里多一句 `dbnum_write_state:{dbnum} SET status = 'finished'`——全库唯一
    /// 写得出 `finished` 的地方。计划 §5 R1 原写着「`TaskEnd` 不需要新字段」，那是按
    /// 「一个任务只调一次 `commit_task_end_on`」估的；实际执行体调两次（数据批次应用之后
    /// 先落一笔 Ref0 统计，模型跑完再落收尾那一笔），没有这一格，中途那一笔就会
    /// 把还在写模型的任务提前宣告写完。
    ///
    /// 任务以 `Partial` 收尾（模型失败但没崩）也要为真：`finished` 说的是「本任务
    /// 的写库结束了」，欠什么由两水位回答。
    pub closes_task: bool,
}

impl TaskEnd {
    /// 模型阶段完成（含各根的房间归属，ADR-073 D2）。
    fn model_stamps(&self) -> bool {
        self.model.as_ref().is_some_and(|model| model.all_roots_ok)
    }
}

/// 一笔收尾事务落地之后的回执。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskEndReceipt {
    /// 盖上的模型水位；未跑模型、模型失败或前置水位不足时为 None。
    pub model_sesno: Option<i32>,
    /// 写了几个 Ref0 的统计；`None` = 本次没有权威快照，没动 info 表。
    pub info_rows: Option<usize>,
    /// 兼容字段名：数据或模型前置水位不足，整条水位更新被挡下。
    pub blocked_by_data: bool,
}

/// 一笔事务的字节上限：Ref0 数量本就是几十到几百量级，超了说明调用方给错了快照，
/// 宁可当场喊出来，也不静默拆成两笔——拆开就不是一个原子的统计快照了。
fn max_tail_bytes() -> usize {
    super::tx_limits::data_tx_limits().max_bytes
}

/// 水位那一句。模型水位不用盖时只剩 `state_revision` 前进，不带数据水位闸。
fn render_watermark_update(end: &TaskEnd) -> String {
    let TaskEnd {
        dbnum,
        target_sesno,
        ..
    } = *end;
    let mut sets: Vec<String> = Vec::new();
    let mut stamps = false;

    if let Some(model) = &end.model {
        if end.model_stamps() {
            stamps = true;
            if let Some(time) = end.target_time.as_deref() {
                sets.push(format!(
                    "model_sesno_time = IF {target_sesno} >= (model_sesno ?: 0) \
                     THEN '{}' ELSE model_sesno_time END",
                    escape_surql_str(time)
                ));
            }
            sets.push(format!(
                "model_sesno = math::max([model_sesno ?: 0, {target_sesno}])"
            ));
            sets.push("model_settled_at = time::now()".to_owned());
        }
        // 根观测是现场记录，不是承诺：失败的那一版也要留下痕迹。
        sets.push(format!("model_dead_roots = {}", model.dead_roots));
        sets.push(format!("model_roots_observed_at = {target_sesno}"));
    }

    sets.push("state_revision = (state_revision ?? 0) + 1".to_owned());
    sets.push("updated_at = time::now()".to_owned());

    // `UPDATE` 而不是 `UPSERT`：没有数据水位行就谈不上「与数据一致到哪一版」，
    // 凭空造一行出来只会造出一个没有对手的水位。
    let guard = if stamps {
        format!(" WHERE (applied_sesno ?: 0) >= {target_sesno}")
    } else {
        String::new()
    };
    format!(
        "UPDATE {WATERMARK_TABLE}:{dbnum} SET {}{guard} RETURN id;",
        sets.join(", ")
    )
}

/// Ref0 统计块：先把本库现有行清零（历史 Ref0 留行不留数，供清理发现），
/// 再按快照逐个绝对覆盖。
fn render_info_snapshot(dbnum: u32, snapshot: &Ref0Snapshot) -> String {
    let file_name = escape_surql_str(&snapshot.file_name);
    let db_type = escape_surql_str(&snapshot.db_type);
    let mut sql = format!("UPDATE {INFO_TABLE} SET count = 0 WHERE dbnum = {dbnum};\n");
    for row in &snapshot.rows {
        let Ref0Stats {
            ref0,
            count,
            max_ref1,
            sesno,
        } = *row;
        sql.push_str(&format!(
            "LET $old_{ref0} = (SELECT * FROM ONLY {INFO_TABLE}:{ref0});\n\
             IF $old_{ref0}.dbnum != NONE AND $old_{ref0}.dbnum != {dbnum} \
             {{ THROW 'PE statistics Ref0 ownership conflict'; }};\n\
             UPSERT {INFO_TABLE}:{ref0} MERGE {{ dbnum: {dbnum}, count: {count}, \
             sesno: math::max([$old_{ref0}.sesno ?? 0, {sesno}]), \
             max_ref1: math::max([$old_{ref0}.max_ref1 ?? 0, {max_ref1}]), \
             file_name: '{file_name}', db_type: '{db_type}', updated_at: time::now() }};\n"
        ));
    }
    sql
}

/// 整笔收尾事务。统计块排在水位之前：统计描述的是**已经落库的数据**，与水位那一句的
/// 数据水位闸各管各的（闸挡下水位时统计照旧成立）。
///
/// 写入标记那一句排在水位**之前**（ADR-072 D1）：它与两水位同生同死，但不带数据水位闸
/// ——闸挡下水位说的是「这一版没盖成」，而本任务的写库确实结束了。位置也有讲究：
/// [`commit_task_end_on`] 按 `num_statements() - 1` 取水位那一条的回执，水位必须是最后一句。
pub(crate) fn render_task_end_tail(end: &TaskEnd) -> String {
    let info = end
        .info
        .as_ref()
        .map(|snapshot| render_info_snapshot(end.dbnum, snapshot))
        .unwrap_or_default();
    let finished = if end.closes_task {
        format!(
            "{}\n",
            super::write_state::render_mark_finished(end.dbnum)
        )
    } else {
        String::new()
    };
    format!(
        "BEGIN TRANSACTION;\n{info}{finished}{}\nCOMMIT TRANSACTION;",
        render_watermark_update(end)
    )
}

/// 落一笔收尾事务，成功后作废这个 dbnum 的镜像格与一致性普查缓存。
pub(crate) async fn commit_task_end_on(
    db: &Surreal<Any>,
    end: &TaskEnd,
) -> anyhow::Result<TaskEndReceipt> {
    let sql = render_task_end_tail(end);
    anyhow::ensure!(
        sql.len() <= max_tail_bytes(),
        "任务收尾事务超出字节上限（dbnum={} Ref0={}）：{} > {}",
        end.dbnum,
        end.info
            .as_ref()
            .map(|snapshot| snapshot.rows.len())
            .unwrap_or_default(),
        sql.len(),
        max_tail_bytes()
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("commit task end of dbnum {}", end.dbnum))?;
    // 先数再取：`take_errors` 会把出错的条目从响应里摘走。回滚掉的事务会给**出错语句
    // 之前**的每一条都报一句笼统的「事务失败」，只看第一条就永远看不到 `THROW` 的原话。
    let index = response.num_statements().saturating_sub(1);
    let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
    errors.sort_by_key(|(index, _)| *index);
    if !errors.is_empty() {
        anyhow::bail!(
            "任务收尾事务失败 dbnum={}: {}",
            end.dbnum,
            errors
                .into_iter()
                .map(|(index, error)| format!("statement {index}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    #[derive(serde::Deserialize)]
    struct Touched {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
    }
    // 统计块在前，水位那一句是最后一条带回执的语句。
    let touched: Vec<Touched> = response
        .take(index)
        .with_context(|| format!("decode task end watermark write of dbnum {}", end.dbnum))?;
    let stamped = !touched.is_empty();

    DbnumState::invalidate_mirrored_on(db, end.dbnum);
    super::dbnum_consistency::invalidate_census();

    let wanted_stamp = end.model_stamps();
    Ok(TaskEndReceipt {
        model_sesno: (stamped && wanted_stamp).then_some(end.target_sesno),
        info_rows: end.info.as_ref().map(|snapshot| snapshot.rows.len()),
        blocked_by_data: wanted_stamp && !stamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> TaskEnd {
        TaskEnd {
            dbnum: 8000,
            target_sesno: 26,
            target_time: Some("2026-09-10T09:00:00+08:00".to_owned()),
            model: Some(TaskModelOutcome {
                all_roots_ok: true,
                dead_roots: 0,
            }),
            info: Some(Ref0Snapshot {
                file_name: "ams".to_owned(),
                db_type: "DESI".to_owned(),
                rows: vec![Ref0Stats {
                    ref0: 24384,
                    count: 3,
                    max_ref1: 9,
                    sesno: 26,
                }],
            }),
            closes_task: true,
        }
    }

    async fn database() -> Surreal<Any> {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("task_end").use_db("tail").await.unwrap();
        db
    }

    #[derive(Debug, serde::Deserialize)]
    struct WatermarkRow {
        model_sesno: Option<i32>,
        model_sesno_time: Option<String>,
        state_revision: Option<u64>,
        model_dead_roots: Option<u32>,
        model_roots_observed_at: Option<i32>,
    }

    async fn watermark(db: &Surreal<Any>, dbnum: u32) -> WatermarkRow {
        db.query(format!("SELECT * FROM ONLY {WATERMARK_TABLE}:{dbnum};"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take::<Option<WatermarkRow>>(0)
            .unwrap()
            .expect("watermark row")
    }

    #[derive(Debug, serde::Deserialize)]
    struct InfoRow {
        dbnum: Option<u32>,
        count: Option<i64>,
        max_ref1: Option<u32>,
        sesno: Option<i32>,
        file_name: Option<String>,
        db_type: Option<String>,
    }

    async fn info(db: &Surreal<Any>, ref0: u32) -> Option<InfoRow> {
        db.query(format!("SELECT * FROM ONLY {INFO_TABLE}:{ref0};"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take::<Option<InfoRow>>(0)
            .unwrap()
    }

    #[test]
    fn the_tail_is_one_transaction_and_stamps_the_model_watermark_under_the_data_gate() {
        let sql = render_task_end_tail(&end());
        assert_eq!(sql.matches("BEGIN TRANSACTION;").count(), 1, "{sql}");
        assert_eq!(sql.matches("COMMIT TRANSACTION;").count(), 1, "{sql}");
        assert!(
            sql.contains("model_sesno = math::max([model_sesno ?: 0, 26])"),
            "{sql}"
        );
        assert!(
            sql.contains("WHERE (applied_sesno ?: 0) >= 26"),
            "数据水位闸（M2）必须在：{sql}"
        );
        // ADR-073 W3：行上只有两枚水位，收尾不写任何房间格。
        assert!(!sql.contains("room_"), "收尾没有房间那一枚可盖：{sql}");
        // 时刻子句排在赋值之前（SurrealDB 的 SET 顺序求值）。
        let model_time = sql
            .find("model_sesno_time = IF")
            .expect("model time clause");
        let model_value = sql
            .find("model_sesno = math::max")
            .expect("model value clause");
        assert!(model_time < model_value, "{sql}");
    }

    /// ADR-072 W2：`status = 'finished'` 全库只有这一处写得出，而且**恰好一句**、
    /// 与两水位同一笔事务、排在水位那一条之前（水位是最后一条，回执按下标取它）。
    #[test]
    fn the_closing_tail_carries_exactly_one_finished_marker_before_the_watermark() {
        let sql = render_task_end_tail(&end());
        assert_eq!(
            sql.matches("status = 'finished'").count(),
            1,
            "收尾恰含一句 finished：{sql}"
        );
        assert!(
            sql.contains("UPDATE dbnum_write_state:8000 SET status = 'finished'"),
            "{sql}"
        );
        assert_eq!(sql.matches("BEGIN TRANSACTION;").count(), 1, "{sql}");
        let marker = sql.find("dbnum_write_state:8000").expect("marker statement");
        let watermark = sql
            .find("UPDATE dbnum_watermark:8000")
            .expect("watermark statement");
        assert!(
            marker < watermark,
            "水位必须是事务最后一条（回执按下标取它）：{sql}"
        );
    }

    /// 中途那一笔（数据批次应用之后的 Ref0 统计）不许宣告写完——模型还没写。
    #[test]
    fn a_mid_task_tail_leaves_the_write_marker_alone() {
        let mut end = end();
        end.closes_task = false;
        end.model = None;
        let sql = render_task_end_tail(&end);
        assert!(!sql.contains("dbnum_write_state"), "{sql}");
        assert!(!sql.contains("finished"), "{sql}");
        assert!(sql.contains(INFO_TABLE), "统计照落：{sql}");
    }

    /// 模型失败（没崩、走到了收尾）照样收标记：`finished` 说的是「本任务的写库
    /// 结束了」，欠什么由两水位回答（ADR-072 D1）。
    #[test]
    fn a_partial_task_still_closes_its_write_marker() {
        let mut end = end();
        end.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 3,
        });
        let sql = render_task_end_tail(&end);
        assert!(sql.contains("status = 'finished'"), "{sql}");
        assert!(!sql.contains("model_sesno = math::max"), "{sql}");
    }

    #[test]
    fn the_tail_never_asks_the_database_what_the_task_already_knows() {
        let sql = render_task_end_tail(&end());
        for probe in [
            "FROM gen_root",
            "settled_floor",
            "model_publish_fragment",
            "FROM pe ",
        ] {
            assert!(!sql.contains(probe), "收尾不得回查 {probe}：{sql}");
        }
    }

    #[test]
    fn a_failed_model_leaves_the_model_watermark_alone_but_still_records_the_roots() {
        let mut end = end();
        end.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 2,
        });
        let sql = render_task_end_tail(&end);
        assert!(!sql.contains("model_sesno = math::max"), "{sql}");
        assert!(!sql.contains("model_sesno_time = IF"), "{sql}");
        assert!(sql.contains("model_dead_roots = 2"), "{sql}");
        assert!(sql.contains("model_roots_observed_at = 26"), "{sql}");
        // 模型没盖成就没有任何一枚水位要盖：数据水位闸随之不带。
        assert!(!sql.contains("WHERE (applied_sesno"), "{sql}");
        assert!(
            sql.contains("state_revision = (state_revision ?? 0) + 1"),
            "{sql}"
        );
    }

    #[test]
    fn a_tail_that_stamps_nothing_only_advances_the_revision_and_drops_the_data_gate() {
        let mut end = end();
        end.model = None;
        end.info = None;
        let sql = render_task_end_tail(&end);
        assert!(
            sql.contains("state_revision = (state_revision ?? 0) + 1"),
            "{sql}"
        );
        assert!(!sql.contains("WHERE (applied_sesno"), "{sql}");
        assert!(!sql.contains(INFO_TABLE), "没有快照就不碰 info 表：{sql}");
    }

    #[test]
    fn the_snapshot_zeroes_this_dbnum_first_and_refuses_a_ref0_owned_by_another_dbnum() {
        let sql = render_task_end_tail(&end());
        let zero = sql
            .find("UPDATE dbnum_info_table SET count = 0 WHERE dbnum = 8000;")
            .expect("zeroing statement");
        let upsert = sql
            .find("UPSERT dbnum_info_table:24384 MERGE")
            .expect("ref0 upsert");
        assert!(zero < upsert, "{sql}");
        assert!(
            sql.contains("THROW 'PE statistics Ref0 ownership conflict'"),
            "{sql}"
        );
        assert!(sql.contains("count: 3"), "统计按绝对值覆盖：{sql}");
    }

    #[test]
    fn single_quotes_in_the_file_name_cannot_escape_the_literal() {
        let mut end = end();
        end.info.as_mut().expect("snapshot").file_name = "a'; THROW 'x".to_owned();
        end.target_time = Some("t'; THROW 'y".to_owned());
        let sql = render_task_end_tail(&end);
        assert!(sql.contains("file_name: 'a\\'; THROW \\'x'"), "{sql}");
        assert!(sql.contains("THEN 't\\'; THROW \\'y'"), "{sql}");
    }

    #[tokio::test]
    async fn a_whole_task_tail_lands_three_facts_in_one_write() {
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, state_revision=4;
             CREATE dbnum_info_table:24384 SET dbnum=8000, count=99, max_ref1=1, sesno=3;
             CREATE dbnum_info_table:24390 SET dbnum=8000, count=7;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let receipt = commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: Some(26),
                info_rows: Some(1),
                blocked_by_data: false,
            }
        );

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(26));
        assert_eq!(
            row.model_sesno_time.as_deref(),
            Some("2026-09-10T09:00:00+08:00")
        );
        assert_eq!(row.state_revision, Some(5));

        let touched = info(&db, 24384).await.expect("ref0 row");
        assert_eq!(touched.count, Some(3), "绝对覆盖，不是 99 上加减");
        assert_eq!(touched.max_ref1, Some(9));
        assert_eq!(touched.sesno, Some(26));
        assert_eq!(touched.file_name.as_deref(), Some("ams"));
        let vanished = info(&db, 24390).await.expect("historical ref0 row");
        assert_eq!(vanished.count, Some(0), "快照里没有的 Ref0 清零留行");
    }

    #[tokio::test]
    async fn the_data_gate_holds_back_the_model_watermark_but_not_the_statistics() {
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=12, model_sesno=12;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let receipt = commit_task_end_on(&db, &end()).await.unwrap();
        assert!(receipt.blocked_by_data);
        assert_eq!(receipt.model_sesno, None);

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(12), "水位一格都没动");
        assert_eq!(row.state_revision, None);
        assert_eq!(
            info(&db, 24384).await.expect("ref0 row").count,
            Some(3),
            "统计描述的是已落库的数据，不受水位闸管"
        );
    }

    /// 模型失败（含某根房间算不出来，ADR-073 D2）：模型水位不动、根观测照记、
    /// 收尾照样落地——这不是「被数据挡下」，是这一版没完成。
    #[tokio::test]
    async fn a_failed_model_leaves_the_watermark_behind_but_records_the_roots() {
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let mut end = end();
        end.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 1,
        });
        let receipt = commit_task_end_on(&db, &end).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: None,
                info_rows: Some(1),
                blocked_by_data: false,
            }
        );

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(20), "模型没成，水位原地");
        assert_eq!(row.model_dead_roots, Some(1), "观测照记");
        assert_eq!(row.model_roots_observed_at, Some(26));
        assert_eq!(row.state_revision, Some(1));
        assert_eq!(
            info(&db, 24384).await.expect("ref0 row").count,
            Some(3),
            "统计描述的是已落库的数据，与模型成败无关"
        );
    }

    #[tokio::test]
    async fn a_brand_new_ref0_row_is_created_from_the_snapshot() {
        // 基线 Fresh 的常态：`dbnum_info_table` 里这个 Ref0 还没有行，
        // `SELECT * FROM ONLY …` 读到 NONE，归属检查放行，UPSERT 建行。
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(info(&db, 24384).await.is_none(), "前提：表里没有这一行");

        let receipt = commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(receipt.info_rows, Some(1));
        assert_eq!(receipt.model_sesno, Some(26));

        let created = info(&db, 24384).await.expect("ref0 row created");
        assert_eq!(created.dbnum, Some(8000));
        assert_eq!(created.count, Some(3));
        assert_eq!(created.max_ref1, Some(9));
        assert_eq!(created.sesno, Some(26));
        assert_eq!(created.file_name.as_deref(), Some("ams"));
        assert_eq!(created.db_type.as_deref(), Some("DESI"));
    }

    #[tokio::test]
    async fn an_empty_snapshot_zeroes_every_ref0_of_this_dbnum_and_leaves_other_dbnums_alone() {
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26;
             CREATE dbnum_info_table:24384 SET dbnum=8000, count=99, max_ref1=1, sesno=3;
             CREATE dbnum_info_table:24390 SET dbnum=8000, count=7;
             CREATE dbnum_info_table:16192 SET dbnum=7997, count=5;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let mut end = end();
        end.info = Some(Ref0Snapshot {
            file_name: "ams".to_owned(),
            db_type: "DESI".to_owned(),
            rows: Vec::new(),
        });
        let receipt = commit_task_end_on(&db, &end).await.unwrap();
        assert_eq!(
            receipt.info_rows,
            Some(0),
            "空快照是权威快照，不是「没快照」"
        );
        assert_eq!(receipt.model_sesno, Some(26));

        assert_eq!(info(&db, 24384).await.expect("row kept").count, Some(0));
        assert_eq!(info(&db, 24390).await.expect("row kept").count, Some(0));
        assert_eq!(
            info(&db, 16192).await.expect("other dbnum").count,
            Some(5),
            "别库的统计一格没动"
        );
    }

    #[tokio::test]
    async fn a_model_only_tail_without_a_snapshot_is_a_single_statement_that_still_stamps() {
        // `info: None` 时整笔只剩水位那一条语句——`num_statements() - 1` 的取值路径
        // 落在下标 0 上，也得取得到回执。
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let mut end = end();
        end.info = None;
        let sql = render_task_end_tail(&end);
        assert_eq!(
            sql.matches(';').count(),
            4,
            "BEGIN / write_state / UPDATE / COMMIT 四句：{sql}"
        );

        let receipt = commit_task_end_on(&db, &end).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: Some(26),
                info_rows: None,
                blocked_by_data: false,
            }
        );
        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(26));
        assert!(info(&db, 24384).await.is_none(), "没有快照就不碰 info 表");
    }

    #[tokio::test]
    async fn a_ref0_owned_by_another_dbnum_aborts_the_whole_tail() {
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26;
             CREATE dbnum_info_table:24384 SET dbnum=7997, count=5;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let error = commit_task_end_on(&db, &end()).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("Ref0 ownership conflict"),
            "{error:#}"
        );
        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, None, "整笔回滚，水位没动");
        assert_eq!(
            info(&db, 24384).await.expect("ref0 row").count,
            Some(5),
            "别库的统计一格没改"
        );
    }
}
