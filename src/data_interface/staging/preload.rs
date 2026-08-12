//! Existing generated rows needed by a staged regeneration.

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// Copy the persistent pre-window products for these roots into the active staging database.
/// Outside a staged window the persistent database already is the working set, so this is a no-op.
pub(crate) async fn preload_existing_generation_products(
    roots: &[RefnoEnum],
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() || roots.is_empty() {
        return Ok(0);
    }
    preload_existing_generation_products_from(&SUL_DB, roots).await
}

/// 一次窗口内模型修改（位姿 / 删除）所依赖的**窗口前**闭包。
///
/// 解析与拷贝拆成两步，是为了让统一根锁夹在中间：锁范围要按窗口前状态解析，而拷贝
/// 本身已经是一次 staging 模型写，必须在持锁之后（ADR-017 I8）。拆开还顺带让两件事
/// 共用同一次闭包计算——`inst_relate` 的整表扫描一个窗口只付一次。
///
/// 2026-08-07 方案 W1（D3）之后，这里只剩两类**文件里没有**的窗口前旧态：
///
/// - **旧生成产物**（inst_relate/inst_info/geo_relate/…，ADR-017 读路由规则②）；
/// - **删除子树的 pe 拓扑**：被删元素已从文件 refno 索引消失、无从解析，而删除
///   级联的暂存子树遍历（`collect_pe_subtree_refnos` → `active_data_db`）靠这份
///   拓扑圈出待清理的产物行——它与产物同类，只在持久层还有。
///
/// Transform / regen 的设计数据（pe + 名词表 + 链边）不再从持久层拷贝，改由
/// [`super::ancestor_preload`] 从 db 文件解析进暂存。
#[derive(Debug, Default)]
pub(crate) struct ModelMutationPreload {
    transform_subtree_len: usize,
    delete_subtree_len: usize,
    model_refnos: Vec<RefnoEnum>,
    transform_model_refnos: Vec<RefnoEnum>,
    delete_hierarchy: Vec<RefnoEnum>,
}

impl ModelMutationPreload {
    /// 子树里**带模型产物**的节点（Transform ∪ Delete）。本窗口要改的正是它们的
    /// 产物，所以统一根锁要覆盖的也正是它们各自所属的生成根——子树里其余节点没有
    /// 产物，锁它们的根是空付出。
    pub(crate) fn model_refnos(&self) -> &[RefnoEnum] {
        &self.model_refnos
    }

    /// Transform 子树里带模型产物的节点——祖先解析式预载的种子。删除子树覆盖的
    /// 节点（含两桶重叠的部分）刻意不在此列：被删元素已从文件消失，解析必败且
    /// 重排不自愈；其产物由删除级联清理（2026-08-07 审核 P1）。
    pub(crate) fn transform_model_refnos(&self) -> &[RefnoEnum] {
        &self.transform_model_refnos
    }
}

/// Copy the authoritative state captured before entering the staged read context.
pub(crate) async fn preload_dbnum_state(
    state: &crate::data_interface::dbnum_state::DbnumState,
) -> anyhow::Result<()> {
    if super::active_staging_writes().is_none() {
        return Ok(());
    }
    let string = |value: &str| serde_json::to_string(value).expect("DBNUM strings serialize");
    crate::surreal_retry::execute_generation_preload(
        &format!(
            "CREATE dbnum_watermark:{dbnum} SET dbnum={dbnum}, owner_project={owner}, \
             db_type={db_type}, file_name={file_name}, file_path={file_path}, file_size={file_size}, \
             file_latest_sesno={file_latest}, applied_sesno={applied}, sesno={applied};",
            dbnum = state.dbnum,
            owner = string(&state.owner_project),
            db_type = string(&state.db_type),
            file_name = string(&state.file_name),
            file_path = string(&state.file_path),
            file_size = state.file_size,
            file_latest = state.file_latest_sesno,
            applied = state.applied_sesno,
        ),
        "preload dbnum_watermark",
    )
    .await
}

/// 只读地解析闭包：不碰暂存库，可以在持锁之前跑。
///
/// Transform 与 Delete 分桶传入：两桶的模型节点合并成锁范围与产物拷贝范围
/// （一次 `inst_relate` 扫描共用），Transform 桶单独交出祖先解析种子，Delete 桶
/// 单独交出持久层 pe 拓扑拷贝范围（见 [`ModelMutationPreload`]）。
pub(crate) async fn plan_model_mutation_preload(
    transform_targets: &[RefnoEnum],
    delete_targets: &[RefnoEnum],
) -> anyhow::Result<ModelMutationPreload> {
    if super::active_staging_writes().is_none()
        || (transform_targets.is_empty() && delete_targets.is_empty())
    {
        return Ok(ModelMutationPreload::default());
    }
    plan_model_mutation_preload_from(&SUL_DB, transform_targets, delete_targets).await
}

