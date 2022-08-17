use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::hash::Hash;
use std::io::{Read, Write};
use std::mem::transmute;
use std::sync::Arc;
use aios_core::pdms_types::{AttrVal, EleTreeNode, RefU64, RefU64Vec};
use anyhow::anyhow;
use arangors_lite::{AqlQuery, Database};
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::{DashMap, DashSet};
use futures::future::OkInto;
use smol_str::SmolStr;
use sqlx::{Acquire, Error, MySql, Pool, Row};
use sqlx::Executor;
use serde::{Serialize, Deserialize};
use sqlx::mysql::MySqlRow;
use crate::api::children::*;
use crate::api::element::*;
use crate::api::ssc_data::*;
use crate::aql_api::children::{query_ancestor_till_type_aql, query_travel_children_aql};
use crate::consts::PDMS_SSC_ELEMENTS_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::structs::{PdmsEleGraphEdge, SSCEleGraphNode};
use crate::tables;


/// site 和 zone 分类 excel 字段
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SiteExcelData {
    pub code: String,
    pub name: String,
    pub att_type: String,
}


#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SiteExcelDataTest {
    pub code: Option<String>,
    pub name: Option<String>,
    pub att_type: Option<String>,
    pub children_code: Option<String>,
    pub children_name: Option<String>,
    pub children_att_type: Option<String>,
}

/// 房间信息 excel 字段
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RoomExcelData {
    pub 房间代码: Option<String>,
    pub 所属机组: Option<u32>,
    pub 安装厂房: Option<String>,
    pub 区域: Option<String>,
    pub 安装层位: Option<String>,
    pub 厂房: Option<String>,
    pub 分区: Option<String>,
    pub 层位及标高: Option<String>,
    pub 序号: Option<u32>,
}

