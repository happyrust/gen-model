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
    let panel_keys = panels
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    let mut copied = if panel_keys.is_empty() {
        0
    } else {
        copy_rows(
            source,
            "pe",
            &format!("SELECT * FROM pe WHERE id IN [{panel_keys}]"),
        )
        .await?
    };
    copied += copy_rows(source, "room_relate", "SELECT * FROM room_relate").await?;
    copied += preload_existing_generation_products_from(source, &panels).await?;
    Ok(copied)
}

async fn preload_existing_generation_products_from(
    source: &Surreal<Any>,
    roots: &[RefnoEnum],
) -> anyhow::Result<usize> {
    let refnos = load_root_refnos(source, roots).await?;
    if refnos.is_empty() {
        return Ok(0);
    }

    let pe_keys = refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",");
    let inst_scope = format!("SELECT VALUE id FROM inst_relate WHERE in IN [{pe_keys}]");
    let info_scope = format!("SELECT VALUE out FROM ({inst_scope})");
    let geo_scope = format!("SELECT VALUE id FROM geo_relate WHERE in IN ({info_scope})");
    let inst_geo_scope = format!("SELECT VALUE out FROM ({geo_scope})");

    let queries = [
        (
            "inst_relate",
            format!("SELECT * FROM inst_relate WHERE in IN [{pe_keys}]"),
        ),
        (
            "inst_info",
            format!("SELECT * FROM inst_info WHERE id IN ({info_scope})"),
        ),
        (
            "geo_relate",
            format!("SELECT * FROM geo_relate WHERE in IN ({info_scope})"),
        ),
        (
            "inst_geo",
            format!("SELECT * FROM inst_geo WHERE id IN ({inst_geo_scope})"),
        ),
        (
            "world_trans",
            format!(
                "SELECT * FROM world_trans WHERE id IN (SELECT VALUE world_trans FROM ({inst_scope}))"
            ),
        ),
        (
            "trans",
            format!("SELECT * FROM trans WHERE id IN (SELECT VALUE trans FROM ({geo_scope}))"),
        ),
        (
            "vec3",
            format!(
                "SELECT * FROM vec3 WHERE id IN array::flatten(SELECT VALUE pts FROM ({inst_geo_scope}))"
            ),
        ),
        (
            "aabb",
            format!(
                "SELECT * FROM aabb WHERE id IN array::distinct(array::flatten([SELECT VALUE aabb FROM ({inst_scope}), SELECT VALUE aabb FROM ({inst_geo_scope})]))"
            ),
        ),
    ];

    let mut copied = 0;
    for (table, query) in queries {
        copied += copy_rows(source, table, &query).await?;
    }
    Ok(copied)
}

async fn load_root_refnos(
    source: &Surreal<Any>,
    roots: &[RefnoEnum],
) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut refnos = Vec::new();
    for root in roots {
        let key = root.to_pe_key();
        let sql = format!(
            "RETURN array::flatten(object::values((SELECT [id] AS p0, \
             <-pe_owner<-(? AS p1)<-pe_owner<-(? AS p2)<-pe_owner<-(? AS p3)\
             <-pe_owner<-(? AS p4)<-pe_owner<-(? AS p5)<-pe_owner<-(? AS p6)\
             <-pe_owner<-(? AS p7)<-pe_owner<-(? AS p8)<-pe_owner<-(? AS p9)\
             <-pe_owner<-(? AS p10)<-pe_owner<-(? AS p11) FROM ONLY {key} \
             WHERE record::exists(id)) ?: {{}}))[? !deleted];"
        );
        let mut response = source.query(sql).await?.check()?;
        refnos.extend(response.take::<Vec<RefnoEnum>>(0)?);
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
        crate::surreal_retry::execute_generation_preload(
            &format!("INSERT IGNORE INTO {table} {value};"),
            &format!("preload {table}"),
        )
        .await?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::staging::ResourceThresholds;
    use crate::data_interface::staging::lifecycle::create_window_on;
    use surrealdb::engine::any::connect;

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
             CREATE pe:⟨4000000001_9⟩ SET deleted=false;
             RELATE pe:⟨4000000001_2⟩->pe_owner->pe:⟨4000000001_1⟩;
             CREATE world_trans:w1 SET d=[1]; CREATE trans:t1 SET d=[2]; CREATE aabb:a1 SET d={x:1};
             CREATE vec3:v1 SET d=[1,2,3]; CREATE inst_info:i1 SET noun='PIPE';
             CREATE inst_geo:g1 SET aabb=aabb:a1, pts=[vec3:v1];
             RELATE pe:⟨4000000001_2⟩->inst_relate->inst_info:i1 SET world_trans=world_trans:w1, aabb=aabb:a1;
             RELATE inst_info:i1->geo_relate->inst_geo:g1 SET trans=trans:t1;
             CREATE inst_info:i9; RELATE pe:⟨4000000001_9⟩->inst_relate->inst_info:i9;"
        ).await.expect("fixture transport").check().expect("fixture");

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
             world_trans:w1.id != NONE, trans:t1.id != NONE, aabb:a1.id != NONE,
             vec3:v1.id != NONE, inst_info:i9.id = NONE];",
            )
            .await
            .expect("inspect staging");
        assert_eq!(response.take::<Vec<bool>>(0).expect("flags"), vec![true; 9]);
        window.drop_database().await.expect("cleanup");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn room_working_set_is_staging_only() {
        let source = connect("mem://").await.expect("source mem");
        source.use_ns("test").use_db("source").await.expect("source db");
        source
            .query(
                "CREATE pe:⟨4000000001_1⟩ SET noun='FRMW';
                 CREATE pe:⟨4000000001_2⟩ SET noun='PANE';
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
            all_panels: std::collections::HashSet::from([RefnoEnum::from("4000000001/2")]),
            ..Default::default()
        };
        let copied = window
            .scope(preload_room_working_set_from(&source, &rooms))
            .await
            .expect("preload rooms");

        assert_eq!(copied, 2);
        assert!(window.journal().await.is_empty());
        let mut response = window
            .staging_db()
            .query("SELECT VALUE record::id(id) FROM room_relate;")
            .await
            .expect("inspect");
        assert_eq!(
            response
                .take::<Vec<String>>(0)
                .expect("room rows")
                .len(),
            1
        );
        window.drop_database().await.expect("cleanup");
    }
}
