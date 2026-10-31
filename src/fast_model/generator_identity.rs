//! 生成器身份（GitHub #73）。
//!
//! 一份几何是谁算出来的 = e3d-model 的构建身份（vendor crate 版本 + 提交号 / 源码哈希，`build.rs` 注入
//! `E3D_MODEL_BUILD_ID`，见 [`crate::build_info::E3D_MODEL_BUILD_ID`]）+ 本仓版本 + 一串手写的画法规则标签
//! （某条画法改了口径就追加一个 `-vN`）。它进两处：
//!
//! - 逐根回执的 `ModelTarget.generator_fingerprint`：批内复用（`cached_root_target_matches`）、刚体前移、
//!   单元级发布（`same_target_outside_the_design_window`）都比它，变了就整根重算；
//! - 水位行的库级落款 `dbnum_watermark.model_generator`（任务收尾随 `model_sesno` 同盖）：启动 / `update/execute`
//!   比它，不一致就整库排 `ModelOnly` 起点 0（`data_interface::generator_reconcile`）——库级水位追平的库一根都不
//!   走批次，逐根指纹再准也拦不住它。
//!
//! 2026-10-27 之前指纹的 `e3d-model:` 槽填的是本仓自己的 `CARGO_PKG_VERSION`：只换 vendor 不动本仓版本号时
//! 指纹一字不变，e3d-model 的几何修复对「源没变」的根一项都落不下去。换成这一版后旧回执的指纹全部对不上：
//! 每根下一次进批次整根重算一次，库级核对把每个库排一次整库重排——这是换 vendor 本该付的那一次。

use std::sync::OnceLock;

/// 手写的画法规则标签：某条画法改了口径就在末尾追加 / 升版一个标签，旧回执随之作废。
pub const RULE_TAGS: &str = "route-atta-pass-through-v1;route-atta-anci-only-v1;catalogue-atta-paxi-frame-v2;catalogue-route-scyl-paxi-frame-v1;catalogue-pml-sqrt-atan-v1;catalogue-pml-flex-mask-v1;catalogue-pml-uda-key-v1;catalogue-lpyr-negative-pbax-v1;section-fitt-posl-frame-v1;catalogue-cylinder-cuts-v1;design-cyli-centered-z-v1;design-snout-revolution-facets-v1;design-negative-ctor-grid-v2;polyhedron-member-order-v1;pohe-polygon-panels-v1;cable-tray-missing-rpath-origin-v1;section-finite-empty-length-v1;explicit-word-angle-units-v1";

/// 本进程的生成器指纹：`e3d-model:<vendor 构建身份>;aios-database:<本仓版本>;<规则标签>`。
/// 编译期常量，整个进程只有这一份；逐根回执和库级落款写的都是它。
pub fn generator_fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        format!(
            "e3d-model:{};aios-database:{};{RULE_TAGS}",
            crate::build_info::E3D_MODEL_BUILD_ID,
            crate::build_info::VERSION
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指纹的三段：vendor 构建身份、本仓版本、规则标签。vendor 身份不许是 `unknown`——没有 `../vendor/e3d-model`
    /// 的树根本编不过（path 依赖），这里红说明 `build.rs` 找错了目录，那样的构建分不出 vendor 换没换。
    #[test]
    fn the_fingerprint_carries_the_vendor_build_identity_and_the_rule_tags() {
        let fingerprint = generator_fingerprint();
        let head = format!(
            "e3d-model:{};aios-database:{};",
            crate::build_info::E3D_MODEL_BUILD_ID,
            crate::build_info::VERSION
        );
        assert!(fingerprint.starts_with(&head), "{fingerprint}");
        assert!(fingerprint.ends_with(RULE_TAGS), "{fingerprint}");
        assert_ne!(
            crate::build_info::E3D_MODEL_BUILD_ID,
            "unknown",
            "build.rs 没找到 e3d-model vendor 目录（../vendor/e3d-model 或 E3D_MODEL_VENDOR_DIR）"
        );
        assert!(
            crate::build_info::E3D_MODEL_BUILD_ID
                .starts_with(&format!("{}@", crate::build_info::E3D_MODEL_VERSION)),
            "{}",
            crate::build_info::E3D_MODEL_BUILD_ID
        );
        assert!(!RULE_TAGS.contains(' '), "标签串不许带空格，它要原样进 SurrealQL 字符串");
    }
}
