use std::fs;
use std::path::{Path, PathBuf};

use aios_core::{options::DbOption, pdms_types::RefnoEnum};
use aios_database::fast_model::gen_model::{
    generate_xtk_by_dbno, generate_xtk_by_dbno_refno, generate_xtk_by_dbno_refnos,
};
use anyhow::Context;
use clap::Parser;
use config::Config;

#[derive(Parser, Debug)]
#[command(name = "gen_xtk", about = "根据数据库编号生成 XKT 文件")]
struct Args {
    /// 数据库编号
    #[arg(long)]
    dbno: Option<u32>,

    /// 输出文件路径
    #[arg(long)]
    output: Option<String>,

    /// DbOption 配置文件（不带扩展名）
    #[arg(long, default_value = "DbOption")]
    config: String,

    /// 是否压缩输出
    #[arg(long, default_value_t = true)]
    compress: bool,

    /// 指定生成的参考号，可重复或以逗号分隔
    #[arg(long, value_delimiter = ',', num_args = 1.., requires = "dbno")]
    refno: Vec<String>,
}

struct DbOptionOverride {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

impl DbOptionOverride {
    fn new(source: &Path, target: &Path) -> anyhow::Result<Self> {
        let new_content =
            fs::read(source).with_context(|| format!("读取配置文件失败: {}", source.display()))?;
        let original = if target.exists() {
            Some(
                fs::read(target)
                    .with_context(|| format!("读取原有 DbOption 失败: {}", target.display()))?,
            )
        } else {
            None
        };
        fs::write(target, &new_content)
            .with_context(|| format!("写入临时 DbOption 失败: {}", target.display()))?;
        Ok(Self {
            path: target.to_path_buf(),
            original,
        })
    }
}

impl Drop for DbOptionOverride {
    fn drop(&mut self) {
        let _ = match self.original.take() {
            Some(data) => fs::write(&self.path, data),
            None => fs::remove_file(&self.path),
        };
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let settings = Config::builder()
        .add_source(config::File::with_name(&args.config))
        .build()
        .with_context(|| format!("无法加载配置文件: {}.toml", args.config))?;
    let db_option: DbOption = settings.try_deserialize().context("配置文件反序列化失败")?;

    let config_filename = format!("{}.toml", args.config);
    let config_file = Path::new(&config_filename);
    let target_db_option = Path::new("DbOption.toml");
    let _override_guard = DbOptionOverride::new(config_file, target_db_option)?;

    let default_output = format!(
        "output/db{}_{}.xkt",
        args.dbno.unwrap_or(0),
        if args.compress { "compressed" } else { "raw" }
    );
    let output_path = args.output.unwrap_or(default_output);

    if let Some(parent) = Path::new(&output_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
        }
    }

    let dbno = match args.dbno {
        Some(dbno) => dbno,
        None => return Err(anyhow::anyhow!("未指定数据库号 (--dbno)，请提供数据库编号")),
    };

    if args.refno.is_empty() {
        generate_xtk_by_dbno(dbno, &output_path, args.compress, &db_option)
            .await
            .with_context(|| format!("生成数据库 {} 的 XKT 文件失败", dbno))?;
    } else {
        let refnos: Vec<RefnoEnum> = args
            .refno
            .iter()
            .flat_map(|entry| entry.split(',').map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .map(|s| RefnoEnum::from(s.as_str()))
            .collect();

        if refnos.is_empty() {
            return Err(anyhow::anyhow!("参考号列表为空"));
        }

        if refnos.len() == 1 {
            generate_xtk_by_dbno_refno(
                dbno,
                refnos.into_iter().next().unwrap(),
                &output_path,
                args.compress,
                &db_option,
            )
            .await
            .with_context(|| format!("生成数据库 {} 指定参考号的 XKT 文件失败", dbno))?;
        } else {
            generate_xtk_by_dbno_refnos(dbno, refnos, &output_path, args.compress, &db_option)
                .await
                .with_context(|| format!("生成数据库 {} 指定参考号的 XKT 文件失败", dbno))?;
        }
    }

    println!("✅ 数据库 {} 的 XKT 文件已生成: {}", dbno, output_path);
    Ok(())
}
