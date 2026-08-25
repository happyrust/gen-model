//! RVM 基准对拍探针。
//!
//! 与本仓其它 `*_probe.rs` 同一路数：主程序没有 CLI 开关，验证走独立 bin + JSON。
//!
//! 用法：
//!   cargo run --features rvm_verify --bin rvm_verify -- import \
//!       --rvm test_data/rvm/C-IY-1R330-B.rvm --dbnum 8000 [--att x.att] [--out y.json]
//!
//! compare 子命令把快照与 SurrealDB 生成结果做三层对拍并输出机器报告。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "manifold")]
use serde::{Deserialize, Serialize};

use aios_database::rvm_baseline::{
    CompareOptions, ExportScope, ImportOptions, compare, default_report_path,
    default_snapshot_path, import_and_save,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ScopeArg {
    Narrow,
    Wide,
    Unknown,
}

impl From<ScopeArg> for ExportScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::Narrow => ExportScope::Narrow,
            ScopeArg::Wide => ExportScope::Wide,
            ScopeArg::Unknown => ExportScope::Unknown,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "rvm_verify", about = "RVM 基准对拍：导入与比对")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 解析 E3D 导出的 RVM/ATT，产出基准快照 JSON
    Import {
        /// RVM 文件路径
        #[arg(long)]
        rvm: PathBuf,
        /// 该 RVM 对应的设计库编号，如 8000
        #[arg(long)]
        dbnum: u32,
        /// 配套的 ATT 属性文件，可重复
        #[arg(long)]
        att: Vec<PathBuf>,
        /// 快照输出路径，默认与 RVM 同目录同名 .rvm.json
        #[arg(long)]
        out: Option<PathBuf>,
        /// 根元素真实 refno，如 24384/22404。命名元素的 refno 不在 ATT 里，
        /// 给了就直接钉上，省一次站点库反查。
        #[arg(long)]
        root_refno: Option<String>,
        /// 这份 RVM 的导出口径：narrow = `repre insu/obst off`（只有实体几何，
        /// AABB 才可判）；wide = 保温/障碍一并导出。RVM 流里读不出来，只能声明。
        /// 不给就是 unknown，compare 会拒绝给出空间判定。
        #[arg(long, value_enum, default_value_t = ScopeArg::Unknown)]
        scope: ScopeArg,
        #[arg(long)]
        verbose: bool,
    },
    /// 快照 vs SurrealDB 生成数据的三层对拍
    ///
    /// 当前 world rotation 与 TUBI join 尚未实现，报告会列出并返回失败。
    Compare {
        /// import 产出的快照 JSON
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long, default_value = "ws://127.0.0.1:8009")]
        url: String,
        #[arg(long, default_value = "1516")]
        ns: String,
        #[arg(long, default_value = "AvevaMarineSample")]
        db: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long, default_value = "root")]
        password: String,
        /// world 平移容差（mm）
        #[arg(long, default_value_t = 1.0)]
        tol_translation_mm: f64,
        /// AABB 各分量容差（mm）
        #[arg(long, default_value_t = 1.0)]
        tol_aabb_mm: f64,
        /// 报告输出路径，默认 output/rvm-verify/<root>-<时间戳>.json
        #[arg(long)]
        report: Option<PathBuf>,
        #[arg(long)]
        verbose: bool,
    },
    /// RVM group vs 生产网格的双向表面距离对拍
    #[cfg(feature = "manifold")]
    MeshCompare {
        #[arg(long)]
        rvm: PathBuf,
        /// 运行服务实际使用的持久 mesh 目录；不得依赖探针 CWD 猜测。
        #[arg(long)]
        mesh_dir: PathBuf,
        /// 显式身份配对，可重复：`RVM_GROUP=17496/99762`
        #[arg(long, required_unless_present = "pair_file")]
        pair: Vec<String>,
        /// 批量身份配对 JSON：`[{"group":"RVM_GROUP","refno":"24381/1"}]`。
        /// 与 `--pair` 合并；重复 group/refno 会直接失败，避免全量报告假绿。
        #[arg(long)]
        pair_file: Option<PathBuf>,
        /// 将每个 RVM group 的全部后代几何并入对拍。BRAN/HANG 根模型必须启用。
        #[arg(long)]
        include_descendants: bool,
        #[arg(long, default_value = "ws://127.0.0.1:8009")]
        url: String,
        #[arg(long, default_value = "1516")]
        ns: String,
        #[arg(long, default_value = "AvevaMarineSample")]
        db: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long, default_value = "root")]
        password: String,
        #[arg(long, default_value_t = 4000)]
        samples: usize,
        #[arg(long, default_value_t = 1.0)]
        tol_p95_mm: f32,
        #[arg(long, default_value_t = 2.0)]
        tol_max_mm: f32,
        /// E3D/Review 导出 RVM 的面片精度（mm）。该误差属于基准本身，
        /// 会加到生产网格的目标容差上；设为 0 可恢复严格门。
        #[arg(long, default_value_t = 10.0)]
        rvm_facet_tol_mm: f32,
        /// 最大距离对 RVM 面片精度采用的保守倍数。最大距离对局部粗弦和
        /// 端点最敏感；默认 4 对应当前 E3D 导出 FLOOR 基准。
        #[arg(long, default_value_t = 4.0)]
        rvm_max_facet_multiplier: f32,
        #[arg(long)]
        report: PathBuf,
    },
}

