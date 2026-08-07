//! 一次性实机探针（2026-08-08）：把 7997@194（PANE =24381/35844 纯位姿会话）
//! 以**窗口重放**方式过一遍 kv-mem 暂存窗口，观察 W1 祖先解析式预载日志与产物落库。
//!
//! 前置（由操作者保证）：
//! - `dbnum_watermark:7997` 已回拨到 193（file_latest 仍是 194）；
//! - 常驻服务可以在跑别的批次（队列是进程内的，本探针在自己进程里单写 7997；
//!   服务里排队的 7997 副本等它轮到时按水位判定 up-to-date 无操作）；
//! - `GEN_MODEL_DIRECT_INCREMENT` 未设置（走暂存轨）。
//!
//! 观察点（stdout，`--nocapture`）：
//! - `窗口内模型计划 dbnum=7997 … Transform …`
//! - `暂存 mutation 预载: …`（产物拷贝桶）
//! - `暂存祖先预载: seeds=… elements=… written=…`（W1 解析式预载）
//! - 收口后水位推进 + world_trans 指针翻新且可解引用 + AABB 值与重放前逐字节一致
//!   （重放收敛：内容等价，指针是新的）。
//!
//! ```text
//! $env:RUST_MIN_STACK = "134217728"
//! cargo test --features http_api --test staged_pane_replay_probe -- --ignored --exact --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use aios_core::SUL_DB;
use aios_database::data_interface::batch_scheduler::{BatchScheduler, DiscoveredBatch};
use aios_database::data_interface::batch_worker::{drain_queue_until_empty, staged_commit_metrics};
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use pdms_io::io::PdmsIO;
use surrealdb::opt::{Config, auth::Root};

const PROJECT: &str = "AvevaMarineSample";
const DBNUM: u32 = 7997;
const PANE: &str = "24381_35844";

async fn connect_live() {
    let endpoint = std::env::var("AIOS_LIVE_WS").unwrap_or_else(|_| "ws://localhost:8009".into());
    let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
    let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| PROJECT.into());
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

async fn world_trans_id() -> Option<String> {
    let mut response = SUL_DB
        .query(format!(
            "RETURN record::id(inst_relate:{PANE}.world_trans);"
        ))
        .await
        .expect("query world_trans id")
        .check()
        .expect("valid world_trans query");
    response.take(0).expect("decode world_trans id")
}

async fn world_trans_resolvable() -> bool {
    let mut response = SUL_DB
        .query(format!("RETURN inst_relate:{PANE}.world_trans.d != NONE;"))
        .await
        .expect("query world_trans deref")
        .check()
        .expect("valid world_trans deref query");
    response
        .take::<Option<bool>>(0)
        .expect("decode deref")
        .unwrap_or(false)
}

