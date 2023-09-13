use crate::api::attr::*;
use crate::api::element::*;
use crate::aql_api::children::*;
use crate::aql_api::foreign_refnos::query_foreign_refnos_fuzzy;
use crate::aql_api::pdms_room::{RoomElement, RoomPanelElement};
use crate::cata::consts::*;
use crate::cata::query_cata::query_axis_params;
use crate::cata::query_cata::query_gm_param;
use crate::cata::resolve::CataContext;
use crate::cata::resolve::{CATA_CONTEXT_MAP, SCOM_INFO_MAP};
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::*;
use crate::defines::*;
use crate::graph_db::pdms_arango::ArPool;
use crate::graph_db::structs::PdmsEleGraphNode;
use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::cache::mgr::*;
use aios_core::cache::refno::*;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::*;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::*;
use aios_core::parsed_data::CateAxisParam;
use aios_core::pdms_data::GmParam;
use aios_core::pdms_data::PlinParam;
use aios_core::pdms_data::PlinParamData;
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
use aios_core::prim_geo::spine::{Spine3D, SpineCurveType};
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::tool::math_tool;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use anyhow::anyhow;
use approx::abs_diff_eq;
use async_trait::async_trait;
use bb8_arangodb::arangors_lite::AqlQuery;
use bevy_transform::prelude::Transform;
use dashmap::mapref::one::Ref;
use dashmap::DashMap;
use futures::StreamExt;
use glam::{Mat3, Quat, Vec3};
use itertools::Itertools;
use lazy_static::lazy_static;
use parry3d::bounding_volume::{aabb::Aabb, BoundingVolume};
use pdms_io::watch::PdmsWatcher;
use redb::{ReadableTable, TableDefinition};
use sqlx::{Executor, MySql, Pool, Row};
use std::boxed::Box;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::default::Default;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::data_interface::db_model::GLOBAL_MDB_WORLD_MAP;

// #[derive(Debug)]
pub struct AiosDBManager {
    //不同project的连接池子
    pub project_map: DashMap<String, Pool<MySql>>,

    // pub local_db_map: DashMap<String, Arc<redb::Database>>,

    // heed
    // pub local_db_map: DashMap<String, (Arc<heed::Env>, Arc<heed::Database<U64<BE>, ByteSlice>>) >,

    //sled
    ///本地缓存的atrr数据
    pub local_attr_db_map: DashMap<String, sled::Tree>,

    ///本地缓存的children数据
    pub local_children_db_map: DashMap<String, sled::Tree>,

    ///本地缓存的mesh数据
    pub local_mesh_db: sled::Tree,

    pub local_mesh_aabb_db: sled::Tree,

    pub ref0_projects: DashMap<u32, Vec<String>>,

    pub info_pool: Pool<MySql>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String, //整个项目的路径

    pub db_option: DbOption,

    pub cached_mesh_mgr: Arc<RwLock<PlantMeshesData>>,

    pub arango_pool: ArPool,

    pub cached_world_transforms_map: Arc<DashMap<RefU64, bevy_transform::prelude::Transform>>,

    pub cache_module_numbdbs: BTreeSet<i32>,

    pub mdb_dbnums: BTreeSet<i32>,

    pub watcher: PdmsWatcher,

    ///所有元素的tree
    pub rtree: Option<AccelerationTree>,

    ///room panels的aabb tree
    pub room_panels_rtree: Option<AccelerationTree>,

    ///room 对应的信息
    pub room_info_map: HashMap<RefU64, RoomElement>,

    ///room panel对应的信息
    pub room_panel_info_map: HashMap<RefU64, RoomPanelElement>,

    pub plin_params_map: DashMap<RefU64, DashMap<String, PlinParamData>>,
}

impl Debug for AiosDBManager {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "db manager project is {}", &self.project_path)
    }
}

const ATTR_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("kv");