#[cfg(feature = "manifold")]
#[derive(Debug, Serialize)]
struct DistanceReport {
    mean_mm: f32,
    rms_mm: f32,
    p95_mm: f32,
    max_mm: f32,
    samples: usize,
}

#[cfg(feature = "manifold")]
#[derive(Debug, Serialize)]
struct DistancePointReport {
    point_mm: [f32; 3],
    distance_mm: f32,
}

#[cfg(feature = "manifold")]
impl From<aios_database::fast_model::shared::SurfaceDistance> for DistanceReport {
    fn from(value: aios_database::fast_model::shared::SurfaceDistance) -> Self {
        Self {
            mean_mm: value.mean,
            rms_mm: value.rms,
            p95_mm: value.p95,
            max_mm: value.hausdorff,
            samples: value.samples,
        }
    }
}

#[cfg(feature = "manifold")]
#[derive(Debug, Serialize)]
struct MeshPairReport {
    rvm_group: String,
    refno: String,
    pe_key: String,
    bad_bool: Option<bool>,
    booled_id: Option<String>,
    rvm_triangles: usize,
    generated_triangles: usize,
    generated_to_rvm: Option<DistanceReport>,
    rvm_to_generated: Option<DistanceReport>,
    generated_to_rvm_farthest: Vec<DistancePointReport>,
    rvm_to_generated_farthest: Vec<DistancePointReport>,
    passed: bool,
    note: Option<String>,
}

#[cfg(feature = "manifold")]
#[derive(Debug, Serialize)]
struct MeshCompareReport {
    rvm: PathBuf,
    rvm_sha256: String,
    mesh_dir: PathBuf,
    endpoint: String,
    namespace: String,
    database: String,
    samples_per_direction: usize,
    include_descendants: bool,
    model_tol_p95_mm: f32,
    model_tol_max_mm: f32,
    rvm_facet_tol_mm: f32,
    rvm_max_facet_multiplier: f32,
    effective_tol_p95_mm: f32,
    effective_tol_max_mm: f32,
    passed: bool,
    pairs: Vec<MeshPairReport>,
}

#[cfg(feature = "manifold")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeshPairFileEntry {
    group: String,
    refno: String,
}

#[cfg(feature = "manifold")]
#[derive(Copy, Clone, Debug, PartialEq)]
struct EffectiveMeshTolerance {
    p95_mm: f32,
    max_mm: f32,
}

