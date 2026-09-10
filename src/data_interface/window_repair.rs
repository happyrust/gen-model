//! 已提交净窗口的维护纠正（ADR-036）。
//!
//! 2026-09-02（ADR-056 P1 / spec 035 T112）：从「开 kv-mem 暂存窗口 → 预载 → 暂存 → journal 写回」
//! 改为直写持久层重放，语句与生产直写路径同一份渲染；水位守卫与空间 epoch 推进仍在一个尾事务里。
//!
//! 2026-09-03 底座换成 e3d-io（`e3d_collector`）之后，ADR-036 的「成员关系仲裁补删」
//! （old-pdms-io `expand_deleted_membership_roots`：把 base 会话成员表里的子树整枝判删）
//! 一并退役——两根索引差分本身就是终态：base 有、target 无的记录**每一条**都已是 `Deleted`，
//! 按 base 成员表再扩一遍只会把 target 端仍在场的元素误杀（evidence `2026-09-02-planner-parity.md`
//! §3 的幻删就是这么来的）。这里保留的审计只剩一半：持久层里没有 OWNER 入边的活行，
//! 若在冻结目标会话的索引里**已不存在**，补一条 `Deleted`；仍在场的一律不动。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aios_core::pdms_types::RefU64;
use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use serde::Serialize;

use super::dbnum_state::DbnumState;
use super::ele_ops::{EleOperationData, EleOperationDetail};
use super::increment_pipeline::IncrementPipeline;

#[derive(Debug, Clone, Serialize)]
pub struct WindowRepairReport {
    pub dbnum: u32,
    pub from_sesno: i32,
    pub to_sesno: i32,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub membership_deleted: usize,
    pub unreachable_rows: usize,
    pub watermark_before: i32,
    pub watermark_after: i32,
    /// 恒为 0：kv-mem 暂存窗口已退役（ADR-056 P1），纠正直写持久层。字段保留一版给
    /// `db_window_repair` 的回执格式，P5 随口径收口删除。
    pub staging_windows: usize,
    pub cleaned_refnos: Vec<String>,
    pub verification: String,
    pub warnings: Vec<String>,
}

fn operation_counts(window: &BTreeMap<u32, Vec<EleOperationData>>) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for operation in window.values().flatten() {
        match operation.detail {
            EleOperationDetail::Add(_) => counts.0 += 1,
            EleOperationDetail::Modified(_) => counts.1 += 1,
            EleOperationDetail::Deleted => counts.2 += 1,
            EleOperationDetail::None => {}
        }
    }
    counts
}

async fn orphan_candidates(
    dbnum: u32,
    affected_owners: &BTreeSet<RefU64>,
) -> anyhow::Result<Vec<RefU64>> {
    if affected_owners.is_empty() {
        return Ok(Vec::new());
    }
    let owners = affected_owners
        .iter()
        .map(|refno| format!("pe:{refno}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE <string>record::id(id) FROM pe \
             WHERE dbnum = {dbnum} AND deleted != true AND noun != 'WORL' \
             AND owner IN [{owners}] AND array::len(<-pe_owner) = 0;"
        ))
        .await
        .context("可达性审计查询传输失败")?
        .check()
        .context("可达性审计查询失败")?;
    let ids: Vec<String> = response.take(0).context("可达性审计结果解码失败")?;
    Ok(ids
        .iter()
        .map(|id| RefnoEnum::from(id.as_str()).refno())
        .collect())
}

fn merge_deleted(
    window: &mut BTreeMap<u32, Vec<EleOperationData>>,
    target_sesno: i32,
    deleted: &BTreeSet<RefU64>,
) -> anyhow::Result<()> {
    if deleted.is_empty() {
        return Ok(());
    }
    for operations in window.values_mut() {
        operations.retain(|operation| !deleted.contains(&operation.refno));
    }
    let target = u32::try_from(target_sesno)
        .map_err(|_| anyhow::anyhow!("纠正目标会话非法: {target_sesno}"))?;
    window.entry(target).or_default().extend(
        deleted
            .iter()
            .copied()
            .map(|refno| EleOperationData::new(refno, target, EleOperationDetail::Deleted)),
    );
    Ok(())
}

