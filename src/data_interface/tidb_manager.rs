use aios_core::cache::mgr::*;
use aios_core::cache::refno::*;
use aios_core::consts::*;
use aios_core::db_number::DbNumMgr;
use aios_core::parsed_data::geo_params_data::CateGeoParam::*;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::*;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_types::*;
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::extrusion::Extrusion;
use aios_core::prim_geo::facet::{Contour, Facet, Polygon};
use aios_core::prim_geo::revolution::Revolution;
use aios_core::prim_geo::tubing::{PdmsTubing, TubiEdge};
use aios_core::prim_geo::wire::CurveType;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PdmsMesh, VerifiedShape};
use aios_core::tool::db_tool::{db1_hash, GLOBAL_UDA_NAME_MAP};
use aios_core::tool::math_tool;
use anyhow::anyhow;
use approx::{abs_diff_eq, abs_diff_ne};
use arangors_lite::{AqlQuery, Connection, Database};
use async_trait::async_trait;
use bevy::prelude::{dbg, Transform};
use config::{Config, ConfigError, Environment, File};
use dashmap::mapref::one::Ref;
use dashmap::{DashMap, DashSet};
use glam::{DMat4, EulerRot, Mat3, Mat4, quat, Quat, Vec2, Vec3};
use id_tree::{Node, NodeId};
use itertools::Itertools;
use lazy_static::lazy_static;
use nalgebra::{Isometry3, Point3, Quaternion, RealField, UnitQuaternion, Vector3};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::{aabb::Aabb, BoundingVolume};
use parry3d::math::{Isometry, Real, Vector};
use smol_str::SmolStr;
use sqlx::pool::PoolOptions;
use sqlx::{Executor, MySql, MySqlPool, Pool, Row};
use std::boxed::Box;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::default::default;
use std::default::Default;
use std::env;
use std::f32::EPSILON;
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::options::DbOption;
use aios_core::pdms_data::ScomInfo;
use aios_core::prim_geo;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::{error, info};
use nom::combinator::map;
use tokio::sync::{mpsc, RwLock};

use crate::api::attr::*;
use crate::api::children::*;
use crate::api::element::*;
use crate::api::project_mdb::*;
use crate::api::refno_info::{cache_plin_plax, get_ref0_projects, sync_refno_basic_map};
use crate::aql_api::children::*;
use crate::aql_api::foreign_refnos::{query_foreign_refno_aql, query_foreign_refno_fuzzy};
use crate::aql_api::para_value::{query_des_para_value, query_para_from_desi_refno};
use crate::aql_api::plin_attr::{match_jusline_attr, query_plin_attrs, query_pline_value};
use crate::cata::consts::{BANG_WIT_EXTRU_TYPES, JUSLINE_TYPES};
use crate::cata::direction_parse::parse_expr_to_dir;
use crate::cata::query_cata::resolve_desi_comp;
use crate::cata::resolve::CataExprContext;
use crate::cata::resolve_helper::{eval_str_to_f32, parse_str_axis_to_vec3};
use crate::cata::sctn;
use crate::cata::sctn::geo::create_profile_geos;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::*;
use crate::defines::*;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::graph_db::pdms_inst_arango::{query_instance_with_refno_in_arangodb, query_instance_with_refnos_in_arangodb, save_instance_to_graph_db};
use crate::mdb::get_project_mdb;
use crate::tables::{gen_create_project_mdb_json_sql, gen_create_project_mdb_sql};
use crate::{AQL_PDMS_ELES_COLLECTION, AQL_PDMS_INST_COLLECTION};

#[cfg(feature = "opencascade")]
use opencascade::{DsShape, Edge, OCCShape, Wire};
use parry3d::query::{Ray, RayCast};
use crate::data_interface::db_manager::GeoEnum;
use crate::graph_db::pdms_mesh_arango::save_mesh_to_arango_db;

use tokio_stream::wrappers::UnboundedReceiverStream;
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;

pub const TUBI_TOL: f32 = 10.0f32;

lazy_static! {
    pub static ref CATAEXPRCONTEXT_MAP: DashMap<RefU64, CataExprContext> = {
        let mut s = DashMap::new();
        s
    };
}

static PDMS_GNERAL_TYPE_NAMES_MAP: Lazy<HashMap<&'static str, PdmsGenericType>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("EQUI", PdmsGenericType::EQUI);
    m.insert("PIPE", PdmsGenericType::PIPE);
    m.insert("ROOM", PdmsGenericType::ROOM);
    m.insert("STRU", PdmsGenericType::STRU);
    m.insert("PANE", PdmsGenericType::PANE);
    m.insert("CFLOOR", PdmsGenericType::CFLOOR);
    m.insert("FLOOR", PdmsGenericType::FLOOR);
    m.insert("EXTR", PdmsGenericType::EXTR);
    m.insert("REVO", PdmsGenericType::REVO);
    m
});

static GENRIC_NOUN_NAMES: Lazy<Vec<SmolStr>> = Lazy::new(|| {
    vec![
        "EQUI".into(),
        "PIPE".into(),
        "STRU".into(),
        "ROOM".into(),
        "STWALL".into(),
        "FLOOR".into(),
    ]
});

#[derive(Debug)]
pub struct AiosDBManager {
    //不同project的连接池子
    pub project_map: DashMap<String, Pool<MySql>>,

    pub ref0_projects: DashMap<u32, Vec<String>>,

    pub info_pool: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String, //整个项目的路径

    pub db_option: DbOption,

    pub cached_mesh_mgr: Arc<RwLock<CachedMeshesMgr>>,

    pub arango_db: Database,

