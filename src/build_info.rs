//! Build identity embedded into every `aios-database` executable.
//!
//! `CARGO_PKG_VERSION` is the release line. `BUILD_ID` identifies one concrete
//! build inside that line and is what binary patch manifests record. Exact
//! patch eligibility is still decided by SHA-256; a human-readable version is
//! never trusted as a substitute for the source hash.

use serde::Serialize;

pub const PRODUCT: &str = "aios-database";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_ID: &str = env!("AIOS_BUILD_ID");
pub const BUILD_EPOCH: &str = env!("AIOS_BUILD_EPOCH");
pub const GIT_COMMIT: &str = env!("AIOS_GIT_COMMIT");
pub const GIT_DIRTY_TEXT: &str = env!("AIOS_GIT_DIRTY");
pub const TARGET: &str = env!("AIOS_BUILD_TARGET");
pub const PROFILE: &str = env!("AIOS_BUILD_PROFILE");
pub const RUSTC: &str = env!("AIOS_RUSTC_VERSION");
/// e3d-model vendor 的 crate 版本（`../vendor/e3d-model/Cargo.toml`，`build.rs` 读出）。
pub const E3D_MODEL_VERSION: &str = env!("E3D_MODEL_VERSION");
/// e3d-model vendor 的构建身份（GitHub #73）：`<版本>@g<提交号>`，工作树脏或没有 git 时补源码内容哈希；
/// vendor 目录找不到时 `unknown`。生成器指纹（`fast_model::generator_identity`）的 `e3d-model:` 槽填的就是它。
pub const E3D_MODEL_BUILD_ID: &str = env!("E3D_MODEL_BUILD_ID");
/// e3d-io vendor 的 crate 版本与构建身份，口径同 [`E3D_MODEL_BUILD_ID`]。e3d-model 经它读库、求目录 PML，
/// 单改它也改几何，所以它是生成器指纹的 `e3d-io:` 段。
pub const E3D_IO_VERSION: &str = env!("E3D_IO_VERSION");
pub const E3D_IO_BUILD_ID: &str = env!("E3D_IO_BUILD_ID");
/// e3d-attlib vendor（属性字典，e3d-model 与 e3d-io 都依赖它）的 crate 版本与构建身份：生成器指纹的 `e3d-attlib:` 段。
pub const E3D_ATTLIB_VERSION: &str = env!("E3D_ATTLIB_VERSION");
pub const E3D_ATTLIB_BUILD_ID: &str = env!("E3D_ATTLIB_BUILD_ID");

/// 日志里点名「本进程按哪一版 vendor 算几何」：`e3d-model <身份>、e3d-io <身份>、e3d-attlib <身份>`。
pub fn vendor_build_ids() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        format!(
            "e3d-model {E3D_MODEL_BUILD_ID}、e3d-io {E3D_IO_BUILD_ID}、e3d-attlib {E3D_ATTLIB_BUILD_ID}"
        )
    })
}

/// Format the embedded build timestamp, never the process startup time.
pub fn build_time() -> &'static str {
    static TIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TIME.get_or_init(|| {
        let epoch = BUILD_EPOCH
            .parse::<i64>()
            .expect("valid embedded build epoch");
        let utc = chrono::DateTime::from_timestamp(epoch, 0).expect("valid build timestamp");
        utc.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
            .format("%Y-%m-%d %H:%M:%S %:z")
            .to_string()
    })
}

pub fn display_version() -> &'static str {
    static VERSION_TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION_TEXT.get_or_init(|| format!("{VERSION} (built {})", build_time()))
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BuildInfo {
    pub product: &'static str,
    pub version: &'static str,
    pub build_id: &'static str,
    pub build_epoch: &'static str,
    pub build_time: &'static str,
    pub git_commit: &'static str,
    pub git_dirty: bool,
    pub target: &'static str,
    pub profile: &'static str,
    pub rustc: &'static str,
    /// e3d-model vendor 的构建身份（[`E3D_MODEL_BUILD_ID`]）：部署后核「换了哪一版 vendor」看这一格和下面两格。
    pub e3d_model_build_id: &'static str,
    pub e3d_io_build_id: &'static str,
    pub e3d_attlib_build_id: &'static str,
}

pub fn current() -> BuildInfo {
    BuildInfo {
        product: PRODUCT,
        version: VERSION,
        build_id: BUILD_ID,
        build_epoch: BUILD_EPOCH,
        build_time: build_time(),
        git_commit: GIT_COMMIT,
        git_dirty: GIT_DIRTY_TEXT == "true",
        target: TARGET,
        profile: PROFILE,
        rustc: RUSTC,
        e3d_model_build_id: E3D_MODEL_BUILD_ID,
        e3d_io_build_id: E3D_IO_BUILD_ID,
        e3d_attlib_build_id: E3D_ATTLIB_BUILD_ID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_names_the_product_and_release() {
        let info = current();
        assert_eq!(info.product, "aios-database");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(info.build_id.starts_with(info.version));
        assert!(!info.git_commit.is_empty());
        assert!(!info.target.is_empty());
        let parsed =
            chrono::DateTime::parse_from_str(info.build_time, "%Y-%m-%d %H:%M:%S %:z").unwrap();
        assert_eq!(parsed.timestamp().to_string(), BUILD_EPOCH);
        assert!(display_version().starts_with(&format!("{VERSION} (built ")));
        assert_eq!(info.e3d_model_build_id, E3D_MODEL_BUILD_ID);
        assert!(!info.e3d_model_build_id.is_empty());
        assert_eq!(info.e3d_io_build_id, E3D_IO_BUILD_ID);
        assert_eq!(info.e3d_attlib_build_id, E3D_ATTLIB_BUILD_ID);
        assert!(vendor_build_ids().contains(E3D_IO_BUILD_ID));
        assert!(vendor_build_ids().contains(E3D_ATTLIB_BUILD_ID));
    }
}
