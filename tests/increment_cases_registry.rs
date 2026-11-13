//! 增量更新自动化测试案例登记表的钉（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §5.1，P0）。
//!
//! `tests/fixtures/increment_cases.json` 是「案例在哪一层、认哪条裁判、由哪些测试承载」的单一事实。
//! 这里钉三件事，都不跑任何库、不起任何进程：
//!
//! 1. 表本身形状对：编号唯一且与组一致、层 / 裁判 / 状态取值在定义域里、`todo` / `corpus` / `partial` 都有落地段；
//! 2. **登记了的测试真的存在**：`lib:` 在 `src/**` 里有同名 `fn`，`test:` 在 `tests/` 下有同名文件，
//!    `l3:` 在 `l3_suite` 的场景表里，`vendor:` 在 `vendor/shared` 下有文件且含同名 `fn`，`script:` / `fixture:` 文件在，
//!    `replay:` 在 `tests/increment_replay/cases/` 下有同名用例。被删 / 改名的测试在这里先红，不让登记表慢慢说谎。
//! 3. **变更类型与执行资产对得上**：`change_types` 只用变更类型总表
//!    （`docs/specs/incr-gen-audit-aux/increment-change-types.md`）里有的编号；`assets` 的 `suite:` 用例在
//!    `scripts/e3d/increment_suite/suite-manifest.json` 里，每个清单用例恰好挂在一条案例下、且就是它
//!    `registration` 指的那条。`inplace:`（原地宏对）的清单未入库，只查形状。
//! 4. **清单片段站得住**：`scripts/e3d/increment_suite/manifest.d/*.json` 是套件用例的第二个来源（新用例先落在
//!    片段里，还没有挂到登记表的案例下，所以第 3 条看不见它们）。片段里的用例号全套唯一、宏文件都在、
//!    变更类型是总表里有的、状态与净效果取值在定义域里；写了 `registration` 的，指的案例在登记表里。
//!    与 `scripts/e3d/increment_suite/SuiteManifest.ps1` 的核对是同一套规矩。
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
    #[serde(default)]
    change_types: Vec<String>,
    #[serde(default)]
    assets: Vec<String>,
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

/// 变更类型总表（`docs/specs/incr-gen-audit-aux/increment-change-types.md`）各族的最大编号。
/// 2026-11-12：D 族加了 CT-D11（被支撑的管件换规格 → 支撑件重算，gen-model #143），总表共 45 类。
const CHANGE_TYPE_FAMILIES: [(char, u32); 5] =
    [('S', 7), ('A', 13), ('C', 4), ('D', 11), ('W', 10)];

fn is_known_change_type(code: &str) -> bool {
    let Some(rest) = code.strip_prefix("CT-") else {
        return false;
    };
    let mut chars = rest.chars();
    let Some(family) = chars.next() else {
        return false;
    };
    let digits = chars.as_str();
    let well_shaped = (1..=2).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit());
    let Ok(number) = digits.parse::<u32>() else {
        return false;
    };
    well_shaped
        && CHANGE_TYPE_FAMILIES
            .iter()
            .any(|&(name, max)| name == family && (1..=max).contains(&number))
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
    assert!(
        problems.is_empty(),
        "登记表形状不对：\n{}",
        problems.join("\n")
    );
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

/// 计划 §4 的各组（A–J 与原地宏对 U）都要在表里，每组至少一条——某一组被整组删掉不该是没声音的事。
#[test]
fn every_plan_group_is_registered() {
    let registry = registry();
    let mut per_group: BTreeMap<&str, usize> = BTreeMap::new();
    for case in &registry.cases {
        *per_group.entry(case.group.as_str()).or_default() += 1;
    }
    for group in ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "U"] {
        assert!(
            per_group.get(group).copied().unwrap_or(0) > 0,
            "组 {group} 一条案例都没有：{per_group:?}"
        );
    }
}

