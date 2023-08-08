use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{CivilEngineeringStp, ExportFloodingStpEvent, FloodingHole};
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::Compound;
use sqlx::encode::IsNull::No;
#[cfg(feature = "opencascade_rs")]
use crate::plug_in::water_calculation::export_stp;
use crate::plug_in::water_calculation::save_stp_data_to_arangodb;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::test_helper::get_test_ams_db_manager_async;

//#[cfg(feature = "opencascade_rs")]
#[tokio::test]
async fn test_export_water_calculation_stp() -> anyhow::Result<()> {
// //测试样例1(孔洞模型测试)
    let mut stp_packet_vec = ExportFloodingStpEvent::default();
    stp_packet_vec.file_name = "孔洞测试1".to_string();
    stp_packet_vec.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
    stp_packet_vec.stp = vec![CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/106430").unwrap(),
        hole_refnos: vec![RefU64::from_str("17496/145333").unwrap(), RefU64::from_str("17496/157058").unwrap()],
        door_refnos: vec![],
    }];
    stp_packet_vec.model_list = vec![(RefU64::from_str("17496/106430").unwrap(), "STWALL 1".to_string())];

    let mut all_hole_map = HashMap::default();
    let map_value = vec![FloodingHole {
        refno: RefU64::from_str("17496/145221").unwrap(),
        name: "/1RS05TT0016T".to_string(),
        is_door: false,
        is_selected: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/145334").unwrap(),
        name: "/1RS05LL0027T".to_string(),
        is_door: false,
        is_selected: false,
    }];
    all_hole_map.insert(RefU64::from_str("17496/106430").unwrap(), map_value);
    stp_packet_vec.all_hole_list = vec![all_hole_map];

    let mut selected_hole_map = HashMap::default();
    let map_value = vec![FloodingHole {
        refno: RefU64::from_str("17496/145333").unwrap(),
        name: "/1RS05LL0028T".to_string(),
        is_door: false,
        is_selected: false,
    }, FloodingHole {
        refno: RefU64::from_str("17496/157058").unwrap(),
        name: "/1RS06PP0001K".to_string(),
        is_door: false,
        is_selected: false,
    }];
    selected_hole_map.insert(RefU64::from_str("17496/106430").unwrap(), map_value);
    stp_packet_vec.selected_hole_list = vec![selected_hole_map];


// //测试样例2(孔洞模型测试)
//     let mut stp_packet_vec = ExportFloodingStpEvent::default();
//     stp_packet_vec.file_name = "孔洞测试2".to_string();
//     stp_packet_vec.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
//     stp_packet_vec.stp = vec![CivilEngineeringStp {
//         wall_refno: RefU64::from_str("17496/106028").unwrap(),
//         hole_refnos: vec![RefU64::from_str("17496/142305").unwrap(), RefU64::from_str("17496/142306").unwrap()],
//         door_refnos: vec![],
//     }];
//     stp_packet_vec.model_list = vec![(RefU64::from_str("17496/106028").unwrap(), "STWALL 1".to_string())];
//
//     let all_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("17496/106029").unwrap(),
//         name: "/Copy-of-R445-M01".to_string(),
//         is_door: false,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("17496/106030").unwrap(),
//         name: "FITT 5".to_string(),
//         is_door: false,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("17496/125330").unwrap(),
//         name: "/1RS04LL0015T".to_string(),
//         is_door: false,
//         is_selected: false,
//     }];
//     all_hole_map.insert(RefU64::from_str("17496/106028").unwrap(), map_value);
//     stp_packet_vec.all_hole_list = vec![all_hole_map];
//
//     let selected_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("17496/142305").unwrap(),
//         name: "/1RS04CC2302T".to_string(),
//         is_door: false,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("17496/142306").unwrap(),
//         name:  "/1RS04CC2301T".to_string(),
//         is_door: false,
//         is_selected: false,
//     }];
//     selected_hole_map.insert(RefU64::from_str("17496/106028").unwrap(), map_value);
//     stp_packet_vec.selected_hole_list = vec![selected_hole_map];


