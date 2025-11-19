use pdms_io::sync::clone::{CloneOptions, execute_clone};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 测试本地文件克隆
    let source_file = "/Volumes/DPC/work/e3d_models/test_sjz/AvevaMarineSample/ams000/test1112.cba";
    let target_file = "/Volumes/DPC/work/e3d_models/test_sjz/AvevaMarineSample/ams000/test1112_local";
    
    println!("Testing local clone from {} to {}", source_file, target_file);
    
    let clone_opt = CloneOptions::new_local(source_file, target_file);
    
    match execute_clone(clone_opt).await {
        Ok(success) => {
            if success {
                println!("Clone successful!");
            } else {
                println!("Clone returned false");
            }
        }
        Err(e) => {
            println!("Clone failed: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}
