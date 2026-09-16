//! 刚体前移（ADR-066）——「只动方位」的窗口不重算网格，只把投影记录左乘一个刚体增量，再按
//! regen 同款发布一次。本模块是它的**纯函数层**：分桶判据、锚点匹配、记录前移、线格式，一个
//! I/O 都没有，`mem://` 之外可测。文件侧（候选链、`Δ`、三条排除）由
//! `window_root_plan::WindowRootSources::collect` 算好喂进来（S4b B4）；发布侧（投影提交、
//! `persist_root_for_file_round`）在 S4b B6 接。计划：
//! `docs/plans/2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md`。
//!
//! 三条纪律：
//!
//! - **`Δ` 是世界系的**：`Δ_a = W_T(a)·W_S(a)⁻¹`，两端世界矩阵在文件侧算。它已含 `a` 之上所有
//!   锚点的运动，所以每条记录只乘**最近锚点祖先**那一个 `Δ`（[`nearest_anchor`]）——乘外层的
//!   会漏掉内层自己的运动，两个都乘是重复计算（ADR-066 D3）。
//! - **盒的难点是旋转**：存下来的 `world_bounds` 是世界网格顶点的紧 AABB，纯平移下加 `t` 精确，
//!   含旋转时任何「盒过矩阵」都松、过不了对拍门——那时由调用方注入的闭包读网格顶点重算
//!   （[`republish_records`] 的 `bounds` 参数）。
//! - **保守**：归不了因、判不了刚、没安定、退化、级联扩展进来的根，一律留在 `regen`。便宜路径
//!   多算一个就是把一次真几何变更降级成刷矩阵，而且没有任何东西会报。

use std::collections::{BTreeMap, BTreeSet};

use aios_core::RefnoEnum;
use glam::{DMat3, DMat4, DVec3, DVec4};
use serde::{Deserialize, Serialize};

use crate::data_interface::generation_root::{GenerationRoot, refno_to_e3d};
use crate::fast_model::model_memory_store::{ProjectedGeometry, packed_refno};

/// 线性部分与单位阵逐元素之差小于它就当纯平移。
pub const PURE_TRANSLATION_EPSILON: f64 = 1e-9;

/// 一个只动了摆放的锚点与它的刚体增量（世界系，`W_T·W_S⁻¹`）。
#[derive(Debug, Clone, PartialEq)]
pub struct TransformAnchor {
    pub refno: RefnoEnum,
    pub delta: DMat4,
}

/// `transform` 桶里的一根：走便宜路，名下记录按 [`nearest_anchor`] 各乘各的 `Δ`。
#[derive(Debug, Clone, PartialEq)]
pub struct TransformRoot {
    pub root: GenerationRoot,
    pub anchors: Vec<TransformAnchor>,
}

/// 一个窗口候选对便宜路径的意义。
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateKind {
    /// 只动了摆放（e3d-model `ElementDiff::is_placement_only`）、两端世界矩阵都算得出。
    /// `rigid` 是排除的判决（ADR-066 D1 规则 3 及 2026-09-15 补条，`window_root_plan::anchor_lineage_exclusion`）：
    /// `Err(原因)` = 它在路由容器 / 点容器 / `CWBRAN` / 规则 M 结构件之内，或自己是桥架件 / P23 摆放派生 noun /
    /// 负体 / 剖面数据 / 几何集成员，或不是单元却挂在消费成员的属主下——位姿变化是几何变化，不刚。
    Anchor {
        delta: DMat4,
        rigid: Result<(), String>,
    },
    /// 别的任何改动：新建 / 删除 / 改挂 / 换类型 / 动成员 / 动了别的属性 / opaque / 读不出来。
    Other(String),
}

/// 一个窗口候选连同它在两端的属主链（不含自身）。T 端链来自 `index.owners`；S 端链只有
/// 删除 / 改挂 / 换类型的候选才有——它们在 T 端不存在或换了地方，旧根只有 S 端说得出。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowCandidate {
    pub refno: RefnoEnum,
    pub kind: CandidateKind,
    pub chain_t: Vec<RefnoEnum>,
    pub chain_s: Vec<RefnoEnum>,
}

/// [`classify_transform_roots`] 的产物。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransformClassification {
    /// 过了全部判据的根及其锚点。
    pub transform: BTreeMap<RefnoEnum, Vec<TransformAnchor>>,
    /// 被波及、却不能走便宜路的根与第一条原因——只供 warnings，它们照旧在 `regen` 里。
    pub rejected: Vec<(RefnoEnum, String)>,
}

