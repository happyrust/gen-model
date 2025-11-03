#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::str::FromStr;
    use std::sync::Arc;

    use aios_core::mesh_precision::{LodLevel, set_active_precision};
    use aios_core::runtime::init_surreal_with_retry;
    use aios_core::{RefU64, RefnoEnum, get_db_option};
    use anyhow::{Context, Result, anyhow};
    use tempfile::tempdir;

    use crate::fast_model::mesh_generate::gen_inst_meshes;

    const TARGET_REFNO: &str = "21491_18946";

    #[tokio::test]
    #[ignore = "依赖本地 SurrealDB 与项目数据，仅在需要对比 LOD 精度时手动运行"]
    async fn compare_mesh_size_across_lods() -> Result<()> {
        let base_option = get_db_option().clone();
        init_surreal_with_retry(&base_option)
            .await
            .context("初始化 SurrealDB 失败")?;

        let refno: RefnoEnum = RefU64::from_str(TARGET_REFNO)
            .map_err(|_| anyhow!("解析参考号失败: {}", TARGET_REFNO))?
            .into();

        let temp_root = tempdir().context("创建临时目录失败")?;
        let lods = [LodLevel::L1, LodLevel::L3];

        let mut results = Vec::new();
        for lod in lods {
            let size = generate_mesh_for_lod(&base_option, lod, temp_root.path(), refno)
                .await
                .with_context(|| format!("生成 LOD {:?} 模型失败", lod))?;
            results.push((lod, size));
        }

        for (lod, size) in &results {
            println!("LOD {:?}: mesh total size = {} bytes", lod, size);
        }

        let low = results[0].1;
        let high = results[1].1;
        assert!(
            low < high,
            "期望高精度 LOD 的模型体积更大: low={} bytes, high={} bytes",
            low,
            high
        );

        Ok(())
    }

    async fn generate_mesh_for_lod(
        base_option: &aios_core::options::DbOption,
        lod: LodLevel,
        root_dir: &Path,
        refno: RefnoEnum,
    ) -> Result<u64> {
        let lod_dir = root_dir.join(format!("lod_{lod:?}"));
        fs::create_dir_all(&lod_dir)?;

        let mut precision = base_option.mesh_precision.clone();
        precision.default_lod = lod;

        set_active_precision(precision.clone());

        gen_inst_meshes(&[refno], true, lod_dir.clone(), Arc::new(precision.clone()))
            .await
            .context("生成 mesh 失败")?;

        let size = dir_size(&lod_dir)?;
        anyhow::ensure!(size > 0, "LOD {:?} 未生成任何 mesh 文件", lod);
        Ok(size)
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
}
