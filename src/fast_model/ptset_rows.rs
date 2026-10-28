//! `pe_ptset`：管件目录 P 点分表的行（ptset 分表计划 W2，`docs/plans/2026-10-26-ptset-split-table-plan.md`）。
//!
//! 行与模型行**同一次发布**写出：e3d-model 生成一根时随 [`e3d_model::GenerationOutcome::point_sets`] 交出每个路由成员
//! （BRAN / HANG 等容器的直属管件）与类 28 NOZZ 的 P 点集；生成服务把它折成 [`ProjectedPointSet`]、挂在
//! [`crate::fast_model::model_memory_store::ProjectionCommit::point_sets`] 上随根进内存投影；write-behind 落库时把它渲成
//! `pe_ptset` 行，跟在这一根**最后一块**几何后面、同一笔事务（`model_db_adapter::persist_group`，与房间边同一个位置）；
//! 整片删那一半按根清旧行（[`render_root_clear`]）。镜像 spool 的整根替换与墓碑走同一套渲染。
//! 读侧只有 `resource/surreal/gy_common.surql` 的 `fn::pe_ptset_of`（长度函数 `fn::bran_comp_len` / `fn::elbo_len` / `fn::tee_len`）。
//!
//! # 行形状（计划 §3，在补写版的现状上只加不改）
//!
//! ```text
//! pe_ptset:⟨w0_w1⟩ = { pe, root, dbnum, noun,
//!                      ptset: { "<P 号>": { pt: [x,y,z], dir: [x,y,z], bore, noun } },
//!                      count, unresolved: [{ number, reason }],
//!                      sesno, generator, storage_version, reason, written_at }
//! ```
//!
//! - 坐标在管件局部系（mm），方向已归一；`bore` 按 E3D `PPBORE` 口径（未设 / 0 → `null`）。
//! - `sesno` = 这根这次发布的 `source_sesno`；`generator` = e3d-model [`e3d_model::PTSET_EVAL_VERSION`]（求值规则的版本）。
//! - `storage_version` = [`aios_core::model_storage::MODEL_STORAGE_VERSION`]，**行形状**的版本。
//!   它与 `generator` 是两条正交的轴：`generator` 说「点算得对不对」，`storage_version` 说「这行读不读得懂」。
//!   读侧（rs-server / aios_core）按 `dbnum_watermark:{dbnum}.model_storage_version` 判整库，
//!   按这一格判单行；对不上就报 `MODEL_STORAGE_VERSION_MISMATCH`，不静默回空集。
//! - **没有行 = 这根还没按新版发布过**；**有行但 `ptset: {}` = 这件真没有 P 点**，`reason` 说明是哪一种
//!   （没有目录链 / 目录件没有 PTRE / 点全解不出 / PTSE 是空的）。长度函数对 `{}` 照旧回 0。
//! - 没有异步补写也没有启动补扫（D5，2026-10-27 拍板）：发布路径是唯一写入口，部署后老根靠逐库
//!   `POST /api/v1/dbnums/{dbnum}/model/rebuild` 补齐；以后 `PTSET_EVAL_VERSION` 加一也走同一条 SOP，`generator` 让巡检分得清哪些库还没跑。
//!
//! # 拼 SQL 的纪律
//!
//! 与 [`crate::fast_model::model_db_adapter`] 同一套：文本一律 [`sql_string`]，记录 id 只由 refno 两个整数拼；清行不写
//! `DELETE … WHERE`（部署引擎上不走索引），而是先按 `root` 走 `pe_ptset_root` 索引选出 id 再 `DELETE` 那个数组。

use e3d_io::refno::RefNo;
use e3d_model::{CataloguePointSet, MemberPointSet, PTSET_EVAL_VERSION};
use serde::{Deserialize, Serialize};

/// 表名；`gy_common.surql` 的 `fn::pe_ptset_of` 按 `type::thing('pe_ptset', record::id($pe))` 取，所以行 id 必须与 `pe` 同键。
pub const TABLE: &str = "pe_ptset";

