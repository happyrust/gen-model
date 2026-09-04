//! 库文件分发：`.cba` 归档压缩（[`compress`]）与远端增量 clone（[`clone`]）。
//!
//! 出处：`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §G（T415）。
//!
//! 这两个功能与 dabacon 解析、与增量都没有关系——它们是文件分发（`SyncPublisher` 压 `.cba`
//! 发 MQTT，`exec_delta_clone_remotes` 从远端按块 clone），只是历史上挂在 old-pdms-io 这个
//! crate 上。e3d-io 是「只读 IO 库」，不该也不会提供它们；old-pdms-io 退出默认依赖图后，
//! 它们搬到这里，实现自 `old-pdms-io/src/sync/{clone,compress}.rs` 原样搬入，底层仍是 `dpcsync`。

#[macro_use]
mod utils;

pub mod clone;
pub mod compress;

pub use clone::{CloneOptions, execute_clone};
pub use compress::{CompressOptions, execute_compress};
