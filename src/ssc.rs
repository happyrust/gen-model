use crate::api::children::*;
use crate::api::element::*;
use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::api::ssc_data::*;
use crate::aql_api::children::{
    query_refnos_ancestor_with_name_till_type_aql, query_travel_children_aql,
};
use crate::aql_api::pdms_room::{query_all_room_aql, query_room_refno_from_room_refno_aql};
use crate::aql_api::PdmsOwnerNameAql;
use crate::arangodb::ArDatabase;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::graph_db::structs::PdmsEleData;
use crate::metadata::convert_str_to_hash;
use crate::tables;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;
use aios_core::aql_types::AqlEdge;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use anyhow::anyhow;
use arangors_lite::collection::CollectionType::Document;
use arangors_lite::collection::CollectionType::*;

use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::{DashMap, DashSet};
use nom::character::complete::u32;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use serde_with::DisplayFromStr;
use sqlx::Executor;
use sqlx::{Acquire, MySql, Pool, Row};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::File;
use std::hash::Hash;
use std::io::Write;
use std::sync::Arc;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SiteExcelData {
    pub code: Option<String>,
    pub name: Option<String>,
    pub att_type: Option<String>,
    pub site_pdms_name: Option<String>,
    pub zone_code: Option<String>,
    pub zone_name: Option<String>,
    pub zone_att_type: Option<String>,
    pub zone_pdms_name: Option<String>,
}

/// pdms site 和 zone name 对应的专业代码
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PdmsSscMajorCode {
    /// pdms site 的 name
    pub site_name: String,
    /// 专业代码
    pub site_code: String,
    /// site 下 zone name 对应的 专业代码
    pub zone_map: HashMap<String, String>,
}

impl SiteExcelData {
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.code.is_some() && self.name.is_some() && self.att_type.is_some()
    }
}

/// 房间信息 excel 字段
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RoomExcelData {
    ///房间代码
    #[serde(rename = "房间代码")]
    pub room_code: Option<String>,
    /// 所属机组
    #[serde(rename = "所属机组")]
    pub aff_unit: Option<String>,
    ///安装厂房
    #[serde(rename = "安装厂房")]
    pub install_plant: Option<String>,
    ///区域
    #[serde(rename = "区域")]
    pub zone: Option<String>,
    ///安装层位
    #[serde(rename = "安装层位")]
    pub install_level: Option<String>,
    ///厂房
    #[serde(rename = "厂房")]
    pub plant: Option<String>,
    ///分区
    #[serde(rename = "分区")]
    pub partion: Option<String>,
    ///层位及标高
    #[serde(rename = "层位及标高")]
    pub layer_elevation: Option<String>,
    /// 序号
    #[serde(rename = "序号")]
    pub number: Option<u32>,
}

pub async fn async_total_ssc_data(
    project_pool: &Pool<MySql>,
    mgr: Arc<AiosDBManager>,
) -> anyhow::Result<()> {
    let mut conn = project_pool;
    // 创建 ssc 表
    let result = conn
        .execute(tables::gen_create_ssc_element_tables_sql().as_str())
        .await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    dbg!("创建SSC表完成");
    let room_data =
        query_all_room_data_aql(&mgr.get_arango_db().await?, project_pool, &mgr.db_option).await?;
    let room_info = deal_room_info(room_data.clone());
    let (zone_level_map, zone_name_map, next_refno) =
        insert_set_ssc_node_sql(room_info.clone(), project_pool).await?;
    dbg!("SSC固定节点生成");
    replace_ssc_room_refno(room_info, project_pool).await?;
    if room_data.len() != 0 {
        let insert_sql = format!("INSERT IGNORE INTO {PDMS_SSC_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, REAL_PDMS_REFNO,ORDER_NUM) VALUES ");
        let sqls = insert_ssc_room_node(
            room_data,
            zone_level_map,
            zone_name_map,
            next_refno,
            project_pool,
            mgr,
        )
        .await?;
        if sqls.len() != 0 {
            for (idx, sql) in sqls.into_iter().enumerate() {
                let sql = format!("{} {}", insert_sql, sql);
                let result = conn.execute(sql.as_str()).await;
                match result {
                    Ok(_) => {
                        println!("第 {} 条 sql 保存完成", idx);
                    }
                    Err(e) => {
                        dbg!(sql);
                        dbg!(&e);
                    }
                }
            }
        }
    }

    Ok(())
}

pub async fn async_total_ssc_data_refactor(mgr: &AiosDBManager) -> anyhow::Result<()> {
    let database = mgr.get_arango_db().await?;
    // 创建图数据库连接
    create_arango_document(&database, AQL_SSC_EDGE_COLLECTION, Edge).await?;
    create_arango_document(&database, AQL_SSC_ELES_COLLECTION, Document).await?;
    Ok(())
}

/// 将ssc房间对应的参考号修改为pdms房间的参考号
pub async fn replace_ssc_room_refno(
    room_info: HashMap<String, RefU64>,
    pool: &Pool<MySql>,
) -> anyhow::Result<()> {
    // 找到 ssc 当前所有房间的参考号
    let room_refnos = query_ssc_room_refnos(&room_info, pool).await?;
    let room_refnos_len = room_refnos.len();
    let mut handles = vec![];
    for (idx, (room_name, (old_refno, new_refno))) in room_refnos.into_iter().enumerate() {
        let pool_clone = pool.clone();
        let handle = tokio::spawn(async move {
            let mut conn = pool_clone.acquire().await.unwrap();
            let replace_sql = gen_replace_room_refno_sql(&room_name, new_refno, old_refno);
            conn.execute(replace_sql.as_str()).await.unwrap();
            println!("正在保存第{}条数据,一共{}条", idx, room_refnos_len);
        });
        handles.push(handle);
    }
    futures::future::join_all(&mut handles).await;
    Ok(())
}

/// 解析 excel 表单， 获取房间下面的ZONE和SITE层级  返回值  1 : key : site 的 name (中文名) value : site 下对应的zone 的 name ;
/// 2 : 英文 code 对应的中文名
fn get_room_level_from_excel(
) -> anyhow::Result<(Vec<(String, Vec<String>)>, DashMap<String, String>)> {
    let mut level: Vec<(String, Vec<String>)> = vec![];
    let mut name_map = DashMap::new();
    let mut pdms_zone_name_map = HashMap::new();
    let mut pdms_ssc_major_codes = Vec::new();

    let mut workbook: Xlsx<_> = open_workbook("resource/专业分类.xlsx")?;
    dbg!("加载专业分类.xlsx 成功");
    let range = workbook
        .worksheet_range("Sheet2")
        .ok_or(anyhow::anyhow!("Cannot find 'Sheet1'"))??;
    dbg!("打开Sheet2成功");

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    let mut zone_name = "".to_string();
    let mut zone_code = "".to_string();
    let mut b_first = true;
    let mut zones = vec![];
    while let Some(result) = iter.next() {
        let v: SiteExcelData = result?;
        // site 的 name 、code 、att_type
        if v.code.is_some() && v.name.is_some() && v.att_type.is_some() {
            let read_site_code = v.code.clone().unwrap(); // 从 excel 文件中读取的 site name
                                                          // 当zone_code和当前读取的值不相等时，就代表不是同一个层级了 （第一次除外,所以加了个b_first 排除第一次的情况）
            if zone_code != read_site_code && !b_first {
                level.push((zone_name.clone(), zones.clone()));
                zones.clear();
            }
            if zone_code != read_site_code {
                if v.site_pdms_name.is_some() {
                    pdms_ssc_major_codes.push(PdmsSscMajorCode {
                        site_name: v.site_pdms_name.unwrap(),
                        site_code: read_site_code.clone(),
                        zone_map: pdms_zone_name_map.clone(),
                    });
                    pdms_zone_name_map.clear();
                }
            }

            let read_site_name = v.name.unwrap();
            zone_name = read_site_name.clone();
            zone_code = read_site_code.clone();

            name_map.insert(read_site_code, read_site_name);
            b_first = false;
        }
        // 存放 site 下的子节点
        if v.zone_name.is_some() && v.zone_code.is_some() {
            let read_zone_name = v.zone_name.unwrap();
            let read_zone_code = v.zone_code.clone().unwrap();

            zones.push(read_zone_name.clone());
            name_map.insert(read_zone_code.clone(), read_zone_name);
            // 存放 pdms的site zone name 对应的 专业代码
            if v.zone_pdms_name.is_some() {
                pdms_zone_name_map
                    .entry(read_zone_code)
                    .or_insert(v.zone_pdms_name.unwrap());
            }
        }
    }
    // 查询结束时 还需要剩最后一条数据没插入
    level.push((zone_name.clone(), zones.clone()));
    Ok((level, name_map))
}

