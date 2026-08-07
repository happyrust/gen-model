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
#[cfg(test)]
mod issue10_add_node;
pub mod lifecycle;
#[cfg(test)]
pub mod parity;
pub mod preload;
pub mod replay_safe;
pub mod resources;
pub mod write_context;

pub use executor::{ExecMode, JournalEntry, TX_CHUNK};
pub use lifecycle::{ActiveStagedWindow, StagingWindowMeta};
pub use resources::{ResourceBand, ResourceGauge, ResourceThresholds};
pub(crate) use write_context::defer_staged_mysql_changes;
pub(crate) use write_context::defer_staged_regen_settlement;
pub(crate) use write_context::hold_staged_generation_root;
pub(crate) use write_context::staged_spatial_removals;
pub(crate) use write_context::{StagedFinalize, register_staged_finalize};
pub(crate) use write_context::{active_staged_finalize_plan, settle_staged_plan_items};
pub(crate) use write_context::{active_staging_writes, with_staging_writes};

/// 模型数据面的当前读库。窗口上下文在场时只读暂存，否则读持久层。
pub(crate) fn active_data_db() -> surrealdb::Surreal<surrealdb::engine::any::Any> {
    aios_core::staging::active_staging_reads()
        .map(|context| context.db().clone())
        .unwrap_or_else(|| aios_core::SUL_DB.clone())
}

/// 查询可参与几何计算的实例。
///
/// `aios_core::query_insts` 只过滤空 AABB；一条悬空的 `world_trans` 会让整批
/// `GeomInstQuery` 反序列化失败。这里把两个必需字段的口径收齐，缺几何的行由调用方按
/// “不可用/待重试”处理，而不是拖垮同批所有正常实例。
pub(crate) async fn query_valid_insts(
    refnos: &[aios_core::RefnoEnum],
) -> anyhow::Result<Vec<aios_core::GeomInstQuery>> {
    if refnos.is_empty() {
        return Ok(Vec::new());
    }
    let keys = aios_core::get_inst_relate_keys(refnos);
    let mut response = active_data_db()
        .query(format!(
            r#"SELECT
                   in AS refno, in.old_pe AS old_refno, in.owner AS owner,
                   generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
                   out.ptset.d.pt AS pts,
                   IF booled_id != NONE {{ [{{ "geo_hash": booled_id }}] }}
                   ELSE {{ (SELECT trans.d AS transform, record::id(out) AS geo_hash
                            FROM out->geo_relate
                            WHERE visible && out.meshed && trans.d != NONE && geo_type = 'Pos') }}
                   AS insts,
                   booled_id != NONE AS has_neg, dt AS date
               FROM {keys}
               WHERE aabb.d != NONE AND world_trans.d != NONE"#
        ))
        .await?
        .check()?;
    Ok(response.take(0)?)
}

/// 读路由 seam 的验收（T0.2）：上下文在场 → 被接线的读入口只看暂存库；
/// 上下文缺席 → 行为与历史一致（直连 `SUL_DB`）；两个世界互不污染进程缓存。
#[cfg(test)]
mod routing_tests {
    use aios_core::RefnoEnum;
    use aios_core::staging::{StagingReadContext, with_staging_reads};
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
                "UPSERT SITE:⟨4000000001_1⟩ CONTENT {{ TYPE: 'SITE', NAME: 'root' }};\
                 UPSERT pe:⟨4000000001_1⟩ CONTENT {{ noun: 'SITE', refno: SITE:⟨4000000001_1⟩ }};\
                 UPSERT PIPE:⟨4000000001_10⟩ CONTENT {{ TYPE: 'PIPE', NAME: 'p1' }};\
                 UPSERT {pe_key} CONTENT {{ noun: 'PIPE', refno: PIPE:⟨4000000001_10⟩, owner: pe:⟨4000000001_1⟩ }};\
                 INSERT RELATION INTO pe_owner [{{ id: pe_owner:[{pe_key}, 0], in: {pe_key}, out: pe:⟨4000000001_1⟩ }}];"
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
        let noun = with_staging_reads(ctx.clone(), aios_core::get_type_name(refno))
            .await
            .expect("type name routed");
        assert_eq!(noun, "PIPE");
        let implicit = with_staging_reads(
            ctx.clone(),
            aios_core::get_implicit_named_attmap(refno, "PIPE"),
        )
        .await
        .expect("implicit row routed");
        assert_eq!(implicit.get_type_str(), "PIPE");

