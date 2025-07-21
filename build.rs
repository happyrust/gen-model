use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 只在启用grpc feature时编译proto文件
    // if cfg!(feature = "grpc") {
    //     let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        
    //     tonic_build::configure()
    //         .build_server(true)
    //         .build_client(true)
    //         .out_dir(&out_dir)
    //         .compile(&["proto/progress_service.proto"], &["proto"])?;
            
    //     println!("cargo:rerun-if-changed=proto/progress_service.proto");
    // }

    // // 现有的cc编译逻辑
    // cc::Build::new()
    //     .flag_if_supported("-std=c++17")
    //     .flag_if_supported("-w") // 忽略警告
    //     .compile("empty"); // 空的编译目标，保持现有结构

    Ok(())
}