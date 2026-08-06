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
#[derive(Debug, Default)]
pub(crate) struct ModelMutationPreload {
    subtree_len: usize,
    model_refnos: Vec<RefnoEnum>,
    hierarchy: Vec<RefnoEnum>,
}

impl ModelMutationPreload {
    /// 子树里**带模型产物**的节点。本窗口要改的正是它们的产物，所以统一根锁要覆盖的
    /// 也正是它们各自所属的生成根——子树里其余节点没有产物，锁它们的根是空付出。
    pub(crate) fn model_refnos(&self) -> &[RefnoEnum] {
        &self.model_refnos
    }
}

/// 只读地解析闭包：不碰暂存库，可以在持锁之前跑。
pub(crate) async fn plan_model_mutation_preload(
    roots: &[RefnoEnum],
) -> anyhow::Result<ModelMutationPreload> {
    if super::active_staging_writes().is_none() || roots.is_empty() {
        return Ok(ModelMutationPreload::default());
    }
    plan_model_mutation_preload_from(&SUL_DB, roots).await
}

async fn plan_model_mutation_preload_from(
    source: &Surreal<Any>,
    roots: &[RefnoEnum],
) -> anyhow::Result<ModelMutationPreload> {
    let subtree = load_root_refnos(source, roots).await?;
    if subtree.is_empty() {
        return Ok(ModelMutationPreload::default());
    }
    let model_refnos = load_model_refnos(source, &subtree).await?;
    let mut hierarchy_seeds = model_refnos.clone();
    hierarchy_seeds.extend_from_slice(roots);
    let mut hierarchy =
        crate::data_interface::helper::collect_pe_ancestor_refnos_from(source, &hierarchy_seeds)
            .await?
            .into_iter()
            .collect::<Vec<_>>();
    hierarchy.sort_unstable();
    Ok(ModelMutationPreload {
        subtree_len: subtree.len(),
        model_refnos,
        hierarchy,
    })
}

/// Copy the unchanged PE subtree and existing products needed by transform/delete prerequisites.
/// `INSERT IGNORE` preserves rows already rewritten by the current parse.
pub(crate) async fn apply_model_mutation_preload(
    preload: &ModelMutationPreload,
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() || preload.hierarchy.is_empty() {
        return Ok(0);
    }
    apply_model_mutation_preload_from(&SUL_DB, preload).await
}