#[async_trait]
impl PdmsDataInterface for AiosDBManager {
    /// 获得最全的数据
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        return if PDMS_ATT_MAP_CACHE.get(&refno).is_some() {
            let k = PDMS_ATT_MAP_CACHE.get(&refno).unwrap();
            Ok(k.value().clone())
        } else {
            // let attr = query_attr(refno, self, None).await?;
            let attr = self.get_attr_from_localdb(refno)?;
            PDMS_ATT_MAP_CACHE
                .insert(refno, &attr)
                .expect("PDMS_ATT_MAP_CACHE save error.");
            Ok(attr)
        };
    }

    fn get_type_name(&self, refno: RefU64) -> String {
        self.get_refno_basic(refno)
            .map(|x| x.get_type().to_string())
            .unwrap_or("unset".to_string())
    }

    ///从本地数据库获取属性
    fn get_attr_from_localdb(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        for project in &self.db_option.included_projects {
            if let Ok(a) = self.get_attr_within_project(refno, project.as_str()) {
                return Ok(a);
            }
        }
        Err(anyhow::anyhow!("{refno}: not found att"))
    }

    fn get_full_attr_from_localdb(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        let mut att_map = self.get_attr_from_localdb(refno)?;
        aios_core::get_default_pdms_db_info().fill_default_values(&mut att_map);

        Ok(att_map)
    }

    ///获得子节点的参考号集合
    fn get_children_from_localdb(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        for project in &self.db_option.included_projects {
            if let Ok(a) = self.get_children_within_project(refno, project.as_str()) {
                return Ok(a);
            }
        }
        // Ok(Default::default())
        Err(anyhow::anyhow!(format!("{refno} does not exist")))
    }

    fn get_mesh_from_localdb(&self, geo_hash: u64) -> anyhow::Result<PlantMesh> {
        let k = geo_hash.to_be_bytes();
        if let Some(bytes) = self.local_mesh_db.get(&k)? {
            return PlantMesh::from_compress_bytes(bytes.as_ref());
        }
        Err(anyhow::anyhow!(format!("{geo_hash} mesh not exist")))
    }

    fn get_mesh_aabb_from_localdb(&self, geo_hash: u64) -> anyhow::Result<Aabb> {
        let k = geo_hash.to_be_bytes();
        if let Some(bytes) = self.local_mesh_aabb_db.get(&k)? {
            return Aabb::from_bytes(bytes.as_ref());
        }
        Err(anyhow::anyhow!(format!("{geo_hash} aabb not exist.")))
    }

    /// 从本地数据库获得最全的数据
    fn get_attr_within_project(&self, refno: RefU64, project: &str) -> anyhow::Result<AttrMap> {
        if let Some(db) = self.local_attr_db_map.get(project) {
            let k = refno.0.to_be_bytes();
            if let Ok(Some(bytes)) = db.get(k.as_slice()) {
                return AttrMap::from_rkvy_compress_bytes(bytes.as_ref());
            }
        }
        Err(anyhow::anyhow!(format!("{refno} att not exist")))
    }

    fn get_children_within_project(
        &self,
        refno: RefU64,
        project: &str,
    ) -> anyhow::Result<RefU64Vec> {
        if let Some(db) = self.local_children_db_map.get(project) {
            let k = refno.0.to_be_bytes();
            if let Ok(Some(bytes)) = db.get(k.as_slice()) {
                return RefU64Vec::from_bytes(bytes.as_ref());
            }
        }
        Err(anyhow::anyhow!(format!(
            "{refno} att not exist in {project}"
        )))
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
    async fn query_foreign_refnos(
        &self,
        refnos: &[RefU64],
        start_types: &[&[&str]],
        end_types: &[&str],
        t_types: &[&str],
        depth: u32,
    ) -> anyhow::Result<Vec<RefU64>> {
        let t_refnos = query_foreign_refnos_fuzzy(
            &self.get_arango_db().await?,
            refnos,
            start_types,
            end_types,
            t_types,
            depth,
        )
            .await;
        t_refnos
    }

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(
        &self,
        refno: RefU64,
        start_types: &[&str],
        end_types: &[&str],
        t_types: &[&str],
    ) -> anyhow::Result<Option<RefU64>> {
        let id = format!("{}/{}", "pdms_eles", refno.to_url_refno());
        let aql = AqlQuery::new(r#"
            with pdms_eles, pdms_edges, foreign_edges
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
            .bind_var("t_types", t_types);
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

    /// 获得缓存的refno基本信息
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

    ///获得当前的项目名称
    fn get_cur_project(&self) -> &str {
        self.db_option.project_name.as_str()
    }

    ///获得当前的项目名称
    fn get_cur_mdb(&self) -> &str {
        self.db_option.mdb_name.as_str()
    }

    ///获得world节点
    async fn get_world(
        &self,
        project: &str,
        mdb_name: &str,
        module: &str,
    ) -> anyhow::Result<PdmsElement> {
        //todo 这里还需要将project的信息利用起来
        let hash_name = format!("{project}_{mdb_name}_{module}");
        if GLOBAL_MDB_WORLD_MAP.contains_key(&hash_name) {
            Ok(GLOBAL_MDB_WORLD_MAP.get(&hash_name).unwrap().clone())
        }else{
            let string = format!(
                "v.mdb_name==\"/{}\" and v.db_type==\"{}\"",
                mdb_name, module
            );
            let mut ele_nodes = self.query_ele_edges_by_expression(&string).await?;
            //从mdb 开始往下找，找到world
            if let Some(node) = ele_nodes.pop() {
                let mut children = self
                    .query_children_eles_order(node.owner, &[], &[module])
                    .await?;
                if let Some(ele) = children.pop() {
                    GLOBAL_MDB_WORLD_MAP.insert(hash_name, ele.clone());
                    return Ok(ele);
                }
            }
            Err(anyhow!("World not exist"))
        }
    }

    ///获得world节点
    async fn get_desi_world(&self) -> anyhow::Result<PdmsElement> {
        self.get_world(self.get_cur_project(), self.get_cur_mdb(), DESI)
            .await
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
    //todo use local db to get children refnos
    fn get_children_attrs(&self, refno: RefU64) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        if let Ok(children) = self.get_children_from_localdb(refno) {
            for child in children {
                let attr = self.get_attr_from_localdb(child).unwrap_or_default();
                r.push(attr);
            }
        }
        Ok(r)
    }

    ///获得参考号下的子节点
    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        self.get_children_from_localdb(refno)
    }

    ///获得参考号的name
    async fn get_name(&self, refno: RefU64) -> anyhow::Result<String> {
        if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
            let name = query_name(refno, &project_pool).await?;
            return Ok(name);
        }
        Err(anyhow::anyhow!("Element不存在"))
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
        let aql = AqlQuery::new(
            "\
        with pdms_edges, pdms_eles
        let negatives = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
                    PRUNE v.noun in @negative_nouns
                    filter v.noun in @negative_nouns
                    return p.vertices[-2]._key)
        return UNIQUE(negatives)
        ",
        )
            .bind_var("key", refno_url)
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        let refno_strs = self
            .get_arango_db()
            .await?
            .aql_query::<Vec<String>>(aql)
            .await?;
        let refnos = refno_strs
            .iter()
            .flatten()
            .map(|x| RefU64::from_url_refno(x).unwrap())
            .collect();
        Ok(refnos)
    }

    ///返回有负实体和正实体的参考号集合，还有对应的NOUN
    ///还要考虑下面有多个LOOP或者PLOO的情况，第二个开始都是负实体
    async fn query_refnos_has_pos_neg_map(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>> {
        let refno_urls = refnos
            .iter()
            .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
            .collect::<Vec<_>>();
        let aql = AqlQuery::new(
            r#"
            with pdms_edges, pdms_eles
            for key in @keys
                FOR v,e,p in 0..15 INBOUND key pdms_edges
                PRUNE v.noun in @neg_nouns
                OPTIONS { "order": "bfs"}
                let parent = p.vertices[-2]
                let children = ( for cc in 1 INBOUND parent._id pdms_edges return cc )
                let has_neg_internal = length(for c in children filter (c.noun in ["LOOP", "PLOO"]) return c._key) >= 2
                filter (v.noun in @neg_nouns) || has_neg_internal
                return [
                     parent._key,
                     (
                        let pos_vec = (for c in children filter c.noun in @pos_nouns return c._key)
                        let parent_is_pos = parent.noun in @pos_nouns
                        return parent_is_pos ? PUSH(pos_vec, parent._key) : pos_vec
                     )[0],
                    (for c in children filter (c.noun in @neg_nouns) return c._key)
                ]
        "#,
        )
            .bind_var("keys", refno_urls)
            .bind_var("neg_nouns", TOTAL_NEG_NOUN_NAMES.to_vec())
            .bind_var("pos_nouns", GENRAL_POS_NOUN_NAMES.to_vec());
        let result: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> = self
            .get_arango_db()
            .await?
            .aql_query::<RefnoHasNegPosInfoTuple>(aql)
            .await?
            .into_iter()
            .map(|x| (x.0, (x.1, x.2)))
            .collect();

        return Ok(result);
    }

    async fn query_parent_refnos_has_neg_geos(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<Vec<RefU64>> {
        let refno_urls = refnos
            .iter()
            .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_url_refno()))
            .collect::<Vec<_>>();
        let aql = AqlQuery::new(
            r#"
            with pdms_edges, pdms_eles
            for key in @keys
                FOR v,e,p in 0..15 INBOUND key pdms_edges
                    filter v.noun in @neg_geo_nouns
                    filter LENGTH(p.vertices) >= 2
                    let parent = p.vertices[-2]
                    return distinct parent._key
        "#,
        )
            .bind_var("keys", refno_urls)
            .bind_var("neg_geo_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        let refno_strs = self.get_arango_db().await?.aql_query::<String>(aql).await?;
        let refnos = refno_strs
            .iter()
            .map(|x| RefU64::from_url_refno(x).unwrap())
            .collect();
        Ok(refnos)
    }

    ///查询refno下是否有几何体
    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new(
            r#"
            with pdms_edges, pdms_eles
            let refnos = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
                        PRUNE v.noun in @geo_nouns
                        OPTIONS { "order": "bfs"}
                        filter v.noun in @geo_nouns
                        filter v != null
                        return LENGTH(p.vertices) > 1 ? p.vertices[-2]._key : p.vertices[0]._key
                    )
            return UNIQUE(refnos)
        "#,
        )
            .bind_var("key", refno_url)
            .bind_var("geo_nouns", TOTAL_GEO_NOUN_NAMES.to_vec());
        let refno_strs = self
            .get_arango_db()
            .await?
            .aql_query::<Vec<String>>(aql)
            .await?;
        let refnos = refno_strs
            .iter()
            .flatten()
            .map(|x| RefU64::from_url_refno(x).unwrap())
            .collect();
        Ok(refnos)
    }

    ///返回有负实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_neg_map(
        &self,
        refno: RefU64,
    ) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>> {
        let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
        let aql = AqlQuery::new(
            r#"
            with pdms_edges, pdms_eles
            FOR v,e,p in 0..15 INBOUND @key pdms_edges
                PRUNE v.noun in @negative_nouns
                OPTIONS { "order": "bfs"}
                filter v.noun in @negative_nouns
                collect parent = p.vertices[-2] into grouped
                return [
                     parent._key,
                     (for v in grouped[*].v filter v.noun in @negative_nouns  return v._key),
                ]
        "#,
        )
            .bind_var("key", refno_url)
            .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        let result: HashMap<RefU64, Vec<RefU64>> = self
            .get_arango_db()
            .await?
            .aql_query::<RefnoHasNegInfoTuple>(aql)
            .await?
            .into_iter()
            .map(|x| (x.0, x.1))
            .collect();

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

    //make a sync function, no need to be async
    ///获得世界坐标系, 需要缓存数据，如果已经存在数据了，直接获取
    fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>> {
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

        for (refno, ref_basic) in ancestors {
            let att = self.get_attr_from_localdb(refno)?;
            let mut pos = att.get_position().unwrap_or_default();
            let mut quat = Quat::IDENTITY;
            //土建特殊情况的一些处理
            if att.contains_attr_name("ZDIS") {
                let zdist = att.get_f32("ZDIS").unwrap_or_default();
                let pkdi = att.get_f32("PKDI").unwrap_or_default();
                let result = self.cal_zdis_pkdi_in_section(ref_basic.owner, pkdi, zdist);
                pos += result.1;
                quat *= result.0;
            }

            if att.contains_attr_name("NPOS") {
                let npos = att.get_vec3("NPOS").unwrap_or_default();
                pos += npos;
            }

            let owner_type_name = self.get_type_name(ref_basic.owner);
            let owner_is_gensec = owner_type_name == "GENSEC";
            let mut quat_v = att.get_rotation();
            let mut need_bangle = false;
            if !owner_is_gensec && quat_v.is_some() {
                quat = quat_v.unwrap();
            } else {
                let (l_poss, l_pose) = if owner_is_gensec {
                    //找到spine，获取spine的两个顶点
                    let mut positions: Vec<Vec3> = self
                        .get_children_from_localdb(ref_basic.owner)
                        .unwrap_or_default()
                        .into_iter()
                        .find(|x| self.get_type_name(*x).as_str() == "SPINE")
                        .map(|x| {
                            self.get_children_attrs(x)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|x| x.get_position().unwrap_or_default())
                        })
                        .into_iter()
                        .flatten()
                        .collect();
                    if positions.len() == 2 {
                        (Some(positions[0]), Some(positions[1]))
                    } else {
                        (None, None)
                    }
                } else {
                    (att.get_poss(), att.get_pose())
                };
                if let Some(poss) = l_poss &&
                    let Some(pose) = l_pose {
                    need_bangle = true;
                    let extru_dir = (pose - poss).normalize();
                    if !extru_dir.is_normalized() {
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
                    quat = Quat::from_mat3(&Mat3::from_cols(
                        p_axis,
                        y_axis,
                        extru_dir,
                    ));
                }
            }

            let bangle = att.get_f32("BANG").unwrap_or_default();
            if need_bangle || att.contains_attr_name("BANG") {
                quat = quat * Quat::from_rotation_z(bangle.to_radians());
            }
            //固定方位，不会怎旋转方向，但是会移动
            let mut fixed_posl_ori = att.get_type() == "ENDATU";

            //对于有CUTB的情况，需要直接对齐过去, 不需要在这里计算
            let c_ref = att.get_foreign_refno("CREF").unwrap_or_default();
            let mut has_cut_back = false;
            let mut cut_dir = Vec3::Y;
            if att.contains_attr_name("CUTB") {
                has_cut_back = true;
                cut_dir = att.get_vec3("CUTP").unwrap_or(cut_dir);
                let cut_len = att.get_f32("CUTB").unwrap_or_default();
                // dbg!(quat_to_pdms_ori_str(&c_t.rotation));
                if c_ref.is_valid() && let Ok(c_att) = self.get_attr_from_localdb(c_ref) &&
                    let Some(poss) = c_att.get_poss() &&
                    let Some(pose) = c_att.get_pose() {
                    let c_t = self.get_world_transform(c_ref)?.unwrap_or_default();
                    let w_poss = c_t.translation;
                    let axis = (pose - poss);
                    let len = axis.length();
                    let w_pose = w_poss + c_t.rotation * Vec3::Z * len;
                    // dbg!((w_poss, w_pose, translation));
                    let dist_s = translation.distance(w_poss);
                    let dist_e = translation.distance(w_pose);
                    //取离node最近的点
                    if dist_s < dist_e {
                        translation = w_poss - cut_dir * cut_len;
                    } else {
                        translation = w_pose - cut_dir * cut_len;
                    }
                }
            }
            //如果有posl
            if att.contains_attr_name("POSL") {
                let pos_line = att.get_str_or_default("POSL");
                let delta_vec = att.get_vec3("DELP").unwrap_or_default();
                // dbg!(pos_line);
                //plin里的位置偏移
                let mut plin_pos = Vec3::ZERO;
                let mut own_plin_pos = Vec3::ZERO;
                let mut pline_plax = Vec3::X;
                let mut new_quat = Quat::IDENTITY;
                let mut plin_owner = att.get_owner().unwrap();
                // POSL 的处理, 获得父节点的形集, 自身的形集处理，已经在profile里处理过
                let mut cur_plin_param = None;
                let mut own_plin_param = None;
                let mut target_own_att = AttrMap::default();
                const HAS_PLIN_TYPES: [&str; 4] = ["SCTN", "GENSEC", "WALL", "STWALL"];
                while cur_plin_param.is_none() {
                    let Some(t) = self.get_refno_basic(plin_owner) else {
                        break;
                    };
                    // #[cfg(debug_assertions)]
                    // dbg!(t.get_type());
                    if !HAS_PLIN_TYPES.contains(&t.get_type()) {
                        plin_owner = t.get_owner();
                        continue;
                    }
                    // dbg!(plin_owner);
                    // dbg!(pos_line);
                    target_own_att = self.get_attr_from_localdb(plin_owner).unwrap_or_default();
                    let own_pos_line = target_own_att.get_str_or_default("JUSL");
                    // dbg!(own_pos_line);
                    cur_plin_param = self.query_pline(plin_owner, pos_line)?;
                    own_plin_param = self.query_pline(plin_owner, own_pos_line)?;
                    if cur_plin_param.is_some() {
                        break;
                    }
                    plin_owner = t.get_owner();
                }
                let is_lmirror = target_own_att.get_bool("LMIRR").unwrap_or_default();
                if let Some(param) = cur_plin_param {
                    plin_pos = param.pt;
                    pline_plax = param.plax;
                    // dbg!(&param);
                }
                if let Some(own_param) = own_plin_param {
                    plin_pos -= own_param.pt;
                    // dbg!(&own_param);
                }
                let mut y_axis = if att.contains_attr_name("YDIR") {
                    att.get_vec3("YDIR").unwrap_or_default()
                } else {
                    Vec3::Z
                };
                //和LMIRROR 有关系
                let z_axis = if is_lmirror { -pline_plax } else { pline_plax };
                let x_axis = y_axis.cross(z_axis).normalize();
                let posl_quat = if fixed_posl_ori {
                    Quat::IDENTITY
                } else {
                    Quat::from_mat3(&Mat3::from_cols(x_axis, y_axis, z_axis))
                };
                // #[cfg(debug_assertions)]
                // {
                //     dbg!(quat_to_pdms_ori_str(&posl_quat));
                //     dbg!(quat_to_pdms_ori_str(&quat));
                // }
                new_quat = posl_quat * quat;
                // #[cfg(debug_assertions)]
                // {
                //     dbg!(quat_to_pdms_ori_str(&new_quat));
                //     dbg!(translation);
                //     dbg!(quat_to_pdms_ori_str(&rotation));
                // }


                translation +=
                    rotation * (pos + plin_pos) + rotation * new_quat * delta_vec;

                #[cfg(debug_assertions)]
                {
                    dbg!(translation);
                    dbg!(quat_to_pdms_ori_str(&rotation));
                }
                //没有POSL时，需要使用cutback的方向
                rotation = rotation * new_quat;
                if pos_line == "unset" && has_cut_back {
                    // dbg!(has_cut_back);
                    //need to perpendicular to the Y axis
                    let mat3 = Mat3::from_quat(rotation);
                    let y_axis = mat3.y_axis;
                    let ref_axis = cut_dir;
                    // dbg!(cut_dir);
                    let x_axis = y_axis.cross(ref_axis).normalize();
                    let z_axis = x_axis.cross(y_axis).normalize();
                    let new_mat = Mat3::from_cols(x_axis, y_axis, z_axis);
                    // dbg!(new_mat);
                    rotation = Quat::from_mat3(&new_mat);
                }
                // #[cfg(debug_assertions)]
                // dbg!(quat_to_pdms_ori_str(&rotation));
            } else {
                translation = translation + rotation * pos;
                rotation = rotation * quat;
            }

            let trans = Transform {
                rotation,
                translation,
                scale: Vec3::ONE,
            };
            if trans.is_nan() {
                return Ok(None);
            }
            self.cached_world_transforms_map
                .entry(refno)
                .or_insert(trans);
        }
        //将rotation 还原为角度
        if self.db_option.debug_print_world_transform {
            let rot_mat = Mat3::from_quat(rotation);
            let ori_str = math_tool::to_pdms_ori_str(&rot_mat);
            println!(
                "{} : {} {:?}",
                refno.to_refno_str(),
                rot_mat,
                (translation, ori_str)
            );
        }
        if rotation.is_nan() || translation.is_nan() {
            return Ok(None);
        }
        Ok(Some(Transform {
            rotation,
            translation,
            scale: Vec3::ONE,
        }))
    }

    ///获得子节点集合的属性
    async fn get_deep_children_attrs(
        &self,
        refno: RefU64,
        nouns: &[&str],
    ) -> anyhow::Result<Vec<AttrMap>> {
        let mut r = vec![];
        let children =
            query_deep_children_refnos_fuzzy(&self.get_arango_db().await?, &[refno], nouns).await?;
        for child in children {
            let attr = self.get_attr_from_localdb(child).unwrap_or_default();
            r.push(attr);
        }
        Ok(r)
    }

    ///指定refno获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(
        &self,
        refno: RefU64,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>> {
        let db = &self.get_arango_db().await?;
        let world_pos = self
            .get_world_transform(refno)?
            .unwrap_or_default()
            .translation;
        self.get_refnos_within_bound_radius_by_pos(world_pos, distance)
    }

    ///指定pos获得在一定范围的构件参考号列表
    fn get_refnos_within_bound_radius_by_pos(
        &self,
        pos: Vec3,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>> {
        let rtree = self
            .rtree
            .as_ref()
            .ok_or(anyhow::anyhow!("空间树未生成。"))?;
        let target_refnos = rtree
            .query_within_distance(pos, distance)
            .map(|x| x.0)
            .collect();
        Ok(target_refnos)
    }

    ///获取对应的截面sweep 线，包含了sctn的处理情况
    fn get_spline_path(&self, refno: RefU64) -> anyhow::Result<Vec<Spine3D>> {
        let children_refs = self.get_children_from_localdb(refno)?;
        let mut paths = vec![];
        for x in children_refs {
            let type_name = self.get_type_name(x);
            if type_name != "SPINE" {
                continue;
            }
            let spine_att = self.get_attr_from_localdb(x)?;
            // drns = spine_att.get_vec3("DRNS").unwrap_or_default();
            // drne = spine_att.get_vec3("DRNE").unwrap_or_default();
            let children_atts = self.get_children_attrs(x)?;
            if (children_atts.len() - 1) % 2 == 0 {
                for i in 0..(children_atts.len() - 1) / 2 {
                    let att1 = &(children_atts[2 * i]);
                    let att2 = &(children_atts[2 * i + 1]);
                    let att3 = &(children_atts[2 * i + 2]);
                    let pt0 = att1.get_position().unwrap_or_default();
                    let pt1 = att3.get_position().unwrap_or_default();
                    let mid_pt = att2.get_position().unwrap_or_default();
                    let cur_type_str = att2.get_str("CURTYP").unwrap_or("unset");
                    let curve_type = match cur_type_str {
                        "CENT" => SpineCurveType::CENT,
                        "THRU" => SpineCurveType::THRU,
                        _ => SpineCurveType::UNKNOWN,
                    };
                    paths.push(Spine3D {
                        pt0,
                        pt1,
                        thru_pt: mid_pt,
                        center_pt: mid_pt,
                        cond_pos: att2.get_vec3("CPOS").unwrap_or_default(),
                        curve_type,
                        preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
                        radius: att2.get_f32("RADI").unwrap_or_default(),
                    });
                }
            } else if children_atts.len() == 2 {
                let att1 = &children_atts[0];
                let att2 = &children_atts[1];
                let pt0 = att1.get_position().unwrap_or_default();
                let pt1 = att2.get_position().unwrap_or_default();
                if att1.get_type() == "POINSP" && att2.get_type() == "POINSP" {
                    paths.push(Spine3D {
                        pt0,
                        pt1,
                        curve_type: SpineCurveType::LINE,
                        preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
                        ..Default::default()
                    });
                }
            }
        }

        //考虑sctn这种直接拉升出来的情况
        if paths.is_empty() {
            let att = self.get_attr_from_localdb(refno)?;
            if let Some(poss) = att.get_poss() &&
                let Some(pose) = att.get_pose() {
                paths.push(Spine3D {
                    pt0: poss,
                    pt1: pose,
                    curve_type: SpineCurveType::LINE,
                    preferred_dir: Vec3::Z,
                    ..Default::default()
                });
            }
        }

        Ok(paths)
    }

    ///获得外键的属性
    #[inline]
    fn get_foreign_refno(&self, refno: RefU64, foreign: &str) -> Option<RefU64> {
        let att = self.get_attr_from_localdb(refno).ok()?;
        att.get_foreign_refno(foreign)
    }

    ///获得外键的属性
    #[inline]
    fn get_foreign_attrmap(&self, refno: RefU64, foreign: &str) -> Option<AttrMap> {
        self.get_foreign_refno(refno, foreign)
            .map(|x| self.get_attr_from_localdb(x).ok())
            .flatten()
    }

    ///获得元件库的spre参考号
    #[inline]
    fn get_spre_ref(&self, refno: RefU64) -> Option<RefU64> {
        self.get_foreign_refno(refno, "SPRE")
    }

    //todo need some test for this function
    ///获得元件库的catr参考号
    #[inline]
    fn get_cat_ref(&self, refno: RefU64) -> Option<RefU64> {
        let cat_ref = self
            .get_foreign_attrmap(refno, "SPRE")
            .map(|x| x.get_foreign_refno("CATR").or(x.get_refno()))
            .flatten();
        if cat_ref.is_some() {
            return cat_ref;
        }
        let self_cat_ref = self
            .get_foreign_attrmap(refno, "CATR")
            .map(|x| {
                let c_refno = x.get_refno().unwrap_or_default();
                match x.get_type() {
                    "TABITE" => self
                        .get_foreign_attrmap(c_refno, "PRTREF")
                        .map(|x| x.get_foreign_refno("CATR").unwrap_or_default()),
                    "SPCO" => self.get_foreign_refno(c_refno, "CATR"),
                    _ => Some(c_refno),
                }
            })
            .flatten();
        self_cat_ref
    }

    ///获得元件库的catr属性数据
    #[inline]
    fn get_cat_attmap(&self, refno: RefU64) -> Option<AttrMap> {
        self.get_cat_ref(refno)
            .map(|x| self.get_attr_from_localdb(x).ok())
            .flatten()
    }

    ///收集几何参数
    fn query_gm_params(&self, refno: RefU64) -> anyhow::Result<Vec<GmParam>> {
        let mut gms = vec![];
        let mut children = vec![];
        for c in self.get_children_attrs(refno)? {
            if TOTAL_CATA_GEO_NOUN_NAMES.contains(&c.get_type()) {
                children.push(c.clone());
            }
            //有可能嵌套负实体
            for cc in self.get_children_attrs(c.get_refno().unwrap_or_default())? {
                if TOTAL_CATA_GEO_NOUN_NAMES.contains(&cc.get_type()) {
                    children.push(cc.clone());
                }
            }
        }
        // let children = interface.get_deep_children_attrs(refno, &TOTAL_CATA_GEO_NOUN_NAMES).await.unwrap();
        for geo_am in children {
            if !geo_am.is_visible_by_level(None).unwrap_or(true) {
                continue;
            }
            let is_spro = geo_am.get_type() == "SPRO"; //todo add other types
            gms.push(query_gm_param(&geo_am, self, is_spro).unwrap_or_default());
        }
        Ok(gms)
    }

    ///收集SCOM的信息
    fn get_or_create_scom_info(&self, cata_refno: RefU64) -> anyhow::Result<ScomInfo> {
        let scom_info = if let Some(info) = SCOM_INFO_MAP.get(&cata_refno) {
            info.value().clone()
        } else {
            let attr_map = self.get_attr_from_localdb(cata_refno)?;
            let type_noun = attr_map.get_type();
            let ptref_name = match type_noun {
                "SPRF" => "PSTR",
                _ => "PTRE",
            };
            let mut axis_params = vec![];
            let mut axis_param_numbers = vec![];
            if let Some(ptre_refno) = attr_map.get_foreign_refno(ptref_name) {
                if let Ok(ptre_am) = self.get_attr_from_localdb(ptre_refno) {
                    if let Ok(axis_param_map) = query_axis_params(&ptre_am, Some(self)) {
                        axis_params = axis_param_map.values().cloned().collect::<Vec<_>>();
                        axis_param_numbers = axis_param_map.keys().cloned().collect::<Vec<_>>();
                    }
                }
            }
            let gmref_name = match type_noun {
                "SPRF" => "GSTR",
                _ => "GMRE",
            };
            let mut gm_params = vec![];
            if let Some(gmse_refno) = attr_map.get_foreign_refno(gmref_name) {
                gm_params = self.query_gm_params(gmse_refno)?;
            }
            let mut ngm_params = vec![];
            //-ve， 和design发生左右的负实体
            if let Some(gmse_refno) = attr_map.get_foreign_refno("NGMR") {
                ngm_params = self.query_gm_params(gmse_refno)?;
            }

            let mut plin_map = HashMap::new();
            if let Some(pstr_refno) = attr_map.get_foreign_refno("PSTR") {
                let pstr_am = self.get_children_attrs(pstr_refno)?;
                for a in pstr_am {
                    if let Some(k) = a.get_as_string("PKEY") {
                        plin_map.insert(
                            k,
                            PlinParam {
                                vxy: [
                                    a.get_as_string("PX").unwrap_or("0".to_string()),
                                    a.get_as_string("PY").unwrap_or("0".to_string()),
                                ],
                                dxy: [
                                    a.get_as_string("DX").unwrap_or("0".to_string()),
                                    a.get_as_string("DY").unwrap_or("0".to_string()),
                                ],
                                plax: a.get_as_string("PLAX").unwrap_or("unset".to_string()),
                            },
                        );
                    }
                }
            }
            ScomInfo {
                gtype: attr_map.get_as_string("GTYP").unwrap_or("unset".into()),
                dtse_params: vec![],
                gm_params,
                ngm_params,
                axis_params,
                params: attr_map
                    .get_as_string("PARA")
                    .unwrap_or_default()
                    .replace("\n", " ")
                    .replace("  ", " ")
                    .into(),
                axis_param_numbers,
                attr_map,
                plin_map,
            }
        };
        Ok(scom_info)
    }

    ///创建desi参考号的元件库计算上下文
    fn get_or_create_cata_context(
        &self,
        desi_refno: RefU64,
        extra_axis_map: Option<&BTreeMap<i32, CateAxisParam>>,
    ) -> anyhow::Result<CataContext> {
        let cata_context = if let Some(cata) = CATA_CONTEXT_MAP.get(&desi_refno) {
            cata.value().clone()
        } else {
            let desi_att = self.get_attr_from_localdb(desi_refno)?;
            let mut context = CataContext::default();
            if let Some(v) = desi_att.get_as_string("JUSL") {
                context.insert("JUSL".into(), v.into());
            }
            context.insert("DESI_REFNO".into(), desi_refno.to_refno_str());
            let mut desp = desi_att.get_f64_vec("DESP").unwrap_or_default();
            for i in 0..desp.len() {
                context.insert(format!("DESI{}", i + 1).into(), desp[i].to_string().into());
                context.insert(format!("DDES{}", i + 1).into(), desp[i].to_string().into());
                context.insert(format!("DESP{}", i + 1).into(), desp[i].to_string().into());
            }
            let height = desi_att.get_as_string("HEIG").unwrap_or("0.0".into());
            context.insert(DDHEIGHT_STR.into(), (height.clone()));
            let angle = desi_att.get_as_string("ANGL").unwrap_or("0.0".into());
            context.insert(DDANGLE_STR.into(), (angle.clone()));
            let radi = desi_att.get_as_string("RADI").unwrap_or("0.0".into());
            context.insert(DDRADIUS_STR.into(), (radi.clone()));

            //将attrmap里，是double的UDA属性，放入context
            for (k, v) in desi_att.iter() {
                let str = db1_dehash(*k);
                let n = if str.starts_with(":") {
                    if str.len() < 5 {
                        str.to_uppercase()
                    } else {
                        str[0..5].to_uppercase()
                    }
                } else {
                    str.to_uppercase()
                };
                match v {
                    AttrVal::DoubleType(d) => {
                        context.insert(n, d.to_string());
                    }
                    AttrVal::DoubleArrayType(ds) => {
                        for (i, d) in ds.into_iter().enumerate() {
                            // dbg!(format!("{}{}", &n, i+1));
                            context.insert(format!("{}{}", &n, i + 1), d.to_string());
                        }
                    }
                    _ => {}
                }
            }

            //添加 LEAWID、 LEAHEI、ARRWID、ARRHEI的值
            if let Some(axis_map) = extra_axis_map {
                if desi_att.contains_attr_name("LEAV") {
                    let arrive = desi_att.get_i32("ARRI").unwrap_or_default();
                    let leave = desi_att.get_i32("LEAV").unwrap_or_default();

                    if axis_map.contains_key(&arrive) {
                        let v = axis_map.get(&arrive).unwrap();
                        context.insert("ARRWID".into(), v.pwidth.to_string());
                        context.insert("ARRHEI".into(), v.pheight.to_string());
                    }

                    if axis_map.contains_key(&leave) {
                        let v = axis_map.get(&leave).unwrap();
                        context.insert("LEAWID".into(), v.pwidth.to_string());
                        context.insert("LEAHEI".into(), v.pheight.to_string());
                    }
                }
            }
            //todo 保温层厚度参数

            context.insert("RS_DES_REFNO".into(), desi_refno.to_refno_str());
            //添加cata的信息
            if let Some(cata_attmap) = self.get_cat_attmap(desi_refno) {
                context.insert(
                    "RS_SCOM_REFNO".into(),
                    cata_attmap.get_refno().unwrap().to_refno_str(),
                );
                // dbg!(&cata_attmap);
                let params = cata_attmap.get_f64_vec("PARA").unwrap_or_default();
                for i in 0..params.len() {
                    context.insert(
                        format!("CPAR{}", i + 1).into(),
                        params[i].to_string().into(),
                    );
                    context.insert(
                        format!("PARA{}", i + 1).into(),
                        params[i].to_string().into(),
                    );
                    context.insert(
                        format!("PARAM{}", i + 1).into(),
                        params[i].to_string().into(),
                    );
                    context.insert(format!("IPAR{}", i + 1).into(), "0".to_string().into());
                }
                let mut owner_ref = desi_att.get_owner().unwrap_or_default();
                let mut owner_att = self.get_attr_from_localdb(owner_ref).unwrap_or_default();
                while !owner_att.contains_attr_name("GTYP") {
                    if owner_att.get_refno().is_none() || owner_att.get_type() == "ZONE" {
                        break;
                    }
                    owner_ref = owner_att.get_owner().unwrap_or_default();
                    owner_att = self.get_attr_from_localdb(owner_ref).unwrap_or_default();
                }

                //dtse 的信息处理
                let dtre_refno: RefU64 = cata_attmap.get_foreign_refno("DTRE").unwrap_or_default();
                let children = self.get_children_attrs(dtre_refno).unwrap_or_default();
                for child in children {
                    if let Some(k) = child.get_as_string("DKEY") {
                        let key = format!("RPRO_{}", &k);
                        let exp = child.get_as_string("PPRO").unwrap_or_default();
                        let default_key = format!("{}_default_expr", key);
                        let default_expr = child.get_as_string("DPRO").unwrap_or_default();
                        context.insert(key, exp);
                        context.insert(default_key.into(), default_expr);
                    }
                }

                let desp = owner_att.get_f64_vec("DESP").unwrap_or_default();
                for i in 0..desp.len() {
                    context.insert(format!("ODES{}", i + 1).into(), desp[i].to_string().into());
                }
                //找到owner 参考号，再找到它的元件库params
                if let Some(parent_cat_am) = self.get_cat_attmap(owner_ref) {
                    let params = parent_cat_am.get_f64_vec("PARA").unwrap_or_default();
                    for i in 0..params.len() {
                        context.insert(
                            format!("OPAR{}", i + 1).into(),
                            params[i].to_string().into(),
                        );
                    }
                }

                if let Some(c_att) = self.get_foreign_attrmap(desi_refno, "CREF") {
                    let desp = c_att.get_f64_vec("DESP").unwrap_or_default();
                    for i in 0..desp.len() {
                        context.insert(format!("ADES{}", i + 1).into(), desp[i].to_string().into());
                    }
                    let c_refno = c_att.get_refno().unwrap_or_default();

                    if let Some(attach_cat_am) = self.get_cat_attmap(c_refno) {
                        let params = attach_cat_am.get_f64_vec("PARA").unwrap_or_default();
                        for i in 0..params.len() {
                            context.insert(
                                format!("APAR{}", i + 1).into(),
                                params[i].to_string().into(),
                            );
                        }
                    }
                }
            }

            context
        };
        Ok(cata_context)
    }
}
