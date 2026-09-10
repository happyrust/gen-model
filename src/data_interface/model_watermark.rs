//! 模型水位（ADR-060）：一趟探针、Task 收尾盖章、启动与运行期对账。
//!
//! ADR-065 D2 把「欠什么」收敛成两枚水位；D3 判 `model_update_pending` 整体退役。
//! 这两条并不矛盾——退的是那张队列表，水位是留下来的那一半。它们此前同住一个文件，
//! 于是「退表」与「退模块」分不开；S5a（计划 §4，缺口 G1）把水位这一半搬出来，
//! T5 删表时才不至于连着水位一起删。
//!
//! **名字与行为不变**，只换住址：调用点（`batch_worker` 的 Task 收尾、`model_completion`、
//! `dbnum_consistency`、`side_effect_pending`、`model_publish_queue`）一律写
//! `model_watermark::read_model_watermark_probe` / `::complete_model_task_on`，
//! `model_update_pending` 不再再导出本模块。
//!
//! 探针仍要读 `model_update_pending` 的死信行（`attempts >= MAX_ATTEMPTS`）算「已放弃」
//! 那一格——那是 ADR-060 D5/M6 现行的判据，随 T5 退表一并改源（D7），不在本次搬家之内。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::SUL_DB;
use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_pending::{MAX_ATTEMPTS, TABLE, root_identity_key};

/// 一次模型水位推进要问的四件事，一趟问完（ADR-060 D5）。
///
/// 为什么不直接把窗口右端 `T` 写上去：**这一组根有没有真的追平到 T，队列自己说了不算。**
/// 一个这一窗没被触到的根，`gen_root` 行上仍是上一版的 `Generated` + 旧 `source_end_sesno`
/// ——它「安定」，却并没有追到 T。凭证前移正常时它会被推到 T，前移退化时不会
/// （`credential_advance_degraded`，ADR-056 实施约束 5）。所以能承诺的上界是**安定凭证的
/// 下限**，不是队列刚跑完的那个 T；两者相等时才写得出 T 来。
///
/// 四条语句各自回答一格：
///
/// 1. **拦路的根**——未安定（状态不在 [`SETTLED_ROOT_STATUSES`] 或凭证 ≤ 0）且不是
///    `http_422`。`http_422` 是「这个 noun 与数据永远解析不出合法生成根」的永久判定
///    （`fn::gen_root_terminal_status` 已经这么认），重试多少次都一样，算进拦路就是让
///    一个修不好的根把整库的水位永久钉死。
/// 2. **`http_422` 的根**——上一条排除掉的那些，进「已放弃」这一格。
/// 3. **死信根**——`model_update_pending` 里 attempts 撞满 [`MAX_ATTEMPTS`] 的行。同样进
///    「已放弃」，也同样从拦路里减掉。它是进程本地账（ADR-050），重启后这一格会重新观察。
/// 4. **安定凭证下限与应用水位**——候选值 `min(下限, applied_sesno)` 的两个输入。
///    封顶那一下就是 M2：模型水位永不超过数据水位。
/// 5. **行上已经记着的模型水位，以及这个库到底有没有 `gen_root` 行**——只有对账
///    （D6/D7）要用：前者是被比的那一端，后者把「幽灵模型水位」与「普通的失去支撑」
///    分开。两件现场的处置措辞不同，混成一句就查不出是哪一种。
///
/// 热路径与对账共用这一份取数：判据只有一处，漂不了。
pub(crate) fn settled_root_predicate() -> String {
    let settled = crate::data_interface::window_root_plan::SETTLED_ROOT_STATUSES
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    // Keep this predicate byte-for-byte equivalent in meaning to
    // `window_root_plan::root_is_settled` and the credential-advance guard.
    // A publication timeout may be followed by a database ACK: the geometry
    // and receipt can be present while the producer has conservatively marked
    // the root `stale`.  Treating that row as settled lets the library
    // watermark advance even though the next window is forbidden to advance
    // the same root's credential.  The contradiction only becomes visible on
    // the following increment, where the floor remains on the old session.
    format!(
        "(status ?: '') IN [{settled}] AND (source_end_sesno ?: 0) > 0 \
         AND (publication_status ?: 'ready') = 'ready' \
         AND (desired_revision ?: 0) = (published_revision ?: 0)"
    )
}

