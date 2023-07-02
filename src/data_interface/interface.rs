use std::collections::{HashMap, VecDeque};
use std::dbg;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::pdms_types::{AiosStr, AttrMap, EleTreeNode, PdmsTree, RefU64, RefU64Vec};
use aios_core::prim_geo::spine::Spine3D;
use aios_core::shape::pdms_shape::PlantMesh;
use smol_str::SmolStr;
use async_trait::async_trait;
use dashmap::mapref::one::Ref;
use id_tree::NodeId;
use bevy_transform::prelude::*;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use crate::data_interface::tidb_manager::AiosDBManager;


#[async_trait]
pub trait PdmsDataInterface : Send + Sync{

    ///同步整个项目
    async fn sync_total_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    ///增量同步项目
    async fn sync_incremental_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    ///获得属性
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    ///获得参考号类型
    fn get_type_name(&self, refno: RefU64) -> anyhow::Result<String>;

    ///从本地获取属性数据
    fn get_attr_from_localdb(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    ///从本地获取children
    fn get_children_from_localdb(&self, refno: RefU64) -> anyhow::Result<RefU64Vec>;

    ///从本地获取mesh属性数据
    fn get_mesh_from_localdb(&self, geo_hash: u64) -> anyhow::Result<PlantMesh>;

    ///从本地获取mesh aabb属性数据
    fn get_mesh_aabb_from_localdb(&self, geo_hash: u64) -> anyhow::Result<Aabb>;

    fn get_attr_within_project(&self, refno: RefU64, project: &str) -> anyhow::Result<AttrMap>;

    fn get_children_within_project(&self, refno: RefU64, project: &str) -> anyhow::Result<RefU64Vec>;

    ///获得包含UDA的属性
    async fn get_attr_with_uda(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    ///获得参考号的Owner
    fn get_owner(&self, refno: RefU64) -> RefU64;

    ///获得根据refno出去的外键路径, 只设置一个终点，返回最后的结果
    async fn query_foreign_refnos(&self, refnos: &[RefU64], start_types: &[&[&str]], end_types: &[&str], t_types: &[&str], depth: u32) -> anyhow::Result<Vec<RefU64>>;

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(&self, refno: RefU64, start_types: &[&str], end_types: &[&str], t_types: &[&str]) -> anyhow::Result<Option<RefU64>>;

    async fn get_implicit_attr(&self, refno: RefU64, columns: Option<Vec<&str>>) -> anyhow::Result<AttrMap>;

    async fn get_implicit_attrs_by_owner(&self, owner: RefU64, type_name: &str, columns: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>>;

    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    //todo 后面要去掉，不需要使用这个，直接用上memcache + arangodb
    fn get_refno_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>>;

    fn get_owner_ref_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>>;

    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    async fn get_world(&self, project: &str, mdb_name: &str, module: &str)  -> anyhow::Result<EleTreeNode>;

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>>;

    ///获得子节点的属性集合
    fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>>;

    ///获得子节点的refno集合
    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec>;

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<String>;

    async fn get_refnos_by_types(&self, project: &str, att_types: &[&str], dbnos: &[i32]) -> anyhow::Result<RefU64Vec>;

    ///获得db的world参考号
    async fn get_db_world(&self, project: &str, db_no: u32) -> anyhow::Result<Option<(RefU64, String)>>;

    ///获得refno的祖先参考号
    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64>;

    ///获得refno的祖先参考号, 排除world
    fn get_ancestors_refnos_without_world(&self, refno: RefU64) -> Vec<RefU64>;

    ///查询指定参考号下哪些有负实体的参考号
    async fn query_refnos_has_neg_geom(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>>;

    ///查询指定参考号下负实体和正实体的集合
    async fn query_refnos_has_pos_neg_map(&self, refnos: &[RefU64]) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>>;

    ///查询哪些节点下面有负实体
    async fn query_parent_refnos_has_neg_geos(&self, refnos: &[RefU64]) -> anyhow::Result<Vec<RefU64>>;

    ///查询有几何体的父节点 refno
    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>>;

    ///查询指定参考号下负实体的集合
    async fn query_refnos_has_neg_map(&self, refno: RefU64) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>>;

    ///获得祖先参考属性集合
    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap>;

    ///获得祖先node集合
    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>>;

    ///获得指定参考号的世界坐标系
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>>;

    // async fn get_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>>;

    ///获取当前节点深度遍历后的所有子节点, 是否指定目标节点
    async fn get_travel_children_attrs(&self, refno:RefU64, nouns: &[&str]) -> anyhow::Result<Vec<AttrMap>>;


    /*******  几何相关算法    ********/

    ///获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(&self, refno: RefU64, distance: f32) -> anyhow::Result<Vec<RefU64>>;

    ///获得spline的路径，包括直线路径，圆弧路径
    fn get_spline_path(&self, refno: RefU64) -> anyhow::Result<Vec<Spine3D>>;

}
