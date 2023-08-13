#[cfg(feature = "opencascade_rs")]
use crate::plug_in::water_calculation::export_stp;
use crate::plug_in::water_calculation::save_stp_data_to_arangodb;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::test_helper::get_test_ams_db_manager_async;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{ExportFloodingStpEvent, FloodingHole};
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::Compound;
use sqlx::encode::IsNull::No;
use std::collections::HashMap;
use std::str::FromStr;

#[cfg(feature = "opencascade_rs")]
#[tokio::test]
async fn test_export_water_calculation_stp_0() -> anyhow::Result<()> {
    //测试样例1(孔洞模型测试)
    let mut stp_packet = ExportFloodingStpEvent::default();
    stp_packet.file_name = "孔洞测试1".to_string();
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("17496/106424").unwrap(),
        "STWALL 1".to_string(),
    );
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(
        RefU64::from_str("17496/106424").unwrap(),
        vec![
            FloodingHole {
                refno: RefU64::from_str("17496/106427").unwrap(),
                name: "FITT 4".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/114643").unwrap(),
                name: "FITT 3".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/161283").unwrap(),
                name: "/1RS05CC0611T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/161284").unwrap(),
                name: "/1RS05CC0610T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
        ],
    );
    stp_packet.walls_map = walls_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    save_stp_data_to_arangodb(&mgr, stp_packet.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}

//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例2(开孔洞测试)
async fn test_export_water_calculation_stp_2() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "孔洞测试2".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("17496/106430").unwrap(),
        "STWALL 1".to_string(),
    );
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(
        RefU64::from_str("17496/106430").unwrap(),
        vec![
            FloodingHole {
                refno: RefU64::from_str("17496/145221").unwrap(),
                name: "/1RS05TT0016T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/145333").unwrap(),
                name: "/1RS05LL0028T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/145334").unwrap(),
                name: "/1RS05LL0027T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("17496/157058").unwrap(),
                name: "/1RS06PP0001K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
        ],
    );
    stp_packet.walls_map = walls_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    // export_stp(&mgr, stp_packet).await?;
    Ok(())
}

//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例3(开门洞测试)
async fn test_export_water_calculation_stp_3() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "门洞测试1".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("25688/8143").unwrap(),
        "STWALL 1".to_string(),
    );
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(
        RefU64::from_str("25688/8143").unwrap(),
        vec![
            FloodingHole {
                refno: RefU64::from_str("25688/8144").unwrap(),
                name: "/1AR01WW0002K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8145").unwrap(),
                name: "/1AR01KK1008T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8146").unwrap(),
                name: "/1AR01KK1011T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8147").unwrap(),
                name: "/1AR01KK1014T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8148").unwrap(),
                name: "/1AR02KK0054T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8151").unwrap(),
                name: "/1AR01EE0019T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8152").unwrap(),
                name: "/1AR01EE0018T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8153").unwrap(),
                name: "/1AR01EE0006K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8154").unwrap(),
                name: "/1AR01EE0002K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8155").unwrap(),
                name: "/1AR01KK0007T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8156").unwrap(),
                name: "/1AR01KK0008T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8157").unwrap(),
                name: "/1AR01KK0009T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8158").unwrap(),
                name: "/1AR01VV0014K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8159").unwrap(),
                name: "/1AR01TT3404T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8160").unwrap(),
                name: "/1AR01TT3406T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8161").unwrap(),
                name: "/1AR01TT3403K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8162").unwrap(),
                name: "/1AR01TT3414T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8163").unwrap(),
                name: "/1AR01EE0043T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8164").unwrap(),
                name: "/1AR01EE0044T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8165").unwrap(),
                name: "/1AR01EE0045T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8166").unwrap(),
                name: "/1AR01TT6011K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8167").unwrap(),
                name: "/1AR01LL0034T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8168").unwrap(),
                name: "/1AR01LL0013T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8169").unwrap(),
                name: "/1AR01LL0037T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8170").unwrap(),
                name: "/1AR01LL0033T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8171").unwrap(),
                name: "/1AR01LL0038T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8172").unwrap(),
                name: "/1AR01EE0016T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8173").unwrap(),
                name: "/1AR01EE0017T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8174").unwrap(),
                name: "/1AR01EE0014T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8175").unwrap(),
                name: "/1AR01LL0014T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8176").unwrap(),
                name: "/1AR01LL0009T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8177").unwrap(),
                name: "/1AR01LL0010T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8178").unwrap(),
                name: "/1AR01TT6000T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8179").unwrap(),
                name: "/1AR01TT6002K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8180").unwrap(),
                name: "/1AR01VV0011K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8181").unwrap(),
                name: "/1AR01VV0012K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8182").unwrap(),
                name: "/1AR01VV0013K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8183").unwrap(),
                name: "FITT 38".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8184").unwrap(),
                name: "FITT 39".to_string(),
                is_door: true,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8185").unwrap(),
                name: "FITT 40".to_string(),
                is_door: true,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8186").unwrap(),
                name: "FITT 41".to_string(),
                is_door: true,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8187").unwrap(),
                name: "FITT 42".to_string(),
                is_door: true,
                is_selected: false,
                is_plugged: false,
            },
        ],
    );
    stp_packet.walls_map = walls_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}

