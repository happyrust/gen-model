//! 构件级核对结果：一次比对里每个带几何的构件一条（[`ElementCheck`]），按 refno 落进
//! `rvm_check_element`（[`super::store`]）。通过的也有，不只是出问题的。
//!
//! 比对按名称配对，报告里没有 refno：配上的件 `pairs[].refno` 是生成成员的路径
//! （`/GENERATED/<显示名>`），只在生成侧的两类也只有成员路径，只在基准侧的三类只有 E3D 的显示名。
//! 身份从两头接回来：
//!
//! - **生成侧**：vendor 发出每个成员时记下它出自哪个元素（[`MemberSource`]，`export_rvm` 的结果里带），
//!   这里按成员路径接（[`element_checks`]）；
//! - **基准侧**：显示名逆着解回库里的元素（`e3d_model::rvm_export::resolve_member_names`，经
//!   [`resolve_baseline_side`]）。解不出来的照样留一条，`refno` 空着。
//!
//! 没有自己库记录的几何（隐式管身一类）记在属主的 refno 名下，带 [`Derived`]。
//!
//! 判档口径不在这里：每条的 `grade` 抄的是比对器的档，问题说明走 [`super::report`] 判色时用的同一组
//! 函数。所以一次比对里各档的条数与 [`RvmCounts`] 逐格对得上（[`counts_of`]）。

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use e3d_model::rvm_compare::{CompareReport, ContainerTubePair, Pair};
use e3d_model::rvm_export::MemberSource;
use serde::{Deserialize, Serialize};

use super::report::{self, RvmCounts, RvmMismatch, reason};

/// 判档的全部取值。前五个是配上的件（比对器的档）；`only_*` / `unkeyable_*` 是没配上的；
/// `centreline_only` 不进判据；`container_*` 是并成一件的容器管身组内的逐根账。
///
/// 与 [`RvmCounts`] 的对应：`exact` … `off` 各对各的格（合起来是 `both`），`only_*` / `unkeyable_*` 同名，
/// `centreline_only` 对 `centreline_only_baseline`，`container_off` / `container_only_*` 同名；
/// `container_exact` / `container_chord` 在 `counts` 里没有格。
pub mod grade {
    pub const EXACT: &str = "exact";
    pub const CHORD: &str = "chord";
    pub const REFERENCE_ARTIFACT: &str = "reference_artifact";
    pub const FILM: &str = "film";
    pub const OFF: &str = "off";
    pub const ONLY_BASELINE: &str = "only_baseline";
    pub const ONLY_GENERATED: &str = "only_generated";
    pub const UNKEYABLE_BASELINE: &str = "unkeyable_baseline";
    pub const UNKEYABLE_GENERATED: &str = "unkeyable_generated";
    pub const CENTRELINE_ONLY: &str = "centreline_only";
    pub const CONTAINER_EXACT: &str = "container_exact";
    pub const CONTAINER_CHORD: &str = "container_chord";
    pub const CONTAINER_OFF: &str = "container_off";
    pub const CONTAINER_ONLY_BASELINE: &str = "container_only_baseline";
    pub const CONTAINER_ONLY_GENERATED: &str = "container_only_generated";

    /// 这一档算不算通过：包围盒在容差档内（含断开薄片、贴体薄膜两档），或不进判据的中心线。
    pub fn passed(grade: &str) -> bool {
        matches!(
            grade,
            EXACT | CHORD | REFERENCE_ARTIFACT | FILM | CONTAINER_EXACT | CONTAINER_CHORD | CENTRELINE_ONLY
        )
    }

    /// 这一档的构件只在基准（E3D 导出）一侧有：身份没有生成成员可接，只能按显示名找回来。
    pub fn baseline_side(grade: &str) -> bool {
        matches!(
            grade,
            ONLY_BASELINE | UNKEYABLE_BASELINE | CENTRELINE_ONLY | CONTAINER_ONLY_BASELINE
        )
    }
}

/// 严重程度：差异行（`rvm_check_mismatch`）的三档之外，没有问题的构件是 `ok`。
pub const SEVERITY_OK: &str = "ok";

/// [`Derived::kind`]：E3D 把隐式管写在容器自己那一组里时，我们同一容器名下的几根管身并成一件去配
/// （vendor 的 `merge_container_tubes`）。这一条是整组。
pub const DERIVED_CONTAINER_TUBES: &str = "container_tubes";
/// [`Derived::kind`]：容器组里 E3D 多画、我们没有对应管身的那条记录。
pub const DERIVED_CONTAINER_RECORD: &str = "container_record";

/// 没有自己的库记录、由属主派生出来的几何：记在属主的 refno 名下。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Derived {
    /// `implied_tube`（隐式管身）/ `hang_axis_tube`（吊架轴线柱）——vendor 给的；
    /// [`DERIVED_CONTAINER_TUBES`] / [`DERIVED_CONTAINER_RECORD`]。
    pub kind: String,
    /// 管身：显示名 `TUBI n of <属主>` 里的 n（只在基准侧的那根是 E3D 的 `TUBE n`）；
    /// 容器组里的记录：它在那一组里的序号（1 起）；整组为 0。
    pub ordinal: usize,
    /// 隐式管身两端的构件 refno（上游 / 下游），知道才有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

impl Derived {
    fn from_member(derived: &e3d_model::rvm_export::DerivedMember) -> Self {
        Self {
            kind: derived.kind.clone(),
            ordinal: derived.ordinal,
            from: derived.from.clone(),
            to: derived.to.clone(),
        }
    }
}

