//! 快照（RVM 基准） vs SurrealDB（本仓生成结果）三层对拍。
//!
//! L1 成员清单：按真实 refno join，输出 matched / missing_in_gen / extra_in_gen。
//! L2 几何构成：两侧几何数量与类型分布——**信息项，不判红灯**，原因见下。
//! L3 空间级：world 平移和成员 AABB 都参与判定。成员 AABB 只在**窄口径**基准
//! （`repre insu/obst off`）下可判——宽口径把保温与障碍/预留体一并写进 RVM，
//! 生成侧有意不产出它们，两边比的不是同一个东西。口径由快照的
//! [`ExportScope`] 声明并在这里强制校验，不再只是一句注释：它曾经只是注释，
//! 而夹具恰好是宽口径导的，于是一个电缆槽 100mm 的预留净空被报成了几何缺陷。
//!
//! 为什么 L2 只能是信息项：两侧的几何表达根本不是一套。RVM 是 E3D 为渲染
//! 做的原语分解（Cylinder / CircularTorus / Snout…），生成侧 `inst_geo.param`
//! 是 catalogue 参数化几何（实测 BEND 的可见几何是 `PrimExtrusion` 拉伸体）。
//! 同一个 BEND，RVM 给 3 个原语，生成侧给 6 个可见几何 + 若干 CataNeg，
//! 逐参数比对没有共同基准。判定因此落在 L1（成员在不在）与 L3（位置和尺寸对不对）。
//!
//! 身份从哪来：快照里只有配了 `.att` 的那份带 refno（ATT 里只有**未命名元素**才写
//! `NAME:= =ref0/ref1`）。其余的在这里连着库现解——见 [`super::resolve`]。隐式管段
//! （`FTUBE n of BRANCH /x`）在 PDMS 里不是元素，落在 `tubi_relate` 上，单独一条路。
//!
//! L4 表面：世界姿态的判据。**不是**拿图元变换比旋转矩阵——RVM 的 CNTB 记录只写平移，
//! 成员的世界姿态只能从它的图元变换里反推，而两侧的图元分解根本不是一套（同一个 BEND，
//! RVM 给 3 个原语，生成侧给 6 个可见几何 + 若干 CataNeg）：按图元集比姿态会把**分解差异**
//! 报成**姿态错误**，正是「一个口径分歧看上去像三十五个几何缺陷」的同一种错法。
//! 用的是 [`super::mesh_compare`] 的**双向表面距离**（两侧统一到世界 mm、与三角化无关）：
//! 姿态错了表面距离必然爆掉，而且它不需要图元一一对应。给了 `--rvm` 才跑；不给就把
//! `world_rotation` 留在 `unsupported_checks` 上——没量就是没量，不许默认放行。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;

use super::att::refno_from_att_name;
use super::resolve::{
    self, Resolved, ResolvedFrom, Resolver, SubtreeIndex, find_refno_by_name, load_subtree,
};
use super::snapshot::{ExportScope, RvmSnapshot};

