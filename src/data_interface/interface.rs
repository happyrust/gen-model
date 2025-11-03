use std::collections::{BTreeMap, HashMap, VecDeque};

use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::{GmParam, ScomInfo};
use aios_core::pdms_types::*;
use aios_core::prim_geo::spine::Spine3D;
use aios_core::*;
use aios_core::{AttrMap, RefU64Vec};
use bevy_transform::prelude::*;
use dashmap::mapref::one::Ref;
use glam::Vec3;

// #[async_trait]
pub trait PdmsDataInterface: Send + Sync {
    ///同步整个项目
    async fn sync_total_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    ///增量同步项目
    async fn sync_incremental_project(&self) -> anyhow::Result<bool> {
        Ok(true)
    }

    ///获得属性
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<NamedAttrMap>;

    ///获得参考号类型
    async fn get_type_name(&self, refno: RefU64) -> String;

    async fn get_next(&self, refno: RefU64) -> anyhow::Result<RefU64>;

    async fn get_prev(&self, refno: RefU64) -> anyhow::Result<RefU64>;

    ///获得参考号的Owner
    fn get_owner(&self, refno: RefU64) -> RefU64;

    ///获得根据refno出去的外键路径, 只设置一个终点，返回最后的结果
    async fn query_foreign_refnos(
        &self,
        refnos: &[RefU64],
        start_types: &[&[&str]],
        end_types: &[&str],
        t_types: &[&str],
        depth: u32,
    ) -> anyhow::Result<Vec<RefU64>>;

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(
        &self,
        refno: RefU64,
        start_types: &[&str],
        end_types: &[&str],
        t_types: &[&str],
    ) -> anyhow::Result<Option<RefU64>>;

    async fn get_implicit_attr(
        &self,
        refno: RefU64,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<AttrMap>;

    async fn get_implicit_attrs_by_owner(
        &self,
        owner: RefU64,
        type_name: &str,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<Vec<AttrMap>>;

    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap>;

    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>>;

    fn get_cur_project(&self) -> &str;

    fn get_cur_mdb(&self) -> &str;

    async fn get_world(
        &self,
        project: &str,
        mdb_name: &str,
        module: &str,
    ) -> anyhow::Result<PdmsElement>;

    async fn get_desi_world(&self) -> anyhow::Result<PdmsElement>;

    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>>;

    ///获得子节点的refno集合
    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec>;

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<String>;

    async fn get_refnos_by_types(
        &self,
        project: &str,
        att_types: &[&str],
        dbnos: &[i32],
    ) -> anyhow::Result<RefU64Vec>;

    ///获得db的world参考号
    async fn get_db_world(
        &self,
        project: &str,
        db_no: u32,
    ) -> anyhow::Result<Option<(RefU64, String)>>;

    ///获得refno的祖先参考号
    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64>;

    ///查询指定参考号下哪些有负实体的参考号
    async fn query_refnos_has_neg_geom(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>>;

    ///查询指定参考号下负实体和正实体的集合
    async fn query_refnos_has_pos_neg_map(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>>;

    ///查询哪些节点下面有负实体
    async fn query_parent_refnos_has_neg_geos(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<Vec<RefU64>>;

    ///查询有几何体的父节点 refno
    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>>;

    ///查询指定参考号下负实体的集合
    async fn query_refnos_has_neg_map(
        &self,
        refno: RefU64,
    ) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>>;

    ///获得祖先参考属性集合
    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap>;

    ///获得祖先node集合
    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>>;

    ///获得指定参考号的世界坐标系
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>>;

    async fn get_world_transform_or_default(&self, refno: RefU64) -> Transform;

    ///获取当前节点深度遍历后的所有子节点, 是否指定目标节点
    async fn get_deep_children_attrs(
        &self,
        refno: RefU64,
        nouns: &[&str],
    ) -> anyhow::Result<Vec<NamedAttrMap>>;

    /*******  几何相关算法    ********/

    ///获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(
        &self,
        refno: RefU64,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>>;
    fn get_refnos_within_bound_radius_by_pos(
        &self,
        pos: Vec3,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>>;

    ///获得spline的路径，包括直线路径，圆弧路径
    async fn get_spline_path(&self, refno: RefU64) -> anyhow::Result<Vec<Spine3D>>;

    //元件库常用的方法
    async fn get_foreign_refno(&self, refno: RefU64, foreign: &str) -> Option<RefU64>;

    async fn get_foreign_attrmap(&self, refno: RefU64, foreign: &str) -> Option<NamedAttrMap>;

    async fn get_spre_ref(&self, refno: RefU64) -> Option<RefU64>;

    async fn get_cat_refno(&self, refno: RefU64) -> Option<RefU64>;

    async fn get_cat_attmap(&self, refno: RefU64) -> Option<NamedAttrMap>;
}
