//! ADR-017 暂存与写回的应用侧基础设施。
//!
//! - [`executor`]：StagedExecutor——暂存执行 + 语句日志 + TX_CHUNK 分块写回（T0.2）；
//! - [`replay_safe`]：ReplaySafe 语句规范与 validator，进日志的语句在入口处过检（T0.5）；
//! - [`lifecycle`]：staging database 命名 / 建库初始化 / 登记表 / 终态清扫（T0.3）；
//! - [`resources`]：资源三级状态机（告警 / 拒绝吸收 / 废弃暂存）（T0.3）；
//! - [`attempts`]：per-root attempts 控制面与窗口阻断记录（T0.4）。
//!
//! 词汇见 `CONTEXT.md`「暂存与写回」章节；决策见 `docs/adr/ADR-017`。

pub mod attempts;
pub mod executor;
pub mod lifecycle;
#[cfg(test)]
pub mod parity;
pub mod replay_safe;
pub mod resources;

pub use executor::{ExecMode, JournalEntry, StagedExecutor, TX_CHUNK};
pub use lifecycle::{StagingWindow, StagingWindowMeta};
pub use resources::{ResourceBand, ResourceGauge, ResourceThresholds};

/// 读路由 seam 的验收（T0.2）：上下文在场 → 被接线的读入口只看暂存库；
/// 上下文缺席 → 行为与历史一致（直连 `SUL_DB`）；两个世界互不污染进程缓存。
#[cfg(test)]
mod routing_tests {
    use aios_core::staging::{with_staging_reads, StagingReadContext};
    use aios_core::RefnoEnum;
    use surrealdb::engine::any::connect;

    /// 刻意不连接 `SUL_DB`：上下文缺席时被路由的读打到未初始化的全局句柄上
    /// 必然报错——这同时是「暂存读绝不静默回落持久层」的负向对照。
    #[tokio::test(flavor = "multi_thread")]
    async fn staging_context_routes_reads_and_never_touches_sul_db() {
        let staging = connect("mem://").await.expect("mem boots");
        staging
            .use_ns("staging")
            .use_db("staging_7997_9")
            .await
            .expect("use staging db");

        let refno: RefnoEnum = "4000000001_10".into();
        let pe_key = refno.to_pe_key();
        staging
            .query(format!(
                "UPSERT PIPE:⟨4000000001_10⟩ CONTENT {{ TYPE: 'PIPE', NAME: 'p1' }};\
                 UPSERT {pe_key} CONTENT {{ noun: 'PIPE', refno: PIPE:⟨4000000001_10⟩ }};"
            ))
            .await
            .expect("plant staged rows")
            .check()
            .expect("planted");

        let ctx = StagingReadContext::new(staging.clone(), "staging_7997_9");

        // 上下文在场：读到的是暂存世界。
        let attmap = with_staging_reads(ctx.clone(), aios_core::get_named_attmap(refno))
            .await
            .expect("staged read");
        assert_eq!(attmap.get_type_str(), "PIPE", "应读到暂存里种下的属性行");

        // 暂存 miss 是合法结果（预载缺失由上层兜底），绝不回落持久层。
        let missing: RefnoEnum = "4000000001_99".into();
        let miss = with_staging_reads(ctx.clone(), aios_core::get_pe(missing))
            .await
            .expect("staged miss 应是 Ok(None) 而不是回落持久层的结果");
        assert!(miss.is_none());

        // 上下文缺席：同一入口回到历史行为（直打 SUL_DB——本测试未连接，必错）。
        // 若这里 Ok，说明暂存世界的值泄漏进了进程缓存或路由错了世界。
        let outside = aios_core::get_named_attmap(refno).await;
        assert!(
            outside.is_err(),
            "上下文缺席时不得看见暂存数据（SUL_DB 未连接应报错）：{outside:?}"
        );
    }
}
