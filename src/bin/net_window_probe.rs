//! 净窗口收集器取证探针：跑一遍生产收集器，再让 **E3D TTY** 判它收得对不对。
//!
//! 探针自己不是尺子。它做两件事：
//!
//! 1. **记账**：`IncrementPipeline::collect_window`（→ `e3d_collector::collect_net_window`，
//!    e3d-io 索引双根差分 + 两端 `to_named_attmap` → `diff_ele_data`）在
//!    `(db 文件, base+1..=target)` 上看见的操作流，每 refno 一条 `Add` / `Deleted` / `Modified`；
//! 2. **判据**：`--tty` 开**一个只读 E3D 会话**，逐 refno `=<refno>` + `Q TYPE`——
//!    `Add` / `Modified` 说的元素现在必须还在且 noun 对得上，`Deleted` 说的必须已经不在。
//!
//! TTY 读的是**文件当前状态**，所以第 2 条只在 `target == 文件最新会话` 时成立；不成立就直接红，
//! 不给「看起来过了」的假绿。同理，一窗全是删除时 TTY 只能做「都不在」这种真空判定——所以
//! 「必须还在」的那一组为空就红，请用 `--tty-anchor` 点一个已知活着的 refno 当活证。
//!
//! 2026-09-04 之前这里是 `shadow_collector_parity`：拿 old-pdms-io 的净窗口当参照臂逐 refno 对拍。
//! old-pdms-io 退役后那条臂先变成「同一根索引差分上的两条 e3d-io 合成路径互查」（自己比自己），
//! 用户 2026-09-03 拍板「对拍要以 e3d tty 的数据为准，删除和 pdms-io 的对拍」后连同
//! `data_interface::shadow_collector` 一起删除。
//!
//! ```text
//! cargo run --bin net_window_probe -- --db-file <ams>\ams7999_0001 --base 45  --target 46  --expect-add 22 --expect-delete 0 --must-not-delete 24383/72318,24383/72319 --tty
//! cargo run --bin net_window_probe -- --db-file <ams>\ams1112_0001 --base 721 --target 722 --expect-delete 24673 --tty --tty-anchor 9304/2
//! ```
//!
//! 期望不满足、或 TTY 判死任何一条 → 退出码 1。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

use aios_core::RefU64;
use aios_database::data_interface::db_file_header;
use aios_database::data_interface::ele_ops::{EleOperationData, EleOperationDetail};
use aios_database::data_interface::increment_pipeline::IncrementPipeline;
use aios_database::data_interface::model_impact::{classify_operation_effects, owner_change};
use aios_database::e3d_query::E3dDriver;
use clap::Parser;

/// 成员表变了记成这个伪属性名（收集器没有 `MEMBERS` 这个真属性）。
const MEMBERS_PSEUDO_ATTR: &str = "MEMBERS";
/// 每桶在控制台 / JSON 里最多落多少条明细；汇总计数不截断。
const SAMPLE_CAP: usize = 20;

#[derive(Parser, Debug)]
#[command(about = "净窗口收集器取证：生产收集器的操作流 + E3D TTY 现场判据")]
struct Args {
    /// E3D 库文件（`.dat` 主文件，例如 `ams7999_0001`）。
    #[arg(long)]
    db_file: PathBuf,
    /// 窗口 base 会话号（生产窗口取 `base+1..=target`）。
    #[arg(long)]
    base: u32,
    /// 窗口 target 会话号。
    #[arg(long)]
    target: u32,
    /// 每桶在控制台最多打印多少条明细。
    #[arg(long, default_value_t = SAMPLE_CAP)]
    samples: usize,
    /// 结构化结果落盘路径（单文件 JSON；TTY 原始回显写同名 `.tty.log`）。
    #[arg(long)]
    json_out: Option<PathBuf>,
    /// JSON 里操作流最多落多少条明细（汇总计数不受影响；24673 条删除全带是 5 MB）。
    #[arg(long, default_value_t = 400)]
    json_rows_cap: usize,
    /// 硬门：`Add` 数必须等于它。
    #[arg(long)]
    expect_add: Option<usize>,
    /// 硬门：`Deleted` 数必须等于它。
    #[arg(long)]
    expect_delete: Option<usize>,
    /// 硬门：`Modified` 数必须等于它。
    #[arg(long)]
    expect_modified: Option<usize>,
    /// 这些 refno（逗号分隔 `a/b`）不许出现 `Deleted`；开 `--tty` 时它们同时进「必须还在」那一组。
    #[arg(long)]
    must_not_delete: Option<String>,
    /// 开一个只读 E3D TTY 会话，逐 refno 判操作流的死活（本探针唯一的判据）。
    #[arg(long, default_value_t = false)]
    tty: bool,
    /// 额外的 TTY 活证 refno（逗号分隔 `a/b`）：必须还在。整窗只有删除时用它给 TTY 一个真阳性。
    #[arg(long)]
    tty_anchor: Option<String>,
    /// 每类操作最多交给 TTY 多少条（按 refno 序等距抽样）；`0` = 全量。
    #[arg(long, default_value_t = 200)]
    tty_sample: usize,
}