/// 仍在追赶、尚未形成可用发布凭证的根。永久拒绝的 `http_422` 单列为已放弃，
/// 死信则在读取后按规范化根键从这一集合中扣除（ADR-060 D5/M6）。
pub(crate) fn blocking_root_predicate() -> String {
    format!(
        "NOT ({}) AND (status ?: '') != 'http_422'",
        settled_root_predicate()
    )
}

pub(crate) fn render_model_watermark_probe(dbnum: u32) -> String {
    let settled = settled_root_predicate();
    let blocking = blocking_root_predicate();
    let projection = crate::data_interface::model_completion::projection_presence_expr(dbnum);
    format!(
        "SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} \
         AND {blocking};\n\
         SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} \
         AND (status ?: '') = 'http_422';\n\
         SELECT VALUE target_refno FROM {TABLE} WHERE dbnum = {dbnum} \
         AND (attempts ?: 0) >= {MAX_ATTEMPTS} \
         AND action NOT IN ['room_recalc_element','room_recalc_panel'];\n\
         RETURN math::min((SELECT VALUE source_end_sesno FROM gen_root \
         WHERE dbnum = {dbnum} AND {settled}));\n\
         RETURN dbnum_watermark:{dbnum}.applied_sesno;\n\
         RETURN dbnum_watermark:{dbnum}.model_sesno;\n\
         SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} LIMIT 1;\n\
         RETURN array::len((SELECT VALUE id FROM pe WHERE dbnum = {dbnum} LIMIT 1)) > 0;\n\
         RETURN IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) > 0 \
         THEN true ELSE ({projection}) END;\n\
         SELECT model_completion FROM dbnum_watermark:{dbnum} \
         WHERE (model_completion_sesno ?: -1) = (model_sesno ?: 0);"
    )
}

/// 一次探针读回来的整幅现场（ADR-060 D5/D6 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelWatermarkProbe {
    /// 还在赶的根：未安定，且不属于已放弃那一格。它们拦住水位。
    pub blockers: Vec<String>,
    /// 已放弃的根（`http_422` 与死信合并去重）。严格完成水位下仍然拦水位，
    /// 但与正在执行的根分开观测。
    pub abandoned: usize,
    /// 安定凭证的下限；一条安定行都没有时是 `None`。
    pub settled_floor: Option<i32>,
    pub applied_sesno: i32,
    /// 行上已经记着的模型水位。
    pub recorded: i32,
    /// 这个库有没有 `gen_root` 行。零行 + 无模型投影 + 水位非零 = 幽灵模型水位（D7）。
    pub has_roots: bool,
    /// 这个库有没有已持久化属性。历史兼容初始化必须同时看到属性与模型。
    pub has_attributes: bool,
    /// 这个库有没有已持久化模型投影。历史库可能有投影但尚无 `gen_root` 凭证。
    pub has_model_projection: bool,
    pub completed_sesno: Option<i32>,
    pub completion_present: bool,
}

impl ModelWatermarkProbe {
    /// 这一刻**有证据支撑**的模型水位（ADR-060 D6 的 `supported`）。
    ///
    /// 还有根在赶就是 0：那说明这个库整体没追平到任何一版，行上记的不管是多少都
    /// 没有支撑。没有根在赶时，上界是安定凭证的下限，再被应用水位封顶（M2）。
    pub fn supported(&self) -> i32 {
        if !self.blockers.is_empty() || self.abandoned > 0 {
            return 0;
        }
        if self.completion_present && self.completed_sesno.is_none() {
            return 0;
        }
        self.completed_sesno
            .or(self.settled_floor)
            .or_else(|| {
                (!self.has_roots && self.has_attributes && self.has_model_projection)
                    .then_some(self.applied_sesno)
            })
            .unwrap_or_default()
            .min(self.applied_sesno)
            .max(0)
    }

