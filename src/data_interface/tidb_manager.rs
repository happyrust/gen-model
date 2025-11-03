use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::CateGeoParam::*;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::*;
use aios_core::parsed_data::{CateAxisParam, CateGeomsInfo};
use aios_core::pdms_data::GmParam;
use aios_core::pdms_data::PlinParam;
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
use aios_core::prim_geo::spine::{Spine3D, SpineCurveType};
use aios_core::types::AttrVal::*;
use aios_core::{AttrMap, RefU64Vec};
use aios_core::{CataContext, rs_surreal};
use anyhow::anyhow;

use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::defines::*;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use dashmap::mapref::one::Ref;
use futures::StreamExt;
use glam::Vec3;
use itertools::Itertools;
use parry3d::bounding_volume::BoundingVolume;
use pdms_io::watch::PdmsWatcher;
use rumqttc::AsyncClient;
#[cfg(feature = "sql")]
use sqlx::{MySql, Pool, Row};
use std::boxed::Box;
use std::collections::BTreeMap;
use std::collections::{HashMap, VecDeque};
use std::default::Default;
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone)]
pub struct AiosDBManager {
    //不同project的连接池子
    #[cfg(feature = "sql")]
    pub project_map: DashMap<String, Pool<MySql>>,

    pub projects: Vec<String>,

    pub needed_parse_files: Option<Vec<String>>,

    pub project_path: String, //整个项目的路径

    pub db_option: DbOption,

    pub watcher: Arc<PdmsWatcher>,

    pub mqtt_client: Arc<AsyncClient>,

    ///所有元素的tree
    pub rtree: Option<AccelerationTree>,
}

/// Implements the `Debug` trait for `AiosDBManager`.
///
/// This allows `AiosDBManager` instances to be formatted using the `fmt` method from the `Debug` trait.
impl Debug for AiosDBManager {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "db manager project is {}", &self.project_path)
    }
}

impl PdmsDataInterface for AiosDBManager {
    /// 获得最全的数据
    async fn get_attr(&self, refno: RefU64) -> anyhow::Result<NamedAttrMap> {
        let attr = aios_core::get_named_attmap(refno.into()).await?;
        Ok(attr)
    }

    ///获得类型名称
    async fn get_type_name(&self, refno: RefU64) -> String {
        aios_core::get_type_name(refno.into())
            .await
            .unwrap_or_default()
    }

    ///获得下一个构件的参考号
    async fn get_next(&self, refno: RefU64) -> anyhow::Result<RefU64> {
        Ok(Default::default())
    }

    ///获得上一个构件的参考号
    async fn get_prev(&self, refno: RefU64) -> anyhow::Result<RefU64> {
        Ok(Default::default())
    }

    //todo 修改为图数据库，尽可能避免使用TIDB
    ///获取owner的参考号，从缓存读取
    #[inline]
    fn get_owner(&self, refno: RefU64) -> RefU64 {
        Default::default()
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
        // let t_refnos = query_foreign_refnos_fuzzy(
        //     &self.get_arango_db().await?,
        //     refnos,
        //     start_types,
        //     end_types,
        //     t_types,
        //     depth,
        // )
        // .await;
        // t_refnos
        Ok(Default::default())
    }

