use aios_core::cache::mgr::BytesTrait;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::consts::NAME_HASH;
use aios_core::helper::qualified_table_name;
use aios_core::pdms_types::*;
use anyhow::anyhow;
use indexmap::IndexMap;
use notify::{RecursiveMode, Watcher};
use pdms_io::io::PdmsIO;
use pdms_io::watch::PdmsWatcher;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
// use pdms_io::watch::PdmsWatcher;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::database::sync_total_async_threaded;
use futures::{
    channel::mpsc::{channel, Receiver},
    future::ok,
    SinkExt, StreamExt,
};
// use pdms_io::io::PdmsIO;
use crate::consts::*;
use crate::data_interface::increment_record::{IncrEleUpdateLog, IncrGeoUpdateLog};
use crate::defines::CACHED_REFNO_BASIC_MAP;
use crate::graph_db::pdms_arango::{remove_edges_arangodb, save_arangodb_with_db_option};
use crate::graph_db::structs::{PdmsEleData, PdmsEleEdge};
use crate::surreal_service;
use crate::surreal_service::SUL_DB;
use aios_core::orm::pdms_element;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::{AttrMap, AttrVal, RefU64Vec};
use pdms_io::defines::DbPageBasicInfo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use walkdir::WalkDir;
use crate::data_interface::gen_model::gen_all_geos_data;

#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    pub refno: RefU64,
    pub db_no: i32,
    pub attr: NamedAttrMap,
    pub children: RefU64Vec,
    pub operation: EleOperation,
}

