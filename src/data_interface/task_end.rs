//! 任务收尾的一笔控制面事务（ADR-071 D3，计划 v3 S2）。
//!
//! 一个 dbnum 任务在自己的目标 `T` 上把数据、模型两段跑完之后（房间归属随根算、随模型块
//! 同事务落边，ADR-073 D1，收尾没有房间段），把「这一版的承诺」**一次**写进库：数据水位
//! （初始化并进来的那一半）、模型水位、写入标记 `finished`、`state_revision`。事实全部来自任务
//! 自己手里的内存结果——一条都不回查库，这正是它替掉的两处的代价所在：
//!
//! | 换掉的 | 它原来的代价 |
//! |---|---|
//! | `model_watermark::complete_model_task_on` → `read_model_watermark_probe` | 每次 10 条语句、两趟 `gen_root` 全库扫（7997 实测 avg 5.5 s） |
//! | `room_completion::PreparedRoomResult::commit_on` 的水位半边 | 房间提交与水位耦在一笔 CAS 里，房间先于模型盖章（d-73） |
//!
//! Ref0 统计（`dbnum_info_table`，ADR-071 D6）已退役：RefNo 归哪个 dbnum 由
//! `refno_route::refno_dbnum` 直接算出，收尾不再序列化任何按 Ref0 的计数。
//!
//! **谁负责什么**：本模块只负责「把任务交上来的事实写成一笔事务」。事实对不对是调用方的事。
//!
//! 水位那一句仍留着 ADR-060 的三条纪律：单调 `math::max`、时刻子句排在赋值之前、
//! `WHERE (applied_sesno ?: 0) >= T` 是数据水位的第二道闸（M2）。闸挡下来不是错误，
//! 回执里 [`TaskEndReceipt::blocked_by_data`] 说明这一次没盖章。

use std::collections::BTreeMap;

use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

use super::dbnum_state::{DbnumState, WATERMARK_TABLE, escape_surql_str};

/// 本任务模型阶段的结果。房间归属开关开启时，`all_roots_ok` 里的「成功」含该根的房间
/// 归属；开关关闭时模型根不计算房间，几何持久化成功即可参与模型水位收尾。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskModelOutcome {
    /// 这一版要生成的根全部落库成功；失败时保持模型水位。
    pub all_roots_ok: bool,
    /// 永久放弃的根数（写进 `model_dead_roots`，供 `/dbnums` 观测）。
    pub dead_roots: usize,
    /// 库级目录落款（ADR-079：随 `gen_root` 退役，逐根的 `published_model_target.catalogue` 收成
    /// 每库一份）：`CATA dbnum → 会话`，「这个库的整份模型与该目录到这一版一致」。`Some` 只在
    /// 本任务有资格说这句话时给——整库重生成（初始化 / 从 0 重排）、或目录级联任务把该目录
    /// 变动波及的根全部重算成了；普通增量窗口只重算被触到的根，说不了这句，给 `None`。
    /// 写法是**合并**：只改点名的目录，其余目录的落款原样保留。`all_roots_ok` 为假时不写。
    pub catalogue_sessions: Option<BTreeMap<u32, u32>>,
    /// durable 侧的房间就绪承诺（ADR-073 D14）：`model_room_generation` 这一格怎么动。与
    /// `catalogue_sessions` 同一条纪律——只有能替整库说话的任务才 `Aligned`，`all_roots_ok` 为假时不写。
    pub room_generation: RoomGenerationStamp,
}

/// 收尾那一笔对 `dbnum_watermark.model_room_generation` 的处置（ADR-073 D14）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum RoomGenerationStamp {
    /// 本任务说不了「整库的房间边对齐到哪一代」（普通增量窗口只重算被触到的根）：那一格原样。
    #[default]
    Untouched,
    /// 算不了房间的进程（`room_membership()` 为 false）盖模型水位：水位此刻只承诺几何，durable 侧的房间就绪承诺随之清空。
    Cleared,
    /// 整库的房间边对齐到这一代常驻模型（初始化 / 整库重排 / 本任务跑过整库房间重算），随 `model_sesno` 同盖。
    Aligned(String),
}

/// 本任务数据阶段的结果（ADR-077 D7）：初始化把 `applied_sesno` 的推进并进收尾这一笔，
/// 不再在摄入一完就先推。`None` = 数据水位已由数据尾事务盖过（稳态增量窗口），这一笔不碰它。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskDataOutcome {
    /// 一次成功的全量基线解析为合法空库（ADR-021「空基线凭据」）。
    pub confirmed_empty: bool,
}

/// 一个 dbnum 任务收尾时要落的全部控制面事实。
///
/// 这一笔恒带 `dbnum_write_state:{dbnum} SET status = 'finished'`（ADR-072 D1 / W2）——全库唯一
/// 写得出 `finished` 的地方，所以任务中途不许落这一笔，否则会把还在写模型的任务提前宣告写完。
/// 任务以 `Partial` 收尾（模型失败但没崩）也照落：`finished` 说的是「本任务的写库结束了」，
/// 欠什么由两水位回答。
#[derive(Debug, Clone)]
pub(crate) struct TaskEnd {
    pub dbnum: u32,
    /// 本任务冻结的目标会话号。
    pub target_sesno: i32,
    /// `Some` = 这一笔同时把 `applied_sesno` 推到 `target_sesno`（排在写入标记与模型水位之前，
    /// 于是模型水位那句的数据水位闸在同一笔里成立）。
    pub data: Option<TaskDataOutcome>,
    /// 目标那一版的保存时刻。号对不上就别传——宁可让界面说「无记录」，
    /// 也不拿 A 的时刻标注 B（ADR-060 M1）。
    pub target_time: Option<String>,
    /// `None` = 本任务没跑模型（纯数据批次），模型水位与根观测都不动。
    pub model: Option<TaskModelOutcome>,
    /// 本任务模型规划的左端（`model_plan_base_sesno` 的返回值，ADR-092 D1）。
    ///
    /// 连续性闸（ADR-092 D2）：只有 `model_base_sesno ≤ 库上当前 model_sesno`（`NONE`
    /// 视为 0）时才盖模型水位、生成器落款与目录章，否则拒盖、任务 Partial
    /// （`model_watermark_discontinuous`），数据水位照盖。`None` 与 ≤0 等价于
    /// 「从 0 起算」——初始化 / 整库重排 / 纯数据批的旧路径逐位不变。
    pub model_base_sesno: Option<i32>,
}

