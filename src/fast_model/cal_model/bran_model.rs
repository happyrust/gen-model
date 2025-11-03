use crate::fast_model::cal_model::update_cal_equip_wtrans;
use aios_core::spatial::pipe::cal_valve_nearest_floor;

//计算管道元件的计算属性
pub async fn update_cal_bran_component() -> anyhow::Result<()> {
    //阀门的距离楼板的高度
    cal_valve_nearest_floor().await.unwrap();
    Ok(())
}
