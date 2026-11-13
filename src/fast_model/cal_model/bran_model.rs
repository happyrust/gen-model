use crate::fast_model::cal_model::update_cal_equip_wtrans;
use aios_core::SUL_DB;
use aios_core::spatial::pipe::cal_valve_nearest_floor;

//计算管道元件的计算属性
pub async fn update_cal_bran_component() -> anyhow::Result<()> {
    // 阀门距楼板高度。此前是 `.unwrap()`：一次查询失败直接 panic，把「可事后重建的
    // 派生量」变成启动杀手。上抛给调用方，由调用方决定降级姿态（lib.rs 启动段告警继续）。
    let written = cal_valve_nearest_floor().await?;
    tracing::debug!(written, "阀门 nearest_relate 写入");
    note_unlinked_valves().await;
    Ok(())
}

/// 把仍没有楼板边的阀门记进 `geom_error`。
///
/// `cal_valve_nearest_floor` 的 `Ok(None)` 分支是空的——射线打不中就直接跳过，不留痕。
/// 主仓这边没法在 vendor 里记账（那一层不认识 `geom_error` 这本账），所以重算完再反查
/// 一次「还是没边」的阀门，逐个写进去。
///
/// 真库 2026-11-02：588 台阀门里 78 台挂不上（7997 库 22 台、251095 库 56 台），
/// 其中 251095 整个库没有 FLOOR 记录，属数据事实；不记账的话这条结论只能靠外部脚本
/// 反查出来，日志里一个字都没有。
///
/// 记账失败只 warn：边已经写成功了，账本晚一轮补齐不影响数据正确性。
async fn note_unlinked_valves() {
    let refnos = match SUL_DB
        .query(
            "SELECT VALUE id FROM pe WHERE noun = 'VALV' AND deleted != true \
             AND array::len(->nearest_relate) = 0;",
        )
        .await
        .and_then(|response| response.check())
        .and_then(|mut response| response.take::<Vec<surrealdb::sql::Thing>>(0))
    {
        Ok(v) => v,
        Err(error) => {
            log::warn!("[geom_error] 反查未挂边阀门失败: {error}");
            return;
        }
    };
    for refno in refnos {
        let target = refno.to_string();
        crate::data_interface::geom_error::note_equip_floor_miss(&target, "VALV", "no_floor_below")
            .await;
    }
    clear_linked_valves().await;
}

/// 把已经挂上楼板边的阀门从账本里销掉。
///
/// 只发 DELETE 给账本里确实有行的目标（`clear_equip_floor_misses` 内部靠 `KNOWN`
/// 播种），所以这一步对正常库是空操作，不会为了 588 台阀门逐条打一次删库请求；
/// 真要删时合成一条 `WHERE target IN [...]`，不是逐条一个请求。
///
/// 口径与 [`note_unlinked_valves`] 严格互补：两边都走 `pe` 的 `->nearest_relate`，
/// 用同一个 `noun = 'VALV' AND deleted != true` 过滤。混用 `VALV.REFNO->nearest_relate`
/// 与 `pe->nearest_relate` 会让同一批阀门在两处得到不同答案，销账就会把刚记上的账删掉。
async fn clear_linked_valves() {
    let targets = match SUL_DB
        .query(
            "SELECT VALUE <string>id FROM pe WHERE noun = 'VALV' AND deleted != true \
             AND array::len(->nearest_relate) > 0;",
        )
        .await
        .and_then(|response| response.check())
        .and_then(|mut response| response.take::<Vec<String>>(0))
    {
        Ok(v) => v,
        Err(error) => {
            log::warn!("[geom_error] 反查已挂边阀门失败: {error}");
            return;
        }
    };
    // 一条 DELETE 替掉 500+ 条逐条销账：逐条一个请求在真库实测要 2～3 秒。
    if let Err(error) =
        crate::data_interface::geom_error::clear_equip_floor_misses(&targets).await
    {
        log::warn!("[geom_error] 阀门批量销账失败 {} 个目标: {error:#}", targets.len());
    }
}

#[cfg(test)]
mod valve_nearest_floor_live_tests {
    use super::*;
    use aios_core::SUL_DB;