/// 操作种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
enum OpKind {
    Add,
    Deleted,
    Modified,
}

impl OpKind {
    fn label(self) -> &'static str {
        match self {
            OpKind::Add => "Add",
            OpKind::Deleted => "Deleted",
            OpKind::Modified => "Modified",
        }
    }

    /// TTY 该看见什么：`Add` / `Modified` 的元素现在还在，`Deleted` 的不在。
    fn expects_alive(self) -> bool {
        !matches!(self, OpKind::Deleted)
    }
}

/// 操作流里的一条。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct Op {
    kind: OpKind,
    /// `Add` / `Modified` 的 noun；`Deleted` 恒空（净窗口的删除不带类型）。
    noun: String,
    /// `Modified`：变了的属性名（归一化、含 `UDA:<id>`），成员表变了追加 `MEMBERS`。
    attrs: BTreeSet<String>,
    /// last-touch 会话号。
    sesno: u32,
    old_owner: Option<String>,
    new_owner: Option<String>,
}

/// JSON 里的一行操作：refno 平铺在操作字段旁边。
#[derive(Debug, serde::Serialize)]
struct OpRow<'a> {
    refno: String,
    #[serde(flatten)]
    op: &'a Op,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
struct Tally {
    add: usize,
    deleted: usize,
    modified: usize,
}

/// 把 `CollectedWindow::range_eles` 折成「每 refno 一条操作」。
///
/// 净窗口每 refno 恰一条，这里照样以 refno 为键；`EleOperationDetail::None` 不是操作，只记个数。
fn fold_ops(range_eles: &BTreeMap<u32, Vec<EleOperationData>>) -> (BTreeMap<u64, Op>, usize) {
    let mut ops: BTreeMap<u64, Op> = BTreeMap::new();
    let mut none_ops = 0usize;
    for op in range_eles.values().flatten() {
        let kind = match &op.detail {
            EleOperationDetail::Add(_) => OpKind::Add,
            EleOperationDetail::Deleted => OpKind::Deleted,
            EleOperationDetail::Modified(_) => OpKind::Modified,
            EleOperationDetail::None => {
                none_ops += 1;
                continue;
            }
        };
        let mut attrs: BTreeSet<String> = BTreeSet::new();
        if let EleOperationDetail::Modified(modified) = &op.detail {
            attrs.extend(
                classify_operation_effects(op)
                    .changed_attributes
                    .into_iter()
                    .map(|name| name.to_ascii_uppercase()),
            );
            if modified.children_changed.is_some() {
                attrs.insert(MEMBERS_PSEUDO_ATTR.to_string());
            }
        }
        let (old_owner, new_owner) = owner_change(op);
        ops.insert(
            op.refno.0,
            Op {
                kind,
                noun: op.get_noun_type().trim().to_ascii_uppercase(),
                attrs,
                sesno: op.sesno,
                old_owner: old_owner.map(|refno| refno.to_pdms_str()),
                new_owner: new_owner.map(|refno| refno.to_pdms_str()),
            },
        );
    }
    (ops, none_ops)
}