/// `change_types` 只用总表里有的编号；`suite:` 资产对得上套件清单：清单里每个用例恰好挂在一条案例下，
/// 而且就是它 `registration` 指的那条——清单和登记表只改了一边时这里先红。
#[test]
fn change_types_and_assets_are_consistent() {
    #[derive(Deserialize)]
    struct Suite {
        cases: Vec<SuiteCase>,
    }
    #[derive(Deserialize)]
    struct SuiteCase {
        id: String,
        registration: String,
    }

    let registry = registry();
    let text = fs::read_to_string(repo().join("scripts/e3d/increment_suite/suite-manifest.json"))
        .expect("read scripts/e3d/increment_suite/suite-manifest.json");
    let suite: Suite = serde_json::from_str(&text).expect("suite-manifest.json parses");
    let registration: BTreeMap<&str, &str> = suite
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case.registration.as_str()))
        .collect();

    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut problems = Vec::new();
    for case in &registry.cases {
        let id = case.id.as_str();
        let mut seen = BTreeSet::new();
        for code in &case.change_types {
            if !is_known_change_type(code) {
                problems.push(format!("{id}: 变更类型 {code:?} 不是总表里的 CT-<族><n>"));
            }
            if !seen.insert(code.as_str()) {
                problems.push(format!("{id}: 变更类型 {code} 重复"));
            }
        }
        for asset in &case.assets {
            match asset.split_once(':') {
                Some(("suite", name)) => {
                    owners.entry(name).or_default().push(id);
                    match registration.get(name) {
                        None => problems.push(format!("{id}: 套件清单里没有用例 {name:?}")),
                        Some(reg) if id.strip_prefix("INC-") != Some(*reg) => {
                            problems.push(format!(
                                "{id}: 套件用例 {name} 登记在 {reg}，与挂靠的案例不一致"
                            ));
                        }
                        Some(_) => {}
                    }
                }
                Some(("inplace", name)) if !name.is_empty() => {}
                _ => problems.push(format!(
                    "{id}: 资产 {asset:?} 要是 `suite:<用例>` 或 `inplace:<宏对>`"
                )),
            }
        }
    }
    for (name, reg) in &registration {
        match owners.get(*name).map(Vec::as_slice) {
            Some([_]) => {}
            None => problems.push(format!(
                "套件用例 {name}（registration {reg}）没有挂到任何案例的 assets"
            )),
            Some(many) => problems.push(format!("套件用例 {name} 挂在多条案例下：{many:?}")),
        }
    }
    assert!(
        problems.is_empty(),
        "变更类型 / 执行资产对不上：\n{}",
        problems.join("\n")
    );
}

/// 片段里一条腿：宏的相对路径，可以带 `@库号` 或 `@库号+库号`。
fn fragment_macro_path(leg: &str) -> &str {
    leg.split_once('@').map_or(leg, |(path, _)| path)
}

