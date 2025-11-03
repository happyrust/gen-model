fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 只在启用grpc feature时编译proto文件
    #[cfg(feature = "grpc")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .out_dir(&out_dir)
            .file_descriptor_set_path(out_dir.join("progress_service.bin"))
            .compile_protos(&["proto/progress_service.proto"], &["proto"])?;

        // 编译空间查询服务 proto
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .out_dir(&out_dir)
            .file_descriptor_set_path(out_dir.join("spatial_query_service.bin"))
            .compile_protos(&["proto/spatial_query_service.proto"], &["proto"])?;

        println!("cargo:rerun-if-changed=proto/progress_service.proto");
        println!("cargo:rerun-if-changed=proto/spatial_query_service.proto");
    }

    Ok(())
}