    /// 兼容接管的历史库（ADR-060 D10，用户 2026-09-10 拍板「模型水位 + 房间水位默认按数据
    /// 水位保持一致」）：新管线一行 `gen_root` 都没写过，库里却有属性与旧管线的模型投影，
    /// 而模型水位已被 `dbnum_state::seed_legacy_model_watermarks_on` 播成数据水位。
    ///
    /// 它不是「落后」，也没有凭证可补——排恢复 Task 只会把整库模型从 0 重算一遍，而
    /// `complete_model_task_on` 对「无根却有投影」的库永远盖不了章。启动核对认出它就跳过；
    /// 数据一旦前移（`recorded < applied`）它就不再是这一格，照常走窗口。
    pub fn legacy_adopted(&self) -> bool {
        !self.has_roots
            && self.has_attributes
            && self.has_model_projection
            && self.recorded > 0
            && self.recorded >= self.applied_sesno
    }
}

pub(crate) async fn read_model_watermark_probe(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<ModelWatermarkProbe> {
    let started = std::time::Instant::now();
    let mut response = db
        .query(render_model_watermark_probe(dbnum))
        .await
        .with_context(|| format!("probe model watermark of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("model watermark probe statement of dbnum {dbnum}"))?;
    let unsettled: Vec<String> = response
        .take(0)
        .with_context(|| format!("decode unsettled gen_root ids of dbnum {dbnum}"))?;
    let refused: Vec<String> = response
        .take(1)
        .with_context(|| format!("decode refused gen_root ids of dbnum {dbnum}"))?;
    let dead_letters: Vec<String> = response
        .take(2)
        .with_context(|| format!("decode dead-lettered model work of dbnum {dbnum}"))?;
    let settled_floor: Option<i32> = response
        .take(3)
        .with_context(|| format!("decode settled credential floor of dbnum {dbnum}"))?;
    let applied_sesno: Option<i32> = response
        .take(4)
        .with_context(|| format!("decode applied watermark of dbnum {dbnum}"))?;
    let recorded: Option<i32> = response
        .take(5)
        .with_context(|| format!("decode recorded model watermark of dbnum {dbnum}"))?;
    let any_root: Vec<String> = response
        .take(6)
        .with_context(|| format!("decode gen_root presence of dbnum {dbnum}"))?;
    let has_attributes: Option<bool> = response
        .take(7)
        .with_context(|| format!("decode attribute presence of dbnum {dbnum}"))?;
    let has_model_projection: Option<bool> = response
        .take(8)
        .with_context(|| format!("decode model projection presence of dbnum {dbnum}"))?;
    #[derive(serde::Deserialize)]
    struct CompletionRow {
        model_completion: Option<String>,
    }
    // 收据住在水位行上（`model_completion::receipt_row`），`WHERE` 已经把「这张收据是
    // 给当前 `model_sesno` 开的」筛过一道；原先那道「收据 source_token == 轮次
    // commit_token」的交叉校验随账本一起退——收据不再寄居在别人的行上。
    let receipt_rows: Vec<CompletionRow> = response.take(9)?;
    let receipt_row = receipt_rows.first();
    let raw_receipt = receipt_row.and_then(|row| row.model_completion.as_deref());
    let completed_sesno = crate::data_interface::model_completion::validated_receipt(
        raw_receipt,
        dbnum,
        recorded.unwrap_or_default(),
    )
    .filter(|receipt| {
        !receipt.empty || (any_root.is_empty() && !has_model_projection.unwrap_or(false))
    })
    .map(|_| recorded.unwrap_or_default());
    println!(
        "[model-query] at={} query=watermark_probe dbnum={dbnum} elapsed_ms={} rows={} completion={:?}",
        chrono::Utc::now().to_rfc3339(),
        started.elapsed().as_millis(),
        unsettled.len() + refused.len() + dead_letters.len() + any_root.len() + receipt_rows.len(),
        completed_sesno
    );

    // 两张表两种拼法（`gen_root` 的记录 id 是 `24381_1`，队列行的 `target_refno` 是
    // `24381/1`），比之前都过同一道归一——直接比字符串一次都不会相等。
    let abandoned: BTreeSet<String> = refused
        .iter()
        .chain(dead_letters.iter())
        .map(|root| root_identity_key(root))
        .collect();
    let blockers = unsettled
        .into_iter()
        .filter(|root| !abandoned.contains(&root_identity_key(root)))
        .collect();
    Ok(ModelWatermarkProbe {
        blockers,
        abandoned: abandoned.len(),
        settled_floor,
        applied_sesno: applied_sesno.unwrap_or_default(),
        recorded: recorded.unwrap_or_default(),
        has_roots: !any_root.is_empty(),
        has_attributes: has_attributes.unwrap_or(false),
        has_model_projection: has_model_projection.unwrap_or(false),
        completed_sesno,
        completion_present: raw_receipt.is_some(),
    })
}

/// 把模型水位推到 `model_sesno`（ADR-060 D1/D5）。
///
/// 与 [`render_watermark_advance`] 逐条对称，两处的坑也是同一个：
///
/// 1. **单调**：`math::max`，永不回退。真要下调只有 D6 的对账那一条路，而且要出声。
/// 2. **时刻跟着序号那条单调条件走，且必须排在覆盖之前**（SurrealDB 的 `SET` 顺序求值，
///    排在后面读到的是刚写完的新值）。调用方只有在候选值恰好等于窗口右端时才把时刻传进来
///    ——号对不上就整条子句不写，宁可让界面说「无记录」，也不拿 A 的时刻标注 B。
/// 3. **`WHERE` 那一句是 M2 的第二道闸**。候选值在 Rust 侧已经被 `applied_sesno` 封过顶，
///    但探针与写入之间数据面还在跑（两条链本就异步，ADR-060 M7），这里再挡一次：
///    数据水位没到，这一句就一行都不改。
///
/// 用 `UPDATE` 而不是 `UPSERT`：没有数据水位行就谈不上「与数据一致到哪一版」，
/// 凭空造一行出来只会造出一个没有对手的水位。
pub(crate) fn render_model_watermark_advance(
    dbnum: u32,
    model_sesno: i32,
    model_sesno_time: Option<&str>,
    dead_roots: usize,
    observed_at: i32,
) -> String {
    let time_clause = model_sesno_time
        .map(|time| {
            format!(
                "model_sesno_time = IF {model_sesno} >= (model_sesno ?: 0) \
                 THEN '{}' ELSE model_sesno_time END, ",
                escape_surql_str(time)
            )
        })
        .unwrap_or_default();
    format!(
        "UPDATE dbnum_watermark:{dbnum} SET \
         {time_clause}\
         model_sesno = math::max([model_sesno?:0, {model_sesno}]), \
         model_dead_roots = {dead_roots}, \
         model_roots_observed_at = {observed_at}, \
         state_revision = (state_revision ?? 0) + 1, \
         model_settled_at = time::now(), updated_at = time::now() \
         WHERE (applied_sesno ?: 0) >= {model_sesno} RETURN id;"
    )
}

/// 一次模型水位推进的去向。四个变体各自对应一种现场，**不许合并**——
/// 「被根拦住」和「跑到数据前面去了」是完全不同的两件事，混成一句就查不出是哪一种。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelWatermarkOutcome {
    /// 水位推到了 `sesno`。
    Advanced { sesno: i32 },
    /// 已经在这个位置或更高，本次没动。
    Unchanged { sesno: i32 },
    /// 还有根没安定（含单独计数的已放弃根）。`blockers` 是仍在赶的抽样，
    /// `total` 是两类未完整根的全量。
    HeldByRoots { total: usize, blockers: Vec<String> },
    /// 模型跑到了数据前面：候选 `candidate` 高于 `applied`，本次不写（M2）。
    /// **只闸水位不闸生成**——生成早就跑完了，这里只是不盖章。
    AheadOfData { candidate: i32, applied: i32 },
}

/// Durable stage evidence owned by one dbnum Task at its frozen target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelTaskCompletion {
    pub target_sesno: i32,
    pub file_round: crate::data_interface::side_effect_pending::CompletedFileRound,
    pub inputs: Option<crate::data_interface::model_completion::CompletionReceipt>,
    /// 本任务这一版的模型结果（ADR-071 D3）。成败与放弃根数由任务自己的单元清单给出，
    /// 不再由 `read_model_watermark_probe` 回查 `gen_root` 反推。
    pub model: crate::data_interface::task_end::TaskModelOutcome,
    /// 本任务这一版落库的根数。`0` = 权威的空模型清单，走 `receipt.empty` 那条。
    pub roots: usize,
}

/// 任务收尾第一步：落完成收据（`model_completion::commit_on`）。
///
/// **只落收据，不盖水位**——三水位由 [`crate::data_interface::task_end::commit_task_end_on`]
/// 在房间那一段之后一笔盖上（ADR-065 G2 (i) 的顺序：模型行 → 房间 → 收尾）。收据先落是
/// 因为它那四道闸问的是「整库都追平到 T 了吗」，那是房间算之前就该确认的前提。
///
/// **不再跑 `read_model_watermark_probe`**（ADR-071 D3，2026-09-10）：它问的三件事
/// 任务自己都知道——「还有根在赶吗」= 本任务单元清单的成败、「凭证下限够不够到 T」=
/// 本任务刚把这一版每一根都落了库、「这库是不是空模型」= 本任务规划出来的根数。
/// 7997 实测那一问要 5.5 s（10 条语句、两趟 `gen_root` 全库扫），换来的只是任务
/// 已经握在手里的事实。整库口径的那一问（有没有别的轮次留下的未安定根）没丢，
/// 它在 `model_completion::commit_on` 的事务里，一条 `LIMIT 1` 的证据闸。
pub(crate) async fn complete_model_task_on(
    db: &Surreal<Any>,
    dbnum: u32,
    completion: ModelTaskCompletion,
) -> anyhow::Result<ModelWatermarkOutcome> {
    anyhow::ensure!(
        completion.file_round.dbnum() == dbnum,
        "file round completion belongs to dbnum {} rather than {dbnum}",
        completion.file_round.dbnum()
    );
    anyhow::ensure!(
        completion.file_round.end_sesno() == completion.target_sesno,
        "file round completion target {} differs from task target {}",
        completion.file_round.end_sesno(),
        completion.target_sesno
    );
    let target = completion.target_sesno;
    if !completion.model.all_roots_ok {
        // 有根没成是「还有根没安定」那个现场，不是错误——四个变体不许合并
        // （见 [`ModelWatermarkOutcome`]）。数量以任务自己数的放弃根数为准、至少 1；
        // 点名样本在任务回执 `units` 里，这里不重复。
        return Ok(ModelWatermarkOutcome::HeldByRoots {
            total: completion.model.dead_roots.max(1),
            blockers: Vec::new(),
        });
    }
    let mut empty = false;
    let token = completion
        .file_round
        .commit_token
        .as_deref()
        .context("file round has no authoritative source token")?;
    let mut receipt = completion
        .inputs
        .context("task has no frozen completion inputs")?;
    anyhow::ensure!(
        receipt.source_token() == token,
        "completion inputs differ from published source"
    );
    if completion.roots == 0 {
        let roots =
            crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, target as u32)
                .await?;
        empty = roots.is_empty();
    }
    receipt.empty = empty;
    receipt.validate(dbnum, target)?;
    if let Err(error) =
        crate::data_interface::model_completion::commit_on(db, dbnum, target, &receipt).await
    {
        // 收据被闸挡下：是闸就按闸分型成收尾去向，不是闸（连接 / 语法）才原样上抛。
        use crate::data_interface::model_completion::{rejection, rejection_of};
        return match rejection_of(&error) {
            Some(rejection::UNSETTLED_ROOTS) | Some(rejection::ROOT_CREDENTIALS_LAG) => {
                // 整库口径：别的轮次留下的未安定根，或未触根凭证前移退化——都是「被根拦住」。
                // 只在这条已经失败的路上才多问一句库，热路径不付这笔。
                let (total, blockers) = sample_held_roots_on(db, dbnum, target).await?;
                Ok(ModelWatermarkOutcome::HeldByRoots { total, blockers })
            }
            Some(rejection::DATA_WATERMARK_BEHIND) => {
                let applied: Option<i32> = db
                    .query(format!("RETURN dbnum_watermark:{dbnum}.applied_sesno;"))
                    .await?
                    .check()?
                    .take(0)?;
                Ok(ModelWatermarkOutcome::AheadOfData {
                    candidate: target,
                    applied: applied.unwrap_or_default(),
                })
            }
            _ => Err(error),
        };
    }
    println!(
        "[model-completion] dbnum={dbnum} target={target} kind={} completed=true",
        if empty {
            "verified_empty"
        } else {
            "settled_roots"
        }
    );
    Ok(ModelWatermarkOutcome::Advanced { sesno: target })
}

