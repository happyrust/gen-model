//! Live staged RegenRoot release gate.
//!
//! Required: `AIOS_STAGED_REGEN_DB_FILE`, `AIOS_STAGED_REGEN_DBNUM`, and
//! `AIOS_STAGED_REGEN_ROOT` (a BRAN/HANG root whose pending session produces
//! TUBI plus boolean geometry). The target must be a disposable project copy.
//! Run with:
//! `cargo test --features http_api --test staged_regen_e2e -- --ignored --exact --nocapture`
//!
//! **已随 kv-mem 暂存窗口退役（ADR-056 P1，spec 035）**：它验证的暂存写回路径、
//! `staged_commit_metrics` 与 `active_staging_writes` 路由都已不存在，整文件停编译；
//! P3 T304 随 `staging/` 目录一起删除，issue #10「连续增量新增分支落进模型树」的
//! 直写替身在那一步补。
#![cfg(any())]

use std::sync::Arc;
use std::time::Instant;

use aios_core::{RefnoEnum, SUL_DB};
use aios_database::data_interface::batch_scheduler::{BatchScheduler, DiscoveredBatch};
use aios_database::data_interface::batch_worker::{drain_queue_until_empty, staged_commit_metrics};
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use pdms_io::io::PdmsIO;
use surrealdb::opt::{Config, auth::Root};

async fn connect_live() {
    let endpoint = std::env::var("AIOS_LIVE_WS").unwrap_or_else(|_| "ws://localhost:8009".into());
    let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
    let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());
    SUL_DB
        .connect((endpoint, Config::default().ast_payload()))
        .with_capacity(1000)
        .await
        .expect("connect live");
    SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
    SUL_DB
        .signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
}

async fn scalar_i32(sql: &str) -> i32 {
    let mut response = SUL_DB
        .query(sql)
        .await
        .expect("query")
        .check()
        .expect("valid query");
    response
        .take::<Option<i32>>(0)
        .expect("decode scalar")
        .expect("scalar exists")
}

async fn rows(sql: String) -> Vec<serde_json::Value> {
    let mut response = SUL_DB
        .query(sql)
        .await
        .expect("query model result")
        .check()
        .expect("valid model query");
    response.take(0).expect("decode model rows")
}

fn batch_result(task_id: &str) -> serde_json::Value {
    TaskRegistry::global()
        .get(task_id)
        .expect("batch task row")
        .result
        .expect("batch terminal result")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live: applies one pending BRAN/HANG RegenRoot through kv-mem staging"]
async fn staged_regen_persists_tubi_mesh_and_boolean_before_advancing_watermark() {
    assert!(
        std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").is_none(),
        "staged release gate requires GEN_MODEL_DIRECT_INCREMENT to be unset"
    );
    connect_live().await;

    let project =
        std::env::var("AIOS_STAGED_REGEN_PROJECT").unwrap_or_else(|_| "AvevaMarineSample".into());
    let db_file =
        std::env::var("AIOS_STAGED_REGEN_DB_FILE").expect("AIOS_STAGED_REGEN_DB_FILE is required");
    let dbnum: u32 = std::env::var("AIOS_STAGED_REGEN_DBNUM")
        .expect("AIOS_STAGED_REGEN_DBNUM is required")
        .parse()
        .expect("AIOS_STAGED_REGEN_DBNUM must be u32");
    let root = RefnoEnum::from(
        std::env::var("AIOS_STAGED_REGEN_ROOT")
            .expect("AIOS_STAGED_REGEN_ROOT is required")
            .as_str(),
    );
    let root_text = root.to_pdms_str();
    let root_key = root.to_pe_key();
    let root_u64 = root.refno().0;

    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum};"
    ))
    .await;
    let file_latest_sesno = PdmsIO::new(&project, &db_file, true)
        .get_latest_sesno()
        .expect("read source sesno") as i32;
    assert!(
        applied_sesno >= 1 && file_latest_sesno > applied_sesno,
        "fixture must contain one steady-state pending window: applied={applied_sesno}, file={file_latest_sesno}"
    );

    let found = DiscoveredBatch {
        intent: aios_database::data_interface::batch_queue::BatchIntent::ApplyWindow,
        project,
        dbnum,
        db_type: "DESI".into(),
        phase: aios_database::data_interface::initialization_phase::DataPhase::Design,
        epoch_id: 0,
        path: db_file.clone().into(),
        file_name: std::path::Path::new(&db_file)
            .file_name()
            .expect("db file name")
            .to_string_lossy()
            .into_owned(),
        applied_sesno,
        file_latest_sesno,
        // 探针没有预览步骤，基线取水位：整个待应用窗口都算新并入。
        previous_observed_sesno: applied_sesno,
        // 保存窗口两端的时刻只喂界面，这条链路不校验它，缺席即可（ADR-0019 降级路径）。
        first_pending_sesno_time: None,
        file_latest_sesno_time: None,
    };
    // 夹具扮演的是「有人真的动了这个库」，与 watch 事件同口径：不挂起，
    // 否则这一行会一直停在 held 上，下面的 drain 永远消费不到它。
    let outcome = BatchScheduler::global().enqueue(TaskRegistry::global(), &found, false);
    let task_id = outcome.info.task_id.clone();
    let commit_before = staged_commit_metrics();
    assert_eq!(
        commit_before["last_duration_ms"].as_u64(),
        Some(0),
        "a fresh release-gate process must not inherit a previous staged commit: {commit_before}"
    );
    let manager = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );

    let started = Instant::now();
    assert_eq!(
        drain_queue_until_empty(&manager).await,
        1,
        "release gate requires an otherwise empty queue"
    );
    println!(
        "[staged-regen-e2e] completed in {} ms",
        started.elapsed().as_millis()
    );

    let result = batch_result(&task_id);
    assert_eq!(result["status"], "success", "{result:#}");
    assert_eq!(result["batch"]["status"], "applied", "{result:#}");
    assert_eq!(
        result["warnings"].as_array().map(Vec::len),
        Some(0),
        "release gate must finish without warnings: {result:#}"
    );
    assert!(
        result["units"].as_array().is_some_and(|units| units
            .iter()
            .any(|unit| { unit["root_refno"] == root_text && unit["status"] == "Generated" })),
        "target root must finish Generated: {result:#}"
    );
    let commit_after = staged_commit_metrics();
    assert!(
        commit_after["last_duration_ms"]
            .as_u64()
            .unwrap_or_default()
            > 0,
        "staged commit metric must advance: {commit_before} -> {commit_after}"
    );
    assert_eq!(
        scalar_i32(&format!(
            "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{dbnum};"
        ))
        .await,
        file_latest_sesno
    );

    let tubi = rows(format!(
        "SELECT VALUE id FROM tubi_relate WHERE in = {root_key} OR anc CONTAINS {root_u64};"
    ))
    .await;
    let meshed = rows(format!(
        "SELECT VALUE out FROM geo_relate WHERE in IN \
         (SELECT VALUE out FROM inst_relate WHERE in = {root_key} OR anc CONTAINS {root_u64}) \
         AND out.meshed = true;"
    ))
    .await;
    let booled = rows(format!(
        "SELECT VALUE id FROM inst_relate WHERE \
         (in = {root_key} OR anc CONTAINS {root_u64}) AND (booled = true OR booled_id != NONE);"
    ))
    .await;
    assert!(!tubi.is_empty(), "target root must persist TUBI relations");
    assert!(
        !meshed.is_empty(),
        "target root must persist meshed inst_geo"
    );
    assert!(
        !booled.is_empty(),
        "target root must persist boolean output"
    );
}
