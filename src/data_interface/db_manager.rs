use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use std::collections::HashMap;
// use bevy::utils::HashMap;
use crate::aql_api::children::{
    query_travel_children_with_types_and_cata_hash, query_travel_children_with_types_aql,
};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use bitflags::bitflags;
use dashmap::DashMap;

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
        const CATA_BRAN_AND_HANGER_REUSE = 0x1 << 4;  //branch
        const CATA_SINGLE_REUSE = 0x1 << 5;   //sctn, fit, fixing, pfit
        const CATA_WITHOUT_REUSE = 0x1 << 6;   //sctn, fit, fixing, pfit
        // const CATA_ONLY_TUBI_REUSE = 0x1 << 4;
        const ALL = Self::PRIM.bits() | Self::LOOP.bits() | Self::CATA.bits() ;
    }
}

impl AiosDBManager {
    pub async fn get_gen_model_root_refnos(&self, db_nos: &[i32]) -> anyhow::Result<Vec<RefU64>> {
        let db_option = &self.db_option;
        let mut target_refnos = vec![];
        let mut is_debug = false;
        if db_option.debug_root_refnos.is_some() {
            //是否是叶子节点
            for str in db_option.debug_root_refnos.as_ref().unwrap() {
                is_debug = true;
                if let Ok(root_refno) = RefU64::from_refno_str(str) {
                    if self.get_attr_from_localdb(root_refno).is_ok() {
                        target_refnos.push(root_refno);
                    }
                }
            }
        }

        if !is_debug {
            for &db_no in db_nos {
                let refnos = self
                    .get_refnos_by_types(db_option.project_name.as_str(), &["SITE"], &[db_no])
                    .await?;
                target_refnos.extend_from_slice(&refnos);
            }
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
        let db_option = &self.db_option;
        let database = self.get_arango_db().await?;
        let mut target_refnos = vec![];
        let mut is_debug = false;
        let types = match geo_type {
            GeoEnum::PRIM => GNERAL_PRIM_NOUN_NAMES.as_slice(),
            GeoEnum::LOOP => GNERAL_LOOP_NOUN_NAMES.as_slice(),
            GeoEnum::CATA => CATA_GEO_NAMES.as_slice(),
            GeoEnum::CATA_BRAN_AND_HANGER_REUSE => CATA_HAS_TUBI_GEO_NAMES.as_slice(),
            GeoEnum::CATA_SINGLE_REUSE => CATA_SINGLE_REUSE_GEO_NAMES.as_slice(),
            GeoEnum::CATA_WITHOUT_REUSE => CATA_WITHOUT_REUSE_GEO_NAMES.as_slice(),
            GeoEnum::ALL => TOTAL_GEO_NOUN_NAMES.as_slice(),
            _ => &[],
        };
        if db_option.debug_root_refnos.is_some() {
            //是否是叶子节点
            for str in db_option.debug_root_refnos.as_ref().unwrap() {
                is_debug = true;
                if let Ok(root_refno) = RefU64::from_refno_str(str) {
                    let Ok(name) = self.get_name(root_refno).await else {
                        continue;
                    };
                    let is_leaf = self.get_children_refs(root_refno).await?.len() == 0;
                    if is_leaf {
                        if let Some(k) = self.query_element(root_refno).await? {
                            let mut add = false;

                            if let Some(owner_ele) = self
                                .query_element(RefU64::from_url_refno(&k.owner).unwrap())
                                .await?
                            {
                                if CATA_HAS_TUBI_GEO_NAMES.contains(&owner_ele.noun.as_str())
                                    || CATA_HAS_TUBI_GEO_NAMES.contains(&k.noun.as_str())
                                {
                                    add = geo_type == GeoEnum::CATA_BRAN_AND_HANGER_REUSE;
                                } else if CATA_SINGLE_REUSE_GEO_NAMES.contains(&k.noun.as_str()) {
                                    add = geo_type == GeoEnum::CATA_SINGLE_REUSE;
                                } else if GNERAL_LOOP_NOUN_NAMES.contains(&k.noun.as_str()) {
                                    add = geo_type == GeoEnum::LOOP;
                                } else if GNERAL_PRIM_NOUN_NAMES.contains(&k.noun.as_str()) {
                                    add = geo_type == GeoEnum::PRIM;
                                }
                            }
                            if add {
                                target_refnos.push(root_refno);
                            }
                        }
                    } else {
                        query_travel_children_with_types_aql(
                            &database, root_refno, types, is_parent,
                        )
                        .await?
                        .iter()
                        .for_each(|x| target_refnos.push(x.refno));
                    }
                }
            }
        }

        if !is_debug {
            target_refnos.extend_from_slice(
                &self
                    .get_refnos_by_types(db_option.project_name.as_str(), types, db_nos)
                    .await?,
            );
        }

        Ok(target_refnos)
    }

    pub async fn get_gen_model_map_by_cata_hash(
        &self,
        geo_type: GeoEnum,
        db_nos: &[i32],
        is_parent: bool,
        skip_exist: bool,
    ) -> anyhow::Result<DashMap<String, CataHashRefnoKV>> {
        let db_option = &self.db_option;
        let database = self.get_arango_db().await?;
        let mut target_refnos_map = DashMap::new();
        let mut is_debug = false;
        let types = match geo_type {
            GeoEnum::PRIM => GNERAL_PRIM_NOUN_NAMES.as_slice(),
            GeoEnum::LOOP => GNERAL_LOOP_NOUN_NAMES.as_slice(),
            GeoEnum::CATA => CATA_GEO_NAMES.as_slice(),
            GeoEnum::CATA_BRAN_AND_HANGER_REUSE => CATA_HAS_TUBI_GEO_NAMES.as_slice(),
            GeoEnum::CATA_SINGLE_REUSE => CATA_SINGLE_REUSE_GEO_NAMES.as_slice(),
            GeoEnum::CATA_WITHOUT_REUSE => CATA_WITHOUT_REUSE_GEO_NAMES.as_slice(),
            GeoEnum::ALL => TOTAL_GEO_NOUN_NAMES.as_slice(),
            _ => &[],
        };

        let mut root_refnos = if let Some(d) = &db_option.debug_root_refnos {
            d.iter()
                .map(|x| RefU64::from_refno_str(x).unwrap_or_default())
                .collect::<Vec<_>>()
        } else {
            self.get_refnos_by_types(db_option.project_name.as_str(), &["SITE"], db_nos)
                .await?
                .0
        };

        //是否是叶子节点
        for root_refno in root_refnos {
            is_debug = true;
            let Ok(name) = self.get_name(root_refno).await else {
                continue;
            };
            let is_leaf = self.get_children_refs(root_refno).await?.len() == 0;
            let mut check_parent = is_parent;
            if is_leaf {
                if let Some(k) = self.query_element(root_refno).await? {
                    let mut add = false;

                    if let Some(owner_ele) = self
                        .query_element(RefU64::from_url_refno(&k.owner).unwrap())
                        .await?
                    {
                        if owner_ele.noun.as_str() == "BRAN" || owner_ele.noun.as_str() == "HANG" {
                            add = geo_type == GeoEnum::CATA_BRAN_AND_HANGER_REUSE;
                        } else if CATA_SINGLE_REUSE_GEO_NAMES.contains(&k.noun.as_str()) {
                            add = geo_type == GeoEnum::CATA_SINGLE_REUSE;
                        } else if CATA_WITHOUT_REUSE_GEO_NAMES.contains(&k.noun.as_str()) {
                            add = geo_type == GeoEnum::CATA_WITHOUT_REUSE;
                        }
                    }
                    if add {
                        if let Some(r) = k.cata_hash.clone() {
                            target_refnos_map.insert(
                                r.clone(),
                                CataHashRefnoKV {
                                    cata_hash: Some(r),
                                    exist_geo: None,
                                    group_refnos: vec![root_refno],
                                },
                            );
                        }
                    }
                }
            } else {
                let s = query_travel_children_with_types_and_cata_hash(
                    &database,
                    root_refno,
                    types,
                    check_parent,
                    skip_exist,
                )
                .await?;
                // dbg!(s.len());
                for k in s {
                    target_refnos_map.insert(k.cata_hash.clone().unwrap_or_default(), k);
                }
            }
        }

        Ok(target_refnos_map)
    }
}