/// `"x"` → `["x"]`，`["a", "b"]` → 两条，其余（`null`、缺席）→ 空。
fn fragment_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(text) => vec![text.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// 片段的用例号：变更类型的格号（不带 `CT-`、不补零）加短名，小写字母、数字、短横。
fn is_fragment_case_id(id: &str) -> bool {
    let Some((cell, short)) = id.split_once('-') else {
        return false;
    };
    is_known_change_type(&format!("CT-{cell}"))
        && !short.is_empty()
        && !short.starts_with('-')
        && short
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 清单片段（`manifest.d/*.json`）自身站得住：用例号全套唯一（连主清单一起）、宏文件都在、变更类型与取值在定义域里。
/// 新用例先落在片段里、还没挂到登记表的案例下，所以这里不要求它们出现在 `assets` 里；写了 `registration` 的要指得到。
/// 规矩与 `scripts/e3d/increment_suite/SuiteManifest.ps1` 的核对一致，两边要一起改。
#[test]
fn manifest_fragments_are_well_formed() {
    use serde_json::Value;

    let suite_dir = repo().join("scripts/e3d/increment_suite");
    let main_text = fs::read_to_string(suite_dir.join("suite-manifest.json"))
        .expect("read scripts/e3d/increment_suite/suite-manifest.json");
    let main: Value = serde_json::from_str(&main_text).expect("suite-manifest.json parses");
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for case in main["cases"].as_array().into_iter().flatten() {
        if let Some(id) = case["id"].as_str() {
            seen.insert(id.to_string(), "suite-manifest.json".to_string());
        }
    }
    let registry = registry();
    let registered: BTreeSet<&str> = registry.cases.iter().map(|case| case.id.as_str()).collect();
    let nets = ["modified", "cancelled", "deleted", "unchanged", "added"];
    let statuses = ["draft", "macro_ok", "verified", "known_gap"];
    let macro_exists = |leg: &str| suite_dir.join(fragment_macro_path(leg)).is_file();

    let mut files: Vec<PathBuf> = fs::read_dir(suite_dir.join("manifest.d"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();

    let mut problems = Vec::new();
    for path in &files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{name}: 读不到：{error}"));
                continue;
            }
        };
        let fragment: Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
            Ok(value) => value,
            Err(error) => {
                problems.push(format!("{name}: JSON 解析失败：{error}"));
                continue;
            }
        };
        let prefix = name.split_once('-').map_or("", |(prefix, _)| prefix);
        let prefix_ok = prefix.len() >= 2
            && prefix.starts_with('T')
            && prefix[1..].bytes().all(|b| b.is_ascii_digit());
        if !prefix_ok {
            problems.push(format!("{name}: 文件名要是 <T1|T2|…>-<主题>.json"));
        }
        if fragment["owner"].as_str() != Some(prefix) {
            problems.push(format!(
                "{name}: owner {:?} 与文件名前缀 {prefix:?} 不一致",
                fragment["owner"]
            ));
        }
        if fragment["version"].as_u64() != Some(1) {
            problems.push(format!("{name}: version 要是 1"));
        }
        let copy = fragment["copy"].as_str().unwrap_or("ams");
        if !["ams", "aps"].contains(&copy) {
            problems.push(format!("{name}: copy {copy:?} 不认识（ams / aps）"));
        }
        let mut chains: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for chain in fragment["chains"].as_array().into_iter().flatten() {
            let id = chain["id"].as_str().unwrap_or_default().to_string();
            let legs = fragment_strings(&chain["legs"]);
            for leg in &legs {
                if !macro_exists(leg) {
                    problems.push(format!("{name}: 链 {id} 的宏不存在 {leg}"));
                }
            }
            chains.insert(id, legs);
        }
        for key in ["setup", "teardown"] {
            for leg in fragment_strings(&fragment[key]) {
                if !macro_exists(&leg) {
                    problems.push(format!("{name}: {key} 的宏不存在 {leg}"));
                }
            }
        }
        for probe in fragment["probes"].as_array().into_iter().flatten() {
            let leg = probe["macro"].as_str().unwrap_or_default();
            if !macro_exists(leg) {
                problems.push(format!("{name}: 探针宏不存在 {leg:?}"));
            }
        }
        for case in fragment["cases"].as_array().into_iter().flatten() {
            let id = case["id"].as_str().unwrap_or_default();
            let place = format!("{name} 的 {id}");
            if !is_fragment_case_id(id) {
                problems.push(format!(
                    "{place}: 用例号要是 <格号>-<短名>（如 D5-pjoi-only）"
                ));
            }
            if let Some(other) = seen.insert(id.to_string(), name.clone()) {
                problems.push(format!("{place}: 用例号和 {other} 里的撞了"));
            }
            let db_ok = match &case["db"] {
                Value::Number(number) => number.is_u64(),
                Value::Array(items) => !items.is_empty() && items.iter().all(Value::is_u64),
                _ => false,
            };
            if !db_ok {
                problems.push(format!("{place}: db 要是库号或库号数组"));
            }
            let mut legs = Vec::new();
            match &case["chain"] {
                Value::String(chain) => match chains.get(chain) {
                    Some(defined) => legs.extend(defined.iter().cloned()),
                    None => {
                        problems.push(format!("{place}: 引用的链 {chain} 不在本片段的 chains 里"))
                    }
                },
                Value::Array(_) => legs.extend(fragment_strings(&case["chain"])),
                _ => {
                    if case["apply"].as_str().is_none() {
                        problems.push(format!("{place}: 没有 apply，也没有 chain"));
                    }
                }
            }
            legs.extend(fragment_strings(&case["apply"]));
            legs.extend(fragment_strings(&case["restore"]));
            for leg in &legs {
                if !macro_exists(leg) {
                    problems.push(format!("{place}: 宏不存在 {leg}"));
                }
            }
            let change_types = fragment_strings(&case["change_types"]);
            if change_types.is_empty() {
                problems.push(format!("{place}: 没有 change_types"));
            }
            for code in &change_types {
                if !is_known_change_type(code) {
                    problems.push(format!(
                        "{place}: 变更类型 {code:?} 不是总表里的 CT-<族><n>"
                    ));
                }
            }
            let net = case["expected_net"].as_str().unwrap_or_default();
            if !nets.contains(&net) {
                problems.push(format!("{place}: expected_net {net:?} 不在定义域"));
            }
            let status = case["status"].as_str().unwrap_or_default();
            if !statuses.contains(&status) {
                problems.push(format!("{place}: status {status:?} 不在定义域"));
            }
            if status == "known_gap" && case["known_gap"].is_null() {
                problems.push(format!("{place}: status=known_gap 要写 known_gap"));
            }
            let registration = case["registration"].as_str().unwrap_or_default();
            if !registration.is_empty() {
                let inc = if registration.starts_with("INC-") {
                    registration.to_string()
                } else {
                    format!("INC-{registration}")
                };
                if !registered.contains(inc.as_str()) {
                    problems.push(format!(
                        "{place}: registration {registration:?} 不是登记表里的案例"
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "清单片段站不住：\n{}",
        problems.join("\n")
    );
}