#[derive(Debug, Clone)]
pub struct CompareOptions {
    pub snapshot_path: PathBuf,
    pub url: String,
    pub ns: String,
    pub db: String,
    pub user: String,
    pub password: String,
    /// world 平移允许偏差（mm）。
    pub tol_translation_mm: f64,
    /// AABB 各分量允许偏差（mm）。
    pub tol_aabb_mm: f64,
    /// RVM 原件。给了才做 L4 表面对拍（快照里只有 AABB，没有面）。
    pub rvm_path: Option<PathBuf>,
    /// 双向表面距离的 p95 容差（mm）。
    ///
    /// **不是 1mm**：两侧都是有限三角化，E3D FacetGroup 的弦误差就是判定地板，
    /// 1mm 量的是三角化精度而不是几何。12mm 取自 `mesh_wall_live` 的实测门限
    /// （`GEN_TO_RVM_P95_TOL` / `P95_TOL_MM`，见 `docs/2026-08-12_live-test-ledger.md`）。
    pub tol_surface_p95_mm: f64,
    /// 双向表面距离的 Hausdorff 容差（mm）。p95 管「整体贴不贴」，Hausdorff 管
    /// 「有没有一处离谱」——开孔、多出/缺失的一块板只影响极少数采样点，p95 完全看不见
    /// （1RS-WF03 的 GWALL 实测 p95 0.001 mm、Hausdorff 2,010 mm）。
    /// 30 取自 `mesh_wall_live` 的实测门限。
    pub tol_surface_hausdorff_mm: f64,
    pub report_path: PathBuf,
    pub verbose: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct CompareSummary {
    /// 基准快照声明的导出口径。非窄口径时 AABB 不参与判定，且不得 PASS。
    pub baseline_scope: ExportScope,
    /// 参与判定的 RVM 成员数（已解析身份且带几何）。
    pub compared: usize,
    pub matched: usize,
    pub missing_in_gen: usize,
    pub extra_in_gen: usize,
    /// 未解析身份，无法 join；单列记账并让最终判定失败。
    pub exempt_unresolved: usize,
    /// 快照里没带 refno、由本步连着库解出来的身份数（按来源分桶）。
    pub resolved_here: BTreeMap<String, usize>,
    /// 参与判定的隐式管段数（`FTUBE n of BRANCH /x` → `tubi_relate`）。
    pub tubes_compared: usize,
    /// 解出了「哪条 BRANCH 的第几根」，但生成侧那条 BRANCH 没有这一根。
    pub tubes_missing_in_gen: usize,
    /// RVM 侧零几何的结构节点（SITE/ZONE/PIPE），不判定。
    pub exempt_no_geometry: usize,
    /// Obstruction / Insulation 桶，按约定豁免 missing 判定。
    pub exempt_geo_type: usize,
    /// RVM 侧只有 `Line` 中心线、没有任何表面：E3D 自己就没画实体，生成侧没有实例是
    /// 两边一致，不判缺件。
    pub exempt_centreline_only: usize,
    pub translation_compared: usize,
    pub translation_mismatch: usize,
    pub aabb_compared: usize,
    pub aabb_mismatch: usize,
    pub max_translation_delta_mm: f64,
    pub max_aabb_delta_mm: f64,
    /// L4：双向表面距离真比过的成员数。
    pub surface_compared: usize,
    pub surface_mismatch: usize,
    /// 匹配上了、但两侧至少一侧建不出网格，姿态因此没判到（隐式管段没有 `pe` 行，
    /// 也落在这里）。**不是豁免**：没量到就不能 PASS。
    pub surface_unavailable: usize,
    pub max_surface_p95_mm: f64,
    pub max_surface_hausdorff_mm: f64,
    pub unsupported_checks: Vec<String>,
}

impl CompareSummary {
    pub fn passed(&self) -> bool {
        // 口径不对时下面的 aabb_compared 本来就凑不满，但那是「顺带不成立」；
        // 显式写出来，是因为它是判定资格问题，不该靠计数巧合来兜。
        self.baseline_scope.allows_aabb_verdict()
            && self.compared > 0
            && self.matched == self.compared
            && self.translation_compared == self.matched
            && self.aabb_compared == self.matched
            && self.exempt_unresolved == 0
            && self.missing_in_gen == 0
            && self.extra_in_gen == 0
            && self.tubes_missing_in_gen == 0
            && self.translation_mismatch == 0
            && self.aabb_mismatch == 0
            // L4：姿态那一格。每个匹配上的成员都得真比过表面——`surface_unavailable`
            // 是「没量到」，与「量了都对」不能共用一个退出码。
            && self.surface_compared == self.matched
            && self.surface_unavailable == 0
            && self.surface_mismatch == 0
            && self.unsupported_checks.is_empty()
    }
}

fn validate_tolerance(name: &str, value: f64) -> Result<()> {
    anyhow::ensure!(
        value.is_finite() && value >= 0.0,
        "{name} tolerance must be a finite non-negative number"
    );
    Ok(())
}

// ───────────────────────── 生成侧（SurrealDB） ─────────────────────────

#[derive(Debug, Deserialize)]
struct GenRow {
    refno: String,
    aabb: Option<GenAabb>,
    world_trans: Option<GenTrans>,
    visible_geos: Option<i64>,
    total_geos: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GenAabb {
    mins: [f64; 3],
    maxs: [f64; 3],
}

impl GenAabb {
    fn into_array(self) -> [f64; 6] {
        [
            self.mins[0],
            self.mins[1],
            self.mins[2],
            self.maxs[0],
            self.maxs[1],
            self.maxs[2],
        ]
    }
}

#[derive(Debug, Deserialize)]
struct TubeRow {
    /// 这根管段离开的那个元件（`tubi_relate.in`）。
    leave: String,
    slot: Option<i64>,
    aabb: Option<GenAabb>,
    world_trans: Option<GenTrans>,
}

#[derive(Debug, Deserialize)]
struct GenTrans {
    translation: [f64; 3],
}

#[derive(Debug, Clone)]
struct GenInstance {
    aabb: Option<[f64; 6]>,
    translation: Option<[f64; 3]>,
    visible_geos: i64,
    total_geos: i64,
}

/// 一次查询的 refno 上限。SurrealQL 的 IN 列表太长会拖慢解析，分批更稳。
const QUERY_CHUNK: usize = 200;

fn canonical_refno(value: &str) -> Result<String> {
    refno_from_att_name(&format!("={}", value.trim()))
        .ok_or_else(|| anyhow::anyhow!("invalid PDMS refno: {value}"))
}

fn pe_key(refno: &str) -> String {
    format!("pe:{}", refno.replace('/', "_"))
}

fn inst_relate_key(refno: &str) -> String {
    format!("inst_relate:{}", refno.replace('/', "_"))
}

/// 一条 BRANCH 的隐式管段，按 E3D 数 `FTUBE n` 的那个次序排好。
///
/// `tubi_relate` 的 id 是 `[pe:{leave}, slot]`——`leave` 是这根管段**离开**的那个元件，
/// 不是 BRANCH。所以一条 BRANCH 的管段要从「BRANCH 本身 + 它的成员」逐个顶点问出边
/// （`pe:X->tubi_relate` 走 `in` 的索引；`WHERE in = pe:X` 会整表扫），再按
/// 「leave 在成员表里的位置，其次 slot」排序：头管段的 leave 是 BRANCH 自己，排最前，
/// 之后依次是各元件流出的那一根——正是 E3D 的编号次序。
async fn load_branch_tubes(
    db: &Surreal<Any>,
    index: &SubtreeIndex,
    branch: &str,
) -> Result<Vec<GenInstance>> {
    let mut vertices = vec![branch.to_string()];
    vertices.extend(index.children_of(branch).iter().cloned());
    let order: HashMap<&str, usize> = vertices
        .iter()
        .enumerate()
        .map(|(at, refno)| (refno.as_str(), at))
        .collect();

    let mut rows: Vec<TubeRow> = Vec::new();
    for chunk in vertices.chunks(QUERY_CHUNK) {
        let froms = chunk
            .iter()
            .map(|refno| format!("{}->tubi_relate", resolve::pe_key(refno)))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT type::string(in) AS leave, record::id(id)[1] AS slot, \
                    aabb.d AS aabb, world_trans.d AS world_trans \
             FROM {froms};"
        );
        let mut response = db.query(sql).await.context("查询 tubi_relate 失败")?;
        rows.extend(
            response
                .take::<Vec<TubeRow>>(0)
                .context("解析 tubi_relate 结果失败")?,
        );
    }

