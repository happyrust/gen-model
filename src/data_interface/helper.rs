use std::collections::HashSet;

use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefnoEnum, SUL_DB};
use anyhow::anyhow;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealdb::sql::Thing;

use crate::surreal_retry::{
    execute_model_scoped_delete, execute_model_write, execute_surreal_checked,
};

/// 图端点查询只拼 record id；256 个仍远低于 ws 消息上限，同时避免大子树产生数百次往返。
const SUBTREE_QUERY_BATCH: usize = 256;

pub(crate) fn pe_thing_to_refno(value: Thing) -> anyhow::Result<RefnoEnum> {
    let raw = value.to_string();
    let refno = RefnoEnum::from(value);
    anyhow::ensure!(refno.is_valid(), "invalid PE record id: {raw}");
    Ok(refno)
}

pub(crate) async fn collect_pe_subtree_refnos(
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    collect_pe_subtree_refnos_from(&crate::data_interface::staging::active_data_db(), refnos).await
}

pub(crate) async fn collect_pe_subtree_refnos_from(
    db: &Surreal<Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let mut all: HashSet<RefnoEnum> = refnos.iter().copied().collect();
    let mut frontier = refnos.to_vec();

    while !frontier.is_empty() {
        let mut next = Vec::new();
        for chunk in frontier.chunks(SUBTREE_QUERY_BATCH) {
            let pe_keys = chunk
                .iter()
                .map(|r| r.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "array::distinct(array::flatten(SELECT VALUE <-pe_owner.in FROM [{pe_keys}]));"
            );
            let mut response = db.query(&sql).await?.check()?;
            for value in response.take::<Vec<Thing>>(0)? {
                let refno = pe_thing_to_refno(value)?;
                if all.insert(refno) {
                    next.push(refno);
                }
            }
        }
        frontier = next;
    }

    Ok(all)
}

pub(crate) async fn collect_pe_ancestor_refnos_from(
    db: &Surreal<Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    let mut all: HashSet<RefnoEnum> = refnos.iter().copied().collect();
    let mut frontier = refnos.to_vec();

    while !frontier.is_empty() {
        let mut next = Vec::new();
        for chunk in frontier.chunks(SUBTREE_QUERY_BATCH) {
            let pe_keys = chunk
                .iter()
                .map(|r| r.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "array::distinct(array::flatten(SELECT VALUE ->pe_owner.out FROM [{pe_keys}]));"
            );
            let mut response = db.query(&sql).await?.check()?;
            for value in response.take::<Vec<Thing>>(0)? {
                let refno = pe_thing_to_refno(value)?;
                if all.insert(refno) {
                    next.push(refno);
                }
            }
        }
        frontier = next;
    }

    Ok(all)
}

/// 渲染单个 refno 的级联删除，模型关系与该 refno 的直管出边共一个事务。
///
/// 事务不是为了跨 refno 的原子性（那反而会让一个坏 refno 拖垮整批），而是因为
/// 「删边」与「按引用计数回收 inst_info」之间**不能存在可观察的中间态**：清理条件
/// 读的正是刚被删掉的那条 `inst_relate`。若边已删而 `if` 块没跑（语句报错、连接
/// 中断、服务端重启），重试时 `$old_inst` 只会读到 `NONE`，整段清理被静默跳过，
/// 而函数照样返回 `Ok`——inst_info 与 geo_relate 就此永久孤儿，且无告警。
/// 包进事务后这种半执行会整体回滚，重试从干净状态开始，可自愈。
///
/// `inst_info` 本身用显式 `delete $old_inst` 回收，而不是靠 `geo_relate` 三元组的
/// `in` 端顺带删除：几何生成半途失败会留下**没有任何 `geo_relate` 边**的 `inst_info`，
/// 顺带删除对它是空集、永远删不掉（2026-07-26 审计 B2）。
///
/// **`geo_relate` 的 `out` 端（`inst_geo`）不删。** 它是内容寻址的——id 就是单位几何的
/// 哈希——因而跨 `inst_info` 共享，而这里的引用计数守卫只数得到 `inst_info` 自己的引用
/// （`<-inst_relate`），数不到还有谁指着那块几何。跟着删的后果是**跨生成根的数据损坏**：
/// 一个根的清理把另一个根正在用的 `inst_geo` 抹掉，两边都不会报错。最极端的是隐含直管段
/// ——`TUBI_GEO_HASH` / `BOXI_GEO_HASH` 是全局常量，全项目所有管段共用那一个单位圆柱。
///
/// 不删的代价只是泄漏，而且有界：同样的几何算出同样的哈希，反复重生成同一个根不会长出
/// 新行，只有几何参数真的变成新值才会。`aabb` / `trans` / `vec3` 这几张内容寻址表一直
/// 就是这么处理的。真要回收，正确的位置是一次按全库引用计数的后台 sweep，而不是写入
/// 路径上的单边删除。
///
/// 投影保留 `[..]` + `array::flatten` 的原形、只摘掉 `out`：`DELETE` 的入参形状不变，
/// 改动只落在删除集本身。
fn render_cascade_delete(inst_relate_key: &str, pe_key: &str) -> String {
    format!(
        r#"BEGIN TRANSACTION;
let $old_inst = (select value out from {inst_relate_key})[0];
delete from {inst_relate_key};
delete {pe_key}->tubi_relate;
if $old_inst != none and array::len($old_inst<-inst_relate) = 0 {{
    delete array::flatten(select value [id] from $old_inst->geo_relate);
    delete $old_inst;
}};
COMMIT TRANSACTION;"#
    )
}