/// 一个带几何构件在一次比对里的结果。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ElementCheck {
    /// 构件自己的 refno（`w0/w1`）；派生件（[`Self::derived`]）是属主的。解不出来为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refno: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<Derived>,
    /// 名词：库里的短名（工程派生的子类型是 UDET 名），管身是 `TUBI`。没解出 refno 的基准成员只有
    /// E3D 显示名里的全称，命名元素则为空。
    pub noun: String,
    /// 构件在 RVM 里的显示名：命名元素是 `/NAME`，无名元素是 `<NOUN> <n> of <属主显示名>`。
    /// 生成侧有这件时是我们写出的名字（名词是短名），只在基准侧的是 E3D 写的（名词是全称）。
    pub name: String,
    /// 配对用的主键（比对报告的 `pairs[].key`），配上的件才有；差异行（`rvm_check_mismatch`）的 `name`
    /// 对配上的件写的就是它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_key: Option<String>,
    /// 判档，取值见 [`grade`]。
    pub grade: String,
    /// 两侧写出的图元种类是否同一套；没配上的件没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind_match: Option<bool>,
    /// 逐轴 |生成 − 基准| 的最大值（mm）；没配上的件没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_axis_delta_mm: Option<f64>,
    /// `reference_artifact` / `film` 两档才有：滤掉断开薄片 / 贴体薄膜之后的差（这两档凭它算通过，
    /// [`Self::max_axis_delta_mm`] 仍是原始的整盒差）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significant_max_axis_delta_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline_kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generated_kinds: Vec<String>,
    /// 两侧的包围盒 `[xmin, ymin, zmin, xmax, ymax, zmax]`（mm），在 [`Self::aabb_frame`] 那一帧里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_aabb: Option<[f64; 6]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_aabb: Option<[f64; 6]>,
    /// 两只包围盒所在的帧：`local`（元素局部系，常态）/ `world`（缺局部帧时按世界系比）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aabb_frame: Option<String>,
    /// 有问题时的原因种类与那句带数值的说明，与这一件在 `rvm_check_mismatch` 里的那一行相同；没有问题为空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason_kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// `ok` / `info`（只画中心线，不进判据）/ `warning`（只是图元种类不一致）/ `error`。
    pub severity: String,
}

impl ElementCheck {
    /// 这一档算不算通过（[`grade::passed`]）。图元种类是否一致另看 [`Self::kind_match`]。
    pub fn passed(&self) -> bool {
        grade::passed(&self.grade)
    }

    fn new(grade: &str, name: String, noun: String) -> Self {
        Self {
            refno: None,
            derived: None,
            noun,
            name,
            pair_key: None,
            grade: grade.to_string(),
            kind_match: None,
            max_axis_delta_mm: None,
            significant_max_axis_delta_mm: None,
            baseline_kinds: Vec::new(),
            generated_kinds: Vec::new(),
            baseline_aabb: None,
            generated_aabb: None,
            aabb_frame: None,
            reason_kind: String::new(),
            reason: String::new(),
            severity: SEVERITY_OK.to_string(),
        }
    }

    /// 把这一件的差异行（判色那一套给的）抄过来；没有差异就是 `ok`。
    fn with_problem(mut self, problem: Option<RvmMismatch>) -> Self {
        if let Some(problem) = problem {
            self.reason_kind = problem.reason_kind;
            self.reason = problem.reason;
            self.severity = if problem.severity.is_empty() {
                reason::ERROR.to_string()
            } else {
                problem.severity
            };
        }
        self
    }

    /// 接上生成成员的出处：refno、派生件标记、名词、我们写出的显示名。
    fn with_source(mut self, source: &MemberSource) -> Self {
        self.refno = Some(source.refno.clone());
        self.derived = source.derived.as_ref().map(Derived::from_member);
        self.noun = source.noun.clone();
        self.name = source.name.clone();
        self
    }
}

/// 成员路径 → 出处。同一路径出现两次且指向不同元素时两个都不认（宁可记成没解出，也不挑一个）。
struct SourceIndex<'a> {
    by_path: HashMap<&'a str, Option<&'a MemberSource>>,
}

impl<'a> SourceIndex<'a> {
    fn new(sources: &'a [MemberSource]) -> Self {
        let mut by_path: HashMap<&str, Option<&MemberSource>> =
            HashMap::with_capacity(sources.len());
        for source in sources {
            by_path
                .entry(source.path.as_str())
                .and_modify(|slot| {
                    if slot.is_some_and(|first| {
                        first.refno != source.refno || first.derived != source.derived
                    }) {
                        *slot = None;
                    }
                })
                .or_insert(Some(source));
        }
        Self { by_path }
    }

    fn get(&self, path: &str) -> Option<&'a MemberSource> {
        self.by_path.get(path).copied().flatten()
    }
}