    cached_world_transforms_map: Arc<DashMap<RefU64, bevy::prelude::Transform>>,

    pub cache_module_numbdbs: BTreeSet<i32>,

    pub mdb_dbnums: BTreeSet<i32>,
}

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    /// 获得最全的数据
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        return if PDMS_ATT_MAP_CACHE.get(&refno).is_some() {
            let k = PDMS_ATT_MAP_CACHE.get(&refno).unwrap();
            Ok(k.value().clone())
        } else {
            let attr = query_attr(refno, self, None).await?;
            PDMS_ATT_MAP_CACHE
                .insert(refno, &attr)
                .expect("PDMS_ATT_MAP_CACHE save error.");
            Ok(attr)
        };
    }

    /// 获得最全的数据
    async fn get_attr_with_uda(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        let mut attr = self.get_attr(refno).await?;
        //暂时把UDA 屏蔽
        // for pool in &self.project_map {
        //     // uda 赋值需要加上元件库
        //     let uda_attr = query_uda_attr(attr.get_type(), &pool).await?;
        //     for (k, v) in uda_attr.map {
        //         attr.entry(k).or_insert(v);
        //     }
        // }
        Ok(attr)
    }

    //todo 修改为图数据库，尽可能避免使用TIDB
    ///获取owner的参考号，从缓存读取
    #[inline]
    fn get_owner(&self, refno: RefU64) -> RefU64 {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.value().get_owner())
            .unwrap_or_default()
    }

    /// t_types 为目标的类型
    async fn query_foreign_refno(&self, refno: RefU64, start_types: &[&[&str]], end_types: &[&str], t_types: &[&str]) -> anyhow::Result<Option<RefU64>> {
        let t_refno = query_foreign_refno_fuzzy(&self.arango_db, refno, start_types, end_types, t_types).await;
        t_refno
    }

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(&self, refno: RefU64, start_types: &[&str], end_types: &[&str], t_types: &[&str]) -> anyhow::Result<Option<RefU64>> {
        let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
        let aql = AqlQuery::new(r#"
            FOR v,e,p in 1..15 OUTBOUND @id pdms_edges
                filter document(v._id) != null
                let xx = (for ver, edge, path in 1..10 OUTBOUND v._id foreign_edges
                           filter document(ver._id) != null
                           //判断是否是叶子节点
                           FILTER LENGTH(@t_types) == 0 and length(for c in 1 INBOUND ver._id foreign_edges
                                return 0 )
                           filter LENGTH(@start_types) == 0 or path.edges[0].foreign_type in @start_types
                           filter LENGTH(@end_types) == 0 or (edge.foreign_type in @end_types)
                           filter LENGTH(@t_types) == 0 or (ver.noun in @t_types)
                           LIMIT 1
                           return ver)
                filter LENGTH(xx) != 0
                LIMIT 1
                return xx[0]._key
                "#)
            .bind_var("id", id)
            .bind_var("start_types", start_types)
            .bind_var("end_types", end_types)
            .bind_var("t_types", t_types)
            ;
        let results: Vec<String> = self.arango_db.aql_query(aql).await?;
        for result in results {
            if let Some(refno) = RefU64::from_url_refno(&result) {
                return Ok(Some(refno));
            }
        }
        Ok(None)
    }


    /// 获得隐含数据的属性
    async fn get_implicit_attr(
        &self,
        refno: RefU64,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<AttrMap> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            if let Some(ref_basic) = self.get_refno_basic(refno) {
                let attr =
                    query_implicit_attr(refno, ref_basic.value(), &project_pool, columns).await?;
                return Ok(attr);
            }
        }
        Ok(AttrMap::default())
    }

    /// 获得OWNER隐含数据的属性
    async fn get_implicit_attrs_by_owner(
        &self,
        owner: RefU64,
        type_name: &str,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<Vec<AttrMap>> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(owner).await {
            let attr =
                query_implicit_attrs_by_owner(owner, type_name, &project_pool, columns).await?;
            return Ok(attr);
        }
        Ok(vec![])
    }

    /// 获取parent的attr数据
    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        todo!()
    }

    /// 获得缓存的refno基本信息, todo 改成使用sql的intersect
    #[inline]
    fn get_refno_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>> {
        if !refno.is_valid() {
            None
        } else {
            CACHED_REFNO_BASIC_MAP.get(&refno)
        }
    }

    /// 获得owner缓存的refno基本信息
    #[inline]
    fn get_owner_ref_basic(&self, refno: RefU64) -> Option<Ref<RefU64, CachedRefBasic>> {
        let owner_ref = self.get_owner(refno);
        self.get_refno_basic(owner_ref)
    }

    /// 获得节点数据
    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            if let Ok(node) = query_ele_node(refno, &project_pool).await {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    ///获得owner
    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        let mut node = None;
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let parent = self.get_owner(refno);
            if parent.is_valid() {
                node = Some(query_ele_node(parent, &project_pool).await?);
            }
        }
        Ok(node)
    }

    ///获得world节点
    async fn get_world(
        &self,
        project: &str,
        mdb_name: &str,
        module: &str,
    ) -> anyhow::Result<EleTreeNode> {
        if let Some(project_pool) = self.project_map.get(project) {
            let v = query_world("SAMPLE", "DESI", project_pool.value()).await?;
            return Ok(v);
        }
        return Err(anyhow!("World not found".to_string()));
    }

    ///获得子节点集合
    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        let mut r = vec![];
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let children = query_children(refno, &project_pool).await?;
            for (refno, _) in children {
                let node = query_ele_node(refno, &project_pool).await?;
                r.push(node);
            }
        }
        Ok(r)
    }

    ///获得children的属性集合
    async fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let children = query_children(refno, &project_pool).await?;
            for child in children {
                let attr = self.get_attr(child.0).await?;
                r.push(attr);
            }
        }
        Ok(r)
    }

    ///获得参考号下的子节点
    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        let mut result = RefU64Vec::default();
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let children = query_children(refno, &project_pool).await?;
            children.into_iter().for_each(|child| {
                result.push(child.0);
            });
        }
        Ok(result)
    }

    ///获得参考号的name
    async fn get_name(&self, refno: RefU64) -> anyhow::Result<SmolStr> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let name = query_name(refno, &project_pool).await?;
            return Ok(SmolStr::new(name));
        }
        Err(anyhow!("Element不存在"))
    }

    /// dbnos为空代表所有db都会去获取
    async fn get_refnos_by_types(
        &self,
        project: &str,
        att_types: &[&str],
        dbnos: &[i32],
    ) -> anyhow::Result<RefU64Vec> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r = query_types_refnos(att_types, project_pool.value(), dbnos).await?;
            return Ok(r);
        }
        Ok(RefU64Vec::default())
    }

    /// 获得当前db的world 参考号
    async fn get_db_world(
        &self,
        project: &str,
        db_no: u32,
    ) -> anyhow::Result<Option<(RefU64, String)>> {
        if let Some(project_pool) = self.project_map.get(project) {
            let r =
                query_id_name_from_dbno_type(db_no as i32, "WORL", project_pool.value()).await?;
            if let Some(mut r) = r {
                return Ok(Some(r.remove(0)));
            }
        }
        return Ok(None);
    }

    /// 获得参考号的祖先参考号
    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64> {
        let mut result = vec![refno]; //需要包含自己
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            cur_refno = b.owner;
            result.push(cur_refno);
        }
        result
    }

    ///获得不包含world的父节点路径
    fn get_ancestors_refnos_without_world(&self, refno: RefU64) -> Vec<RefU64> {
        let mut result = vec![refno]; //需要包含自己
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            if b.get_type() == "WORL" {
                break;
            }
            cur_refno = b.owner;
            result.push(cur_refno);
        }
        result
    }

    ///查询哪些有负实体的参考号
    async fn query_refnos_has_neg_geom(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new("\
        let negatives = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
                    PRUNE v.noun in @negative_nouns
                    filter v.noun in @negative_nouns
                    return p.vertices[-2]._key)
        return UNIQUE(negatives)
        "
        ).bind_var("key", refno_url)
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        let refno_strs = self.arango_db.aql_query::<Vec<String>>(aql).await?;
        let refnos = refno_strs.iter().flatten().map(|x| RefU64::from_url_refno(x).unwrap()).collect();
        Ok(refnos)
    }

    ///返回有负实体和正实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_pos_neg_map(&self, refno: RefU64) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new(r#"
                FOR v,e,p in 0..15 INBOUND @key pdms_edges
                PRUNE v.noun in @neg_nouns
                OPTIONS { "order": "bfs"}
                filter v.noun in @neg_nouns
                let parent = p.vertices[-2]
                let children = ( for cc in 1 INBOUND parent._id pdms_edges return cc )
                return [
                     parent._key,
                     (
                        let pos_vec = (for c in children filter c.noun in @pos_nouns return c._key)
                        let parent_is_pos = parent.noun in @pos_nouns
                        return parent_is_pos ? PUSH(pos_vec, parent._key) : pos_vec
                     )[0],
                    (for c in children filter c.noun in @neg_nouns  return c._key)
                ]
        "#).bind_var("key", refno_url)
            .bind_var("neg_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
            .bind_var("pos_nouns", GENRAL_POS_NOUN_NAMES.to_vec())
            ;
        let result: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> = self.arango_db
            .aql_query::<RefnoHasNegPosInfoTuple>(aql).await?.into_iter().map(|x| (x.0, (x.1, x.2))).collect();

        return Ok(result);
    }

    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new(r#"
            let refnos = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
                        PRUNE v.noun in @geo_nouns
                        OPTIONS { "order": "bfs"}
                        filter v.noun in @geo_nouns
                        filter v != null
                        return LENGTH(p.vertices) > 1 ? p.vertices[-2]._key : p.vertices[0]._key
                    )
            return UNIQUE(refnos)
        "#
        ).bind_var("key", refno_url)
            .bind_var("geo_nouns", TOTAL_GEO_NOUN_NAMES.to_vec());
        let refno_strs = self.arango_db.aql_query::<Vec<String>>(aql).await?;
        let refnos = refno_strs.iter().flatten().map(|x| RefU64::from_url_refno(x).unwrap()).collect();
        Ok(refnos)
    }

    ///返回有负实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_neg_map(&self, refno: RefU64) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new(r#"
            FOR v,e,p in 0..15 INBOUND @key pdms_edges
                PRUNE v.noun in @negative_nouns
                OPTIONS { "order": "bfs"}
                filter v.noun in @negative_nouns
                collect parent = p.vertices[-2] into grouped
                return [
                     parent._key,
                     (for v in grouped[*].v filter v.noun in @negative_nouns  return v._key),
                ]
        "#).bind_var("key", refno_url)
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        let result: HashMap<RefU64, Vec<RefU64>> = self.arango_db
            .aql_query::<RefnoHasNegInfoTuple>(aql).await?.into_iter().map(|x| (x.0, x.1)).collect();

        return Ok(result);
    }

    /// 获得参考号的祖先属性
    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap> {
        let mut cur_refno = refno;
        let mut r = vec![];
        if let Some((_, pool)) = self.get_project_pool_by_refno(refno).await {
            while let Ok(attr) = self.get_implicit_attr(cur_refno, None).await {
                //后面是不是要缓存这个层级结构
                if let Ok(Some(owner)) = query_owner_from_id(cur_refno, &pool).await {
                    r.push(attr);
                    cur_refno = owner;
                } else {
                    break;
                }
            }
        }
        r
    }

    /// 获得参考号的祖先节点
    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>> {
        let mut cur_refno = refno;
        let mut ancestors = VecDeque::new();
        while let Some(node) = self.get_ele_node(cur_refno).await? {
            cur_refno = node.owner;
            ancestors.push_front(node);
        }
        Ok(ancestors)
    }

    ///获得世界坐标系, 需要缓存数据，如果已经存在数据了，直接获取
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>> {
        let mut ancestors = VecDeque::new();
        let mut rotation = Quat::IDENTITY;
        let mut translation = Vec3::ZERO;
        let mut cur_refno = refno;
        while let Some(ref_basic) = self.get_refno_basic(cur_refno) {
            //后面是不是要缓存这个层级结构
            if self.cached_world_transforms_map.contains_key(&cur_refno) {
                self.cached_world_transforms_map.get(&cur_refno).map(|x| {
                    rotation = x.rotation;
                    translation = x.translation;
                });
                break;
            }
            let tmp_owner = ref_basic.get_owner();
            ancestors.push_front((cur_refno, ref_basic));
            cur_refno = tmp_owner;
        }
        //需要判断owner 下是不是有spine，如wall，顺时针逆时针会影响plin的方向
        for (refno, ref_basic) in ancestors {
            let mut jusl_vec = Vec3::new(0.0, 0.0, 0.0);
            let att = self.get_attr(refno).await?;
            let mut pos = att.get_position().unwrap_or_default();
            let mut quat = Quat::IDENTITY;
            let type_name = att.get_type();
            if let Some(jusl) = att.get_str("JUSL") {
                if let Some(exps) = query_pline_value(refno, jusl, &self.arango_db).await? {
                    let x = self.resolve_expression_to_f32(&exps[0], refno).await?;
                    let y = self.resolve_expression_to_f32(&exps[1], refno).await?;
                    jusl_vec = Vec3::new(x, y, 0.0);
                }
            }
            //先获得下面的PLOO
            let owner = ref_basic.owner;
            if let Some(owner_basic) = self.get_refno_basic(owner) {
                //特殊情况的一些处理
                match owner_basic.get_type() {
                    "FLOOR" => {
                        let sjus = att.get_str("JUSL").unwrap_or("unset");
                        let height = self
                            .get_attr(refno)
                            .await?
                            .get_f32("HEIG")
                            .unwrap_or_default();
                        let mut off_z = if sjus == "UTOP" || sjus == "DTOP" {
                            -height
                        } else if sjus == "UCEN" || sjus == "DCEN" {
                            -height / 2.0
                        } else {
                            0.0
                        };
                        pos.z += off_z;
                    }
                    "PLDATU" => {
                        let grand = owner_basic.owner;
                        let grand_att = self.get_attr(grand).await?;
                        let zdis = (att.get_f32("ZDIS").unwrap_or_default() * Vec3::Z);
                        let pkdi = att.get_f32("PKDI").unwrap_or_default();
                        //获取比例的位置
                        pos = pos + zdis;
                    }
                    _ => {}
                }
            }

            let mut quat_v = att.get_rotation();
            let mut need_bangle = false;
            if quat_v.is_some() {
                quat = quat_v.unwrap();
            } else {
                let extru_dir: Vec3 = if let Some(poss) = att.get_poss() &&
                    let Some(pose) = att.get_pose()
                {
                    need_bangle = true;
                    (pose - poss).normalize()
                } else {
                    Vec3::Z
                };
                let d = extru_dir.dot(Vec3::Z).abs();
                let mut ref_axis = if abs_diff_eq!(1.0, d) {
                    Vec3::Y
                } else {
                    Vec3::Z
                };

                let p_axis = ref_axis.cross(extru_dir).normalize();
                let y_axis = extru_dir.cross(p_axis).normalize();
                quat = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(&[
                    p_axis.to_array(),
                    y_axis.to_array(),
                    extru_dir.to_array(),
                ]));
            }

            if let Some(bangle) = att.get_f32("BANG") {
                need_bangle |= type_name == "PFIT";
                if need_bangle {
                    quat = quat * Quat::from_rotation_z(bangle.to_radians());
                }
            }
            //弧墙下方没有fitt
            if let Some(pos_line) = att.get_str("POSL") {
                // dbg!(pos_line);
                //plin里的位置偏移
                let mut plin_pos = Vec3::new(0.0, 0.0, 0.0);
                let mut pline_plax = -Vec3::X;

                let delta_vec = att.get_vec3("DELP").unwrap_or_default() /*+ plin_pos*/;
                let zdis = (att.get_f32("ZDIS").unwrap_or_default() * Vec3::Z);
                let bangle = att.get_f32("BANG").unwrap_or_default();
                let pos_line =
                    query_pline_value(att.get_owner().unwrap(), pos_line, &self.arango_db)
                        .await?;
                if let Some(pos_line) = pos_line {
                    // dbg!(&pos_line);
                    let owner = ref_basic.owner;
                    //对于fitting这种，需要取parent的值
                    let x = self.resolve_expression_to_f32(&pos_line[0], owner).await?;
                    let y = self.resolve_expression_to_f32(&pos_line[1], owner).await?;
                    plin_pos = Vec3::new(x, y, 0.0);
                }
                if let Some(v) = CACHED_PLIN_MAP.get(&refno) {
                    pline_plax = parse_expr_to_dir(&v.value());
                }
                let bangle_rot = Quat::from_rotation_z(bangle.to_radians());
                let y_axis = Vec3::Z;
                let z_axis = pline_plax;
                let x_axis = y_axis.cross(z_axis).normalize();
                let quat = Quat::from_mat3(&glam::f32::Mat3::from_cols_array_2d(&[
                    x_axis.to_array(),
                    y_axis.to_array(),
                    z_axis.to_array(),
                ]));
                translation = translation
                    + rotation * (pos + zdis + plin_pos - jusl_vec)
                    + rotation * quat * bangle_rot * delta_vec;
                rotation = rotation * quat * bangle_rot;
            } else {
                translation = translation + rotation * pos;
                rotation = rotation * quat;
            }

            self.cached_world_transforms_map
                .entry(refno)
                .or_insert(Transform {
                    rotation,
                    translation,
                    scale: Vec3::ONE,
                });
        }
        //将rotation 还原为角度
        // let angles = rotation.to_euler(EulerRot::XYZ);
        if self.db_option.debug_print_world_transform {
            let rot_mat = Mat3::from_quat(rotation);
            let ori_str = math_tool::to_pdms_ori_str(&rot_mat);
            println!("{} : {:?}", refno.to_refno_str(), (translation, ori_str));
        }
        Ok(Some(Transform {
            rotation,
            translation,
            scale: Vec3::ONE,
        }))
    }

    ///获得子节点集合的属性
    async fn get_travel_children_attrs(&self, refno: RefU64, nouns: &[&str]) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        let children = query_deep_children_refnos_fuzzy(self.get_arangodb().await?, refno, nouns).await?;
        // dbg!(children.len());
        for child in children {
            let attr = self.get_attr(child).await?;
            r.push(attr);
        }
        Ok(r)
    }


    ///获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(&self, refno: RefU64, distance: f32) -> anyhow::Result<Vec<RefU64>>{
        let rtree = self.compute_aabb_tree().await?;

        let instances = query_instance_with_refnos_in_arangodb(vec![refno],
                                                               &self.arango_db).await?.unwrap_or_default();
        if instances.is_empty() { return Ok(vec![]); }
        let pos = instances[0].world_transform.translation;
        let target_refnos = rtree.query_within_distance(pos, distance)
            .collect();;

        Ok(target_refnos)
    }

}

