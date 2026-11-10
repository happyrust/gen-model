//! 把逐件比对（`e3d_model::rvm_compare::CompareReport`，落盘为 `compare.json`）收成页面用的一页报告。
//!
//! 颜色不看进程退出码：配上了一部分、同时又有 `only_*` 时，那个进程仍可能是 0，
//! 差异只在 JSON 里。口径与 `scripts/Compare-SlykRvmStrict.ps1` 相同：
//! 没有缺件、没有 off、种类也一致是绿；只是种类不一致是黄；其余是红。

use e3d_model::rvm_compare::{CompareReport, ContainerTubePair, Pair};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERDICT_PASS: &str = "pass";
pub const VERDICT_KIND: &str = "kind_differ";
pub const VERDICT_FAIL: &str = "fail";

/// 页面上的三色。`Fail` 同时覆盖「比对发现差异」和「某一步没跑完」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

impl RvmCounts {
    /// 逐格累加：批量汇总与库里的按根汇总都按这一处算。
    pub fn add(&mut self, from: &Self) {
        self.both += from.both;
        self.exact += from.exact;
        self.chord += from.chord;
        self.reference_artifact += from.reference_artifact;
        self.film += from.film;
        self.off += from.off;
        self.only_baseline += from.only_baseline;
        self.only_generated += from.only_generated;
        self.kind_match += from.kind_match;
        self.kind_differ += from.kind_differ;
        self.container_off += from.container_off;
        self.container_only_baseline += from.container_only_baseline;
        self.container_only_generated += from.container_only_generated;
        self.centreline_only_baseline += from.centreline_only_baseline;
        self.unkeyable_baseline += from.unkeyable_baseline;
        self.unkeyable_generated += from.unkeyable_generated;
    }
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
    /// 原因种类（[`reason`] 模块的常量之一）：比 `category` 细一档，按几何形态归因——位置偏移、
    /// 轴长互换、生成体偏大 / 偏小、容器管身账、面片 vs 解析图元……查库时按它分组。
    #[serde(default)]
    pub reason_kind: String,
    /// 原因的一句话说明（含具体数值）。
    #[serde(default)]
    pub reason: String,
    /// `error` / `warning`：种类不一致只是黄，其余都是红。
    #[serde(default)]
    pub severity: String,
}

/// 原因种类的全部取值。一条差异一个；判法在 [`classify_pair`] / [`classify_single`]。
pub mod reason {
    pub const ERROR: &str = "error";
    pub const WARNING: &str = "warning";
    pub const INFO: &str = "info";