    rows.sort_by_key(|row| {
        let leave = resolve::refno_from_pe_key(&row.leave);
        (
            order.get(leave.as_str()).copied().unwrap_or(usize::MAX),
            row.slot.unwrap_or(i64::MAX),
        )
    });
    Ok(rows
        .into_iter()
        .map(|row| GenInstance {
            aabb: row.aabb.map(GenAabb::into_array),
            translation: row.world_trans.as_ref().map(|t| t.translation),
            visible_geos: 0,
            total_geos: 0,
        })
        .collect())
}

async fn load_gen_side(
    db: &Surreal<Any>,
    refnos: &[String],
) -> Result<HashMap<String, GenInstance>> {
    let mut out = HashMap::new();

    for chunk in refnos.chunks(QUERY_CHUNK) {
        // 按记录 id 直接寻址（与生产读路径的 `get_inst_relate_keys` 同一形态）：
        // `inst_relate` 的 id 就是 refno，而这张表没有 `(in, out)` 索引，
        // `WHERE in IN [..]` 只能整表扫。不存在的 id 不出行，与谓词形式一致。
        let ids = chunk
            .iter()
            .map(|r| inst_relate_key(r))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT type::string(in) AS refno, \
                    aabb.d AS aabb, \
                    world_trans.d AS world_trans, \
                    array::len(out->geo_relate[WHERE visible = true] ?? []) AS visible_geos, \
                    array::len(out->geo_relate ?? []) AS total_geos \
             FROM {ids};"
        );

        let mut response = db.query(sql).await.context("查询 inst_relate 失败")?;
        let rows: Vec<GenRow> = response.take(0).context("解析 inst_relate 结果失败")?;

        for row in rows {
            let refno = row.refno.trim_start_matches("pe:").replace('_', "/");
            out.insert(
                refno,
                GenInstance {
                    aabb: row.aabb.map(GenAabb::into_array),
                    translation: row.world_trans.map(|t| t.translation),
                    visible_geos: row.visible_geos.unwrap_or(0),
                    total_geos: row.total_geos.unwrap_or(0),
                },
            );
        }
    }

    Ok(out)
}

// ───────────────────────────── 对拍主流程 ─────────────────────────────

/// L4 的 RVM 侧：按 group 名归并的世界 mm 三角网格。`manifold` 关掉时生成侧建不出网格，
/// 这一层整个不跑（`world_rotation` 留在未支持名单上）。
#[cfg(feature = "manifold")]
fn load_rvm_meshes(
    rvm_path: Option<&Path>,
) -> Result<Option<HashMap<String, parry3d::shape::TriMesh>>> {
    let Some(path) = rvm_path else {
        return Ok(None);
    };
    Ok(Some(super::mesh_compare::rvm_world_meshes_by_name(path)?))
}

#[cfg(not(feature = "manifold"))]
fn load_rvm_meshes(_rvm_path: Option<&Path>) -> Result<Option<()>> {
    Ok(None)
}

