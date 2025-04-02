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
// use pdms_io::sync::compress::{execute_compress, CompressOptions};
use pdms_io::watch::PdmsWatcher;
use petgraph::visit::Walker;
use rumqttc::QoS;
use tokio::fs::create_dir_all;
use walkdir::WalkDir;

use crate::data_interface::increment_record::IncrGeoUpdateLog;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::*;
use crate::mqtt_service::SyncE3dFileMsg;
use parse_pdms_db::parse::DbBasicInfo;

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
    /// 执行增量更新操作
    ///
    /// 该函数处理多个数据库文件的增量更新。
    ///
    /// # 参数
    ///
    /// * `increment_ranges_map` - 包含路径和对应的数据库页面基本信息及会话号范围的映射
    ///   键为数据库文件路径，值为元组，包含数据库页面基本信息和需要更新的会话号范围
    ///
    /// # 返回值
    ///
    /// * `anyhow::Result<bool>` - 成功返回Ok(true)，失败返回错误
    ///
    /// # 错误
    ///
    /// 当数据库操作失败时会返回错误
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, RangeInclusive<i32>)>,
    ) -> anyhow::Result<bool> {
        for (path, (basic_info, sesno_range)) in increment_ranges_map {
            //call execute_incr_update_single_sesno
            //一步一步执行更新
            let new_sesno = sesno_range.end().clone();
            for sesno in sesno_range {
                self.execute_incr_update_single_sesno(&path, &basic_info, sesno)
                    .await?;
                //执行完后，需要更新sesno 到最新的 db_file_info 中
                // let db_num = basic_info.pdms_header.db_num;
                //更新 sesno 到 db_file_info 中

                // let latest_sesno = query_latest_sesno(db_num).await?;
                // update_latest_sesno(db_num, sesno).await?;
            }
            //更新 sesno 到 db_file_info 中
            let file_name = path.file_stem().unwrap().to_str().unwrap();
            // dbg!(&file_name);
            //更新 sesno 到 db_file_info 中的sql
            let sql = format!("UPDATE db_file_info:{} SET sesno={};", file_name, new_sesno);
            //执行更新
            SUL_DB.query(sql).await.unwrap();
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

        let mut io = PdmsIO::new(&project, path, true);
        io.open()?;
        let cur_ses_data = io.get_ses_data(start_sesno as _)?.clone();
        let mut eles_map = io
            .collect_increment_eles((start_sesno..=start_sesno))
            .await?;
        // dbg!(&eles_map);
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
        let sql = format!("SELECT VALUE [id, (select value in from <-pe_owner)] FROM [{pes}] where record::exists(id) && !deleted");
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
                        (
                            ele.children.clone(),
                            RefU64Vec(old_children.clone()),
                            !children_eq,
                        ),
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
                            // println!("Delete: {:?}", t);
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
                    children_changed_map
                        .insert(refno, (ele.children.clone(), RefU64Vec::default(), true));
                }
            }
        }

        //执行 backup deleted
        if !deleted_refnos_set.is_empty() {
            // dbg!(&deleted_refnos_set);
            //删除的几何体处理，需要判断是否是几何体
            for &refno in &deleted_refnos_set {
                final_check_geom_refnos.insert(refno.into());
            }
            backup_data(deleted_refnos_set.iter(), true, start_sesno as _)
                .await
                .unwrap();
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
                let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
                //处理几何体的筛选
                let noun = attmap.get_type();
                let noun_str = noun.as_str();
                let refno_enum = refno.into();
                if PRIMITIVE_NOUN_NAMES.contains(&noun_str) {
                    geo_update_log.prim_refnos.insert(refno_enum);
                } else if GNERAL_LOOP_OWNER_NOUN_NAMES.contains(&noun_str) {
                    geo_update_log.loop_owner_refnos.insert(refno_enum);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&noun_str) {
                    geo_update_log.bran_hanger_refnos.insert(refno_enum);
                } else if CATA_GEO_NAMES.contains(&noun_str) {
                    geo_update_log.basic_cata_refnos.insert(refno_enum);
                } else if TOTAL_NEG_NOUN_NAMES.contains(&noun_str) {
                    if GENRAL_NEG_NOUN_NAMES.contains(&noun_str) {
                        geo_update_log.prim_refnos.insert(refno_enum);
                    } else if CATE_NEG_NOUN_NAMES.contains(&noun_str) {
                        geo_update_log.basic_cata_refnos.insert(refno_enum);
                    }
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_LOOP_NOUN_NAMES.contains(&noun_str) {
                    need_backup_geom_refnos.insert(pe_data.owner);
                    final_check_geom_refnos.insert(pe_data.owner);
                } else if TOTAL_VERT_NOUN_NAMES.contains(&noun_str) {
                    if let Some(pe) = aios_core::get_pe(pe_data.owner).await.unwrap() {
                        final_check_geom_refnos.insert(pe.refno);
                        need_backup_geom_refnos.insert(pe.refno);
                    }
                } else {
                    final_check_geom_refnos.insert(pe_data.owner);
                }

                //保存 pe 数据到数据库
                let mut m_children_updated = None;
                if let Some((chidlren, old_chidlren, children_updated)) =
                    children_changed_map.remove(&refno.into())
                {
                    if children_updated {
                        //需要更新 children
                        modifed_owner_map.insert(refno, (chidlren, old_chidlren));
                        m_children_updated = Some(true);
                    } else {
                        m_children_updated = Some(false);
                    }
                }
                //保存 pe 数据到数据库
                let pe_json = pe_data.gen_sur_json(Some(refno.to_pe_key()));
                let sql = format!("UPSERT {} MERGE {}", refno.to_pe_key(), pe_json);
                // println!("{}", sql);
                SUL_DB.query(sql).await.unwrap();
                //保存 att 数据到数据库
                if let Some(att_json) = attmap.gen_sur_json_exclude(&["id"], None) {
                    let sql = format!(
                        "UPSERT {}:{} CONTENT {}",
                        attmap.get_type(),
                        refno.to_string(),
                        att_json
                    );
                    // println!("{}", sql);
                    SUL_DB.query(sql).await.unwrap();
                }
                //保存 UDA 数据到数据库
                if let Some(uda_json) = attmap.gen_sur_json_uda(&[]) {
                    let normalized_uda_json = aios_core::helper::normalize_sql_string(&uda_json);
                    // dbg!(&normalized_uda_json);
                    let sql = format!(
                        "UPSERT ATT_UDA:{} CONTENT {}",
                        refno.to_string(),
                        normalized_uda_json
                    );
                    SUL_DB.query(sql).await.unwrap();
                }
            }

            //执行 modifed 的 owner relate
            if !modifed_owner_map.is_empty() {
                #[cfg(feature = "debug_model")]
                dbg!(&modifed_owner_map);
                SUL_DB.query("BEGIN TRANSACTION;").await.unwrap();
                // backup_owner_relate(modifed_owner_map.keys()).await.unwrap();
                for (owner, (new_children, old_children)) in modifed_owner_map {
                    // 删除owner的relate关系
                    let sql = format!(
                        "select value in from (DELETE select value id FROM {}<-pe_owner RETURN BEFORE)",
                        owner.to_pe_key()
                    );
                    let mut resp = SUL_DB.query(sql).await.unwrap();
                    let mut merged_children: Vec<RefU64> = resp.take(0).unwrap();

                    // 合并 children 和 old_children，维持 old_children 的顺序
                    let mut insert_pos = 0;
                    for child in new_children {
                        if !merged_children.contains(&child) {
                            // 找到合适的位置插入
                            merged_children.insert(insert_pos, child);
                        } else {
                            insert_pos =
                                merged_children.iter().position(|&o| o == child).unwrap() + 1;
                        }
                    }

                    // dbg!(&merged_children);

                    // 生成 relate_json, 如果是被删除的，需要加上deleted 标签
                    let relate_json = merged_children
                        .iter()
                        .enumerate()
                        .map(|(i, child)| {
                            let cp = child.to_pe_key();
                            let op = owner.to_pe_key();
                            let deleted = deleted_refnos_set.contains(child);
                            if deleted {
                                format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1}, deleted: true }}", cp, op)
                            } else {
                                format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1} }}", cp, op)
                            }
                        })
                        .collect::<Vec<String>>();
                    for chunk in relate_json.chunks(200) {
                        let sql = format!("INSERT RELATION INTO pe_owner [{}]", chunk.join(","));
                        // println!("{}", sql);
                        SUL_DB.query(sql).await.unwrap();
                    }
                }
                SUL_DB.query("COMMIT TRANSACTION;").await.unwrap();
            }
        }
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
                let pe_json = pe_data.gen_sur_json(Some(refno.to_pe_key()));
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
            if !pe_json_vec.is_empty() {
                for chunk in pe_json_vec.chunks(300) {
                    let sql = format!("INSERT IGNORE INTO pe [{}]", chunk.join(","));
                    // println!("{}", sql);
                    let mut response = SUL_DB.query(sql).await.unwrap();
                    let erros = response.take_errors();
                    if !erros.is_empty() {
                        dbg!(&erros);
                    }
                }
            }
        }

        //新的 owner relate
        #[cfg(feature = "debug_model")]
        dbg!(&children_changed_map);
        //最后执行backup_owner_relate，然后添加新的 owner relate
        // let owner_changed_refnos = children_changed_map
        //     .iter()
        //     .filter(|x| x.1 .1 > 0)
        //     .map(|x| x.0.refno())
        //     .collect::<Vec<_>>();
        // // #[cfg(feature = "debug_model")]
        // dbg!(&owner_changed_refnos);
        // if !owner_changed_refnos.is_empty() {
        //     // backup_owner_relate(owner_changed_refnos.iter())
        //     //     .await
        //     //     .unwrap();
        // }

        for (&owner, (children, old_children, children_updated)) in &children_changed_map {
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
            //modified_refnos_set 里已经包含的，就不需要备份了
            need_backup_geom_refnos.retain(|x| !modified_refnos_set.contains(&x.refno()));
            #[cfg(feature = "debug_model")]
            dbg!(&need_backup_geom_refnos);
            backup_data(
                need_backup_geom_refnos.iter().map(|x| x.ref_refno()),
                false,
                start_sesno as _,
            )
            .await
            .unwrap();

            // #[cfg(feature = "debug_model")]
            // dbg!(&geo_update_log);
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
                .expect("gen_all_geos_data failed");
            #[cfg(feature = "debug_model")]
            dbg!(&all_deep_refnos);
            // process_meshes_update_db(Some(Arc::new(self.db_option.clone())), &all_deep_refnos)
            //     .await
            //     .unwrap();
            process_meshes_update_db_deep(&self.db_option, &all_deep_refnos)
                .await
                .expect("process_meshes_update_db_deep failed");
            println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");
        }
        Ok(true)
    }

    ///初始化监测
    /// 启动时监测数据文件夹里的文件变化
    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        fs::create_dir_all("assets/archives")?;
        let mut time = Instant::now();
        dbg!(&self.watcher.watch_dirs);
        let db_option = get_db_option();
        let manual_dbnums = db_option.manual_db_nums.clone().unwrap_or_default();
        let exclude_dbnums = db_option.exclude_db_nums.clone().unwrap_or_default();

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

                let DbBasicInfo {
                    db_type,
                    ses_pgno,
                    db_no,
                } = parse_db_basic_info(path.to_path_buf());
                //是否调试里有筛选
                if !manual_dbnums.is_empty() && !manual_dbnums.contains(&db_no) {
                    continue;
                }
                //过滤掉排除的数据库编号
                if !exclude_dbnums.is_empty() && exclude_dbnums.contains(&db_no) {
                    continue;
                }
                let project = get_db_option().project_name.clone();
                let file_latest_sesno = PdmsIO::new(&project, path.to_path_buf(), true)
                    .get_latest_sesno()
                    .unwrap_or_default();
                // dbg!((db_no, file_latest_sesno));

                if !CHECK_DB_TYPES.contains(&db_type.as_str()) {
                    continue;
                }
                //TODO 这种情况，需要全新的解析
                let Ok(db_latest_sesno) = aios_core::query_latest_sesno(db_no).await else {
                    //先暂时跳过数据库里没有的文件，todo 考虑自动追加文件全新解析
                    continue;
                };
                // dbg!((db_no, db_latest_sesno));
                if db_latest_sesno == 0 {
                    continue;
                }
                self.watcher
                    .file_name_full_path_map
                    .insert(file_name.to_owned(), path.to_path_buf());
                // dbg!(db_latest_sesno);
                //只有开启异地同步时，才需要初始化异地更新压缩数据包
                #[cfg(feature = "mqtt")]
                {
                    // 初始化CBA的Archive文件，来保证后续增量下载, 后面是否需要加一个环境变量，来控制是否需要重新生成archive文件
                    // 是否需要完全初始化
                    let input = path.to_path_buf();
                    let output: PathBuf = format!("assets/archives/{}.cba", file_name).into();
                    // join_set.spawn(async move {
                    let compress_opt = CompressOptions::new(input, output, "assets/temp");
                    execute_compress(compress_opt)
                        .await
                        .expect("compress failed");
                    // });
                }

                //每个path 都要检查一遍
                if db_latest_sesno != 0 {
                    // #[cfg(feature = "debug_parse")]
                    dbg!((db_no, db_latest_sesno));
                    //暂时先跳过更新比较大的
                    if file_latest_sesno > db_latest_sesno {
                        let mut io = PdmsIO::new(&project, path, true);
                        io.open()?;
                        if let Ok(basic_info) = io.get_page_basic_info() {
                            println!("发现需要增量更新的文件: {:?}, 当前数据库属性最大pgno: {db_latest_sesno},\
                                        文件属性对应pgno: {file_latest_sesno}", &file_name);
                            params.insert(
                                path.to_path_buf(),
                                (
                                    basic_info.clone(),
                                    (db_latest_sesno as i32 + 1)..=file_latest_sesno as i32,
                                ),
                            );
                            self.watcher.headers.insert(path.to_path_buf(), basic_info);
                        }
                    }
                }
            }
        }

        //等所有的文件都检查同步完毕，才执行更新
        //按每个单独的 sesno
        dbg!(params.len());
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
                            // dbg!(&new_header.pdms_header);
                            // dbg!(path);
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                // dbg!(path);
                                // dbg!(&old.pdms_header);
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
                                    let dbno = new_header.pdms_header.db_num as u32;
                                    if path.is_dir() {
                                        continue;
                                    }
                                    // dbg!(&file_name);
                                    //这个地方是不是需要直接去读取文件，然后更新headers，不能太依赖json数据
                                    //或者每次启动都重新更新这个文件？
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        // dbg!((
                                        //     old.latest_ses_data.sesno,
                                        //     new_header.latest_ses_data.sesno
                                        // ));
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
                                            format!("assets/archives/{}.cba", file_name).into();
                                        // dbg!(&output);

                                        let compress_opt = CompressOptions::new(
                                            path.clone(),
                                            output,
                                            "assets/temp",
                                        );
                                        let file_hash = execute_compress(compress_opt)
                                            .await
                                            .unwrap()
                                            .to_string();
                                        // dbg!(&file_hash);

                                        //如果location_dbs为空，则不进行筛选
                                        //说明是所有地区都推送，跳过检查
                                        //必须要是地区对应的dbnos才能推送
                                        if let Some(location_dbs) = &get_db_option().location_dbs {
                                            if !location_dbs.contains(&dbno) {
                                                continue;
                                            }
                                        }

                                        //数据库里不存在这个file hash的记录，才需要发送
                                        //是自己创建的，在记录里还没有的，才能发送消息出去
                                        //如果是别的创建的，就应该调过
                                        let sql = format!(
                                            "select value <string>\
                                            id from (select * from e3d_sync where location != '{}' and '{}' in file_names and '{}' in file_hashes order by timestamp desc) ",
                                            get_db_option().location.as_str(),
                                            file_name,
                                            &file_hash
                                        );
                                        // dbg!(&sql);
                                        // println!("sql is {}", &sql);
                                        let mut response = SUL_DB.query(&sql).await.unwrap();
                                        // dbg!(&response);
                                        let id = response.take::<Vec<String>>(0).unwrap();
                                        // dbg!(id.len());
                                        if id.is_empty() {
                                            println!("发生了增量更新，推送：{}", &file_name);
                                            notify_file_hashes.push(file_hash);
                                            notify_file_names.push(file_name.to_owned());
                                        }
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
                        #[cfg(feature = "mqtt")]
                        if !notify_file_names.is_empty() {
                            let payload =
                                SyncE3dFileMsg::new(notify_file_names, notify_file_hashes);
                            //自己本地也要保存
                            // todo 后续还是要配置哪些dbs，哪个地方能修改，哪个地方是不能改的
                            SUL_DB
                                .query(format!(
                                    "INSERT IGNORE INTO e3d_sync {} ",
                                    serde_json::to_string(&payload).unwrap()
                                ))
                                .await
                                .unwrap();
                            //todo 检查是否只是发生了claim page的变化，如果只是claim修改，是需要每次都同步？
                            //会导致出现循环
                            self.mqtt_client
                                .clone()
                                .publish("Sync/E3d", QoS::ExactlyOnce, true, payload)
                                .await
                                .unwrap();
                        }
                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