/// 纯函数：从被波及的根里挑出能走刚体前移的（ADR-066 D1 五条规则）。
///
/// - `roots_t`：T 端枚举出的生成根；`root_ancestors_t`：每个根在 T 端的属主链（不含自身），
///   锚点在根**之上**（挪 ZONE）时靠它归因。
/// - `settled`：`roots_S` 里已发布且安定的根（`ready ∧ desired == published ∧ 状态在
///   `SETTLED_ROOT_STATUSES` ∧ 凭证 > 0`）。
/// - `touched`：`expand_touched` **之前**的波及集；`expanded`：三条级联扩展（C2 / C8 / P4）加进来
///   的根——规则 5，一律 `regen`。
/// - `degraded`：`WindowImpact::degraded_reason()`，非空则整窗没有便宜路（规则 4）。
/// - `candidates`：账本条目按 refno 去重后的候选。
/// - `enabled`：开关（`options::model_transform_cheap_path`）。关着返回默认值——`transform` 空、
///   `rejected` 也空：关着不是拒绝，是没有这条路。
pub fn classify_transform_roots(
    roots_t: &[GenerationRoot],
    root_ancestors_t: &BTreeMap<RefnoEnum, Vec<RefnoEnum>>,
    settled: &BTreeSet<RefnoEnum>,
    touched: &BTreeSet<RefnoEnum>,
    expanded: &BTreeSet<RefnoEnum>,
    degraded: Option<&str>,
    candidates: &[WindowCandidate],
    enabled: bool,
) -> TransformClassification {
    let mut out = TransformClassification::default();
    if !enabled {
        return out;
    }
    for root in roots_t {
        if !touched.contains(&root.root) {
            continue;
        }
        let chain = root_ancestors_t
            .get(&root.root)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        match classify_one(root.root, chain, settled, expanded, degraded, candidates) {
            Ok(anchors) => {
                out.transform.insert(root.root, anchors);
            }
            Err(reason) => out.rejected.push((root.root, reason)),
        }
    }
    out
}

fn classify_one(
    root: RefnoEnum,
    root_chain_t: &[RefnoEnum],
    settled: &BTreeSet<RefnoEnum>,
    expanded: &BTreeSet<RefnoEnum>,
    degraded: Option<&str>,
    candidates: &[WindowCandidate],
) -> Result<Vec<TransformAnchor>, String> {
    if let Some(reason) = degraded {
        return Err(format!("window degraded: {reason}"));
    }
    if !settled.contains(&root) {
        return Err("root is not a settled persisted root".into());
    }
    if expanded.contains(&root) {
        return Err(
            "reached through a cascade expansion (C2 / C8 / P4), not a rigid motion".into(),
        );
    }
    // 归因两个方向都要看：候选在根之下（或就是根）——候选的链经过根；候选在根之上——根的链
    // 经过候选。删除 / 改挂的候选还要看 S 端链，旧根只有它说得出。
    let touching: Vec<&WindowCandidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.refno == root
                || candidate.chain_t.contains(&root)
                || candidate.chain_s.contains(&root)
                || root_chain_t.contains(&candidate.refno)
        })
        .collect();
    if touching.is_empty() {
        return Err("touched, but no candidate could be attributed to it".into());
    }
    let mut anchors = Vec::with_capacity(touching.len());
    for candidate in touching {
        match &candidate.kind {
            CandidateKind::Anchor {
                delta,
                rigid: Ok(()),
            } => anchors.push(TransformAnchor {
                refno: candidate.refno,
                delta: *delta,
            }),
            CandidateKind::Anchor {
                rigid: Err(reason), ..
            }
            | CandidateKind::Other(reason) => {
                return Err(format!("{}: {reason}", candidate.refno.to_pdms_str()));
            }
        }
    }
    Ok(anchors)
}

/// 记录该乘哪个 `Δ`：`ancestors` 是「自身 → 顶层」序，第一个命中锚点集的就是最近的锚点祖先
/// （最内层）。没有命中 = 这条记录不在任何锚点之下，矩阵不动。
pub fn nearest_anchor<'a>(
    record: &ProjectedGeometry,
    anchors: &'a [TransformAnchor],
) -> Option<&'a TransformAnchor> {
    record.ancestors.iter().find_map(|packed| {
        anchors
            .iter()
            .find(|anchor| packed_refno(refno_to_e3d(anchor.refno)) == *packed)
    })
}

/// `Δ` 是不是纯平移：线性部分与单位阵逐元素之差都小于 [`PURE_TRANSLATION_EPSILON`]、且最后一行
/// 是 `(0, 0, 0, 1)`，返回平移量。旋转 / 缩放 / 投影任一不满足都回 `None`。
pub fn pure_translation(delta: &DMat4) -> Option<DVec3> {
    let linear = DMat3::from_mat4(*delta);
    let linear_is_identity = (0..3).all(|column| {
        (linear.col(column) - DMat3::IDENTITY.col(column))
            .abs()
            .max_element()
            < PURE_TRANSLATION_EPSILON
    });
    let affine = (delta.row(3) - DVec4::new(0.0, 0.0, 0.0, 1.0))
        .abs()
        .max_element()
        < PURE_TRANSLATION_EPSILON;
    (linear_is_identity && affine).then(|| delta.w_axis.truncate())
}

