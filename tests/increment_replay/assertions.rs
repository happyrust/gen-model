use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::case::{Case, HealthExpect, OtherDbTaskExpect, Restart, Step};
use crate::harness::{Service, read_service_logs, repo};

fn room_nums(value: Option<&Value>) -> Option<BTreeSet<String>> {
    let items = value?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|item| item.get("room_num").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
    )
}

/// 一条模型记录的身份：refno + 隐式管身的 `tube{ordinal, from, to}`（spec §4.5 的四元组）；没有就退到整条的文本。
pub fn record_identity(item: &Value) -> String {
    let refno = item.get("refno").map(Value::to_string).unwrap_or_default();
    let tube = item
        .get("tube")
        .filter(|tube| !tube.is_null())
        .map(Value::to_string)
        .unwrap_or_default();
    if refno.is_empty() {
        item.to_string()
    } else {
        format!("{refno}|{tube}")
    }
}

/// 递归比两份 JSON：数字差超过 `tolerance` 才算不同，其余逐字比。回第一处不同的路径与两边的值。
/// 键 `rotation` 下的四元数不分符号，见 [`rotation_differs`]。
fn json_differs(a: &Value, b: &Value, tolerance: f64, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            ((x - y).abs() > tolerance || x.is_nan() != y.is_nan())
                .then(|| format!("{path}: {x} vs {y}"))
        }
        (Value::Array(xs), Value::Array(ys)) => {
            if xs.len() != ys.len() {
                return Some(format!("{path}: 长度 {} vs {}", xs.len(), ys.len()));
            }
            xs.iter()
                .zip(ys)
                .enumerate()
                .find_map(|(i, (x, y))| json_differs(x, y, tolerance, &format!("{path}[{i}]")))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            if xs.len() != ys.len() || xs.keys().any(|k| !ys.contains_key(k)) {
                return Some(format!(
                    "{path}: 键集 {:?} vs {:?}",
                    xs.keys().collect::<Vec<_>>(),
                    ys.keys().collect::<Vec<_>>()
                ));
            }
            xs.iter().find_map(|(k, x)| {
                let path = format!("{path}.{k}");
                match (k.as_str(), quaternion(x), quaternion(&ys[k])) {
                    ("rotation", Some(p), Some(q)) => rotation_differs(p, q, tolerance, &path),
                    _ => json_differs(x, &ys[k], tolerance, &path),
                }
            })
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

/// 四个数的数组（`[x, y, z, w]`）读成四元数；不是就 `None`，照普通数组逐个比。
fn quaternion(value: &Value) -> Option<[f64; 4]> {
    let items = value.as_array().filter(|items| items.len() == 4)?;
    let mut q = [0.0; 4];
    for (slot, item) in q.iter_mut().zip(items) {
        *slot = item.as_f64()?;
    }
    Some(q)
}

/// 四元数不分符号地比：`q` 与 `-q` 是同一个旋转，刚体前移左乘 Δ 与整根重算各自分解矩阵，取到的符号可以不同。
/// 取 `p - q` 与 `p + q` 里分量差小的那一边，不给单个四元数定符号：半圈附近（如 `Z is D` 的设备）`w` 只剩
/// 浮点噪声，按「第一个不可忽略的分量」定符号，两边会各取各的。
fn rotation_differs(p: [f64; 4], q: [f64; 4], tolerance: f64, path: &str) -> Option<String> {
    let gap = |sign: f64| {
        p.iter()
            .zip(&q)
            .map(|(a, b)| (a - sign * b).abs())
            .fold(0.0, f64::max)
    };
    let worst = gap(1.0).min(gap(-1.0));
    (worst > tolerance).then(|| format!("{path}: {p:?} vs {q:?}（不分符号，分量最大差 {worst}）"))
}

/// 比两份按身份排好序的记录（四元数不分符号，见 [`rotation_differs`]）。
pub fn records_differ(before: &[Value], after: &[Value], tolerance: f64) -> Option<String> {
    if before.len() != after.len() {
        return Some(format!("行数 {} vs {}", before.len(), after.len()));
    }
    before.iter().zip(after).find_map(|(x, y)| {
        let (ix, iy) = (record_identity(x), record_identity(y));
        if ix != iy {
            return Some(format!("身份序列不同：{ix} vs {iy}"));
        }
        json_differs(x, y, tolerance, &format!("[{ix}]"))
    })
}

/// 截到最多 `max` 个字符（报错里带一段任务 JSON，不要整个刷屏）。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// 把窗口规划那一行（`window_root_plan::plan_totals_line`）拆成「键 → 数」；不是那一行回 `None`。
///
/// 原文：`window S→T: candidates=N (+a ~b -c) rolled_up=… unchanged=… no_model=… (metadata_only=…
/// unread_attribute=… cache_stamp=…) unresolved=… cascades=… units regen=… remove=… derived=…`。
/// `+a ~b -c` 三格记成 `added` / `modified` / `deleted`，其余用它自己的名字。
///
/// 任务回执里这一行前面还拼着根一级的账：`dbnum=N: window S→T roots_T=… roots_S=… touched=…
/// regen=… delete=… advance=… lazy=… | window S→T: candidates=…`。两半都有 `regen`，数还不一样
/// （2026-11-12 实测删一个 BOX 的那一窗：根一级 `regen=1`，件一级 `units regen=0 remove=1`），
/// 所以根一级的四格改名记成 `root_regen` / `root_delete` / `root_advance` / `root_lazy`，
/// `roots_T` / `roots_S` / `touched` 用原名。
pub fn parse_plan_totals(line: &str) -> Option<BTreeMap<String, u64>> {
    let candidates = line.find("candidates=")?;
    let start = line[..candidates].rfind("window ")?;
    let mut totals = BTreeMap::new();
    for token in line[start..].split_whitespace() {
        let token = token.trim_matches(|ch| matches!(ch, '(' | ')' | ':' | ','));
        let (key, value) = if let Some((key, value)) = token.split_once('=') {
            (key, value)
        } else if let Some(value) = token.strip_prefix('+') {
            ("added", value)
        } else if let Some(value) = token.strip_prefix('~') {
            ("modified", value)
        } else if let Some(value) = token.strip_prefix('-') {
            ("deleted", value)
        } else {
            continue;
        };
        if let Ok(number) = value.parse::<u64>() {
            totals.insert(key.to_string(), number);
        }
    }
    let head = &line[..start];
    if let Some(roots) = head.find("window ") {
        for token in head[roots..].split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            let Ok(number) = value.parse::<u64>() else {
                continue;
            };
            let key = match key {
                "regen" | "delete" | "advance" | "lazy" => format!("root_{key}"),
                other => other.to_string(),
            };
            totals.insert(key, number);
        }
    }
    Some(totals)
}

/// 包围盒平移比对的容差：生成器浮点往返会在最后一位上抖（与 `unit_geometry_snapshot::MOVED_TOLERANCE_MM` 同量级）。
const AABB_TOLERANCE_MM: f32 = 1.0;

/// `geometry_before` / `geometry_after` 两格的 `aabb` 对出来的 min / max 平移；任一格缺席或没盒就 `None`。
fn aabb_shift_of(unit: &Value) -> Option<([f32; 3], [f32; 3])> {
    let aabb = |which: &str| -> Option<[f32; 6]> {
        let values = unit.pointer(&format!("/{which}/aabb"))?.as_array()?;
        let mut out = [0f32; 6];
        for (slot, value) in out.iter_mut().zip(values) {
            *slot = value.as_f64()? as f32;
        }
        (values.len() == 6).then_some(out)
    };
    let before = aabb("geometry_before")?;
    let after = aabb("geometry_after")?;
    Some((
        [
            after[0] - before[0],
            after[1] - before[1],
            after[2] - before[2],
        ],
        [
            after[3] - before[3],
            after[4] - before[4],
            after[5] - before[5],
        ],
    ))
}

fn within_tolerance(actual: [f32; 3], expected: [f32; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (a - e).abs() <= AABB_TOLERANCE_MM)
}

