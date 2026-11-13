//! 一个节点两版之间的**属性净差**（plant3d-web ADR 0066 第四条 `element/attribute-diff` / 词条「属性净差」）。
//!
//! 与 [`crate::fast_model::attribute_history`] 的分工：时间线说的是 **谁在哪一版改了什么**（逐会话一行）；
//! 这条只回答 **A 与 B 两版之间它到底差在哪**——两个会话各钉一个 `DbSet`，属性面板同一个渲染器各出一次字
//! （[`ElementAttributeService::read_pinned`]），字不同就是变了。不走历史投影、不要快照、不折时间线：
//! 前端此前用 (A, B] 逐会话 before / after 折净差顶着，改过又改回去的要靠「两头一样就不算」补，
//! 这里两端直接读终态，天然就是净差；成员增删 / 重排与 owner 改挂也给的是两端的真差，不再是「动过」一句提示。
//!
//! `impact` 仍走 [`modified_impact`] 同一把尺子（mesh / placement / noop），A 侧不存在 → `created` / `delivery`，
//! B 侧不存在 → `deleted` / `tombstone`，两端都在且一字没差、影响也判不出 → `unchanged`。两端都不存在 404
//! `REFNO_NOT_FOUND`；A 不早于 B 400 `INVALID_VERSION_PAIR`（与差异摘要同口径）。
//! 两个会话集 + 两次渲染是毫秒级的活，不进缓存（ADR-081：按需算、不落库）。

use std::path::Path;
use std::time::Instant;

use e3d_io::refno::RefNo;
use serde::Serialize;

use crate::data_interface::db_file_header::session_chain;
use crate::data_interface::element_attributes::{AttributeValue, ElementAttributeService};
use crate::data_interface::generation_root::configured_delivery_unit_types;
use crate::data_interface::model_impact::OperationImpact;
use crate::fast_model::attribute_history::{
    AttributeChange, MemberChanges, changed_count, diff_attribute_tables, diff_members, read_side,
};
use crate::fast_model::e3d_model_service::E3dModelService;
use crate::fast_model::element_versions::{
    SessionSets, impact_of, modified_impact, resolve_unit_root,
};
use crate::fast_model::historical_model::source_service;
use crate::fast_model::unit_versions::{UnitVersionImpact, UnitVersionsError};

/// A → B 之间这条记录的去向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AttributeDiffKind {
    /// A 侧不存在、B 侧在：`changes` 列 B 侧全部已设的属性，`before` 一律为空
    Created,
    /// 两端都在，且有可见差别（或按模型影响判出记录改过）
    Modified,
    /// A 侧在、B 侧不存在：`changes` 列 A 侧全部已设的属性，`after` 一律为空
    Deleted,
    /// 两端都在、一字没差、影响也判不出：原样换页不是它的一版
    Unchanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AttributeDiffStats {
    pub sets_built: usize,
    pub set_build_ms: u128,
    /// 两端渲染属性表花的时间
    pub render_ms: u128,
    /// 模型影响判定花的时间
    pub impact_ms: u128,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct AttributeDiffReport {
    pub dbnum: u32,
    /// `a/b`
    pub refno: String,
    pub noun: String,
    pub unit_root: Option<String>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub a: u32,
    pub b: u32,
    pub kind: AttributeDiffKind,
    /// 与版本表同一词表：created → delivery，deleted → tombstone，modified → mesh / placement / noop；unchanged 为 `null`。
    pub impact: Option<UnitVersionImpact>,
    /// `changes.len() + members / owner 各算一项`
    pub changed_count: usize,
    pub changes: Vec<AttributeChange>,
    /// 成员表两端的真差（只在两端都在时才有意义）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<MemberChanges>,
    /// owner 改挂：`[A 侧, B 侧]`（`a/b`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<[String; 2]>,
    /// 某一端属性行渲染不出来的原因（模板缺失等）；此时 `changes` 可能为空但 `kind` / `impact` 仍成立
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes_unavailable: Option<String>,
    pub elapsed_ms: u128,
    pub stats: AttributeDiffStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn refno_text(refno: RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1)
}

/// 入口。同步、重 I/O（两个会话集 + 两端各渲染一次属性表），调用方负责 `spawn_blocking`。
pub fn attribute_diff(
    dbnum: u32,
    refno: RefNo,
    a: u32,
    b: u32,
    attributes: &ElementAttributeService,
) -> Result<AttributeDiffReport, UnitVersionsError> {
    if refno.dbno() != dbnum {
        return Err(UnitVersionsError::new(
            "INVALID_REFNO",
            format!(
                "refno {} 属于 dbnum {}，不是请求的 dbnum {dbnum}",
                refno_text(refno),
                refno.dbno()
            ),
        ));
    }
    let service = source_service()
        .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?;
    let source_file = service
        .source_file(dbnum)
        .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?
        .to_path_buf();
    compute(&service, &source_file, dbnum, refno, a, b, attributes)
}

/// A / B 都得在链上，且 A 链序在 B 之前（纯函数；与差异摘要同一口径、同一组错误码）。
pub fn pair_positions(
    chain: &[u32],
    dbnum: u32,
    a: u32,
    b: u32,
) -> Result<(usize, usize), UnitVersionsError> {
    let position = |sesno: u32| chain.iter().position(|candidate| *candidate == sesno);
    let (Some(pos_a), Some(pos_b)) = (position(a), position(b)) else {
        let missing = if position(a).is_none() { a } else { b };
        return Err(UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("会话 {missing} 不在 dbnum {dbnum} 的会话链上"),
        ));
    };
    if pos_a >= pos_b {
        return Err(UnitVersionsError::new(
            "INVALID_VERSION_PAIR",
            format!("A 必须早于 B（链序）：A={a} B={b}"),
        ));
    }
    Ok((pos_a, pos_b))
}