pub async fn compare(options: &CompareOptions) -> Result<CompareSummary> {
    validate_tolerance("translation", options.tol_translation_mm)?;
    validate_tolerance("AABB", options.tol_aabb_mm)?;
    validate_tolerance("surface p95", options.tol_surface_p95_mm)?;
    validate_tolerance("surface hausdorff", options.tol_surface_hausdorff_mm)?;
    let snapshot = RvmSnapshot::load(&options.snapshot_path)?;
    let rvm_meshes = load_rvm_meshes(options.rvm_path.as_deref())?;

    let db: Surreal<Any> = connect(options.url.clone())
        .await
        .with_context(|| format!("连接 SurrealDB 失败: {}", options.url))?;
    db.signin(Root {
        username: &options.user,
        password: &options.password,
    })
    .await
    .context("SurrealDB 登录失败")?;
    db.use_ns(options.ns.clone())
        .use_db(options.db.clone())
        .await
        .context("切换 SurrealDB namespace/database 失败")?;

    let root_name = snapshot
        .meta
        .root_name
        .as_deref()
        .context("快照没有 root_name，无法枚举生成侧子树")?;
    // 根 refno：快照带了就用（`--root-refno` 或 ATT），没带就按名字在库里找一次。
    // 这是整轮唯一一次全表扫；子树进了内存之后所有身份解析都不再碰库。
    let root_refno = match snapshot
        .members
        .iter()
        .find(|member| member.name == root_name)
        .and_then(|member| member.refno.as_deref())
    {
        Some(refno) => canonical_refno(refno)?,
        None => find_refno_by_name(&db, snapshot.meta.dbnum, root_name)
            .await?
            .with_context(|| {
                format!(
                    "快照根 {root_name} 在 dbnum {} 里按名字找不到（或不止一个），无法枚举生成侧子树",
                    snapshot.meta.dbnum
                )
            })?,
    };
    let index = load_subtree(&db, &root_refno).await?;
    let refnos: Vec<String> = index.refnos().cloned().collect();
    let gen_side = load_gen_side(&db, &refnos).await?;
    let mut resolver = Resolver::new(&index);
    let mut branch_tubes: HashMap<String, Vec<GenInstance>> = HashMap::new();

    let mut summary = CompareSummary {
        baseline_scope: snapshot.meta.export_scope,
        // 姿态由 L4 表面对拍判；没给 `--rvm`（或没有 `manifold`）就是没量到，
        // 那时 `world_rotation` 仍然挂在未支持名单上阻止 PASS。
        unsupported_checks: match rvm_meshes.is_some() {
            true => Vec::new(),
            false => vec!["world_rotation".into()],
        },
        ..Default::default()
    };
    let aabb_is_judgeable = summary.baseline_scope.allows_aabb_verdict();
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut geo_kind_counts: BTreeMap<String, usize> = BTreeMap::new();

    // 先把全部成员的身份定下来（包括豁免的那些）：`extra_in_gen` 要拿「RVM 认识的
    // refno 全集」去减，漏掉豁免成员会把它们的实例误报成生成侧多出来的。
    let identities: Vec<Option<(Resolved, ResolvedFrom)>> = snapshot
        .members
        .iter()
        .map(|member| match member.refno.as_deref() {
            Some(refno) => canonical_refno(refno)
                .ok()
                .map(|refno| (Resolved::Element(refno), ResolvedFrom::PeName)),
            None => resolver.resolve(&member.name).inspect(|(_, source)| {
                *summary
                    .resolved_here
                    .entry(source.as_str().to_string())
                    .or_insert(0) += 1;
            }),
        })
        .collect();

    for (member, identity) in snapshot.members.iter().zip(&identities) {
        // 结构节点（SITE/ZONE/PIPE）没有几何，只提供层级，不判定。
        if member.geometries.is_empty() {
            summary.exempt_no_geometry += 1;
            continue;
        }

        // Obstruction / Insulation 桶按约定豁免 missing 判定。
        let all_exempt_bucket = member.geometries.iter().all(|g| g.geo_type != "Primitive");
        if all_exempt_bucket {
            summary.exempt_geo_type += 1;
            continue;
        }

        // RVM 只给了中心线：`Line` 没有表面（`mesh_compare::add_geometry` 同样跳过它），
        // 说明 **E3D 自己也没画实体**——SCTN 无 SPRE / 截面一个成员都出不了形状时它就只画
        // 中心线（`e3d-model::elmodl::section_body` 的 `CentrelineOnly` 臂，理由逐条写着）。
        // 这种成员生成侧没有实例是**两边一致**，不是缺件。
        // `1RX03-MSUP-E0110` 整份 41 个图元**全是 Line**，之前被逐个判成 `missing_in_gen`。
        let centreline_only = member.geometries.iter().all(|g| g.kind == "Line");
        if centreline_only {
            summary.exempt_centreline_only += 1;
            continue;
        }

        let Some((resolved, source)) = identity else {
            summary.exempt_unresolved += 1;
            items.push(json!({
                "name": member.name,
                "status": "unresolved_identity",
            }));
            continue;
        };

        summary.compared += 1;
        for geometry in &member.geometries {
            *geo_kind_counts.entry(geometry.kind.clone()).or_insert(0) += 1;
        }

        // 隐式管段查 `tubi_relate`（一条 BRANCH 的管段一次读全，之后按序号取）；
        // 真实元素查 `inst_relate`。
        let (id_label, instance) = match resolved {
            Resolved::Element(refno) => (refno.clone(), gen_side.get(refno).cloned()),
            Resolved::Tube { branch, ordinal } => {
                if !branch_tubes.contains_key(branch) {
                    let tubes = load_branch_tubes(&db, &index, branch).await?;
                    branch_tubes.insert(branch.clone(), tubes);
                }
                let tube = branch_tubes[branch].get(ordinal - 1).cloned();
                if tube.is_some() {
                    summary.tubes_compared += 1;
                } else {
                    summary.tubes_missing_in_gen += 1;
                }
                (format!("tubi {branch}#{ordinal}"), tube)
            }
        };

        let Some(instance) = instance else {
            summary.missing_in_gen += 1;
            items.push(json!({
                "refno": id_label,
                "name": member.name,
                "noun": member.noun,
                "identity_source": source.as_str(),
                "rvm_geos": member.geometries.len(),
                "status": "missing_in_gen",
            }));
            continue;
        };

        summary.matched += 1;
        let mut item = json!({
            "refno": id_label,
            "name": member.name,
            "noun": member.noun,
            "identity_source": source.as_str(),
            "status": "matched",
            "rvm_geos": member.geometries.len(),
            "gen_geos_visible": instance.visible_geos,
            "gen_geos_total": instance.total_geos,
        });

        // L3-a：world 平移。RVM 的 CNTB translation 已经是绝对世界坐标（mm）。
        if let Some(gen_translation) = instance.translation {
            summary.translation_compared += 1;
            let delta = (0..3)
                .map(|i| (member.translation_mm[i] as f64 - gen_translation[i]).abs())
                .fold(0.0_f64, f64::max);
            summary.max_translation_delta_mm = summary.max_translation_delta_mm.max(delta);
            if delta > options.tol_translation_mm {
                summary.translation_mismatch += 1;
                item["translation_mismatch"] = json!({
                    "rvm": member.translation_mm,
                    "gen": gen_translation,
                    "max_delta_mm": delta,
                });
            } else {
                item["translation_delta_mm"] = json!(delta);
            }
        }

        // L3-b：只有窄口径基准排除了保温与障碍/预留体，AABB 才与生成侧同口径。
        // 口径不对就一条都不比——比出来的差异说的是口径，不是几何。
        if !aabb_is_judgeable {
            item["aabb_skipped"] = json!(summary.baseline_scope.as_str());
        } else if let (Some(rvm_aabb), Some(gen_aabb)) = (member.aabb_world_mm, instance.aabb) {
            summary.aabb_compared += 1;
            let delta = (0..6)
                .map(|i| (rvm_aabb[i] - gen_aabb[i]).abs())
                .fold(0.0_f64, f64::max);
            summary.max_aabb_delta_mm = summary.max_aabb_delta_mm.max(delta);
            if delta > options.tol_aabb_mm {
                summary.aabb_mismatch += 1;
                item["aabb_mismatch"] = json!({
                    "rvm": rvm_aabb,
                    "gen": gen_aabb,
                    "max_delta_mm": delta,
                });
            } else {
                item["aabb_delta_mm"] = json!(delta);
            }
        }

        // L4：双向表面距离——姿态错了它必然爆，而且不需要两侧图元一一对应。
        // 口径不对时同样不能比（宽口径的 RVM 面里有保温/预留体，生成侧有意不产），
        // 但那是**没量到**、不是豁免：照样挡住 PASS。
        if rvm_meshes.is_some() {
            if aabb_is_judgeable {
                surface_check(
                    rvm_meshes.as_ref(),
                    &db,
                    resolved,
                    &member.name,
                    (options.tol_surface_p95_mm, options.tol_surface_hausdorff_mm),
                    &mut summary,
                    &mut item,
                )
                .await?;
            } else {
                summary.surface_unavailable += 1;
                item["surface_unavailable"] = json!("wide_scope_baseline");
            }
        }

        items.push(item);
    }

    // extra：生成侧有、RVM 基准没有的实例。
    let rvm_refnos: HashSet<&String> = identities
        .iter()
        .filter_map(|identity| match identity {
            Some((Resolved::Element(refno), _)) => Some(refno),
            _ => None,
        })
        .collect();
    for refno in gen_side.keys() {
        if !rvm_refnos.contains(refno) {
            summary.extra_in_gen += 1;
            items.push(json!({ "refno": refno, "status": "extra_in_gen" }));
        }
    }

    write_report(options, &snapshot, &summary, &geo_kind_counts, &items)?;
    print_summary(options, &summary, &geo_kind_counts);

    Ok(summary)
}