fn tally_of(ops: &BTreeMap<u64, Op>) -> Tally {
    let mut tally = Tally::default();
    for op in ops.values() {
        match op.kind {
            OpKind::Add => tally.add += 1,
            OpKind::Deleted => tally.deleted += 1,
            OpKind::Modified => tally.modified += 1,
        }
    }
    tally
}

fn pdms_str(key: u64) -> String {
    RefU64(key).to_pdms_str()
}

fn parse_key(text: &str) -> Option<u64> {
    let (w0, w1) = text.trim().split_once('/')?;
    Some(RefU64::from_two_nums(w0.parse().ok()?, w1.parse::<i64>().ok()? as u32).0)
}

fn parse_key_list(list: Option<&String>) -> (Vec<(String, u64)>, Vec<String>) {
    let mut keys = Vec::new();
    let mut bad = Vec::new();
    for text in list
        .map(String::as_str)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        match parse_key(text) {
            Some(key) => keys.push((text.to_string(), key)),
            None => bad.push(text.to_string()),
        }
    }
    (keys, bad)
}

/// 一条交给 TTY 的判据。
#[derive(Debug, Clone, serde::Serialize)]
struct TtyCheck {
    refno: String,
    /// 出处：操作种类，或 `anchor` / `must-not-delete`。
    origin: String,
    /// 收集器说的 noun；空表示不判 noun（删除、活证）。
    expect_noun: String,
    expect_alive: bool,
    observed_alive: Option<bool>,
    observed_noun: Option<String>,
}

impl TtyCheck {
    fn verdict(&self) -> Option<String> {
        let observed = self.observed_alive?;
        if observed != self.expect_alive {
            return Some(format!(
                "{} {}：收集器说 {}，TTY 说 {}",
                self.refno,
                self.origin,
                if self.expect_alive {
                    "还在"
                } else {
                    "已删"
                },
                if observed { "还在" } else { "不在" }
            ));
        }
        let observed_noun = self.observed_noun.as_deref().unwrap_or_default();
        if observed
            && !self.expect_noun.is_empty()
            && !observed_noun.is_empty()
            && !observed_noun.eq_ignore_ascii_case(&self.expect_noun)
        {
            return Some(format!(
                "{} {}：收集器说 noun={}，TTY 说 {observed_noun}",
                self.refno, self.origin, self.expect_noun
            ));
        }
        None
    }
}

/// 按 refno 序等距抽样，`cap == 0` 或不足时全取。抽样是确定的：同一窗口每次取同一批。
fn sample<T>(rows: Vec<T>, cap: usize) -> Vec<T> {
    if cap == 0 || rows.len() <= cap {
        return rows;
    }
    let stride = rows.len() as f64 / cap as f64;
    let picked = (0..cap)
        .map(|index| ((index as f64 * stride).floor() as usize).min(rows.len() - 1))
        .collect::<BTreeSet<_>>();
    rows.into_iter()
        .enumerate()
        .filter(|(index, _)| picked.contains(index))
        .map(|(_, row)| row)
        .collect()
}

/// 一个 refno 一段：导航过去，能到就打 `ALIVE` 再 `Q TYPE`；到不了就只有空段。
///
/// 不用 `else` / `NOT`：段里有没有 `ALIVE` 这一行就是死活本身，少一种 PML 方言风险。
fn tty_source(checks: &[TtyCheck]) -> String {
    let mut out = String::new();
    for (index, check) in checks.iter().enumerate() {
        out.push_str(&format!(
            "!nwalive = TRUE\n={}\nhandle any\n!nwalive = FALSE\nelsehandle none\nendhandle\n\
             $P NW-{index}-BEGIN\nif (!nwalive) then\n$P NW-{index}-ALIVE\nQ TYPE\nendif\n\
             $P NW-{index}-END\n",
            check.refno
        ));
    }
    out
}

fn marker_section<'a>(raw: &'a str, index: usize) -> anyhow::Result<&'a str> {
    let begin = format!("NW-{index}-BEGIN");
    let end = format!("NW-{index}-END");
    let start = raw
        .find(&begin)
        .ok_or_else(|| anyhow::anyhow!("TTY 回显缺少 {begin}"))?
        + begin.len();
    let tail = &raw[start..];
    let finish = tail
        .find(&end)
        .ok_or_else(|| anyhow::anyhow!("TTY 回显缺少 {end}"))?;
    Ok(&tail[..finish])
}

