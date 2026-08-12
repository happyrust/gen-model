//! fork-surreal-compat-suite（ADR-017 / 开发方案 T0.1）。
//!
//! 暂存与写回把「嵌入式 mem 引擎」当作 fork 服务器（rocksdb 后端）的行为等价物：
//! 生成读窗口新态走暂存库，写回时按语句日志对持久层重放。这个等价假设必须被
//! 机器验证，而不是靠「同一个 core 版本」的直觉——本套件让同一批 SQL 在两个引擎
//! 上双跑并逐语句对拍。
//!
//! 覆盖面（与开发方案 T0.1 清单一一对应）：
//! - 项目启动序列的全部 DEFINE（`define_common_functions` 目录序加载 +
//!   project_hd 对 `fn::room_code` 的重放矫正 + `define_dbnum_event` + 索引）；
//! - fn:: 覆盖顺序（`fn::room_code` hd/hh 两版、`fn::room_num_of` 排序键语义）；
//! - `ast_payload` 兼容（fork 侧连接与生产完全同款：`Config::default().ast_payload()`）；
//! - `INSERT RELATION` 撞 id 行为（ADR-010 D13：fork 服务器静默保留旧行）;
//! - 事务语义（失败整段回滚、CANCEL、THROW）;
//! - record id（含 `⟨⟩` 形制与数组 id）序列化往返；
//! - schemaless 表接受裸对象。
//!
//! 双跑侧基建：`bin/surreal.exe`（fork 2.1.4，gitignore 的机器本地文件）+ 一次性
//! rocksdb 临时目录，每个用例自带服务器、互不共享全局连接（刻意不用 `SUL_DB`，
//! 避开 live 用例「全局连接跨运行时失效」的坑）。二进制缺失时用例软跳过；
//! 设 `AIOS_COMPAT_REQUIRE=1` 把缺基建变成硬失败（门槛式运行用）。
//! 已确认的结论与差异逐条记录在 `docs/2026-08-05_fork-surreal-compat-findings.md`。

use std::collections::HashMap;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aios_core::RefU64;
use itertools::Itertools;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::Config;
use surrealdb::opt::auth::Root;

/// 嵌入式 mem 引擎上开一个独立 database（ns 固定 `compat`，db 按用例隔离）。
async fn mem_db(db: &str) -> Surreal<Any> {
    let handle = connect("mem://")
        .await
        .expect("mem 引擎应能启动（surrealdb 需带 kv-mem feature）");
    handle
        .use_ns("compat")
        .use_db(db)
        .await
        .expect("mem use_ns/use_db");
    handle
}

/// 一次性 fork 服务器：rocksdb 后端落在临时目录，进程随守卫销毁，目录尽力清扫。
struct SpawnedFork {
    child: Child,
    dir: PathBuf,
    ws_url: String,
}