    /// 包围盒：尺寸一致、位置整体偏移。
    pub const AABB_SHIFTED: &str = "aabb_shifted";
    /// 包围盒：两根轴的长度互换（局部标架转了 90°）。
    pub const AABB_AXES_SWAPPED: &str = "aabb_axes_swapped";
    /// 包围盒：只有一根轴长度不同（端面偏移、长度算错一类）。
    pub const AABB_ONE_AXIS: &str = "aabb_one_axis_size";
    /// 包围盒：生成体整体包住基准（多出一块）。
    pub const GENERATED_LARGER: &str = "generated_larger";
    /// 包围盒：基准整体包住生成体（缺一块）。
    pub const GENERATED_SMALLER: &str = "generated_smaller";
    /// 包围盒：以上都不是的一般超差。
    pub const AABB_OFF: &str = "aabb_off";
    /// 包围盒在容差内，是容器组内隐式管逐根账把它判成 off 的。
    pub const CONTAINER_TUBE_MISMATCH: &str = "container_tube_mismatch";
    /// 图元种类：一侧写面片、一侧写解析图元。
    pub const FACET_VS_ANALYTIC: &str = "facet_vs_analytic";
    /// 图元种类：种类集合相同、条数不同。
    pub const PRIMITIVE_COUNT_DIFFERS: &str = "primitive_count_differs";
    /// 图元种类：种类集合不同。
    pub const PRIMITIVE_KIND_DIFFERS: &str = "primitive_kind_differs";
    /// 只在基准：E3D 的隐式管身（`TUBE n of …`）。
    pub const MISSING_IMPLIED_TUBE: &str = "missing_implied_tube";
    /// 只在基准：命名元素（`/NAME`）。
    pub const MISSING_NAMED_ELEMENT: &str = "missing_named_element";
    /// 只在基准：无名元素（`NOUN n of OWNER`）。
    pub const MISSING_UNNAMED_ELEMENT: &str = "missing_unnamed_element";
    /// 只在生成：隐式管身。
    pub const EXTRA_IMPLIED_TUBE: &str = "extra_implied_tube";
    /// 只在生成：其余。
    pub const EXTRA_IN_GENERATED: &str = "extra_in_generated";
    /// 基准成员发不出主键。
    pub const UNKEYABLE_BASELINE: &str = "unkeyable_baseline";
    /// 生成元素发不出主键（属主无名）。
    pub const UNKEYABLE_GENERATED: &str = "unkeyable_generated";
    /// 容器组内一根管身超差。
    pub const CONTAINER_TUBE_OFF: &str = "container_tube_off";
    /// 容器组内 E3D 多画一条记录。
    pub const CONTAINER_TUBE_MISSING: &str = "container_tube_missing_in_generated";
    /// 容器组内我们多画一根。
    pub const CONTAINER_TUBE_EXTRA: &str = "container_tube_extra_in_generated";
    /// 基准成员只有中心线（E3D 画线、我们不画），不进判据，只记账。
    pub const CENTRELINE_ONLY: &str = "centreline_only";
    /// [`CENTRELINE_ONLY`] 那一类的说明：差异行与构件行写同一句。
    pub const CENTRELINE_ONLY_TEXT: &str = "E3D 只画了中心线、没画实体，不进判据（我们不画线）";
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
    /// `reference_artifact` / `film` 判过的件才有：滤掉薄片 / 膜之后的差。
    significant_max_axis_delta_mm: Option<f64>,
    /// `local` / `world`；缺局部帧时按世界系比，旋转差会被算成尺寸差。
    comparison_frame: Option<String>,
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

impl PairFacts {
    /// 一对配上的件要判的事实。有问题的件进 [`JudgeFacts::problem_pairs`]；没有问题的件也摊得出来——
    /// 构件级结果（[`super::element`]）每一件都要过同一套归因。
    fn of(pair: &Pair) -> Self {
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
        Self {
            name: pair.key.clone(),
            noun: pair.noun.clone(),
            verdict: pair.verdict.to_string(),
            kinds_differ: pair.kind_match == Some(false),
            max_axis_delta_mm: Some(pair.max_axis_delta_mm),
            significant_max_axis_delta_mm: pair.significant_max_axis_delta_mm,
            comparison_frame: Some(pair.comparison_frame.to_string()),
            baseline_kinds: pair.baseline_kinds.clone(),
            generated_kinds: pair.generated_kinds.clone(),
            baseline_aabb: Some(pair.baseline_aabb),
            generated_aabb: Some(pair.generated_aabb),
            container_rows,
        }
    }

    /// 这一对有没有问题：`off`、种类不一致，或容器组内逐根账里有 off / only_*。
    fn is_problem(pair: &Pair) -> bool {
        pair.verdict == "off"
            || pair.kind_match == Some(false)
            || pair
                .container_tubes
                .as_ref()
                .is_some_and(|tubes| tubes.rows.iter().any(|row| is_problem_row(row.verdict)))
    }
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
                    significant_max_axis_delta_mm: pair
                        .get("significant_max_axis_delta_mm")
                        .and_then(Value::as_f64),
                    comparison_frame: pair
                        .get("comparison_frame")
                        .and_then(Value::as_str)
                        .map(str::to_string),
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
            .filter(|pair| PairFacts::is_problem(pair))
            .map(PairFacts::of)
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

/// 一次判色的完整结果：差异清单**不截断**（入库要全量），页面与 `run.json` 用
/// [`Judgement::page_mismatches`] 取截到 [`MISMATCH_LIMIT`] 的那一份。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Judgement {
    pub verdict: RvmVerdict,
    pub counts: RvmCounts,
    pub mismatches: Vec<RvmMismatch>,
    pub centreline_only_baseline: Vec<String>,
}

impl Judgement {
    /// 页面那一份：最多 [`MISMATCH_LIMIT`] 条，以及是否截过。
    pub fn page_mismatches(&self) -> (Vec<RvmMismatch>, bool) {
        let truncated = self.mismatches.len() > MISMATCH_LIMIT;
        (
            self.mismatches
                .iter()
                .take(MISMATCH_LIMIT)
                .cloned()
                .collect(),
            truncated,
        )
    }