/// ssc专业配置excel表 返回的对应数据
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SscMajorCodeExcel {
    /// key : site 的 name (中文名) value : site 下对应的zone 的 name
    pub level: Vec<(String, Vec<String>)>,
    /// 英文 code 对应的中文名
    pub name_map: DashMap<String, String>,
    /// pdms中 site 和 zone name 对应的专业代码
    pub pdms_name_code_map: Vec<PdmsSscMajorCode>,
}

/// 读取 专业分类 excel表 ，返回需要的值
pub fn get_room_level_from_excel_refactor() -> anyhow::Result<SscMajorCodeExcel> {
    let mut level: Vec<(String, Vec<String>)> = Vec::new();
    let mut name_map = DashMap::new();
    let mut pdms_zone_name_map = HashMap::new();
    let mut pdms_ssc_major_codes = Vec::new();

    let mut workbook: Xlsx<_> = open_workbook("resource/专业分类.xlsx")?;
    dbg!("加载专业分类.xlsx 成功");
    let range = workbook
        .worksheet_range("Sheet2")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;
    dbg!("打开Sheet2成功");

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    let mut b_first = true;
    let mut site_code = "".to_string();
    let mut site_chinese_name = "".to_string();
    let mut pdms_site_name = "".to_string();
    let mut zones = Vec::new();
    while let Some(result) = iter.next() {
        let v: SiteExcelData = result?;
        // site 的 name 、code 、att_type
        if v.code.is_some()
            && v.name.is_some()
            && v.att_type.is_some()
            && v.site_pdms_name.is_some()
        {
            let read_site_code = v.code.unwrap();
            let read_site_chinese_name = v.name.unwrap();
            let read_pdms_site_name = v.site_pdms_name.unwrap();
            // code != site_code 代表是下一个site的数据了 , b_first 防止第一个判断就是 != 会导致读取的数据错开，第一个site没值
            if read_site_code != site_code && !b_first {
                pdms_ssc_major_codes.push(PdmsSscMajorCode {
                    site_name: pdms_site_name.clone(),
                    site_code: site_code.clone(),
                    zone_map: pdms_zone_name_map.clone(),
                });
                pdms_zone_name_map.clear();

                level.push((site_code, zones.clone()));
                zones.clear();
            }
            b_first = false;
            site_code = read_site_code.clone();
            site_chinese_name = read_site_chinese_name.clone();
            pdms_site_name = read_pdms_site_name.clone();
            // 存储专业编码对应的中文名称
            name_map.insert(read_site_code, site_chinese_name.clone());

            // 存放 site 下 zone 的专业代码
            if v.zone_name.is_some() && v.zone_code.is_some() {
                let read_zone_name = v.zone_name.unwrap();
                let read_zone_code = v.zone_code.unwrap();
                name_map.insert(read_zone_code.clone(), read_zone_name.clone());
                // 存放 pdms的site下 zone name 对应的 专业代码
                if v.zone_pdms_name.is_some() {
                    pdms_zone_name_map
                        .entry(v.zone_pdms_name.unwrap())
                        .or_insert(read_zone_code.clone());
                }
                zones.push(read_zone_code);
            }
        }
    }
    Ok(SscMajorCodeExcel {
        level,
        name_map,
        pdms_name_code_map: pdms_ssc_major_codes,
    })
}
/// 解析 excel 表单 ，找到每一层下面所有的房间号 返回所有的安装厂房下对应的层位，层位下对应的房间
pub fn parse_room_info_from_excel() -> anyhow::Result<HashMap<String, BTreeMap<i32, Vec<String>>>> {
    let mut r = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/ssc_room.xlsx")?;
    let range = workbook
        .worksheet_range("Sheet1")
        .ok_or(anyhow::anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        if let Some(install_workshop) = v.install_plant {
            if let Some(belong_unit) = v.aff_unit {
                let install_workshop = format!("{}{}", belong_unit.to_string(), install_workshop);
                if let Some(install_level) = v.install_level {
                    if let Some(workshop) = v.room_code {
                        r.entry(install_workshop)
                            .or_insert_with(BTreeMap::new)
                            .entry(install_level.parse().unwrap_or(1))
                            .or_insert_with(Vec::new)
                            .push(workshop);
                    }
                }
            }
        }
    }
    Ok(r)
}