/// 两端在不在 + 模型影响 + 可见差别数 → 去向与影响档（纯函数）。
///
/// 两端都在时：影响判不出且一字没差 = `unchanged`；影响判出来是 `Skip` 但字没差 = `modified` / `noop`
/// （记录确实重写了，只是渲染出来的字一样——前端照实说「记录重写但字没变」）。
pub fn classify(
    exists_a: bool,
    exists_b: bool,
    impact: Option<OperationImpact>,
    visible_changes: usize,
) -> (AttributeDiffKind, Option<UnitVersionImpact>) {
    match (exists_a, exists_b) {
        (false, true) => (
            AttributeDiffKind::Created,
            Some(UnitVersionImpact::Delivery),
        ),
        (true, false) => (
            AttributeDiffKind::Deleted,
            Some(UnitVersionImpact::Tombstone),
        ),
        (true, true) => match impact {
            None if visible_changes == 0 => (AttributeDiffKind::Unchanged, None),
            None => (AttributeDiffKind::Modified, Some(UnitVersionImpact::Noop)),
            Some(operation) => (AttributeDiffKind::Modified, Some(impact_of(operation))),
        },
        (false, false) => (AttributeDiffKind::Unchanged, None),
    }
}

fn compute(
    service: &E3dModelService,
    source_file: &Path,
    dbnum: u32,
    refno: RefNo,
    a: u32,
    b: u32,
    attributes: &ElementAttributeService,
) -> Result<AttributeDiffReport, UnitVersionsError> {
    let started = Instant::now();
    let chain = session_chain(source_file)
        .map_err(|e| UnitVersionsError::new("SESSION_TIME_UNAVAILABLE", format!("{e:#}")))?;
    let latest = chain.last().map(|stamp| stamp.sesno).ok_or_else(|| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("{} 的会话链是空的", source_file.display()),
        )
    })?;
    let sesnos: Vec<u32> = chain.iter().map(|stamp| stamp.sesno).collect();
    pair_positions(&sesnos, dbnum, a, b)?;

    let mut warnings = Vec::new();
    let mut stats = AttributeDiffStats::default();
    let mut sets = SessionSets::open(service, dbnum)?;
    let set_a = sets.at(a)?;
    let set_b = sets.at(b)?;

    let node_at_a = set_a.element(refno);
    let node_at_b = set_b.element(refno);
    let exists_a = node_at_a.exists().map_err(|e| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("查 {} 在会话 {a} 是否存在失败: {e}", refno_text(refno)),
        )
    })?;
    let exists_b = node_at_b.exists().map_err(|e| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("查 {} 在会话 {b} 是否存在失败: {e}", refno_text(refno)),
        )
    })?;
    if !exists_a && !exists_b {
        return Err(UnitVersionsError::new(
            "REFNO_NOT_FOUND",
            format!("{} 在会话 {a} 与 {b} 都不存在", refno_text(refno)),
        ));
    }
    // noun 与所属单元从还在的那一侧读（B 优先：那是「现在」）
    let alive = if exists_b { &node_at_b } else { &node_at_a };
    let noun = alive.element_type().map_err(|e| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("读 {} 的类型失败: {e}", refno_text(refno)),
        )
    })?;

    let render_started = Instant::now();
    let side_a = exists_a.then(|| read_side(attributes, &set_a, &sets.pin(a), refno));
    let side_b = exists_b.then(|| read_side(attributes, &set_b, &sets.pin(b), refno));
    stats.render_ms = render_started.elapsed().as_millis();

    // 不存在的那一侧就是空表：created 列 B 侧全部已设属性（before 空），deleted 反之
    let before_table: &[AttributeValue] = side_a
        .as_ref()
        .and_then(|side| side.attributes.as_deref())
        .unwrap_or(&[]);
    let after_table: &[AttributeValue] = side_b
        .as_ref()
        .and_then(|side| side.attributes.as_deref())
        .unwrap_or(&[]);
    // 某一端存在但渲染失败：不能拿空表去比（会把全部属性报成「删光 / 新增」），改成不列、只报原因
    let unavailable = side_a
        .as_ref()
        .and_then(|side| side.unavailable.clone())
        .or_else(|| side_b.as_ref().and_then(|side| side.unavailable.clone()));
    let render_ok = side_a
        .as_ref()
        .map_or(true, |side| side.attributes.is_some())
        && side_b
            .as_ref()
            .map_or(true, |side| side.attributes.is_some());
    let changes: Vec<AttributeChange> = if render_ok {
        diff_attribute_tables(before_table, after_table)
    } else {
        Vec::new()
    };

    let (members, owner, impact) = match (&side_a, &side_b) {
        (Some(before), Some(after)) => {
            let members = {
                let delta = diff_members(&before.members, &after.members);
                (!delta.is_empty()).then_some(delta)
            };
            let owner = match (before.owner, after.owner) {
                (Some(x), Some(y)) if x != y => Some([refno_text(x), refno_text(y)]),
                _ => None,
            };
            let impact_started = Instant::now();
            let impact = match modified_impact(&set_a, &set_b, refno, b) {
                Ok(operation) => operation,
                Err(reason) => {
                    warnings.push(format!("模型影响判定失败：{reason}，按重生成记"));
                    Some(OperationImpact::Regen)
                }
            };
            stats.impact_ms = impact_started.elapsed().as_millis();
            (members, owner, impact)
        }
        _ => (None, None, None),
    };
    let count = changed_count(&changes, members.as_ref(), owner.as_ref());
    let (kind, impact) = classify(exists_a, exists_b, impact, count);

    let unit_types = configured_delivery_unit_types();
    let mut unit_root = None;
    let mut unit_noun = None;
    let alive_set = if exists_b { &set_b } else { &set_a };
    match resolve_unit_root(alive_set, refno, &unit_types) {
        Ok(Some((root, root_noun))) => {
            unit_root = Some(root);
            unit_noun = Some(root_noun);
        }
        Ok(None) => warnings.push(format!(
            "{} 的 owner 链上没有最小交付单元（类型集 {}）",
            refno_text(refno),
            unit_types.join(" / ")
        )),
        Err(reason) => warnings.push(format!("解所属单元失败：{reason}")),
    }
    stats.sets_built = sets.built;
    stats.set_build_ms = sets.build_time.as_millis();

    Ok(AttributeDiffReport {
        dbnum,
        refno: refno_text(refno),
        noun,
        unit_root: unit_root.map(refno_text),
        unit_noun,
        file_latest_sesno: latest,
        a,
        b,
        kind,
        impact,
        changed_count: count,
        changes,
        members,
        owner,
        attributes_unavailable: unavailable,
        elapsed_ms: started.elapsed().as_millis(),
        stats,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_must_be_on_chain_and_in_chain_order() {
        let chain = [1, 5, 7, 9];
        assert_eq!(pair_positions(&chain, 8000, 5, 9).expect("ok"), (1, 3));
        let missing = pair_positions(&chain, 8000, 6, 9).expect_err("6 不在链上");
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
        assert!(missing.message.contains("会话 6"));
        let missing_b = pair_positions(&chain, 8000, 5, 8).expect_err("8 不在链上");
        assert!(missing_b.message.contains("会话 8"));
        let reversed = pair_positions(&chain, 8000, 9, 5).expect_err("A 不早于 B");
        assert_eq!(reversed.code, "INVALID_VERSION_PAIR");
        let same = pair_positions(&chain, 8000, 5, 5).expect_err("A == B");
        assert_eq!(same.code, "INVALID_VERSION_PAIR");
    }

    #[test]
    fn classify_maps_presence_and_impact_onto_the_version_vocabulary() {
        assert_eq!(
            classify(false, true, None, 12),
            (
                AttributeDiffKind::Created,
                Some(UnitVersionImpact::Delivery)
            )
        );
        assert_eq!(
            classify(true, false, None, 12),
            (
                AttributeDiffKind::Deleted,
                Some(UnitVersionImpact::Tombstone)
            )
        );
        assert_eq!(
            classify(true, true, None, 0),
            (AttributeDiffKind::Unchanged, None),
            "差分为空、一字没差 = 原样换页"
        );
        assert_eq!(
            classify(true, true, None, 1),
            (AttributeDiffKind::Modified, Some(UnitVersionImpact::Noop)),
            "模型口径没差但渲染出来的字变了（UDA 之类）：算改过、几何 noop"
        );
        assert_eq!(
            classify(true, true, Some(OperationImpact::Skip), 0),
            (AttributeDiffKind::Modified, Some(UnitVersionImpact::Noop)),
            "记录重写但字没变：照实说 modified / noop"
        );
        assert_eq!(
            classify(true, true, Some(OperationImpact::TransformOnly), 2),
            (
                AttributeDiffKind::Modified,
                Some(UnitVersionImpact::Placement)
            )
        );
        assert_eq!(
            classify(true, true, Some(OperationImpact::Regen), 3),
            (AttributeDiffKind::Modified, Some(UnitVersionImpact::Mesh))
        );
    }

    #[test]
    fn report_serializes_lowercase_kind_and_null_impact_when_unchanged() {
        let report = AttributeDiffReport {
            dbnum: 8000,
            refno: "24384/23262".into(),
            noun: "FTUB".into(),
            unit_root: Some("24384/23257".into()),
            unit_noun: Some("BRAN".into()),
            file_latest_sesno: 640,
            a: 626,
            b: 630,
            kind: AttributeDiffKind::Unchanged,
            impact: None,
            changed_count: 0,
            changes: Vec::new(),
            members: None,
            owner: None,
            attributes_unavailable: None,
            elapsed_ms: 3,
            stats: AttributeDiffStats::default(),
            warnings: Vec::new(),
        };
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["kind"], "unchanged");
        assert!(json["impact"].is_null());
        assert!(json.get("members").is_none(), "空成员差不占字段");
        assert!(json.get("owner").is_none());
        assert!(json.get("warnings").is_none());
        let modified = AttributeDiffReport {
            kind: AttributeDiffKind::Modified,
            impact: Some(UnitVersionImpact::Placement),
            changed_count: 2,
            changes: vec![AttributeChange {
                name: "POS".into(),
                value_type: "POSITION".into(),
                before: Some("E 1 N 2 U 3400".into()),
                after: Some("E 1 N 2 U 2900".into()),
                stamp: false,
            }],
            owner: Some(["24384/1".into(), "24384/2".into()]),
            ..report
        };
        let json = serde_json::to_value(&modified).expect("serialize");
        assert_eq!(json["kind"], "modified");
        assert_eq!(json["impact"], "placement");
        assert_eq!(json["changes"][0]["before"], "E 1 N 2 U 3400");
        assert_eq!(json["owner"][1], "24384/2");
    }
}
