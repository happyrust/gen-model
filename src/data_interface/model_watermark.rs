//! 模型水位（ADR-060）：**启动对账**那一趟探针与下调。
//!
//! ADR-065 D2 把「欠什么」收敛成两枚水位；ADR-072 D5 把第二条异步账整个退了。留下的只有
//! 启动核对要的这一半：一趟探针读 `gen_root` 凭证（计划 §4.5 白名单里的「启动水化」那一次读）
//! 算出这一刻有证据支撑的模型水位，比行上记的高就下调、说出原因；上调只由任务收尾那一笔
//! （`task_end`）做。Task 收尾盖章（`complete_model_task_on` / `advance_model_watermark_on`）
//! 与整库证据闸（`CompletionReceipt`）随 publish 阶段退了，「已放弃」那一格只剩 `http_422`
//! （死信随 pending 表一起没了）。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::SUL_DB;
use anyhow::Context;
use surrealdb::{Surreal, engine::any::Any};

/// `gen_root` 记录 id（`24381_1`）与 PDMS 引用串（`24381/1`）归到同一把键上。
fn root_identity_key(root: &str) -> String {
    root.trim().replace('_', "/")
}

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
/// 2. **`http_422` 的根**——上一条排除掉的那些，进「已放弃」这一格。（死信那一格随
///    pending 表退了，ADR-072 D5。）
/// 3. **安定凭证下限与应用水位**——候选值 `min(下限, applied_sesno)` 的两个输入。
///    封顶那一下就是 M2：模型水位永不超过数据水位。
/// 4. **行上已经记着的模型水位，以及这个库到底有没有 `gen_root` 行**——只有对账
///    （D6/D7）要用：前者是被比的那一端，后者把「幽灵模型水位」与「普通的失去支撑」
///    分开。两件现场的处置措辞不同，混成一句就查不出是哪一种。
///
/// 只有启动对账用这一份取数：判据只有一处，漂不了。
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
    let projection = crate::data_interface::dbnum_state::projection_presence_expr(dbnum);
    format!(
        "SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} \
         AND {blocking};\n\
         SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} \
         AND (status ?: '') = 'http_422';\n\
         RETURN math::min((SELECT VALUE source_end_sesno FROM gen_root \
         WHERE dbnum = {dbnum} AND {settled}));\n\
         RETURN dbnum_watermark:{dbnum}.applied_sesno;\n\
         RETURN dbnum_watermark:{dbnum}.model_sesno;\n\
         SELECT VALUE record::id(id) FROM gen_root WHERE dbnum = {dbnum} LIMIT 1;\n\
         RETURN array::len((SELECT VALUE id FROM pe WHERE dbnum = {dbnum} LIMIT 1)) > 0;\n\
         RETURN IF array::len((SELECT VALUE id FROM gen_root WHERE dbnum={dbnum} LIMIT 1)) > 0 \
         THEN true ELSE ({projection}) END;"
    )
}

/// 一次探针读回来的整幅现场（ADR-060 D6，启动对账用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelWatermarkProbe {
    /// 还在赶的根：未安定，且不属于已放弃那一格。它们拦住水位。
    pub blockers: Vec<String>,
    /// 已放弃的根（`http_422`）。严格完成水位下仍然拦水位，但与正在执行的根分开观测。
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
}

impl ModelWatermarkProbe {
    /// 这一刻**有证据支撑**的模型水位（ADR-060 D6 的 `supported`）。
    ///
    /// 还有根在赶就是 0：那说明这个库整体没追平到任何一版，行上记的不管是多少都
    /// 没有支撑。没有根在赶时，上界是安定凭证的下限，再被应用水位封顶（M2）。
    /// 零根的库（只有 WORL + SITE 之类，或兼容接管的历史库）以行上记的水位为准：
    /// 它是任务收尾盖上去的，而任务只在这一版的根全落进去时才盖（ADR-072 D4）。
    pub fn supported(&self) -> i32 {
        if !self.blockers.is_empty() || self.abandoned > 0 {
            return 0;
        }
        self.settled_floor
            .or_else(|| {
                (!self.has_roots && self.has_attributes && self.has_model_projection)
                    .then_some(self.applied_sesno)
            })
            .or_else(|| (!self.has_roots).then_some(self.recorded))
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
    let settled_floor: Option<i32> = response
        .take(2)
        .with_context(|| format!("decode settled credential floor of dbnum {dbnum}"))?;
    let applied_sesno: Option<i32> = response
        .take(3)
        .with_context(|| format!("decode applied watermark of dbnum {dbnum}"))?;
    let recorded: Option<i32> = response
        .take(4)
        .with_context(|| format!("decode recorded model watermark of dbnum {dbnum}"))?;
    let any_root: Vec<String> = response
        .take(5)
        .with_context(|| format!("decode gen_root presence of dbnum {dbnum}"))?;
    let has_attributes: Option<bool> = response
        .take(6)
        .with_context(|| format!("decode attribute presence of dbnum {dbnum}"))?;
    let has_model_projection: Option<bool> = response
        .take(7)
        .with_context(|| format!("decode model projection presence of dbnum {dbnum}"))?;
    println!(
        "[model-query] at={} query=watermark_probe dbnum={dbnum} elapsed_ms={} rows={}",
        chrono::Utc::now().to_rfc3339(),
        started.elapsed().as_millis(),
        unsettled.len() + refused.len() + any_root.len(),
    );

    let abandoned: BTreeSet<String> = refused.iter().map(|root| root_identity_key(root)).collect();
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
    })
}

/// 把模型水位**绝对**写成对账算出来的支撑值（ADR-060 D6）。
///
/// 与任务收尾那一笔（`task_end::render_task_end_tail`）是两种写法，故意不合并：
/// 收尾是 `math::max` 的**承诺**，对账是 `=` 的**更正**。更正必须能往下写——
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