/// 解析 excel 表单 ，找到每一层下面所有的房间号,并按树结构保存到图数据库中
pub async fn get_room_info_from_excel_refactor(database: &ArDatabase) -> anyhow::Result<()> {
    // 获取 pdms site 和 zone 对应的专业代码
    let pdms_level = get_room_level_from_excel_refactor()?;
    let mut r = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/ssc_room.xlsx")?;
    let range = workbook
        .worksheet_range("Sheet1")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        let Some(room_code) = v.room_code else {
            continue;
        };
        let Some(install_level) = v.install_level else {
            continue;
        };
        let Some(workshop) = v.plant else {
            continue;
        };
        r.entry(workshop)
            .or_insert_with(BTreeMap::new)
            .entry(install_level.parse().unwrap_or(1))
            .or_insert_with(Vec::new)
            .push(room_code);
    }
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (idx, (workshop, level_map)) in r.into_iter().enumerate() {
        // 解决厂房的排列
        let name_hash = convert_str_to_hash(&workshop);
        let owner = if workshop.starts_with("1") {
            convert_str_to_hash("一号机组")
        } else if workshop.starts_with("2") {
            convert_str_to_hash("二号机组")
        } else {
            convert_str_to_hash("双机组共用")
        };
        let refno = RefU64(name_hash);
        let owner = RefU64(owner);
        nodes.push(PdmsEleData {
            refno,
            owner,
            name: workshop.to_string(),
            noun: "SSC".to_string(),
            dbnum: 0,
            order: idx as u32,
            cata_hash: "".to_string(),
            // tag_lock:false,
        });
        edges.push(AqlEdge::new(
            refno,
            owner,
            AQL_SSC_ELES_COLLECTION,
            AQL_SSC_ELES_COLLECTION,
        ));
        for (idx, (level, rooms)) in level_map.into_iter().enumerate() {
            // 解决厂房下的层位
            let level_name_hash = convert_str_to_hash(format!("{}{}", &workshop, level).as_str());
            let Some(level_name) = match_level_name(level) else {
                continue;
            };
            let refno = RefU64(level_name_hash);
            let owner = RefU64(name_hash);
            let node = PdmsEleData {
                refno,
                owner,
                name: level_name.to_string(),
                noun: "SSC".to_string(),
                dbnum: 0,
                order: idx as u32,
                cata_hash: "".to_string(),
                // tag_lock:false,
            };
            nodes.push(node);
            edges.push(AqlEdge::new(
                refno,
                owner,
                AQL_SSC_ELES_COLLECTION,
                AQL_SSC_ELES_COLLECTION,
            ));
            for (idx, room) in rooms.into_iter().enumerate() {
                let room_name_hash = convert_str_to_hash(&room);
                let refno = RefU64(room_name_hash);
                let owner = RefU64(level_name_hash);
                edges.push(AqlEdge::new(
                    refno,
                    owner,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
                nodes.push(PdmsEleData {
                    refno,
                    owner,
                    name: room.to_string(),
                    noun: "SSC".to_string(),
                    dbnum: 0,
                    order: idx as u32,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                // 解决房间下的专业层级
                // 专业层级
                for (idx, (site, zones)) in pdms_level.level.iter().enumerate() {
                    let site_level_name_hash =
                        convert_str_to_hash(format!("{}{}", room, site).as_str());
                    let Some(site_name) = pdms_level.name_map.get(site) else {
                        continue;
                    };
                    let refno = RefU64(site_level_name_hash);
                    let owner = RefU64(room_name_hash);
                    edges.push(AqlEdge::new(
                        refno,
                        owner,
                        AQL_SSC_ELES_COLLECTION,
                        AQL_SSC_ELES_COLLECTION,
                    ));
                    nodes.push(PdmsEleData {
                        refno,
                        owner,
                        name: site_name.to_string(),
                        noun: "SSC".to_string(),
                        dbnum: 0,
                        order: idx as u32,
                        cata_hash: "".to_string(),
                        // tag_lock:false,
                    });
                    // 专业下具体细分
                    for (idx, zone) in zones.iter().enumerate() {
                        let zone_level_name_hash =
                            convert_str_to_hash(format!("{}{}", room, zone).as_str());
                        let refno = RefU64(zone_level_name_hash);
                        let owner = RefU64(site_level_name_hash);
                        let Some(zone_name) = pdms_level.name_map.get(zone) else {
                            continue;
                        };
                        edges.push(AqlEdge::new(
                            refno,
                            owner,
                            AQL_SSC_ELES_COLLECTION,
                            AQL_SSC_ELES_COLLECTION,
                        ));
                        nodes.push(PdmsEleData {
                            refno,
                            owner,
                            name: zone_name.to_string(),
                            noun: "SSC".to_string(),
                            dbnum: 0,
                            order: idx as u32,
                            cata_hash: "".to_string(),
                            // tag_lock:false,
                        });
                    }
                }
            }
        }
    }
    for nodes in nodes.chunks(ARANGODB_SAVE_AMOUNT) {
        let eles_value = serde_json::to_value(nodes)?;
        save_arangodb_doc(eles_value, AQL_SSC_ELES_COLLECTION, database, false).await?;
    }
    for edges in edges.chunks(ARANGODB_SAVE_AMOUNT) {
        let edge_value = serde_json::to_value(edges)?;
        save_arangodb_doc(edge_value, AQL_SSC_EDGE_COLLECTION, database, false).await?;
    }
    Ok(())
}

pub fn get_rooms_from_excel() -> anyhow::Result<Vec<String>> {
    let mut r = vec![];
    let mut workbook: Xlsx<_> = open_workbook("../resource/ssc_room.xlsx")?;
    let range = workbook
        .worksheet_range("Sheet1")
        .ok_or(anyhow::anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        if let Some(workshop) = v.room_code {
            r.push(workshop);
        }
    }
    Ok(r)
}

/// 创建ssc固定节点
pub async fn insert_set_ssc_node_sql(
    room_info: HashMap<String, RefU64>,
    pool: &Pool<MySql>,
) -> anyhow::Result<(DashMap<String, RefU64>, DashMap<String, String>, RefU64)> {
    let insert_sql = format!("INSERT IGNORE INTO {PDMS_SSC_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, REAL_PDMS_REFNO,ORDER_NUM) VALUES ");
    let (sql, zone_level_map, zone_name_map, next_refno) = set_ssc_node()?;
    let sql = format!("{}{}", insert_sql, sql);
    let mut conn = pool;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    Ok((zone_level_map, zone_name_map, next_refno))
}

/// 保存房间下的元件
pub async fn insert_ssc_room_node(
    mut room_data: HashMap<RefU64, SscEleNode>,
    zone_level_map: DashMap<String, RefU64>,
    zone_name_map: DashMap<String, String>,
    mut next_refno: RefU64,
    pool: &Pool<MySql>,
    mgr: Arc<AiosDBManager>,
) -> anyhow::Result<Vec<String>> {
    // let mut handles = vec![];
    let mut sqls = Arc::new(DashSet::new());
    let mut under_zone_map = DashMap::new();
    // 工艺支架等特殊的层级  key: 专业下细分类名称 + 房间号.流水号 + "REST"/"STRU" value : fake_refno
    let mut special_under_zone_map: Arc<DashMap<String, RefU64>> = Arc::new(DashMap::new());
    let zone_name_map = Arc::new(zone_name_map);
    let zone_level_map = Arc::new(zone_level_map);
    let mut room_data_len = room_data.len();
    let mut undefined_zone_refno = DashSet::new();
    // 找到每个参考号的属于那个zone
    for (idx, (_room_refno, room_ori)) in room_data.iter().enumerate() {
        // let room_ori = room_ori.value();
        if room_ori.noun == "EQUI" {
            continue;
        }
        // 该房间号所在的zone没有对应的uda，直接跳过
        if undefined_zone_refno.contains(&room_ori.refno) {
            room_data_len -= 1;
            continue;
        }

        let zone_name_map = zone_name_map.clone();
        let zone_level_map = zone_level_map.clone();
        let special_under_zone_map_clone = special_under_zone_map.clone();
        let sqls_clone = sqls.clone();
        let pool = pool.clone();
        let room = Arc::new(room_ori).clone();

        // let handle = tokio::spawn(async move {
        let room_name = format!("1{}", room.room_code); // 默认都是 1号机组
                                                        // let room_name = room.room_code.to_string();
        if let Ok(mut zone_refnos) =
            query_ancestor_refnos_till_type(room.refno, "ZONE", &pool).await
        {
            // 想拿到 zone的参考号
            if let Some(zone_refno) = zone_refnos.pop() {
                let mut divco = get_zone_divco(zone_refno, &pool).await;
                if divco == "" {
                    divco = "PIPEP".to_string()
                }
                if divco != "" {
                    // 找到专业属性对应的中文名称
                    if let Some(divco_name) = zone_name_map.get(&divco) {
                        let divco_name = divco_name.trim();
                        let room_divco_name = format!("{}_{}", room_name, divco_name);
                        dbg!(&room_divco_name);
                        // 一个房间下只有一个专业的子类，所以直接通过name获取参考号
                        if let Some(zone_level_refno) = zone_level_map.get(&room_divco_name) {
                            // 找到 pdms 树 zone 下的层级放到ssc下面
                            if let Some(pdms_under_zone_refno) = zone_refnos.pop() {
                                // 特殊处理 将zone下的节点拆成两层，房间号+流水号 和 type名
                                if divco_name == "工艺支架"
                                    || divco_name == "仪表架"
                                    || divco_name == "仪表管支吊架"
                                {
                                    if let Ok(pdms_under_zone_ele) =
                                        query_ele_node(pdms_under_zone_refno, &pool).await
                                    {
                                        // zone下面 不为 STRU 和 REST 的直接跳过
                                        if pdms_under_zone_ele.noun != "STRU"
                                            && pdms_under_zone_ele.noun != "REST"
                                        {
                                            continue;
                                        }
                                        // 找到 name 中房间号+流水号
                                        if let Some(room_serial_number) =
                                            pdms_under_zone_ele.name.find('.')
                                        {
                                            let room_serial_name = pdms_under_zone_ele.name
                                                [room_serial_number - 4..room_serial_number + 4]
                                                .to_string();
                                            if let Some(special_refno) =
                                                special_under_zone_map_clone.get(&format!(
                                                    "{}_{}_{}",
                                                    divco,
                                                    room_serial_name,
                                                    pdms_under_zone_ele.noun
                                                ))
                                            {
                                                // 房间层级
                                                let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                    room.refno,
                                                    &room.noun,
                                                    *special_refno.value(),
                                                    &room.name,
                                                    room.refno,
                                                    0,
                                                );
                                                sqls_clone.insert(insert_sql);
                                            } else {
                                                // 房间号+流水号层级
                                                let (next_refno_n, insert_sql) =
                                                    gen_insert_ssc_node_sql(
                                                        next_refno,
                                                        "SSC",
                                                        *zone_level_refno,
                                                        &room_serial_name,
                                                        RefU64(0),
                                                        0,
                                                    );
                                                // 房间号 + 流水号 参考号 ，STRU 和 REST 的 owner
                                                let room_level_refno = next_refno;
                                                sqls_clone.insert(insert_sql);
                                                next_refno = next_refno_n;
                                                // STRU/REST层级 直接给两个默认的
                                                let (next_refno_n, insert_sql) =
                                                    gen_insert_ssc_node_sql(
                                                        next_refno,
                                                        "STRU",
                                                        room_level_refno,
                                                        "STRU",
                                                        RefU64(0),
                                                        0,
                                                    );
                                                sqls_clone.insert(insert_sql);
                                                special_under_zone_map_clone.insert(
                                                    format!(
                                                        "{}_{}_{}",
                                                        divco, room_serial_name, "STRU"
                                                    ),
                                                    next_refno,
                                                );
                                                let special_stru_refno = next_refno;
                                                next_refno = next_refno_n;

                                                let (next_refno_n, insert_sql) =
                                                    gen_insert_ssc_node_sql(
                                                        next_refno,
                                                        "REST",
                                                        room_level_refno,
                                                        "REST",
                                                        RefU64(0),
                                                        0,
                                                    );
                                                sqls_clone.insert(insert_sql);
                                                special_under_zone_map_clone.insert(
                                                    format!(
                                                        "{}_{}_{}",
                                                        divco, room_serial_name, "REST"
                                                    ),
                                                    next_refno,
                                                );
                                                let special_rest_refno = next_refno;
                                                next_refno = next_refno_n;

                                                // 房间层级
                                                if pdms_under_zone_ele.noun == "STRU" {
                                                    let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                        room.refno,
                                                        &room.noun,
                                                        special_stru_refno,
                                                        &room.name,
                                                        room.refno,
                                                        0,
                                                    );
                                                    sqls_clone.insert(insert_sql);
                                                } else if pdms_under_zone_ele.noun == "REST" {
                                                    let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                        room.refno,
                                                        &room.noun,
                                                        special_rest_refno,
                                                        &room.name,
                                                        room.refno,
                                                        0,
                                                    );
                                                    sqls_clone.insert(insert_sql);
                                                }
                                            }
                                        }
                                    }
                                } else if divco_name.contains("支架") || divco_name.contains("设备")
                                {
                                    if let Some(under_zone_refno) = under_zone_map.get(&format!(
                                        "{}_{}",
                                        pdms_under_zone_refno.0,
                                        room.room_code.clone()
                                    )) {
                                        let (_, insert_sql) = gen_insert_ssc_node_sql(
                                            room.refno,
                                            &room.noun,
                                            *under_zone_refno,
                                            &room.name,
                                            room.refno,
                                            0,
                                        );
                                        sqls_clone.insert(insert_sql);
                                    } else {
                                        if let Ok(pdms_under_zone_ele) =
                                            query_ele_node(pdms_under_zone_refno, &pool).await
                                        {
                                            let (next_refno_n, insert_sql) =
                                                gen_insert_ssc_node_sql(
                                                    next_refno,
                                                    &pdms_under_zone_ele.noun,
                                                    *zone_level_refno,
                                                    &pdms_under_zone_ele.name,
                                                    RefU64(0),
                                                    0,
                                                );
                                            sqls_clone.insert(insert_sql);
                                            under_zone_map.insert(
                                                format!(
                                                    "{}_{}",
                                                    pdms_under_zone_refno.0,
                                                    room.room_code.clone()
                                                ),
                                                next_refno,
                                            );

                                            let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                room.refno, &room.noun, next_refno, &room.name,
                                                room.refno, 0,
                                            );
                                            sqls_clone.insert(insert_sql);
                                            next_refno = next_refno_n;
                                        }
                                    }
                                } else {
                                    if let Some(pdms_under_bran_refno) = zone_refnos.pop() {
                                        if let Some(under_bran_refno) =
                                            under_zone_map.get(&format!(
                                                "{}_{}",
                                                pdms_under_bran_refno.0,
                                                room.room_code.clone()
                                            ))
                                        {
                                            let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                room.refno,
                                                &room.noun,
                                                *under_bran_refno,
                                                &room.name,
                                                room.refno,
                                                0,
                                            );
                                            sqls_clone.insert(insert_sql);
                                        } else {
                                            if let Ok(pdms_under_bran_ele) =
                                                query_ele_node(pdms_under_bran_refno, &pool).await
                                            {
                                                let (next_refno_n, insert_sql) =
                                                    gen_insert_ssc_node_sql(
                                                        next_refno,
                                                        &pdms_under_bran_ele.noun,
                                                        *zone_level_refno,
                                                        &pdms_under_bran_ele.name,
                                                        RefU64(0),
                                                        0,
                                                    );
                                                sqls_clone.insert(insert_sql);
                                                under_zone_map.insert(
                                                    format!(
                                                        "{}_{}",
                                                        pdms_under_bran_refno.0,
                                                        room.room_code.clone()
                                                    ),
                                                    next_refno,
                                                );
                                                let (_, insert_sql) = gen_insert_ssc_node_sql(
                                                    room.refno, &room.noun, next_refno, &room.name,
                                                    room.refno, 0,
                                                );
                                                sqls_clone.insert(insert_sql);
                                                next_refno = next_refno_n;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // 如果发现该zone下 :CNPE_divco 没有值，直接把整个zone下的refno全部移除
                    let database = mgr.get_arango_db().await?;
                    if let Ok(children) = query_travel_children_aql(&database, zone_refno).await {
                        let children_len = children.len();
                        println!(
                            "删除不符合条件的 zone {:?} 下的所有参考号,共有{}条",
                            zone_refno, children_len
                        );
                        for child in children.into_iter() {
                            undefined_zone_refno.insert(child.refno);
                        }
                    }
                }
            }
        }
        if sqls_clone.len() > 1000 {
            let mut sql = String::new();
            for s in sqls_clone.iter() {
                sql.push_str(s.as_str());
            }
            sql.remove(sql.len() - 1);
            let insert_sql = format!("INSERT IGNORE INTO {PDMS_SSC_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME,REAL_PDMS_REFNO,ORDER_NUM) VALUES ");
            let sql = format!("{} {}", insert_sql, sql);
            if let Ok(mut conn) = pool.acquire().await {
                let result = conn.execute(sql.as_str()).await;
                match result {
                    Ok(_) => {
                        dbg!("保存成功");
                        sqls_clone.clear();
                    }
                    Err(e) => {
                        let path = format!("resource/{}", idx);
                        if let Ok(mut file) = File::create(path) {
                            if let Ok(_) = file.write(sql.as_bytes()) {
                                sqls_clone.clear();
                            }
                        }
                        dbg!(sql);
                        dbg!(&e);
                    }
                }
            }
        }
        println!("生成SSC,已生成 {} 总共 {} ", idx, room_data_len);
        // });
        // handles.push(handle);
    }
    // futures::future::join_all(handles).await;
    let mut insert_sql = String::new();
    let mut insert_sql_vec = vec![];
    let sqls = Arc::try_unwrap(sqls).unwrap();
    println!("一共生成了 {} 个 SSC非固定节点", sqls.len());
    let mut i = 0;
    for sql in sqls {
        if i == 100 {
            if insert_sql.len() > 0 {
                insert_sql.remove(insert_sql.len() - 1);
            }
            insert_sql_vec.push(insert_sql.clone());
            insert_sql.clear();
        }
        insert_sql.push_str(sql.as_str());
        i += 1;
    }
    // 把剩余不满1000的sqls放到vec中
    if insert_sql.len() > 0 {
        insert_sql.remove(insert_sql.len() - 1);
    }
    insert_sql_vec.push(insert_sql.clone());
    Ok(insert_sql_vec)
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PdmsNodeMajor {
    #[serde_as(as = "DisplayFromStr")]
    pub refno: RefU64,
    pub noun: String,
    pub name: String,
    #[serde_as(as = "DisplayFromStr")]
    pub zone_refno: RefU64,
    pub zone_name: String,
    pub major: Option<String>,
}

/// 返回参考号集合所属的zone以及专业代码
pub async fn query_refnos_belong_zones(
    refnos: Vec<RefU64>,
    database: &Database,
) -> anyhow::Result<Vec<PdmsNodeMajor>> {
    let refnos = refnos
        .into_iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "
    With @@pdms_eles,@@pdms_edges
    for refno in @refnos
        let node = document(@@pdms_eles,refno)
        for v in 0..10 outbound node._id @@pdms_edges
            filter v.noun == 'ZONE'
            return {
                'refno':node._key,
                'name':node.name,
                'noun':node.noun,
                'zone_refno':v._key,
                'zone_name':v.name,
                'major':v.major
       }",
    )
    .bind_var("refnos", refnos)
    .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
    .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    let result = database.aql_query::<PdmsNodeMajor>(aql).await?;
    Ok(result)
}

/// 保存房间下的元件
pub async fn insert_ssc_room_node_refactor(database: &ArDatabase) -> anyhow::Result<()> {
    // 找到图数据库中所有的房间
    let rooms = query_all_room_aql(database).await.unwrap();
    let owner_types = vec![
        "BRAN".to_string(),
        "STRU".to_string(),
        "REST".to_string(),
        "EQUI".to_string(),
    ];
    let zone_type = vec!["PIPESU", "RACKSU", "INSTSU", "WATRSU"]; // 特殊处理得zone专业代码
    for room in rooms {
        dbg!(&room.room_name);
        // 依次查询每个房间下所有的节点
        let nodes = query_room_refno_from_room_refno_aql(room.refno, database)
            .await
            .unwrap();
        // 查询房间下所有节点所属的zone的专业号和所在 bran stru rest equi 的参考号和name
        let zone_major_infos = query_refnos_belong_zones(nodes.clone(), database)
            .await
            .unwrap();
        let owners =
            query_refnos_ancestor_with_name_till_type_aql(database, nodes, owner_types.clone())
                .await
                .unwrap();
        let owners = owners
            .into_iter()
            .map(|x| (x.refno, x))
            .collect::<HashMap<RefU64, PdmsOwnerNameAql>>();
        // 根据不同zone的分类规则来划分节点所在位置
        let mut ssc_nodes = Vec::new();
        let mut ssc_edges = Vec::new();
        for (idx, info) in zone_major_infos.into_iter().enumerate() {
            let Some(owner) = owners.get(&info.refno) else {
                continue;
            };
            if info.major.is_none() {
                continue;
            };
            let major = info.major.unwrap();
            // 节点的上一级
            let room_name = format!("1{}", room.room_name); // 默认都是一号机组
            if !zone_type.contains(&major.to_string().as_str()) {
                let owner_owner_hash =
                    convert_str_to_hash(format!("{}{}", room_name, major).as_str());
                let owner_name_hash =
                    convert_str_to_hash(format!("{}{}", room_name, owner.owner_name).as_str());
                let refno = RefU64(owner_name_hash);
                let owner_refno = RefU64(owner_owner_hash);
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner_refno,
                    name: owner.owner_name.to_string(),
                    noun: owner.owner_noun.to_string(),
                    dbnum: 0,
                    order: idx as u32,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner_refno,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
                // 存放元件
                let refno = info.refno;
                let owner = RefU64(owner_name_hash);
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner,
                    name: info.name.to_string(),
                    noun: info.noun.to_string(),
                    order: idx as u32,
                    dbnum: 0,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
            } else {
                // 该分类需要将owner拆分成两个层级
                let owner_owner_hash =
                    convert_str_to_hash(format!("{}{}", room_name, major).as_str());
                let owner_name_split = owner.owner_name.split("/").collect::<Vec<_>>();
                let owner_name_split = owner_name_split.get(1).unwrap_or(&"").to_string();
                let refno = RefU64(convert_str_to_hash(
                    format!("{}{}", room_name, owner_name_split).as_str(),
                ));
                let owner_refno = RefU64(owner_owner_hash);
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner_refno,
                    name: owner_name_split.to_string(),
                    noun: "SSC".to_string(),
                    order: idx as u32,
                    dbnum: 0,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner_refno,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
                // owner下两个固定层级
                let owner_refno = refno;
                let refno = RefU64(convert_str_to_hash(
                    format!("{}{}{}", room_name, owner_name_split, "STRU").as_str(),
                ));
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner_refno,
                    name: "STRU".to_string(),
                    noun: "STRU".to_string(),
                    dbnum: 0,
                    order: idx as u32,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner_refno,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
                let refno = RefU64(convert_str_to_hash(
                    format!("{}{}{}", room_name, owner_name_split, "REST").as_str(),
                ));
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner_refno,
                    name: "REST".to_string(),
                    noun: "REST".to_string(),
                    dbnum: 0,
                    order: idx as u32,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner_refno,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
                // 将房间下元件放到这两个固定层级下面
                let refno = info.refno;
                let owner_refno = RefU64(convert_str_to_hash(
                    format!("{}{}{}", room_name, owner_name_split, owner.owner_noun).as_str(),
                ));
                ssc_nodes.push(PdmsEleData {
                    refno,
                    owner: owner_refno,
                    name: info.name.to_string(),
                    noun: info.noun.to_string(),
                    dbnum: 0,
                    order: idx as u32,
                    cata_hash: "".to_string(),
                    // tag_lock:false,
                });
                ssc_edges.push(AqlEdge::new(
                    refno,
                    owner_refno,
                    AQL_SSC_ELES_COLLECTION,
                    AQL_SSC_ELES_COLLECTION,
                ));
            }
        }
        let eles_value = serde_json::to_value(&ssc_nodes)?;
        save_arangodb_doc(eles_value, AQL_SSC_ELES_COLLECTION, database, false).await?;
        let edge_value = serde_json::to_value(&ssc_edges)?;
        save_arangodb_doc(edge_value, AQL_SSC_EDGE_COLLECTION, database, false).await?;
    }
    Ok(())
}

/// 设置 ssc 的固定节点
pub fn set_ssc_node() -> anyhow::Result<(
    String,
    DashMap<String, RefU64>,
    DashMap<String, String>,
    RefU64,
)> {
    let mut next_refno = RefU64(0);
    let mut sql = String::new();
    let refno = RefU64(1);
    let mut owner_refno = RefU64(0);
    // root
    let (root_refno, root_sql) = gen_insert_ssc_node_sql(
        refno,
        "WORL",
        owner_refno,
        "\"华龙一号\" 标准SSC结构",
        RefU64(0),
        0,
    );
    sql.push_str(&root_sql);
    owner_refno = refno;
    // 第二层
    let (civil_n_refno, civil_node) =
        gen_insert_ssc_node_sql(root_refno, "SSC", owner_refno, "土建子项", RefU64(0), 0);
    sql.push_str(&civil_node);
    let (c_n_refno, c_node) =
        gen_insert_ssc_node_sql(civil_n_refno, "SSC", owner_refno, "安装厂房", RefU64(0), 1);
    sql.push_str(&c_node);
    let (x_n_refno, x_node) =
        gen_insert_ssc_node_sql(c_n_refno, "SSC", owner_refno, "系统", RefU64(0), 2);
    sql.push_str(&x_node);
    let (s_n_refno, s_node) =
        gen_insert_ssc_node_sql(x_n_refno, "SSC", owner_refno, "设备", RefU64(0), 3);
    sql.push_str(&s_node);
    let (q_n_refno, q_node) =
        gen_insert_ssc_node_sql(s_n_refno, "SSC", owner_refno, "全局性信息", RefU64(0), 4);
    sql.push_str(&q_node);
    // 安装厂房的子节点
    owner_refno = civil_n_refno;
    let (ni_n_refno, ni_node) =
        gen_insert_ssc_node_sql(q_n_refno, "SSC", owner_refno, "NI", RefU64(0), 0);
    sql.push_str(&ni_node);
    let (ci_n_refno, ni_node) =
        gen_insert_ssc_node_sql(ni_n_refno, "SSC", owner_refno, "CI", RefU64(0), 1);
    sql.push_str(&ni_node);
    let (bop_n_refno, ni_node) =
        gen_insert_ssc_node_sql(ci_n_refno, "SSC", owner_refno, "BOP", RefU64(0), 2);
    sql.push_str(&ni_node);
    // ni 下的子节点
    owner_refno = q_n_refno;
    let (one_n_refno, ni_node) =
        gen_insert_ssc_node_sql(bop_n_refno, "SSC", owner_refno, "一号机组", RefU64(0), 0);
    sql.push_str(&ni_node);
    let (two_n_refno, ni_node) =
        gen_insert_ssc_node_sql(one_n_refno, "SSC", owner_refno, "二号机组", RefU64(0), 1);
    sql.push_str(&ni_node);
    let (three_refno, ni_node) =
        gen_insert_ssc_node_sql(two_n_refno, "SSC", owner_refno, "双机组共用", RefU64(0), 2);
    sql.push_str(&ni_node);
    // 一号机组 安装层位
    let (one_level_refno, insert_sql) =
        gen_insert_ssc_node_sql(three_refno, "SSC", bop_n_refno, "安装层位", RefU64(0), 0);
    sql.push_str(insert_sql.as_str());
    // 安装分区
    let (n_refno, insert_sql) = gen_insert_ssc_node_sql(
        one_level_refno,
        "SSC",
        bop_n_refno,
        "安装分区",
        RefU64(0),
        1,
    );
    sql.push_str(insert_sql.as_str());
    // 二号机组 安装层位
    let (one_level_refno, insert_sql) =
        gen_insert_ssc_node_sql(n_refno, "SSC", one_n_refno, "安装层位", RefU64(0), 0);
    sql.push_str(insert_sql.as_str());
    // 安装分区
    let (two_level_refno, insert_sql) = gen_insert_ssc_node_sql(
        one_level_refno,
        "SSC",
        one_n_refno,
        "安装分区",
        RefU64(0),
        1,
    );
    next_refno = two_level_refno;
    sql.push_str(insert_sql.as_str());
    // 一号机组的子节点
    let mut zone_level_map = DashMap::new();
    let mut zone_name_map = DashMap::new();
    if let Ok(map) = parse_room_info_from_excel() {
        let (zone_level_map_r, zone_name_map_r, next_refno_level) =
            set_ssc_level_node(map, (three_refno, n_refno), two_level_refno, &mut sql)?;
        next_refno = next_refno_level;
        zone_level_map = zone_level_map_r;
        zone_name_map = zone_name_map_r;
    }

    sql.remove(sql.len() - 1);
    sql.push_str(";");
    Ok((sql, zone_level_map, zone_name_map, next_refno))
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct SSCLevelExcelData {
    pub name: Option<String>,
    pub att_type: Option<String>,
    pub owner: Option<String>,
}

/// ssc 假节点
pub fn gen_insert_ssc_node_sql(
    refno: RefU64,
    type_name: &str,
    owner: RefU64,
    name: &str,
    real_pdms_refno: RefU64,
    order_num: usize,
) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_string().to_string();
    sql.push_str(&format!(
        "({},'{refno_str}','{type_name}',{},'{name}',{},{order_num}),",
        refno.0, owner.0, real_pdms_refno.0
    ));
    (RefU64(refno.0 + 1), sql)
}

/// ssc 节点引用 pdms refno
pub fn gen_insert_ssc_node_sql_with_pdms_refno(
    refno: RefU64,
    type_name: &str,
    owner: RefU64,
    name: &str,
    pdms_real_refno: RefU64,
    order_num: usize,
) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_string().to_string();
    sql.push_str(&format!(
        "({},'{refno_str}','{type_name}',{},'{name}',{},{order_num}),",
        refno.0, pdms_real_refno.0, owner.0
    ));

    (RefU64(refno.0 + 1), sql)
}

/// 将refno有那些children存放在hashmap中
pub fn change_children_vec_to_map(
    refno: RefU64,
    children: Vec<EleTreeNode>,
) -> HashMap<RefU64, Vec<RefU64>> {
    let mut map = HashMap::new();
    children.into_iter().for_each(|child| {
        map.entry(refno).or_insert_with(Vec::new).push(child.refno);
    });
    map
}

/// 给每个房间下附上各专业对应的 site 和 zone
fn gen_insert_room_level_node_sql(
    level: Vec<(String, Vec<String>)>,
    mut refno: RefU64,
    site_owner: RefU64,
    zone_map: &mut HashMap<String, RefU64>,
    room_name: String,
) -> (RefU64, String) {
    let mut sql = String::new();
    let mut site_order = 0;
    for (site, zones) in level {
        let refno_str = refno.to_string();
        sql.push_str(&format!(
            "({},'{refno_str}','{}',{},'{site}',{site_order}),",
            refno.0, "SITE", site_owner.0
        ));

        let zone_owner = refno;
        refno = RefU64(refno.0 + 1);
        let mut zone_order = 0;
        site_order += 1;

        for zone in zones {
            let refno_str = refno.to_string();
            sql.push_str(&format!(
                "({},'{refno_str}','{}',{},'{}',{zone_order}),",
                refno.0,
                "ZONE",
                zone_owner.0,
                zone.clone()
            ));
            zone_map.insert(format!("{}_{}", zone, room_name), refno);
            refno = RefU64(refno.0 + 1);
            zone_order += 1;
        }
    }
    (RefU64(refno.0 + 1), sql)
}

/// 创建ssc固定层级中的层位(1层....) <br>
/// 参数：node_map： 从房间excel文件中读取机组号下的层位，层位下对应的房间 <br>
/// unit_refnos : 0:一号机组 参考号 1 : 二号机组参考号 暂时没有机组共用这一个分类 <br>
/// next_refnos ： ssc参考号是从0开始排的，这个就是下一个节点需要用到的参考号
fn set_ssc_level_node(
    node_map: HashMap<String, BTreeMap<i32, Vec<String>>>,
    unit_refnos: (RefU64, RefU64),
    mut next_refno: RefU64,
    insert_sql: &mut String,
) -> anyhow::Result<(DashMap<String, RefU64>, DashMap<String, String>, RefU64)> {
    let mut zone_level_map = DashMap::new();
    let mut unit_refno = RefU64(0); // 不同机组对应的参考号
    let excel_result = get_room_level_from_excel_refactor()?;
    let site_level_map = excel_result.level;
    let zone_name_map = excel_result.name_map;
    // 机组号 + 厂房号
    for (unit_name, v) in node_map {
        // 一号机组
        if unit_name.starts_with("1") {
            unit_refno = next_refno;
            let (refno, sql) =
                gen_insert_ssc_node_sql(next_refno, "SSC", unit_refnos.0, &unit_name, RefU64(0), 0);
            insert_sql.push_str(sql.as_str());
            next_refno = refno;
        } else {
            unit_refno = next_refno;
            let (refno, sql) =
                gen_insert_ssc_node_sql(next_refno, "SSC", unit_refnos.1, &unit_name, RefU64(0), 0);
            insert_sql.push_str(sql.as_str());
            next_refno = refno;
        }
        for (level, rooms) in v {
            let level_name = match level {
                1 => "1层(-6.70m)",
                2 => "2层(-3.30m)",
                3 => "3层(0.00m)",
                4 => "4层(+3.60m)",
                5 => "5层(+7.5m)",
                6 => "6层(+13.50m)",
                7 => "7层(+16.50m)",
                8 => "8层(+22.00m及以上)",
                9 => "9层(内穹顶)",
                _ => "",
            };
            if level_name != "" {
                let leve_refno = next_refno;
                let (refno, sql) = gen_insert_ssc_node_sql(
                    next_refno,
                    "SSC",
                    unit_refno,
                    level_name,
                    RefU64(0),
                    0,
                );
                insert_sql.push_str(sql.as_str());
                next_refno = refno;
                // 给每一层附上对应的房间号
                let mut order = 0;
                for room_name in rooms {
                    let room_refno = next_refno;
                    let (refno, sql) = gen_insert_ssc_node_sql(
                        next_refno,
                        "SSC_ROOM",
                        leve_refno,
                        room_name.as_str(),
                        RefU64(0),
                        order,
                    );
                    insert_sql.push_str(sql.as_str());
                    next_refno = refno;
                    order += 1;
                    // 给每个房间附上专业的节点
                    let mut site_order = 0;

                    for (site_name, zone_names) in &site_level_map {
                        // 给site附上节点
                        let site_refno = next_refno;
                        let (refno, sql) = gen_insert_ssc_node_sql(
                            next_refno,
                            "SSC",
                            room_refno,
                            site_name.as_str(),
                            RefU64(0),
                            site_order,
                        );
                        insert_sql.push_str(sql.as_str());
                        next_refno = refno;
                        site_order += 1;
                        // 给zone附上节点
                        let mut zone_order = 0;
                        for zone_name in zone_names {
                            let zone_refno = next_refno;
                            let (refno, sql) = gen_insert_ssc_node_sql(
                                next_refno,
                                "SSC",
                                site_refno,
                                &zone_name,
                                RefU64(0),
                                zone_order,
                            );
                            insert_sql.push_str(sql.as_str());
                            next_refno = refno;
                            zone_order += 1;
                            zone_level_map
                                .entry(format!("{}_{}", &room_name, zone_name))
                                .or_insert(zone_refno);
                        }
                    }
                }
            }
        }
    }
    Ok((zone_level_map, zone_name_map, next_refno))
}

/// 处理从数据库获取的和 room 相关的信息转换成 房间名 对应 参考号
pub fn deal_room_info(room_data: HashMap<RefU64, SscEleNode>) -> HashMap<String, RefU64> {
    let mut map = HashMap::new();
    for (room_refno, ele) in room_data {
        map.entry(ele.room_code).or_insert(room_refno);
    }
    map
}

/// 查询ssc对应的房间名，value ： 0: ssc 第一次创建的refno, 1: pdms 中房间的参考号
pub async fn query_ssc_room_refnos(
    room_info: &HashMap<String, RefU64>,
    pool: &Pool<MySql>,
) -> anyhow::Result<HashMap<String, (RefU64, RefU64)>> {
    let mut map = HashMap::new();
    if room_info.is_empty() {
        return Ok(HashMap::default());
    }
    let sql = gen_query_ssc_room_refnos_sql(room_info);
    let results = sqlx::query(&sql).fetch_all(pool).await;
    match results {
        Ok(results) => {
            for result in results {
                let name = result.get::<String, _>("NAME");
                let old_refno = RefU64(result.get::<i64, _>("ID") as u64);
                let new_refno = room_info.get(&name);
                if new_refno.is_none() {
                    continue;
                }
                let new_refno = new_refno.unwrap();
                map.entry(name).or_insert((old_refno, *new_refno));
            }
        }
        Err(error) => {
            dbg!(&error);
        }
    }
    Ok(map)
}

/// 通过 专业分类.xlsx 表中 pdms name 包含的关键字，将pdms_eles保存上对应的专业代码
///
/// name_map: 从 get_room_level_from_excel() 直接读出来的 , pdms site 和 其下面 zone 的 name 对应的专业代码
///
/// 返回值，不符合命名规则的site参考号和site name
pub async fn set_pdms_major_from_excel(
    name_map: &Vec<PdmsSscMajorCode>,
    sites: Vec<(RefU64, String)>,
    db_option: &DbOption,
    database: &ArDatabase,
    pool: &Pool<MySql>,
) -> anyhow::Result<Vec<(RefU64, String)>> {
    let numbs = query_db_nums_of_mdb(&db_option.mdb_name, &db_option.module, pool).await?;
    // 先查找到 mdb下的所有 site
    // let sites = query_types_refnos_names(&vec!["SITE"], pool, Some(&numbs)).await?;
    let mut update_aqls = Vec::new();
    // site 下其余name的情况
    let mut res_aqls = Vec::new();
    // 不符合命名规则的site
    let mut error_sites = Vec::new();
    // 将mdb所有的site查找到后，用 name_map 进行分组和过滤，一个site下面的zone为一组
    for (site_refno, site_name) in sites {
        let mut contains_key = Vec::new();
        let mut filter_aql = String::new();
        // 匹配 site 的 名字包含哪些专业代码
        for name in name_map {
            if site_name.contains(&name.site_name) {
                contains_key.push(name.clone());
            }
        }
        if contains_key.is_empty() {
            error_sites.push((site_refno, site_name.clone()));
            continue;
        }
        // 如果site 名字 同时包含两个专业代码，取长度最长的那个,例如 PIPE , PIPE-F
        if contains_key.len() > 1 {
            let Some(max_site_code) = contains_key
                .clone()
                .into_iter()
                .max_by(|a, b| a.site_name.len().cmp(&b.site_name.len()))
            else {
                continue;
            };
            filter_aql.push_str(&format!(
                "filter v.name like {}\r\n",
                max_site_code.site_name
            ));
            // 如果一个site name 同时满足多个条件，filter 字符长度最长的那个 ， 其他的 取否
            for key in contains_key {
                if &key.site_name == &max_site_code.site_name {
                    continue;
                }
                filter_aql.push_str(&format!(
                    "filter v.name !like {}\r\n",
                    max_site_code.site_name
                ));
            }
            // 写好过滤条件之后 就不需要其他数据了，只保留最符合的那个条件就可以了
            contains_key = vec![max_site_code.clone()];
        } else {
            filter_aql.push_str(&format!(
                "filter v.name like {}\r\n",
                contains_key[0].site_name
            ));
        }
        // 每一个 site 和下面的 zone 的更新 pdms_eles 语句
        let update_site_aql = format!(
            "With {AQL_PDMS_ELES_COLLECTION}
        update {{'_key':'{}' , 'major': '{}'}} in {}",
            site_refno.to_string(),
            contains_key[0].site_code,
            AQL_PDMS_ELES_COLLECTION
        );
        update_aqls.push(update_site_aql);
        for (zone_name, zone_code) in &contains_key[0].zone_map {
            if zone_name == "%ELSE" {
                let mut update_zone_aql = format!(
                    "\
                With {AQL_PDMS_ELES_COLLECTION},{AQL_PDMS_EDGES_COLLECTION}
                let zones = ( for v in 1 inbound '{}/{}' pdms_edges return v ) ",
                    AQL_PDMS_ELES_COLLECTION,
                    site_refno.to_string()
                );
                update_zone_aql.push_str(&format!("for zone in zones "));
                update_zone_aql.push_str(&format!("filter zone.major !like '%{}%' ", zone_name));
                update_zone_aql.push_str(&format!(
                    "update {{'_key':zone._key , 'major': '{}'}} in {}",
                    zone_code, AQL_PDMS_ELES_COLLECTION
                ));
                res_aqls.push(update_zone_aql);
            } else {
                let mut update_zone_aql = format!(
                    "\
                With {AQL_PDMS_ELES_COLLECTION},{AQL_PDMS_EDGES_COLLECTION}
                let zones = ( for v in 1 inbound '{}/{}' pdms_edges return v ) ",
                    AQL_PDMS_ELES_COLLECTION,
                    site_refno.to_string()
                );
                update_zone_aql.push_str(&format!("for zone in zones "));
                update_zone_aql.push_str(&format!("filter zone.name like '%{}%' ", zone_name));
                update_zone_aql.push_str(&format!(
                    "update {{'_key':zone._key , 'major': '{}'}} in {}",
                    zone_code, AQL_PDMS_ELES_COLLECTION
                ));
                update_aqls.push(update_zone_aql);
            }
        }
    }
    // todo 不能同时update多次 后期将这些aql优化到一次执行
    for update_aql in update_aqls {
        let _r = database
            .aql_query::<()>(AqlQuery::new(update_aql.as_str()))
            .await;
    }
    Ok(error_sites)
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct SscLevelExcel {
    pub name: Option<String>,
    pub att_type: Option<String>,
    pub owner: Option<String>,
}

impl SscLevelExcel {
    pub fn is_valid(&self) -> bool {
        if self.name.is_none() || self.att_type.is_none() {
            return false;
        }
        true
    }
}

/// 将 ssc_level.xlsx  ssc 固定节点保存到图数据库中
pub async fn save_ssc_level_excel(database: &ArDatabase) -> anyhow::Result<()> {
    let mut eles_results = Vec::new();
    let mut edge_results = Vec::new();

    let mut workbook: Xlsx<_> = open_workbook("resource/ssc_level.xlsx")?;
    let range = workbook
        .worksheet_range("Sheet1")
        .ok_or(anyhow::anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    let mut idx = 0;
    while let Some(result) = iter.next() {
        let v: SscLevelExcel = result?;
        if v.is_valid() {
            let name = v.name.unwrap();
            let name_hash = convert_str_to_hash(&name);
            let owner = if v.owner.is_some() {
                convert_str_to_hash(&v.owner.unwrap())
            } else {
                0
            };
            let refno = RefU64(name_hash);
            let owner = RefU64(owner);
            eles_results.push(PdmsEleData {
                refno,
                noun: v.att_type.unwrap(),
                order: idx,
                name,
                owner,
                dbnum: 0,
                cata_hash: "".to_string(),
                // tag_lock:false,
            });

            edge_results.push(AqlEdge {
                _key: refno.hash_with_another_refno(owner).to_string(),
                _from: format!("{}/{}", AQL_SSC_ELES_COLLECTION, refno.to_string()),
                _to: format!("{}/{}", AQL_SSC_ELES_COLLECTION, owner.to_string()),
            });
            idx += 1;
        }
    }
    let eles_value = serde_json::to_value(&eles_results)?;
    save_arangodb_doc(eles_value, AQL_SSC_ELES_COLLECTION, database, false).await?;
    let edge_value = serde_json::to_value(&edge_results)?;
    save_arangodb_doc(edge_value, AQL_SSC_EDGE_COLLECTION, database, false).await?;
    Ok(())
}

fn gen_query_ssc_room_refnos_sql(room_info: &HashMap<String, RefU64>) -> String {
    let mut sql = String::new();
    let mut rooms = String::new();
    for (room_name, _) in room_info {
        rooms.push_str(&format!("'{}' ,", room_name));
    }
    rooms.remove(rooms.len() - 1);
    sql.push_str(&format!(
        "SELECT ID,NAME FROM {PDMS_SSC_ELEMENTS_TABLE} WHERE NAME IN ({})",
        rooms
    ));
    sql
}

fn gen_replace_room_refno_sql(room_name: &str, refno: RefU64, old_refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        "UPDATE {PDMS_SSC_ELEMENTS_TABLE} SET ID = {} , REFNO = '{}' WHERE NAME = '{}' ;",
        refno.0,
        refno.to_string(),
        room_name
    ));
    sql.push_str(&format!(
        "UPDATE {PDMS_SSC_ELEMENTS_TABLE} SET OWNER = {} WHERE OWNER = {} ;",
        refno.0, old_refno.0
    ));
    sql
}

fn match_level_name(level: i32) -> Option<String> {
    match level {
        1 => Some("1层(-6.70m)".to_string()),
        2 => Some("2层(-3.30m)".to_string()),
        3 => Some("3层(0.00m)".to_string()),
        4 => Some("4层(+3.60m)".to_string()),
        5 => Some("5层(+7.5m)".to_string()),
        6 => Some("6层(+13.50m)".to_string()),
        7 => Some("7层(+16.50m)".to_string()),
        8 => Some("8层(+22.00m及以上)".to_string()),
        9 => Some("9层(内穹顶)".to_string()),
        _ => None,
    }
}

#[tokio::test]
async fn test_save_ssc_level_excel() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    // let pool = AiosDBManager::get_db_pool(&url, "AvevaMarineSample").await?;
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let _ = save_ssc_level_excel(&database).await?;
    let _result = get_room_info_from_excel_refactor(&database).await.unwrap();
    let _result = insert_ssc_room_node_refactor(&database).await.unwrap();
    Ok(())
}

#[test]
fn test_parse_room_info_from_excel() -> anyhow::Result<()> {
    let result = parse_room_info_from_excel()?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_get_room_info_from_excel_refactor() {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()
        .unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option)
        .await
        .unwrap();
    let result = get_room_info_from_excel_refactor(&database).await.unwrap();
}

#[tokio::test]
async fn test_insert_ssc_room_node_refactor() {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()
        .unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option)
        .await
        .unwrap();
    let result = insert_ssc_room_node_refactor(&database).await.unwrap();
}

#[test]
fn test_get_room_level_from_excel_refactor() {
    let result = get_room_level_from_excel_refactor().unwrap();
    dbg!(&result.level);
    // dbg!(&result.name_map.len());
    // dbg!(&result.pdms_name_code_map);
}
