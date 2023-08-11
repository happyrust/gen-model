use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{ExportFloodingStpEvent, FloodingHole};
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::Compound;
use sqlx::encode::IsNull::No;
#[cfg(feature = "opencascade_rs")]
use crate::plug_in::water_calculation::export_stp;
use crate::plug_in::water_calculation::save_stp_data_to_arangodb;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[cfg(feature = "opencascade_rs")]
#[tokio::test]
async fn test_export_water_calculation_stp() -> anyhow::Result<()> {
//测试样例1(孔洞模型测试)
    let mut stp_packet = ExportFloodingStpEvent::default();
    stp_packet.file_name = "孔洞测试1".to_string();
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    // stp_packet_vec.model_list = vec![(RefU64::from_str("17496/106430").unwrap(), "STWALL 1".to_string())];
    // let mut opening_hole_map = HashMap::default();
    let map_value = vec![FloodingHole {
        refno: RefU64::from_str("17496/145221").unwrap(),
        name: "/1RS05TT0016T".to_string(),
        is_door: false,
        is_plugged: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/145334").unwrap(),
        name: "/1RS05LL0027T".to_string(),
        is_door: false,
        is_plugged: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/145333").unwrap(),
        name: "/1RS05LL0028T".to_string(),
        is_door: false,
        is_plugged: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/157058").unwrap(),
        name: "/1RS06PP0001K".to_string(),
        is_door: false,
        is_plugged: true,
    }];
    stp_packet.walls_map.insert(RefU64::from_str("17496/106430").unwrap(), map_value);
    // stp_packet_vec.plugging_hole_list = vec![plugging_hole_map];

    let mgr = get_test_ams_db_manager_async().await;

    //测试将数据保存至图数据库
    save_stp_data_to_arangodb(&mgr, stp_packet.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}