impl Drop for SpawnedFork {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Windows 上 rocksdb 句柄释放可能滞后于 kill，清不掉就留给系统临时目录。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn compat_require() -> bool {
    std::env::var("AIOS_COMPAT_REQUIRE").is_ok_and(|v| v == "1")
}

/// 找一个空闲端口。绑定后立刻释放存在竞态，本地测试可接受。
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// 起一台一次性 fork 服务器（rocksdb 后端）。二进制缺失时返回 `None`（软跳过）。
fn spawn_fork_server(case: &str) -> Option<SpawnedFork> {
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/surreal.exe");
    if !exe.exists() {
        if compat_require() {
            panic!("AIOS_COMPAT_REQUIRE=1 但缺少 {}", exe.display());
        }
        eprintln!(
            "[compat] skip {case}: 缺 bin/surreal.exe（fork 2.1.4 服务器二进制，机器本地文件）"
        );
        return None;
    }
    let port = free_port();
    let dir = std::env::temp_dir().join(format!(
        "aios-compat-{}-{}-{port}",
        std::process::id(),
        case
    ));
    std::fs::create_dir_all(&dir).expect("create compat temp dir");
    let child = Command::new(&exe)
        .args([
            "start",
            "--user",
            "root",
            "--pass",
            "root",
            "--bind",
            &format!("127.0.0.1:{port}"),
            // 相对路径 + current_dir：避开 Windows 绝对路径塞进 datastore URI 的解析歧义。
            "rocksdb:compat-db",
        ])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn bin/surreal.exe");

    let spawned = SpawnedFork {
        child,
        dir,
        ws_url: format!("ws://127.0.0.1:{port}"),
    };
    // 先等 TCP 可达，WS 握手交给带重试的 connect_fork。
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return Some(spawned);
        }
        if Instant::now() > deadline {
            if compat_require() {
                panic!("AIOS_COMPAT_REQUIRE=1 但 fork 服务器 20s 内未就绪（port {port}）");
            }
            eprintln!("[compat] skip {case}: fork 服务器 20s 内未就绪");
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 与生产完全同款的 fork 连接（`ast_payload` 打开、root 登录）。
async fn connect_fork(ws_url: &str, db: &str) -> Surreal<Any> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let handle = loop {
        match connect((ws_url.to_string(), Config::default().ast_payload())).await {
            Ok(h) => break h,
            Err(error) => {
                if Instant::now() > deadline {
                    panic!("connect fork {ws_url}: {error}");
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    handle
        .signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("fork signin root");
    handle
        .use_ns("compat")
        .use_db(db)
        .await
        .expect("fork use_ns/use_db");
    handle
}

/// fork 侧连接的生命周期守卫：自起进程要杀、外部服务器不归我们管。
enum ForkGuard {
    Spawned(#[allow(dead_code)] SpawnedFork),
    External,
}

/// 双跑基建：mem 句柄 + fork 句柄。`AIOS_COMPAT_WS` 指向已运行的服务器时
/// 直接连它（人工排查用），否则自起一次性服务器；基建缺失时 `None`（软跳过）。
async fn dual_dbs(case: &str) -> Option<(Surreal<Any>, Surreal<Any>, ForkGuard)> {
    if let Ok(url) = std::env::var("AIOS_COMPAT_WS") {
        let mem = mem_db(case).await;
        let fork = connect_fork(&url, case).await;
        return Some((mem, fork, ForkGuard::External));
    }
    let server = spawn_fork_server(case)?;
    let mem = mem_db(case).await;
    let fork = connect_fork(&server.ws_url, case).await;
    Some((mem, fork, ForkGuard::Spawned(server)))
}

/// 把一次 `query()` 调用的每条语句结果规范化成 `Ok(JSON 文本)` / `Err(错误文本)`。
///
/// 渲染用 serde_json 而不是 `surrealdb::Value` 的 Display：后者是给人看的简化
/// 渲染（字符串不带引号、record id 不带 ⟨⟩ 转义），字符串 `"pe:x"` 和记录 id
/// `pe:x` 会打成同一串，对拍会假相等。serde 序列化走 core 的结构化表示，
/// 类型差异（string vs record id vs number）在文本里保真。
async fn exec_capture(db: &Surreal<Any>, sql: &str) -> Vec<Result<String, String>> {
    match db.query(sql).await {
        Err(error) => vec![Err(format!("transport: {error}"))],
        Ok(mut response) => {
            let statements = response.num_statements();
            let mut errors: HashMap<usize, surrealdb::Error> = response.take_errors();
            let mut out = Vec::with_capacity(statements);
            for index in 0..statements {
                if let Some(error) = errors.remove(&index) {
                    out.push(Err(error.to_string()));
                } else {
                    match response.take::<surrealdb::Value>(index) {
                        Ok(value) => out.push(Ok(serde_json::to_string(&value)
                            .unwrap_or_else(|e| format!("<serialize error: {e}>")))),
                        Err(error) => out.push(Err(format!("take: {error}"))),
                    }
                }
            }
            out
        }
    }
}

/// 双跑一段脚本并逐步对拍。`scripts` 的每个元素 = 一次 `query()` 调用
/// （事务块必须整块作为一个元素提交）。
async fn assert_dual_same(case: &str, mem: &Surreal<Any>, fork: &Surreal<Any>, scripts: &[&str]) {
    for (step, sql) in scripts.iter().enumerate() {
        let mem_out = exec_capture(mem, sql).await;
        let fork_out = exec_capture(fork, sql).await;
        assert_eq!(
            mem_out, fork_out,
            "[{case}] 第 {step} 步 mem 与 fork 行为不一致\nsql:\n{sql}\nmem : {mem_out:#?}\nfork: {fork_out:#?}"
        );
    }
}

/// 暂存库建库初始化（T0.3）＝生产启动序列减去 update_dbnum_event（F4：该事件
/// 与字符串 id 的 pe 最新行写入不能共存）。单一事实来源在
/// `staging::lifecycle::init_staging_schema`——本套件排练的就是生产那个函数。
async fn replay_startup_defines(db: &Surreal<Any>) -> anyhow::Result<()> {
    crate::data_interface::staging::lifecycle::init_staging_schema(db).await
}

/// F4 行为钉子：update_dbnum_event 定义在场时，字符串 id 的 pe 最新行在
/// **两个引擎上同样**写不进去（事务里也一样整体失败），数组 id 的历史行
/// 形制则同样成功并推进 dbnum_info_table。mem↔fork 无分歧——这是生产写
/// 路径自身的雷点，不是暂存介质的行为差异。
#[tokio::test(flavor = "multi_thread")]
async fn dual_update_dbnum_event_rejects_string_pe_ids_identically() {
    let Some((mem, fork, _guard)) = dual_dbs("dbnum_event").await else {
        return;
    };
    assert_dual_same(
        "dbnum_event",
        &mem,
        &fork,
        &[
            r#"DEFINE EVENT OVERWRITE update_dbnum_event ON pe WHEN $event = "CREATE" OR $event = "UPDATE" OR $event = "DELETE" THEN { LET $id = record::id($value.id); LET $ref_0 = array::at($id, 0); UPSERT type::thing('dbnum_info_table', $ref_0) SET count = count?:0 + 1; };"#,
            // 字符串 id 的最新行：事件体 array::at 类型错误，事务整体失败。
            "BEGIN TRANSACTION; UPSERT pe:24381_100677 SET noun = 'PIPE'; COMMIT TRANSACTION;",
            "SELECT * FROM pe;",
            // 数组 id 的历史行形制：事件正常工作。
            "UPSERT pe:['24381_100677', 5] SET noun = 'PIPE_H';",
            "SELECT * FROM dbnum_info_table;",
        ],
    )
    .await;
}

/// `INFO FOR DB` 的 SurrealQL 文本（对拍与标记断言共用）。
async fn info_for_db(db: &Surreal<Any>) -> String {
    let mut response = db.query("INFO FOR DB").await.expect("INFO FOR DB");
    let value: surrealdb::Value = response.take(0).expect("take info");
    value.to_string()
}

// ───────────────────────── mem-only：暂存介质自身的行为 ─────────────────────────

/// kv-mem 打通的最小证明：`⟨⟩` 形制与数组形制的 record id 在 mem 引擎上
/// 建得进、查得回、渲染稳定。这两种形制是 pe / 历史表 id 的日常形态。
#[tokio::test(flavor = "multi_thread")]
async fn mem_engine_round_trips_record_id_shapes() {
    let mem = mem_db("record_id_shapes").await;

    let out = exec_capture(
        &mem,
        r#"CREATE pe:⟨4000000001_10⟩ SET noun = 'PIPE', name = 'p1';"#,
    )
    .await;
    assert_eq!(out.len(), 1, "{out:?}");
    let created = out[0].as_ref().expect("create bracket id");
    assert!(
        created.contains(r#""Thing":{"tb":"pe","id":{"String":"4000000001_10"}}"#),
        "⟨⟩ 形制 id 应作为 Thing(tb=pe, String id) 往返: {created}"
    );

    let out = exec_capture(
        &mem,
        r#"CREATE pe:['4000000001_10', 5] SET noun = 'PIPE_H';"#,
    )
    .await;
    assert!(out[0].is_ok(), "数组形制 id 应能创建: {out:?}");

    let out = exec_capture(&mem, "SELECT VALUE id FROM pe ORDER BY id;").await;
    let ids = out[0].as_ref().expect("select ids");
    assert!(
        ids.contains(r#"{"Thing":{"tb":"pe","id":{"String":"4000000001_10"}}}"#),
        "⟨⟩ 形制应在场: {ids}"
    );
    assert!(
        ids.contains(r#""id":{"Array":[{"Strand":"4000000001_10"},{"Number":{"Int":5}}]}"#),
        "数组形制应在场且元素类型保真: {ids}"
    );

    let out = exec_capture(&mem, "RETURN record::id(pe:⟨4000000001_10⟩);").await;
    assert_eq!(
        out[0].as_deref(),
        Ok(r#"{"Strand":"4000000001_10"}"#),
        "record::id 应还原裸 id 文本"
    );
}

/// 暂存库初始化排练：生产启动序列的 DEFINE 全套在一个全新 mem 库上重放后，
/// 关键 fn:: / 索引在场，且 project_hd 下 `fn::room_code` 的胜者是 hd 版
/// （hh 版的 `$uda_room` 标记不得出现——这就是 D11 覆盖顺序矫正的验收）。
///
/// 同时钉住一个既有事实：`define_common_functions` 不 `check()`，逐语句错误
/// （如全新库上 `REMOVE FUNCTION` 不存在的函数）被静默吞掉、后续 DEFINE 照常
/// 生效。T0.5 的 journal validator 必须显式 `check()`，不得继承这个行为。
#[tokio::test(flavor = "multi_thread")]
async fn mem_startup_define_replay_applies_and_hd_room_code_wins() {
    let mem = mem_db("startup_defines").await;

    replay_startup_defines(&mem)
        .await
        .expect("全新 mem 库上重放启动 DEFINE 序列");

    let info = info_for_db(&mem).await;
    for function in [
        "ancestor",
        "room_code",
        "room_num_of",
        "room_relate_of",
        "newest_pe",
        "default_full_name",
    ] {
        assert!(
            info.contains(&format!("fn::{function}")),
            "启动重放后应有 fn::{function}，INFO: {info_head}…",
            info_head = &info[..info.len().min(400)]
        );
    }

    #[cfg(feature = "project_hd")]
    {
        assert!(
            info.contains("room_relate_of"),
            "hd 版 room_code 依赖 fn::room_relate_of"
        );
        assert!(
            !info.contains("$uda_room"),
            "project_hd 下 fn::room_code 不应是 hh 版（$uda_room 是 hh 独有标记）"
        );
    }

    let table_info = {
        let mut response = mem
            .query("INFO FOR TABLE pe")
            .await
            .expect("INFO FOR TABLE");
        let value: surrealdb::Value = response.take(0).expect("take table info");
        value.to_string()
    };
    for index in ["pe_refno_index", "pe_noun_dbnum_index", "fulltext_name"] {
        assert!(
            table_info.contains(index),
            "pe 表应有索引 {index}: {table_info}"
        );
    }
}

/// fn:: 覆盖顺序的执行面验收：`fn::room_num_of` 按归属强度取最强一条
/// （inside_count 降序 → center_dist 升序 → room_num 升序，ADR-010 §5）。
#[tokio::test(flavor = "multi_thread")]
async fn mem_room_num_of_picks_strongest_relation() {
    let mem = mem_db("room_num_of").await;
    replay_startup_defines(&mem).await.expect("replay defines");

    let out = exec_capture(
        &mem,
        r#"
        CREATE elem:e1 SET noun = 'BOX';
        CREATE panel:p1 SET noun = 'PANE';
        CREATE panel:p2 SET noun = 'PANE';
        INSERT RELATION INTO room_relate [
            { id: room_relate:weak, in: panel:p2, out: elem:e1, room_num: 'R102', inside_count: 4, center_dist: 0.5 },
            { id: room_relate:strong, in: panel:p1, out: elem:e1, room_num: 'R101', inside_count: 8, center_dist: 1.5 }
        ];
        RETURN fn::room_num_of(elem:e1);
        "#,
    )
    .await;
    let verdict = out.last().unwrap().as_ref().expect("room_num_of");
    assert_eq!(
        verdict, r#"{"Strand":"R101"}"#,
        "inside_count 高者应胜出: {out:?}"
    );
}

// ───────────────────────── 双跑：mem ↔ fork 行为对拍 ─────────────────────────

/// ADR-010 D13 的行为在两个引擎上必须一致：`INSERT RELATION` 撞 id 静默保留旧行。
/// 普通表 `INSERT` 撞 id 的行为也一并对拍（写回重放的语句里两类都有）。
#[tokio::test(flavor = "multi_thread")]
async fn dual_insert_id_collision_behavior_agrees() {
    let Some((mem, fork, _guard)) = dual_dbs("insert_collision").await else {
        return;
    };
    assert_dual_same(
        "insert_collision",
        &mem,
        &fork,
        &[
            "CREATE pe:a SET noun = 'ZONE'; CREATE pe:b SET noun = 'BRAN';",
            "INSERT RELATION INTO rel_t [{ id: rel_t:dup, in: pe:a, out: pe:b, tag: 'first' }];",
            // 撞 id：D13 说 fork 服务器静默保留旧行——mem 引擎必须同款。
            "INSERT RELATION INTO rel_t [{ id: rel_t:dup, in: pe:a, out: pe:b, tag: 'second' }];",
            "SELECT tag FROM rel_t ORDER BY id;",
            "INSERT INTO plain_t { id: plain_t:dup, v: 1 };",
            "INSERT INTO plain_t { id: plain_t:dup, v: 2 };",
            "SELECT v FROM plain_t ORDER BY id;",
        ],
    )
    .await;
}

/// 一个 owner 现存的成员边（`[owner, 槽位]` 升序）。
async fn owner_block(db: &Surreal<Any>, owner: &str) -> Vec<String> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM pe_owner WHERE out = {owner} ORDER BY id"
        ))
        .await
        .expect("读 owner 块传输")
        .check()
        .expect("读 owner 块");
    let mut ids = response
        .take::<Vec<surrealdb::sql::Thing>>(0)
        .expect("解码边 id")
        .into_iter()
        .map(|thing| thing.to_string())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

/// issue #14 的落库形态押在「owner 复合 id 前缀范围」上：
/// `DELETE pe_owner:[owner, NONE]..=[owner, ..]` 要正好圈住这一个 owner 的全部槽位。
///
/// 这一条只有跨引擎对拍才算数。范围的边界依赖 SurrealDB 的**值序**（`NONE` 作下界、
/// `..` 排在所有槽位号之上），而生产写回落在 rocksdb 后端的 fork 服务器上、
/// 暂存与本仓全部回归跑在 mem 引擎上——两边值序但凡有出入，失败形态是静默把
/// 相邻 owner 的边一起删掉，没有任何报错。
///
/// 语句一律由生产渲染函数 [`render_pe_owner_replace`] 现渲染，不在这里抄一份字面量：
/// 抄下来的 SQL 只能证明「这个形状在两个引擎上一致」，证明不了生产还在发这个形状。
#[tokio::test(flavor = "multi_thread")]
async fn dual_pe_owner_owner_range_delete_agrees() {
    use crate::data_interface::cata_closure::render_pe_owner_replace;

    let Some((mem, fork, _guard)) = dual_dbs("pe_owner_range").await else {
        return;
    };
    // 相邻 owner 取同一个 ref0 的相邻序号，是前缀范围最容易串味的排布。
    let owner_a = RefU64((16189u64 << 32) | 0);
    let owner_b = RefU64((16189u64 << 32) | 1);
    let key_a = owner_a.to_pe_key();
    let key_b = owner_b.to_pe_key();
    let children = (0..4)
        .map(|i| RefU64((24381u64 << 32) | (34109 + i)))
        .collect::<Vec<_>>();

    let seed_a = render_pe_owner_replace(&key_a, &children[..3]).expect("render owner A");
    // B 的槽位号与 A 完全重叠，且首个成员与 A 共用——A 的范围删除不许碰它。
    let seed_b = render_pe_owner_replace(&key_b, &[children[0], children[3]]).expect("render B");
    let shrink_a = render_pe_owner_replace(&key_a, &children[1..2]).expect("render shrunk A");
    let bare_range_delete = format!("DELETE pe_owner:[{key_a}, NONE]..=[{key_a}, ..];");

    // 唯一索引在场是前提：issue #14 的原始故障就是换槽重插撞 `unique_pe_owner`。
    let schema = "DEFINE TABLE pe_owner TYPE RELATION IN pe OUT pe SCHEMALESS; \
                  DEFINE INDEX unique_pe_owner ON pe_owner FIELDS in, out UNIQUE;";
    let readback = "SELECT id, in FROM pe_owner ORDER BY id;";

    assert_dual_same(
        "pe_owner_range",
        &mem,
        &fork,
        &[
            schema,
            &seed_a,
            &seed_b,
            readback,
            // oracle 点名的裸范围删除：只清 A。
            &bare_range_delete,
            readback,
            // 成员表缩短 3 → 1，且同一事务重放两次必须收敛到同一终态。
            &seed_a,
            &shrink_a,
            &shrink_a,
            readback,
        ],
    )
    .await;

    // 对拍只证明两边一致，终态对不对要另外钉：A 剩尾槽已清，B 一根没少。
    let survivor = format!("pe_owner:[{key_a}, 0]");
    let b_block = vec![
        format!("pe_owner:[{key_b}, 0]"),
        format!("pe_owner:[{key_b}, 1]"),
    ];
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        assert_eq!(
            owner_block(db, &key_a).await,
            vec![survivor.clone()],
            "[{engine}] 缩短后的尾槽必须全部删除"
        );
        assert_eq!(
            owner_block(db, &key_b).await,
            b_block,
            "[{engine}] owner 前缀范围删除不能越界到相邻 owner"
        );
    }
}

/// 事务语义对拍：中途失败整段回滚、CANCEL 显式回滚、THROW 中止。
/// 写回协议按 TX_CHUNK 分块事务重放，块内任一语句失败整块不生效——
/// 这个语义两边必须一字不差。
#[tokio::test(flavor = "multi_thread")]
async fn dual_transaction_semantics_agree() {
    let Some((mem, fork, _guard)) = dual_dbs("transactions").await else {
        return;
    };
    assert_dual_same(
        "transactions",
        &mem,
        &fork,
        &[
            // 撞 id 的 CREATE 让第二条失败 → 整段回滚。
            "BEGIN; CREATE tx_t:a SET v = 1; CREATE tx_t:a SET v = 2; COMMIT;",
            "SELECT * FROM tx_t;",
            "BEGIN; CREATE tx_t:b SET v = 1; CANCEL;",
            "SELECT * FROM tx_t;",
            "BEGIN; CREATE tx_t:c SET v = 1; THROW 'boom'; COMMIT;",
            "SELECT * FROM tx_t;",
            // 成功路径对照。
            "BEGIN; CREATE tx_t:d SET v = 4; UPSERT tx_t:e SET v = 5; COMMIT;",
            "SELECT v FROM tx_t ORDER BY id;",
        ],
    )
    .await;
}

/// record id 形制与类型转换对拍：`⟨⟩`、数组 id、`type::thing`、字符串投射。
#[tokio::test(flavor = "multi_thread")]
async fn dual_record_id_shapes_agree() {
    let Some((mem, fork, _guard)) = dual_dbs("record_ids").await else {
        return;
    };
    assert_dual_same(
        "record_ids",
        &mem,
        &fork,
        &[
            r#"CREATE pe:⟨4000000001_10⟩ SET noun = 'PIPE', name = 'p1';"#,
            r#"CREATE pe:['4000000001_10', 5] SET noun = 'PIPE_H';"#,
            "SELECT * FROM pe ORDER BY id;",
            "RETURN type::thing('pe', '4000000001_10');",
            "RETURN record::id(pe:⟨4000000001_10⟩);",
            r#"RETURN <string> pe:⟨4000000001_10⟩;"#,
            "RETURN pe:⟨4000000001_10⟩ == type::thing('pe', '4000000001_10');",
        ],
    )
    .await;
}

/// schemaless 表接受裸对象（嵌套对象 / 数组 / 点路径更新）的行为对拍。
/// 生成产物（inst_geo.pts 等）就是这类形态。
#[tokio::test(flavor = "multi_thread")]
async fn dual_schemaless_bare_objects_agree() {
    let Some((mem, fork, _guard)) = dual_dbs("bare_objects").await else {
        return;
    };
    assert_dual_same(
        "bare_objects",
        &mem,
        &fork,
        &[
            "UPSERT inst_geo:x CONTENT { pts: [{ x: 1.0, y: 2.0, z: 3.0 }], meta: { hash: 'abc', n: 3 }, arr: [1, 2, 3] };",
            "SELECT * FROM inst_geo;",
            "UPDATE inst_geo:x SET meta.n += 1, arr += 4;",
            "SELECT VALUE meta.n FROM inst_geo;",
            "SELECT VALUE arr FROM inst_geo;",
            "UPSERT world_trans:⟨123_45⟩ CONTENT { m: [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1] };",
            "SELECT * FROM world_trans;",
        ],
    )
    .await;
}

/// P0 判据（层级查询优化方案，`docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`）：
/// `inst_relate` 增加 `anc: array<int>`（RefU64 打包祖先链）与 `dbnum: int` 之后，
/// 2.1.4 上「普通索引 + CONTAINS」这条主路线是否成立。
///
/// 三层验收，故意按信息量排序——一次运行无论 Go / No-Go 都拿到完整判据：
/// 1. 行为对拍：数组列/标量列索引 DEFINE 合法、CONTAINS / CONTAINSANY / dbnum
///    过滤在 mem 与 fork 上结果一致，i64::MAX 边界值往返保真；
/// 2. 标量索引绝对断言（备选路线地板）：`dbnum = n` 的 EXPLAIN 走 `idx_ir_dbnum`；
/// 3. 数组索引绝对断言（主路线 Go/No-Go）：`anc CONTAINS n` 的 EXPLAIN 走
///    `idx_ir_anc`。第 3 步失败即 No-Go，方案切固定标量列备选，第 2 步已证明
///    备选可行。
///
/// 追加钉 P3 迁移语句：`REMOVE INDEX IF EXISTS` 在「索引不存在」「表都不存在」
/// 「定义过再摘除、重复摘除」三种情况下都是双引擎一致的安全 no-op——生产启动
/// 与暂存建库共用的 `INST_RELATE_INDEX_SQL` 首行靠它退役 zone_refno 索引
/// （fork 侧已实测；这里连 mem 一起钉住，防升级回归）。
#[tokio::test(flavor = "multi_thread")]
async fn dual_inst_relate_anc_u64_contains_index_agrees() {
    let Some((mem, fork, _guard)) = dual_dbs("anc_u64_index").await else {
        return;
    };

    // 真实 RefU64 打包形制：高位 ref0、低位 ref1（与生产 refno 数量级一致）。
    let site = RefU64((24383u64 << 32) | 1).0;
    let zone = RefU64((24383u64 << 32) | 66457).0;
    let pipe = RefU64((24383u64 << 32) | 66458).0;
    let bran = RefU64((24383u64 << 32) | 66459).0;
    let leaf_a = RefU64((24383u64 << 32) | 70001).0;
    let leaf_b = RefU64((24383u64 << 32) | 70002).0;
    let other_zone = RefU64((17496u64 << 32) | 195578).0;
    let leaf_c = RefU64((17496u64 << 32) | 200001).0;
    let boundary = i64::MAX as u64;

    // 生产形制：inst_relate 是 pe -> inst_info 的 RELATION 边表（schemaless 裸字段）。
    let schema = "DEFINE TABLE inst_relate TYPE RELATION IN pe OUT inst_info SCHEMALESS; \
                  DEFINE INDEX IF NOT EXISTS idx_ir_anc ON TABLE inst_relate COLUMNS anc; \
                  DEFINE INDEX IF NOT EXISTS idx_ir_dbnum ON TABLE inst_relate COLUMNS dbnum;";
    let seed = format!(
        "CREATE pe:a SET noun = 'BOX'; CREATE pe:b SET noun = 'BOX'; CREATE pe:c SET noun = 'BOX'; \
         CREATE inst_info:h1; CREATE inst_info:h2; CREATE inst_info:h3; \
         INSERT RELATION INTO inst_relate [\
            {{ id: inst_relate:ra, in: pe:a, out: inst_info:h1, dbnum: 7997, anc: [{leaf_a}, {bran}, {pipe}, {zone}, {site}], aabb: {{ d: [0,0,0,1,1,1] }} }}, \
            {{ id: inst_relate:rb, in: pe:b, out: inst_info:h2, dbnum: 7997, anc: [{leaf_b}, {bran}, {pipe}, {zone}, {site}], aabb: {{ d: [1,1,1,2,2,2] }} }}, \
            {{ id: inst_relate:rc, in: pe:c, out: inst_info:h3, dbnum: 7999, anc: [{leaf_c}, {other_zone}, {site}], aabb: {{ d: [2,2,2,3,3,3] }} }}\
         ];"
    );
    // 读侧目标形态：任意根一条 CONTAINS 直接圈出子树实例（P2 的查询骨架）。
    let q_bran = format!(
        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINS {bran} AND aabb.d != none ORDER BY id;"
    );
    let q_zone = format!(
        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINS {zone} AND aabb.d != none ORDER BY id;"
    );
    let q_site = format!(
        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINS {site} AND aabb.d != none ORDER BY id;"
    );
    // dbnum 是按库补丁式刷新的过滤键（P3 衔接），与 anc 组合必须可用。
    let q_site_dbnum = format!(
        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINS {site} AND dbnum = 7999 ORDER BY id;"
    );
    // 多根一把查（全场景重载 19 根可合并成一条）。
    let q_multi = format!(
        "SELECT VALUE record::id(id) FROM inst_relate WHERE anc CONTAINSANY [{zone}, {other_zone}] ORDER BY id;"
    );
    // i64 边界：打包值列的天花板必须往返保真、CONTAINS 命中。
    let seed_boundary = format!(
        "CREATE pe:bmax SET noun = 'BOX'; CREATE inst_info:hmax; \
         INSERT RELATION INTO inst_relate [{{ id: inst_relate:rmax, in: pe:bmax, out: inst_info:hmax, dbnum: 1, anc: [{boundary}], aabb: {{ d: [0,0,0,1,1,1] }} }}];"
    );
    let q_boundary = format!("SELECT VALUE anc FROM inst_relate WHERE anc CONTAINS {boundary};");

    assert_dual_same(
        "anc_u64_index",
        &mem,
        &fork,
        &[
            schema,
            &seed,
            &q_bran,
            &q_zone,
            &q_site,
            &q_site_dbnum,
            &q_multi,
            &seed_boundary,
            &q_boundary,
            // P3 迁移语句（REMOVE INDEX IF EXISTS）的三种 no-op 情况 + INFO 终态对拍。
            "REMOVE INDEX IF EXISTS idx_inst_relate_zone_refno ON TABLE inst_relate;",
            "REMOVE INDEX IF EXISTS idx_gone ON TABLE table_never_created;",
            "DEFINE INDEX IF NOT EXISTS idx_ir_tmp ON TABLE inst_relate COLUMNS generic;",
            "REMOVE INDEX IF EXISTS idx_ir_tmp ON TABLE inst_relate;",
            "REMOVE INDEX IF EXISTS idx_ir_tmp ON TABLE inst_relate;",
            "INFO FOR TABLE inst_relate;",
        ],
    )
    .await;

    // 对拍只证两边一致，走没走索引要绝对断言。先钉标量（备选路线的地板），
    // 再钉数组 CONTAINS（主路线 Go/No-Go）——数组失败时标量结论已经在手。
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        let plan = exec_capture(db, "SELECT * FROM inst_relate WHERE dbnum = 7999 EXPLAIN;").await;
        let text = plan[0].as_ref().expect("dbnum EXPLAIN 应可执行");
        assert!(
            text.contains("idx_ir_dbnum"),
            "[{engine}] 标量等值过滤应走 idx_ir_dbnum 索引，plan: {text}"
        );
    }
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        let plan = exec_capture(
            db,
            &format!("SELECT * FROM inst_relate WHERE anc CONTAINS {bran} EXPLAIN;"),
        )
        .await;
        let text = plan[0].as_ref().expect("anc EXPLAIN 应可执行");
        assert!(
            text.contains("idx_ir_anc"),
            "[{engine}] anc CONTAINS 应走 idx_ir_anc 索引（主路线 Go/No-Go 判据），plan: {text}"
        );
    }
}

/// P1 判据（层级查询优化方案 P1 写入侧）：`fn::refno_u64` / `fn::anc_u64`
/// 在 2.1.4 上**真的定义得进、执行得对**。
///
/// 为什么不能只靠启动重放对拍：`define_common_functions` 逐语句吞错，
/// `DEFINE FUNCTION OVERWRITE` 或闭包（`|$a| …`）若在 2.1.4 不合法，两引擎会
/// **同样**缺函数、INFO 对拍照样一字不差——假绿。这里先绝对断言两函数在场，
/// 再逐个执行：字符串 id / 历史数组 id 打包、owner 链上溯（NONE 与 pe:0_0
/// 哨兵过滤）、悬空 record 的空数组兜底，最后按生产字面量形态
/// （`save_instance_data` 的 INSERT RELATION、`gen_cata_geos` 的 RELATE SET）
/// 端到端落值并绝对断言打包值与链序。
///
/// 一并钉 P3 读侧便捷层 `fn::zone_u64` / `fn::site_u64`（anc 尾部定位，
/// 链尾 ref1==0 即 WORL 的自适应偏移）：本用例的两族种子恰好覆盖两种尾形——
/// 24383 族 SITE.owner→pe:0_0 哨兵被滤、链止于 SITE（无 WORL 形），24384 族
/// SITE.owner 悬空指向 ref1=0 的 WORL（生产形，WORL 行不入库）。函数体里的
/// `array::at` 负下标与 `%` 若在 2.1.4 不合法，同样靠在场断言防假绿。
#[tokio::test(flavor = "multi_thread")]
async fn dual_anc_u64_functions_execute_and_agree() {
    let Some((mem, fork, _guard)) = dual_dbs("anc_u64_fns").await else {
        return;
    };
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        replay_startup_defines(db)
            .await
            .unwrap_or_else(|e| panic!("[{engine}] 启动 DEFINE 重放失败: {e}"));
        let info = info_for_db(db).await;
        for function in [
            "fn::refno_u64",
            "fn::anc_u64",
            "fn::zone_u64",
            "fn::site_u64",
        ] {
            assert!(
                info.contains(function),
                "[{engine}] 启动重放后 {function} 必须在场——define_common_functions \
                 吞逐语句错误，缺失说明 common.surql 里该定义没被 2.1.4 接受"
            );
        }
    }

    let site = RefU64((24383u64 << 32) | 1).0;
    let zone = RefU64((24383u64 << 32) | 66457).0;
    let pipe = RefU64((24383u64 << 32) | 66458).0;
    let bran = RefU64((24383u64 << 32) | 66459).0;

    // owner 链：SITE.owner → pe:0_0（null 哨兵）；链顶之上的多余跳是 NONE——
    // 两种哨兵都必须被滤掉。历史数组 id 行与最新行并存（F4 的现实形制）。
    // 24384 族是生产形：WORL（pe:24384_0，ref1=0）行不入库、owner 悬空指向它，
    // 链尾收着真实打包值——zone_u64/site_u64 的自适应偏移在这一形态下取 1。
    let seed = "CREATE pe:0_0; \
         CREATE pe:24383_1 SET noun='SITE', dbnum=7997, owner=pe:0_0; \
         CREATE pe:24383_66457 SET noun='ZONE', dbnum=7997, owner=pe:24383_1; \
         CREATE pe:24383_66458 SET noun='PIPE', dbnum=7997, owner=pe:24383_66457; \
         CREATE pe:24383_66459 SET noun='BRAN', dbnum=7997, owner=pe:24383_66458; \
         CREATE pe:['24383_66459', 5] SET noun='BRAN'; \
         CREATE pe:24384_2 SET noun='SITE', dbnum=7998, owner=pe:24384_0; \
         CREATE pe:24384_3 SET noun='ZONE', dbnum=7998, owner=pe:24384_2; \
         CREATE pe:24384_4 SET noun='EQUI', dbnum=7998, owner=pe:24384_3; \
         CREATE inst_info:h1; CREATE inst_geo:g1;";
    // 函数调用与 `pe:x.dbnum` 字段访问出现在字面量值位的形态 1:1——这是
    // 回填（backfill_inst_relate_anc）与搬家重算（render_anc_repair_statements）
    // 在持久层现场求值的同款构造（生成主路径 W4 起已是纯数据字面量）。
    let insert_inst_relate = "insert relation into inst_relate [{id: inst_relate:⟨24383_66459⟩, \
         in: pe:24383_66459, out: inst_info:h1, generic: 'BOX', \
         anc: fn::anc_u64(pe:24383_66459), dbnum: pe:24383_66459.dbnum, \
         has_cata_neg: false, solid: true}];";
    let relate_tubi = "relate pe:24383_66459->tubi_relate:[pe:24383_66459, 0]->inst_geo:g1 \
         set bore_size=100, anc=fn::anc_u64(pe:24383_66459), dbnum=pe:24383_66459.dbnum;";

    assert_dual_same(
        "anc_u64_fns",
        &mem,
        &fork,
        &[
            seed,
            "RETURN fn::refno_u64(pe:24383_66459);",
            // 历史数组 id → 取首元素，同一打包值。
            "RETURN fn::refno_u64(pe:['24383_66459', 5]);",
            "RETURN fn::anc_u64(pe:24383_66459);",
            // 悬空 record（行不存在）→ 空数组，不报错。
            "RETURN fn::anc_u64(pe:24383_99999);",
            // P3 读侧便捷层：两种尾形的取位 + 列上投影 + 空链兜底。
            "RETURN fn::zone_u64(fn::anc_u64(pe:24383_66459));",
            "RETURN fn::site_u64(fn::anc_u64(pe:24383_66459));",
            "RETURN fn::anc_u64(pe:24384_4);",
            "RETURN fn::zone_u64(fn::anc_u64(pe:24384_4));",
            "RETURN fn::site_u64(fn::anc_u64(pe:24384_4));",
            "RETURN fn::zone_u64([]);",
            insert_inst_relate,
            "SELECT VALUE anc FROM inst_relate;",
            "SELECT VALUE dbnum FROM inst_relate;",
            "SELECT VALUE fn::zone_u64(anc) FROM inst_relate;",
            relate_tubi,
            "SELECT VALUE anc FROM tubi_relate;",
        ],
    )
    .await;

    // 对拍只证两边一致——错得一样也对拍得过。打包值与链序必须绝对断言。
    let packed = |refno: u64| format!(r#"{{"Number":{{"Int":{refno}}}}}"#);
    let chain = format!(
        r#"{{"Array":[{},{},{},{}]}}"#,
        packed(bran),
        packed(pipe),
        packed(zone),
        packed(site)
    );
    let site_worl = RefU64((24384u64 << 32) | 2).0;
    let zone_worl = RefU64((24384u64 << 32) | 3).0;
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        let out = exec_capture(db, "RETURN fn::refno_u64(pe:24383_66459);").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(packed(bran).as_str()),
            "[{engine}] refno_u64 打包值必须等于 RefU64 的 high<<32|low"
        );
        let out = exec_capture(db, "RETURN fn::anc_u64(pe:24383_66459);").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(chain.as_str()),
            "[{engine}] anc_u64 必须是自身→SITE 的打包链（哨兵滤净、顺序保真）"
        );
        let out = exec_capture(db, "SELECT VALUE anc FROM inst_relate;").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(format!(r#"{{"Array":[{chain}]}}"#).as_str()),
            "[{engine}] 生产 INSERT 字面量写出的 anc 必须落成同一条链"
        );
        let out = exec_capture(db, "SELECT VALUE dbnum FROM inst_relate;").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(r#"{"Array":[{"Number":{"Int":7997}}]}"#),
            "[{engine}] 生产 INSERT 字面量写出的 dbnum 必须取自 pe 行"
        );
        // P3 便捷层的绝对取位：无 WORL 尾形（0_0 哨兵被滤、链止于 SITE）偏移 0。
        let out = exec_capture(db, "RETURN fn::zone_u64(fn::anc_u64(pe:24383_66459));").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(packed(zone).as_str()),
            "[{engine}] 链止于 SITE 的尾形下 zone_u64 必须取到倒数第 2 的 ZONE"
        );
        let out = exec_capture(db, "RETURN fn::site_u64(fn::anc_u64(pe:24383_66459));").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(packed(site).as_str()),
            "[{engine}] 链止于 SITE 的尾形下 site_u64 必须取到链尾 SITE"
        );
        // 生产形（链尾是 ref1=0 的 WORL 悬空链接）偏移 1。
        let out = exec_capture(db, "RETURN fn::zone_u64(fn::anc_u64(pe:24384_4));").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(packed(zone_worl).as_str()),
            "[{engine}] 含 WORL 尾形下 zone_u64 必须取到倒数第 3 的 ZONE"
        );
        let out = exec_capture(db, "RETURN fn::site_u64(fn::anc_u64(pe:24384_4));").await;
        assert_eq!(
            out[0].as_deref(),
            Ok(packed(site_worl).as_str()),
            "[{engine}] 含 WORL 尾形下 site_u64 必须取到倒数第 2 的 SITE"
        );
    }
}

