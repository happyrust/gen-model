//! 增量更新自动化测试案例登记表的钉（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §5.1，P0）。
//!
//! `tests/fixtures/increment_cases.json` 是「案例在哪一层、认哪条裁判、由哪些测试承载」的单一事实。
//! 这里钉两件事，都不跑任何库、不起任何进程：
//!
//! 1. 表本身形状对：编号唯一且与组一致、层 / 裁判 / 状态取值在定义域里、`todo` / `corpus` / `partial` 都有落地段；
//! 2. **登记了的测试真的存在**：`lib:` 在 `src/**` 里有同名 `fn`，`test:` 在 `tests/` 下有同名文件，
//!    `l3:` 在 `l3_suite` 的场景表里，`vendor:` 在 `vendor/shared` 下有文件且含同名 `fn`，`script:` / `fixture:` 文件在，
//!    `replay:` 在 `tests/increment_replay/cases/` 下有同名用例。被删 / 改名的测试在这里先红，不让登记表慢慢说谎。
//!
//! 跑法：`cargo test --test increment_cases_registry`。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Registry {
    format: String,
    plan: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    group: String,
    layer: String,
    title: String,
    oracle: Vec<String>,
    status: String,
    #[serde(default)]
    tests: Vec<String>,
    #[serde(default)]
    stage: Option<String>,
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn registry() -> Registry {
    let path = repo().join("tests/fixtures/increment_cases.json");
    let text = fs::read_to_string(&path).expect("read tests/fixtures/increment_cases.json");
    serde_json::from_str(&text).expect("increment_cases.json parses")
}

/// 把一棵目录下所有 `.rs` 读成一串（CRLF 归一），给 `fn name(` 查找用。
/// 只读源码，不跳进联接点外的其他目录（`vendor/shared` 单独按文件读）。
fn rust_sources_under(root: &Path) -> String {
    fn walk(dir: &Path, out: &mut String) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                if let Ok(text) = fs::read_to_string(&path) {
                    out.push_str(&text.replace("\r\n", "\n"));
                    out.push('\n');
                }
            }
        }
    }
    let mut out = String::new();
    walk(root, &mut out);
    out
}

fn has_fn(sources: &str, name: &str) -> bool {
    sources.contains(&format!("fn {name}("))
}

/// 一条 `tests[]` 引用解不解得开。解不开返回原因。
fn resolve(reference: &str, lib_sources: &str, l3_suite: &str) -> Result<(), String> {
    let (kind, target) = reference
        .split_once(':')
        .ok_or_else(|| format!("{reference:?} 没有 `kind:` 前缀"))?;
    match kind {
        "lib" => has_fn(lib_sources, target)
            .then_some(())
            .ok_or_else(|| format!("src/** 里没有 `fn {target}(`")),
        "test" => {
            let path = repo().join("tests").join(format!("{target}.rs"));
            path.is_file()
                .then_some(())
                .ok_or_else(|| format!("没有 tests/{target}.rs"))
        }
        "l3" => l3_suite
            .contains(&format!("id: \"{target}\","))
            .then_some(())
            .ok_or_else(|| format!("l3_suite 场景表里没有 id {target:?}")),
        "vendor" => {
            let (rel, func) = target
                .split_once("::")
                .ok_or_else(|| format!("vendor 引用 {target:?} 要是 `<相对路径>::<fn>`"))?;
            let path = repo().join("vendor/shared").join(rel);
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("读不到 vendor/shared/{rel}: {error}"))?;
            has_fn(&text, func)
                .then_some(())
                .ok_or_else(|| format!("vendor/shared/{rel} 里没有 `fn {func}(`"))
        }
        "script" | "fixture" => {
            let path = repo().join(target);
            path.is_file()
                .then_some(())
                .ok_or_else(|| format!("没有文件 {target}"))
        }
        "replay" => {
            let path = repo()
                .join("tests/increment_replay/cases")
                .join(format!("{target}.json"));
            path.is_file()
                .then_some(())
                .ok_or_else(|| format!("没有 tests/increment_replay/cases/{target}.json"))
        }
        other => Err(format!("不认识的引用种类 {other:?}（{reference}）")),
    }
}