impl AiosDBManager {
    ///执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (DbPageBasicInfo, u32)>,
    ) -> anyhow::Result<bool> {
        if increment_ranges_map.is_empty() {
            return Ok(true);
        }
        let mut type_eles_map = HashMap::new();
        let mut delete_keys = vec![];
        let mut delete_maps: HashMap<RefU64, IncrementInfo> = HashMap::new();
        let mut deleted_refnos_set = HashSet::new();

        let mut pdms_elements = vec![];
        let mut total_add_len = 0;
        let mut total_modify_len = 0;
        let mut total_deleted_len = 0;
        for (path, (basic_info, last_pageno)) in increment_ranges_map {
            let mut io = PdmsIO::new(path, true);
            io.open()?;
            let eles = io.collect_increment_eles(&basic_info, Some(last_pageno))?;
            dbg!(eles.len());
            for ele in eles {
                let mut attmap: NamedAttrMap = ele.whole_attmap.merge().into();
                attmap.set_e3d_version(ele.version as _);
                let mut ele_op = EleOperation::Modified;
                // 删除只是owner的children变化了，但是需要记录删除的节点
                if let Ok(old_refnos) = surreal_service::get_children_refnos(ele.refno).await {
                    // if let Ok(old_refnos) = surreal_service::get_children_refnos(ele.refno) {
                    old_refnos
                        .iter()
                        .filter(|x| !ele.children.contains(*x))
                        .for_each(|x| {
                            let key = x.hash_with_another_refno(ele.refno);
                            delete_keys.push(key.to_string());
                            deleted_refnos_set.insert(*x);
                            //执行的是父节点的操作
                            ele_op = EleOperation::Deleted;
                            dbg!(*x);
                            delete_maps.entry(*x).or_insert(IncrementInfo {
                                refno: *x,
                                db_no: basic_info.pdms_header.db_num,
                                attr: Default::default(),
                                children: Default::default(),
                                operation: EleOperation::Deleted,
                            });
                        });
                } else {
                    ele_op = EleOperation::Add;
                }
                type_eles_map
                    .entry(ele.noun)
                    .or_insert(Vec::new())
                    // .push(attmap)
                    .push(IncrementInfo {
                        refno: ele.refno,
                        db_no: basic_info.pdms_header.db_num,
                        attr: attmap,
                        children: ele.children,
                        operation: ele_op,
                    })
                ;

                match ele_op {
                    EleOperation::None => {}
                    EleOperation::Add => {
                        total_add_len += 1;
                    }
                    EleOperation::Modified => {
                        total_modify_len += 1;
                    }
                    EleOperation::Deleted => {
                        total_deleted_len += 1;
                    }
                }
            }
        }

        for (&noun, v) in type_eles_map.iter() {
            let type_name = db1_dehash(noun as _);
            if type_name.is_empty() {
                continue;
            }
            let atts = v.iter().map(|x| &x.attr).collect::<Vec<_>>();
            //使用surreal 保存NamedAttrMap
            SUL_DB
                .query(format!("INSERT IGNORE INTO {} $values", &type_name))
                .bind(("values", &atts))
                .await
                .unwrap();
        }

        // let mut increment_data_record = Vec::new();
        // for (noun, eles) in &type_eles_map {
        //     for ele in eles {
        //         match ele.operation {
        //             EleOperation::None => { continue; }
        //             EleOperation::Add => {
        //                 increment_data_record.push(IncrEleUpdateLog {
        //                     refno: ele.refno,
        //                     data_operate: ele.operation,
        //                     numbdb: ele.db_no,
        //                     children: Default::default(),
        //                     old_attr: Default::default(),
        //                     new_attr: ele.attr.clone(),
        //                     new_version: 0,
        //                     old_version: 0,
        //                     timestamp: timestamp.clone(),
        //                 });
        //             }
        //             EleOperation::Modified => {
        //                 let Ok(old_attr) = self.get_attr(ele.refno).await else { continue; };
        //                 increment_data_record.push(IncrEleUpdateLog {
        //                     refno: ele.refno,
        //                     data_operate: ele.operation,
        //                     numbdb: ele.db_no,
        //                     children: ele.children.clone(),
        //                     old_attr,
        //                     new_attr: ele.attr.clone(),
        //                     new_version: 0,
        //                     old_version: 0,
        //                     timestamp: timestamp.clone(),
        //                 });
        //             }
        //             EleOperation::Deleted => { continue; }
        //         }
        //     }
        // }
        //
        // 删除做单独处理
        // for (refno, map) in delete_maps {
        //     let Ok(old_attr) = self.get_attr(refno).await else { continue; };
        //     increment_data_record.push(IncrEleUpdateLog {
        //         refno: map.refno,
        //         data_operate: map.operation,
        //         numbdb: map.db_no,
        //         children: map.children.clone(),
        //         old_attr,
        //         new_attr: map.attr,
        //         new_version: 0,
        //         old_version: 0,
        //         timestamp : timestamp.clone(),
        //     });
        // }
        // 暂时都保存到desi项目里面
        // if let Some(pool) = self.project_map.get(&self.db_option.project_name) {
        //     let _ = IncrEleUpdateLog::save_increment_data_to_sql(increment_data_record, "default".to_string(), pool.value()).await?;
        // }
        ///先更新一遍到本地数据库
        for (noun, eles) in &type_eles_map {
            for ele in eles {
                // let owner = ele.attr.get_owner();
                // if owner.is_unset() {  continue; }
                // let mut vec = ele.children.to_bytes()?;
                // children_db.insert((ele.refno).to_be_bytes().as_slice(), &*vec)?;
                //
                // let mut bytes = ele.attr.into_rkyv_compress_bytes();
                // attmap_db.insert((ele.refno).to_be_bytes().as_slice(), &*bytes)?;
            }
        }

        let mut updated_sets = HashSet::new();
        let mut geo_update_log = IncrGeoUpdateLog::default();
        for (mut noun, mut incrs) in type_eles_map {
            while let Some(mut incr) = incrs.pop() {
                let refno = incr.refno;
                updated_sets.insert(incr.refno);
                let owner = incr.attr.get_owner();
                if owner.is_unset() {
                    continue;
                }

                let type_name = incr.attr.get_type_str();
                if PRIMITIVE_NOUN_NAMES.contains(&type_name) {
                    geo_update_log.prim_refnos.push(refno);
                } else if GNERAL_LOOP_NOUN_NAMES.contains(&type_name) {
                    geo_update_log.loop_refnos.push(refno);
                } else if CATA_HAS_TUBI_GEO_NAMES.contains(&type_name) {
                    geo_update_log.basic_cata_refnos.push(refno);
                } else if CATA_GEO_NAMES.contains(&type_name) {
                    geo_update_log.basic_cata_refnos.push(refno);
                }

                let pdms_element = pdms_element::Model {
                    id: refno.to_string(),
                    refno,
                    owner,
                    name: incr.attr.get_string_or_default("NAME"),
                    noun: db1_dehash(noun),
                    dbnum: incr.db_no,
                    e3d_version: incr.attr.get_e3d_version(),
                    version_tag: None,
                    status_tag: None,
                    cata_hash: None,
                    // tag_lock:false,
                    lock: false,
                };
                pdms_elements.push(pdms_element);
            }
        }

        for c in pdms_elements.chunks(500) {
            SUL_DB
                .query("INSERT IGNORE INTO pe $values")
                .bind(("values", &c))
                .await
                .unwrap();
        }

        // let database = self.get_arango_db().await?;
        // for result in pdms_elements.chunks(ARANGODB_SAVE_AMOUNT) {
        //     let json = serde_json::to_value(result)?;
        //     save_arangodb_with_db_option(&database, json, AQL_PDMS_ELES_COLLECTION).await?;
        // }

        //删除边
        // for result in delete_keys.chunks(ARANGODB_SAVE_AMOUNT) {
        //     // dbg!(result);
        //     remove_edges_arangodb(&database, result, AQL_PDMS_EDGES_COLLECTION).await;
        // }

        // for edge in edges.chunks(ARANGODB_SAVE_AMOUNT) {
        //     let json = serde_json::to_value(edge)?;
        //     save_arangodb_with_db_option(&database, json, AQL_PDMS_EDGES_COLLECTION).await?;
        // }

        dbg!(geo_update_log.prim_refnos.len());
        let r: Vec<IncrGeoUpdateLog> = SUL_DB
            .create("incr_model_log")
            .content(&geo_update_log)
            .await?;

        gen_all_geos_data(Arc::new(self.clone()), Some(geo_update_log)).await.unwrap();

        println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_deleted_len}");

        Ok(true)
    }

    pub async fn init_watcher(&self) -> anyhow::Result<()> {
        let mut params = IndexMap::new();
        let mut latest_need_update_headers = IndexMap::new();
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
                if path.is_dir() {
                    continue;
                }
                let mut io = PdmsIO::new(path, true);
                io.open()?;
                if let Ok(basic_info) = io.get_page_basic_info() {
                    if let Some(mut old) = self.watcher.headers.get_mut(&path.to_path_buf()) {
                        //未发生修改，直接跳过
                        if old.pdms_header.page_no == basic_info.pdms_header.page_no {
                            continue;
                        }
                        //pdms_header.db_num
                        params.insert(
                            path.to_path_buf(),
                            (basic_info.clone(), old.pdms_header.page_no),
                        );
                        //在old里有出现，但是版本号不一致，需要更新
                        latest_need_update_headers.insert(path.to_path_buf(), basic_info);
                    } else {
                        //在old里面没有出现，需要更新进来
                        self.watcher.headers.insert(path.to_path_buf(), basic_info);
                    }
                }
            }
        }
        match self.execute_incr_update(params).await {
            Ok(_) => {
                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                for (path, new_header) in latest_need_update_headers {
                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                        //未发生修改，直接跳过
                        if old.pdms_header.page_no == new_header.pdms_header.page_no {
                            continue;
                        }
                        *old.value_mut() = new_header;
                    }
                }
                //now save the watch.json
                self.watcher.save()?;
                println!("执行启动后的自动增量完成。")
            }
            Err(e) => {
                println!("Execute increment update error: {:?}", e);
            }
        }

        anyhow::Ok(())
    }

    //开始监测数据文件夹
    pub async fn async_watch(&self) -> notify::Result<()> {
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher
                .watch(x.as_path(), RecursiveMode::NonRecursive)
                .expect("watch files failed");
        });

        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    // println!("changed: {:?}", &event);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(event.paths) {
                        // dbg!(&new_headers);
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                //未发生修改，直接跳过
                                if old.pdms_header.page_no == new_header.pdms_header.page_no {
                                    continue;
                                }
                                params.insert(
                                    path.clone(),
                                    (new_header.clone(), old.pdms_header.page_no),
                                );
                                // *old.value_mut() = new_header;
                            }
                        }
                        // dbg!(&params);
                        match self.execute_incr_update(params).await {
                            Ok(_) => {
                                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                                for (path, new_header) in new_headers {
                                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                        //未发生修改，直接跳过
                                        if old.pdms_header.page_no == new_header.pdms_header.page_no
                                        {
                                            continue;
                                        }
                                        *old.value_mut() = new_header;
                                    }
                                }
                                //now save the watch.json
                                self.watcher.save();
                            }
                            Err(e) => {
                                println!("Execute increment update error: {:?}", e);
                            }
                        }
                    }
                }
                Err(e) => println!("watch error: {:?}", e),
            }
        }

        Ok(())
    }
}
