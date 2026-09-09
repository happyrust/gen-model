//! TEAM_DATA 镜像——**已退役**（2026-09-07，随 `feature = "sql"`）。
//!
//! 这个模块只做过一件事：把 Surreal 里的 `DB` 元素连同它们所属的 `TEAM` 走一遍，拼成
//! `SysDBData`，写进 MySQL 的 `TEAM_DATA` 表。写出口在 `#[cfg(feature = "sql")]` 门后，
//! 而那个 feature 2026-09-07 整条退役（它早就编不过：`AiosDBMgr::get_project_pool` 不在
//! `aios_core` 里了，仓里也没有一处开过它）。
//!
//! 门一摘，剩下的就是「把整棵 DB / TEAM 树查一遍，然后把结果丢掉」——每个 SYS 窗口的
//! `SideEffectKind::SystDerived` 作业都会付这一趟。所以这里按 `CataRefRev` 的同一口径收口：
//! **队列变体与入队点原样留着**（旧库里已经排进表的行还要认得），drain 到它直接返回成功，
//! 出一行声。真要 TEAM_DATA，它的家应该是 Surreal 而不是第二个持久层（宪法 I）——
//! 那时给它自己的写入路径，别再把这条 MySQL 镜像接回来。

use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;

/// 收口即返回。入参留着是为了不动 `SideEffectCompensator` 那边的调用形状。
pub async fn sync_team_data(_mgr: &AiosDBMgr) -> anyhow::Result<()> {
    println!(
        "sync_team_data: TEAM_DATA 镜像已随 feature = \"sql\" 退役（2026-09-07），本作业直接收口"
    );
    Ok(())
}
