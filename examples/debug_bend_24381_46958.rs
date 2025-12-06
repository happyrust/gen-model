//! BEND 24381/46958 调试脚本
//!
//! 运行: cargo run --example debug_bend_24381_46958

use aios_core::{RefnoEnum, SUL_DB, SurrealQueryExt, init_surreal_with_retry, init_test_surreal};
use aios_core::utils::RecordIdExt;
use anyhow::Result;
use serde::Deserialize;
use surrealdb::types::{self as surrealdb_types, SurrealValue};

#[derive(Debug, Deserialize, SurrealValue)]
struct BendBasicRow {
    noun: Option<String>,
    name: Option<String>,
    #[serde(rename = "spre_value")]
    spre_value: Option<surrealdb_types::RecordId>,
    #[serde(rename = "catr_value")]
    catr_value: Option<surrealdb_types::RecordId>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SpreChainRow {
    #[serde(rename = "spre")]
    spre: Option<surrealdb_types::RecordId>,
    #[serde(rename = "spre_noun")]
    spre_noun: Option<String>,
    #[serde(rename = "spre_name")]
    spre_name: Option<String>,
    #[serde(rename = "scom_catr")]
    scom_catr: Option<surrealdb_types::RecordId>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct NodeRow {
    id: Option<surrealdb_types::RecordId>,
    noun: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ScomRow {
    #[serde(rename = "GMRE")]
    gmre: Option<surrealdb_types::RecordId>,
    #[serde(rename = "PTRE")]
    ptre: Option<surrealdb_types::RecordId>,
    #[serde(rename = "DTRE")]
    dtre: Option<surrealdb_types::RecordId>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct GroupRow {
    noun: Option<String>,
    cnt: Option<i64>,
    names: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SslcRow {
    id: Option<surrealdb_types::RecordId>,
    noun: Option<String>,
    name: Option<String>,
    #[serde(rename = "pxts")]
    pxts: Option<f64>,
    #[serde(rename = "pyts")]
    pyts: Option<f64>,
    #[serde(rename = "pxbs")]
    pxbs: Option<f64>,
    #[serde(rename = "pybs")]
    pybs: Option<f64>,
    #[serde(rename = "pdia")]
    pdia: Option<f64>,
    #[serde(rename = "phei")]
    phei: Option<f64>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct OwnerRow {
    id: Option<RefnoEnum>,
    noun: Option<String>,
    name: Option<String>,
    #[serde(rename = "owner_id")]
    owner_id: Option<RefnoEnum>,
    #[serde(rename = "owner_noun")]
    owner_noun: Option<String>,
}

fn fmt_record(id: &Option<surrealdb_types::RecordId>) -> String {
    id.as_ref()
        .map(|r| r.to_raw())
        .unwrap_or_else(|| "-".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    // 连接数据库
    let db_option = init_test_surreal().await?;
    init_surreal_with_retry(&db_option).await?;

    let bend_refno = "24381_46958";

    println!("\n========== BEND {} 数据结构分析 ==========\n", bend_refno);

    // 1. 查询 BEND 基本信息
    println!("### 1. 查询 BEND 基本信息和 SPRE/CATR 引用 ###");
    let sql = "SELECT noun, name, refno.SPRE as spre_value, refno.CATR as catr_value FROM pe:24381_46958";
    let basic_rows: Vec<BendBasicRow> = SUL_DB.query_take(sql, 0).await?;
    println!("BEND 基本信息:");
    if basic_rows.is_empty() {
        println!("  ❌ 未找到 BEND 记录");
    }
    for row in &basic_rows {
        println!(
            "  noun={:?} name={:?} SPRE={} CATR={}",
            row.noun,
            row.name,
            fmt_record(&row.spre_value),
            fmt_record(&row.catr_value)
        );
    }

    // 2. 查询 SPRE 引用的 SCOM
    println!("\n### 2. 查询 SPRE 引用链 ###");
    let sql = "SELECT refno.SPRE as spre, refno.SPRE.noun as spre_noun, refno.SPRE.name as spre_name, refno.SPRE.refno.CATR as scom_catr FROM pe:24381_46958";
    let spre_rows: Vec<SpreChainRow> = SUL_DB.query_take(sql, 0).await?;
    println!("SPRE 引用链:");
    if spre_rows.is_empty() {
        println!("  ❌ 未找到 SPRE 引用");
    }
    for row in &spre_rows {
        println!(
            "  SPRE={} noun={:?} name={:?} CATR={}",
            fmt_record(&row.spre),
            row.spre_noun,
            row.spre_name,
            fmt_record(&row.scom_catr)
        );
    }

    // 2.5 检查 CATR 节点本身是否存在
    println!("\n### 2.5 检查 CATR 节点 (pe:15194_5835) ###");
    let sql = "SELECT * FROM pe:15194_5835";
    let catr_rows: Vec<NodeRow> = SUL_DB.query_take(sql, 0).await?;
    println!("CATR 节点信息:");
    if catr_rows.is_empty() {
        println!("  ❌ CATR 节点不存在！");
    }
    for item in &catr_rows {
        println!(
            "  id={} noun={:?} name={:?}",
            fmt_record(&item.id),
            item.noun,
            item.name
        );
    }

    // 2.6 检查 SCOM 属性表 (SCOM:15194_5835)
    println!("\n### 2.6 检查 SCOM 属性表 (SCOM:15194_5835) ###");
    let sql = "SELECT * FROM SCOM:15194_5835";
    let scom_rows: Vec<ScomRow> = SUL_DB.query_take(sql, 0).await.unwrap_or_default();
    println!("SCOM 属性信息:");
    if scom_rows.is_empty() {
        println!("  ❌ SCOM 属性记录不存在！");
    }

    let mut gmre_id: Option<surrealdb_types::RecordId> = None;
    let mut ptre_id: Option<surrealdb_types::RecordId> = None;
    let mut dtre_id: Option<surrealdb_types::RecordId> = None;

    for row in &scom_rows {
        println!(
            "  GMRE={} PTRE={} DTRE={}",
            fmt_record(&row.gmre),
            fmt_record(&row.ptre),
            fmt_record(&row.dtre)
        );
        if gmre_id.is_none() {
            gmre_id = row.gmre.clone();
        }
        if ptre_id.is_none() {
            ptre_id = row.ptre.clone();
        }
        if dtre_id.is_none() {
            dtre_id = row.dtre.clone();
        }
    }

    // 3. 查询 SCOM(CATR) 下的所有子元素类型
    println!("\n### 3. 查询 SCOM(CATR) 下的所有子元素类型 ###");
    let sql = "SELECT noun, count() as cnt, array::group(name) as names FROM pe WHERE owner = (SELECT VALUE refno.SPRE.refno.CATR FROM pe:24381_46958)[0] GROUP BY noun";
    let group_rows: Vec<GroupRow> = SUL_DB.query_take(sql, 0).await.unwrap_or_default();
    println!("SCOM 子元素类型统计:");
    if group_rows.is_empty() {
        println!("  ❌ 没有找到子元素！可能 CATR 引用无效");
    }
    for row in &group_rows {
        println!(
            "  noun={:?} cnt={:?} names={:?}",
            row.noun, row.cnt, row.names
        );
    }

    // 3.1 检查 GMRE
    if let Some(id) = gmre_id {
        println!("\n### 3.1 检查 GMRE ({}) ###", id.to_raw());
        let sql = format!("SELECT * FROM {}", id.to_raw());
        let rows: Vec<NodeRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("GMRE 节点信息:");
        if rows.is_empty() {
            println!("  ❌ GMRE 节点不存在");
        }
        for row in &rows {
            println!(
                "  id={} noun={:?} name={:?}",
                fmt_record(&row.id),
                row.noun,
                row.name
            );
        }

        let sql = format!(
            "SELECT noun, count() as cnt FROM pe WHERE owner = {} GROUP BY noun",
            id.to_raw()
        );
        let stats: Vec<GroupRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("GMRE 子元素统计:");
        for s in &stats {
            println!("  noun={:?} cnt={:?}", s.noun, s.cnt);
        }
    }

    // 3.2 检查 PTRE
    if let Some(id) = ptre_id {
        println!("\n### 3.2 检查 PTRE ({}) ###", id.to_raw());
        let sql = format!("SELECT * FROM {}", id.to_raw());
        let rows: Vec<NodeRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("PTRE 节点信息:");
        if rows.is_empty() {
            println!("  ❌ PTRE 节点不存在");
        }
        for row in &rows {
            println!(
                "  id={} noun={:?} name={:?}",
                fmt_record(&row.id),
                row.noun,
                row.name
            );
        }

        let sql = format!(
            "SELECT noun, count() as cnt FROM pe WHERE owner = {} GROUP BY noun",
            id.to_raw()
        );
        let stats: Vec<GroupRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("PTRE 子元素统计:");
        for s in &stats {
            println!("  noun={:?} cnt={:?}", s.noun, s.cnt);
        }
    }

    // 3.3 检查 DTRE
    if let Some(id) = dtre_id {
        println!("\n### 3.3 检查 DTRE ({}) ###", id.to_raw());
        let sql = format!("SELECT * FROM {}", id.to_raw());
        let rows: Vec<NodeRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("DTRE 节点信息:");
        if rows.is_empty() {
            println!("  ❌ DTRE 节点不存在");
        }
        for row in &rows {
            println!(
                "  id={} noun={:?} name={:?}",
                fmt_record(&row.id),
                row.noun,
                row.name
            );
        }

        let sql = format!(
            "SELECT noun, count() as cnt FROM pe WHERE owner = {} GROUP BY noun",
            id.to_raw()
        );
        let stats: Vec<GroupRow> = SUL_DB.query_take(&sql, 0).await.unwrap_or_default();
        println!("DTRE 子元素统计:");
        for s in &stats {
            println!("  noun={:?} cnt={:?}", s.noun, s.cnt);
        }
    }

    // 4. 查询 SSLC 子元素及其 shear angles 属性
    println!("\n### 4. 查询 SSLC 子元素及 shear angles ###");
    let sql = "SELECT id, noun, name, refno.PXTS as pxts, refno.PYTS as pyts, refno.PXBS as pxbs, refno.PYBS as pybs, refno.PDIA as pdia, refno.PHEI as phei FROM pe WHERE owner = (SELECT VALUE refno.SPRE.refno.CATR FROM pe:24381_46958)[0] AND noun = 'SSLC'";
    let sslc_rows: Vec<SslcRow> = SUL_DB.query_take(sql, 0).await.unwrap_or_default();
    println!("SSLC 子元素:");
    if sslc_rows.is_empty() {
        println!("  ❌ 未找到 SSLC");
    } else {
        println!("找到 {} 个 SSLC:", sslc_rows.len());
    }
    for sslc in &sslc_rows {
        println!(
            "  id={} name={:?} pxts={:?} pyts={:?} pxbs={:?} pybs={:?} pdia={:?} phei={:?}",
            fmt_record(&sslc.id),
            sslc.name,
            sslc.pxts,
            sslc.pyts,
            sslc.pxbs,
            sslc.pybs,
            sslc.pdia,
            sslc.phei
        );
    }

    // 5. 直接查询 BEND 的 owner chain
    println!("\n### 5. 查询 BEND 的完整 owner 链 ###");
    let sql = "SELECT id, noun, name, owner.id as owner_id, owner.noun as owner_noun FROM pe:24381_46958";
    let owner_rows: Vec<OwnerRow> = SUL_DB.query_take(sql, 0).await?;
    println!("Owner 链:");
    if owner_rows.is_empty() {
        println!("  ❌ 未查询到 owner 链");
    }
    for row in &owner_rows {
        println!(
            "  id={:?} noun={:?} owner={:?} owner_noun={:?}",
            row.id, row.noun, row.owner_id, row.owner_noun
        );
    }

    println!("\n========== 分析完成 ==========\n");

    Ok(())
}
