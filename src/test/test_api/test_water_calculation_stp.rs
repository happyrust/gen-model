use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{CivilEngineeringStp, WaterComputeStp};
#[cfg(feature = "opencascade_rs")]
use opencascade::primitives::Compound;
use sqlx::encode::IsNull::No;
#[cfg(feature = "opencascade_rs")]
use crate::plug_in::water_calculation::export_stp;
use crate::rvm::data_api::query_rvm_geo_instance_aql;
use crate::test::test_helper::get_test_ams_db_manager_async;

#[cfg(feature = "opencascade_rs")]
#[tokio::test]
async fn test_export_water_calculation_stp()  -> anyhow::Result<()> {
    let mut stp_packet = WaterComputeStp::default();
    let mut map_1 = HashMap::new();
    map_1.insert(RefU64::from_str("17496/106429").unwrap(), vec![CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/106430").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/145221").unwrap()),
        door_refno: None,
    }, CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/106430").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/145333").unwrap()),
        door_refno: None,
    }, CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/106430").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/145334").unwrap()),
        door_refno: None,
    }, CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/106430").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/157058").unwrap()),
        door_refno: None,
    }, ]);
    stp_packet.civil_engineering = vec![map_1];

    // //测试样例2(门洞模型测试)
    // let mut stp = WaterComputeStp::default();
    // let mut map_1 = HashMap::new();
    // map_1.insert(RefU64::from_str("25688/8143").unwrap(), vec![CivilEngineeringStp {
    //     wall_refno: RefU64::from_str("25688/8143").unwrap(),
    //     hole_refno: None,
    //     door_refno: Some(RefU64::from_str("25688/8183").unwrap()),
    // }, CivilEngineeringStp {
    //     wall_refno: RefU64::from_str("25688/8143").unwrap(),
    //     hole_refno:None,
    //     door_refno: Some(RefU64::from_str("25688/8184").unwrap()),
    // }, ]);
    // stp.civil_engineering = vec![map_1];

    let mgr = get_test_ams_db_manager_async().await;
    export_stp(&mgr, &stp_packet).await?;
    Ok(())
}