async fn deleted_nouns(refnos: &[RefnoEnum]) -> anyhow::Result<Vec<(RefnoEnum, String)>> {
    let mut rows = Vec::new();
    for refno in refnos {
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE noun FROM ONLY {};",
                refno.to_pe_key()
            ))
            .await?
            .check()?;
        if let Some(noun) = response.take::<Option<String>>(0)? {
            anyhow::ensure!(
                !noun.is_empty()
                    && noun
                        .chars()
                        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_'),
                "纠正删除的 noun 非法: {noun}"
            );
            rows.push((*refno, noun));
        }
    }
    Ok(rows)
}

async fn verify_hard_deleted(refnos: &[RefnoEnum]) -> anyhow::Result<()> {
    for refno in refnos {
        let mut response = SUL_DB
            .query(format!("RETURN record::exists({});", refno.to_pe_key()))
            .await?
            .check()?;
        anyhow::ensure!(
            !response.take::<Option<bool>>(0)?.unwrap_or(false),
            "纠正验证失败: {} 仍存在",
            refno.to_pe_key()
        );
    }
    Ok(())
}

/// 服务停止后重放一个已提交窗口；只纠正数据，水位前后必须相同。
pub async fn repair_committed_window(
    dbnum: u32,
    from_sesno: i32,
    to_sesno: i32,
    expect_watermark: i32,
    file: &Path,
) -> anyhow::Result<WindowRepairReport> {
    anyhow::ensure!(from_sesno > 0 && from_sesno <= to_sesno, "纠正会话区间非法");
    let state = DbnumState::read(dbnum)
        .await?
        .ok_or_else(|| anyhow::anyhow!("dbnum={dbnum} 没有水位记录"))?;
    anyhow::ensure!(
        state.applied_sesno == expect_watermark,
        "dbnum={dbnum} 水位预检失败: expected={expect_watermark} actual={}",
        state.applied_sesno
    );
    anyhow::ensure!(to_sesno <= expect_watermark, "纠正窗口超过已提交水位");

    let mut collected = IncrementPipeline::collect_window(file, from_sesno..=to_sesno)?;
    let original_membership_deleted = collected.membership_deleted;
    let frozen = collected
        .frozen
        .clone()
        .ok_or_else(|| anyhow::anyhow!("纠正窗口缺少冻结源 FrozenSource"))?
        .reopen_verified_at(u32::try_from(to_sesno).context("纠正目标会话非法")?)
        .context("纠正成员审计重开冻结源失败")?;

    // 只对持久层已经没有 OWNER 入边的活行做文件存活复核；
    // 普通索引记录不会被这个审计面扩大成删除。
    let already_deleted = collected
        .range_eles
        .values()
        .flatten()
        .filter(|operation| matches!(operation.detail, EleOperationDetail::Deleted))
        .map(|operation| operation.refno)
        .collect::<BTreeSet<_>>();
    let affected_owners = collected
        .range_eles
        .values()
        .flatten()
        .filter(|operation| {
            matches!(
                &operation.detail,
                EleOperationDetail::Modified(modified) if modified.children_changed.is_some()
            )
        })
        .map(|operation| operation.refno)
        .collect::<BTreeSet<_>>();
    let orphans = orphan_candidates(dbnum, &affected_owners)
        .await?
        .into_iter()
        .filter(|refno| !already_deleted.contains(refno))
        .collect::<BTreeSet<_>>();
    let live_at_target = if orphans.is_empty() {
        BTreeSet::new()
    } else {
        super::e3d_collector::contains_refnos_at(&frozen, &orphans)
            .context("纠正成员审计查冻结目标会话索引失败")?
    };
    // 索引差分是终态：不再按 base 成员表扩枝（见模块头）。
    let audit_deleted = orphans
        .difference(&live_at_target)
        .copied()
        .collect::<BTreeSet<_>>();
    merge_deleted(&mut collected.range_eles, to_sesno, &audit_deleted)?;

    let deleted_refnos = collected
        .range_eles
        .values()
        .flatten()
        .filter(|operation| matches!(operation.detail, EleOperationDetail::Deleted))
        .map(|operation| RefnoEnum::from(operation.refno))
        .collect::<Vec<_>>();
    let hard_delete_rows = deleted_nouns(&deleted_refnos).await?;

    // 纠正的是一个已提交窗口，水位不动；重新投影审计补出的删除后，使用与生产相同的
    // 终态渲染（`element_batch` 批量先删后插，含墓碑与旧 noun 行）幂等重放。
    // 本工具在服务停止后运行，不与批次并发。
    let uda_keys = crate::data_interface::uda_keys::UdaKeyMap::load().await?;
    let (batches, report) = IncrementPipeline::terminal_persist_batches(
        &collected.persist_changes,
        &collected.retired_type_rows,
        dbnum as i32,
        &uda_keys,
    )?;
    if let Some(warning) = report.warning(dbnum) {
        println!("{warning}");
    }
    for sql in &batches {
        super::pe_batch::execute_on(&aios_core::SUL_DB, sql).await?;
    }
    let statements = super::manual_update::build_reverse_index_statements(&collected.range_eles);
    for sql in &statements {
        crate::surreal_retry::execute_surreal_checked(sql, "纠正窗口数据重放")
            .await
            .context("纠正窗口数据重放失败")?;
    }
    super::helper::delete_inst_relate_subtree(&deleted_refnos, 100)
        .await
        .context("纠正窗口模型清理失败")?;
    for (refno, noun) in &hard_delete_rows {
        let pe = refno.to_pe_key();
        // 两个方向都要清：作为成员走边目标（`{pe}->pe_owner`），作为属主走
        // 复合 id 前缀范围。谓词形式的 `WHERE in = {pe}` 在 SurrealDB 2.1 的
        // DELETE 里拿不到 `unique_pe_owner`，退化成边表全扫
        // （`increment_pipeline::render_persist_statements` 同一对语句）。
        let sql = super::pe_batch::transaction(
            dbnum,
            &[refno.refno().to_string()],
            &format!(
                "DELETE {pe}->pe_owner;\n\
                 DELETE pe_owner:[{pe}, NONE]..=[{pe}, ..];\n\
                 DELETE {};\nDELETE {noun}:{};\nDELETE {pe};",
                refno.refno().to_table_key("ATT_UDA"),
                refno.refno()
            ),
        )?;
        super::pe_batch::execute_on(&aios_core::SUL_DB, &sql)
            .await
            .context("纠正窗口主数据硬删除失败")?;
    }

    // 尾事务：水位守卫 + 空间 epoch 推进，一个事务；水位值原样写回（纠正不推进水位）。
    let tail = format!(
        "BEGIN TRANSACTION;\n\
         LET $wm = (SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum});\n\
         IF $wm != {expect_watermark} {{ THROW '纠正提交时水位已变化'; }};\n\
         UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {expect_watermark}, sesno = {expect_watermark};\n\
         {}\n\
         COMMIT TRANSACTION;",
        crate::fast_model::aabb_tree::render_spatial_epoch_bump()
    );
    crate::surreal_retry::execute_surreal_checked(&tail, "纠正窗口尾事务")
        .await
        .context("纠正窗口尾事务失败")?;

    let watermark_after = DbnumState::applied_sesno(dbnum).await?;
    anyhow::ensure!(
        watermark_after == expect_watermark,
        "纠正后水位异常: expected={expect_watermark} actual={watermark_after}"
    );
    verify_hard_deleted(&deleted_refnos).await?;
    let (added, modified, deleted) = operation_counts(&collected.range_eles);
    let mut cleaned_refnos = deleted_refnos
        .iter()
        .map(RefnoEnum::to_pdms_str)
        .collect::<Vec<_>>();
    cleaned_refnos.sort_unstable();
    let mut warnings = collected.warnings;
    warnings.extend(uda_keys.misses_warning(dbnum));
    Ok(WindowRepairReport {
        dbnum,
        from_sesno,
        to_sesno,
        added,
        modified,
        deleted,
        membership_deleted: original_membership_deleted,
        unreachable_rows: audit_deleted.len(),
        watermark_before: state.applied_sesno,
        watermark_after,
        staging_windows: 0,
        cleaned_refnos,
        verification: "watermark unchanged; deleted pe/noun/UDA/owner/model rows absent"
            .to_string(),
        warnings,
    })
}
