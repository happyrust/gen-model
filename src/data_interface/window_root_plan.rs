//! 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`
//! （ADR-056 P2-1 / D3 / D9 / N7）。
//!
//! 输入只有两类：**文件**（e3d-io `DbSet@S` / `DbSet@T`、e3d-model `plan_update` 的
//! [`UpdatePlan`] 与它的祖先闭包 [`AffectedClosure`]、[`enumerate_generation_roots`] 在
//! `DbSet@T` 上枚举出的 `roots_T`）与**模型面自己的状态**（`gen_root` 行 = `roots_S`）。
//! 一行 `pe` 都不读——从未跑过数据增量、`pe` 零行的库对一个窗口也能选根（N7）。
//! `fn::sync_gen_roots` 不在这条路上（它的 `fn::gen_root_cover` 第一句就是查 `pe`，F10）。
//!
//! 候选根 = `roots_S ∪ roots_T`；`touched` = 其中落在闭包里的那些（闭包按祖先封闭，
//! 「根不在闭包里」＝「它子树里没有任何一件受影响的东西」）。四个去向（D9）：
//!
//! | 去向 | 集合 | 动作 |
//! |---|---|---|
//! | `regen`   | `touched ∩ roots_T`             | `RegenRoot`，eager 生成 |
//! | `delete`  | `roots_S \ roots_T`             | `DeleteCleanup`：根已删，或还在但不再是根（改挂到别的 MDU 之下 / 类型变了）——旧根名下的几何不能留 |
//! | `advance` | `(roots_S ∩ roots_T) \ touched` | 凭证前移到 T（P2-2 执行；不动几何、manifest、revision） |
//! | `lazy`    | `(roots_T \ roots_S) \ touched` | 什么都不做：从未生成过又没被波及，等按需 `ensure` |
//!
//! 护栏（ADR-056 实施约束 5）：闭包不完整、`plan_update` 报 `unresolved`、或候选数超过
//! 索引键数的 [`CREDENTIAL_ADVANCE_CANDIDATE_RATIO`] → **放弃前移**：`advance` 清空、
//! `roots_S ∩ roots_T` 全部进 `regen`（今天「全部根过期」的行为），[`WindowRootPlan::degraded`]
//! 记原因，回执字段 `credential_advance_degraded`。`lazy` 在退化时也不动——没生成过的根
//! 即便波及不明也只需等按需生成，不必 eager。
//!
//! 执行端：`RegenRoot` / `DeleteCleanup` 照旧进 `model_update_pending`；`advance` 由本文件
//! 的 [`advance_root_credentials_on`]（P2-2）在数据尾事务**之后**、模型 drain 之前批量前移
//! ——`finalize_attempt_on` 提交完水位就调它，失败只记日志（N4：模型面不拦水位）。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_model::increment::{AffectedClosure, UpdatePlan, collect_window, plan_update};
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::generation_root::{
    GenerationRoot, enumerate_generation_roots, refno_to_e3d,
};
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::fast_model::e3d_model_service::scan_index;

/// 候选数占索引键数的比例超过它就放弃凭证前移（ADR-056 实施约束 5、审核 P1-1）。
pub const CREDENTIAL_ADVANCE_CANDIDATE_RATIO: f64 = 0.30;

/// 一条已持久化的 `gen_root` 行（模型面自己的状态）：`roots_S` 的元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRoot {
    pub refno: RefnoEnum,
    pub noun: String,
}

/// 一窗差分对候选根的波及情况——纯数据，由 [`WindowRootSources::impact`] 从 e3d-model
/// 的产物折出来，也可以在测试里手搓。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowImpact {
    /// `roots_S ∪ roots_T` 里落在祖先闭包中的根。
    pub touched: BTreeSet<RefnoEnum>,
    /// [`AffectedClosure::is_complete`]：有一条 owner 链读挂就是 `false`。
    pub closure_complete: bool,
    /// `plan_update` 连分类都做不了的候选数（`IncrementReport::unresolved`）。
    pub unresolved: usize,
    /// 索引差分候选数（`IncrementReport::candidates`）。
    pub candidates: usize,
    /// target 端索引键总数（护栏分母）。
    pub index_keys: usize,
}

impl WindowImpact {
    /// 不能前移凭证的原因；`None` = 闭包可信、可以前移。
    ///
    /// 比例护栏是「超过」不是「达到」：空库（0 键 0 候选）不算退化。
    pub fn degraded_reason(&self) -> Option<String> {
        let mut reasons = Vec::new();
        if !self.closure_complete {
            reasons.push("closure incomplete (an owner chain was unreadable)".to_string());
        }
        if self.unresolved > 0 {
            reasons.push(format!("unresolved={}", self.unresolved));
        }
        let limit = CREDENTIAL_ADVANCE_CANDIDATE_RATIO * self.index_keys as f64;
        if self.candidates as f64 > limit {
            reasons.push(format!(
                "too many candidates: {} > {:.0}% of {} index keys",
                self.candidates,
                CREDENTIAL_ADVANCE_CANDIDATE_RATIO * 100.0,
                self.index_keys
            ));
        }
        (!reasons.is_empty()).then(|| reasons.join("; "))
    }
}