/// P4 判据（写时物化）：平表副本三件套在 2.1.4 上**写得进、扫得动、读得对**。
///
/// 三条生产语句形态逐一钉死：
/// 1. 建行字面量带 `world_trans_d` / `aabb_d`（`save_instance_data` 普通行 /
///    TUBI 行，serde 同形 JSON 纯字面量）；
/// 2. 指针+副本**同语句** UPDATE（`update_inst_relate_aabbs_by_refnos` 的 aabb
///    刷新、`refresh_world_transform_products` 的 transform 便宜路径）；
/// 3. 清扫语句（`sweep_inst_relate_flat`）：LET 圈行 + UPDATE SET 值位里的
///    `out->geo_relate` 图遍历子查询 + `aabb_d = aabb.d` 服务端拷贝——UPDATE
///    记录上下文里的图遍历是全设计最险的构造，两引擎都必须真执行出对的产物。
///
/// 最后绝对断言「平表投影 == 解引用投影」（读侧两段式的等价性判据）与清扫
/// 终止条件（第二轮圈不到行）。
#[tokio::test(flavor = "multi_thread")]
async fn dual_inst_relate_flat_materialization_agrees() {
    let Some((mem, fork, _guard)) = dual_dbs("flat_mat").await else {
        return;
    };

    // 与生产同一 serde 形态渲染副本字面量（Transform: bevy、Aabb: parry3d）。
    let wt_a = serde_json::to_string(&bevy_transform::prelude::Transform::IDENTITY).unwrap();
    let wt_b = serde_json::to_string(&bevy_transform::prelude::Transform::from_xyz(1.0, 2.0, 3.0))
        .unwrap();
    let aabb_a = serde_json::to_string(&parry3d::bounding_volume::Aabb::new(
        parry3d::math::Point::new(0.0f32, 0.0, 0.0),
        parry3d::math::Point::new(1.0f32, 1.0, 1.0),
    ))
    .unwrap();
    let edge_t =
        serde_json::to_string(&bevy_transform::prelude::Transform::from_xyz(9.0, 0.0, 0.0))
            .unwrap();

    // 图形：inst_info:i1 挂三条 geo_relate 边——g1（可见+meshed+Pos，唯一入选）、
    // g2（未 meshed）、g3（Neg）；inst_info:i2 无边（insts_flat 应落空数组而非 NONE）。
    let seed = format!(
        "CREATE pe:24383_1 SET noun='SITE', dbnum=7997; \
         CREATE inst_info:i1; CREATE inst_info:i2; \
         CREATE inst_geo:g1 SET meshed = true; CREATE inst_geo:g2; \
         CREATE inst_geo:g3 SET meshed = true; \
         CREATE trans:t1 SET d = {wt_a}; CREATE trans:t2 SET d = {wt_b}; \
         CREATE trans:te SET d = {edge_t}; \
         CREATE aabb:a1 SET d = {aabb_a}; \
         INSERT RELATION INTO geo_relate [\
            {{ id: 'e1', in: inst_info:i1, out: inst_geo:g1, trans: trans:te, visible: true, geo_type: 'Pos' }}, \
            {{ id: 'e2', in: inst_info:i1, out: inst_geo:g2, trans: trans:te, visible: true, geo_type: 'Pos' }}, \
            {{ id: 'e3', in: inst_info:i1, out: inst_geo:g3, trans: trans:te, visible: true, geo_type: 'Neg' }}\
         ];"
    );
    // 生产建行字面量：普通行 world_trans_d 建行即带（aabb 尚无）；TUBI 行双副本齐活。
    let create_normal = format!(
        "INSERT RELATION INTO inst_relate [{{id: inst_relate:⟨24383_100⟩, in: pe:24383_1, \
         out: inst_info:i1, world_trans: trans:t1, world_trans_d: {wt_a}, generic: 'BOX', \
         anc: [42], dbnum: 7997, dt: NONE, has_cata_neg: false, solid: true}}];"
    );
    let create_tubi = format!(
        "INSERT RELATION INTO inst_relate [{{id: inst_relate:⟨24383_101⟩, in: pe:24383_1, \
         out: inst_info:i2, world_trans: trans:t1, world_trans_d: {wt_a}, aabb: aabb:a1, \
         aabb_d: {aabb_a}, generic: 'TUBI', anc: [42], dbnum: 7997, \
         has_cata_neg: false, solid: true}}];"
    );
    // aabb 刷新与 transform 便宜路径：指针+副本同语句。
    let refresh_aabb =
        format!("UPDATE inst_relate:⟨24383_100⟩ SET aabb = aabb:a1, aabb_d = {aabb_a};");
    let cheap_transform = format!(
        "UPDATE inst_relate:⟨24383_100⟩ SET world_trans = trans:t2, world_trans_d = {wt_b};"
    );
    // 清扫（sweep_inst_relate_flat 同形，BATCH 缩到 10）。
    let sweep = "LET $rows = SELECT VALUE id FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none LIMIT 10;\n\
         UPDATE $rows SET insts_flat = (SELECT trans.d AS transform, record::id(out) AS geo_hash \
         FROM out->geo_relate WHERE visible && out.meshed && trans.d != none && geo_type='Pos'), \
         aabb_d = aabb.d, world_trans_d = world_trans.d RETURN NONE;\n\
         RETURN array::len($rows);";
    // 读侧两形态（FROM 显式记录列表定序，避开 2.1.4 `SELECT VALUE … ORDER BY` 的坑）。
    let read_flat = "SELECT in AS refno, generic, aabb_d AS world_aabb, \
         world_trans_d AS world_trans, insts_flat AS insts \
         FROM [inst_relate:⟨24383_100⟩, inst_relate:⟨24383_101⟩];";
    let read_slim = "SELECT in AS refno, generic, aabb.d AS world_aabb, \
         world_trans.d AS world_trans, \
         (SELECT trans.d AS transform, record::id(out) AS geo_hash FROM out->geo_relate \
         WHERE visible && out.meshed && trans.d != none && geo_type='Pos') AS insts \
         FROM [inst_relate:⟨24383_100⟩, inst_relate:⟨24383_101⟩];";

    assert_dual_same(
        "flat_mat",
        &mem,
        &fork,
        &[
            &seed,
            &create_normal,
            &create_tubi,
            &refresh_aabb,
            &cheap_transform,
            sweep,
            read_flat,
            read_slim,
            // 第二轮应圈不到行（insts_flat 已非 NONE，空边行落的是 [] 不是 NONE）
            // ——sweep 循环的终止条件。
            sweep,
        ],
    )
    .await;

    // 对拍只证两边一致——错得一样也对拍得过。等价性与清扫产物要绝对断言。
    for (engine, db) in [("mem", &mem), ("fork", &fork)] {
        let flat = exec_capture(db, read_flat).await;
        let slim = exec_capture(db, read_slim).await;
        assert_eq!(
            flat[0], slim[0],
            "[{engine}] 平表副本投影必须与解引用投影逐字段相等（读侧两段式的根据）"
        );
        let swept = exec_capture(db, sweep).await;
        assert_eq!(
            swept[2].as_deref(),
            Ok(r#"{"Number":{"Int":0}}"#),
            "[{engine}] 清扫后不应再有 insts_flat = NONE 且对读者可见的行"
        );
        let insts = exec_capture(
            db,
            "SELECT VALUE insts_flat.geo_hash \
             FROM [inst_relate:⟨24383_100⟩, inst_relate:⟨24383_101⟩];",
        )
        .await;
        assert_eq!(
            insts[0].as_deref(),
            Ok(r#"{"Array":[{"Array":[{"Strand":"g1"}]},{"Array":[]}]}"#),
            "[{engine}] insts_flat 应只含可见+meshed+Pos 的边（g2 未 meshed、g3 是 Neg 都不得入选）"
        );
    }
}

/// P2 前置验证（手动 bench）：**旧读路径 vs 新读路径**在同一台 fork rocksdb
/// 服务器、同一棵 AMS 量级合成树上的实测对比 + refno 集合对拍。
///
/// 合成树：19 SITE × 10 ZONE × 15 PIPE × 7 BRAN × 2 叶（ELBO，带 inst_relate）
/// + 每 ZONE 5 个 BOX（可见类型直挂），共 ~6.4 万 pe / ~6.4 万 pe_owner /
/// ~4.1 万 inst_relate——与 AMS 实测口径（19 根 / 3.8 万实例）同量级。
///
/// 旧路径 1:1 复刻 plant-ui vendor/rs-core 的查询形态（`geom.rs`
/// `query_deep_visible_inst_refnos` + `inst.rs` `query_insts`）：
/// 1. 12 层 `<-pe_owner<-` 反向图遍历拉全子树（`graph.rs:33`）；
/// 2. 全部子孙 pe key **内联进 SQL** 过滤 BRAN/HANG（`graph.rs:106`）；
/// 3. 每个 BRAN 一次子查询取成员（`query.rs:909`，串行 N 次往返）；
/// 4. **再来一遍** 12 层遍历 + 内联过滤 39 种可见类型（`geom.rs:70`）；
/// 5. `query_insts`：500/批 record id 列表查投影（`inst.rs:152`）。
///
/// 新路径 = 方案 §2 的一条查询：`WHERE anc CONTAINS $root AND aabb.d != none`
/// 直接回全投影；另测 `CONTAINSANY` 19 根一把查。
///
/// 两路径逐根断言 refno 集合完全一致（P2 对拍口径的合成数据预演）。
///
/// 运行：`cargo test --lib --features http_api bench_anc_contains -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual bench: seeds ~170k rows on a throwaway fork rocksdb server"]
async fn bench_anc_contains_vs_deep_traversal() {
    const SITES: usize = 19;
    const ZONES_PER_SITE: usize = 10;
    const PIPES_PER_ZONE: usize = 15;
    const BRANS_PER_PIPE: usize = 7;
    const LEAVES_PER_BRAN: usize = 2;
    const BOXES_PER_ZONE: usize = 5;
    const REF0: u64 = 24383;
    const DBNUM: u64 = 7997;
    const SHARED_HASHES: usize = 200;
    const SHARED_BOXES: usize = 500;
    const VISIBLE_NOUNS: &str = "'BOX','CYLI','SLCY','CONE','DISH','CTOR','RTOR','PYRA','SNOU','POHE','POLYHE','EXTR','REVO','FLOOR','PANE','ELCONN','CMPF','WALL','GWALL','SJOI','FITT','PFIT','FIXING','PJOI','GENSEC','RNODE','PRTELE','GPART','SCREED','PALJ','CABLE','BATT','CMFI','SCOJ','SEVE','SBFI','STWALL','SCTN','NOZZ'";

    let Some(server) = spawn_fork_server("anc_bench") else {
        return;
    };
    let db = connect_fork(&server.ws_url, "anc_bench").await;

    // ── 建模：一次性生成整棵树的内存描述 ────────────────────────────────
    struct Ele {
        ref1: u64,
        noun: &'static str,
        owner_ref1: u64, // 0 = pe:0_0 哨兵
        anc_ref1s: Vec<u64>,
    }
    let mut eles: Vec<Ele> = Vec::new();
    let mut insts: Vec<usize> = Vec::new(); // eles 下标：带 inst_relate 的行
    let mut site_roots: Vec<u64> = Vec::new();
    let mut next_ref1 = 0u64;
    let mut alloc = || {
        next_ref1 += 1;
        next_ref1
    };
    for _ in 0..SITES {
        let site = alloc();
        site_roots.push(site);
        eles.push(Ele {
            ref1: site,
            noun: "SITE",
            owner_ref1: 0,
            anc_ref1s: vec![site],
        });
        for _ in 0..ZONES_PER_SITE {
            let zone = alloc();
            eles.push(Ele {
                ref1: zone,
                noun: "ZONE",
                owner_ref1: site,
                anc_ref1s: vec![zone, site],
            });
            for _ in 0..BOXES_PER_ZONE {
                let bx = alloc();
                eles.push(Ele {
                    ref1: bx,
                    noun: "BOX",
                    owner_ref1: zone,
                    anc_ref1s: vec![bx, zone, site],
                });
                insts.push(eles.len() - 1);
            }
            for _ in 0..PIPES_PER_ZONE {
                let pipe = alloc();
                eles.push(Ele {
                    ref1: pipe,
                    noun: "PIPE",
                    owner_ref1: zone,
                    anc_ref1s: vec![pipe, zone, site],
                });
                for _ in 0..BRANS_PER_PIPE {
                    let bran = alloc();
                    eles.push(Ele {
                        ref1: bran,
                        noun: "BRAN",
                        owner_ref1: pipe,
                        anc_ref1s: vec![bran, pipe, zone, site],
                    });
                    for _ in 0..LEAVES_PER_BRAN {
                        let leaf = alloc();
                        eles.push(Ele {
                            ref1: leaf,
                            noun: "ELBO",
                            owner_ref1: bran,
                            anc_ref1s: vec![leaf, bran, pipe, zone, site],
                        });
                        insts.push(eles.len() - 1);
                    }
                }
            }
        }
    }
    let packed = |ref1: u64| (REF0 << 32) | ref1;
    println!(
        "[bench] 合成树：pe {} 行，inst_relate {} 行，{} 个 SITE 根",
        eles.len() + 1,
        insts.len(),
        SITES
    );

    // ── 落库：索引先建（写入时维护，与生产一致），再分批灌数 ───────────────
    let exec = |sql: String| {
        let db = db.clone();
        async move {
            db.query(sql)
                .await
                .expect("bench exec")
                .check()
                .expect("bench check");
        }
    };
    exec(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL.to_string()).await;
    exec("CREATE pe:0_0;".into()).await;

    let seed_started = Instant::now();
    for chunk in eles.chunks(1000) {
        let objs = chunk
            .iter()
            .map(|e| {
                let owner = if e.owner_ref1 == 0 {
                    "pe:0_0".to_string()
                } else {
                    format!("pe:{REF0}_{}", e.owner_ref1)
                };
                format!(
                    "{{id: pe:{REF0}_{}, noun: '{}', dbnum: {DBNUM}, owner: {owner}, deleted: false}}",
                    e.ref1, e.noun
                )
            })
            .join(",");
        exec(format!("INSERT INTO pe [{objs}];")).await;
    }
    for chunk in eles.chunks(1000) {
        let objs = chunk
            .iter()
            .map(|e| {
                let owner = if e.owner_ref1 == 0 {
                    "pe:0_0".to_string()
                } else {
                    format!("pe:{REF0}_{}", e.owner_ref1)
                };
                format!(
                    "{{id: pe_owner:[pe:{REF0}_{r}, 0], in: pe:{REF0}_{r}, out: {owner}}}",
                    r = e.ref1
                )
            })
            .join(",");
        exec(format!("INSERT RELATION INTO pe_owner [{objs}];")).await;
    }
    // 共享网格资产：inst_info -> geo_relate -> geo（meshed），与生产实例化形态一致。
    for i in 0..SHARED_HASHES {
        exec(format!(
            "CREATE inst_info:⟨h{i}⟩; CREATE geo:⟨g{i}⟩ SET meshed = true; \
             CREATE trans:⟨tg{i}⟩ SET d = [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1]; \
             RELATE inst_info:⟨h{i}⟩->geo_relate->geo:⟨g{i}⟩ SET visible = true, geo_type = 'Pos', trans = trans:⟨tg{i}⟩;"
        ))
        .await;
    }
    for i in 0..SHARED_BOXES {
        exec(format!(
            "CREATE aabb:⟨a{i}⟩ SET d = [0,0,0, 1,1,1]; \
             CREATE trans:⟨t{i}⟩ SET d = [1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1];"
        ))
        .await;
    }
    for chunk in insts.chunks(1000) {
        let objs = chunk
            .iter()
            .map(|&idx| {
                let e = &eles[idx];
                let anc = e.anc_ref1s.iter().map(|&r| packed(r).to_string()).join(",");
                format!(
                    "{{id: inst_relate:⟨{REF0}_{r}⟩, in: pe:{REF0}_{r}, out: inst_info:⟨h{h}⟩, \
                      generic: '{g}', aabb: aabb:⟨a{a}⟩, world_trans: trans:⟨t{a}⟩, \
                      anc: [{anc}], dbnum: {DBNUM}, solid: true}}",
                    r = e.ref1,
                    h = idx % SHARED_HASHES,
                    g = e.noun,
                    a = idx % SHARED_BOXES,
                )
            })
            .join(",");
        exec(format!("INSERT RELATION INTO inst_relate [{objs}];")).await;
    }
    println!("[bench] 灌数完成，耗时 {:?}", seed_started.elapsed());

    // ── 查询形态（1:1 vendor/rs-core）─────────────────────────────────────
    let traversal_sql = |root: u64| {
        format!(
            r#"return array::flatten( object::values( (select
                  [id] as p0, <-pe_owner[? !in.deleted]<-(? as p1)<-pe_owner<-(? as p2)<-pe_owner<-(? as p3)<-pe_owner<-(? as p4)<-pe_owner<-(? as p5)<-pe_owner<-(? as p6)<-pe_owner<-(? as p7)<-pe_owner<-(? as p8)<-pe_owner<-(? as p9)<-pe_owner<-(? as p10)<-pe_owner<-(? as p11)
                   from only pe:{REF0}_{root} where record::exists(id))?:{{}} ) )[? !deleted];"#
        )
    };
    const INST_PROJECTION: &str = "in.id as refno, in.old_pe as old_refno, in.owner as owner, generic, \
         aabb.d as world_aabb, world_trans.d as world_trans, out.ptset.d.pt as pts, \
         (select trans.d as transform, record::id(out) as geo_hash from out->geo_relate \
          where visible && out.meshed && trans.d != none && geo_type='Pos') as insts, \
         booled_id != none as has_neg, dt as date";

    // 结果行 → refno 文本集合（serde 外部标签形制：{"Array":[{"Object":{"refno":{"Thing":…}}}]}）。
    fn refno_set(rows: &serde_json::Value) -> Vec<String> {
        let mut out: Vec<String> = rows["Array"]
            .as_array()
            .unwrap_or_else(|| panic!("rows not array: {rows}"))
            .iter()
            .map(|row| {
                let obj = row.get("Object").unwrap_or(row);
                obj["refno"]["Thing"]["id"]["String"]
                    .as_str()
                    .unwrap_or_else(|| panic!("row without refno thing: {row}"))
                    .to_string()
            })
            .collect();
        out.sort();
        out
    }
    async fn take_rows(db: &Surreal<Any>, sql: &str, idx: usize) -> serde_json::Value {
        let mut response = db.query(sql).await.expect("query").check().expect("check");
        let value: surrealdb::Value = response.take(idx).expect("take rows");
        serde_json::to_value(&value).expect("serialize rows")
    }

    let mut old_total = Duration::ZERO;
    let mut new_total = Duration::ZERO;
    let (mut t_trav, mut t_filter, mut t_children, mut t_insts) = (
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
    );
    let mut max_inline_sql_bytes = 0usize;

    for (i, &root) in site_roots.iter().enumerate() {
        // ── 旧路径 ──
        let old_started = Instant::now();
        // 1) 深遍历 ×1
        let t = Instant::now();
        let mut response = db.query(traversal_sql(root)).await.expect("trav1");
        let descendants: Vec<aios_core::RefnoEnum> = response.take(0).expect("trav1 take");
        t_trav += t.elapsed();
        // 2) 巨型内联过滤 BRAN/HANG
        let pe_keys = descendants.iter().map(|x| x.to_pe_key()).join(",");
        let t = Instant::now();
        let sql = format!("select value id from [{pe_keys}] where noun in ['BRAN','HANG']");
        max_inline_sql_bytes = max_inline_sql_bytes.max(sql.len());
        let mut response = db.query(sql).await.expect("filter brans");
        let brans: Vec<aios_core::RefnoEnum> = response.take(0).expect("brans take");
        t_filter += t.elapsed();
        // 3) 每 BRAN 一次子查询
        let t = Instant::now();
        let mut union: Vec<aios_core::RefnoEnum> = Vec::new();
        for bran in &brans {
            let sql = format!(
                "select value in from {}<-pe_owner  where in.id!=none and record::exists(in.id) and !in.deleted",
                bran.to_pe_key()
            );
            let mut response = db.query(sql).await.expect("children");
            let children: Vec<aios_core::RefnoEnum> = response.take(0).expect("children take");
            union.extend(children);
        }
        t_children += t.elapsed();
        // 4) 深遍历 ×2 + 内联过滤可见类型
        let t = Instant::now();
        let mut response = db.query(traversal_sql(root)).await.expect("trav2");
        let descendants2: Vec<aios_core::RefnoEnum> = response.take(0).expect("trav2 take");
        t_trav += t.elapsed();
        let pe_keys2 = descendants2.iter().map(|x| x.to_pe_key()).join(",");
        let t = Instant::now();
        let sql = format!("select value id from [{pe_keys2}] where noun in [{VISIBLE_NOUNS}]");
        max_inline_sql_bytes = max_inline_sql_bytes.max(sql.len());
        let mut response = db.query(sql).await.expect("filter visible");
        let visible: Vec<aios_core::RefnoEnum> = response.take(0).expect("visible take");
        t_filter += t.elapsed();
        union.extend(visible);
        // 5) query_insts：500/批 record id 列表
        let t = Instant::now();
        let mut old_rows: Vec<String> = Vec::new();
        for chunk in union.chunks(500) {
            let inst_keys = chunk.iter().map(|x| x.to_inst_relate_key()).join(",");
            let sql = format!("select {INST_PROJECTION} from {inst_keys} where aabb.d != none");
            let rows = take_rows(&db, &sql, 0).await;
            old_rows.extend(refno_set(&rows));
        }
        t_insts += t.elapsed();
        old_rows.sort();
        let old_elapsed = old_started.elapsed();
        old_total += old_elapsed;

        // ── 新路径：一条索引查询 ──
        let new_started = Instant::now();
        let sql = format!(
            "select {INST_PROJECTION} from inst_relate where anc CONTAINS {} and aabb.d != none",
            packed(root)
        );
        let new_rows = refno_set(&take_rows(&db, &sql, 0).await);
        let new_elapsed = new_started.elapsed();
        new_total += new_elapsed;

        assert_eq!(old_rows, new_rows, "root #{i} 新旧路径 refno 集合必须一致");
        if i == 0 {
            println!(
                "[bench] root #0：{} 实例；旧 {old_elapsed:?} → 新 {new_elapsed:?}",
                new_rows.len()
            );
        }
    }

    // 全场景 19 根一把查（CONTAINSANY 合并）。
    let t = Instant::now();
    let roots = site_roots.iter().map(|&r| packed(r).to_string()).join(",");
    let sql = format!(
        "select {INST_PROJECTION} from inst_relate where anc CONTAINSANY [{roots}] and aabb.d != none"
    );
    let all_rows = refno_set(&take_rows(&db, &sql, 0).await);
    let containsany_elapsed = t.elapsed();
    assert_eq!(all_rows.len(), insts.len(), "全场景实例数必须对上");

    println!("┌──────────────────────────────────────────────────────────");
    println!("│ [bench] {SITES} 根全场景（{} 实例）", insts.len());
    println!("│ 旧路径合计         : {old_total:?}");
    println!("│   ├─ 12 层深遍历×2 : {t_trav:?}");
    println!("│   ├─ 巨型 IN 过滤×2: {t_filter:?}（单条 SQL 最大 {max_inline_sql_bytes} 字节）");
    println!("│   ├─ 每 BRAN 子查询: {t_children:?}");
    println!("│   └─ query_insts   : {t_insts:?}");
    println!("│ 新路径合计（19 条）: {new_total:?}");
    println!("│ 新路径 CONTAINSANY : {containsany_elapsed:?}（19 根一把查）");
    println!(
        "│ 加速比             : {:.1}x（逐根） / {:.1}x（一把查）",
        old_total.as_secs_f64() / new_total.as_secs_f64().max(1e-9),
        old_total.as_secs_f64() / containsany_elapsed.as_secs_f64().max(1e-9)
    );
    println!("└──────────────────────────────────────────────────────────");
}

