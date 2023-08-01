use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{CivilEngineeringStp, WaterComputeStp};
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
    map_1.insert(RefU64::from_str("17496/107396").unwrap(), vec![CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/107397").unwrap(),
        hole_refno: None,
        door_refno: Some(RefU64::from_str("17496/124123").unwrap()),
    }, CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/107397").unwrap(),
        hole_refno: None,
        door_refno: Some(RefU64::from_str("17496/124124").unwrap()),
    },
    ]);
    let mut map_2 = HashMap::new();
    map_2.insert(RefU64::from_str("17496/106640").unwrap(), vec![CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/107037").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/107067").unwrap()),
        door_refno: None,
    }, CivilEngineeringStp {
        wall_refno: RefU64::from_str("17496/107397").unwrap(),
        hole_refno: Some(RefU64::from_str("17496/107068").unwrap()),
        door_refno: None,
    },
    ]);
    stp_packet.civil_engineering = vec![map_1, map_2];
    stp_packet.non_civil_engineering.push(RefU64::from_str("24381/34110").unwrap());

    let mgr = get_test_ams_db_manager_async().await;
    export_stp(&mgr, &stp_packet).await?;
    Ok(())
}