/// 一窗选根的结果，见模块头的四个去向。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRootPlan {
    pub regen: Vec<GenerationRoot>,
    pub delete: Vec<PersistedRoot>,
    pub advance: Vec<RefnoEnum>,
    pub lazy: Vec<GenerationRoot>,
    /// 放弃凭证前移的原因（`credential_advance_degraded`）。
    pub degraded: Option<String>,
}

impl WindowRootPlan {
    /// `RegenRoot` / `DeleteCleanup` 工作项，按 `(action, target)` 排序去重——与
    /// `build_model_update_plan` 的口径一致。
    pub fn work_items(&self, dbnum: u32, db_type: &str, end_sesno: i32) -> Vec<ModelWorkItem> {
        let item = |action: ModelWorkAction, refno: RefnoEnum, noun: &str| ModelWorkItem {
            dbnum,
            db_type: db_type.to_string(),
            source_end_sesno: end_sesno,
            action,
            target_refno: refno.to_pdms_str(),
            noun: noun.to_string(),
        };
        let mut items: Vec<ModelWorkItem> = self
            .regen
            .iter()
            .map(|root| item(ModelWorkAction::RegenRoot, root.root, &root.noun))
            .chain(
                self.delete
                    .iter()
                    .map(|root| item(ModelWorkAction::DeleteCleanup, root.refno, &root.noun)),
            )
            .collect();
        items.sort_by_key(|item| (item.action, item.target_refno.clone()));
        items.dedup_by(|a, b| a.action == b.action && a.target_refno == b.target_refno);
        items
    }

    /// 折成数据尾事务要持久化的 [`ModelUpdatePlan`]：工作项 + 前移名单 + 退化告警。
    pub fn into_model_update_plan(
        self,
        dbnum: u32,
        db_type: &str,
        end_sesno: i32,
        mut warnings: Vec<String>,
    ) -> ModelUpdatePlan {
        let work_items = self.work_items(dbnum, db_type, end_sesno);
        if let Some(reason) = &self.degraded {
            warnings.push(format!(
                "dbnum={dbnum}: credential_advance_degraded — {reason}; every persisted root \
                 that is still a root at {end_sesno} is regenerated instead of advanced"
            ));
        }
        ModelUpdatePlan {
            work_items,
            warnings,
            credential_advance: self
                .advance
                .iter()
                .map(|refno| refno.to_pdms_str())
                .collect(),
            ..Default::default()
        }
    }
}

/// 纯函数：把候选根按波及情况分到四个去向。
///
/// `roots_t` 按枚举序（存储成员序前序）、`roots_s` 按持久层返回序进来；输出各桶保持
/// 输入的相对顺序，`work_items` 再统一排序。
pub fn plan_window_roots(
    roots_t: &[GenerationRoot],
    roots_s: &[PersistedRoot],
    impact: &WindowImpact,
) -> WindowRootPlan {
    let degraded = impact.degraded_reason();
    let at_target: BTreeSet<RefnoEnum> = roots_t.iter().map(|root| root.root).collect();
    let persisted: BTreeSet<RefnoEnum> = roots_s.iter().map(|root| root.refno).collect();
    let mut plan = WindowRootPlan {
        degraded: degraded.clone(),
        ..Default::default()
    };
    for root in roots_t {
        let touched = impact.touched.contains(&root.root);
        let was_persisted = persisted.contains(&root.root);
        if touched || (degraded.is_some() && was_persisted) {
            plan.regen.push(root.clone());
        } else if was_persisted {
            plan.advance.push(root.root);
        } else {
            plan.lazy.push(root.clone());
        }
    }
    plan.delete = roots_s
        .iter()
        .filter(|root| !at_target.contains(&root.refno))
        .cloned()
        .collect();
    plan
}

/// 文件侧输入：只读两端 `DbSet`，不碰 SurrealDB。生产由
/// [`build_model_update_plan_from_window`] 用 `E3dModelService::build_set` 的两端调用；
/// 测试与探针可以直接用 attlib 开两个钉住会话的 `DbSet`。
pub struct WindowRootSources {
    pub base_sesno: u32,
    pub target_sesno: u32,
    /// `DbSet@T` 上按 MDU / significant 口径枚举出的全部生成根。
    pub roots_t: Vec<GenerationRoot>,
    pub plan: UpdatePlan,
    pub closure: AffectedClosure,
    /// target 端索引键总数（`scan_index(...).owners.len()`）。
    pub index_keys: usize,
}

impl WindowRootSources {
    /// 一次窗口的全部文件侧工作：索引差分 → `plan_update` → 祖先闭包 → `DbSet@T` 根枚举。
    /// 阻塞 I/O，异步上下文请放进 `spawn_blocking`。
    pub fn collect(
        file: &Path,
        base_sesno: u32,
        target_sesno: u32,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        unit_types: &[String],
    ) -> anyhow::Result<Self> {
        let window = collect_window(file, base_sesno, target_sesno).with_context(|| {
            format!(
                "collect window {base_sesno}→{target_sesno} of {}",
                file.display()
            )
        })?;
        let plan = plan_update(base, target, &window);
        let closure = plan.affected_closure(base, target);
        let index = scan_index(file, Some(target_sesno))
            .with_context(|| format!("scan index of {} at {target_sesno}", file.display()))?;
        let roots_t =
            enumerate_generation_roots(target, &index.roots, unit_types).with_context(|| {
                format!(
                    "enumerate generation roots of {} at {target_sesno}",
                    file.display()
                )
            })?;
        Ok(Self {
            base_sesno,
            target_sesno,
            roots_t,
            plan,
            closure,
            index_keys: index.owners.len(),
        })
    }