/// 拦路根抽样上限：日志里点几个名字够定位，全量只报数。
const MODEL_WATERMARK_BLOCKER_SAMPLE: usize = 5;

/// 收据被 `UNSETTLED_ROOTS` / `ROOT_CREDENTIALS_LAG` 挡下之后，把「是哪些根」问清楚：
/// 全量只报数，点名抽 [`MODEL_WATERMARK_BLOCKER_SAMPLE`] 个。谓词与 `commit_on` 那两道闸的
/// 并集逐字同口径（未安定 ∨ 凭证落在 T 之前）。**只在失败路上调**——成功路一句库都不问。
async fn sample_held_roots_on(
    db: &Surreal<Any>,
    dbnum: u32,
    target: i32,
) -> anyhow::Result<(usize, Vec<String>)> {
    let settled = settled_root_predicate();
    let held = format!("(NOT ({settled}) OR (source_end_sesno ?: 0) < {target})");
    let mut response = db
        .query(format!(
            "SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} AND {held} \
             LIMIT {MODEL_WATERMARK_BLOCKER_SAMPLE};\n\
             RETURN array::len((SELECT VALUE id FROM gen_root WHERE dbnum = {dbnum} AND {held}));"
        ))
        .await
        .with_context(|| format!("sample held roots of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("sample held roots statement of dbnum {dbnum}"))?;
    let blockers: Vec<String> = response.take(0)?;
    let total: Option<usize> = response.take(1)?;
    Ok((total.unwrap_or_default().max(blockers.len()), blockers))
}

/// 在 `db` 上按探针结果推进一次模型水位（ADR-060 D5 的执行体）。
///
/// `window_end_sesno` / `window_end_time` 是刚收口那一窗的右端与它的保存时刻，**只用来
/// 决定要不要贴时刻**（候选值恰好等于它才贴），不直接当水位值——理由见
/// [`render_model_watermark_probe`]。
pub(crate) async fn advance_model_watermark_on(
    db: &Surreal<Any>,
    dbnum: u32,
    window_end_sesno: i32,
    window_end_time: Option<&str>,
) -> anyhow::Result<ModelWatermarkOutcome> {
    let probe = read_model_watermark_probe(db, dbnum).await?;
    if !probe.blockers.is_empty() || probe.abandoned > 0 {
        return Ok(ModelWatermarkOutcome::HeldByRoots {
            total: probe.blockers.len() + probe.abandoned,
            blockers: probe
                .blockers
                .into_iter()
                .take(MODEL_WATERMARK_BLOCKER_SAMPLE)
                .collect(),
        });
    }

    // 候选是**数据水位**，不再是 `min(根凭证)`（共识 d-427，2026-09-10）。
    //
    // min 那条定义要求「抬一个库级承诺必须先把每一根凭证各抬一遍」，而担这活的
    // ADR-056 P2-2 凭证前移已经退役——min 从此永远停在最老那根。现场取证：7997
    // 的 `applied_sesno` 已经 168，八行 `gen_root` 里六行凭证还是 165，`model_sesno`
    // 就卡死在 165，差额只会越拉越大。
    //
    // 「还有根在赶就不许承诺」那道保守没有丢：上面 `blockers` / `abandoned` 那道闸
    // 继续担着，它直接数未安定根，不经过凭证。这里只负责「没人在赶时，承诺追平到
    // 数据这一版」。零根且零投影的库仍然没有可承诺的追平点（D7 幽灵水位），照旧空转。
    // 凭证下限不再决定候选，但它保住了原来的**反常检测**那一半：安定凭证的下限都比
    // 数据还新，说明文件被换回了更旧的版本（M2 的反面）。照旧报 `AheadOfData`、不写
    // 水位——清凭证是重建批次（ADR-021）的活，不在这里猜。
    if let Some(floor) = probe.settled_floor
        && floor > probe.applied_sesno
    {
        return Ok(ModelWatermarkOutcome::AheadOfData {
            candidate: floor,
            applied: probe.applied_sesno,
        });
    }
    let candidate = if probe.has_roots || probe.has_model_projection {
        probe.applied_sesno
    } else {
        0
    };
    if candidate <= 0 {
        return Ok(ModelWatermarkOutcome::Unchanged { sesno: 0 });
    }

    // 号对上了才贴时刻（M1 的时刻纪律）。
    let time = (candidate == window_end_sesno)
        .then_some(window_end_time)
        .flatten();
    let statement =
        render_model_watermark_advance(dbnum, candidate, time, probe.abandoned, window_end_sesno);
    let mut response = db
        .query(statement)
        .await
        .with_context(|| format!("advance model watermark of dbnum {dbnum} to {candidate}"))?
        .check()
        .with_context(|| format!("model watermark statement of dbnum {dbnum}"))?;
    #[derive(serde::Deserialize)]
    struct Touched {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
    }
    let touched: Vec<Touched> = response
        .take(0)
        .with_context(|| format!("decode model watermark write of dbnum {dbnum}"))?;
    if touched.is_empty() {
        // `WHERE` 挡下了（数据水位在探针之后掉队），或水位行不在。两条都不是错误：
        // 下一次收口或启动对账会再来一遍。
        return Ok(ModelWatermarkOutcome::AheadOfData {
            candidate,
            applied: probe.applied_sesno,
        });
    }
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);
    Ok(ModelWatermarkOutcome::Advanced { sesno: candidate })
}

/// 把模型水位**绝对**写成对账算出来的支撑值（ADR-060 D6）。
///
/// 与 [`render_model_watermark_advance`] 是两种写法，故意不合并：
/// 热路径是 `math::max` 的**承诺**，对账是 `=` 的**更正**。更正必须能往下写——
/// 「水位失去支撑」的处置就是把它降回去。合并成一个 `math::max` 的话，降不下来的那一半
/// 静默失效，而失效的表现是一枚永远偏高的水位。
///
/// **改了就把时刻抹掉。** 行上存的那个时刻描述的是**旧的**那个号；新号不管是升是降都
/// 与它对不上，留着就是拿 A 的时刻标注 B。界面降级成「无记录」是对的，下一次热路径
/// 盖章会重新写上。
pub(crate) fn render_model_watermark_reconcile(
    dbnum: u32,
    supported: i32,
    dead_roots: usize,
) -> String {
    format!(
        "UPDATE dbnum_watermark:{dbnum} SET \
         model_sesno = {supported}, model_sesno_time = NONE, \
         model_dead_roots = {dead_roots}, \
         state_revision = (state_revision ?? 0) + 1, \
         model_settled_at = time::now(), updated_at = time::now() RETURN id;"
    )
}

/// 一次对账的去向（ADR-060 D6/D7）。四种现场各有各的措辞，**不许合并**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelWatermarkReconcile {
    /// 行上记的与证据一致，什么都没动。
    Aligned { sesno: i32 },
    /// 水位比证据高：失去支撑，降回去（D6 的下调）。
    LoweredUnbacked { from: i32, to: i32 },
    /// 根凭证能支撑更高版本，但普通探针没有资格上调完成水位。启动恢复必须排 Task，
    /// 由 Task 在发布、空间与房间全部完成后盖章。
    AwaitingTaskCompletion { recorded: i32, supported: i32 },
    /// 水位说模型追平过，这个库却既无 `gen_root` 也无模型投影：幽灵模型水位（D7）。
    ClearedGhost { from: i32 },
}

