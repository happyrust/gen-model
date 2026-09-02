//! ADR-017 暂存与写回的应用侧基础设施。
//!
//! - [`executor`]：StagedExecutor——暂存执行 + 语句日志 + TX_CHUNK 分块写回（T0.2）；
//! - [`replay_safe`]：ReplaySafe 语句规范与 validator，进日志的语句在入口处过检（T0.5）；
//! - [`lifecycle`]：staging database 命名 / 建库初始化 / 登记表 / 终态清扫（T0.3）；
//! - [`resources`]：资源三级状态机（告警 / 拒绝吸收 / 废弃暂存）（T0.3）；
//! - [`attempts`]：per-root attempts 控制面与窗口阻断记录（T0.4）；
//! - [`ancestor_preload`]：模型工作项祖先链的解析式预载（2026-08-07 方案 W1）。
//!
//! 词汇见 `CONTEXT.md`「暂存与写回」章节；决策见 `docs/adr/ADR-017`。
//!
//! **状态（ADR-056，2026-09-02）**：稳态增量窗口已不再开暂存库（P1，`batch_worker` /
//! `increment_pipeline::apply_one` 只走直写），[`active_data_db`] 恒返持久层；本目录
//! 除 [`attempts`]（持久层控制面，搬家保留）外在 P3 整体拆除，剩余调用点只有
//! ADR-036 维护纠正 `window_repair` 与几条借 `mem://` 窗口当载体的测试。

pub mod ancestor_preload;
pub mod attempts;
pub mod executor;
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
pub(crate) use write_context::{
    active_staged_finalize_context, active_staged_finalize_plan, settle_staged_plan_items,
};
pub(crate) use write_context::{active_staging_writes, with_staging_writes};

/// 模型数据面的当前读库：持久层 `SUL_DB`。
///
/// ADR-056 P1 之前它按 `aios_core::staging::active_staging_reads()` 分流到暂存库；
/// 暂存窗口不再开之后那条路恒为 `None`，这里直接钉死持久层，模型面的读与写从此
/// 只有一个世界。名字与调用点（`fast_model/*`、`cata_closure`、`helper` 等）P3 搬家
/// 时一起收口（spec 035 T302）。
pub(crate) fn active_data_db() -> surrealdb::Surreal<surrealdb::engine::any::Any> {
    aios_core::SUL_DB.clone()
}

/// 查询可参与几何计算的实例。
///
/// `aios_core::query_insts` 只过滤空 AABB；一条悬空的 `world_trans` 会让整批
/// `GeomInstQuery` 反序列化失败。这里把两个必需字段的口径收齐，缺几何的行由调用方按
/// “不可用/待重试”处理，而不是拖垮同批所有正常实例。
///
/// `owner` 先取 `in.owner`（解析进库的 `pe` 行），取不到就退到本行 `anc[1]`（见
/// [`OWNER_PROJECTION`]）。零解析部署（ADR-054）下 `pe` 没有行，`in.owner` 是 NONE，
/// 没有这一步整批实例会在 `owner: RefnoEnum` 反序列化上失败——模型已经生成好了，
/// 却一条也报不出来。
///
/// `pub`：Python 调试绑定（`aios_db.model.export_obj`）复用同一口径取实例集。
pub async fn query_valid_insts(
    refnos: &[aios_core::RefnoEnum],
) -> anyhow::Result<Vec<aios_core::GeomInstQuery>> {
    query_valid_insts_on(&active_data_db(), refnos).await
}