    /// `roots_S ∪ roots_T` 里哪些根被这一窗波及（`UpdatePlan::touches_roots` 的判据），
    /// 连同护栏要看的三个数。
    pub fn impact(&self, roots_s: &[PersistedRoot]) -> WindowImpact {
        let touched = roots_s
            .iter()
            .map(|root| root.refno)
            .chain(self.roots_t.iter().map(|root| root.root))
            .filter(|refno| self.closure.contains(refno_to_e3d(*refno)))
            .collect();
        WindowImpact {
            touched,
            closure_complete: self.closure.is_complete(),
            unresolved: self.plan.report.unresolved.len(),
            candidates: self.plan.report.candidates,
            index_keys: self.index_keys,
        }
    }
}

/// `roots_S`：这个库已持久化的 `gen_root` 行（凭证 / CAS 状态表）。
pub async fn load_persisted_roots(dbnum: u32) -> anyhow::Result<Vec<PersistedRoot>> {
    #[derive(serde::Deserialize)]
    struct Row {
        pe: surrealdb::sql::Thing,
        #[serde(default)]
        noun: String,
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT pe, noun FROM gen_root WHERE dbnum = {dbnum} ORDER BY pe;"
        ))
        .await
        .with_context(|| format!("load gen_root rows of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("gen_root statement for dbnum {dbnum}"))?;
    let rows: Vec<Row> = response
        .take(0)
        .with_context(|| format!("decode gen_root rows of dbnum {dbnum}"))?;
    rows.into_iter()
        .map(|row| {
            Ok(PersistedRoot {
                refno: crate::data_interface::helper::pe_thing_to_refno(row.pe)?,
                noun: row.noun,
            })
        })
        .collect()
}

/// DESI 窗口 S→T 的模型计划，输入全部来自文件与 `gen_root`（P2-1）。
///
/// 非 DESI 库返回空计划：CATA 窗口仍走 `build_cata_cascade_plan`（`ref_rev` 反查，P2-3），
/// SYST / 其它库没有模型面。两端 `DbSet` 由 `E3dModelService::build_set` 给出，其它库钉
/// 各自当前 pin、目录库经 `E3dDbResolver` 惰性解析。
pub(crate) async fn build_model_update_plan_from_window(
    dbnum: u32,
    db_type: &str,
    base_sesno: u32,
    target_sesno: u32,
) -> anyhow::Result<ModelUpdatePlan> {
    if !db_type.eq_ignore_ascii_case("DESI") {
        return Ok(ModelUpdatePlan::default());
    }
    let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let file = service.source_file(dbnum)?.to_path_buf();
    let base = service.build_set(dbnum, Some(base_sesno))?;
    let target = service.build_set(dbnum, Some(target_sesno))?;
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let sources = tokio::task::spawn_blocking(move || {
        WindowRootSources::collect(&file, base_sesno, target_sesno, &base, &target, &unit_types)
    })
    .await
    .map_err(|error| anyhow::anyhow!("window root planning task failed: {error}"))??;
    let roots_s = load_persisted_roots(dbnum).await?;
    let impact = sources.impact(&roots_s);
    let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
    let warnings = vec![format!(
        "dbnum={dbnum}: window {base_sesno}→{target_sesno} roots_T={} roots_S={} touched={} \
         regen={} delete={} advance={} lazy={} | {}",
        sources.roots_t.len(),
        roots_s.len(),
        impact.touched.len(),
        plan.regen.len(),
        plan.delete.len(),
        plan.advance.len(),
        plan.lazy.len(),
        sources.plan.report.totals_line()
    )];
    Ok(plan.into_model_update_plan(dbnum, db_type, target_sesno as i32, warnings))
}

// ---- P2-2 凭证前移 ----

/// 凭证可以前移的完成态——与 `generation_root_cache_current` 认「已生成」的那一组同一份。
pub const SETTLED_ROOT_STATUSES: [&str; 3] =
    ["Generated", "AlreadyAvailable", "NoRenderableGeometry"];

/// 一条前移语句最多点名多少个根（与本仓其它按键批量语句同一块大小）。
const CREDENTIAL_ADVANCE_CHUNK: usize = 500;

/// 一次凭证前移的账。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialAdvanceOutcome {
    /// 计划名单上的根数。
    pub requested: usize,
    /// 真正改了行的根数。差额是没落到「已发布且安定」态、凭证已不早于 T、或行不存在的
    /// 根——它们照旧按 ADR-054 的单调判据过期，等按需 `ensure`。
    pub advanced: usize,
}