impl AiosDBManager {
    /// 从默认配置文件初始化
    pub async fn init_form_config() -> anyhow::Result<Self> {
        let db_option = Self::get_db_option()?;
        let mut mgr = Self::init(&db_option).await?;
        dbg!("正在初始化uda");
        mgr.init_uda_map().await?;
        mgr.init_mdb(
            &db_option.project_name,
            &db_option.mdb_name,
            &db_option.module,
        ).await?;
        Ok(mgr)
    }

    ///重新连接arangodb
    #[inline]
    pub async fn reconnect_arangodb(&mut self) {
        // self.arango_db = get_arangodb_conn_from_db_option(&self.db_option).await.unwrap();
    }

    pub async fn compute_aabb_tree(&self) -> anyhow::Result<AccelerationTree> {
        //测试分页查询
        let mut rstar_objs = vec![];
        let mut offset = 0;
        loop {
            //需要排除负实体
            let aql = AqlQuery::new(r#"
            FOR doc IN pdms_instances
                SORT doc._key
                LIMIT @offset, @batch_size
                filter doc.aabb != null
                filter LENGTH(doc.geo_insts) > 1 or (LENGTH(doc.geo_insts) == 1 and !doc.geo_insts[0].is_neg)
                RETURN [
                    doc._key,
                    doc.aabb,
                ]
        "#)
                .bind_var("offset", offset)
                .bind_var("batch_size", 1000);
            offset += 1000;
            // let mut query_ok = false;
            if let Ok(refno_aabbs) = self.arango_db.aql_query::<(String, Aabb)>(aql).await {
                if refno_aabbs.is_empty() {
                    break;
                }
                for (refno_str, aabb) in refno_aabbs {
                    if aabb.extents().magnitude().is_finite() {
                        let refno = RefU64::from_url_refno(&refno_str).unwrap();
                        rstar_objs.push(RStarBoundingBox::from_aabb(&aabb, refno));
                    }
                }
            } else {
                break;
            }
        }

        dbg!(offset);

        let rtree = AccelerationTree::load(rstar_objs);
        dbg!(rtree.size());

        Ok(rtree)
    }

    async fn calculate_room(&self, inst: &EleGeosInfo, rtree: &AccelerationTree) -> anyhow::Result<Vec<RefU64>> {
        let mut withing_room_refnos = vec![];
        let room_refno = inst.refno;
        if let Some(room_abb) = inst.aabb {
            // dbg!(&room_abb);
            withing_room_refnos = rtree
                .locate_intersecting_bounds(&room_abb)
                .collect::<Vec<_>>();
            let hashes = inst.geo_insts.iter().map(|x| x.geo_hash).collect::<Vec<_>>();
            let room_mesh_mgr = query_pdms_mesh_aql(hashes.clone(), &self.arango_db).await.unwrap_or_default();
            for hash in hashes {
                if let Some(room_mesh) = room_mesh_mgr.get_mesh(hash) {
                    let t = inst.get_geo_world_transform(&inst.geo_insts[0]);
                    // dbg!(&t);
                    let collider_mesh = room_mesh.get_tri_mesh(t.compute_matrix());
                    // let local_aabb = collider_mesh.local_aabb();
                    // dbg!(collider_mesh.local_aabb());
                    let mut outer_refnos = vec![];
                    for refno in &withing_room_refnos {
                        let world_trans = self.get_world_transform(*refno).await?.unwrap_or_default();
                        let world_point: parry3d::math::Point<f32> = world_trans.translation.into();

                        //检查目标的坐标点不在它自身包围盒的情况，这种就需要用相交的算法去计算

                        //check 是否包含在房间内
                        let contain_point = match collider_mesh.cast_local_ray_and_get_normal(
                            &Ray::new(world_point, Vector::new(0.0, 0.0, 1.0)),
                            100000.0,
                            false,
                        ) {
                            Some(intersection) => {
                                collider_mesh.is_backface(intersection.feature)
                            }
                            None => false,
                        };
                        // dbg!(contain_point);
                        // dbg!(outer_refnos.len());
                        if !contain_point {
                            outer_refnos.push(*refno);
                        }
                        //如果是风管，就需要这么去检测是否发生碰撞
                        //后续需要用包围盒再去判断一次
                        // collider_mesh.intersection_with_aabb();
                    }

                    withing_room_refnos.retain(|refno| {
                        !outer_refnos.contains(refno) && *refno != room_refno
                    });

                    // dbg!(&withing_room_refnos);
                }
            }
            //再次过滤room，通过判断位置是否在room的mesh里来判断
        }

        return Ok(withing_room_refnos);
    }

    ///计算所有房间包含的其他参考号
    pub async fn calculate_rooms(&self) -> anyhow::Result<()> {
        let rtree = self.compute_aabb_tree().await?;

        //指定哪个site下有房间节点
        let Some(room_root_refnos) = &self.db_option.room_root_refnos else {
            return Ok(());
        };

        let mut room_hashmap = HashMap::new();
        for r in room_root_refnos {
            let Ok(room_root_refno) = RefU64::from_refno_str(r) else{
                continue;
            };
            let panes = query_deep_children_refnos_fuzzy(&self.arango_db, room_root_refno, &["PANE"]).await?;
            // dbg!(&panes);
            let instances = query_instance_with_refnos_in_arangodb(panes,
                                                                   &self.arango_db).await?.unwrap_or_default();
            // dbg!(&instances);
            let mut final_within_room_refnos = vec![];
            for inst in &instances {
                let r = self.calculate_room(inst, &rtree).await?;
                final_within_room_refnos.extend_from_slice(&r);
            }

            // dbg!(&final_within_room_refnos);
            // final_within_room_refnos.remove
            room_hashmap.insert(room_root_refno, final_within_room_refnos);
        }

        self.save_room_info_to_arangodb(room_hashmap).await?;


        Ok(())
    }


    ///快速获得table名称
    pub fn get_table_name(&self, refno: RefU64) -> String {
        CACHED_REFNO_BASIC_MAP
            .get(&refno)
            .map(|x| x.get_table_name().to_string())
            .unwrap_or("UNSET".to_string())
    }


    ///获得db option
    #[inline]
    pub fn get_db_option() -> anyhow::Result<DbOption> {
        use config::{Config, ConfigError, Environment, File};
        let s = Config::builder()
            .add_source(File::with_name("DbOption"))
            .build()?;
        s.try_deserialize::<DbOption>()
            .map_err(|x| anyhow!(x.to_string()))
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn get_default_conn_str(d: &DbOption) -> String {
        let user = d.user.as_str();
        let pwd = d.password.as_str();
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }

    ///获得默认的连接字符串
    #[inline]
    pub fn default_conn_str(&self) -> String {
        let d = &self.db_option;
        let user = d.user.as_str();
        let pwd = d.password.as_str();
        let ip = d.ip.as_str();
        let port = d.port.as_str();
        format!("mysql://{user}:{pwd}@{ip}:{port}")
    }
    /// 获得pool
    #[inline]
    pub async fn get_db_pool(connection_str: &str, project: &str) -> anyhow::Result<Pool<MySql>> {
        let url = &format!("{connection_str}/{}", project);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow!(x.to_string()) })
    }