    ///沿着owner path找到需要找的第一个foreign目标节点，可以找到父节点，也可以找到子节点
    async fn query_first_foreign_along_path(
        &self,
        refno: RefU64,
        start_types: &[&str],
        end_types: &[&str],
        t_types: &[&str],
    ) -> anyhow::Result<Option<RefU64>> {
        // let id = format!("{}/{}", "pdms_eles", refno.to_string());
        // let aql = AqlQuery::new(r#"
        //     with pdms_eles, pdms_edges, foreign_edges
        //     FOR v,e,p in 1..15 OUTBOUND @id pdms_edges
        //         filter document(v._id) != null
        //         let xx = (for ver, edge, path in 1..10 OUTBOUND v._id foreign_edges
        //                    filter document(ver._id) != null
        //                    //判断是否是叶子节点
        //                    FILTER LENGTH(@t_types) == 0 and length(for c in 1 INBOUND ver._id foreign_edges
        //                         return 0 )
        //                    filter LENGTH(@start_types) == 0 or path.edges[0].foreign_type in @start_types
        //                    filter LENGTH(@end_types) == 0 or (edge.foreign_type in @end_types)
        //                    filter LENGTH(@t_types) == 0 or (ver.noun in @t_types)
        //                    LIMIT 1
        //                    return ver)
        //         filter LENGTH(xx) != 0
        //         LIMIT 1
        //         return xx[0]._key
        //         "#)
        //     .bind_var("id", id)
        //     .bind_var("start_types", start_types)
        //     .bind_var("end_types", end_types)
        //     .bind_var("t_types", t_types);
        // let results: Vec<String> = self.get_arango_db().await?.aql_query(aql).await?;
        // for result in results {
        //     if let Ok(refno) = RefU64::from_str(&result) {
        //         return Ok(Some(refno));
        //     }
        // }
        Ok(None)
    }

    /// 获得隐含数据的属性
    async fn get_implicit_attr(
        &self,
        refno: RefU64,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<AttrMap> {
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
        //     if let Some(ref_basic) = self.get_refno_basic(refno) {
        //         let attr =
        //             query_implicit_attr(refno, ref_basic.value(), &project_pool, columns).await?;
        //         return Ok(attr);
        //     }
        // }
        Ok(AttrMap::default())
    }

    /// 获得OWNER隐含数据的属性
    async fn get_implicit_attrs_by_owner(
        &self,
        owner: RefU64,
        type_name: &str,
        columns: Option<Vec<&str>>,
    ) -> anyhow::Result<Vec<AttrMap>> {
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(owner).await {
        //     let attr =
        //         query_implicit_attrs_by_owner(owner, type_name, &project_pool, columns).await?;
        //     return Ok(attr);
        // }
        Ok(vec![])
    }

    /// 获取parent的attr数据
    async fn get_parent_attr(&self, refno: RefU64) -> anyhow::Result<AttrMap> {
        todo!()
    }

    /// 获得节点数据
    async fn get_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
        //     if let Ok(node) = query_ele_node(refno, &project_pool).await {
        //         return Ok(Some(node));
        //     }
        // }
        Ok(None)
    }

    ///获得owner
    async fn get_owner_ele_node(&self, refno: RefU64) -> anyhow::Result<Option<EleTreeNode>> {
        let mut node = None;
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
        //     let parent = self.get_owner(refno);
        //     if parent.is_valid() {
        //         node = Some(query_ele_node(parent, &project_pool).await?);
        //     }
        // }
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
        // //todo 这里还需要将project的信息利用起来
        // let hash_name = format!("{project}_{mdb_name}_{module}");
        // if GLOBAL_MDB_WORLD_MAP.contains_key(&hash_name) {
        //     Ok(GLOBAL_MDB_WORLD_MAP.get(&hash_name).unwrap().clone())
        // } else {
        //     // 通过 fulltext在数据库中查询
        //     let database = self.get_arango_db().await?;
        //     let ele = query_mdb_world_fulltext(mdb_name, module, &database).await?;
        //     if let Some(ele) = ele {
        //         GLOBAL_MDB_WORLD_MAP.insert(hash_name, ele.clone());
        //         return Ok(ele);
        //     }
        //     Err(anyhow!("World not exist"))
        // }
        Ok(PdmsElement::default())
    }

    ///获得world节点
    async fn get_desi_world(&self) -> anyhow::Result<PdmsElement> {
        self.get_world(self.get_cur_project(), self.get_cur_mdb(), DESI)
            .await
    }

    ///获得子节点集合
    async fn get_children_nodes(&self, refno: RefU64) -> anyhow::Result<Vec<EleTreeNode>> {
        let mut r = vec![];
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
        //     let children = query_children(refno, &project_pool).await?;
        //     for (refno, _) in children {
        //         let node = query_ele_node(refno, &project_pool).await?;
        //         r.push(node);
        //     }
        // }
        Ok(r)
    }

    ///获得参考号下的子节点
    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        Ok(Default::default())
    }

