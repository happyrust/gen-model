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
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::db_tool::{db1_hash, GLOBAL_UDA_NAME_MAP};
use aios_core::tool::math_tool;
use anyhow::anyhow;
use approx::{abs_diff_eq, abs_diff_ne};
use bb8_arangodb::arangors::{AqlQuery, Database};
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
use std::fmt::{Debug, Formatter};
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
use crate::aql_api::foreign_refnos::{query_foreign_refno_aql, query_foreign_refnos_fuzzy};
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
use crate::graph_db::pdms_arango::{ArDatabase, ArPool, connect_arangodb, save_arangodb_doc};
use crate::graph_db::pdms_inst_arango::*;
use crate::mdb::get_project_mdb;
use crate::tables::{gen_create_project_mdb_json_sql, gen_create_project_mdb_sql};
// use bb8_arangodb::bb8::Pool as ArangoPool;
use bb8_arangodb::{ArangoConnectionManager, AuthenticationMethod};

#[cfg(feature = "opencascade")]
use opencascade::{DsShape, Edge, OCCShape, Wire};
use parry3d::query::{Ray, RayCast};
use crate::data_interface::db_manager::GeoEnum;
use crate::graph_db::pdms_mesh_arango::save_mesh_to_arango_db;

use tokio_stream::wrappers::UnboundedReceiverStream;
use crate::aql_api::pdms_mesh::query_pdms_mesh_aql;
use crate::consts::{AQL_PDMS_ELES_COLLECTION};

lazy_static! {
    pub static ref CATAEXPRCONTEXT_MAP: DashMap<RefU64, CataExprContext> = {
        let mut s = DashMap::new();
        s
    };
}

// #[derive(Debug)]
pub struct AiosDBManager {
    //不同project的连接池子
    pub project_map: DashMap<String, Pool<MySql>>,

    pub ref0_projects: DashMap<u32, Vec<String>>,

    pub info_pool: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String, //整个项目的路径

    pub db_option: DbOption,

    pub cached_mesh_mgr: Arc<RwLock<MeshesData>>,

    pub arango_pool: ArPool,

    pub cached_world_transforms_map: Arc<DashMap<RefU64, bevy::prelude::Transform>>,

    pub cache_module_numbdbs: BTreeSet<i32>,

    pub mdb_dbnums: BTreeSet<i32>,

    pub rtree: Option<AccelerationTree>,
}

