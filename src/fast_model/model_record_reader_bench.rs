//! 冷读翻页的回归与基准夹具（2026-11-11 从 `droid/coldread-paging` 并进来，改成对着本仓现在的读法）。
//!
//! `model_record_reader` 自己的单测夹具最多几行，一页（[`DURABLE_READ_CHUNK`] = 5000）都装不满，翻页的毛病一条也碰不到。
//! 这里造行数过页的根，把 `/model/records` 用的两种翻法——整根（[`durable_model_records_of_root`]）与按窗口
//! （[`durable_model_records_window`]）——都走一遍，逐条对着不分页的整根读法（[`durable_records_of_root`]）：
//! 行不能少、不能重、顺序相同。
//!
//! 四种形状各管一个毛病（「没修时」是 2026-11-11 对着修之前的读法实跑的结果）：
//!
//! | 用例 | 形状 | 没修时 |
//! |---|---|---|
//! | `coldread_paging_advances_past_the_first_chunk` | 5001 行，记录 id 与 `in` 不是同一串，两路都有行 | 整根读回 7502 行，其中 2501 行重复 |
//! | `coldread_paging_keeps_rows_from_both_scopes` | 10000 行，两路各 5000 | 整根读不完：游标停在原地，同一页翻不完 |
//! | `coldread_paging_keeps_the_slot_of_a_tube_cursor` | 5010 条管身，记录 id 是 `[分支, 槽位]` | 整根读不完；按窗口翻一直有「下一页」，读回的全是头一页（重复 10000 行），最后 10 行读不到 |
//! | `coldread_paging_keeps_the_rows_ahead_of_the_roots_own_row` | 现场的 id 形状，根自己的那一行排在 6000 行子树行后面 | 两种翻法都悄悄少 1000 行 |
//!
//! 基准那一条要开环境变量才跑：
//! ```text
//! AIOS_COLDREAD_BENCH=1 cargo test --lib coldread_ -- --nocapture --test-threads=1
//! ```
//!
//! 另有一条真库只读对拍（`coldread_live_paging_matches_the_row_counts_on_the_database`，`--ignored`）：给一份根清单，
//! 逐根把两种翻法读回来的行数对着库上直接数出来的准数。只发 `SELECT`。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use aios_core::RefnoEnum;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

use super::model_record_reader::{
    DURABLE_READ_CHUNK, ModelRecord, durable_count_sql, durable_model_records_of_root,
    durable_model_records_window, durable_records_of_root,
};

/// 基准的规模：`2 * CHUNK + 1`，至少跨三页。
const BENCH_ROWS: usize = 2 * DURABLE_READ_CHUNK + 1;

/// 按窗口翻时一窗多少条。故意不取页大小的约数：窗口的边与库上一页的边错开。
const WINDOW: usize = 3000;

/// 一条分支上几段管身。槽位过 10：`[分支, 10]` 得排在 `[分支, 9]` 后面（按数排，不是按字面排）。
const TUBE_SLOTS: usize = 15;

fn bench_enabled() -> bool {
    std::env::var("AIOS_COLDREAD_BENCH").is_ok()
}