//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例4(开门洞测试)
async fn test_export_water_calculation_stp_4() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "门洞测试2".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("25688/19684").unwrap(),
        "STWALL 3".to_string(),
    );
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(
        RefU64::from_str("25688/19684").unwrap(),
        vec![
            FloodingHole {
                refno: RefU64::from_str("25688/19685").unwrap(),
                name: "/1AR04VV0005K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19686").unwrap(),
                name: "/1AR04TT3504K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19687").unwrap(),
                name: "/1AR04KK1019T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19688").unwrap(),
                name: "/1AR04KK1020T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19689").unwrap(),
                name: "/1AR04LL0021T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8151").unwrap(),
                name: "/1AR01EE0019T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19690").unwrap(),
                name: "/1AR04LL0022T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19691").unwrap(),
                name: "/1AR04LL0023T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19692").unwrap(),
                name: "/1AR04LL0024T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19693").unwrap(),
                name: "/1AR04LL0026T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19694").unwrap(),
                name: "/1AR04LL0025T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19695").unwrap(),
                name: "/1AR04EE0085T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19696").unwrap(),
                name: "/1AR04EE0086T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19697").unwrap(),
                name: "/1AR04EE0087T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19698").unwrap(),
                name: "/1AR04EE0109T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19699").unwrap(),
                name: "/1AR04EE0110T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19700").unwrap(),
                name: "/1AR04EE0111T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19701").unwrap(),
                name: "/1AR04KK0043T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8164").unwrap(),
                name: "/1AR01EE0044T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19702").unwrap(),
                name: "FITT 18".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19703").unwrap(),
                name: "FITT 19".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
        ],
    );
    stp_packet.walls_map = walls_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    save_stp_data_to_arangodb(&mgr, stp_packet.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}

//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
//测试样例5(同时开孔洞和开门洞测试)
async fn test_export_water_calculation_stp_5() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "孔洞+门洞测试".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("25688/19684").unwrap(),
        "STWALL 3".to_string(),
    );
    stp_packet.export_models_map = export_models_map;
    //所有墙与孔洞的map
    let mut walls_map = HashMap::new();
    walls_map.insert(
        RefU64::from_str("25688/19684").unwrap(),
        vec![
            FloodingHole {
                refno: RefU64::from_str("25688/19685").unwrap(),
                name: "/1AR04VV0005K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19686").unwrap(),
                name: "/1AR04TT3504K".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19687").unwrap(),
                name: "/1AR04KK1019T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19688").unwrap(),
                name: "/1AR04KK1020T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19689").unwrap(),
                name: "/1AR04LL0021T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8151").unwrap(),
                name: "/1AR01EE0019T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19690").unwrap(),
                name: "/1AR04LL0022T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19691").unwrap(),
                name: "/1AR04LL0023T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19692").unwrap(),
                name: "/1AR04LL0024T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19693").unwrap(),
                name: "/1AR04LL0026T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19694").unwrap(),
                name: "/1AR04LL0025T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19695").unwrap(),
                name: "/1AR04EE0085T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19696").unwrap(),
                name: "/1AR04EE0086T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19697").unwrap(),
                name: "/1AR04EE0087T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19698").unwrap(),
                name: "/1AR04EE0109T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19699").unwrap(),
                name: "/1AR04EE0110T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19700").unwrap(),
                name: "/1AR04EE0111T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19701").unwrap(),
                name: "/1AR04KK0043T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/8164").unwrap(),
                name: "/1AR01EE0044T".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19702").unwrap(),
                name: "FITT 18".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: true,
            },
            FloodingHole {
                refno: RefU64::from_str("25688/19703").unwrap(),
                name: "FITT 19".to_string(),
                is_door: false,
                is_selected: false,
                is_plugged: false,
            },
        ],
    );
    stp_packet.walls_map = walls_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}

#[tokio::test]
//测试样例5(测试电气导出)
async fn test_export_electric_without_walls() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "电气测试1".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(RefU64::from_str("24383/96911").unwrap(), "托臂".to_string());
    export_models_map.insert(RefU64::from_str("24383/95023").unwrap(), "桥架".to_string());
    stp_packet.export_models_map = export_models_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    save_stp_data_to_arangodb(&mgr, stp_packet.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}

#[tokio::test]
//测试样例6(测试Floor导出)
async fn test_export_floor_with_negs() -> anyhow::Result<()> {
    let mut stp_packet = ExportFloodingStpEvent::default();
    //文件名
    stp_packet.file_name = "Floor测试1".to_string();
    //保存事件
    stp_packet.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    //导出模型列表
    let mut export_models_map = HashMap::new();
    export_models_map.insert(
        RefU64::from_str("17496/172226").unwrap(),
        "Floor".to_string(),
    );
    // export_models_map.insert(RefU64::from_str("24383/95023").unwrap(), "桥架".to_string());
    stp_packet.export_models_map = export_models_map;

    dbg!(&stp_packet);
    let mgr = get_test_ams_db_manager_async().await;
    //测试将数据保存至图数据库
    save_stp_data_to_arangodb(&mgr, stp_packet.clone()).await;
    //孔洞封堵
    export_stp(&mgr, stp_packet).await?;
    Ok(())
}
