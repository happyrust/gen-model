//! 一次性烟测：整库生成一遍，落进库，再按 Plant UI 的读法查回来。
//!
//! **生成那一步走的就是生产那条路**：`generation_roots_of` 取根（每个 ZONE 一根，不上溯
//! SITE；`--roots file|site` 是对照档）→ `E3dModelService::generate_and_persist_roots`
//! （`model_refresh` 用的并行生产者 + 串行消费者）。此前它走的是已退役的 `generate_dbnum`，
//! 那条逐根串行、不过几何并发闸，量出来的生成耗时跟生产对不上（闸利用率实测 0.0%，换过来
//! 之后 6.2%；根换成 ZONE 之后 39%）。
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
    let mut batched = false;
    let mut analyze = false;
    let mut bindbench = false;
    let mut bind_rows = 2_000usize;
    let mut bind_rounds = 3usize;
    let mut bind_endpoint: Option<String> = None;
    let mut bind_noindex = false;
    let mut roots_level = String::from("zone");
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
            "--batched" => batched = true,
            "--analyze" => analyze = true,
            "--bindbench" => bindbench = true,
            "--bind-rows" => {
                bind_rows = args
                    .next()
                    .context("--bind-rows 少了取值")?
                    .parse()
                    .context("--bind-rows 不是数字")?;
            }
            "--bind-rounds" => {
                bind_rounds = args
                    .next()
                    .context("--bind-rounds 少了取值")?
                    .parse()
                    .context("--bind-rounds 不是数字")?;
            }
            "--bind-endpoint" => {
                bind_endpoint = Some(args.next().context("--bind-endpoint 少了取值")?);
            }
            "--bind-noindex" => bind_noindex = true,
            "--roots" => {
                roots_level = args.next().context("--roots 少了取值（zone | file | site）")?;
            }
            other => {
                anyhow::bail!(
                    "认不得的参数 {other}；只有 --dbnum <n>、--root <a/b>、--roots <zone|file|site>、\
                     --batched、--analyze 与 --bindbench（配 --bind-rows / --bind-rounds / --bind-endpoint）"
                )
            }
        }
    }

    // 只放行两条账：落库适配器的 per-group（`model publication group settled`）与生成服务的
    // per-root（`model root generated`）。整条 Info 全开会把 7997 这种体量的运行淹掉，也会
    // 自己成为一项开销；这两条一根/一组一行，7997 上合计一百多行。
    let log_config = simplelog::ConfigBuilder::new()
        .add_filter_allow_str("aios_database::fast_model::model_db_adapter")
        .add_filter_allow_str("aios_database::fast_model::e3d_model_service")
        .set_time_format_rfc3339()
        .build();
    let _ = simplelog::TermLogger::init(
        simplelog::LevelFilter::Info,
        log_config,
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Never,
    );

    let db_option = aios_core::get_db_option();
    let store = aios_database::options::store_mode();
    if matches!(store, aios_database::options::StoreMode::Mem) {
        aios_core::SUL_DB.connect("mem://").await?;
        aios_core::SUL_DB
            .use_ns(&db_option.surreal_ns)
            .use_db(&db_option.project_name)
            .await?;
        println!("介质：进程内嵌 kv-mem（mem，跑完即消失）");
    } else {
        // `rocksdb` 那档由 `ensure_store_connected` 连配置指的那台 surreal，与 `run_app`
        // 同一条路——本进程一台库都不拉（ADR-074）。**介质由配置说了算，本程序不自己挑**。
        aios_database::ensure_store_connected(&db_option)
            .await
            .with_context(|| format!("按 {} 备好存储", store.as_str()))?;
        println!(
            "介质：{}（{}，{}）",
            db_option.get_version_db_conn_str(),
            store.as_str(),
            if store.is_durable() {
                "真写、活得过重启"
            } else {
                "进程退出即消失"
            }
        );
    }
    // 落库路径上的谓词（整片删的 `anc CONTAINSANY`、`slice_sources_on`）全靠 `idx_*_anc`。
    // 全新库上没人替它建，量到的就是整表扫，越往后越慢——那条曲线是缺索引画出来的。
    // 语句自带 `IF NOT EXISTS`，对已经建好的库是 no-op。
    aios_core::SUL_DB
        .query(aios_database::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
        .await?
        .check()?;

    if let Some(root) = root {
        return directed(dbnum, &root).await;
    }

    let started = Instant::now();
    let service = E3dModelService::from_current()
        .await
        .context("打开生成服务")?;
    // 几何并发闸的利用率：生成慢是「一根太肥、并行不起来」还是「根本不在几何上」，
    // 这一对快照的差值直接答得出来，不必另外插桩。
    // 根取哪一层（`--roots`）：`zone`（默认）= 整库取根的正式口径 `generation_roots_of`——
    // 每个 ZONE 一根、不上溯 SITE；`file` = 2026-09-14 之前的口径（文件顶层，7997 上就是一个
    // WORL 装着全部几何），`site` = 每个 SITE 一根。后两档只留作对照实测，不是生产形态。
    let roots = match roots_level.as_str() {
        "zone" => service
            .generation_roots_of(dbnum)
            .await
            .with_context(|| format!("枚举 {dbnum} 的生成根（ZONE）"))?,
        "file" => service
            .top_level_roots_of(dbnum)
            .await
            .with_context(|| format!("枚举 {dbnum} 的文件顶层根"))?,
        "site" => {
            let picked = service
                .elements_with_noun(dbnum, "SITE")?
                .into_iter()
                .map(|refno| format!("{}/{}", refno.word0, refno.word1))
                .collect::<Vec<_>>();
            anyhow::ensure!(!picked.is_empty(), "{dbnum} 里没有 SITE 元素");
            picked
        }
        other => anyhow::bail!("--roots 只认 zone | file | site，给的是 {other}"),
    };
    println!("ROOTS level={roots_level} count={}", roots.len());
    let gate_before = aios_database::fast_model::concurrency::snapshot();
    // 生成那一段的分档账：`model root generated` 只答「墙钟压在哪一根」，这两档答
    // 「那一根里面是哪一层」——几何侧的 choke point（e3d-model 自己的 RAII 计时器，
    // 各档**嵌套**、不可相加）与源库读取（e3d-io 的读计数）。都是进程级累计量，
    // 所以只取这一段的差值。
    let phases_before = e3d_model::perf::snapshot();
    let reads_before = e3d_io::read_stats::snapshot();
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

    // 生成这两行先打出来再去等落库：落库挂了的时候，生成侧的数正是要看的那份，
    // 攒到最后一起打就等于把它跟着错误一起丢了。
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
    println!(
        "PHASES {}",
        e3d_model::perf::snapshot().delta_from(phases_before).summary()
    );
    println!(
        "READS {}",
        e3d_io::read_stats::snapshot()
            .delta_from(reads_before)
            .summary()
    );

    if analyze {
        return analyze_grouping(&service, dbnum);
    }
    if bindbench {
        return bind_bench(
            dbnum,
            bind_rows,
            bind_rounds,
            bind_endpoint.as_deref(),
            bind_noindex,
        )
        .await;
    }
    if batched {
        return batched_persist(dbnum).await;
    }

    let flushed =
        model_db_adapter::wait_until_model_rows_are_durable(dbnum, Duration::from_secs(1800))
            .await
            .with_context(|| format!("落库 {dbnum}"))?;
    let persisted_at = started.elapsed();

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

/// 合块那一档（`--batched`）：直接走 [`model_db_adapter::persist_roots_batched`]。
///
/// 逐根那一路（`persist_dbnum`）碰上第一根落不下去就整个 bail，量不到后面的组；合块这一路
/// 的赔付边界是根——整组失败会二分到单根，坏根记进 `failed`、其余照常提交，于是「连续落 N 组」
/// 的那条曲线才走得完。**只供一次性测量**，生产的持久档走 write-behind。
async fn batched_persist(dbnum: u32) -> anyhow::Result<()> {
    use aios_database::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};

    let store = ModelMemoryStore::global();
    let roots = store
        .roots(ProjectionScope::Current)
        .into_iter()
        .filter(|root| root.dbno() == dbnum)
        .collect::<Vec<_>>();
    let rooms = model_db_adapter::frozen_room_calculator()?;
    let started = Instant::now();
    let result = model_db_adapter::persist_roots_batched(
        model_db_adapter::PersistTarget::process(),
        store,
        dbnum,
        &roots,
        rooms.as_ref(),
    )
    .await
    .with_context(|| format!("合块落库 {dbnum}"))?;
    println!(
        "BATCHED 根={} 组={} 失败={} 最大事务={}B 用时={:?}",
        roots.len(),
        result.groups,
        result.failed.len(),
        result.max_transaction_bytes,
        started.elapsed(),
    );
    for (root, error) in &result.failed {
        println!("BATCHED-FAILED {root} {error}");
    }
    read_back(dbnum).await
}