async fn plan_model_mutation_preload_from(
    source: &Surreal<Any>,
    transform_targets: &[RefnoEnum],
    delete_targets: &[RefnoEnum],
) -> anyhow::Result<ModelMutationPreload> {
    let transform_subtree = load_root_refnos(source, transform_targets).await?;
    let delete_subtree = load_root_refnos(source, delete_targets).await?;
    if transform_subtree.is_empty() && delete_subtree.is_empty() {
        return Ok(ModelMutationPreload::default());
    }
    let mut union_subtree = transform_subtree.clone();
    union_subtree.extend_from_slice(&delete_subtree);
    union_subtree.sort_unstable();
    union_subtree.dedup();
    let model_refnos = load_model_refnos(source, &union_subtree).await?;

    let transform_scope = transform_subtree
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let delete_scope = delete_subtree
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    // Transform 子树与 Delete 子树重叠的节点不进祖先解析种子（2026-08-07 审核 P1）：
    // 被删元素已从文件 refno 索引消失，交给 ancestor_preload 解析必然「祖先链断裂」
    // 整批 fail-closed，且重排不自愈——扩窗吸收不会让被删元素回到文件索引。
    // 它们的产物仍在 model_refnos（锁范围与拷贝范围）里，由删除级联清理。
    let transform_model_refnos = model_refnos
        .iter()
        .copied()
        .filter(|refno| transform_scope.contains(refno) && !delete_scope.contains(refno))
        .collect::<Vec<_>>();

    let delete_hierarchy = if delete_targets.is_empty() {
        Vec::new()
    } else {
        let mut hierarchy_seeds = model_refnos
            .iter()
            .copied()
            .filter(|refno| delete_scope.contains(refno))
            .collect::<Vec<_>>();
        hierarchy_seeds.extend_from_slice(delete_targets);
        let mut hierarchy = crate::data_interface::helper::collect_pe_ancestor_refnos_from(
            source,
            &hierarchy_seeds,
        )
        .await?
        .into_iter()
        .collect::<Vec<_>>();
        hierarchy.sort_unstable();
        hierarchy
    };

    Ok(ModelMutationPreload {
        transform_subtree_len: transform_subtree.len(),
        delete_subtree_len: delete_subtree.len(),
        model_refnos,
        transform_model_refnos,
        delete_hierarchy,
    })
}

/// Copy the pre-window products (and the delete subtrees' PE topology) into staging.
/// `INSERT IGNORE` preserves rows already rewritten by the current parse.
///
/// Transform / regen 的 pe+pe_owner 持久层拷贝已退役（W1/D3）：那部分设计数据由
/// [`super::ancestor_preload`] 从 db 文件解析进暂存。
pub(crate) async fn apply_model_mutation_preload(
    preload: &ModelMutationPreload,
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none()
        || (preload.model_refnos.is_empty() && preload.delete_hierarchy.is_empty())
    {
        return Ok(0);
    }
    apply_model_mutation_preload_from(&SUL_DB, preload).await
}

async fn apply_model_mutation_preload_from(
    source: &Surreal<Any>,
    preload: &ModelMutationPreload,
) -> anyhow::Result<usize> {
    let started = std::time::Instant::now();
    let mut copied = 0usize;
    if !preload.delete_hierarchy.is_empty() {
        let keys = preload
            .delete_hierarchy
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        // 按记录 id 直接寻址，**不要**写成 `WHERE id IN [...]`：那是全表扫描加过滤，
        // 不走主键。本项目 `pe` 有 895 万行，取这 4 条实测 64.4 秒，直接寻址 0.5 毫秒
        // ——一次删除子树的窗口有 96% 的时间耗在这一句上。
        // 键本身已经是 `pe:xxx` 的完整形态，表名在 FROM 里是多余的。
        copied += copy_rows(source, "pe", &format!("SELECT * FROM {keys}")).await?;
        copied += copy_relations(
            source,
            "pe_owner",
            &format!("SELECT * FROM pe_owner WHERE in IN [{keys}] AND out IN [{keys}]"),
        )
        .await?;
    }
    let topology_elapsed = started.elapsed();
    copied +=
        preload_existing_generation_products_for_refnos(source, &preload.model_refnos).await?;
    println!(
        "暂存 mutation 预载: transform_subtree={} delete_subtree={} model={} \
         transform_model={} delete_hierarchy={} copied={}，delete 拓扑={:?} total={:?}",
        preload.transform_subtree_len,
        preload.delete_subtree_len,
        preload.model_refnos.len(),
        preload.transform_model_refnos.len(),
        preload.delete_hierarchy.len(),
        copied,
        topology_elapsed,
        started.elapsed()
    );
    Ok(copied)
}

