use std::collections::{HashMap, HashSet};

use aios_core::geometry::ShapeInstancesData;
use aios_core::pdms_types::*;
use aios_core::types::*;
use aios_core::{SUL_DB, get_db_option};
use bevy_transform::prelude::Transform;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use itertools::Itertools;

use crate::data_interface::helper::delete_inst_relate_cascade;
use crate::data_interface::tidb_manager::AiosDBManager;
// use crate::fast_model::EXIST_MESH_GEOS;

type DbWriteTask = tokio::task::JoinHandle<anyhow::Result<()>>;

fn spawn_db_write(sql: String) -> DbWriteTask {
    crate::data_interface::staging::write_context::spawn_with_staged_io(async move {
        crate::surreal_retry::execute_model_write(&sql, "save instance data").await
    })
}

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
fn render_inst_relate_replace(rows: &[(String, String)]) -> String {
    let ids = rows.iter().map(|(id, _)| id.as_str()).join(", ");
    let values = rows.iter().map(|(_, json)| json.as_str()).join(",");
    format!(
        "BEGIN TRANSACTION;\n\
         DELETE {ids};\n\
         INSERT RELATION INTO inst_relate [{values}];\n\
         COMMIT TRANSACTION;"
    )
}

async fn finish_db_writes(mut tasks: FuturesUnordered<DbWriteTask>) -> anyhow::Result<()> {
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
/// 在 fork 2.1.4 上是解析错误，又被 `let _ =` 吞掉——索引在生产从未建成，
/// `zone_refno` 过滤一直全表扫。现在用合法语法建普通索引并显式上抛错误；
/// `IF NOT EXISTS` 保证每次启动重放幂等。
pub async fn init_inst_relate_indices() -> anyhow::Result<()> {
    SUL_DB.query(INST_RELATE_INDEX_SQL).await?.check()?;
    Ok(())
}

/// 实例边表索引的唯一事实来源：生产启动（[`init_inst_relate_indices`]）与
/// 暂存库建库（`staging::lifecycle::init_staging_schema`）用同一组语句。
///
/// `anc`（RefU64 打包祖先链，数组列）与 `dbnum` 服务层级查询优化
/// （`docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`）：
/// 任意根的子树实例 = `WHERE anc CONTAINS $root` 一条索引查询。
/// 2.1.4 上数组列普通索引 + CONTAINS 走索引已由双跑套件钉住（P0，Go）。
pub const INST_RELATE_INDEX_SQL: &str = "\
    DEFINE INDEX IF NOT EXISTS idx_inst_relate_zone_refno ON TABLE inst_relate COLUMNS zone_refno;\n\
    DEFINE INDEX IF NOT EXISTS idx_inst_relate_anc ON TABLE inst_relate COLUMNS anc;\n\
    DEFINE INDEX IF NOT EXISTS idx_inst_relate_dbnum ON TABLE inst_relate COLUMNS dbnum;\n\
    DEFINE INDEX IF NOT EXISTS idx_tubi_relate_anc ON TABLE tubi_relate COLUMNS anc;\n\
    DEFINE INDEX IF NOT EXISTS idx_tubi_relate_dbnum ON TABLE tubi_relate COLUMNS dbnum;";

/// 存量 `inst_relate` / `tubi_relate` 行的 `anc` + `dbnum` 回填（幂等，自愈式）。
///
/// 每轮圈 `anc = NONE` 的一批行、按 `in` 端 pe 的活 owner 链重算，直到无行可补。
/// 分批是为了限住单事务体量；顺序无所谓，中断重跑无害。TUBI 行历史上连
/// `zone_refno` 都没有，这里一并补上。返回 (inst_relate 补行数, tubi_relate 补行数)。
pub async fn backfill_inst_relate_anc() -> anyhow::Result<(usize, usize)> {
    async fn drain_table(table: &str) -> anyhow::Result<usize> {
        const BATCH: usize = 2000;
        let mut total = 0usize;
        loop {
            let sql = format!(
                "LET $rows = SELECT VALUE id FROM {table} WHERE anc = NONE LIMIT {BATCH};\n\
                 UPDATE $rows SET anc = fn::anc_u64(in), dbnum = in.dbnum, \
                 zone_refno = zone_refno ?? fn::find_ancestor_type(in, 'ZONE') RETURN NONE;\n\
                 RETURN array::len($rows);"
            );
            let mut response = SUL_DB.query(sql).await?.check()?;
            let filled: Option<usize> = response.take(2)?;
            let filled = filled.unwrap_or(0);
            total += filled;
            if filled < BATCH {
                return Ok(total);
            }
        }
    }
    let inst = drain_table("inst_relate").await?;
    let tubi = drain_table("tubi_relate").await?;
    if inst > 0 || tubi > 0 {
        println!("anc/dbnum 回填完成：inst_relate {inst} 行，tubi_relate {tubi} 行");
    }
    Ok((inst, tubi))
}

/// P4 写时物化的脏位：本进程生成/刷新过 inst_relate 之后置位，worker 空闲轮
/// 据此决定要不要跑一轮 [`sweep_inst_relate_flat`]（避免每个空闲轮白扫整表）。
static INSTS_FLAT_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_insts_flat_dirty() {
    INSTS_FLAT_DIRTY.store(true, std::sync::atomic::Ordering::SeqCst);
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
/// （脏位门控）。行只会「缺」（NONE，读侧走 slim 兜底）不会「错」：置 meshed
/// 的生成批与建行同任务同 refno 锚点，任务成功 ⇒ 可达 geo 全 meshed|bad。
pub async fn sweep_inst_relate_flat() -> anyhow::Result<usize> {
    const BATCH: usize = 500;
    let mut total = 0usize;
    loop {
        let sql = format!(
            "LET $rows = SELECT VALUE id FROM inst_relate WHERE insts_flat = NONE AND aabb.d != none LIMIT {BATCH};\n\
             UPDATE $rows SET insts_flat = (SELECT trans.d AS transform, record::id(out) AS geo_hash \
             FROM out->geo_relate WHERE visible && out.meshed && trans.d != none && geo_type='Pos'), \
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
    }
    if total > 0 {
        println!("inst_relate 平表副本清扫完成：补 {total} 行");
    }
    Ok(total)
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

/// 一个生成行（inst_relate / tubi_relate）的渲染期元数据（W4，决议 D6）。
///
/// 这些值从前以 `fn::find_ancestor_type` / `fn::anc_u64` / `{pe}.dbnum` /
/// `fn::ses_date` 的形态**内联在 journal 字面量里**：窗口内对暂存求值一遍、
/// 写回重放时对持久层**再求值一遍**——重放硬依赖持久层灌了这些函数（与
/// issue #16 同族的故障面）。现在渲染时解一次、写死固定值，journal 变成纯数据。
#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedInstMeta {
    /// 最近的 ZONE（含自身），无则 NONE——与 `fn::find_ancestor_type(pe,'ZONE')` 同义。
    pub zone_refno: Option<RefU64>,
    /// 自身 → 顶的祖先链 RefU64 打包值（含自身）——与 `fn::anc_u64(pe)` 同义，
    /// 但走 Rust 链不吃函数展开层数的静默截断。
    pub anc: Vec<u64>,
    /// `pe.dbnum`。
    pub dbnum: Option<i64>,
    /// `fn::ses_date(pe)` 的渲染就绪字面量（`d'…'` / NONE）。
    pub dt_literal: Option<String>,
}

impl ResolvedInstMeta {
    pub(crate) fn zone_literal(&self) -> String {
        self.zone_refno
            .map(|refno| refno.to_pe_key())
            .unwrap_or_else(|| "NONE".into())
    }

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
        noun: Option<String>,
        #[serde(default)]
        dbnum: Option<i64>,
        #[serde(default)]
        sesno: Option<i64>,
    }
    type PeMeta = (Option<RefU64>, Option<String>, Option<i64>, Option<i64>);

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
                    "SELECT id, owner, noun, dbnum, sesno FROM [{keys}] WHERE record::exists(id);"
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
                cache.insert(refno, Some((owner, row.noun, row.dbnum, row.sesno)));
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

    // 2) 逐 seed 出链：zone / anc / dbnum / sesno。
    let mut out: HashMap<RefU64, ResolvedInstMeta> = HashMap::new();
    let mut ses_pairs: std::collections::BTreeSet<(i64, i64)> = std::collections::BTreeSet::new();
    for &seed in &seeds {
        let Some(Some(seed_meta)) = cache.get(&seed).cloned() else {
            // 行不在：与旧字面量对缺行的求值语义一致（fn:: 全 NONE / 空数组）。
            println!(
                "生成行元数据解析：{} 的 pe 行不在当前世界，zone/anc/dbnum/dt 按空态渲染",
                seed.to_pe_key()
            );
            out.insert(seed, ResolvedInstMeta::default());
            continue;
        };
        let mut chain = Vec::new();
        let mut zone = None;
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
            if zone.is_none() && meta.1.as_deref() == Some("ZONE") {
                zone = Some(cursor);
            }
            match meta.0 {
                Some(owner) => cursor = owner,
                None => break,
            }
        }
        if let (Some(dbnum), Some(sesno)) = (seed_meta.2, seed_meta.3) {
            ses_pairs.insert((dbnum, sesno));
        }
        out.insert(
            seed,
            ResolvedInstMeta {
                zone_refno: zone,
                anc: chain,
                dbnum: seed_meta.2,
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
            && let (Some(dbnum), Some(sesno)) = (seed_meta.2, seed_meta.3)
            && let Some(date) = ses_dates.get(&(dbnum, sesno))
            && let Some(meta) = out.get_mut(&seed)
        {
            meta.dt_literal = Some(date.clone());
        }
    }

    Ok(out)
}

///保存instance 数据到数据库（并行优化版本）
pub async fn save_instance_data(
    inst_mgr: &ShapeInstancesData,
    replace_exist: bool,
) -> anyhow::Result<()> {
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    //标识单位矩阵
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut param_map = HashMap::new();
    let mut vec3_map: HashMap<u64, String> = HashMap::new();
    let test_refno = get_db_option().get_test_refno();

    let chunk_size = 300;

    // 创建一个任务集合来管理并发操作
    let mut db_futures = FuturesUnordered::new();

    //把delete 提前，因为后面的插入都是异步的执行
    if replace_exist {
        let keys = inst_mgr.inst_info_map.keys().copied().collect::<Vec<_>>();
        delete_inst_relate_cascade(&keys, chunk_size).await?;
    }

    // W4（D6）：zone_refno / anc / dbnum / dt 在渲染时解一次、写死进字面量——
    // journal 纯数据化，写回重放不再对持久层求值任何 fn::。
    let inst_meta = {
        let mut meta_refnos: Vec<RefnoEnum> = inst_mgr.inst_info_map.keys().copied().collect();
        meta_refnos.extend(inst_mgr.inst_tubi_map.keys().copied());
        resolve_inst_meta(&meta_refnos).await?
    };
    let missing_meta = ResolvedInstMeta::default();

    let keys = inst_mgr.inst_geos_map.keys().collect::<Vec<_>>();
    let mut inst_geo_vec = vec![];
    let mut geo_relate_vec = vec![];

    // 准备inst_geo和geo_relate数据
    for k in keys {
        let v = inst_mgr.inst_geos_map.get(k).unwrap();
        for inst in &v.insts {
            if inst.transform.is_nan() {
                dbg!(&inst);
                continue;
            }
            let transform_hash = gen_bytes_hash::<_, 64>(&inst.transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&inst.transform).unwrap(),
                );
            }
            let param_hash = gen_bytes_hash::<_, 64>(&inst.geo_param);
            if !param_map.contains_key(&param_hash) {
                param_map.insert(param_hash, serde_json::to_string(&inst.geo_param).unwrap());
            }
            let key_pts = inst.geo_param.key_points();
            let mut pt_hashes = vec![];
            for k in key_pts {
                let pts_hash = k.gen_hash();
                pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
                if !vec3_map.contains_key(&pts_hash) {
                    vec3_map.insert(pts_hash, serde_json::to_string(&k).unwrap());
                }
            }
            //还需要加入geo_param的指向，param 是否填原始参数？ param=param:{}
            //使用cata_key -> inst_geos
            let cat_negs_str = if !inst.cata_neg_refnos.is_empty() {
                format!(
                    ", cata_neg: [{}]",
                    inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                )
            } else {
                "".to_string()
            };
            //如果是replace, 直接这里需要先删除之前的sql语句
            let mut relate_json = format!(
                r#"in: inst_info:⟨{0}⟩, out: inst_geo:⟨{1}⟩, trans: trans:⟨{2}⟩, geom_refno: pe:{3}, pts: [{4}], geo_type: '{5}', visible: {6} {7}"#,
                v.id(),
                inst.geo_hash,
                transform_hash,
                inst.refno,
                pt_hashes.join(","),
                inst.geo_type.to_string(),
                inst.visible,
                cat_negs_str
            );
            //将 string 转成一个 hash id
            let id = gen_bytes_hash::<_, 64>(&relate_json);
            let final_json = format!("{{ {relate_json}, id: '{id}' }}");
            geo_relate_vec.push(final_json);
            //保存 unit shape 的几何参数
            inst_geo_vec.push(inst.gen_unit_geo_sur_json());
        }
    }

    // 并发保存inst_geo数据
    if !inst_geo_vec.is_empty() {
        for chunk in inst_geo_vec.chunks(chunk_size) {
            let sql_string = format!(
                "insert ignore into {} [{}];",
                stringify!(inst_geo),
                chunk.join(",")
            );
            db_futures.push(spawn_db_write(sql_string));
        }
    }

    // 并发保存geo_relate数据
    if !geo_relate_vec.is_empty() {
        for chunk in geo_relate_vec.chunks(chunk_size) {
            let sql = format!("INSERT RELATION INTO geo_relate [{}];", chunk.join(","));
            db_futures.push(spawn_db_write(sql));
        }
    }

    // 处理tubi数据 - 创建inst_relate记录
    let keys = inst_mgr.inst_tubi_map.keys().collect::<Vec<_>>();
    let mut tubi_inst_relate_vec = vec![];

    for chunk in keys.chunks(chunk_size) {
        for &k in chunk {
            let v = inst_mgr.inst_tubi_map.get(k).unwrap();
            let aabb = v.aabb.unwrap();
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
            let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);

            // 保存aabb和transform到映射中
            if !aabb_map.contains_key(&aabb_hash) {
                aabb_map.insert(aabb_hash, serde_json::to_string(&aabb).unwrap());
            }
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }

            // 为TUBI创建inst_relate记录（zone_refno/anc/dbnum 为渲染期已解值，W4）。
            // aabb_d/world_trans_d 是指针目标 `d` 值的行内副本（P4 写时物化）：值
            // 就在内存里，渲染成纯字面量与指针同语句落行，journal 维持纯数据。
            let meta = inst_meta.get(&k.refno()).unwrap_or(&missing_meta);
            let tubi_relate_sql = format!(
                "{{id: {0},  in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, world_trans_d: {11}, aabb: aabb:⟨{4}⟩, aabb_d: {12}, generic: '{5}', zone_refno: {6}, anc: {7}, dbnum: {8}, has_cata_neg: {9}, solid: {10}}}",
                k.to_inst_relate_key(),
                k.to_pe_key(),
                v.id_str(),
                transform_hash,
                aabb_hash,
                v.generic_type.to_string(),
                meta.zone_literal(),
                meta.anc_literal(),
                meta.dbnum_literal(),
                v.has_cata_neg,
                v.is_solid,
                transform_map[&transform_hash],
                aabb_map[&aabb_hash],
            );

            if let Some(t_refno) = test_refno {
                if *k == t_refno.into() {
                    println!("TUBI inst relate sql: {}", &tubi_relate_sql);
                }
            }

            tubi_inst_relate_vec.push((k.to_inst_relate_key(), tubi_relate_sql));
        }
    }

    // TUBI 行的写入挪到下面与普通元素行一起发：两边的 id 都是 `inst_relate:{refno}`，
    // 得先都算出来才能对一次重叠（见那里的说明）。

    // 处理负关系数据并并发保存
    if !inst_mgr.neg_relate_map.is_empty() {
        let mut neg_relate_vec = vec![];
        for (k, refnos) in &inst_mgr.neg_relate_map {
            for (indx, r) in refnos.into_iter().enumerate() {
                neg_relate_vec.push(format!(
                    "{{ in: {}, id: [{}, {indx}], out: {} }}",
                    r.to_pe_key(),
                    r.to_string(),
                    k.to_pe_key(),
                ));
            }
        }
        if !neg_relate_vec.is_empty() {
            for chunk in neg_relate_vec.chunks(chunk_size) {
                let neg_relate_sql =
                    format!("INSERT RELATION INTO neg_relate [{}];", chunk.join(","));
                db_futures.push(spawn_db_write(neg_relate_sql));
            }
        }
    }

    // 处理ngmr负关系数据并并发保存
    if !inst_mgr.ngmr_neg_relate_map.is_empty() {
        let mut ngmr_relate_vec = vec![];
        for (k, refnos) in &inst_mgr.ngmr_neg_relate_map {
            let kpe = k.to_pe_key();
            for (ele_refno, ngmr_geom_refno) in refnos {
                let ele_pe = ele_refno.to_pe_key();
                let ngmr_pe = ngmr_geom_refno.to_pe_key();
                ngmr_relate_vec.push(format!(
                    "{{ in: {0}, id: [{0}, {1}, {2}], out: {1}, ngmr: {2}}}",
                    ele_pe, kpe, ngmr_pe
                ));
            }
        }
        if !ngmr_relate_vec.is_empty() {
            for chunk in ngmr_relate_vec.chunks(chunk_size) {
                let ngmr_relate_sql =
                    format!("INSERT RELATION INTO ngmr_relate [{}];", chunk.join(","));
                db_futures.push(spawn_db_write(ngmr_relate_sql));
            }
        }
    }

    // 处理inst_info数据
    let keys = inst_mgr.inst_info_map.keys().collect::<Vec<_>>();
    let mut inst_info_vec = vec![];
    let mut inst_relate_vec = vec![];

    for k in keys.clone() {
        let v = inst_mgr.inst_info_map.get(k).unwrap();
        if v.world_transform.is_nan() {
            continue;
        }
        inst_info_vec.push(v.gen_sur_json(&mut vec3_map));

        let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
        if !transform_map.contains_key(&transform_hash) {
            transform_map.insert(
                transform_hash,
                serde_json::to_string(&v.world_transform).unwrap(),
            );
        }

        // zone_refno/anc/dbnum/dt 为渲染期已解值（W4）：journal 纯数据化。
        // world_trans_d 为指针目标 `d` 值的行内副本（P4 写时物化，值在内存渲染
        // 纯字面量）；aabb 建行时尚不存在，aabb_d 随 aabb 刷新同语句写入。
        let meta = inst_meta.get(&k.refno()).unwrap_or(&missing_meta);
        let relate_sql = format!(
            "{{id: {0},  in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, world_trans_d: {11}, generic: '{4}', zone_refno: {5}, anc: {6}, dbnum: {7}, dt: {8}, has_cata_neg: {9}, solid: {10}}}",
            k.to_inst_relate_key(),
            k.to_pe_key(),
            v.id_str(),
            transform_hash,
            v.generic_type.to_string(),
            meta.zone_literal(),
            meta.anc_literal(),
            meta.dbnum_literal(),
            meta.dt_literal(),
            v.has_cata_neg,
            v.is_solid,
            transform_map[&transform_hash],
        );
        if let Some(t_refno) = test_refno {
            if *k == t_refno.into() {
                dbg!(v);
                println!("inst relate sql: {}", &relate_sql);
            }
        }
        inst_relate_vec.push((k.to_inst_relate_key(), relate_sql));
    }

    // 隐含直管段的行 id 与普通元素同样是 `inst_relate:{refno}`，而 `insert_tubi` 的键
    // 除了 BRAN 自身还有「管段离开的那个元件」。两边真撞上的话，两条替换事务会各删各写
    // 同一行，谁最后提交谁赢——那是数据错误，不是并发噪音。静态定不下来它在真实库上能否
    // 发生，所以如实喊一声：无声地丢掉一行几何，比报出来难查得多。
    let overlapping: Vec<&str> = {
        let tubi_ids: HashSet<&str> = tubi_inst_relate_vec
            .iter()
            .map(|(id, _)| id.as_str())
            .collect();
        inst_relate_vec
            .iter()
            .map(|(id, _)| id.as_str())
            .filter(|id| tubi_ids.contains(id))
            .collect()
    };
    if !overlapping.is_empty() {
        let msg = format!(
            "同一个 inst_relate id 同时被普通元素与隐含直管段写入（{} 条，例如 {}）：\
             两者只有一条能留在库里。请核对 insert_tubi 的键与 inst_info_map 的键为何重叠",
            overlapping.len(),
            overlapping.iter().take(3).join(", ")
        );
        log::error!("{msg}");
        eprintln!("{msg}");
    }

    for rows in [&inst_relate_vec, &tubi_inst_relate_vec] {
        for chunk in rows.chunks(chunk_size) {
            db_futures.push(spawn_db_write(render_inst_relate_replace(chunk)));
        }
    }

    // 并发保存inst_info数据
    if !inst_info_vec.is_empty() {
        for chunk in inst_info_vec.chunks(chunk_size) {
            let sql_string = format!(
                "insert ignore into {} [{}];",
                stringify!(inst_info),
                chunk.join(",")
            );
            db_futures.push(spawn_db_write(sql_string));
        }
    }

    // 并发保存aabb数据
    if !aabb_map.is_empty() {
        let keys = aabb_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = aabb_map.get(&k).unwrap();
                let json = format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", k, v);
                jsons.push(json);
            }
            let sql = format!("INSERT IGNORE INTO aabb [{}];", jsons.join(","));
            db_futures.push(spawn_db_write(sql));
        }
    }

    // 并发保存transform数据（优化批量插入语法）
    if !transform_map.is_empty() {
        let keys = transform_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = transform_map.get(&k).unwrap();
                jsons.push(format!("{{'id':trans:⟨{}⟩, 'd':{}}}", k, v));
            }
            let sql = format!("INSERT IGNORE INTO trans [{}];", jsons.join(","));
            db_futures.push(spawn_db_write(sql));
        }
    }

    // 并发保存vec3数据（优化批量插入语法）
    if !vec3_map.is_empty() {
        let keys = vec3_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(chunk_size) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = vec3_map.get(&k).unwrap();
                jsons.push(format!("{{'id':vec3:⟨{}⟩, 'd':{}}}", k, v));
            }
            let sql = format!("INSERT IGNORE INTO vec3 [{}];", jsons.join(","));
            db_futures.push(spawn_db_write(sql));
        }
    }

    finish_db_writes(db_futures).await?;

    // 新行的 insts_flat 建行时留空（meshed 状态未知）：置脏，空闲轮清扫收口。
    mark_insts_flat_dirty();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

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

        let mut response = SUL_DB
            .query(
                "RETURN [array::len((SELECT VALUE id FROM inst_relate WHERE anc != NONE LIMIT 1)), \
                         array::len((SELECT VALUE id FROM inst_relate WHERE anc = NONE LIMIT 1)), \
                         array::len((SELECT VALUE id FROM tubi_relate WHERE anc = NONE LIMIT 1))];",
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
        assert_eq!(inst_none, 0, "inst_relate 不应残留 anc = NONE 行");
        assert_eq!(tubi_none, 0, "tubi_relate 不应残留 anc = NONE 行");
        println!("[live] 覆盖复核通过：两表 anc 无残留 NONE");
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
            .split_once("pub async fn save_instance_data(")
            .expect("save_instance_data 必须存在")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(head, _)| head)
            .expect("函数体到测试模块为止");

        assert!(
            !body.contains("INSERT RELATION INTO inst_relate"),
            "inst_relate 必须走 render_inst_relate_replace 的替换写入: {body}"
        );
        assert!(
            body.contains("render_inst_relate_replace(chunk)"),
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
                "RETURN {} == fn::find_ancestor_type({pe}, 'ZONE');\
                 RETURN {} == fn::anc_u64({pe});\
                 RETURN {} == {pe}.dbnum;\
                 RETURN {} == fn::ses_date({pe});",
                meta.zone_literal(),
                meta.anc_literal(),
                meta.dbnum_literal(),
                meta.dt_literal(),
            ))
            .await
            .expect("parity transport")
            .check()
            .expect("parity query");
        for (index, field) in ["zone_refno", "anc", "dbnum", "dt"].iter().enumerate() {
            let equal: Option<bool> = response.take(index).expect("take flag");
            assert_eq!(
                equal,
                Some(true),
                "{field} 的已解值必须与被退役的 fn:: 求值逐值相等: {meta:?}"
            );
        }
        assert_eq!(meta.zone_refno, Some(refu(786003)));
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
    /// 错误的 zone/anc 进 journal。
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
    /// （zone NONE / anc [] / dbnum NONE / dt NONE），不报错。
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
        assert_eq!(meta.zone_literal(), "NONE");
        assert_eq!(meta.anc_literal(), "[]");
        assert_eq!(meta.dbnum_literal(), "NONE");
        assert_eq!(meta.dt_literal(), "NONE");
    }

    /// 「回退即红」源码钉（W4/D6）：生成写入路径的字面量必须是纯数据——
    /// `save_instance_data` 与 `gen_cata_geos` 的函数体里不许再出现
    /// `fn::find_ancestor_type(` / `fn::ses_date(` / `fn::anc_u64(` 内联求值。
    /// （启动自愈回填 `backfill_inst_relate_anc` 是直打持久层的非 journal 路径，
    /// 允许继续用 fn::，不在本钉范围。）
    #[test]
    fn generation_literals_are_pure_data_with_no_inline_fn_calls() {
        let inst_source = include_str!("pdms_inst.rs");
        let inst_body = inst_source
            .split_once("pub async fn save_instance_data(")
            .expect("save_instance_data 必须存在")
            .1
            .split_once("\n#[cfg(test)]")
            .map(|(head, _)| head)
            .expect("函数体到测试模块为止");
        let cata_source = include_str!("cata_model.rs");
        let cata_body = cata_source
            .split_once("pub async fn gen_cata_geos(")
            .expect("gen_cata_geos 必须存在")
            .1;

        for marker in ["fn::find_ancestor_type(", "fn::ses_date(", "fn::anc_u64("] {
            assert!(
                !inst_body.contains(marker),
                "save_instance_data 的字面量必须是已解值（D6 回退即红）: {marker}"
            );
            assert!(
                !cata_body.contains(marker),
                "gen_cata_geos 的字面量必须是已解值（D6 回退即红）: {marker}"
            );
        }
    }
}