    #[inline]
    pub async fn get_global_pool(&self) -> anyhow::Result<Pool<MySql>> {
        let connection_str = self.default_conn_str();
        let url = &format!("{connection_str}/{}", GLOBAL_DATABASE);
        PoolOptions::new()
            .max_connections(500)
            .acquire_timeout(Duration::from_secs(10 * 60))
            .connect(url)
            .await
            .map_err({ |x| anyhow!(x.to_string()) })
    }

    #[inline]
    pub async fn get_arangodb(&self) -> anyhow::Result<&Database> {
        Ok(&self.arango_db)
        // let conn = Connection::establish_jwt(
        //     &self.db_option.arangodb_url,
        //     &self.db_option.arangodb_user,
        //     &self.db_option.arangodb_password,
        // )
        //     .await?;

        // Ok(conn.db(&self.db_option.arangodb_database).await?)
    }

    ///获得默认的pool
    #[inline]
    pub async fn get_default_pool(conn_str: &str) -> anyhow::Result<Pool<MySql>> {
        MySqlPool::connect(conn_str)
            .await
            .map_err(|x| anyhow!(x.to_string()))
    }


    /// 初始化mdb
    pub async fn init_mdb(&mut self, project: &str, mdb: &str, module: &str) -> anyhow::Result<()> {
        let project_pool = self.get_project_pool(project).ok_or(anyhow!("Unknown project pool"))?;
        info!("正在初始化mdb: {mdb}");
        let mut conn = project_pool.acquire().await?;
        let time = Instant::now();
        let need_sync_refno_basic = self.db_option.need_sync_refno_basic;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    sync_refno_basic_map(kv.value() /* &self.mdb_dbnums*/).await.unwrap();
                }
            }
        }
        // 将对应mdb module 下所有的 numbdb 存下来
        //创建table, 如果已经存在，可以忽略
        if self.db_option.reset_mdb_project.is_some() && self.db_option.reset_mdb_project.unwrap() {
            let create_sql = gen_create_project_mdb_sql();
            let _ = conn.execute(create_sql.as_str()).await;
            println!("正在插入mdb数据");
            let _ = self.insert_project_mdb(&project_pool, &self.info_pool).await;
        }
        cache_mdb_site_map(mdb, module, &project_pool).await;
        self.mdb_dbnums = query_mdb_all_dbnums(mdb, &project_pool).await?;
        if need_sync_refno_basic {
            for project in &self.db_option.included_projects {
                if let Some(kv) = self.project_map.get(project) {
                    let dbnums = self.mdb_dbnums.iter().cloned().collect::<Vec<_>>();
                    if let Ok(m) = cache_plin_plax(
                        kv.value(),
                        &dbnums,
                        &self.arango_db,
                    ).await {
                        for (k, v) in m {
                            CACHED_PLIN_MAP.insert(k, &v.into());
                        }
                    }
                }
            }
        }
        if need_sync_refno_basic {
            CACHED_REFNO_BASIC_MAP.save_to_file(stringify!(CACHED_REFNO_BASIC_MAP))?;
            CACHED_PLIN_MAP.save_to_file(stringify!(CACHED_PLIN_MAP))?;
        } else {
            CACHED_REFNO_BASIC_MAP.load_map_from_file(stringify!(CACHED_REFNO_BASIC_MAP))?;
            CACHED_PLIN_MAP.load_map_from_file(stringify!(CACHED_PLIN_MAP))?;
        }

        // 将 mdb对应的 module 下的所有 numbdb保存下来
        let results = cache_mdb_module_numbdbs(mdb, module, &project_pool).await?;
        for r in results {
            self.cache_module_numbdbs.insert(r);
        }
        Ok(())
    }

    ///初始化db manager
    pub async fn init(db_option: &DbOption) -> anyhow::Result<Self> {
        let dir = db_option.project_path.to_string();
        let mut project_map = DashMap::new();
        let db_option = Self::get_db_option()?;
        let default_conn = AiosDBManager::get_default_conn_str(&db_option);
        for project in &db_option.included_projects {
            let project_pool = AiosDBManager::get_db_pool(&default_conn, project).await;
            match project_pool {
                Ok(pool) => {
                    println!("数据库连接成功 {project}");
                    project_map.entry(project.clone()).or_insert(pool.clone());
                }
                Err(_) => {
                    println!("项目: {} 连接创建失败", project);
                }
            }
            println!("正在创建数据库连接 {project}");
        }
        let info_conn = AiosDBManager::get_db_pool(
            &default_conn,
            &format!(
                "{}_{}",
                PDMS_INFO_DB,
                &db_option.project_name.to_uppercase()
            ),
        )
            .await?;
        let ref0_projects = get_ref0_projects(&info_conn).await?;
        // dbg!(&ref0_projects);
        let projects = db_option.included_projects.clone();
        println!("正在创建图数据库连接");
        let database = get_arangodb_conn_from_db_option(&db_option).await.unwrap();
        Ok(Self {
            project_map,
            ref0_projects,
            info_pool: info_conn,
            projects,
            needed_parse_files: None,
            project_path: dir,
            db_option,
            cached_mesh_mgr: Arc::new(Default::default()),
            arango_db: database,
            cached_world_transforms_map: Arc::new(Default::default()),
            cache_module_numbdbs: Default::default(),
            mdb_dbnums: Default::default(),
        })
    }

    /// 初始化 uda_map
    pub async fn init_uda_map(&self) -> anyhow::Result<()> {
        for pool in &self.project_map {
            if let Ok(uda_map) = query_uda_ukey_udna_all(pool.value()).await {
                for (ukey, udna) in uda_map {
                    let udna = format!(":{}", udna);
                    GLOBAL_UDA_NAME_MAP.entry(ukey).or_insert(udna);
                }
            }
        }
        Ok(())
    }

    /// 根据project获取连接池
    #[inline]
    pub fn get_project_pool(&self, project: &str) -> Option<Pool<MySql>> {
        self.project_map.get(project).map(|x| x.value().clone())
    }

    ///获得project 的db
    #[inline]
    pub async fn get_project_pool_by_refno(&self, refno: RefU64) -> Option<(String, Pool<MySql>)> {
        if let Some(projects) = self.ref0_projects.get(&refno.get_0()) {
            ///只有一个的时候
            if projects.len() == 1 {
                let project = projects.value().iter().next().as_ref().unwrap().clone();
                if let Some(project_pool) = self.project_map.get(project) {
                    return Some((project.clone(), project_pool.value().clone()));
                }
            } else {
                //check if exist in pdms_elements
                // for project in projects.value() {
                for project in &self.db_option.included_projects {
                    if let Some(pool) = self.get_project_pool(project) {
                        if check_exist_refno(refno, &pool, &self.mdb_dbnums).await.ok()? {
                            return Some((project.clone(), pool.clone()));
                        }
                    }
                }
            }
        }
        (None)
    }

    /// 获得dbnum 对应的 dbtype 和 world refno
    pub async fn query_quick_info_by_dbno(&self, db_refno: RefU64, db_num: i32, pool: &Pool<MySql>) -> anyhow::Result<Option<DbQuickInfo>> {
        let mut sql = String::new();
        //todo 参考号相同的情况，导致refno获取出来的不准
        sql.push_str(&format!(r#"SELECT DB_TYPE, PROJECT  FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {}"#, db_num));
        let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
        for v in result {
            if let project = v.get::<String, _>(1) {
                dbg!(&project);
                let Some(project_pool) = self.get_project_pool(&project) else { continue };
                if let Some(world_refno) = query_world_refno_by_dbno(db_num, &project_pool).await? {
                    let db_type = v.get::<String, _>(0);
                    return Ok(Some(DbQuickInfo {
                        refno: db_refno,
                        world_refno,
                        db_num,
                        db_type,
                        project,
                        order_number: 0,
                    }));
                }
            }
        }
        Ok(None)
    }

    /// 获得mdb下所有的world的参考号
    pub async fn query_mdb_quickinfo_map(
        &self,
        project_pool: &Pool<MySql>,
        info_pool: &Pool<MySql>,
    ) -> anyhow::Result<MdbQuickInfoMap> {
        let mut mdb_map = HashMap::new();
        let mdbs = query_types_refnos(&vec!["MDB"], project_pool, &[]).await?;
        for mdb_refno in mdbs {
            let Ok(mdb_attr) = query_attr(mdb_refno, self, None).await else {
                continue;
            };
            let Ok(mdb_name) = query_name(mdb_refno, &project_pool).await else {
                continue;
            };
            // if &mdb_name != "/ALL" { continue; }
            if let Some(dbs) = mdb_attr.get_refu64_vec("CURD") {
                let mut map = HashMap::new();
                for (i, db_refno) in dbs.iter().enumerate() {
                    if let Ok(att) = self.get_implicit_attr(*db_refno, Some(vec!["NUMBDB"])).await {
                        let db_num = att.get_i32("NUMBDB").unwrap_or_default();
                        if let Ok(Some(mut quick_info)) = self.query_quick_info_by_dbno(*db_refno, db_num, info_pool).await {
                            quick_info.order_number = i as _;
                            map.entry(quick_info.db_type.clone())
                                .or_insert_with(Vec::new).push(quick_info);
                        }
                    }
                }
                mdb_map.entry(mdb_name).or_insert(map);
                dbg!("ok");
            }
        }
        Ok(mdb_map)
    }

    /// save project mdb info to database
    pub async fn insert_project_mdb(
        &self,
        project_pool: &Pool<MySql>,
        info_pool: &Pool<MySql>,
    ) -> anyhow::Result<()> {
        let project_mdb_map = self.query_mdb_quickinfo_map(project_pool, info_pool).await?;
        if !project_mdb_map.is_empty() {
            let sql = gen_insert_project_mdb_sql(&project_mdb_map);
            let mut conn = project_pool.acquire().await?;
            let result = conn.execute(sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(sql.as_str());
                }
            }
        }
        Ok(())
    }

    ///获得参考号对应的一般类型
    pub fn get_generic_type(&self, refno: RefU64) -> PdmsGenericType {
        let mut cur_refno = refno;
        while let Some(b) = CACHED_REFNO_BASIC_MAP.get(&cur_refno) {
            let type_name = b.get_type();
            if PDMS_GNERAL_TYPE_NAMES_MAP.contains_key(&type_name) {
                return *PDMS_GNERAL_TYPE_NAMES_MAP.get(type_name).unwrap();
            }
            cur_refno = b.owner;
        }
        PdmsGenericType::UNKOWN
    }

    /// 通用的解析表达式的方法, 解析desi参考号下的 表达式值
    /// 如果 desi_refno 为空，代表design的数据不需要参与计算
    pub async fn resolve_expression_to_f32(
        &self,
        expr: &str,
        desi_refno: RefU64,
    ) -> anyhow::Result<f32> {
        let cata_context = if let Some(cata) = CATAEXPRCONTEXT_MAP.get(&desi_refno) {
            cata.value().clone()
        } else {
            let cata = CataExprContext::create(desi_refno, &self.arango_db)
                .await
                .unwrap_or_default()
                .unwrap_or_default();
            CATAEXPRCONTEXT_MAP.insert(desi_refno, cata.clone());
            cata
        };
        let context = cata_context.build(self, desi_refno).await;
        eval_str_to_f32(expr, &context, Some(self))
    }


    // 需要区分project，不同project的mesh，是不同的
    pub async fn cache_geos_data(
        mut mgr: Arc<AiosDBManager>,
        db_option: DbOption,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }

    /// 获取缓存好的site
    pub async fn get_cached_site_nodes(
        &self,
        world_refno: RefU64,
    ) -> anyhow::Result<Option<Vec<PdmsElement>>> {
        if let Some(k) = CACHED_MDB_SITE_MAP.read().await.get(&world_refno) {
            return Ok(Some(k.0.clone()));
        }
        Ok(None)
    }
}

#[tokio::test]
async fn test_get_attr() -> anyhow::Result<()> {
    // let mut mgr = AiosDBManager::init_form_config().await?;
    // let refno: RefU64 = RefI32Tuple((23584, 8)).into();
    // let v = mgr.get_attr(refno).await?;
    // println!("v={:?}", v.to_string_hashmap());

    // mgr.cache_geos_data("Sample", "SAMPLE").await?;

    Ok(())
}

#[test]
fn test_compute_distance() {
    let x = Vec3::new(3460.0, 9230.0, 5013.23);
    let y = Vec3::new(3460.0, 9230.0, 5081.305);
    let distance = x.distance(y);
    dbg!(&distance);
}