/// 生成成员的路径还原成显示名：`/GENERATED/NAME` → `/NAME`，默认命名原样。
fn name_from_path(path: &str) -> String {
    let Some(rest) = path.strip_prefix("/GENERATED/") else {
        return path.to_string();
    };
    if rest.contains(" of ") {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// 基准成员显示名里的名词：默认命名取开头那个词（`TUBE` 归成 `TUBI`），命名元素读不出来。
fn noun_from_name(name: &str) -> String {
    let Some((head, _)) = name.trim().split_once(" of ") else {
        return String::new();
    };
    match head.rsplit_once(' ') {
        Some(("TUBE", _)) => "TUBI".to_string(),
        Some((noun, _)) => noun.trim().to_string(),
        None => String::new(),
    }
}

/// `unkeyable_baseline` 的原串是「成员名（说明）」：取回成员名。
fn strip_note(entry: &str) -> &str {
    entry
        .rsplit_once('（')
        .map_or(entry, |(name, _)| name)
        .trim()
}

/// 把一份比对报告摊成构件级结果：配上的每一对、容器组内的每一根、两侧各自多出来的、发不出主键的、
/// 只画中心线的，各一条。`sources` 是生成侧每个成员的出处（[`MemberSource`]）；基准侧的几类此时
/// `refno` 还空着，交给 [`resolve_baseline_side`]。
pub fn element_checks(report: &CompareReport, sources: &[MemberSource]) -> Vec<ElementCheck> {
    let index = SourceIndex::new(sources);
    let mut checks = Vec::with_capacity(
        report.pairs.len()
            + report.only_baseline.len()
            + report.only_generated.len()
            + report.unkeyable_baseline.len()
            + report.unkeyable_generated.len()
            + report.centreline_only_baseline.len(),
    );
    for pair in &report.pairs {
        let pair_check = pair_check(pair, &index);
        let owner = pair_check.refno.clone();
        checks.push(pair_check);
        if let Some(tubes) = &pair.container_tubes {
            for row in &tubes.rows {
                checks.push(container_row_check(pair, owner.as_deref(), row, &index));
            }
        }
    }
    for name in &report.only_baseline {
        checks.push(
            ElementCheck::new(
                grade::ONLY_BASELINE,
                name.trim().to_string(),
                noun_from_name(name),
            )
            .with_problem(Some(report::single_problem("only_baseline", name))),
        );
    }
    generated_only(
        &mut checks,
        grade::ONLY_GENERATED,
        "only_generated",
        &report.only_generated,
        &report.only_generated_members,
        &index,
    );
    for entry in &report.unkeyable_baseline {
        let name = strip_note(entry);
        checks.push(
            ElementCheck::new(
                grade::UNKEYABLE_BASELINE,
                name.to_string(),
                noun_from_name(name),
            )
            .with_problem(Some(report::single_problem("unkeyable_baseline", entry))),
        );
    }
    generated_only(
        &mut checks,
        grade::UNKEYABLE_GENERATED,
        "unkeyable_generated",
        &report.unkeyable_generated,
        &report.unkeyable_generated_members,
        &index,
    );
    for name in &report.centreline_only_baseline {
        let mut check = ElementCheck::new(
            grade::CENTRELINE_ONLY,
            name.trim().to_string(),
            noun_from_name(name),
        );
        check.reason_kind = reason::CENTRELINE_ONLY.to_string();
        check.reason = reason::CENTRELINE_ONLY_TEXT.to_string();
        check.severity = reason::INFO.to_string();
        checks.push(check);
    }
    checks
}

/// 只在生成侧的两类：`entries` 是报告里给人读的原串（差异行用它归因），`members` 是与之一一对应的
/// 成员路径。
fn generated_only(
    checks: &mut Vec<ElementCheck>,
    grade: &str,
    category: &str,
    entries: &[String],
    members: &[String],
    index: &SourceIndex<'_>,
) {
    for (at, entry) in entries.iter().enumerate() {
        let path = members.get(at).map(String::as_str);
        let name = path.map_or_else(|| entry.clone(), name_from_path);
        let noun = noun_from_name(&name);
        let mut check = ElementCheck::new(grade, name, noun)
            .with_problem(Some(report::single_problem(category, entry)));
        if let Some(source) = path.and_then(|path| index.get(path)) {
            check = check.with_source(source);
        }
        checks.push(check);
    }
}

/// 配上的一对。并成一件的容器管身整组也在这里：它没有对应的单个生成成员，出处取组内任何一根的属主。
fn pair_check(pair: &Pair, index: &SourceIndex<'_>) -> ElementCheck {
    let mut check = ElementCheck::new(pair.verdict, name_from_path(&pair.refno), pair.noun.clone())
        .with_problem(report::pair_problem(pair));
    check.pair_key = Some(pair.key.clone());
    check.kind_match = pair.kind_match;
    check.max_axis_delta_mm = Some(pair.max_axis_delta_mm);
    check.significant_max_axis_delta_mm = pair.significant_max_axis_delta_mm;
    check.baseline_kinds = pair.baseline_kinds.clone();
    check.generated_kinds = pair.generated_kinds.clone();
    check.baseline_aabb = Some(pair.baseline_aabb);
    check.generated_aabb = Some(pair.generated_aabb);
    check.aabb_frame = Some(pair.comparison_frame.to_string());
    match &pair.container_tubes {
        Some(tubes) => {
            check.refno = tubes
                .rows
                .iter()
                .filter_map(|row| row.generated.as_deref())
                .find_map(|path| index.get(path))
                .map(|source| source.refno.clone());
            check.derived = Some(Derived {
                kind: DERIVED_CONTAINER_TUBES.to_string(),
                ordinal: 0,
                from: None,
                to: None,
            });
            check.noun = "TUBI".to_string();
        }
        None => {
            if let Some(source) = index.get(&pair.refno) {
                check = check.with_source(source);
            }
        }
    }
    check
}

/// 容器组内逐根账的一行：我们的一根管身（对上若干条 E3D 记录，或没对上），或 E3D 多画的一条记录。
fn container_row_check(
    pair: &Pair,
    owner: Option<&str>,
    row: &ContainerTubePair,
    index: &SourceIndex<'_>,
) -> ElementCheck {
    let grade = format!("container_{}", row.verdict);
    let mut check = match row.generated.as_deref() {
        Some(path) => {
            let check = ElementCheck::new(&grade, name_from_path(path), "TUBI".to_string());
            match index.get(path) {
                Some(source) => check.with_source(source),
                None => check,
            }
        }
        None => {
            let ordinal = row.baseline_records.first().map_or(0, |record| record + 1);
            let mut check = ElementCheck::new(
                &grade,
                format!("{} 容器组的第 {ordinal} 条记录", pair.key.trim()),
                "TUBI".to_string(),
            );
            check.refno = owner.map(str::to_string);
            check.derived = Some(Derived {
                kind: DERIVED_CONTAINER_RECORD.to_string(),
                ordinal,
                from: None,
                to: None,
            });
            check
        }
    }
    .with_problem(report::container_row_problem(row));
    check.max_axis_delta_mm = row.max_axis_delta_mm;
    check
}

/// 只在基准一侧、还没有 refno 的那几条按显示名找回来。`resolve` 收一组名字、回同样多的出处
/// （解不出的给 `None`）。返回 `(解出的条数, 没解出的条数)`；`resolve` 整个失败时这几条保持没解出，
/// 原因在 `Err` 里，结果照样能入库。
pub fn resolve_baseline_side(
    checks: &mut [ElementCheck],
    resolve: impl FnOnce(&[String]) -> Result<Vec<Option<MemberSource>>, String>,
) -> Result<(usize, usize), String> {
    let pending: Vec<usize> = checks
        .iter()
        .enumerate()
        .filter(|(_, check)| check.refno.is_none() && grade::baseline_side(&check.grade))
        .map(|(at, _)| at)
        .collect();
    if pending.is_empty() {
        return Ok((0, 0));
    }
    let names: Vec<String> = pending.iter().map(|at| checks[*at].name.clone()).collect();
    let resolved = resolve(&names)
        .map_err(|error| format!("{} 个基准成员的 refno 没有解出来：{error}", names.len()))?;
    if resolved.len() != names.len() {
        return Err(format!(
            "按名字解 refno 给回 {} 条，问的是 {} 条",
            resolved.len(),
            names.len()
        ));
    }
    let mut found = 0usize;
    for (at, source) in pending.iter().zip(resolved) {
        let Some(source) = source else { continue };
        let check = &mut checks[*at];
        check.refno = Some(source.refno);
        check.derived = source.derived.as_ref().map(Derived::from_member);
        if !source.noun.is_empty() {
            check.noun = source.noun;
        }
        found += 1;
    }
    Ok((found, pending.len() - found))
}

/// 从构件级结果数回各档条数：与同一份报告判出来的 [`RvmCounts`] 逐格相同（对账用）。
pub fn counts_of(checks: &[ElementCheck]) -> RvmCounts {
    let mut counts = RvmCounts::default();
    for check in checks {
        let paired = match check.grade.as_str() {
            grade::EXACT => Some(&mut counts.exact),
            grade::CHORD => Some(&mut counts.chord),
            grade::REFERENCE_ARTIFACT => Some(&mut counts.reference_artifact),
            grade::FILM => Some(&mut counts.film),
            grade::OFF => Some(&mut counts.off),
            _ => None,
        };
        if let Some(slot) = paired {
            *slot += 1;
            counts.both += 1;
            match check.kind_match {
                Some(true) => counts.kind_match += 1,
                Some(false) => counts.kind_differ += 1,
                None => {}
            }
            continue;
        }
        match check.grade.as_str() {
            grade::ONLY_BASELINE => counts.only_baseline += 1,
            grade::ONLY_GENERATED => counts.only_generated += 1,
            grade::UNKEYABLE_BASELINE => counts.unkeyable_baseline += 1,
            grade::UNKEYABLE_GENERATED => counts.unkeyable_generated += 1,
            grade::CENTRELINE_ONLY => counts.centreline_only_baseline += 1,
            grade::CONTAINER_OFF => counts.container_off += 1,
            grade::CONTAINER_ONLY_BASELINE => counts.container_only_baseline += 1,
            grade::CONTAINER_ONLY_GENERATED => counts.container_only_generated += 1,
            _ => {}
        }
    }
    counts
}

/// 作业目录里的构件级结果：生成即比对时由生成子进程写、父进程读（进程间运输件，与 `judgement.json`
/// 同一个角色）；读文件比对时由比对那一侧写。续跑补入库也读它。
pub(crate) const ELEMENTS_FILE: &str = "element-checks.json";

/// 不走生成即比对时，生成侧把每个成员的出处落在 `ours/` 下，比对那一步按成员路径接回 refno。
pub(crate) const SOURCES_FILE: &str = "member-sources.json";

const ELEMENTS_FILE_VERSION: u32 = 1;

#[derive(Serialize)]
struct ElementsFileOut<'a> {
    version: u32,
    rows: &'a [ElementCheck],
}

#[derive(Deserialize)]
struct ElementsFileIn {
    #[serde(default)]
    version: u32,
    rows: Vec<ElementCheck>,
}

fn write_compact(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let file = std::fs::File::create(path)
        .map_err(|error| format!("建 {} 失败：{error}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| format!("{} 无法序列化：{error}", path.display()))?;
    writer
        .flush()
        .map_err(|error| format!("写 {} 失败：{error}", path.display()))
}

/// 写 `<作业目录>/element-checks.json`（紧凑 JSON：整根 8 万件约 30 MB）。
pub(crate) fn write_checks(dir: &Path, checks: &[ElementCheck]) -> Result<(), String> {
    write_compact(
        &dir.join(ELEMENTS_FILE),
        &ElementsFileOut {
            version: ELEMENTS_FILE_VERSION,
            rows: checks,
        },
    )
}

/// 读回 [`write_checks`] 写的那一份。文件不在是 `Ok(None)`（老产物目录、没比对出结果）；在却读不成是 `Err`。
pub(crate) fn read_checks(dir: &Path) -> Result<Option<Vec<ElementCheck>>, String> {
    let path = dir.join(ELEMENTS_FILE);
    let reader = match std::fs::File::open(&path) {
        Ok(file) => std::io::BufReader::with_capacity(1 << 20, file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读 {} 失败：{error}", path.display())),
    };
    // 边读边解：整根 8 万件的文件近 40 MB，不先整个读进内存再解。
    let file: ElementsFileIn = serde_json::from_reader(reader)
        .map_err(|error| format!("解析 {} 失败：{error}", path.display()))?;
    if file.version != ELEMENTS_FILE_VERSION {
        return Err(format!(
            "{} 的版本是 {}，这个程序认的是 {ELEMENTS_FILE_VERSION}",
            path.display(),
            file.version
        ));
    }
    Ok(Some(file.rows))
}

/// 写 `<作业目录>/ours/member-sources.json`。
pub(crate) fn write_sources(task_dir: &Path, sources: &[MemberSource]) -> Result<(), String> {
    write_compact(&task_dir.join("ours").join(SOURCES_FILE), &sources)
}

/// 读回 [`write_sources`] 写的那一份；文件不在（老产物目录）给空表——生成侧的构件就都记成没解出 refno。
pub(crate) fn read_sources(task_dir: &Path) -> Result<Vec<MemberSource>, String> {
    let path = task_dir.join("ours").join(SOURCES_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读 {} 失败：{error}", path.display())),
    };
    serde_json::from_slice(&bytes).map_err(|error| format!("解析 {} 失败：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rvm_check::report::judge_report_full;
    use e3d_model::rvm_compare::{ContainerTubeReport, ContainerTubeTally, NounTally};
    use e3d_model::rvm_export::DerivedMember;

    fn rvm(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test_data/rvm")
            .join(name)
    }

    fn source(path: &str, refno: &str, noun: &str) -> MemberSource {
        MemberSource {
            path: path.to_string(),
            name: name_from_path(path),
            refno: refno.to_string(),
            noun: noun.to_string(),
            derived: None,
        }
    }

    fn tube_source(path: &str, owner: &str, ordinal: usize) -> MemberSource {
        MemberSource {
            derived: Some(DerivedMember {
                kind: e3d_model::rvm_export::DERIVED_IMPLIED_TUBE.to_string(),
                ordinal,
                from: Some("1/100".to_string()),
                to: Some("1/101".to_string()),
            }),
            ..source(path, owner, "TUBI")
        }
    }

    /// 一份真报告（同一份 RVM 自比对，全部配上）：每一对一条，refno 按成员路径接回来；各档条数与判色
    /// 的计数逐格相同；通过的件没有原因。
    #[test]
    fn every_paired_member_gets_a_row_with_its_refno_and_the_rows_add_up_to_the_counts() {
        let fixture = rvm("1RCV0101-B1.rvm");
        let report = e3d_model::rvm_compare::compare_rvm_files(&fixture, &fixture).unwrap();
        let judgement = judge_report_full(&report);
        assert!(judgement.counts.both > 0);
        // 生成侧每个成员给一个编出来的 refno：第 i 对是 9/<i+1>。
        let sources: Vec<MemberSource> = report
            .pairs
            .iter()
            .enumerate()
            .map(|(at, pair)| source(&pair.refno, &format!("9/{}", at + 1), "ELBO"))
            .collect();

        let checks = element_checks(&report, &sources);
        assert_eq!(counts_of(&checks), judgement.counts);
        // 配上的件排在最前、与报告同序（这份夹具没有并成一件的容器）。
        let paired = &checks[..report.pairs.len()];
        for (at, (check, pair)) in paired.iter().zip(&report.pairs).enumerate() {
            assert_eq!(check.refno.as_deref(), Some(format!("9/{}", at + 1).as_str()));
            assert_eq!(check.grade, pair.verdict);
            assert_eq!(check.pair_key.as_deref(), Some(pair.key.as_str()));
            assert_eq!(check.noun, "ELBO", "名词取出处给的，不是报告里读不出来的空串");
            assert_eq!(check.baseline_aabb, Some(pair.baseline_aabb));
            assert_eq!(check.aabb_frame.as_deref(), Some(pair.comparison_frame));
            assert_eq!(
                check.significant_max_axis_delta_mm,
                pair.significant_max_axis_delta_mm
            );
            assert!(check.passed() && check.severity == SEVERITY_OK);
            assert!(check.reason_kind.is_empty() && check.reason.is_empty());
        }

        // 出处一条都没有（老产物目录）：行照样全，只是 refno 空着、名字从成员路径还原。
        let bare = element_checks(&report, &[]);
        assert_eq!(counts_of(&bare), judgement.counts);
        assert!(bare.iter().all(|check| check.refno.is_none()));
        assert!(bare.iter().all(|check| !check.name.starts_with("/GENERATED/")));
    }

    /// 两份不相干的 RVM：全是单侧件与发不出主键的件。每条差异都有对应的构件行，原因与差异行逐字相同；
    /// 只在生成侧的按成员路径接回 refno，只在基准侧的等按名字解。
    #[test]
    fn one_sided_members_get_rows_whose_reasons_match_the_mismatch_rows() {
        let report = e3d_model::rvm_compare::compare_rvm_files(
            &rvm("1RCV0101-B1.rvm"),
            &rvm("1RS-WF03-W-C-RR001.rvm"),
        )
        .unwrap();
        let judgement = judge_report_full(&report);
        assert_eq!(
            report.only_generated_members.len(),
            report.only_generated.len()
        );
        assert_eq!(
            report.unkeyable_generated_members.len(),
            report.unkeyable_generated.len()
        );
        assert!(!report.only_generated.is_empty() || !report.unkeyable_generated.is_empty());
        for (entry, member) in report
            .only_generated
            .iter()
            .zip(&report.only_generated_members)
        {
            assert!(entry.starts_with(member.as_str()), "{entry} / {member}");
        }
        let sources: Vec<MemberSource> = report
            .only_generated_members
            .iter()
            .chain(&report.unkeyable_generated_members)
            .enumerate()
            .map(|(at, path)| source(path, &format!("7/{}", at + 1), "PANE"))
            .collect();

        let checks = element_checks(&report, &sources);
        assert_eq!(counts_of(&checks), judgement.counts);
        // 每条构件行的原因与同一件的差异行逐字相同（差异清单与构件清单同序；只记账的中心线不在差异清单里）。
        let problems: Vec<&ElementCheck> = checks
            .iter()
            .filter(|check| check.severity != SEVERITY_OK && check.severity != reason::INFO)
            .collect();
        assert_eq!(problems.len(), judgement.mismatches.len());
        for (check, mismatch) in problems.iter().zip(&judgement.mismatches) {
            assert_eq!(check.grade, mismatch.category);
            assert_eq!(check.reason_kind, mismatch.reason_kind);
            assert_eq!(check.reason, mismatch.reason);
            assert_eq!(check.severity, mismatch.severity);
            assert!(!check.passed());
        }
        for check in &checks {
            match check.grade.as_str() {
                grade::ONLY_GENERATED | grade::UNKEYABLE_GENERATED => {
                    assert!(check.refno.as_deref().is_some_and(|r| r.starts_with("7/")));
                    assert_eq!(check.noun, "PANE");
                }
                grade::ONLY_BASELINE | grade::UNKEYABLE_BASELINE => {
                    assert!(check.refno.is_none(), "基准侧的等按名字解");
                    assert!(!check.name.contains('（'), "名字里不带括号说明：{}", check.name);
                }
                grade::CENTRELINE_ONLY => assert!(check.refno.is_none()),
                other => panic!("不相干的两份 RVM 不该有 {other}"),
            }
        }
    }

    fn pair(key: &str, refno: &str, verdict: &'static str) -> Pair {
        Pair {
            key: key.to_string(),
            refno: refno.to_string(),
            noun: String::new(),
            max_axis_delta_mm: 0.01,
            raw_max_axis_delta_mm: 0.01,
            significant_max_axis_delta_mm: None,
            verdict,
            film_thickness_mm: None,
            film_side: None,
            aabb_source: "facets",
            comparison_frame: "local",
            kind_match: Some(true),
            baseline_kinds: vec!["Cylinder".to_string()],
            generated_kinds: vec!["Cylinder".to_string()],
            container_tubes: None,
            delta_min_mm: [0.0; 3],
            delta_max_mm: [0.0; 3],
            baseline_aabb: [0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            generated_aabb: [0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            significant_baseline_aabb: None,
            significant_generated_aabb: None,
            volume_mm3: 0.0,
            genus: 0,
            negatives_applied: 0,
            negatives_skipped: 0,
        }
    }

    fn report_of(pairs: Vec<Pair>) -> CompareReport {
        let mut by_noun: std::collections::BTreeMap<String, NounTally> = Default::default();
        let mut container_tubes = ContainerTubeTally::default();
        for pair in &pairs {
            let tally = by_noun.entry(pair.noun.clone()).or_default();
            tally.both += 1;
            match pair.verdict {
                "exact" => tally.exact += 1,
                "chord" => tally.chord += 1,
                _ => tally.off += 1,
            }
            match pair.kind_match {
                Some(true) => tally.kind_match += 1,
                Some(false) => tally.kind_differ += 1,
                None => {}
            }
            if let Some(tubes) = &pair.container_tubes {
                container_tubes.exact += tubes.tally.exact;
                container_tubes.chord += tubes.tally.chord;
                container_tubes.off += tubes.tally.off;
                container_tubes.only_baseline += tubes.tally.only_baseline;
                container_tubes.only_generated += tubes.tally.only_generated;
            }
        }
        CompareReport {
            baseline: String::new(),
            elements: String::new(),
            facets: None,
            aabb_from_facets: pairs.len(),
            aabb_from_snapshot: 0,
            dbnum: None,
            root_name: None,
            export_scope: Some("rvm".to_string()),
            baseline_members: Some(pairs.len()),
            baseline_geometries: Some(pairs.len()),
            unkeyable_generated: Vec::new(),
            unkeyable_baseline: Vec::new(),
            centreline_only_baseline: Vec::new(),
            by_noun,
            pairs,
            only_baseline: Vec::new(),
            only_generated: Vec::new(),
            only_generated_members: Vec::new(),
            unkeyable_generated_members: Vec::new(),
            container_tube_groups: 0,
            container_tubes,
        }
    }

    /// 隐式管身记在属主（BRAN）名下、带管号；超差的那一件带上与差异行相同的原因；种类不一致的件
    /// 包围盒照样算通过、单标 `warning`；只画中心线的不进判据。
    #[test]
    fn derived_tubes_sit_under_their_owner_and_problem_rows_carry_the_reason() {
        let mut off = pair("ELBO 1 /B1", "/GENERATED/ELBO 1 of BRAN /B1", "off");
        off.noun = "ELBO".to_string();
        off.max_axis_delta_mm = 109.0;
        off.baseline_aabb = [0.0, 0.0, 0.0, 100.0, 50.0, 20.0];
        off.generated_aabb = [109.0, 0.0, 0.0, 209.0, 50.0, 20.0];
        let mut kinds = pair(" /NUT", "/GENERATED/NUT", "exact");
        kinds.kind_match = Some(false);
        kinds.generated_kinds = vec!["Box".to_string()];
        let mut tube = pair("TUBI 2 /B1 ↔ TUBI 1 /B1", "/GENERATED/TUBI 1 of BRAN /B1", "exact");
        tube.noun = "TUBI".to_string();
        let mut report = report_of(vec![off, kinds, tube]);
        report.centreline_only_baseline = vec!["SNOD 1 of SCTN /S".to_string()];
        let sources = vec![
            source("/GENERATED/ELBO 1 of BRAN /B1", "1/21", "ELBO"),
            source("/GENERATED/NUT", "1/30", "CONE"),
            tube_source("/GENERATED/TUBI 1 of BRAN /B1", "1/20", 1),
        ];
        let judgement = judge_report_full(&report);

        let checks = element_checks(&report, &sources);
        assert_eq!(counts_of(&checks), judgement.counts);
        let [off, kinds, tube, line] = checks.as_slice() else {
            panic!("四条：{checks:?}");
        };
        assert_eq!(off.refno.as_deref(), Some("1/21"));
        assert!(!off.passed());
        assert_eq!(off.reason_kind, reason::AABB_SHIFTED);
        assert_eq!(off.reason, judgement.mismatches[0].reason);
        assert_eq!(off.severity, reason::ERROR);

        assert_eq!(kinds.name, "/NUT");
        assert!(kinds.passed(), "包围盒在容差内");
        assert_eq!(kinds.kind_match, Some(false));
        assert_eq!(kinds.severity, reason::WARNING);
        assert_eq!(kinds.reason_kind, judgement.mismatches[1].reason_kind);

        assert_eq!(tube.refno.as_deref(), Some("1/20"), "记在属主名下");
        let derived = tube.derived.as_ref().expect("派生件");
        assert_eq!(derived.kind, "implied_tube");
        assert_eq!(derived.ordinal, 1);
        assert_eq!(derived.from.as_deref(), Some("1/100"));
        assert_eq!(tube.noun, "TUBI");
        assert_eq!(tube.pair_key.as_deref(), Some("TUBI 2 /B1 ↔ TUBI 1 /B1"));

        assert_eq!(line.grade, grade::CENTRELINE_ONLY);
        assert_eq!(line.noun, "SNOD");
        assert_eq!(line.severity, reason::INFO);
        assert_eq!(line.reason, reason::CENTRELINE_ONLY_TEXT);
        assert!(line.passed() && line.refno.is_none());
    }

    /// E3D 把隐式管写在容器那一组里时：整组一条（记在容器名下），组内每一根、E3D 多画的每一条记录各一条，
    /// 档名带 `container_` 前缀，条数与 `counts` 的三格对得上。
    #[test]
    fn a_merged_container_gets_a_group_row_plus_one_row_per_tube_and_record() {
        let mut group = pair("BRAN /B1", "BRAN /B1 名下的隐式管", "off");
        group.noun = "BRAN".to_string();
        let rows = vec![
            ContainerTubePair {
                generated: Some("/GENERATED/TUBI 1 of BRAN /B1".to_string()),
                baseline_records: vec![0],
                max_axis_delta_mm: Some(0.0),
                verdict: "exact",
            },
            ContainerTubePair {
                generated: Some("/GENERATED/TUBI 2 of BRAN /B1".to_string()),
                baseline_records: vec![1],
                max_axis_delta_mm: Some(12.0),
                verdict: "off",
            },
            ContainerTubePair {
                generated: None,
                baseline_records: vec![2],
                max_axis_delta_mm: None,
                verdict: "only_baseline",
            },
            ContainerTubePair {
                generated: Some("/GENERATED/TUBI 3 of BRAN /B1".to_string()),
                baseline_records: Vec::new(),
                max_axis_delta_mm: None,
                verdict: "only_generated",
            },
        ];
        group.container_tubes = Some(ContainerTubeReport {
            tally: ContainerTubeTally {
                exact: 1,
                chord: 0,
                off: 1,
                only_baseline: 1,
                only_generated: 1,
            },
            rows,
        });
        let report = report_of(vec![group]);
        let sources: Vec<MemberSource> = (1..=3)
            .map(|n| tube_source(&format!("/GENERATED/TUBI {n} of BRAN /B1"), "1/20", n))
            .collect();
        let judgement = judge_report_full(&report);
        assert_eq!(judgement.counts.container_off, 1);

        let checks = element_checks(&report, &sources);
        assert_eq!(counts_of(&checks), judgement.counts);
        let grades: Vec<&str> = checks.iter().map(|check| check.grade.as_str()).collect();
        assert_eq!(
            grades,
            [
                grade::OFF,
                grade::CONTAINER_EXACT,
                grade::CONTAINER_OFF,
                grade::CONTAINER_ONLY_BASELINE,
                grade::CONTAINER_ONLY_GENERATED,
            ]
        );
        assert!(checks.iter().all(|check| check.refno.as_deref() == Some("1/20")));
        assert_eq!(checks[0].derived.as_ref().unwrap().kind, DERIVED_CONTAINER_TUBES);
        assert_eq!(checks[0].reason_kind, reason::CONTAINER_TUBE_MISMATCH);
        assert_eq!(checks[2].derived.as_ref().unwrap().ordinal, 2, "我们的 TUBI 2");
        assert_eq!(checks[2].reason_kind, reason::CONTAINER_TUBE_OFF);
        let record = checks[3].derived.as_ref().unwrap();
        assert_eq!((record.kind.as_str(), record.ordinal), (DERIVED_CONTAINER_RECORD, 3));
        assert_eq!(checks[3].reason_kind, reason::CONTAINER_TUBE_MISSING);
        assert_eq!(checks[4].reason_kind, reason::CONTAINER_TUBE_EXTRA);
        // 四条有问题的构件行（整组 + 三行）与差异行一一对应。
        let problems = checks
            .iter()
            .filter(|check| check.severity != SEVERITY_OK)
            .count();
        assert_eq!(problems, judgement.mismatches.len());
    }

    /// 基准侧的几条按名字解：解出的填上 refno / 名词 / 派生标记，解不出的保持空着；解析整个失败时
    /// 一条都不动，原因报回来。生成侧没接上出处的不拿去问名字。
    #[test]
    fn baseline_side_rows_are_resolved_by_name_and_the_rest_stay_unresolved() {
        let mut report = report_of(vec![pair(" /A", "/GENERATED/A", "exact")]);
        report.only_baseline = vec![
            "BOX 2 of EQUIPMENT /E1".to_string(),
            "TUBE 3 of BRANCH /B1".to_string(),
            "/GONE".to_string(),
        ];
        report.unkeyable_baseline =
            vec!["/NAMED（缺 noun/owner_desc/ordinal，且无同名生成元素）".to_string()];
        report.only_generated = vec!["/GENERATED/X  /X".to_string()];
        report.only_generated_members = vec!["/GENERATED/X".to_string()];
        let mut checks = element_checks(&report, &[]);
        assert_eq!(checks[1].noun, "BOX");
        assert_eq!(checks[2].noun, "TUBI");

        let mut asked = Vec::new();
        let outcome = resolve_baseline_side(&mut checks, |names| {
            asked = names.to_vec();
            Ok(vec![
                Some(source("", "1/12", "BOX")),
                Some(tube_source("", "1/20", 3)),
                None,
                Some(source("", "1/40", "EQUI")),
            ])
        });
        assert_eq!(outcome, Ok((3, 1)));
        assert_eq!(
            asked,
            ["BOX 2 of EQUIPMENT /E1", "TUBE 3 of BRANCH /B1", "/GONE", "/NAMED"]
        );
        assert_eq!(checks[0].refno, None, "生成侧没有出处的不问名字");
        assert_eq!(checks[1].refno.as_deref(), Some("1/12"));
        assert_eq!(checks[1].name, "BOX 2 of EQUIPMENT /E1", "名字保持 E3D 写的那个");
        assert_eq!(checks[2].refno.as_deref(), Some("1/20"));
        assert_eq!(checks[2].derived.as_ref().unwrap().ordinal, 3);
        assert_eq!(checks[3].refno, None);
        assert_eq!(checks[4].grade, grade::ONLY_GENERATED);
        assert_eq!(checks[4].name, "/X");
        assert_eq!(checks[5].refno.as_deref(), Some("1/40"));
        assert_eq!(checks[5].noun, "EQUI");

        // 已经都解过了：不再问。
        assert_eq!(
            resolve_baseline_side(&mut checks, |names| Ok(vec![None; names.len()])),
            Ok((0, 1))
        );
        let mut fresh = element_checks(&report, &[]);
        let error = resolve_baseline_side(&mut fresh, |_| Err("库打不开".to_string())).unwrap_err();
        assert!(error.contains("库打不开") && error.contains('4'), "{error}");
        assert!(fresh.iter().all(|check| check.refno.is_none()));
    }

    /// 同一个成员路径指向两个不同元素时两个都不认；指向同一个元素（重复登记）照认。
    #[test]
    fn an_ambiguous_member_path_is_left_unresolved() {
        let report = report_of(vec![
            pair("BOX 1", "/GENERATED/BOX 1", "exact"),
            pair(" /A", "/GENERATED/A", "exact"),
        ]);
        let sources = vec![
            source("/GENERATED/BOX 1", "1/1", "BOX"),
            source("/GENERATED/BOX 1", "1/2", "BOX"),
            source("/GENERATED/A", "1/3", "EQUI"),
            source("/GENERATED/A", "1/3", "EQUI"),
        ];
        let checks = element_checks(&report, &sources);
        assert_eq!(checks[0].refno, None);
        assert_eq!(checks[1].refno.as_deref(), Some("1/3"));
    }

    /// 作业目录里的两份文件写了读得回；文件不在时一个给 `None`、一个给空表。
    #[test]
    fn the_job_directory_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("rvm-check-elements-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ours")).unwrap();
        assert_eq!(read_checks(&dir), Ok(None));
        assert_eq!(read_sources(&dir), Ok(Vec::new()));

        let report = report_of(vec![pair("TUBI 1 /B1", "/GENERATED/TUBI 1 of BRAN /B1", "chord")]);
        let sources = vec![tube_source("/GENERATED/TUBI 1 of BRAN /B1", "1/20", 1)];
        let checks = element_checks(&report, &sources);
        write_checks(&dir, &checks).unwrap();
        write_sources(&dir, &sources).unwrap();
        assert_eq!(read_checks(&dir), Ok(Some(checks)));
        assert_eq!(read_sources(&dir), Ok(sources));
        // 紧凑 JSON、没值的格不写：通过的一行不带 reason / severity 之外的空格。
        let text = std::fs::read_to_string(dir.join(ELEMENTS_FILE)).unwrap();
        assert!(!text.contains('\n') && !text.contains("reason_kind"), "{text}");

        std::fs::write(dir.join(ELEMENTS_FILE), b"{\"version\":99,\"rows\":[]}").unwrap();
        assert!(read_checks(&dir).unwrap_err().contains("版本"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