/// 一个成员的 L4：RVM 面 vs 生成面的双向表面距离。
///
/// 量不到的三种情形都记进 `surface_unavailable`，**不是豁免**：隐式管段没有 `pe` 行、
/// RVM 侧只有中心线（`Line` 没有表面）、生成侧建不出网格——三种都意味着这个成员的
/// 姿态没有被判过，不能和「判了都对」共用一个退出码。
#[cfg(feature = "manifold")]
#[allow(clippy::too_many_arguments)]
async fn surface_check(
    rvm_meshes: Option<&HashMap<String, parry3d::shape::TriMesh>>,
    db: &Surreal<Any>,
    resolved: &Resolved,
    member_name: &str,
    tol: (f64, f64),
    summary: &mut CompareSummary,
    item: &mut serde_json::Value,
) -> Result<()> {
    let (tol_p95_mm, tol_hausdorff_mm) = tol;
    let Some(meshes) = rvm_meshes else {
        return Ok(());
    };
    let Resolved::Element(refno) = resolved else {
        summary.surface_unavailable += 1;
        item["surface_unavailable"] = json!("implicit_tube_has_no_pe_row");
        return Ok(());
    };
    let Some(rvm_mesh) = meshes.get(member_name) else {
        summary.surface_unavailable += 1;
        item["surface_unavailable"] = json!("no_rvm_surface");
        return Ok(());
    };
    let gen_mesh = super::mesh_compare::gen_world_mesh(db, &resolve::pe_key(refno))
        .await
        .with_context(|| format!("重建生成侧网格失败: {refno}"))?;
    let Some(gen_mesh) = gen_mesh else {
        summary.surface_unavailable += 1;
        item["surface_unavailable"] = json!("no_gen_mesh");
        return Ok(());
    };
    // 每侧采样数与 `mesh_wall_live` 同量级：低差异序列，可复跑。
    let Some(distance) =
        crate::fast_model::shared::two_sided_surface_distance(rvm_mesh, &gen_mesh, 4_000)
    else {
        summary.surface_unavailable += 1;
        item["surface_unavailable"] = json!("no_samples");
        return Ok(());
    };
    summary.surface_compared += 1;
    summary.max_surface_p95_mm = summary.max_surface_p95_mm.max(distance.p95 as f64);
    summary.max_surface_hausdorff_mm = summary
        .max_surface_hausdorff_mm
        .max(distance.hausdorff as f64);
    let payload = json!({
        "mean_mm": distance.mean,
        "p95_mm": distance.p95,
        "hausdorff_mm": distance.hausdorff,
        "samples": distance.samples,
    });
    // p95 管「整体贴不贴」，Hausdorff 管「有没有一处离谱」——开孔或多出一块板只影响
    // 极少数采样点，p95 完全看不见（GWALL 实测 p95 0.001 mm 而 Hausdorff 2,010 mm）。
    if distance.p95 as f64 > tol_p95_mm || distance.hausdorff as f64 > tol_hausdorff_mm {
        summary.surface_mismatch += 1;
        let mut payload = payload;
        payload["where"] = locate_difference(rvm_mesh, &gen_mesh);
        item["surface_mismatch"] = payload;
    } else {
        item["surface"] = payload;
    }
    Ok(())
}

