use std::collections::HashMap;
use aios_core::pdms_types::{EleTreeNode, RefU64};
use anyhow::anyhow;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use smol_str::SmolStr;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::Executor;
use serde::{Serialize, Deserialize};
use sqlx::mysql::MySqlRow;
use crate::consts::PDMS_SSC_ELEMENTS_TABLE;

// 房间信息 excel 字段
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

pub async fn insert_set_ssc_node_sql(pool: Pool<MySql>) -> anyhow::Result<()> {
    let insert_sql = "INSERT IGNORE INTO PDMS_SSC_ELEMENTS (ID, REFNO, TYPE, OWNER, NAME, ORDER_NUM) VALUES ";
    let sql = format!("{}{}", insert_sql, set_ssc_node());
    let mut conn = pool.acquire().await?;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
            dbg!(sql.as_str());
        }
    }
    Ok(())
}

pub fn set_ssc_node() -> String {
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
        let mut next_refno = nine_n_refno;
        for (level, room_names) in rooms {
            let mut order = 0;
            for room_name in room_names {
                match level.as_str() {
                    "1" => {
                        let (next, ni_node) = gen_insert_ssc_node_sql(next_refno, "SSC", cw_n_refno, room_name.as_str(), order);
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

    sql.remove(sql.len() - 1);
    sql.push_str(";");
    sql
}

pub fn gen_insert_ssc_node_sql(refno: RefU64, type_name: &str, owner: RefU64, name: &str, order_num: usize) -> (RefU64, String) {
    let mut sql = String::new();
    let refno_str = refno.to_refno_str().to_string();
    sql.push_str(&format!("({},'{refno_str}','{type_name}',{},'{name}',{order_num}),", refno.0, owner.0));

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
                let refno = RefU64(val.get::<i64, _>("id") as u64);
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
    let sql = set_ssc_node();
    println!("sql={:?}", sql);
}

#[test]
fn test_read_excel() {
    let result = get_room_info_from_excel().unwrap();
    println!("result={:?}", result);
}