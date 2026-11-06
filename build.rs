use std::env;
use std::fs;
use std::path::{Path, PathBuf};
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

/// `[package] version = "…"`，只认 `[package]` 节里的那一行（依赖节里也有 `version =`）。
fn package_version(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

/// vendor 源码树的内容哈希（Cargo.toml + `src/**/*.rs`，按相对路径排序）：没有 git、或工作树脏时的身份。
/// 只求「内容变了哈希就变」，不求密码学强度。
fn source_tree_hash(src_dir: &Path, manifest: &str) -> u64 {
    let mut hash = FNV_OFFSET;
    fnv1a(&mut hash, manifest.as_bytes());
    let mut files = Vec::new();
    rust_sources(src_dir, &mut files);
    files.sort();
    for file in files {
        let relative = file.strip_prefix(src_dir).unwrap_or(&file);
        fnv1a(
            &mut hash,
            relative.to_string_lossy().replace('\\', "/").as_bytes(),
        );
        if let Ok(bytes) = fs::read(&file) {
            fnv1a(&mut hash, &bytes);
        }
    }
    hash
}

/// 一个 vendor 仓（`Cargo.toml` 里的 path 依赖，默认 `../vendor/<name>`，可用 `env_var` 指到别处）的构建身份
/// （GitHub #73）：`<crate 版本>@g<提交号>`，工作树脏或没有 git 时补源码内容哈希。e3d-model 与它读库、读目录 PML
/// 所经的 e3d-io、e3d-attlib 三家的身份都进模型回执的 `generator_fingerprint` 与水位行的库级落款——只换 vendor
/// 不动本仓版本号时指纹也要变，否则换了几何修复旧行一根都不重算。找不到 vendor 目录时两样都是 `unknown`：那样的
/// 构建分不出 vendor 换没换。
fn vendor_identity(name: &str, env_var: &str) -> (String, String) {
    println!("cargo:rerun-if-env-changed={env_var}");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let vendor_dir = env::var(env_var)
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("..").join("vendor").join(name));
    let cargo_toml = vendor_dir.join("Cargo.toml");
    let Ok(manifest) = fs::read_to_string(&cargo_toml) else {
        println!(
            "cargo:warning={name} vendor 目录找不到（{}），生成器指纹里的 {name} 身份记为 unknown",
            vendor_dir.display()
        );
        return ("unknown".into(), "unknown".into());
    };
    println!("cargo:rerun-if-changed={}", cargo_toml.display());
    let src_dir = vendor_dir.join("src");
    if src_dir.is_dir() {
        println!("cargo:rerun-if-changed={}", src_dir.display());
    }
    for marker in [".git/HEAD", ".git/index"] {
        let path = vendor_dir.join(marker);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let version = package_version(&manifest).unwrap_or_else(|| "unknown".into());
    let vendor = vendor_dir.to_string_lossy().into_owned();
    let commit = command_stdout("git", &["-C", &vendor, "rev-parse", "--short=12", "HEAD"]);
    let dirty = command_stdout(
        "git",
        &["-C", &vendor, "status", "--porcelain", "--untracked-files=no"],
    )
    .is_some_and(|status| !status.is_empty());
    let identity = match commit {
        Some(commit) if !dirty => format!("g{commit}"),
        Some(commit) => format!(
            "g{commit}.dirty-src{:016x}",
            source_tree_hash(&src_dir, &manifest)
        ),
        None => format!("src{:016x}", source_tree_hash(&src_dir, &manifest)),
    };
    let build_id = format!("{version}@{identity}");
    (version, build_id)
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
    let (e3d_model_version, e3d_model_build_id) =
        vendor_identity("e3d-model", "E3D_MODEL_VENDOR_DIR");
    let (e3d_io_version, e3d_io_build_id) = vendor_identity("e3d-io", "E3D_IO_VENDOR_DIR");
    let (e3d_attlib_version, e3d_attlib_build_id) =
        vendor_identity("e3d-attlib", "E3D_ATTLIB_VENDOR_DIR");

    println!("cargo:rustc-env=AIOS_BUILD_ID={build_id}");
    println!("cargo:rustc-env=AIOS_BUILD_EPOCH={build_epoch}");
    println!("cargo:rustc-env=AIOS_GIT_COMMIT={git_commit}");
    println!("cargo:rustc-env=AIOS_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=AIOS_BUILD_TARGET={target}");
    println!("cargo:rustc-env=AIOS_BUILD_PROFILE={profile}");
    println!("cargo:rustc-env=AIOS_RUSTC_VERSION={rustc}");
    println!("cargo:rustc-env=E3D_MODEL_VERSION={e3d_model_version}");
    println!("cargo:rustc-env=E3D_MODEL_BUILD_ID={e3d_model_build_id}");
    println!("cargo:rustc-env=E3D_IO_VERSION={e3d_io_version}");
    println!("cargo:rustc-env=E3D_IO_BUILD_ID={e3d_io_build_id}");
    println!("cargo:rustc-env=E3D_ATTLIB_VERSION={e3d_attlib_version}");
    println!("cargo:rustc-env=E3D_ATTLIB_BUILD_ID={e3d_attlib_build_id}");

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
                &format!(
                    "git={git_commit}; dirty={git_dirty}; build_epoch={build_epoch}; e3d-model={e3d_model_build_id}; e3d-io={e3d_io_build_id}; e3d-attlib={e3d_attlib_build_id}"
                ),
            );
        resource
            .compile()
            .expect("compile Windows VERSIONINFO resource");
    }
}