/// 差在哪一块：**方向**说的是「谁多了面」，坐标说的是「多在什么位置」。
///
/// 双向合并会把方向信息抹掉，而方向正是要问的那件事：
/// - `rvm_to_gen` 大 = RVM 有面而生成侧没有 → 我们**少了材料**（多切了一个洞 / 少了一块板）。
/// - `gen_to_rvm` 大 = 生成侧有面而 RVM 没有 → 我们**多了材料**（该切的洞没切 / 多了一块板）。
///
/// 再给最远的几个点与它们在对侧的最近点：同一处的一簇点说明是**一块**局部差异
/// （开孔、一块板），散在整面上才是姿态或尺寸问题。
#[cfg(feature = "manifold")]
fn locate_difference(
    rvm_mesh: &parry3d::shape::TriMesh,
    gen_mesh: &parry3d::shape::TriMesh,
) -> serde_json::Value {
    use crate::fast_model::shared::{farthest_surface_pairs, one_way_surface_distance};

    let side = |d: Option<crate::fast_model::shared::SurfaceDistance>| match d {
        Some(d) => json!({ "p95_mm": d.p95, "hausdorff_mm": d.hausdorff, "mean_mm": d.mean }),
        None => serde_json::Value::Null,
    };
    let r2g = one_way_surface_distance(rvm_mesh, gen_mesh, 4_000);
    let g2r = one_way_surface_distance(gen_mesh, rvm_mesh, 4_000);
    let verdict = match (
        r2g.as_ref().map(|d| d.hausdorff).unwrap_or(0.0),
        g2r.as_ref().map(|d| d.hausdorff).unwrap_or(0.0),
    ) {
        (r, g) if r > g * 2.0 => "gen_missing_material",
        (r, g) if g > r * 2.0 => "gen_extra_material",
        _ => "both_sides",
    };
    let worst = |from: &parry3d::shape::TriMesh, to: &parry3d::shape::TriMesh| {
        farthest_surface_pairs(from, to, 4_000, 5)
            .into_iter()
            .map(|(at, nearest, dist)| json!({ "at_mm": at, "nearest_mm": nearest, "dist_mm": dist }))
            .collect::<Vec<_>>()
    };
    json!({
        "verdict": verdict,
        "rvm_to_gen": side(r2g),
        "gen_to_rvm": side(g2r),
        "worst_rvm_points_without_gen_surface": worst(rvm_mesh, gen_mesh),
        "worst_gen_points_without_rvm_surface": worst(gen_mesh, rvm_mesh),
    })
}

