use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::mem::transmute;
use aios_core::pdms_types::{AttrVal, EleTreeNode, RefU64, RefU64Vec};
use anyhow::anyhow;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use dashmap::DashMap;
use futures::future::OkInto;
use smol_str::SmolStr;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::Executor;
use serde::{Serialize, Deserialize};
use sqlx::mysql::MySqlRow;
use crate::api::children::query_owner_type_from_id;
use crate::api::element::{query_name, query_owner_from_id, query_refno_type};
use crate::consts::PDMS_SSC_ELEMENTS_TABLE;

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

/// 解析 excel 表单， 获取房间下面的ZONE和SITE层级  返回值  1 : key : site 的 name  value : site 下对应的zone 的 name ; 2 : 英文 code 对应的中文名
fn get_room_level_from_excel() -> anyhow::Result<(Vec<(String, Vec<String>)>, HashMap<String, String>)> {
    let mut level: Vec<(String, Vec<String>)> = vec![];
    let mut name_map = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("专业分类.xlsx")?;
    let range = workbook.worksheet_range("Sheet2")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;

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

/// 解析 excel 表单 ，找到每一层下面所有的房间号
fn get_room_info_from_excel() -> anyhow::Result<HashMap<String, Vec<SmolStr>>> {
    let mut r = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook("test.xlsx")?;
    let range = workbook.worksheet_range("Sheet1")
        .ok_or(anyhow!("Cannot find 'Sheet1'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: RoomExcelData = result?;
        if v.安装厂房 == Some("RX".to_string()) {
            let room_name = SmolStr::new(v.房间代码.ok_or(anyhow!("房间代码 filed is empty"))?);
            r.entry(v.安装层位.ok_or(anyhow!("安装层位 filed is empty"))?).or_insert_with(Vec::new).push(room_name.clone());
        }
    }
    Ok(r)
}

pub async fn insert_set_ssc_node_sql(pool: Pool<MySql>) -> anyhow::Result<(HashMap<String, RefU64>, HashMap<String, String>)> {
    // let insert_sql = "INSERT IGNORE INTO PDMS_SSC_ELEMENTS (ID, REFNO, TYPE, OWNER, NAME, ORDER_NUM) VALUES ";
    let insert_sql = "REPLACE INTO PDMS_SSC_ELEMENTS (ID, REFNO, TYPE, OWNER, NAME, ORDER_NUM) VALUES ";
    let (sql, zone_map, zone_name_map) = set_ssc_node();
    let sql = format!("{}{}", insert_sql, sql);
    let mut conn = pool.acquire().await?;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
            dbg!(sql.as_str());
        }
    }
    Ok((zone_map, zone_name_map))
}

