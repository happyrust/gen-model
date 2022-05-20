use aios_core::pdms_types::{AiosStr, AttrMap, PdmsTree, RefU64, RefU64Vec};
use smol_str::SmolStr;
use async_trait::async_trait;
use glam::TransformRT;
use id_tree::NodeId;

#[async_trait]
pub trait PdmsDataInterface {
    async fn sync_total_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn sync_incremental_project(&mut self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn get_ele_attr(&mut self, refno: RefU64) -> anyhow::Result<AttrMap>;

    async fn get_ele_children_attrs(&mut self, refno: RefU64) -> Vec<AttrMap>;

    async fn get_ele_children_refs(&mut self, refno: RefU64) -> RefU64Vec;

    async fn get_ele_world_transform(&mut self, refno: RefU64) -> TransformRT;

    async fn get_pdms_tree(&mut self, project: &str, db_no: u32) -> Option<PdmsTree>;

    async fn get_node_id(&mut self, refno: RefU64) -> Option<NodeId>;

    async fn get_name(&mut self, refno: RefU64) -> SmolStr;

    async fn get_name_by_hash(&mut self, refno: RefU64, name_hash: u32) -> Option<SmolStr>;

    async fn get_refnos_by_type(&mut self,project_name:SmolStr,att_type:&str) -> Option<RefU64Vec>;

    async fn get_tree_root(&self,project:&str,db_no:u32) -> Option<(RefU64,AiosStr)>;
}