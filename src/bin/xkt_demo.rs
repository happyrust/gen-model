use aios_database::xkt_generator::examples;
use clap::{Arg, Command};
use tokio;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Command::new("XKT Generator Demo")
        .version("1.0")
        .author("AIOS Database Team")
        .about("演示 XKT 格式生成器的功能")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .value_name("MODE")
                .help("运行模式: test, examples, all")
                .default_value("all")
                .value_parser(["test", "examples", "all"]),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("DIR")
                .help("输出目录")
                .default_value("./output"),
        )
        .get_matches();

    let mode = matches.get_one::<String>("mode").unwrap();
    let _output_dir = matches.get_one::<String>("output").unwrap();

    println!("=== XKT 生成器演示程序 ===");
    println!("模式: {}", mode);
    println!();

    match mode.as_str() {
        "test" => {
            println!("测试模式暂时不可用（需要启用 test 特性）");
            println!("请使用 'examples' 模式运行示例程序");
        }
        "examples" => {
            println!("运行示例程序...");
            examples::run_all_examples().await?;
        }
        "all" => {
            println!("测试模式暂时不可用（需要启用 test 特性）");
            println!("运行示例程序...");
            examples::run_all_examples().await?;
        }
        _ => {
            eprintln!("未知模式: {}", mode);
            std::process::exit(1);
        }
    }

    println!();
    println!("=== 演示完成 ===");
    println!("生成的 XKT 文件可以使用 xeokit 查看器加载和显示。");
    println!("更多信息请访问: https://xeokit.github.io/");

    Ok(())
}