impl TaskEnd {
    /// 模型阶段完成；房间归属开启时含各根房间边，关闭时只承诺模型几何。
    fn model_stamps(&self) -> bool {
        self.model.as_ref().is_some_and(|model| model.all_roots_ok)
    }
}

/// 一笔收尾事务落地之后的回执。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskEndReceipt {
    /// 盖上的模型水位；未跑模型、模型失败或前置水位不足时为 None。
    pub model_sesno: Option<i32>,
    /// 兼容字段名：数据或模型前置水位不足，整条水位更新被挡下。
    pub blocked_by_data: bool,
    /// ADR-092 D2：连续性闸拒盖——本任务的模型左端越过了库上当前 `model_sesno`，
    /// 模型水位、生成器落款、目录章都没盖（数据水位照盖）。为真时 `model_sesno`
    /// 是本笔读到的现值而不是目标值，补救走启动核对 / 稳态补偿。
    pub model_watermark_discontinuous: bool,
}

/// 一笔事务的字节上限：收尾只有几句定长语句加目录落款，正常远低于上限；超了说明调用方
/// 给错了事实，宁可当场喊出来，也不静默拆成两笔——拆开就不是一笔原子的收尾了。
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
            // ADR-092 D1/D2 连续性闸：本任务模型左端越过库上当前模型水位时拒盖
            // （`model_base_sesno <= (model_sesno ?: 0)`，NONE 视为 0）。≤0 与没带
            // 左端的旧路径恒真，渲染出的 SQL 与从前逐位相同。判据落在 SET 表达式里
            // ——同一笔事务内读判改写，不是「读后写两笔」。
            let base = end.model_base_sesno.unwrap_or(0).max(0);
            let gate = |field: &str, expr: String| -> String {
                if base > 0 {
                    format!(
                        "{field} = IF {base} <= (model_sesno ?: 0) THEN {expr} ELSE {field} END"
                    )
                } else {
                    format!("{field} = {expr}")
                }
            };
            if let Some(time) = end.target_time.as_deref() {
                sets.push(gate(
                    "model_sesno_time",
                    format!(
                        "IF {target_sesno} >= (model_sesno ?: 0) \
                         THEN '{}' ELSE model_sesno_time END",
                        escape_surql_str(time)
                    ),
                ));
            }
            sets.push(gate(
                "model_sesno",
                format!("math::max([model_sesno ?: 0, {target_sesno}])"),
            ));
            sets.push(gate("model_settled_at", "time::now()".to_owned()));
            // GitHub #73：库级生成器落款随 `model_sesno` 同一笔盖——「这个库的几何是哪一版 e3d-model 算的」。
            // 启动 / `update/execute` 拿它与本进程的指纹比，不一致整库重排（`generator_reconcile`）。
            sets.push(gate(
                "model_generator",
                format!(
                    "'{}'",
                    escape_surql_str(
                        crate::fast_model::generator_identity::generator_fingerprint()
                    )
                ),
            ));
            // 数据库架构版本（`storage_version:current`）**不在这里写**：它是整库一条记录，
            // 只在全新空库初始化完成或全部库升级完成时由 `storage_upgrade` 写。
            // 单库水位盖章不代表整个数据库已经换成新架构。
            if let Some(stamps) = model
                .catalogue_sessions
                .as_ref()
                .filter(|stamps| !stamps.is_empty())
            {
                sets.push(gate(
                    "model_catalogue_sessions",
                    catalogue_stamp_merge_expr(stamps),
                ));
            }
            // 盖章（`Aligned`）/ 清空（`Cleared`）都把「盖章之后按另一代写过根」那一格一并清掉（ADR-073 F3 落库）：
            // 整库 / 定向盖到这一代 = 库上每条边都与这一代一致，此前记的另几代已被覆盖；关着开关这一格也没有意义。
            // `Untouched` 两格都不碰。
            match &model.room_generation {
                RoomGenerationStamp::Untouched => {}
                RoomGenerationStamp::Cleared => {
                    sets.push(gate("model_room_generation", "NONE".to_owned()));
                    sets.push(gate("model_room_stray_generations", "[]".to_owned()));
                }
                RoomGenerationStamp::Aligned(generation) => {
                    sets.push(gate(
                        "model_room_generation",
                        format!("'{}'", escape_surql_str(generation)),
                    ));
                    sets.push(gate("model_room_stray_generations", "[]".to_owned()));
                }
            }
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
        "UPDATE {WATERMARK_TABLE}:{dbnum} SET {}{guard} RETURN id, model_sesno;",
        sets.join(", ")
    )
}