/// 在 `db` 上核验一个库的模型水位。普通探针只保持或下调；上调只允许由 Task 收尾执行。
pub(crate) async fn reconcile_model_watermark_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<ModelWatermarkReconcile> {
    let probe = read_model_watermark_probe(db, dbnum).await?;
    reconcile_model_watermark_from_probe(db, dbnum, &probe).await
}

async fn reconcile_model_watermark_from_probe(
    db: &Surreal<Any>,
    dbnum: u32,
    probe: &ModelWatermarkProbe,
) -> anyhow::Result<ModelWatermarkReconcile> {
    let supported = probe.supported();
    if probe.recorded == supported {
        return Ok(ModelWatermarkReconcile::Aligned { sesno: supported });
    }
    if probe.recorded < supported {
        return Ok(ModelWatermarkReconcile::AwaitingTaskCompletion {
            recorded: probe.recorded,
            supported,
        });
    }
    db.query(render_model_watermark_reconcile(
        dbnum,
        supported,
        probe.abandoned,
    ))
    .await
    .with_context(|| format!("reconcile model watermark of dbnum {dbnum}"))?
    .check()
    .with_context(|| format!("model watermark reconcile statement of dbnum {dbnum}"))?;
    crate::data_interface::dbnum_state::DbnumState::invalidate_mirrored_on(db, dbnum);

    Ok(if !probe.has_roots && !probe.has_model_projection {
        ModelWatermarkReconcile::ClearedGhost {
            from: probe.recorded,
        }
    } else {
        ModelWatermarkReconcile::LoweredUnbacked {
            from: probe.recorded,
            to: supported,
        }
    })
}

