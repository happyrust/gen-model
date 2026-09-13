use std::env;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-env-changed=AIOS_BUILD_ID");
    println!("cargo:rerun-if-env-changed=AIOS_BUILD_EPOCH");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let version = env::var("CARGO_PKG_VERSION").expect("Cargo sets CARGO_PKG_VERSION");
    let git_commit = command_stdout("git", &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".into());
    let git_dirty = command_stdout("git", &["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|status| !status.is_empty());
    let build_epoch = env::var("AIOS_BUILD_EPOCH")
        .or_else(|_| env::var("SOURCE_DATE_EPOCH"))
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_secs()
        });
    let build_id = env::var("AIOS_BUILD_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "{version}+g{git_commit}.{build_epoch}{}",
                if git_dirty { ".dirty" } else { "" }
            )
        });
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let rustc = command_stdout("rustc", &["--version"]).unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=AIOS_BUILD_ID={build_id}");
    println!("cargo:rustc-env=AIOS_BUILD_EPOCH={build_epoch}");
    println!("cargo:rustc-env=AIOS_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=AIOS_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=AIOS_BUILD_TARGET={target}");
    println!("cargo:rustc-env=AIOS_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=AIOS_RUSTC_VERSION={rustc}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set("CompanyName", "AIOS")
            .set("ProductName", "AIOS Database")
            .set(
                "FileDescription",
                "AVEVA E3D incremental parsing and geometry service",
            )
            .set("InternalName", "aios-database")
            .set("OriginalFilename", "aios-database.exe")
            .set("FileVersion", &version)
            .set("ProductVersion", &build_id)
            .set(
                "Comments",
                &format!("git={git_commit}; dirty={git_dirty}; build_epoch={build_epoch}"),
            );
        resource
            .compile()
            .expect("compile Windows VERSIONINFO resource");
    }
}