/// 一只空的内存库：建好现场那几条索引（冷读的两路各走各的索引）、放好共用的包围盒与世界变换。
async fn scratch_db(name: &str) -> Surreal<Any> {
    let db = connect("mem://").await.unwrap();
    db.use_ns("bench").use_db(name).await.unwrap();
    db.query(crate::fast_model::pdms_inst::INST_RELATE_INDEX_SQL)
        .await
        .unwrap()
        .check()
        .unwrap();
    db.query(
        "CREATE aabb:bench SET d={mins:[0,0,0],maxs:[1,1,1]};
         CREATE trans:bench SET d={translation:[0,0,0],rotation:[0,0,0,1],scale:[1,1,1]};",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    db
}

/// 造 `count` 条 `inst_relate`：记录 id 是 `24384_<first + i>`，`in` 由 `in_expr` 给（可以用 `$i`、`$k`），`anc` 命中根。
/// 每行把自己的 id 那一串写进 `booled_id`——读回来在 `insts[0].geo_hash`，用例拿它认行。
async fn seed_inst_rows(db: &Surreal<Any>, packed: u64, in_expr: &str, first: usize, count: usize) {
    db.query(format!(
        "FOR $i IN 0..{count} {{
           LET $k = '24384_' + <string> ({first} + $i);
           CREATE type::thing('inst_relate', $k) SET
             in = {in_expr}, anc = [{packed}],
             aabb = aabb:bench, world_trans = trans:bench, generic = 'FLOOR', booled_id = $k;
         }};"
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
}

/// 造一根名下有 `rows` 条 `inst_relate` 的库，分两组，id 区间不重叠：
///
/// - 子树组：id `24384_1xxxxx`，`in` 各不相同（`pe:24384_8xxxxx`）；
/// - 根自身组：id `24384_5xxxxx`，`in` 就是根——`in = 根` 那一路读到的就是它们。
///
/// 记录 id 与 `in` 不是同一串：游标要是拿 `in` 去拼，拼出来的就不是这一行的 id。
async fn seed_root(db: &Surreal<Any>, root: RefnoEnum, rows: usize) {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let subtree = rows / 2;
    let own = rows - subtree;
    seed_inst_rows(
        db,
        packed,
        "type::thing('pe', '24384_' + <string> (800000 + $i))",
        100_000,
        subtree,
    )
    .await;
    seed_inst_rows(db, packed, &owner, 500_000, own).await;
}

/// 造 `branches` 条分支、每条 [`TUBE_SLOTS`] 段管身：记录 id 是 `[pe:<分支>, <槽位>]`，`in` 是分支，`anc` 命中根。
/// 每段把 `<分支>_<槽位>` 写进 `out` 的 id——读回来在 `insts[0].geo_hash`。两端都是分支自己，所以同一条分支上
/// 各段的序号靠出现的先后补：读回来的 `tube.ordinal` 必须等于槽位。
async fn seed_tubes(db: &Surreal<Any>, packed: u64, branches: usize) {
    db.query(format!(
        "FOR $b IN 0..{branches} {{
           LET $bk = '24384_' + <string> (200000 + $b);
           FOR $s IN 0..{TUBE_SLOTS} {{
             CREATE type::thing('tubi_relate', [type::thing('pe', $bk), $s]) SET
               in = type::thing('pe', $bk), out = type::thing('inst_geo', $bk + '_' + <string> $s),
               anc = [{packed}], aabb = aabb:bench, world_trans = trans:bench,
               leave = type::thing('pe', $bk), arrive = type::thing('pe', $bk);
           }};
         }};"
    ))
    .await
    .unwrap()
    .check()
    .unwrap();
}

/// 一条记录在夹具里的标记。
fn tag_of_inst(inst: &aios_core::GeomInstQuery) -> String {
    inst.insts
        .first()
        .map(|inst| inst.geo_hash.clone())
        .unwrap_or_default()
}

fn tags(records: &[ModelRecord]) -> Vec<String> {
    records
        .iter()
        .map(|record| tag_of_inst(&record.inst))
        .collect()
}

/// 读回来的行与该有的行差在哪；一样就是 `None`。
fn difference(what: &str, got: &[String], want: &[String]) -> Option<String> {
    if got == want {
        return None;
    }
    let mut seen = HashSet::new();
    let repeated = got.iter().filter(|tag| !seen.insert(tag.as_str())).count();
    let wanted: HashSet<&str> = want.iter().map(String::as_str).collect();
    let missing = wanted.iter().filter(|tag| !seen.contains(**tag)).count();
    let unexpected = seen.iter().filter(|tag| !wanted.contains(**tag)).count();
    Some(format!(
        "{what}：读回 {} 行，该有 {} 行；重复 {repeated}、缺 {missing}、多出 {unexpected}{}",
        got.len(),
        want.len(),
        if repeated + missing + unexpected == 0 {
            "（行集相同，顺序不同）"
        } else {
            ""
        }
    ))
}

