use crate::fast_model::utils::save_transforms_to_surreal;
use aios_core::{RefU64, RefnoEnum, SUL_DB, gen_bytes_hash};
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
use aios_core::{query_neareast_along_axis, query_neareast_by_pos_dir};

use bevy_transform::components::Transform;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::bounding_volume::BoundingVolume;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

pub async fn update_cal_equip() -> anyhow::Result<()> {
    update_cal_equip_wtrans().await?;
    #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
    cal_equip_nearest_floor().await?;
    Ok(())
}

//将equip的 world transform 这些放到cal_equip 这张表里，存储一些需要计算的缓存数据
//这样可以减少查询次数，提高性能
//这个表的数据是在equip的数据变更时，由equip的数据变更触发的
pub async fn update_cal_equip_wtrans() -> anyhow::Result<()> {
    let mut response = SUL_DB
        .query(format!(
            r#"select value id from {} where type::record("cal_equi", record::id(id))!=none"#,
            "EQUI"
        ))
        .await?;
    let equips: Vec<RefnoEnum> = response.take(0)?;
    if equips.is_empty() {
        return Ok(());
    }
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut sql = String::new();
    for refno in equips {
        let world_trans = match aios_core::get_world_transform(refno).await {
            Ok(transform) => transform.unwrap_or_default(),
            Err(e) => {
                eprintln!(
                    "[cal_equip] 获取 world_transform 失败, refno={} 错误: {}",
                    refno.to_normal_str(),
                    e
                );
                return Err(e);
            }
        };
        let transform_hash = gen_bytes_hash(&world_trans);
        if !transform_map.contains_key(&transform_hash) {
            transform_map.insert(transform_hash, serde_json::to_string(&world_trans).unwrap());
        }
        sql.push_str(&format!(
            "create cal_equi:{refno} SET world_trans=trans:⟨{}⟩;",
            transform_hash
        ));
    }
    save_transforms_to_surreal(&transform_map).await?;
    SUL_DB.query(sql).await.unwrap();
    Ok(())
}

//取得设备下的所有aabb，然后取下面的点到楼板的最近距离
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
pub async fn cal_equip_nearest_floor() -> anyhow::Result<()> {
    let mut response = SUL_DB
        .query(format!(r#"select value record::id(id) from {} where type::record("cal_equi", record::id(id))!=none"#, "EQUI"))
        .await?;
    let equips: Vec<RefnoEnum> = response.take(0)?;
    if equips.is_empty() {
        return Ok(());
    }

    let mut equip_sql = String::new();
    for equip in equips {
        println!("[cal_equip] 处理设备 {}", equip.to_normal_str());

        let sql = format!(
            r#"
            (select value array::flatten([(select value aabb.d from <-pe_owner<-pe<-pe_owner<-pe->inst_relate),
                (select value aabb.d from <-pe_owner<-pe->inst_relate)]) from {} where array::len(->nearest_relate)=0)[0]
            "#,
            equip.to_pe_key()
        );
        // dbg!(&sql);
        let mut response = SUL_DB.query(sql).await?;
        let Ok(raw_values) = response.take::<Vec<JsonValue>>(0) else {
            continue;
        };
        let Ok(aabbs) = raw_values
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<Aabb>, _>>()
        else {
            continue;
        };
        if aabbs.is_empty() {
            continue;
        }
        let mut final_aabb = Aabb::new_invalid();
        for aabb in aabbs {
            final_aabb.merge(&aabb);
        }
        //得到底部的中心点，去计算最近的楼板
        let btm_pts = &final_aabb.vertices()[..4];
        for btm_pt in btm_pts {
            let pt: Vec3 = (*btm_pt).into();
            if let Ok(Some((nearest, dist))) =
                query_neareast_by_pos_dir(pt, Vec3::NEG_Z, "FLOOR").await
            {
                // dbg!((btm_pt, nearest, dist));
                equip_sql.push_str(&format!(
                    "relate {}->nearest_relate->{} set dist={};",
                    equip.to_pe_key(),
                    nearest.to_pe_key(),
                    dist
                ));
                break;
            }
        }
    }
    if !equip_sql.is_empty() {
        SUL_DB.query(equip_sql).await.unwrap();
    }
    Ok(())
}