/// `Q TYPE` 的回显形如 `Type ZONE`；导航失败的段里没有它。
fn noun_in(section: &str) -> Option<String> {
    section
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Type "))
        .map(|noun| noun.trim().to_ascii_uppercase())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.base < args.target,
        "base 必须小于 target（数字口径；链序由收集器自己校验）"
    );
    println!("== 文件 == {}", args.db_file.display());
    println!("== 窗口 == {} → {}", args.base, args.target);

    // ① 记账：数据面今天真正走的收集器。
    let started = Instant::now();
    let collected = IncrementPipeline::collect_window(
        &args.db_file,
        (args.base as i32 + 1)..=(args.target as i32),
    )
    .map_err(|error| anyhow::anyhow!("collect_window 失败：{error:#}"))?;
    let collect_ms = started.elapsed().as_millis();
    println!(
        "[收集] {}..={} 用时 {collect_ms}ms，会话 {:?}",
        args.base + 1,
        args.target,
        collected.session_sesnos
    );
    for warning in &collected.warnings {
        println!("[收集]   warning: {warning}");
    }
    let (ops, none_ops) = fold_ops(&collected.range_eles);
    let tally = tally_of(&ops);
    println!(
        "[收集] 操作流 {} 条 = add {} + mod {} + del {}（None {none_ops} 条）",
        ops.len(),
        tally.add,
        tally.modified,
        tally.deleted
    );
    for kind in [OpKind::Add, OpKind::Modified, OpKind::Deleted] {
        let rows = ops
            .iter()
            .filter(|(_, op)| op.kind == kind)
            .take(args.samples)
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        println!("\n-- {} --", kind.label());
        for (key, op) in rows {
            println!(
                "  {:<16} {:<6} sesno={} {}",
                pdms_str(*key),
                op.noun,
                op.sesno,
                op.attrs.iter().cloned().collect::<Vec<_>>().join(",")
            );
        }
    }

    let mut gate_failures: Vec<String> = Vec::new();
    let mut expect = |name: &str, expected: Option<usize>, actual: usize| {
        if let Some(expected) = expected
            && expected != actual
        {
            gate_failures.push(format!("{name}: 期望 {expected}，实际 {actual}"));
        }
    };
    expect("add", args.expect_add, tally.add);
    expect("deleted", args.expect_delete, tally.deleted);
    expect("modified", args.expect_modified, tally.modified);

    let (must_not_delete, bad_keys) = parse_key_list(args.must_not_delete.as_ref());
    let (anchors, bad_anchors) = parse_key_list(args.tty_anchor.as_ref());
    for text in bad_keys.iter().chain(bad_anchors.iter()) {
        gate_failures.push(format!("{text:?} 不是 a/b 形式"));
    }
    for (text, key) in &must_not_delete {
        if ops.get(key).is_some_and(|op| op.kind == OpKind::Deleted) {
            gate_failures.push(format!("{text} 被收集器判 Deleted"));
        }
    }

    // ② 判据：一个只读 TTY 会话。
    let mut tty_checks: Vec<TtyCheck> = Vec::new();
    let mut tty_raw = String::new();
    if args.tty {
        let latest = db_file_header::latest_sesno(&args.db_file)?;
        if latest != args.target {
            gate_failures.push(format!(
                "TTY 判据只在 target 就是文件当前状态时成立：文件最新会话 {latest}，target {}",
                args.target
            ));
        }
        for kind in [OpKind::Add, OpKind::Modified, OpKind::Deleted] {
            let rows = ops
                .iter()
                .filter(|(_, op)| op.kind == kind)
                .map(|(key, op)| TtyCheck {
                    refno: pdms_str(*key),
                    origin: kind.label().to_string(),
                    expect_noun: op.noun.clone(),
                    expect_alive: kind.expects_alive(),
                    observed_alive: None,
                    observed_noun: None,
                })
                .collect::<Vec<_>>();
            let total = rows.len();
            let picked = sample(rows, args.tty_sample);
            if total > picked.len() {
                println!(
                    "[tty] {} {total} 条按 --tty-sample {} 抽 {} 条",
                    kind.label(),
                    args.tty_sample,
                    picked.len()
                );
            }
            tty_checks.extend(picked);
        }
        for (text, key) in must_not_delete.iter().chain(anchors.iter()) {
            let refno = pdms_str(*key);
            if tty_checks.iter().any(|check| check.refno == refno) {
                continue;
            }
            tty_checks.push(TtyCheck {
                refno,
                origin: if anchors.iter().any(|(name, _)| name == text) {
                    "anchor".into()
                } else {
                    "must-not-delete".into()
                },
                expect_noun: ops.get(key).map(|op| op.noun.clone()).unwrap_or_default(),
                expect_alive: true,
                observed_alive: None,
                observed_noun: None,
            });
        }
        let alive_expected = tty_checks.iter().filter(|check| check.expect_alive).count();
        if alive_expected == 0 {
            gate_failures.push(
                "TTY 臂一条「必须还在」都没有：全是删除的窗口请用 --tty-anchor 点一个活证，\
                 否则「都不在」这种真空判定判不出 MDB 里根本没这个库"
                    .into(),
            );
        }
        println!(
            "\n[tty] 一个只读会话判 {} 条（必须还在 {alive_expected}，必须不在 {}）",
            tty_checks.len(),
            tty_checks.len() - alive_expected
        );
        let repo = std::env::current_dir()?;
        tty_raw = E3dDriver::from_env(&repo)?.run_source(
            &repo,
            "net-window-probe",
            &tty_source(&tty_checks),
        )?;
        for (index, check) in tty_checks.iter_mut().enumerate() {
            let section = marker_section(&tty_raw, index)?;
            check.observed_alive = Some(section.contains(&format!("NW-{index}-ALIVE")));
            check.observed_noun = noun_in(section);
        }
        let verdicts = tty_checks
            .iter()
            .filter_map(TtyCheck::verdict)
            .collect::<Vec<_>>();
        println!(
            "[tty] 判完：{} 条一致，{} 条被 TTY 判死",
            tty_checks.len() - verdicts.len(),
            verdicts.len()
        );
        for verdict in verdicts.iter().take(args.samples) {
            println!("  {verdict}");
        }
        if verdicts.len() > args.samples {
            println!(
                "  … 另 {} 条（见 --json-out）",
                verdicts.len() - args.samples
            );
        }
        gate_failures.extend(verdicts);
    } else {
        println!("\n== 判据 == 没开 --tty：本次只记账，收集器对不对**未判**。");
    }

    if let Some(path) = &args.json_out {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let output = serde_json::json!({
            "db_file": args.db_file.display().to_string(),
            "base": args.base,
            "target": args.target,
            "collect_ms": collect_ms,
            "warnings": collected.warnings,
            "session_sesnos": collected.session_sesnos,
            "tally": tally,
            "none_ops": none_ops,
            "ops": ops
                .iter()
                .take(args.json_rows_cap)
                .map(|(key, op)| OpRow {
                    refno: pdms_str(*key),
                    op,
                })
                .collect::<Vec<_>>(),
            "tty": {
                "enabled": args.tty,
                "checks": tty_checks,
                "verdicts": tty_checks.iter().filter_map(TtyCheck::verdict).collect::<Vec<_>>(),
            },
            "gate_failures": gate_failures,
        });
        std::fs::write(path, serde_json::to_string_pretty(&output)?)?;
        println!("\n结构化结果已写入 {}", path.display());
        if args.tty {
            let log = path.with_extension("tty.log");
            std::fs::write(&log, &tty_raw)?;
            println!("TTY 原始回显已写入 {}", log.display());
        }
    }

    println!();
    if gate_failures.is_empty() {
        println!("== 门 == 全部通过");
        Ok(())
    } else {
        for failure in &gate_failures {
            println!("== 门 == 未过：{failure}");
        }
        std::process::exit(1);
    }
}