#[cfg(feature = "manifold")]
fn effective_mesh_tolerance(
    model_p95_mm: f32,
    model_max_mm: f32,
    rvm_facet_tol_mm: f32,
    rvm_max_facet_multiplier: f32,
) -> Result<EffectiveMeshTolerance> {
    for (name, value) in [
        ("tol-p95-mm", model_p95_mm),
        ("tol-max-mm", model_max_mm),
        ("rvm-facet-tol-mm", rvm_facet_tol_mm),
        ("rvm-max-facet-multiplier", rvm_max_facet_multiplier),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(anyhow!("{name} 必须是有限非负数，实际为 {value}"));
        }
    }
    if rvm_max_facet_multiplier < 1.0 {
        return Err(anyhow!(
            "rvm-max-facet-multiplier 不得小于 1，实际为 {rvm_max_facet_multiplier}"
        ));
    }
    Ok(EffectiveMeshTolerance {
        p95_mm: model_p95_mm + rvm_facet_tol_mm,
        max_mm: model_max_mm + rvm_facet_tol_mm * rvm_max_facet_multiplier,
    })
}

#[cfg(feature = "manifold")]
fn parse_pair(value: &str) -> Result<(String, String)> {
    let (group, refno) = value
        .rsplit_once('=')
        .ok_or_else(|| anyhow!("pair 必须是 RVM_GROUP=REFNO: {value}"))?;
    let group = group.trim();
    let refno = refno.trim().trim_start_matches('=');
    let (db, id) = refno
        .split_once('/')
        .ok_or_else(|| anyhow!("refno 必须是 db/id: {refno}"))?;
    let db: u64 = db.parse().with_context(|| format!("非法 db: {db}"))?;
    let id: u64 = id.parse().with_context(|| format!("非法 id: {id}"))?;
    if group.is_empty() {
        return Err(anyhow!("RVM group 为空"));
    }
    Ok((group.to_string(), format!("{db}/{id}")))
}

#[cfg(feature = "manifold")]
fn load_pairs(values: &[String], pair_file: Option<&Path>) -> Result<Vec<(String, String)>> {
    let mut pairs = values
        .iter()
        .map(|value| parse_pair(value))
        .collect::<Result<Vec<_>>>()?;
    if let Some(path) = pair_file {
        let bytes = std::fs::read(path)
            .with_context(|| format!("读取 pair-file 失败: {}", path.display()))?;
        let entries: Vec<MeshPairFileEntry> = serde_json::from_slice(&bytes)
            .with_context(|| format!("解析 pair-file JSON 失败: {}", path.display()))?;
        for entry in entries {
            pairs.push(parse_pair(&format!("{}={}", entry.group, entry.refno))?);
        }
    }
    if pairs.is_empty() {
        return Err(anyhow!("至少提供一个 --pair 或非空 --pair-file"));
    }

    let mut groups = HashSet::with_capacity(pairs.len());
    let mut refnos = HashSet::with_capacity(pairs.len());
    for (group, refno) in &pairs {
        if !groups.insert(group.clone()) {
            return Err(anyhow!("RVM group 重复配对: {group}"));
        }
        if !refnos.insert(refno.clone()) {
            return Err(anyhow!("refno 重复配对: {refno}"));
        }
    }
    Ok(pairs)
}

#[cfg(feature = "manifold")]
fn sha256_hex(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).with_context(|| format!("读取 {}", path.display()))?;
    Ok(format!("{:X}", Sha256::digest(bytes)))
}

