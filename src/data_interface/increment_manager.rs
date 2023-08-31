use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use aios_core::cache::mgr::BytesTrait;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::consts::NAME_HASH;
use aios_core::helper::qualified_table_name;
use aios_core::pdms_types::{AttrMap, AttrVal, RefU64, RefU64Vec};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::db1_dehash;
use anyhow::anyhow;
use indexmap::IndexMap;
use notify::{RecursiveMode, Watcher};
use pdms_io::watch::PdmsWatcher;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::database::sync_total_async_threaded;
use futures::{
    channel::mpsc::{channel, Receiver},
    SinkExt, StreamExt, future::ok,
};
use pdms_io::io::PdmsIO;
use crate::consts::*;
use crate::defines::CACHED_REFNO_BASIC_MAP;
use crate::graph_db::pdms_arango::{remove_edges_arangodb, save_arangodb_with_db_option};
use crate::graph_db::structs::{PdmsEleGraphEdgeWithKey, PdmsEleGraphNode};
use std::sync::Arc;
use walkdir::WalkDir;
use serde::{Serialize, Deserialize};
use crate::data_interface::increment_cecord::IncreaseDataTiDB;

#[derive(Debug, Default, Clone)]
pub struct IncrementInfo {
    pub refno: RefU64,
    pub db_no: i32,
    pub attr: AttrMap,
    pub children: RefU64Vec,
    pub operation: EleOperation,
}

#[derive(PartialEq, Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub enum EleOperation {
    #[default]
    None,
    Add,
    Modified,
    Deleted,
}

impl EleOperation {
    pub fn into_tidb_num(&self) -> u8 {
        match &self {
            EleOperation::None => { 0 }
            EleOperation::Add => { 1 }
            EleOperation::Modified => { 2 }
            EleOperation::Deleted => { 3 }
        }
    }
}

impl AiosDBManager {
    ///默认的名称
    pub fn default_name(&self, refno: RefU64) -> anyhow::Result<String> {
        let mut attmap = self.get_attr_from_localdb(refno)?;
        let owner = attmap.get_owner().unwrap();
        let type_name = attmap.get_type();
        let owner_children = self.get_children_attrs(owner).unwrap_or_default();
        let idx = owner_children.iter().filter(|x| {
            x.get_type() == type_name
        }).position(|node| node.get_refno().unwrap_or_default() == refno).unwrap_or_default() + 1;
        Ok(format!("{} {}", type_name, idx))
    }

    ///计算名称
    pub fn cal_name(&self, refno: RefU64) -> anyhow::Result<(String, bool)> {
        let mut attmap = self.get_attr_from_localdb(refno)?;
        return if let Some(name) = attmap.get(&NAME_HASH) {
            Ok((name.string_value(), false))
        } else {
            let default_name = self.default_name(refno)?;
            Ok((default_name, true))
        };
    }