#[cfg(not(feature = "manifold"))]
#[allow(clippy::too_many_arguments)]
async fn surface_check(
    _rvm_meshes: Option<&()>,
    _db: &Surreal<Any>,
    _resolved: &Resolved,
    _member_name: &str,
    _tol: (f64, f64),
    _summary: &mut CompareSummary,
    _item: &mut serde_json::Value,
) -> Result<()> {
    Ok(())
}

fn write_report(
    options: &CompareOptions,
    snapshot: &RvmSnapshot,
    summary: &CompareSummary,
    geo_kind_counts: &BTreeMap<String, usize>,
    items: &[serde_json::Value],
) -> Result<()> {
    if let Some(dir) = options.report_path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建报告目录失败: {}", dir.display()))?;
        }
    }
    let report = json!({
        "snapshot": options.snapshot_path.display().to_string(),
        "root": snapshot.meta.root_name,
        "dbnum": snapshot.meta.dbnum,
        "surreal": { "url": options.url, "ns": options.ns, "db": options.db },
        "tolerance": {
            "translation_mm": options.tol_translation_mm,
            "aabb_mm": options.tol_aabb_mm,
        },
        "summary": summary,
        "rvm_geometry_kinds": geo_kind_counts,
        "items": items,
    });
    std::fs::write(
        &options.report_path,
        serde_json::to_string_pretty(&report).context("序列化报告失败")?,
    )
    .with_context(|| format!("写入报告失败: {}", options.report_path.display()))?;
    Ok(())
}

