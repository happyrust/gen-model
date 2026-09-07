//! 一次性烟测：整库生成一遍，落进库，再按 Plant UI 的读法查回来。
//!
//! 它走的就是生产那两步（`E3dModelService::generate_dbnum` → `model_db_adapter::persist_dbnum`），
//! 中间没有别的路子，所以它绿了就说明这条链是通的。
//!
//! **介质由配置说了算，本程序不自己挑**（`in_memory_db` / `AIOS_IN_MEMORY_DB`，与 `run_app`
//! 同一个开关）：开着就接进程内嵌 kv-mem、跑完即消失、不碰任何外部库；关着就连
//! `DbOption.toml` 里那台 SurrealDB 真写。让程序自己挑介质，回执里那句「落在哪」立刻就会
//! 说谎——`PersistTarget` 的判据读的是配置。
//!
//! ```text
//! $env:AIOS_IN_MEMORY_DB="1"; cargo run --release --bin e3d_persist_smoke -- --dbnum 7997
//! ```
//!
//! `--chunk <n>` 拨落库的事务块大小（默认 `PERSIST_CHUNK`，`0` = 整根一笔的对照组）。
//!
//! `--root <a/b>` 换成**定向那一档**（`generate_and_persist_roots` → `persist_root`，
//! `model_refresh` 的生产路径），整库那一档整个跳过。两档在落库上不是同一个口径：定向按
//! `RoomEffectPolicy::Directed` 排房间重算，整库按 ADR-010 §4 收窄 1 只清边——只跑整库那一档，
//! 房间重算那一支一次都走不到。
//!
//! 它是**有尽头的一次性命令**，不驻留、不监听。

use std::time::Instant;

use anyhow::Context;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use aios_database::fast_model::e3d_model_service::E3dModelService;
use aios_database::fast_model::model_db_adapter;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut dbnum = 7997u32;
    let mut chunk = model_db_adapter::PERSIST_CHUNK;
    let mut root: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dbnum" => {
                dbnum = args
                    .next()
                    .context("--dbnum 少了取值")?
                    .parse()
                    .context("--dbnum 不是数字")?;
            }
            "--chunk" => {
                let value: usize = args
                    .next()
                    .context("--chunk 少了取值")?
                    .parse()
                    .context("--chunk 不是数字")?;
                // 0 = 不切，整根一笔——量切块买到了什么，得有个对照组。
                chunk = if value == 0 { usize::MAX } else { value };
            }
            "--root" => {
                root = Some(args.next().context("--root 少了取值")?);
            }
            other => {
                anyhow::bail!("认不得的参数 {other}；只有 --dbnum <n>、--chunk <n> 与 --root <a/b>")
            }
        }
    }

    let db_option = aios_core::get_db_option();
    if aios_database::options::in_memory_db() {
        aios_core::SUL_DB.connect("mem://").await?;
        aios_core::SUL_DB
            .use_ns(&db_option.surreal_ns)
            .use_db(&db_option.project_name)
            .await?;
        println!("介质：进程内嵌 kv-mem（in_memory_db=true，跑完即消失）");
    } else {
        aios_core::init_surreal()
            .await
            .with_context(|| format!("连 {}", db_option.get_version_db_conn_str()))?;
        println!(
            "介质：{}（外部库，真写）",
            db_option.get_version_db_conn_str()
        );
    }

    if let Some(root) = root {
        return directed(dbnum, &root).await;
    }

    let started = Instant::now();
    let service = E3dModelService::from_current()
        .await
        .context("打开生成服务")?;
    let report = service
        .generate_dbnum(
            dbnum,
            aios_database::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
        )
        .await
        .with_context(|| format!("生成 {dbnum}"))?;
    let generated_at = started.elapsed();

    let persisted = model_db_adapter::persist_dbnum_in_chunks(dbnum, chunk)
        .await
        .with_context(|| format!("落库 {dbnum}"))?;
    let persisted_at = started.elapsed();

    println!(
        "GENERATE dbnum={dbnum} upserted={} removed={} shared={} baked={} unique_meshes={} failed={} 用时={:?}",
        report.upserted,
        report.removed,
        report.shared_instances,
        report.baked_instances,
        report.unique_meshes,
        report.failed,
        generated_at,
    );
    println!(
        "PERSIST 介质={:?} 可持久={} 块大小={} 事务={} 落行={} 对账删={} 盖章={} 用时={:?}",
        persisted.medium,
        persisted.medium.is_durable(),
        if chunk == usize::MAX {
            "整根一笔".to_string()
        } else {
            chunk.to_string()
        },
        persisted.commits,
        persisted.upserted,
        persisted.removed,
        persisted.receipts,
        persisted_at - generated_at,
    );

    read_back(dbnum).await
}