/// `24384/24776` / `24384_24776` → `(24384 << 32) | 24776`，与 `store_rows_under` 同一算法。
pub fn refno_u64(root: &str) -> u64 {
    let (word1, word2) = root
        .split_once('/')
        .or_else(|| root.split_once('_'))
        .unwrap_or_else(|| panic!("refno {root:?} 不是 a/b"));
    (word1.trim().parse::<u64>().expect("refno word1") << 32)
        | word2.trim().parse::<u64>().expect("refno word2")
}

/// INC-I01 / I02 / I04 / I05：`collect --check` 对基线。FAIL（含必需指标没样本）让这条用例红；
/// WARN 与只记不判的不挡。样本写在运行目录的 `budget.json`。
pub fn check_increment_budget(service: &mut Service, run_dir: &Path, case_name: &str) {
    let _ = service.child.kill();
    let _ = service.child.wait();
    let script = repo().join("scripts/increment_budget.py");
    let out = run_dir.join("budget.json");
    let output = Command::new("python")
        .arg(&script)
        .arg("collect")
        .arg(run_dir)
        .arg("--case")
        .arg(case_name)
        .arg("--out")
        .arg(&out)
        .arg("--check")
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "[{case_name}] 跑不了 python {}：{error}。预算门要本机 PATH 里有 python",
                script.display()
            )
        });
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "[{case_name}] 预算门没过（exit {:?}）：\n{text}样本 {}",
        output.status.code(),
        out.display()
    );
    eprint!("{text}");
}

/// INC-J01 / J02：链已经追平之后看 `/health` 的监听范围和启动复核，再 `POST /update/execute` 只勾选本库。
/// 非设计库不受勾选名单约束，回执里可能有它们的入队；这里不判 `enqueued` 为空。
/// `catalogue_reconcile` 是另起的任务，不等它落定，只要求这一格在。
pub fn assert_operator_surface(service: &Service, case: &Case) {
    let health = service
        .get("health")
        .unwrap_or_else(|| panic!("GET /health 没有 JSON"));
    let watched_nums: Vec<u64> = health
        .get("watch_dbnums")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    assert!(
        watched_nums.contains(&(case.dbnum as u64)),
        "INC-J02 /health.watch_dbnums={watched_nums:?}，期望含 {}",
        case.dbnum
    );
    let origin = health
        .get("watch_dbnums_origin")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        origin.contains("watch_dbnums"),
        "INC-J02 watch_dbnums_origin={origin:?}，期望点名 watch_dbnums"
    );
    let behind = health.pointer("/initialization/startup/reconcile/catalogue_behind_out_of_scope");
    assert!(
        behind.and_then(Value::as_array).is_some(),
        "INC-J02 启动复核没有 catalogue_behind_out_of_scope 数组：initialization={}",
        health.get("initialization").unwrap_or(&Value::Null)
    );
    assert!(
        health.get("catalogue_reconcile").is_some(),
        "INC-J02 /health 没有 catalogue_reconcile 这一格"
    );
    let reconcile_state = match health.get("catalogue_reconcile") {
        Some(value) if value.is_null() => "null",
        Some(_) => "有",
        None => "缺",
    };
    eprintln!(
        "  [health] watch_dbnums={watched_nums:?} origin={origin} 域外落后目录={behind:?} catalogue_reconcile={reconcile_state}"
    );

    let receipt = service.post(
        "update/execute",
        serde_json::json!({ "dbnums": [case.dbnum] }),
    );
    let warning_text: Vec<String> = receipt
        .get("warnings")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        warning_text
            .iter()
            .any(|line| line.contains("目录版本核对已随本次执行另起任务")),
        "INC-J01 warnings 里没有目录核对句：{warning_text:?}"
    );
    assert!(
        warning_text.iter().any(|line| line.contains("监听限定")),
        "INC-J01 warnings 里没有监听限定声明：{warning_text:?}"
    );
    let unselected_nums: Vec<u64> = receipt
        .get("unselected")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    assert!(
        receipt
            .get("unselected")
            .and_then(Value::as_array)
            .is_some(),
        "INC-J01 回执缺 unselected 数组：scanned={:?} up_to_date={:?}",
        receipt.get("scanned"),
        receipt.get("up_to_date")
    );
    assert!(
        !unselected_nums.contains(&(case.dbnum as u64)),
        "INC-J01 勾选了 {} 却出现在 unselected：{unselected_nums:?}",
        case.dbnum
    );
    // 「非空」不能无条件断言：`subset_selects` 对非 DESI（SYST / DICT / CATA …）永远放行，
    // 而影子工程根里目前只有 8000 一个 DESI（2026-10-27 实测 scanned=18 / up_to_date=18 /
    // unselected=[]），所以这里只判「回执里这一格在、且不把勾选的库算成未勾选」。
    // 等 INC-H03 把第二个 dbnum 的夹具做出来，再把 `expect_unselected_non_empty` 打开。
    if case.expect_unselected_non_empty {
        assert!(
            !unselected_nums.is_empty(),
            "INC-J01 unselected 是空的：工程里应有别的 DESI 没被勾选（scanned={:?} up_to_date={:?} enqueued 长度 {:?} ）",
            receipt.get("scanned"),
            receipt.get("up_to_date"),
            receipt
                .get("enqueued")
                .and_then(Value::as_array)
                .map(|items| items.len())
        );
    }
    for key in ["scanned", "enqueued", "up_to_date", "project", "warnings"] {
        assert!(receipt.get(key).is_some(), "INC-J01 回执缺 {key}");
    }
    eprintln!(
        "  [execute] scanned={:?} up_to_date={:?} enqueued={} unselected={unselected_nums:?}",
        receipt.get("scanned"),
        receipt.get("up_to_date"),
        receipt
            .get("enqueued")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0),
    );
}

/// INC-G02 的库上半边（O2）：这些根按边读到的房间号必须含 `present`、不含 `absent`。
/// 与 `rooms_changed` 分工：那个比回执（O3，只在回执被拍对的那一版之后才有意义），这个比库上的边。
pub fn assert_room_edges(service: &Service, step: &Step, what: &str) {
    for (root, expect) in &step.room_edges {
        let rooms = service.store_rooms_of(root);
        let mut problems = Vec::new();
        for room in &expect.present {
            if !rooms.contains(room) {
                problems.push(format!("少了 {room}"));
            }
        }
        for room in &expect.absent {
            if rooms.contains(room) {
                problems.push(format!("还留着 {room}"));
            }
        }
        assert!(
            problems.is_empty(),
            "[{what}] 根 {root} 库上 room_relate 边对不上：{}（实读 {rooms:?}）",
            problems.join("、")
        );
        eprintln!(
            "  [{what}] 根 {root} 库上房间边 {rooms:?}（present {:?} / absent {:?} 都对上）",
            expect.present, expect.absent
        );
    }
}

