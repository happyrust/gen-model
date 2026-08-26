use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use aios_core::SUL_DB;
use aios_core::geometry::EleInstGeo;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_types::*;
use aios_core::prim_geo::LCylinder;
use aios_core::types::*;
use bevy_transform::prelude::Transform;
use itertools::Itertools;

use crate::data_interface::helper::delete_inst_relate_cascade;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::shape_save::{
    DIRECT_MAX_IN_FLIGHT, FlushReason, FrozenShapeBatch, SQL_PACKET_BYTES, SQL_PACKET_ROWS,
    SaveConflict, SaveMode, SaveOutcome, SavePhase, SavePlan, SqlPacket,
};
// use crate::fast_model::EXIST_MESH_GEOS;

/// 渲染一批 `inst_relate` 行的**替换写入**：同一事务里先删同 id 的旧行，再整批插入。
///
/// 只发 `INSERT RELATION` 是不够的：本仓的 SurrealDB fork 撞已有 id 时**不报错、保留
/// 旧行**（ADR-010 D13 在 8009 上实测）。于是「这一行到底写没写进去」完全取决于前面
/// 那个级联删除集有没有覆盖到它，而那个集合是从**本次生成的产物**推出来的，天然漏掉
/// 「上一版生成过、这一版不再产出几何」的元素，以及挂在 BRAN 名下的隐含直管段。漏一项
/// 就是那一行的 `aabb` / `world_trans` 永远停在第一次生成的值，而整条链路一声不响。
///
/// 删除集改从**本批要写的 id** 推出来之后，这个依赖就断了：要写哪行就先删哪行，与外面
/// 那个集合覆不覆盖得到无关。包进一个事务，是为了不给读者留下「这行刚被删、还没写回来」
/// 的窗口。
///
/// 为什么不用 `UPSERT`：本仓对边表一律 `RELATE` / `INSERT RELATION` 写、`DELETE` 删，
/// `UPSERT` 只用在普通表上（`pe` / `inst_info` / `aabb`），fork 对边表的 `UPSERT` 语义
/// 没有实证。真做成 `UPSERT ... MERGE` 还多一个好处——`aabb` 这类本函数不写的字段会留存
/// 下来，房间变更判定就能回到行内基线而不必抵押在空间树上；那要等有人在 fork 上验证过。
pub(crate) fn render_inst_relate_replace(rows: &[(String, String)]) -> String {
    let ids = rows.iter().map(|(id, _)| id.as_str()).join(", ");
    let values = rows.iter().map(|(_, json)| json.as_str()).join(",");
    format!(
        "BEGIN TRANSACTION;\n\
         DELETE {ids};\n\
         INSERT RELATION INTO inst_relate [{values}];\n\
         COMMIT TRANSACTION;"
    )
}

/// 用当前解析结果原子替换一个共享 CATA 身份的全部 `geo_relate`。
///
/// `inst_info` 会被同一 catalogue identity 的多个设计实例共享。定向重生成其中一个
/// 实例时，级联删除不能移除仍被其他实例引用的 `inst_info`，也就不会清掉旧版几何代码
/// 写入的关系。若随后只做 `INSERT RELATION`，旧、新关系会同时可见。7997 BEND
/// `24381/100848` 因 RTorus 轴缩放修复留下两条同源关系，正是这个失效模式。
fn render_geo_relate_replace(inst_info_id: &str, rows: &BTreeMap<String, String>) -> String {
    let values = rows.values().join(",");
    format!(
        "BEGIN TRANSACTION;\n\
         DELETE inst_info:⟨{inst_info_id}⟩->geo_relate;\n\
         INSERT RELATION INTO geo_relate [{values}];\n\
         COMMIT TRANSACTION;"
    )
}

/// 刷新确定性 `inst_geo` 的几何参数，同时保留已经生成的 mesh 派生字段。
///
/// `geo_hash` 相同意味着记录 id 相同——而**不同的 `PdmsGeoParam` 变体可以合法地
/// 共享同一个 id**：普通 LCylinder 与非切角 SCylinder 的单位网格同为单位圆柱，
/// `hash_unit_mesh_params` 按设计都返回 `CYLINDER_GEO_HASH`。因此 `param` 绝不能
/// 走对象深合并：`UPSERT … MERGE { param: … }`（第一版 `INSERT IGNORE` 的替换写法）
/// 会把先后两个变体并成 `{ PrimLCylinder: …, PrimSCylinder: … }` 双键对象，enum
/// 反序列化从此永久失败，**所有**引用该共享行的根一个都生成不出来（2026-08-13
/// live A/B 全链路执行实测击中，`.scratch/net-ab-run4.log`：2,229 根批量重生成
/// 全灭）。`UPSERT … SET param = …` 整值覆盖：行缺失时补齐、半成品被修复、已
/// meshed 的派生字段（`meshed` / `aabb` / `pts`）原样保留；对已被旧写法打坏的
/// 双键行也是自愈——下一次参数刷新整值盖掉即恢复可解。
pub(crate) fn render_inst_geo_upsert(
    geo_hash: u64,
    unit_param_json: &str,
    reset_bad: bool,
) -> String {
    let mut sql = format!("UPSERT inst_geo:⟨{geo_hash}⟩ SET param = {unit_param_json};");
    // 强制再生成代表调用方明确要求用当前解析/网格代码重试。旧 `bad=true` 若不清，
    // gen_inst_meshes 的入口过滤会在真正构形之前永久跳过这条记录。
    if reset_bad {
        sql.push_str(&format!("\nUPDATE inst_geo:⟨{geo_hash}⟩ SET bad = false;"));
    }
    sql
}

#[cfg(test)]
async fn finish_db_writes(
    mut tasks: futures::stream::FuturesUnordered<tokio::task::JoinHandle<anyhow::Result<()>>>,
) -> anyhow::Result<()> {
    use futures::StreamExt;

    let mut first_error = None;
    while let Some(result) = tasks.next().await {
        let error = match result {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => error,
            Err(error) => anyhow::anyhow!("instance write task failed: {error}"),
        };
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// 初始化数据库的 inst_relate / tubi_relate 表的索引。
///
/// F1（`docs/2026-08-05_fork-surreal-compat-findings.md`）：旧语法带 `TYPE BTREE`，
/// 在 fork 2.1.4 上是解析错误，又被 `let _ =` 吞掉——索引在生产从未建成。
/// 现在用合法语法建普通索引并显式上抛错误；`IF NOT EXISTS` 保证每次启动重放幂等。
///
/// P3（层级查询优化收尾）：`zone_refno` 已退役——读侧全部切到 `anc CONTAINS`，
/// 该列不再写入、其索引由常量里的 `REMOVE INDEX IF EXISTS` 迁移语句在启动时
/// 摘除（存量行的旧值保留不动，只是不再被索引与维护）。
pub async fn init_inst_relate_indices() -> anyhow::Result<()> {
    println!("正在初始化 inst_relate/tubi_relate 索引（首次构建需全表扫描，可能较久）...");
    let started = std::time::Instant::now();
    SUL_DB.query(INST_RELATE_INDEX_SQL).await?.check()?;
    println!(
        "inst_relate/tubi_relate 索引就绪，耗时 {}",
        crate::fmt_elapsed(started.elapsed())
    );
    Ok(())
}

/// 实例边表索引的唯一事实来源：生产启动（[`init_inst_relate_indices`]）与
/// 暂存库建库（`staging::lifecycle::init_staging_schema`）用同一组语句。
///
/// `anc`（RefU64 打包祖先链，数组列）与 `dbnum` 服务层级查询优化
/// （`docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`）：
/// 任意根的子树实例 = `WHERE anc CONTAINS $root` 一条索引查询。
/// 2.1.4 上数组列普通索引 + CONTAINS 走索引已由双跑套件钉住（P0，Go）。
///
/// 前两行的 `REMOVE INDEX IF EXISTS` 是 P3 的一次性迁移：摘掉已退役的
/// zone_refno 索引的**两个历史名字**——`idx_inst_relate_zone_refno`（本仓
/// F1 修复后建的）与 `inst_relate_zone_refno_index`（plant-ui rs-core
/// `define_pe_index` 历史上建的，AMS 实库两者并存实测在案）。旧库有、
/// 新库/暂存库没有——`IF EXISTS` 各种情况都是安全 no-op，含表尚不存在的
/// 全新库，2.1.4 双引擎语义由双跑套件钉住。
pub const INST_RELATE_INDEX_SQL: &str = "\
    REMOVE INDEX IF EXISTS idx_inst_relate_zone_refno ON TABLE inst_relate;\n\
    REMOVE INDEX IF EXISTS inst_relate_zone_refno_index ON TABLE inst_relate;\n\
    DEFINE INDEX IF NOT EXISTS idx_inst_relate_anc ON TABLE inst_relate COLUMNS anc;\n\
    DEFINE INDEX IF NOT EXISTS idx_inst_relate_dbnum ON TABLE inst_relate COLUMNS dbnum;\n\
    DEFINE INDEX IF NOT EXISTS idx_tubi_relate_anc ON TABLE tubi_relate COLUMNS anc;\n\
    DEFINE INDEX IF NOT EXISTS idx_tubi_relate_dbnum ON TABLE tubi_relate COLUMNS dbnum;";

/// 存量 `inst_relate` / `tubi_relate` 行的 `anc` + `dbnum` 回填（幂等，自愈式）。
///
/// 每轮圈 `anc = NONE` 的一批行、按 `in` 端 pe 的活 owner 链重算，直到无行可补。
/// 分批是为了限住单事务体量；顺序无所谓，中断重跑无害。
/// 返回 (inst_relate 补行数, tubi_relate 补行数)。
///
/// P3 之后这里不再顺手补 `zone_refno`（该列已退役，读侧无消费者）——每行一次的
/// `fn::find_ancestor_type`（9 跳 owner 上溯）从回填成本里整个消失。
pub async fn backfill_inst_relate_anc() -> anyhow::Result<(usize, usize)> {
    /// `RefU64` 把 ref0 放高 32 位，而 Surreal 数值是 i64——ref0 超过 `i32::MAX`
    /// 的行（live 夹具的魔术 dbnum 残留是唯一已知来源）在 `fn::anc_u64` 里必然
    /// 乘法溢出，且一行就能炸掉整批 UPDATE 事务（2026-08-13 testbed 实测）。
    /// 回填按此上限跳过它们并告警，不让外来行阻断整表自愈。
    const PACKABLE_REF0_MAX: u32 = i32::MAX as u32;

    async fn drain_table(table: &str) -> anyhow::Result<usize> {
        const BATCH: usize = 2000;
        println!("正在回填 {table} 的存量 anc/dbnum（老库首次启动需全表回填）...");
        let started = std::time::Instant::now();

        let skip_sql = format!(
            "RETURN array::len(SELECT VALUE id FROM {table} WHERE anc = NONE \
             AND type::number(string::split(record::id(id), '_')[0]) > {PACKABLE_REF0_MAX});"
        );
        let mut response = SUL_DB.query(skip_sql).await?.check()?;
        let skipped: Option<usize> = response.take(0)?;
        let skipped = skipped.unwrap_or(0);
        if skipped > 0 {
            let msg = format!(
                "{table}: {skipped} 行 ref0 超出 u64 打包上限（fixture 魔术 dbnum 残留？），\
                 anc 回填跳过它们"
            );
            log::warn!("{msg}");
            eprintln!("{msg}");
        }

        let mut total = 0usize;
        loop {
            let sql = format!(
                "LET $rows = SELECT VALUE id FROM {table} WHERE anc = NONE \
                 AND type::number(string::split(record::id(id), '_')[0]) <= {PACKABLE_REF0_MAX} \
                 LIMIT {BATCH};\n\
                 UPDATE $rows SET anc = fn::anc_u64(in), dbnum = in.dbnum RETURN NONE;\n\
                 RETURN array::len($rows);"
            );
            let mut response = SUL_DB.query(sql).await?.check()?;
            let filled: Option<usize> = response.take(2)?;
            let filled = filled.unwrap_or(0);
            total += filled;
            if filled < BATCH {
                return Ok(total);
            }
            // 还有下一批才报进度：已收敛的常态启动只有开始/完成两行。
            println!(
                "  anc/dbnum 回填中（{table}）：累计 {total} 行，耗时 {}",
                crate::fmt_elapsed(started.elapsed())
            );
        }
    }
    let started = std::time::Instant::now();
    let inst = drain_table("inst_relate").await?;
    let tubi = drain_table("tubi_relate").await?;
    println!(
        "anc/dbnum 回填完成：inst_relate {inst} 行，tubi_relate {tubi} 行，耗时 {}",
        crate::fmt_elapsed(started.elapsed())
    );
    Ok((inst, tubi))
}

/// P4 写时物化的脏位：本进程生成/刷新过 inst_relate 之后置位，worker 空闲轮
/// 据此决定要不要跑一轮 [`sweep_inst_relate_flat`]（避免每个空闲轮白扫整表）。
static INSTS_FLAT_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_insts_flat_dirty() {
    INSTS_FLAT_DIRTY.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 「这一行有没有可用的布尔成品」的唯一判据（SurrealQL 片段）。
///
/// 空串与任意大小写的字面 `'none'` 都是脏值，按「无成品」处理（Spec 019 FR-006）。
/// 回填段的分支条件与修复段的 `WHERE` 必须引用这同一份：两处曾经各写各的——回填只判
/// `booled_id != NONE`，而空串不是 NONE，于是 `booled_id = ''` 的行被回填成
/// `insts_flat = [{ geo_hash: '' }]`，与同一函数下面两段「空串按无成品处理、平表不得
/// 改写」的定义正相反。
///
/// 实参一律 `?? ''` 兜底：生产 8009 服务器对 NONE 实参直接报错（AND/OR 不短路，
/// 另一臂照样求值），mem/fork 2.1.4 反而容忍——三引擎的真实分叉，2026-08-20 实测钉死。
const VALID_BOOLED: &str =
    "booled_id != NONE AND booled_id != '' AND string::lowercase(booled_id ?? '') != 'none'";

/// 监听限定域（`watch_dbnums` / `--watch-dbnum`）在 `inst_relate` 上的谓词**前缀**。
///
/// 空表 = 未限定 = 空串，判定与本前缀引入前逐位相同（`watch_scope` 的既有契约）。
///
/// **列名必须裸着出现。** 隔壁 [`crate::data_interface::model_update_pending`] 的
/// 同义片段写的是 `(dbnum?:0) IN […]`——那张表一个索引都没有，怎么写都是全表扫，
/// 缺值兜底不花钱。这里恰恰相反：`idx_inst_relate_dbnum`（见
/// [`INST_RELATE_INDEX_SQL`]）是本前缀存在的**全部理由**，把列包进 `?:` 表达式
/// 等于把它藏起来，planner 只能回落全表，那就白改了。代价是 `dbnum` 为 NONE 的
/// 历史行落在限定域之外——由 [`scoped_maintenance_notice`] 当场说出来。
///
/// 单成员渲染成 `=` 而不是 `IN [x]`：限定域绝大多数时候就是一个库，而等值是任何
/// planner 都认的最朴素形态，不必赌 fork 2.1.4 对 `IN` 的索引联合支持。多成员那支
/// 仍是 `IN`，落地前值得对着现场库跑一次 `EXPLAIN FULL` 复核（specs/025 T02 的口径）。
fn render_watch_scope_filter(dbnums: &[u32]) -> String {
    match dbnums {
        [] => String::new(),
        [only] => format!("dbnum = {only} AND "),
        many => {
            let members = many
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("dbnum IN [{members}] AND ")
        }
    }
}

/// 限定域生效时，缓存维护这几段必须**自己出声**。
///
/// `watch_scope::mode_notice` 那句说的是增量摄入的收窄，而这里收窄的是整表自愈：
/// 「限定域外的行这一轮没被维护」在别处一个字都不会出现，不说就是静默收窄——
/// 与 issue #10 同一个形状。
fn scoped_maintenance_notice(
    dbnums: &[u32],
    origin: crate::data_interface::watch_scope::Origin,
) -> Option<String> {
    (!dbnums.is_empty()).then(|| {
        format!(
            "本轮 inst_relate 缓存维护按 {} {} 收窄（来自{}），走 idx_inst_relate_dbnum：\
             限定域外的行、以及 dbnum 为 NONE 的历史行这一轮都不维护，它们的 insts_flat \
             保持原样、读侧走 slim 兜底（慢，不错）。全库收敛要在不带限定域的进程里\
             跑一次启动序列。",
            crate::data_interface::watch_scope::WATCH_CONFIG_KEY,
            dbnums
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            origin.describe()
        )
    })
}

/// 存量 `inst_relate` 行的平表副本清扫（P4 写时物化；幂等，自愈式）。
///
/// 圈 `insts_flat = NONE` 且对读者可见（`aabb.d != none`）的行，服务端一次性
/// 物化三件：`insts_flat`（读投影 insts 子查询的派生缓存）、`aabb_d`、
/// `world_trans_d`。**持久层非 journal 路径**（与 [`backfill_inst_relate_anc`]
/// 同族）：建行/刷新语句只写纯字面量，唯一需要现场求值的 insts 子查询收口在
/// 这里，不进 journal、不碰暂存窗口。
///
/// 挂两处：启动序列（存量回填 = 首轮全量，pre-P4 库一次付清）＋ worker 空闲轮
/// （脏位门控）。新行只会「缺」（NONE，读侧走 slim 兜底）不会「错」：置 meshed
/// 的生成批与建行同任务同 refno 锚点，任务成功 ⇒ 可达 geo 全 meshed|bad。
///
/// 但存量库里有一类历史「错」行（Spec 019）：2026-08-20 之前布尔成功只写
/// `booled_id` 不同步平表，而首轮全量回填按正体原语落了 `insts_flat`——
/// 行「三副本齐活」，读侧不落 slim 兜底，端给查看者的是带原语缩放的错误正体
/// （RM13 事故，`docs/evidence/2026-08-20-rm13-dome-live/`）。这批行的**修复**
/// 自 Spec 025 FR-9 起改制为带库上标记的一次性 migration
/// （[`run_booled_flat_repair_migration_on`]）：标记已落的库这里只付一次
/// record id 点查，不再每轮全表扫。
///
/// # 监听限定域收窄（ADR-048 决策 2 的延伸）
///
/// 三段谓词都不可索引，`LIMIT` 在命中稀少时一格也省不下来——引擎要把整张表走完
/// 才敢说「不足一批」，而 `aabb.d` 是记录链接，每一条 `insts_flat = NONE` 的行还
/// 要多付一次 `aabb` 点查（按 issue #21 在库 A 的普查，NONE 行里约 97% 是读者不可
/// 见的，钱全花在这上面）。448 个 dbnum 的现场库上这一步能把整个启动序列压住。
///
/// 所以本进程声明了 `watch_dbnums` 时，这几段一律带上 [`render_watch_scope_filter`]
/// 的前缀走 `idx_inst_relate_dbnum`：**限定域只收窄、不放宽**，域外的行保持原样、
/// 读侧走 slim 兜底。这与 ADR-048 决策 3（不让限定域把全量房间重建拖回来）同向——
/// 手写「本次跑就要这几个库」的人，要的不是全库缓存维护。未声明限定域时空前缀，
/// 行为逐位不变。
///
/// 这只是把成本压回索引，**不等于 specs/025 FR-1 已闭环**：不带限定域的进程照旧
/// 在启动与空闲轮上发全表谓词。
pub async fn sweep_inst_relate_flat() -> anyhow::Result<usize> {
    const BATCH: usize = 500;
    let (scope, origin) = crate::data_interface::watch_scope::resolved();
    let scoped = render_watch_scope_filter(&scope);
    println!("正在清扫 inst_relate 平表副本（insts_flat 物化；pre-P4 存量库首轮为全表）...");
    if let Some(notice) = scoped_maintenance_notice(&scope, origin) {
        println!("  {notice}");
    }
    let started = std::time::Instant::now();
    let mut total = 0usize;
    loop {
        let sql = format!(
            "LET $rows = SELECT VALUE id FROM inst_relate WHERE {scoped}insts_flat = NONE AND aabb.d != none LIMIT {BATCH};\n\
             UPDATE $rows SET insts_flat = IF {VALID_BOOLED} THEN \
             [{{ geo_hash: booled_id }}] ELSE (SELECT trans.d AS transform, record::id(out) AS geo_hash \
             FROM out->geo_relate WHERE visible && out.meshed && trans.d != none && geo_type='Pos') END, \
             aabb_d = aabb.d, world_trans_d = world_trans.d RETURN NONE;\n\
             RETURN array::len($rows);"
        );
        let mut response = SUL_DB.query(sql).await?.check()?;
        let filled: Option<usize> = response.take(2)?;
        let filled = filled.unwrap_or(0);
        total += filled;
        if filled < BATCH {
            break;
        }
        // 还有下一批才报进度：已收敛的常态启动只有开始/完成两行。
        println!(
            "  inst_relate 平表清扫中：累计 {total} 行，耗时 {}",
            crate::fmt_elapsed(started.elapsed())
        );
    }
    // 修复段（Spec 019 → Spec 025 FR-9 改制）：一次性 migration，判据是库上的
    // 标记。常态（标记已落）这里只付一次 record id 点查，不再每轮全表扫。
    let repaired = run_booled_flat_repair_migration_on(&SUL_DB, &scope).await?;
    // 脏值可见性（FR-006）：不修、不藏，有就喊一声留给人裁决。
    //
    // 只探有无、不数个数。这段谓词在 `inst_relate` 上不可索引（该表只有 anc / dbnum
    // 两个索引），原写法为了在日志里多一个数字，付的是一次**无界**全表扫，而空闲轮
    // 每一轮都要付一遍。精确计数走人工诊断入口。
    //
    // 限定域前缀之后那对括号是必须的：这一段是 `OR`，不括起来 `AND` 结合更紧，
    // 谓词会变成「域内的空串 或 全库的字面 'none'」——半边收窄等于没收窄。
    let mut response = SUL_DB
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM inst_relate \
             WHERE {scoped}(booled_id = '' OR string::lowercase(booled_id ?? '') = 'none') LIMIT 1)) > 0;"
        ))
        .await?
        .check()?;
    let junk: Option<bool> = response.take(0)?;
    if junk.unwrap_or(false) {
        println!(
            "  警告：存在 booled_id 为空串/字面 'none' 的脏值行，按无成品处理，平表未改写\
             （只探有无：该谓词不可索引，精确计数走人工诊断入口）"
        );
    }
    println!(
        "inst_relate 平表副本清扫完成：补 {total} 行、修复布尔存量 {repaired} 行，耗时 {}",
        crate::fmt_elapsed(started.elapsed())
    );
    Ok(total + repaired)
}