/// 启动 DEFINE 全套在两个引擎上重放后，`INFO FOR DB` 渲染必须一致
/// （函数体、事件、分析器一字不差），`fn::room_num_of` 的执行结果也一致。
#[tokio::test(flavor = "multi_thread")]
async fn dual_startup_define_replay_info_parity() {
    let Some((mem, fork, _guard)) = dual_dbs("define_parity").await else {
        return;
    };
    replay_startup_defines(&mem).await.expect("mem replay");
    replay_startup_defines(&fork).await.expect("fork replay");

    let mem_info = info_for_db(&mem).await;
    let fork_info = info_for_db(&fork).await;
    assert_eq!(
        mem_info, fork_info,
        "启动 DEFINE 重放后的 INFO FOR DB 应一字不差"
    );

    assert_dual_same(
        "define_parity_exec",
        &mem,
        &fork,
        &[
            r#"
            CREATE elem:e1 SET noun = 'BOX';
            CREATE panel:p1 SET noun = 'PANE';
            INSERT RELATION INTO room_relate [
                { id: room_relate:r1, in: panel:p1, out: elem:e1, room_num: 'R201', inside_count: 6, center_dist: 2.0 }
            ];
            RETURN fn::room_num_of(elem:e1);
            "#,
        ],
    )
    .await;
}