/// 渲染凭证前移语句（每 [`CREDENTIAL_ADVANCE_CHUNK`] 个根一条）。
///
/// 只放过**已发布且安定**的行：完成态（[`SETTLED_ROOT_STATUSES`]）、`publication_status`
/// 是 `ready`（旧式行没有这一列，视同 `ready`）、`desired_revision == published_revision`
/// （同上）。任何一条不满足都说明这个根还有没落地的工作或上次没成——它的凭证不是
/// 「S 时刻几何有效」的证明，前移它就是给陈旧几何盖新章：`generation_root_cache_current`
/// 会判它「当前」，而队里那条旧窗口的 regen 收口时又把凭证写回旧值。
///
/// 单调：`0 < source_end_sesno < T` 才动。`0` 是人工强制重试的「未认领」凭证
/// （`ensure_regen_pending`），永不算覆盖、也永不前移。`source_end_sesno_time` 跟着序号写，
/// 没有会话时刻就不写那一列（plant-ui ADR-0019：不许拿挂钟顶替）。旧凭证记进
/// `credential_advanced_from`，让「这个凭证是前移来的还是生成来的」在行上看得出来（D2）。
///
/// 不碰几何、manifest、`published_*`、`desired_*`、`status`——前移改的只是「这份几何到
/// 哪个会话为止仍然有效」这一个事实。
pub fn render_credential_advance(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<Vec<String>> {
    use std::str::FromStr;

    let mut ids = Vec::with_capacity(roots.len());
    for root in roots {
        let refno = aios_core::RefU64::from_str(root).map_err(|error| {
            anyhow::anyhow!("credential advance root `{root}` is not an a/b refno: {error}")
        })?;
        let root_id = RefnoEnum::from(refno).to_pdms_str().replace('/', "_");
        ids.push(format!("type::thing('gen_root', '{root_id}')"));
    }
    let statuses = SETTLED_ROOT_STATUSES
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let time_clause = end_sesno_time
        .map(|time| format!(", source_end_sesno_time = '{}'", escape_surql_str(time)))
        .unwrap_or_default();
    Ok(ids
        .chunks(CREDENTIAL_ADVANCE_CHUNK)
        .map(|chunk| {
            format!(
                "UPDATE gen_root SET credential_advanced_from = source_end_sesno, \
                 source_end_sesno = {end_sesno}{time_clause}, \
                 credential_advanced_at = time::now(), updated_at = time::now() \
                 WHERE dbnum = {dbnum} AND id IN [{}] \
                 AND status IN [{statuses}] \
                 AND (publication_status ?: 'ready') = 'ready' \
                 AND (desired_revision ?: 0) = (published_revision ?: 0) \
                 AND (source_end_sesno ?: 0) > 0 AND (source_end_sesno ?: 0) < {end_sesno} \
                 RETURN id;",
                chunk.join(", ")
            )
        })
        .collect())
}

/// 在 `db` 上把名单里安定且落后的根凭证前移到 `end_sesno`（P2-2）。
///
/// 调用点在数据尾事务之后：水位已经落了，这里成败都拦不了它；返回的账给日志与
/// `credential_advance_degraded` 之外的那半观测——「这一窗省掉了多少根的重生成」。
pub(crate) async fn advance_root_credentials_on(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<CredentialAdvanceOutcome> {
    #[derive(serde::Deserialize)]
    struct Advanced {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
    }
    let mut outcome = CredentialAdvanceOutcome {
        requested: roots.len(),
        advanced: 0,
    };
    for statement in render_credential_advance(dbnum, end_sesno, end_sesno_time, roots)? {
        let mut response = db
            .query(statement)
            .await
            .with_context(|| {
                format!("advance gen_root credentials of dbnum {dbnum} to {end_sesno}")
            })?
            .check()
            .with_context(|| format!("credential advance statement of dbnum {dbnum}"))?;
        let rows: Vec<Advanced> = response
            .take(0)
            .with_context(|| format!("decode advanced gen_root ids of dbnum {dbnum}"))?;
        outcome.advanced += rows.len();
    }
    Ok(outcome)
}

/// 生产入口：在 `SUL_DB` 上前移。
pub async fn advance_root_credentials(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<CredentialAdvanceOutcome> {
    advance_root_credentials_on(&SUL_DB, dbnum, end_sesno, end_sesno_time, roots).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::GenerationRootKind;
    use aios_core::RefU64;

    fn r(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(24384, id))
    }

    fn root(id: u32, noun: &str, kind: GenerationRootKind) -> GenerationRoot {
        GenerationRoot {
            root: r(id),
            noun: noun.into(),
            name: format!("/N{id}"),
            kind,
        }
    }

    fn persisted(id: u32, noun: &str) -> PersistedRoot {
        PersistedRoot {
            refno: r(id),
            noun: noun.into(),
        }
    }

    fn impact(touched: &[u32]) -> WindowImpact {
        WindowImpact {
            touched: touched.iter().map(|id| r(*id)).collect(),
            closure_complete: true,
            unresolved: 0,
            candidates: 3,
            index_keys: 1000,
        }
    }

    fn ids(roots: &[GenerationRoot]) -> Vec<RefnoEnum> {
        roots.iter().map(|root| root.root).collect()
    }

    /// 四个去向各来一个：5 被波及且仍是根 → regen；7 在 gen_root 里但 T 端不再是根 → delete；
    /// 8 两端都是根且没被波及 → advance；9 是 T 端新根、没被波及 → lazy。
    #[test]
    fn candidates_split_into_regen_delete_advance_and_lazy() {
        let roots_t = vec![
            root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            root(9, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let roots_s = vec![
            persisted(5, "BRAN"),
            persisted(7, "BRAN"),
            persisted(8, "EQUI"),
        ];
        let plan = plan_window_roots(&roots_t, &roots_s, &impact(&[5, 7]));
        assert_eq!(ids(&plan.regen), vec![r(5)]);
        assert_eq!(plan.delete, vec![persisted(7, "BRAN")]);
        assert_eq!(plan.advance, vec![r(8)]);
        assert_eq!(ids(&plan.lazy), vec![r(9)]);
        assert_eq!(plan.degraded, None);
    }

    /// 被波及的新根（本窗口新建的 BRAN）也 eager：它在 `roots_T` 且在闭包里。
    #[test]
    fn a_touched_new_root_is_regenerated_not_lazy() {
        let roots_t = vec![root(9, "BRAN", GenerationRootKind::DeliveryUnit)];
        let plan = plan_window_roots(&roots_t, &[], &impact(&[9]));
        assert_eq!(ids(&plan.regen), vec![r(9)]);
        assert!(plan.lazy.is_empty());
        assert!(plan.advance.is_empty());
    }

    /// 消失的根不管闭包说什么都要清：`roots_S \ roots_T` 整体 → delete。
    #[test]
    fn a_persisted_root_missing_at_target_is_deleted_even_if_untouched() {
        let roots_s = vec![persisted(7, "BRAN")];
        let plan = plan_window_roots(&[], &roots_s, &impact(&[]));
        assert_eq!(plan.delete, vec![persisted(7, "BRAN")]);
        assert!(plan.regen.is_empty() && plan.advance.is_empty());
    }

    /// 三条护栏任一触发都退化：`advance` 清空，`roots_S ∩ roots_T` 全部 regen；
    /// `delete` 与 `lazy` 不受影响。
    #[test]
    fn guards_degrade_credential_advance_into_regenerating_every_persisted_root() {
        let roots_t = vec![
            root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            root(9, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let roots_s = vec![
            persisted(5, "BRAN"),
            persisted(7, "BRAN"),
            persisted(8, "EQUI"),
        ];
        let mut unresolved = impact(&[5]);
        unresolved.unresolved = 2;
        let mut incomplete = impact(&[5]);
        incomplete.closure_complete = false;
        let mut too_wide = impact(&[5]);
        too_wide.candidates = 301;

        for (label, degraded) in [
            ("unresolved", unresolved),
            ("incomplete", incomplete),
            ("too_wide", too_wide),
        ] {
            let plan = plan_window_roots(&roots_t, &roots_s, &degraded);
            assert_eq!(ids(&plan.regen), vec![r(5), r(8)], "{label}");
            assert!(plan.advance.is_empty(), "{label}");
            assert_eq!(plan.delete, vec![persisted(7, "BRAN")], "{label}");
            assert_eq!(ids(&plan.lazy), vec![r(9)], "{label}");
            let reason = plan.degraded.expect(label);
            assert!(
                reason.contains(label.trim_end_matches("_wide")),
                "{label}: {reason}"
            );
        }
    }

    /// 比例护栏是「超过」不是「达到」；空库（0 键 0 候选）不算退化。
    #[test]
    fn the_candidate_ratio_guard_is_strictly_greater_than() {
        let mut at_limit = impact(&[]);
        at_limit.candidates = 300;
        at_limit.index_keys = 1000;
        assert_eq!(at_limit.degraded_reason(), None);
        at_limit.candidates = 301;
        assert!(at_limit.degraded_reason().is_some());

        let empty = WindowImpact {
            closure_complete: true,
            ..Default::default()
        };
        assert_eq!(empty.degraded_reason(), None);
    }

    /// 工作项：regen → `RegenRoot`（noun 取 T 端）、delete → `DeleteCleanup`（noun 取
    /// gen_root 行），按 `(action, target)` 排序；`advance` / `lazy` 不产生工作项。
    #[test]
    fn work_items_carry_only_regen_and_delete() {
        let plan = WindowRootPlan {
            regen: vec![
                root(8, "EQUI", GenerationRootKind::DeliveryUnit),
                root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            ],
            delete: vec![persisted(7, "HANG")],
            advance: vec![r(1)],
            lazy: vec![root(9, "EQUI", GenerationRootKind::DeliveryUnit)],
            degraded: None,
        };
        let items = plan.work_items(8000, "DESI", 26);
        assert_eq!(
            items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str(), item.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (ModelWorkAction::RegenRoot, "24384/5", "BRAN"),
                (ModelWorkAction::RegenRoot, "24384/8", "EQUI"),
                (ModelWorkAction::DeleteCleanup, "24384/7", "HANG"),
            ]
        );
        assert!(items.iter().all(|item| item.dbnum == 8000
            && item.db_type == "DESI"
            && item.source_end_sesno == 26));
    }

    /// 折成 `ModelUpdatePlan`：前移名单落 `credential_advance`，退化原因进 warnings。
    #[test]
    fn model_update_plan_carries_the_advance_list_and_degradation() {
        let plan = WindowRootPlan {
            regen: vec![root(5, "BRAN", GenerationRootKind::DeliveryUnit)],
            delete: vec![],
            advance: vec![r(8), r(1)],
            lazy: vec![],
            degraded: Some("unresolved=2".into()),
        };
        let out = plan.into_model_update_plan(8000, "DESI", 26, vec!["earlier".into()]);
        assert_eq!(out.work_items.len(), 1);
        assert_eq!(out.credential_advance, vec!["24384/8", "24384/1"]);
        assert_eq!(out.warnings[0], "earlier");
        assert!(
            out.warnings[1].contains("credential_advance_degraded")
                && out.warnings[1].contains("unresolved=2"),
            "{:?}",
            out.warnings
        );
        assert!(out.units.is_empty() && out.design_refnos.is_empty());
    }

    /// 旧的 durable attempt 行没有 `credential_advance` 字段，反序列化必须给空表。
    #[test]
    fn model_update_plan_without_credential_advance_deserializes_to_empty() {
        let json = r#"{"work_items":[],"warnings":[]}"#;
        let plan: ModelUpdatePlan = serde_json::from_str(json).unwrap();
        assert!(plan.credential_advance.is_empty());
    }

    // ---- P2-2 凭证前移 ----

    fn advance_sql(roots: &[&str], time: Option<&str>) -> Vec<String> {
        let roots: Vec<String> = roots.iter().map(|root| root.to_string()).collect();
        render_credential_advance(8000, 26, time, &roots).unwrap()
    }

    /// 前移语句只碰本库、点名的根；只放过「已发布且安定」的行（完成态、`ready`、
    /// desired == published）；单调（凭证 < T 才动，`0` 凭证永不前移）；时刻跟着序号走；
    /// 几何、manifest、revision、publication 状态一个字不碰。
    #[test]
    fn credential_advance_statement_is_scoped_settled_and_monotonic() {
        let sql = advance_sql(&["24384/8", "24384/1"], Some("2026-09-02T10:00:00+08:00"));
        assert_eq!(sql.len(), 1, "{sql:?}");
        let sql = &sql[0];
        for needle in [
            "UPDATE gen_root SET",
            "credential_advanced_from = source_end_sesno",
            "source_end_sesno = 26",
            "source_end_sesno_time = '2026-09-02T10:00:00+08:00'",
            "WHERE dbnum = 8000",
            "type::thing('gen_root', '24384_8')",
            "type::thing('gen_root', '24384_1')",
            "status IN ['Generated', 'AlreadyAvailable', 'NoRenderableGeometry']",
            "(publication_status ?: 'ready') = 'ready'",
            "(desired_revision ?: 0) = (published_revision ?: 0)",
            "(source_end_sesno ?: 0) > 0",
            "(source_end_sesno ?: 0) < 26",
            "RETURN id",
        ] {
            assert!(sql.contains(needle), "missing `{needle}` in {sql}");
        }
        // 「记旧值」必须排在「写新值」之前，否则记下的就是 T 自己。
        assert!(
            sql.find("credential_advanced_from = source_end_sesno")
                .unwrap()
                < sql.find("source_end_sesno = 26").unwrap(),
            "{sql}"
        );
        for forbidden in [
            "published_revision =",
            "desired_revision =",
            "published_manifest_hash",
            "published_target =",
            "status =",
            "publication_status =",
        ] {
            assert!(!sql.contains(forbidden), "`{forbidden}` in {sql}");
        }
        // 没有会话时刻就不写那一列（不许拿挂钟顶替，plant-ui ADR-0019）。
        let without = advance_sql(&["24384/8"], None);
        assert!(
            !without[0].contains("source_end_sesno_time"),
            "{}",
            without[0]
        );
    }

    /// 空名单不出语句；名单按 500 分块；不是 `a/b` 的串是规划器的 bug，整体报错。
    #[test]
    fn credential_advance_is_chunked_and_rejects_malformed_refnos() {
        assert!(advance_sql(&[], None).is_empty());
        let many: Vec<String> = (1..=1001).map(|i| format!("24384/{i}")).collect();
        let sql = render_credential_advance(8000, 26, None, &many).unwrap();
        assert_eq!(sql.len(), 3);
        assert!(sql[0].contains("'24384_1')") && !sql[0].contains("'24384_501')"));
        assert!(sql[1].contains("'24384_501')") && sql[1].contains("'24384_1000')"));
        assert!(sql[2].contains("'24384_1001')"));
        assert!(render_credential_advance(8000, 26, None, &["not-a-refno".to_string()]).is_err());
    }

    async fn credential_advance_db(name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("window_root_plan").use_db(name).await.unwrap();
        // 1 安定可前移；2 有未发布的 desired（stale）；3 上次失败；4 凭证已经更新；
        // 5 人工强制重试的 0 凭证；6 旧式行（没有 publication 字段）但完成态；
        // 7 别的库；9 安定但不在名单上。
        db.query(
            "CREATE gen_root:24384_1 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20, \
               source_end_sesno_time = '2026-09-01T00:00:00+08:00';\
             CREATE gen_root:24384_2 SET dbnum = 8000, status = 'Generated', publication_status = 'stale', \
               desired_revision = 4, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_3 SET dbnum = 8000, status = 'Failed', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_4 SET dbnum = 8000, status = 'AlreadyAvailable', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 30;\
             CREATE gen_root:24384_5 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 0;\
             CREATE gen_root:24384_6 SET dbnum = 8000, status = 'NoRenderableGeometry', source_end_sesno = 20;\
             CREATE gen_root:24384_7 SET dbnum = 1112, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_9 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db
    }

    /// `(凭证, 时刻, 前移前的凭证)`，按根 id 排。
    async fn credential_rows(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> Vec<(String, Option<i32>, Option<String>, Option<i32>)> {
        #[derive(serde::Deserialize)]
        struct Row {
            key: String,
            source_end_sesno: Option<i32>,
            source_end_sesno_time: Option<String>,
            credential_advanced_from: Option<i32>,
        }
        let mut response = db
            .query(
                "SELECT type::string(record::id(id)) AS key, source_end_sesno, \
                 source_end_sesno_time, credential_advanced_from FROM gen_root ORDER BY key;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<Row> = response.take(0).unwrap();
        rows.into_iter()
            .map(|row| {
                (
                    row.key,
                    row.source_end_sesno,
                    row.source_end_sesno_time,
                    row.credential_advanced_from,
                )
            })
            .collect()
    }

    /// 真引擎上跑一遍：只有安定且落后的行前移到 T（记下旧凭证与新时刻），其余原样；
    /// 名单里不存在的行不算失败。
    #[tokio::test]
    async fn credential_advance_moves_only_settled_lagging_roots() {
        let db = credential_advance_db("moves_settled").await;
        let roots: Vec<String> = [
            "24384/1", "24384/2", "24384/3", "24384/4", "24384/5", "24384/6", "24384/7", "24384/8",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let time = "2026-09-02T10:00:00+08:00";
        let outcome = advance_root_credentials_on(&db, 8000, 26, Some(time), &roots)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            CredentialAdvanceOutcome {
                requested: 8,
                advanced: 2,
            }
        );
        let expected_time = Some(time.to_string());
        let old_time = Some("2026-09-01T00:00:00+08:00".to_string());
        assert_eq!(
            credential_rows(&db).await,
            vec![
                (
                    "24384_1".to_string(),
                    Some(26),
                    expected_time.clone(),
                    Some(20)
                ),
                ("24384_2".to_string(), Some(20), None, None),
                ("24384_3".to_string(), Some(20), None, None),
                ("24384_4".to_string(), Some(30), None, None),
                ("24384_5".to_string(), Some(0), None, None),
                ("24384_6".to_string(), Some(26), expected_time, Some(20)),
                ("24384_7".to_string(), Some(20), None, None),
                ("24384_9".to_string(), Some(20), None, None),
            ],
            "old time of root 1 was {old_time:?}"
        );
        // 幂等：再跑一次一个都不动。
        let again = advance_root_credentials_on(&db, 8000, 26, Some(time), &roots)
            .await
            .unwrap();
        assert_eq!(again.advanced, 0);
    }

    /// 收口尾事务之后顺手前移：`finalize_attempt_on` 拿到带 `credential_advance` 的计划，
    /// 水位推进到 T，名单上安定的根凭证也到 T——而且是在尾事务**之后**（水位先落）。
    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_attempt_advances_the_planned_credentials_after_the_tail() {
        use crate::data_interface::model_update_pending::finalize_attempt_on;

        let db = credential_advance_db("finalize_hook").await;
        let plan = ModelUpdatePlan {
            credential_advance: vec!["24384/1".into(), "24384/2".into()],
            ..Default::default()
        };
        finalize_attempt_on(&db, 8000, 26, Some("2026-09-02T10:00:00+08:00"), &plan, &[])
            .await
            .expect("finalize");
        let mut response = db
            .query("SELECT VALUE applied_sesno FROM dbnum_watermark:8000;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let watermark: Option<i32> = response.take(0).unwrap();
        assert_eq!(watermark, Some(26));
        let rows = credential_rows(&db).await;
        let sesno_of = |key: &str| {
            rows.iter()
                .find(|row| row.0 == key)
                .map(|row| row.1)
                .unwrap()
        };
        assert_eq!(sesno_of("24384_1"), Some(26));
        assert_eq!(sesno_of("24384_2"), Some(20), "stale 行不得前移");
        assert_eq!(sesno_of("24384_9"), Some(20), "不在名单上的根不动");
    }

    /// P2-1 真库门：ams8000 上四个钉死的窗口，`roots_S` 取 **base 端**枚举（当作 S 时刻
    /// 每个根都已发布），`roots_T` 取 target 端枚举，看四个去向落得对不对。窗口与
    /// `docs/evidence/2026-09-02-planner-parity.md` §1 / e3d-model `increment_real.rs` 同一批：
    ///
    /// - 255→256「修改」：PANE `24384/26250` 的 CACHID 变了。它上方 SBFR ⊂ FRMW ⊂ STRU 三个
    ///   都是 Core3D 显著 noun，按 `generation_roots_in_subtree` 的口径**各自都是根、各自整棵
    ///   子树生成**，三份 manifest 都含这块 PANE，所以三个根都受波及 → regen 3。对拍文档 §2.3
    ///   记的 G 侧「1 RegenRoot」是 legacy `resolve_element_generation_root` 的**最近显著属主**
    ///   口径，两套根口径在嵌套显著结构上本来就不同（见报告：嵌套显著根重叠是待拍的设计点）。
    /// - 195→196「EQUI 位姿」：EQUI `24384/26186` POS 变 → 1 根（D7-B 便宜路径本期尚未接）。
    /// - 45→46「BOX 改挂」：BOX `24384/25802` 从 EQUI `25801` 挪到 EQUI `25803` → 新根 + 旧根
    ///   两端都算（ADR-009；旧根 manifest 里还留着这件）→ regen 2。这条一度只出 1：e3d-model
    ///   `affected_closure` 两端共用早退集把 base 链吞了，已修并在 e3d-model 真库门钉住。
    /// - 24→26「删 EQUI 子树」：EQUI `24384/24778` 连同名下 BOX 被删 → 一条 `DeleteCleanup`、
    ///   零 regen、其余根全部前移。
    ///
    /// e3d-model 的窗口是**两端会话**：legacy `collect_changes(start..=end)` 对应这里的
    /// `base = start − 1`、`target = end`——即 ADR-056 实施约束 4「S = 提交前 applied_sesno，
    /// 显式传入」。只读文件：`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+
    /// `AIOS_E3D_TEMPLATE_DIR`。
    #[test]
    #[ignore = "manual live: needs the real ams8000 DESI file and the E3D template directory"]
    fn live_ams8000_pinned_windows_land_in_the_expected_buckets() {
        use std::path::PathBuf;

        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let open = |sesno: u32| -> Arc<DbSet> {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: file.clone(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin DESI");
            set
        };
        let unit_types = resolve_delivery_unit_types(&[]);

        struct Expect {
            base: u32,
            target: u32,
            label: &'static str,
            regen: usize,
            delete: Vec<&'static str>,
        }
        let table = [
            Expect {
                base: 255,
                target: 256,
                label: "修改（嵌套显著根 STRU/FRMW/SBFR）",
                regen: 3,
                delete: vec![],
            },
            Expect {
                base: 195,
                target: 196,
                label: "EQUI 位姿",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 45,
                target: 46,
                label: "BOX 改挂（新根 + 旧根）",
                regen: 2,
                delete: vec![],
            },
            Expect {
                base: 24,
                target: 26,
                label: "删 EQUI 子树",
                regen: 0,
                delete: vec!["24384/24778"],
            },
        ];

        for expect in table {
            let (base, target) = (open(expect.base), open(expect.target));
            let sources = WindowRootSources::collect(
                &file,
                expect.base,
                expect.target,
                &base,
                &target,
                &unit_types,
            )
            .expect("collect");
            assert!(
                sources.closure.is_complete(),
                "{}: {:?}",
                expect.label,
                sources.closure.unreadable
            );

            // S 时刻每个根都已发布：roots_S = base 端枚举。
            let base_index = scan_index(&file, Some(expect.base)).expect("scan base index");
            let roots_s: Vec<PersistedRoot> =
                enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                    .expect("enumerate base roots")
                    .into_iter()
                    .map(|root| PersistedRoot {
                        refno: root.root,
                        noun: root.noun,
                    })
                    .collect();
            let impact = sources.impact(&roots_s);
            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
            for entry in sources.plan.ledger.entries() {
                println!(
                    "  ledger: {} {} {:?} {}",
                    entry.refno, entry.noun, entry.kind, entry.detail
                );
            }
            let describe = |roots: &[GenerationRoot]| {
                roots
                    .iter()
                    .map(|root| format!("{} {}", root.root.to_pdms_str(), root.noun))
                    .collect::<Vec<_>>()
            };
            println!(
                "window {}→{} [{}]: roots_S={} roots_T={} touched={:?} regen={:?} delete={:?} advance={} lazy={:?} degraded={:?} | {}",
                expect.base,
                expect.target,
                expect.label,
                roots_s.len(),
                sources.roots_t.len(),
                impact
                    .touched
                    .iter()
                    .map(|refno| refno.to_pdms_str())
                    .collect::<Vec<_>>(),
                describe(&plan.regen),
                plan.delete
                    .iter()
                    .map(|root| format!("{} {}", root.refno.to_pdms_str(), root.noun))
                    .collect::<Vec<_>>(),
                plan.advance.len(),
                describe(&plan.lazy),
                plan.degraded,
                sources.plan.report.totals_line()
            );

            assert_eq!(plan.degraded, None, "{}", expect.label);
            assert_eq!(
                plan.regen.len(),
                expect.regen,
                "{}: regen {:?}",
                expect.label,
                describe(&plan.regen)
            );
            assert_eq!(
                plan.delete
                    .iter()
                    .map(|root| root.refno.to_pdms_str())
                    .collect::<Vec<_>>(),
                expect.delete,
                "{}",
                expect.label
            );
            // 每个候选根恰好落一个桶：regen + advance + lazy = roots_T；delete = roots_S \ roots_T。
            assert_eq!(
                plan.regen.len() + plan.advance.len() + plan.lazy.len(),
                sources.roots_t.len(),
                "{}",
                expect.label
            );
            let at_target: BTreeSet<RefnoEnum> =
                sources.roots_t.iter().map(|root| root.root).collect();
            assert_eq!(
                plan.delete.len(),
                roots_s
                    .iter()
                    .filter(|root| !at_target.contains(&root.refno))
                    .count(),
                "{}",
                expect.label
            );
            // 被波及的根都在 regen 里，没被波及的持久根都前移了。
            for root in &plan.regen {
                assert!(
                    impact.touched.contains(&root.root),
                    "{}: {} regen but untouched",
                    expect.label,
                    root.root
                );
            }
            for refno in &plan.advance {
                assert!(
                    !impact.touched.contains(refno),
                    "{}: {} advanced but touched",
                    expect.label,
                    refno
                );
            }
            let items = plan.work_items(8000, "DESI", expect.target as i32);
            assert_eq!(
                items.len(),
                expect.regen + expect.delete.len(),
                "{}: {:?}",
                expect.label,
                items
            );
        }
    }
}