/// 空闲轮入口：脏位置位过才真的扫；失败把脏位放回去，下一轮重试。
pub async fn sweep_inst_relate_flat_if_dirty() -> anyhow::Result<usize> {
    if !INSTS_FLAT_DIRTY.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Ok(0);
    }
    match sweep_inst_relate_flat().await {
        Ok(total) => Ok(total),
        Err(error) => {
            mark_insts_flat_dirty();
            Err(error)
        }
    }
}

/// RM13 存量修复的迁移标记（Spec 025 FR-9 / T06）：修复在此库上**完整跑完且复核
/// 无残留**之后才落下。判据是库上的标记而不是「本进程跑过一次」——旧备份恢复、
/// 库拷贝会把库带回没有标记的状态，修复随之自动重跑；而「跑了一半死掉」的库
/// 因为标记未落，下一次照样从头收敛。与暂停旗标（`queue_control:main`）、播种
/// 标记（`queue_control:watermark_seed`）同表不同行。
const BOOLED_FLAT_REPAIR_MARKER: &str = "queue_control:booled_flat_repair_migration";

/// 「这一行的 `insts_flat` 与布尔成品不符」的唯一判据（SurrealQL 片段，恒与
/// [`VALID_BOOLED`] 联用）：平表缺失，或首元素 `geo_hash` 不等于 `booled_id`。
/// 修复循环、复核、启动探针三处引用这同一份，不得各写各的（与 [`VALID_BOOLED`]
/// 同一条纪律）。`array::first` 实参 `?? []` 兜底：生产 8009 对 NONE 实参直接报错。
const BOOLED_FLAT_MISMATCH: &str =
    "insts_flat = NONE OR array::first(insts_flat ?? []).geo_hash != booled_id";

/// 修复段（Spec 019 FR-001/FR-003，Spec 025 FR-9 改制为一次性 migration）：
/// 收「`booled_id` 有值而 `insts_flat` 与成品不符」的历史错行，改写为单位变换
/// 成品单实例。流程钉死为**标记不存在 → 修复到收敛 → 复核无残留 → 落标记**：
///
/// - 标记点查按 record id 寻址，常态成本与表容量无关（FR-1 的口径）；
/// - 修复谓词刻意不设 aabb 门槛——布尔行必然可见，个别不可见行修了也无害；
///   空串/字面 'none' 当缺失跳过（判据见 [`VALID_BOOLED`]，FR-006）；
/// - 复核仍有残留时**不落标记**并告警，下一次清扫从头重跑（可复活）；
/// - 修复语句幂等，中断重跑无害；标记只在复核干净后写入，「跑了一半」与
///   「跑完了」由标记区分（与播种标记同一论证）。
///
/// `scope` 是监听限定域（[`render_watch_scope_filter`]，空表 = 全库）。收窄时修复
/// 循环与复核都只覆盖域内的库，因此**这一轮不落标记**：标记的含义是「这一库全表
/// 收敛过」，域内跑干净担保不了这句话。宁可下一次启动重跑（域内已收敛时是一次
/// record id 点查加一轮空转），也不能让一次收窄的运行把整库判成收敛完毕——那正是
/// FR-9 把判据从「本进程跑过一次」搬到库上标记时要防的东西。
pub(crate) async fn run_booled_flat_repair_migration_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    scope: &[u32],
) -> anyhow::Result<usize> {
    const BATCH: usize = 500;
    let scoped = render_watch_scope_filter(scope);
    // 1) 标记点查（record id 寻址，非全表谓词；布尔在引擎侧算好，客户端不碰
    //    datetime 反序列化）。
    let mut response = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM {BOOLED_FLAT_REPAIR_MARKER})) > 0;"
        ))
        .await?
        .check()?;
    let marker_present: Option<bool> = response.take(0)?;
    if marker_present.unwrap_or(false) {
        return Ok(0);
    }

    // 2) 修复到收敛。
    println!("RM13 布尔平表存量修复 migration：库上无完成标记，开始收敛...");
    let started = std::time::Instant::now();
    let mut repaired = 0usize;
    loop {
        let sql = format!(
            "LET $rows = SELECT VALUE id FROM inst_relate \
             WHERE {scoped}{VALID_BOOLED} \
             AND ({BOOLED_FLAT_MISMATCH}) LIMIT {BATCH};\n\
             UPDATE $rows SET insts_flat = [{{ geo_hash: booled_id }}] RETURN NONE;\n\
             RETURN array::len($rows);"
        );
        let mut response = db.query(sql).await?.check()?;
        let fixed: Option<usize> = response.take(2)?;
        let fixed = fixed.unwrap_or(0);
        repaired += fixed;
        if fixed < BATCH {
            break;
        }
        println!(
            "  inst_relate 布尔平表修复中：累计 {repaired} 行，耗时 {}",
            crate::fmt_elapsed(started.elapsed())
        );
    }

    // 3) 复核无残留（同一份谓词，只探有无）。
    let mut response = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM inst_relate \
             WHERE {scoped}{VALID_BOOLED} \
             AND ({BOOLED_FLAT_MISMATCH}) \
             LIMIT 1)) > 0;"
        ))
        .await?
        .check()?;
    let residue: Option<bool> = response.take(0)?;
    if residue.unwrap_or(true) {
        println!(
            "  警告：RM13 修复收敛后复核仍有残留（修复 {repaired} 行），\
             **不落标记**，下一次清扫从头重跑"
        );
        return Ok(repaired);
    }

    // 3.5) 收窄跑过的这一轮担保不了「这一库全表收敛过」，所以不落标记。
    if !scope.is_empty() {
        println!(
            "  RM13 修复本轮按监听限定域收窄，只覆盖了域内的库（修复 {repaired} 行）：\
             **不落标记**——标记的含义是全表收敛过，域内干净担保不了它。下一次启动\
             照常重跑（域内已收敛时是一次点查加一轮空转）；要落标记就在不带限定域的\
             进程里跑一次启动序列。"
        );
        return Ok(repaired);
    }

    // 4) 落标记（复核干净才写；spec 名过 escape 是纪律，不是这里真有特殊字符）。
    let spec_name = crate::data_interface::dbnum_state::escape_surql_str(
        "specs/019-booled-flat-backfill-closure + specs/025 FR-9",
    );
    db.query(format!(
        "UPSERT {BOOLED_FLAT_REPAIR_MARKER} SET spec = '{spec_name}', \
         repaired = {repaired}, completed_at = time::now();"
    ))
    .await?
    .check()?;
    println!(
        "RM13 布尔平表存量修复 migration 完成：修复 {repaired} 行并落下完成标记，耗时 {}",
        crate::fmt_elapsed(started.elapsed())
    );
    Ok(repaired)
}

/// 启动序列专用的「老格式再现」探针（Spec 025 FR-9 的盲区补口）。
///
/// FR-9 列的三个会重新产生老格式的场景里，「旧备份恢复」「库拷贝」行与标记同库
/// 同退，migration 盖得住；「滚动部署期间旧 writer 混跑」恰恰盖不住——标记已落
/// 的库上旧 writer 写出的老格式行，migration 按标记跳过、回填段只圈 NONE、脏值
/// 探针只探空串/'none'。在 FR-8/T20（读侧行内自检）落地前，这里是「它跳过的
/// 东西谁会发现」的唯一答案。发现了**只喊不修**：修复的权威入口仍是 migration，
/// 操作员删掉 [`BOOLED_FLAT_REPAIR_MARKER`] 标记行即可强制下一次清扫重跑收敛。
///
/// 只挂启动序列（FR-1：`inst_relate` 全表谓词只允许启动序列与人工诊断入口，
/// 空闲轮不得调用，谓词无索引、`LIMIT 1` 不豁免）。标记点查在前：标记未落时
/// migration 本轮自己会跑并报告，这里直接返回，不多付一次全表谓词。
///
/// 返回：`None` = 标记未落（未探）；`Some(true)` = 探到老格式再现，已告警；
/// `Some(false)` = 干净。
///
/// `scope` 同 [`run_booled_flat_repair_migration_on`]：收窄时只探域内的库。探针
/// 只喊不修，漏喊的代价是「域外的老格式行这一轮没人报告」——而收窄运行本来就不
/// 声称覆盖域外，比让启动卡在一次全表谓词上诚实。
pub async fn probe_booled_flat_regression_after_migration() -> anyhow::Result<Option<bool>> {
    probe_booled_flat_regression_after_migration_on(
        &SUL_DB,
        &crate::data_interface::watch_scope::dbnums(),
    )
    .await
}

pub(crate) async fn probe_booled_flat_regression_after_migration_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    scope: &[u32],
) -> anyhow::Result<Option<bool>> {
    let scoped = render_watch_scope_filter(scope);
    let mut response = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM {BOOLED_FLAT_REPAIR_MARKER})) > 0;"
        ))
        .await?
        .check()?;
    let marker_present: Option<bool> = response.take(0)?;
    if !marker_present.unwrap_or(false) {
        return Ok(None);
    }
    let mut response = db
        .query(format!(
            "RETURN array::len((SELECT VALUE id FROM inst_relate \
             WHERE {scoped}{VALID_BOOLED} \
             AND ({BOOLED_FLAT_MISMATCH}) \
             LIMIT 1)) > 0;"
        ))
        .await?
        .check()?;
    let mismatch: Option<bool> = response.take(0)?;
    // 读不出按有事算：探针的产出只有一行告警，宁多喊不漏喊。
    let mismatch = mismatch.unwrap_or(true);
    if mismatch {
        let msg = format!(
            "迁移标记已落的库上再次出现布尔平表老格式行（booled_id 有值而 insts_flat \
             与之不符）——多半是旧 writer 混跑（滚动部署窗口）。migration 按标记跳过、\
             清扫两段也够不着这批行，读侧会把错误正体端给查看者；删除 \
             {BOOLED_FLAT_REPAIR_MARKER} 标记行可强制下一次清扫重跑修复"
        );
        log::warn!("{msg}");
        eprintln!("{msg}");
    }
    Ok(Some(mismatch))
}

/// 一个生成行（inst_relate / tubi_relate）的渲染期元数据（W4，决议 D6）。
///
/// 这些值从前以 `fn::find_ancestor_type` / `fn::anc_u64` / `{pe}.dbnum` /
/// `fn::ses_date` 的形态**内联在 journal 字面量里**：窗口内对暂存求值一遍、
/// 写回重放时对持久层**再求值一遍**——重放硬依赖持久层灌了这些函数（与
/// issue #16 同族的故障面）。现在渲染时解一次、写死固定值，journal 变成纯数据。
#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedInstMeta {
    /// 自身 → 顶的祖先链 RefU64 打包值（含自身）——与 `fn::anc_u64(pe)` 同义，
    /// 但走 Rust 链不吃函数展开层数的静默截断。
    /// （最近 ZONE 从前是独立的 `zone_refno` 字段，P3 已随该列退役——语义上
    /// 它就是链上第一个 ZONE，读侧要用直接查 `anc`。）
    pub anc: Vec<u64>,
    /// `pe.dbnum`。
    pub dbnum: Option<i64>,
    /// `fn::ses_date(pe)` 的渲染就绪字面量（`d'…'` / NONE）。
    pub dt_literal: Option<String>,
}