/// 增量结果 == 同数据整库全量重建结果，两种口径共用一次重建：
/// - INC-G01（RI-12）：`rebuild_edges_equal` 的根名下房间边集合逐边相等（O2），不比条数；
/// - 刚体前移（ADR-066）：`rebuild_records_equal` 的根 `/model/records` 逐条相等（数字容差 `rebuild_records_tolerance`）。
/// 全量重建是 `POST /api/v1/dbnums/{dbnum}/model/rebuild` 排的一条起点 0 的 `ModelOnly` 任务（ADR-079），
/// 不改水位、不删模型，所以这条断言可以在链尾安全地打一枪。
pub fn assert_rebuild_parity(service: &mut Service, case: &Case, what: &str) {
    let mut records_before = BTreeMap::new();
    for root in &case.rebuild_records_equal {
        let (source, items) = service
            .records_items(root)
            .unwrap_or_else(|| panic!("[{what}] 根 {root} 不在索引里，没法对拍"));
        assert!(
            !items.is_empty(),
            "[{what}] 根 {root} 重建前一行都没有（source={source}）：先让它真生成过（generate_prime / records_present）"
        );
        eprintln!(
            "  [{what}] 重建前根 {root}：{} 行（source={source}）",
            items.len()
        );
        records_before.insert(root.clone(), items);
    }
    let mut before = BTreeMap::new();
    for root in &case.rebuild_edges_equal {
        let edges = service.store_room_edges_under(root);
        assert!(
            !edges.is_empty(),
            "[{what}] 根 {root} 名下一条 room_relate 边都没有：增量那半本该写出边，先看根名 / 房间段是否真的跑过"
        );
        eprintln!("  [{what}] 重建前根 {root}：{} 条房间边", edges.len());
        before.insert(root.clone(), edges);
    }
    let receipt = service.post(
        &format!("dbnums/{}/model/rebuild", case.dbnum),
        serde_json::json!({}),
    );
    let task_id = receipt
        .get("task_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        !task_id.is_empty(),
        "[{what}] model/rebuild 没回 task_id：{receipt}"
    );
    eprintln!(
        "  [{what}] 整库重建任务 {task_id}（expected_roots={:?} state={:?}）",
        receipt.get("expected_roots"),
        receipt.get("state")
    );
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        service.assert_alive(what);
        let task = service
            .get(&format!("tasks/{task_id}"))
            .unwrap_or_else(|| panic!("[{what}] /tasks/{task_id} 没有 JSON"));
        let state = task.get("state").and_then(Value::as_str).unwrap_or("");
        if matches!(state, "succeeded" | "failed" | "cancelled" | "partial") {
            assert_eq!(
                state, "succeeded",
                "[{what}] 整库重建任务 {task_id} 落了 {state}：{task}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "[{what}] 900 s 内整库重建任务 {task_id} 没落终态（state={state}）；日志 {}",
            service.stdout_path.display()
        );
        std::thread::sleep(Duration::from_millis(1000));
    }
    for (root, before_edges) in &before {
        let after = service.store_room_edges_under(root);
        let missing: Vec<_> = before_edges.difference(&after).cloned().collect();
        let extra: Vec<_> = after.difference(before_edges).cloned().collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "[{what}] 根 {root} 的增量边集合 != 同数据全量重建边集合（RI-12）：\n\
             只在增量有 {missing:?}\n  只在重建有 {extra:?}"
        );
        eprintln!(
            "  [{what}] 根 {root}：增量后与全量重建后都是 {} 条房间边（逐边相等，RI-12）",
            after.len()
        );
    }
    let mut problems = Vec::new();
    for (root, before_items) in &records_before {
        match service.records_items(root) {
            Some((source, after)) => {
                match records_differ(before_items, &after, case.rebuild_records_tolerance) {
                    Some(diff) => problems.push(format!(
                        "根 {root} 增量后的行与整库重建后的不同（source={source}）：{diff}"
                    )),
                    None => eprintln!(
                        "  [{what}] 根 {root}：增量后与整库重建后 {} 行逐条相等（容差 {}，source={source}）",
                        after.len(),
                        case.rebuild_records_tolerance
                    ),
                }
            }
            None => problems.push(format!("根 {root} 整库重建后不在索引里")),
        }
    }
    assert!(
        problems.is_empty(),
        "[{what}] 增量 vs 整库重建的记录对拍不过：\n{}\n日志 {}",
        problems.join("\n"),
        service.stdout_path.display()
    );
}

/// 查询值里除了未保留字符都按百分号写进 URL。`created_at` 的 `+` 要是原样拼进去，会被读成空格。
fn encode_query_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn task_page(service: &Service, query: &str, what: &str) -> Vec<Value> {
    let page = service
        .get(query)
        .unwrap_or_else(|| panic!("[{what}] GET /api/v1/{query} 没有 JSON"));
    page.get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("[{what}] {query} 回的不是任务页：{page}"))
}

/// INC-F03：这一窗的数据批次在（可能的）冷重启之后仍在 `/tasks` 里，`?before=` 往回翻不含上界，
/// 变化清单每条都有 `op` / `refno`，并且至少一条折进 `history_root`。
pub fn assert_task_history(service: &Service, case: &Case, step: &Step, what: &str) {
    let root = step.history_root.as_deref().unwrap_or("");
    let rows = task_page(service, "tasks?limit=160&kind=data_batch", what);
    let task = rows
        .iter()
        .find(|row| {
            row.get("dbnum").and_then(Value::as_u64) == Some(case.dbnum as u64)
                && row.get("end_sesno").and_then(Value::as_u64) == Some(step.to as u64)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "[{what}] /tasks?kind=data_batch 里找不到 dbnum {} 右端 {} 的批次（共 {} 条）",
                case.dbnum,
                step.to,
                rows.len()
            )
        });
    let task_id = task
        .get("task_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let created_at = task
        .get("created_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        !task_id.is_empty() && !created_at.is_empty(),
        "[{what}] 任务行缺 task_id / created_at：{task}"
    );
    let older = task_page(
        service,
        &format!(
            "tasks?limit=160&kind=data_batch&before={}",
            encode_query_component(&created_at)
        ),
        what,
    );
    assert!(
        older
            .iter()
            .all(|row| row.get("task_id").and_then(Value::as_str) != Some(task_id.as_str())),
        "[{what}] ?before= 仍然含上界任务 {task_id}"
    );
    assert!(
        older.iter().any(|row| {
            row.get("created_at")
                .and_then(Value::as_str)
                .is_some_and(|at| at < created_at.as_str())
        }),
        "[{what}] ?before={created_at} 没有翻出更早的数据批次"
    );
    let changes = service
        .get(&format!("tasks/{task_id}/changes?limit=1000"))
        .unwrap_or_else(|| panic!("[{what}] /tasks/{task_id}/changes 没有 JSON"));
    let items = changes
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("[{what}] 变化清单不可用：{changes}"));
    let total = changes.get("total").and_then(Value::as_u64).unwrap_or(0);
    assert!(
        changes.get("end_sesno").and_then(Value::as_u64) == Some(step.to as u64),
        "[{what}] 变化清单右端不是 {}：{changes}",
        step.to
    );
    assert!(
        total >= 1
            && total == items.len() as u64
            && changes.get("truncated").and_then(Value::as_bool) != Some(true),
        "[{what}] 变化清单条数对不上：total={total} items={} truncated={:?}",
        items.len(),
        changes.get("truncated")
    );
    let mut rooted = false;
    for item in &items {
        let op = item.get("op").and_then(Value::as_str).unwrap_or("");
        let refno = item.get("refno").and_then(Value::as_str).unwrap_or("");
        assert!(
            !op.is_empty() && !refno.is_empty(),
            "[{what}] 变化条目缺 op / refno：{item}"
        );
        if item.get("root_refno").and_then(Value::as_str) == Some(root) {
            rooted = true;
        }
    }
    assert!(rooted, "[{what}] 变化清单里没有折进根 {root} 的条目");
    eprintln!(
        "  [{what}] 任务 {task_id} 重启后仍在，?before= 翻出 {} 条更早的，变化清单 {total} 条含根 {root}",
        older.len()
    );
}

/// `records_equal_to_step`：当前库上每一根的行与早先在 `at` 存下的行相等（数字容差）。
pub fn compare_with_stash(
    service: &Service,
    stash: &BTreeMap<(u32, String), Vec<Value>>,
    at: u32,
    tolerance: f64,
    what: &str,
) {
    let stashed: Vec<_> = stash
        .iter()
        .filter(|((sesno, _), _)| *sesno == at)
        .collect();
    assert!(
        !stashed.is_empty(),
        "[{what}] 没有在 sesno {at} 存过任何根的行（前面的步要带 stash_records）"
    );
    let mut problems = Vec::new();
    for ((_, root), before) in stashed {
        match service.records_items(root) {
            Some((source, now)) => match records_differ(before, &now, tolerance) {
                Some(diff) => problems.push(format!(
                    "根 {root} 现在的行（source={source}）与 sesno {at} 时存下的不同：{diff}"
                )),
                None => eprintln!(
                    "  [{what}] 根 {root} {} 行与 sesno {at} 时相等（source={source}）",
                    now.len()
                ),
            },
            None => problems.push(format!("根 {root} 不在索引里，却在 sesno {at} 时有行")),
        }
    }
    assert!(
        problems.is_empty(),
        "[{what}] 与 sesno {at} 对拍不过：\n{}\n日志 {}",
        problems.join("\n"),
        service.stdout_path.display()
    );
}

