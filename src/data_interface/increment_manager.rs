use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::version::{backup_data, backup_owner_relate};
use aios_core::{clear_all_caches, SUL_DB};
use aios_core::{get_db_option, RefU64Vec};
use futures::StreamExt;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use notify::{RecursiveMode, Watcher};
use parse_pdms_db::parse::parse_db_basic_info;
use pdms_io::defines::DbPageBasicInfo;
use pdms_io::io::PdmsIO;
use pdms_io::sync::compress::{execute_compress, CompressOptions};
use pdms_io::watch::PdmsWatcher;
use petgraph::visit::Walker;
use tokio::fs::create_dir_all;
use walkdir::WalkDir;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::*;
use crate::mqtt_service::SyncE3dFileMsg;

#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    pub refno: RefU64,
    pub db_no: i32,
    pub attr: NamedAttrMap,
    pub children: RefU64Vec,
    pub operation: EleOperation,
}

impl IncrementInfo {
    #[inline]
    pub fn is_modified(&self) -> bool {
        matches!(self.operation, EleOperation::Modified)
    }

    #[inline]
    pub fn is_deleted(&self) -> bool {
        matches!(self.operation, EleOperation::Deleted)
    }

    #[inline]
    pub fn is_added(&self) -> bool {
        matches!(self.operation, EleOperation::Add)
    }
}

const JSON_CHUNK_COUNT: usize = 200;

