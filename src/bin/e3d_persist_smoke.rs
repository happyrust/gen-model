//! 一次性烟测：整库生成一遍，落进库，再按 Plant UI 的读法查回来。
//!
//! **生成那一步走的就是生产那条路**：`generation_roots_of` 取根 →
//! `E3dModelService::generate_and_persist_roots`（`model_refresh` 用的并行生产者 + 串行
//! 消费者）。此前它走的是已退役的 `generate_dbnum`，那条逐根串行、不过几何并发闸，量出来
//! 的生成耗时跟生产对不上（闸利用率实测 0.0%，换过来之后 6.2%）。
//!
//! **落库那一步是「等」，不是「写」**：持久介质上 durable 行只过 write-behind 那道缝
//! （ADR-071），投影一提交后台 worker 就在刷了，所以这里只
//! `model_db_adapter::wait_until_model_rows_are_durable` 等它刷完——一次性命令跑完就退，
//! 不等这一下进程一没行只落一半。此前这里直接调 `persist_dbnum`，那是第二个写者，跟后台
//! worker 抢同一批行，热跑 5 次挂 2 次（`read or write conflict` 整笔回退）。内存介质那一档
//! 没有 write-behind，由那个函数自己补落，抢不起来。
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
//! `--root <a/b>` 换成**定向那一档**（`generate_and_persist_roots` → `persist_root`，
//! `model_refresh` 的生产路径），整库那一档整个跳过。两档在落库上不是同一个口径：定向按
//! `RoomEffectPolicy::Directed` 排房间重算，整库按 ADR-010 §4 收窄 1 只清边——只跑整库那一档，
//! 房间重算那一支一次都走不到。
//!
//! 它是**有尽头的一次性命令**，不驻留、不监听。

use std::time::{Duration, Instant};

use anyhow::Context;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use aios_database::fast_model::e3d_model_service::E3dModelService;
use aios_database::fast_model::model_db_adapter;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut dbnum = 7997u32;
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
            "--root" => {
                root = Some(args.next().context("--root 少了取值")?);
            }
            other => {
                anyhow::bail!("认不得的参数 {other}；只有 --dbnum <n> 与 --root <a/b>")
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
    // 几何并发闸的利用率：生成慢是「一根太肥、并行不起来」还是「根本不在几何上」，
    // 这一对快照的差值直接答得出来，不必另外插桩。
    let roots = service
        .generation_roots_of(dbnum)
        .await
        .with_context(|| format!("枚举 {dbnum} 的生成根"))?;
    let gate_before = aios_database::fast_model::concurrency::snapshot();
    let report = service
        .generate_and_persist_roots(
            dbnum,
            &roots,
            aios_database::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
        )
        .await
        .with_context(|| format!("生成 {dbnum}"))?;
    let generated_at = started.elapsed();
    let gate_after = aios_database::fast_model::concurrency::snapshot();

    let flushed =
        model_db_adapter::wait_until_model_rows_are_durable(dbnum, Duration::from_secs(600))
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
        "GATE 额度={} 几何段利用率={} 等许可={:?}",
        gate_after.quota,
        gate_after
            .utilization_since(&gate_before)
            .map_or("n/a".to_string(), |value| format!("{:.1}%", value * 100.0)),
        Duration::from_micros(
            gate_after
                .permit_wait_micros
                .saturating_sub(gate_before.permit_wait_micros)
        ),
    );
    // 这个「用时」是**生成收尾之后还要等的那一截**，不是落库总耗时：持久介质上
    // write-behind 在生成还没跑完时就已经在刷了，那一部分被生成盖住、量不出来。
    println!(
        "PERSIST 介质={:?} 可持久={} 根={} 收尾用时={:?}",
        flushed.medium,
        flushed.medium.is_durable(),
        flushed.roots,
        persisted_at - generated_at,
    );

    read_back(dbnum).await
}

/// 定向那一档：只把一根交给同一条生产路径，落库照旧等 write-behind。
///
/// 与整库那一档的差别只剩范围。两档落库曾经不同口径（定向按 `RoomEffectPolicy::Directed` 排
/// 房间重算），但那两个形参在房间流程退役时就成了空壳、现已删除，今天两档走的是同一套清边。
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

    let flushed =
        model_db_adapter::wait_until_model_rows_are_durable(dbnum, Duration::from_secs(600))
            .await
            .with_context(|| format!("落库根 {root}"))?;
    println!(
        "GENERATE root={root} dbnum={dbnum} upserted={} removed={} failed={} 用时={:?}",
        report.upserted, report.removed, report.failed, generated_at,
    );
    println!(
        "PERSIST 介质={:?} 根={} 收尾用时={:?}",
        flushed.medium,
        flushed.roots,
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
            "RETURN array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={dbnum} AND model_sesno != NONE));\
             RETURN array::len((SELECT VALUE id FROM tubi_relate WHERE dbnum={dbnum} AND model_sesno != NONE));\
             RETURN array::len((SELECT VALUE id FROM inst_relate WHERE dbnum={dbnum} AND model_sesno != NONE AND aabb_d != NONE AND world_trans_d != NONE AND insts_flat != NONE));\
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