impl Debug for AiosDBManager {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "db manager project is {}", &self.project_path)
    }
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
    #[inline]
    async fn query_foreign_refnos(&self, refnos: &[RefU64], start_types: &[&[&str]], end_types: &[&str], t_types: &[&str], depth: u32) -> anyhow::Result<Vec<RefU64>> {
        let t_refnos = query_foreign_refnos_fuzzy(&self.get_arango_db().await?, refnos, start_types, end_types, t_types, depth).await;
        t_refnos
    }

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(&self, refno: RefU64, start_types: &[&str], end_types: &[&str], t_types: &[&str]) -> anyhow::Result<Option<RefU64>> {
        let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
        let aql = AqlQuery::builder().query(r#"
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
            .build()
            ;
        let results: Vec<String> = self.get_arango_db().await?.aql_query(aql).await?;
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
        let aql = AqlQuery::builder().query("\
        let negatives = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
                    PRUNE v.noun in @negative_nouns
                    filter v.noun in @negative_nouns
                    return p.vertices[-2]._key)
        return UNIQUE(negatives)
        "
        ).bind_var("key", refno_url)
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
            .build();
        let refno_strs = self.get_arango_db().await?.aql_query::<Vec<String>>(aql).await?;
        let refnos = refno_strs.iter().flatten().map(|x| RefU64::from_url_refno(x).unwrap()).collect();
        Ok(refnos)
    }

    ///返回有负实体和正实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_pos_neg_map(&self, refnos: &[RefU64]) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>> {
        let refno_urls = refnos.iter()
            .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
            .collect::<Vec<_>>();
        let aql = AqlQuery::builder().query(r#"
            for key in @keys
                FOR v,e,p in 0..15 INBOUND key pdms_edges
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
        "#).bind_var("keys", refno_urls)
            .bind_var("neg_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
            .bind_var("pos_nouns", GENRAL_POS_NOUN_NAMES.to_vec())
            .build()
            ;
        let result: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> = self.get_arango_db().await?
            .aql_query::<RefnoHasNegPosInfoTuple>(aql).await?.into_iter().map(|x| (x.0, (x.1, x.2))).collect();

        return Ok(result);
    }

    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::builder().query(r#"
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
            .bind_var("geo_nouns", TOTAL_GEO_NOUN_NAMES.to_vec()).build();
        let refno_strs = self.get_arango_db().await?.aql_query::<Vec<String>>(aql).await?;
        let refnos = refno_strs.iter().flatten().map(|x| RefU64::from_url_refno(x).unwrap()).collect();
        Ok(refnos)
    }

    async fn query_parent_refnos_has_neg_geos(&self, refnos: &[RefU64]) -> anyhow::Result<Vec<RefU64>> {
        let refno_urls = refnos.iter()
            .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
            .collect::<Vec<_>>();
        let aql = AqlQuery::builder().query(r#"
            for key in @keys
                FOR v,e,p in 0..15 INBOUND key pdms_edges
                    filter v.noun in @neg_geo_nouns
                    filter LENGTH(p.vertices) >= 2
                    let parent = p.vertices[-2]
                    return distinct parent._key
        "#
        ).bind_var("keys", refno_urls)
            .bind_var("neg_geo_nouns", GENRAL_NEG_NOUN_NAMES.to_vec()).build();
        let refno_strs = self.get_arango_db().await?.aql_query::<String>(aql).await?;
        let refnos = refno_strs.iter().map(|x| RefU64::from_url_refno(x).unwrap()).collect();
        Ok(refnos)
    }

    ///返回有负实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_neg_map(&self, refno: RefU64) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::builder().query(r#"
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
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
            .build();
        let result: HashMap<RefU64, Vec<RefU64>> = self.get_arango_db().await?
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
        let database = self.get_arango_db().await?;
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
                if let Some(exps) = query_pline_value(refno, jusl, &database).await? {
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
                if extru_dir.is_nan() {
                    return Ok(None);
                }
                let d = extru_dir.dot(Vec3::Z).abs();
                let mut ref_axis = if abs_diff_eq!(1.0, d) {
                    Vec3::Y
                } else {
                    Vec3::Z
                };

                let p_axis = ref_axis.cross(extru_dir).normalize();
                let y_axis = extru_dir.cross(p_axis).normalize();
                quat = Quat::from_mat3(&Mat3::from_cols_array_2d(&[
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
                    query_pline_value(att.get_owner().unwrap(), pos_line, &database)
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
                    pline_plax = parse_expr_to_dir(&v.value()).unwrap_or(Vec3::Z);
                }
                let bangle_rot = Quat::from_rotation_z(bangle.to_radians());
                let y_axis = Vec3::Z;
                let z_axis = pline_plax;
                let x_axis = y_axis.cross(z_axis).normalize();
                let quat = Quat::from_mat3(&Mat3::from_cols_array_2d(&[
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
            if rotation.is_nan() {
                // dbg!(&rotation);
                return Ok(None);
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
        if self.db_option.debug_print_world_transform {
            let rot_mat = Mat3::from_quat(rotation);
            let ori_str = math_tool::to_pdms_ori_str(&rot_mat);
            println!("{} : {} {:?}", refno.to_refno_str(), rot_mat, (translation, ori_str));
        }
        if rotation.is_nan() || translation.is_nan() { return Ok(None); }
        Ok(Some(Transform {
            rotation,
            translation,
            scale: Vec3::ONE,
        }))
    }

    ///获得子节点集合的属性
    async fn get_travel_children_attrs(&self, refno: RefU64, nouns: &[&str]) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        let children = query_deep_children_refnos_fuzzy(&self.get_arango_db().await?, refno, nouns).await?;
        for child in children {
            let attr = self.get_attr(child).await?;
            r.push(attr);
        }
        Ok(r)
    }


    ///获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(&self, refno: RefU64, distance: f32) -> anyhow::Result<Vec<RefU64>> {
        let rtree = self.rtree.as_ref().ok_or(anyhow!("空间树未生成。"))?;

        let db = &self.get_arango_db().await?;
        let instances = query_insts_shape_data(db, &[refno]).await?;
        if instances.inst_info_map.is_empty() { return Ok(vec![]); }
        let pos = instances.inst_info_map.iter().next().unwrap().1.world_transform.translation;
        let target_refnos = rtree.query_within_distance(pos, distance)
            .collect();
        ;

        Ok(target_refnos)
    }
}