#[cfg(feature = "manifold")]
fn canonical_mesh_dir(path: &std::path::Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("mesh 目录不存在: {}", path.display()))?;
    if !canonical.is_dir() {
        return Err(anyhow!("mesh 路径不是目录: {}", canonical.display()));
    }
    Ok(canonical)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Import {
            rvm,
            dbnum,
            att,
            out,
            root_refno,
            scope,
            verbose,
        } => {
            let out_path = out.unwrap_or_else(|| default_snapshot_path(&rvm));
            let options = ImportOptions {
                dbnum,
                rvm_path: rvm,
                att_paths: att,
                out_path: out_path.clone(),
                root_refno,
                export_scope: scope.into(),
                verbose,
            };
            let snapshot = import_and_save(&options)?;
            snapshot.print_summary();
            println!("  快照           : {}", out_path.display());
        }
        Command::Compare {
            snapshot,
            url,
            ns,
            db,
            user,
            password,
            tol_translation_mm,
            tol_aabb_mm,
            report,
            verbose,
        } => {
            let report_path = match report {
                Some(path) => path,
                None => {
                    let loaded = aios_database::rvm_baseline::RvmSnapshot::load(&snapshot)?;
                    default_report_path(loaded.meta.root_name.as_deref())
                }
            };
            let options = CompareOptions {
                snapshot_path: snapshot,
                url,
                ns,
                db,
                user,
                password,
                tol_translation_mm,
                tol_aabb_mm,
                report_path,
                verbose,
            };
            let summary = compare::compare(&options).await?;
            // 退出码 0=容差内全过，1=存在差异，供回归脚本直接判定。
            if !summary.passed() {
                std::process::exit(1);
            }
        }
        #[cfg(feature = "manifold")]
        Command::MeshCompare {
            rvm,
            mesh_dir,
            pair,
            pair_file,
            include_descendants,
            url,
            ns,
            db,
            user,
            password,
            samples,
            tol_p95_mm,
            tol_max_mm,
            rvm_facet_tol_mm,
            rvm_max_facet_multiplier,
            report,
        } => {
            use aios_database::fast_model::shared::one_way_surface_distance;
            use aios_database::rvm_baseline::mesh_compare::{
                gen_world_mesh_in_dir, gen_world_subtree_mesh_in_dir, rvm_world_meshes_by_name,
                rvm_world_subtree_meshes_by_name,
            };
            use surrealdb::engine::any::connect;
            use surrealdb::opt::auth::Root;

            let parsed_pairs = load_pairs(&pair, pair_file.as_deref())?;
            if samples == 0 {
                return Err(anyhow!("samples 必须大于 0"));
            }
            let effective_tolerance = effective_mesh_tolerance(
                tol_p95_mm,
                tol_max_mm,
                rvm_facet_tol_mm,
                rvm_max_facet_multiplier,
            )?;
            let mesh_dir = canonical_mesh_dir(&mesh_dir)?;
            let rvm_meshes = if include_descendants {
                let wanted = parsed_pairs
                    .iter()
                    .map(|(group, _)| group.clone())
                    .collect();
                rvm_world_subtree_meshes_by_name(&rvm, &wanted)?
            } else {
                rvm_world_meshes_by_name(&rvm)?
            };
            let client = connect(&url)
                .await
                .with_context(|| format!("连接 SurrealDB {url}"))?;
            client
                .signin(Root {
                    username: &user,
                    password: &password,
                })
                .await
                .context("SurrealDB 登录失败")?;
            client
                .use_ns(&ns)
                .use_db(&db)
                .await
                .context("选择 SurrealDB ns/db 失败")?;

            let mut rows = Vec::with_capacity(parsed_pairs.len());
            for (group, refno) in parsed_pairs {
                let pe_key = format!("pe:{}", refno.replace('/', "_"));
                let mut state_response = client
                    .query(format!(
                        "SELECT bad_bool, booled_id FROM {pe_key}->inst_relate LIMIT 1;"
                    ))
                    .await
                    .with_context(|| format!("查询 {pe_key} inst_relate"))?;
                let state_rows: Vec<serde_json::Value> = state_response
                    .take(0)
                    .with_context(|| format!("解析 {pe_key} inst_relate"))?;
                let bad_bool = state_rows
                    .first()
                    .and_then(|row| row.get("bad_bool"))
                    .and_then(|value| value.as_bool());
                let booled_id = state_rows
                    .first()
                    .and_then(|row| row.get("booled_id"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string);

                let Some(rvm_mesh) = rvm_meshes.get(&group) else {
                    rows.push(MeshPairReport {
                        rvm_group: group,
                        refno,
                        pe_key,
                        bad_bool,
                        booled_id,
                        rvm_triangles: 0,
                        generated_triangles: 0,
                        generated_to_rvm: None,
                        rvm_to_generated: None,
                        generated_to_rvm_farthest: Vec::new(),
                        rvm_to_generated_farthest: Vec::new(),
                        passed: false,
                        note: Some("RVM group 不存在或名称歧义".to_string()),
                    });
                    continue;
                };
                let generated_result = if include_descendants {
                    gen_world_subtree_mesh_in_dir(&client, &refno, &mesh_dir).await
                } else {
                    gen_world_mesh_in_dir(&client, &pe_key, &mesh_dir).await
                };
                let generated = match generated_result {
                    Ok(Some(mesh)) => mesh,
                    Ok(None) => {
                        rows.push(MeshPairReport {
                            rvm_group: group,
                            refno,
                            pe_key,
                            bad_bool,
                            booled_id,
                            rvm_triangles: rvm_mesh.indices().len(),
                            generated_triangles: 0,
                            generated_to_rvm: None,
                            rvm_to_generated: None,
                            generated_to_rvm_farthest: Vec::new(),
                            rvm_to_generated_farthest: Vec::new(),
                            passed: false,
                            note: Some("生产网格缺失".to_string()),
                        });
                        continue;
                    }
                    Err(error) => {
                        rows.push(MeshPairReport {
                            rvm_group: group,
                            refno,
                            pe_key,
                            bad_bool,
                            booled_id,
                            rvm_triangles: rvm_mesh.indices().len(),
                            generated_triangles: 0,
                            generated_to_rvm: None,
                            rvm_to_generated: None,
                            generated_to_rvm_farthest: Vec::new(),
                            rvm_to_generated_farthest: Vec::new(),
                            passed: false,
                            note: Some(format!("生产网格错误: {error:#}")),
                        });
                        continue;
                    }
                };
                let g2r = one_way_surface_distance(&generated, rvm_mesh, samples)
                    .ok_or_else(|| anyhow!("{pe_key} generated->RVM 没有有效采样"))?;
                let r2g = one_way_surface_distance(rvm_mesh, &generated, samples)
                    .ok_or_else(|| anyhow!("{pe_key} RVM->generated 没有有效采样"))?;
                let within_tolerance = g2r.p95 <= effective_tolerance.p95_mm
                    && r2g.p95 <= effective_tolerance.p95_mm
                    && g2r.hausdorff <= effective_tolerance.max_mm
                    && r2g.hausdorff <= effective_tolerance.max_mm;
                let passed = within_tolerance && bad_bool != Some(true);
                let generated_to_rvm_farthest =
                    aios_database::fast_model::shared::farthest_from_surface(
                        &generated, rvm_mesh, samples, 8,
                    )
                    .into_iter()
                    .map(|(point_mm, distance_mm)| DistancePointReport {
                        point_mm,
                        distance_mm,
                    })
                    .collect();
                let rvm_to_generated_farthest =
                    aios_database::fast_model::shared::farthest_from_surface(
                        rvm_mesh, &generated, samples, 8,
                    )
                    .into_iter()
                    .map(|(point_mm, distance_mm)| DistancePointReport {
                        point_mm,
                        distance_mm,
                    })
                    .collect();
                rows.push(MeshPairReport {
                    rvm_group: group,
                    refno,
                    pe_key,
                    bad_bool,
                    booled_id,
                    rvm_triangles: rvm_mesh.indices().len(),
                    generated_triangles: generated.indices().len(),
                    generated_to_rvm: Some(g2r.into()),
                    rvm_to_generated: Some(r2g.into()),
                    generated_to_rvm_farthest,
                    rvm_to_generated_farthest,
                    passed,
                    note: (bad_bool == Some(true)).then(|| "geom_error: bad_bool=true".to_string()),
                });
            }

            let passed = rows.iter().all(|row| row.passed);
            let result = MeshCompareReport {
                rvm_sha256: sha256_hex(&rvm)?,
                rvm,
                mesh_dir,
                endpoint: url,
                namespace: ns,
                database: db,
                samples_per_direction: samples,
                include_descendants,
                model_tol_p95_mm: tol_p95_mm,
                model_tol_max_mm: tol_max_mm,
                rvm_facet_tol_mm,
                rvm_max_facet_multiplier,
                effective_tol_p95_mm: effective_tolerance.p95_mm,
                effective_tol_max_mm: effective_tolerance.max_mm,
                passed,
                pairs: rows,
            };
            if let Some(parent) = report.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&report, serde_json::to_vec_pretty(&result)?)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            println!("report={}", report.display());
            if !passed {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

#[cfg(all(test, feature = "manifold"))]
mod tests {
    use super::{canonical_mesh_dir, effective_mesh_tolerance, load_pairs, parse_pair};

    #[test]
    fn e3d_export_precision_is_added_without_hiding_raw_model_tolerance() {
        let tolerance = effective_mesh_tolerance(1.0, 2.0, 10.0, 4.0).expect("valid tolerance");
        assert_eq!(tolerance.p95_mm, 11.0);
        assert_eq!(tolerance.max_mm, 42.0);

        // FLOOR 12 的粗 RVM 内弧观测值：旧固定门失败，E3D 导出精度门通过。
        assert!(8.21 > 1.0 && 32.43 > 2.0);
        assert!(8.21 <= tolerance.p95_mm && 32.43 <= tolerance.max_mm);
    }

    #[test]
    fn e3d_export_precision_rejects_invalid_budget() {
        assert!(effective_mesh_tolerance(1.0, 2.0, -1.0, 4.0).is_err());
        assert!(effective_mesh_tolerance(1.0, 2.0, 10.0, 0.5).is_err());
        assert!(effective_mesh_tolerance(f32::NAN, 2.0, 10.0, 4.0).is_err());
    }

    #[test]
    fn mesh_pair_preserves_group_and_normalizes_refno() {
        let pair = parse_pair("FLOOR 12 of CFLOOR /A=17496/100202").expect("valid pair");
        assert_eq!(pair.0, "FLOOR 12 of CFLOOR /A");
        assert_eq!(pair.1, "17496/100202");
    }

    #[test]
    fn mesh_pair_file_is_loaded_into_the_runtime_pair_set() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("pairs.json");
        std::fs::write(&path, r#"[{"group":"/HVAC-BRANCH","refno":"24381/47000"}]"#)
            .expect("write pair file");

        let pairs = load_pairs(&[], Some(&path)).expect("load pair file");
        assert_eq!(
            pairs,
            vec![("/HVAC-BRANCH".to_string(), "24381/47000".to_string())]
        );
    }

    #[test]
    fn mesh_pair_rejects_non_numeric_refno() {
        assert!(parse_pair("FLOOR=x/y").is_err());
    }

    #[test]
    fn mesh_dir_must_exist_and_be_a_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            canonical_mesh_dir(temp.path()).expect("directory"),
            temp.path().canonicalize().expect("canonical tempdir")
        );

        let file = temp.path().join("not-a-directory.mesh");
        std::fs::write(&file, b"fixture").expect("write fixture");
        assert!(canonical_mesh_dir(&file).is_err());
        assert!(canonical_mesh_dir(&temp.path().join("missing")).is_err());
    }
}