/// [`query_valid_insts`] 的显式句柄版：同一份投影与过滤口径，测试对一次性
/// `mem://` 实例执行时用。
pub(crate) async fn query_valid_insts_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    refnos: &[aios_core::RefnoEnum],
) -> anyhow::Result<Vec<aios_core::GeomInstQuery>> {
    if refnos.is_empty() {
        return Ok(Vec::new());
    }
    let keys = aios_core::get_inst_relate_keys(refnos);
    let derived_inputs = refnos
        .iter()
        .map(|refno| refno.to_pe_key().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let mut response = db
        .query(format!(
            r#"SELECT
                   in AS refno, in.old_pe AS old_refno, {owner} AS owner,
                   generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
                   out.ptset.d.pt AS pts,
                   IF booled_id != NONE {{ [{{ "geo_hash": booled_id }}] }}
                   ELSE {{ (SELECT trans.d AS transform, record::id(out) AS geo_hash
                            FROM out->geo_relate
                            WHERE visible && out.meshed && trans.d != NONE && geo_type = 'Pos') }}
                   AS insts,
                   booled_id != NONE AS has_neg, dt AS date
               FROM {keys}
               WHERE aabb.d != NONE AND world_trans.d != NONE;
               SELECT
                   in AS refno, in.old_pe AS old_refno, {owner} AS owner,
                   generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
                   out.ptset.d.pt AS pts,
                   IF booled_id != NONE {{ [{{ "geo_hash": booled_id }}] }}
                   ELSE {{ (SELECT trans.d AS transform, record::id(out) AS geo_hash
                            FROM out->geo_relate
                            WHERE visible && out.meshed && trans.d != NONE && geo_type = 'Pos') }}
                   AS insts,
                   booled_id != NONE AS has_neg, dt AS date
               FROM inst_relate
               WHERE in IN [{derived_inputs}] AND id NOT IN [{keys}]
                 AND aabb.d != NONE AND world_trans.d != NONE"#,
            owner = OWNER_PROJECTION,
        ))
        .await?
        .check()?;
    let mut rows: Vec<aios_core::GeomInstQuery> = response.take(0)?;
    rows.extend(response.take::<Vec<aios_core::GeomInstQuery>>(1)?);
    Ok(rows)
}

/// 实例行 `owner` 的投影：`in.owner` 取不到时，用本行 `anc[1]` 还原成 `pe` 记录链接。
///
/// `anc` 是「自身 → 顶层」的打包祖先链（`fn::anc_u64` 与 `e3d_model_service::ancestor_chain`
/// 同口径：`[0]` 自身、`[1]` 属主），生成期就写进了产物行，不依赖解析进库的 `pe`。
/// 还原成记录链接而不是直接给整数：`RefnoEnum` 的反序列化只认 Thing / 字符串 / u64，
/// SurrealDB 的整数是 i64，裸给会照样报「RefnoEnum parse error」。
/// 打包规则 `word0 << 32 | word1`，除法先 floor 再转 int，整数除法在 SurrealQL 里出浮点。
const OWNER_PROJECTION: &str = "(in.owner ?? type::thing('pe', string::concat(\
    <string><int>math::floor(anc[1] / 4294967296), '_', <string>(anc[1] % 4294967296))))";

/// `query_valid_insts` 的口径验收（对一次性 `mem://` 实例执行）。ADR-056 P1 之前
/// 这里还有一条读路由 seam 的验收（`staging_context_routes_reads_and_never_touches_sul_db`）：
/// 暂存窗口退役、`active_data_db` 钉死持久层之后它没有对象了，随之删除。
#[cfg(test)]
mod routing_tests {
    use aios_core::RefnoEnum;
    use surrealdb::engine::any::connect;

    /// ADR-056 P1：模型面的读只有持久层一个世界——`active_data_db` 不得再按任何
    /// 暂存上下文分流。
    #[test]
    fn active_data_db_is_pinned_to_the_persistent_layer() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("pub(crate) fn active_data_db()")
            .expect("active_data_db must exist")
            .1
            .split_once("\n}")
            .expect("function body must close")
            .0;
        assert!(
            body.contains("aios_core::SUL_DB.clone()") && !body.contains("active_staging_reads"),
            "active_data_db 必须直接返回 SUL_DB：{body}"
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
                 UPSERT inst_relate:derived_tube SET in = pe:⟨4000000001_1⟩, \
                     out = inst_info:valid, aabb = aabb:valid, world_trans = trans:identity, \
                     generic = 'TUBI', booled_id = 'tube_mesh';\
                 RELATE pe:⟨4000000001_2⟩->inst_relate:⟨4000000001_2⟩->inst_info:dangling \
                     SET aabb = aabb:valid, world_trans = trans:missing, generic = 'PANE';",
            )
            .await
            .expect("plant inst rows")
            .check()
            .expect("planted");

        let good: RefnoEnum = "4000000001_1".into();
        let dangling: RefnoEnum = "4000000001_2".into();
        let rows = super::query_valid_insts_on(&staging, &[good, dangling])
            .await
            .expect("dangling transform must not poison the batch");
        assert_eq!(
            rows.len(),
            2,
            "direct instance and derived TUBI must both load"
        );
        assert!(rows.iter().all(|row| row.refno == good));
        assert!(rows.iter().any(|row| row.generic == "TUBI"));
    }

    /// ADR-054 零解析部署：`pe` 一行都没有，产物行的 `in` 指向一个不存在的记录，
    /// `in.owner` 是 NONE。`owner` 必须退到本行 `anc[1]`（打包祖先链，`[0]` 是自身），
    /// 否则整批实例在 `owner: RefnoEnum` 反序列化上失败——2026-09-02 实测就是这一句
    /// 「Serialization error: RefnoEnum parse error」把已生成好的模型拦在 ensure 回执之外。
    #[tokio::test(flavor = "multi_thread")]
    async fn valid_inst_query_takes_the_owner_from_anc_when_pe_has_no_rows() {
        let staging = connect("mem://").await.expect("mem boots");
        staging
            .use_ns("staging")
            .use_db("zero_parse_insts")
            .await
            .expect("use staging db");
        // 24384/24829 归 24384/24828；打包值 = word0 << 32 | word1。
        let self_packed: i64 = ((24384u64 << 32) | 24829u64) as i64;
        let owner_packed: i64 = ((24384u64 << 32) | 24828u64) as i64;
        staging
            .query(format!(
                "CREATE aabb:valid SET d = {{ mins: [0, 0, 0], maxs: [1, 1, 1] }};\
                 CREATE trans:identity SET d = {{ translation: [0, 0, 0], \
                     rotation: [0, 0, 0, 1], scale: [1, 1, 1] }};\
                 CREATE inst_info:valid;\
                 UPSERT inst_relate:⟨24384_24829⟩ SET in = pe:⟨24384_24829⟩, out = inst_info:valid, \
                     aabb = aabb:valid, world_trans = trans:identity, generic = 'BOX', \
                     anc = [{self_packed}, {owner_packed}];"
            ))
            .await
            .expect("plant a product row without any pe row")
            .check()
            .expect("planted");

        let refno: RefnoEnum = "24384_24829".into();
        let rows = super::query_valid_insts_on(&staging, &[refno])
            .await
            .expect("a missing pe row must not poison the batch");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].refno, refno);
        assert_eq!(
            rows[0].owner,
            RefnoEnum::from("24384_24828"),
            "owner 退到 anc[1]"
        );
    }
}