/// 按窗口把一根翻完。翻了该翻的窗数还说有下一页就停——第二格回 `false`。
async fn read_by_windows(
    db: &Surreal<Any>,
    root: RefnoEnum,
    expected: usize,
) -> (Vec<ModelRecord>, bool) {
    let mut out = Vec::new();
    let mut skip = 0;
    for _ in 0..(expected / WINDOW + 4) {
        let window = durable_model_records_window(db, root, skip, WINDOW)
            .await
            .unwrap_or_else(|error| panic!("按窗口读 {root}（skip={skip}）失败: {error:#}"));
        skip += window.rows.len();
        out.extend(window.rows);
        if !window.more {
            return (out, true);
        }
    }
    (out, false)
}

/// 两种翻法各读一遍，对着不分页的整根读法。交回哪里不对；空 = 都对。
///
/// 整根那一遍有时限：翻页的游标不往前走时它读不完，得让用例败下来而不是挂着。时限跟着不分页那一遍的用时走
/// （同一台机器、同一批行），机器忙的时候不会误报。
async fn paging_problems(db: &Surreal<Any>, root: RefnoEnum, expected: usize) -> Vec<String> {
    let mut problems = Vec::new();

    let started = Instant::now();
    let reference = durable_records_of_root(db, root).await.unwrap();
    let reference_took = started.elapsed();
    let want: Vec<String> = reference.iter().map(tag_of_inst).collect();
    if want.len() != expected || want.iter().collect::<HashSet<_>>().len() != expected {
        problems.push(format!(
            "夹具不对：不分页读回 {} 行，该是 {expected} 行各不相同",
            want.len()
        ));
    }

    let limit = Duration::from_secs(10) + reference_took * 20;
    let started = Instant::now();
    match tokio::time::timeout(limit, durable_model_records_of_root(db, root)).await {
        Err(_) => problems.push(format!(
            "整根：{} 秒没读完（不分页读一遍只要 {} 毫秒）——翻页没有往前走",
            limit.as_secs(),
            reference_took.as_millis()
        )),
        Ok(Err(error)) => problems.push(format!("整根：读失败: {error:#}")),
        Ok(Ok(records)) => {
            println!(
                "[coldread] {root} 整根 {} 行，{} 毫秒（不分页 {} 毫秒）",
                records.len(),
                started.elapsed().as_millis(),
                reference_took.as_millis()
            );
            problems.extend(difference("整根", &tags(&records), &want));
            problems.extend(tube_ordinal_problem("整根", &records));
        }
    }

    let (records, finished) = read_by_windows(db, root, expected).await;
    if !finished {
        problems.push(format!(
            "按窗口：翻了 {} 窗还说有下一页",
            expected / WINDOW + 4
        ));
    }
    problems.extend(difference("按窗口", &tags(&records), &want));
    problems.extend(tube_ordinal_problem("按窗口", &records));
    problems
}

/// 管身的序号得等于槽位（[`seed_tubes`]）：页的边落在一条分支中间时，序号不能从头再数。
fn tube_ordinal_problem(what: &str, records: &[ModelRecord]) -> Option<String> {
    let wrong = records
        .iter()
        .filter_map(|record| {
            let tube = record.tube.as_ref()?;
            let tag = tag_of_inst(&record.inst);
            let slot: u32 = tag.rsplit('_').next()?.parse().ok()?;
            (tube.ordinal != slot).then(|| format!("{tag} 的序号是 {}", tube.ordinal))
        })
        .collect::<Vec<_>>();
    (!wrong.is_empty()).then(|| {
        format!(
            "{what}：{} 段管身的序号不等于槽位，头一段：{}",
            wrong.len(),
            wrong[0]
        )
    })
}