/// 定向那一档：一根，走 `model_refresh` 的生产两步（生成进投影 → [`model_db_adapter::persist_root`]）。
///
/// 与整库那一档的区别不只是范围：定向落库按 `RoomEffectPolicy::Directed` 排房间重算，整库落库按
/// ADR-010 §4 收窄 1 只清边。房间那一支只有这条路走得到。
async fn directed(dbnum: u32, root: &str) -> anyhow::Result<()> {
    let started = Instant::now();
    let roots = vec![root.to_string()];
    let service = E3dModelService::from_current()
        .await
        .context("打开生成服务")?;
    let report = service
        .generate_and_persist_roots(
            dbnum,
            &roots,
            aios_database::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
        )
        .await
        .with_context(|| format!("生成根 {root}"))?;
    let generated_at = started.elapsed();

    let refno = aios_database::fast_model::e3d_model_service::parse_refno(root)?;
    let persisted = model_db_adapter::persist_root(refno)
        .await
        .with_context(|| format!("落库根 {root}"))?;
    println!(
        "GENERATE root={root} dbnum={dbnum} upserted={} removed={} failed={} 用时={:?}",
        report.upserted, report.removed, report.failed, generated_at,
    );
    println!(
        "PERSIST 介质={:?} 事务={} 落行={} 对账删={} 盖章={} 用时={:?}",
        persisted.medium,
        persisted.commits,
        persisted.upserted,
        persisted.removed,
        persisted.receipts,
        started.elapsed() - generated_at,
    );
    Ok(())
}

/// 按 Plant UI 的读法查回来：平表快路要的三个行内副本、实例子查询里的本地变换、管身那条边。
/// 库里查不回来的东西，生成报表说得再好听也没用。
async fn read_back(dbnum: u32) -> anyhow::Result<()> {
    let db: &Surreal<Any> = &aios_core::SUL_DB;
    let mut response = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={dbnum} AND direct_model.source='e3d-model'));\
             RETURN array::len((SELECT VALUE id FROM tubi_relate WHERE dbnum={dbnum} AND direct_model.source='e3d-model'));\
             RETURN array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={dbnum} AND direct_model.source='e3d-model' AND aabb_d != NONE AND world_trans_d != NONE AND insts_flat != NONE));\
             RETURN array::len((SELECT VALUE id FROM geo_relate WHERE transform != NONE));\
             RETURN array::len((SELECT VALUE id FROM inst_geo));"
        ))
        .await?
        .check()?;
    let inst = response.take::<Option<i64>>(0)?.unwrap_or_default();
    let tubi = response.take::<Option<i64>>(1)?.unwrap_or_default();
    let flat_ready = response.take::<Option<i64>>(2)?.unwrap_or_default();
    let edges = response.take::<Option<i64>>(3)?.unwrap_or_default();
    let meshes = response.take::<Option<i64>>(4)?.unwrap_or_default();
    println!(
        "READBACK inst_relate={inst} tubi_relate={tubi} 平表三件齐活={flat_ready} geo_relate带本地变换={edges} inst_geo={meshes}"
    );
    anyhow::ensure!(inst > 0 || tubi > 0, "落库之后一行都查不到");
    anyhow::ensure!(
        flat_ready == inst,
        "有 {} 行缺行内副本，会整行退回 slim 兜底",
        inst - flat_ready
    );
    Ok(())
}
