use crate::api::element::gen_pdms_element_insert_sql;
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::versioned_db::database::SenderJsonsData;
#[cfg(feature = "surreal-save")]
use aios_core::SUL_DB;

use aios_core::db::*;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::tool::db_tool::db1_hash;
use config::File;
use dashmap::DashMap;
use dashmap::DashSet;
use futures::StreamExt;
use itertools::Itertools;
use log::{error, info};
use petgraph::Directed;
use petgraph::Undirected;
use petgraph::algo::all_simple_paths;
use petgraph::graph::Graph;
use petgraph::graph::NodeIndex;
use petgraph::graphmap::GraphMap;
use petgraph::graphmap::UnGraphMap;
use petgraph::prelude::DiGraphMap;
use petgraph::visit::IntoEdgesDirected;
use rayon::prelude::*;
#[cfg(feature = "sql")]
use sqlx::Executor;
#[cfg(feature = "sql")]
use sqlx::{MySql, Pool};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// dbnum_info_table 的元数据统计结构
///
/// 用于在解析阶段统计每个 ref_0 下的 PDMS 元素信息，
/// 之后通过 UPSERT 语句一次性写入到 SurrealDB
#[cfg(feature = "surreal-save")]
#[derive(Clone, Copy, Debug, Default)]
struct DbnumInfo {
    /// 数据库号，指示该数据来自哪个数据库 (dbnum)
    dbnum: i32,

    /// 该 ref_0 下的元素计数
    /// 初始值：解析过程中按 ref_0 累计的元素个数
    /// 作用：快速了解数据库中该 ref_0 有多少条记录
    count: i32,

    /// 该 ref_0 下的最大会话号 (sesno)
    /// 初始值：解析数据中该 ref_0 对应所有元素的最大 sesno
    /// 作用：用于版本控制和增量更新判断
    /// 示例：如果 max_sesno=15，表示最新修改是在会话号15
    max_sesno: i32,

    /// 该 ref_0 下最大的低32位参考号 (max_ref1)
    /// 初始值：从所有元素的 refno 低32位中取最大值
    /// 计算：ref_1 = (refno.0 & 0xFFFFFFFF) as u64
    /// 作用：记录该 ref_0 分组中最大的 ref_1，便于数据范围查询
    max_ref1: u64,
}

