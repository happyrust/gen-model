use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use std::collections::HashMap;
use std::str::FromStr;
// use bevy::utils::HashMap;
// use crate::aql_api::children::{
//     query_travel_children_with_types_and_cata_hash, query_travel_children_with_types_aql,
// };
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

use bitflags::bitflags;
use dashmap::DashMap;

bitflags! {
    #[derive(Copy, Clone, Eq, PartialEq)]
    pub struct GeoEnum: i32 {
        const PRIM = 0x1 << 1;
        const LOOP_AND_PLOO = 0x1 << 2;
        const LOOP = 0x1 << 3;
        const PLOO = 0x1 << 4;
        const CATA = 0x1 << 5;
        const POHE = 0x1 << 6;
        const CATA_BRAN_AND_HANGER_REUSE = 0x1 << 7;  //branch
        const CATA_SINGLE_REUSE = 0x1 << 8;   //sctn, fit, fixing, pfit
        const CATA_WITHOUT_REUSE = 0x1 << 9;   //sctn, fit, fixing, pfit
        // const CATA_ONLY_TUBI_REUSE = 0x1 << 4;
        const ALL = Self::PRIM.bits() | Self::LOOP_AND_PLOO.bits() | Self::LOOP.bits() |
        Self::PLOO.bits() | Self::CATA.bits()|
        Self::POHE.bits() | Self::CATA_BRAN_AND_HANGER_REUSE.bits() |
        Self::CATA_SINGLE_REUSE.bits() | Self::CATA_WITHOUT_REUSE.bits();
    }
}

impl AiosDBManager {
    ///获得db number 对应的site参考号
    pub async fn get_gen_model_root_refnos(&self, db_nos: &[i32]) -> anyhow::Result<Vec<RefU64>> {
        let db_option = &self.db_option;
        let mut target_refnos = vec![];
        for &db_no in db_nos {
            let refnos: RefU64Vec = self
                .get_refnos_by_types(db_option.project_name.as_str(), &["SITE"], &[db_no])
                .await?;
            target_refnos.extend_from_slice(&refnos);
        }

        Ok(target_refnos)
    }

    ///获取待调试或者整个db的参考号集合
    pub async fn get_gen_model_target_refnos(
        &self,
        geo_type: GeoEnum,
        db_nos: &[i32],
        is_parent: bool,
    ) -> anyhow::Result<Vec<RefU64>> {
        Ok(Vec::new())
    }
}