/// INC-F02：重启前记 `/model/records`，冷重启，水位不动、库上行与重启前逐字段相等；再把根叫回投影，重算后仍相等。
pub fn restart_parity(service: &mut Service, case: &Case, to: u32, restart: &Restart, what: &str) {
    let mut before = BTreeMap::new();
    for root in &restart.records_equal {
        let (source, items) = service
            .records_items(root)
            .unwrap_or_else(|| panic!("[{what}] 根 {root} 重启前已不在索引里"));
        eprintln!(
            "  [{what}] 重启前根 {root}：{} 行（source={source}）",
            items.len()
        );
        before.insert(root.clone(), items);
    }

    service.restart(what);
    // 水位在库里：重启后立刻还是 to，不许重新初始化。
    service.wait_in_sync(case.dbnum, to, Duration::from_secs(120), what);
    let mut problems = Vec::new();
    let mut log = Vec::new();
    for root in &restart.records_equal {
        let Some((source, after)) = service.records_items(root) else {
            problems.push(format!("根 {root} 重启后不在索引里"));
            continue;
        };
        if let Some(diff) = records_differ(&before[root], &after, restart.tolerance) {
            problems.push(format!(
                "根 {root} 重启后库上行（source={source}）与重启前不同：{diff}"
            ));
        } else {
            log.push(format!(
                "{root} 冷读 {} 行相等（source={source}）",
                after.len()
            ));
        }
        // 叫回投影：首次 Generated（重算），第二次 AlreadyAvailable；重算后的行与重启前在容差内相等。
        let first = service.post("model/generate", serde_json::json!({ "refno": root }));
        let first_status = first.get("status").and_then(Value::as_str).unwrap_or("");
        let second = service.post("model/generate", serde_json::json!({ "refno": root }));
        let second_status = second.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(first_status, "Generated" | "AlreadyAvailable")
            || second_status != "AlreadyAvailable"
        {
            problems.push(format!(
                "根 {root} 重启后 /model/generate → {first_status} / {second_status}，期望 Generated|AlreadyAvailable / AlreadyAvailable"
            ));
        }
        match service.records_items(root) {
            Some((source, reprimed)) => {
                if let Some(diff) =
                    records_differ(&before[root], &reprimed, restart.reprime_tolerance)
                {
                    problems.push(format!(
                        "根 {root} 重启 + 补常驻重算后（source={source}）与重启前不同：{diff}"
                    ));
                } else {
                    log.push(format!(
                        "{root} 补常驻（{first_status} / {second_status}）后 {} 行相等（source={source}）",
                        reprimed.len()
                    ));
                }
            }
            None => problems.push(format!("根 {root} 补常驻后不在索引里")),
        }
    }
    for line in &log {
        eprintln!("  [{what}] {line}");
    }
    assert!(
        problems.is_empty(),
        "[{what}] 重启对拍不过：\n{}\n日志 {}",
        problems.join("\n"),
        service.stdout_path.display()
    );
    eprintln!("  [{what}] 重启对拍全过");
}

/// 一步（或初始化态）的断言：任务终态与单元 kind（O3）、库上行有无（O2：HTTP + 直查 surreal）、未触根 `AlreadyAvailable`。
/// `/health` 里的一格满不满足期望；不满足时回「现在是什么、期望什么」。
fn health_unmet(health: &Value, expect: &HealthExpect) -> Option<String> {
    if expect.present && expect.absent {
        return Some(format!(
            "/health 的 {} 同时写了 present 和 absent，用例写错了",
            expect.path
        ));
    }
    let actual = health
        .pointer(&expect.path)
        .filter(|value| !value.is_null());
    let (ok, wanted) = if expect.absent {
        (actual.is_none(), "没有这一格".to_string())
    } else if let Some(want) = &expect.eq {
        let same = match (actual.and_then(Value::as_f64), want.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-9,
            _ => actual == Some(want),
        };
        (same, format!("等于 {want}"))
    } else if let Some(floor) = expect.ge {
        (
            actual
                .and_then(Value::as_f64)
                .is_some_and(|value| value >= floor),
            format!("不小于 {floor}"),
        )
    } else {
        (actual.is_some(), "有这一格".to_string())
    };
    (!ok).then(|| {
        format!(
            "/health 的 {} 现在是 {}，期望{wanted}",
            expect.path,
            actual.map_or_else(|| "（没有）".to_string(), Value::to_string)
        )
    })
}

