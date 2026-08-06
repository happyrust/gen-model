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
