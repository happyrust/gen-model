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

    async fn sync_incremental_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    async fn get_ele_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>>;

    async fn get_ele_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec>;

    async fn get_ele_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<TransformRT>>;

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr>;

    async fn get_refnos_by_type(&self,project_name:SmolStr,att_type:&str) -> anyhow::Result<RefU64Vec>;

    async fn get_tree_root(&self,project:&str,db_no:u32) -> anyhow::Result<Option<(RefU64,AiosStr)>>;
}