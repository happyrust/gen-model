use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use aios_core::mesh_precision::{LodLevel, MeshPrecisionProfile, set_active_precision};
use aios_core::runtime::init_surreal_with_retry;
use aios_core::{RefU64, RefnoEnum, get_db_option};
use aios_database::fast_model::export_model::export_gltf::export_gltf_for_refnos;
use aios_database::fast_model::mesh_generate::gen_inst_meshes;
use anyhow::{Context, Result, anyhow};

const TARGET_REFNOS: &[&str] = &["21491_18962"];
const BASE_MESH_DIR: &str = "assets/meshes";

#[tokio::main]
async fn main() -> Result<()> {
    println!("📊 LOD 精度对比报告 (RefNos: {:?})\n", TARGET_REFNOS);

    let base_option = get_db_option().clone();
    init_surreal_with_retry(&base_option)
        .await
        .context("初始化 SurrealDB 失败")?;

    println!(
        "{:<8} {:>16} {:>16} {:>24}",
        "LOD", "Mesh 总体积 (MB)", "本次新增 (MB)", "GLTF 输出路径"
    );
    println!("{:-<76}", "");

    let lods = [LodLevel::L1, LodLevel::L2, LodLevel::L3];
    for lod in lods {
        let (total_size, delta_size, output_path) =
            generate_mesh_for_lod(&base_option, lod, TARGET_REFNOS)
                .await
                .with_context(|| format!("生成 LOD {:?} 模型失败", lod))?;
        println!(
            "{:<8} {:>16.4} {:>16.4} {:>24}",
            format!("{lod:?}"),
            to_megabytes(total_size),
            to_megabytes(delta_size),
            output_path
        );
    }

    Ok(())
}

async fn generate_mesh_for_lod(
    base_option: &aios_core::options::DbOption,
    lod: LodLevel,
    refnos: &[&str],
) -> Result<(u64, u64, String)> {
    let lod_dir = Path::new(BASE_MESH_DIR).join(format!("lod_{lod:?}"));
    if lod_dir.exists() {
        fs::remove_dir_all(&lod_dir)
            .with_context(|| format!("清理旧目录失败: {}", lod_dir.display()))?;
    }
    fs::create_dir_all(&lod_dir).with_context(|| format!("创建目录失败: {}", lod_dir.display()))?;

    let mut precision = base_option.mesh_precision.clone();
    let profile = precision
        .lod_profiles
        .get(&lod)
        .copied()
        .unwrap_or_else(MeshPrecisionProfile::default);
    precision.lod_profiles.insert(lod, profile);
    precision.default_lod = lod;
    let precision_arc = Arc::new(precision.clone());
    set_active_precision(precision);

    let mut refno_enums = Vec::new();
    let before = dir_size(&lod_dir)?;

    for refno_str in refnos {
        let refno: RefnoEnum = RefU64::from_str(refno_str)
            .map_err(|_| anyhow!("解析参考号失败: {}", refno_str))?
            .into();
        refno_enums.push(refno);

        gen_inst_meshes(&[refno], true, lod_dir.clone(), precision_arc.clone())
            .await
            .with_context(|| format!("生成 mesh 失败: {}", refno_str))?;
    }

    let after =
        dir_size(&lod_dir).with_context(|| format!("统计目录大小失败: {}", lod_dir.display()))?;

    // 导出 glTF
    let output_dir = Path::new("output").join(format!("lod_{lod:?}"));
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("创建输出目录失败: {}", output_dir.display()))?;
    let output_path = output_dir.join(format!("{}_{}.gltf", refnos.join("_"), format!("{lod:?}")));
    export_gltf_for_refnos(
        &refno_enums,
        &lod_dir,
        output_path
            .to_str()
            .ok_or_else(|| anyhow!("无法转换输出路径"))?,
        None,
        true,
    )
    .await
    .with_context(|| format!("导出 glTF 失败 (LOD {:?})", lod))?;

    Ok((
        after,
        after.saturating_sub(before),
        output_path.display().to_string(),
    ))
}

fn dir_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

fn to_megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}