/// 基准：一根 [`BENCH_ROWS`] 行的根，走真实冷读路径。
#[tokio::test(flavor = "multi_thread")]
async fn coldread_bench_multi_page_root() {
    if !bench_enabled() {
        return;
    }
    let db = scratch_db("coldread").await;
    let root: RefnoEnum = "24384_26606".into();
    let seed_start = Instant::now();
    seed_root(&db, root, BENCH_ROWS).await;
    let seed_ms = seed_start.elapsed().as_millis();

    let started = Instant::now();
    let records = durable_model_records_of_root(&db, root).await.unwrap();
    let elapsed = started.elapsed();
    let mut seen = HashSet::new();
    let duplicates = tags(&records)
        .into_iter()
        .filter(|tag| !seen.insert(tag.clone()))
        .count();

    println!(
        "\n[coldread-bench] 夹具 {BENCH_ROWS} 行（CHUNK={DURABLE_READ_CHUNK}），造数 {seed_ms} ms"
    );
    println!(
        "[coldread-bench] 冷读返回 {} 行，耗时 {elapsed:?}，重复 {duplicates}",
        records.len()
    );
    assert_eq!(records.len(), BENCH_ROWS, "冷读必须取回整根的全部行");
    assert_eq!(duplicates, 0, "分页不得重复返回同一行");
}

