//! 显式启用的历史 OCC/gen_model 入口；生产任务不得调用本二进制。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut dbnums = Vec::new();
    let mut output_dir = None;
    let mut namespace = None;
    let mut database = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dbnum" => dbnums.push(
                args.next()
                    .ok_or_else(|| anyhow::anyhow!("--dbnum requires a value"))?
                    .parse::<u32>()?,
            ),
            "--output-dir" => {
                output_dir = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--output-dir requires a value"))?,
                );
            }
            "--namespace" => {
                namespace = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--namespace requires a value"))?,
                );
            }
            "--database" => {
                database = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--database requires a value"))?,
                );
            }
            _ => anyhow::bail!("unknown argument: {arg}"),
        }
    }
    anyhow::ensure!(!dbnums.is_empty(), "at least one --dbnum is required");
    let output_dir = output_dir.ok_or_else(|| anyhow::anyhow!("--output-dir is required"))?;
    let namespace = namespace.ok_or_else(|| anyhow::anyhow!("--namespace is required"))?;
    let database = database.ok_or_else(|| anyhow::anyhow!("--database is required"))?;
    aios_core::init_test_surreal().await?;
    aios_core::SUL_DB
        .use_ns(&namespace)
        .use_db(&database)
        .await?;
    let mut db_option = aios_core::get_db_option().clone();
    db_option.meshes_path = Some(output_dir.clone());
    aios_database::fast_model::legacy::generate_dbnums(&dbnums, &db_option).await?;
    let dbnums_sql = dbnums
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    aios_core::SUL_DB
        .query(format!(
            "UPDATE inst_relate SET direct_model.source='legacy', \
             direct_model.format='legacy-model' WHERE dbnum IN [{dbnums_sql}];"
        ))
        .await?
        .check()?;
    println!(
        "LEGACY_MODEL_GENERATION source=legacy dbnums={} namespace={namespace} database={database} output_dir={output_dir}",
        dbnums
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    );
    Ok(())
}