        let root: RefnoEnum = "4000000001_1".into();
        let children = with_staging_reads(ctx.clone(), aios_core::get_children_refnos(root))
            .await
            .expect("children routed");
        assert_eq!(children, vec![refno]);
        let types = with_staging_reads(ctx.clone(), aios_core::get_self_and_owner_type_name(refno))
            .await
            .expect("owner types routed");
        assert_eq!(types, vec!["PIPE", "SITE"]);
        let deep = with_staging_reads(ctx.clone(), aios_core::query_deep_children_refnos(root))
            .await
            .expect("deep hierarchy routed");
        assert!(deep.contains(&refno), "{deep:?}");

        let child = with_staging_reads(ctx.clone(), async move {
            aios_core::staging::spawn_with_staging_reads(aios_core::get_type_name(refno))
                .await
                .expect("child join")
        })
        .await
        .expect("child keeps staged context");
        assert_eq!(child, "PIPE");

        // 暂存 miss 是合法结果（预载缺失由上层兜底），绝不回落持久层。
        let missing: RefnoEnum = "4000000001_99".into();
        let miss = with_staging_reads(ctx.clone(), aios_core::get_pe(missing))
            .await
            .expect("staged miss 应是 Ok(None) 而不是回落持久层的结果");
        assert!(miss.is_none());
        let typed_miss = with_staging_reads(ctx.clone(), aios_core::get_type_name(missing)).await;
        assert!(
            typed_miss
                .expect_err("type-name miss must fail closed")
                .to_string()
                .contains("staging read miss")
        );

        // 上下文缺席：同一入口回到历史行为（直打 SUL_DB——本测试未连接，必错）。
        // 若这里 Ok，说明暂存世界的值泄漏进了进程缓存或路由错了世界。
        let outside = aios_core::get_named_attmap(refno).await;
        assert!(
            outside.is_err(),
            "上下文缺席时不得看见暂存数据（SUL_DB 未连接应报错）：{outside:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn valid_inst_query_skips_dangling_world_transform() {
        let staging = connect("mem://").await.expect("mem boots");
        staging
            .use_ns("staging")
            .use_db("valid_insts")
            .await
            .expect("use staging db");
        staging
            .query(
                "CREATE pe:⟨4000000001_1⟩ SET owner = pe:⟨4000000001_1⟩;\
                 CREATE pe:⟨4000000001_2⟩ SET owner = pe:⟨4000000001_1⟩;\
                 CREATE aabb:valid SET d = { mins: [0, 0, 0], maxs: [1, 1, 1] };\
                 CREATE trans:identity SET d = { translation: [0, 0, 0], \
                     rotation: [0, 0, 0, 1], scale: [1, 1, 1] };\
                 CREATE inst_info:valid; CREATE inst_info:dangling;\
                 RELATE pe:⟨4000000001_1⟩->inst_relate:⟨4000000001_1⟩->inst_info:valid \
                     SET aabb = aabb:valid, world_trans = trans:identity, generic = 'PANE';\
                 RELATE pe:⟨4000000001_2⟩->inst_relate:⟨4000000001_2⟩->inst_info:dangling \
                     SET aabb = aabb:valid, world_trans = trans:missing, generic = 'PANE';",
            )
            .await
            .expect("plant inst rows")
            .check()
            .expect("planted");

        let good: RefnoEnum = "4000000001_1".into();
        let dangling: RefnoEnum = "4000000001_2".into();
        let ctx = StagingReadContext::new(staging, "valid_insts");
        let rows = with_staging_reads(ctx, super::query_valid_insts(&[good, dangling]))
            .await
            .expect("dangling transform must not poison the batch");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].refno, good);
    }
}
