/// 验证 LOD 功能的示例程序
///
/// 功能：
/// 1. 验证 LOD 配置是否正确加载
/// 2. 验证不同 LOD 级别的精度参数是否不同
/// 3. 验证 LOD 子目录是否正确创建
use aios_core::mesh_precision::{LodLevel, MeshPrecisionSettings};
use std::fs;

fn main() -> anyhow::Result<()> {
    println!("🔍 验证 LOD 功能配置\n");

    // 1. 加载配置文件
    let config_content = fs::read_to_string("DbOption.toml")?;

    // 直接解析 precision 部分
    let toml_value: toml::Value = toml::from_str(&config_content)?;
    let precision: MeshPrecisionSettings =
        if let Some(precision_table) = toml_value.get("precision") {
            toml::from_str(&toml::to_string(precision_table)?)?
        } else {
            println!("❌ 配置文件中未找到 [precision] 部分");
            return Ok(());
        };

    println!("📋 当前 LOD 配置:");
    println!("   默认 LOD 级别: {:?}", precision.default_lod);
    println!("   配置的 LOD 级别数量: {}", precision.lod_profiles.len());
    println!();

    // 2. 检查各个 LOD 级别的配置
    let lod_levels = vec![LodLevel::L1, LodLevel::L2, LodLevel::L3];

    for lod in &lod_levels {
        if let Some(profile) = precision.lod_profiles.get(lod) {
            println!("✅ LOD {:?} 配置:", lod);
            println!("   - occ_linear_coeff: {}", profile.occ_linear_coeff);
            println!("   - occ_linear_min: {}", profile.occ_linear_min);
            println!("   - occ_linear_max: {}", profile.occ_linear_max);
            println!("   - neg_multiplier: {}", profile.neg_multiplier);
            println!("   - sweep_multiplier: {}", profile.sweep_multiplier);
            if let Some(tol) = profile.polyhedron_fixed_tol {
                println!("   - polyhedron_fixed_tol: {}", tol);
            }
            println!("   - sphere_subdiv: {}", profile.sphere_subdiv);
            println!("   - radial_segments: {}", profile.radial_segments);
            println!("   - mesh_tol_ratio: {}", profile.mesh_tol_ratio);
            println!();
        } else {
            println!("❌ LOD {:?} 未配置", lod);
            println!();
        }
    }

    // 3. 验证精度参数是否递增
    println!("🔬 验证精度参数递增:");

    let l1_profile = precision.lod_profiles.get(&LodLevel::L1);
    let l2_profile = precision.lod_profiles.get(&LodLevel::L2);
    let l3_profile = precision.lod_profiles.get(&LodLevel::L3);

    if let (Some(l1), Some(l2), Some(l3)) = (l1_profile, l2_profile, l3_profile) {
        // 容差系数应该递减（精度递增）
        let coeff_ok =
            l1.occ_linear_coeff > l2.occ_linear_coeff && l2.occ_linear_coeff > l3.occ_linear_coeff;
        println!(
            "   容差系数递减 (L1 > L2 > L3): {} ({} > {} > {})",
            if coeff_ok { "✅" } else { "❌" },
            l1.occ_linear_coeff,
            l2.occ_linear_coeff,
            l3.occ_linear_coeff
        );

        // 球体细分应该递增
        let s1 = l1.sphere_subdiv;
        let s2 = l2.sphere_subdiv;
        let s3 = l3.sphere_subdiv;
        let subdiv_ok = s1 < s2 && s2 < s3;
        println!(
            "   球体细分递增 (L1 < L2 < L3): {} ({} < {} < {})",
            if subdiv_ok { "✅" } else { "❌" },
            s1,
            s2,
            s3
        );

        // 径向分段应该递增
        let r1 = l1.radial_segments;
        let r2 = l2.radial_segments;
        let r3 = l3.radial_segments;
        let radial_ok = r1 < r2 && r2 < r3;
        println!(
            "   径向分段递增 (L1 < L2 < L3): {} ({} < {} < {})",
            if radial_ok { "✅" } else { "❌" },
            r1,
            r2,
            r3
        );
    } else {
        println!("   ❌ 缺少必要的 LOD 配置");
    }
    println!();

    // 4. 检查 meshes 目录
    let meshes_path = std::path::PathBuf::from("assets/meshes");
    println!("📁 Meshes 目录配置:");
    println!("   路径: {}", meshes_path.display());
    println!("   存在: {}", meshes_path.exists());

    if meshes_path.exists() {
        println!("\n   子目录:");
        for lod in &lod_levels {
            let lod_dir = meshes_path.join(format!("lod_{:?}", lod));
            let exists = lod_dir.exists();
            let file_count = if exists {
                std::fs::read_dir(&lod_dir)
                    .map(|entries| entries.filter_map(Result::ok).count())
                    .unwrap_or(0)
            } else {
                0
            };

            println!(
                "   - lod_{:?}: {} (文件数: {})",
                lod,
                if exists {
                    "✅ 存在"
                } else {
                    "⚠️  不存在"
                },
                file_count
            );
        }
    }
    println!();

    // 5. 总结
    println!("📊 验证总结:");
    let has_all_profiles = lod_levels
        .iter()
        .all(|lod| precision.lod_profiles.contains_key(lod));

    if has_all_profiles {
        println!("   ✅ 所有 LOD 级别都已配置");
        println!("   ✅ LOD 功能已启用");
        println!("\n💡 提示: 运行模型生成时，会自动创建 lod_L1/lod_L2/lod_L3 子目录");
    } else {
        println!("   ❌ 部分 LOD 级别未配置");
        println!("   ⚠️  请检查 DbOption.toml 配置文件");
    }

    Ok(())
}