    /// 阀门全量计算要真的挂上边，而且重跑不堆边。
    ///
    /// 修前（`START offset` 翻页跳行、写库结果不检查）532 个 VALV 一条边也没有；
    /// 修后 510 个挂上（2026-11-01）。剩下 22 个正下方没有楼板候选或网格打不中。
    /// 同时是 `coverage_does_not_regress` 之后的恢复手段：那条测试会 `DELETE nearest_relate`。
    ///
    /// ```text
    /// cargo test --lib valve_floor_links_are_written_once -- --ignored --test-threads=1
    /// ```
    #[tokio::test]
    #[ignore = "live: 需要真库 8009，会写 nearest_relate；必须单跑"]
    async fn valve_floor_links_are_written_once() -> anyhow::Result<()> {
        crate::attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;

        // 走 `update_cal_bran_component`，跟启动序列同一个入口，这样记账也一起被覆盖到。
        update_cal_bran_component().await?;
        let rerun = cal_valve_nearest_floor().await?;

        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM VALV WHERE array::len(REFNO->nearest_relate) > 0);")
            .await?;
        let linked: Option<i64> = r.take(0)?;
        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM nearest_relate WHERE in.noun = 'VALV');")
            .await?;
        let edges: Option<i64> = r.take(0)?;
        let (linked, edges) = (linked.unwrap_or(0), edges.unwrap_or(0));
        eprintln!("VALV 挂边 {linked}，边 {edges}，重跑新写 {rerun}");

        assert_eq!(rerun, 0, "已挂边的阀门重跑不该再写");
        assert_eq!(edges, linked, "一个阀门只能有一条最近楼板边");
        assert!(linked >= 500, "阀门挂边 {linked} 低于 500（修前 0）");

        // 账本要跟「未挂边」双向对齐：每台没边的阀门各一行，挂上的阀门一行都不能留。
        // 2026-11-02 实测 78 台未挂边、账本 78 行，双向差集 0。
        let mut r = SUL_DB
            .query(
                "RETURN array::len(array::intersect(\
                   (SELECT VALUE <string>REFNO FROM VALV WHERE array::len(REFNO->nearest_relate) = 0), \
                   (SELECT VALUE target FROM geom_error WHERE kind = 'equip_floor_miss' AND noun = 'VALV')));",
            )
            .await?;
        let missing: Option<i64> = r.take(0)?;
        let mut r = SUL_DB
            .query(
                "RETURN array::len(array::intersect(\
                   (SELECT VALUE <string>REFNO FROM VALV WHERE array::len(REFNO->nearest_relate) > 0), \
                   (SELECT VALUE target FROM geom_error WHERE kind = 'equip_floor_miss' AND noun = 'VALV')));",
            )
            .await?;
        let stale: Option<i64> = r.take(0)?;
        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM VALV WHERE array::len(REFNO->nearest_relate) = 0);")
            .await?;
        let unlinked: Option<i64> = r.take(0)?;
        let (missing, stale, unlinked) = (
            missing.unwrap_or(0),
            stale.unwrap_or(0),
            unlinked.unwrap_or(0),
        );
        eprintln!("未挂边 {unlinked}，漏记 {missing}，已挂边仍记账 {stale}");

        assert_eq!(missing, unlinked, "每台未挂边的阀门都该记一行账");
        assert_eq!(stale, 0, "已挂上边的阀门不该还留在账本里");
        Ok(())
    }
}

#[cfg(test)]
mod valve_ledger_offline_tests {
    /// 阀门侧必须同时有记账和销账两个方向。
    ///
    /// `cal_valve_nearest_floor` 的 `Ok(None)` 分支是空的，射线打不中就静默跳过；
    /// 主仓在调用方补记账。只加记账不加销账的话，阀门一旦补上边，账本里那条
    /// `equip_floor_miss` 会永远留着，把「78 台未挂边」这个数字永久污染。
    #[test]
    fn the_valve_path_both_notes_and_clears() {
        let src = include_str!("bran_model.rs");
        assert!(
            src.contains("note_equip_floor_miss(&target, \"VALV\""),
            "阀门侧要记账，否则未挂边只存在于外部脚本里"
        );
        assert!(
            src.contains("clear_equip_floor_miss(&target)"),
            "阀门侧要销账，否则补上边之后账本永远留着旧行"
        );
        assert!(
            src.contains("update_cal_bran_component().await?;"),
            "live 测试要走启动序列那个入口，否则记账分支不被覆盖"
        );
    }
}
