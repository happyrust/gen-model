//! 快照（RVM 基准） vs SurrealDB（本仓生成结果）三层对拍。
//!
//! L1 成员清单：按真实 refno join，输出 matched / missing_in_gen / extra_in_gen。
//! L2 几何构成：两侧几何数量与类型分布——**信息项，不判红灯**，原因见下。
//! L3 空间级：world 平移和成员 AABB 都参与判定。基准 RVM 必须使用导出器默认的
//! 窄口径（insu/obst off、level 6），排除生成侧有意忽略的障碍/预留几何。
//!
//! 为什么 L2 只能是信息项：两侧的几何表达根本不是一套。RVM 是 E3D 为渲染
//! 做的原语分解（Cylinder / CircularTorus / Snout…），生成侧 `inst_geo.param`
//! 是 catalogue 参数化几何（实测 BEND 的可见几何是 `PrimExtrusion` 拉伸体）。
//! 同一个 BEND，RVM 给 3 个原语，生成侧给 6 个可见几何 + 若干 CataNeg，
//! 逐参数比对没有共同基准。判定因此落在 L1（成员在不在）与 L3（位置和尺寸对不对）。
//! world rotation 与隐式 TUBI join 尚未实现，会列入 `unsupported_checks` 并阻止 PASS。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;

use super::att::refno_from_att_name;
use super::snapshot::RvmSnapshot;

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
    pub report_path: PathBuf,
    pub verbose: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct CompareSummary {
    /// 参与判定的 RVM 成员数（已解析身份且带几何）。
    pub compared: usize,
    pub matched: usize,
    pub missing_in_gen: usize,
    pub extra_in_gen: usize,
    /// 未解析身份，无法 join；单列记账并让最终判定失败。
    pub exempt_unresolved: usize,
    /// RVM 侧零几何的结构节点（SITE/ZONE/PIPE），不判定。
    pub exempt_no_geometry: usize,
    /// Obstruction / Insulation 桶，按约定豁免 missing 判定。
    pub exempt_geo_type: usize,
    pub translation_compared: usize,
    pub translation_mismatch: usize,
    pub aabb_compared: usize,
    pub aabb_mismatch: usize,
    pub max_translation_delta_mm: f64,
    pub max_aabb_delta_mm: f64,
    pub unsupported_checks: Vec<String>,
}

