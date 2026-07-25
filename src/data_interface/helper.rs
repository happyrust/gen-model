use std::collections::HashSet;

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::anyhow;

/// 查询子树时的分批大小（与 increment_manager 的 QUERY_BATCH_SIZE 一致，避免 SQL 过长）。
const SUBTREE_QUERY_BATCH: usize = 20;

/// 级联删除 inst_relate 及其关联的 geo_relate 和 inst_geo 数据
///
/// 当 replace_mesh 开启时，需要完全删除之前生成的数据，包括：
/// - inst_geo: 几何体节点
/// - geo_relate: 几何关系边
/// - inst_info: 实例信息节点
/// - inst_relate: 实例关系边
///
/// # 参数
/// * `refnos` - 需要删除的 refno 列表
/// * `chunk_size` - 分批处理的大小
///
/// # 删除顺序
/// 1. inst_geo (最外层)
/// 2. geo_relate (关系边)
/// 3. inst_info (信息节点)
/// 4. inst_relate (关系边)
pub async fn delete_inst_relate_cascade(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    for chunk in refnos.chunks(chunk_size) {
        let mut delete_sql_vec = vec![];

        let mut inst_ids: Vec<String> = vec![];
        for refno in chunk {
            inst_ids.push(refno.to_inst_relate_key());
            let delete_sql = format!(
                r#"delete array::flatten(select value [out, id, in] from {}->inst_info->geo_relate);delete from {}"#,
                refno.to_inst_relate_key(),
                refno.to_inst_relate_key()
            );
            delete_sql_vec.push(delete_sql);
        }
        if !delete_sql_vec.is_empty() {
            let sql = delete_sql_vec.join(";");
            match SUL_DB.query(&sql).await {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&sql);
                    return Err(anyhow!(e.to_string()));
                }
            }
        }
    }

    Ok(())
}

/// 收集给定 refno 及其**子树（含已软删节点）**的全部节点，级联删除它们的
/// `inst_relate / geo_relate / inst_info` 几何数据（F1：删除元素几何孤儿清理）。
///
/// 背景：被删元素只做软删（`pe.deleted = true`），几何重生成时被 `!deleted` 过滤，
/// 因而**不会**进入 `save_instance_data(replace_exist)` 的删除集（那只删本次生成的键），
/// 其旧 `inst_relate` 等会成为孤儿。这里按 `pe_owner` 子树（**不**过滤 deleted）收集
/// self + 全部后代，再交给幂等的 [`delete_inst_relate_cascade`]（对无 inst_relate 的
/// refno 为 no-op），从而无论删除是「逐元素记录」还是「只记顶层」都能清干净。
///
/// 子树收集失败按 refno 退化为「仅删传入 refno 自身」（仍优于完全不清理），不 panic。
pub async fn delete_inst_relate_subtree(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    // 先把传入 refno 自身纳入删除集（子树查询失败时的兜底）。
    let mut all: HashSet<RefnoEnum> = refnos.iter().copied().collect();

    for chunk in refnos.chunks(SUBTREE_QUERY_BATCH) {
        let pe_keys = chunk
            .iter()
            .map(|r| r.to_pe_key())
            .collect::<Vec<_>>()
            .join(",");

        // 自身 + 最多 10 层后代（沿 pe_owner 向下），刻意不加 `!in.deleted`：
        // 我们要清理的正是已软删节点。返回 pe 记录链接，反序列化为 RefnoEnum。
        let sql = format!(
            r#"array::distinct(array::flatten(
                select value [
                    [id],
                    array::flatten(
                        select value in
                        from [{0}]<-pe_owner<-(? as p1)<-pe_owner<-(? as p2)<-pe_owner<-(? as p3)
                        <-pe_owner<-(? as p4)<-pe_owner<-(? as p5)<-pe_owner<-(? as p6)<-pe_owner<-(? as p7)
                        <-pe_owner<-(? as p8)<-pe_owner<-(? as p9)<-pe_owner<-(? as p10)
                        where record::exists(in.id)
                    )
                ] from [{0}]
            ))"#,
            pe_keys
        );

        match SUL_DB.query(&sql).await {
            Ok(mut resp) => {
                if let Ok(subtree) = resp.take::<Vec<RefnoEnum>>(0) {
                    all.extend(subtree);
                }
            }
            Err(e) => {
                eprintln!("delete_inst_relate_subtree: 子树收集失败，退化为仅删自身: {e}");
            }
        }
    }

    let all_vec: Vec<RefnoEnum> = all.into_iter().collect();
    delete_inst_relate_cascade(&all_vec, chunk_size).await
}
