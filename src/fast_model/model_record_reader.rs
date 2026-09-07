//! Read the published durable projection without regenerating process-local caches.
use aios_core::{GeomInstQuery, RefnoEnum};
use surrealdb::{Surreal, engine::any::Any};

pub async fn records_of_root(
    db: &Surreal<Any>,
    root: RefnoEnum,
) -> anyhow::Result<Vec<GeomInstQuery>> {
    let packed = root.refno().0;
    let owner = root.to_pe_key();
    let mut response = db
        .query(format!(
            r#"
        SELECT id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner,
          generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
          out.ptset.d.pt AS pts, booled_id != NONE AS has_neg, dt AS date,
          IF booled_id != NONE {{ [{{geo_hash: booled_id}}] }}
          ELSE {{ insts_flat ?? (SELECT transform ?? trans.d AS transform,
              record::id(out) AS geo_hash FROM out->geo_relate
              WHERE visible AND out.meshed AND geo_type = 'Pos'
                AND (transform != NONE OR trans.d != NONE)) }} AS insts
        FROM inst_relate
        WHERE (anc CONTAINS {packed} OR in = {owner})
          AND aabb.d != NONE AND world_trans.d != NONE ORDER BY record_order;
        SELECT id AS record_order, in AS refno, in.old_pe AS old_refno, {owner} AS owner,
          'TUBI' AS generic, aabb.d AS world_aabb, world_trans.d AS world_trans,
          false AS has_neg,
          [{{geo_hash: record::id(out), is_tubi: true,
             is_invalid_tubi: invalid ?? false}}] AS insts
        FROM tubi_relate
        WHERE (anc CONTAINS {packed} OR in = {owner})
          AND aabb.d != NONE AND world_trans.d != NONE ORDER BY record_order;
    "#
        ))
        .await?
        .check()?;
    let mut records: Vec<GeomInstQuery> = response.take(0)?;
    records.extend(response.take::<Vec<GeomInstQuery>>(1)?);
    Ok(records)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn cold_read_includes_baked_canonical_and_tube_but_not_other_roots() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("test").use_db("cold_records").await.unwrap();
        let root: aios_core::RefnoEnum = "24384_26606".into();
        let packed = root.refno().0;
        db.query(format!(r#"
          CREATE aabb:box SET d={{mins:[0,0,0],maxs:[1,1,1]}};
          CREATE trans:world SET d={{translation:[120,0,0],rotation:[0,0,0,1],scale:[1,1,1]}};
          CREATE inst_relate:baked SET in=pe:⟨24384_26609⟩, anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='FLOOR',booled_id='hole';
          CREATE inst_relate:canonical SET in=pe:⟨24384_26610⟩,anc=[{packed}],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',
            insts_flat=[{{geo_hash:'box',transform:{{translation:[5,0,0],rotation:[0,0,0,1],scale:[2,3,4]}}}}];
          CREATE inst_relate:other SET in=pe:⟨24384_999⟩,anc=[1],
            aabb=aabb:box,world_trans=trans:world,generic='BOX',booled_id='other';
          CREATE tubi_relate:tube SET in=pe:⟨24384_26611⟩,anc=[{packed}],
            out=inst_geo:tube,aabb=aabb:box,world_trans=trans:world,invalid=true;
        "#)).await.unwrap().check().unwrap();
        let records = super::records_of_root(&db, root).await.unwrap();
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|r| r.owner == root));
        let canonical = records.iter().find(|r| r.generic == "BOX").unwrap();
        assert_eq!(canonical.insts[0].transform.translation.x, 5.0);
        assert_eq!(canonical.world_trans.translation.x, 120.0);
        let tube = records.iter().find(|r| r.generic == "TUBI").unwrap();
        assert!(tube.insts[0].is_tubi && tube.insts[0].is_invalid_tubi);
        assert_eq!(
            records.iter().find(|r| r.generic == "FLOOR").unwrap().insts[0].geo_hash,
            "hole"
        );
    }
}