async fn aabb_string() -> Option<String> {
    let mut response = SUL_DB
        .query(format!("RETURN <string>inst_relate:{PANE}.aabb.d;"))
        .await
        .expect("query aabb")
        .check()
        .expect("valid aabb query");
    response.take(0).expect("decode aabb")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual live probe: replays 7997@194 through the staged window in this process"]
async fn staged_pane_replay_goes_through_the_kvmem_window() {
    assert!(
        std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").is_none(),
        "本探针针对暂存路径：不要设置 GEN_MODEL_DIRECT_INCREMENT"
    );
    connect_live().await;

    let db_file = std::env::var("AIOS_STAGED_E2E_DB_FILE").unwrap_or_else(|_| {
        format!("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams{DBNUM}_0001")
    });

    let mgr = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .expect("init db manager"),
    );

    let applied_sesno = scalar_i32(&format!(
        "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{DBNUM};"
    ))
    .await;
    let file_latest_sesno = PdmsIO::new(PROJECT, &db_file, true)
        .get_latest_sesno()
        .expect("read live file sesno") as i32;
    assert!(
        file_latest_sesno > applied_sesno,
        "水位必须先回拨出一个待重放窗口 file={file_latest_sesno} applied={applied_sesno}"
    );
    println!("[pane-replay] 窗口 {}..={file_latest_sesno}", applied_sesno + 1);

    let before_trans = world_trans_id().await.expect("基线必须有 world_trans 指针");
    let before_aabb = aabb_string().await.expect("基线必须有 AABB");
    println!("[pane-replay] 基线 world_trans={before_trans}");
    println!("[pane-replay] 基线 aabb={before_aabb}");

    let found = DiscoveredBatch {
        project: PROJECT.to_string(),
        dbnum: DBNUM,
        db_type: "DESI".to_string(),
        path: std::path::PathBuf::from(&db_file),
        file_name: std::path::Path::new(&db_file)
            .file_name()
            .expect("db file name")
            .to_string_lossy()
            .into_owned(),
        applied_sesno,
        file_latest_sesno,
    };
    let outcome = BatchScheduler::global().enqueue(TaskRegistry::global(), &found);
    println!("[pane-replay] 入队: {outcome:?}");
    let task_id = outcome.info.task_id.clone();

    let started = Instant::now();
    let ran = drain_queue_until_empty(&mgr).await;
    println!(
        "[pane-replay] 消费了 {ran} 个批次，耗时 {} ms",
        started.elapsed().as_millis()
    );
    assert_eq!(ran, 1, "本进程队列里只应有本探针排的这一个批次");

    let entry = TaskRegistry::global().get(&task_id).expect("任务行在册");
    let result = entry
        .result
        .unwrap_or_else(|| panic!("本批必须已进终态（当前 {:?}）", entry.state));
    println!(
        "[pane-replay] 终态: {}",
        serde_json::to_string_pretty(&result).expect("render result")
    );
    assert_eq!(
        result["status"].as_str(),
        Some("success"),
        "本批终态必须成功: {result:#}"
    );
    // 此前 fail-closed 的窗口留下过 durable attempt 记录（prepare_attempt 在首写前
    // 落、finalize 才清），本轮会如实带出一条「replay unfinished range」——它是
    // 预期的恢复语义；除它之外不许有任何告警。
    let unexpected: Vec<&str> = result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .filter_map(|warning| warning.as_str())
        .filter(|warning| !warning.contains("replay unfinished range"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "本批除 attempt 恢复提示外不许带告警: {unexpected:?}"
    );
    assert_eq!(
        result["batch"]["status"].as_str(),
        Some("applied"),
        "数据必须真的落库并推进水位: {}",
        result["batch"]
    );

    let commit = staged_commit_metrics();
    assert!(
        commit["last_duration_ms"].as_u64().unwrap_or(0) > 0,
        "本批没有经过暂存写回: {commit}"
    );
    println!("[pane-replay] 暂存写回指标: {commit}");

    assert_eq!(
        scalar_i32(&format!(
            "SELECT VALUE applied_sesno FROM ONLY dbnum_watermark:{DBNUM};"
        ))
        .await,
        file_latest_sesno,
        "批次成功后水位必须推进到文件最新会话"
    );

    let after_trans = world_trans_id().await.expect("重放后 world_trans 指针必须在场");
    assert_ne!(
        after_trans, before_trans,
        "Transform 刷新必须改指新 trans 记录（重放也一样）"
    );
    assert!(
        world_trans_resolvable().await,
        "world_trans 必须指向持久层里存在的 trans 记录（不悬空）"
    );

    let after_aabb = aabb_string().await.expect("重放后 AABB 必须在场");
    assert_eq!(
        after_aabb, before_aabb,
        "重放收敛：AABB 值必须与重放前逐字节一致（文件态没变，只是指针翻新）"
    );

    println!(
        "[pane-replay] PASS: world_trans {before_trans} -> {after_trans}，AABB 收敛一致，水位 {applied_sesno} -> {file_latest_sesno}"
    );
}
