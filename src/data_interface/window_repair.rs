//! 已提交净窗口的维护纠正（ADR-036）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aios_core::pdms_types::RefU64;
use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use pdms_io::io::{EleOperationData, EleOperationDetail};
use serde::Serialize;

use super::dbnum_state::DbnumState;
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
    anyhow::ensure!(
        super::staging::lifecycle::registered_windows().is_empty(),
        "存在活动 staging window，终止维护纠正"
    );
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
    let snapshot_token = collected
        .snapshot_token
        .clone()
        .ok_or_else(|| anyhow::anyhow!("纠正窗口缺少冻结 SnapshotToken"))?;
    let mut snapshot = pdms_io::snapshot::DabaconSnapshot::open_verified_at(
        "",
        &snapshot_token,
        u32::try_from(to_sesno).context("纠正目标会话非法")?,
    )
    .context("纠正成员审计重开冻结快照失败")?;

    // 只对持久层已经没有 OWNER 入边的活行做文件成员复核；
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
    let mut unreachable_roots = BTreeSet::new();
    for refno in orphan_candidates(dbnum, &affected_owners).await? {
        if !already_deleted.contains(&refno) && !snapshot.member_alive_at(refno, to_sesno)? {
            unreachable_roots.insert(refno);
        }
    }
    let audit_deleted = if unreachable_roots.is_empty() {
        BTreeSet::new()
    } else {
        snapshot.expand_deleted_membership_roots(&unreachable_roots, from_sesno - 1, to_sesno)?
    };
    merge_deleted(&mut collected.range_eles, to_sesno, &audit_deleted)?;

    let deleted_refnos = collected
        .range_eles
        .values()
        .flatten()
        .filter(|operation| matches!(operation.detail, EleOperationDetail::Deleted))
        .map(|operation| RefnoEnum::from(operation.refno))
        .collect::<Vec<_>>();
    let hard_delete_rows = deleted_nouns(&deleted_refnos).await?;
    let mut window = super::staging::lifecycle::create_window(dbnum, from_sesno, to_sesno).await?;
    let preload_result = window
        .scope(async {
            super::staging::preload::preload_dbnum_state(&state).await?;
            let preload =
                super::staging::preload::plan_model_mutation_preload(&[], &deleted_refnos).await?;
            super::staging::preload::apply_model_mutation_preload(&preload).await
        })
        .await;
    if let Err(error) = preload_result {
        let _ = window.drop_database().await;
        return Err(error.context("纠正窗口预载失败"));
    }
    if let Err(error) =
        IncrementPipeline::stage_parsed_window(&mut window, &collected.range_eles, dbnum).await
    {
        let _ = window.drop_database().await;
        return Err(error.context("纠正窗口数据暂存失败"));
    }
    if let Err(error) = window
        .scope(super::helper::delete_inst_relate_subtree(
            &deleted_refnos,
            100,
        ))
        .await
    {
        let _ = window.drop_database().await;
        return Err(error.context("纠正窗口模型清理暂存失败"));
    }
    if let Err(error) = window
        .scope(async {
            let context = super::staging::active_staging_writes()
                .ok_or_else(|| anyhow::anyhow!("纠正硬删除缺少 staging 写上下文"))?;
            for (refno, noun) in &hard_delete_rows {
                let pe = refno.to_pe_key();
                // 两个方向都要清：作为成员走边目标（`{pe}->pe_owner`），作为属主走
                // 复合 id 前缀范围。谓词形式的 `WHERE in = {pe}` 在 SurrealDB 2.1 的
                // DELETE 里拿不到 `unique_pe_owner`，退化成边表全扫
                // （`increment_pipeline::render_persist_statements` 同一对语句）。
                context
                    .execute(
                        format!(
                            "DELETE {pe}->pe_owner;\n\
                             DELETE pe_owner:[{pe}, NONE]..=[{pe}, ..];\n\
                             DELETE {};\nDELETE {noun}:{};\nDELETE {pe};",
                            refno.refno().to_table_key("ATT_UDA"),
                            refno.refno()
                        ),
                        super::staging::ExecMode::Both,
                    )
                    .await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
    {
        let _ = window.drop_database().await;
        return Err(error.context("纠正窗口主数据硬删除暂存失败"));
    }

    let tail = format!(
        "LET $wm = (SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum});\n\
         IF $wm != {expect_watermark} {{ THROW '纠正提交时水位已变化'; }};\n\
         UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {expect_watermark}, sesno = {expect_watermark};\n\
         {}",
        crate::fast_model::aabb_tree::render_spatial_epoch_bump()
    );
    if let Err(error) = window.commit_to(&SUL_DB, &[], Some(&tail)).await {
        let _ = window.drop_database().await;
        return Err(error.context("纠正窗口写回失败"));
    }
    window.drop_database().await?;

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
        staging_windows: super::staging::lifecycle::registered_windows().len(),
        cleaned_refnos,
        verification: "watermark unchanged; deleted pe/noun/UDA/owner/model rows absent"
            .to_string(),
        warnings: collected.warnings,
    })
}