/// 一个路由成员随根发布的目录 P 点集（[`MemberPointSet`] 的 wire 形，进 [`crate::fast_model::model_memory_store::ProjectionCommit`]，
/// 随它序列化进模型产物）。`root` 是**生成根**，不是直属属主：清行、删根都按生成根整批做。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedPointSet {
    #[serde(with = "refno_pair")]
    pub root: RefNo,
    #[serde(with = "refno_pair")]
    pub refno: RefNo,
    pub noun: String,
    /// 按 PTSE 成员序；坐标 / 方向在成员局部系，非有限的点在折进来时已被按「解不出」记进 `unresolved`。
    pub points: Vec<ProjectedPoint>,
    pub unresolved: Vec<UnresolvedProjectedPoint>,
    /// 没有点时的缘由；有点是 `None`。
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedPoint {
    pub number: u32,
    pub noun: String,
    pub pt: [f64; 3],
    pub dir: [f64; 3],
    pub bore: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnresolvedProjectedPoint {
    pub number: u32,
    pub reason: String,
}

mod refno_pair {
    use super::RefNo;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(r: &RefNo, s: S) -> Result<S::Ok, S::Error> {
        (r.word0, r.word1).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<RefNo, D::Error> {
        let (a, b) = <(u32, u32)>::deserialize(d)?;
        Ok(RefNo::new(a, b))
    }
}

/// 把生成器交出的一根的 P 点集折成要进投影的行（纯函数，不读库不读盘）。
///
/// 每件一条，**没有点的也一条**（D2：让「没行」只有一个含义）。非有限的坐标 / 方向不写进行（SurrealQL 没有 NaN 字面量），
/// 那一点按解不出记进 `unresolved`。
pub fn project_point_sets(root: RefNo, sets: Vec<MemberPointSet>) -> Vec<ProjectedPointSet> {
    sets.into_iter()
        .map(|member| project_point_set(root, member))
        .collect()
}

fn project_point_set(root: RefNo, member: MemberPointSet) -> ProjectedPointSet {
    let MemberPointSet { refno, noun, set } = member;
    let CataloguePointSet {
        component,
        point_set,
        points,
        unresolved,
    } = set;
    let mut projected = Vec::with_capacity(points.len());
    let mut unresolved: Vec<UnresolvedProjectedPoint> = unresolved
        .into_iter()
        .map(|point| UnresolvedProjectedPoint {
            number: point.number,
            reason: point.reason,
        })
        .collect();
    for point in points {
        let pt = point.pos.to_array();
        let dir = point.dir.to_array();
        if !pt.iter().chain(dir.iter()).all(|value| value.is_finite()) {
            unresolved.push(UnresolvedProjectedPoint {
                number: point.number,
                reason: format!("P{}（{}）坐标或方向非有限", point.number, point.noun),
            });
            continue;
        }
        projected.push(ProjectedPoint {
            number: point.number,
            noun: point.noun,
            pt,
            dir,
            bore: point.bore.filter(|bore| bore.is_finite()),
        });
    }
    let reason = if !projected.is_empty() {
        None
    } else if component.is_none() {
        Some("no catalogue chain".to_string())
    } else if point_set.is_none() {
        Some("no PTRE".to_string())
    } else if !unresolved.is_empty() {
        Some("all points unresolved".to_string())
    } else {
        Some("empty PTSE".to_string())
    };
    ProjectedPointSet {
        root,
        refno,
        noun,
        points: projected,
        unresolved,
        reason,
    }
}

/// 清掉一根名下的旧行：先按 `root` 走 `pe_ptset_root` 索引选 id，再点名删（不写 `DELETE … WHERE`，部署引擎上不走索引）。
/// 整片删那一半调它；没有成员的根也要清（成员全删了）。
pub fn render_root_clear(root: RefNo) -> String {
    format!(
        "DELETE (SELECT VALUE id FROM {TABLE} WHERE root = pe:⟨{}⟩);\n",
        refno_key(root)
    )
}

/// 一根的全部行，每件一条 `UPSERT`（幂等：同一根重发、spool 重放都是同一份内容覆盖）。空表渲成空串。
pub fn render_rows(sesno: u32, sets: &[ProjectedPointSet]) -> String {
    let mut out = String::new();
    for set in sets {
        out.push_str(&render_row(sesno, set));
    }
    out
}

fn render_row(sesno: u32, set: &ProjectedPointSet) -> String {
    let key = refno_key(set.refno);
    let entries = set
        .points
        .iter()
        .map(|point| {
            format!(
                "\"{number}\": {{ pt: {pt}, dir: {dir}, bore: {bore}, noun: {noun} }}",
                number = point.number,
                pt = render_vec3(point.pt),
                dir = render_vec3(point.dir),
                bore = point.bore.map_or_else(|| "null".to_owned(), render_num),
                noun = sql_string(&point.noun),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let unresolved = set
        .unresolved
        .iter()
        .map(|point| {
            format!(
                "{{ number: {}, reason: {} }}",
                point.number,
                sql_string(&point.reason)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "UPSERT {TABLE}:⟨{key}⟩ CONTENT {{ pe: pe:⟨{key}⟩, root: pe:⟨{root}⟩, dbnum: {dbnum}, noun: {noun}, \
         ptset: {{ {entries} }}, count: {count}, unresolved: [{unresolved}], sesno: {sesno}, generator: {generator}, \
         storage_version: {storage_version}, reason: {reason}, written_at: time::now() }};\n",
        root = refno_key(set.root),
        dbnum = set.refno.dbno(),
        noun = sql_string(&set.noun),
        count = set.points.len(),
        generator = PTSET_EVAL_VERSION,
        storage_version = aios_core::model_storage::MODEL_STORAGE_VERSION,
        reason = set
            .reason
            .as_deref()
            .map_or_else(|| "NONE".to_owned(), sql_string),
    )
}

/// 与 `pe` 表同键：`word0_word1`（`model_db_adapter::refno_key` 同式）。
fn refno_key(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1)
}

/// 数字用 Rust 的 `Display`（不出科学计数法），先四舍五入到 1e-6 mm：再细的位没有工程意义，只会把语句撑长。
fn render_num(value: f64) -> String {
    let rounded = (value * 1e6).round() / 1e6;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{rounded}")
}

fn render_vec3(values: [f64; 3]) -> String {
    format!(
        "[{}, {}, {}]",
        render_num(values[0]),
        render_num(values[1]),
        render_num(values[2])
    )
}

/// SurrealQL 单引号字符串字面量。
fn sql_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use e3d_model::{CataloguePoint, UnresolvedCataloguePoint};
    use glam::DVec3;

    fn member(refno: RefNo, noun: &str, set: CataloguePointSet) -> MemberPointSet {
        MemberPointSet {
            refno,
            noun: noun.into(),
            set,
        }
    }

    fn point(number: u32, pos: [f64; 3], dir: [f64; 3], bore: Option<f64>) -> CataloguePoint {
        CataloguePoint {
            number,
            noun: "PTAX".into(),
            pos: DVec3::from_array(pos),
            dir: DVec3::from_array(dir),
            bore,
        }
    }

    /// 行形状就是 `gy_common.surql` 头上写的那一行：同键、按 P 号字串取、bore 未设是 null，sesno / generator / reason 三格在。
    #[test]
    fn a_row_carries_the_plan_shape() {
        let root = RefNo::new(24384, 2860);
        let sets = project_point_sets(
            root,
            vec![member(
                RefNo::new(24384, 2861),
                "FTUB",
                CataloguePointSet {
                    component: Some(RefNo::new(13244, 51906)),
                    point_set: Some(RefNo::new(13244, 51859)),
                    points: vec![
                        point(1, [0.0, 0.0, 0.0], [-1.0, 0.0, 0.0], Some(500.0)),
                        point(21, [2000.0, 0.0, 1e-9], [0.0, 0.0, 1.0], None),
                    ],
                    unresolved: vec![UnresolvedCataloguePoint {
                        number: 7,
                        reason: "P7（PTCA）方向解不出".into(),
                    }],
                },
            )],
        );
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].reason, None);
        let sql = render_rows(738, &sets);
        assert_eq!(
            sql,
            format!(
                "UPSERT pe_ptset:⟨24384_2861⟩ CONTENT {{ pe: pe:⟨24384_2861⟩, root: pe:⟨24384_2860⟩, dbnum: 8000, noun: 'FTUB', \
                 ptset: {{ \"1\": {{ pt: [0, 0, 0], dir: [-1, 0, 0], bore: 500, noun: 'PTAX' }}, \
                 \"21\": {{ pt: [2000, 0, 0], dir: [0, 0, 1], bore: null, noun: 'PTAX' }} }}, count: 2, \
                 unresolved: [{{ number: 7, reason: 'P7（PTCA）方向解不出' }}], sesno: 738, generator: {}, \
                 storage_version: {}, reason: NONE, written_at: time::now() }};\n",
                PTSET_EVAL_VERSION,
                aios_core::model_storage::MODEL_STORAGE_VERSION
            )
        );

        // 两条轴必须并存且各说各的：`generator` 是求值器版本，`storage_version` 是行形状版本。
        // 把它们合并成一个数会让「换求值规则」与「换行形状」分不开。
        assert!(
            sql.contains("generator: ") && sql.contains("storage_version: "),
            "两格都要在: {sql}"
        );
    }

    /// 没有点的件也有一行，`reason` 分得清四种缘由；非有限的点不进行、记进 unresolved。
    #[test]
    fn empty_sets_say_why_and_non_finite_points_are_unresolved() {
        let root = RefNo::new(24384, 2860);
        let no_chain = project_point_set(
            root,
            member(
                RefNo::new(24384, 2862),
                "ATTA",
                CataloguePointSet::default(),
            ),
        );
        assert_eq!(no_chain.reason.as_deref(), Some("no catalogue chain"));
        assert!(
            render_rows(1, &[no_chain]).contains("ptset: {  }, count: 0, unresolved: [], sesno: 1")
        );

        let no_ptre = project_point_set(
            root,
            member(
                RefNo::new(24384, 2863),
                "ATTA",
                CataloguePointSet {
                    component: Some(RefNo::new(13244, 1)),
                    ..CataloguePointSet::default()
                },
            ),
        );
        assert_eq!(no_ptre.reason.as_deref(), Some("no PTRE"));

        let nan = project_point_set(
            root,
            member(
                RefNo::new(24384, 2864),
                "BEND",
                CataloguePointSet {
                    component: Some(RefNo::new(13244, 1)),
                    point_set: Some(RefNo::new(13244, 2)),
                    points: vec![point(1, [f64::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], Some(100.0))],
                    unresolved: Vec::new(),
                },
            ),
        );
        assert!(nan.points.is_empty());
        assert_eq!(nan.unresolved.len(), 1);
        assert_eq!(nan.reason.as_deref(), Some("all points unresolved"));

        let empty = project_point_set(
            root,
            member(
                RefNo::new(24384, 2865),
                "ATTA",
                CataloguePointSet {
                    component: Some(RefNo::new(13244, 1)),
                    point_set: Some(RefNo::new(13244, 2)),
                    ..CataloguePointSet::default()
                },
            ),
        );
        assert_eq!(empty.reason.as_deref(), Some("empty PTSE"));
    }

    /// 清行先按索引选 id 再点名删，不写 `DELETE … WHERE`。
    #[test]
    fn the_clear_statement_selects_by_root_then_deletes_the_ids() {
        let sql = render_root_clear(RefNo::new(24384, 2860));
        assert_eq!(
            sql,
            "DELETE (SELECT VALUE id FROM pe_ptset WHERE root = pe:⟨24384_2860⟩);\n"
        );
        assert!(!sql.starts_with("DELETE pe_ptset WHERE"));
    }

    /// 文本一律走 `sql_string`：带引号的 reason 不会把语句截断。
    #[test]
    fn strings_are_escaped() {
        assert_eq!(sql_string("it's"), "'it\\'s'");
        assert_eq!(render_num(1234.56789012), "1234.56789");
        assert_eq!(render_num(-0.0), "0");
    }
}