async fn load_model_refnos(
    source: &Surreal<Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<RefnoEnum>> {
    let scope = refnos
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    // ponytail: one narrow relation scan beats one graph endpoint lookup per PE; add an `in`
    // endpoint index/query API if inst_relate itself grows beyond memory-budget measurements.
    let mut response = source
        .query("SELECT VALUE in FROM inst_relate;")
        .await?
        .check()?;
    let mut model_refnos = response.take::<Vec<RefnoEnum>>(0)?;
    model_refnos.retain(|refno| scope.contains(refno));
    model_refnos.sort_unstable();
    model_refnos.dedup();
    Ok(model_refnos)
}

/// Reparse the current root subtree and its catalogue references into staging.
/// Persistent ids recover unchanged descendants; the staged traversal adds nodes born in this window.
pub(crate) async fn preload_generation_root_closure(
    project: &str,
    roots: &[RefnoEnum],
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() || roots.is_empty() {
        return Ok(0);
    }
    let mut refnos = load_root_refnos(&SUL_DB, roots).await?;
    for root in roots {
        refnos.extend(aios_core::query_deep_children_refnos(*root).await?);
        refnos.push(*root);
    }
    refnos.sort_unstable();
    refnos.dedup();
    let seeds = refnos.iter().map(RefnoEnum::refno).collect::<Vec<RefU64>>();
    let outcome =
        crate::data_interface::cata_closure::ensure_cata_refnos_parsed(project, &seeds).await?;
    Ok(outcome.parsed)
}

/// Persistent pre-window descendants of generation roots.  Staged modified
/// rows may not carry an unchanged `pe_owner` edge, so the live window alone
/// cannot enumerate all primitives whose partial ATT rows need file backfill.
pub(crate) async fn persistent_generation_subtree(
    roots: &[RefnoEnum],
) -> anyhow::Result<Vec<RefnoEnum>> {
    load_root_refnos(&SUL_DB, roots).await
}

/// Enumerate descendants from the authoritative `pe.owner` field in the
/// active window.  This also finds modified/new rows when their unchanged
/// `pe_owner` relation was not materialized into the staging database.
pub(crate) async fn active_generation_subtree_by_owner(
    roots: &[RefnoEnum],
) -> anyhow::Result<Vec<RefnoEnum>> {
    let db = super::active_data_db();
    let mut all = roots.to_vec();
    let mut frontier = roots.to_vec();
    for _ in 0..=SUBTREE_CLOSURE_DEPTH {
        if frontier.is_empty() {
            break;
        }
        let owners = frontier
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = db
            .query(format!(
                "SELECT VALUE id FROM pe WHERE owner IN [{owners}] AND deleted != true;"
            ))
            .await?
            .check()?;
        let mut next = response.take::<Vec<RefnoEnum>>(0)?;
        next.retain(|refno| !all.contains(refno));
        if next.is_empty() {
            break;
        }
        all.extend(next.iter().copied());
        frontier = next;
    }
    all.sort_unstable();
    all.dedup();
    Ok(all)
}

async fn preload_existing_generation_products_from(
    source: &Surreal<Any>,
    roots: &[RefnoEnum],
) -> anyhow::Result<usize> {
    let refnos = load_root_refnos(source, roots).await?;
    preload_existing_generation_products_for_refnos(source, &refnos).await
}

async fn preload_existing_generation_products_for_refnos(
    source: &Surreal<Any>,
    refnos: &[RefnoEnum],
) -> anyhow::Result<usize> {
    if refnos.is_empty() {
        return Ok(0);
    }

    let pe_keys = refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    // 边从起点走图拿，**不要**写成 `WHERE in IN [...]`：`inst_relate` 上没有 `in` 的
    // 索引（只有 anc / dbnum），那个谓词是 11.3 万行的全表扫，取 1 条边
    // 实测 1.57s，走 `->inst_relate` 是 3.1ms。`geo_relate` 更彻底——它一个索引都没有。
    let inst_edges = format!("array::flatten(SELECT VALUE ->inst_relate FROM [{pe_keys}])");
    let inst_query = format!("SELECT * FROM {inst_edges}");
    let info_keys =
        select_record_keys(source, &format!("SELECT VALUE out FROM {inst_edges}")).await?;
    let world_keys = select_record_keys(
        source,
        &format!("SELECT VALUE world_trans FROM {inst_edges} WHERE world_trans != NONE"),
    )
    .await?;
    let mut aabb_keys = select_record_keys(
        source,
        &format!("SELECT VALUE aabb FROM {inst_edges} WHERE aabb != NONE"),
    )
    .await?;
    let mut copied = copy_relations(source, "inst_relate", &inst_query).await?;
    if info_keys.is_empty() {
        return Ok(copied);
    }

    let info_scope = info_keys.join(",");
    let geo_edges = format!("array::flatten(SELECT VALUE ->geo_relate FROM [{info_scope}])");
    let geo_query = format!("SELECT * FROM {geo_edges}");
    let inst_geo_keys =
        select_record_keys(source, &format!("SELECT VALUE out FROM {geo_edges}")).await?;
    let mut trans_keys = select_record_keys(
        source,
        &format!("SELECT VALUE trans FROM {geo_edges} WHERE trans != NONE"),
    )
    .await?;
    // 真实库的 `inst_relate.world_trans` 指向 `trans:*`；旧夹具也出现过
    // `world_trans:*`。按记录自身的表拷贝，不能把 `trans:*` 拿去查 `world_trans` 表。
    let (world_keys, world_trans_keys): (Vec<_>, Vec<_>) = world_keys
        .into_iter()
        .partition(|key| key.starts_with("trans:"));
    trans_keys.extend(world_keys);
    trans_keys.sort_unstable();
    trans_keys.dedup();
    // 直接寻址，理由同 `apply_model_mutation_preload_from` 里那条注释。
    copied += copy_rows(source, "inst_info", &format!("SELECT * FROM {info_scope}")).await?;
    copied += copy_relations(source, "geo_relate", &geo_query).await?;

    let mut vec_keys = Vec::new();
    if !inst_geo_keys.is_empty() {
        let inst_geo_scope = inst_geo_keys.join(",");
        vec_keys = select_record_keys(
            source,
            &format!("RETURN array::flatten(SELECT VALUE pts FROM {inst_geo_scope});"),
        )
        .await?;
        aabb_keys.extend(
            select_record_keys(
                source,
                &format!("SELECT VALUE aabb FROM {inst_geo_scope} WHERE aabb != NONE"),
            )
            .await?,
        );
        aabb_keys.sort_unstable();
        aabb_keys.dedup();
        copied += copy_rows(
            source,
            "inst_geo",
            &format!("SELECT * FROM {inst_geo_scope}"),
        )
        .await?;
    }

    for (table, keys) in [
        ("world_trans", world_trans_keys),
        ("trans", trans_keys),
        ("vec3", vec_keys),
        ("aabb", aabb_keys),
    ] {
        if !keys.is_empty() {
            copied +=
                copy_rows(source, table, &format!("SELECT * FROM {}", keys.join(","))).await?;
        }
    }
    Ok(copied)
}

async fn select_record_keys(source: &Surreal<Any>, query: &str) -> anyhow::Result<Vec<String>> {
    let mut response = source.query(query).await?.check()?;
    let value: surrealdb::Value = response.take(0)?;
    let mut keys = Vec::new();
    collect_record_keys(value.into_inner(), &mut keys)?;
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

fn collect_record_keys(value: surrealdb::sql::Value, keys: &mut Vec<String>) -> anyhow::Result<()> {
    match value {
        surrealdb::sql::Value::Thing(thing) => keys.push(thing.to_string()),
        surrealdb::sql::Value::Array(values) => {
            for value in values {
                collect_record_keys(value, keys)?;
            }
        }
        surrealdb::sql::Value::None | surrealdb::sql::Value::Null => {}
        value => anyhow::bail!("record-key query returned {value} instead of a record id"),
    }
    Ok(())
}

/// 子树闭包的服务端图查询深度（`p1..=p11`，外加 `p0` 的根自身）。
///
/// 祖先方向的链遍历有 `MAX_ANCESTOR_DEPTH = 32` 兜底，这个向下的方向却是**硬截断**：
/// 第 12 层真有行时闭包就不完整——预载缺行、统一根锁漏根，而且没有任何报错。所以
/// [`load_root_refnos`] 带一个溢出探针，宁可整批 fail-closed 也不静默截断；真撞上了，
/// 把这个常数加深即可（查询按层展开，成本随深度线性）。
const SUBTREE_CLOSURE_DEPTH: usize = 11;

async fn load_root_refnos(
    source: &Surreal<Any>,
    roots: &[RefnoEnum],
) -> anyhow::Result<Vec<RefnoEnum>> {
    const HOP: &str = "<-pe_owner<-(?)";
    let levels = std::iter::once("p0: [id]".to_string())
        .chain((1..=SUBTREE_CLOSURE_DEPTH).map(|depth| format!("p{depth}: {}", HOP.repeat(depth))))
        .collect::<Vec<_>>()
        .join(", ");
    let probe = HOP.repeat(SUBTREE_CLOSURE_DEPTH + 1);
    let mut refnos = Vec::new();
    for roots in roots.chunks(100) {
        let keys = roots
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = source
            .query(format!(
                "RETURN array::flatten(SELECT VALUE array::flatten(object::values({{ {levels} }})) \
                 FROM [{keys}] WHERE record::exists(id));\n\
                 RETURN [array::len(array::flatten(SELECT VALUE {probe} \
                 FROM [{keys}] WHERE record::exists(id)))];"
            ))
            .await?
            .check()?;
        refnos.extend(response.take::<Vec<RefnoEnum>>(0)?);
        let overflow = response
            .take::<Vec<usize>>(1)?
            .into_iter()
            .next()
            .unwrap_or(0);
        if overflow > 0 {
            anyhow::bail!(
                "子树闭包在第 {} 层仍有 {overflow} 行（根样本: {}）：超出 SUBTREE_CLOSURE_DEPTH，\
                 拒绝静默截断——预载缺行会让统一根锁漏根、暂存读回落到错误状态",
                SUBTREE_CLOSURE_DEPTH + 1,
                roots
                    .iter()
                    .take(3)
                    .map(RefnoEnum::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    refnos.sort_unstable();
    refnos.dedup();
    Ok(refnos)
}

async fn copy_rows(source: &Surreal<Any>, table: &str, query: &str) -> anyhow::Result<usize> {
    let mut response = source.query(query).await?.check()?;
    let value: surrealdb::Value = response.take(0)?;
    let value = value.into_inner();
    let count = match &value {
        surrealdb::sql::Value::Array(rows) => rows.len(),
        _ => 0,
    };
    if count > 0 {
        let literal = render_preload_value(&value);
        crate::surreal_retry::execute_generation_preload(
            &format!("INSERT IGNORE INTO {table} {literal};"),
            &format!("preload {table}"),
        )
        .await?;
    }
    Ok(count)
}

async fn copy_relations(source: &Surreal<Any>, table: &str, query: &str) -> anyhow::Result<usize> {
    let mut response = source.query(query).await?.check()?;
    let value: surrealdb::Value = response.take(0)?;
    let rows = match value.into_inner() {
        surrealdb::sql::Value::Array(rows) => rows
            .into_iter()
            .map(|row| match row {
                surrealdb::sql::Value::Object(row) => Ok(row),
                _ => anyhow::bail!("preload {table} returned a non-object relation row"),
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        _ => anyhow::bail!("preload {table} returned a non-array relation result"),
    };
    let mut statements = Vec::with_capacity(rows.len());
    for mut row in rows {
        let id = row.remove("id").and_then(|value| match value {
            surrealdb::sql::Value::Thing(value) => Some(value),
            _ => None,
        });
        let input = row.remove("in").and_then(|value| match value {
            surrealdb::sql::Value::Thing(value) => Some(value),
            _ => None,
        });
        let output = row.remove("out").and_then(|value| match value {
            surrealdb::sql::Value::Thing(value) => Some(value),
            _ => None,
        });
        let (Some(id), Some(input), Some(output)) = (id, input, output) else {
            anyhow::bail!("preload {table} returned a row without relation id/in/out");
        };
        statements.push(format!(
            "IF !record::exists({id}) {{ RELATE {input}->{id}->{output} CONTENT {}; }};",
            render_preload_value(&surrealdb::sql::Value::Object(row))
        ));
    }
    let count = statements.len();
    let batches = relation_statement_batches(&statements);
    let batch_count = batches.len();
    if batch_count > 1 {
        println!(
            "preload {table}: {count} 条关系拆为 {batch_count} 批（每批最多 {RELATION_PRELOAD_MAX_ROWS} 行 / {RELATION_PRELOAD_MAX_BYTES} 字节）"
        );
    }
    for (index, sql) in batches.into_iter().enumerate() {
        crate::surreal_retry::execute_generation_preload(
            &sql,
            &format!("preload {table} batch {}/{}", index + 1, batch_count),
        )
        .await?;
        if batch_count > 1 && (index == 0 || index + 1 == batch_count || (index + 1) % 25 == 0) {
            println!("preload {table}: 已完成 {}/{} 批", index + 1, batch_count);
        }
    }
    Ok(count)
}

/// kv-mem 会先解析完整的多语句请求；把整张 `room_relate`（AMS 已超过八万行）拼成
/// 一条请求会长期占住暂存执行器，看起来像批次无响应。行数与字节数双限，既限制解析
/// 峰值，也避免少量大 CONTENT 行重新制造同类问题。单条语句本身超过字节上限时仍单独
/// 成批，保持数据完整性并让 SurrealDB 返回明确结果。
const RELATION_PRELOAD_MAX_ROWS: usize = 500;
const RELATION_PRELOAD_MAX_BYTES: usize = 256 * 1024;

fn relation_statement_batches(statements: &[String]) -> Vec<String> {
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut rows = 0usize;

    for statement in statements {
        let separator_bytes = usize::from(!current.is_empty());
        let would_exceed_bytes = !current.is_empty()
            && current
                .len()
                .saturating_add(separator_bytes)
                .saturating_add(statement.len())
                > RELATION_PRELOAD_MAX_BYTES;
        if !current.is_empty() && (rows >= RELATION_PRELOAD_MAX_ROWS || would_exceed_bytes) {
            batches.push(std::mem::take(&mut current));
            rows = 0;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(statement);
        rows += 1;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// 把一个 Surreal 值渲染成可嵌入语句的字面量（字符串走 JSON 转义，datetime →
/// `d'…'`，NONE → NONE）。预载拷贝与 W4 的已解值渲染共用同一份口径。
pub(crate) fn render_preload_value(value: &surrealdb::sql::Value) -> String {
    match value {
        surrealdb::sql::Value::Strand(value) => {
            serde_json::to_string(value.as_str()).expect("Surreal strings are JSON serializable")
        }
        surrealdb::sql::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(render_preload_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        surrealdb::sql::Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    serde_json::to_string(key).expect("object keys are JSON serializable"),
                    render_preload_value(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        value => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;

    #[test]
    fn relation_preload_batches_bound_rows_and_bytes_without_loss() {
        let statements = (0..1_201)
            .map(|index| format!("RELATE pe:{index}->room_relate:r{index}->pe:{};", index + 1))
            .collect::<Vec<_>>();
        let batches = relation_statement_batches(&statements);

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].lines().count(), RELATION_PRELOAD_MAX_ROWS);
        assert_eq!(batches[1].lines().count(), RELATION_PRELOAD_MAX_ROWS);
        assert_eq!(batches[2].lines().count(), 201);
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.lines().count())
                .sum::<usize>(),
            statements.len()
        );
        assert!(
            batches
                .iter()
                .all(|batch| batch.len() <= RELATION_PRELOAD_MAX_BYTES)
        );

        let large = vec!["X".repeat(140 * 1024), "Y".repeat(140 * 1024)];
        let byte_batches = relation_statement_batches(&large);
        assert_eq!(byte_batches.len(), 2, "字节上限也必须触发拆批");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_window_sees_the_persistent_watermark() {
        let instance = connect("mem://").await.expect("staging mem");
        let window = create_window_on(&instance, 7999, 43, 43, ResourceThresholds::default())
            .await
            .expect("window");
        let state = crate::data_interface::dbnum_state::DbnumState {
            dbnum: 7999,
            applied_sesno: 42,
            initialized: true,
            ..Default::default()
        };
        window
            .scope(preload_dbnum_state(&state))
            .await
            .expect("preload watermark");

        let mut response = window
            .staging_db()
            .query("RETURN dbnum_watermark:7999.applied_sesno;")
            .await
            .expect("inspect watermark");
        assert_eq!(
            response.take::<Option<i32>>(0).expect("watermark"),
            Some(42)
        );
        assert!(window.journal().await.is_empty());
        window.drop_database().await.expect("cleanup");
    }
    use crate::data_interface::staging::lifecycle::create_window_on;
    use surrealdb::engine::any::connect;

    /// 子树闭包的深度是查询按层展开出来的**硬上限**：第 12 层真有行时，闭包缺行、
    /// 统一根锁漏根，而旧实现一声不响。溢出探针必须把它变成整批失败。
    ///
    /// 链深恰好压线（11 跳）时照常返回完整闭包——探针不许把合法深度误杀。
    #[tokio::test(flavor = "multi_thread")]
    async fn a_subtree_deeper_than_the_closure_ceiling_fails_closed() {
        let source = connect("mem://").await.expect("source mem");
        source
            .use_ns("test")
            .use_db("source")
            .await
            .expect("use source");
        // 根 200，链式后代 201..=211：正好 11 跳，压线合法。
        let mut fixture = String::from("CREATE pe:⟨4000000001_200⟩ SET deleted=false;");
        for seq in 201..=211 {
            fixture.push_str(&format!(
                "CREATE pe:⟨4000000001_{seq}⟩ SET deleted=false; \
                 RELATE pe:⟨4000000001_{seq}⟩->pe_owner->pe:⟨4000000001_{}⟩;",
                seq - 1
            ));
        }
        source
            .query(fixture)
            .await
            .expect("fixture transport")
            .check()
            .expect("fixture");

        let root = RefnoEnum::from("4000000001/200");
        let closure = load_root_refnos(&source, &[root])
            .await
            .expect("11 跳压线的闭包必须成功");
        assert_eq!(closure.len(), 12, "根 + 11 层后代一个都不能少: {closure:?}");

        // 第 12 跳出现 → 拒绝静默截断。
        source
            .query(
                "CREATE pe:⟨4000000001_212⟩ SET deleted=false; \
                 RELATE pe:⟨4000000001_212⟩->pe_owner->pe:⟨4000000001_211⟩;",
            )
            .await
            .expect("deepen transport")
            .check()
            .expect("deepen");
        let error = load_root_refnos(&source, &[root])
            .await
            .expect_err("超过闭包深度必须整批失败，而不是静默截断");
        assert!(
            error.to_string().contains("SUBTREE_CLOSURE_DEPTH"),
            "{error:#}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copies_only_the_root_product_closure_without_journaling() {
        let source = connect("mem://").await.expect("source mem");
        source
            .use_ns("test")
            .use_db("source")
            .await
            .expect("use source");
        source.query(
            "CREATE pe:⟨4000000001_1⟩ SET deleted=false; CREATE pe:⟨4000000001_2⟩ SET deleted=false;
             CREATE pe:⟨4000000001_3⟩ SET deleted=false;
             CREATE pe:⟨4000000001_9⟩ SET deleted=false;
             RELATE pe:⟨4000000001_2⟩->pe_owner->pe:⟨4000000001_1⟩;
             RELATE pe:⟨4000000001_3⟩->pe_owner->pe:⟨4000000001_2⟩;
             CREATE trans:wt1 SET d=[1]; CREATE trans:t1 SET d=[2]; CREATE aabb:a1 SET d={x:1};
             CREATE vec3:v1 SET d=[1,2,3]; CREATE inst_info:i1 SET noun='PIPE';
             CREATE inst_geo:g1 SET aabb=aabb:a1, pts=[vec3:v1];
             RELATE pe:⟨4000000001_3⟩->inst_relate->inst_info:i1 SET world_trans=trans:wt1, aabb=aabb:a1;
             RELATE inst_info:i1->geo_relate->inst_geo:g1 SET trans=trans:t1;
             CREATE inst_info:i9; RELATE pe:⟨4000000001_9⟩->inst_relate->inst_info:i9;"
        ).await.expect("fixture transport").check().expect("fixture");

        let loaded = load_root_refnos(
            &source,
            &[
                RefnoEnum::from("4000000001/1"),
                RefnoEnum::from("4000000001/9"),
            ],
        )
        .await
        .expect("batch root closure");
        assert_eq!(
            loaded,
            vec![
                RefnoEnum::from("4000000001/1"),
                RefnoEnum::from("4000000001/2"),
                RefnoEnum::from("4000000001/3"),
                RefnoEnum::from("4000000001/9"),
            ]
        );
        assert_eq!(
            select_record_keys(&source, "RETURN [inst_info:i1, NONE, [inst_info:i1]];")
                .await
                .expect("record keys ignore absent optional links"),
            vec!["inst_info:i1"]
        );

        let instance = connect("mem://").await.expect("staging mem");
        let window = create_window_on(&instance, 7992, 1, 1, ResourceThresholds::default())
            .await
            .expect("window");
        let copied = window
            .scope(preload_existing_generation_products_from(
                &source,
                &[RefnoEnum::from("4000000001/1")],
            ))
            .await
            .expect("preload products");

        assert_eq!(copied, 8);
        assert!(window.journal().await.is_empty());
        let mut response = window
            .staging_db()
            .query(
                "RETURN [count(SELECT * FROM inst_relate) = 1, inst_info:i1.id != NONE,
             count(SELECT * FROM geo_relate) = 1, inst_geo:g1.id != NONE,
             trans:wt1.id != NONE, trans:t1.id != NONE, aabb:a1.id != NONE,
             vec3:v1.id != NONE, inst_info:i9.id = NONE];",
            )
            .await
            .expect("inspect staging");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 9]);

        // Transform 桶：设计数据（pe + pe_owner）不再从持久层拷（W1/D3——那部分
        // 改由 ancestor_preload 从文件解析），apply 只剩产物；Transform 子树的
        // 模型节点单独成桶，交给祖先解析当种子。
        let plan = window
            .scope(plan_model_mutation_preload_from(
                &source,
                &[RefnoEnum::from("4000000001/1")],
                &[],
            ))
            .await
            .expect("plan transform preload");
        assert_eq!(
            plan.transform_model_refnos(),
            &[RefnoEnum::from("4000000001/3")],
            "Transform 子树的模型节点必须单独成桶（祖先解析种子）"
        );
        let copied = window
            .scope(apply_model_mutation_preload_from(&source, &plan))
            .await
            .expect("preload transform target");
        assert_eq!(copied, 8, "Transform 桶只拷产物，不再拷 pe/pe_owner");
        let mut response = window
            .staging_db()
            .query(
                "RETURN [pe:⟨4000000001_1⟩.id = NONE, pe:⟨4000000001_2⟩.id = NONE,
                 pe:⟨4000000001_3⟩.id = NONE, count(SELECT * FROM pe_owner) = 0];",
            )
            .await
            .expect("inspect transform rows");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 4]);

        // Delete 桶：被删元素已从文件消失、无从解析，删除级联的暂存子树遍历
        // （collect_pe_subtree_refnos → active_data_db）靠这份持久层拓扑拷贝。
        let plan = window
            .scope(plan_model_mutation_preload_from(
                &source,
                &[],
                &[RefnoEnum::from("4000000001/1")],
            ))
            .await
            .expect("plan delete preload");
        let copied = window
            .scope(apply_model_mutation_preload_from(&source, &plan))
            .await
            .expect("preload delete target");
        assert_eq!(copied, 13, "Delete 桶保留 pe 拓扑 + 产物");
        let mut response = window
            .staging_db()
            .query(
                "RETURN [pe:⟨4000000001_1⟩.id != NONE, pe:⟨4000000001_2⟩.id != NONE,
                 pe:⟨4000000001_3⟩.id != NONE, count(SELECT * FROM pe_owner) = 2,
                 pe:⟨4000000001_9⟩.id = NONE];",
            )
            .await
            .expect("inspect delete rows");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 5]);
        // 级联删除的枚举入口必须能在暂存里从删除目标走到带产物的后代。
        let staged_subtree = window
            .scope(crate::data_interface::helper::collect_pe_subtree_refnos(&[
                RefnoEnum::from("4000000001/1"),
            ]))
            .await
            .expect("staged subtree walk");
        assert!(
            staged_subtree.contains(&RefnoEnum::from("4000000001/3")),
            "删除级联必须能沿暂存拓扑找到带产物的后代: {staged_subtree:?}"
        );
        assert!(window.journal().await.is_empty());
        window.drop_database().await.expect("cleanup");
    }

    /// P1 修复钉（2026-08-07 审核）：Transform 子树与 Delete 子树**重叠**的节点
    /// 不得进祖先解析种子桶。
    ///
    /// 场景：容器只有纯位姿变更（Transform 目标），其子树里某个带产物的后代在
    /// 同一窗口被删（DeleteCleanup 目标）。子树按窗口前持久态遍历，被删节点
    /// 仍在场且带产物——不排除的话它会进 `ancestor_seed_refnos`，而被删元素
    /// 已从文件 refno 索引消失，`ancestor_preload` 解析必然「祖先链断裂」，
    /// 整批 fail-closed；水位不动 → 重排同一窗口 → 同样失败，扩窗吸收也不会
    /// 让被删元素回到文件索引——该 dbnum 永久阻塞。
    ///
    /// 重叠节点的产物本就由删除级联清理，Transform 刷它们无意义；但它必须留在
    /// `model_refnos`（锁范围与产物拷贝范围）里——级联要清的正是它的产物。
    #[tokio::test(flavor = "multi_thread")]
    async fn overlapping_delete_subtree_nodes_never_become_ancestor_seeds() {
        let source = connect("mem://").await.expect("source mem");
        source
            .use_ns("test")
            .use_db("overlap_source")
            .await
            .expect("use source");
        // 1 ← 2 ← 3（3 带产物）：1 是纯位姿 Transform 目标，2 在同窗被删。
        source
            .query(
                "CREATE pe:⟨4000000001_1⟩ SET deleted=false; CREATE pe:⟨4000000001_2⟩ SET deleted=false;
                 CREATE pe:⟨4000000001_3⟩ SET deleted=false;
                 RELATE pe:⟨4000000001_2⟩->pe_owner->pe:⟨4000000001_1⟩;
                 RELATE pe:⟨4000000001_3⟩->pe_owner->pe:⟨4000000001_2⟩;
                 CREATE inst_info:i1; RELATE pe:⟨4000000001_3⟩->inst_relate->inst_info:i1;",
            )
            .await
            .expect("fixture transport")
            .check()
            .expect("fixture");

        let plan = plan_model_mutation_preload_from(
            &source,
            &[RefnoEnum::from("4000000001/1")],
            &[RefnoEnum::from("4000000001/2")],
        )
        .await
        .expect("plan overlap preload");

        assert_eq!(
            plan.transform_model_refnos(),
            &[] as &[RefnoEnum],
            "Delete 子树覆盖的节点必须从祖先解析种子里排除（文件里已无从解析）"
        );
        assert_eq!(
            plan.model_refnos(),
            &[RefnoEnum::from("4000000001/3")],
            "锁范围与产物拷贝仍要覆盖重叠节点——删除级联要清它的产物"
        );
    }
}
