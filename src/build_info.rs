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

#[derive(Debug, Clone, Copy, Serialize)]
pub struct BuildInfo {
    pub product: &'static str,
    pub version: &'static str,
    pub build_id: &'static str,
    pub build_epoch: &'static str,
    pub git_commit: &'static str,
    pub git_dirty: bool,
    pub target: &'static str,
    pub profile: &'static str,
    pub rustc: &'static str,
}

pub fn current() -> BuildInfo {
    BuildInfo {
        product: PRODUCT,
        version: VERSION,
        build_id: BUILD_ID,
        build_epoch: BUILD_EPOCH,
        git_commit: GIT_COMMIT,
        git_dirty: GIT_DIRTY_TEXT == "true",
        target: TARGET,
        profile: PROFILE,
        rustc: RUSTC,
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
    }
}