    /// 旧形态的五元组（截断后的差异清单），给还按元组读的调用方。
    pub fn into_tuple(self) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
        let (mismatches, truncated) = self.page_mismatches();
        (
            self.verdict,
            self.counts,
            mismatches,
            self.centreline_only_baseline,
            truncated,
        )
    }
}

/// 从 `rvm_compare` 的 JSON 取值。缺 `by_noun` 或一件都没配上，判红。
pub fn judge_compare(
    value: &Value,
) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
    judge_compare_full(value).into_tuple()
}

/// 进程内刚算完的报告直接判色，口径与 [`judge_compare`] 读落盘 JSON 完全一致。
pub fn judge_report(
    report: &CompareReport,
) -> (RvmVerdict, RvmCounts, Vec<RvmMismatch>, Vec<String>, bool) {
    judge_report_full(report).into_tuple()
}

/// [`judge_compare`] 的不截断版。
pub fn judge_compare_full(value: &Value) -> Judgement {
    judge(JudgeFacts::from_value(value))
}

/// [`judge_report`] 的不截断版。
pub fn judge_report_full(report: &CompareReport) -> Judgement {
    judge(JudgeFacts::from_report(report))
}

fn judge(facts: JudgeFacts) -> Judgement {
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
        mismatches.extend(pair_verdict_mismatch(pair));
        for (generated, row_verdict, max_axis_delta_mm) in &pair.container_rows {
            mismatches.push(container_row_mismatch(
                generated.as_deref(),
                row_verdict,
                *max_axis_delta_mm,
            ));
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
    Judgement {
        verdict,
        counts,
        mismatches,
        centreline_only_baseline: centreline,
    }
}

// ============================================================================
// 差异归因：category 之下再分一档「原因种类」
// ============================================================================

fn fmt_mm(value: f64) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.3}")
    }
}