/// 盒平移：两角各加 `t`（f32 存储，f64 算完再截）。
pub fn translate_bounds(bounds: [[f32; 3]; 2], t: DVec3) -> [[f32; 3]; 2] {
    let shift = |corner: [f32; 3]| {
        [
            (corner[0] as f64 + t.x) as f32,
            (corner[1] as f64 + t.y) as f32,
            (corner[2] as f64 + t.z) as f32,
        ]
    };
    [shift(bounds[0]), shift(bounds[1])]
}

/// 一组世界坐标点的 AABB。旋转时调用方把网格顶点乘完 `world'` 喂进来；空集回 `None`。
pub fn bounds_of_points(points: impl IntoIterator<Item = DVec3>) -> Option<[[f32; 3]; 2]> {
    let mut iter = points.into_iter();
    let first = iter.next()?;
    let (mut min, mut max) = (first, first);
    for point in iter {
        min = min.min(point);
        max = max.max(point);
    }
    Some([
        [min.x as f32, min.y as f32, min.z as f32],
        [max.x as f32, max.y as f32, max.z as f32],
    ])
}

/// 把一根的记录按锚点前移到 `target_sesno`（ADR-066 D3 / D4 的记录半边）。
///
/// - 在某个锚点之下的记录：`world' = Δ · world`；盒：纯平移直接加 `t`，含旋转交给 `bounds`
///   闭包（读网格顶点重算，见 [`bounds_of_points`]）。
/// - 不在任何锚点之下的记录：矩阵与盒不动。
/// - **每条**记录的 `source_sesno` 都改成 `target_sesno`：同根记录的会话必须一致，
///   `persist_root_chunks_impl` 断言它。
/// - `record_id` / `geometry_id` / `mesh_id` / `shape` / `tube` / `ancestors` 一个字不动。
pub fn republish_records(
    records: Vec<ProjectedGeometry>,
    anchors: &[TransformAnchor],
    target_sesno: u32,
    mut bounds: impl FnMut(&ProjectedGeometry, DMat4) -> anyhow::Result<[[f32; 3]; 2]>,
) -> anyhow::Result<Vec<ProjectedGeometry>> {
    records
        .into_iter()
        .map(|mut record| {
            if let Some(anchor) = nearest_anchor(&record, anchors) {
                let world = anchor.delta * record.world_transform;
                record.world_bounds = match pure_translation(&anchor.delta) {
                    Some(t) => translate_bounds(record.world_bounds, t),
                    None => bounds(&record, world)?,
                };
                record.world_transform = world;
            }
            record.source_sesno = target_sesno;
            Ok(record)
        })
        .collect()
}

/// 线格式里的矩阵：glam 列主序 16 个 f64。相等**按位**比——线格式要能 `Eq`（`ModelUpdatePlan` 与
/// `IncrementUpdateAttempt` 都 derive 了 `Eq`），而刚体增量里不会出现 NaN。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatrixWire(pub [f64; 16]);

impl PartialEq for MatrixWire {
    fn eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}

impl Eq for MatrixWire {}

/// 随计划携带的锚点（[`TransformUnit::anchors`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformAnchorWire {
    pub refno: String,
    pub delta: MatrixWire,
}

impl From<&TransformAnchor> for TransformAnchorWire {
    fn from(anchor: &TransformAnchor) -> Self {
        Self {
            refno: anchor.refno.to_pdms_str(),
            delta: MatrixWire(anchor.delta.to_cols_array()),
        }
    }
}

impl From<&TransformAnchorWire> for TransformAnchor {
    fn from(wire: &TransformAnchorWire) -> Self {
        Self {
            refno: RefnoEnum::from(wire.refno.as_str()),
            delta: DMat4::from_cols_array(&wire.delta.0),
        }
    }
}

/// `ModelUpdatePlan.transform_units` 的一条：这个 `RegenRoot` 可以按刚体前移便宜地做。
///
/// 它是 `regen` 的**子集加提示**，不是第五个去向：计划说的是「这根要带到 T」，怎么带由执行体定
/// ——试便宜路，任何一步不成就照旧整根重算（ADR-066 D6）。所以 `work_items` 里它仍是
/// `RegenRoot`、`units` 里它仍 `will_generate`；pending drain 与不认识提示的执行体看到的就是今天的
/// 计划，一字不差。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformUnit {
    /// 生成根，`a/b`。
    pub root_refno: String,
    pub noun: String,
    /// 规划器算 `Δ` 时的窗口基线 S（`W_S` 的会话）。执行体的重放闸（[`republish_gate`]）拿它对
    /// 投影回执：回执不在 S 上，`Δ` 就对不上记录。旧计划的线格式没有这一格，`serde(default)`
    /// 补 0——0 不会等于任何真实回执会话，基线闸把它挡回 regen，保守正确。
    #[serde(default)]
    pub source_start_sesno: u32,
    pub anchors: Vec<TransformAnchorWire>,
}