pub const CHECK_DB_TYPES: [&'static str; 6] = ["CATA", "DESI", "DICT", "SYST", "GLB", "GLOB"];

impl AiosDBManager {
    /// 执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
    ) -> anyhow::Result<bool> {
        for (path, (basic_info, sesno_range)) in increment_ranges_map {
            //call execute_incr_update_single_sesno
            //一步一步执行更新
            for sesno in sesno_range {
                self.execute_incr_update_single_sesno(&path, &basic_info, sesno)
                    .await?;
            }
        }

        Ok(true)
    }

    /// 执行单个sesno的增量更新
    pub async fn execute_incr_update_single_sesno(
        &self,
        path: &PathBuf,
        basic_info: &DbPageBasicInfo,
        start_sesno: i32,
    ) -> anyhow::Result<bool> {
        //没有增量更新的数据，直接返回
        if start_sesno == 0 {
            return Ok(false);
        }
        let dbnum = basic_info.pdms_header.db_num;
        let mut deleted_refnos_set: HashSet<RefU64> = HashSet::new();
        let mut added_refnos_set: HashSet<RefU64> = HashSet::new();
        let mut modified_refnos_set: HashSet<RefU64> = HashSet::new();
        let mut children_changed_map = BTreeMap::new();

        let mut total_add_len = 0;
        let mut total_modify_len = 0;
        let mut total_deleted_len = 0;
        let mut geo_update_log = IncrGeoUpdateLog::default();
        let mut has_changed = false;
        let project = get_db_option().project_name.clone();
        //TODO: 多历史情况的处理，现在先暂时只处理单个的历史情况
        //TODO: 如何鉴别只有claim page的变化，没有数据的更新，就不需要执行增量更新
        // for (path, (basic_info, start_sesno)) in increment_ranges_map

        //sesno_range 按递增 1 去执行
        let mut io = PdmsIO::new(&project, path, true);
        io.open()?;
        let cur_ses_data = io.get_ses_data(start_sesno as _)?.clone();
        let mut eles_map = io
            .collect_increment_eles((start_sesno..=start_sesno))
            .await?;
        let sync_refnos = self.db_option.get_manual_sync_refnos();
        if !sync_refnos.is_empty() {
            for r in sync_refnos {
                if let Ok(sync_map) = io.auto_get_elements_deep(r).await {
                    eles_map.extend(sync_map);
                }
                // dbg!(&sync_map);
            }
        }
        if eles_map.is_empty() {
            return Ok(false);
        }
        #[cfg(feature = "debug_model")]
        dbg!((start_sesno, eles_map.len()));
        // dbg!(&eles_map);
        //批量检测是否存在这些eles
        let mut exist_refnos_map = BTreeMap::new();
        let pes = eles_map.keys().map(|x| x.to_pe_key()).join(",");
        let sql = format!("SELECT VALUE [id, (select value in from <-pe_owner)] FROM [{pes}] where record::exists(id)");
        // println!("{}", sql);
        let mut resp = SUL_DB.query(sql).await?;
        let refnos: Vec<(RefU64, Vec<RefU64>)> = resp.take(0).unwrap();
        //这里需要把所有的children都添加进来
        for (refno, children) in refnos {
            clear_all_caches(refno.into()).await;
            exist_refnos_map.insert(refno, children);
        }
        #[cfg(feature = "debug_model")]
        dbg!(&exist_refnos_map);

        let mut final_check_geom_refnos: BTreeSet<RefnoEnum> = BTreeSet::new();

        //处理删除的增量更新
        // let mut processed_owner_set = HashSet::new();
        for (&r, ele) in &eles_map {
            let refno: RefnoEnum = r.into();
            let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
            let owner = attmap.get_owner();
            let noun = attmap.get_type();
            let noun = noun.as_str();
            has_changed = true;
            let mut ele_op = EleOperation::Modified;
            //两种情况，要么是删除，要么是修改，首先肯定是有修改的情况
            if let Some(old_children) = exist_refnos_map.get(&ele.refno()) {
                //检查如果有不一致的情况，即需要重新建立owner关系
                let children_eq = old_children.len() == ele.children.len()
                    && old_children
                        .iter()
                        .zip(ele.children.iter())
                        .all(|(a, b)| *a == *b);
                if !children_eq {
                    children_changed_map.insert(
                        refno,
                        (ele.children.clone(), old_children.len(), !children_eq),
                    );
                    //找出删除的children
                    for (i, &child) in old_children.into_iter().enumerate() {
                        if !ele.children.contains(&child) {
                            //index current delete refno, owner refno
                            //需要get deep children
                            let deep_children = aios_core::query_deep_children_refnos(child.into())
                                .await?
                                .into_iter()
                                .map(|x| x.refno())
                                .collect::<HashSet<_>>();
                            let t = (i, child, refno);
                            println!("Delete: {:?}", t);
                            //删除需要扩展到所有的子节点
                            total_deleted_len += deep_children.len();
                            // total_deleted_len += 1;
                            //加入 owner
                            let refno_enum = refno.into();
                            let child_enum = child.into();
                            //处理模型删除的各种情况
                            if PRIMITIVE_NOUN_NAMES.contains(&noun) {
                                geo_update_log.delete_refnos.insert(child_enum);
                            } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun) {
                                geo_update_log.delete_refnos.insert(child_enum);
                            } else if CATA_HAS_TUBI_GEO_NAMES.contains(&noun) {
                                geo_update_log.delete_refnos.insert(child_enum);
                            } else if CATA_GEO_NAMES.contains(&noun) {
                                geo_update_log.delete_refnos.insert(child_enum);
                            } else if TOTAL_NEG_NOUN_NAMES.contains(&noun) {
                                geo_update_log.delete_refnos.insert(child_enum);
                                final_check_geom_refnos.insert(refno_enum);
                            } else if TOTAL_LOOP_NOUN_NAMES.contains(&noun) {
                                final_check_geom_refnos.insert(refno_enum);
                            } else {
                                //几何体，需要往上找
                                final_check_geom_refnos.insert(child_enum);
                            }
                            // else if TOTAL_VERT_NOUN_NAMES.contains(&noun) {
                            //     if let Some(pe) = aios_core::get_pe(owner).await.unwrap() {
                            //         final_check_geom_refnos.insert(pe.refno);
                            //     }
                            // }
                            deleted_refnos_set.extend(deep_children);
                        }
                    }
                }
                modified_refnos_set.insert(refno.refno());
                total_modify_len += 1;
            } else {
                //这里的就是新增的
                total_add_len += 1;
                ele_op = EleOperation::Add;
                added_refnos_set.insert(refno.refno());
                if ele.children.len() > 0 {
                    children_changed_map.insert(refno, (ele.children.clone(), 0, true));
                }
            }
        }

        //执行 backup deleted
        if !deleted_refnos_set.is_empty() {
            dbg!(&deleted_refnos_set);
            //删除的几何体处理，需要判断是否是几何体
            for &refno in &deleted_refnos_set {
                final_check_geom_refnos.insert(refno.into());
            }
            backup_data(deleted_refnos_set.iter(), true, start_sesno as _)
                .await
                .unwrap();
        }

        let mut need_backup_geom_refnos = BTreeSet::new();
        //执行 modifed 的数据备份
        if !modified_refnos_set.is_empty() {
            #[cfg(feature = "debug_model")]
            dbg!(&modified_refnos_set);
            backup_data(modified_refnos_set.iter(), false, start_sesno as _)
                .await
                .unwrap();
            let mut modifed_owner_map = BTreeMap::new();
            for &refno in &modified_refnos_set {
                let Some(ele) = eles_map.get(&refno.into()) else {
                    continue;
                };
                let pe_data = ele.att_map().pe(dbnum);
                //处理几何体的筛选
                let noun = ele.att_map().get_type();
                let noun = noun.as_str();
                let refno_enum = refno.into();
                if PRIMITIVE_NOUN_NAMES.contains(&noun) {
                    geo_update_log.prim_refnos.insert(refno_enum);
                } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun) {
                    geo_update_log.loop_owner_refnos.insert(refno_enum);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&noun) {
                    geo_update_log.bran_hanger_refnos.insert(refno_enum);
                } else if CATA_GEO_NAMES.contains(&noun) {
                    geo_update_log.basic_cata_refnos.insert(refno_enum);
                } else if TOTAL_NEG_NOUN_NAMES.contains(&noun) {
                    if GENRAL_NEG_NOUN_NAMES.contains(&noun) {
                        geo_update_log.prim_refnos.insert(refno_enum);
                    } else if CATE_NEG_NOUN_NAMES.contains(&noun) {
                        geo_update_log.basic_cata_refnos.insert(refno_enum);
                    }
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_LOOP_NOUN_NAMES.contains(&noun) {
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_VERT_NOUN_NAMES.contains(&noun) {
                    if let Some(pe) = aios_core::get_pe(pe_data.owner).await.unwrap() {
                        final_check_geom_refnos.insert(pe.refno);
                        need_backup_geom_refnos.insert(pe.refno);
                    }
                } else {
                    final_check_geom_refnos.insert(pe_data.owner);
                }

                //保存 pe 数据到数据库
                let mut m_children_updated = None;
                if let Some((chidlren, _, children_updated)) =
                    children_changed_map.remove(&refno.into())
                {
                    if children_updated {
                        //需要更新 children
                        modifed_owner_map.insert(refno, chidlren);
                        m_children_updated = Some(true);
                    } else {
                        m_children_updated = Some(false);
                    }
                }
                //保存 pe 数据到数据库
                let pe_json = pe_data.gen_sur_json(m_children_updated, None);
                let sql = format!("UPSERT {} MERGE {}", refno.to_pe_key(), pe_json);
                // println!("{}", sql);
                SUL_DB.query(sql).await.unwrap();
                if let Some(att_json) = ele.att_map().gen_sur_json_exclude(&["id"], None) {
                    let sql = format!(
                        "UPSERT {}:{} CONTENT {}",
                        ele.att_map().get_type(),
                        refno.to_string(),
                        att_json
                    );
                    // println!("{}", sql);
                    SUL_DB.query(sql).await.unwrap();
                }
            }

            //执行 modifed 的 owner relate
            if !modifed_owner_map.is_empty() {
                #[cfg(feature = "debug_model")]
                dbg!(&modifed_owner_map);
                backup_owner_relate(modifed_owner_map.keys()).await.unwrap();
                for (owner, children) in modifed_owner_map {
                    let relate_json = children
                        .iter()
                        .enumerate()
                        .map(|(i, child)| {
                            let cp = child.to_pe_key();
                            let op = owner.to_pe_key();
                            format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1} }}", cp, op)
                        })
                        .collect::<Vec<String>>();
                    for chunk in relate_json.chunks(200) {
                        let sql = format!("INSERT RELATION INTO pe_owner [{}]", chunk.join(","));
                        println!("{}", sql);
                        SUL_DB.query(sql).await.unwrap();
                    }
                }
            }
        }

        //执行 added， todo use batch insert
        // let final_refnos = added_refnos_set.union(&modified_refnos_set).collect::<HashSet<_>>();
        // added_refnos_set.extend(modified_refnos_set);
        // let mut final_refnos = vec![];
        // final_refnos.extend(added_refnos_set.into_iter());
        // final_refnos.extend(modified_refnos_set.into_iter());
        if !added_refnos_set.is_empty() {
            #[cfg(feature = "debug_model")]
            dbg!(&added_refnos_set);
            let mut pe_json_vec = vec![];
            for &refno in added_refnos_set.iter() {
                let Some(ele) = eles_map.get(&refno.into()) else {
                    continue;
                };
                let pe_data = ele.att_map().pe(dbnum);

                //处理几何体的筛选
                let noun = ele.att_map().get_type();
                let noun = noun.as_str();
                let refno_enum = refno.into();
                if PRIMITIVE_NOUN_NAMES.contains(&noun) {
                    geo_update_log.prim_refnos.insert(refno_enum);
                } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun) {
                    geo_update_log.loop_owner_refnos.insert(refno_enum);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&noun) {
                    geo_update_log.bran_hanger_refnos.insert(refno_enum);
                } else if CATA_GEO_NAMES.contains(&noun) {
                    geo_update_log.basic_cata_refnos.insert(refno_enum);
                } else if TOTAL_NEG_NOUN_NAMES.contains(&noun) {
                    if GENRAL_NEG_NOUN_NAMES.contains(&noun) {
                        geo_update_log.prim_refnos.insert(refno_enum);
                    } else if CATE_NEG_NOUN_NAMES.contains(&noun) {
                        geo_update_log.basic_cata_refnos.insert(refno_enum);
                    }
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_LOOP_NOUN_NAMES.contains(&noun) {
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_VERT_NOUN_NAMES.contains(&noun) {
                    if let Some(pe) = aios_core::get_pe(pe_data.owner).await.unwrap() {
                        final_check_geom_refnos.insert(pe.refno);
                        need_backup_geom_refnos.insert(pe.refno);
                    }
                } else {
                    //检查是否是 pipe 一类的
                    final_check_geom_refnos.insert(pe_data.owner);
                }

                #[cfg(feature = "debug_model")]
                dbg!(refno);
                let children_updated = children_changed_map.get(&refno.into()).map(|x| x.2);
                let pe_json = pe_data.gen_sur_json(children_updated, Some(refno.to_pe_key()));
                pe_json_vec.push(pe_json);

                if let Some(att_json) = ele.att_map().gen_sur_json() {
                    SUL_DB
                        .query(format!(
                            "INSERT IGNORE INTO {} {}",
                            ele.att_map().get_type(),
                            att_json
                        ))
                        .await
                        .unwrap();
                }
            }
            let sql = format!("INSERT IGNORE INTO pe [{}]", pe_json_vec.join(","));
            // println!("{}", sql);
            let mut response = SUL_DB.query(sql).await.unwrap();
            let erros = response.take_errors();
            if !erros.is_empty() {
                dbg!(&erros);
            }
        }

        //新的 owner relate ，旧的 owner relate 要重新创建
        #[cfg(feature = "debug_model")]
        dbg!(&children_changed_map);
        //最后执行backup_owner_relate，然后添加新的 owner relate
        let owner_changed_refnos = children_changed_map
            .iter()
            .filter(|x| x.1 .1 > 0)
            .map(|x| x.0.refno())
            .collect::<Vec<_>>();
        #[cfg(feature = "debug_model")]
        dbg!(&owner_changed_refnos);
        if !owner_changed_refnos.is_empty() {
            backup_owner_relate(owner_changed_refnos.iter())
                .await
                .unwrap();
        }
        for (&owner, (children, _, children_updated)) in &children_changed_map {
            let relate_json = children
                .iter()
                .enumerate()
                .map(|(i, child)| {
                    let cp = child.to_pe_key();
                    let op = owner.to_pe_key();
                    format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1} }}", cp, op)
                })
                .collect::<Vec<String>>();
            for chunk in relate_json.chunks(200) {
                let sql = format!("INSERT RELATION INTO pe_owner [{}]", chunk.join(","));
                #[cfg(feature = "debug_sql")]
                println!("{}", sql);
                SUL_DB.query(sql).await.unwrap();
            }
        }

        //保存cur_ses_data 数据到 ses
        {
            let json = cur_ses_data.gen_sur_json(dbnum);
            let sql = format!("insert ignore into ses {};", json);
            SUL_DB.query(sql).await.unwrap();
            let sql = format!(
                "UPDATE ses:[{dbnum}, {start_sesno}] set add_cnt={}, mod_cnt={}, del_cnt={};",
                total_add_len, total_modify_len, total_deleted_len
            );
            SUL_DB.query(sql).await.unwrap();
        }

        //几何体的处理
        {
            
            //是否需要往上查找上面一点的层级来确定是否是几何体
            if let Ok(nouns) = aios_core::get_type_names(final_check_geom_refnos.iter()).await {
                for (&refno, noun) in final_check_geom_refnos.iter().zip(nouns) {
                    let noun = noun.as_str();
                    if PRIMITIVE_NOUN_NAMES.contains(&noun) {
                        geo_update_log.prim_refnos.insert(refno);
                    } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun) {
                        geo_update_log.loop_owner_refnos.insert(refno);
                    } else if CATA_HAS_TUBI_GEO_NAMES.contains(&noun) {
                        //如果发现是bran/hang， 需要备份之前的数据
                        dbg!(&refno);
                        need_backup_geom_refnos.insert(refno);
                        geo_update_log.bran_hanger_refnos.insert(refno);
                    } else if CATA_GEO_NAMES.contains(&noun) {
                        geo_update_log.basic_cata_refnos.insert(refno);
                    }
                }
            }
            dbg!(&need_backup_geom_refnos);
            backup_data(need_backup_geom_refnos.iter().map(|x| x.ref_refno()), false, start_sesno as _)
                .await
                .unwrap();

            // #[cfg(feature = "debug_model")]
            dbg!(&geo_update_log);
            let all_deep_refnos = geo_update_log
                .get_all_geom_refnos_deep()
                .await
                .into_iter()
                .collect::<Vec<_>>();
            #[cfg(feature = "debug_model")]
            dbg!(&all_deep_refnos);

            //有可能没更新完，就update了模型？
            gen_all_geos_data(vec![], &self.db_option, Some(geo_update_log))
                .await
                .unwrap();
            #[cfg(feature="debug_model")]
            dbg!(&all_deep_refnos);
            process_meshes_update_db(Some(Arc::new(self.db_option.clone())), &all_deep_refnos)
                .await
                .unwrap();
            // process_meshes_update_db_deep(&self.db_option, &all_deep_refnos)
            //     .await
            //     .unwrap();
            println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");
        }

        Ok(true)
    }

    //执行增量更新
    // pub async fn execute_incr_update_old(
    //     &self,
    //     increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
    // ) -> anyhow::Result<bool> {
    //     //没有增量更新的数据，直接返回
    //     if increment_ranges_map.is_empty() {
    //         return Ok(false);
    //     }
    //     let mut update_type_eles_map = HashMap::new();
    //     let mut deleted_refnos_set = HashSet::new();

    //     let mut total_add_len = 0;
    //     let mut total_modify_len = 0;
    //     let mut total_deleted_len = 0;
    //     let mut geo_update_log = IncrGeoUpdateLog::default();
    //     let mut delete_relate_sqls = vec![];
    //     let mut all_relate_sqls = vec![];
    //     let mut has_changed = false;
    //     let project = get_db_option().project_name.clone();
    //     //TODO: 如何鉴别只有claim page的变化，没有数据的更新，就不需要执行增量更新
    //     for (path, (basic_info, sesno_range)) in increment_ranges_map {
    //         let mut io = PdmsIO::new(&project, path, true);
    //         io.open()?;
    //         let mut eles_map = io.collect_increment_eles(sesno_range.clone()).await?;
    //         let sync_refnos = self.db_option.get_manual_sync_refnos();
    //         if !sync_refnos.is_empty() {
    //             for r in sync_refnos {
    //                 if let Ok(sync_map) = io.auto_get_elements_deep(r).await {
    //                     eles_map.extend(sync_map);
    //                 }
    //                 // dbg!(&sync_map);
    //             }
    //         }
    //         if eles_map.is_empty() {
    //             continue;
    //         }
    //         dbg!((sesno_range, eles_map.len()));
    //         if eles_map.is_empty() {
    //             continue;
    //         }
    //         // dbg!(&eles_map);
    //         //批量检测是否存在这些eles
    //         let mut exist_refnos = HashSet::new();
    //         let pes = eles_map.keys().map(|x| x.to_pe_key()).join(",");
    //         let mut resp = SUL_DB
    //             .query(format!("SELECT VALUE id FROM [{pes}];"))
    //             .await?;
    //         // dbg!(&resp);
    //         let refnos: Vec<RefU64> = resp.take(0).unwrap();
    //         exist_refnos.extend(refnos);
    //         for (&refno, _) in &eles_map {
    //             clear_all_caches(refno.into()).await;
    //         }

    //         let mut processed_owner_set = HashSet::new();
    //         for (&refno, ele) in &eles_map {
    //             let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
    //             let owner = attmap.get_owner().refno();
    //             let type_name = attmap.get_type();
    //             let type_name = type_name.as_str();
    //             has_changed = true;
    //             let mut ele_op = EleOperation::Modified;
    //             let mut need_update_all_relate_after_delete = false;
    //             let mut need_update_all_relate_after_add = false;
    //             //需要处理虽然存在，但是owner关系还没建立的情况
    //             if exist_refnos.contains(&ele.refno) {
    //                 let old_children = aios_core::get_children_refnos(ele.refno.into()).await?;
    //                 for (i, child) in old_children.into_iter().enumerate() {
    //                     let child = child.refno();
    //                     if !ele.children.contains(&child) {
    //                         //index current delete refno, owner refno
    //                         //需要get deep children
    //                         let deep_children =
    //                             aios_core::query_deep_children_refnos(child.into()).await?;
    //                         let t = (i, child, refno);
    //                         println!("Delete: {:?}", t);
    //                         deleted_refnos_set.extend(deep_children);
    //                         // deleted_refnos_set.insert(t);
    //                         total_deleted_len += 1;
    //                         need_update_all_relate_after_delete = true;
    //                     }
    //                 }

    //                 if ele_op == EleOperation::Modified {
    //                     total_modify_len += 1;
    //                 }
    //             } else {
    //                 total_add_len += 1;
    //                 ele_op = EleOperation::Add;
    //                 let mut index = 0;
    //                 let mut is_last_add = false;
    //                 dbg!(ele.refno);
    //                 if let Some(owner_ele) = eles_map.get(&owner.re) {
    //                     index = owner_ele
    //                         .children
    //                         .iter()
    //                         .position(|&x| x == refno.refno())
    //                         .unwrap_or(0);
    //                     is_last_add = index == owner_ele.children.len() - 1;

    //                     let cp = refno.to_pe_key();
    //                     let op = owner.to_pe_key();
    //                     //如果是最后一个，啥子都不用管，直接插入到最后
    //                     if is_last_add {
    //                         all_relate_sqls
    //                             .push(
    //                                 format!("RELATE {0}->pe_owner:[{1}, {index}]->{1};", cp, op,),
    //                             );
    //                     } else {
    //                         need_update_all_relate_after_add = true;
    //                     }
    //                 }
    //             }

    //             {
    //                 //只要有children，都可以直接进行一次
    //                 if !processed_owner_set.contains(&refno) {
    //                     //只有不是add的情况下才需要这么去删除
    //                     if ele_op != EleOperation::Add {
    //                         delete_relate_sqls.push(format!(
    //                             "delete pe_owner:[{0}, 0]..[{0}, 100];",
    //                             refno.to_pe_key()
    //                         ));
    //                     }

    //                     if !ele.children.is_empty() {
    //                         let relate_sqls = ele
    //                             .children
    //                             .iter()
    //                             .enumerate()
    //                             .map(|(i, child)| {
    //                                 let cp = child.to_pe_key();
    //                                 format!(
    //                                     "RELATE {0}->pe_owner:[{1}, {i}]->{1};",
    //                                     cp,
    //                                     refno.to_pe_key()
    //                                 )
    //                             })
    //                             .collect::<Vec<String>>();
    //                         all_relate_sqls.extend_from_slice(&relate_sqls);
    //                     }
    //                     processed_owner_set.insert(refno);
    //                 }
    //             }
    //             if eles_map.get(&owner).is_none() {
    //                 //如果有未发现的element 就需要去数据文件里找出这个element，然后添加
    //                 // dbg!(ele);
    //                 //找到这个owner，然后把最新的结果更新进来
    //                 if !processed_owner_set.contains(&owner) {
    //                     if let Ok(owner_ele) = io.auto_get_element(owner.refno()).await {
    //                         //始终维持最新的情况
    //                         delete_relate_sqls.push(format!(
    //                             "delete pe_owner:[{0}, 0]..[{0}, 100];",
    //                             owner.to_pe_key()
    //                         ));
    //                         let owner_pe_key = owner.to_pe_key();
    //                         if !owner_ele.children.is_empty() {
    //                             let relate_sqls = owner_ele
    //                                 .children
    //                                 .iter()
    //                                 .enumerate()
    //                                 .map(|(i, child)| {
    //                                     format!(
    //                                         "RELATE {0}->pe_owner:[{1}, {i}]->{1};",
    //                                         child.to_pe_key(),
    //                                         &owner_pe_key
    //                                     )
    //                                 })
    //                                 .collect::<Vec<String>>();
    //                             #[cfg(feature = "debug_parse")]
    //                             dbg!(&relate_sqls);
    //                             all_relate_sqls.extend_from_slice(&relate_sqls);
    //                         }
    //                         processed_owner_set.insert(owner);
    //                     }
    //                 }
    //             }

    //             #[cfg(feature = "debug_parse")]
    //             dbg!((refno, ele_op));
    //             //如果修改的是顶点， 这里要考虑到最终的构件，也要添加进来，比如 vert -> GWALL/FLOOR 等
    //             if PRIMITIVE_NOUN_NAMES.contains(&type_name) {
    //                 geo_update_log.prim_refnos.insert(refno);
    //             } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&type_name) {
    //                 geo_update_log.loop_owner_refnos.insert(refno);
    //             } else if CATA_HAS_TUBI_GEO_NAMES.contains(&type_name) {
    //                 geo_update_log.bran_hanger_refnos.insert(refno);
    //             } else if CATA_GEO_NAMES.contains(&type_name) {
    //                 geo_update_log.basic_cata_refnos.insert(refno);
    //             } else {
    //                 let owner_type = aios_core::get_type_name(owner).await?;
    //                 if CATA_HAS_TUBI_GEO_NAMES.contains(&owner_type.as_str()) {
    //                     // geo_update_log.basic_cata_refnos.insert(refno);
    //                     //直接更新整个bran，简化逻辑
    //                     geo_update_log.bran_hanger_refnos.insert(owner);
    //                 }
    //             }
    //             let increment_info = IncrementInfo {
    //                 refno: refno.refno(),
    //                 db_no: basic_info.pdms_header.db_num,
    //                 attr: attmap,
    //                 children: ele.children.clone(),
    //                 operation: ele_op,
    //             };
    //             if ele_op == EleOperation::Modified || ele_op == EleOperation::Add {
    //                 update_type_eles_map
    //                     .entry(ele.noun)
    //                     .or_insert(Vec::new())
    //                     .push(increment_info);
    //             };
    //         }
    //     }
    //     //如果没有发生变化，直接返回
    //     if !has_changed {
    //         return Ok(false);
    //     }

    //     //备份更新 tubi 的数据
    //     if !geo_update_log.bran_hanger_refnos.is_empty() {
    //         let bran_refnos = geo_update_log
    //             .bran_hanger_refnos
    //             .iter()
    //             .cloned()
    //             .collect::<Vec<_>>();
    //         // backup_data(&bran_refnos)
    //         //     .await
    //         //     .unwrap();
    //     }

    //     //relate 优先处理
    //     let mut relate_join_set = tokio::task::JoinSet::new();
    //     let mut time = Instant::now();
    //     // dbg!(all_relate_sqls.len());

    //     let mut chunks = delete_relate_sqls.chunks(100);
    //     for mut s in chunks {
    //         let sql = s.into_iter().join("");
    //         // dbg!(&sql);
    //         // #[cfg(feature = "debug_parse")]
    //         // println!("delete relates sql: {}", &sql);
    //         // relate_join_set.spawn(async move {
    //         SUL_DB.query(sql).await.unwrap();
    //         // });
    //     }

    //     let mut chunks = all_relate_sqls.chunks(100);
    //     for mut s in chunks {
    //         let sql = s.into_iter().join("");
    //         // dbg!(&sql);
    //         // #[cfg(feature = "debug_parse")]
    //         // println!("add relates sql: {}", &sql);
    //         relate_join_set.spawn(async move {
    //             SUL_DB.query(sql).await.unwrap();
    //         });
    //     }
    //     while let Some(_) = relate_join_set.join_next().await {}
    //     // println!("Relate pes task costs {} s", time.elapsed().as_secs_f32());

    //     // let mut att_pe_handles = vec![];
    //     //新增模型的处理
    //     for (noun, v) in update_type_eles_map {
    //         let type_name = db1_dehash(noun as _);
    //         if type_name.is_empty() {
    //             continue;
    //         }
    //         let type_name = type_name.as_str();

    //         for chunk in v.chunks(JSON_CHUNK_COUNT) {
    //             let mut insert_pe_jsons_str = String::new();
    //             let mut update_pe_sql_str = String::new();
    //             let mut update_att_sql_str = String::new();
    //             let mut history_refnos_set = Vec::new();
    //             for k in chunk {
    //                 let refno = k.refno.into();
    //                 history_refnos_set.push(refno);
    //                 let name = k.attr.get_name();
    //                 let pe = SPdmsElement {
    //                     refno,
    //                     owner: k.attr.get_owner(),
    //                     name: name.unwrap_or_default(),
    //                     noun: k.attr.get_type(),
    //                     dbnum: k.db_no,
    //                     sesno: k.attr.sesno(),
    //                     cata_hash: k.attr.cal_cata_hash(),
    //                     ..Default::default()
    //                 };

    //                 let pe_json = pe.gen_sur_json(None);
    //                 let att_json = k.attr.gen_sur_json_exclude(&["id"], None);
    //                 if k.is_modified() {
    //                     update_pe_sql_str.push_str(
    //                         format!("UPSERT {} CONTENT {};", refno.to_pe_key(), pe_json).as_str(),
    //                     );
    //                 } else {
    //                     insert_pe_jsons_str.push_str(&pe_json);
    //                     insert_pe_jsons_str.push_str(",");
    //                 }

    //                 //不管怎样，update和add，都用update的方式
    //                 if let Some(att_json) = att_json {
    //                     update_att_sql_str.push_str(
    //                         format!(
    //                             "UPSERT {} CONTENT {};",
    //                             refno.to_table_key(&type_name),
    //                             att_json
    //                         )
    //                         .as_str(),
    //                     );
    //                 }
    //             }

    //             //备份需要修改的历史数据
    //             // backup_data(&history_refnos_set)
    //             //     .await
    //             //     .unwrap();

    //             //调用函数，将当前数据存储到版本表里
    //             // println!("{}", &update_pe_sql_str);
    //             let insert_pe_sql = if !insert_pe_jsons_str.is_empty() {
    //                 insert_pe_jsons_str.pop();
    //                 format!("INSERT IGNORE INTO pe [{}];", insert_pe_jsons_str)
    //             } else {
    //                 "".to_owned()
    //             };

    //             // let handle = tokio::task::spawn(async move {
    //             if !update_att_sql_str.is_empty() {
    //                 // println!("update_att_sql_str: {}", &update_att_sql_str);
    //                 SUL_DB.query(update_att_sql_str).await.unwrap();
    //             }
    //             if !update_pe_sql_str.is_empty() {
    //                 // println!("update_pe_sql: {}", &update_pe_sql_str);
    //                 SUL_DB.query(update_pe_sql_str).await.unwrap();
    //             }

    //             //使用surreal 保存pe
    //             if !insert_pe_sql.is_empty() {
    //                 // println!("insert_pe_sql: {}", &insert_pe_sql);
    //                 SUL_DB.query(insert_pe_sql).await.unwrap();
    //             }
    //         }
    //     }

    //     //删除模型的处理
    //     let deleted_refnos: Vec<RefnoEnum> = deleted_refnos_set.into_iter().collect::<Vec<_>>();
    //     //备份需要删除的模型数据，还是暂时保留在原来的地方？
    //     // backup_data(&deleted_refnos)
    //     //     .await
    //     //     .unwrap();
    //     geo_update_log.delete_refnos.extend(deleted_refnos.clone());
    //     for chunk in deleted_refnos.chunks(JSON_CHUNK_COUNT) {
    //         let del_sql = chunk
    //             .into_iter()
    //             .map(|refno| format!("update {} set deleted=true;", refno.to_pe_key()))
    //             .join("");
    //         // dbg!(&del_sql);
    //         SUL_DB.query(&del_sql).await.unwrap();
    //     }

    //     let all_deep_refnos = geo_update_log
    //         .get_all_visible_refnos_deep()
    //         .await
    //         .into_iter()
    //         .collect::<Vec<_>>();

    //     //有可能没更新完，就update了模型？
    //     // gen_all_geos_data(vec![], &self.db_option, Some(geo_update_log))
    //     //     .await
    //     //     .unwrap();

    //     #[cfg(feature = "debug_sql")]
    //     dbg!(&all_deep_refnos);

    //     // process_meshes_update_db(Some(Arc::new(self.db_option.clone())), &all_deep_refnos)
    //     //     .await
    //     //     .unwrap();
    //     println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");
    //     Ok(true)
    // }

    //初始化监测
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        fs::create_dir_all("asset/archives")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);
        let db_option = get_db_option();
        let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();
        dbg!(&manual_dbnums);
        // let project = get_db_option().project_name.clone();

        for watch_dir in &self.watcher.watch_dirs {
            for entry in WalkDir::new(watch_dir).sort_by(|a, b| {
                b.path()
                    .metadata()
                    .unwrap()
                    .len()
                    .cmp(&a.path().metadata().unwrap().len())
            }) {
                let dir_entry = entry.unwrap();
                let path = dir_entry.path();
                let file_name = path.file_stem().unwrap().to_str().unwrap();
                if path.is_dir() {
                    continue;
                }

                let (db_type, file_version, db_num) = parse_db_basic_info(path.to_path_buf());
                //是否调试里有筛选
                if !manual_dbnums.is_empty() && !manual_dbnums.contains(&db_num) {
                    continue;
                }
                let project = get_db_option().project_name.clone();
                let file_latest_sesno = PdmsIO::new(&project, path.to_path_buf(), true)
                    .get_latest_att_sesno()
                    .unwrap_or_default();
                dbg!((db_num, file_latest_sesno));

                if !CHECK_DB_TYPES.contains(&db_type.as_str()) {
                    continue;
                }
                //TODO 这种情况，需要全新的解析
                let Ok(db_latest_sesno) = aios_core::query_db_latest_sesno(db_num).await else {
                    //先暂时跳过数据库里没有的文件，todo 考虑自动追加文件全新解析
                    continue;
                };
                dbg!(db_latest_sesno);
                // self.watcher
                //     .file_name_full_path_map
                //     .insert(file_name.to_owned(), path.to_path_buf());
                //初始化CBA的Archive文件，来保证后续增量下载, 后面是否需要加一个环境变量，来控制是否需要重新生成archive文件
                //是否需要完全初始化
                // let input = path.to_path_buf();
                // let output: PathBuf = format!("asset/archives/{}.cba", file_name).into();
                // join_set.spawn(async move {
                //     // let compress_opt = CompressOptions::new(input, output);
                //     // execute_compress(compress_opt).await.unwrap();
                // });

                let mut io = PdmsIO::new(&project, path, true);
                io.open().unwrap();
                //每个path 都要检查一遍
                if let Ok(basic_info) = io.get_page_basic_info() {
                    if db_latest_sesno != 0 {
                        #[cfg(feature = "debug_parse")]
                        dbg!((db_num, db_latest_sesno));
                        //暂时先跳过更新比较大的
                        if file_latest_sesno > db_latest_sesno
                            && (file_latest_sesno - db_latest_sesno < 1000)
                        {
                            println!("发现需要增量更新的文件: {:?}, 当前数据库属性最大pgno: {db_latest_sesno}, 文件属性对应pgno: {file_latest_sesno}", &file_name);
                            params.insert(
                                path.to_path_buf(),
                                (
                                    basic_info.clone(),
                                    (db_latest_sesno as i32 + 1)..=file_latest_sesno as i32,
                                ),
                            );
                        }
                        self.watcher.headers.insert(path.to_path_buf(), basic_info);
                    }
                }
            }
        }

        //等所有的文件都检查同步完毕，才执行更新
        //按每个单独的 sesno
        match self.execute_incr_update(params).await {
            Ok(true) => {
                println!("执行启动后的自动增量完成。")
            }
            Ok(false) => {
                println!("没有发生增量更新。")
            }
            Err(e) => {
                println!("Execute increment update error: {:?}", e);
            }
        }

        println!("初始化增量更新耗时: {} s", time.elapsed().as_secs_f32());

        anyhow::Ok(())
    }

    //开始监测数据文件夹
    pub async fn async_watch(&self) -> notify::Result<()> {
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        dbg!(&self.watcher.watch_dirs);
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher
                .watch(x.as_path(), RecursiveMode::NonRecursive)
                .expect("watch files failed");
        });

        create_dir_all("assets/archives").await.unwrap();
        create_dir_all("assets/temp").await.unwrap();
        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    // dbg!(&event);
                    //跳过只是meta data变动的情况
                    let data_changed = matches!(
                        event.kind,
                        notify::EventKind::Modify(notify::event::ModifyKind::Data(_))
                            | notify::EventKind::Modify(notify::event::ModifyKind::Any)
                            | notify::EventKind::Create(notify::event::CreateKind::File)
                            | notify::EventKind::Remove(notify::event::RemoveKind::File)
                    );
                    if !data_changed {
                        continue;
                    }
                    //后面用派发任务的方式,不要放在这里阻塞
                    // println!("changed: {:?}", &event);
                    // dbg!(&self.watcher.headers);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(&event.paths) {
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            // dbg!(new_header.pdms_header.page_no);
                            // dbg!(path);
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                // dbg!(path);
                                // dbg!(old.pdms_header.page_no);
                                //未发生修改，直接跳过
                                if old.latest_ses_data.sesno == new_header.latest_ses_data.sesno {
                                    continue;
                                }
                                //比如给出准确的范围next_sesno..=end_sesno
                                params.insert(
                                    path.clone(),
                                    (
                                        new_header.clone(),
                                        (old.latest_ses_data.sesno + 1)
                                            ..=new_header.latest_ses_data.sesno,
                                    ),
                                );
                            }
                        }
                        // dbg!(&params);
                        if params.is_empty() {
                            continue;
                        }
                        let mut notify_file_names = vec![];
                        let mut notify_file_hashes = vec![];

                        //如果数据没有发生变化，则不需要推出变化，不需要执行增量
                        match self.execute_incr_update(params).await {
                            Ok(true) => {
                                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                                for (path, new_header) in new_headers {
                                    let file_name = path.file_stem().unwrap().to_str().unwrap();
                                    if path.is_dir() {
                                        continue;
                                    }
                                    dbg!(&file_name);
                                    //这个地方是不是需要直接去读取文件，然后更新headers，不能太依赖json数据
                                    //或者每次启动都重新更新这个文件？
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        dbg!((
                                            old.latest_ses_data.sesno,
                                            new_header.latest_ses_data.sesno
                                        ));
                                        //未发生修改，直接跳过
                                        if old.latest_ses_data.sesno
                                            >= new_header.latest_ses_data.sesno
                                        {
                                            continue;
                                        }
                                        *old.value_mut() = new_header;

                                        //发生修改的文件，重新生成archive
                                        // dbg!(&path);
                                        let output: PathBuf =
                                            format!("asset/archives/{}.cba", file_name).into();
                                        // dbg!(&output);

                                        // let compress_opt = CompressOptions::new(
                                        //     path.clone(),
                                        //     output,
                                        //     "asset/temp",
                                        // );
                                        // let file_hash = execute_compress(compress_opt)
                                        //     .await
                                        //     .unwrap()
                                        //     .to_string();
                                        // // dbg!(&file_hash);
                                        //
                                        // //数据库里不存在这个file hash的记录，才需要发送
                                        // //是自己创建的，在记录里还没有的，才能发送消息出去
                                        // //如果是别的创建的，就应该调过
                                        // let sql = format!(
                                        //     "select value id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                                        //     get_db_option().location.as_str(),
                                        //     file_name,
                                        //     &file_hash
                                        // );
                                        // // dbg!(&sql);
                                        // let mut response = SUL_DB.query(&sql).await.unwrap();
                                        // // dbg!(&response);
                                        // let id = response.take::<Vec<String>>(0).unwrap();
                                        // // dbg!(id.len());
                                        // if id.is_empty() {
                                        //     println!("发生了增量更新，推送：{}", &file_name);
                                        //     notify_file_hashes.push(file_hash);
                                        //     notify_file_names.push(file_name.to_owned());
                                        // }
                                    }
                                }
                                //now save the watch.json
                                // self.watcher.save(None).expect("save watch.json failed");
                            }
                            Ok(false) => {
                                println!("{:?} 文件发生修改，但是没有发生增量更新。", &event.paths);
                                continue;
                            }
                            Err(e) => {
                                println!("Execute increment update error: {:?}", e);
                            }
                        }
                        //publish notify db file updates
                        dbg!(&notify_file_names);
                        let payload = SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);
                        //自己本地也要保存, todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                        SUL_DB
                            .query(format!(
                                "INSERT IGNORE INTO e3d_sync {} ",
                                serde_json::to_string(&payload).unwrap()
                            ))
                            .await
                            .unwrap();
                        //todo 检查是否只是发生了claim page的变化，如果只是claim修改，是需要每次都同步？
                        //会导致出现循环
                        // self.mqtt_client
                        //     .clone()
                        //     .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                        //     .await
                        //     .unwrap();
                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
