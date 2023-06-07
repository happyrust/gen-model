use std::collections::HashMap;
use aios_core::options::DbOption;
use aios_core::pdms_types::{CATA_GEO_NAMES, CATA_HAS_TUBI_GEO_NAMES, CataHashRefnoKV, GNERAL_LOOP_NOUN_NAMES, GNERAL_PRIM_NOUN_NAMES, PdmsElement, RefU64, TOTAL_GEO_NOUN_NAMES};
// use bevy::utils::HashMap;
use bitflags::bitflags;
use dashmap::DashMap;
use crate::aql_api::children::{query_travel_children_with_types_and_cata_hash, query_travel_children_with_types_aql};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;

// pub enum GeoEnum {
//     ALL,
//     PRIM,
//     LOOP,
//     CATA,
// }

bitflags! {
    // #[derive(, Copy, Clone, PartialEq, Eq, Hash)]
    pub struct GeoEnum: u8 {
        const PRIM = 0x1 << 1;
        const LOOP = 0x1 << 2;
        const CATA = 0x1 << 3;
        const CATA_ONLY_TUBI = 0x1 << 4;
        const ALL = Self::PRIM.bits() | Self::LOOP.bits() | Self::CATA.bits() ;
    }
}

impl AiosDBManager {
    pub async fn get_gen_model_root_refnos(&self, db_nos: &[i32]) -> anyhow::Result<Vec<RefU64>> {
        let db_option = &self.db_option;
        let mut target_refnos = vec![];
        let mut is_debug = false;
        let target_debug_refno = db_option
            .debug_desi_refno
            .as_ref()
            .map(|x| RefU64::from_refno_str(x).unwrap_or_default());
        if let Some(refno) = target_debug_refno {
            if let Ok(name) = self.get_name(refno).await {
                dbg!(&name);
                target_refnos = vec![target_debug_refno.unwrap()];
            }
            is_debug = true;
        } else if db_option.debug_root_refnos.is_some() {
            //是否是叶子节点
            for str in db_option.debug_root_refnos.as_ref().unwrap() {
                is_debug = true;
                if let Ok(root_refno) = RefU64::from_refno_str(str) {
                    if let Ok(name) = self.get_name(root_refno).await {
                        target_refnos.push(root_refno);
                    }
                }
            }
        }

        if !is_debug {
            for &db_no in db_nos {
                let refnos = self.get_refnos_by_types(db_option.project_name.as_str(), &["SITE"], &[db_no]).await?;
                target_refnos.extend_from_slice(&refnos);
            }
        }

        Ok(target_refnos)
    }


    ///获取待调试或者整个db的参考号集合
    pub async fn get_gen_model_target_refnos(&self, geo_type: GeoEnum, db_nos: &[i32], is_parent: bool) -> anyhow::Result<Vec<RefU64>> {
        let db_option = &self.db_option;
        let database = self.get_arango_db().await?;
        let mut target_refnos = vec![];
        let mut is_debug = false;
        let target_debug_refno = db_option
            .debug_desi_refno
            .as_ref()
            .map(|x| RefU64::from_refno_str(x).unwrap_or_default());
        let types = match geo_type {
            GeoEnum::PRIM => GNERAL_PRIM_NOUN_NAMES.as_slice(),
            GeoEnum::LOOP => GNERAL_LOOP_NOUN_NAMES.as_slice(),
            GeoEnum::CATA => CATA_GEO_NAMES.as_slice(),
            GeoEnum::CATA_ONLY_TUBI => CATA_HAS_TUBI_GEO_NAMES.as_slice(),
            GeoEnum::ALL => TOTAL_GEO_NOUN_NAMES.as_slice(),
            _ => &[],
        };
        if let Some(refno) = target_debug_refno {
            if let Ok(name) = self.get_name(refno).await {
                dbg!(&name);
                target_refnos = vec![target_debug_refno.unwrap()];
            }
            is_debug = true;
        } else if db_option.debug_root_refnos.is_some() {
            //是否是叶子节点
            for str in db_option.debug_root_refnos.as_ref().unwrap() {
                is_debug = true;
                if let Ok(root_refno) = RefU64::from_refno_str(str) {
                    let Ok(name) = self.get_name(root_refno).await else {
                        continue;
                    };
                    let is_leaf = self.get_children_refs(root_refno).await?.len() == 0;
                    if is_leaf {
                        target_refnos.push(root_refno);
                    } else {
                        query_travel_children_with_types_aql(
                            &database,
                            root_refno,
                            types,
                            is_parent,
                        )
                            .await?
                            .iter()
                            .for_each(|x| target_refnos.push(x.refno));
                    }
                }
            }
        }

        if !is_debug {
            target_refnos.extend_from_slice(&self
                .get_refnos_by_types(
                    db_option.project_name.as_str(),
                    types,
                    db_nos,
                )
                .await?);
        }

        Ok(target_refnos)
    }


    pub async fn get_gen_model_target_refnos_by_cata_hash(&self, geo_type: GeoEnum, db_nos: &[i32], is_parent: bool, skip_exist: bool) -> anyhow::Result<DashMap<u64, CataHashRefnoKV>> {
        let db_option = &self.db_option;
        let database = self.get_arango_db().await?;
        let mut target_refnos_map = DashMap::new();
        let mut is_debug = false;
        let target_debug_refno = db_option
            .debug_desi_refno
            .as_ref()
            .map(|x| RefU64::from_refno_str(x).unwrap_or_default());
        let types = match geo_type {
            GeoEnum::PRIM => GNERAL_PRIM_NOUN_NAMES.as_slice(),
            GeoEnum::LOOP => GNERAL_LOOP_NOUN_NAMES.as_slice(),
            GeoEnum::CATA => CATA_GEO_NAMES.as_slice(),
            GeoEnum::CATA_ONLY_TUBI => CATA_HAS_TUBI_GEO_NAMES.as_slice(),
            GeoEnum::ALL => TOTAL_GEO_NOUN_NAMES.as_slice(),
            _ => &[],
        };
        if let Some(refno) = target_debug_refno {
            // if let Ok(name) = self.get_name(refno).await {
            //     dbg!(&name);
            //     target_refnos_map = vec![target_debug_refno.unwrap()];
            // }
            is_debug = true;
        } else if db_option.debug_root_refnos.is_some() {
            //是否是叶子节点
            for str in db_option.debug_root_refnos.as_ref().unwrap() {
                is_debug = true;
                if let Ok(root_refno) = RefU64::from_refno_str(str) {
                    let Ok(name) = self.get_name(root_refno).await else {
                        continue;
                    };
                    let is_leaf = self.get_children_refs(root_refno).await?.len() == 0;
                    let mut is_parent = is_parent;
                    if is_leaf {
                        is_parent = false;
                        // target_refnos_map.push(root_refno);
                    }
                    let s = query_travel_children_with_types_and_cata_hash(
                        &database,
                        root_refno,
                        types,
                        is_parent,
                        skip_exist,
                    )
                        .await?;
                    for k in s {
                        target_refnos_map.insert(k.cata_hash, k);
                    }
                    // .iter()
                    // .for_each(|x| target_refnos_map.push(x.refno));
                }
            }
        }

        // if !is_debug {
        //     target_refnos_map.extend_from_slice(&self
        //         .get_refnos_by_types(
        //             db_option.project_name.as_str(),
        //             types,
        //             db_nos,
        //         )
        //         .await?);
        // }

        Ok(target_refnos_map)
    }
}