    ///获得参考号的name
    async fn get_name(&self, refno: RefU64) -> anyhow::Result<String> {
        // if let Some((_, project_pool)) = self.get_project_pool_by_refno(refno).await {
        //     let name = query_name(refno, &project_pool).await?;
        //     return Ok(name);
        // }
        Err(anyhow::anyhow!("Element不存在"))
    }

    /// dbnos为空代表所有db都会去获取
    async fn get_refnos_by_types(
        &self,
        project: &str,
        att_types: &[&str],
        dbnos: &[i32],
    ) -> anyhow::Result<RefU64Vec> {
        // if let Some(project_pool) = self.project_map.get(project) {
        //     let r = query_types_refnos(att_types, project_pool.value(), dbnos).await?;
        //     return Ok(r);
        // }
        Ok(RefU64Vec::default())
    }

    /// 获得当前db的world 参考号
    async fn get_db_world(
        &self,
        project: &str,
        db_no: u32,
    ) -> anyhow::Result<Option<(RefU64, String)>> {
        // if let Some(project_pool) = self.project_map.get(project) {
        //     let r =
        //         query_id_name_from_dbno_type(db_no as i32, "WORL", project_pool.value()).await?;
        //     if let Some(mut r) = r {
        //         return Ok(Some(r.remove(0)));
        //     }
        // }
        return Ok(None);
    }

    /// 获得参考号的祖先参考号
    fn get_ancestors_refnos(&self, refno: RefU64) -> Vec<RefU64> {
        let mut result = vec![refno]; //需要包含自己
        let mut cur_refno = refno;
        // 已废弃: cache 模块已移除，使用 get_owner 替代
        loop {
            let owner = self.get_owner(cur_refno);
            if !owner.is_valid() || owner == cur_refno {
                break;
            }
            cur_refno = owner;
            result.push(cur_refno);
        }
        result
    }

    ///查询哪些有负实体的参考号
    async fn query_refnos_has_neg_geom(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        // let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string());
        // let aql = AqlQuery::new(
        //     "\
        // with pdms_edges, pdms_eles
        // let negatives = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
        //             PRUNE v.noun in @negative_nouns
        //             filter v.noun in @negative_nouns
        //             return p.vertices[-2]._key)
        // return UNIQUE(negatives)
        // ",
        // )
        // .bind_var("key", refno_url)
        // .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        // let refno_strs = self
        //     .get_arango_db()
        //     .await?
        //     .aql_query::<Vec<String>>(aql)
        //     .await?;
        // let refnos = refno_strs
        //     .iter()
        //     .flatten()
        //     .map(|x| RefU64::from_str(x).unwrap())
        //     .collect();
        // Ok(refnos)
        Ok(Default::default())
    }

    ///返回有负实体和正实体的参考号集合，还有对应的NOUN
    ///还要考虑下面有多个LOOP或者PLOO的情况，第二个开始都是负实体
    async fn query_refnos_has_pos_neg_map(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)>> {
        // let refno_urls = refnos
        //     .iter()
        //     .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_string()))
        //     .collect::<Vec<_>>();
        // let aql = AqlQuery::new(
        //     r#"
        //     with pdms_edges, pdms_eles
        //     for key in @keys
        //         FOR v,e,p in 0..15 INBOUND key pdms_edges
        //         PRUNE v.noun in @neg_nouns
        //         OPTIONS { "order": "bfs"}
        //         let parent = p.vertices[-2]
        //         let children = ( for cc in 1 INBOUND parent._id pdms_edges return cc )
        //         let has_neg_internal = length(for c in children filter (c.noun in ["LOOP", "PLOO"]) return c._key) >= 2
        //         filter (v.noun in @neg_nouns) || has_neg_internal
        //         return [
        //              parent._key,
        //              (
        //                 let pos_vec = (for c in children filter c.noun in @pos_nouns return c._key)
        //                 let parent_is_pos = parent.noun in @pos_nouns
        //                 return parent_is_pos ? PUSH(pos_vec, parent._key) : pos_vec
        //              )[0],
        //             (for c in children filter (c.noun in @neg_nouns) return c._key)
        //         ]
        // "#,
        // )
        //     .bind_var("keys", refno_urls)
        //     .bind_var("neg_nouns", TOTAL_NEG_NOUN_NAMES.to_vec())
        //     .bind_var("pos_nouns", GENRAL_POS_NOUN_NAMES.to_vec());
        // let result: HashMap<RefU64, (Vec<RefU64>, Vec<RefU64>)> = self
        //     .get_arango_db()
        //     .await?
        //     .aql_query::<RefnoHasNegPosInfoTuple>(aql)
        //     .await?
        //     .into_iter()
        //     .map(|x| (x.0, (x.1, x.2)))
        //     .collect();

        return Ok(Default::default());
    }

