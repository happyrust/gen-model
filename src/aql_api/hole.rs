use std::collections::{BTreeMap, HashMap};
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::{PrimExtrusion, PrimSCylinder};
use aios_core::pdms_types::{GeoBasicType, PdmsElement, RefU64};
use aios_core::pdms_types::GeoBasicType::CateNeg;
use aios_core::plugging_material::PluggingData;
use aios_core::virtual_hole::HoleInstInfo;
use anyhow::anyhow;
use arangors_lite::AqlQuery;
use bitvec::macros::internal::funty::Floating;
use glam::Vec3;
use crate::aql_api::children::*;
use crate::aql_api::pdms_room::*;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use crate::test::test_helper::get_test_ams_db_manager_async;

/// 返回封堵材料统计插件数据
pub async fn get_plugging_material_datas(select_refno: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PluggingData>> {
    // 找到所有需要计算的孔洞
    let holes = query_hole_elements(select_refno, &database).await?;
    dbg!(&holes.len());
    // 查找他的instance
    let insts = query_hole_instance(&holes, &database).await?;
    let name_map = holes.into_iter().map(|refno| (refno.refno, refno)).collect::<HashMap<RefU64, PdmsElement>>();
    compute_hole_instance_data(&database, insts, name_map).await
}

/// 获取需要计算的孔洞
pub async fn query_hole_elements(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<Vec<PdmsElement>> {
    // 根据指定规则获取实体孔洞
    let aql = gen_query_pdms_hole_aql(refnos);
    let result = database.aql_query::<PdmsElement>(aql).await?;
    if result.len() > 100000 { return Err(anyhow::anyhow!("超过最大查询数量!")); }
    Ok(result)
}

/// 查询孔洞的模型数据
pub async fn query_hole_instance(holes: &Vec<PdmsElement>, database: &ArDatabase) -> anyhow::Result<Vec<HoleInstInfo>> {
    // 该节点子节点才是孔洞,需要找下面的fixing或者sbfi
    let mut gtypes = Vec::new();
    // 对应的节点就是孔洞
    let mut fitts = Vec::new();
    // 分类
    for hole in holes {
        match hole.noun.as_str() {
            "JLDATUM" | "CMFI" | "CMPF" => {
                gtypes.push(format!("{}/{}", AQL_PDMS_ELES_COLLECTION, hole.refno.to_url_refno()));
            }
            _ => {
                fitts.push(format!("{}", hole.refno.to_url_refno()));
            }
        }
    }
    let aql = AqlQuery::new("\
    With @@pdms_eles,@@pdms_edges,@@pdms_inst_infos,@@pdms_inst_geos
    let gtypes = (
        for id in @ids
            for v in 0..3 inbound id pdms_edges
                filter v != null
                filter v.noun in ['FIXING','SBFI']
                return v._key
    )
    let nodes = append(@fitts,gtypes)
    for node in nodes
        let cata_hash = document('pdms_inst_infos',node)
        let hash = cata_hash.cata_hash == null ? cata_hash._key : cata_hash.cata_hash
        let geo = document('pdms_inst_geos',hash)
        filter geo != null
        return {
        'refno': node,
        'inst': geo.insts
        }
    ").bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION)
        .bind_var("@pdms_inst_geos", AQL_PDMS_INST_GEO_COLLECTION)
        .bind_var("ids", gtypes)
        .bind_var("fitts", fitts);
    let result = database.aql_query::<HoleInstInfo>(aql).await?;
    Ok(result)
}

/// 计算孔洞的体积
pub async fn compute_hole_instance_data(/*mgr: &AiosDBManager,*/
                                        database: &ArDatabase,
                                        hole_instance: Vec<HoleInstInfo>,
                                        name_map: HashMap<RefU64, PdmsElement>,
) -> anyhow::Result<Vec<PluggingData>> {
    // let database = &mgr.get_arango_db().await?;
    let mut result = Vec::new();
    for hole in hole_instance {
        let mut hole_circle_inst = Vec::new();
        let mut hole_rect_inst = Vec::new();
        let insts = hole.inst;
        // 统计一个节点下的所有模型，过滤掉不需要的模型
        for inst in &insts {
            match inst.geo_type {
                // 圆孔
                CateNeg => {
                    match &inst.geo_param {
                        // 套管做特殊处理，他是上下两个合并在一起的
                        PrimSCylinder(data) => {
                            hole_circle_inst.push(data);
                        }
                        _ => { continue; }
                    }
                }
                // 方孔
                GeoBasicType::Neg => {
                    match &inst.geo_param {
                        PrimExtrusion(extrusion) => {
                            if extrusion.verts.len() != 4 { continue; };
                            hole_rect_inst.push(extrusion);
                        }
                        _ => {}
                    }
                }
                _ => { continue; }
            }
        }
        // 将套管两个模型合并在一起
        if hole_circle_inst.len() == 2 {
            let diameter = hole_circle_inst[0].pdia as f64;
            let height = (hole_circle_inst[0].phei + hole_circle_inst[1].phei) as f64;

            let element = if name_map.contains_key(&hole.refno) {
                name_map.get(&hole.refno).unwrap().clone()
            } else {
                // 这几个类型name取他上面的对应层级
                query_ancestor_till_types_aql(database, hole.refno, vec!["JLDATUM", "CMFI", "CMPF"]).await?.unwrap_or_default()
            };
            // let (room_1, room_2) = mgr.query_through_element_room_nums(&[hole.refno]).await?.values().nth(0).cloned().unwrap_or_default();
            let cable_area = get_cable_area(&element.name).await;
            let plugging_area = f64::PI * (diameter / 2.0) * (diameter / 2.0) - cable_area;
            let fill_percent = get_plugging_fill_percent().await;
            let plugging_volume = f64::PI * (diameter / 2.0) * (diameter / 2.0) * height * (1.0 - fill_percent);
            result.push(PluggingData {
                own_refno: Default::default(),
                refno: hole.refno,
                name: element.name,
                size: format!("{}", diameter),
                room_1: "".to_string(),
                room_2: "".to_string(),
                height,
                cable_area,
                plugging_area,
                plugging_volume,
                materials: "".to_string(),
            })
        }
        // 计算方孔的数据
        if hole_rect_inst.len() == 1 {
            let points = &hole_rect_inst[0].verts;
            let height = hole_rect_inst[0].height as f64;
            let Some((size_1, size_2)) = compute_rectangle_data([points[0], points[1], points[2], points[3]]) else { continue; };
            let size_1 = size_1 as f64;
            let size_2 = size_2 as f64;
            let element = if name_map.contains_key(&hole.refno) {
                name_map.get(&hole.refno).unwrap().clone()
            } else {
                // 这几个类型name取他上面的对应层级
                query_ancestor_till_types_aql(database, hole.refno, vec!["JLDATUM", "CMFI", "CMPF"]).await?.unwrap_or_default()
            };
            // let (room_1, room_2) = mgr.query_through_element_room_nums(&[hole.refno]).await?.values().nth(0).cloned().unwrap_or_default();
            let cable_area = get_cable_area(&element.name).await;
            let plugging_area = size_1 * size_2 - cable_area;
            let fill_percent = get_plugging_fill_percent().await;
            let plugging_volume = size_1 * size_2 * height * (1.0 - fill_percent);
            result.push(PluggingData {
                own_refno: Default::default(),
                refno: hole.refno,
                name: element.name,
                size: format!("{:.2}X{:.2}", size_1, size_2),
                room_1: "".to_string(),
                room_2: "".to_string(),
                height,
                cable_area,
                plugging_area,
                plugging_volume,
                materials: "".to_string(),
                
            })
        }
    }
    Ok(result)
}

/// 请求图为接⼝获取电缆占⽤⾯积
pub async fn get_cable_area(hole_name: &str) -> f64 {
    0.0
}

/// 获取孔洞填充率
pub async fn get_plugging_fill_percent() -> f64 {
    0.0
}

/// 判断四个点是否构成矩形，并返回长宽
pub fn compute_rectangle_data(points: [Vec3; 4]) -> Option<(f32, f32)> {
    let dot1 = (points[1] - points[0]).normalize().dot((points[2] - points[1]).normalize());
    let dot2 = (points[2] - points[1]).normalize().dot((points[3] - points[2]).normalize());
    if dot1.abs() > 0.001 && dot2.abs() > 0.001 {
        return None;
    }
    Some((points[2].distance(points[0]), points[1].distance(points[0])))
}

/// 生成查询pdms实体孔洞的aql语句
fn gen_query_pdms_hole_aql<'a>(refnos:Vec<RefU64>) -> AqlQuery<'a> {
    let ids = refnos.into_iter()
        .map(|refno| format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno()))
        .collect::<Vec<_>>();
    AqlQuery::new("
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    let gwall = (
    for v in 0..5 inbound id @@pdms_edges
        prune v.noun == 'GWALL'
        filter v.noun == 'GWALL'
        return v._id
    )
    let stwall = (
        for v in 0..5 inbound id @@pdms_edges
            prune v.noun == 'STWALL'
            filter v.noun == 'STWALL'
            return v._id
    )
    let wall = (
        for v in 0..5 inbound id @@pdms_edges
            prune v.noun == 'WALL'
            filter v.noun == 'WALL'
            return v._id
    )
    let pane = (
        for v in 0..5 inbound id @@pdms_edges
            prune v.noun == 'PANE'
            filter v.noun == 'PANE'
            return v._id
    )
    let floor = (
        for v in 0..5 inbound id @@pdms_edges
            prune v.noun == 'FLOOR'
            filter v.noun == 'FLOOR'
            return v._id
    )
    let gwall_children = (
    for g in gwall
       for v in 0..3 inbound g @@pdms_edges
            filter v.noun in ['PFIT','NXTR']
            return {
                '_key':v._key,
                'owner':v.owner,
                'name':v.name,
                'noun':v.noun,
                'version':0,
                'children_count':0,
            } )
    let stwall_children = (
        for s in stwall
        for v in 0..3 inbound s @@pdms_edges
            filter v.noun in ['FITT','CMFI']
            return {
                '_key':v._key,
                'owner':v.owner,
                'name':v.name,
                'noun':v.noun,
                'version':0,
                'children_count':0,
            } )
    let wall_children = (
        for w in wall
        for v in 0..3 inbound w @@pdms_edges
            filter v.noun == ['JLDATU']
            return {
                '_key':v._key,
                'owner':v.owner,
                'name':v.name,
                'noun':v.noun,
                'version':0,
                'children_count':0,
            } )
    let pane_children = (
        for f in floor
        for v in 0..3 inbound f @@pdms_edges
            filter v.noun == ['NXTR']
            return {
                '_key':v._key,
                'owner':v.owner,
                'name':v.name,
                'noun':v.noun,
                'version':0,
                'children_count':0,
            } )
    let floor_children = (
        for f in floor
        for v in 0..3 inbound f @@pdms_edges
            filter v.noun == ['NXTR']
            return {
                '_key':v._key,
                'owner':v.owner,
                'name':v.name,
                'noun':v.noun,
                'version':0,
                'children_count':0,
        } )
    let inter = INTERLEAVE(gwall_children,stwall_children,wall_children,floor_children)
    for i in inter
    return i")
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
        .bind_var("ids", ids)
}

#[tokio::test]
async fn test_query_hole_elements() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    // let mgr = get_test_ams_db_manager_async().await;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_refno_str("17496/106258").unwrap();
    let result = get_plugging_material_datas(vec![refno], &database).await?;
    dbg!(&result);
    Ok(())
}

#[test]
fn test_fn() {
    let p1 = Vec3::from_array([0.0, -0.001, 4050.0]);
    let p2 = Vec3::from_array([0.0, 799.999, 4050.0]);
    let p3 = Vec3::from_array([1900.0, 799.999, 4050.0]);
    let p4 = Vec3::from_array([1900.0, -0.001, 4050.0]);
    let dot1 = (p2 - p1).normalize().dot((p3 - p2).normalize());
    let dot2 = (p3 - p2).normalize().dot((p4 - p3).normalize());
    dbg!(&dot1);
    dbg!(&dot2);
}

#[test]
fn test_f32() {
    let result_f32: f32 = 1796.0686 * 70.0 * 70.0 * (1.0 - 0.0);
    dbg!(&result_f32);

    let result_f64: f64 = 1796.0686 * 70.0 * 70.0 * (1.0 - 0.0);
    dbg!(&result_f64);
}