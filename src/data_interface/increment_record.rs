use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::pdms_types::*;
use aios_core::{AttrMap, RefnoEnum};
use chrono::{DateTime, Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
#[cfg(feature = "sql")]
use sqlx::types::Uuid;
#[cfg(feature = "sql")]
use sqlx::{Executor, MySql, Pool, Row};
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;
use surrealdb::sql::Thing;

pub const INCREMENT_DATA: &'static str = "INCREMENT_DATA";

///需要修改的模型的增量参考号数据
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncrGeoUpdateLog {
    //基本体模型修改了的参考号
    pub prim_refnos: HashSet<RefnoEnum>,
    //拉伸体模型修改了的参考号
    pub loop_owner_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub bran_hanger_refnos: HashSet<RefnoEnum>,
    //元件库模型的属性修改了的参考号
    pub basic_cata_refnos: HashSet<RefnoEnum>,
    //删除了的模型
    pub delete_refnos: HashSet<RefnoEnum>,
}

impl IncrGeoUpdateLog {
    #[inline]
    pub fn count(&self) -> usize {
        self.prim_refnos.len()
            + self.loop_owner_refnos.len()
            + self.basic_cata_refnos.len()
            + self.bran_hanger_refnos.len()
    }

    #[inline]
    pub fn get_all_visible_refnos(&self) -> HashSet<RefnoEnum> {
        let mut refnos = HashSet::new();
        refnos.extend(self.prim_refnos.iter());
        refnos.extend(self.loop_owner_refnos.iter());
        refnos.extend(self.basic_cata_refnos.iter());
        refnos.extend(self.bran_hanger_refnos.iter());
        refnos
    }

    #[inline]
    pub async fn get_all_geom_refnos_deep(&self) -> HashSet<RefnoEnum> {
        let mut refnos = HashSet::new();
        refnos.extend(self.prim_refnos.iter());
        refnos.extend(self.loop_owner_refnos.iter());
        refnos.extend(self.basic_cata_refnos.iter());
        let children = aios_core::get_all_children_refnos(self.bran_hanger_refnos.iter())
            .await
            .unwrap_or_default();
        refnos.extend(children);
        refnos
    }
}

//各个db的信息记录，需要跟踪起来？

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct IncrEleUpdateLog {
    pub refno: RefnoEnum,
    pub data_operate: EleOperation,
    pub numbdb: i32,
    // pub children: RefnoEnumVec,
    pub old_attr: AttrMap,
    pub new_attr: AttrMap,
    pub new_version: u32,
    pub old_version: u32,

    //按时间戳去对比更新是否完成
    pub timestamp: surrealdb::sql::Datetime,
}

#[derive(Debug, Deserialize)]
struct Record {
    #[allow(dead_code)]
    id: Thing,
}