/// 级联删除 inst_relate 及其关联的 geo_relate / inst_info
///
/// 当 replace_mesh 开启时，需要删除之前生成的数据：
/// - inst_relate: 实例关系边（目标元素自己的那条）
/// - geo_relate: 几何关系边
/// - inst_info: 实例信息节点（仅在已无其他 inst_relate 引用时）
///
/// **不含 `inst_geo`**：它是内容寻址的共享节点，写入路径上不做单边回收，理由见
/// [`render_cascade_delete`]。
///
/// # 参数
/// * `refnos` - 需要删除的 refno 列表
/// * `chunk_size` - 分批处理的大小
///
/// # 删除顺序
/// 1. inst_relate（仅删除目标元素的关系）
/// 2. 若 inst_info 已无其他 inst_relate 引用，再删除其 geo_relate 边与 inst_info 自身
///
/// inst_info 可能由相同 catalogue hash 的多个元素共享，不能在仍有引用时删除。
/// 每个 refno 的这两步在一个事务里（见 [`render_cascade_delete`]），失败整体回滚。
///
/// 共享正是并发下的冲突来源：两个各自重生成的根同时删到同一个 `inst_info`，
/// SurrealDB 的乐观事务会让后提交的那个报「read or write conflict」。整段 SQL 幂等
/// ——边已删时 `$old_inst` 读到 NONE，清理块整个跳过——所以交给
/// [`execute_surreal_checked`] 退避重试即可，不必在调用方另设补偿。
pub async fn delete_inst_relate_cascade(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        chunk_size > 0,
        "delete chunk_size must be greater than zero"
    );

    for chunk in refnos.chunks(chunk_size) {
        let mut delete_sql_vec = vec![];

        for refno in chunk {
            delete_sql_vec.push(render_cascade_delete(
                &refno.to_inst_relate_key(),
                &refno.to_pe_key(),
            ));
        }
        if crate::data_interface::staging::active_staging_writes().is_some() {
            for sql in delete_sql_vec {
                execute_model_scoped_delete(&sql, "delete model relations").await?;
            }
        } else if !delete_sql_vec.is_empty() {
            execute_surreal_checked(&delete_sql_vec.join("\n"), "delete model relations").await?;
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
/// 子树收集失败必须上抛，让 pending/补偿任务保留并重试；否则只删根自身会把子件旧
/// 模型永久遗留，同时错误地把任务标记为完成。
pub async fn delete_inst_relate_subtree(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<()> {
    if refnos.is_empty() {
        return Ok(());
    }

    // 自身 + 全部后代（沿 pe_owner 向下），刻意不加 `!in.deleted`：
    // 我们要清理的正是已软删节点。
    let all = collect_pe_subtree_refnos(refnos)
        .await
        .map_err(|e| anyhow!("collect deleted PE subtree failed: {e}"))?;

    let all_vec: Vec<RefnoEnum> = all.into_iter().collect();
    delete_inst_relate_cascade(&all_vec, chunk_size).await?;
    delete_room_membership(&all_vec, chunk_size).await
}

/// 这些 refno 里，当前**确实还有** `inst_relate` 行的那些。
///
/// 只是为了让清理的数量与日志说的是同一件事：候选集来自 pe 子树，其中绝大多数元素
/// 本来就没有几何，对它们调级联删除是空操作，但会把「清理了 N 行」虚报成子树大小。
async fn existing_inst_relate_refnos(
    refnos: &[RefnoEnum],
    chunk_size: usize,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut existing = Vec::new();
    for chunk in refnos.chunks(chunk_size) {
        let keys = chunk
            .iter()
            .map(RefnoEnum::to_inst_relate_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = crate::data_interface::staging::active_data_db()
            .query(format!(
                "SELECT VALUE in FROM [{keys}] WHERE record::exists(id);"
            ))
            .await
            .map_err(|error| anyhow!("查询现存模型关系失败: {error}"))?
            .check()
            .map_err(|error| anyhow!("查询现存模型关系语句失败: {error}"))?;
        for thing in response
            .take::<Vec<Thing>>(0)
            .map_err(|error| anyhow!("解析现存模型关系失败: {error}"))?
        {
            existing.push(pe_thing_to_refno(thing)?);
        }
    }
    Ok(existing)
}

/// 重生成收尾：清掉这些生成根名下**本轮没有产出任何几何**的旧模型行。
///
/// `save_instance_data(replace)` 的删除集是从本次产物推出来的，所以它只替换得掉「这次
/// 也生成了」的那些行。上一版画得出、这一版画不出的元素——参数改到不再产生几何、分支
/// 尾部不再有隐含直管段——旧行会一直留着，而模型里已经没有它了。
///
/// 判据是「仍挂在这个根下、却不在本轮产物里」。搬走或被删的元素**不**在这个集合里：
/// `query_deep_visible_inst_refnos` 是对 `pe` 的活子树查询，它们早已不在根下，那条路
/// 归 [`delete_inst_relate_subtree`]。
///
/// **只在生成与写入都成功之后调用。** 这个集合分不清「真的不画了」与「本轮生成没做
/// 出来」，它的正确性押在「生成成功 ⇒ 产物完整」上（2026-08-05 决策；ADR-014 的
/// 「保留旧显示」因此收窄为「生成失败时保留」，生成成功时以产物为准）。
///
/// 任何一步失败都上抛，让生成根保留 pending 并重试。尤其是 `inst_relate` 已删、房间边或
/// 空间树清理失败时，下一轮仍须按原始候选补完收尾，不能因 stale 已空提前返回成功。
pub async fn prune_roots_stale_model_rows(
    roots: &[RefnoEnum],
    produced: &HashSet<RefnoEnum>,
    chunk_size: usize,
) -> anyhow::Result<usize> {
    let mut candidates: HashSet<RefnoEnum> = HashSet::new();
    for &root in roots {
        let subtree = aios_core::query_deep_visible_inst_refnos(root)
            .await
            .map_err(|error| anyhow!("生成根 {root} 的子树查询失败: {error}"))?;
        candidates.extend(
            subtree
                .into_iter()
                .filter(|refno| !produced.contains(refno)),
        );
    }
    if candidates.is_empty() {
        return Ok(0);
    }

    // 原始候选是整套收尾的权威目标；排序固定，日志、分块和重放才对得上。
    let mut candidates: Vec<RefnoEnum> = candidates.into_iter().collect();
    candidates.sort_by_key(RefnoEnum::to_string);
    let mut stale = existing_inst_relate_refnos(&candidates, chunk_size).await?;
    // 顺序固定，日志与重放才对得上。
    stale.sort_by_key(RefnoEnum::to_string);

    if !stale.is_empty() {
        println!(
            "重生成收尾：清理 {} 行本轮未产出几何的旧模型关系（例如 {}）",
            stale.len(),
            stale
                .iter()
                .take(5)
                .map(RefnoEnum::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        delete_inst_relate_cascade(&stale, chunk_size).await?;
    }
    // 元素还在，只是不再有几何——那它也不再属于任何房间，空间树上同样不该留着它。
    // 始终按原始候选清：若上次已删 inst_relate、却在这里失败，本轮 stale 会是空集，仍要
    // 把未完成的房间边与空间树清理补上。
    delete_room_membership(&candidates, chunk_size).await?;
    Ok(stale.len())
}

/// 渲染一批被删元素的房间归属清理。
///
/// **两个方向都要删。** 作为成员，元素有 `room_relate` 入边；如果它本身是一块 PANE，
/// 它还是某间房的面板，另有 `room_relate` 出边与 `room_panel_relate` 入边。这里不按
/// noun 分情况：`pe.noun` 此刻可能已随软删一起不可靠，而对非面板元素那两条子句本来
/// 就是空操作。
/// 走图遍历，**不要**写成 `WHERE in IN [..] OR out IN [..]`：那个 `OR` 会让
/// `(in, out)` 复合索引整个失效，退化成边表全扫。同一个构件的 2 条边，全扫写法实测
/// 558.8ms，图遍历 2.1ms（`room_relate` 现有 6.6 万条边，全扫成本还随边数线性涨）。
///
/// 图遍历要写成 DELETE 的**边目标**（`{pe_key}<->{table}`），不能塞进目标表达式
/// （`DELETE array::flatten(SELECT ... FROM [..])`）：后者的目标由执行时刻的查询结果
/// 决定，ReplaySafe R1 整类拒绝，而暂存窗口里这道校验在执行之前——语句连跑都不会跑，
/// 于是 staged 模式下**每一次**删除清理都必然失败、整窗口零落盘（2026-08-11 现场）。
/// `<->` 与 `array::concat(->, <-)` 覆盖的方向完全相同。
fn render_room_membership_delete(pe_keys: &[String]) -> String {
    let targets = |table: &str| {
        pe_keys
            .iter()
            .map(|key| format!("{key}<->{table}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "DELETE {};\nDELETE {};",
        targets("room_relate"),
        targets("room_panel_relate")
    )
}

/// 同上，但把 spatial epoch 的递增并进同一个事务——本块确实要从空间树上摘条目时用。
///
/// 直写删除不产生 `spatial_reconcile` 意图行，epoch 是它在库侧留下的**唯一**痕迹。
/// 少 bump 一次，「摘完树、落盘前崩溃」的重启就会看到 sidecar 与库指纹相等、按 Reuse
/// 复用一棵还留着被删构件的树，启动全量房间重建随即把幽灵构件按旧包围盒重新收编进
/// `room_relate`——ADR-010 D4 修掉的缺陷借崩溃复活，而 `DeleteCleanup` 任务早已 done，
/// 没有任何重放会再清一次。
fn render_room_membership_delete_transaction(pe_keys: &[String]) -> String {
    crate::data_interface::increment_pipeline::wrap_in_transaction(&[
        render_room_membership_delete(pe_keys),
        crate::fast_model::aabb_tree::render_spatial_epoch_bump(),
    ])
    .expect("删除事务至少包含边删除与 epoch bump")
}

/// 清掉被删元素在房间归属里留下的一切，并把它们从空间树上摘掉（ADR-010 §4）。
///
/// 删除是房间增量里唯一不走队列的分支：被删元素没有新的包围盒，「AABB 变了」这个触发源
/// 对它根本不成立，所以由删除路径当场清边。此前生产路径上**从来没有人删过**
/// `room_relate`——全仓只有夹具清理里有一条删除语句，于是房间归属只增不减。
///
/// 刻意不与 [`delete_inst_relate_cascade`] 合成一个事务：那个函数同时服务于重生成时的
/// 「先删旧几何再写新几何」，而那条路径上元素还活着，房间边不该被动。两段各自幂等，
/// 中间崩了 `DeleteCleanup` 任务会重试，从头再走一遍即可收敛。
///
/// 窗口外分支按块走「锁下探测 → 边删除与 epoch bump 同事务 → 摘树 → 标脏」
/// （见 [`render_room_membership_delete_transaction`]）：直写路径的变更必须在库里留下
/// 痕迹，崩溃后启动判据才认得出「树文件之后还有过空间提交」。
async fn delete_room_membership(refnos: &[RefnoEnum], chunk_size: usize) -> anyhow::Result<()> {
    // 树上留着已删元素的包围盒，`locate_intersecting_bounds` 会继续把它当候选返回，
    // 于是重算时一个已经不存在的构件仍会被算进某间房（缺陷 D4）。
    if let Some(context) = crate::data_interface::staging::active_staging_writes() {
        for chunk in refnos.chunks(chunk_size) {
            let pe_keys = chunk.iter().map(RefnoEnum::to_pe_key).collect::<Vec<_>>();
            execute_model_write(
                &render_room_membership_delete(&pe_keys),
                "delete room membership",
            )
            .await?;
        }
        // 窗口内不动树：摘除意图寄存进上下文，随尾事务连同 epoch bump 一起提交，
        // 提交后由 `apply_deferred_spatial_mutations` 收敛。
        context.defer_spatial_remove(refnos).await;
        return Ok(());
    }

    for chunk in refnos.chunks(chunk_size) {
        let pe_keys = chunk.iter().map(RefnoEnum::to_pe_key).collect::<Vec<_>>();
        let stale: HashSet<aios_core::RefU64> = chunk.iter().map(RefnoEnum::refno).collect();

        // 锁序（一致性闭环方案 D6）：SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE，
        // 与 staged 提交后收敛、指针重建换树段、快照发布同一条串行线。
        // 写锁跨「探测 → 提交 → 摘除」：探测放在锁下，「要不要 bump」与「树到底动
        // 没动」才由同一个快照裁决，不会出现 bump 了却无人落盘追平（白白触发下次
        // 重建）、或动了树却没 bump（回到静默漂移）的错位。
        let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
        let mut tree = GLOBAL_AABB_TREE.write().await;
        let present = tree.iter().any(|bbox| stale.contains(&bbox.refno));
        if present {
            execute_surreal_checked(
                &render_room_membership_delete_transaction(&pe_keys),
                "delete room membership with spatial epoch bump",
            )
            .await?;
            // 崩溃窗口 ①（direct 删除侧，一致性闭环方案 §8）：事务已提交
            // （epoch 已 bump）、树未摘除——重启判据必然认出失配并重建。
            crate::fast_model::spatial_state::failpoint("spatial_direct_after_db_commit");
        } else {
            // 树上本来就没有这些条目，删边不改变「树应有内容」，不作废别人的树文件。
            execute_model_write(
                &render_room_membership_delete(&pe_keys),
                "delete room membership",
            )
            .await?;
        }
        if tree.remove_by_refnos(&stale) > 0 {
            // 摘除同样是「内存树相对项目树文件的未持久化变更」：不标脏的话，
            // 重启读回旧文件，被删构件会重新以候选身份出现在房间重算里。
            crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第一次尝试可能已经删掉 inst_relate、随后才在房间/空间树收尾失败。重试时 stale
    /// 是空集，但原始 candidates 仍须继续清理，不能提前报成功。
    #[test]
    fn stale_empty_retry_still_cleans_room_and_tree_for_original_candidates() {
        let source = include_str!("helper.rs");
        let body = source
            .split_once("pub async fn prune_roots_stale_model_rows(")
            .expect("prune function")
            .1
            .split_once("/// 渲染一批被删元素的房间归属清理")
            .expect("prune tail")
            .0;

        assert!(
            !body.contains("if stale.is_empty()"),
            "stale 空集不得跳过补偿收尾: {body}"
        );
        assert!(
            body.contains("delete_room_membership(&candidates, chunk_size).await?"),
            "房间/空间树必须始终按原始候选补完: {body}"
        );
    }

    /// 直写删除必须把两个方向的房间边删除与 spatial epoch bump 放进同一个事务。
    ///
    /// 直写路径不产生 `spatial_reconcile` 意图行，epoch 是它在库侧留下的唯一痕迹：
    /// 少 bump 一次，「摘完树、落盘前崩溃」的重启就会按指纹相等复用一棵还留着被删
    /// 构件的树，启动全量房间重建随即把幽灵构件按旧包围盒重新收编（ADR-010 D4 借
    /// 崩溃复活，而 DeleteCleanup 任务早已 done，没有重放会再清一次）。
    #[test]
    fn direct_delete_pairs_the_room_edge_removal_with_the_spatial_epoch_bump() {
        let sql = render_room_membership_delete_transaction(&["pe:7997_1".to_string()]);

        assert!(sql.starts_with("BEGIN TRANSACTION;"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
        let member_at = sql
            .find("pe:7997_1<->room_relate")
            .expect("成员边（两个方向）必须删");
        let panel_at = sql
            .find("pe:7997_1<->room_panel_relate")
            .expect("面板边必须删");
        let bump_at = sql.find("spatial_epoch:current").expect("epoch bump 缺失");
        assert!(
            member_at < bump_at && panel_at < bump_at,
            "bump 必须与两个方向的边删除同处一个事务: {sql}"
        );
    }

    /// 窗口外删除的次序纪律：写锁 → 锁下探测 → 提交 → 摘树 → 标脏。
    ///
    /// 探测放在锁下，「要不要 bump」与「树到底动没动」才由同一个快照裁决，不会出现
    /// bump 了却无人落盘追平、或动了树却没 bump 的错位；bump 必须先于
    /// `remove_by_refnos`，崩溃时才只会多做一次指针重建而不是静默漂移。暂存分支反过来
    /// 一条 bump 都不许有——那条路的 epoch 由窗口尾事务与意图行统一收口，在这里提前
    /// 递增等于拿未提交的窗口变更去作废别人已经落好的树文件。回退即红。
    #[test]
    fn direct_delete_bumps_under_the_tree_lock_before_it_evicts() {
        let source = include_str!("helper.rs");
        let body = source
            .split_once("async fn delete_room_membership(")
            .expect("delete_room_membership must exist")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(body, _)| body)
            .unwrap_or(source);

        let staged = body
            .split_once("defer_spatial_remove")
            .expect("暂存分支必须把摘除寄存进窗口")
            .0;
        assert!(
            !staged.contains("render_room_membership_delete_transaction"),
            "暂存分支不得自行 bump，epoch 归窗口尾事务: {staged}"
        );

        let serial_at = body
            .find("lock_spatial_serial().await")
            .expect("窗口外分支必须先取空间串行锁（锁序 D6）");
        let lock_at = body
            .find("GLOBAL_AABB_TREE.write().await")
            .expect("窗口外分支必须持写锁");
        let probe_at = body
            .find("tree.iter().any(")
            .expect("present 探测必须在锁下进行");
        let bump_at = body
            .find("render_room_membership_delete_transaction")
            .expect("确有条目时必须走带 bump 的事务");
        let evict_at = body.find("remove_by_refnos").expect("摘树缺失");
        let dirty_at = body.find("mark_aabb_tree_dirty").expect("标脏缺失");

        assert!(
            serial_at < lock_at,
            "锁序必须 SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE: {body}"
        );
        assert!(lock_at < probe_at, "探测必须在锁下: {body}");
        assert!(probe_at < bump_at, "bump 与否由锁下那一个快照裁决: {body}");
        assert!(
            bump_at < evict_at,
            "bump 必须先于摘树，否则崩溃即静默漂移: {body}"
        );
        assert!(evict_at < dirty_at, "标脏在摘树之后: {body}");
    }

    #[test]
    fn cascade_delete_keeps_the_edge_delete_and_refcount_gc_in_one_transaction() {
        let sql = render_cascade_delete("inst_relate:7997_1", "pe:7997_1");

        crate::data_interface::staging::replay_safe::validate_scoped_delete_transaction(&sql)
            .expect("级联删除必须能进入暂存 journal");

        assert!(sql.starts_with("BEGIN TRANSACTION;"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
        assert!(
            sql.contains("delete pe:7997_1->tubi_relate;"),
            "BRAN cleanup must remove its straight-run edges: {sql}"
        );
        // The GC condition reads the edge this block just deleted, so a commit
        // boundary between the two would strand inst_info on retry.
        let delete_at = sql
            .find("delete from inst_relate:7997_1")
            .expect("edge delete");
        let gc_at = sql
            .find("array::len($old_inst<-inst_relate)")
            .expect("gc guard");
        assert!(delete_at < gc_at, "{sql}");
        assert!(
            sql.find("let $old_inst").expect("binding") < delete_at,
            "{sql}"
        );
    }

    /// B2（2026-07-26 审计 round2）：`inst_info` 必须被显式删除。若只靠
    /// `geo_relate` 三元组的 `in` 端顺带删除，一个没有任何 `geo_relate` 边的
    /// `inst_info`（几何生成半途失败的残留）将永远不被回收。
    #[test]
    fn cascade_delete_reclaims_inst_info_even_without_geo_relate_edges() {
        let sql = render_cascade_delete("inst_relate:7997_1", "pe:7997_1");

        let explicit_gc_at = sql
            .find("delete $old_inst;")
            .expect("inst_info must be deleted explicitly, not via geo_relate rows");
        // 显式回收发生在引用计数守卫之内、几何三元组清理之后。
        let guard_at = sql
            .find("array::len($old_inst<-inst_relate)")
            .expect("gc guard");
        let geo_rows_at = sql
            .find("from $old_inst->geo_relate")
            .expect("geo triple cleanup");
        assert!(guard_at < explicit_gc_at, "{sql}");
        assert!(geo_rows_at < explicit_gc_at, "{sql}");
        assert!(
            !sql.contains("[out, id, in]"),
            "inst_info must not ride the geo_relate triple delete: {sql}"
        );
    }

    /// 共享的 `inst_geo` 不许跟着 `inst_info` 一起删。
    ///
    /// 它是内容寻址的（id = 单位几何的哈希），跨 `inst_info` 共享；而这里的引用计数
    /// 守卫只数 `inst_info` 自己的引用（`<-inst_relate`），数不到谁还指着那块几何。
    /// 跟着删就是跨生成根的数据损坏，且两边都不报错——最极端的是隐含直管段，
    /// `TUBI_GEO_HASH` / `BOXI_GEO_HASH` 是全局常量，全项目所有管段共用一个单位圆柱。
    ///
    /// 反过来「不删」只是有界泄漏：同样的几何算出同样的哈希，重生成同一个根不长新行。
    #[test]
    fn cascade_delete_never_reclaims_shared_content_addressed_geometry() {
        let sql = render_cascade_delete("inst_relate:7997_1", "pe:7997_1");

        assert!(
            sql.contains("from $old_inst->geo_relate"),
            "边本身仍要清: {sql}"
        );
        assert!(
            !sql.contains("[out, id]"),
            "geo_relate 的 out 端是共享的 inst_geo，不得随边一起删: {sql}"
        );
        assert!(
            sql.contains("delete $old_inst;"),
            "inst_info 仍按引用计数回收，它的引用集就是 <-inst_relate: {sql}"
        );
    }

    /// 删除是房间增量里唯一不走队列的分支（ADR-010 §4），两个方向都得清：作为成员是
    /// `room_relate` 入边，作为面板还有出边和 `room_panel_relate`。少清一个方向，
    /// 房间归属就会留下指向已删元素的悬空边，而 `fn::room_relate_of` 照样会把它取出来。
    #[test]
    fn deleting_an_element_clears_room_membership_in_both_directions() {
        let sql =
            render_room_membership_delete(&["pe:7997_1".to_string(), "pe:7997_2".to_string()]);

        for table in ["room_relate", "room_panel_relate"] {
            for key in ["pe:7997_1", "pe:7997_2"] {
                assert!(
                    sql.contains(&format!("{key}<->{table}")),
                    "{table} 的两个方向都要取到: {sql}"
                );
            }
        }
        // 边表全扫会让删除随边数线性变慢，回退即红。
        assert!(!sql.contains(" OR out IN ["), "{sql}");
        // 被 validator 拒的语句连执行都到不了，而拒绝是静默的：准入本身必须有断言，
        // 否则 staged 模式下每一次删除清理都失败、窗口零落盘，外面只看到「没反应」。
        crate::data_interface::staging::replay_safe::validate_statement(&sql)
            .unwrap_or_else(|error| panic!("房间归属清理必须能进 journal：{error}\n{sql}"));
    }

    #[tokio::test]
    async fn delete_rejects_zero_chunk_size() {
        let error = delete_inst_relate_cascade(&[RefnoEnum::from("1/1")], 0)
            .await
            .expect_err("zero chunk size must not panic");
        assert!(error.to_string().contains("chunk_size"));
    }

    #[tokio::test]
    #[ignore = "manual live: requires the configured AvevaMarineSample Surreal database"]
    async fn live_deleted_branch_subtree_includes_known_damp_child() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let branch = RefnoEnum::from("24381/100817");
        let damp = RefnoEnum::from("24381/100819");
        let subtree = collect_pe_subtree_refnos(&[branch])
            .await
            .expect("collect BRAN subtree");
        assert!(subtree.contains(&branch));
        assert!(subtree.contains(&damp));
    }

    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_shared_inst_info_is_deleted_only_after_last_reference() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let first = RefnoEnum::from("4000000000/1");
        let second = RefnoEnum::from("4000000000/2");
        let cleanup = "delete inst_relate:4000000000_1, inst_relate:4000000000_2, \
            geo_relate:zz_increment_cleanup_shared, inst_info:zz_increment_cleanup_shared, \
            inst_geo:zz_increment_cleanup_shared, pe:4000000000_1, pe:4000000000_2;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_1;
            create pe:4000000000_2;
            create inst_info:zz_increment_cleanup_shared;
            create inst_geo:zz_increment_cleanup_shared;
            relate pe:4000000000_1->inst_relate:4000000000_1->inst_info:zz_increment_cleanup_shared;
            relate pe:4000000000_2->inst_relate:4000000000_2->inst_info:zz_increment_cleanup_shared;
            relate inst_info:zz_increment_cleanup_shared->geo_relate:zz_increment_cleanup_shared
                ->inst_geo:zz_increment_cleanup_shared;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create shared graph")
            .check()
            .expect("valid setup");

        delete_inst_relate_cascade(&[first], 1)
            .await
            .expect("delete first reference");
        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_2').id != none,
                    inst_info:zz_increment_cleanup_shared.id != none,
                    geo_relate:zz_increment_cleanup_shared.id != none,
                    inst_geo:zz_increment_cleanup_shared.id != none
                ];",
            )
            .await
            .expect("query shared graph after first delete")
            .check()
            .expect("valid first-delete query");
        let after_first = response
            .take::<Vec<bool>>(0)
            .expect("decode first-delete state");

        delete_inst_relate_cascade(&[second], 1)
            .await
            .expect("delete last reference");
        let mut response = SUL_DB
            .query(
                "return [
                    inst_info:zz_increment_cleanup_shared.id != none,
                    geo_relate:zz_increment_cleanup_shared.id != none,
                    inst_geo:zz_increment_cleanup_shared.id != none
                ];",
            )
            .await
            .expect("query shared graph after last delete")
            .check()
            .expect("valid last-delete query");
        let after_last = response
            .take::<Vec<bool>>(0)
            .expect("decode last-delete state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup shared graph")
            .check()
            .expect("valid cleanup");
        assert_eq!(after_first, vec![true, true, true, true]);
        // inst_info 与它的 geo_relate 边在最后一个引用消失后回收；`inst_geo` 留着
        // ——它是内容寻址的共享节点，写入路径上不做单边回收（见 render_cascade_delete）。
        assert_eq!(after_last, vec![false, false, true]);
    }

    /// 临时探针：从 `AIOS_PROBE_SQL` 读 `;;` 分隔的查询并打印截断结果，用完即删。
    #[tokio::test]
    #[ignore = "temp probe: run ad-hoc SQL from AIOS_PROBE_SQL"]
    async fn probe_live_sql() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let batch = std::env::var("AIOS_PROBE_SQL").expect("set AIOS_PROBE_SQL");
        for sql in batch.split(";;").map(str::trim).filter(|s| !s.is_empty()) {
            let mut response = SUL_DB
                .query(sql)
                .await
                .expect("probe query")
                .check()
                .expect("probe statement");
            let v: surrealdb::Value = response.take(0).expect("take probe");
            let s = format!("{v:?}");
            println!("== {sql}\n{}\n", &s[..s.len().min(4000)]);
        }
    }

    /// B2 Live：几何生成半途失败会留下没有任何 `geo_relate` 边的 `inst_info`。
    /// 级联删除必须把它一并回收，而不是因「几何三元组为空集」而永久遗留。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_inst_info_without_geo_relate_is_reclaimed() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let orphan = RefnoEnum::from("4000000000/3");
        let cleanup =
            "delete inst_relate:4000000000_3, inst_info:zz_no_geo_orphan, pe:4000000000_3;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_3;
            create inst_info:zz_no_geo_orphan;
            relate pe:4000000000_3->inst_relate:4000000000_3->inst_info:zz_no_geo_orphan;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create orphan graph")
            .check()
            .expect("valid setup");

        delete_inst_relate_cascade(&[orphan], 1)
            .await
            .expect("delete lone reference");

        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_3').id != none,
                    inst_info:zz_no_geo_orphan.id != none
                ];",
            )
            .await
            .expect("query orphan state")
            .check()
            .expect("valid orphan query");
        let state = response.take::<Vec<bool>>(0).expect("decode orphan state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup orphan graph")
            .check()
            .expect("valid cleanup");
        assert_eq!(
            state,
            vec![false, false],
            "an inst_info with no geo_relate edges must still be reclaimed"
        );
    }

    /// 「all model nodes」指的是这几个元素**自己的**行：`inst_relate`、它们独占的
    /// `inst_info`、以及那些 `inst_info` 的 `geo_relate` 边。`inst_geo` 不在其列
    /// ——内容寻址的共享节点，写入路径上不做单边回收（见 [`render_cascade_delete`]）。
    /// 名字保留原样，因为两份测试计划文档按名字引用了它。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_soft_deleted_subtree_removes_all_model_nodes() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let root = RefnoEnum::from("4000000000/10");
        let cleanup = "delete pe:4000000000_10, pe:4000000000_11, pe:4000000000_12, \
            inst_relate:4000000000_10, inst_relate:4000000000_11, inst_relate:4000000000_12, \
            inst_info:zz_delete_10, inst_info:zz_delete_11, inst_info:zz_delete_12, \
            geo_relate:zz_delete_10, geo_relate:zz_delete_11, geo_relate:zz_delete_12, \
            inst_geo:zz_delete_10, inst_geo:zz_delete_11, inst_geo:zz_delete_12;";
        let setup = format!(
            "{cleanup}
            create pe:4000000000_10 set deleted = true;
            create pe:4000000000_11 set deleted = true;
            create pe:4000000000_12 set deleted = true;
            relate pe:4000000000_11->pe_owner->pe:4000000000_10;
            relate pe:4000000000_12->pe_owner->pe:4000000000_11;
            create inst_info:zz_delete_10;
            create inst_info:zz_delete_11;
            create inst_info:zz_delete_12;
            create inst_geo:zz_delete_10;
            create inst_geo:zz_delete_11;
            create inst_geo:zz_delete_12;
            relate pe:4000000000_10->inst_relate:4000000000_10->inst_info:zz_delete_10;
            relate pe:4000000000_11->inst_relate:4000000000_11->inst_info:zz_delete_11;
            relate pe:4000000000_12->inst_relate:4000000000_12->inst_info:zz_delete_12;
            relate inst_info:zz_delete_10->geo_relate:zz_delete_10->inst_geo:zz_delete_10;
            relate inst_info:zz_delete_11->geo_relate:zz_delete_11->inst_geo:zz_delete_11;
            relate inst_info:zz_delete_12->geo_relate:zz_delete_12->inst_geo:zz_delete_12;"
        );
        SUL_DB
            .query(setup)
            .await
            .expect("create deleted subtree")
            .check()
            .expect("valid setup");

        delete_inst_relate_subtree(&[root], 10)
            .await
            .expect("delete model subtree");
        let mut response = SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000000_10').id != none,
                    type::thing('inst_relate', '4000000000_11').id != none,
                    type::thing('inst_relate', '4000000000_12').id != none,
                    inst_info:zz_delete_10.id != none,
                    inst_info:zz_delete_11.id != none,
                    inst_info:zz_delete_12.id != none,
                    geo_relate:zz_delete_10.id != none,
                    geo_relate:zz_delete_11.id != none,
                    geo_relate:zz_delete_12.id != none,
                    inst_geo:zz_delete_10.id != none,
                    inst_geo:zz_delete_11.id != none,
                    inst_geo:zz_delete_12.id != none
                ];",
            )
            .await
            .expect("query deleted model subtree")
            .check()
            .expect("valid deleted-subtree query");
        let state = response
            .take::<Vec<bool>>(0)
            .expect("decode deleted-subtree state");

        SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup deleted subtree")
            .check()
            .expect("valid cleanup");
        // 前九项（inst_relate / inst_info / geo_relate）全清；最后三项是 `inst_geo`,
        // 内容寻址的共享节点，写入路径上不做单边回收（见 render_cascade_delete）。
        let mut expected = vec![false; 9];
        expected.extend([true; 3]);
        assert_eq!(state, expected);
    }

    /// 2026-08-12 epoch 痕迹方案 §6 场景 1 的 live 验收：直写删除 → 落盘前
    /// 「崩溃」→ 重启按指针重建，幽灵条目消失、指纹追平、/health 全程说得出话。
    ///
    /// 崩溃用「清空内存树 + 重新走启动加载」模拟：崩溃真正丢失的只有进程态
    /// （内存树与脏标记），磁盘文件的陈旧与库侧 epoch 的痕迹与真实崩溃逐字节
    /// 相同，恢复判据走的又是同一个 `load_project_tree_verified`——语义等价，
    /// 差的只是没真的 kill 进程。真杀进程的剧本仍归 W5 门禁的故障注入轮。
    ///
    /// 注意：本用例会推进沙箱库的 spatial epoch、并以「按库指针重建」的结果
    /// 覆盖项目树文件（终态自洽）。用 testbed 沙箱跑（`DB_OPTION_FILE` 指向
    /// `python/testbed/DbOption-pytest`），别对着正式库。单独 `--exact` 跑最
    /// 可靠：`startup_verdict` 是进程内一次性记录，与其它会触发启动加载的
    /// 用例同进程会互相占位（本用例族期望值相同，混跑不误判，但别依赖）。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: 用 testbed 沙箱库跑（见 python/testbed/README.md）；会推进 epoch 并重建项目树文件"]
    async fn live_direct_delete_crash_before_persist_recovers_by_rebuild() {
        use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        // 本用例自己灌树（幽灵构件）、不走启动装载：按状态机的测试装载模式显式
        // 声明，否则进程态停在 Uninitialized，下面的基线 persist 会被发布门拒绝
        // （一致性闭环方案 §2 步骤 0；用例写于状态机落地之前，2026-08-12 补）。
        crate::fast_model::spatial_state::mark_spatial_tree_fixture_preloaded();
        let pending =
            crate::data_interface::side_effect_pending::SideEffectCompensator::has_pending_spatial_work()
                .await
                .expect("query pending spatial work");
        assert!(
            !pending,
            "沙箱库还有未收敛的空间意图（会走 HealByReplay 而不是本用例要验的 Rebuild）：\
             先起服务把收敛跑完，或清掉 incr_side_effect_pending 的 spatial_reconcile 行"
        );

        // 幽灵构件：只存在于内存树与树文件里，库中没有它的任何指针行——
        // 正是「已删元素的旧包围盒」在崩溃窗口里的形态。
        let ghost = RefnoEnum::from("4009999901/77");
        let ghost_box = parry3d::bounding_volume::Aabb::new(
            [0.0f32, 0.0, 0.0].into(),
            [1000.0f32, 1000.0, 1000.0].into(),
        );
        GLOBAL_AABB_TREE
            .write()
            .await
            .sync_refnos(vec![RStarBoundingBox::new(
                ghost_box,
                ghost,
                "BOX".to_string(),
            )]);
        crate::fast_model::aabb_tree::persist_aabb_tree()
            .await
            .expect("baseline persist");
        let baseline = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(baseline["drift"], false, "基线必须自洽: {baseline}");
        let epoch_before = baseline["db_epoch"].as_u64().expect("baseline db epoch");

        // 直写删除：树上有它 → 房间边删除与 epoch bump 同事务，随后摘树。
        delete_room_membership(&[ghost], 16)
            .await
            .expect("direct delete");
        let after_delete = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(
            after_delete["db_epoch"].as_u64(),
            Some(epoch_before + 1),
            "在树上的直写删除必须恰好 bump 一次: {after_delete}"
        );
        assert_eq!(
            after_delete["drift"], true,
            "落盘前的漂移必须在 /health 可见（修复前这里恒 false）: {after_delete}"
        );

        // 模拟崩溃重启：进程态（内存树、脏标记）丢失，磁盘上是陈旧文件。
        *GLOBAL_AABB_TREE.write().await = AccelerationTree::load(Vec::new());
        crate::fast_model::aabb_tree::load_project_tree_verified()
            .await
            .expect("startup load");
        let recovered = crate::fast_model::aabb_tree::spatial_tree_status().await;
        assert_eq!(
            recovered["startup_verdict"], "rebuilt",
            "指纹失配且无意图必须走指针重建: {recovered}"
        );
        assert!(
            !GLOBAL_AABB_TREE
                .read()
                .await
                .iter()
                .any(|entry| entry.refno == ghost.refno()),
            "重建后的树不得再含幽灵条目（库里本没有它的指针）"
        );
        assert_eq!(
            recovered["drift"], false,
            "重建落盘后指纹必须追平: {recovered}"
        );
    }
}
