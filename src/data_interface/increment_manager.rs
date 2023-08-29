use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use aios_core::cache::mgr::BytesTrait;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::consts::NAME_HASH;
use aios_core::helper::qualified_table_name;
use aios_core::pdms_types::{RefU64, RefU64Vec};
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

#[derive(PartialEq, Debug, Default, Clone, Copy)]
pub enum EleOperation{
    #[default]
    None,
    Add,
    Modified,
    Deleted,
}

impl AiosDBManager {
    pub fn cal_default_name(&self, refno: RefU64) -> anyhow::Result<String> {
        let mut attmap = self.get_attr_from_localdb(refno)?;
        return if let Some(name) = attmap.get(&NAME_HASH) {
            Ok(name.string_value())
        } else {
            let owner = attmap.get_owner().unwrap();
            let type_name = attmap.get_type();
            let owner_children = self.get_children_attrs(owner).unwrap_or_default();
            let idx = owner_children.iter().filter(|x| {
                x.get_type() == type_name
            }).position(|node| node.get_refno().unwrap_or_default() == refno).unwrap_or_default() + 1;
            let default_name = format!("{} {}", type_name, idx);
            attmap.insert(NAME_HASH, StringType(default_name.clone()));
            let mut bytes = attmap.into_rkyv_compress_bytes();
            self.get_cur_attmap_tree().unwrap().insert((*refno).to_be_bytes().as_slice(), &*bytes)?;
            Ok(default_name)
        };
    }


    ///执行增量更新
    pub async fn execute_incr_update(
        &self,
        increment_ranges_map: IndexMap<PathBuf, i32>,
    ) -> anyhow::Result<bool> {
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
        for (path, dbno) in increment_ranges_map {
            let mut io = PdmsIO::new(path, true);
            io.open()?;
            let eles = io.collect_increment_eles()?;
            // dbnum_eles_map.insert(dbno, eles);
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
                }else{
                    ele_op = EleOperation::Add;
                }

                let mut vec = ele.children.to_bytes()?;
                children_db.insert((*ele.refno).to_be_bytes().as_slice(), &*vec)?;


                if ele_op != EleOperation::Deleted {
                    //保存到本地数据库
                    let mut bytes = attmap.into_rkyv_compress_bytes();
                    attmap_db.insert((*ele.refno).to_be_bytes().as_slice(), &*bytes)?;
                    if ele_op == EleOperation::Add { total_add_len += 1;  } else{ total_modify_len += 1;}

                    type_eles_map.entry(ele.noun).or_insert(Vec::new()).push((ele.refno, dbno, attmap, ele_op));
                }else{
                    total_delted_len += 1;
                }
            }
        }

        for (noun, eles) in type_eles_map {
            for (refno, dbnum, ele, ele_op) in eles {
                let Some(owner) = ele.get_owner() else {
                    continue;
                };

                let type_name = ele.get_type();
                let _ = CACHED_REFNO_BASIC_MAP.insert(refno, &CachedRefBasic {
                    owner,
                    table: qualified_table_name(type_name),
                });
                let owner_children = self.get_children_from_localdb(owner).unwrap_or_default();
                let order = owner_children.iter().position(|x| *x == refno).unwrap_or_default() as u32;
                let cata_hash = ele.cal_cata_hash().map(|x| x.to_string());
                let name = self.cal_default_name(refno).unwrap();
                let pdms_element = PdmsEleGraphNode {
                    refno,
                    owner,
                    name,
                    noun: db1_dehash(noun),
                    order,
                    dbnum,
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
            dbg!(result);
            remove_edges_arangodb(&database, result, AQL_PDMS_EDGES_COLLECTION).await;
        }

        for edge in edges.chunks(ARANGODB_SAVE_AMOUNT) {
            let json = serde_json::to_value(edge)?;
            save_arangodb_with_db_option(&database, json, AQL_PDMS_EDGES_COLLECTION).await?;
        }

        println!("增加:{total_add_len}，修改:{total_modify_len}，删除:{total_delted_len}");

        Ok(true)
    }

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
                        for (path, new_header) in new_headers {
                            if let Some(mut old) = self.watcher.headers.get_mut(&path) {
                                //未发生修改，直接跳过
                                if old.pdms_header.page_no == new_header.pdms_header.page_no { continue; }
                                // let range = (old.file_size..new_header.file_size);
                                params.insert(path.clone(), new_header.pdms_header.db_num);
                                // self.watcher.headers.insert(path, new_header);
                                *old.value_mut() = new_header;
                            }
                        }
                        // dbg!(&params);
                        match self.execute_incr_update(params).await {
                            Ok(_) => {}
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