/// 保存element数据到版本管理
pub async fn save_pes(
    db_basic: &DbBasicData,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    db_num: i32,
    file_name: &str,
    db_type: &str,
    option: &DbOption,
    output: flume::Sender<SenderJsonsData>,
) -> anyhow::Result<()> {
    use itertools::Itertools;

    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let mut chunk_index = 0;

    /// 按 ref_0 统计元数据的 map
    /// 键：ref_0（RefU64 的高 32 位）
    /// 值：DbnumInfo - 该 ref_0 对应的统计信息
    ///
    /// 作用：在内存中累积 ref_0 对应的统计数据，
    /// 之后生成 UPSERT 语句一次性写入 SurrealDB
    #[cfg(feature = "surreal-save")]
    let mut dbnum_info_map: BTreeMap<u64, DbnumInfo> = BTreeMap::new();

    /// 用于按 db_num 分表的简化 PE 数据
    /// 键：db_num（数据库号）
    /// 值：Vec<String> - 简化后的 PE JSON 数据
    ///
    /// 简化数据包含字段：id, noun, children, name, owner
    /// 这些数据会写入 pe_{dbnum} 分表中
    #[cfg(feature = "surreal-save")]
    let mut pe_simple_by_dbnum: HashMap<i32, Vec<String>> = HashMap::new();

    for chunk in keys.chunks(option.pe_chunk as _) {
        #[cfg(feature = "surreal-save")]
        let mut insert_jsons = Vec::new();

        for &refno in chunk {
            let att_map = total_attr_map.get(&refno).unwrap();

            #[cfg(feature = "surreal-save")]
            let pe_data = att_map.pe(db_num);

            #[cfg(feature = "surreal-save")]
            {
                let mut json = pe_data.gen_sur_json(Some(refno.to_pe_key()));
                // 将 children 字段注入到 JSON（Surreal 对象字面量）中；若无子节点则为空数组
                let children_links = if let Some(children) = db_basic.children_map.get(&refno) {
                    if children.is_empty() {
                        String::from("")
                    } else {
                        children.iter().map(|c| c.to_pe_key()).join(", ")
                    }
                } else {
                    String::from("")
                };

                if json.ends_with('}') {
                    // 在末尾大括号前追加 children 字段
                    // 若已有其它字段，添加逗号分隔；这里直接在最后一个 '}' 之前插入
                    json.pop();
                    if json.ends_with('}') || json.contains(':') {
                        if !children_links.is_empty() {
                            json.push_str(&format!(", children: [{}]}}", children_links));
                        } else {
                            json.push_str(", children: []}");
                        }
                    } else {
                        if !children_links.is_empty() {
                            json.push_str(&format!("children: [{}]}}", children_links));
                        } else {
                            json.push_str("children: []}");
                        }
                    }
                }
                insert_jsons.push(json);

                // 生成简化版 PE 数据：只包含 id、noun、children、name、owner
                // 使用传入的 db_num 参数作为分表标识
                let owner_key = if pe_data.owner.is_none() {
                    "NONE".to_string()
                } else {
                    pe_data.owner.to_pe_key()
                };

                let simple_json = format!(
                    "{{id: {}, noun: '{}', children: [{}], name: '{}', owner: {}}}",
                    refno.to_pe_key(),
                    pe_data.noun.as_str(),
                    children_links,
                    pe_data
                        .name
                        .as_str()
                        .replace('\\', "\\\\")
                        .replace('\'', "\\'"),
                    owner_key
                );

                pe_simple_by_dbnum
                    .entry(db_num)
                    .or_insert_with(Vec::new)
                    .push(simple_json);

                /// 将 RefU64 分解为 ref_0 (高32位) 和 ref_1 (低32位)
                ///
                /// RefU64 的结构:
                /// - 完整值: 0xAAAAAAAABBBBBBBB (64位)
                /// - ref_0:   0xAAAAAAAA (高32位)
                /// - ref_1:   0xBBBBBBbb (低32位)
                ///
                /// 例子: refno = 0x0000445000266203
                /// - ref_0 = 0x00004450 = 17496
                /// - ref_1 = 0x00266203 = 2500099
                let refno_u64 = refno.0;
                let ref_0 = (refno_u64 >> 32) as u64;
                let ref_1 = (refno_u64 & 0xFFFFFFFF) as u64;

                /// 从元素数据中提取会话号
                let sesno = pe_data.sesno as i32;

                /// 按 ref_0 聚合统计信息
                ///
                /// 如果 ref_0 已存在：
                ///   - count += 1              (元素计数递增)
                ///   - max_sesno = max(max_sesno, sesno)  (取最大会话号)
                ///   - max_ref1 = max(max_ref1, ref_1)    (取最大 ref_1)
                ///
                /// 如果 ref_0 不存在：
                ///   创建新的 DbnumInfo 记录，初始值为当前元素的值
                dbnum_info_map
                    .entry(ref_0)
                    .and_modify(|info| {
                        info.count += 1;
                        info.max_sesno = info.max_sesno.max(sesno);
                        info.max_ref1 = info.max_ref1.max(ref_1);
                    })
                    .or_insert(DbnumInfo {
                        dbnum: db_num,
                        count: 1,
                        max_sesno: sesno,
                        max_ref1: ref_1,
                    });
            }

            #[cfg(not(feature = "surreal-save"))]
            {
                let _ = (att_map, refno);
            }
        }

        // 发送到 SurrealDB
        #[cfg(feature = "surreal-save")]
        {
            output
                .send_async(SenderJsonsData::PEJson(insert_jsons))
                .await
                .expect("send pes error");
        }

        chunk_index += 1;
    }

    /// 生成 dbnum_info_table 的 UPSERT 语句
    ///
    /// dbnum_info_table 用于存储每个 ref_0 下的元素统计信息
    ///
    /// 表结构说明：
    /// ┌─────────────────────────────────────────────────┐
    /// │ dbnum_info_table:{ref_0}                       │
    /// ├──────────────┬─────────────────────────────────┤
    /// │ dbnum        │ 数据库号 (i32)                   │
    /// │ count        │ 元素计数 (i32)                   │
    /// │ sesno        │ 最大会话号 (i32)                │
    /// │ max_ref1     │ 最大 ref_1 值 (u64)             │
    /// │ file_name    │ 源文件名 (String)               │
    /// │ db_type      │ 数据库类型 (String)             │
    /// └──────────────┴─────────────────────────────────┘
    ///
    /// UPSERT 逻辑：
    /// - 如果记录存在：增量更新字段
    /// - 如果记录不存在：创建新记录
    #[cfg(feature = "surreal-save")]
    {
        if !dbnum_info_map.is_empty() {
            let mut dbnum_info_updates = Vec::new();
            for (ref_0, info) in dbnum_info_map {
                /// UPSERT 语句详解：
                ///
                /// 语法：UPSERT dbnum_info_table:{ref_0} SET ...
                ///
                /// 字段说明：
                /// - dbnum = {dbnum}
                ///   直接设置数据库号，确保一致性
                ///
                /// - count = count?:0 + {count}
                ///   ?:0 表示如果字段不存在则默认为0
                ///   累加新增的元素数（支持增量更新）
                ///
                /// - sesno = math::max([sesno?:0, {max_sesno}])
                ///   取当前值和新值中的最大值
                ///   确保总是记录最新的会话号
                ///
                /// - max_ref1 = math::max([max_ref1?:0, {max_ref1}])
                ///   取当前值和新值中的最大值
                ///   确保总是记录最大的 ref_1
                ///
                /// - file_name = '{file_name}'
                ///   记录源文件名，便于追溯数据来源
                ///
                /// - db_type = '{db_type}'
                ///   记录数据库类型（DESI/CATA/DICT/SYST 等）
                let sql = format!(
                    "UPSERT dbnum_info_table:{} SET dbnum = {}, count = count?:0 + {}, sesno = math::max([sesno?:0, {}]), max_ref1 = math::max([max_ref1?:0, {}]), file_name = '{}', db_type = '{}';",
                    ref_0,
                    info.dbnum,
                    info.count,
                    info.max_sesno,
                    info.max_ref1,
                    file_name,
                    db_type
                );
                dbnum_info_updates.push(sql);
            }

            /// 分批发送 dbnum_info 更新到数据库
            /// 使用 chunking 避免单次 SQL 过大导致超时
            for chunk in dbnum_info_updates.chunks(option.pe_chunk as _) {
                output
                    .send_async(SenderJsonsData::DbnumInfoUpdate(chunk.to_vec()))
                    .await
                    .expect("send dbnum_info error");
            }
        }

        // 保存简化版PE数据到按db_num分表
        if !pe_simple_by_dbnum.is_empty() {
            info!(
                "开始保存简化PE数据到分表，共 {} 个表",
                pe_simple_by_dbnum.len()
            );

            for (dbnum, simple_jsons) in pe_simple_by_dbnum {
                // 使用 db_num 作为表名的一部分，形成 pe_{db_num} 这样的表名
                let table_name = format!("pe_{}", dbnum);

                // 分批插入到对应的分表
                for chunk in simple_jsons.chunks(option.pe_chunk as _) {
                    let insert_sql = format!("INSERT INTO {} [{}];", table_name, chunk.join(", "));

                    // 通过 output 发送 SQL，由接收端执行
                    output
                        .send_async(SenderJsonsData::PartitionedPEJson {
                            table_name: table_name.clone(),
                            sql: insert_sql,
                        })
                        .await
                        .expect("send partitioned pe data error");
                }

                info!("已发送 {} 条记录到表 {}", simple_jsons.len(), table_name);
            }
        }
    }

    Ok(())
}