/// `model_catalogue_sessions` 的合并表达式（不含 `field = ` 前缀）：连续性闸（ADR-092 D2）
/// 把它包进 `IF` 时需要右值本身，[`render_catalogue_stamp_merge`] 则是整句。
fn catalogue_stamp_merge_expr(stamps: &BTreeMap<u32, u32>) -> String {
    let dbnums = stamps
        .keys()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let entries = stamps
        .iter()
        .map(|(dbnum, session)| format!("{{ dbnum: {dbnum}, session: {session} }}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "array::concat(\
         (model_catalogue_sessions ?? []).filter(|$stamp| $stamp.dbnum NOT IN [{dbnums}]), \
         [{entries}])"
    )
}

/// 库级目录落款那一句（ADR-079）：`model_catalogue_sessions` 是 `[{ dbnum, session }, …]`，
/// 合并写——先把点名的目录从旧数组里摘掉，再把新落款并进去；没点名的目录原样保留。
/// 行上没有这一列（旧行）时从空数组起。
pub(crate) fn render_catalogue_stamp_merge(stamps: &BTreeMap<u32, u32>) -> String {
    format!(
        "model_catalogue_sessions = {}",
        catalogue_stamp_merge_expr(stamps)
    )
}

/// 整笔收尾事务。
///
/// 写入标记那一句排在水位**之前**（ADR-072 D1）：它与两水位同生同死，但不带数据水位闸
/// ——闸挡下水位说的是「这一版没盖成」，而本任务的写库确实结束了。位置也有讲究：
/// [`commit_task_end_on`] 按 `num_statements() - 1` 取水位那一条的回执，水位必须是最后一句。
pub(crate) fn render_task_end_tail(end: &TaskEnd) -> String {
    // 数据水位（初始化并进来的那一半，ADR-077 D7）排最前：它是 `UPSERT`，建行；后面的模型水位
    // 那句是 `UPDATE … WHERE applied_sesno >= T`，同一笔里读到的就是刚推上去的值。
    let data = end
        .data
        .as_ref()
        .map(|data| {
            let advance = super::increment_tail::render_watermark_advance(
                end.dbnum,
                end.target_sesno,
                end.target_time.as_deref(),
            );
            let empty = super::increment_tail::render_confirmed_empty_baseline(
                end.dbnum,
                end.target_sesno,
                data.confirmed_empty,
            );
            format!("{advance}\n{empty}\n")
        })
        .unwrap_or_default();
    let finished = super::write_state::render_mark_finished(end.dbnum);
    format!(
        "BEGIN TRANSACTION;\n{data}{finished}\n{}\nCOMMIT TRANSACTION;",
        render_watermark_update(end)
    )
}

/// 落一笔收尾事务，成功后作废这个 dbnum 的镜像格（一致性普查读进程内账本，没有缓存要作废，ADR-079）。
pub(crate) async fn commit_task_end_on(
    db: &Surreal<Any>,
    end: &TaskEnd,
) -> anyhow::Result<TaskEndReceipt> {
    let touched_model_sesno =
        commit_task_end_via(end, |sql| async move { db.query(sql).await }).await?;

    DbnumState::invalidate_mirrored_on(db, end.dbnum);

    let stamped = touched_model_sesno.is_some();
    let wanted_stamp = end.model_stamps();
    let observed_model_sesno = touched_model_sesno.unwrap_or(0);
    // ADR-092 D2：连续性闸拒盖时水位句照样碰到行（数据闸已过），但 `model_sesno`
    // 留在现值没到目标——回执如实报，任务由调用方判 Partial。
    let discontinuous = stamped && wanted_stamp && observed_model_sesno < end.target_sesno;
    // 库上 `model_room_generation` 真盖上 / 真清空之后，告诉投影这一代（d-149 补丁 2026-09-22）：按根数限额逐出
    // 时，ACK 房间代号是本进程写过的 stray 代号、又不是库上这一代的根是 F3 的证据、不进候选；盖到这一代之后
    // 同代的 ACK 才放行。`Untouched` 不说话——普通增量窗口替不了整库。
    // 连续性闸拒盖（D2）时房间落款与水位的 SET 全被同一个 IF 拦下，不算「真盖上」，也不许通知。
    if stamped && wanted_stamp && !discontinuous {
        match end.model.as_ref().map(|model| &model.room_generation) {
            Some(RoomGenerationStamp::Aligned(generation)) => {
                crate::fast_model::model_memory_store::ModelMemoryStore::global()
                    .note_room_generation_stamped(end.dbnum, Some(generation));
            }
            Some(RoomGenerationStamp::Cleared) => {
                crate::fast_model::model_memory_store::ModelMemoryStore::global()
                    .note_room_generation_stamped(end.dbnum, None);
            }
            Some(RoomGenerationStamp::Untouched) | None => {}
        }
    }
    Ok(TaskEndReceipt {
        model_sesno: (stamped && wanted_stamp).then_some(observed_model_sesno),
        blocked_by_data: wanted_stamp && !stamped,
        model_watermark_discontinuous: discontinuous,
    })
}

/// 收尾那一笔的发送与回执解读，`run` 是「把这段 SQL 发出去」（生产是 `db.query`；钉用它注入一次冲突）。
/// 回水位那一句碰到的行上的 `model_sesno`（`NONE` 归一成 0；数据水位闸挡下 → `None`）。
///
/// 与刷盘那一笔同一个冲突重试（ADR-073 D13 收口总表第 10 行，2026-09-19 晚）：收尾是一笔 `BEGIN … COMMIT`，
/// 与 write-behind 刷盘的 stray epilogue 同写水位行，两台引擎（surrealkv / rocksdb）都是**谁后提交谁输**、
/// 输的那一笔整笔零提交；这里的语句都是绝对值的 UPSERT / UPDATE，重发安全。只有「read or write conflict」
/// 那句才重试（16 次、25 ms 起指数退避），`THROW` / 语法错照旧第一次就报。
async fn commit_task_end_via<F, Fut>(end: &TaskEnd, mut run: F) -> anyhow::Result<Option<i32>>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = surrealdb::Result<surrealdb::Response>>,
{
    let sql = render_task_end_tail(end);
    anyhow::ensure!(
        sql.len() <= max_tail_bytes(),
        "任务收尾事务超出字节上限（dbnum={}）：{} > {}",
        end.dbnum,
        sql.len(),
        max_tail_bytes()
    );
    #[derive(serde::Deserialize)]
    struct Touched {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
        model_sesno: Option<i32>,
    }
    let dbnum = end.dbnum;
    crate::surreal_retry::retry_surreal_write_operation(
        &format!("commit task end of dbnum {dbnum}"),
        || {
            // 每次尝试重发同一段 SQL（输的那一笔什么都没提交）。
            let attempt = run(sql.clone());
            async move {
                let mut response = attempt.await.map_err(anyhow::Error::from)?;
                // 先数再取：`take_errors` 会把出错的条目从响应里摘走。回滚掉的事务会给**出错语句
                // 之前**的每一条都报一句笼统的「事务失败」，只看第一条就永远看不到 `THROW` 的原话。
                let index = response.num_statements().saturating_sub(1);
                let mut errors = response.take_errors().into_iter().collect::<Vec<_>>();
                errors.sort_by_key(|(index, _)| *index);
                if !errors.is_empty() {
                    anyhow::bail!(
                        "任务收尾事务失败 dbnum={dbnum}: {}",
                        errors
                            .into_iter()
                            .map(|(index, error)| format!("statement {index}: {error}"))
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                }
                // 数据水位与写入标记在前，水位那一句是最后一条带回执的语句；`RETURN id, model_sesno`
                // 把落笔后行上的模型水位带回来——连续性闸拒盖时它还是旧值（ADR-092 D2）。
                let touched: Vec<Touched> = response
                    .take(index)
                    .with_context(|| format!("decode task end watermark write of dbnum {dbnum}"))?;
                Ok::<Option<i32>, anyhow::Error>(
                    touched
                        .first()
                        .map(|touched| touched.model_sesno.unwrap_or(0)),
                )
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> TaskEnd {
        TaskEnd {
            dbnum: 8000,
            target_sesno: 26,
            data: None,
            target_time: Some("2026-09-10T09:00:00+08:00".to_owned()),
            model: Some(TaskModelOutcome {
                all_roots_ok: true,
                dead_roots: 0,
                catalogue_sessions: None,
                room_generation: RoomGenerationStamp::Untouched,
            }),
            model_base_sesno: None,
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
        model_generator: Option<String>,
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
        // ADR-073 W3 / 钉 2：行上只有两枚水位，收尾不写房间那一枚的任何格（`model_room_generation`
        // 是模型水位的属性——「边对齐到哪一代」——不是第三枚水位，D14）。
        for retired in [
            "room_sesno",
            "room_settled_at",
            "room_input_version",
            "room_required_input",
            "room_result_version",
        ] {
            assert!(
                !sql.contains(retired),
                "收尾没有房间那一枚可盖（{retired}）：{sql}"
            );
        }
        assert!(
            !sql.contains("model_room_generation"),
            "说不了整库对齐的任务不碰 model_room_generation：{sql}"
        );
        assert!(
            !sql.contains("model_room_stray_generations"),
            "同一条纪律：说不了整库对齐的任务也不清「盖章之后按另一代写过根」那一格（F3 落库）：{sql}"
        );
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
        let marker = sql
            .find("dbnum_write_state:8000")
            .expect("marker statement");
        let watermark = sql
            .find("UPDATE dbnum_watermark:8000")
            .expect("watermark statement");
        assert!(
            marker < watermark,
            "水位必须是事务最后一条（回执按下标取它）：{sql}"
        );
    }

    /// 模型失败（没崩、走到了收尾）照样收标记：`finished` 说的是「本任务的写库
    /// 结束了」，欠什么由两水位回答（ADR-072 D1）。
    #[test]
    fn a_partial_task_still_closes_its_write_marker() {
        let mut end = end();
        end.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 3,
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Untouched,
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
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Aligned("gen-not-stamped".to_owned()),
        });
        let sql = render_task_end_tail(&end);
        assert!(!sql.contains("model_sesno = math::max"), "{sql}");
        assert!(!sql.contains("model_sesno_time = IF"), "{sql}");
        assert!(
            !sql.contains("model_room_generation"),
            "模型没成，房间对齐代号也不盖（D14 与 catalogue 落款同一条纪律）：{sql}"
        );
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
        let sql = render_task_end_tail(&end);
        assert!(
            sql.contains("state_revision = (state_revision ?? 0) + 1"),
            "{sql}"
        );
        assert!(!sql.contains("WHERE (applied_sesno"), "{sql}");
    }

    /// Ref0 统计已退役：收尾一句都不再写 `dbnum_info_table` / `dbnum_stats_state`。
    #[test]
    fn the_tail_writes_no_ref0_statistics() {
        let mut initialization = end();
        initialization.data = Some(TaskDataOutcome::default());
        for end in [end(), initialization] {
            let sql = render_task_end_tail(&end);
            for retired in [
                "dbnum_info_table",
                "dbnum_stats_state",
                "ownership conflict",
            ] {
                assert!(
                    !sql.contains(retired),
                    "收尾不再写 Ref0 统计（{retired}）：{sql}"
                );
            }
        }
    }

    #[test]
    fn single_quotes_in_the_target_time_cannot_escape_the_literal() {
        let mut end = end();
        end.target_time = Some("t'; THROW 'y".to_owned());
        let sql = render_task_end_tail(&end);
        assert!(sql.contains("THEN 't\\'; THROW \\'y'"), "{sql}");
    }

    /// ADR-077 D7：初始化收尾一笔——库里还没有水位行，`data` 那一半先 `UPSERT` 出 `applied_sesno`，
    /// 写入标记与模型水位跟在同一笔里，模型水位那句的数据闸在同一笔里就成立。
    #[tokio::test]
    async fn an_initialization_tail_lands_applied_model_and_finished_in_one_write() {
        let db = database().await;
        // 只有写入标记行（任务开工时落的 `writing`），没有水位行——这正是首次初始化的形状。
        super::super::write_state::mark_writing_on(
            &db,
            8000,
            0,
            26,
            super::super::write_state::WriteIntent::Initialize,
        )
        .await
        .unwrap();

        let mut init_end = end();
        init_end.data = Some(TaskDataOutcome {
            confirmed_empty: false,
        });
        let sql = render_task_end_tail(&init_end);
        let applied = sql
            .find("applied_sesno = math::max([applied_sesno?:0, 26])")
            .expect("applied advance");
        let empty = sql
            .find("confirmed_empty_baseline_sesno = NONE")
            .expect("empty credential cleared");
        let finished = sql.find("status = 'finished'").expect("marker closed");
        let model = sql
            .find("model_sesno = math::max([model_sesno ?: 0, 26])")
            .expect("model stamp");
        assert!(
            applied < empty && empty < finished && finished < model,
            "顺序：applied → 空基线凭据 → 写入标记 → 模型水位: {sql}"
        );

        let receipt = commit_task_end_on(&db, &init_end).await.unwrap();
        assert_eq!(receipt.model_sesno, Some(26), "数据闸在同一笔里成立");
        assert!(!receipt.blocked_by_data);

        #[derive(Debug, serde::Deserialize)]
        struct Row {
            applied_sesno: Option<i32>,
            model_sesno: Option<i32>,
            confirmed_empty_baseline_sesno: Option<i32>,
        }
        let row: Option<Row> = db
            .query(format!("SELECT * FROM ONLY {WATERMARK_TABLE}:8000;"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        let row = row.expect("watermark row created by the tail");
        assert_eq!(row.applied_sesno, Some(26));
        assert_eq!(row.model_sesno, Some(26));
        assert_eq!(row.confirmed_empty_baseline_sesno, None);
        let status: Option<String> = db
            .query("SELECT VALUE status FROM ONLY dbnum_write_state:8000;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(status.as_deref(), Some("finished"));

        // 空库那一档：凭据钉在这一版。
        let mut empty_end = TaskEnd {
            dbnum: 8001,
            ..end()
        };
        empty_end.model = None;
        empty_end.data = Some(TaskDataOutcome {
            confirmed_empty: true,
        });
        commit_task_end_on(&db, &empty_end).await.unwrap();
        let row: Option<Row> = db
            .query(format!("SELECT * FROM ONLY {WATERMARK_TABLE}:8001;"))
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        let row = row.expect("watermark row");
        assert_eq!(row.applied_sesno, Some(26));
        assert_eq!(row.confirmed_empty_baseline_sesno, Some(26));
        assert_eq!(row.model_sesno, None, "没跑模型就不盖模型水位");
    }

    /// ADR-079：库级目录落款随 `model_sesno` 同一笔盖，写法是合并——点名的目录换新值、其余原样留；
    /// 没点名（`None` / 空）的任务一个字不碰它；模型没成的任务也不碰。
    #[tokio::test]
    async fn catalogue_stamps_merge_into_the_watermark_row_only_when_the_model_stamps() {
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20, \
             model_catalogue_sessions=[{ dbnum: 7000, session: 11 }, { dbnum: 5052, session: 180 }];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        #[derive(Debug, serde::Deserialize, PartialEq, Eq, PartialOrd, Ord)]
        struct Stamp {
            dbnum: u32,
            session: u32,
        }
        async fn stamps_of(db: &Surreal<Any>, dbnum: u32) -> Vec<Stamp> {
            // `SELECT VALUE <数组列> FROM ONLY` 回的就是那个数组本身，按 `Vec` 取。
            let mut rows: Vec<Stamp> = db
                .query(format!(
                    "SELECT VALUE model_catalogue_sessions ?? [] FROM ONLY dbnum_watermark:{dbnum};"
                ))
                .await
                .unwrap()
                .check()
                .unwrap()
                .take::<Vec<Stamp>>(0)
                .unwrap();
            rows.sort();
            rows
        }
        async fn stamps(db: &Surreal<Any>) -> Vec<Stamp> {
            stamps_of(db, 8000).await
        }

        // 只点名 7000：换成 12，5052 原样。
        let mut stamped = end();
        stamped.model = Some(TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: Some(BTreeMap::from([(7000, 12)])),
            room_generation: RoomGenerationStamp::Untouched,
        });
        let sql = render_task_end_tail(&stamped);
        assert!(
            sql.contains("$stamp.dbnum NOT IN [7000]")
                && sql.contains("{ dbnum: 7000, session: 12 }"),
            "{sql}"
        );
        commit_task_end_on(&db, &stamped).await.unwrap();
        assert_eq!(
            stamps(&db).await,
            vec![
                Stamp {
                    dbnum: 5052,
                    session: 180
                },
                Stamp {
                    dbnum: 7000,
                    session: 12
                },
            ]
        );

        // 没点名：不碰。
        commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(stamps(&db).await.len(), 2);
        assert_eq!(stamps(&db).await[1].session, 12);

        // 模型没成：即便带着落款也不写。
        db.query("UPDATE dbnum_watermark:8000 SET model_sesno = 20;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut failed = stamped.clone();
        failed.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 1,
            catalogue_sessions: Some(BTreeMap::from([(7000, 13), (9000, 1)])),
            room_generation: RoomGenerationStamp::Untouched,
        });
        let sql = render_task_end_tail(&failed);
        assert!(!sql.contains("model_catalogue_sessions"), "{sql}");
        commit_task_end_on(&db, &failed).await.unwrap();
        assert_eq!(stamps(&db).await[1].session, 12);

        // 新目录追加：9000 进来、旧的都留着。
        let mut more = stamped.clone();
        more.model = Some(TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: Some(BTreeMap::from([(9000, 1)])),
            room_generation: RoomGenerationStamp::Untouched,
        });
        commit_task_end_on(&db, &more).await.unwrap();
        assert_eq!(
            stamps(&db).await,
            vec![
                Stamp {
                    dbnum: 5052,
                    session: 180
                },
                Stamp {
                    dbnum: 7000,
                    session: 12
                },
                Stamp {
                    dbnum: 9000,
                    session: 1
                },
            ]
        );

        // 旧行没有这一列：从空数组起。
        db.query("CREATE dbnum_watermark:8001 SET dbnum=8001, applied_sesno=26, model_sesno=20;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let fresh = TaskEnd {
            dbnum: 8001,
            ..stamped.clone()
        };
        commit_task_end_on(&db, &fresh).await.unwrap();
        assert_eq!(
            stamps_of(&db, 8001).await,
            vec![Stamp {
                dbnum: 7000,
                session: 12
            }]
        );
    }

    /// ADR-073 D14：`model_room_generation` 随 `model_sesno` 同盖、同一条纪律——能替整库说话的
    /// 任务 `Aligned` 写代号，关着开关盖章 `Cleared` 清空，普通增量窗口 `Untouched` 原样不动，
    /// 模型没成一律不写。F3 落库（2026-09-19）：「盖章之后按另一代写过根」那一格 `model_room_stray_generations`
    /// 随 `Aligned` / `Cleared` 一并清空，`Untouched` / 模型没成同样不碰。
    #[tokio::test]
    async fn the_room_generation_stamp_follows_the_model_watermark() {
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20, \
             model_room_stray_generations=['g2'];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        async fn stamp(db: &Surreal<Any>) -> Option<String> {
            db.query("SELECT VALUE model_room_generation FROM ONLY dbnum_watermark:8000;")
                .await
                .unwrap()
                .check()
                .unwrap()
                .take::<Option<String>>(0)
                .unwrap()
        }
        async fn stray(db: &Surreal<Any>) -> Vec<String> {
            db.query(
                "SELECT VALUE model_room_stray_generations ?? [] FROM ONLY dbnum_watermark:8000;",
            )
            .await
            .unwrap()
            .check()
            .unwrap()
            .take::<Vec<String>>(0)
            .unwrap()
        }

        // 普通增量窗口：两格都原样（盖章之前记下的另一代还在）。
        commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(stamp(&db).await, None);
        assert_eq!(stray(&db).await, vec!["g2".to_string()]);

        let mut aligned = end();
        aligned.model = Some(TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Aligned("efd4'ebe1".to_owned()),
        });
        let sql = render_task_end_tail(&aligned);
        assert!(
            sql.contains("model_room_generation = 'efd4\\'ebe1'"),
            "代号按字符串转义写入：{sql}"
        );
        assert!(
            sql.contains("model_room_stray_generations = []"),
            "盖章一并清掉「盖章之后按另一代写过根」那一格：{sql}"
        );
        commit_task_end_on(&db, &aligned).await.unwrap();
        assert_eq!(stamp(&db).await.as_deref(), Some("efd4'ebe1"));
        assert!(
            stray(&db).await.is_empty(),
            "盖到这一代 = 此前记的另几代都被覆盖"
        );

        // 普通增量窗口：那一格原样。
        commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(stamp(&db).await.as_deref(), Some("efd4'ebe1"));

        // 模型没成：带着代号也不写（水位那一枚同样不动）。
        let mut failed = aligned.clone();
        failed.model = Some(TaskModelOutcome {
            all_roots_ok: false,
            dead_roots: 1,
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Aligned("newer".to_owned()),
        });
        commit_task_end_on(&db, &failed).await.unwrap();
        assert_eq!(stamp(&db).await.as_deref(), Some("efd4'ebe1"));

        // 关着开关盖章：清空——模型水位此刻只承诺几何。
        let mut cleared = aligned.clone();
        cleared.model = Some(TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Cleared,
        });
        assert!(
            render_task_end_tail(&cleared).contains("model_room_generation = NONE")
                && render_task_end_tail(&cleared).contains("model_room_stray_generations = []"),
            "{}",
            render_task_end_tail(&cleared)
        );
        commit_task_end_on(&db, &cleared).await.unwrap();
        assert_eq!(stamp(&db).await, None);
        assert!(stray(&db).await.is_empty());
    }

    /// ADR-073 D13 收口总表第 10 行（2026-09-19 晚）：收尾那一笔与 write-behind 刷盘的 stray epilogue 同写水位行，
    /// 实测两台引擎都是**谁后提交谁输**、输的一方整笔零提交、错误就是 `Error::TxRetryable`（「read or write conflict …
    /// can be retried」）。这里让收尾输一次：第一次发送时竞争者先改了同一行、引擎回冲突错，第二次才真发——那一路必须
    /// 重发成功、行上是收尾的值（`Aligned` 盖的代号、清空的格、模型水位），而不是回 `Err` 让任务落 `Partial`；
    /// `THROW` 那种错第一次就报、不重发。（本进程内同一个 `mem://` 句柄上的请求是串行的，撞不出真冲突，所以冲突由
    /// `run` 注入；引擎那一侧「真会撞、撞了零提交」的证据是钉 36 记的 8009 实测。）
    #[tokio::test]
    async fn the_tail_retries_past_a_conflict_with_the_flush_epilogue_on_the_same_row() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let db = database().await;
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20, \
             model_room_generation='g0', model_room_stray_generations=['g2'];",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let mut aligned = end();
        aligned.model = Some(TaskModelOutcome {
            all_roots_ok: true,
            dead_roots: 0,
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Aligned("g1".to_owned()),
        });

        let attempts = AtomicUsize::new(0);
        let stamped = commit_task_end_via(&aligned, |sql| {
            let attempt = attempts.fetch_add(1, SeqCst);
            let db = db.clone();
            async move {
                if attempt == 0 {
                    // 竞争者（刷盘的 epilogue）赶在前面提交了同一行；引擎让收尾这一笔输，整笔零提交。
                    db.query(
                        "UPDATE dbnum_watermark:8000 SET model_room_stray_generations = \
                         array::union(model_room_stray_generations ?? [], ['g3']);",
                    )
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
                    return Err(surrealdb::Error::Db(surrealdb::error::Db::TxRetryable));
                }
                db.query(sql).await
            }
        })
        .await
        .expect("the tail must retry past the conflict instead of failing the task");
        assert_eq!(stamped, Some(26), "重发那一笔碰到了水位行并读回盖上的水位");
        assert_eq!(attempts.load(SeqCst), 2, "输一次、重发一次");

        #[derive(Debug, serde::Deserialize)]
        struct Row {
            model_sesno: Option<i32>,
            model_room_generation: Option<String>,
            model_room_stray_generations: Option<Vec<String>>,
        }
        let row = db
            .query("SELECT * FROM ONLY dbnum_watermark:8000;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take::<Option<Row>>(0)
            .unwrap()
            .expect("watermark row");
        assert_eq!(row.model_sesno, Some(26));
        assert_eq!(row.model_room_generation.as_deref(), Some("g1"));
        assert_eq!(
            row.model_room_stray_generations.unwrap_or_default(),
            Vec::<String>::new(),
            "重发在竞争者之后落：`Aligned` 把格清空，竞争者的 g2 / g3 都被盖掉"
        );

        // 不是冲突的错第一次就报，不重发。
        let attempts = AtomicUsize::new(0);
        let error = commit_task_end_via(&aligned, |_sql| {
            attempts.fetch_add(1, SeqCst);
            async {
                Err(surrealdb::Error::Db(surrealdb::error::Db::Thrown(
                    "specific-tail-cause".to_owned(),
                )))
            }
        })
        .await
        .expect_err("a non-conflict error is not retried");
        assert_eq!(attempts.load(SeqCst), 1);
        assert!(error.to_string().contains("specific-tail-cause"), "{error}");

        // 源码钉：生产那一路 `commit_task_end_on` 就是经 `commit_task_end_via` 发的，后者包在冲突重试里、每次重发同一段 SQL。
        // 本文件是 CRLF：先归一再切函数体。
        let source = include_str!("task_end.rs").replace("\r\n", "\n");
        let production = source
            .split_once("pub(crate) async fn commit_task_end_on(")
            .expect("commit_task_end_on")
            .1
            .split_once("\n}\n")
            .expect("commit_task_end_on end")
            .0;
        assert!(
            production
                .contains("commit_task_end_via(end, |sql| async move { db.query(sql).await })"),
            "{production}"
        );
        let via = source
            .split_once("async fn commit_task_end_via<")
            .expect("commit_task_end_via")
            .1
            .split_once("\n}\n")
            .expect("commit_task_end_via end")
            .0;
        assert!(
            via.contains("crate::surreal_retry::retry_surreal_write_operation(")
                && via.contains("let attempt = run(sql.clone());"),
            "收尾那一笔要包进冲突重试、每次尝试重发同一段 SQL: {via}"
        );
    }

    /// 模型水位、`state_revision`、写入标记 `finished` 同一笔落。
    #[tokio::test]
    async fn a_whole_task_tail_lands_three_facts_in_one_write() {
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, state_revision=4;")
            .await
            .unwrap()
            .check()
            .unwrap();
        super::super::write_state::mark_writing_on(
            &db,
            8000,
            0,
            26,
            super::super::write_state::WriteIntent::Initialize,
        )
        .await
        .unwrap();

        let receipt = commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: Some(26),
                blocked_by_data: false,
                model_watermark_discontinuous: false,
            }
        );

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(26));
        assert_eq!(
            row.model_sesno_time.as_deref(),
            Some("2026-09-10T09:00:00+08:00")
        );
        assert_eq!(row.state_revision, Some(5));

        let status: Option<String> = db
            .query("SELECT VALUE status FROM ONLY dbnum_write_state:8000;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            status.as_deref(),
            Some("finished"),
            "写入标记与水位同一笔关上"
        );
    }

    /// ADR-092 D2 连续性闸：模型左端越过库上当前 `model_sesno` 时，水位 / 落款 / 目录章
    /// 全部条件化拒盖；根观测与 `applied_sesno` 数据闸原样。左端 ≤ 0 与没带左端（`None`）
    /// 的旧路径渲染出的 SQL 逐位不变。
    #[test]
    fn a_task_whose_model_base_overruns_the_watermark_gets_every_stamp_gated() {
        let mut overrun = end();
        overrun.model_base_sesno = Some(30);
        let sql = render_task_end_tail(&overrun);
        let gate = "30 <= (model_sesno ?: 0)";
        assert!(
            sql.contains(&format!(
                "model_sesno = IF {gate} THEN math::max([model_sesno ?: 0, 26]) ELSE model_sesno END"
            )),
            "{sql}"
        );
        assert!(
            sql.contains(&format!("model_sesno_time = IF {gate} THEN IF 26 >=")),
            "{sql}"
        );
        assert!(
            sql.contains(&format!("model_settled_at = IF {gate} THEN time::now()")),
            "{sql}"
        );
        assert!(
            sql.contains(&format!("model_generator = IF {gate} THEN '")),
            "{sql}"
        );
        // 数据水位闸照旧、观测照常落、回执把行上的 model_sesno 带回来。
        assert!(sql.contains("WHERE (applied_sesno ?: 0) >= 26"), "{sql}");
        assert!(sql.contains("model_dead_roots = 0"), "{sql}");
        assert!(sql.contains("model_roots_observed_at = 26"), "{sql}");
        assert!(sql.contains("RETURN id, model_sesno"), "{sql}");

        // 左端为 0 与没带左端：闸恒真，SQL 与旧版逐位相同。
        for base in [None, Some(0)] {
            let mut end = end();
            end.model_base_sesno = base;
            let sql = render_task_end_tail(&end);
            assert!(
                sql.contains("model_sesno = math::max([model_sesno ?: 0, 26])"),
                "base={base:?}: {sql}"
            );
            assert!(!sql.contains("IF 0 <="), "base={base:?}: {sql}");
        }
    }

    /// ADR-092 D2/D3 端到端：左端 30 越过库上 20 → 水位与落款都没盖，回执报
    /// `model_watermark_discontinuous` 且 `model_sesno` 是读到的现值；左端 20 贴住
    /// 库上 20 → 正常盖到 26；`model_sesno` 为 `NONE` 的行按 0 判。
    #[tokio::test]
    async fn the_continuity_gate_refuses_a_skip_stamp_but_lets_contiguous_ones_through() {
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20;")
            .await
            .unwrap()
            .check()
            .unwrap();

        // 跳盖：本任务计划从左端 30 起算，库上模型水位才到 20——中间一段没人算过，拒盖。
        let mut skipping = end();
        skipping.model_base_sesno = Some(30);
        let receipt = commit_task_end_on(&db, &skipping).await.unwrap();
        assert!(receipt.model_watermark_discontinuous, "{receipt:?}");
        assert_eq!(receipt.model_sesno, Some(20), "回执带读到的现值");
        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(20), "水位原地");
        assert_eq!(row.model_generator, None, "落款随水位一起没盖");
        assert_eq!(row.model_roots_observed_at, Some(26), "观测照常落");

        // 连续：左端贴住库上水位（追平窗），正常盖到目标。
        let mut contiguous = end();
        contiguous.model_base_sesno = Some(20);
        let receipt = commit_task_end_on(&db, &contiguous).await.unwrap();
        assert!(!receipt.model_watermark_discontinuous, "{receipt:?}");
        assert_eq!(receipt.model_sesno, Some(26));
        assert_eq!(watermark(&db, 8000).await.model_sesno, Some(26));

        // `NONE` 视 0：左端 0 / 没带的任务照常盖。
        db.query("CREATE dbnum_watermark:8001 SET dbnum=8001, applied_sesno=26;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let mut zero_based = TaskEnd {
            dbnum: 8001,
            model_base_sesno: Some(0),
            ..end()
        };
        let receipt = commit_task_end_on(&db, &zero_based).await.unwrap();
        assert_eq!(receipt.model_sesno, Some(26));
        assert_eq!(watermark(&db, 8001).await.model_sesno, Some(26));

        // 「左端越过库上水位」与「欠没欠」分开判：水位拨回 10、左端 15 → 拒盖；
        // 左端改成 10 贴住库上水位 → 闸放行，正常盖到 26。
        db.query("UPDATE dbnum_watermark:8001 SET model_sesno = 10;")
            .await
            .unwrap()
            .check()
            .unwrap();
        zero_based.model_base_sesno = Some(15);
        let receipt = commit_task_end_on(&db, &zero_based).await.unwrap();
        assert!(receipt.model_watermark_discontinuous, "{receipt:?}");
        assert_eq!(watermark(&db, 8001).await.model_sesno, Some(10));
        zero_based.model_base_sesno = Some(10);
        let receipt = commit_task_end_on(&db, &zero_based).await.unwrap();
        assert!(!receipt.model_watermark_discontinuous, "{receipt:?}");
        assert_eq!(watermark(&db, 8001).await.model_sesno, Some(26));
    }

    /// GitHub #73：库级生成器落款随 `model_sesno` 同一笔盖，写的就是本进程的指纹（逐字相同，启动核对直接比）；
    /// 模型没成一个字不写。
    #[test]
    fn the_model_stamp_carries_the_generator_fingerprint() {
        let sql = render_task_end_tail(&end());
        let fingerprint = crate::fast_model::generator_identity::generator_fingerprint();
        assert!(
            sql.contains(&format!("model_generator = '{fingerprint}'")),
            "{sql}"
        );
        let stamp = sql.find("model_sesno = math::max").expect("model stamp");
        let generator = sql.find("model_generator = '").expect("generator stamp");
        assert!(
            stamp < generator,
            "落款跟在水位之后、同一句 UPDATE 里: {sql}"
        );

        let mut failed = end();
        failed.model.as_mut().expect("model outcome").all_roots_ok = false;
        let sql = render_task_end_tail(&failed);
        assert!(!sql.contains("model_generator"), "模型没成不盖落款: {sql}");
    }

    /// ADR-076 D5（增量路计划 S2）：增量任务收尾只有这一笔——`model_sesno` 与写入标记 `finished`
    /// 落在**同一个** `state_revision` 上，`state_revision` 一个任务只前进一格。
    #[tokio::test]
    async fn model_sesno_and_finished_land_in_one_state_revision() {
        let db = database().await;
        super::super::write_state::mark_writing_on(
            &db,
            8000,
            25,
            26,
            super::super::write_state::WriteIntent::ApplyWindow,
        )
        .await
        .unwrap();
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=25, state_revision=7;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let receipt = commit_task_end_on(&db, &end()).await.unwrap();
        assert_eq!(receipt.model_sesno, Some(26));

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(26), "模型水位盖上");
        assert_eq!(
            row.state_revision,
            Some(8),
            "一个任务只前进一格 state_revision"
        );
        let status: Option<String> = db
            .query("SELECT VALUE status FROM ONLY dbnum_write_state:8000;")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(status.as_deref(), Some("finished"), "写入标记同一笔收");
    }

    #[tokio::test]
    async fn the_data_gate_holds_back_the_model_watermark() {
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
            catalogue_sessions: None,
            room_generation: RoomGenerationStamp::Untouched,
        });
        let receipt = commit_task_end_on(&db, &end).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: None,
                blocked_by_data: false,
                model_watermark_discontinuous: false,
            }
        );

        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(20), "模型没成，水位原地");
        assert_eq!(row.model_dead_roots, Some(1), "观测照记");
        assert_eq!(row.model_roots_observed_at, Some(26));
        assert_eq!(row.state_revision, Some(1));
    }

    #[tokio::test]
    async fn a_model_only_tail_is_the_marker_and_the_watermark_and_still_stamps() {
        // 没有数据水位那一半时整笔只剩写入标记与水位两句——`num_statements() - 1` 的取值路径
        // 也得取得到回执。
        let db = database().await;
        db.query("CREATE dbnum_watermark:8000 SET dbnum=8000, applied_sesno=26, model_sesno=20;")
            .await
            .unwrap()
            .check()
            .unwrap();

        let end = end();
        let sql = render_task_end_tail(&end);
        // 2026-10-28：`model_generator` 那一格是 `;` 分隔的 capability 清单（`route-…;catalogue-…;…`），
        // 裸数 `';'` 会被它带偏（实测 23 个）。判据改成「语句分隔符」——`;` 后面紧跟换行或串尾。
        // 这一笔现在是四句：BEGIN / write_state / UPDATE 水位 / COMMIT。版本记录已迁到升级流程。
        let statements = sql
            .match_indices(';')
            .filter(|(i, _)| {
                sql[i + 1..]
                    .chars()
                    .next()
                    .is_none_or(|c| c == '\n' || c == '\r')
            })
            .count();
        assert_eq!(
            statements, 4,
            "BEGIN / write_state / UPDATE / COMMIT 四句：{sql}"
        );
        assert!(
            !sql.contains("storage_version"),
            "task end 不再写数据库架构版本：{sql}"
        );

        let receipt = commit_task_end_on(&db, &end).await.unwrap();
        assert_eq!(
            receipt,
            TaskEndReceipt {
                model_sesno: Some(26),
                blocked_by_data: false,
                model_watermark_discontinuous: false,
            }
        );
        let row = watermark(&db, 8000).await;
        assert_eq!(row.model_sesno, Some(26));
    }
}