async fn apply_model_mutation_preload_from(
    source: &Surreal<Any>,
    preload: &ModelMutationPreload,
) -> anyhow::Result<usize> {
    let started = std::time::Instant::now();
    let keys = preload
        .hierarchy
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    let mut copied = copy_rows(
        source,
        "pe",
        &format!("SELECT * FROM pe WHERE id IN [{keys}]"),
    )
    .await?;
    let pe_elapsed = started.elapsed();
    copied += copy_relations(
        source,
        "pe_owner",
        &format!("SELECT * FROM pe_owner WHERE in IN [{keys}] AND out IN [{keys}]"),
    )
    .await?;
    let owner_elapsed = started.elapsed();
    copied += preload_existing_generation_products_for_refnos(source, &preload.model_refnos)
        .await?;
    println!(
        "暂存 mutation 预载: subtree={} model={} hierarchy={} copied={}，pe={:?} owner={:?} total={:?}",
        preload.subtree_len,
        preload.model_refnos.len(),
        preload.hierarchy.len(),
        copied,
        pe_elapsed,
        owner_elapsed,
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

/// Preload the small, shared room-classification working set before parsing the window.
/// Parsed room/panel edits then overwrite these pre-window rows inside staging.
pub(crate) async fn preload_room_working_set(
    rooms: &crate::fast_model::room_model::RoomPanelMap,
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() {
        return Ok(0);
    }
    preload_room_working_set_from(&SUL_DB, rooms).await
}

async fn preload_room_working_set_from(
    source: &Surreal<Any>,
    rooms: &crate::fast_model::room_model::RoomPanelMap,
) -> anyhow::Result<usize> {
    let panels = rooms.all_panels.iter().copied().collect::<Vec<_>>();
    let room_roots = rooms.rooms.iter().map(|room| room.room).collect::<Vec<_>>();
    let mut topology =
        crate::data_interface::helper::collect_pe_ancestor_refnos_from(source, &panels)
            .await?
            .into_iter()
            .collect::<Vec<_>>();
    topology.extend(room_roots);
    topology.sort_unstable();
    topology.dedup();
    let topology_keys = topology
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    let mut copied = if topology_keys.is_empty() {
        0
    } else {
        let mut copied = copy_rows(
            source,
            "pe",
            &format!("SELECT * FROM pe WHERE id IN [{topology_keys}]"),
        )
        .await?;
        copied += copy_relations(
            source,
            "pe_owner",
            &format!(
                "SELECT * FROM pe_owner WHERE in IN [{topology_keys}] AND out IN [{topology_keys}]"
            ),
        )
        .await?;
        copied
    };
    copied += copy_relations(source, "room_relate", "SELECT * FROM room_relate").await?;
    copied += copy_relations(
        source,
        "room_panel_relate",
        "SELECT * FROM room_panel_relate",
    )
    .await?;
    copied += preload_existing_generation_products_from(source, &panels).await?;
    Ok(copied)
}

/// Copy the room subtrees named by structural triggers (renamed rooms / moved panels) into staging.
///
/// 窗口起点的 [`preload_room_working_set`] 只认「提交前已合规」的房间；本窗口内改名
/// **成为**合规房间的组不在那份映射里，而面板的 `pe_owner` 边不随改名重写、也不会被
/// 本窗口解析写进暂存——不补拷的话，暂存房间轮从暂存 PE 解析出的这间房永远是 0 块
/// 面板，计划里的整间目标随即走「已不在册」的清边成功路径（H-1）。结构触发在计划期
/// 已知，这里按触发种子把子树拓扑与面板产物补进暂存；`INSERT IGNORE` /
/// `record::exists` 语义保证已被本窗口解析重写的行不被回退。
pub(crate) async fn preload_room_structural_targets(
    rooms: &[RefnoEnum],
    panels: &[RefnoEnum],
) -> anyhow::Result<usize> {
    if super::active_staging_writes().is_none() || (rooms.is_empty() && panels.is_empty()) {
        return Ok(0);
    }
    preload_room_structural_targets_from(&SUL_DB, rooms, panels).await
}

async fn preload_room_structural_targets_from(
    source: &Surreal<Any>,
    rooms: &[RefnoEnum],
    panels: &[RefnoEnum],
) -> anyhow::Result<usize> {
    let mut topology = load_root_refnos(source, rooms).await?;
    topology.extend(panels.iter().copied());
    topology.sort_unstable();
    topology.dedup();
    if topology.is_empty() {
        return Ok(0);
    }
    let keys = topology
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    let mut copied = copy_rows(
        source,
        "pe",
        &format!("SELECT * FROM pe WHERE id IN [{keys}]"),
    )
    .await?;
    copied += copy_relations(
        source,
        "pe_owner",
        &format!("SELECT * FROM pe_owner WHERE in IN [{keys}] AND out IN [{keys}]"),
    )
    .await?;
    copied += preload_existing_generation_products_for_refnos(source, panels).await?;
    Ok(copied)
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
    let inst_query = format!("SELECT * FROM inst_relate WHERE in IN [{pe_keys}]");
    let info_keys = select_record_keys(
        source,
        &format!("SELECT VALUE out FROM inst_relate WHERE in IN [{pe_keys}]"),
    )
    .await?;
    let world_keys = select_record_keys(
        source,
        &format!(
            "SELECT VALUE world_trans FROM inst_relate WHERE in IN [{pe_keys}] AND world_trans != NONE"
        ),
    )
    .await?;
    let mut aabb_keys = select_record_keys(
        source,
        &format!("SELECT VALUE aabb FROM inst_relate WHERE in IN [{pe_keys}] AND aabb != NONE"),
    )
    .await?;
    let mut copied = copy_relations(source, "inst_relate", &inst_query).await?;
    if info_keys.is_empty() {
        return Ok(copied);
    }

    let info_scope = info_keys.join(",");
    let geo_query = format!("SELECT * FROM geo_relate WHERE in IN [{info_scope}]");
    let inst_geo_keys = select_record_keys(
        source,
        &format!("SELECT VALUE out FROM geo_relate WHERE in IN [{info_scope}]"),
    )
    .await?;
    let mut trans_keys = select_record_keys(
        source,
        &format!("SELECT VALUE trans FROM geo_relate WHERE in IN [{info_scope}] AND trans != NONE"),
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
    copied += copy_rows(
        source,
        "inst_info",
        &format!("SELECT * FROM inst_info WHERE id IN [{info_scope}]"),
    )
    .await?;
    copied += copy_relations(source, "geo_relate", &geo_query).await?;

    let mut vec_keys = Vec::new();
    if !inst_geo_keys.is_empty() {
        let inst_geo_scope = inst_geo_keys.join(",");
        vec_keys = select_record_keys(
            source,
            &format!(
                "RETURN array::flatten(SELECT VALUE pts FROM inst_geo WHERE id IN [{inst_geo_scope}]);"
            ),
        )
        .await?;
        aabb_keys.extend(
            select_record_keys(
                source,
                &format!(
                    "SELECT VALUE aabb FROM inst_geo WHERE id IN [{inst_geo_scope}] AND aabb != NONE"
                ),
            )
            .await?,
        );
        aabb_keys.sort_unstable();
        aabb_keys.dedup();
        copied += copy_rows(
            source,
            "inst_geo",
            &format!("SELECT * FROM inst_geo WHERE id IN [{inst_geo_scope}]"),
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
            copied += copy_rows(
                source,
                table,
                &format!("SELECT * FROM {table} WHERE id IN [{}]", keys.join(",")),
            )
            .await?;
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
    if !statements.is_empty() {
        crate::surreal_retry::execute_generation_preload(
            &statements.join("\n"),
            &format!("preload {table}"),
        )
        .await?;
    }
    Ok(statements.len())
}

fn render_preload_value(value: &surrealdb::sql::Value) -> String {
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

        let plan = window
            .scope(plan_model_mutation_preload_from(
                &source,
                &[RefnoEnum::from("4000000001/1")],
            ))
            .await
            .expect("plan mutation preload");
        let copied = window
            .scope(apply_model_mutation_preload_from(&source, &plan))
            .await
            .expect("preload mutation target");
        assert_eq!(copied, 13);
        let mut response = window
            .staging_db()
            .query(
                "RETURN [pe:⟨4000000001_1⟩.id != NONE, pe:⟨4000000001_2⟩.id != NONE,
                 pe:⟨4000000001_3⟩.id != NONE, count(SELECT * FROM pe_owner) = 2,
                 pe:⟨4000000001_9⟩.id = NONE];",
            )
            .await
            .expect("inspect mutation rows");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 5]);
        assert!(window.journal().await.is_empty());
        window.drop_database().await.expect("cleanup");
    }

    /// H-1 回归：改名「成为」合规房间的组不在窗口起点的预载范围里，结构触发预载要把
    /// 子树拓扑与面板产物补进暂存、且不回退本窗口解析已写入的行——之后暂存映射才能
    /// 解析出「面板属于这间房」。
    #[tokio::test(flavor = "multi_thread")]
    async fn room_structural_targets_backfill_renamed_room_topology() {
        let source = connect("mem://").await.expect("source mem");
        source
            .use_ns("test")
            .use_db("source")
            .await
            .expect("source db");
        source
            .query(
                "CREATE pe:⟨4000000001_1⟩ SET noun='FRMW', name='X-RM-BAD1', deleted=false;
                 CREATE pe:⟨4000000001_2⟩ SET noun='PANE', deleted=false;
                 CREATE pe:⟨4000000001_9⟩ SET noun='PANE', deleted=false;
                 RELATE pe:⟨4000000001_2⟩->pe_owner->pe:⟨4000000001_1⟩;
                 CREATE world_trans:w1 SET d=[1]; CREATE trans:t1 SET d=[2]; CREATE aabb:a1 SET d={x:1};
                 CREATE vec3:v1 SET d=[1,2,3]; CREATE inst_info:i1 SET noun='PANE';
                 CREATE inst_geo:g1 SET aabb=aabb:a1, pts=[vec3:v1];
                 RELATE pe:⟨4000000001_2⟩->inst_relate->inst_info:i1 SET world_trans=world_trans:w1, aabb=aabb:a1;
                 RELATE inst_info:i1->geo_relate->inst_geo:g1 SET trans=trans:t1;",
            )
            .await
            .expect("fixture transport")
            .check()
            .expect("fixture");

        let instance = connect("mem://").await.expect("staging mem");
        let window = create_window_on(&instance, 7987, 3, 3, ResourceThresholds::default())
            .await
            .expect("window");
        // 模拟本窗口的解析：改名会话把房间自己的 pe 行写进暂存（新名字已合规）。
        window
            .staging_db()
            .query("CREATE pe:⟨4000000001_1⟩ SET noun='FRMW', name='X-RM-R100', deleted=false;")
            .await
            .expect("parsed rename transport")
            .check()
            .expect("parsed rename");

        let copied = window
            .scope(preload_room_structural_targets_from(
                &source,
                &[RefnoEnum::from("4000000001/1")],
                &[RefnoEnum::from("4000000001/2")],
            ))
            .await
            .expect("preload structural targets");

        assert_eq!(copied, 11);
        assert!(window.journal().await.is_empty());
        let mut response = window
            .staging_db()
            .query(
                "RETURN [pe:⟨4000000001_1⟩.name = 'X-RM-R100', pe:⟨4000000001_2⟩.id != NONE,
                 count(SELECT * FROM pe_owner) = 1, count(SELECT * FROM inst_relate) = 1,
                 inst_geo:g1.id != NONE, vec3:v1.id != NONE, pe:⟨4000000001_9⟩.id = NONE];",
            )
            .await
            .expect("inspect staging");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 7]);

        let map = window
            .scope(crate::fast_model::room_model::load_room_panel_map_from_pe(
                &aios_core::options::DbOption::default(),
            ))
            .await
            .expect("load staged room map");
        assert_eq!(
            map.room_num_of(RefnoEnum::from("4000000001/2")),
            Some("R100"),
            "改名成为合规房间后，暂存映射必须解析出它名下的面板"
        );
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn room_working_set_is_staging_only() {
        let source = connect("mem://").await.expect("source mem");
        source
            .use_ns("test")
            .use_db("source")
            .await
            .expect("source db");
        source
            .query(
                "CREATE pe:⟨4000000001_1⟩ SET noun='FRMW', name='X-RM-R100';
                 CREATE pe:⟨4000000001_2⟩ SET noun='PANE';
                 RELATE pe:⟨4000000001_2⟩->pe_owner->pe:⟨4000000001_1⟩;
                 RELATE pe:⟨4000000001_1⟩->room_panel_relate:room_panel->pe:⟨4000000001_2⟩ SET room_num='R100';
                 RELATE pe:⟨4000000001_2⟩->room_relate:panel_member->pe:⟨4000000001_3⟩ SET room_num='R100';",
            )
            .await
            .expect("fixture transport")
            .check()
            .expect("fixture");

        let instance = connect("mem://").await.expect("staging mem");
        let window = create_window_on(&instance, 7989, 2, 2, ResourceThresholds::default())
            .await
            .expect("window");
        let rooms = crate::fast_model::room_model::RoomPanelMap {
            rooms: vec![crate::fast_model::room_model::RoomPanels {
                room: RefnoEnum::from("4000000001/1"),
                room_num: "R100".into(),
                panels: vec![RefnoEnum::from("4000000001/2")],
            }],
            all_panels: std::collections::HashSet::from([RefnoEnum::from("4000000001/2")]),
        };
        let copied = window
            .scope(preload_room_working_set_from(&source, &rooms))
            .await
            .expect("preload rooms");

        assert_eq!(copied, 5);
        assert!(window.journal().await.is_empty());
        let mut response = window
            .staging_db()
            .query(
                "RETURN [count(SELECT * FROM room_relate), count(SELECT * FROM room_panel_relate), count(SELECT * FROM pe),
                 count(SELECT * FROM pe_owner)];",
            )
            .await
            .expect("inspect");
        assert_eq!(
            response.take::<Vec<usize>>(0).expect("room rows"),
            vec![1, 1, 2, 1]
        );
        let map = window
            .scope(crate::fast_model::room_model::load_room_panel_map_from_pe(
                &aios_core::options::DbOption::default(),
            ))
            .await
            .expect("load staged room map");
        assert_eq!(
            map.room_num_of(RefnoEnum::from("4000000001/2")),
            Some("R100")
        );
        window.drop_database().await.expect("cleanup");
    }
}