#[cfg(all(test, feature = "surreal-save"))]
#[tokio::test]
async fn test_query_pe_with_children() -> anyhow::Result<()> {
    // 初始化 SurrealDB 连接
    let _ = aios_core::init_test_surreal().await;
    // 查询任意一条 pe 记录，验证 children 反序列化
    let sql = "SELECT * FROM pe LIMIT 1";
    let mut response = SUL_DB.query(sql).await?;
    let result: Vec<SPdmsElement> = response.take(0).unwrap_or_default();
    if let Some(pe) = result.get(0) {
        println!("refno={:?}, children={:?}", pe.refno(), pe.children);
    } else {
        println!("没有查询到 pe 记录，无法验证 children 字段");
    }
    Ok(())
}

#[cfg(feature = "sql")]
pub async fn save_pes_mysql(
    db_basic: &DbBasicData,
    project: &str,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
    project_maps: &HashMap<String, Pool<MySql>>,
    option: &DbOption,
    db_num: i32,
    output: &flume::Sender<String>,
) {
    let keys = total_attr_map.iter().map(|x| *x.key()).collect::<Vec<_>>();
    let debug_refnos: Vec<RefU64> = option
        .debug_model_refnos
        .as_ref()
        .map(|x| {
            x.iter()
                .map(|x| RefU64::from_str(x).unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let is_debug = !debug_refnos.is_empty();

    let children_map = &db_basic.children_map;

    for chunk in keys.chunks(option.pe_chunk as _) {
        let mut insert_sql = String::new();
        for &refno in chunk {
            if is_debug && !debug_refnos.contains(&refno) {
                continue;
            }
            let att_map = total_attr_map.get(&refno).unwrap();
            let sql = gen_pdms_element_insert_sql(att_map.value(), db_num, children_map);
            if !sql.is_empty() {
                insert_sql.push_str(&sql);
            }
        }
        let mut sql = format!(
            "INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB , ORDER_NUM,CHILDREN_COUNT, IS_DEL  ) VALUES {insert_sql}",
        );
        if option.replace_dbs {
            sql = sql.replace("INSERT IGNORE", "REPLACE");
        }
        sql.remove(sql.len() - 1);
        // output.send(MysqlSql((project.to_string(),sql))).expect("send pdmselement mysql sql failed");
        let Some(pool) = project_maps.get(project) else {
            continue;
        };
        let mut conn = pool.acquire().await.expect("get pool failed");
        match conn.execute(sql.as_str()).await {
            Ok(_) => {}
            Err(e) => {
                dbg!(e.to_string());
                dbg!(&sql);
            }
        }
    }
}

//使用insert relations 去保存图数据关联关系
#[cfg(feature = "surreal-save")]
pub async fn save_pe_relates(db_basic: &DbBasicData, output: flume::Sender<SenderJsonsData>) {
    let mut all_relate_jsons = vec![];
    for kv in &db_basic.children_map {
        let owner = kv.0;
        let children = kv.1;
        if children.is_empty() {
            continue;
        }
        let relate_json = children
            .iter()
            .enumerate()
            .map(|(i, child)| {
                let cp = child.to_pe_key();
                let op = owner.to_pe_key();
                format!("{{ id: pe_owner:[{1}, {i}], in: {0}, out: {1} }}", cp, op)
            })
            .collect::<Vec<String>>();
        all_relate_jsons.extend_from_slice(&relate_json);
        if all_relate_jsons.len() >= 500 {
            output
                .send(SenderJsonsData::PERelateJson(std::mem::take(
                    &mut all_relate_jsons,
                )))
                .expect("send pe_relates error");
        }
    }
    if !all_relate_jsons.is_empty() {
        output
            .send(SenderJsonsData::PERelateJson(std::mem::take(
                &mut all_relate_jsons,
            )))
            .expect("send pe_relates error");
    }
}

#[cfg(not(feature = "surreal-save"))]
pub async fn save_pe_relates(db_basic: &DbBasicData, output: flume::Sender<SenderJsonsData>) {
    let _ = db_basic;
    let _ = output;
}
