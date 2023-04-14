use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::pdms_types::{EleTreeNode, PdmsElement, RefU64};
use aios_core::plot_struct::hanger::*;
use arangors_lite::{AqlQuery, Database};
use arangors_lite::collection::CollectionType::{Document, Edge};
use calamine::{Error, open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use glam::{Vec2, Vec3};
use nom::Parser;
use geo::Area;
use geo::LineString;
use parse_pdms_db::parse_explict_tools::times_keep_f32_two_decimal_place;
// use sea_orm::sea_query::IndexType::Hash;
use sqlx::{MySql, Pool, Row};
use crate::api::children::{travel_children_eles, travel_children_for_elenode, travel_children_with_type};
use crate::aql_api::children::{query_travel_children_aql, query_travel_children_with_type_aql};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use serde::{Deserialize, Serialize};
use crate::api::attr::{query_explicit_attr, query_implicit_attr};
use crate::api::element::query_name;
use crate::api::ssc_data::travel_ssc_children;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{create_arangodb_conn, get_arangodb_conn_from_db_option, save_arangodb_with_db_option};
use crate::options::DbOption;
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::data_interface::interface::PdmsDataInterface;

/// 提前将支吊架出图需要的数据存储在图数据库中
pub async fn save_hangers_data(mgr: Arc<AiosDBManager>) -> anyhow::Result<Option<HangerData>> {
    let project_map = &mgr.project_map;
    let pool = project_map.get(&mgr.db_option.project_name);
    if pool.is_none() { return Ok(None); }
    let pool = pool.unwrap();
    let database = &mgr.get_arangodb_conn().await?;
    let atta_name = "R320.060"; // 先拿这一个做测试
    let pipe_size_map = read_pipe_size_excel()?;
    let (hanger_map, atta_refnos) = get_all_hangers_with_atta(atta_name, pool.value()).await?;
    // 存储管道的数据
    let mut pipe_datas = vec![];
    let mut refnos = vec![];
    let mut bran_refnos = vec![];
    for (mut atta_name, atta_refno) in atta_refnos {
        let bran = mgr.get_owner(atta_refno);
        let bran_name = query_name(bran, pool.value()).await?;
        bran_refnos.push((atta_name.clone(), bran));
        // 获取 bran_name 按 "-" 分割的前两个
        let bran_name_split = bran_name.split('-').map(|x| x.to_string()).collect::<Vec<_>>();
        if bran_name_split.len() < 3 { continue; }
        let bran_name_first = &bran_name_split[0];
        let bran_name_second = &bran_name_split[1];
        let item_code = &bran_name_split[2];
        if item_code.len() < 3 { continue; }
        let pipe_size = pipe_size_map.get(bran_name_second);
        if pipe_size.is_none() { continue; }
        let number = format!("{}-{}", bran_name_first, bran_name_second);
        let item_code = item_code[2..3].to_string();
        let item_code = match_item_code(&item_code);
        // 获取 atta name 的 最后一位
        let mark = atta_name.split_off(atta_name.len() - 1);
        let elevation = mgr.get_world_transform(atta_refno).await?;
        if elevation.is_none() { continue; }
        let elevation = elevation.unwrap().translation.z as i32;
        pipe_datas.push(HangerPipeData {
            mark,
            number,
            elevation,
            item_code,
        })
    }
    let rest_refno = hanger_map.get("REST");
    let stru_refno = hanger_map.get("STRU");
    if rest_refno.is_none() || stru_refno.is_none() { return Ok(None); }
    let rest_refno = rest_refno.unwrap();
    let stru_refno = stru_refno.unwrap();

    let pcla_refnos = query_travel_children_aql(database, *rest_refno.value()).await?;
    for pcla_refno in &pcla_refnos {
        let refno = RefU64::from_refno_str(&pcla_refno.refno);
        if refno.is_err() { continue; }
        refnos.push(refno.unwrap());
    }
    // 查找 pcla 的 数据
    let pcla_data = get_pcla_data(pcla_refnos, database).await?;

    // 查找 stru下的所有参考号
    let stru_children = query_travel_children_aql(database, *stru_refno.value()).await?;
    for stru_child in &stru_children {
        let refno = RefU64::from_refno_str(&stru_child.refno);
        if refno.is_err() { continue; }
        refnos.push(refno.unwrap());
    }
    // 统计 sctn的数据
    let sctn_datas = get_sctn_data(&stru_children, database, pool.value()).await?;
    // 统计 pfit 的 数据
    let pfit_datas = get_pfit_data(&stru_children, database).await?;
    // 统计 pave 的数据
    let pave_datas = get_pane_data(&stru_children, database, pool.value()).await?;
    let hangers_data = HangerData {
        _key: atta_name.to_string(),
        refnos,
        bran_refno: bran_refnos,
        pipe_datas,
        pcla_datas: pcla_data,
        sctn_datas,
        pfit_datas,
        pave_datas,
    };
    Ok(Some(hangers_data))
}

/// 根据atta的名字获取所有的支吊架
async fn get_all_hangers_with_atta(atta_name: &str, pool: &Pool<MySql>) -> anyhow::Result<(DashMap<String, RefU64>, DashMap<String, RefU64>)> {
    // 找到 atta的名称对应的支吊架 STRU 和 REST 两种
    let mut hangers_map: DashMap<String, RefU64> = DashMap::new(); // key -> atta_name , value ->  map : key att_type(REST/STRU) value:refno
    let mut atta_map: DashMap<String, RefU64> = DashMap::new(); // key : atta 的 name , value ： atta 的 refno
    let sql = gen_query_stru_and_rest_with_atta_name_sql(atta_name);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    if results.is_err() { return Ok((hangers_map, atta_map)); }
    let results = results.unwrap();
    for result in results {
        let refno = RefU64(result.get::<i64, _>("ID") as u64);
        let att_type = result.get::<String, _>("TYPE");
        let name = result.get::<String, _>("NAME");
        if att_type == "STRU" || att_type == "REST" {
            hangers_map.entry(att_type).or_insert(refno);
        } else if att_type == "ATTA" {
            atta_map.entry(name).or_insert(refno);
        }
    }
    Ok((hangers_map, atta_map))
}

/// 获取需要的 pcla 的数据
async fn get_pcla_data(pcla_refnos: Vec<PdmsElement>, database: &Database) -> anyhow::Result<Vec<HangerPclaData>> {
    let mut pcla_datas = vec![];
    let mut pcla_map = HashMap::new(); // pcla 只记录 spre的 name 和 相同 spre的数量
    for pcla_refno in pcla_refnos {
        if pcla_refno.noun != "PCLA" { continue; }
        let refno = RefU64::from_refno_str(&pcla_refno.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let spre_name = query_foreign_name_aql(refno, vec!["SPRE", "SPRE"], database).await?;
        if spre_name.is_none() { continue; }
        let spre_name = spre_name.unwrap();
        let spre_collections = spre_name.split('/').map(|x| x.to_string()).collect::<Vec<_>>();
        if spre_collections.len() < 2 { continue; }
        let spre_name = format!("{}{}", spre_collections[spre_collections.len() - 2], spre_collections[spre_collections.len() - 1]);
        let count = pcla_map.entry(spre_name).or_insert(0);
        *count += 1;
    }
    for pcla_data in pcla_map {
        pcla_datas.push(HangerPclaData {
            spre_name: pcla_data.0,
            count: pcla_data.1,
            unit_weight: 0,
            total_weight: 0,
        });
    }
    Ok(pcla_datas)
}

/// 获取 sctn 的数据
async fn get_sctn_data(stru_children: &Vec<PdmsElement>, database: &Database, pool: &Pool<MySql>) -> anyhow::Result<Vec<HangerSctnData>> {
    let mut result = vec![];
    let mut sctn_map = HashMap::new();
    for child in stru_children {
        if child.noun != "SCTN" { continue; }
        let refno = RefU64::from_refno_str(&child.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let spre_name = query_foreign_name_aql(refno, vec!["SPRE", "SPRE"], database).await?;
        if spre_name.is_none() { continue; }
        let spre_name = spre_name.unwrap();
        let mut spre_name = spre_name.split('/').map(|x| x.to_string()).collect::<Vec<_>>();
        if spre_name.len() == 0 { continue; }
        // 获取截面积
        let across_section = spre_name.remove(spre_name.len() - 1);
        // 获取长度
        let cache_basic = CachedRefBasic { owner: child.owner, table: child.noun.clone() };
        let implicit_data = query_implicit_attr(refno, &cache_basic, pool, Some(vec!["POSS", "POSE"])).await?;
        let poss = implicit_data.get_vec3("POSS");
        let pose = implicit_data.get_vec3("POSE");
        if pose.is_none() || poss.is_none() { continue; }
        let poss = poss.unwrap();
        let pose = pose.unwrap();
        let poss = Vec3::new(poss.x, poss.y, poss.z);
        let pose = Vec3::new(pose.x, pose.y, pose.z);
        let distance = pose.distance(poss) as i32;
        // 统计个数
        let mut count = sctn_map.entry((across_section, distance)).or_insert(0);
        *count += 1;
    }
    for ((across_section, distance), count) in sctn_map.into_iter() {
        result.push(HangerSctnData {
            across_section: across_section.to_string(),
            length: distance,
            count,
            unit_weight: 0,
            total_weight: 0,
        })
    }
    Ok(result)
}

/// 获取 pfit 的数据
async fn get_pfit_data(stru_children: &Vec<PdmsElement>, database: &Database) -> anyhow::Result<Vec<HangerPfitData>> {
    let mut result = Vec::new();
    let mut pfit_map = HashMap::new();
    for stru_child in stru_children {
        if stru_child.noun != "PFIT" { continue; }
        let refno = RefU64::from_refno_str(&stru_child.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        let spre_name = query_foreign_name_aql(refno, vec!["SPRE", "SPRE"], database).await?;
        if spre_name.is_none() { continue; }
        let spre_name = spre_name.unwrap();
        // 只要 spre_name 按 “/”分割的最后一段数据
        let mut spre_names = spre_name.split('/').map(|x| x.to_string()).collect::<Vec<_>>();
        if spre_names.is_empty() { continue; }
        let mut count = pfit_map.entry(spre_names.remove(spre_names.len() - 1)).or_insert(0);
        *count += 1;
    }
    for (spre_name, count) in pfit_map.into_iter() {
        result.push(HangerPfitData {
            spre_name,
            count,
        })
    }
    Ok(result)
}

/// 获取 pane 的数据
async fn get_pane_data(stru_children: &Vec<PdmsElement>, database: &Database, pool: &Pool<MySql>) -> anyhow::Result<Vec<HangerPaneData>> {
    let mut result = vec![];
    let mut pane_map = HashMap::new();
    for stru_child in stru_children {
        if stru_child.noun != "PANE" { continue; }
        let refno = RefU64::from_refno_str(&stru_child.refno);
        if refno.is_err() { continue; }
        let refno = refno.unwrap();
        // 获取 func 的属性
        let stru_attr = query_explicit_attr(refno, pool).await?;
        let func_name = stru_attr.get_str("FUNC");
        if func_name.is_none() { continue; }
        let mut func_name = func_name.unwrap().split('.').map(|x| x.to_string()).collect::<Vec<_>>();
        if func_name.len() < 1 { continue; }
        let func_name = func_name.remove(func_name.len() - 1);
        // 获取 pane 下的 所有子节点
        let pane_children = travel_children_for_elenode(refno, pool).await?;
        let mut pave_positions = vec![];
        let mut heig = 0.0;
        for child in pane_children {
            // 获取 ploo 的 heig
            if child.noun == "PLOO" {
                let ploo_refno = child.refno;
                let ref_basic = CachedRefBasic { owner: child.owner, table: child.noun.to_string() };
                let implicit_attr = query_implicit_attr(ploo_refno, &ref_basic, pool, Some(vec!["HEIG"])).await?;
                let heig_opt = implicit_attr.get_f64("HEIG");
                if heig_opt.is_none() { continue; }
                heig = heig_opt.unwrap() as f32;
            }
            // 获取 pave 的 pos
            if child.noun == "PAVE" {
                // let pave_refno = RefU64::from_refno_str(&child.refno);
                let pave_refno = child.refno;
                // if pave_refno.is_err() { continue; }
                // let pave_refno = pave_refno.unwrap();
                let ref_basic = CachedRefBasic { owner: child.owner, table: child.noun.to_string() };
                let implicit_attr = query_implicit_attr(pave_refno, &ref_basic, pool, Some(vec!["POS"])).await?;
                let pos = implicit_attr.get_vec3("POS");
                if pos.is_none() { continue; }
                let pos = pos.unwrap();
                pave_positions.push((pos.x, pos.y));
            }
        }
        // 计算 pane 面积
        dbg!(&pave_positions);
        let polygon = geo::geometry::Polygon::new(
            LineString::from(pave_positions),
            vec![],
        );
        let area = polygon.unsigned_area();
        // 计算体积
        let volume = area * heig;
        // 计算单重  单重 (kg) = 体积 * 7850kg/m³
        let unit_weight = (volume * 0.00000785 * 100.0) as u32; // * 100 变为 u64 进行 hash 然后 再 /100
        let count = pane_map.entry((func_name, unit_weight)).or_insert(0);
        *count += 1;
    }
    for ((func_name, unit_weight), count) in pane_map {
        let unit_weight = (unit_weight as f32 / 100.0 * 10.0).trunc() / 10.0; // 保留 1位 小数 ，
        result.push(HangerPaneData {
            func_name,
            count,
            unit_weight,
            total_weight: unit_weight * (count as f32),
        })
    }
    Ok(result)
}

fn gen_query_stru_and_rest_with_atta_name_sql(atta_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,TYPE,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE NAME LIKE '%{}%' AND TYPE IN ( 'STRU','REST','ATTA')", atta_name));
    sql
}

/// 从图数据库获取单个hanger需要的数据
pub async fn query_hangers_element(atta_name: &str, database: &Database) -> anyhow::Result<Vec<HangerData>> {
    let aql = AqlQuery::new("\
        return document('hanger_data',@name)
    ").bind_var("name", atta_name);
    let result: Vec<HangerData> = database.aql_query(aql).await?;
    Ok(result)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PipeSize {
    寸: Option<String>,
    mm: Option<String>,
}

fn read_pipe_size_excel() -> anyhow::Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/管道单位转换.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(Error::Msg("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let value: PipeSize = result?;
        if value.寸.is_some() && value.mm.is_some() {
            map.entry(value.mm.unwrap()).or_insert(value.寸.unwrap());
        }
    }
    Ok(map)
}

/// 返回对应的物像编码
fn match_item_code(item_code: &str) -> Option<String> {
    match item_code {
        "B" => { return Some("1级".to_string()); }
        "C" => { return Some("2级".to_string()); }
        "D" => { return Some("3级".to_string()); }
        "S" => { return Some("NA级".to_string()); }
        &_ => {}
    }
    None
}
