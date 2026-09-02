use crate::fast_model::cal_model::update_cal_equip_wtrans;
use aios_core::spatial::pipe::cal_valve_nearest_floor;

//计算管道元件的计算属性
pub async fn update_cal_bran_component() -> anyhow::Result<()> {
    // 阀门距楼板高度。此前是 `.unwrap()`：一次查询失败直接 panic，把「可事后重建的
    // 派生量」变成启动杀手。上抛给调用方，由调用方决定降级姿态（lib.rs 启动段告警继续）。
    cal_valve_nearest_floor().await?;
    Ok(())
}