/// D 方案的小基准（`--bindbench`）：**同一批行**，一遍渲成 SurrealQL 文本发过去，一遍铸成
/// 值挂在 `.bind()` 参数上发过去，交错 A/B 跑几轮，比客户端准备耗时、整笔事务耗时，
/// 再逐行比两个库落下去的东西是不是一模一样。
///
/// **为什么值得量**：`db.query(String)` 收到字符串之后，在**客户端**先 `syn::parse` 解析成
/// AST（`IntoQuery for String`），再 `Display` 回字符串装进请求（`stringify_queries`），
/// 服务端拿到又解析第三遍。66 MB 的 SurrealQL 于是被解析两次、打印一次，而这三下全计在
/// 落库账的 `commit=` 那一列里——上一轮据此得出的「单核 3.9 MB/s 是服务端天花板」未必成立。
/// 参数那条路语句只剩几百字节，行以二进制值过线，两头都不解析。
///
/// **两臂各写各的库**（`text_r<轮>` / `bind_r<轮>`，ns 固定 `bindbench`）：同一个库里第二臂
/// 撞上已有 id，`INSERT IGNORE` 直接跳过，量出来的是一次空跑。每个库照生产建同一组索引。
///
/// **交错着跑**：这台机器上单轮孤测不可信（2026-09-14 实测，别的会话一跑重活就漂几秒），
/// 所以每轮内两臂紧挨着跑、轮间换先后手，报的是各轮的中位数。
///
/// 生成那一步照旧（本程序前半段），落库目标由 `--bind-endpoint` 单独连——与 `SUL_DB` 不是
/// 同一个连接，生成期写进哪儿（配 `AIOS_IN_MEMORY_DB=1` 就是进程内 kv-mem）与这里无关。
async fn bind_bench(
    dbnum: u32,
    rows: usize,
    rounds: usize,
    endpoint: Option<&str>,
    noindex: bool,
) -> anyhow::Result<()> {
    use std::collections::BTreeSet;

    use aios_database::fast_model::model_db_adapter::{
        WriteMode, bind_records, publication_transaction, render_records,
    };
    use aios_database::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};
    use surrealdb::engine::any::connect;
    use surrealdb::opt::auth::Root;
    use surrealdb::sql::{Array as SurArray, Value as SurValue};

    anyhow::ensure!(rows > 0 && rounds > 0, "--bind-rows / --bind-rounds 要大于 0");

    // 这一批行：与 `--analyze` 同一套取法（按 record_id 去重，嵌套根之间会重复），再**等距**
    // 抽 N 条。不取前 N 条：投影是按根排下来的，头上那几千条清一色同形状（7997 上前 2000 条
    // 一个共享图元都没有），量出来的是五张表不是七张表。
    let store = ModelMemoryStore::global();
    let mut seen = BTreeSet::new();
    let mut all = Vec::new();
    for root in store
        .roots(ProjectionScope::Current)
        .into_iter()
        .filter(|root| root.dbno() == dbnum)
    {
        for record in store.records_of_root(ProjectionScope::Current, root) {
            if seen.insert(record.record_id.clone()) {
                all.push(record);
            }
        }
    }
    anyhow::ensure!(!all.is_empty(), "内存投影里没有 {dbnum} 的记录");
    let stride = (all.len() / rows.min(all.len())).max(1);
    let records = all
        .into_iter()
        .step_by(stride)
        .take(rows)
        .collect::<Vec<_>>();

    let db_option = aios_core::get_db_option();
    let endpoint = endpoint
        .map(str::to_owned)
        .unwrap_or_else(|| db_option.get_version_db_conn_str());
    let db = connect(endpoint.clone())
        .await
        .with_context(|| format!("连 {endpoint}"))?;
    if endpoint.starts_with("ws") || endpoint.starts_with("http") {
        db.signin(Root {
            username: &db_option.v_user,
            password: &db_option.v_password,
        })
        .await
        .with_context(|| format!("登录 {endpoint}"))?;
    }

    // 形状先报一次：两臂发出去的语句各多大、行分到几张表。
    let text_sql = publication_transaction(&render_records(&records, WriteMode::Initial)?);
    let bound = bind_records(&records)?;
    let bind_sql = publication_transaction(
        &bound
            .iter()
            .enumerate()
            .map(|(index, table)| table.statement(&format!("p{index}")))
            .collect::<String>(),
    );
    let bound_rows: usize = bound.iter().map(|table| table.rows.len()).sum();
    println!(
        "BINDBENCH endpoint={endpoint} records={} rounds={rounds} 表={} 行={bound_rows}",
        records.len(),
        bound.len(),
    );
    println!(
        "BINDBENCH-SHAPE text_sql={:.2}MB bind_sql={}B（行走参数） {}",
        text_sql.len() as f64 / 1_048_576.0,
        bind_sql.len(),
        bound
            .iter()
            .map(|table| format!("{}={}", table.table, table.rows.len()))
            .collect::<Vec<_>>()
            .join(" "),
    );

    // `db.query(String)` 内部那一下客户端解析，单独量一次：文本那一臂的账里有多少根本没出
    // 本进程，看的就是它。`stringify_queries` 随后还要把 AST 打印回字符串。
    let started = Instant::now();
    let ast = surrealdb::syn::parse(&text_sql).context("客户端解析文本 SQL")?;
    let client_parse = started.elapsed();
    let started = Instant::now();
    let restringified = ast.to_string();
    let client_print = started.elapsed();
    println!(
        "BINDBENCH-CLIENT syn::parse={:?} Display={:?} 打印回来={:.2}MB（`db.query(String)` 每次都要做这两下）",
        client_parse,
        client_print,
        restringified.len() as f64 / 1_048_576.0,
    );

    let index_sql = aios_database::fast_model::pdms_inst::INST_RELATE_INDEX_SQL;
    let mut text_samples = Vec::new();
    let mut bind_samples = Vec::new();
    for round in 1..=rounds {
        // 轮间换先后手：先跑的那一臂占着刚建好的库与刚热起来的缓存，固定顺序等于把这点
        // 差额一直记在同一臂头上。
        let order = if round % 2 == 1 {
            [false, true]
        } else {
            [true, false]
        };
        for bind in order {
            let database = format!("{}_r{round}", if bind { "bind" } else { "text" });
            db.use_ns("bindbench")
                .use_db(database.clone())
                .await
                .with_context(|| format!("切到 {database}"))?;
            if !noindex {
                db.query(index_sql)
                    .await
                    .with_context(|| format!("{database} 建索引"))?
                    .check()?;
            }

            // 客户端准备：两臂都从同一批记录现做一遍，谁也不吃上一轮的缓存。
            let started = Instant::now();
            let prepared_text = if bind {
                String::new()
            } else {
                publication_transaction(&render_records(&records, WriteMode::Initial)?)
            };
            let prepared_bound = if bind {
                bind_records(&records)?
            } else {
                Vec::new()
            };
            let prep = started.elapsed();

            let started = Instant::now();
            let mut response = if bind {
                let mut query = db.query(bind_sql.clone());
                for (index, table) in prepared_bound.into_iter().enumerate() {
                    query =
                        query.bind((format!("p{index}"), SurValue::Array(SurArray::from(table.rows))));
                }
                query.with_stats().await?
            } else {
                db.query(prepared_text).with_stats().await?
            };
            let query = started.elapsed();
            let errors = response.take_errors();
            anyhow::ensure!(errors.is_empty(), "{database} 落库报错：{errors:?}");

            let arm = if bind { "bind" } else { "text" };
            println!(
                "BINDBENCH-ROUND {round} {arm:<4} prep={:>8.1}ms query={:>9.1}ms db={database}",
                prep.as_secs_f64() * 1000.0,
                query.as_secs_f64() * 1000.0,
            );
            // 服务端自报的逐句耗时：客户端那一截（渲染 / 解析 / 序列化 / 过线）全不在里面，
            // 所以「差值花在谁身上」看这一行。事务里的 `BEGIN` / `COMMIT` 不回结果，所以
            // 拿回来几条就按几条对位——条数对不上表数时只报序号，不猜。
            let mut server = Vec::new();
            for index in 0..bound.len() + 2 {
                if let Some((stats, _)) = response.take::<surrealdb::Value>(index) {
                    server.push((index, stats.execution_time.unwrap_or_default()));
                }
            }
            let aligned = server.len() == bound.len();
            let total = server
                .iter()
                .map(|(_, elapsed)| elapsed.as_secs_f64() * 1000.0)
                .sum::<f64>();
            if !server.is_empty() {
                // 差额不全是客户端：`BEGIN` / `COMMIT` 不回结果，真正那一下提交也落在这里。
                println!(
                    "BINDBENCH-SERVER {round} {arm:<4} 逐句合计={total:.0}ms 余下={:.0}ms（客户端+提交） | {}",
                    query.as_secs_f64() * 1000.0 - total,
                    server
                        .iter()
                        .map(|(index, elapsed)| {
                            let label = match aligned.then(|| bound.get(*index)).flatten() {
                                Some(table) => table.table.to_string(),
                                None => format!("#{index}"),
                            };
                            format!("{label}={:.0}ms", elapsed.as_secs_f64() * 1000.0)
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            if bind {
                bind_samples.push((prep, query));
            } else {
                text_samples.push((prep, query));
            }
        }
    }

    let median = |samples: &[(Duration, Duration)], pick: fn(&(Duration, Duration)) -> Duration| {
        let mut values = samples.iter().map(pick).collect::<Vec<_>>();
        values.sort_unstable();
        values[values.len() / 2].as_secs_f64() * 1000.0
    };
    let text_query = median(&text_samples, |sample| sample.1);
    let bind_query = median(&bind_samples, |sample| sample.1);
    println!(
        "BINDBENCH-SUMMARY text prep={:.1}ms query={:.1}ms | bind prep={:.1}ms query={:.1}ms | query 差={:+.1}%（均为 {rounds} 轮中位数）",
        median(&text_samples, |sample| sample.0),
        text_query,
        median(&bind_samples, |sample| sample.0),
        bind_query,
        (bind_query - text_query) * 100.0 / text_query,
    );

    bind_bench_verify(&db, &bound, rounds).await
}

/// 两臂落下去的东西是不是同一批行：按表逐行比。不一致就把第一处差异原样打出来——
/// 耗时差只有在这一行说「一致」的时候才算数。
async fn bind_bench_verify(
    db: &Surreal<Any>,
    bound: &[aios_database::fast_model::model_db_adapter::BoundTable],
    rounds: usize,
) -> anyhow::Result<()> {
    use surrealdb::sql::Value as SurValue;

    let mut mismatched = 0usize;
    for table in bound {
        let mut dumps = Vec::new();
        for arm in ["text", "bind"] {
            db.use_db(format!("{arm}_r{rounds}")).await?;
            // 取成 SurrealDB 自己的值（不是 JSON）：两条路的差可能只在类型上
            // （同一个 `0.1` 一边 Float 一边 Decimal），JSON 会把它抹平。
            let dumped = db
                .query(format!("SELECT * FROM {} ORDER BY id;", table.table))
                .await?
                .check()?
                .take::<surrealdb::Value>(0)?
                .into_inner();
            let SurValue::Array(rows) = dumped else {
                anyhow::bail!("{} 的查询结果不是数组", table.table);
            };
            dumps.push(rows.0);
        }
        let (left, right) = (&dumps[0], &dumps[1]);
        if left.len() != right.len() {
            mismatched += 1;
            println!(
                "BINDBENCH-DIFF {} 行数 text={} bind={}",
                table.table,
                left.len(),
                right.len()
            );
            continue;
        }
        if let Some((index, (left, right))) = left
            .iter()
            .zip(right)
            .enumerate()
            .find(|(_, (left, right))| left != right)
        {
            mismatched += 1;
            let cut = |value: &SurValue| {
                let text = value.to_string();
                text.chars().take(400).collect::<String>()
            };
            println!("BINDBENCH-DIFF {} 第 {index} 行", table.table);
            println!("  text {}", cut(left));
            println!("  bind {}", cut(right));
        }
    }
    println!(
        "BINDBENCH-VERIFY 表={} 不一致={mismatched} 逐行一致={}",
        bound.len(),
        mismatched == 0
    );
    anyhow::ensure!(mismatched == 0, "两条路落下去的行不一致，耗时差不作数");
    Ok(())
}

/// 分析那一档（`--analyze`）：生成完**不落库**，拿解析侧的层级（`hierarchy_of`）+ 内存里的投影
/// 记录，算「按哪一层分组落库」各方案的形状：组数、每组记录数分布、估算的事务数与字节、
/// 跨组共享的 mesh（多写者下 `inst_geo` 的撞点）、以及 K 个写者下的估算总时长。
///
/// 估算常数来自 2026-09-14 kv-mem 上 7997 的实测：52,373 条切成 27 笔共 19.9 s → 0.38 ms/条；
/// 只有回执没有几何的根一笔 0.15–0.22 s → 每组固定开销取 0.2 s；渲成 SQL 约 1.26 KB/条。
fn analyze_grouping(service: &E3dModelService, dbnum: u32) -> anyhow::Result<()> {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use aios_database::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};

    const MS_PER_RECORD: f64 = 0.38;
    const GROUP_OVERHEAD_S: f64 = 0.2;
    const BYTES_PER_RECORD: f64 = 1260.0;
    const CHUNK: usize = 2_000;

    let pack = |word0: u32, word1: u32| (((word0 as u64) << 32) | word1 as u64) as i64;
    let unpack = |key: i64| format!("{}/{}", (key as u64) >> 32, (key as u64) & 0xffff_ffff);

    // 解析侧：refno → (owner, noun)
    let hierarchy = service.hierarchy_of(dbnum)?;
    let mut nouns = HashMap::<i64, String>::with_capacity(hierarchy.len());
    let mut owners = HashMap::<i64, i64>::with_capacity(hierarchy.len());
    for (refno, owner, noun) in &hierarchy {
        nouns.insert(pack(refno.word0, refno.word1), noun.clone());
        owners.insert(pack(refno.word0, refno.word1), pack(owner.word0, owner.word1));
    }
    let mut noun_hist = BTreeMap::<&str, usize>::new();
    for (_, _, noun) in &hierarchy {
        *noun_hist.entry(noun.as_str()).or_default() += 1;
    }
    println!(
        "HIERARCHY dbnum={dbnum} elements={} SITE={} ZONE={}",
        hierarchy.len(),
        noun_hist.get("SITE").copied().unwrap_or(0),
        noun_hist.get("ZONE").copied().unwrap_or(0),
    );

    // 投影侧：全部记录，按 record_id 去重（嵌套根之间会重复）。
    let store = ModelMemoryStore::global();
    let roots = store
        .roots(ProjectionScope::Current)
        .into_iter()
        .filter(|root| root.dbno() == dbnum)
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    let mut records = Vec::new();
    let mut per_generation_root = BTreeMap::<String, usize>::new();
    for root in &roots {
        let mine = store.records_of_root(ProjectionScope::Current, *root);
        let noun = nouns
            .get(&pack(root.word0, root.word1))
            .cloned()
            .unwrap_or_else(|| "?".into());
        per_generation_root.insert(format!("{root} {noun}"), mine.len());
        for record in mine {
            if seen.insert(record.record_id.clone()) {
                records.push(record);
            }
        }
    }
    println!(
        "RECORDS dbnum={dbnum} total={} generation_roots={}",
        records.len(),
        roots.len()
    );
    for (root, count) in &per_generation_root {
        println!("  GENERATION-ROOT {root} records={count}");
    }

    // 记录的祖先链：自身 → 顶层。找链上第一个 noun == SITE / ZONE 的节点；ZONE 下一级 = 交付单元。
    struct Keyed {
        site: Option<i64>,
        zone: Option<i64>,
        unit: Option<i64>,
        mesh: String,
    }
    let mut keyed = Vec::with_capacity(records.len());
    let mut chain_len = BTreeMap::<usize, usize>::new();
    for record in &records {
        *chain_len.entry(record.ancestors.len()).or_default() += 1;
        let mut site = None;
        let mut zone = None;
        let mut unit = None;
        for (index, key) in record.ancestors.iter().enumerate() {
            match nouns.get(key).map(String::as_str) {
                Some("ZONE") if zone.is_none() => {
                    zone = Some(*key);
                    unit = index.checked_sub(1).map(|below| record.ancestors[below]);
                }
                Some("SITE") if site.is_none() => site = Some(*key),
                _ => {}
            }
        }
        keyed.push(Keyed {
            site,
            zone,
            unit,
            mesh: record.mesh_id.clone(),
        });
    }
    println!(
        "CHAIN-LEN {}",
        chain_len
            .iter()
            .map(|(len, count)| format!("{len}:{count}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // 每个方案：key → 记录数 + mesh 集合
    struct Scheme {
        name: &'static str,
        groups: BTreeMap<String, (usize, BTreeSet<String>)>,
    }
    let mut schemes = Vec::new();
    for (name, pick) in [
        ("A 文件顶层根（旧口径 top_level_roots_of）", 0usize),
        ("B SITE", 1),
        ("C ZONE", 2),
        ("D ZONE 下一级（交付单元）", 3),
    ] {
        let mut groups = BTreeMap::<String, (usize, BTreeSet<String>)>::new();
        for (record, k) in records.iter().zip(&keyed) {
            let key = match pick {
                0 => record
                    .ancestors
                    .last()
                    .map(|top| unpack(*top))
                    .unwrap_or_else(|| "?".into()),
                1 => k.site.map(unpack).unwrap_or_else(|| "(no SITE)".into()),
                2 => k.zone.map(unpack).unwrap_or_else(|| "(no ZONE)".into()),
                _ => k.unit.map(unpack).unwrap_or_else(|| "(no unit)".into()),
            };
            let entry = groups.entry(key).or_default();
            entry.0 += 1;
            entry.1.insert(k.mesh.clone());
        }
        schemes.push(Scheme { name, groups });
    }
    // E：不看层级，固定 2000 条一组（现在事务切块的粒度），多写者按组抢。
    {
        let mut groups = BTreeMap::<String, (usize, BTreeSet<String>)>::new();
        for (index, k) in keyed.iter().enumerate() {
            let entry = groups.entry(format!("chunk-{:04}", index / CHUNK)).or_default();
            entry.0 += 1;
            entry.1.insert(k.mesh.clone());
        }
        schemes.push(Scheme {
            name: "E 固定 2000 条一组（不看层级）",
            groups,
        });
    }

    let all_meshes = keyed.iter().map(|k| k.mesh.as_str()).collect::<BTreeSet<_>>().len();
    for scheme in &schemes {
        let mut sizes = scheme.groups.values().map(|(n, _)| *n).collect::<Vec<_>>();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        let total: usize = sizes.iter().sum();
        let max = sizes.first().copied().unwrap_or(0);
        let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0);
        let p90 = sizes
            .get(sizes.len().saturating_sub(1) / 10)
            .copied()
            .unwrap_or(0);
        let txns: usize = sizes.iter().map(|n| n.div_ceil(CHUNK).max(1)).sum();
        // 跨组共享 mesh：一个 mesh 出现在几组里；>1 就是多写者下 INSERT IGNORE 可能互撞的 key。
        let mut mesh_groups = HashMap::<&str, usize>::new();
        for (_, meshes) in scheme.groups.values() {
            for mesh in meshes {
                *mesh_groups.entry(mesh.as_str()).or_default() += 1;
            }
        }
        let shared = mesh_groups.values().filter(|n| **n > 1).count();
        // K 个写者的估算总时长：LPT（最大的先派、派给最闲的）。
        let cost = |n: usize| GROUP_OVERHEAD_S + n as f64 * MS_PER_RECORD / 1000.0;
        let makespan = |writers: usize| {
            let mut load = vec![0f64; writers];
            for n in &sizes {
                let (slot, _) = load
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .unwrap();
                load[slot] += cost(*n);
            }
            load.iter().cloned().fold(0f64, f64::max)
        };
        println!(
            "SCHEME {} · groups={} records={total} max={max} ({:.0}%) median={median} p90={p90} txns={txns} est_bytes_max={:.1}MB shared_mesh={shared}/{all_meshes} makespan K=1:{:.1}s K=2:{:.1}s K=4:{:.1}s K=8:{:.1}s K=16:{:.1}s",
            scheme.name,
            scheme.groups.len(),
            if total > 0 { max as f64 * 100.0 / total as f64 } else { 0.0 },
            max as f64 * BYTES_PER_RECORD / 1_048_576.0,
            makespan(1),
            makespan(2),
            makespan(4),
            makespan(8),
            makespan(16),
        );
        let mut top = scheme.groups.iter().collect::<Vec<_>>();
        top.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
        for (key, (n, meshes)) in top.iter().take(8) {
            let noun = key
                .split('/')
                .next()
                .and_then(|w0| key.split('/').nth(1).map(|w1| (w0, w1)))
                .and_then(|(w0, w1)| Some(pack(w0.parse().ok()?, w1.parse().ok()?)))
                .and_then(|k| nouns.get(&k).cloned())
                .unwrap_or_default();
            println!(
                "  TOP {key} {noun} records={n} meshes={} txns={} est={:.1}MB",
                meshes.len(),
                n.div_ceil(CHUNK).max(1),
                *n as f64 * BYTES_PER_RECORD / 1_048_576.0
            );
        }
    }
    render_cost(&records)?;
    let _ = &owners;
    Ok(())
}

/// 渲成 SQL 之后的字节账：**落库时间几乎全部按字节收费**（2026-09-14 RocksDB 实测
/// `commit(ms) ≈ 37.5 + 0.231 × sql_KB`，R²=0.84，每笔固定开销只占 7%），所以「字节花在哪」
/// 就是压落库耗时的清单。
///
/// 按表分摊，再单列几个**为读侧快路而故意重复**的字段——它们的值同一笔事务里另一张表
/// 已经写过一份：`aabb_d` / `world_trans_d` 是 `aabb` / `trans` 两行的副本（`inst_relate`
/// 上的平表三件），`insts_flat` 里的 `transform` 是 `geo_relate.transform` 的副本。
fn render_cost(
    records: &[aios_database::fast_model::model_memory_store::ProjectedGeometry],
) -> anyhow::Result<()> {
    use std::collections::BTreeMap;

    use aios_database::fast_model::model_db_adapter::{WriteMode, render_records};

    /// `open` 起、到 `close` 前为止的总字节与出现次数（不含 `close` 本身）。
    fn span(haystack: &str, open: &str, close: &str) -> (usize, usize) {
        let (mut bytes, mut hits) = (0usize, 0usize);
        let mut from = 0usize;
        while let Some(offset) = haystack[from..].find(open) {
            let start = from + offset;
            let Some(offset) = haystack[start + open.len()..].find(close) else {
                break;
            };
            let close_at = start + open.len() + offset;
            bytes += close_at - start;
            hits += 1;
            from = close_at;
        }
        (bytes, hits)
    }

    let mut per_table = BTreeMap::<String, (usize, usize)>::new();
    let mut total = 0usize;
    let (mut flat, mut insts, mut anc) = ((0usize, 0usize), (0usize, 0usize), (0usize, 0usize));
    for chunk in records.chunks(2_000) {
        let sql = render_records(chunk, WriteMode::Initial)?;
        total += sql.len();
        for statement in sql.lines() {
            let Some(rest) = statement
                .strip_prefix("INSERT RELATION IGNORE INTO ")
                .or_else(|| statement.strip_prefix("INSERT IGNORE INTO "))
            else {
                continue;
            };
            let table = rest.split(' ').next().unwrap_or("?");
            let entry = per_table.entry(table.to_owned()).or_default();
            entry.0 += statement.len() + 1;
            entry.1 += 1;
        }
        for (into, one) in [
            (&mut flat, span(&sql, ",aabb_d:", ",insts_flat:")),
            (&mut insts, span(&sql, ",insts_flat:", ",model_sesno:")),
            (&mut anc, span(&sql, ",anc:[", "]")),
        ] {
            into.0 += one.0;
            into.1 += one.1;
        }
    }
    let share = |bytes: usize| bytes as f64 * 100.0 / total.max(1) as f64;
    println!(
        "RENDER records={} sql={:.1}MB bytes_per_record={:.0}",
        records.len(),
        total as f64 / 1_048_576.0,
        total as f64 / records.len().max(1) as f64,
    );
    let mut tables = per_table.iter().collect::<Vec<_>>();
    tables.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (table, (bytes, statements)) in tables {
        println!(
            "  TABLE {table:<12} {:>6.1}MB {:>5.1}% statements={statements}",
            *bytes as f64 / 1_048_576.0,
            share(*bytes),
        );
    }
    for (what, (bytes, hits)) in [
        ("aabb_d+world_trans_d（aabb/trans 两行的副本）", flat),
        ("insts_flat（geo_relate.transform 的副本）", insts),
        ("anc（祖先链数组）", anc),
    ] {
        println!(
            "  FIELD {what} {:>6.1}MB {:>5.1}% rows={hits} avg={:.0}B",
            bytes as f64 / 1_048_576.0,
            share(bytes),
            bytes as f64 / hits.max(1) as f64,
        );
    }
    Ok(())
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
    println!(
        "GENERATE root={root} dbnum={dbnum} upserted={} removed={} failed={} 用时={:?}",
        report.upserted, report.removed, report.failed, generated_at,
    );

    let flushed =
        model_db_adapter::wait_until_model_rows_are_durable(dbnum, Duration::from_secs(1800))
            .await
            .with_context(|| format!("落库根 {root}"))?;
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