    async fn query_parent_refnos_has_neg_geos(
        &self,
        refnos: &[RefU64],
    ) -> anyhow::Result<Vec<RefU64>> {
        // let refno_urls = refnos
        //     .iter()
        //     .map(|x| format!("{AQL_PDMS_ELES_COLLECTION}/{}", x.to_string()))
        //     .collect::<Vec<_>>();
        // let aql = AqlQuery::new(
        //     r#"
        //     with pdms_edges, pdms_eles
        //     for key in @keys
        //         FOR v,e,p in 0..15 INBOUND key pdms_edges
        //             filter v.noun in @neg_geo_nouns
        //             filter LENGTH(p.vertices) >= 2
        //             let parent = p.vertices[-2]
        //             return distinct parent._key
        // "#,
        // )
        // .bind_var("keys", refno_urls)
        // .bind_var("neg_geo_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        // let refno_strs = self.get_arango_db().await?.aql_query::<String>(aql).await?;
        // let refnos = refno_strs
        //     .iter()
        //     .map(|x| RefU64::from_str(x).unwrap())
        //     .collect();
        // Ok(refnos)
        Ok(Default::default())
    }

    ///查询refno下是否有几何体
    async fn query_refnos_has_geos(&self, refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        // let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string());
        // let aql = AqlQuery::new(
        //     r#"
        //     with pdms_edges, pdms_eles
        //     let refnos = ( FOR v,e,p in 0..15 INBOUND @key pdms_edges
        //                 PRUNE v.noun in @geo_nouns
        //                 OPTIONS { "order": "bfs"}
        //                 filter v.noun in @geo_nouns
        //                 filter v != null
        //                 return LENGTH(p.vertices) > 1 ? p.vertices[-2]._key : p.vertices[0]._key
        //             )
        //     return UNIQUE(refnos)
        // "#,
        // )
        // .bind_var("key", refno_url)
        // .bind_var("geo_nouns", TOTAL_GEO_NOUN_NAMES.to_vec());
        // let refno_strs = self
        //     .get_arango_db()
        //     .await?
        //     .aql_query::<Vec<String>>(aql)
        //     .await?;
        // let refnos = refno_strs
        //     .iter()
        //     .flatten()
        //     .map(|x| RefU64::from_str(x).unwrap())
        //     .collect();
        // Ok(refnos)
        Ok(Default::default())
    }