/// 「后端自己的状态」那两样断言：`/health` 里的格子（轮询到全部满足）、这一步开始之后必须出现的日志。
/// 都在这一步的时限内等。主库不动的步（`hold_primary`）只跑这两样。
/// 一条任务的一句话：任务号、种类、终态、意图、会话区间、交付单元的根。
fn task_brief(task: &Value) -> String {
    let text = |pointer: &str| {
        task.pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    };
    let roots: Vec<String> = task
        .pointer("/result/units")
        .and_then(Value::as_array)
        .map(|units| {
            units
                .iter()
                .filter_map(|unit| unit.get("root_refno").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    format!(
        "{} kind={} state={} intent={} 区间 {:?}..={:?} 单元 {:?}",
        text("/task_id"),
        text("/kind"),
        text("/state"),
        text("/detail/intent"),
        task.get("start_sesno").and_then(Value::as_i64),
        task.get("end_sesno").and_then(Value::as_i64),
        roots
    )
}

fn shift_close(actual: [f32; 3], expected: [f32; 3], tolerance: f32) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (a - e).abs() <= tolerance)
}

/// 步上的 `companion_tasks`：主库这一窗做完之后，别的库上该多出来（或不该多出来）的任务。
///
/// `before` 是推文件之前各库已有的任务号。要等的是一条**新的**、已落终态的任务；写了 `intent_contains`
/// 就只认意图里含它的。等到之后把它的每个交付单元打进日志（写期望的人从这里抄数），再按格子比。
pub fn assert_companion_tasks(
    service: &mut Service,
    expectations: &[OtherDbTaskExpect],
    before: &BTreeMap<u32, BTreeSet<String>>,
    stash: &BTreeMap<(u32, String), Vec<Value>>,
    what: &str,
) {
    for expect in expectations {
        let tag = format!("{what} 库 {} 上的新任务", expect.dbnum);
        let known = before.get(&expect.dbnum).cloned().unwrap_or_default();
        let wanted_intent = expect.intent_contains.as_deref().map(str::to_lowercase);
        let intent_matches = |task: &Value| match &wanted_intent {
            Some(needle) => task
                .pointer("/detail/intent")
                .and_then(Value::as_str)
                .is_some_and(|intent| intent.to_lowercase().contains(needle)),
            None => true,
        };
        let started = Instant::now();
        if expect.absent {
            let settle = Duration::from_secs(expect.timeout_secs.unwrap_or(15));
            loop {
                service.assert_alive(&tag);
                let fresh: Vec<Value> = service
                    .new_tasks(expect.dbnum, &known)
                    .into_iter()
                    .filter(|task| intent_matches(task))
                    .collect();
                assert!(
                    fresh.is_empty(),
                    "[{tag}] 不该有，却多出来了：{:?}；日志 {}",
                    fresh.iter().map(task_brief).collect::<Vec<_>>(),
                    service.stdout_path.display()
                );
                if started.elapsed() >= settle {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
            eprintln!(
                "  [{tag}] 看了 {} 秒，没有多出任务（对照）",
                settle.as_secs()
            );
            continue;
        }
        let timeout = Duration::from_secs(expect.timeout_secs.unwrap_or(180));
        let task = loop {
            service.assert_alive(&tag);
            let fresh = service.new_tasks(expect.dbnum, &known);
            let done = fresh.iter().rev().find(|task| {
                intent_matches(task)
                    && matches!(
                        task.get("state").and_then(Value::as_str),
                        Some("succeeded" | "failed" | "partial")
                    )
            });
            if let Some(task) = done {
                break task.clone();
            }
            assert!(
                started.elapsed() < timeout,
                "[{tag}] {timeout:?} 内没有等到一条落终态的新任务{}；这个库上新出现的：{:?}；日志 {}",
                wanted_intent
                    .as_ref()
                    .map(|needle| format!("（意图含 {needle:?}）"))
                    .unwrap_or_default(),
                fresh.iter().map(task_brief).collect::<Vec<_>>(),
                service.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(1000));
        };
        eprintln!(
            "  [{tag}] {}（等了 {:.1}s）",
            task_brief(&task),
            started.elapsed().as_secs_f64()
        );
        let units = task
            .pointer("/result/units")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for unit in &units {
            let shift = aabb_shift_of(unit)
                .map(|(mins, maxs)| format!(" 包围盒 min 位移 {mins:?} max 位移 {maxs:?}"))
                .unwrap_or_default();
            eprintln!(
                "  [{tag}] 单元 {} kind={} 几何差 moved/added/removed={}/{}/{} 行数 {}→{}{shift}",
                unit.get("root_refno")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                unit.get("kind").and_then(Value::as_str).unwrap_or("regen"),
                unit.pointer("/geometry_diff/moved").unwrap_or(&Value::Null),
                unit.pointer("/geometry_diff/added").unwrap_or(&Value::Null),
                unit.pointer("/geometry_diff/removed")
                    .unwrap_or(&Value::Null),
                unit.pointer("/geometry_before/records")
                    .unwrap_or(&Value::Null),
                unit.pointer("/geometry_after/records")
                    .unwrap_or(&Value::Null),
            );
        }
        let normal = |root: &str| root.trim().trim_start_matches('=').replace('_', "/");
        let unit_of = |root: &str| {
            let root = normal(root);
            units.iter().find(|unit| {
                unit.get("root_refno")
                    .and_then(Value::as_str)
                    .is_some_and(|have| normal(have) == root)
            })
        };
        let mut problems = Vec::new();
        let state = task.get("state").and_then(Value::as_str).unwrap_or("");
        let expected_state = expect.task_state.as_deref().unwrap_or("succeeded");
        if state != expected_state {
            problems.push(format!("任务 state={state}，期望 {expected_state}"));
        }
        if let Some(expected) = &expect.units_exactly {
            let have: BTreeSet<String> = units
                .iter()
                .filter_map(|unit| unit.get("root_refno").and_then(Value::as_str))
                .map(normal)
                .collect();
            let want: BTreeSet<String> = expected.iter().map(|root| normal(root)).collect();
            if have != want {
                problems.push(format!(
                    "交付单元的根对不上：缺 {:?}，多 {:?}",
                    want.difference(&have).collect::<Vec<_>>(),
                    have.difference(&want).collect::<Vec<_>>()
                ));
            }
        }
        for (root, kind) in &expect.unit_kinds {
            match unit_of(root) {
                Some(unit) => {
                    let actual = unit.get("kind").and_then(Value::as_str).unwrap_or("regen");
                    if actual != kind {
                        problems.push(format!("根 {root} kind={actual}，期望 {kind}"));
                    }
                }
                None => problems.push(format!("根 {root} 不在这条任务的单元里")),
            }
        }
        let tolerance = expect.aabb_tolerance.unwrap_or(AABB_TOLERANCE_MM);
        let shifts = [
            ("aabb_shift", &expect.aabb_shift, true, true),
            ("aabb_min_shift", &expect.aabb_min_shift, true, false),
            ("aabb_max_shift", &expect.aabb_max_shift, false, true),
        ];
        for (label, table, check_min, check_max) in shifts {
            for (root, expected) in table {
                let Some(unit) = unit_of(root) else {
                    problems.push(format!("根 {root} 不在这条任务的单元里（{label}）"));
                    continue;
                };
                match aabb_shift_of(unit) {
                    Some((mins, maxs)) => {
                        if (check_min && !shift_close(mins, *expected, tolerance))
                            || (check_max && !shift_close(maxs, *expected, tolerance))
                        {
                            problems.push(format!(
                                "根 {root} {label}：min 位移 {mins:?}、max 位移 {maxs:?}，期望 {expected:?}（容差 {tolerance} mm）"
                            ));
                        }
                    }
                    None => problems.push(format!(
                        "根 {root} 的回执里没有前后两只包围盒（{label}）：{:?} / {:?}",
                        unit.get("geometry_before"),
                        unit.get("geometry_after")
                    )),
                }
            }
        }
        for (root, expected) in &expect.geometry_counts {
            let Some(unit) = unit_of(root) else {
                problems.push(format!("根 {root} 不在这条任务的单元里（几何计数）"));
                continue;
            };
            let checks = [
                (
                    "geometry_diff.moved",
                    "/geometry_diff/moved",
                    expected.moved,
                ),
                (
                    "geometry_diff.added",
                    "/geometry_diff/added",
                    expected.added,
                ),
                (
                    "geometry_diff.removed",
                    "/geometry_diff/removed",
                    expected.removed,
                ),
                (
                    "geometry_before.records",
                    "/geometry_before/records",
                    expected.records_before,
                ),
                (
                    "geometry_after.records",
                    "/geometry_after/records",
                    expected.records_after,
                ),
            ];
            for (label, pointer, want) in checks {
                let Some(want) = want else {
                    continue;
                };
                let actual = unit.pointer(pointer).and_then(Value::as_u64);
                if actual != Some(want) {
                    problems.push(format!("根 {root} {label}={actual:?}，期望 {want}"));
                }
            }
        }
        let warnings_text = task
            .pointer("/result/warnings")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        for needle in &expect.warnings_contain {
            if !warnings_text.contains(needle.as_str()) {
                problems.push(format!(
                    "任务回执的 warnings 里没有 {needle:?}：{}",
                    truncate(&warnings_text, 600)
                ));
            }
        }
        for needle in &expect.warnings_absent {
            if warnings_text.contains(needle.as_str()) {
                problems.push(format!("任务回执的 warnings 里不该有 {needle:?}"));
            }
        }
        assert!(
            problems.is_empty(),
            "[{tag}] 断言不过（{}）：\n{}\n日志 {}",
            task_brief(&task),
            problems.join("\n"),
            service.stdout_path.display()
        );
        eprintln!("  [{tag}] 断言全过");
        if let Some(at) = expect.records_equal_to_step {
            compare_with_stash(
                service,
                stash,
                at,
                expect.records_equal_tolerance.unwrap_or(0.5),
                &tag,
            );
        }
    }
}

pub fn health_and_log_problems(service: &mut Service, step: &Step, what: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let timeout = Duration::from_secs(step.timeout_secs.unwrap_or(180));
    if !step.expect_health.is_empty() {
        let deadline = Instant::now() + timeout;
        loop {
            service.assert_alive(what);
            let health = service.get("health").unwrap_or(Value::Null);
            let unmet: Vec<String> = step
                .expect_health
                .iter()
                .filter_map(|expect| health_unmet(&health, expect))
                .collect();
            if unmet.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                problems.extend(
                    unmet
                        .into_iter()
                        .map(|line| format!("{timeout:?} 内没等到：{line}")),
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    for needle in &step.log_must_contain {
        match service.wait_for_log_after_mark(needle, timeout, what) {
            Some(line) => eprintln!("  [{what}] 日志里有：{}", truncate(&line, 200)),
            None => problems.push(format!(
                "{timeout:?} 内这一步开始之后的日志里没出现「{needle}」"
            )),
        }
    }
    problems
}

pub fn assert_step(service: &mut Service, case: &Case, step: &Step, what: &str) {
    {
        let mut problems = health_and_log_problems(service, step, what);
        // 目录版本核对（INC-E02 / E03）先等：它们等的是「下一拍对账」，与这一窗的数据水位无关。
        let catalogue_timeout = Duration::from_secs(step.timeout_secs.unwrap_or(300));
        // INC-E03（GitHub #67）：域外落后库在周期拍上**只点名、不出声、不排工**。
        if !step.expect_catalogue_out_of_scope_behind.is_empty() {
            let snapshot = service.wait_catalogue_out_of_scope_behind(
                &step.expect_catalogue_out_of_scope_behind,
                catalogue_timeout,
                what,
            );
            let stale = snapshot
                .get("stale_catalogue_dbnums")
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            assert_eq!(
                stale, 0,
                "[{what}] 域外的库不该进点名 → 收窗 → 反查：stale_catalogue_dbnums 应空，实际 {snapshot:?}"
            );
            let planned = snapshot
                .pointer("/catalogue_cascade/roots")
                .and_then(Value::as_object)
                .map(|roots| roots.len())
                .unwrap_or(0);
            assert_eq!(
                planned, 0,
                "[{what}] 域外落后库本进程不排工，反查却排了 {planned} 个库的根：{snapshot:?}"
            );
            let log = fs::read_to_string(&service.stdout_path).unwrap_or_default();
            let noisy = log
                .lines()
                .filter(|line| line.contains("但在监听限定（"))
                .collect::<Vec<_>>();
            assert!(
                noisy.is_empty(),
                "[{what}] 域外落后库这一拍不该出声（周期拍没有别的过期库），日志里却有：{noisy:?}"
            );
            eprintln!(
                "  [{what}] 域外落后库 {:?} 只在 /health 点名（trigger={:?}，stale 空、没排工），日志里没有那句横幅",
                step.expect_catalogue_out_of_scope_behind,
                snapshot.get("trigger")
            );
        }
        // INC-E02（ADR-079）：域内落后、却一根都没被命中的库 → 一条只盖落款的空任务，盖完之后下一拍安静。
        if !step.expect_catalogue_stamp_only.is_empty() {
            let snapshot = service.wait_catalogue_stamp_only(
                &step.expect_catalogue_stamp_only,
                catalogue_timeout,
                what,
            );
            eprintln!(
                "  [{what}] 目录版本核对（trigger={:?}）排了只盖落款的空任务 {:?}，stale={:?}",
                snapshot.get("trigger"),
                step.expect_catalogue_stamp_only,
                snapshot.get("stale_catalogue_dbnums")
            );
            for dbnum in &step.expect_catalogue_stamp_only {
                let rows = service.store_rows(&format!(
                    "SELECT model_catalogue_sessions FROM dbnum_watermark:{dbnum};"
                ));
                let stamps = rows
                    .first()
                    .and_then(|row| row.get("model_catalogue_sessions"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let sessions = stamps
                    .iter()
                    .filter_map(|stamp| stamp.get("session").and_then(Value::as_u64))
                    .collect::<Vec<_>>();
                assert!(
                    !sessions.is_empty() && sessions.iter().all(|session| *session > 0),
                    "[{what}] dbnum {dbnum} 的空任务跑完该把落款盖成当前会话，现在读到的还是 {stamps:?}"
                );
                eprintln!("  [{what}] dbnum {dbnum} 落款已盖上：{stamps:?}");
            }
            let at = snapshot.get("at").and_then(Value::as_str).unwrap_or("");
            service.wait_catalogue_quiet(at, catalogue_timeout, what);
        }
        // 赛跑步：单元 / 几何 / 房间断言落在追上来的后继窗（右端 next）上；被超越的那一窗另有断言。
        let end = step.race.as_ref().map_or(step.to, |race| race.next);
        if let Some(race) = &step.race {
            let refused = service.latest_task(case.dbnum, step.to).unwrap_or_else(|| {
                panic!(
                    "[{what}] /tasks 里找不到被追上的那一窗（dbnum {} 右端 {}）",
                    case.dbnum, step.to
                )
            });
            let follow_up = service
                .latest_task(case.dbnum, race.next)
                .unwrap_or_else(|| {
                    panic!(
                        "[{what}] /tasks 里找不到后继窗（dbnum {} 右端 {}）",
                        case.dbnum, race.next
                    )
                });
            let refused_id = refused.get("task_id").and_then(Value::as_str).unwrap_or("");
            let follow_up_id = follow_up
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let refused_state = refused.get("state").and_then(Value::as_str).unwrap_or("");
            if refused_state != "failed" {
                problems.push(format!(
                    "被追上的窗 {refused_id}（右端 {}）state={refused_state}，期望 failed",
                    step.to
                ));
            }
            let refused_text = refused.to_string();
            if !refused_text.contains(race.message_contains.as_str()) {
                problems.push(format!(
                    "被追上的窗 {refused_id} 里没有「{}」：{}",
                    race.message_contains,
                    truncate(&refused_text, 600)
                ));
            }
            let superseded_by = refused.get("superseded_by").and_then(Value::as_str);
            if superseded_by != Some(follow_up_id) {
                problems.push(format!(
                    "被追上的窗 {refused_id}.superseded_by={superseded_by:?}，期望 {follow_up_id}"
                ));
            }
            let supersedes = follow_up
                .get("supersedes")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !supersedes.iter().any(|id| id == refused_id) {
                problems.push(format!(
                    "后继窗 {follow_up_id}.supersedes={supersedes:?}，期望含 {refused_id}"
                ));
            }
            let start = follow_up.get("start_sesno").and_then(Value::as_u64);
            if start != Some(step.to as u64) {
                problems.push(format!(
                    "后继窗 {follow_up_id} 应从 {} 重覆盖（start_sesno），实际 {start:?}",
                    step.to
                ));
            }
            eprintln!(
                "  [race] {refused_id}（{}..={}）{refused_state} → superseded_by {follow_up_id}（{start:?}..={}）",
                refused
                    .get("start_sesno")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                step.to,
                race.next
            );
        }
        // 任务终态与单元 kind / 几何差 / 房间前后。
        if !step.unit_kinds.is_empty()
            || step.task_state.is_some()
            || step.unit_count.is_some()
            || step.expect_intent.is_some()
            || step.expect_reinit_trigger.is_some()
            || !step.geometry_moved.is_empty()
            || !step.rooms_unchanged.is_empty()
            || !step.rooms_changed.is_empty()
            || !step.aabb_shift.is_empty()
            || !step.units_absent.is_empty()
            || !step.warnings_contain.is_empty()
            || !step.warnings_absent.is_empty()
            || !step.plan_totals.is_empty()
            || !step.geometry_counts.is_empty()
            || !step.geometry_sample.is_empty()
        {
            // 对账轮会把同一个右端反复重排，两处都踩过：① 预期失败的那一窗（模型侧生成失败不进连败账，
            // 对账一直重试）；② **已经追平之后**——2026-10-27 实测 fail-inject 的 728：上一任务
            // `succeeded` 之后 2 s，日志里落下 `[reconcile] 新排 sesno 728..=728`，于是「最新一条」是
            // `queued`，断言读空（同一份代码连跑两次都红在同一行）。所以只要期望的是终态，就挑「最近一条
            // 已落终态的」；期望排队 / 在跑时才看最新一条。
            let expected_state = step.task_state.as_deref().unwrap_or("succeeded");
            let want_terminal = matches!(expected_state, "succeeded" | "failed" | "partial");
            let task = if step.expect_model_sesno_stay.is_some() || want_terminal {
                service.latest_terminal_task(case.dbnum, end)
            } else {
                service.latest_task(case.dbnum, end)
            }
            .unwrap_or_else(|| {
                panic!(
                    "[{what}] /tasks 里找不到 dbnum {} 右端 {} 的批次",
                    case.dbnum, end
                )
            });
            let state = task.get("state").and_then(Value::as_str).unwrap_or("");
            if state != expected_state {
                problems.push(format!("任务 state={state}，期望 {expected_state}"));
            }
            // 建行时的意图在 `detail` 里（`batch_scheduler` 入队时写：`intent` / `reinit_trigger` / `frozen_target_sesno`）。
            let detail_str = |key: &str| {
                task.pointer(&format!("/detail/{key}"))
                    .or_else(|| task.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            if let Some(expected) = &step.expect_intent {
                let intent = detail_str("intent");
                if &intent != expected {
                    problems.push(format!("任务 detail.intent={intent}，期望 {expected}"));
                }
            }
            if let Some(expected) = &step.expect_reinit_trigger {
                let trigger = detail_str("reinit_trigger");
                if &trigger != expected {
                    problems.push(format!(
                        "任务 detail.reinit_trigger={trigger}，期望 {expected}"
                    ));
                }
            }
            if step.expect_intent.is_some() || step.expect_reinit_trigger.is_some() {
                eprintln!(
                    "  任务 {} intent={} reinit_trigger={} 区间 {:?}..={:?} state={state}",
                    task.get("task_id").and_then(Value::as_str).unwrap_or("?"),
                    detail_str("intent"),
                    detail_str("reinit_trigger"),
                    task.get("start_sesno").and_then(Value::as_i64),
                    task.get("end_sesno").and_then(Value::as_i64)
                );
            }
            let units = task
                .pointer("/result/units")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(expected) = step.unit_count {
                if units.len() != expected {
                    problems.push(format!(
                        "这一窗交付单元 {} 个，期望 {expected}：{:?}",
                        units.len(),
                        units
                            .iter()
                            .map(|unit| unit.get("root_refno").cloned().unwrap_or(Value::Null))
                            .collect::<Vec<_>>()
                    ));
                }
            }
            for (root, kind) in &step.unit_kinds {
                match units.iter().find(|unit| {
                    unit.get("root_refno").and_then(Value::as_str) == Some(root.as_str())
                }) {
                    Some(unit) => {
                        let actual = unit.get("kind").and_then(Value::as_str).unwrap_or("regen");
                        if actual != kind {
                            problems.push(format!(
                                "根 {root} kind={actual}，期望 {kind}（fallback: {:?}）",
                                unit.get("transform_fallback_reason")
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里：{units:?}")),
                }
            }
            let unit_of = |root: &str| {
                units
                    .iter()
                    .find(|unit| unit.get("root_refno").and_then(Value::as_str) == Some(root))
            };
            for (root, moved) in &step.geometry_moved {
                match unit_of(root) {
                    Some(unit) => {
                        let actual = unit.pointer("/geometry_diff/moved").and_then(Value::as_u64);
                        if actual != Some(*moved) {
                            problems.push(format!(
                                "根 {root} geometry_diff.moved={actual:?}，期望 {moved}（diff: {:?}）",
                                unit.get("geometry_diff")
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里（几何差）")),
                }
            }
            // 对照：这些根不该出现在这一窗的交付单元里。
            for root in &step.units_absent {
                if unit_of(root).is_some() {
                    problems.push(format!("根 {root} 不该在这一窗的交付单元里，却在"));
                }
            }
            // 回执的 warnings：子串，以及窗口规划那一行按字段比。
            let warnings: Vec<String> = task
                .pointer("/result/warnings")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let warnings_text = warnings.join("\n");
            // 把这一窗「后端说了什么」打进日志：规划那一行、每个交付单元的 kind 与几何差。
            // 写期望的人第一次跑的时候从这里抄数（会话号每次录都不同，只有跑过才知道）。
            if let Some(line) = warnings
                .iter()
                .find(|line| parse_plan_totals(line).is_some())
            {
                eprintln!("  [{what}] {line}");
            }
            for unit in &units {
                // 没走成单元路 / 变换路时后端写在回执里的原因（走成了就没有这两格）。
                let fallback: Vec<String> = ["unit_fallback_reason", "transform_fallback_reason"]
                    .iter()
                    .filter_map(|key| {
                        unit.get(*key)
                            .and_then(Value::as_str)
                            .map(|reason| format!(" {key}={reason:?}"))
                    })
                    .collect();
                eprintln!(
                    "  [{what}] 单元 {} kind={} 几何差 moved/added/removed={}/{}/{} 行数 {}→{} 样本 {}{}",
                    unit.get("root_refno")
                        .and_then(Value::as_str)
                        .unwrap_or("?"),
                    unit.get("kind").and_then(Value::as_str).unwrap_or("regen"),
                    unit.pointer("/geometry_diff/moved").unwrap_or(&Value::Null),
                    unit.pointer("/geometry_diff/added").unwrap_or(&Value::Null),
                    unit.pointer("/geometry_diff/removed")
                        .unwrap_or(&Value::Null),
                    unit.pointer("/geometry_before/records")
                        .unwrap_or(&Value::Null),
                    unit.pointer("/geometry_after/records")
                        .unwrap_or(&Value::Null),
                    unit.pointer("/geometry_diff/sample")
                        .unwrap_or(&Value::Null),
                    fallback.concat(),
                );
            }
            for needle in &step.warnings_contain {
                if !warnings_text.contains(needle.as_str()) {
                    problems.push(format!(
                        "任务回执的 warnings 里没有 {needle:?}：{}",
                        truncate(&warnings_text, 600)
                    ));
                }
            }
            for needle in &step.warnings_absent {
                if warnings_text.contains(needle.as_str()) {
                    problems.push(format!("任务回执的 warnings 里不该有 {needle:?}"));
                }
            }
            if !step.plan_totals.is_empty() {
                match warnings.iter().find_map(|line| parse_plan_totals(line)) {
                    Some(actual) => {
                        for (key, expected) in &step.plan_totals {
                            if actual.get(key.as_str()) != Some(expected) {
                                problems.push(format!(
                                    "窗口规划 {key}={:?}，期望 {expected}（整行：{actual:?}）",
                                    actual.get(key.as_str())
                                ));
                            }
                        }
                    }
                    None => problems.push(format!(
                        "任务回执的 warnings 里没有窗口规划那一行（window S→T: candidates=…）：{}",
                        truncate(&warnings_text, 400)
                    )),
                }
            }
            // 单元回执里的几何差与行数，只比写了的格。
            for (root, expected) in &step.geometry_counts {
                match unit_of(root) {
                    Some(unit) => {
                        let checks = [
                            (
                                "geometry_diff.moved",
                                "/geometry_diff/moved",
                                expected.moved,
                            ),
                            (
                                "geometry_diff.added",
                                "/geometry_diff/added",
                                expected.added,
                            ),
                            (
                                "geometry_diff.removed",
                                "/geometry_diff/removed",
                                expected.removed,
                            ),
                            (
                                "geometry_before.records",
                                "/geometry_before/records",
                                expected.records_before,
                            ),
                            (
                                "geometry_after.records",
                                "/geometry_after/records",
                                expected.records_after,
                            ),
                        ];
                        for (label, pointer, want) in checks {
                            let Some(want) = want else {
                                continue;
                            };
                            let actual = unit.pointer(pointer).and_then(Value::as_u64);
                            if actual != Some(want) {
                                problems.push(format!("根 {root} {label}={actual:?}，期望 {want}"));
                            }
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里（几何计数）")),
                }
            }
            // 几何有变化的是哪几只：样本没截断就要正好是这几只，截断了只要求列出的都在。
            for (root, expected) in &step.geometry_sample {
                let Some(unit) = unit_of(root) else {
                    problems.push(format!("根 {root} 不在这一窗的单元里（几何样本）"));
                    continue;
                };
                let normal = |text: &str| text.trim().trim_start_matches('=').replace('/', "_");
                let sample: Vec<String> = unit
                    .pointer("/geometry_diff/sample")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(normal))
                            .collect()
                    })
                    .unwrap_or_default();
                let changed = ["moved", "added", "removed"]
                    .iter()
                    .filter_map(|key| {
                        unit.pointer(&format!("/geometry_diff/{key}"))
                            .and_then(Value::as_u64)
                    })
                    .sum::<u64>();
                let wanted: Vec<String> = expected.iter().map(|text| normal(text)).collect();
                let missing: Vec<&String> = wanted
                    .iter()
                    .filter(|refno| !sample.contains(refno))
                    .collect();
                let complete = sample.len() as u64 == changed;
                let extra: Vec<&String> = sample
                    .iter()
                    .filter(|refno| complete && !wanted.contains(refno))
                    .collect();
                if !missing.is_empty() || !extra.is_empty() {
                    problems.push(format!(
                        "根 {root} 几何有变化的构件对不上：缺 {missing:?}，多 {extra:?}（回执样本 {sample:?}，共 {changed} 只有变化{}）",
                        if complete { "" } else { "，样本被截断，只查缺的" }
                    ));
                }
            }
            for root in &step.rooms_unchanged {
                match unit_of(root) {
                    Some(unit) => {
                        let before = unit.get("rooms_before");
                        let after = unit.get("rooms_after");
                        if before.is_none() || before != after {
                            problems.push(format!(
                                "根 {root} 房间前后应相同：before={before:?} after={after:?}"
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里（房间）")),
                }
            }
            for (root, change) in &step.rooms_changed {
                let Some(unit) = unit_of(root) else {
                    problems.push(format!("根 {root} 不在这一窗的单元里（跨房）"));
                    continue;
                };
                let before = room_nums(unit.get("rooms_before"));
                let after = room_nums(unit.get("rooms_after"));
                let expect_before: BTreeSet<String> = change.before.iter().cloned().collect();
                let expect_after: BTreeSet<String> = change.after.iter().cloned().collect();
                if before.as_ref() != Some(&expect_before) || after.as_ref() != Some(&expect_after)
                {
                    problems.push(format!(
                        "根 {root} 房间号 before={before:?} after={after:?}，期望 {:?} → {:?}",
                        change.before, change.after
                    ));
                } else {
                    eprintln!("  根 {root} 房间 {:?} → {:?}", change.before, change.after);
                }
            }
            for (root, expected) in &step.aabb_shift {
                let Some(unit) = unit_of(root) else {
                    problems.push(format!("根 {root} 不在这一窗的单元里（包围盒）"));
                    continue;
                };
                match aabb_shift_of(unit) {
                    Some((mins, maxs)) => {
                        if !within_tolerance(mins, *expected) || !within_tolerance(maxs, *expected)
                        {
                            problems.push(format!(
                                "根 {root} 包围盒平移 min={mins:?} max={maxs:?}，期望 {expected:?}（before={:?} after={:?}）",
                                unit.get("geometry_before"),
                                unit.get("geometry_after")
                            ));
                        } else {
                            eprintln!("  根 {root} 包围盒平移 {mins:?}（期望 {expected:?}）");
                        }
                    }
                    None => problems.push(format!(
                        "根 {root} 回执里缺几何前后或没包围盒：before={:?} after={:?}",
                        unit.get("geometry_before"),
                        unit.get("geometry_after")
                    )),
                }
                let records = |which: &str| {
                    unit.pointer(&format!("/{which}/records"))
                        .and_then(Value::as_u64)
                };
                let (before, after) = (records("geometry_before"), records("geometry_after"));
                if before.is_none() || before != after {
                    problems.push(format!(
                        "根 {root} 刚体位移前后行数应相同：before={before:?} after={after:?}"
                    ));
                }
            }
        }
        for root in &step.records_present {
            match service.records_count(root) {
                Some(0) => problems.push(format!("根 {root} /model/records 应有行，实际 0")),
                Some(_) => {}
                None => problems.push(format!("根 {root} 已不在文件索引里，却期望有模型行")),
            }
            let rows = service.store_rows_under(root);
            if rows == 0 {
                problems.push(format!("根 {root} 库上 inst_relate 应有行，实际 0"));
            }
        }
        for root in &step.records_absent {
            if let Some(count) = service.records_count(root) {
                if count != 0 {
                    problems.push(format!("根 {root} /model/records 应为空，实际 {count} 行"));
                }
            }
            let rows = service.store_rows_under(root);
            if rows != 0 {
                problems.push(format!(
                    "根 {root} 库上还留着 {rows} 条 inst_relate（ADR-088 那类老行）"
                ));
            }
        }
        for root in &step.generate_prime {
            let first = service.post("model/generate", serde_json::json!({ "refno": root }));
            let first_status = first.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(first_status, "Generated" | "AlreadyAvailable") {
                problems.push(format!(
                    "根 {root} 首次 /model/generate → {first_status}，期望 Generated 或 AlreadyAvailable：{first}"
                ));
                continue;
            }
            let started = Instant::now();
            let second = service.post("model/generate", serde_json::json!({ "refno": root }));
            let elapsed = started.elapsed();
            let second_status = second.get("status").and_then(Value::as_str).unwrap_or("");
            if second_status != "AlreadyAvailable" {
                problems.push(format!(
                    "根 {root} 常驻后第二次 /model/generate → {second_status}，期望 AlreadyAvailable"
                ));
            }
            eprintln!(
                "  根 {root} 常驻：首次 {first_status}，第二次 {second_status}（{} ms）",
                elapsed.as_millis()
            );
        }
        for root in &step.generate_already_available {
            let started = Instant::now();
            let payload = service.post("model/generate", serde_json::json!({ "refno": root }));
            let elapsed = started.elapsed();
            let status = payload.get("status").and_then(Value::as_str).unwrap_or("");
            if status != "AlreadyAvailable" {
                problems.push(format!(
                    "根 {root} /model/generate → {status}，期望 AlreadyAvailable"
                ));
            }
            eprintln!(
                "  根 {root} /model/generate → {status}，{} ms",
                elapsed.as_millis()
            );
            if let Some(budget) = step.generate_budget_ms {
                if elapsed.as_millis() > u128::from(budget) {
                    problems.push(format!(
                        "根 {root} /model/generate 用了 {} ms，超过预算 {budget} ms",
                        elapsed.as_millis()
                    ));
                }
            }
        }
        // 改后的模型对上 E3D 在这个状态导出的 RVM。
        if let Some(compare) = &step.rvm_compare {
            let snapshot = repo().join(&compare.snapshot);
            let rvm = compare.rvm.as_ref().map(|path| repo().join(path));
            match service.rvm_compare(&snapshot, rvm.as_deref(), &step.to.to_string()) {
                Ok(()) => eprintln!(
                    "  [{what}] 生成结果对上了 E3D 导出的快照 {}",
                    compare.snapshot
                ),
                Err(reason) => problems.push(reason),
            }
        }
        // 交付删除记录：读本库、从用例起点会话往后的全部记录，逐条对期望。
        if !step.datacenter_deletes.is_empty() {
            let route = format!(
                "datacenter/deletes?dbnum={}&since_sesno={}&limit=1000",
                case.dbnum, case.start
            );
            let page = service.get(&route);
            if page.is_none() {
                problems.push(format!("GET /{route} 没有回 JSON"));
            }
            let items = page
                .as_ref()
                .and_then(|page| page.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for expect in &step.datacenter_deletes {
                let rows: Vec<&Value> = items
                    .iter()
                    .filter(|row| {
                        row.get("refno").and_then(Value::as_str) == Some(expect.refno.as_str())
                    })
                    .collect();
                if !expect.present {
                    if !rows.is_empty() {
                        problems.push(format!(
                            "交付删除记录里不该有 {}，却有：{rows:?}",
                            expect.refno
                        ));
                    }
                    continue;
                }
                let matched =
                    rows.iter().any(|row| {
                        expect.name.as_deref().is_none_or(|name| {
                            row.get("name").and_then(Value::as_str) == Some(name)
                        }) && expect.reason.as_deref().is_none_or(|reason| {
                            row.get("reason").and_then(Value::as_str) == Some(reason)
                        }) && expect.sesno.is_none_or(|sesno| {
                            row.get("sesno").and_then(Value::as_u64) == Some(u64::from(sesno))
                        })
                    });
                if !matched {
                    problems.push(format!(
                        "交付删除记录里没有对得上的 {}（期望 name={:?} reason={:?} sesno={:?}）：{rows:?}",
                        expect.refno, expect.name, expect.reason, expect.sesno
                    ));
                }
            }
        }
        if !step.log_must_not_contain.is_empty() {
            let log = read_service_logs(&service.stdout_path);
            for needle in &step.log_must_not_contain {
                let hits = log
                    .lines()
                    .filter(|line| line.contains(needle.as_str()))
                    .count();
                if hits > 0 {
                    let sample = log
                        .lines()
                        .find(|line| line.contains(needle.as_str()))
                        .unwrap_or_default();
                    problems.push(format!(
                        "服务日志里不该有「{needle}」，却有 {hits} 行，如：{sample}"
                    ));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "[{what}] 断言不过：\n{}\n日志 {}",
            problems.join("\n"),
            service.stdout_path.display()
        );
        eprintln!("  [{what}] 断言全过");
    }
}