impl ResolvedInstMeta {
    pub(crate) fn anc_literal(&self) -> String {
        format!(
            "[{}]",
            self.anc
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    pub(crate) fn dbnum_literal(&self) -> String {
        self.dbnum
            .map(|dbnum| dbnum.to_string())
            .unwrap_or_else(|| "NONE".into())
    }

    pub(crate) fn dt_literal(&self) -> &str {
        self.dt_literal.as_deref().unwrap_or("NONE")
    }
}

/// 生产入口：经读路由解析（暂存窗口内查暂存——W1 已保证生成根的子树与祖先都在；
/// 直写模式查持久层）。`ses` 行是 append-only 历史：暂存里只有本窗口的新会话，
/// 旧会话的日期回落持久层点查——单写者下「暂存新态 + 持久层旧态」的合成恰等于
/// 写回重放后的持久层，也就是老字面量在重放时求值看到的同一个世界。
pub(crate) async fn resolve_inst_meta(
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashMap<RefU64, ResolvedInstMeta>> {
    let db = crate::data_interface::staging::active_data_db();
    let ses_fallback = aios_core::staging::active_staging_reads().map(|_| SUL_DB.clone());
    resolve_inst_meta_on(&db, ses_fallback.as_ref(), refnos).await
}

pub(crate) async fn resolve_inst_meta_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ses_fallback: Option<&surrealdb::Surreal<surrealdb::engine::any::Any>>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<HashMap<RefU64, ResolvedInstMeta>> {
    use crate::data_interface::cata_closure::is_valid_ref0;
    const CHUNK: usize = 200;
    /// 防御性走链上限（owner 环 / 数据损坏的最后一道闸）。
    const WALK_CAP: usize = 64;

    #[derive(serde::Deserialize)]
    struct PeMetaRow {
        id: RefnoEnum,
        #[serde(default)]
        owner: Option<RefnoEnum>,
        #[serde(default)]
        dbnum: Option<i64>,
        #[serde(default)]
        sesno: Option<i64>,
    }
    /// (owner, dbnum, sesno)。
    type PeMeta = (Option<RefU64>, Option<i64>, Option<i64>);

    let mut seeds: Vec<RefU64> = refnos
        .iter()
        .map(RefnoEnum::refno)
        .filter(|refno| is_valid_ref0(refno.get_0()))
        .collect();
    seeds.sort_unstable();
    seeds.dedup();
    if seeds.is_empty() {
        return Ok(HashMap::new());
    }

    // 1) 层级预取：seed 层 → owner 层 → …（`None` = 行不存在，问过了）。
    let mut cache: HashMap<RefU64, Option<PeMeta>> = HashMap::new();
    let mut frontier = seeds.clone();
    let mut level = 0usize;
    while !frontier.is_empty() {
        level += 1;
        anyhow::ensure!(
            level <= WALK_CAP,
            "生成行元数据解析：owner 链层级超过 {WALK_CAP}（疑似成环），拒绝继续"
        );
        let mut next = Vec::new();
        for chunk in frontier.chunks(CHUNK) {
            let keys = chunk
                .iter()
                .map(|refno| refno.to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let mut response = db
                .query(format!(
                    "SELECT id, owner, dbnum, sesno FROM [{keys}] WHERE record::exists(id);"
                ))
                .await?
                .check()?;
            let rows: Vec<PeMetaRow> = response.take(0)?;
            let mut found = std::collections::HashSet::new();
            for row in rows {
                let refno = row.id.refno();
                found.insert(refno);
                let owner = row
                    .owner
                    .map(|owner| owner.refno())
                    .filter(|owner| is_valid_ref0(owner.get_0()));
                if let Some(owner) = owner
                    && !cache.contains_key(&owner)
                {
                    next.push(owner);
                }
                cache.insert(refno, Some((owner, row.dbnum, row.sesno)));
            }
            for refno in chunk {
                if !found.contains(refno) {
                    cache.entry(*refno).or_insert(None);
                }
            }
        }
        next.sort_unstable();
        next.dedup();
        next.retain(|refno| !cache.contains_key(refno));
        frontier = next;
    }

    // 2) 逐 seed 出链：anc / dbnum / sesno。
    let mut out: HashMap<RefU64, ResolvedInstMeta> = HashMap::new();
    let mut ses_pairs: std::collections::BTreeSet<(i64, i64)> = std::collections::BTreeSet::new();
    for &seed in &seeds {
        let Some(Some(seed_meta)) = cache.get(&seed).cloned() else {
            // 行不在：与旧字面量对缺行的求值语义一致（fn:: 全 NONE / 空数组）。
            println!(
                "生成行元数据解析：{} 的 pe 行不在当前世界，anc/dbnum/dt 按空态渲染",
                seed.to_pe_key()
            );
            out.insert(seed, ResolvedInstMeta::default());
            continue;
        };
        let mut chain = Vec::new();
        let mut cursor = seed;
        loop {
            let Some(Some(meta)) = cache.get(&cursor).cloned() else {
                // 全量入库刻意不保存 WORL 行（database.rs 的 ignore_world_refno），
                // 但元素的 owner 仍指向 ref1=0 的 WORL；旧 fn::ancestor 会把这个
                // record 链接保留在 anc 后自然到顶。这里保持同一语义。
                if cursor.get_1() == 0 {
                    chain.push(cursor.0);
                    break;
                }
                // owner 字段指向的行不存在：这不是「到顶」，是断链。W1 之后暂存
                // 里生成根的闭包与祖先都应在场，缺 = 预载被破坏；直写模式缺 =
                // 持久层数据损坏。宁可响亮失败进重试，也不烘一个错值进 journal。
                anyhow::bail!(
                    "生成行元数据解析：{} 的祖先链在 {} 处断裂（owner 指向的行不存在）。\
                     暂存窗口内这意味着祖先预载被破坏，直写模式意味着持久层所有权数据损坏",
                    seed.to_pe_key(),
                    cursor.to_pe_key()
                );
            };
            anyhow::ensure!(
                cursor.0 <= i64::MAX as u64,
                "refno 打包值 {} 超出 SurrealDB int（i64）上限，拒绝静默截断",
                cursor.0
            );
            anyhow::ensure!(
                chain.len() <= WALK_CAP,
                "生成行元数据解析：{} 的祖先链超过 {WALK_CAP} 跳（疑似成环）",
                seed.to_pe_key()
            );
            chain.push(cursor.0);
            match meta.0 {
                Some(owner) => cursor = owner,
                None => break,
            }
        }
        if let (Some(dbnum), Some(sesno)) = (seed_meta.1, seed_meta.2) {
            ses_pairs.insert((dbnum, sesno));
        }
        out.insert(
            seed,
            ResolvedInstMeta {
                anc: chain,
                dbnum: seed_meta.1,
                dt_literal: None, // 下面按 (dbnum, sesno) 批量补
            },
        );
    }

    // 3) 会话日期：`ses:[dbnum, sesno].date`。先问当前世界（暂存窗口内含本窗口
    //    新会话），miss 再回落持久层（旧会话的历史行）。
    let mut ses_dates: HashMap<(i64, i64), String> = HashMap::new();
    let pairs = ses_pairs.into_iter().collect::<Vec<_>>();
    for chunk in pairs.chunks(CHUNK) {
        let probes = chunk
            .iter()
            .map(|(dbnum, sesno)| format!("ses:[{dbnum}, {sesno}].date"))
            .collect::<Vec<_>>()
            .join(",");
        let mut response = db.query(format!("RETURN [{probes}];")).await?.check()?;
        let values: surrealdb::Value = response.take(0)?;
        let surrealdb::sql::Value::Array(values) = values.into_inner() else {
            anyhow::bail!("会话日期探针返回了非数组结果");
        };
        for (pair, value) in chunk.iter().zip(values.iter()) {
            if !matches!(
                value,
                surrealdb::sql::Value::None | surrealdb::sql::Value::Null
            ) {
                ses_dates.insert(
                    *pair,
                    crate::data_interface::staging::preload::render_preload_value(value),
                );
            }
        }
    }
    if let Some(fallback) = ses_fallback {
        let missing = pairs
            .iter()
            .filter(|pair| !ses_dates.contains_key(pair))
            .copied()
            .collect::<Vec<_>>();
        for chunk in missing.chunks(CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let probes = chunk
                .iter()
                .map(|(dbnum, sesno)| format!("ses:[{dbnum}, {sesno}].date"))
                .collect::<Vec<_>>()
                .join(",");
            let mut response = fallback
                .query(format!("RETURN [{probes}];"))
                .await?
                .check()?;
            let values: surrealdb::Value = response.take(0)?;
            let surrealdb::sql::Value::Array(values) = values.into_inner() else {
                anyhow::bail!("会话日期回落探针返回了非数组结果");
            };
            for (pair, value) in chunk.iter().zip(values.iter()) {
                if !matches!(
                    value,
                    surrealdb::sql::Value::None | surrealdb::sql::Value::Null
                ) {
                    ses_dates.insert(
                        *pair,
                        crate::data_interface::staging::preload::render_preload_value(value),
                    );
                }
            }
        }
    }
    for &seed in &seeds {
        if let Some(Some(seed_meta)) = cache.get(&seed)
            && let (Some(dbnum), Some(sesno)) = (seed_meta.1, seed_meta.2)
            && let Some(date) = ses_dates.get(&(dbnum, sesno))
            && let Some(meta) = out.get_mut(&seed)
        {
            meta.dt_literal = Some(date.clone());
        }
    }

    Ok(out)
}

fn insert_unique_record(
    records: &mut BTreeMap<String, String>,
    kind: &'static str,
    record_id: String,
    rendered: String,
) -> anyhow::Result<()> {
    match records.get(&record_id) {
        Some(existing) if existing == &rendered => Ok(()),
        Some(_) => Err(SaveConflict::RecordContent { kind, record_id }.into()),
        None => {
            records.insert(record_id, rendered);
            Ok(())
        }
    }
}

fn push_array_packets(
    packets: &mut Vec<SqlPacket>,
    phase: SavePhase,
    prefix: &str,
    suffix: &str,
    rows: &BTreeMap<String, String>,
) {
    let mut current: Vec<String> = Vec::new();
    let mut current_bytes = prefix.len() + suffix.len();
    let flush =
        |current: &mut Vec<String>, current_bytes: &mut usize, packets: &mut Vec<SqlPacket>| {
            if current.is_empty() {
                return;
            }
            let sql = format!("{prefix}{}{suffix}", current.join(","));
            packets.push(SqlPacket {
                phase,
                row_count: current.len(),
                estimated_bytes: sql.len(),
                sql,
            });
            current.clear();
            *current_bytes = prefix.len() + suffix.len();
        };

    for row in rows.values() {
        let next_bytes = row.len() + usize::from(!current.is_empty());
        if !current.is_empty()
            && (current.len() >= SQL_PACKET_ROWS
                || current_bytes.saturating_add(next_bytes) > SQL_PACKET_BYTES)
        {
            flush(&mut current, &mut current_bytes, packets);
        }
        current_bytes = current_bytes.saturating_add(next_bytes);
        current.push(row.clone());
    }
    flush(&mut current, &mut current_bytes, packets);
}

fn push_statement_packets(
    packets: &mut Vec<SqlPacket>,
    phase: SavePhase,
    statements: &BTreeMap<String, String>,
) {
    let mut current: Vec<String> = Vec::new();
    let mut current_bytes = 0usize;
    let flush =
        |current: &mut Vec<String>, current_bytes: &mut usize, packets: &mut Vec<SqlPacket>| {
            if current.is_empty() {
                return;
            }
            let sql = current.join("\n");
            packets.push(SqlPacket {
                phase,
                row_count: current.len(),
                estimated_bytes: sql.len(),
                sql,
            });
            current.clear();
            *current_bytes = 0;
        };

    for statement in statements.values() {
        let next_bytes = statement.len() + usize::from(!current.is_empty());
        if !current.is_empty()
            && (current.len() >= SQL_PACKET_ROWS
                || current_bytes.saturating_add(next_bytes) > SQL_PACKET_BYTES)
        {
            flush(&mut current, &mut current_bytes, packets);
        }
        current_bytes = current_bytes.saturating_add(next_bytes);
        current.push(statement.clone());
    }
    flush(&mut current, &mut current_bytes, packets);
}

fn push_inst_relate_packets(packets: &mut Vec<SqlPacket>, rows: &BTreeMap<String, String>) {
    let entries = rows
        .iter()
        .map(|(id, json)| (id.clone(), json.clone()))
        .collect::<Vec<_>>();
    let mut start = 0usize;
    while start < entries.len() {
        let mut end = start;
        let mut best_sql = String::new();
        while end < entries.len() && end - start < SQL_PACKET_ROWS {
            let candidate = render_inst_relate_replace(&entries[start..=end]);
            if end > start && candidate.len() > SQL_PACKET_BYTES {
                break;
            }
            best_sql = candidate;
            end += 1;
        }
        if end == start {
            end += 1;
            best_sql = render_inst_relate_replace(&entries[start..end]);
        }
        packets.push(SqlPacket {
            phase: SavePhase::InstanceRelations,
            row_count: end - start,
            estimated_bytes: best_sql.len(),
            sql: best_sql,
        });
        start = end;
    }
}

fn canonical_unit_param_json(inst: &EleInstGeo) -> anyhow::Result<String> {
    let param = if inst.geo_hash == aios_core::prim_geo::basic::CYLINDER_GEO_HASH {
        // LCylinder 与非切角 SCylinder 共用单位圆柱 id；统一为一个单键 enum，
        // 不能让先后顺序决定 `param` 的变体，更不能 MERGE 成双键对象。
        PdmsGeoParam::PrimLCylinder(LCylinder::default())
    } else {
        inst.geo_param.convert_to_unit_param()
    };
    serde_json::to_string(&param).map_err(Into::into)
}

fn build_canonical_save_plan(
    frozen: &FrozenShapeBatch,
    mode: SaveMode,
    flush_reason: FlushReason,
    coalesce_wait: Duration,
    inst_meta: &HashMap<RefU64, ResolvedInstMeta>,
    metadata_query_count: usize,
) -> anyhow::Result<SavePlan> {
    let missing_meta = ResolvedInstMeta::default();
    let mut transforms = BTreeMap::new();
    let mut aabbs = BTreeMap::new();
    let mut vec3s = BTreeMap::new();
    let mut inst_geos = BTreeMap::new();
    let mut inst_infos = BTreeMap::new();
    let mut geo_relates = BTreeMap::new();
    let mut geo_relates_by_inst_info: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut neg_relates = BTreeMap::new();
    let mut ngmr_relates = BTreeMap::new();
    let mut normal_inst_relates = BTreeMap::new();
    let mut tubi_inst_relates = BTreeMap::new();
    let mut written_by_id: BTreeMap<String, RefnoEnum> = BTreeMap::new();
    let mut canonical_neg_sequences: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for batch in frozen.batches() {
        let mut geo_keys = batch.inst_geos_map.keys().collect::<Vec<_>>();
        geo_keys.sort();
        for key in geo_keys {
            let data = &batch.inst_geos_map[key];
            for inst in &data.insts {
                if inst.transform.is_nan() {
                    return Err(SaveConflict::NonFiniteTransform {
                        kind: "geo_relate",
                        record_id: format!("{}:{}", data.id(), inst.geo_hash),
                    }
                    .into());
                }
                let transform_json = serde_json::to_string(&inst.transform)?;
                let transform_hash = gen_bytes_hash::<_, 64>(&inst.transform);
                insert_unique_record(
                    &mut transforms,
                    "transform",
                    format!("trans:⟨{transform_hash}⟩"),
                    format!("{{'id':trans:⟨{transform_hash}⟩, 'd':{transform_json}}}"),
                )?;

                let mut point_ids = Vec::new();
                for point in inst.geo_param.key_points() {
                    let point_hash = point.gen_hash();
                    let point_json = serde_json::to_string(&point)?;
                    let point_id = format!("vec3:⟨{point_hash}⟩");
                    insert_unique_record(
                        &mut vec3s,
                        "vec3",
                        point_id.clone(),
                        format!("{{'id':{point_id}, 'd':{point_json}}}"),
                    )?;
                    point_ids.push(point_id);
                }

                let cat_negs = if inst.cata_neg_refnos.is_empty() {
                    String::new()
                } else {
                    format!(
                        ", cata_neg: [{}]",
                        inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                    )
                };
                let relate_body = format!(
                    r#"in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩, geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}"#,
                    data.id(),
                    inst.geo_hash,
                    transform_hash,
                    inst.refno,
                    point_ids.join(","),
                    inst.geo_type,
                    inst.visible,
                    cat_negs,
                );
                let relate_id = gen_bytes_hash::<_, 64>(&relate_body);
                let relation_id = format!("geo_relate:{relate_id}");
                let relation = format!("{{ {relate_body}, id: '{relate_id}' }}");
                insert_unique_record(
                    &mut geo_relates,
                    "geo_relate",
                    relation_id.clone(),
                    relation.clone(),
                )?;
                insert_unique_record(
                    geo_relates_by_inst_info.entry(data.id()).or_default(),
                    "geo_relate for shared CATA identity",
                    relation_id,
                    relation,
                )?;

                let unit_param = canonical_unit_param_json(inst)?;
                insert_unique_record(
                    &mut inst_geos,
                    "inst_geo",
                    format!("inst_geo:⟨{}⟩", inst.geo_hash),
                    render_inst_geo_upsert(inst.geo_hash, &unit_param, mode.replaces_existing()),
                )?;
            }
        }

        let mut normal_keys = batch.inst_info_map.keys().collect::<Vec<_>>();
        normal_keys.sort_by_key(|refno| refno.to_string());
        for refno in normal_keys {
            let info = &batch.inst_info_map[refno];
            if info.world_transform.is_nan() {
                return Err(SaveConflict::NonFiniteTransform {
                    kind: "inst_relate",
                    record_id: refno.to_inst_relate_key(),
                }
                .into());
            }
            let mut ignored_vec3_map = HashMap::new();
            let info_json = info.gen_sur_json(&mut ignored_vec3_map);
            insert_unique_record(
                &mut inst_infos,
                "inst_info",
                format!("inst_info:⟨{}⟩", info.id_str()),
                info_json,
            )?;

            let transform_json = serde_json::to_string(&info.world_transform)?;
            let transform_hash = gen_bytes_hash::<_, 64>(&info.world_transform);
            insert_unique_record(
                &mut transforms,
                "transform",
                format!("trans:⟨{transform_hash}⟩"),
                format!("{{'id':trans:⟨{transform_hash}⟩, 'd':{transform_json}}}"),
            )?;
            let meta = inst_meta.get(&refno.refno()).unwrap_or(&missing_meta);
            let rendered = format!(
                "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, world_trans_d: {10}, generic: '{4}', anc: {5}, dbnum: {6}, dt: {7}, has_cata_neg: {8}, solid: {9}}}",
                refno.to_inst_relate_key(),
                refno.to_pe_key(),
                info.id_str(),
                transform_hash,
                info.generic_type,
                meta.anc_literal(),
                meta.dbnum_literal(),
                meta.dt_literal(),
                info.has_cata_neg,
                info.is_solid,
                transform_json,
            );
            let record_id = refno.to_inst_relate_key();
            insert_unique_record(
                &mut normal_inst_relates,
                "normal inst_relate",
                record_id.clone(),
                rendered,
            )?;
            written_by_id.entry(record_id).or_insert(*refno);
        }

        let mut tubi_keys = batch.inst_tubi_map.keys().collect::<Vec<_>>();
        tubi_keys.sort_by_key(|refno| refno.to_string());
        for refno in tubi_keys {
            let info = &batch.inst_tubi_map[refno];
            if info.world_transform.is_nan() {
                return Err(SaveConflict::NonFiniteTransform {
                    kind: "tubi inst_relate",
                    record_id: refno.to_inst_relate_key(),
                }
                .into());
            }
            let aabb = info.aabb.ok_or_else(|| SaveConflict::MissingTubiAabb {
                record_id: refno.to_inst_relate_key(),
            })?;
            let aabb_json = serde_json::to_string(&aabb)?;
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
            insert_unique_record(
                &mut aabbs,
                "aabb",
                format!("aabb:⟨{aabb_hash}⟩"),
                format!("{{'id':aabb:⟨{aabb_hash}⟩, 'd':{aabb_json}}}"),
            )?;
            let transform_json = serde_json::to_string(&info.world_transform)?;
            let transform_hash = gen_bytes_hash::<_, 64>(&info.world_transform);
            insert_unique_record(
                &mut transforms,
                "transform",
                format!("trans:⟨{transform_hash}⟩"),
                format!("{{'id':trans:⟨{transform_hash}⟩, 'd':{transform_json}}}"),
            )?;
            let meta = inst_meta.get(&refno.refno()).unwrap_or(&missing_meta);
            let rendered = format!(
                "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, world_trans_d: {10}, aabb: aabb:⟨{4}⟩, aabb_d: {11}, generic: '{5}', anc: {6}, dbnum: {7}, has_cata_neg: {8}, solid: {9}}}",
                refno.to_inst_relate_key(),
                refno.to_pe_key(),
                info.id_str(),
                transform_hash,
                aabb_hash,
                info.generic_type,
                meta.anc_literal(),
                meta.dbnum_literal(),
                info.has_cata_neg,
                info.is_solid,
                transform_json,
                aabb_json,
            );
            let record_id = refno.to_inst_relate_key();
            insert_unique_record(
                &mut tubi_inst_relates,
                "tubi inst_relate",
                record_id.clone(),
                rendered,
            )?;
            written_by_id.entry(record_id).or_insert(*refno);
        }

        let mut neg_keys = batch.neg_relate_map.keys().collect::<Vec<_>>();
        neg_keys.sort_by_key(|refno| refno.to_string());
        for owner in neg_keys {
            let sequence = batch.neg_relate_map[owner]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let owner_key = owner.to_string();
            match canonical_neg_sequences.get(&owner_key) {
                Some(existing) if existing != &sequence => {
                    return Err(SaveConflict::RecordContent {
                        kind: "neg relation sequence",
                        record_id: owner_key,
                    }
                    .into());
                }
                Some(_) => continue,
                None => {
                    canonical_neg_sequences.insert(owner_key, sequence);
                }
            }
            for (index, related) in batch.neg_relate_map[owner].iter().enumerate() {
                let record_id = format!("neg_relate:[{related},{index}]");
                insert_unique_record(
                    &mut neg_relates,
                    "neg_relate",
                    record_id,
                    format!(
                        "{{ in: {}, id: [{}, {index}], out: {} }}",
                        related.to_pe_key(),
                        related,
                        owner.to_pe_key()
                    ),
                )?;
            }
        }

        let mut ngmr_keys = batch.ngmr_neg_relate_map.keys().collect::<Vec<_>>();
        ngmr_keys.sort_by_key(|refno| refno.to_string());
        for owner in ngmr_keys {
            for (element, geometry) in &batch.ngmr_neg_relate_map[owner] {
                let element_key = element.to_pe_key();
                let owner_key = owner.to_pe_key();
                let geometry_key = geometry.to_pe_key();
                let record_id = format!("ngmr_relate:[{element_key},{owner_key},{geometry_key}]");
                insert_unique_record(
                    &mut ngmr_relates,
                    "ngmr_relate",
                    record_id,
                    format!(
                        "{{ in: {0}, id: [{0}, {1}, {2}], out: {1}, ngmr: {2}}}",
                        element_key, owner_key, geometry_key
                    ),
                )?;
            }
        }
    }

    if let Some(record_id) = normal_inst_relates
        .keys()
        .find(|record_id| tubi_inst_relates.contains_key(*record_id))
    {
        return Err(SaveConflict::NormalTubiOverlap {
            record_id: record_id.clone(),
        }
        .into());
    }

    let has_persistent_rows = !transforms.is_empty()
        || !aabbs.is_empty()
        || !vec3s.is_empty()
        || !inst_geos.is_empty()
        || !inst_infos.is_empty()
        || !geo_relates.is_empty()
        || !neg_relates.is_empty()
        || !ngmr_relates.is_empty()
        || !normal_inst_relates.is_empty()
        || !tubi_inst_relates.is_empty();
    if has_persistent_rows {
        let identity_json = serde_json::to_string(&Transform::IDENTITY)?;
        insert_unique_record(
            &mut transforms,
            "transform",
            "trans:⟨0⟩".into(),
            format!("{{'id':trans:⟨0⟩, 'd':{identity_json}}}"),
        )?;
    }

    let mut packets = Vec::new();
    push_array_packets(
        &mut packets,
        SavePhase::SharedContent,
        "INSERT IGNORE INTO trans [",
        "];",
        &transforms,
    );
    push_array_packets(
        &mut packets,
        SavePhase::SharedContent,
        "INSERT IGNORE INTO aabb [",
        "];",
        &aabbs,
    );
    push_array_packets(
        &mut packets,
        SavePhase::SharedContent,
        "INSERT IGNORE INTO vec3 [",
        "];",
        &vec3s,
    );
    push_statement_packets(&mut packets, SavePhase::SharedContent, &inst_geos);
    push_array_packets(
        &mut packets,
        SavePhase::SharedContent,
        "INSERT IGNORE INTO inst_info [",
        "];",
        &inst_infos,
    );
    if mode.replaces_existing() {
        let mut replacements = BTreeMap::new();
        for (inst_info_id, rows) in &geo_relates_by_inst_info {
            let sql = render_geo_relate_replace(inst_info_id, rows);
            anyhow::ensure!(
                sql.len() <= SQL_PACKET_BYTES,
                "shared CATA identity {inst_info_id} geo_relate replacement is {} bytes, exceeding the {} byte packet limit",
                sql.len(),
                SQL_PACKET_BYTES
            );
            replacements.insert(inst_info_id.clone(), sql);
        }
        push_statement_packets(&mut packets, SavePhase::Relations, &replacements);
    } else {
        push_array_packets(
            &mut packets,
            SavePhase::Relations,
            "INSERT RELATION INTO geo_relate [",
            "];",
            &geo_relates,
        );
    }
    push_array_packets(
        &mut packets,
        SavePhase::Relations,
        "INSERT RELATION INTO neg_relate [",
        "];",
        &neg_relates,
    );
    push_array_packets(
        &mut packets,
        SavePhase::Relations,
        "INSERT RELATION INTO ngmr_relate [",
        "];",
        &ngmr_relates,
    );
    push_inst_relate_packets(&mut packets, &normal_inst_relates);
    push_inst_relate_packets(&mut packets, &tubi_inst_relates);

    let written_refnos = written_by_id.values().copied().collect::<Vec<_>>();
    let delete_refnos = if mode.replaces_existing() {
        written_refnos.clone()
    } else {
        Vec::new()
    };
    Ok(SavePlan {
        mode,
        flush_reason,
        source_batch_count: frozen.source_batch_count(),
        instance_rows: frozen.instance_rows(),
        geo_occurrences: frozen.geo_occurrences(),
        coalesce_wait,
        delete_refnos,
        written_refnos,
        packets,
        metadata_query_count,
        conflict_count: 0,
    })
}

pub(crate) async fn build_save_plan(
    frozen: &FrozenShapeBatch,
    mode: SaveMode,
    flush_reason: FlushReason,
    coalesce_wait: Duration,
) -> anyhow::Result<SavePlan> {
    let mut refnos_by_id = BTreeMap::new();
    for batch in frozen.batches() {
        for refno in batch.inst_info_map.keys().chain(batch.inst_tubi_map.keys()) {
            refnos_by_id.entry(refno.to_string()).or_insert(*refno);
        }
    }
    let refnos = refnos_by_id.values().copied().collect::<Vec<_>>();
    let metadata_query_count = usize::from(!refnos.is_empty());
    let inst_meta = resolve_inst_meta(&refnos).await?;
    build_canonical_save_plan(
        frozen,
        mode,
        flush_reason,
        coalesce_wait,
        &inst_meta,
        metadata_query_count,
    )
}

async fn execute_packet(packet: &SqlPacket) -> anyhow::Result<()> {
    crate::surreal_retry::execute_model_write(&packet.sql, "save instance data").await
}

pub(crate) async fn execute_save_plan(plan: SavePlan) -> anyhow::Result<SaveOutcome> {
    if plan.mode.replaces_existing() && !plan.delete_refnos.is_empty() {
        delete_inst_relate_cascade(&plan.delete_refnos, SQL_PACKET_ROWS).await?;
    }

    if crate::data_interface::staging::active_staging_writes().is_some() {
        for packet in &plan.packets {
            execute_packet(packet).await?;
        }
    } else {
        // 阶段之间严格有序；只在同一阶段内部放最多四个 packet 并发。
        for phase in [
            SavePhase::SharedContent,
            SavePhase::Relations,
            SavePhase::InstanceRelations,
        ] {
            let phase_packets = plan
                .packets
                .iter()
                .filter(|packet| packet.phase == phase)
                .collect::<Vec<_>>();
            for packet_group in phase_packets.chunks(DIRECT_MAX_IN_FLIGHT) {
                let mut handles = Vec::with_capacity(packet_group.len());
                for packet in packet_group {
                    let packet = (*packet).clone();
                    handles.push(
                        crate::data_interface::staging::write_context::spawn_with_staged_io(
                            async move { execute_packet(&packet).await },
                        ),
                    );
                }
                let mut first_error = None;
                for handle in handles {
                    let error = match handle.await {
                        Ok(Ok(())) => continue,
                        Ok(Err(error)) => error,
                        Err(error) => anyhow::anyhow!("instance write task failed: {error}"),
                    };
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
                if let Some(error) = first_error {
                    return Err(error);
                }
            }
        }
    }

    if !plan.packets.is_empty() {
        mark_insts_flat_dirty();
    }
    Ok(SaveOutcome {
        written_refnos: plan.written_refnos,
        source_batch_count: plan.source_batch_count,
        flush_reason: plan.flush_reason,
        instance_rows: plan.instance_rows,
        geo_occurrences: plan.geo_occurrences,
        coalesce_wait: plan.coalesce_wait,
        metadata_query_count: plan.metadata_query_count,
        sql_packet_count: plan.packets.len(),
        sql_bytes: plan
            .packets
            .iter()
            .map(|packet| packet.estimated_bytes)
            .sum(),
        scoped_delete_count: plan.delete_refnos.len(),
        conflict_count: plan.conflict_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::geometry::{EleGeosInfo, EleInstGeosData, ShapeInstancesData};
    use aios_core::parsed_data::{CateProfileParam, SRectData};
    use aios_core::prim_geo::spine::{Line3D, SweepPath3D};
    use aios_core::prim_geo::{LSnout, SCylinder, SweepSolid};
    use aios_core::shape::pdms_shape::BrepShapeTrait;
    use futures::stream::FuturesUnordered;
    use glam::{DVec3, Vec2, Vec3};
    use parry3d::bounding_volume::Aabb;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn flat_cache_prefers_booled_mesh_over_positive_primitives() {
        let source = include_str!("pdms_inst.rs");
        let body = source
            .split_once("pub async fn sweep_inst_relate_flat()")
            .expect("flat sweep exists")
            .1
            .split_once("pub async fn sweep_inst_relate_flat_if_dirty()")
            .expect("flat sweep boundary")
            .0;
        assert!(body.contains("IF {VALID_BOOLED} THEN"), "{body}");
        assert!(body.contains("geo_hash: booled_id"), "{body}");
    }

    /// 回填段、修复段与启动探针必须引用**同一份**「有没有布尔成品」/「与成品
    /// 不符」的判据。
    ///
    /// 两处曾经各写各的：回填只判 `booled_id != NONE`，而空串不是 NONE——于是
    /// `booled_id = ''` 的行被回填成 `insts_flat = [{ geo_hash: '' }]`，与同一函数
    /// 下面两段「空串按无成品处理、平表不得改写」的定义正相反。回退到分别写死谓词
    /// 时本测试必红。
    #[test]
    fn both_sweep_segments_share_one_valid_booled_predicate() {
        assert!(VALID_BOOLED.contains("booled_id != NONE"), "{VALID_BOOLED}");
        assert!(VALID_BOOLED.contains("booled_id != ''"), "{VALID_BOOLED}");
        assert!(
            VALID_BOOLED.contains("string::lowercase(booled_id ?? '') != 'none'"),
            "{VALID_BOOLED}"
        );
        assert!(
            BOOLED_FLAT_MISMATCH.contains("insts_flat = NONE"),
            "{BOOLED_FLAT_MISMATCH}"
        );
        assert!(
            BOOLED_FLAT_MISMATCH.contains("array::first(insts_flat ?? []).geo_hash != booled_id"),
            "mismatch 判据必须圈「与成品不符」，且 array::first 实参必须 `?? []` 兜底\
             （生产服对 NONE 实参报错）：{BOOLED_FLAT_MISMATCH}"
        );

        let source = include_str!("pdms_inst.rs");
        let sweep_body = source
            .split_once("pub async fn sweep_inst_relate_flat()")
            .expect("flat sweep exists")
            .1
            .split_once("pub async fn sweep_inst_relate_flat_if_dirty()")
            .expect("flat sweep boundary")
            .0;
        assert_eq!(
            sweep_body.matches("{VALID_BOOLED}").count(),
            1,
            "回填分支引用一次共享判据（修复段已迁去 migration，FR-9）: {sweep_body}"
        );
        let migration_body = source
            .split_once("pub(crate) async fn run_booled_flat_repair_migration_on(")
            .expect("repair migration exists")
            .1
            .split_once("pub async fn probe_booled_flat_regression_after_migration()")
            .expect("repair migration boundary")
            .0;
        assert_eq!(
            migration_body.matches("{VALID_BOOLED}").count(),
            2,
            "修复循环与复核各引用一次共享判据，不得各写各的: {migration_body}"
        );
        assert_eq!(
            migration_body.matches("{BOOLED_FLAT_MISMATCH}").count(),
            2,
            "修复循环与复核各引用一次共享 mismatch 判据，不得各写各的: {migration_body}"
        );
        let probe_body = source
            .split_once("pub async fn probe_booled_flat_regression_after_migration()")
            .expect("startup probe exists")
            .1
            .split_once("pub(crate) struct ResolvedInstMeta")
            .expect("startup probe boundary")
            .0;
        assert_eq!(
            probe_body.matches("{VALID_BOOLED}").count(),
            1,
            "启动探针引用一次共享判据: {probe_body}"
        );
        assert_eq!(
            probe_body.matches("{BOOLED_FLAT_MISMATCH}").count(),
            1,
            "启动探针引用一次共享 mismatch 判据: {probe_body}"
        );
        for body in [sweep_body, migration_body, probe_body] {
            assert!(
                !body.contains("IF booled_id != NONE THEN"),
                "不得退回「只判 NONE」的旧写法: {body}"
            );
        }
    }

    /// 脏值探测不许在空闲轮里做无界全表扫。
    ///
    /// 这段谓词在 `inst_relate` 上不可索引，而它唯一的产出是一行警告。原写法把全表
    /// id 物化出来再 `array::len`，每个空闲轮付一次；改成只探有无。
    #[test]
    fn the_junk_probe_is_bounded() {
        let source = include_str!("pdms_inst.rs");
        let body = source
            .split_once("pub async fn sweep_inst_relate_flat()")
            .expect("flat sweep exists")
            .1
            .split_once("pub async fn sweep_inst_relate_flat_if_dirty()")
            .expect("flat sweep boundary")
            .0;
        assert!(
            body.contains("= 'none') LIMIT 1))"),
            "脏值探测必须带 LIMIT: {body}"
        );
        // 这一段是本文件里唯一一个 `OR` 谓词。限定域前缀拼在 `WHERE` 之后，若不把
        // `OR` 括起来，`AND` 结合更紧——谓词会变成「域内的空串 或 全库的字面
        // 'none'」，收窄掉了一半、另一半照旧全表扫，而且两种写法都能跑、都不报错。
        assert!(
            body.contains("WHERE {scoped}(booled_id = ''"),
            "脏值探测的 OR 必须整体括起来，否则限定域前缀只收窄左边那一支: {body}"
        );
    }

    /// 限定域前缀的渲染（ADR-048 决策 2 在缓存维护侧的延伸）。
    ///
    /// 三件事：未限定时一个字都不多（判定逐位不变）、单库走等值、多库走 `IN`。
    /// 最后一条最要紧——**列名必须裸着出现**。隔壁 `model_update_pending` 写的是
    /// `(dbnum?:0) IN […]`，那张表没有索引所以随便写；照抄到这里就把
    /// `idx_inst_relate_dbnum` 藏进了一个表达式里，planner 回落全表，改了等于没改，
    /// 而且日志上看不出任何区别。
    #[test]
    fn the_watch_scope_filter_keeps_the_indexed_column_bare() {
        assert_eq!(render_watch_scope_filter(&[]), "", "未限定时不得加谓词");
        assert_eq!(render_watch_scope_filter(&[30999]), "dbnum = 30999 AND ");
        assert_eq!(
            render_watch_scope_filter(&[7998, 8000]),
            "dbnum IN [7998, 8000] AND "
        );
        for rendered in [
            render_watch_scope_filter(&[30999]),
            render_watch_scope_filter(&[7998, 8000]),
        ] {
            assert!(
                !rendered.contains("?:") && !rendered.contains("dbnum?"),
                "限定域谓词不得把索引列包进缺值兜底表达式，那会让 planner 回落全表: {rendered}"
            );
            assert!(
                rendered.ends_with(" AND "),
                "前缀形态：拼在 WHERE 之后、真谓词之前: {rendered}"
            );
        }
    }

    /// `inst_relate` 上每一条全表谓词都必须挂上限定域前缀。
    ///
    /// 漏掉任意一条，这个进程的启动序列就仍然会在那一条上把整表走完——而其余几条
    /// 都变快了，日志上只剩「某一步很慢」，比全都慢更难定位。四条：回填圈行、
    /// 脏值探测、修复循环、修复复核，外加启动探针那条。
    #[test]
    fn every_full_table_predicate_carries_the_watch_scope_filter() {
        let source = include_str!("pdms_inst.rs");
        let sweep_body = source
            .split_once("pub async fn sweep_inst_relate_flat()")
            .expect("flat sweep exists")
            .1
            .split_once("pub async fn sweep_inst_relate_flat_if_dirty()")
            .expect("flat sweep boundary")
            .0;
        assert!(
            sweep_body.contains("WHERE {scoped}insts_flat = NONE"),
            "回填圈行必须带限定域前缀: {sweep_body}"
        );
        assert_eq!(
            sweep_body.matches("FROM inst_relate").count(),
            sweep_body.matches("{scoped}").count(),
            "清扫里每一条 inst_relate 谓词都要带前缀，一条都不能漏: {sweep_body}"
        );

        let migration_body = source
            .split_once("pub(crate) async fn run_booled_flat_repair_migration_on(")
            .expect("repair migration exists")
            .1
            .split_once("pub async fn probe_booled_flat_regression_after_migration()")
            .expect("repair migration boundary")
            .0;
        assert_eq!(
            migration_body.matches("FROM inst_relate").count(),
            migration_body.matches("{scoped}").count(),
            "修复循环与复核都要带前缀: {migration_body}"
        );

        let probe_body = source
            .split_once("pub(crate) async fn probe_booled_flat_regression_after_migration_on(")
            .expect("startup probe exists")
            .1
            .split_once("pub(crate) struct ResolvedInstMeta")
            .expect("startup probe boundary")
            .0;
        assert_eq!(
            probe_body.matches("FROM inst_relate").count(),
            probe_body.matches("{scoped}").count(),
            "启动探针也要带前缀: {probe_body}"
        );
    }

    /// Spec 019 FR-001/FR-003/FR-006：修复段必须存在——旧代码时代「先回填正体、
    /// 后写 booled_id」的行 `insts_flat` 是错值而非缺值，只圈 NONE 的清扫永远
    /// 修不到它们。删掉修复段或把清扫与 migration 的调用关系断开，本测试必须红。
    #[test]
    fn flat_sweep_repairs_stale_booled_rows() {
        let source = include_str!("pdms_inst.rs");
        let sweep_body = source
            .split_once("pub async fn sweep_inst_relate_flat()")
            .expect("flat sweep exists")
            .1
            .split_once("pub async fn sweep_inst_relate_flat_if_dirty()")
            .expect("flat sweep boundary")
            .0;
        assert!(
            sweep_body.contains("run_booled_flat_repair_migration_on(&SUL_DB, &scope)"),
            "清扫必须仍然驱动修复 migration（改制不等于删除），且把本轮的监听限定域\
             原样传下去——两段各自解析一次限定域，就有了两个可能不一致的答案：{sweep_body}"
        );
        let body = source
            .split_once("pub(crate) async fn run_booled_flat_repair_migration_on(")
            .expect("repair migration exists")
            .1
            .split_once("pub async fn probe_booled_flat_regression_after_migration()")
            .expect("repair migration boundary")
            .0;
        assert!(
            body.contains("AND ({BOOLED_FLAT_MISMATCH})"),
            "修复段必须圈「booled_id 有值而 insts_flat 与之不符」的行\
             （共享判据 BOOLED_FLAT_MISMATCH，内容由
             both_sweep_segments_share_one_valid_booled_predicate 钉住）：{body}"
        );
        assert!(
            body.contains("SET insts_flat = [{{ geo_hash: booled_id }}]"),
            "修复段必须把脏行改写为单位变换成品单实例：{body}"
        );
        // 空串/字面 'none' 的排除已收进共享判据 [`VALID_BOOLED`]，由
        // `both_sweep_segments_share_one_valid_booled_predicate` 单独钉住；这里只确认
        // 修复段确实引用了它，而不是自己又写了一份。
        assert!(
            body.contains("WHERE {scoped}{VALID_BOOLED}"),
            "修复段必须引用共享判据，不得自己重写空串/'none' 的排除：{body}"
        );
    }

    /// Spec 025 FR-9 的流程顺序钉（源码顺序断言，仓内惯例）：
    /// **标记点查 → 修复循环 → 复核无残留 → 落标记**，四步在函数体里的文本顺序
    /// 不得颠倒。尤其是「落标记」必须在「复核」之后——先落标记再修复的写法会把
    /// 「跑了一半死掉」永久判成「跑完了」，正是播种标记当年防的那类缺陷。
    #[test]
    fn booled_flat_repair_migration_marks_only_after_a_clean_recheck() {
        let source = include_str!("pdms_inst.rs");
        let body = source
            .split_once("pub(crate) async fn run_booled_flat_repair_migration_on(")
            .expect("repair migration exists")
            .1
            .split_once("pub async fn probe_booled_flat_regression_after_migration()")
            .expect("repair migration boundary")
            .0;
        let marker_probe = body
            .find("SELECT VALUE id FROM {BOOLED_FLAT_REPAIR_MARKER}")
            .expect("第一步必须是标记点查");
        let repair = body
            .find("UPDATE $rows SET insts_flat = [{{ geo_hash: booled_id }}]")
            .expect("第二步必须是修复循环");
        let recheck = body
            .find("LIMIT 1)) > 0")
            .expect("第三步必须是复核无残留（只探有无）");
        let mark = body
            .find("UPSERT {BOOLED_FLAT_REPAIR_MARKER}")
            .expect("第四步必须是落标记");
        assert!(
            marker_probe < repair && repair < recheck && recheck < mark,
            "FR-9 流程顺序被打乱：probe={marker_probe} repair={repair} \
             recheck={recheck} mark={mark}"
        );
        assert!(
            body.contains("return Ok(repaired);"),
            "复核有残留时必须提前返回、不落标记：{body}"
        );
    }

    /// Spec 025 FR-9 的行为钉（mem，进 CI）：
    /// 1. 无标记 + 有脏行 → 修复、复核干净、落标记；
    /// 2. 有标记 + 新脏行 → **跳过**（判据是库上的标记，不是本进程状态）；
    /// 3. 标记被删（旧备份恢复的形态）→ 自动重跑，修掉那批行。
    #[tokio::test(flavor = "multi_thread")]
    async fn booled_flat_repair_migration_marks_once_and_reruns_when_the_marker_vanishes() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("pdms_inst")
            .use_db("booled_repair_migration")
            .await
            .expect("use db");
        db.query(
            "CREATE pe:24379_300 SET noun='SITE', dbnum=7997; CREATE inst_info:zzmig; \
             INSERT RELATION INTO inst_relate [\
                { id: inst_relate:⟨24379_300⟩, in: pe:24379_300, out: inst_info:zzmig, dbnum: 7997, \
                  booled_id: 'b9', insts_flat: [{ geo_hash: 'pos1' }] }, \
                { id: inst_relate:⟨24379_301⟩, in: pe:24379_300, out: inst_info:zzmig, dbnum: 7997, \
                  booled_id: '', insts_flat: [{ geo_hash: 'pos2' }] }, \
                { id: inst_relate:⟨24379_302⟩, in: pe:24379_300, out: inst_info:zzmig, dbnum: 7997, \
                  booled_id: 'b6', insts_flat: [{ geo_hash: 'b6' }] }];",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seed dirty rows");

        // 1) 无标记：修复 300，302 幂等不圈，301 空串按无成品不动；落标记。
        let repaired = run_booled_flat_repair_migration_on(&db, &[])
            .await
            .expect("first migration run");
        assert_eq!(repaired, 1, "首轮应恰好修复 booled_id='b9' 那一行");
        let mut response = db
            .query(
                "RETURN [inst_relate:⟨24379_300⟩.insts_flat.geo_hash = ['b9'], \
                         inst_relate:⟨24379_301⟩.insts_flat.geo_hash = ['pos2'], \
                         array::len((SELECT completed_at FROM queue_control:booled_flat_repair_migration)) = 1];",
            )
            .await
            .expect("verify transport")
            .check()
            .expect("verify query");
        let flags: Vec<bool> = response.take(0).expect("take flags");
        assert_eq!(
            flags,
            vec![true, true, true],
            "修复后成品单实例、脏值行原样、标记在库"
        );

        // 2) 有标记 + 新脏行：跳过（判据是库上的标记）。
        db.query(
            "INSERT RELATION INTO inst_relate [\
                { id: inst_relate:⟨24379_303⟩, in: pe:24379_300, out: inst_info:zzmig, dbnum: 7997, \
                  booled_id: 'b5', insts_flat: [] }];",
        )
        .await
        .expect("late row transport")
        .check()
        .expect("seed late dirty row");
        let repaired = run_booled_flat_repair_migration_on(&db, &[])
            .await
            .expect("second migration run");
        assert_eq!(repaired, 0, "标记在库时必须整段跳过");
        let mut response = db
            .query("RETURN inst_relate:⟨24379_303⟩.insts_flat = [];")
            .await
            .expect("skip verify transport")
            .check()
            .expect("skip verify query");
        let untouched: Option<bool> = response.take(0).expect("take untouched");
        assert_eq!(untouched, Some(true), "标记在库时新脏行不得被触碰");

        // 3) 标记消失（旧备份恢复的形态）：自动重跑并修掉迟到的脏行。
        db.query("DELETE queue_control:booled_flat_repair_migration;")
            .await
            .expect("drop marker transport")
            .check()
            .expect("drop marker");
        let repaired = run_booled_flat_repair_migration_on(&db, &[])
            .await
            .expect("third migration run");
        assert_eq!(repaired, 1, "标记消失后必须从头重跑并修复迟到的脏行");
        let mut response = db
            .query("RETURN inst_relate:⟨24379_303⟩.insts_flat.geo_hash = ['b5'];")
            .await
            .expect("rerun verify transport")
            .check()
            .expect("rerun verify query");
        let fixed: Option<bool> = response.take(0).expect("take fixed");
        assert_eq!(fixed, Some(true), "重跑必须修复迟到的脏行");
    }

    /// 收窄跑过的 migration **不许落标记**（mem，进 CI）。
    ///
    /// 标记的含义是「这一库全表收敛过」。限定域内跑干净担保不了这句话，而标记一旦
    /// 落下，`run_booled_flat_repair_migration_on` 与启动探针都按「已收敛」处理——
    /// 域外那批 RM13 老格式行从此无人问津，读侧继续把带原语缩放的错误正体端给
    /// 查看者，且没有任何一行日志会提到它们。这正是 FR-9 把判据从「本进程跑过一次」
    /// 搬到库上标记时要防的东西，收窄是它的一个新入口。
    ///
    /// 把「收窄时也落标记」写回去，本测试必红。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_narrowed_repair_run_fixes_only_its_scope_and_refuses_the_marker() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("pdms_inst")
            .use_db("booled_repair_scoped")
            .await
            .expect("use db");
        db.query(
            "CREATE pe:24379_400 SET noun='SITE'; CREATE inst_info:zzscope; \
             INSERT RELATION INTO inst_relate [\
                { id: inst_relate:⟨24379_400⟩, in: pe:24379_400, out: inst_info:zzscope, dbnum: 7997, \
                  booled_id: 'in7997', insts_flat: [{ geo_hash: 'stale' }] }, \
                { id: inst_relate:⟨24379_401⟩, in: pe:24379_400, out: inst_info:zzscope, dbnum: 8000, \
                  booled_id: 'in8000', insts_flat: [{ geo_hash: 'stale' }] }];",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seed two dbnums");

        let repaired = run_booled_flat_repair_migration_on(&db, &[7997])
            .await
            .expect("scoped migration run");
        assert_eq!(repaired, 1, "收窄时只该修域内那一行");
        let mut response = db
            .query(
                "RETURN inst_relate:⟨24379_400⟩.insts_flat.geo_hash = ['in7997']; \
                 RETURN inst_relate:⟨24379_401⟩.insts_flat.geo_hash = ['stale']; \
                 RETURN array::len((SELECT VALUE id FROM queue_control:booled_flat_repair_migration));",
            )
            .await
            .expect("verify transport")
            .check()
            .expect("verify query");
        let in_scope_fixed: Option<bool> = response.take(0).expect("take in-scope");
        let out_of_scope_untouched: Option<bool> = response.take(1).expect("take out-of-scope");
        let marker_rows: Option<i64> = response.take(2).expect("take marker");
        assert_eq!(in_scope_fixed, Some(true), "域内的脏行必须被修好");
        assert_eq!(
            out_of_scope_untouched,
            Some(true),
            "域外的脏行不得被这一轮触碰"
        );
        assert_eq!(
            marker_rows,
            Some(0),
            "收窄跑过的一轮不得落完成标记——落了，域外那批行就永远没人再看"
        );

        // 同一个库改跑全库：域外那行被补上，这时才允许落标记。
        let repaired = run_booled_flat_repair_migration_on(&db, &[])
            .await
            .expect("full migration run");
        assert_eq!(repaired, 1, "全库一轮补上域外那行");
        let mut response = db
            .query(
                "RETURN [inst_relate:⟨24379_401⟩.insts_flat.geo_hash = ['in8000'], \
                         array::len((SELECT VALUE id FROM queue_control:booled_flat_repair_migration)) = 1];",
            )
            .await
            .expect("full verify transport")
            .check()
            .expect("full verify query");
        let flags: Vec<bool> = response.take(0).expect("take flags");
        assert_eq!(
            flags,
            vec![true, true],
            "全库收敛之后域外行修好、标记才落下"
        );
    }

    /// 启动探针的接线钉（源码形状断言，FR-9 盲区补口 × FR-1）：
    /// 1. 启动序列（lib.rs）必须在清扫**之后**调用探针——migration 先有机会落标记/
    ///    报残留，探针只管「标记已落还冒老格式」的那一种；
    /// 2. 空闲轮（batch_worker.rs）不得出现探针——谓词无索引，FR-1 全表扫只许
    ///    启动序列与人工诊断入口，`LIMIT 1` 不豁免；
    /// 3. 探针体内标记点查必须先于 mismatch 全表谓词：标记未落时提前返回，
    ///    不多付一次全表扫。
    #[test]
    fn the_stale_writer_probe_is_startup_only_and_checks_the_marker_first() {
        let startup = include_str!("../lib.rs");
        let sweep_at = startup
            .find("sweep_inst_relate_flat()")
            .expect("启动序列必须调用清扫");
        let probe_at = startup
            .find("probe_booled_flat_regression_after_migration()")
            .expect("启动序列必须调用老格式再现探针（FR-9 盲区补口）");
        assert!(
            sweep_at < probe_at,
            "探针必须在清扫之后：sweep={sweep_at} probe={probe_at}"
        );

        let idle = include_str!("../data_interface/batch_worker.rs");
        assert!(
            !idle.contains("probe_booled_flat_regression_after_migration"),
            "探针的谓词无索引，不得进空闲轮（FR-1）"
        );

        let source = include_str!("pdms_inst.rs");
        let probe_body = source
            .split_once("pub(crate) async fn probe_booled_flat_regression_after_migration_on(")
            .expect("startup probe exists")
            .1
            .split_once("pub(crate) struct ResolvedInstMeta")
            .expect("startup probe boundary")
            .0;
        let marker_probe = probe_body
            .find("SELECT VALUE id FROM {BOOLED_FLAT_REPAIR_MARKER}")
            .expect("探针第一步必须是标记点查");
        let early_exit = probe_body
            .find("return Ok(None);")
            .expect("标记未落必须提前返回，不付全表谓词");
        let mismatch_probe = probe_body
            .find("{BOOLED_FLAT_MISMATCH}")
            .expect("探针必须引用共享 mismatch 判据");
        assert!(
            marker_probe < early_exit && early_exit < mismatch_probe,
            "探针顺序被打乱：marker={marker_probe} early_exit={early_exit} \
             mismatch={mismatch_probe}"
        );
    }

    /// 启动探针的行为钉（mem，进 CI）：
    /// 1. 标记未落 → 不探直接返回（migration 自己会跑并报告）；
    /// 2. 标记已落 + 老格式行（旧 writer 混跑的形态）→ 报告再现；
    /// 3. 行修好 → 干净。
    #[tokio::test(flavor = "multi_thread")]
    async fn the_startup_probe_stays_silent_without_a_marker_and_shouts_on_regression() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("pdms_inst")
            .use_db("booled_repair_probe")
            .await
            .expect("use db");
        db.query(
            "CREATE pe:24379_400 SET noun='SITE', dbnum=7997; CREATE inst_info:zzprobe; \
             INSERT RELATION INTO inst_relate [\
                { id: inst_relate:⟨24379_400⟩, in: pe:24379_400, out: inst_info:zzprobe, \
                  dbnum: 7997, booled_id: 'b9', insts_flat: [{ geo_hash: 'pos1' }] }];",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seed stale-writer row");

        // 1) 标记未落：不探（这批行归 migration 收，探针不重复报告）。
        let probed = probe_booled_flat_regression_after_migration_on(&db, &[])
            .await
            .expect("probe without marker");
        assert_eq!(probed, None, "标记未落时探针必须不探直接返回");

        // 2) 标记已落 + 老格式行：旧 writer 混跑的形态，必须报告。
        db.query(
            "UPSERT queue_control:booled_flat_repair_migration SET spec = 'test', \
             repaired = 0, completed_at = time::now();",
        )
        .await
        .expect("marker transport")
        .check()
        .expect("land marker");
        let probed = probe_booled_flat_regression_after_migration_on(&db, &[])
            .await
            .expect("probe with marker");
        assert_eq!(
            probed,
            Some(true),
            "标记已落而老格式行在库，探针必须报告再现"
        );

        // 3) 行修好：干净。
        db.query("UPDATE inst_relate:⟨24379_400⟩ SET insts_flat = [{ geo_hash: 'b9' }];")
            .await
            .expect("fix transport")
            .check()
            .expect("fix row");
        let probed = probe_booled_flat_regression_after_migration_on(&db, &[])
            .await
            .expect("probe clean");
        assert_eq!(probed, Some(false), "库干净时探针必须回 Some(false)");
    }

    fn normal_batch(refno_text: &str, visible: bool) -> ShapeInstancesData {
        let refno = RefnoEnum::from(refno_text);
        let mut info = EleGeosInfo::default();
        info.refno = refno;
        info.sesno = 1;
        info.visible = visible;
        info.world_transform = Transform::IDENTITY;
        let mut batch = ShapeInstancesData::default();
        batch.inst_info_map.insert(refno, info);
        batch
    }

    fn plan_for_test(batches: Vec<ShapeInstancesData>) -> anyhow::Result<SavePlan> {
        let frozen = FrozenShapeBatch::from_batches_for_test(batches)?;
        build_canonical_save_plan(
            &frozen,
            SaveMode::TargetedReplace,
            FlushReason::ChannelClosed,
            Duration::ZERO,
            &HashMap::new(),
            0,
        )
    }

    fn plan_sql(plan: &SavePlan) -> Vec<(SavePhase, String)> {
        plan.packets
            .iter()
            .map(|packet| (packet.phase, packet.sql.clone()))
            .collect()
    }

    #[test]
    fn equal_record_content_is_deduplicated_but_different_content_fails_closed() {
        let equal = plan_for_test(vec![
            normal_batch("1/101", true),
            normal_batch("1/101", true),
        ])
        .expect("equal rows deduplicate");
        assert_eq!(equal.written_refnos.len(), 1);

        let error = plan_for_test(vec![
            normal_batch("1/101", true),
            normal_batch("1/101", false),
        ])
        .expect_err("same record id with different content must fail");
        assert!(error.downcast_ref::<SaveConflict>().is_some(), "{error:#}");
    }

    #[test]
    fn normal_tubi_overlap_and_nan_fail_before_a_plan_can_execute() {
        let refno = RefnoEnum::from("1/102");
        let mut overlap = normal_batch("1/102", true);
        let mut tubi = EleGeosInfo::default();
        tubi.refno = refno;
        tubi.sesno = 1;
        tubi.world_transform = Transform::IDENTITY;
        tubi.aabb = Some(Aabb::new(Vec3::ZERO.into(), Vec3::ONE.into()));
        overlap.inst_tubi_map.insert(refno, tubi);
        let error = plan_for_test(vec![overlap]).expect_err("overlap must fail");
        assert!(
            matches!(
                error.downcast_ref::<SaveConflict>(),
                Some(SaveConflict::NormalTubiOverlap { .. })
            ),
            "{error:#}"
        );

        let mut nan = normal_batch("1/103", true);
        nan.inst_info_map
            .get_mut(&RefnoEnum::from("1/103"))
            .expect("fixture row")
            .world_transform
            .translation
            .x = f32::NAN;
        let error = plan_for_test(vec![nan]).expect_err("NaN must fail");
        assert!(
            matches!(
                error.downcast_ref::<SaveConflict>(),
                Some(SaveConflict::NonFiniteTransform { .. })
            ),
            "{error:#}"
        );
    }

    #[test]
    fn negative_relation_sequence_is_not_sorted_or_last_wins() {
        let owner = RefnoEnum::from("1/110");
        let a = RefnoEnum::from("1/111");
        let b = RefnoEnum::from("1/112");
        let mut first = ShapeInstancesData::default();
        first.neg_relate_map.insert(owner, vec![a, b]);
        let mut second = ShapeInstancesData::default();
        second.neg_relate_map.insert(owner, vec![b, a]);
        let error = plan_for_test(vec![first, second]).expect_err("order conflict must fail");
        assert!(error.downcast_ref::<SaveConflict>().is_some(), "{error:#}");
    }

    fn cylinder_batch(param: PdmsGeoParam) -> ShapeInstancesData {
        let refno = RefnoEnum::from("1/120");
        let inst = EleInstGeo {
            geo_hash: aios_core::prim_geo::basic::CYLINDER_GEO_HASH,
            refno,
            geo_param: param,
            transform: Transform::IDENTITY,
            visible: true,
            ..Default::default()
        };
        let data = EleInstGeosData {
            inst_key: "shared-cylinder".into(),
            refno,
            insts: vec![inst],
            type_name: "CYLI".into(),
            ..Default::default()
        };
        let mut batch = ShapeInstancesData::default();
        batch.inst_geos_map.insert("shared-cylinder".into(), data);
        batch
    }

    #[test]
    fn shared_cylinder_id_has_one_canonical_single_variant_param() {
        let plan = plan_for_test(vec![
            cylinder_batch(PdmsGeoParam::PrimLCylinder(LCylinder::default())),
            cylinder_batch(PdmsGeoParam::PrimSCylinder(SCylinder::default())),
        ])
        .expect("shared unit cylinder variants normalize");
        let sql = plan
            .packets
            .iter()
            .map(|packet| packet.sql.as_str())
            .join("\n");
        assert_eq!(sql.matches("UPSERT inst_geo:⟨2⟩").count(), 1, "{sql}");
        assert!(sql.contains("PrimLCylinder"), "{sql}");
        assert!(!sql.contains("PrimSCylinder"), "{sql}");
        assert!(!sql.contains("MERGE"), "{sql}");
    }

    fn snout_batch(param: LSnout, refno_text: &str) -> ShapeInstancesData {
        let refno = RefnoEnum::from(refno_text);
        let geo_hash = param.hash_unit_mesh_params();
        let inst = EleInstGeo {
            geo_hash,
            refno,
            geo_param: PdmsGeoParam::PrimLSnout(param),
            transform: Transform::IDENTITY,
            visible: true,
            ..Default::default()
        };
        let data = EleInstGeosData {
            inst_key: refno_text.into(),
            refno,
            insts: vec![inst],
            type_name: "PrimLSnout".into(),
            ..Default::default()
        };
        let mut batch = ShapeInstancesData::default();
        batch.inst_geos_map.insert(refno_text.into(), data);
        batch
    }

    #[test]
    fn rounded_equal_snout_hashes_produce_one_canonical_param() {
        let from_ratio = LSnout {
            ptdi: 1.0,
            pbdi: 0.0,
            ptdm: 5.0,
            pbdm: 9.0,
            ..Default::default()
        };
        let copied_rounded = LSnout {
            ptdi: 1.0,
            pbdi: 0.0,
            ptdm: 0.555_555_5,
            pbdm: 1.0,
            ..Default::default()
        };
        assert_eq!(
            from_ratio.hash_unit_mesh_params(),
            copied_rounded.hash_unit_mesh_params()
        );

        let plan = plan_for_test(vec![
            snout_batch(from_ratio, "1/125"),
            snout_batch(copied_rounded, "1/126"),
        ])
        .expect("hash-equal snouts must persist one canonical inst_geo row");
        let sql = plan
            .packets
            .iter()
            .map(|packet| packet.sql.as_str())
            .join("\n");
        assert_eq!(sql.matches("\"ptdm\":0.556").count(), 1, "{sql}");
    }

    use crate::fast_model::libgm_discretise::{self, FACET_TOL_MM};

    // ─── T041 身份键带段数：五类复用曲面原语的门 ────────────────────────────
    //
    // **这一组里带 `t041_` 前缀的都还是红的，红得是对的**——段数今天根本没进
    // `hash_unit_mesh_params()`。它们是 T041 的验收判据先落地，实现随后；
    // 每一条的红都写在 `specs/009-retire-occ/tasks.md` 的「既有红测」里。
    //
    // 判据的形状统一是「**同一个形状比例、不同绝对尺寸**」：单位行的半径恒为 1，
    // 真实尺寸只在实例变换的 `scale` 里，所以只有这种配对才问得出「段数有没有进键」。
    // 拿两个不同比例的件对比是问不出来的——它们的键本来就不同。
    //
    // 期望段数全部**手算自 `libgm_discretise` 的规则 + `FACET_TOL_MM = 0.5`**，
    // 不是从实现反取的；每条测试先自检这些数，规则改了先在自检这一行红。

    fn dish_of(dia: f32, height_ratio: f32, prad_ratio: f32) -> aios_core::prim_geo::Dish {
        aios_core::prim_geo::Dish {
            pdia: dia,
            pheig: dia * height_ratio,
            prad: dia * prad_ratio,
            ..Default::default()
        }
    }

    fn ctorus_of(rout: f32, ratio: f32, angle: f32) -> aios_core::prim_geo::CTorus {
        aios_core::prim_geo::CTorus {
            rins: rout * ratio,
            rout,
            angle,
        }
    }

    fn rtorus_of(rout: f32, height: f32) -> aios_core::prim_geo::RTorus {
        aios_core::prim_geo::RTorus {
            rins: rout * 0.5,
            rout,
            height,
            angle: 90.0,
        }
    }

    fn snout_of(pbdm: f32) -> LSnout {
        LSnout {
            ptdi: 0.5,
            pbdi: -0.5,
            pbdm,
            ptdm: pbdm * 0.5,
            poff: 0.0,
            ..Default::default()
        }
    }

    /// A1·柱：跨段数等价类必须分行。r=100 是 32 段、r=295 是 56 段。
    ///
    /// 今天 `Cylinder::hash_unit_mesh_params()` 恒返回 `CYLINDER_GEO_HASH`，
    /// 一根 6mm 的螺栓杆和一根 590mm 的立柱共用同一份 32 段网格——大的那根弦高
    /// 1.42mm，是 `FACET_TOL_MM` 的近三倍。
    #[test]
    fn t041_a1_cylinders_in_different_segment_classes_need_their_own_rows() {
        use aios_core::prim_geo::LCylinder;
        assert_eq!(libgm_discretise::cylinder_segments(100.0, FACET_TOL_MM), 32);
        assert_eq!(libgm_discretise::cylinder_segments(295.0, FACET_TOL_MM), 56);

        let small = LCylinder {
            pdia: 200.0,
            pbdi: -0.5,
            ptdi: 0.5,
            ..Default::default()
        };
        let large = LCylinder {
            pdia: 590.0,
            ..small.clone()
        };
        assert_ne!(
            small.hash_unit_mesh_params(),
            large.hash_unit_mesh_params(),
            "32 段与 56 段的柱不能共用一行网格"
        );
    }

    /// A2·柱：**同**等价类必须仍然共享。r=1 与 r=2 在 0.5mm 容差下都撞 45° 下限、
    /// 都是 8 段。这条比 A1 更要紧——丢了它，T053 数出来的 392→474 会变成
    /// 392→上万，ADR-044 决策 2 的复用就没了。今天是绿的，别让它变红。
    #[test]
    fn t041_a2_two_cylinders_in_one_segment_class_still_share_a_row() {
        use aios_core::prim_geo::LCylinder;
        assert_eq!(libgm_discretise::cylinder_segments(1.0, FACET_TOL_MM), 8);
        assert_eq!(libgm_discretise::cylinder_segments(2.0, FACET_TOL_MM), 8);

        let a = LCylinder {
            pdia: 2.0,
            pbdi: -0.5,
            ptdi: 0.5,
            ..Default::default()
        };
        let b = LCylinder {
            pdia: 4.0,
            ..a.clone()
        };
        assert_eq!(
            a.hash_unit_mesh_params(),
            b.hash_unit_mesh_params(),
            "同为 8 段的两根柱必须共享一行，否则复用垮掉"
        );
    }

    /// B1·碟的三元组不能只混绕轴。同一副母线比例（`h/a = 0.25`）下，
    /// `a = 41` 与 `a = 46` 的**绕轴同为 24**，而 `(hub, knuckle)` 是 (2,2) 与 (2,3)。
    /// 只把 `around` 混进键的实现会让这两件共用一份经向划分不同的网格。
    ///
    /// **今天它绿，但绿得不作数**：现在两个键之所以不同，是因为 `hash_unit_mesh_params`
    /// 哈希的是未归一化的 `prad`（16.4 与 18.4）。等下面那条 `b1b` 把 `prad` 收成比值，
    /// 这两件的其余分量就全相同了，这条才开始真的量三元组。
    #[test]
    fn t041_b1_a_dish_needs_all_three_segment_counts_in_its_key() {
        let (small, large) = (82.0_f32, 92.0_f32); // pdia = 2a
        let fs = libgm_discretise::elliptical_dish_facets(41.0, 41.0 * 0.25, FACET_TOL_MM)
            .expect("legal dish");
        let fl = libgm_discretise::elliptical_dish_facets(46.0, 46.0 * 0.25, FACET_TOL_MM)
            .expect("legal dish");
        assert_eq!((fs.around, fs.hub, fs.knuckle), (24, 2, 2));
        assert_eq!((fl.around, fl.hub, fl.knuckle), (24, 2, 3));
        assert_eq!(fs.around, fl.around, "夹具失效：绕轴本该相同");

        assert_ne!(
            dish_of(small, 0.25, 0.2).hash_unit_mesh_params(),
            dish_of(large, 0.25, 0.2).hash_unit_mesh_params(),
            "绕轴相同而拐角段数不同的两件碟不能共用一行"
        );
    }

    /// B1 的另一半·碟：形状比例与三个段数**全部相同**时必须共享一行。
    /// `a = 5` 与 `a = 6`（`h/a = 0.25`）的三元组都是 (8, 2, 2)。
    ///
    /// 这条今天红，而且红的原因不是段数——是 `Dish::hash_unit_mesh_params()` 哈希的
    /// 是**未归一化**的 `prad`，而 `gen_unit_shape()` 落库的是 `prad/dia`。两件几何
    /// 相似的碟因此拿到两个键（本该一个），反过来两件 raw `prad` 相同而 `dia` 不同的
    /// 碟会拿到同一个键、却各自落不同内容。与 T002 那条 snout 双键是同一个形状，
    /// T053 第 (3) 条记的就是它。
    #[test]
    fn t041_b1b_geometrically_similar_dishes_in_one_segment_class_share_a_row() {
        let a = libgm_discretise::elliptical_dish_facets(5.0, 5.0 * 0.25, FACET_TOL_MM)
            .expect("legal dish");
        let b = libgm_discretise::elliptical_dish_facets(6.0, 6.0 * 0.25, FACET_TOL_MM)
            .expect("legal dish");
        assert_eq!((a.around, a.hub, a.knuckle), (8, 2, 2));
        assert_eq!((b.around, b.hub, b.knuckle), (8, 2, 2));

        assert_eq!(
            dish_of(10.0, 0.25, 0.2).hash_unit_mesh_params(),
            dish_of(12.0, 0.25, 0.2).hash_unit_mesh_params(),
            "比例相同、段数相同的两件碟必须共享一行（今天红在 raw prad 进了键）"
        );
    }

    /// B2·碟的两个分支元数不同，而且元组真的会撞：椭圆碟 `a=1000, h=5` 是
    /// `(100, 2, 2)`，球碟 `a=1000, h=35` 是 `(100, 2)`——前者的前两位与后者逐位相同。
    /// 分支今天靠 `prad` 分；加段数时把变长元组摊平成「逐个哈希、不带长度、不带分支」
    /// 的写法，这一对就同键了。
    #[test]
    fn t041_b2_the_two_dish_branches_must_not_collide_on_a_shared_prefix() {
        let ell = libgm_discretise::elliptical_dish_facets(1000.0, 5.0, FACET_TOL_MM)
            .expect("legal elliptical dish");
        let sph = libgm_discretise::spherical_dish_facets(1000.0, 35.0, FACET_TOL_MM)
            .expect("legal spherical dish");
        assert_eq!((ell.around, ell.hub, ell.knuckle), (100, 2, 2));
        assert_eq!((sph.around, sph.meridional), (100, 2));
        assert_eq!(
            (ell.around, ell.hub),
            (sph.around, sph.meridional),
            "夹具失效：这一对本该前两位相同"
        );

        // pdia = 2000；椭圆碟 prad > 0，球碟 prad = 0。
        let elliptical = dish_of(2000.0, 5.0 / 2000.0, 0.2);
        let spherical = dish_of(2000.0, 35.0 / 2000.0, 0.0);
        assert_ne!(
            elliptical.hash_unit_mesh_params(),
            spherical.hash_unit_mesh_params(),
            "球碟的二元组不得与椭圆碟三元组的前两位撞成同一个键"
        );
    }

    /// B3·圆环面的二元组不能只混环向。`rins/rout = 0.5`、360° 下，`rout = 104` 与
    /// `rout = 105` 的**环向同为 36**，管截面却是 16 与 20 段——两个数都是 `rout` 的
    /// 函数，但量化粒度不同，这就是缝。
    #[test]
    fn t041_b3_a_circular_torus_needs_both_the_ring_and_the_tube_count() {
        for (rout, ring, tube) in [(104.0_f64, 36, 16), (105.0_f64, 36, 20)] {
            assert_eq!(
                libgm_discretise::torus_ring_segments(rout, FACET_TOL_MM, 360.0),
                ring
            );
            assert_eq!(
                libgm_discretise::circular_torus_tube_segments(rout * 0.5, rout, FACET_TOL_MM),
                tube
            );
        }
        assert_ne!(
            ctorus_of(104.0, 0.5, 360.0).hash_unit_mesh_params(),
            ctorus_of(105.0, 0.5, 360.0).hash_unit_mesh_params(),
            "环向相同而管截面段数不同的两件圆环面不能共用一行"
        );
    }

    /// B4·矩形环面**只有一元**：矩形截面没有管向曲率，别照抄圆环面加一个不存在的轴。
    /// 两件只差 `height` 的 RTorus 必须仍是同一行——`height` 走 `get_scaled_vec3`
    /// 的 z，本来就不进键。**这条今天绿**，是防「顺手多混一个」的反向门。
    #[test]
    fn t041_b4_a_rectangular_torus_key_ignores_height() {
        assert_eq!(
            rtorus_of(250.0, 40.0).hash_unit_mesh_params(),
            rtorus_of(250.0, 900.0).hash_unit_mesh_params(),
            "height 被归一化掉了，不该进键"
        );
    }

    /// B4 的另一半·矩形环面那**一元**要真的进键：`rout = 250`（90° 下 13 段）与
    /// `rout = 1000`（25 段）必须分行。今天红。
    #[test]
    fn t041_b4b_a_rectangular_torus_splits_by_its_ring_segment_class() {
        assert_eq!(
            libgm_discretise::torus_ring_segments(250.0, FACET_TOL_MM, 90.0),
            13
        );
        assert_eq!(
            libgm_discretise::torus_ring_segments(1000.0, FACET_TOL_MM, 90.0),
            25
        );
        assert_ne!(
            rtorus_of(250.0, 40.0).hash_unit_mesh_params(),
            rtorus_of(1000.0, 40.0).hash_unit_mesh_params(),
            "13 段与 25 段的矩形环面要分行"
        );
    }

    /// B5·球只混 `n` 一个数：`GM_Sphere::calcFacetsWithoutSurfaces`（libgm 3.1
    /// `0x100A20F0`）的经向带数恒为 `n/2`，不是独立自由度。
    /// 今天 `Sphere::hash_unit_mesh_params()` 是个常量，所有球共用一行。
    #[test]
    fn t041_b5_a_sphere_key_carries_exactly_one_segment_count() {
        use aios_core::prim_geo::Sphere;
        assert_eq!(libgm_discretise::cylinder_segments(100.0, FACET_TOL_MM), 32);
        assert_eq!(libgm_discretise::cylinder_segments(295.0, FACET_TOL_MM), 56);

        let small = Sphere {
            radius: 100.0,
            ..Default::default()
        };
        let large = Sphere {
            radius: 295.0,
            ..Default::default()
        };
        assert_ne!(
            small.hash_unit_mesh_params(),
            large.hash_unit_mesh_params(),
            "32 段与 56 段的球要分行"
        );

        let same = Sphere {
            radius: 101.0,
            ..Default::default()
        };
        assert_eq!(libgm_discretise::cylinder_segments(101.0, FACET_TOL_MM), 32);
        assert_eq!(
            small.hash_unit_mesh_params(),
            same.hash_unit_mesh_params(),
            "同为 32 段的两个球仍要共享一行"
        );
    }

    /// B6·已经带真实尺寸的两支，键**逐位不变**。SSCL 与偏心 Snout 的
    /// `gen_unit_shape()` 返回的是带真实尺寸的克隆、`get_scaled_vec3()` 是单位阵，
    /// 段数本来就能从参数自身算出；再混一遍是冗余，还会让同一件出现两个键。
    ///
    /// 这条今天绿，靠的是**记下当前值**——T041 落地后这两个数一位都不许动。
    #[test]
    fn t041_b6_the_already_sized_variants_keep_their_keys() {
        let sscl = SCylinder {
            pdia: 590.0,
            phei: 1000.0,
            btm_shear_angles: [15.0, 0.0],
            top_shear_angles: [0.0, 0.0],
            ..Default::default()
        };
        assert!(sscl.is_sscl(), "夹具失效：这件本该是切角柱");
        let eccentric = LSnout {
            poff: 12.06,
            ..snout_of(66.33)
        };

        // 记录值本身不重要，「改动前后一致」才是判据；换夹具时同步换这两个数。
        assert_eq!(sscl.hash_unit_mesh_params(), 4_898_598_737_821_989_684);
        assert_eq!(eccentric.hash_unit_mesh_params(), 3_389_970_894_213_923_445);
    }

    /// A1/A2·Snout：同一锥度比（`ptdm/pbdm = 0.5`）下 `pbdm = 100` 是 24 段、
    /// `pbdm = 600` 是 56 段，必须分行；而 `pbdm = 1` 与 `2` 都撞 8 段下限，
    /// 必须仍共享。段数取**两端半径的大者**（`GM_Snout::calcFacets` `0x1009EA30`）。
    #[test]
    fn t041_a_snout_splits_by_its_larger_end_segment_class() {
        assert_eq!(
            libgm_discretise::snout_segments(50.0, 25.0, FACET_TOL_MM),
            24
        );
        assert_eq!(
            libgm_discretise::snout_segments(300.0, 150.0, FACET_TOL_MM),
            56
        );
        assert_ne!(
            snout_of(100.0).hash_unit_mesh_params(),
            snout_of(600.0).hash_unit_mesh_params(),
            "24 段与 56 段的异径管要分行"
        );
        assert_eq!(
            snout_of(1.0).hash_unit_mesh_params(),
            snout_of(2.0).hash_unit_mesh_params(),
            "同为 8 段的两件必须共享一行"
        );
    }

    fn reusable_linear_loft() -> SweepSolid {
        SweepSolid {
            profile: CateProfileParam::UNKOWN,
            path: SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end: Vec3::X * 7.0,
                is_spine: true,
            }),
            drns: Some(DVec3::NEG_X),
            drne: Some(DVec3::X),
            ..Default::default()
        }
    }

    fn loft_batch(param: SweepSolid, geo_hash: u64, refno_text: &str) -> ShapeInstancesData {
        let refno = RefnoEnum::from(refno_text);
        let inst = EleInstGeo {
            geo_hash,
            refno,
            geo_param: PdmsGeoParam::PrimLoft(param),
            transform: Transform::IDENTITY,
            visible: true,
            ..Default::default()
        };
        let data = EleInstGeosData {
            inst_key: refno_text.into(),
            refno,
            insts: vec![inst],
            type_name: "PrimLoft".into(),
            ..Default::default()
        };
        let mut batch = ShapeInstancesData::default();
        batch.inst_geos_map.insert(refno_text.into(), data);
        batch
    }

    #[test]
    fn reusable_linear_loft_aliases_emit_one_canonical_inst_geo() {
        let left = reusable_linear_loft();
        let mut right = left.clone();
        right.drns = Some(DVec3::NEG_X);
        right.drne = Some(DVec3::X);
        right.bangle = 37.0;
        right.plax = Vec3::Y;
        right.extrude_dir = DVec3::X;
        right.height = 42.0;
        right.lmirror = true;
        right.path = SweepPath3D::Line(Line3D {
            start: Vec3::ZERO,
            end: Vec3::X * 11.0,
            is_spine: false,
        });
        let geo_hash = left.hash_unit_mesh_params();
        assert_eq!(geo_hash, right.hash_unit_mesh_params());

        let plan = plan_for_test(vec![
            loft_batch(left, geo_hash, "1/121"),
            loft_batch(right, geo_hash, "1/122"),
        ])
        .expect("profile-identical reusable lofts share one canonical row");
        let sql = plan
            .packets
            .iter()
            .map(|packet| packet.sql.as_str())
            .join("\n");
        assert_eq!(
            sql.matches(&format!("UPSERT inst_geo:⟨{geo_hash}⟩"))
                .count(),
            1,
            "{sql}"
        );
    }

    #[test]
    fn forced_linear_loft_hash_collision_still_fails_closed() {
        let left = reusable_linear_loft();
        let mut right = left.clone();
        right.profile = CateProfileParam::SREC(SRectData {
            size: Vec2::new(2.0, 3.0),
            ..Default::default()
        });
        let forced_hash = left.hash_unit_mesh_params();

        let error = plan_for_test(vec![
            loft_batch(left, forced_hash, "1/123"),
            loft_batch(right, forced_hash, "1/124"),
        ])
        .expect_err("different canonical profiles sharing an id must fail");
        assert!(
            matches!(
                error.downcast_ref::<SaveConflict>(),
                Some(SaveConflict::RecordContent {
                    kind: "inst_geo",
                    ..
                })
            ),
            "{error:#}"
        );
    }

    #[test]
    fn input_permutations_produce_identical_plan_and_reduce_requests_by_seventy_percent() {
        let ids = (1..=16)
            .map(|i| format!("1/{}", 200 + i))
            .collect::<Vec<_>>();
        let baseline_packets = ids
            .iter()
            .map(|id| {
                plan_for_test(vec![normal_batch(id, true)])
                    .unwrap()
                    .packets
                    .len()
            })
            .sum::<usize>();
        let canonical = plan_for_test(ids.iter().map(|id| normal_batch(id, true)).collect())
            .expect("canonical plan");
        let expected = plan_sql(&canonical);
        assert_eq!(canonical.source_batch_count, 16);
        assert!(
            canonical.packets.len() * 10 <= baseline_packets * 3,
            "combined={} baseline={baseline_packets}",
            canonical.packets.len()
        );

        let mut seed = 0x5eed_u64;
        for _ in 0..100 {
            let mut order = ids.clone();
            for i in (1..order.len()).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                order.swap(i, seed as usize % (i + 1));
            }
            let actual = plan_for_test(order.iter().map(|id| normal_batch(id, true)).collect())
                .expect("permutation plan");
            assert_eq!(plan_sql(&actual), expected);
        }
    }

    #[test]
    fn sql_packets_obey_row_and_byte_limits_and_phase_order() {
        let rows = (0..301)
            .map(|index| (format!("row:{index:03}"), format!("{{id: row:{index:03}}}")))
            .collect::<BTreeMap<_, _>>();
        let mut packets = Vec::new();
        push_array_packets(
            &mut packets,
            SavePhase::SharedContent,
            "INSERT IGNORE INTO row [",
            "];",
            &rows,
        );
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].row_count, SQL_PACKET_ROWS);
        assert_eq!(packets[1].row_count, 1);

        let large_rows = (0..3)
            .map(|index| (format!("large:{index}"), "x".repeat(600 * 1024)))
            .collect::<BTreeMap<_, _>>();
        let mut byte_packets = Vec::new();
        push_array_packets(
            &mut byte_packets,
            SavePhase::Relations,
            "INSERT RELATION INTO large [",
            "];",
            &large_rows,
        );
        assert_eq!(byte_packets.len(), 3);
        assert!(
            byte_packets
                .iter()
                .all(|packet| packet.estimated_bytes <= SQL_PACKET_BYTES)
        );

        let plan = plan_for_test(vec![normal_batch("1/401", true)]).expect("plan");
        let ranks = plan
            .packets
            .iter()
            .map(|packet| match packet.phase {
                SavePhase::SharedContent => 0,
                SavePhase::Relations => 1,
                SavePhase::InstanceRelations => 2,
            })
            .collect::<Vec<_>>();
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]), "{ranks:?}");
    }

    fn execution_plan_for_test(packets: Vec<SqlPacket>) -> SavePlan {
        SavePlan {
            mode: SaveMode::FullBuild,
            flush_reason: FlushReason::ChannelClosed,
            source_batch_count: 1,
            instance_rows: 0,
            geo_occurrences: 0,
            coalesce_wait: Duration::ZERO,
            delete_refnos: Vec::new(),
            written_refnos: Vec::new(),
            packets,
            metadata_query_count: 0,
            conflict_count: 0,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_save_packets_execute_serially_and_replay_idempotently() {
        use crate::data_interface::staging::ResourceThresholds;
        use crate::data_interface::staging::lifecycle::create_window_on;
        use crate::data_interface::staging::write_context::with_staging_writes;
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7991, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");
        let packets = || {
            vec![
                SqlPacket {
                    phase: SavePhase::SharedContent,
                    sql: "UPSERT trans:shape_save_serial SET d = 1;".into(),
                    row_count: 1,
                    estimated_bytes: 46,
                },
                SqlPacket {
                    phase: SavePhase::Relations,
                    sql: "UPDATE trans:shape_save_serial SET d = 2;".into(),
                    row_count: 1,
                    estimated_bytes: 46,
                },
            ]
        };

        with_staging_writes(window.write_context(), async {
            execute_save_plan(execution_plan_for_test(packets())).await?;
            execute_save_plan(execution_plan_for_test(packets())).await
        })
        .await
        .expect("staged execution");

        let journal = window.journal().await;
        assert_eq!(journal.len(), 4);
        assert!(journal[0].sql.starts_with("UPSERT"), "{journal:?}");
        assert!(journal[1].sql.starts_with("UPDATE"), "{journal:?}");
        assert!(journal[2].sql.starts_with("UPSERT"), "{journal:?}");
        assert!(journal[3].sql.starts_with("UPDATE"), "{journal:?}");

        let mut response = window
            .staging_db()
            .query("RETURN trans:shape_save_serial.d;")
            .await
            .expect("query staged row")
            .check()
            .expect("check staged row");
        let value: Option<i64> = response.take(0).expect("take staged value");
        assert_eq!(
            value,
            Some(2),
            "replay twice must converge to the same final state"
        );
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_save_failure_stops_later_packets_without_detaching_work() {
        use crate::data_interface::staging::ResourceThresholds;
        use crate::data_interface::staging::lifecycle::create_window_on;
        use crate::data_interface::staging::write_context::with_staging_writes;
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 7990, 1, 1, ResourceThresholds::default())
            .await
            .expect("create window");
        let packet = |phase, sql: &str| SqlPacket {
            phase,
            sql: sql.into(),
            row_count: 1,
            estimated_bytes: sql.len(),
        };
        let plan = execution_plan_for_test(vec![
            packet(
                SavePhase::SharedContent,
                "UPSERT trans:shape_save_before_failure SET d = 1;",
            ),
            packet(SavePhase::Relations, "THIS IS NOT SURREALQL;"),
            packet(
                SavePhase::InstanceRelations,
                "UPSERT trans:shape_save_after_failure SET d = 1;",
            ),
        ]);

        let error = with_staging_writes(window.write_context(), execute_save_plan(plan))
            .await
            .expect_err("bad packet must fail the flush");
        assert!(!error.to_string().is_empty());
        assert_eq!(
            window.journal().await.len(),
            1,
            "only the first packet committed"
        );

        let mut response = window
            .staging_db()
            .query("RETURN record::exists(trans:shape_save_after_failure);")
            .await
            .expect("query tail record")
            .check()
            .expect("check tail record");
        let exists: Option<bool> = response.take(0).expect("take existence");
        assert_eq!(exists, Some(false), "later packet must not be detached");
        window.drop_database().await.expect("cleanup");
    }

    /// 中断留下的半成品必须能被同一批生成重放修好；已有 mesh 派生字段不能被参数
    /// 刷新抹掉。执行两次还钉住了 journal/直写两条路径共同需要的幂等性。
    #[tokio::test(flavor = "multi_thread")]
    async fn inst_geo_upsert_repairs_partial_rows_and_preserves_mesh_fields() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("pdms_inst")
            .use_db("inst_geo_replay")
            .await
            .expect("use db");
        db.query(
            "UPSERT inst_geo:⟨42⟩ CONTENT { meshed: true, bad: true, aabb: aabb:keep, pts: [vec3:keep] };",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seed partial row");

        let statement = render_inst_geo_upsert(42, "{ kind: 'box', size: [1, 2, 3] }", true);
        db.query(format!("{statement}\n{statement}"))
            .await
            .expect("replay transport")
            .check()
            .expect("replay twice");

        let mut response = db
            .query(
                "RETURN [inst_geo:⟨42⟩.param.kind = 'box', \
                         inst_geo:⟨42⟩.bad = false, \
                         inst_geo:⟨42⟩.meshed = true, \
                         inst_geo:⟨42⟩.aabb = aabb:keep, \
                         inst_geo:⟨42⟩.pts = [vec3:keep]];",
            )
            .await
            .expect("verify transport")
            .check()
            .expect("verify query");
        let flags: Vec<bool> = response.take(0).expect("take flags");
        assert_eq!(flags, vec![true, true, true, true, true]);
    }

    /// 同一个单位网格行被两个不同的 `PdmsGeoParam` 变体先后刷新——普通 LCylinder
    /// 与非切角 SCylinder 共享 `CYLINDER_GEO_HASH`，这正是生产会天天发生的形状。
    /// `param` 必须整值覆盖成后写的**单变体**对象：回退到 `MERGE { param: … }`
    /// 的旧写法，两个变体被深合并成双键对象（enum 反序列化永久失败、所有引用该
    /// 共享行的根全部生成失败——2026-08-13 live A/B 实测），本断言当场变红。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("pdms_inst")
            .use_db("inst_geo_variant_switch")
            .await
            .expect("use db");

        let first = render_inst_geo_upsert(
            7,
            r#"{"PrimLCylinder":{"pdia":1.0,"pbdi":-0.5,"ptdi":0.5}}"#,
            false,
        );
        let second =
            render_inst_geo_upsert(7, r#"{"PrimSCylinder":{"pdia":1.0,"phei":1.0}}"#, false);
        db.query(format!("{first}\n{second}"))
            .await
            .expect("variant switch transport")
            .check()
            .expect("variant switch statements");

        let mut response = db
            .query(
                "RETURN [object::len(inst_geo:⟨7⟩.param) = 1, \
                         inst_geo:⟨7⟩.param.PrimSCylinder != NONE, \
                         inst_geo:⟨7⟩.param.PrimLCylinder = NONE];",
            )
            .await
            .expect("verify transport")
            .check()
            .expect("verify query");
        let flags: Vec<bool> = response.take(0).expect("take flags");
        assert_eq!(
            flags,
            vec![true, true, true],
            "param 必须是后写变体的单键对象，绝不能深合并出双键"
        );
    }

    #[test]
    fn production_inst_geo_writes_replace_param_wholesale() {
        let sql = render_inst_geo_upsert(7, r#"{"PrimLCylinder":{}}"#, true);
        assert!(sql.contains("UPSERT inst_geo:⟨7⟩ SET param ="), "{sql}");
        assert!(!sql.to_ascii_lowercase().contains("insert ignore"), "{sql}");
        assert!(!sql.contains("MERGE"), "{sql}");
    }

    /// 手动 live（层级查询优化 P1→P2 部署步）：对**配置库**执行 anc/dbnum
    /// 部署三件套——灌 `fn::refno_u64` / `fn::anc_u64`（只抠 common.surql 里这
    /// 两个定义，不整目录重放、不盖其他函数）、建索引（IF NOT EXISTS）、幂等
    /// 回填。等价于「新版 gen-model 启动一次」中与本方案相关的那部分，供不重启
    /// 服务先行验收读侧（plant-ui `tests/anc_model_query_parity.rs`）。可重复跑。
    #[tokio::test]
    #[ignore = "manual live: writes fn defines + indexes + anc backfill to the configured Surreal"]
    async fn live_backfill_anc_on_configured_db() {
        aios_core::init_test_surreal().await.expect("连接配置库");

        aios_core::function::define_common_functions_on(&SUL_DB)
            .await
            .expect("load common functions and inst_meta compatibility definitions");
        println!("[live] fn::refno_u64 / fn::anc_u64 已通过 core 正式加载入口就绪");

        init_inst_relate_indices().await.expect("建索引");
        println!("[live] 索引就绪（IF NOT EXISTS）");

        let started = std::time::Instant::now();
        let (inst, tubi) = backfill_inst_relate_anc().await.expect("回填");
        println!(
            "[live] 回填完成：inst_relate {inst} 行，tubi_relate {tubi} 行，耗时 {:?}",
            started.elapsed()
        );

        // 复核范围与回填一致：ref0 超出 u64 打包上限的行（fixture 魔术 dbnum
        // 残留）本就不可打包，回填按设计跳过，不算残留。
        let mut response = SUL_DB
            .query(
                "RETURN [array::len((SELECT VALUE id FROM inst_relate WHERE anc != NONE LIMIT 1)), \
                         array::len((SELECT VALUE id FROM inst_relate WHERE anc = NONE \
                            AND type::number(string::split(record::id(id), '_')[0]) <= 2147483647 LIMIT 1)), \
                         array::len((SELECT VALUE id FROM tubi_relate WHERE anc = NONE \
                            AND type::number(string::split(record::id(id), '_')[0]) <= 2147483647 LIMIT 1))];",
            )
            .await
            .expect("覆盖复核查询")
            .check()
            .expect("覆盖复核");
        let [has_anc, inst_none, tubi_none]: [i64; 3] = response
            .take::<Vec<i64>>(0)
            .expect("take 覆盖复核")
            .try_into()
            .expect("三元组");
        assert_eq!(has_anc, 1, "回填后 inst_relate 应存在带 anc 的行");
        assert_eq!(inst_none, 0, "inst_relate 不应残留可打包的 anc = NONE 行");
        assert_eq!(tubi_none, 0, "tubi_relate 不应残留可打包的 anc = NONE 行");
        println!("[live] 覆盖复核通过：两表可打包范围内 anc 无残留 NONE");
    }

    /// 手动 live（P4 写时物化部署步）：对**配置库**跑一轮平表副本清扫——
    /// 等价于「新版 gen-model 启动一次」中 P4 相关的那部分（存量回填 = 首轮
    /// 全量），供不重启服务先行验收读侧（plant-ui 对拍测试的 flat 路径）。
    /// 幂等可重复跑：已清扫的库一轮空转即返回。
    #[tokio::test]
    #[ignore = "manual live: materializes insts_flat/aabb_d/world_trans_d on the configured Surreal"]
    async fn live_sweep_inst_relate_flat_on_configured_db() {
        aios_core::init_test_surreal().await.expect("连接配置库");

        let started = std::time::Instant::now();
        let swept = sweep_inst_relate_flat().await.expect("清扫");
        println!(
            "[live] 平表副本清扫完成：补 {swept} 行，耗时 {:?}",
            started.elapsed()
        );

        let mut response = SUL_DB
            .query(
                "RETURN [array::len((SELECT VALUE id FROM inst_relate WHERE insts_flat != NONE LIMIT 1)), \
                         array::len((SELECT VALUE id FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none LIMIT 1))];",
            )
            .await
            .expect("覆盖复核查询")
            .check()
            .expect("覆盖复核");
        let [has_flat, residue]: [i64; 2] = response
            .take::<Vec<i64>>(0)
            .expect("take 覆盖复核")
            .try_into()
            .expect("二元组");
        assert_eq!(has_flat, 1, "清扫后 inst_relate 应存在带 insts_flat 的行");
        assert_eq!(residue, 0, "不应残留 insts_flat = NONE 且对读者可见的行");
        println!("[live] 覆盖复核通过：可见行 insts_flat 无残留 NONE");
    }

    /// T01（specs/025，ADR-043「未决」）：共享 geo 的 `bad → meshed` 恢复必须刷新
    /// **所有**引用者的 `insts_flat`，而不只是本轮被重写的那一行。
    ///
    /// 生产可达序列（本用例逐句同形复刻，语句形态各自指向生产渲染点）：
    ///
    /// 1. 首轮构建：A、B 两个 `inst_relate` 行（不同生成根）共用一个内容寻址
    ///    `inst_geo` G（单位网格按内容寻址，跨根共享是设计本意）；G 网格失败
    ///    （occ_generate 失败路径 `set bad = true` 同形），清扫把两行物化成
    ///    **不含 G 的 insts_flat（非 NONE）**——这是「旧值」。
    /// 2. 只对 B 做定向重生成（`SaveMode::TargetedReplace` 的三笔同形写）：
    ///    [`render_inst_relate_replace`] 替换 B 行（新行无 insts_flat）、
    ///    [`render_inst_geo_upsert`] 第三参为真清掉 bad、网格重试成功
    ///    （occ_generate 成功路径 `set meshed = true` 同形）、AABB 阶段落指针与副本。
    /// 3. 空闲轮清扫（真函数 [`sweep_inst_relate_flat`]）：B 行 `insts_flat = NONE`
    ///    被回填出含 G 的新值；A 行**非 NONE**，回填段够不着、修复段（只圈
    ///    booled_id 不符）也够不着。
    ///
    /// 断言：A 行的 `insts_flat` 不得停在旧值（必须与 B 一样含 G）。**今天这条会
    /// 红**——读侧对 A 行三副本齐活、直接采信旧缓存，那件几何在 A 上静默消失，
    /// 正是 ADR-043 要证的缺口（RM13 同形态）。T18 闭环后本用例转绿，回退旧写法
    /// 必须复红。
    ///
    /// 前置（写进测试名）：配置库必须是可丢弃沙箱（pytest testbed 8019）——用例
    /// 在配置库上播种 zzflat_* / 20260823_* 合成行并于断言**之前**清理（预期红的
    /// 用例不给沙箱留残留），且会对全库跑两轮真实清扫。
    #[tokio::test]
    #[ignore = "manual live: seeds zzflat_* fixture rows and runs full sweeps on the configured (disposable) Surreal"]
    async fn live_shared_geo_bad_retry_must_refresh_sibling_insts_flat_on_disposable_db() {
        aios_core::init_test_surreal().await.expect("连接配置库");

        const SHARED_GEO_HASH: u64 = 20260823;
        const CLEANUP: &str = "DELETE inst_relate:⟨20260823_101⟩, inst_relate:⟨20260823_102⟩; \
             DELETE geo_relate:zzflat_e1, geo_relate:zzflat_e2; \
             DELETE inst_geo:⟨20260823⟩; \
             DELETE inst_info:zzflat_a, inst_info:zzflat_b; \
             DELETE trans:zzflat_t, trans:zzflat_te; \
             DELETE aabb:zzflat_box; \
             DELETE pe:20260823_1, pe:20260823_2;";
        SUL_DB
            .query(CLEANUP)
            .await
            .expect("预清理传输")
            .check()
            .expect("预清理（幂等重跑）");

        // --- 首轮构建：两行共用 G，G 网格失败（bad = true，meshed 缺省）。---
        let wt = serde_json::to_string(&Transform::IDENTITY).expect("序列化恒等变换");
        let box_aabb = serde_json::to_string(&Aabb::new(
            parry3d::math::Point::new(0.0f32, 0.0, 0.0),
            parry3d::math::Point::new(1.0f32, 1.0, 1.0),
        ))
        .expect("序列化包围盒");
        let unit_param = r#"{"PrimLCylinder":{"pdia":1.0,"pbdi":-0.5,"ptdi":0.5}}"#;
        // 首轮建 G 用生产渲染器（FullBuild：reset_bad = false），失败路径同形置 bad。
        let create_geo = render_inst_geo_upsert(SHARED_GEO_HASH, unit_param, false);
        let seed = format!(
            "{create_geo}\n\
             update inst_geo:⟨{SHARED_GEO_HASH}⟩ set bad = true;\n\
             CREATE pe:20260823_1 SET noun='SITE', dbnum=7997; \
             CREATE pe:20260823_2 SET noun='SITE', dbnum=7997; \
             CREATE inst_info:zzflat_a; CREATE inst_info:zzflat_b; \
             CREATE trans:zzflat_t SET d = {wt}; CREATE trans:zzflat_te SET d = {wt}; \
             CREATE aabb:zzflat_box SET d = {box_aabb}; \
             INSERT RELATION INTO geo_relate [\
                {{ id: 'zzflat_e1', in: inst_info:zzflat_a, out: inst_geo:⟨{SHARED_GEO_HASH}⟩, \
                   trans: trans:zzflat_te, visible: true, geo_type: 'Pos' }}, \
                {{ id: 'zzflat_e2', in: inst_info:zzflat_b, out: inst_geo:⟨{SHARED_GEO_HASH}⟩, \
                   trans: trans:zzflat_te, visible: true, geo_type: 'Pos' }}\
             ]; \
             INSERT RELATION INTO inst_relate [\
                {{ id: inst_relate:⟨20260823_101⟩, in: pe:20260823_1, out: inst_info:zzflat_a, \
                   world_trans: trans:zzflat_t, world_trans_d: {wt}, aabb: aabb:zzflat_box, \
                   aabb_d: {box_aabb}, generic: 'BOX', anc: [42], dbnum: 7997, dt: NONE, \
                   has_cata_neg: false, solid: true }}, \
                {{ id: inst_relate:⟨20260823_102⟩, in: pe:20260823_2, out: inst_info:zzflat_b, \
                   world_trans: trans:zzflat_t, world_trans_d: {wt}, aabb: aabb:zzflat_box, \
                   aabb_d: {box_aabb}, generic: 'BOX', anc: [42], dbnum: 7997, dt: NONE, \
                   has_cata_neg: false, solid: true }}\
             ];"
        );
        SUL_DB
            .query(seed)
            .await
            .expect("播种传输")
            .check()
            .expect("播种首轮世界");

        // 首轮清扫：两行都被物化成不含 G 的旧值（G 未 meshed，被子查询过滤）。
        sweep_inst_relate_flat().await.expect("首轮清扫");
        let mut response = SUL_DB
            .query(
                "RETURN [inst_relate:⟨20260823_101⟩.insts_flat != NONE, \
                         array::len(inst_relate:⟨20260823_101⟩.insts_flat ?? [-1]) = 0, \
                         inst_relate:⟨20260823_102⟩.insts_flat != NONE, \
                         array::len(inst_relate:⟨20260823_102⟩.insts_flat ?? [-1]) = 0];",
            )
            .await
            .expect("旧值复核传输")
            .check()
            .expect("旧值复核");
        let staled: Vec<bool> = response.take(0).expect("take 旧值复核");
        assert_eq!(
            staled,
            vec![true, true, true, true],
            "首轮清扫后 A/B 都必须是「非 NONE 且不含 G」的已物化旧值——否则本用例前提不成立"
        );

        // --- 只对 B 定向重生成（TargetedReplace 的写序同形）。---
        let b_row = format!(
            "{{id: inst_relate:⟨20260823_102⟩, in: pe:20260823_2, out: inst_info:zzflat_b, \
             world_trans: trans:zzflat_t, world_trans_d: {wt}, generic: 'BOX', anc: [42], \
             dbnum: 7997, dt: NONE, has_cata_neg: false, solid: true}}"
        );
        let replace =
            render_inst_relate_replace(&[("inst_relate:⟨20260823_102⟩".to_string(), b_row)]);
        let reset = render_inst_geo_upsert(SHARED_GEO_HASH, unit_param, true);
        // 网格重试成功（occ_generate 成功路径同形）＋ AABB 阶段回填指针与副本。
        let regen = format!(
            "{replace}\n{reset}\n\
             update inst_geo:⟨{SHARED_GEO_HASH}⟩ set meshed = true, aabb = aabb:zzflat_box, pts=[];\n\
             UPDATE inst_relate:⟨20260823_102⟩ SET aabb = aabb:zzflat_box, aabb_d = {box_aabb};"
        );
        SUL_DB
            .query(regen)
            .await
            .expect("定向重生成传输")
            .check()
            .expect("定向重生成 B");

        // --- 空闲轮清扫，然后先清理、后断言（红跑不留残留）。---
        sweep_inst_relate_flat().await.expect("第二轮清扫");
        let mut response = SUL_DB
            .query(
                "RETURN [inst_relate:⟨20260823_101⟩.insts_flat.geo_hash ?? [], \
                         inst_relate:⟨20260823_102⟩.insts_flat.geo_hash ?? []];",
            )
            .await
            .expect("终态读取传输")
            .check()
            .expect("终态读取");
        let [a_hashes, b_hashes]: [Vec<String>; 2] = response
            .take::<Vec<Vec<String>>>(0)
            .expect("take 终态投影")
            .try_into()
            .expect("二元组");

        SUL_DB
            .query(CLEANUP)
            .await
            .expect("清理传输")
            .check()
            .expect("清理合成行");

        let shared = SHARED_GEO_HASH.to_string();
        assert_eq!(
            b_hashes,
            vec![shared.clone()],
            "被重写的 B 行必须经回填段物化出含 G 的新值（这条不红说明夹具没搭对）"
        );
        assert_eq!(
            a_hashes,
            vec![shared],
            "A 行的 insts_flat 不得停在旧值：共享 geo bad→meshed 之后，未被重写的引用者\
             也必须看到 G——回填段只圈 NONE、修复段只圈 booled_id 不符，今天两段都够不着它\
             （ADR-043 的缺口，FR-7）"
        );
    }

    #[tokio::test]
    async fn failed_instance_write_reaches_the_caller() {
        let mut tasks = FuturesUnordered::new();
        tasks.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async {
                Err::<(), anyhow::Error>(anyhow::anyhow!("forced instance write failure"))
            }),
        );

        let error = finish_db_writes(tasks)
            .await
            .expect_err("a failed database write must fail model generation");

        assert!(
            format!("{error:#}").contains("forced instance write failure"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn failed_instance_write_still_waits_for_the_other_writes_to_settle() {
        let completed = Arc::new(AtomicBool::new(false));
        let delayed_completed = completed.clone();
        let mut tasks = FuturesUnordered::new();
        tasks.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async {
                Err::<(), anyhow::Error>(anyhow::anyhow!("first write failed"))
            }),
        );
        tasks.push(
            crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                delayed_completed.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );

        finish_db_writes(tasks)
            .await
            .expect_err("the write set must fail");
        assert!(
            completed.load(Ordering::SeqCst),
            "returning early would detach a database write into the retry window"
        );
    }

    /// `inst_relate` 的写入必须自带删除，且删除集恰好是本批要写的那些 id。
    ///
    /// fork 的 `INSERT RELATION` 撞已有 id 时不报错、保留旧行，所以「这一行写没写进去」
    /// 一旦取决于外面那个级联删除集，就等于取决于一个从**本次产物**推出来的、天然漏项的
    /// 集合——漏掉的那行会永远停在第一次生成的值，且无人报错。删除与插入同处一个事务，
    /// 是为了不给读者留下「刚删完、还没写回来」的窗口。
    #[test]
    fn inst_relate_rows_are_replaced_by_id_in_one_transaction() {
        let rows = vec![
            (
                "inst_relate:7997_1".to_string(),
                "{id: inst_relate:7997_1, in: pe:7997_1}".to_string(),
            ),
            (
                "inst_relate:7997_2".to_string(),
                "{id: inst_relate:7997_2, in: pe:7997_2}".to_string(),
            ),
        ];
        let sql = render_inst_relate_replace(&rows);

        assert!(sql.starts_with("BEGIN TRANSACTION;\n"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");
        let delete_at = sql
            .find("DELETE inst_relate:7997_1, inst_relate:7997_2;")
            .expect("按本批 id 删旧行");
        let insert_at = sql
            .find("INSERT RELATION INTO inst_relate [")
            .expect("再整批插入");
        assert!(delete_at < insert_at, "删必须排在插之前: {sql}");
        // 删除集恰好覆盖本批：多一个会误删别人的行，少一个就退回「撞 id 静默保留旧行」。
        assert_eq!(sql.matches("inst_relate:7997_1").count(), 2, "{sql}");
        assert_eq!(sql.matches("inst_relate:7997_2").count(), 2, "{sql}");
    }

    /// 生产写入路径上不许再出现裸的 `INSERT RELATION INTO inst_relate`。
    ///
    /// 换回去不会报错、不会编译失败，只会让那一行的新值在撞 id 时被静默丢弃。
    #[test]
    fn the_write_path_never_inserts_inst_relate_without_replacing() {
        let source = include_str!("pdms_inst.rs");
        let body = source
            .split_once("fn build_canonical_save_plan(")
            .expect("canonical SavePlan builder 必须存在")
            .1
            .split_once("\npub(crate) async fn build_save_plan(")
            .map(|(head, _)| head)
            .expect("builder 边界");

        assert!(
            !body.contains("INSERT RELATION INTO inst_relate"),
            "inst_relate 必须走 render_inst_relate_replace 的替换写入: {body}"
        );
        assert!(
            body.matches("push_inst_relate_packets").count() == 2,
            "两处 inst_relate 写入都要走同一个渲染函数: {body}"
        );
    }
}

/// W4（D6）：生成行元数据的渲染期解析——journal 纯数据化。
#[cfg(test)]
mod inst_meta_tests {
    use super::*;
    use aios_core::RefnoEnum;
    use surrealdb::engine::any::connect;

    /// 注意**不能**用 4000000001 保留段：ref0 超过 2^31，RefU64 打包值越过
    /// SurrealDB int（i64）上限，会被 anc 的溢出守卫按设计拒绝（P1 边界约束）。
    /// 这里用生产量级的 ref0（两万级），序号取 786xxx 避开其它系列；本模块只读
    /// pe/ses，不碰进程级 GLOBAL_AABB_TREE。
    fn refu(n: u64) -> RefU64 {
        RefU64((24379u64 << 32) | n)
    }

    /// WORL(786001) ← SITE(786002) ← ZONE(786003) ← EQUI(786004, dbnum/sesno 带全)。
    async fn seeded_db(name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("inst_meta").use_db(name).await.expect("use db");
        crate::data_interface::staging::lifecycle::init_staging_schema(&db)
            .await
            .expect("schema + fn definitions");
        db.query(
            "UPSERT pe:24379_786001 CONTENT { noun: 'WORL' };\
             UPSERT pe:24379_786002 CONTENT { noun: 'SITE', owner: pe:24379_786001 };\
             UPSERT pe:24379_786003 CONTENT { noun: 'ZONE', owner: pe:24379_786002 };\
             UPSERT pe:24379_786004 CONTENT { noun: 'EQUI', owner: pe:24379_786003, dbnum: 7997, sesno: 43 };\
             UPSERT ses:[7997, 43] CONTENT { date: d'2026-08-07T00:00:00Z' };",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seeded");
        db
    }

    /// R3 的核心钉：渲染期解出的固定字面量与被退役的 `fn::` 在**同一个世界**上
    /// 求值必须逐值相等（在引擎里用 `==` 比，绕开字符串形制差异）。
    #[tokio::test(flavor = "multi_thread")]
    async fn resolved_literals_equal_the_retired_fn_evaluations() {
        let db = seeded_db("fn_parity").await;
        let equi = RefnoEnum::from(refu(786004));
        let metas = resolve_inst_meta_on(&db, None, &[equi])
            .await
            .expect("resolve");
        let meta = metas.get(&refu(786004)).expect("meta for equi");

        let pe = equi.to_pe_key();
        let mut response = db
            .query(format!(
                "RETURN {} == fn::anc_u64({pe});\
                 RETURN {} == {pe}.dbnum;\
                 RETURN {} == fn::ses_date({pe});",
                meta.anc_literal(),
                meta.dbnum_literal(),
                meta.dt_literal(),
            ))
            .await
            .expect("parity transport")
            .check()
            .expect("parity query");
        for (index, field) in ["anc", "dbnum", "dt"].iter().enumerate() {
            let equal: Option<bool> = response.take(index).expect("take flag");
            assert_eq!(
                equal,
                Some(true),
                "{field} 的已解值必须与被退役的 fn:: 求值逐值相等: {meta:?}"
            );
        }
        assert_eq!(
            meta.anc,
            vec![
                refu(786004).0,
                refu(786003).0,
                refu(786002).0,
                refu(786001).0
            ],
            "anc = 自身 → 顶（含自身）"
        );
        assert_eq!(meta.dt_literal(), "d'2026-08-07T00:00:00Z'");
    }

    /// P3 读侧便捷层：`fn::zone_u64` / `fn::site_u64` 从 anc 尾部定位，判据与
    /// Rust 解析器同源（链尾打包值 ref1==0 即 WORL，偏移 1）。世界按生产形制
    /// 搭建：WORL 行不入库（database.rs 的 ignore_world_refno），SITE.owner
    /// 悬空指向 ref1=0 的 WORL——两个生产者（resolve_inst_meta_on /
    /// fn::anc_u64）必须产出同一条含 WORL 的链，helpers 在其上取出 ZONE/SITE；
    /// 「含自身」语义与空链 NONE 一并钉住。
    #[tokio::test(flavor = "multi_thread")]
    async fn zone_and_site_helpers_locate_from_the_anc_tail() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("inst_meta")
            .use_db("anc_tail_helpers")
            .await
            .expect("use db");
        crate::data_interface::staging::lifecycle::init_staging_schema(&db)
            .await
            .expect("schema + fn definitions");
        db.query(
            "UPSERT pe:24379_786021 CONTENT { noun: 'SITE', owner: pe:24379_0 };\
             UPSERT pe:24379_786022 CONTENT { noun: 'ZONE', owner: pe:24379_786021 };\
             UPSERT pe:24379_786023 CONTENT { noun: 'EQUI', owner: pe:24379_786022, dbnum: 7997, sesno: 43 };",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seeded");

        let worl = 24379u64 << 32;
        let site = refu(786021).0;
        let zone = refu(786022).0;
        let equi = refu(786023).0;

        let metas = resolve_inst_meta_on(&db, None, &[RefnoEnum::from(refu(786023))])
            .await
            .expect("resolve");
        let meta = metas.get(&refu(786023)).expect("meta for equi");
        assert_eq!(
            meta.anc,
            vec![equi, zone, site, worl],
            "生产形制的链尾必须收着 ref1=0 的 WORL"
        );

        let mut response = db
            .query(format!(
                "RETURN fn::anc_u64(pe:24379_786023) == {};\
                 RETURN fn::zone_u64(fn::anc_u64(pe:24379_786023));\
                 RETURN fn::site_u64(fn::anc_u64(pe:24379_786023));\
                 RETURN fn::zone_u64({});\
                 RETURN fn::zone_u64(fn::anc_u64(pe:24379_786022));\
                 RETURN fn::site_u64(fn::anc_u64(pe:24379_786021));\
                 RETURN fn::zone_u64(fn::anc_u64(pe:24379_786021));\
                 RETURN fn::zone_u64([]);\
                 RETURN fn::site_u64([]);",
                meta.anc_literal(),
                meta.anc_literal(),
            ))
            .await
            .expect("helper transport")
            .check()
            .expect("helper queries");
        let agree: Option<bool> = response.take(0).expect("take parity");
        assert_eq!(
            agree,
            Some(true),
            "fn::anc_u64 必须与 Rust 解析器产出同一条链"
        );
        let checks: [(usize, Option<u64>, &str); 8] = [
            (1, Some(zone), "zone = 倒数第 3（链尾 WORL 偏移 1）"),
            (2, Some(site), "site = 倒数第 2（链尾 WORL 偏移 1）"),
            (3, Some(zone), "字面量入参与函数值入参同一取位"),
            (4, Some(zone), "ZONE 自身按「含自身」语义返回自己"),
            (5, Some(site), "SITE 自身按「含自身」语义返回自己"),
            (6, None, "SITE 之上没有 ZONE → NONE"),
            (7, None, "空链 zone → NONE 不误报"),
            (8, None, "空链 site → NONE 不误报"),
        ];
        for (index, expected, why) in checks {
            let got: Option<i64> = response.take(index).expect("take helper value");
            assert_eq!(got, expected.map(|v| v as i64), "{why}");
        }
    }

    /// `ses` 行是 append-only 历史：当前世界（暂存）miss 时回落持久层——
    /// 未变更元素的 dt 才不会被烘成 NONE。
    #[tokio::test(flavor = "multi_thread")]
    async fn session_dates_fall_back_to_the_persistent_history() {
        let staging = connect("mem://").await.expect("mem boots");
        staging
            .use_ns("inst_meta")
            .use_db("ses_fallback_staging")
            .await
            .expect("use db");
        staging
            .query(
                "UPSERT pe:24379_786011 CONTENT { noun: 'ZONE' };\
                 UPSERT pe:24379_786012 CONTENT { noun: 'EQUI', owner: pe:24379_786011, dbnum: 7997, sesno: 7 };",
            )
            .await
            .expect("staging seed transport")
            .check()
            .expect("staging seeded");

        let persistent = connect("mem://").await.expect("mem boots");
        persistent
            .use_ns("inst_meta")
            .use_db("ses_fallback_persistent")
            .await
            .expect("use db");
        persistent
            .query("UPSERT ses:[7997, 7] CONTENT { date: d'2025-01-02T03:04:05Z' };")
            .await
            .expect("persistent seed transport")
            .check()
            .expect("persistent seeded");

        let equi = RefnoEnum::from(refu(786012));
        let metas = resolve_inst_meta_on(&staging, Some(&persistent), &[equi])
            .await
            .expect("resolve");
        assert_eq!(
            metas.get(&refu(786012)).expect("meta").dt_literal(),
            "d'2025-01-02T03:04:05Z'",
            "旧会话的日期必须从持久层历史回落解出"
        );
    }

    /// 断链（owner 指向的行不存在）→ 响亮失败：宁可生成任务进重试，也不烘一个
    /// 错误的 anc 进 journal。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_broken_owner_chain_fails_loudly() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("inst_meta")
            .use_db("broken_chain")
            .await
            .expect("use db");
        db.query(
            "UPSERT pe:24379_786022 CONTENT { noun: 'EQUI', owner: pe:24379_786021, dbnum: 7997, sesno: 1 };",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seeded");

        let error = resolve_inst_meta_on(&db, None, &[RefnoEnum::from(refu(786022))])
            .await
            .expect_err("断链必须失败");
        assert!(error.to_string().contains("断裂"), "{error:#}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_omitted_world_row_still_terminates_the_chain() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("inst_meta")
            .use_db("omitted_world")
            .await
            .expect("use db");
        db.query(
            "UPSERT pe:24384_23823 CONTENT { noun: 'BEND', owner: pe:16192_0, dbnum: 8000, sesno: 73 };",
        )
        .await
        .expect("seed transport")
        .check()
        .expect("seeded");

        let bend = RefnoEnum::from(RefU64((24384u64 << 32) | 23823));
        let meta = resolve_inst_meta_on(&db, None, &[bend])
            .await
            .expect("省略的 WORL 行必须按全量入库契约到顶")
            .remove(&bend.refno())
            .expect("meta");
        assert_eq!(meta.anc, vec![bend.refno().0, RefU64(16192u64 << 32).0]);
    }

    /// 与旧 fn:: 对缺行的语义一致：seed 自己的行不在 → 空态渲染
    /// （anc [] / dbnum NONE / dt NONE），不报错。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_seed_renders_the_empty_shapes_the_fns_produced() {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("inst_meta")
            .use_db("missing_seed")
            .await
            .expect("use db");

        let metas = resolve_inst_meta_on(&db, None, &[RefnoEnum::from(refu(786031))])
            .await
            .expect("resolve");
        let meta = metas.get(&refu(786031)).expect("meta");
        assert_eq!(meta.anc_literal(), "[]");
        assert_eq!(meta.dbnum_literal(), "NONE");
        assert_eq!(meta.dt_literal(), "NONE");
    }

    /// 「回退即红」源码钉（W4/D6）：生成写入路径的字面量必须是纯数据——
    /// `build_canonical_save_plan` 到 `execute_save_plan` 的生产保存路径，以及
    /// `gen_cata_geos` 的函数体里不许再出现
    /// `fn::find_ancestor_type(` / `fn::ses_date(` / `fn::anc_u64(` 内联求值。
    /// （启动自愈回填 `backfill_inst_relate_anc` 是直打持久层的非 journal 路径，
    /// 允许继续用 fn::，不在本钉范围。）
    #[test]
    fn generation_literals_are_pure_data_with_no_inline_fn_calls() {
        // `include_str!` preserves the checkout's line endings. Normalize them so this
        // source-order guard behaves identically on Windows (CRLF) and CI (LF).
        let inst_source = include_str!("pdms_inst.rs").replace("\r\n", "\n");
        let inst_body = inst_source
            .split_once("fn build_canonical_save_plan(")
            .expect("build_canonical_save_plan 必须存在")
            .1
            .split_once("\n#[cfg(test)]\nmod tests")
            .map(|(head, _)| head)
            .expect("生产保存路径到测试模块为止");
        let cata_source = include_str!("cata_model.rs");
        let cata_body = cata_source
            .split_once("pub async fn gen_cata_geos(")
            .expect("gen_cata_geos 必须存在")
            .1;

        for marker in ["fn::find_ancestor_type(", "fn::ses_date(", "fn::anc_u64("] {
            assert!(
                !inst_body.contains(marker),
                "instance 保存计划的字面量必须是已解值（D6 回退即红）: {marker}"
            );
            assert!(
                !cata_body.contains(marker),
                "gen_cata_geos 的字面量必须是已解值（D6 回退即红）: {marker}"
            );
        }
    }
}