pub async fn async_total_ssc_data(project_pool: &Pool<MySql>, mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    let mut conn = project_pool.acquire().await?;
    // 创建 ssc 表
    let result = conn.execute(tables::gen_create_ssc_element_tables_sql().as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    dbg!("创建SSC表完成");
    let (zone_level_map, zone_name_map, next_refno) = insert_set_ssc_node_sql(project_pool).await?;
    dbg!("SSC固定节点生成");
    let room_data = query_all_room_data(project_pool).await?;
    if room_data.len() != 0 {
        let insert_sql = format!("INSERT IGNORE INTO {PDMS_SSC_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, REAL_PDMS_REFNO,ORDER_NUM) VALUES ");
        let sqls = insert_ssc_room_node(room_data, zone_level_map, zone_name_map, next_refno, project_pool, mgr).await;
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

/// 解析 excel 表单， 获取房间下面的ZONE和SITE层级  返回值  1 : key : site 的 name (中文名) value : site 下对应的zone 的 name ;
/// 2 : 英文 code 对应的中文名
fn get_room_level_from_excel() -> anyhow::Result<(Vec<(String, Vec<String>)>, DashMap<String, String>)> {
    let mut level: Vec<(String, Vec<String>)> = vec![];
    let mut name_map = DashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/专业分类.xlsx")?;
    dbg!("加载专业分类.xlsx 成功");
    let range = workbook.worksheet_range("Sheet2")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;
    dbg!("打开Sheet2成功");

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;
    let mut zone_name = "".to_string();
    let mut zone_code = "".to_string();
    let mut b_first = true;
    let mut zones = vec![];
    while let Some(result) = iter.next() {
        let v: SiteExcelDataTest = result?;
        // site 的 name 、code 、att_type
        if v.code.is_some() && v.name.is_some() && v.att_type.is_some() {
            // 当zone_code和当前读取的值不相等时，就代表不是同一个层级了 （第一次除外,所以加了个b_first 排除第一次的情况）
            let read_site_code = v.code.clone().unwrap(); // 从 excel 文件中读取的 site name
            if zone_code != read_site_code && !b_first {
                level.push((zone_name.clone(), zones.clone()));
                zones.clear();
            }

            let read_site_name = v.name.unwrap();
            zone_name = read_site_name.clone();
            zone_code = read_site_code.clone();

            name_map.insert(read_site_code, read_site_name);
            b_first = false;
        }
        // 存放 site 下的子节点
        if v.children_name.is_some() && v.children_code.is_some() {
            let read_zone_name = v.children_name.unwrap();
            let read_zone_code = v.children_code.clone().unwrap();

            zones.push(read_zone_name.clone());
            name_map.insert(read_zone_code, read_zone_name);
        }
    }
    level.push((zone_name.clone(), zones.clone())); // 查询结束时 还需要剩最后一条数据没插入
    Ok((level, name_map))
}

/// 解析 excel 表单 ，找到每一层下面所有的房间号 返回所有的安装厂房下对应的层位，层位下对应的房间
fn get_room_info_from_excel() -> anyhow::Result<HashMap<String, BTreeMap<i32, Vec<String>>>> {
    let mut r = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("resource/test.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        if let Some(install_workshop) = v.安装厂房 {
            if let Some(belong_unit) = v.所属机组 {
                let install_workshop = format!("{}{}", belong_unit.to_string(), install_workshop);
                if let Some(install_level) = v.安装层位 {
                    if let Some(workshop) = v.房间代码 {
                        r.entry(install_workshop).or_insert_with(BTreeMap::new)
                            .entry(install_level.parse().unwrap_or(1)).or_insert_with(Vec::new).push(workshop);
                    }
                }
            }
        }
    }
    Ok(r)
}

pub fn get_rooms_from_excel() -> anyhow::Result<Vec<String>> {
    let mut r = vec![];
    let mut workbook: Xlsx<_> = open_workbook("resource/test.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        if let Some(workshop) = v.房间代码 {
            r.push(workshop);
        }
    }
    Ok(r)
}

/// 创建ssc固定节点
pub async fn insert_set_ssc_node_sql(pool: &Pool<MySql>) -> anyhow::Result<(DashMap<String, RefU64>, DashMap<String, String>, RefU64)> {
    let insert_sql = format!("INSERT IGNORE INTO {PDMS_SSC_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, REAL_PDMS_REFNO,ORDER_NUM) VALUES ");
    let (sql, zone_level_map, zone_name_map, next_refno) = set_ssc_node()?;
    let sql = format!("{}{}", insert_sql, sql);
    let mut conn = pool.acquire().await?;
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
pub async fn insert_ssc_room_node(mut room_data: HashMap<RefU64, SscEleNode>, zone_level_map: DashMap<String, RefU64>,
                                  zone_name_map: DashMap<String, String>, mut next_refno: RefU64,
                                  pool: &Pool<MySql>, mgr: Arc<AiosDBManager>) -> Vec<String> {
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
        if room_ori.noun == "EQUI" { continue; }
        // 该房间号所在的zone没有对应的uda，直接跳过
        if undefined_zone_refno.contains(&room_ori.refno.to_refno_string()) {
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
        if let Ok(mut zone_refnos) = query_ancestor_refnos_till_type(room.refno, "ZONE", &pool).await {
            // 想拿到 zone的参考号
            if let Some(zone_refno) = zone_refnos.pop() {
                let divco = get_zone_divco(zone_refno, &pool).await;
                if divco != "" {
                    // 找到专业属性对应的中文名称
                    if let Some(divco_name) = zone_name_map.get(&divco) {
                        let divco_name = divco_name.trim();
                        let room_divco_name = format!("{}_{}", room_name, divco_name);
                        // 一个房间下只有一个专业的子类，所以直接通过name获取参考号
                        if let Some(zone_level_refno) = zone_level_map.get(&room_divco_name) {
                            // 找到 pdms 树 zone 下的层级放到ssc下面
                            if let Some(pdms_under_zone_refno) = zone_refnos.pop() {
                                // 特殊处理 将zone下的节点拆成两层，房间号+流水号 和 type名
                                if divco_name == "工艺支架" || divco_name == "仪表架" || divco_name == "仪表管支吊架" {
                                    if let Ok(pdms_under_zone_ele) = query_ele_node(pdms_under_zone_refno, &pool).await {
                                        // zone下面 不为 STRU 和 REST 的直接跳过
                                        if pdms_under_zone_ele.noun != "STRU" && pdms_under_zone_ele.noun != "REST" { continue; }
                                        // 找到 name 中房间号+流水号
                                        if let Some(room_serial_number) = pdms_under_zone_ele.name.find('.') {
                                            let room_serial_name = pdms_under_zone_ele.name[room_serial_number - 4..room_serial_number + 4].to_string();
                                            if let Some(special_refno) = special_under_zone_map_clone.get(&format!("{}_{}_{}", divco, room_serial_name, pdms_under_zone_ele.noun)) {
                                                // 房间层级
                                                let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                              *special_refno.value(), &room.name, room.refno, 0);
                                                sqls_clone.insert(insert_sql);
                                            } else {
                                                // 房间号+流水号层级
                                                let (next_refno_n, insert_sql) = gen_insert_ssc_node_sql(next_refno, "SSC",
                                                                                                         *zone_level_refno, &room_serial_name, RefU64(0), 0);
                                                // 房间号 + 流水号 参考号 ，STRU 和 REST 的 owner
                                                let room_level_refno = next_refno;
                                                sqls_clone.insert(insert_sql);
                                                next_refno = next_refno_n;
                                                // STRU/REST层级 直接给两个默认的
                                                let (next_refno_n, insert_sql) = gen_insert_ssc_node_sql(next_refno, "STRU",
                                                                                                         room_level_refno, "STRU", RefU64(0), 0);
                                                sqls_clone.insert(insert_sql);
                                                special_under_zone_map_clone.insert(format!("{}_{}_{}", divco, room_serial_name, "STRU"), next_refno);
                                                let special_stru_refno = next_refno;
                                                next_refno = next_refno_n;

                                                let (next_refno_n, insert_sql) = gen_insert_ssc_node_sql(next_refno, "REST",
                                                                                                         room_level_refno, "REST", RefU64(0), 0);
                                                sqls_clone.insert(insert_sql);
                                                special_under_zone_map_clone.insert(format!("{}_{}_{}", divco, room_serial_name, "REST"), next_refno);
                                                let special_rest_refno = next_refno;
                                                next_refno = next_refno_n;

                                                // 房间层级
                                                if pdms_under_zone_ele.noun == "STRU" {
                                                    let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                                  special_stru_refno, &room.name, room.refno, 0);
                                                    sqls_clone.insert(insert_sql);
                                                } else if pdms_under_zone_ele.noun == "REST" {
                                                    let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                                  special_rest_refno, &room.name, room.refno, 0);
                                                    sqls_clone.insert(insert_sql);
                                                }
                                            }
                                        }
                                    }
                                } else if divco_name.contains("支架") || divco_name.contains("设备") {
                                    if let Some(under_zone_refno) = under_zone_map.get(&format!("{}_{}", pdms_under_zone_refno.0, room.room_code.clone())) {
                                        let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                      *under_zone_refno, &room.name, room.refno, 0);
                                        sqls_clone.insert(insert_sql);
                                    } else {
                                        if let Ok(pdms_under_zone_ele) = query_ele_node(pdms_under_zone_refno, &pool).await {
                                            let (next_refno_n, insert_sql) = gen_insert_ssc_node_sql(next_refno, &pdms_under_zone_ele.noun,
                                                                                                     *zone_level_refno, &pdms_under_zone_ele.name, RefU64(0), 0);
                                            sqls_clone.insert(insert_sql);
                                            under_zone_map.insert(format!("{}_{}", pdms_under_zone_refno.0, room.room_code.clone()), next_refno);

                                            let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                          next_refno, &room.name, room.refno, 0);
                                            sqls_clone.insert(insert_sql);
                                            next_refno = next_refno_n;
                                        }
                                    }
                                } else {
                                    if let Some(pdms_under_bran_refno) = zone_refnos.pop() {
                                        if let Some(under_bran_refno) = under_zone_map.get(&format!("{}_{}", pdms_under_bran_refno.0, room.room_code.clone())) {
                                            let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                          *under_bran_refno, &room.name, room.refno, 0);
                                            sqls_clone.insert(insert_sql);
                                        } else {
                                            if let Ok(pdms_under_bran_ele) = query_ele_node(pdms_under_bran_refno, &pool).await {
                                                let (next_refno_n, insert_sql) = gen_insert_ssc_node_sql(next_refno, &pdms_under_bran_ele.noun,
                                                                                                         *zone_level_refno, &pdms_under_bran_ele.name, RefU64(0), 0);
                                                sqls_clone.insert(insert_sql);
                                                under_zone_map.insert(format!("{}_{}", pdms_under_bran_refno.0, room.room_code.clone()), next_refno);
                                                let (_, insert_sql) = gen_insert_ssc_node_sql(room.refno, &room.noun,
                                                                                              next_refno, &room.name, room.refno, 0);
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
                    if mgr.get_arangodb_conn().await.is_err() { continue; }
                    let database = mgr.get_arangodb_conn().await.unwrap();
                    if let Ok(children) = query_travel_children_aql(&database, zone_refno).await {
                        let children_len = children.len();
                        println!("删除不符合条件的 zone {:?} 下的所有参考号,共有{}条", zone_refno, children_len);
                        for child in children.into_iter() {
                            undefined_zone_refno.insert(child.refno);
                        }
                    }
                }
            }
        }
        if sqls_clone.len() > 100 {
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
    insert_sql_vec
}

/// 设置 ssc 的固定节点
pub fn set_ssc_node() -> anyhow::Result<(String, DashMap<String, RefU64>, DashMap<String, String>, RefU64)> {
    let mut next_refno = RefU64(0);
    let mut sql = String::new();
    let refno = RefU64(1);
    let mut owner_refno = RefU64(0);
    // root
    let (root_refno, root_sql) = gen_insert_ssc_node_sql(refno, "WORL", owner_refno, "\"华龙一号\" 标准SSC结构", RefU64(0), 0);
    sql.push_str(&root_sql);
    owner_refno = refno;
    // 第二层
    let (civil_n_refno, civil_node) = gen_insert_ssc_node_sql(root_refno, "SSC", owner_refno, "土建子项", RefU64(0), 0);
    sql.push_str(&civil_node);
    let (c_n_refno, c_node) = gen_insert_ssc_node_sql(civil_n_refno, "SSC", owner_refno, "安装厂房", RefU64(0), 1);
    sql.push_str(&c_node);
    let (x_n_refno, x_node) = gen_insert_ssc_node_sql(c_n_refno, "SSC", owner_refno, "系统", RefU64(0), 2);
    sql.push_str(&x_node);
    let (s_n_refno, s_node) = gen_insert_ssc_node_sql(x_n_refno, "SSC", owner_refno, "设备", RefU64(0), 3);
    sql.push_str(&s_node);
    let (q_n_refno, q_node) = gen_insert_ssc_node_sql(s_n_refno, "SSC", owner_refno, "全局性信息", RefU64(0), 4);
    sql.push_str(&q_node);
    // 安装厂房的子节点
    owner_refno = civil_n_refno;
    let (ni_n_refno, ni_node) = gen_insert_ssc_node_sql(q_n_refno, "SSC", owner_refno, "NI", RefU64(0), 0);
    sql.push_str(&ni_node);
    let (ci_n_refno, ni_node) = gen_insert_ssc_node_sql(ni_n_refno, "SSC", owner_refno, "CI", RefU64(0), 1);
    sql.push_str(&ni_node);
    let (bop_n_refno, ni_node) = gen_insert_ssc_node_sql(ci_n_refno, "SSC", owner_refno, "BOP", RefU64(0), 2);
    sql.push_str(&ni_node);
    // ni 下的子节点
    owner_refno = q_n_refno;
    let (one_n_refno, ni_node) = gen_insert_ssc_node_sql(bop_n_refno, "SSC", owner_refno, "一号机组", RefU64(0), 0);
    sql.push_str(&ni_node);
    let (two_n_refno, ni_node) = gen_insert_ssc_node_sql(one_n_refno, "SSC", owner_refno, "二号机组", RefU64(0), 1);
    sql.push_str(&ni_node);
    let (three_refno, ni_node) = gen_insert_ssc_node_sql(two_n_refno, "SSC", owner_refno, "双机组共用", RefU64(0), 2);
    sql.push_str(&ni_node);
    // 一号机组 安装层位
    let (one_level_refno, insert_sql) = gen_insert_ssc_node_sql(three_refno, "SSC", bop_n_refno, "安装层位", RefU64(0), 0);
    sql.push_str(insert_sql.as_str());
    // 安装分区
    let (n_refno, insert_sql) = gen_insert_ssc_node_sql(one_level_refno, "SSC", bop_n_refno, "安装分区", RefU64(0), 1);
    sql.push_str(insert_sql.as_str());
    // 二号机组 安装层位
    let (one_level_refno, insert_sql) = gen_insert_ssc_node_sql(n_refno, "SSC", one_n_refno, "安装层位", RefU64(0), 0);
    sql.push_str(insert_sql.as_str());
    // 安装分区
    let (two_level_refno, insert_sql) = gen_insert_ssc_node_sql(one_level_refno, "SSC", one_n_refno, "安装分区", RefU64(0), 1);
    next_refno = two_level_refno;
    sql.push_str(insert_sql.as_str());
    // 一号机组的子节点
    let mut zone_level_map = DashMap::new();
    let mut zone_name_map = DashMap::new();
    if let Ok(map) = get_room_info_from_excel() {
        let (zone_level_map_r, zone_name_map_r, next_refno_level) = set_ssc_level_node(map, (three_refno, n_refno), two_level_refno, &mut sql)?;
        next_refno = next_refno_level;
        zone_level_map = zone_level_map_r;
        zone_name_map = zone_name_map_r;
    }

    sql.remove(sql.len() - 1);
    sql.push_str(";");
    Ok((sql, zone_level_map, zone_name_map, next_refno))
}

/// ssc 假节点
pub fn gen_insert_ssc_node_sql(refno: RefU64, type_name: &str, owner: RefU64, name: &str, real_pdms_refno: RefU64, order_num: usize) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_refno_str().to_string();
    sql.push_str(&format!("({},'{refno_str}','{type_name}',{},'{name}',{},{order_num}),", refno.0, owner.0, real_pdms_refno.0));

    (RefU64(refno.0 + 1), sql)
}

/// ssc 节点引用pdmsrefno
pub fn gen_insert_ssc_node_sql_with_pdms_refno(refno: RefU64, type_name: &str, owner: RefU64, name: &str, pdms_real_refno: RefU64, order_num: usize) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_refno_str().to_string();
    sql.push_str(&format!("({},'{refno_str}','{type_name}',{},'{name}',{},{order_num}),", refno.0, pdms_real_refno.0, owner.0));

    (RefU64(refno.0 + 1), sql)
}

/// 将refno有那些children存放在hashmap中
pub fn change_children_vec_to_map(refno: RefU64, children: Vec<EleTreeNode>) -> HashMap<RefU64, Vec<RefU64>> {
    let mut map = HashMap::new();
    children.into_iter().for_each(|child| {
        map.entry(refno).or_insert_with(Vec::new).push(child.refno);
    });
    map
}

/// 给每个房间下附上各专业对应的 site 和 zone
fn gen_insert_room_level_node_sql(level: Vec<(String, Vec<String>)>, mut refno: RefU64,
                                  site_owner: RefU64, zone_map: &mut HashMap<String, RefU64>, room_name: String) -> (RefU64, String) {
    let mut sql = String::new();
    let mut site_order = 0;
    for (site, zones) in level {
        let refno_str = refno.to_refno_str();
        sql.push_str(&format!("({},'{refno_str}','{}',{},'{site}',{site_order}),", refno.0, "SITE", site_owner.0));

        let zone_owner = refno;
        refno = RefU64(refno.0 + 1);
        let mut zone_order = 0;
        site_order += 1;

        for zone in zones {
            let refno_str = refno.to_refno_str();
            sql.push_str(&format!("({},'{refno_str}','{}',{},'{}',{zone_order}),", refno.0, "ZONE", zone_owner.0, zone.clone()));
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
fn set_ssc_level_node(node_map: HashMap<String, BTreeMap<i32, Vec<String>>>, unit_refnos: (RefU64, RefU64),
                      mut next_refno: RefU64, insert_sql: &mut String) -> anyhow::Result<(DashMap<String, RefU64>, DashMap<String, String>, RefU64)> {
    let mut zone_level_map = DashMap::new();
    let mut unit_refno = RefU64(0); // 不同机组对应的参考号
    let (site_level_map, zone_name_map) = get_room_level_from_excel()?;
    // 机组号 + 厂房号
    for (unit_name, v) in node_map {
        // 一号机组
        if unit_name.starts_with("1") {
            unit_refno = next_refno;
            let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC", unit_refnos.0, &unit_name, RefU64(0), 0);
            insert_sql.push_str(sql.as_str());
            next_refno = refno;
        } else {
            unit_refno = next_refno;
            let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC", unit_refnos.1, &unit_name, RefU64(0), 0);
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
                let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC", unit_refno, level_name, RefU64(0), 0);
                insert_sql.push_str(sql.as_str());
                next_refno = refno;
                // 给每一层附上对应的房间号
                let mut order = 0;
                for room_name in rooms {
                    let room_refno = next_refno;
                    let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC_ROOM", leve_refno, room_name.as_str(), RefU64(0), order);
                    insert_sql.push_str(sql.as_str());
                    next_refno = refno;
                    order += 1;
                    // 给每个房间附上专业的节点
                    let mut site_order = 0;

                    for (site_name, zone_names) in &site_level_map {
                        // 给site附上节点
                        let site_refno = next_refno;
                        let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC", room_refno, site_name.as_str(), RefU64(0), site_order);
                        insert_sql.push_str(sql.as_str());
                        next_refno = refno;
                        site_order += 1;
                        // 给zone附上节点
                        let mut zone_order = 0;
                        for zone_name in zone_names {
                            let zone_refno = next_refno;
                            let (refno, sql) = gen_insert_ssc_node_sql(next_refno, "SSC", site_refno, &zone_name, RefU64(0), zone_order);
                            insert_sql.push_str(sql.as_str());
                            next_refno = refno;
                            zone_order += 1;
                            zone_level_map.entry(format!("{}_{}", &room_name, zone_name)).or_insert(zone_refno);
                        }
                    }
                }
            }
        }
    }
    Ok((zone_level_map, zone_name_map, next_refno))
}


#[test]
fn test_set_ssc_tree() {
    let sql = set_ssc_node().unwrap();
    println!("zone_map={:?}", sql.1);
}

#[test]
fn test_read_excel() {
    let result = get_room_info_from_excel().unwrap();
    // let (level, name_map) = get_room_level_from_excel().unwrap();
    if let Some(map) = result.get("1RX") {
        if let Some(val) = map.get(&1) {
            println!("val={:?}", val);
        }
    }
    // dbg!(&name_map);
}

#[test]
fn test_split_name() {
    let room_name = "R101";
    let room_split_name = room_name.split("-").collect::<Vec<_>>();
    println!("name={:?}", room_split_name.last());
}

#[test]
fn test_foreach() {
    let result = vec![1, 2, 3, 4, 5];
    for r in result {
        if r != 3 { continue; }
        dbg!(r);
    }
}