    ///执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, (i32, u32)>,
    ) -> anyhow::Result<bool> {
        if increment_ranges_map.is_empty() {  return Ok(true); }
        let mut type_eles_map = HashMap::new();
        let mut delete_keys = vec![];
        let mut deleted_refnos_set = HashSet::new();

        let attmap_db = self.get_cur_attmap_tree().unwrap();
        let children_db = self.get_cur_children_tree().unwrap();
        let mut pdms_elements = vec![];
        let mut edges = vec![];
        let mut total_add_len = 0;
        let mut total_modify_len = 0;
        let mut total_delted_len = 0;
        for (path, (dbno, last_pageno)) in increment_ranges_map {
            let mut io = PdmsIO::new(path, true);
            io.open()?;
            let eles = io.collect_increment_eles(Some(last_pageno))?;
            for ele in eles {
                let attmap = ele.whole_attmap.merge();
                let mut ele_op = EleOperation::Modified;
                if let Ok(old_refnos) = self.get_children_from_localdb(ele.refno) {
                    old_refnos.iter()
                        .filter(|x| !ele.children.contains(*x))
                        .for_each(|x| {
                            let key = x.hash_with_another_refno(ele.refno);
                            delete_keys.push(key.to_string());
                            deleted_refnos_set.insert(*x);
                            //执行的是父节点的操作
                            ele_op = EleOperation::Deleted;
                        });
                } else {
                    ele_op = EleOperation::Add;
                }
                type_eles_map.entry(ele.noun).or_insert(Vec::new()).push(IncrementInfo {
                    refno: ele.refno,
                    db_no: dbno,
                    attr: attmap,
                    children: ele.children,
                    operation: ele_op,
                });

                match ele_op {
                    EleOperation::None => {}
                    EleOperation::Add => { total_add_len += 1; }
                    EleOperation::Modified => { total_modify_len += 1; }
                    EleOperation::Deleted => { total_delted_len += 1; }
                }
            }
        }
        // 将记录保存到tidb
        let mut increment_data_record = Vec::new();
        for (noun, eles) in &type_eles_map {
            for ele in eles {
                let Ok(old_attr) = self.get_attr(ele.refno).await else { continue; };
                increment_data_record.push(IncreaseDataTiDB {
                    refno: ele.refno,
                    data_operate: ele.operation,
                    numbdb: ele.db_no,
                    children: ele.children.clone(),
                    old_attr,
                    new_attr: ele.attr.clone(),
                    new_version: 0,
                    old_version: 0,
                });
            }
        }
        // 暂时都保存到desi项目里面
        if let Some(pool) = self.project_map.get(&self.db_option.project_name) {
            let _ = IncreaseDataTiDB::save_increment_data(increment_data_record, "default".to_string(), pool.value()).await?;
        }
        ///先更新一遍到本地数据库
        for (noun, eles) in &type_eles_map {
            for ele in eles {
                let Some(owner) = ele.attr.get_owner() else {
                    continue;
                };
                let mut vec = ele.children.to_bytes()?;
                children_db.insert((ele.refno).to_be_bytes().as_slice(), &*vec)?;

                let mut bytes = ele.attr.into_rkyv_compress_bytes();
                attmap_db.insert((ele.refno).to_be_bytes().as_slice(), &*bytes)?;
            }
        }


        let mut updated_sets = HashSet::new();
        for (mut noun, mut eles) in type_eles_map {
            while let Some(mut ele) = eles.pop() {
                let refno = ele.refno;
                updated_sets.insert(ele.refno);
                let Some(owner) = ele.attr.get_owner() else {
                    continue;
                };
                let type_name = ele.attr.get_type();
                let _ = CACHED_REFNO_BASIC_MAP.insert(refno, &CachedRefBasic {
                    owner,
                    table: qualified_table_name(type_name),
                });
                let owner_children = self.get_children_from_localdb(owner).unwrap_or_default();
                let order = owner_children.iter().position(|x| *x == refno).unwrap_or_default() as u32;
                let cata_hash = ele.attr.cal_cata_hash().map(|x| x.to_string());
                //owner children need update all the name, if current name not set
                let (name, is_default) = self.cal_name(refno).unwrap();
                let next = order as usize + 1;
                if is_default && next < owner_children.len() {
                    let remind_siblings = &owner_children[next..];
                    let mut tmp_default_name = name.clone();
                    ele.attr.insert(NAME_HASH, AttrVal::StringType(tmp_default_name.clone()));
                    let mut bytes = ele.attr.into_rkyv_compress_bytes();
                    attmap_db.insert((*refno).to_be_bytes().as_slice(), &*bytes)?;
                    for r in remind_siblings {
                        //如果在缓存里，才加入到这个列表里, 需要刷新一下列表
                        if CACHED_REFNO_BASIC_MAP.contains_key(r) {
                            if let Ok(mut tmp_att) = self.get_attr_from_localdb(*r) {
                                if tmp_att.get_name_string() == tmp_default_name {
                                    tmp_default_name = self.default_name(*r)?;
                                    tmp_att.insert(NAME_HASH, AttrVal::StringType(tmp_default_name.clone()));
                                    let mut bytes = tmp_att.into_rkyv_compress_bytes();
                                    attmap_db.insert((**r).to_be_bytes().as_slice(), &*bytes)?;

                                    // eles.push((*r, dbnum, tmp_att, Default::default()));
                                    eles.push(IncrementInfo {
                                        refno: *r,
                                        db_no: ele.db_no,
                                        attr: tmp_att,
                                        children: Default::default(),
                                        operation: Default::default(),
                                    });
                                }
                            }
                        }
                    }
                }
                let pdms_element = PdmsEleGraphNode {
                    refno,
                    owner,
                    name,
                    noun: db1_dehash(noun),
                    order,
                    dbnum: ele.db_no,
                    cata_hash,
                };
                let key = refno.hash_with_another_refno(owner);
                let pdms_edge = PdmsEleGraphEdgeWithKey {
                    _key: key.to_string(),
                    _from: format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno()),
                    _to: format!("{}/{}", AQL_PDMS_ELES_COLLECTION, owner.to_url_refno()),
                };
                pdms_elements.push(pdms_element);
                edges.push(pdms_edge);
            }
        }

        let database = self.get_arango_db().await?;
        for result in pdms_elements.chunks(ARANGODB_SAVE_AMOUNT) {
            let json = serde_json::to_value(result)?;
            save_arangodb_with_db_option(&database, json, AQL_PDMS_ELES_COLLECTION).await?;
        }

        //删除边
        for result in delete_keys.chunks(ARANGODB_SAVE_AMOUNT) {
            // dbg!(result);
            remove_edges_arangodb(&database, result, AQL_PDMS_EDGES_COLLECTION).await;
        }

        for edge in edges.chunks(ARANGODB_SAVE_AMOUNT) {
            let json = serde_json::to_value(edge)?;
            save_arangodb_with_db_option(&database, json, AQL_PDMS_EDGES_COLLECTION).await?;
        }

        println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_delted_len}");

        // 将记录保存到tidb


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
                        if old.pdms_header.page_no == basic_info.pdms_header.page_no { continue; }
                        params.insert(path.to_path_buf(), (basic_info.pdms_header.db_num, old.pdms_header.page_no));
                        //在old里有出现，但是版本号不一致，需要更新
                        latest_need_update_headers.insert(path.to_path_buf(), basic_info);
                    }else {
                        //在old里面没有出现，需要更新进来
                        self.watcher.headers.insert(path.to_path_buf(), basic_info);
                    }

                }
            }
        }
        dbg!(params.len());
        match self.execute_incr_update(params).await {
            Ok(_) => {
                //执行没问题了，再更新当前的版本记录，headers直接存本地json
                for (path, new_header) in latest_need_update_headers {
                    if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                        //未发生修改，直接跳过
                        if old.pdms_header.page_no == new_header.pdms_header.page_no { continue; }
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

    ///开始监测数据文件夹
    pub async fn async_watch(&self) -> notify::Result<()> {
        let (mut watcher, mut rx) = PdmsWatcher::async_watcher()?;
        self.watcher.watch_dirs.iter().for_each(|x| {
            watcher.watch(x.as_path(), RecursiveMode::NonRecursive);
        });


        while let Some(res) = rx.next().await {
            match res {
                Ok(event) => {
                    println!("changed: {:?}", &event);
                    if let Ok(new_headers) = PdmsWatcher::scan_db_headers(event.paths) {
                        // dbg!(&new_headers);
                        let mut params = IndexMap::new();
                        for (path, new_header) in &new_headers {
                            if let Some(mut old) = self.watcher.headers.get_mut(path) {
                                //未发生修改，直接跳过
                                if old.pdms_header.page_no == new_header.pdms_header.page_no { continue; }
                                params.insert(path.clone(), (new_header.pdms_header.db_num, old.pdms_header.page_no));
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
                                        if old.pdms_header.page_no == new_header.pdms_header.page_no { continue; }
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