fn print_summary(
    options: &CompareOptions,
    summary: &CompareSummary,
    geo_kind_counts: &BTreeMap<String, usize>,
) {
    println!("RVM 基准对拍");
    println!(
        "  基准导出口径   : {}{}",
        summary.baseline_scope.as_str(),
        if summary.baseline_scope.allows_aabb_verdict() {
            ""
        } else {
            "（含保温/障碍或未声明，AABB 不参与判定，不得 PASS）"
        }
    );
    println!("  参与判定       : {}", summary.compared);
    println!(
        "  L1 匹配/缺失/多出: {} / {} / {}",
        summary.matched, summary.missing_in_gen, summary.extra_in_gen
    );
    println!(
        "  豁免 无几何/未解析/非Primitive/仅中心线: {} / {} / {} / {}",
        summary.exempt_no_geometry,
        summary.exempt_unresolved,
        summary.exempt_geo_type,
        summary.exempt_centreline_only
    );
    let resolved = summary
        .resolved_here
        .iter()
        .map(|(source, count)| format!("{source}={count}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!(
        "  本步解出的身份  : {}",
        if resolved.is_empty() {
            "<快照自带>".into()
        } else {
            resolved
        }
    );
    println!(
        "  隐式管段 比较/缺失: {} / {}",
        summary.tubes_compared, summary.tubes_missing_in_gen
    );
    let kinds = geo_kind_counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!(
        "  L2 RVM 原语分布 : {}",
        if kinds.is_empty() {
            "<空>".into()
        } else {
            kinds
        }
    );
    println!(
        "  L3 平移 比较/超限: {} / {}  (最大 {:.3} mm，容差 {} mm)",
        summary.translation_compared,
        summary.translation_mismatch,
        summary.max_translation_delta_mm,
        options.tol_translation_mm
    );
    println!(
        "  L3 AABB 比较/超限: {} / {}  (最大 {:.3} mm，容差 {} mm)",
        summary.aabb_compared,
        summary.aabb_mismatch,
        summary.max_aabb_delta_mm,
        options.tol_aabb_mm
    );
    println!(
        "  L4 表面 比较/超限/没量到: {} / {} / {}  (p95 最大 {:.3} / 容差 {} mm；Hausdorff 最大 {:.3} / 容差 {} mm)",
        summary.surface_compared,
        summary.surface_mismatch,
        summary.surface_unavailable,
        summary.max_surface_p95_mm,
        options.tol_surface_p95_mm,
        summary.max_surface_hausdorff_mm,
        options.tol_surface_hausdorff_mm,
    );
    println!(
        "  未支持的必检项  : {}",
        match summary.unsupported_checks.is_empty() {
            true => "<无>".to_string(),
            false => summary.unsupported_checks.join(", "),
        }
    );
    println!("  报告           : {}", options.report_path.display());
    println!(
        "  判定           : {}",
        if summary.passed() { "PASS" } else { "FAIL" }
    );
}

pub fn default_report_path(root: Option<&str>) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let name = root
        .map(|r| r.trim_start_matches('/').replace('/', "-"))
        .unwrap_or_else(|| "unknown".to_string());
    Path::new("output/rvm-verify").join(format!("{name}-{stamp}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 宽口径基准不具备 AABB 判定资格，其它指标再漂亮也不能 PASS。
    ///
    /// 这条锁的是一次真实事故：口径要求只写在模块注释里，夹具却是
    /// `repre obst on` 导的，于是 17 个电缆槽的预留净空（100mm）被逐个报成
    /// AABB 超限，一个口径分歧看上去像三十五个几何缺陷。
    #[test]
    fn a_wide_scope_baseline_cannot_produce_a_spatial_verdict() {
        let mut summary = CompareSummary {
            baseline_scope: ExportScope::Narrow,
            compared: 1,
            matched: 1,
            translation_compared: 1,
            aabb_compared: 1,
            surface_compared: 1,
            ..Default::default()
        };
        assert!(summary.passed(), "窄口径且各项齐备时应当通过");

        for scope in [ExportScope::Wide, ExportScope::Unknown] {
            summary.baseline_scope = scope;
            assert!(
                !summary.passed(),
                "{} 口径的基准不该给出空间判定",
                scope.as_str()
            );
        }
    }

    #[test]
    fn pass_is_fail_closed_for_missing_or_mismatched_spatial_checks() {
        let mut summary = CompareSummary {
            baseline_scope: ExportScope::Narrow,
            ..Default::default()
        };
        assert!(
            !summary.passed(),
            "zero compared members cannot prove correctness"
        );

        summary.compared = 1;
        summary.matched = 1;
        summary.translation_compared = 1;
        summary.aabb_compared = 1;
        summary.surface_compared = 1;
        summary.aabb_mismatch = 1;
        assert!(
            !summary.passed(),
            "an AABB mismatch must fail the spatial verdict"
        );
        summary.aabb_mismatch = 0;
        assert!(summary.passed());

        summary
            .unsupported_checks
            .push("world_rotation".to_string());
        assert!(
            !summary.passed(),
            "an unimplemented required check cannot produce PASS"
        );
        summary.unsupported_checks.clear();

        summary.exempt_unresolved = 1;
        assert!(
            !summary.passed(),
            "unresolved identities make the verdict incomplete"
        );
        summary.exempt_unresolved = 0;
        summary.translation_compared = 0;
        assert!(
            !summary.passed(),
            "a matched member without a transform was not checked"
        );
    }

    #[test]
    fn compare_rejects_invalid_tolerances() {
        assert!(validate_tolerance("translation", 0.0).is_ok());
        assert!(validate_tolerance("translation", -0.1).is_err());
        assert!(validate_tolerance("translation", f64::NAN).is_err());
        assert!(validate_tolerance("translation", f64::INFINITY).is_err());
    }

    #[test]
    fn subtree_query_uses_a_validated_root() {
        assert_eq!(canonical_refno(" 24384/22404 ").unwrap(), "24384/22404");
        assert!(canonical_refno("24384/0").is_err());
        assert!(canonical_refno("24384/22404 OR true").is_err());
    }

    /// 隐式 TUBI 接上了，不该再挂在「未实现」名单上；world rotation 只有在
    /// **真跑了** L4 表面对拍时才摘得掉——没给 `--rvm` 就没量到，必须继续挡着 PASS。
    #[test]
    fn world_rotation_leaves_the_unsupported_list_only_when_the_surface_layer_ran() {
        let source = include_str!("compare.rs");
        let wired = source
            .split_once("let mut summary = CompareSummary {")
            .expect("summary 初始化还在")
            .1
            .split_once("let aabb_is_judgeable")
            .expect("summary 初始化的边界")
            .0;
        assert!(
            wired.contains("match rvm_meshes.is_some()")
                && wired.contains("vec![\"world_rotation\".into()]"),
            "没跑表面对拍时 world_rotation 必须留在名单上: {wired}"
        );
        assert!(
            !wired.contains("\"tubi_relate\".into()"),
            "隐式 TUBI 已接进 `load_branch_tubes`，名单上不该还挂着它"
        );
    }

    /// 「没量到」不许和「量了都对」共用一个退出码：匹配上的成员必须每一个都真比过表面。
    #[test]
    fn a_member_whose_surface_was_never_measured_cannot_pass() {
        let mut summary = CompareSummary {
            baseline_scope: ExportScope::Narrow,
            compared: 2,
            matched: 2,
            translation_compared: 2,
            aabb_compared: 2,
            surface_compared: 2,
            ..Default::default()
        };
        assert!(summary.passed(), "两个成员都判过表面时应当通过");

        summary.surface_compared = 1;
        summary.surface_unavailable = 1;
        assert!(!summary.passed(), "一个成员的姿态没量到，整轮就不能算通过");

        summary.surface_compared = 2;
        summary.surface_unavailable = 0;
        summary.surface_mismatch = 1;
        assert!(!summary.passed(), "表面超限必须判红");
    }
}