#[test]
fn the_registry_is_well_formed() {
    let registry = registry();
    assert_eq!(registry.format, "increment-cases-v1");
    assert!(
        repo().join(&registry.plan).is_file(),
        "计划文档不在：{}",
        registry.plan
    );
    assert!(!registry.cases.is_empty());

    let layers: BTreeSet<&str> = ["L0", "L1", "L1'", "L2", "L3", "L4"].into();
    let oracles: BTreeSet<&str> = ["O1", "O2", "O3", "O4", "O5"].into();
    let statuses: BTreeSet<&str> = ["has", "partial", "todo", "corpus"].into();
    let stages: BTreeSet<&str> = ["P0", "P1", "P2", "P3", "P4", "P5"].into();

    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for case in &registry.cases {
        let id = &case.id;
        if !seen.insert(id.clone()) {
            problems.push(format!("{id}: 编号重复"));
        }
        let well_shaped = id.len() == 7
            && id.starts_with("INC-")
            && id.as_bytes()[4].is_ascii_uppercase()
            && id[5..].bytes().all(|b| b.is_ascii_digit());
        if !well_shaped {
            problems.push(format!("{id}: 编号不是 INC-<组><nn>"));
        }
        if &id[4..5] != case.group {
            problems.push(format!("{id}: group {:?} 与编号里的组不一致", case.group));
        }
        if !layers.contains(case.layer.as_str()) {
            problems.push(format!("{id}: layer {:?} 不在定义域", case.layer));
        }
        if case.oracle.is_empty() || case.oracle.iter().any(|o| !oracles.contains(o.as_str())) {
            problems.push(format!("{id}: oracle {:?} 不在 O1–O5 里", case.oracle));
        }
        if !statuses.contains(case.status.as_str()) {
            problems.push(format!("{id}: status {:?} 不在定义域", case.status));
        }
        if case.title.trim().is_empty() {
            problems.push(format!("{id}: 没有标题"));
        }
        match case.status.as_str() {
            "has" if case.tests.is_empty() => {
                problems.push(format!("{id}: status=has 却没有承载测试"));
            }
            "partial" if case.tests.is_empty() => {
                problems.push(format!("{id}: status=partial 却没有已有的承载测试"));
            }
            "partial" | "todo" | "corpus" => match case.stage.as_deref() {
                Some(stage) if stages.contains(stage) => {}
                other => problems.push(format!(
                    "{id}: status={} 要有 P0–P5 的落地段，现在是 {other:?}",
                    case.status
                )),
            },
            _ => {}
        }
    }
    assert!(problems.is_empty(), "登记表形状不对：\n{}", problems.join("\n"));
}

#[test]
fn every_registered_test_exists() {
    let registry = registry();
    let lib_sources = rust_sources_under(&repo().join("src"));
    let l3_suite = fs::read_to_string(repo().join("src/bin/l3_suite.rs"))
        .expect("src/bin/l3_suite.rs")
        .replace("\r\n", "\n");
    let mut problems = Vec::new();
    for case in &registry.cases {
        for reference in &case.tests {
            if let Err(reason) = resolve(reference, &lib_sources, &l3_suite) {
                problems.push(format!("{} → {reference}: {reason}", case.id));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "登记了的测试解不开（被删了、改名了，或表里写错了）：\n{}",
        problems.join("\n")
    );
}

/// 计划 §4 的十组 A–J 都要在表里，每组至少一条——某一组被整组删掉不该是没声音的事。
#[test]
fn every_plan_group_is_registered() {
    let registry = registry();
    let mut per_group: BTreeMap<&str, usize> = BTreeMap::new();
    for case in &registry.cases {
        *per_group.entry(case.group.as_str()).or_default() += 1;
    }
    for group in ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"] {
        assert!(
            per_group.get(group).copied().unwrap_or(0) > 0,
            "组 {group} 一条案例都没有：{per_group:?}"
        );
    }
}
