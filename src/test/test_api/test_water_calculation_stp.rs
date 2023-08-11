use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{ExportFloodingStpEvent, FloodingHole};
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::Compound;
use sqlx::encode::IsNull::No;

use crate::plug_in::water_calculation::export_stp;
use crate::plug_in::water_calculation::save_stp_data_to_arangodb;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::test_helper::get_test_ams_db_manager_async;


//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例1(开孔洞测试)
async fn test_export_water_calculation_stp_1() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "孔洞测试1".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(RefU64::from_str("17496/106430").unwrap(), "STWALL 1".to_string());
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(RefU64::from_str("17496/106430").unwrap(), vec![FloodingHole {
        refno: RefU64::from_str("17496/145221").unwrap(),
        name: "/1RS05TT0016T".to_string(),
        is_door: false,
        is_selected: false,
        is_plugged: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/145333").unwrap(),
        name: "/1RS05LL0028T".to_string(),
        is_door: false,
        is_selected: false,
        is_plugged: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/14533").unwrap(),
        name: "/1RS05LL0027T".to_string(),
        is_door: false,
        is_selected: false,
        is_plugged: true,
    }, FloodingHole {
        refno: RefU64::from_str("17496/157058").unwrap(),
        name: "/1RS06PP0001K".to_string(),
        is_door: false,
        is_selected: false,
        is_plugged: true,
    }, ]);

    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}


//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例2(无需开孔洞且无需开门洞测试)
async fn test_export_water_calculation_stp_2() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "无需开洞".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(RefU64::from_str("25688/75905").unwrap(), "STWALL 5".to_string());
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(RefU64::from_str("25688/75905").unwrap(), vec![FloodingHole {
        refno: RefU64::from_str("25688/75906").unwrap(),
        name: "/1RS07SS0001M".to_string(),
        is_door: true,
        is_selected: false,
        is_plugged: true,
    }, FloodingHole {
        refno: RefU64::from_str("25688/75907").unwrap(),
        name: "/1RS07SS0002M".to_string(),
        is_door: true,
        is_selected: false,
        is_plugged: true,
    }]);

    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}