impl CompareSummary {
    pub fn passed(&self) -> bool {
        self.compared > 0
            && self.matched == self.compared
            && self.translation_compared == self.matched
            && self.aabb_compared == self.matched
            && self.exempt_unresolved == 0
            && self.missing_in_gen == 0
            && self.extra_in_gen == 0
            && self.translation_mismatch == 0
            && self.aabb_mismatch == 0
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

#[derive(Debug, Deserialize)]
struct ChildRow {
    refno: String,
}

fn canonical_refno(value: &str) -> Result<String> {
    refno_from_att_name(&format!("={}", value.trim()))
        .ok_or_else(|| anyhow::anyhow!("invalid PDMS refno: {value}"))
}

fn pe_key(refno: &str) -> String {
    format!("pe:{}", refno.replace('/', "_"))
}

fn render_children_select(refnos: &[String]) -> String {
    let ids = refnos
        .iter()
        .map(|refno| pe_key(refno))
        .collect::<Vec<_>>()
        .join(", ");
    format!("SELECT type::string(in) AS refno FROM pe_owner WHERE out IN [{ids}];")
}

async fn load_subtree_refnos(db: &Surreal<Any>, root_refno: &str) -> Result<Vec<String>> {
    let root_refno = canonical_refno(root_refno)?;
    let mut all = HashSet::from([root_refno.clone()]);
    let mut frontier = vec![root_refno];

    while !frontier.is_empty() {
        let mut next = Vec::new();
        for chunk in frontier.chunks(QUERY_CHUNK) {
            let mut response = db
                .query(render_children_select(chunk))
                .await
                .context("查询 PE 子树失败")?;
            let rows: Vec<ChildRow> = response.take(0).context("解析 PE 子树失败")?;
            for row in rows {
                let child = row.refno.trim_start_matches("pe:").replace('_', "/");
                let child = canonical_refno(&child)?;
                if all.insert(child.clone()) {
                    next.push(child);
                }
            }
        }
        frontier = next;
    }

    Ok(all.into_iter().collect())
}

async fn load_gen_side(
    db: &Surreal<Any>,
    refnos: &[String],
) -> Result<HashMap<String, GenInstance>> {
    let mut out = HashMap::new();

    for chunk in refnos.chunks(QUERY_CHUNK) {
        let ids = chunk
            .iter()
            .map(|r| pe_key(r))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT type::string(in) AS refno, \
                    aabb.d AS aabb, \
                    world_trans.d AS world_trans, \
                    array::len(out->geo_relate[WHERE visible = true]) AS visible_geos, \
                    array::len(out->geo_relate) AS total_geos \
             FROM inst_relate WHERE in IN [{ids}];"
        );

        let mut response = db.query(sql).await.context("查询 inst_relate 失败")?;
        let rows: Vec<GenRow> = response.take(0).context("解析 inst_relate 结果失败")?;

        for row in rows {
            let refno = row.refno.trim_start_matches("pe:").replace('_', "/");
            out.insert(
                refno,
                GenInstance {
                    aabb: row.aabb.map(|a| {
                        [
                            a.mins[0], a.mins[1], a.mins[2], a.maxs[0], a.maxs[1], a.maxs[2],
                        ]
                    }),
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

pub async fn compare(options: &CompareOptions) -> Result<CompareSummary> {
    validate_tolerance("translation", options.tol_translation_mm)?;
    validate_tolerance("AABB", options.tol_aabb_mm)?;
    let snapshot = RvmSnapshot::load(&options.snapshot_path)?;

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
    let root_refno = snapshot
        .members
        .iter()
        .find(|member| member.name == root_name)
        .and_then(|member| member.refno.as_deref())
        .with_context(|| format!("快照根 {root_name} 没有真实 refno，无法枚举生成侧子树"))?;
    let refnos = load_subtree_refnos(&db, root_refno).await?;
    let gen_side = load_gen_side(&db, &refnos).await?;

    let mut summary = CompareSummary {
        unsupported_checks: vec!["world_rotation".into(), "tubi_relate".into()],
        ..Default::default()
    };
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut geo_kind_counts: BTreeMap<String, usize> = BTreeMap::new();

    for member in &snapshot.members {
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

        let Some(refno) = member.refno.as_ref() else {
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

        let Some(instance) = gen_side.get(refno) else {
            summary.missing_in_gen += 1;
            items.push(json!({
                "refno": refno,
                "name": member.name,
                "noun": member.noun,
                "rvm_geos": member.geometries.len(),
                "status": "missing_in_gen",
            }));
            continue;
        };

        summary.matched += 1;
        let mut item = json!({
            "refno": refno,
            "name": member.name,
            "noun": member.noun,
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

        // L3-b：窄口径基准已排除 OBST/预留体，AABB 可直接参与判定。
        if let (Some(rvm_aabb), Some(gen_aabb)) = (member.aabb_world_mm, instance.aabb) {
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

        items.push(item);
    }

    // extra：生成侧有、RVM 基准没有的实例。
    let rvm_refnos: std::collections::HashSet<&String> = snapshot
        .members
        .iter()
        .filter_map(|m| m.refno.as_ref())
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
    println!("  参与判定       : {}", summary.compared);
    println!(
        "  L1 匹配/缺失/多出: {} / {} / {}",
        summary.matched, summary.missing_in_gen, summary.extra_in_gen
    );
    println!(
        "  豁免 无几何/未解析/非Primitive: {} / {} / {}",
        summary.exempt_no_geometry, summary.exempt_unresolved, summary.exempt_geo_type
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
        "  未支持的必检项  : {}",
        summary.unsupported_checks.join(", ")
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

    #[test]
    fn pass_is_fail_closed_for_missing_or_mismatched_spatial_checks() {
        let mut summary = CompareSummary::default();
        assert!(
            !summary.passed(),
            "zero compared members cannot prove correctness"
        );

        summary.compared = 1;
        summary.matched = 1;
        summary.translation_compared = 1;
        summary.aabb_compared = 1;
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
    fn subtree_query_uses_a_validated_root_and_the_owner_edge() {
        assert_eq!(canonical_refno(" 24384/22404 ").unwrap(), "24384/22404");
        assert!(canonical_refno("24384/0").is_err());
        assert!(canonical_refno("24384/22404 OR true").is_err());

        let sql = render_children_select(&["24384/22404".to_string()]);
        assert_eq!(
            sql,
            "SELECT type::string(in) AS refno FROM pe_owner WHERE out IN [pe:24384_22404];"
        );
    }
}