impl TransformUnit {
    pub fn anchors(&self) -> Vec<TransformAnchor> {
        self.anchors.iter().map(TransformAnchor::from).collect()
    }
}

/// 重放 / 基线闸的判决（ADR-066 B6 幂等性，S4b 审核 P0）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepublishGate {
    /// 回执在规划基线 S 上：`Δ` 对得上，可以左乘。
    Proceed,
    /// 回执已在 T：这一窗已经前移过（发布成功但回合没收尾就崩 / 水位没推进的重放）。
    /// 再乘一次 `Δ` 是双重搬移——跳过，按已完成计。
    SkipAlreadyAtTarget,
    /// 回执既不在 S 也不在 T（半截状态 / 旧计划线格式 / 别的路径动过它）：`Δ` 对不上，
    /// 回退整根重算——regen 从文件@T 重算，天然幂等，永远正确。
    FallbackBaselineMismatch,
}

/// transform 不像 regen 天然幂等：`Δ = W_T·W_S⁻¹` 只对「投影还停在 S」的记录成立，记录已在 T
/// 再乘一次就是双重搬移，而且不会有任何报错。执行体读出旧回执后、动记录之前先过这道闸。
pub fn republish_gate(
    receipt_sesno: u32,
    planned_base_sesno: u32,
    target_sesno: u32,
) -> RepublishGate {
    if receipt_sesno == target_sesno {
        RepublishGate::SkipAlreadyAtTarget
    } else if receipt_sesno != planned_base_sesno {
        RepublishGate::FallbackBaselineMismatch
    } else {
        RepublishGate::Proceed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::{GenerationRootKind, refno_from_e3d};
    use crate::fast_model::model_memory_store::{ProjectedShape, ProjectedTube};
    use e3d_io::refno::RefNo;
    use e3d_model::elmodl::GeometryId;

    fn r(n: u32) -> RefnoEnum {
        refno_from_e3d(RefNo::new(24384, n))
    }

    fn root(n: u32, noun: &str) -> GenerationRoot {
        GenerationRoot {
            root: r(n),
            noun: noun.into(),
            name: format!("/{noun}-{n}"),
            kind: GenerationRootKind::DeliveryUnit,
        }
    }

    fn translation(x: f64, y: f64, z: f64) -> DMat4 {
        DMat4::from_translation(DVec3::new(x, y, z))
    }

    fn rotation_z_90() -> DMat4 {
        DMat4::from_rotation_z(std::f64::consts::FRAC_PI_2)
    }

    fn anchor(n: u32, delta: DMat4) -> WindowCandidate {
        WindowCandidate {
            refno: r(n),
            kind: CandidateKind::Anchor {
                delta,
                rigid: Ok(()),
            },
            chain_t: Vec::new(),
            chain_s: Vec::new(),
        }
    }

    fn other(n: u32, reason: &str) -> WindowCandidate {
        WindowCandidate {
            refno: r(n),
            kind: CandidateKind::Other(reason.into()),
            chain_t: Vec::new(),
            chain_s: Vec::new(),
        }
    }

    fn chain(candidate: WindowCandidate, chain_t: &[u32]) -> WindowCandidate {
        WindowCandidate {
            chain_t: chain_t.iter().map(|n| r(*n)).collect(),
            ..candidate
        }
    }

    fn set(ns: &[u32]) -> BTreeSet<RefnoEnum> {
        ns.iter().map(|n| r(*n)).collect()
    }

    fn ancestors(pairs: &[(u32, &[u32])]) -> BTreeMap<RefnoEnum, Vec<RefnoEnum>> {
        pairs
            .iter()
            .map(|(root, chain)| (r(*root), chain.iter().map(|n| r(*n)).collect()))
            .collect()
    }

    fn record(id: &str, ancestors: &[u32], world: DMat4) -> ProjectedGeometry {
        let source = RefNo::new(24384, ancestors[0]);
        ProjectedGeometry {
            record_id: id.to_string(),
            geometry_id: GeometryId::Element {
                refno: source.to_string(),
            },
            source_refno: source,
            dbnum: 7997,
            source_sesno: 40,
            noun: "BOX".into(),
            mesh_id: format!("mesh_{id}"),
            world_transform: world,
            world_bounds: [[0.0, 0.0, 0.0], [1.0, 2.0, 3.0]],
            ancestors: ancestors
                .iter()
                .map(|n| packed_refno(RefNo::new(24384, *n)))
                .collect(),
            shape: ProjectedShape::Baked,
            tube: None,
        }
    }

    // 层级：ZONE 1 ─┬─ EQUI 10（根）─── BOX 11
    //               └─ PIPE 20 ─── BRAN 21（根）─── ELBO 22
    //          ZONE 2 ─── EQUI 30（根）─── SUBE 31 ─── BOX 32
    const ROOTS: [(u32, &str); 3] = [(10, "EQUI"), (21, "BRAN"), (30, "EQUI")];

    fn roots() -> Vec<GenerationRoot> {
        ROOTS.iter().map(|(n, noun)| root(*n, noun)).collect()
    }

    fn root_chains() -> BTreeMap<RefnoEnum, Vec<RefnoEnum>> {
        ancestors(&[(10, &[1]), (21, &[20, 1]), (30, &[2])])
    }

    fn classify(
        touched: &[u32],
        expanded: &[u32],
        degraded: Option<&str>,
        candidates: &[WindowCandidate],
        enabled: bool,
    ) -> TransformClassification {
        classify_transform_roots(
            &roots(),
            &root_chains(),
            &set(&[10, 21, 30]),
            &set(touched),
            &set(expanded),
            degraded,
            candidates,
            enabled,
        )
    }

    /// 锚点即根：EQUI 自己的 POS 变了，它就是自己的锚点。
    #[test]
    fn a_root_that_is_itself_a_rigid_placement_anchor_goes_to_transform() {
        let out = classify(
            &[10],
            &[],
            None,
            &[chain(anchor(10, translation(5.0, 0.0, 0.0)), &[1])],
            true,
        );
        assert_eq!(out.rejected, Vec::new());
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(10)]
        );
        assert_eq!(out.transform[&r(10)][0].refno, r(10));
    }

    /// 锚点在根之上：挪 ZONE 1，名下 EQUI 10 与 BRAN 21 都进 transform、各带同一个锚点；
    /// ZONE 2 下的 EQUI 30 不被波及也就不出现。
    #[test]
    fn an_anchor_above_the_roots_carries_every_settled_root_beneath_it() {
        let out = classify(
            &[10, 21],
            &[],
            None,
            &[anchor(1, translation(0.0, 100.0, 0.0))],
            true,
        );
        assert_eq!(out.rejected, Vec::new());
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(10), r(21)]
        );
        assert!(
            out.transform
                .values()
                .all(|anchors| anchors.len() == 1 && anchors[0].refno == r(1))
        );
    }

    /// 同根另有一个非位姿候选（ELBO 的 DIAM 变了）：BRAN 21 回 regen 并点名原因；
    /// 没被那个候选碰到的 EQUI 10 照旧 transform。
    #[test]
    fn a_non_placement_candidate_under_the_root_forces_regen_for_that_root_only() {
        let out = classify(
            &[10, 21],
            &[],
            None,
            &[
                anchor(1, translation(0.0, 100.0, 0.0)),
                chain(other(22, "DIAM changed"), &[21, 20, 1]),
            ],
            true,
        );
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(10)]
        );
        assert_eq!(out.rejected.len(), 1);
        assert_eq!(out.rejected[0].0, r(21));
        assert!(
            out.rejected[0].1.contains("DIAM changed"),
            "{}",
            out.rejected[0].1
        );
    }

    /// 锚点在 BRAN 之内（挪一个管件）：三条排除之一，位姿变化是几何变化。
    #[test]
    fn an_anchor_inside_a_route_container_sends_the_root_back_to_regen() {
        let inside = WindowCandidate {
            kind: CandidateKind::Anchor {
                delta: translation(1.0, 0.0, 0.0),
                rigid: Err("anchor inside BRAN 24384/21".into()),
            },
            ..chain(anchor(22, DMat4::IDENTITY), &[21, 20, 1])
        };
        let out = classify(&[21], &[], None, &[inside], true);
        assert!(out.transform.is_empty());
        assert_eq!(out.rejected[0].0, r(21));
        assert!(out.rejected[0].1.contains("inside BRAN"));
    }

    /// 删除候选只在 S 端链上（T 端已经没有它）：靠 `chain_s` 归因，根回 regen。
    #[test]
    fn a_deleted_candidate_attributed_through_the_base_chain_forces_regen() {
        let deleted = WindowCandidate {
            chain_s: vec![r(30), r(2)],
            ..other(33, "deleted at target")
        };
        let out = classify(
            &[30],
            &[],
            None,
            &[anchor(2, translation(0.0, 0.0, 7.0)), deleted],
            true,
        );
        assert!(out.transform.is_empty());
        assert!(out.rejected[0].1.contains("deleted at target"));
    }

    /// 级联扩展（C2 / C8 / P4）进来的根不是刚体运动：即便它自己的锚点齐全也回 regen。
    #[test]
    fn roots_reached_through_cascade_expansion_never_transform() {
        let out = classify(
            &[10, 30],
            &[30],
            None,
            &[
                anchor(1, translation(1.0, 0.0, 0.0)),
                anchor(2, translation(1.0, 0.0, 0.0)),
            ],
            true,
        );
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(10)]
        );
        assert_eq!(out.rejected[0].0, r(30));
        assert!(out.rejected[0].1.contains("cascade expansion"));
    }

    /// 没安定的根（在飞 / 失败 / 从没生成过）没有可前移的记录。
    #[test]
    fn an_unsettled_root_never_transforms() {
        let out = classify_transform_roots(
            &roots(),
            &root_chains(),
            &set(&[21, 30]),
            &set(&[10]),
            &set(&[]),
            None,
            &[anchor(1, translation(1.0, 0.0, 0.0))],
            true,
        );
        assert!(out.transform.is_empty());
        assert!(out.rejected[0].1.contains("not a settled"));
    }

    /// 退化窗口一切进 regen，与今天一样。
    #[test]
    fn a_degraded_window_transforms_nothing() {
        let out = classify(
            &[10],
            &[],
            Some("closure incomplete"),
            &[anchor(1, translation(1.0, 0.0, 0.0))],
            true,
        );
        assert!(out.transform.is_empty());
        assert!(out.rejected[0].1.contains("closure incomplete"));
    }

    /// 被波及却归不了因（只有级联扩展能造出这种根，这里手搓一个）：保守 regen。
    #[test]
    fn a_touched_root_without_an_attributable_candidate_stays_regen() {
        let out = classify(
            &[30],
            &[],
            None,
            &[anchor(1, translation(1.0, 0.0, 0.0))],
            true,
        );
        assert!(out.transform.is_empty());
        assert!(out.rejected[0].1.contains("no candidate"));
    }

    /// 开关关：不是拒绝，是没有这条路——两个集合都空，计划与今天逐位相同。
    #[test]
    fn the_switch_off_yields_no_transform_and_no_rejections() {
        let out = classify(
            &[10, 21],
            &[],
            None,
            &[anchor(1, translation(1.0, 0.0, 0.0))],
            false,
        );
        assert_eq!(out, TransformClassification::default());
    }

    /// 祖孙锚点：ZONE 2 与它名下的 SUBE 31 同窗都动。SUBE 下的 BOX 32 只乘 SUBE 的 Δ
    /// （它已含 ZONE 的运动），EQUI 30 自己的记录乘 ZONE 的 Δ。
    #[test]
    fn nested_anchors_apply_the_innermost_delta_to_each_record() {
        let zone = TransformAnchor {
            refno: r(2),
            delta: translation(100.0, 0.0, 0.0),
        };
        // W_T(31)·W_S(31)⁻¹ = 先被 ZONE 带走 100，再自己挪 5：合起来 105。
        let sube = TransformAnchor {
            refno: r(31),
            delta: translation(105.0, 0.0, 0.0),
        };
        let anchors = [zone, sube];
        let under_sube = record("32", &[32, 31, 30, 2], translation(1.0, 1.0, 1.0));
        let under_equi = record("30", &[30, 2], translation(2.0, 2.0, 2.0));

        assert_eq!(nearest_anchor(&under_sube, &anchors).unwrap().refno, r(31));
        assert_eq!(nearest_anchor(&under_equi, &anchors).unwrap().refno, r(2));

        let out = republish_records(vec![under_sube, under_equi], &anchors, 41, |_, _| {
            panic!("纯平移不许读网格")
        })
        .unwrap();
        assert_eq!(out[0].world_transform, translation(106.0, 1.0, 1.0));
        assert_eq!(out[1].world_transform, translation(102.0, 2.0, 2.0));
        assert!(out.iter().all(|record| record.source_sesno == 41));
    }

    /// 兄弟锚点各自的 Δ 只作用于各自子树。
    #[test]
    fn sibling_anchors_move_only_their_own_subtrees() {
        let anchors = [
            TransformAnchor {
                refno: r(10),
                delta: translation(10.0, 0.0, 0.0),
            },
            TransformAnchor {
                refno: r(30),
                delta: translation(0.0, 30.0, 0.0),
            },
        ];
        let out = republish_records(
            vec![
                record("11", &[11, 10, 1], DMat4::IDENTITY),
                record("32", &[32, 31, 30, 2], DMat4::IDENTITY),
            ],
            &anchors,
            41,
            |_, _| panic!("纯平移不许读网格"),
        )
        .unwrap();
        assert_eq!(out[0].world_transform, translation(10.0, 0.0, 0.0));
        assert_eq!(out[0].world_bounds, [[10.0, 0.0, 0.0], [11.0, 2.0, 3.0]]);
        assert_eq!(out[1].world_transform, translation(0.0, 30.0, 0.0));
        assert_eq!(out[1].world_bounds, [[0.0, 30.0, 0.0], [1.0, 32.0, 3.0]]);
    }

    /// 不在任何锚点之下的记录：矩阵与盒不动，只前移 source_sesno（同根记录会话必须一致）。
    #[test]
    fn records_outside_every_anchor_only_advance_their_source_sesno() {
        let anchors = [TransformAnchor {
            refno: r(31),
            delta: translation(5.0, 0.0, 0.0),
        }];
        let untouched = record("30", &[30, 2], translation(2.0, 2.0, 2.0));
        let out = republish_records(vec![untouched.clone()], &anchors, 41, |_, _| {
            panic!("不该算盒")
        })
        .unwrap();
        assert_eq!(out[0].world_transform, untouched.world_transform);
        assert_eq!(out[0].world_bounds, untouched.world_bounds);
        assert_eq!(out[0].source_sesno, 41);
        assert_eq!(out[0].record_id, untouched.record_id);
        assert_eq!(out[0].mesh_id, untouched.mesh_id);
    }

    /// 含旋转的 Δ 不许拿盒过矩阵凑数：交给闭包（读网格顶点）；纯平移不问闭包。
    #[test]
    fn a_rotating_delta_asks_the_bounds_callback_and_a_translation_does_not() {
        let rotating = [TransformAnchor {
            refno: r(2),
            delta: rotation_z_90(),
        }];
        let mut asked = Vec::new();
        let out = republish_records(
            vec![record("30", &[30, 2], DMat4::IDENTITY)],
            &rotating,
            41,
            |record, world| {
                asked.push((record.record_id.clone(), world));
                Ok([[-2.0, 0.0, 0.0], [0.0, 1.0, 3.0]])
            },
        )
        .unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].1, rotation_z_90());
        assert_eq!(out[0].world_bounds, [[-2.0, 0.0, 0.0], [0.0, 1.0, 3.0]]);
        assert_eq!(out[0].world_transform, rotation_z_90());
    }

    /// 闭包读网格失败就是这根的回退理由，整批 Err 出去，不留半套。
    #[test]
    fn a_failing_bounds_callback_fails_the_whole_root() {
        let rotating = [TransformAnchor {
            refno: r(2),
            delta: rotation_z_90(),
        }];
        let error = republish_records(
            vec![record("30", &[30, 2], DMat4::IDENTITY)],
            &rotating,
            41,
            |_, _| anyhow::bail!("mesh_30.mesh: not found"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("mesh_30.mesh"));
    }

    #[test]
    fn pure_translation_recognises_translation_and_rejects_rotation_and_scale() {
        assert_eq!(
            pure_translation(&translation(1.0, -2.0, 3.5)),
            Some(DVec3::new(1.0, -2.0, 3.5))
        );
        assert_eq!(pure_translation(&DMat4::IDENTITY), Some(DVec3::ZERO));
        assert_eq!(pure_translation(&rotation_z_90()), None);
        assert_eq!(
            pure_translation(&DMat4::from_scale(DVec3::splat(2.0))),
            None
        );
        // 复合：先转再平移——旋转部分非单位，不是纯平移。
        assert_eq!(
            pure_translation(&(translation(1.0, 0.0, 0.0) * rotation_z_90())),
            None
        );
    }

    #[test]
    fn bounds_of_points_is_the_tight_box_and_empty_is_none() {
        assert_eq!(bounds_of_points(Vec::<DVec3>::new()), None);
        assert_eq!(
            bounds_of_points([
                DVec3::new(1.0, 5.0, -1.0),
                DVec3::new(-3.0, 2.0, 4.0),
                DVec3::new(0.0, 0.0, 0.0),
            ]),
            Some([[-3.0, 0.0, -1.0], [1.0, 5.0, 4.0]])
        );
    }

    /// 线格式往返：refno 与 16 个 f64 一个都不丢。
    #[test]
    fn anchors_survive_the_wire_format_round_trip() {
        let anchor = TransformAnchor {
            refno: r(1),
            delta: translation(1.0, 2.0, 3.0) * rotation_z_90(),
        };
        let wire = TransformAnchorWire::from(&anchor);
        assert_eq!(wire.refno, "24384/1");
        let json = serde_json::to_string(&wire).unwrap();
        assert!(
            json.contains("\"delta\":[") && !json.contains("MatrixWire"),
            "矩阵在线上就是 16 个数: {json}"
        );
        let back: TransformAnchorWire = serde_json::from_str(&json).unwrap();
        assert_eq!(back, wire, "线格式按位相等");
        assert_eq!(TransformAnchor::from(&back), anchor);

        let unit = TransformUnit {
            root_refno: "24384/10".into(),
            noun: "EQUI".into(),
            source_start_sesno: 40,
            anchors: vec![wire],
        };
        let unit_json = serde_json::to_string(&unit).unwrap();
        let unit_back: TransformUnit = serde_json::from_str(&unit_json).unwrap();
        assert_eq!(unit_back, unit);
        assert_eq!(unit_back.source_start_sesno, 40, "基线 S 随线格式走");
        assert_eq!(unit_back.anchors(), vec![anchor]);
    }

    /// 旧计划的线格式没有 `source_start_sesno`：反序列化默认 0，基线闸把它挡回 regen（保守正确），
    /// 老的持久化计划 drain 起来不会双重搬移。
    #[test]
    fn a_legacy_transform_unit_without_baseline_deserialises_to_zero_and_falls_back() {
        let legacy: TransformUnit =
            serde_json::from_str(r#"{"root_refno":"24384/10","noun":"EQUI","anchors":[]}"#)
                .unwrap();
        assert_eq!(legacy.source_start_sesno, 0);
        assert_eq!(
            republish_gate(40, legacy.source_start_sesno, 41),
            RepublishGate::FallbackBaselineMismatch
        );
    }

    /// 幂等闸（S4b 审核 P0 / mem-42）：回执在 S 才放行；已在 T = 窗口重放（发布成功但回合没收尾），
    /// 跳过不再乘；其余一律回退 regen。
    #[test]
    fn the_republish_gate_only_proceeds_from_the_planned_baseline() {
        use RepublishGate::*;
        assert_eq!(republish_gate(40, 40, 41), Proceed);
        assert_eq!(
            republish_gate(41, 40, 41),
            SkipAlreadyAtTarget,
            "已在 T：再乘一次 Δ 就是双重搬移"
        );
        assert_eq!(
            republish_gate(39, 40, 41),
            FallbackBaselineMismatch,
            "半截 / 更旧的状态"
        );
        assert_eq!(
            republish_gate(42, 40, 41),
            FallbackBaselineMismatch,
            "被别的路径推过了头"
        );
        assert_eq!(
            republish_gate(41, 41, 41),
            SkipAlreadyAtTarget,
            "S == T 的退化窗：无事可做"
        );
    }

    /// 回执配方只有一份：重生成与刚体前移都从 `manifest_hash` 取章，两条路对同一批记录算出同一个。
    #[test]
    fn regeneration_and_rigid_motion_share_one_manifest_recipe() {
        let service = include_str!("../fast_model/e3d_model_service.rs");
        let body = service
            .split_once("fn project_geometry_delta(")
            .expect("project_geometry_delta exists")
            .1
            .split_once("\npub(crate) fn manifest_entry(")
            .expect("manifest_entry follows")
            .0;
        assert!(
            body.contains("manifest_hash(&commit.upserts)"),
            "重生成的回执必须从共用配方取章"
        );
        assert!(
            !body.contains("Sha256::digest"),
            "project_geometry_delta 不许自己再拼一份清单摘要"
        );

        let a = record("30", &[30, 2], translation(2.0, 2.0, 2.0));
        let b = record("32", &[32, 31, 30, 2], DMat4::IDENTITY);
        let (before, count) =
            crate::fast_model::e3d_model_service::manifest_hash(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(count, 2);
        let moved = republish_records(
            vec![a, b],
            &[TransformAnchor {
                refno: r(2),
                delta: translation(1.0, 0.0, 0.0),
            }],
            41,
            |_, _| unreachable!(),
        )
        .unwrap();
        let (after, _) = crate::fast_model::e3d_model_service::manifest_hash(&moved).unwrap();
        assert_ne!(before, after, "世界变换变了，章就得变");
        let (again, _) = crate::fast_model::e3d_model_service::manifest_hash(&moved).unwrap();
        assert_eq!(after, again, "同一批记录两次算出同一个章");
    }

    /// 管段记录与图元记录的条目各有自己的字段；这里只钉「同一 geometry_id 只算一条、后者覆盖」。
    #[test]
    fn manifest_hash_counts_one_entry_per_geometry_id() {
        let mut a = record("30", &[30, 2], DMat4::IDENTITY);
        let mut b = record("30-dup", &[30, 2], translation(1.0, 0.0, 0.0));
        b.geometry_id = a.geometry_id.clone();
        let (_, count) =
            crate::fast_model::e3d_model_service::manifest_hash(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(count, 1);
        a.tube = Some(ProjectedTube {
            route_ordinal: 0,
            from: RefNo::new(24384, 21),
            to: RefNo::new(24384, 22),
            invalid: false,
        });
        let (tube_hash, _) = crate::fast_model::e3d_model_service::manifest_hash(&[a]).unwrap();
        let (plain_hash, _) = crate::fast_model::e3d_model_service::manifest_hash(&[b]).unwrap();
        assert_ne!(tube_hash, plain_hash);
    }
}
