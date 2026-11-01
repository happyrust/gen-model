use crate::fast_model::cal_model::update_cal_equip_wtrans;
use aios_core::spatial::pipe::cal_valve_nearest_floor;

//计算管道元件的计算属性
pub async fn update_cal_bran_component() -> anyhow::Result<()> {
    // 阀门距楼板高度。此前是 `.unwrap()`：一次查询失败直接 panic，把「可事后重建的
    // 派生量」变成启动杀手。上抛给调用方，由调用方决定降级姿态（lib.rs 启动段告警继续）。
    let written = cal_valve_nearest_floor().await?;
    tracing::debug!(written, "阀门 nearest_relate 写入");
    Ok(())
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

        cal_valve_nearest_floor().await?;
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
        Ok(())
    }
}