/// 启动那一轮对每个候选库做一次对账，按去向各自出声。
///
/// **一库失败不中止**：与它挂靠的启动覆盖核对同一条纪律（ADR-050 的补偿路径不许把
/// 一个库的问题变成整个服务的崩溃循环）。
pub(crate) async fn reconcile_model_watermarks(
    dbnums: impl IntoIterator<Item = u32>,
) -> BTreeMap<u32, ModelWatermarkProbe> {
    let mut probes = BTreeMap::new();
    for dbnum in dbnums {
        let mut probe = match read_model_watermark_probe(&SUL_DB, dbnum).await {
            Ok(probe) => probe,
            Err(error) => {
                println!("[model-completion] dbnum={dbnum} reason=probe_failed error={error:#}");
                continue;
            }
        };
        match reconcile_model_watermark_from_probe(&SUL_DB, dbnum, &probe).await {
            Ok(ModelWatermarkReconcile::Aligned { .. }) => {}
            Ok(ModelWatermarkReconcile::LoweredUnbacked { from, to }) => {
                println!(
                    "[模型水位] dbnum={dbnum} 失去支撑，已下调 {from} → {to}\
                     （行上记的追平点高于安定凭证能证明的那一版）"
                );
            }
            Ok(ModelWatermarkReconcile::AwaitingTaskCompletion {
                recorded,
                supported,
            }) => {
                println!(
                    "[模型水位] dbnum={dbnum} 凭证可支撑 {supported}、完成水位仍为 {recorded}；\
                     普通对账不上调，排恢复 Task 完成发布、空间与房间后盖章"
                );
            }
            Ok(ModelWatermarkReconcile::ClearedGhost { from }) => {
                println!(
                    "[模型水位] dbnum={dbnum} 幽灵模型水位：水位说追平到 {from}，\
                     这个库的 gen_root 一行都没有，已清零；模型按需重新生成\
                     （不删数据、不阻断）"
                );
            }
            Err(error) => {
                println!("[模型水位] dbnum={dbnum} 对账失败，本次跳过（启动继续）: {error:#}");
            }
        }
        probe.recorded = probe.recorded.min(probe.supported());
        probes.insert(dbnum, probe);
    }
    probes
}