/// 保存房间下的元件
pub async fn insert_ssc_room_node(room_code_map: DashMap<String, RefU64Vec>, zone_code_map: DashMap<RefU64, String>,
                                  ssc_zone_map: HashMap<String, RefU64>, ssc_zone_name_map: HashMap<String, String>, pool: &Pool<MySql>) -> String {
    let mut sql = String::new();
    let mut owner_set = HashSet::new(); // 存放已经将数据放到sql中的owner数据
    let mut pipe_su_map: HashMap<String, RefU64> = HashMap::new(); // 工艺支架做特殊处理
    let mut pipe_su_order = 100000; // 用来给自定义的ssc节点赋参考号
    // 找到每个参考号的属于那个zone
    for (room_code, refnos) in room_code_map {
        if let Some(room_name) = room_code.split('-').collect::<Vec<_>>().last() {
            for refno in refnos {
                if let Ok(Some((mut owner, _))) = query_owner_type_from_id(refno, pool).await {
                    let mut f_refno = owner; // 循环往上查找owner，知道找到zone结束
                    while let Ok(Some((owner_refno, owner_att_type))) = query_owner_type_from_id(f_refno, pool).await {
                        if owner_att_type == "ZONE" && zone_code_map.contains_key(&owner_refno) {
                            // 房间下元件的owner需要保存到ssc中
                            if !owner_set.contains(&owner) {
                                // 将 zone_code_map 的value命名格式转换为 ssc_zone_map 的key格式
                                let zone_code = zone_code_map.get(&owner_refno).unwrap();
                                // 将 zone_code 转换为对应的中文
                                if let Some(zone_name) = ssc_zone_name_map.get(zone_code.value()) {
                                    let zone_name = zone_name.trim().to_string();
                                    let divco = format!("{}_{}", zone_name, room_name);
                                    if let Some(ssc_zone_refno) = ssc_zone_map.get(&divco) {
                                        // 工艺支架层级，元件的 owner 是 pdms 中对应的 owner 的 owner
                                        if zone_name == "工艺支架" {
                                            if let Ok(Some((owner_owner, _))) = query_owner_type_from_id(owner, pool).await {
                                                if let Ok(owner_name) = query_name(owner_owner, pool).await {
                                                    if let Some(pipe_room_name) = owner_name.split('-').collect::<Vec<_>>().get(1) {
                                                        let pipe_room_name_split = pipe_room_name.split('/').collect::<Vec<_>>();
                                                        if let Some(pipe_room_name) = pipe_room_name_split.get(1) {
                                                            if let Some(refno) = pipe_su_map.get(pipe_room_name_split[0]) {
                                                                sql.push_str(&format!("({},'{}','SSC',{},'{pipe_room_name}',{}),",
                                                                                      owner_owner.0, owner_owner.to_refno_str(), refno.0, 0));
                                                            } else {
                                                                let refno = ssc_zone_refno.0 + pipe_su_order;
                                                                sql.push_str(&format!("({},'{}','SSC',{},'{}',{}),",
                                                                                      refno, RefU64(refno).to_refno_str(), ssc_zone_refno.0, pipe_room_name_split[0], 0));
                                                                pipe_su_map.insert(pipe_room_name_split[0].to_string(), RefU64(refno));
                                                                pipe_su_order += 1;
                                                                sql.push_str(&format!("({},'{}','SSC',{},'{pipe_room_name}',{}),",
                                                                                      owner_owner.0, owner_owner.to_refno_str(), refno, 0));
                                                            }
                                                            owner = owner_owner;
                                                            owner_set.insert(owner);
                                                        }
                                                    }
                                                }
                                            }
                                        } else if zone_name == "通风支架" || zone_name == "电缆主桥架支架" || zone_name == "电缆次桥架支架" {
                                            if let Ok(Some((owner_owner, _))) = query_owner_type_from_id(owner, pool).await {
                                                if let Ok(owner_name) = query_name(owner_owner, pool).await {
                                                    if let Ok(owner_type) = query_refno_type(owner_owner, pool).await {
                                                        sql.push_str(&format!("({},'{}','SSC',{},'{}',{}),",
                                                                              owner_owner.0, owner_owner.to_refno_str(), ssc_zone_refno.0, format!("{owner_type}  {owner_name}"), 0));
                                                        owner = owner_owner;
                                                        owner_set.insert(owner);
                                                    }
                                                }
                                            }
                                        } else {
                                            if let Ok(owner_name) = query_name(owner, pool).await {
                                                if let Ok(owner_type) = query_refno_type(owner, pool).await {
                                                    sql.push_str(&format!("({},'{}','SSC',{},'{}',{}),", owner.0,
                                                                          owner.to_refno_str(), ssc_zone_refno.0, format!("{owner_type}  {owner_name}"), 0));
                                                    owner_set.insert(owner);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // 将元件保存到数据库中
                            if let Ok(name) = query_name(refno, pool).await {
                                sql.push_str(&format!("({},'{}','SSC',{},'{name}',{}),", refno.0, refno.to_refno_str(), owner.0, 0));
                            }
                        }
                        f_refno = owner_refno;
                    }
                }
            }
        }
    }
    sql
}

/// 设置 ssc 的固定节点
pub fn set_ssc_node() -> (String, HashMap<String, RefU64>, HashMap<String, String>) {
    let mut zone_map = HashMap::new(); // 每个房间下对应的zone的refno key : zone_name + room_name
    let mut zone_name_map = HashMap::new(); // 存放 zone 属性 :CNPEdivco 属性对应的中文名
    let mut sql = String::new();
    let refno = RefU64(1);
    let mut owner_refno = RefU64(0);
    // root
    let (_root_refno, root_sql) = gen_insert_ssc_node_sql(refno, "WORL", owner_refno, "\"华龙一号\" 标准SSC结构", 0);
    sql.push_str(&root_sql);
    owner_refno = refno;
    // 第二层
    let (civil_n_refno, civil_node) = gen_insert_ssc_node_sql(refno, "SSC", owner_refno, "土建子项", 0);
    sql.push_str(&civil_node);
    let (c_n_refno, c_node) = gen_insert_ssc_node_sql(civil_n_refno, "SSC", owner_refno, "安装厂房", 1);
    sql.push_str(&c_node);
    let (x_n_refno, x_node) = gen_insert_ssc_node_sql(c_n_refno, "SSC", owner_refno, "系统", 2);
    sql.push_str(&x_node);
    let (s_n_refno, s_node) = gen_insert_ssc_node_sql(x_n_refno, "SSC", owner_refno, "设备", 3);
    sql.push_str(&s_node);
    let (q_n_refno, q_node) = gen_insert_ssc_node_sql(s_n_refno, "SSC", owner_refno, "全局性信息", 4);
    sql.push_str(&q_node);
    // 安装厂房的子节点
    owner_refno = civil_n_refno;
    let (ni_n_refno, ni_node) = gen_insert_ssc_node_sql(q_n_refno, "SSC", owner_refno, "NI", 0);
    sql.push_str(&ni_node);
    let (ci_n_refno, ni_node) = gen_insert_ssc_node_sql(ni_n_refno, "SSC", owner_refno, "CI", 1);
    sql.push_str(&ni_node);
    let (bop_n_refno, ni_node) = gen_insert_ssc_node_sql(ci_n_refno, "SSC", owner_refno, "BOP", 2);
    sql.push_str(&ni_node);
    // ni 下的子节点
    owner_refno = q_n_refno;
    let (one_n_refno, ni_node) = gen_insert_ssc_node_sql(bop_n_refno, "SSC", owner_refno, "一号机组", 0);
    sql.push_str(&ni_node);
    let (two_n_refno, ni_node) = gen_insert_ssc_node_sql(one_n_refno, "SSC", owner_refno, "二号机组", 1);
    sql.push_str(&ni_node);
    let (three_refno, ni_node) = gen_insert_ssc_node_sql(two_n_refno, "SSC", owner_refno, "双机组共用", 2);
    sql.push_str(&ni_node);
    // 一号机组的子节点
    owner_refno = bop_n_refno;
    let (dx_n_refno, ni_node) = gen_insert_ssc_node_sql(three_refno, "SSC", owner_refno, "1DX", 0);
    sql.push_str(&ni_node);
    let (du_n_refno, ni_node) = gen_insert_ssc_node_sql(dx_n_refno, "SSC", owner_refno, "1DU", 1);
    sql.push_str(&ni_node);
    let (ka_n_refno, ni_node) = gen_insert_ssc_node_sql(du_n_refno, "SSC", owner_refno, "1KA", 2);
    sql.push_str(&ni_node);
    let (kp_n_refno, ni_node) = gen_insert_ssc_node_sql(ka_n_refno, "SSC", owner_refno, "1KP", 3);
    sql.push_str(&ni_node);
    let (ky_n_refno, ni_node) = gen_insert_ssc_node_sql(kp_n_refno, "SSC", owner_refno, "1KY", 4);
    sql.push_str(&ni_node);
    let (la_n_refno, ni_node) = gen_insert_ssc_node_sql(ky_n_refno, "SSC", owner_refno, "1LA", 5);
    sql.push_str(&ni_node);
    let (nh_n_refno, ni_node) = gen_insert_ssc_node_sql(la_n_refno, "SSC", owner_refno, "1NH", 6);
    sql.push_str(&ni_node);
    let (pr_n_refno, ni_node) = gen_insert_ssc_node_sql(nh_n_refno, "SSC", owner_refno, "1PR", 7);
    sql.push_str(&ni_node);
    let (rx_n_refno, ni_node) = gen_insert_ssc_node_sql(pr_n_refno, "SSC", owner_refno, "1RX", 8);
    sql.push_str(&ni_node);
    let (sl_n_refno, ni_node) = gen_insert_ssc_node_sql(rx_n_refno, "SSC", owner_refno, "1SL", 9);
    sql.push_str(&ni_node);
    let (sr_n_refno, ni_node) = gen_insert_ssc_node_sql(sl_n_refno, "SSC", owner_refno, "1SR", 10);
    sql.push_str(&ni_node);
    let (ur_n_refno, ni_node) = gen_insert_ssc_node_sql(sr_n_refno, "SSC", owner_refno, "1UR", 11);
    sql.push_str(&ni_node);
    // 1RX 的 子节点
    owner_refno = pr_n_refno;
    let (fq_n_refno, ni_node) = gen_insert_ssc_node_sql(ur_n_refno, "SSC", owner_refno, "安装分区", 0);
    sql.push_str(&ni_node);
    let (cw_n_refno, ni_node) = gen_insert_ssc_node_sql(fq_n_refno, "SSC", owner_refno, "安装层位", 0);
    sql.push_str(&ni_node);
    // 安装层位下的子节点
    owner_refno = fq_n_refno;
    let (one_n_refno, ni_node) = gen_insert_ssc_node_sql(cw_n_refno, "SSC", owner_refno, "1层(-6.70m)", 0);
    sql.push_str(&ni_node);
    let (two_n_refno, ni_node) = gen_insert_ssc_node_sql(one_n_refno, "SSC", owner_refno, "2层(-3.30m)", 0);
    sql.push_str(&ni_node);
    let (three_n_refno, ni_node) = gen_insert_ssc_node_sql(two_n_refno, "SSC", owner_refno, "3层(0.00m)", 0);
    sql.push_str(&ni_node);
    let (four_n_refno, ni_node) = gen_insert_ssc_node_sql(three_n_refno, "SSC", owner_refno, "4层(+3.60m)", 0);
    sql.push_str(&ni_node);
    let (five_n_refno, ni_node) = gen_insert_ssc_node_sql(four_n_refno, "SSC", owner_refno, "5层(+7.5m)", 0);
    sql.push_str(&ni_node);
    let (six_n_refno, ni_node) = gen_insert_ssc_node_sql(five_n_refno, "SSC", owner_refno, "6层(+13.50m)", 0);
    sql.push_str(&ni_node);
    let (seven_n_refno, ni_node) = gen_insert_ssc_node_sql(six_n_refno, "SSC", owner_refno, "7层(+16.50m)", 0);
    sql.push_str(&ni_node);
    let (eight_n_refno, ni_node) = gen_insert_ssc_node_sql(seven_n_refno, "SSC", owner_refno, "8层(+22.00m及以上)", 0);
    sql.push_str(&ni_node);
    let (nine_n_refno, ni_node) = gen_insert_ssc_node_sql(eight_n_refno, "SSC", owner_refno, "9层(内穹顶)", 0);
    sql.push_str(&ni_node);
    // 对应层数下面的房间
    if let Ok(rooms) = get_room_info_from_excel() {
        // room 下 对应的 site zone 层级
        if let Ok((room_level, zone_name_map_excel)) = get_room_level_from_excel() {
            zone_name_map = zone_name_map_excel;
            let mut next_refno = nine_n_refno;
            for (level, room_names) in rooms {
                let mut order = 0;
                for room_name in room_names {
                    match level.as_str() {
                        "1" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", cw_n_refno, room_name.clone().as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                            let (next, ni_node) = gen_insert_room_level_node_sql(room_level.clone(),
                                                                                 next_refno, RefU64(next.0 - 1), &mut zone_map, room_name.to_string());
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "2" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", one_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "3" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", two_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "4" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", three_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "5" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", four_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "6" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", five_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "7" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", six_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "8" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", seven_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }
                        "9" => {
                            let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", eight_n_refno, room_name.as_str(), order);
                            sql.push_str(&ni_node);
                            next_refno = next;
                        }

                        _ => {}
                    }
                    order += 1;
                }
            }
        }
    }

    sql.remove(sql.len() - 1);
    sql.push_str(";");
    (sql, zone_map, zone_name_map)
}

pub fn gen_insert_ssc_node_sql(refno: RefU64, type_name: &str, owner: RefU64, name: &str, order_num: usize) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_refno_str().to_string();
    sql.push_str(&format!("({},'{refno_str}','{type_name}',{},'{name}',{order_num}),", refno.0, owner.0));

    (RefU64(refno.0 + 1), sql)
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

pub async fn query_ssc_world(pool: &Pool<MySql>) -> anyhow::Result<Option<EleTreeNode>> {
    let sql = gen_query_ssc_world_sql();
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(val) => {
            let refno = RefU64(val.get::<i64, _>("ID") as u64);
            let children_count = query_ssc_children_count(refno, &pool).await?;
            let node = EleTreeNode {
                refno,
                noun: val.get::<String, _>("TYPE"),
                name: val.get::<String, _>("NAME"),
                owner: RefU64(val.get::<i64, _>("OWNER") as u64),
                children_count,
            };
            Ok(Some(node))
        }
        Err(e) => {
            dbg!(sql);
            dbg!(e);
            Ok(None)
        }
    };
}

pub async fn query_ssc_children(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let sql = gen_query_ssc_children_sql(refno);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("ID") as u64);
                let children_count = query_ssc_children_count(refno, &pool).await?;
                let node = EleTreeNode {
                    refno,
                    noun: val.get::<String, _>("TYPE"),
                    name: val.get::<String, _>("NAME"),
                    owner: RefU64(val.get::<i64, _>("OWNER") as u64),
                    children_count,
                };
                r.push(node);
            }
            Ok(r)
        }
        Err(e) => {
            dbg!(sql);
            dbg!(e);
            Ok(vec![])
        }
    };
}

pub async fn query_ssc_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<usize> {
    let count_sql = gen_query_ssc_children_count_sql(refno);
    let count_result = sqlx::query(&count_sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(count_result.get::<i32, _>(0) as usize)
}

fn gen_query_ssc_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select count(*) from {PDMS_SSC_ELEMENTS_TABLE} where owner = {}", refno.0));
    sql
}

fn gen_query_ssc_children_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {PDMS_SSC_ELEMENTS_TABLE} where owner = {}", refno.0));
    sql
}

fn gen_query_ssc_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {PDMS_SSC_ELEMENTS_TABLE} where type = 'WORL' ;"));
    sql
}

#[test]
fn test_set_ssc_tree() {
    let (sql, zone_map, zone_name_map) = set_ssc_node();
    println!("zone_map={:?}", zone_map);
}

#[test]
fn test_read_excel() {
    // let result = get_room_info_from_excel().unwrap();
    let (level, name_map) = get_room_level_from_excel().unwrap();
    println!("result={:?}", level);
    dbg!(&name_map);
}