// // //测试样例3(门洞模型测试)
//     let mut stp_packet_vec = ExportFloodingStpEvent::default();
//     stp_packet_vec.file_name = "门洞测试1".to_string();
//     stp_packet_vec.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
//     stp_packet_vec.stp = vec![CivilEngineeringStp {
//         wall_refno: RefU64::from_str("25688/8143").unwrap(),
//         hole_refnos: vec![],
//         door_refnos: vec![RefU64::from_str("25688/8186").unwrap(), RefU64::from_str("25688/8187").unwrap()],
//     }];
//     stp_packet_vec.model_list = vec![(RefU64::from_str("25688/8143").unwrap(), "STWALL 1".to_string())];
//
//     let all_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/8144").unwrap(),
//         name:  "/1AR01WW0002K".to_string(),
//         is_door: false,
//         is_selected: false,
//     },];
//     all_hole_map.insert(RefU64::from_str("25688/8143").unwrap(), map_value);
//     stp_packet_vec.all_hole_list = vec![all_hole_map];
//
//     let selected_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/8186").unwrap(),
//         name: "FITT 41".to_string(),
//         is_door: true,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("25688/8187").unwrap(),
//         name: "FITT 42".to_string(),
//         is_door: true,
//         is_selected: false,
//     }];
//     selected_hole_map.insert(RefU64::from_str("25688/8143").unwrap(), map_value);
//     stp_packet_vec.selected_hole_list = vec![selected_hole_map];

//
// // //测试样例4(门洞模型测试)
//     let mut stp_packet_vec = ExportFloodingStpEvent::default();
//     stp_packet_vec.file_name = "门洞模型2".to_string();
//     stp_packet_vec.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
//     stp_packet_vec.stp = vec![CivilEngineeringStp {
//         wall_refno: RefU64::from_str("25688/19684").unwrap(),
//         hole_refnos: vec![],
//         door_refnos: vec![RefU64::from_str("25688/19702").unwrap(), RefU64::from_str("25688/19703").unwrap()],
//     }];
//     stp_packet_vec.model_list = vec![(RefU64::from_str("25688/19684").unwrap(), "STWALL 3".to_string())];
//
//     let all_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/19685").unwrap(),
//         name: "/1AR04VV0005K".to_string(),
//         is_door: false,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("25688/19686").unwrap(),
//         name:"/1AR04TT3504K".to_string(),
//         is_door: false,
//         is_selected: false,
//     },
//     ];
//     all_hole_map.insert(RefU64::from_str("25688/19684").unwrap(), map_value);
//     stp_packet_vec.all_hole_list = vec![all_hole_map];
//
//     let selected_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/19702").unwrap(),
//         name: "FITT 18".to_string(),
//         is_door: true,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("25688/19703").unwrap(),
//         name: "FITT 19".to_string(),
//         is_door: true,
//         is_selected: false,
//     }];
//     selected_hole_map.insert(RefU64::from_str("25688/19684").unwrap(), map_value);
//     stp_packet_vec.selected_hole_list = vec![selected_hole_map];


// // //测试样例5(孔，门洞模型联合测试)
//     let mut stp_packet_vec = ExportFloodingStpEvent::default();
//     stp_packet_vec.file_name = "孔，门洞模型联合测试".to_string();
//     stp_packet_vec.save_time = "2023-08-07 20:39:16.867354400 +08:00".to_string();
//     stp_packet_vec.stp = vec![CivilEngineeringStp {
//         wall_refno: RefU64::from_str("25688/19684").unwrap(),
//         hole_refnos: vec![],
//         door_refnos: vec![RefU64::from_str("25688/19702").unwrap(), RefU64::from_str("25688/19703").unwrap()],
//     }, CivilEngineeringStp {
//         wall_refno: RefU64::from_str("25688/19684").unwrap(),
//         hole_refnos: vec![RefU64::from_str("25688/19685").unwrap()],
//         door_refnos: vec![],
//     }];
//     stp_packet_vec.model_list = vec![(RefU64::from_str("25688/19684").unwrap(), "STWALL 3".to_string())];
//
//     let all_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/19686").unwrap(),
//         name: "/1AR04TT3504K".to_string(),
//         is_door: false,
//         is_selected: false,
//     },
//     ];
//     all_hole_map.insert(RefU64::from_str("25688/19684").unwrap(), map_value);
//     stp_packet_vec.all_hole_list = vec![all_hole_map];
//
//     let selected_hole_map = HashMap::default();
//     let map_value = vec![FloodingHole {
//         refno: RefU64::from_str("25688/19702").unwrap(),
//         name: "FITT 18".to_string(),
//         is_door: true,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("25688/19703").unwrap(),
//         name: "FITT 19".to_string(),
//         is_door: true,
//         is_selected: false,
//     }, FloodingHole {
//         refno: RefU64::from_str("25688/19685").unwrap(),
//         name: "/1AR04VV0005K".to_string(),
//         is_door: false,
//         is_selected: false,
//     }];
//     selected_hole_map.insert(RefU64::from_str("25688/19684").unwrap(), map_value);
//     stp_packet_vec.selected_hole_list = vec![selected_hole_map];

    let mgr = get_test_ams_db_manager_async().await;

    //测试将数据保存至图数据库
    // save_stp_data_to_arangodb(&mgr, stp_packet_vec.clone()).await;
    //孔洞封堵
    // export_stp(&mgr, stp_packet_vec).await?;
    Ok(())
}