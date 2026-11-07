//! 把逐件比对（`e3d_model::rvm_compare::CompareReport`，落盘为 `compare.json`）收成页面用的一页报告。
//!
//! 颜色不看进程退出码：配上了一部分、同时又有 `only_*` 时，那个进程仍可能是 0，
//! 差异只在 JSON 里。口径与 `scripts/Compare-SlykRvmStrict.ps1` 相同：
//! 没有缺件、没有 off、种类也一致是绿；只是种类不一致是黄；其余是红。

use e3d_model::rvm_compare::CompareReport;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERDICT_PASS: &str = "pass";
pub const VERDICT_KIND: &str = "kind_differ";
pub const VERDICT_FAIL: &str = "fail";

/// 页面上的三色。`Fail` 同时覆盖「比对发现差异」和「某一步没跑完」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RvmVerdict {
    Pass,
    KindDiffer,
    Fail,
}

impl RvmVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => VERDICT_PASS,
            Self::KindDiffer => VERDICT_KIND,
            Self::Fail => VERDICT_FAIL,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RvmCounts {
    pub both: u64,
    pub exact: u64,
    pub chord: u64,
    pub reference_artifact: u64,
    pub film: u64,
    pub off: u64,
    pub only_baseline: u64,
    pub only_generated: u64,
    pub kind_match: u64,
    pub kind_differ: u64,
    pub container_off: u64,
    pub container_only_baseline: u64,
    pub container_only_generated: u64,
    pub centreline_only_baseline: u64,
    pub unkeyable_baseline: u64,
    pub unkeyable_generated: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RvmMismatch {
    /// `off` / `kind_differ` / `only_baseline` / `only_generated` /
    /// `unkeyable_baseline` / `unkeyable_generated` / `container_off` /
    /// `container_only_baseline` / `container_only_generated`。
    pub category: String,
    pub name: String,
    pub noun: String,
    pub max_axis_delta_mm: Option<f64>,
    pub baseline_kinds: Vec<String>,
    pub generated_kinds: Vec<String>,
    pub baseline_aabb: Option<[f64; 6]>,
    pub generated_aabb: Option<[f64; 6]>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RvmTimingsMs {
    pub resolve: u64,
    pub generate_ours: u64,
    pub e3d_export: u64,
    pub compare: u64,
}

impl Default for RvmTimingsMs {
    fn default() -> Self {
        Self {
            resolve: 0,
            generate_ours: 0,
            e3d_export: 0,
            compare: 0,
        }
    }
}

/// 一次比对的全部给页面看的东西。步骤失败时 `counts` 是空的，`message` 说明停在哪。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RvmCheckReport {
    pub verdict: String,
    pub message: String,
    /// 停在或走完的阶段：`resolve` / `generate_ours` / `e3d_export` / `compare`。
    pub stage: String,
    pub refno: String,
    pub name: Option<String>,
    pub noun: Option<String>,
    pub dbnum: Option<u32>,
    pub db_type: Option<String>,
    pub sesno: Option<u32>,
    pub db_file: Option<String>,
    /// `refno` 或 `name`：E3D 最终是按哪种写法导出的。
    pub e3d_export_by: Option<String>,
    pub counts: RvmCounts,
    pub mismatches: Vec<RvmMismatch>,
    pub mismatches_truncated: bool,
    pub centreline_only_baseline: Vec<String>,
    pub timings_ms: RvmTimingsMs,
    pub files: Vec<String>,
    pub warnings: Vec<String>,
}

const MISMATCH_LIMIT: usize = 400;

pub fn failed_report(
    stage: &str,
    refno: &str,
    message: impl Into<String>,
    timings: RvmTimingsMs,
) -> RvmCheckReport {
    RvmCheckReport {
        verdict: VERDICT_FAIL.to_string(),
        message: message.into(),
        stage: stage.to_string(),
        refno: refno.to_string(),
        name: None,
        noun: None,
        dbnum: None,
        db_type: None,
        sesno: None,
        db_file: None,
        e3d_export_by: None,
        counts: RvmCounts::default(),
        mismatches: Vec::new(),
        mismatches_truncated: false,
        centreline_only_baseline: Vec::new(),
        timings_ms: timings,
        files: Vec::new(),
        warnings: Vec::new(),
    }
}

/// 判色要从一份比对报告里读的全部事实。两种来源摊成同一份：落盘的 `compare.json`
/// （[`JudgeFacts::from_value`]，宽松读法，缺字段按空算）与进程内刚算完的
/// [`CompareReport`]（[`JudgeFacts::from_report`]，不必先序列化成 `Value` 再读回来——
/// 80k 件的报告那棵树要几百 MB）。
struct JudgeFacts {
    /// 按 noun 的账合计（`both` … `kind_differ`），`only_*` 还没与清单长度取大。
    counts: RvmCounts,
    only_baseline: Vec<String>,
    only_generated: Vec<String>,
    unkeyable_baseline: Vec<String>,
    unkeyable_generated: Vec<String>,
    centreline: Vec<String>,
    /// `baseline_members` 在且为 0 才算「E3D 一侧确实读过、而且是空的」。
    baseline_members_zero: bool,
    /// 只收有问题的件：`off`、种类不一致、或容器组内逐根账里有 off / only_*。
    problem_pairs: Vec<PairFacts>,
}

struct PairFacts {
    name: String,
    noun: String,
    verdict: String,
    kinds_differ: bool,
    max_axis_delta_mm: Option<f64>,
    baseline_kinds: Vec<String>,
    generated_kinds: Vec<String>,
    baseline_aabb: Option<[f64; 6]>,
    generated_aabb: Option<[f64; 6]>,
    /// 容器组内逐根账里有问题的行：（我们那根的名字、行的判档、差）。
    container_rows: Vec<(Option<String>, String, Option<f64>)>,
}

fn is_problem_row(verdict: &str) -> bool {
    matches!(verdict, "off" | "only_baseline" | "only_generated")
}

impl JudgeFacts {
    fn from_value(value: &Value) -> Self {
        let mut counts = RvmCounts::default();
        if let Some(by_noun) = value.get("by_noun").and_then(Value::as_object) {
            for tally in by_noun.values() {
                counts.both += num(tally, "both");
                counts.exact += num(tally, "exact");
                counts.chord += num(tally, "chord");
                counts.reference_artifact += num(tally, "reference_artifact");
                counts.film += num(tally, "film");
                counts.off += num(tally, "off");
                counts.only_baseline += num(tally, "only_baseline");
                counts.only_generated += num(tally, "only_generated");
                counts.kind_match += num(tally, "kind_match");
                counts.kind_differ += num(tally, "kind_differ");
            }
        }
        if let Some(tubes) = value.get("container_tubes") {
            counts.container_off = num(tubes, "off");
            counts.container_only_baseline = num(tubes, "only_baseline");
            counts.container_only_generated = num(tubes, "only_generated");
        }
        let mut problem_pairs = Vec::new();
        if let Some(pairs) = value.get("pairs").and_then(Value::as_array) {
            for pair in pairs {
                let verdict = pair.get("verdict").and_then(Value::as_str).unwrap_or("");
                let kinds_differ = pair.get("kind_match").and_then(Value::as_bool) == Some(false);
                let container_rows: Vec<(Option<String>, String, Option<f64>)> = pair
                    .get("container_tubes")
                    .and_then(|tubes| tubes.get("rows"))
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| {
                                let row_verdict =
                                    row.get("verdict").and_then(Value::as_str).unwrap_or("");
                                is_problem_row(row_verdict).then(|| {
                                    (
                                        row.get("generated")
                                            .and_then(Value::as_str)
                                            .map(str::to_string),
                                        row_verdict.to_string(),
                                        row.get("max_axis_delta_mm").and_then(Value::as_f64),
                                    )
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if verdict != "off" && !kinds_differ && container_rows.is_empty() {
                    continue;
                }
                problem_pairs.push(PairFacts {
                    name: pair
                        .get("key")
                        .and_then(Value::as_str)
                        .or_else(|| pair.get("refno").and_then(Value::as_str))
                        .unwrap_or("?")
                        .to_string(),
                    noun: pair
                        .get("noun")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    verdict: verdict.to_string(),
                    kinds_differ,
                    max_axis_delta_mm: pair.get("max_axis_delta_mm").and_then(Value::as_f64),
                    baseline_kinds: string_array(pair.get("baseline_kinds")),
                    generated_kinds: string_array(pair.get("generated_kinds")),
                    baseline_aabb: arr6(pair.get("baseline_aabb")),
                    generated_aabb: arr6(pair.get("generated_aabb")),
                    container_rows,
                });
            }
        }
        Self {
            counts,
            only_baseline: string_list(value, "only_baseline"),
            only_generated: string_list(value, "only_generated"),
            unkeyable_baseline: string_list(value, "unkeyable_baseline"),
            unkeyable_generated: string_list(value, "unkeyable_generated"),
            centreline: string_list(value, "centreline_only_baseline"),
            baseline_members_zero: value.get("baseline_members").and_then(Value::as_u64) == Some(0),
            problem_pairs,
        }
    }

    fn from_report(report: &CompareReport) -> Self {
        let mut counts = RvmCounts::default();
        for tally in report.by_noun.values() {
            counts.both += tally.both as u64;
            counts.exact += tally.exact as u64;
            counts.chord += tally.chord as u64;
            counts.reference_artifact += tally.reference_artifact as u64;
            counts.film += tally.film as u64;
            counts.off += tally.off as u64;
            counts.only_baseline += tally.only_baseline as u64;
            counts.only_generated += tally.only_generated as u64;
            counts.kind_match += tally.kind_match as u64;
            counts.kind_differ += tally.kind_differ as u64;
        }
        counts.container_off = report.container_tubes.off as u64;
        counts.container_only_baseline = report.container_tubes.only_baseline as u64;
        counts.container_only_generated = report.container_tubes.only_generated as u64;
        let problem_pairs = report
            .pairs
            .iter()
            .filter_map(|pair| {
                let kinds_differ = pair.kind_match == Some(false);
                let container_rows: Vec<(Option<String>, String, Option<f64>)> = pair
                    .container_tubes
                    .as_ref()
                    .map(|tubes| {
                        tubes
                            .rows
                            .iter()
                            .filter(|row| is_problem_row(row.verdict))
                            .map(|row| {
                                (
                                    row.generated.clone(),
                                    row.verdict.to_string(),
                                    row.max_axis_delta_mm,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if pair.verdict != "off" && !kinds_differ && container_rows.is_empty() {
                    return None;
                }
                Some(PairFacts {
                    name: pair.key.clone(),
                    noun: pair.noun.clone(),
                    verdict: pair.verdict.to_string(),
                    kinds_differ,
                    max_axis_delta_mm: Some(pair.max_axis_delta_mm),
                    baseline_kinds: pair.baseline_kinds.clone(),
                    generated_kinds: pair.generated_kinds.clone(),
                    baseline_aabb: Some(pair.baseline_aabb),
                    generated_aabb: Some(pair.generated_aabb),
                    container_rows,
                })
            })
            .collect();
        Self {
            counts,
            only_baseline: report.only_baseline.clone(),
            only_generated: report.only_generated.clone(),
            unkeyable_baseline: report.unkeyable_baseline.clone(),
            unkeyable_generated: report.unkeyable_generated.clone(),
            centreline: report.centreline_only_baseline.clone(),
            baseline_members_zero: report.baseline_members == Some(0),
            problem_pairs,
        }
    }
}

/// 从 `rvm_compare` 的 JSON 取值。缺 `by_noun` 或一件都没配上，判红。
pub fn judge_compare(
    value: &Value,
) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
    judge(JudgeFacts::from_value(value))
}

/// 进程内刚算完的报告直接判色，口径与 [`judge_compare`] 读落盘 JSON 完全一致。
pub fn judge_report(
    report: &CompareReport,
) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
    judge(JudgeFacts::from_report(report))
}

fn judge(facts: JudgeFacts) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
    let JudgeFacts {
        mut counts,
        only_baseline,
        only_generated,
        unkeyable_baseline,
        unkeyable_generated,
        centreline,
        baseline_members_zero,
        problem_pairs,
    } = facts;
    counts.only_baseline = counts.only_baseline.max(only_baseline.len() as u64);
    counts.only_generated = counts.only_generated.max(only_generated.len() as u64);
    counts.unkeyable_baseline = unkeyable_baseline.len() as u64;
    counts.unkeyable_generated = unkeyable_generated.len() as u64;
    counts.centreline_only_baseline = centreline.len() as u64;

    let mut mismatches = Vec::new();
    for pair in &problem_pairs {
        if pair.verdict == "off" {
            mismatches.push(pair_mismatch(pair, "off"));
        } else if pair.kinds_differ {
            mismatches.push(pair_mismatch(pair, "kind_differ"));
        }
        for (generated, row_verdict, max_axis_delta_mm) in &pair.container_rows {
            mismatches.push(RvmMismatch {
                category: format!("container_{row_verdict}"),
                name: generated
                    .clone()
                    .unwrap_or_else(|| "(容器管身)".to_string()),
                noun: "TUBI".to_string(),
                max_axis_delta_mm: *max_axis_delta_mm,
                baseline_kinds: Vec::new(),
                generated_kinds: Vec::new(),
                baseline_aabb: None,
                generated_aabb: None,
            });
        }
    }
    for name in &only_baseline {
        mismatches.push(named("only_baseline", name));
    }
    for name in &only_generated {
        mismatches.push(named("only_generated", name));
    }
    for name in &unkeyable_baseline {
        mismatches.push(named("unkeyable_baseline", name));
    }
    for name in &unkeyable_generated {
        mismatches.push(named("unkeyable_generated", name));
    }

    let truncated = mismatches.len() > MISMATCH_LIMIT;
    mismatches.truncate(MISMATCH_LIMIT);

    let structural = counts.off
        + counts.only_baseline
        + counts.only_generated
        + counts.container_off
        + counts.container_only_baseline
        + counts.container_only_generated
        + counts.unkeyable_baseline
        + counts.unkeyable_generated;
    // E3D 一侧确实读过（`baseline_members` 在且为 0）、生成侧也一件没出，两边同样空：
    // 只画中心线的 SITE（/1AXIS 的 SCTN 没有 SPRE）就是这样，不是对不上。缺这个字段的
    // JSON 说明比对本身没跑完整，仍判红。
    let both_empty =
        baseline_members_zero && counts.both == 0 && counts.kind_differ == 0 && structural == 0;
    let verdict = if both_empty {
        RvmVerdict::Pass
    } else if counts.both == 0 || structural > 0 {
        RvmVerdict::Fail
    } else if counts.kind_differ > 0 {
        RvmVerdict::KindDiffer
    } else {
        RvmVerdict::Pass
    };
    (verdict, counts, mismatches, centreline, truncated)
}

pub fn verdict_message(verdict: RvmVerdict, counts: &RvmCounts) -> String {
    match verdict {
        RvmVerdict::Pass if counts.both == 0 => "通过：两侧都没有可比几何".to_string(),
        RvmVerdict::Pass => format!("通过：两侧 {} 个构件一致", counts.both),
        RvmVerdict::KindDiffer => format!(
            "包围盒通过，但有 {} 个构件的图元种类不一致",
            counts.kind_differ
        ),
        RvmVerdict::Fail
            if counts.both == 0 && counts.only_baseline == 0 && counts.only_generated == 0 =>
        {
            "没有配上任何构件".to_string()
        }
        RvmVerdict::Fail => format!(
            "不一致：偏差 {}，仅 E3D {}，仅生成 {}，种类不同 {}",
            counts.off + counts.container_off,
            counts.only_baseline + counts.container_only_baseline,
            counts.only_generated + counts.container_only_generated,
            counts.kind_differ
        ),
    }
}

fn pair_mismatch(pair: &PairFacts, category: &str) -> RvmMismatch {
    RvmMismatch {
        category: category.to_string(),
        name: pair.name.clone(),
        noun: pair.noun.clone(),
        max_axis_delta_mm: pair.max_axis_delta_mm,
        baseline_kinds: pair.baseline_kinds.clone(),
        generated_kinds: pair.generated_kinds.clone(),
        baseline_aabb: pair.baseline_aabb,
        generated_aabb: pair.generated_aabb,
    }
}

fn named(category: &str, name: &str) -> RvmMismatch {
    RvmMismatch {
        category: category.to_string(),
        name: name.to_string(),
        noun: String::new(),
        max_axis_delta_mm: None,
        baseline_kinds: Vec::new(),
        generated_kinds: Vec::new(),
        baseline_aabb: None,
        generated_aabb: None,
    }
}

fn num(value: &Value, key: &str) -> u64 {
    value.get(key).map(as_u64).unwrap_or(0)
}

fn as_u64(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|number| number as u64))
        .unwrap_or(0)
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    string_array(value.get(key))
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn arr6(value: Option<&Value>) -> Option<[f64; 6]> {
    let items = value?.as_array()?;
    if items.len() != 6 {
        return None;
    }
    let mut out = [0.0; 6];
    for (index, item) in items.iter().enumerate() {
        out[index] = item.as_f64()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(exact: u64, kind_differ: u64, off: u64, only_b: u64, only_g: u64) -> Value {
        serde_json::json!({
            "both": exact + off,
            "exact": exact,
            "chord": 0,
            "reference_artifact": 0,
            "film": 0,
            "off": off,
            "only_baseline": only_b,
            "only_generated": only_g,
            "kind_match": exact.saturating_sub(kind_differ),
            "kind_differ": kind_differ
        })
    }

    #[test]
    fn green_when_every_paired_member_matches() {
        let value = serde_json::json!({
            "by_noun": { "BRAN": tally(4, 0, 0, 0, 0), "": tally(2, 0, 0, 0, 0) },
            "pairs": [],
            "only_baseline": [],
            "only_generated": [],
            "container_tubes": { "off": 0, "only_baseline": 0, "only_generated": 0 },
            "centreline_only_baseline": ["centre"],
            "unkeyable_baseline": [],
            "unkeyable_generated": []
        });
        let (verdict, counts, mismatches, centreline, _) = judge_compare(&value);
        assert_eq!(verdict, RvmVerdict::Pass);
        assert_eq!(counts.both, 6);
        assert_eq!(counts.exact, 6);
        assert!(mismatches.is_empty());
        assert_eq!(centreline, vec!["centre".to_string()]);
    }

    #[test]
    fn yellow_when_only_the_primitive_kinds_differ() {
        let value = serde_json::json!({
            "by_noun": { "BOX": tally(3, 1, 0, 0, 0) },
            "pairs": [{
                "key": "/NUT",
                "noun": "BOX",
                "verdict": "exact",
                "kind_match": false,
                "max_axis_delta_mm": 0.0,
                "baseline_kinds": ["Cylinder"],
                "generated_kinds": ["Box"]
            }],
            "only_baseline": [],
            "only_generated": [],
            "container_tubes": {}
        });
        let (verdict, counts, mismatches, _, _) = judge_compare(&value);
        assert_eq!(verdict, RvmVerdict::KindDiffer);
        assert_eq!(counts.kind_differ, 1);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].category, "kind_differ");
        assert_eq!(mismatches[0].baseline_kinds, vec!["Cylinder".to_string()]);
    }

    #[test]
    fn red_when_a_member_is_off_or_one_sided() {
        let off = serde_json::json!({
            "by_noun": { "ELBO": tally(1, 0, 1, 0, 0) },
            "pairs": [{
                "key": "/BEND",
                "noun": "ELBO",
                "verdict": "off",
                "kind_match": true,
                "max_axis_delta_mm": 12.5,
                "baseline_aabb": [0, 0, 0, 1, 1, 1],
                "generated_aabb": [0, 0, 0, 10, 1, 1]
            }],
            "only_baseline": [],
            "only_generated": []
        });
        let (verdict, _, mismatches, _, _) = judge_compare(&off);
        assert_eq!(verdict, RvmVerdict::Fail);
        assert_eq!(mismatches[0].max_axis_delta_mm, Some(12.5));
        assert_eq!(mismatches[0].generated_aabb.unwrap()[3], 10.0);

        let missing = serde_json::json!({
            "by_noun": { "BRAN": tally(2, 0, 0, 1, 0) },
            "only_baseline": ["/ONLY-E3D"],
            "only_generated": []
        });
        assert_eq!(judge_compare(&missing).0, RvmVerdict::Fail);

        let extra = serde_json::json!({
            "by_noun": { "BRAN": tally(2, 0, 0, 0, 0) },
            "only_baseline": [],
            "only_generated": ["/ONLY-OURS"]
        });
        let (verdict, counts, mismatches, _, _) = judge_compare(&extra);
        assert_eq!(verdict, RvmVerdict::Fail);
        assert_eq!(counts.only_generated, 1);
        assert!(mismatches.iter().any(|row| row.name == "/ONLY-OURS"));
    }

    #[test]
    fn red_when_container_tubes_or_nothing_paired() {
        let tubes = serde_json::json!({
            "by_noun": { "BRAN": tally(1, 0, 0, 0, 0) },
            "container_tubes": { "off": 0, "only_baseline": 2, "only_generated": 0 },
            "only_baseline": [],
            "only_generated": []
        });
        assert_eq!(judge_compare(&tubes).0, RvmVerdict::Fail);
        assert_eq!(judge_compare(&serde_json::json!({})).0, RvmVerdict::Fail);
    }

    /// /1AXIS、/Steel_Template_Site、/MDS/TEMPLATES/ORI 的 compare.json：两边都没有带体的成员。
    #[test]
    fn green_when_both_sides_have_nothing_comparable() {
        let empty = serde_json::json!({
            "baseline_members": 0,
            "baseline_geometries": 0,
            "by_noun": {},
            "pairs": [],
            "only_baseline": [],
            "only_generated": [],
            "unkeyable_baseline": [],
            "unkeyable_generated": [],
            "centreline_only_baseline": [],
            "container_tube_groups": 0,
            "container_tubes": { "exact": 0, "chord": 0, "off": 0, "only_baseline": 0, "only_generated": 0 }
        });
        let (verdict, counts, mismatches, _, _) = judge_compare(&empty);
        assert_eq!(verdict, RvmVerdict::Pass);
        assert!(mismatches.is_empty());
        assert_eq!(verdict_message(verdict, &counts), "通过：两侧都没有可比几何");

        let mut ours_only = empty.clone();
        ours_only["only_generated"] = serde_json::json!(["/ONLY-OURS"]);
        assert_eq!(judge_compare(&ours_only).0, RvmVerdict::Fail);

        let mut unread = empty.clone();
        unread.as_object_mut().unwrap().remove("baseline_members");
        assert_eq!(judge_compare(&unread).0, RvmVerdict::Fail);
    }

    /// `judge_compare` 只吃 `compare.json` 的内容：函数签名里压根没有退出码这个
    /// 参数。`rvm_compare.exe` 可能退出 0 但 JSON 里全是 only_*（部分配上一半、
    /// 进程本身没崩），也可能假设退出非 0 但 JSON 仍写着干净的 pass 数据——两种
    /// 情况下判定都必须只跟着 JSON 走，不能被进程状态污染。
    #[test]
    fn verdict_follows_compare_json_content_not_process_exit_code() {
        let exit_zero_but_content_says_fail = serde_json::json!({
            "by_noun": { "BRAN": tally(2, 0, 0, 1, 0) },
            "pairs": [],
            "only_baseline": ["/ONLY-E3D"],
            "only_generated": []
        });
        let (verdict, counts, mismatches, _, _) = judge_compare(&exit_zero_but_content_says_fail);
        assert_eq!(
            verdict,
            RvmVerdict::Fail,
            "进程退出码可以是 0，内容里有 only_baseline 仍必须判 fail"
        );
        assert_eq!(counts.only_baseline, 1);
        assert!(mismatches.iter().any(|row| row.name == "/ONLY-E3D"));

        let exit_nonzero_shaped_but_content_says_pass = serde_json::json!({
            "by_noun": { "BRAN": tally(3, 0, 0, 0, 0) },
            "pairs": [],
            "only_baseline": [],
            "only_generated": []
        });
        let (verdict, counts, mismatches, _, _) =
            judge_compare(&exit_nonzero_shaped_but_content_says_pass);
        assert_eq!(
            verdict,
            RvmVerdict::Pass,
            "JSON 内容干净时就该判 pass，不看进程是否被认为失败"
        );
        assert_eq!(counts.both, 3);
        assert!(mismatches.is_empty());

        // 同一份 fail 数据改一个字段（`pairs` 里加一条 verdict:"off"）就从 fail
        // 的某一类变成另一类差异，证明判定输入确实是 JSON 内容在变，不是外部状态。
        let mut mutated = exit_zero_but_content_says_fail.clone();
        mutated["only_baseline"] = serde_json::json!([]);
        mutated["pairs"] = serde_json::json!([{
            "key": "/BEND",
            "noun": "ELBO",
            "verdict": "off",
            "kind_match": true,
            "max_axis_delta_mm": 12.5
        }]);
        mutated["by_noun"] = serde_json::json!({ "ELBO": tally(1, 0, 1, 0, 0) });
        let (verdict, _, mismatches, _, _) = judge_compare(&mutated);
        assert_eq!(verdict, RvmVerdict::Fail);
        assert_eq!(mismatches[0].category, "off");
    }

    #[test]
    fn a_real_passing_compare_file_stays_green_when_present() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".cutb-backup-7999-dirty/compare.json");
        if !path.is_file() {
            eprintln!("skip real compare.json: {}", path.display());
            return;
        }
        let bytes = std::fs::read(&path).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let (verdict, counts, mismatches, _, _) = judge_compare(&value);
        assert_eq!(verdict, RvmVerdict::Pass, "{counts:?}");
        assert!(counts.both > 0, "{counts:?}");
        assert!(mismatches.is_empty(), "{}", mismatches.len());
    }
}