    ///返回有负实体的参考号集合，还有对应的NOUN
    async fn query_refnos_has_neg_map(
        &self,
        refno: RefU64,
    ) -> anyhow::Result<HashMap<RefU64, Vec<RefU64>>> {
        // let refno_url = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_string());
        // let aql = AqlQuery::new(
        //     r#"
        //     with pdms_edges, pdms_eles
        //     FOR v,e,p in 0..15 INBOUND @key pdms_edges
        //         PRUNE v.noun in @negative_nouns
        //         OPTIONS { "order": "bfs"}
        //         filter v.noun in @negative_nouns
        //         collect parent = p.vertices[-2] into grouped
        //         return [
        //              parent._key,
        //              (for v in grouped[*].v filter v.noun in @negative_nouns  return v._key),
        //         ]
        // "#,
        // )
        // .bind_var("key", refno_url)
        // .bind_var("negative_nouns", GENRAL_NEG_NOUN_NAMES.to_vec());
        // let result: HashMap<RefU64, Vec<RefU64>> = self
        //     .get_arango_db()
        //     .await?
        //     .aql_query::<RefnoHasNegInfoTuple>(aql)
        //     .await?
        //     .into_iter()
        //     .map(|x| (x.0, x.1))
        //     .collect();

        return Ok(Default::default());
    }

    /// 获得参考号的祖先属性
    async fn get_ancestors_attrs(&self, refno: RefU64) -> Vec<AttrMap> {
        let mut cur_refno = refno;
        let mut r = vec![];
        // if let Some((_, pool)) = self.get_project_pool_by_refno(refno).await {
        //     while let Ok(attr) = self.get_implicit_attr(cur_refno, None).await {
        //         //后面是不是要缓存这个层级结构
        //         if let Ok(Some(owner)) = query_owner_from_id(cur_refno, &pool).await {
        //             r.push(attr);
        //             cur_refno = owner;
        //         } else {
        //             break;
        //         }
        //     }
        // }
        r
    }

    /// 获得参考号的祖先节点
    async fn get_ancestor_nodes(&self, refno: RefU64) -> anyhow::Result<VecDeque<EleTreeNode>> {
        let mut cur_refno = refno;
        let mut ancestors = VecDeque::new();
        // while let Some(node) = self.get_ele_node(cur_refno).await? {
        //     cur_refno = node.owner;
        //     ancestors.push_front(node);
        // }
        Ok(ancestors)
    }

    ///使用cache，需要从db manager里移除出来
    ///获得世界坐标系, 需要缓存数据，如果已经存在数据了，直接获取
    async fn get_world_transform(&self, refno: RefU64) -> anyhow::Result<Option<Transform>> {
        aios_core::get_world_transform(refno.into()).await
    }

    #[inline]
    async fn get_world_transform_or_default(&self, refno: RefU64) -> Transform {
        self.get_world_transform(refno)
            .await
            .unwrap_or_default()
            .unwrap_or_default()
    }

    ///获得子节点集合的属性
    async fn get_deep_children_attrs(
        &self,
        refno: RefU64,
        nouns: &[&str],
    ) -> anyhow::Result<Vec<NamedAttrMap>> {
        let mut r = vec![];
        // let children =
        //     query_deep_children_refnos_fuzzy(&self.get_arango_db().await?, &[refno], nouns).await?;
        // for child in children {
        //     let attr = aios_core::get_named_attmap(child).await.unwrap_or_default();
        //     r.push(attr);
        // }
        Ok(r)
    }

