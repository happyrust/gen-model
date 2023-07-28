use std::collections::HashMap;
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use aios_core::water_calculation::{CivilEngineeringStp, WaterComputeStp};
use sqlx::encode::IsNull::No;
use crate::plug_in::water_calculation::export_stp;


#[test]
fn test_export_water_calculation_stp() {
    let mut stp = WaterComputeStp::default();
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
    stp.civil_engineering = vec![map_1, map_2];
    stp.non_civil_engineering.push(RefU64::from_str("24381/34110").unwrap());
    // dbg!(&stp);
    export_stp(stp);
}