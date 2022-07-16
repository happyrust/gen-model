use std::collections::VecDeque;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::pdms_types::{AiosStr, AttrMap, EleTreeNode, PdmsTree, RefU64, RefU64Vec};
use smol_str::SmolStr;
use async_trait::async_trait;
use dashmap::mapref::one::Ref;
use glam::TransformRT;
use id_tree::NodeId;


#[async_trait]
pub trait PdmsDataInterface : Send + Sync{

    async fn sync_total_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn sync_incremental_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    fn get_owner(&self, refno: RefU64) -> RefU64;

    async fn get_implicit_attr(&self, refno: RefU64, columns: Option<Vec<&str>>) -> anyhow::Result<AttrMap>;

    async fn get_implicit_attrs_by_owner(&self, owner: RefU64, type_name: &str, columns: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>>;

    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    fn get_refno_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>>;

    fn get_owner_ref_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>>;

    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    async fn get_world(&self, project: &str, mdb_name: &str, module: &str)  -> anyhow::Result<EleTreeNode>;

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>>;

    async fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>>;

    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec>;

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr>;

    async fn get_refnos_by_types<'a>(&self, project: &'a str, att_types: &'a Vec<&str>, dbnos: Option<Vec<i32>>) -> anyhow::Result<RefU64Vec>;

    async fn get_db_world(&self, project: &str, db_no: u32) -> anyhow::Result<Option<(RefU64, String)>>;

    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64>;

    fn get_ancestors_refnos_without_world(&self, refno: RefU64) -> Vec<RefU64>;

    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap>;

    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>>;

    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<glam::TransformRT>>;

}