    ///指定refno获得在一定范围的构件参考号列表
    async fn get_refnos_within_bound_radius(
        &self,
        refno: RefU64,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>> {
        // let db = &self.get_arango_db().await?;
        // let world_pos = self
        //     .get_world_transform(refno)
        //     .await?
        //     .unwrap_or_default()
        //     .translation;
        // self.get_refnos_within_bound_radius_by_pos(world_pos, distance)
        Ok(vec![])
    }

    ///指定pos获得在一定范围的构件参考号列表
    fn get_refnos_within_bound_radius_by_pos(
        &self,
        pos: Vec3,
        distance: f32,
    ) -> anyhow::Result<Vec<RefU64>> {
        // let rtree = self
        //     .rtree
        //     .as_ref()
        //     .ok_or(anyhow::anyhow!("空间树未生成。"))?;
        // let target_refnos = rtree
        //     .query_within_distance(pos, distance)
        //     .map(|x| x.0)
        //     .collect();
        // Ok(target_refnos)
        Ok(vec![])
    }

    ///获取对应的截面sweep 线，包含了sctn的处理情况
    async fn get_spline_path(&self, refno: RefU64) -> anyhow::Result<Vec<Spine3D>> {
        // let children_refs = aios_core::get_children_refnos(refno).await?;
        let mut paths = vec![];
        // for x in children_refs {
        //     let type_name = self.get_type_name(x).await;
        //     if type_name != "SPINE" {
        //         continue;
        //     }
        //     let spine_att = aios_core::get_named_attmap(x).await?;
        //     let children_atts = aios_core::get_children_named_attmaps(x).await?;
        //     if (children_atts.len() - 1) % 2 == 0 {
        //         for i in 0..(children_atts.len() - 1) / 2 {
        //             let att1 = &(children_atts[2 * i]);
        //             let att2 = &(children_atts[2 * i + 1]);
        //             let att3 = &(children_atts[2 * i + 2]);
        //             let pt0 = att1.get_position().unwrap_or_default();
        //             let pt1 = att3.get_position().unwrap_or_default();
        //             let mid_pt = att2.get_position().unwrap_or_default();
        //             let cur_type_str = att2.get_str("CURTYP").unwrap_or("unset");
        //             let curve_type = match cur_type_str {
        //                 "CENT" => SpineCurveType::CENT,
        //                 "THRU" => SpineCurveType::THRU,
        //                 _ => SpineCurveType::UNKNOWN,
        //             };
        //             paths.push(Spine3D {
        //                 pt0,
        //                 pt1,
        //                 thru_pt: mid_pt,
        //                 center_pt: mid_pt,
        //                 cond_pos: att2.get_vec3("CPOS").unwrap_or_default(),
        //                 curve_type,
        //                 preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
        //                 radius: att2.get_f32("RADI").unwrap_or_default(),
        //             });
        //         }
        //     } else if children_atts.len() == 2 {
        //         let att1 = &children_atts[0];
        //         let att2 = &children_atts[1];
        //         let pt0 = att1.get_position().unwrap_or_default();
        //         let pt1 = att2.get_position().unwrap_or_default();
        //         if att1.get_type_str() == "POINSP" && att2.get_type_str() == "POINSP" {
        //             paths.push(Spine3D {
        //                 pt0,
        //                 pt1,
        //                 curve_type: SpineCurveType::LINE,
        //                 preferred_dir: spine_att.get_vec3("YDIR").unwrap_or(Vec3::Z),
        //                 ..Default::default()
        //             });
        //         }
        //     }
        // }
        //
        // //考虑sctn这种直接拉升出来的情况
        // if paths.is_empty() {
        //     let att = aios_core::get_named_attmap(refno).await?;
        //     if let Some(poss) = att.get_poss()
        //         && let Some(pose) = att.get_pose()
        //     {
        //         paths.push(Spine3D {
        //             pt0: poss,
        //             pt1: pose,
        //             curve_type: SpineCurveType::LINE,
        //             preferred_dir: Vec3::Z,
        //             ..Default::default()
        //         });
        //     }
        // }
        //
        Ok(paths)
    }

    ///获得外键的属性
    #[inline]
    async fn get_foreign_refno(&self, refno: RefU64, foreign: &str) -> Option<RefU64> {
        None
    }

    ///获得外键的属性
    #[inline]
    async fn get_foreign_attrmap(&self, refno: RefU64, foreign: &str) -> Option<NamedAttrMap> {
        None
    }

    ///获得元件库的spre参考号
    #[inline]
    async fn get_spre_ref(&self, refno: RefU64) -> Option<RefU64> {
        None
    }

    ///获得元件库的catr参考号
    #[inline]
    async fn get_cat_refno(&self, refno: RefU64) -> Option<RefU64> {
        // aios_core::get_cat_refno(refno.into()).await.ok().flatten()
        None
    }

    ///获得元件库的catr属性数据
    #[inline]
    async fn get_cat_attmap(&self, refno: RefU64) -> Option<NamedAttrMap> {
        // aios_core::get_cat_attmap(refno.into()).await.ok()
        None
    }
}