fn fmt_kinds(kinds: &[String]) -> String {
    let mut counted: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for kind in kinds {
        *counted.entry(kind.as_str()).or_default() += 1;
    }
    counted
        .into_iter()
        .map(|(kind, n)| {
            if n > 1 {
                format!("{kind}×{n}")
            } else {
                kind.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// 一对配上的件：`off` 按两只盒的形态归因；`kind_differ` 按两侧图元种类归因。
fn classify_pair(pair: &PairFacts, category: &str) -> (&'static str, String) {
    if category == "kind_differ" {
        return classify_kinds(&pair.baseline_kinds, &pair.generated_kinds);
    }
    let tolerance = e3d_model::rvm_compare::chord_tolerance_mm(&pair.noun);
    let delta = pair.max_axis_delta_mm.unwrap_or(f64::NAN);
    let frame_note = match pair.comparison_frame.as_deref() {
        Some("world") => "；缺局部帧，按世界系比较（旋转差会表现为尺寸差）",
        _ => "",
    };
    let significant_note = pair
        .significant_max_axis_delta_mm
        .map(|value| format!("；滤掉断开薄片后仍差 {} mm", fmt_mm(value)))
        .unwrap_or_default();
    let has_container_problem = !pair.container_rows.is_empty();

    // 包围盒本身在容差内：是容器组内逐根账把它判成 off 的。
    if delta.is_finite() && delta <= tolerance && has_container_problem {
        return (
            reason::CONTAINER_TUBE_MISMATCH,
            format!(
                "包围盒在容差内（最大轴差 {} mm ≤ {} mm），容器组内隐式管逐根账有 {} 行 off / 多画 / 少画",
                fmt_mm(delta),
                fmt_mm(tolerance),
                pair.container_rows.len()
            ),
        );
    }
    let (Some(b), Some(g)) = (pair.baseline_aabb, pair.generated_aabb) else {
        return (
            reason::AABB_OFF,
            format!(
                "包围盒最大轴差 {} mm（容差 {} mm）{significant_note}{frame_note}",
                fmt_mm(delta),
                fmt_mm(tolerance)
            ),
        );
    };
    let axis = ["X", "Y", "Z"];
    let size_b: [f64; 3] = std::array::from_fn(|i| b[i + 3] - b[i]);
    let size_g: [f64; 3] = std::array::from_fn(|i| g[i + 3] - g[i]);
    let centre_b: [f64; 3] = std::array::from_fn(|i| (b[i] + b[i + 3]) / 2.0);
    let centre_g: [f64; 3] = std::array::from_fn(|i| (g[i] + g[i + 3]) / 2.0);
    let size_delta: [f64; 3] = std::array::from_fn(|i| size_g[i] - size_b[i]);
    let centre_delta: [f64; 3] = std::array::from_fn(|i| centre_g[i] - centre_b[i]);
    let within = |value: f64| value.abs() <= tolerance;
    let sizes_match = size_delta.iter().all(|d| within(*d));
    let centres_match = centre_delta.iter().all(|d| within(*d));
    let tail = format!(
        "（最大轴差 {} mm，容差 {} mm）{significant_note}{frame_note}",
        fmt_mm(delta),
        fmt_mm(tolerance)
    );

    if sizes_match && !centres_match {
        return (
            reason::AABB_SHIFTED,
            format!(
                "尺寸一致、位置偏移 ({}, {}, {}) mm{tail}",
                fmt_mm(centre_delta[0]),
                fmt_mm(centre_delta[1]),
                fmt_mm(centre_delta[2])
            ),
        );
    }
    // 两根轴长互换：局部标架转了 90°。只看长度不等的那两根。
    for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
        let third = 3 - p - q;
        let swapped = within(size_g[p] - size_b[q])
            && within(size_g[q] - size_b[p])
            && within(size_delta[third])
            && !within(size_delta[p])
            && !within(size_delta[q]);
        if swapped {
            return (
                reason::AABB_AXES_SWAPPED,
                format!(
                    "{}/{} 轴长互换：基准 {}×{}，生成 {}×{}（局部标架绕 {} 转了 90°）{tail}",
                    axis[p],
                    axis[q],
                    fmt_mm(size_b[p]),
                    fmt_mm(size_b[q]),
                    fmt_mm(size_g[p]),
                    fmt_mm(size_g[q]),
                    axis[third]
                ),
            );
        }
    }
    let differing: Vec<usize> = (0..3).filter(|i| !within(size_delta[*i])).collect();
    if differing.len() == 1 {
        let i = differing[0];
        return (
            reason::AABB_ONE_AXIS,
            format!(
                "仅 {} 轴长度不同：基准 {}、生成 {}（差 {} mm）{tail}",
                axis[i],
                fmt_mm(size_b[i]),
                fmt_mm(size_g[i]),
                fmt_mm(size_delta[i])
            ),
        );
    }
    let generated_contains =
        (0..3).all(|i| g[i] <= b[i] + tolerance && g[i + 3] >= b[i + 3] - tolerance);
    let baseline_contains =
        (0..3).all(|i| b[i] <= g[i] + tolerance && b[i + 3] >= g[i + 3] - tolerance);
    let sizes = format!(
        "基准 {}×{}×{}，生成 {}×{}×{}",
        fmt_mm(size_b[0]),
        fmt_mm(size_b[1]),
        fmt_mm(size_b[2]),
        fmt_mm(size_g[0]),
        fmt_mm(size_g[1]),
        fmt_mm(size_g[2])
    );
    if generated_contains && !baseline_contains {
        return (
            reason::GENERATED_LARGER,
            format!("生成体整体包住基准（多出一块）：{sizes}{tail}"),
        );
    }
    if baseline_contains && !generated_contains {
        return (
            reason::GENERATED_SMALLER,
            format!("基准整体包住生成体（缺一块）：{sizes}{tail}"),
        );
    }
    (reason::AABB_OFF, format!("包围盒超差：{sizes}{tail}"))
}

fn classify_kinds(baseline: &[String], generated: &[String]) -> (&'static str, String) {
    let is_facet = |kind: &String| kind == "FacetGroup";
    let detail = format!(
        "E3D [{}] / 生成 [{}]",
        fmt_kinds(baseline),
        fmt_kinds(generated)
    );
    let baseline_facets = !baseline.is_empty() && baseline.iter().all(is_facet);
    let generated_facets = !generated.is_empty() && generated.iter().all(is_facet);
    if baseline_facets != generated_facets
        && (baseline_facets || generated_facets)
        && !(baseline.iter().any(is_facet) && generated.iter().any(is_facet))
    {
        let note = if baseline_facets {
            "E3D 写面片、我们写解析图元"
        } else {
            "E3D 写解析图元、我们写面片"
        };
        return (reason::FACET_VS_ANALYTIC, format!("{note}：{detail}"));
    }
    let mut b_set: Vec<&String> = baseline.iter().collect();
    let mut g_set: Vec<&String> = generated.iter().collect();
    b_set.sort();
    b_set.dedup();
    g_set.sort();
    g_set.dedup();
    if b_set == g_set {
        return (
            reason::PRIMITIVE_COUNT_DIFFERS,
            format!(
                "图元种类相同、条数不同（E3D {} 条 / 生成 {} 条）：{detail}",
                baseline.len(),
                generated.len()
            ),
        );
    }
    (
        reason::PRIMITIVE_KIND_DIFFERS,
        format!("图元种类不同：{detail}"),
    )
}

/// 单侧件：按名字形态分「隐式管 / 命名元素 / 无名元素」，`unkeyable_*` 把括号里的原因抄过来。
fn classify_single(category: &str, name: &str) -> (&'static str, String, &'static str) {
    let trimmed = name.trim();
    // `only_generated` 的写法是 `<refno 路径> <noun> <key>`，路径本身带空格（`/GENERATED/TUBI 1 of BRAN /X`），
    // 所以不按位置取 noun，只看有没有 TUBI / TUBE 这个词。
    let is_tube = trimmed
        .split_whitespace()
        .any(|token| token.eq_ignore_ascii_case("TUBI") || token == "TUBE");
    match category {
        "only_baseline" => {
            let kind = if trimmed.starts_with("TUBE ") || trimmed.starts_with("TUBI ") {
                reason::MISSING_IMPLIED_TUBE
            } else if trimmed.starts_with('/') {
                reason::MISSING_NAMED_ELEMENT
            } else {
                reason::MISSING_UNNAMED_ELEMENT
            };
            (
                kind,
                format!("E3D 导出有、生成侧没有：{trimmed}"),
                reason::ERROR,
            )
        }
        "only_generated" => {
            let kind = if is_tube {
                reason::EXTRA_IMPLIED_TUBE
            } else {
                reason::EXTRA_IN_GENERATED
            };
            (
                kind,
                format!("生成侧多出、E3D 导出没有：{trimmed}"),
                reason::ERROR,
            )
        }
        "unkeyable_baseline" => {
            let detail = trimmed
                .rsplit_once('（')
                .map(|(_, tail)| tail.trim_end_matches('）').to_string())
                .unwrap_or_else(|| "发不出主键".to_string());
            (
                reason::UNKEYABLE_BASELINE,
                format!("基准成员发不出主键：{detail}"),
                reason::ERROR,
            )
        }
        "unkeyable_generated" => (
            reason::UNKEYABLE_GENERATED,
            format!("生成元素发不出主键（属主无名）：{trimmed}"),
            reason::ERROR,
        ),
        _ => (reason::AABB_OFF, trimmed.to_string(), reason::ERROR),
    }
}

fn classify_container_row(
    row_verdict: &str,
    generated: Option<&str>,
    max_axis_delta_mm: Option<f64>,
) -> (&'static str, String) {
    match row_verdict {
        "off" => (
            reason::CONTAINER_TUBE_OFF,
            format!(
                "容器组内这根管身与 E3D 记录的包围盒差 {} mm：{}",
                max_axis_delta_mm
                    .map(fmt_mm)
                    .unwrap_or_else(|| "?".to_string()),
                generated.unwrap_or("?")
            ),
        ),
        "only_baseline" => (
            reason::CONTAINER_TUBE_MISSING,
            "E3D 在容器组里多画了一条记录，我们没有对应的管身".to_string(),
        ),
        _ => (
            reason::CONTAINER_TUBE_EXTRA,
            format!(
                "我们多画了一根管身，E3D 容器组里没有对应记录：{}",
                generated.unwrap_or("?")
            ),
        ),
    }
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
    let (reason_kind, reason) = classify_pair(pair, category);
    RvmMismatch {
        category: category.to_string(),
        name: pair.name.clone(),
        noun: pair.noun.clone(),
        max_axis_delta_mm: pair.max_axis_delta_mm,
        baseline_kinds: pair.baseline_kinds.clone(),
        generated_kinds: pair.generated_kinds.clone(),
        baseline_aabb: pair.baseline_aabb,
        generated_aabb: pair.generated_aabb,
        reason_kind: reason_kind.to_string(),
        reason,
        severity: if category == "kind_differ" {
            reason::WARNING
        } else {
            reason::ERROR
        }
        .to_string(),
    }
}

/// 一对配上的件自己那一行差异：`off` 一行，否则种类不一致一行，都不是就没有。
fn pair_verdict_mismatch(pair: &PairFacts) -> Option<RvmMismatch> {
    if pair.verdict == "off" {
        Some(pair_mismatch(pair, "off"))
    } else if pair.kinds_differ {
        Some(pair_mismatch(pair, "kind_differ"))
    } else {
        None
    }
}

/// 容器组内逐根账里有问题的一行落成的差异行。
fn container_row_mismatch(
    generated: Option<&str>,
    row_verdict: &str,
    max_axis_delta_mm: Option<f64>,
) -> RvmMismatch {
    let (reason_kind, reason) = classify_container_row(row_verdict, generated, max_axis_delta_mm);
    RvmMismatch {
        category: format!("container_{row_verdict}"),
        name: generated
            .map(str::to_string)
            .unwrap_or_else(|| "(容器管身)".to_string()),
        noun: "TUBI".to_string(),
        max_axis_delta_mm,
        baseline_kinds: Vec::new(),
        generated_kinds: Vec::new(),
        baseline_aabb: None,
        generated_aabb: None,
        reason_kind: reason_kind.to_string(),
        reason,
        severity: reason::ERROR.to_string(),
    }
}

/// 构件级结果用：一对配上的件按 [`judge`] 同一套规则得出的那行差异；没有问题为 `None`。
pub(super) fn pair_problem(pair: &Pair) -> Option<RvmMismatch> {
    pair_verdict_mismatch(&PairFacts::of(pair))
}

/// 构件级结果用：容器组内逐根账的一行按 [`judge`] 同一套规则得出的差异；配上且在容差内为 `None`。
pub(super) fn container_row_problem(row: &ContainerTubePair) -> Option<RvmMismatch> {
    is_problem_row(row.verdict).then(|| {
        container_row_mismatch(row.generated.as_deref(), row.verdict, row.max_axis_delta_mm)
    })
}

/// 构件级结果用：单侧件 / 发不出主键的件按 [`judge`] 同一套规则得出的差异。`name` 是比对报告里的原串。
pub(super) fn single_problem(category: &str, name: &str) -> RvmMismatch {
    named(category, name)
}

fn named(category: &str, name: &str) -> RvmMismatch {
    let (reason_kind, reason, severity) = classify_single(category, name);
    RvmMismatch {
        category: category.to_string(),
        name: name.to_string(),
        noun: String::new(),
        max_axis_delta_mm: None,
        baseline_kinds: Vec::new(),
        generated_kinds: Vec::new(),
        baseline_aabb: None,
        generated_aabb: None,
        reason_kind: reason_kind.to_string(),
        reason,
        severity: severity.to_string(),
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
        assert_eq!(mismatches[0].reason_kind, reason::PRIMITIVE_KIND_DIFFERS);
        assert_eq!(mismatches[0].severity, reason::WARNING);
    }

    /// 原因种类按两只盒的形态归因：整体平移、两轴互换、单轴长度、包含关系、一般超差；
    /// 包围盒在容差内却因容器账判 off 的单列；种类差异再分面片 vs 解析 / 条数 / 种类。
    #[test]
    fn mismatches_carry_a_reason_kind_derived_from_the_two_boxes() {
        let pair = |key: &str, noun: &str, delta: f64, b: [f64; 6], g: [f64; 6]| {
            serde_json::json!({
                "key": key, "noun": noun, "verdict": "off", "kind_match": true,
                "max_axis_delta_mm": delta, "comparison_frame": "local",
                "baseline_aabb": b, "generated_aabb": g,
                "baseline_kinds": ["Box"], "generated_kinds": ["Box"]
            })
        };
        let value = serde_json::json!({
            "by_noun": { "SJOI": tally(0, 0, 6, 0, 0) },
            "pairs": [
                // 尺寸一致、沿 X 挪了 109 mm（7356 SJOI 端面偏移那一类）。
                pair("shift", "SJOI", 109.0, [0.0, 0.0, 0.0, 100.0, 50.0, 20.0], [109.0, 0.0, 0.0, 209.0, 50.0, 20.0]),
                // X/Y 轴长互换（7333 PCLA 局部标架绕 Z 差 90° 那一类）。
                pair("swap", "PCLA", 80.0, [0.0, 0.0, 0.0, 100.0, 20.0, 30.0], [0.0, 0.0, 0.0, 20.0, 100.0, 30.0]),
                // 只有 Z 长度不同。
                pair("one", "HELE", 40.0, [0.0, 0.0, 0.0, 10.0, 10.0, 100.0], [0.0, 0.0, 0.0, 10.0, 10.0, 140.0]),
                // 生成体整体包住基准。
                pair("larger", "FITT", 5.0, [0.0, 0.0, 0.0, 10.0, 10.0, 10.0], [-5.0, -5.0, -5.0, 15.0, 15.0, 15.0]),
                // 基准整体包住生成体。
                pair("smaller", "FITT", 5.0, [-5.0, -5.0, -5.0, 15.0, 15.0, 15.0], [0.0, 0.0, 0.0, 10.0, 10.0, 10.0]),
                // 形状、位置都不同。
                pair("general", "ATTA", 42.0, [0.0, 0.0, 0.0, 10.0, 20.0, 30.0], [5.0, -7.0, 0.0, 52.0, 9.0, 61.0]),
            ],
            "only_baseline": [],
            "only_generated": []
        });
        let (_, _, mismatches, _, _) = judge_compare(&value);
        let kind_of = |key: &str| {
            mismatches
                .iter()
                .find(|row| row.name == key)
                .map(|row| (row.reason_kind.as_str(), row.reason.clone()))
                .unwrap_or_else(|| panic!("没有 {key}"))
        };
        let (kind, text) = kind_of("shift");
        assert_eq!(kind, reason::AABB_SHIFTED, "{text}");
        assert!(text.contains("109.0"), "{text}");
        let (kind, text) = kind_of("swap");
        assert_eq!(kind, reason::AABB_AXES_SWAPPED, "{text}");
        assert!(text.contains("X/Y") && text.contains("绕 Z"), "{text}");
        let (kind, text) = kind_of("one");
        assert_eq!(kind, reason::AABB_ONE_AXIS, "{text}");
        assert!(text.contains("仅 Z 轴"), "{text}");
        assert_eq!(kind_of("larger").0, reason::GENERATED_LARGER);
        assert_eq!(kind_of("smaller").0, reason::GENERATED_SMALLER);
        assert_eq!(kind_of("general").0, reason::AABB_OFF);
        assert!(mismatches.iter().all(|row| row.severity == reason::ERROR));

        // 盒在容差内、容器账有问题：原因是容器管身，不是包围盒。
        let container = serde_json::json!({
            "by_noun": { "BRAN": tally(0, 0, 1, 0, 0) },
            "pairs": [{
                "key": "BRAN /B1", "noun": "BRAN", "verdict": "off", "kind_match": true,
                "max_axis_delta_mm": 0.01, "comparison_frame": "world",
                "baseline_aabb": [0, 0, 0, 1, 1, 1], "generated_aabb": [0, 0, 0, 1, 1, 1],
                "container_tubes": { "tally": { "off": 1 }, "rows": [
                    { "generated": "/GENERATED/TUBI 2 of BRAN /B1", "baseline_records": [1], "max_axis_delta_mm": 12.0, "verdict": "off" },
                    { "generated": null, "baseline_records": [2], "max_axis_delta_mm": null, "verdict": "only_baseline" }
                ] }
            }],
            "only_baseline": [], "only_generated": []
        });
        let (_, _, mismatches, _, _) = judge_compare(&container);
        assert_eq!(mismatches[0].reason_kind, reason::CONTAINER_TUBE_MISMATCH);
        assert_eq!(mismatches[1].category, "container_off");
        assert_eq!(mismatches[1].reason_kind, reason::CONTAINER_TUBE_OFF);
        assert_eq!(mismatches[2].reason_kind, reason::CONTAINER_TUBE_MISSING);

        // 种类差异三分法。
        let kinds = |b: &[&str], g: &[&str]| {
            classify_kinds(
                &b.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                &g.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
            .0
        };
        assert_eq!(
            kinds(&["FacetGroup"], &["Cylinder", "Box"]),
            reason::FACET_VS_ANALYTIC
        );
        assert_eq!(
            kinds(&["Cylinder", "Cylinder"], &["Cylinder"]),
            reason::PRIMITIVE_COUNT_DIFFERS
        );
        assert_eq!(
            kinds(&["Cylinder"], &["Snout"]),
            reason::PRIMITIVE_KIND_DIFFERS
        );

        // 单侧件按名字形态分。
        assert_eq!(
            classify_single("only_baseline", "TUBE 3 of BRANCH /X").0,
            reason::MISSING_IMPLIED_TUBE
        );
        assert_eq!(
            classify_single("only_baseline", "/NAMED").0,
            reason::MISSING_NAMED_ELEMENT
        );
        assert_eq!(
            classify_single("only_baseline", "BOX 2 of EQUIPMENT /E").0,
            reason::MISSING_UNNAMED_ELEMENT
        );
        assert_eq!(
            classify_single("only_generated", "/GENERATED/x TUBI TUBI 1 /B").0,
            reason::EXTRA_IMPLIED_TUBE
        );
        assert_eq!(
            classify_single("only_generated", "/GENERATED/x ELBO ELBO 1 /B").0,
            reason::EXTRA_IN_GENERATED
        );
        let (kind, text, _) = classify_single(
            "unkeyable_baseline",
            "/MDS/VH111/80/H2（缺 noun/owner_desc/ordinal，且无同名生成元素）",
        );
        assert_eq!(kind, reason::UNKEYABLE_BASELINE);
        assert!(text.contains("缺 noun/owner_desc/ordinal"), "{text}");
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
        assert_eq!(
            verdict_message(verdict, &counts),
            "通过：两侧都没有可比几何"
        );

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