/// 回归：过了第一页，游标必须真的往前走。`CHUNK + 1` 行，两页即暴露。
#[tokio::test(flavor = "multi_thread")]
async fn coldread_paging_advances_past_the_first_chunk() {
    let db = scratch_db("coldread_advance").await;
    let root: RefnoEnum = "24384_26607".into();
    let rows = DURABLE_READ_CHUNK + 1;
    seed_root(&db, root, rows).await;

    let problems = paging_problems(&db, root, rows).await;
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 回归：两路作用域各读各的、各自截断之后，合起来不能丢行也不能原地打转。
#[tokio::test(flavor = "multi_thread")]
async fn coldread_paging_keeps_rows_from_both_scopes() {
    let db = scratch_db("coldread_scopes").await;
    let root: RefnoEnum = "24384_26608".into();
    let rows = 2 * DURABLE_READ_CHUNK;
    seed_root(&db, root, rows).await;

    let problems = paging_problems(&db, root, rows).await;
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 回归：管身记录的 id 是 `[分支, 槽位]`，游标得带着槽位。拿分支（`in`）拼出来的 id 排在所有管身行前面，
/// 下一页又从头读起。一页的边落在一条分支中间（5000 不是 [`TUBE_SLOTS`] 的倍数）。
#[tokio::test(flavor = "multi_thread")]
async fn coldread_paging_keeps_the_slot_of_a_tube_cursor() {
    let db = scratch_db("coldread_tubes").await;
    let root: RefnoEnum = "24384_26620".into();
    let branches = DURABLE_READ_CHUNK / TUBE_SLOTS + 1;
    let rows = branches * TUBE_SLOTS;
    assert!(rows > DURABLE_READ_CHUNK && DURABLE_READ_CHUNK % TUBE_SLOTS != 0);
    seed_tubes(&db, root.refno().0, branches).await;

    let problems = paging_problems(&db, root, rows).await;
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 回归：现场的 id 形状（`inst_relate` 的 id 就是 `in` 那一串）。根 `24384/9` 自己有一行几何，按 id 排在它名下
/// 6000 行子树行（`24384_1xxxxx`）的后面。头一页里两路各取各的：子树那一路取前 5000，`in = 根` 那一路取到根自己那一行；
/// 游标要是落到根那一行上，子树的第 5001–6000 行就再也读不到了。
#[tokio::test(flavor = "multi_thread")]
async fn coldread_paging_keeps_the_rows_ahead_of_the_roots_own_row() {
    let db = scratch_db("coldread_own_row").await;
    let root: RefnoEnum = "24384_9".into();
    let packed = root.refno().0;
    let subtree = DURABLE_READ_CHUNK + 1000;
    seed_inst_rows(&db, packed, "type::thing('pe', $k)", 100_000, subtree).await;
    db.query(format!(
        "CREATE inst_relate:⟨24384_9⟩ SET in = {owner}, anc = [{packed}],
           aabb = aabb:bench, world_trans = trans:bench, generic = 'EQUI', booled_id = '24384_9';",
        owner = root.to_pe_key()
    ))
    .await
    .unwrap()
    .check()
    .unwrap();

    let problems = paging_problems(&db, root, subtree + 1).await;
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 真库只读对拍（`--ignored`）：冷读翻页读回来的行数，对着库上直接数出来的准数。
///
/// 环境变量 `AIOS_COLDREAD_LIVE_ROOTS` = 一行一个根（`a/b` 或 `a_b`）的文件。逐根：先数准数（两张表各一句，两路作用域
/// 并起来只数 id、不分页），再整根读一遍、按窗口翻一遍。三个数必须相等，两种翻法读回来的记录序列化之后逐条相同。
/// 只发 `SELECT`。连的是 `DbOption.toml`（或 `DB_OPTION_FILE`）里那台库；连接参数不打印。
///
/// 另设 `AIOS_COLDREAD_LIVE_DUMP` = 一个目录时，把每根整根读回来的记录按 `/model/records` 的 `items` 同一个形状
/// 写成 `<目录>/<根>.json`——拿去和在跑的后端逐条对内容用（换读法前后各取一份）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the configured SurrealDB; read-only"]
async fn coldread_live_paging_matches_the_row_counts_on_the_database() {
    let Ok(roots_file) = std::env::var("AIOS_COLDREAD_LIVE_ROOTS") else {
        eprintln!("AIOS_COLDREAD_LIVE_ROOTS 没设，跳过");
        return;
    };
    let roots: Vec<RefnoEnum> = std::fs::read_to_string(&roots_file)
        .expect("根清单")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| RefnoEnum::from(line.replace('/', "_").as_str()))
        .collect();
    assert!(!roots.is_empty(), "根清单是空的");
    let db = super::model_read_pool::open_connection()
        .await
        .expect("连库");

    #[derive(serde::Deserialize)]
    struct Count {
        n: usize,
    }
    let mut wrong = Vec::new();
    for root in roots {
        let mut response = db
            .query(durable_count_sql(root.refno().0, &root.to_pe_key()))
            .await
            .expect("数准数")
            .check()
            .expect("数准数");
        let mut count = |index: usize| {
            response
                .take::<Vec<Count>>(index)
                .expect("准数")
                .first()
                .map_or(0, |row| row.n)
        };
        let (inst, tubi) = (count(0), count(1));
        let expected = inst + tubi;

        let started = Instant::now();
        let whole = durable_model_records_of_root(&db, root)
            .await
            .unwrap_or_else(|error| panic!("整根读 {root} 失败: {error:#}"));
        let whole_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let (by_window, finished) = read_by_windows(&db, root, expected).await;
        let window_ms = started.elapsed().as_millis();
        // 与 handler 同一条序列化路：先折成 `serde_json::Value`（`json!` 就是这么做的，单精度数在这一步放宽成双精度），
        // 再写成文本——直接把记录写成文本的话，单精度数按单精度的最短写法出来，和接口回的字面对不上。
        let whole_json = serde_json::to_value(&whole).unwrap().to_string();
        let same = whole_json == serde_json::to_value(&by_window).unwrap().to_string();
        if let Ok(dump) = std::env::var("AIOS_COLDREAD_LIVE_DUMP") {
            let file = std::path::Path::new(&dump).join(format!("{root}.json"));
            std::fs::write(&file, &whole_json).expect("写对拍用的记录");
        }
        let tubes = whole
            .iter()
            .filter(|record| record.inst.generic == "TUBI")
            .count();
        println!(
            "coldread-live root={root} expected={expected} (inst={inst} tubi={tubi}) whole={} ({whole_ms} ms) \
             window={} ({window_ms} ms, finished={finished}) tubes_read={tubes} same_sequence={same}",
            whole.len(),
            by_window.len()
        );
        if whole.len() != expected || by_window.len() != expected || !finished || !same {
            wrong.push(root.to_string());
        }
    }
    assert!(
        wrong.is_empty(),
        "这些根翻页读回来的与库上的准数对不上: {wrong:?}"
    );
}
