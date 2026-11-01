use crate::fast_model::utils::save_transforms_to_surreal;
use aios_core::{
    RefU64, RefnoEnum, SUL_DB, gen_bytes_hash, query_neareast_along_axis, query_neareast_by_pos_dir,
};
use bevy_transform::components::Transform;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::bounding_volume::BoundingVolume;
use std::collections::HashMap;

pub async fn update_cal_equip() -> anyhow::Result<()> {
    update_cal_equip_wtrans().await?;
    cal_equip_nearest_floor().await?;
    Ok(())
}

//将equip的 world transform 这些放到cal_equip 这张表里，存储一些需要计算的缓存数据
//这样可以减少查询次数，提高性能
//这个表的数据是在equip的数据变更时，由equip的数据变更触发的
pub async fn update_cal_equip_wtrans() -> anyhow::Result<()> {
    let mut response = SUL_DB
        .query(format!(r#"select value record::id(id) from {} where type::thing("cal_equi", record::id(id))!=none"#, "EQUI"))
        .await?;
    let equips: Vec<RefnoEnum> = response.take(0)?;
    if equips.is_empty() {
        return Ok(());
    }
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut sql = String::new();
    for refno in equips {
        let world_trans = aios_core::get_world_transform(refno)
            .await?
            .unwrap_or_default();
        let transform_hash = gen_bytes_hash::<_, 64>(&world_trans);
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
pub async fn cal_equip_nearest_floor() -> anyhow::Result<()> {
    let mut response = SUL_DB
        .query(format!(r#"select value record::id(id) from {} where type::thing("cal_equi", record::id(id))!=none"#, "EQUI"))
        .await?;
    let equips: Vec<RefnoEnum> = response.take(0)?;
    if equips.is_empty() {
        return Ok(());
    }

    let mut equip_sql = String::new();
    for equip in equips {
        // 2026-11-01 修：`->inst_relate` 是**错的边写法**。`inst_relate` 是普通表
        // （`inst_relate:⟨ref0_ref1⟩`），不是从 `pe` 出去的边，所以
        // `<-pe_owner<-pe->inst_relate` 恒为空——真库实测该 EQUI 的 13 个子件
        // aabb 一条都取不到（原写法返回 0 条，修后 13 条）。同一个坑
        // `fn::get_world_pos` 在 2026-10-29 已经修过，这里是漏掉的一处。
        //
        // 读法改成 `array::map(... |$c| type::thing('inst_relate', record::id($c)).aabb.d)`：
        // 一层是子件、两层是孙件，与原来那两段子查询的范围一致。
        //
        // 2026-11-01 修：最外层必须 `array::filter(..., |$v| $v != NONE)`。
        // `parry3d::Aabb` 的 `Deserialize` 是严格的对象结构体，数组里混进一个
        // `NONE` 就会让**整个** `Vec<Aabb>` 反序列化失败（实测：
        // `[{mins,..},{mins,..}]` 成功 2 个；`[{mins,..}, NONE, {mins,..}]` 直接
        // `expected an object-like struct named Aabb, found None`）。
        //
        // 而设备子件里缺 aabb 是常态（没有几何的 SUBE/NOZZ 子件、只存在于历史
        // 数据里的构件）。原来 `let Ok(aabbs) = ... else { continue }` 会把这台
        // 设备整个静默跳过——真库实测 1165 台里 525 台是这样被丢掉的，它们
        // 子件 aabb 大多齐全（如 pe:24381_100725 的 38 个子件全有），只是其中
        // 掺了一两个 NONE。过滤掉 NONE 后这些设备就能正常参与计算。
        let sql = format!(
            r#"
            (select value array::filter(array::flatten([
                array::map((select value in from {e}<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                array::map((select value in from {e}<-pe_owner<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d)
            ]), |$v| $v != NONE) from {e} where array::len(->nearest_relate)=0)[0]
            "#,
            e = equip.to_pe_key()
        );
        let mut response = SUL_DB.query(sql).await?;
        // 不要再静默 `continue`：反序列化失败以前意味着「这台设备被悄悄丢掉」，
        // 排查时看不到任何线索。现在过滤过 NONE，真失败就是数据异常，得留痕。
        let aabbs = match response.take::<Vec<Aabb>>(0) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(equip = ?equip, error = %err, "读取设备子件 aabb 失败，跳过");
                continue;
            }
        };
        if aabbs.is_empty() {
            tracing::debug!(equip = ?equip, "设备无子件 aabb，跳过");
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
                    "relate {}->nearest_relate->FLOOR:{} set dist={};",
                    equip.to_pe_key(),
                    nearest.to_string(),
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

#[cfg(test)]
mod nearest_floor_live_tests {
    //! `cal_equip_nearest_floor` 的两条 live 钉，都需要真库 8009。
    //!
    //! 对应 2026-11-01 修掉的两个缺陷：
    //!
    //! 1. **射线候选集**（vendor `accel_tree`）：`should_unpack_leaf` 曾只保留
    //!    AABB 最近的那一个候选。AABB 的 `time_of_impact` 是真实三角网格 toi
    //!    的**下界**，所以「AABB 最近」经常是网格在该 (x,y) 镂空的构件（楼板
    //!    有孔洞、开口、异形轮廓）。只留它，精筛失败就返回 `None`，真实命中的
    //!    候选全被丢掉。
    //!
    //! 2. **子件缺 aabb 整台跳过**（本模块）：`parry3d::Aabb` 的 `Deserialize`
    //!    是严格对象结构体，数组里混一个 `NONE` 就让整个 `Vec<Aabb>` 失败，
    //!    原来的 `let Ok(...) else { continue }` 把整台设备静默吞掉。
    //!
    //! 运行（**必须逐条单跑**：`SUL_DB` 是进程级全局连接，同一进程里第二次
    //! `attach_store_connected` 会让后一条测试的射线查询全部返回 None）：
    //!
    //! ```text
    //! cargo test --lib child_aabb_survives_none_entries -- --ignored --test-threads=1
    //! cargo test --lib coverage_does_not_regress          -- --ignored --test-threads=1
    //! ```

    use super::*;
    // 走 `attach_store_connected`（lib.rs 里点名的旁路入口）而不是
    // `test::test_query::init_test_surreal`——后者所在模块已被注释掉，
    // 且它的连接建在当前 runtime 上，进程跑完就没了。
    use crate::attach_store_connected;

    /// 设备子件里混着 `NONE` 时，过滤后必须还能反序列化出非空 `Vec<Aabb>`。
    ///
    /// 这条钉的是 SQL 最外层的 `array::filter(..., |$v| $v != NONE)`。去掉它，
    /// `take::<Vec<Aabb>>` 就整体失败——真库实测 1165 台 EQUI 里 525 台因此
    /// 被静默跳过。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009（AvevaMarineSample）；必须单跑，见模块注释"]
    async fn child_aabb_survives_none_entries() -> anyhow::Result<()> {
        attach_store_connected().await?;

        // 找一台「有子件、且子件里确实混了 NONE」的设备。
        let mut r = SUL_DB
            .query(
                r#"
                SELECT VALUE id FROM pe
                WHERE noun = 'EQUI'
                  AND array::len(<-pe_owner) > 0
                  AND array::len(array::flatten([
                        array::map((SELECT VALUE in FROM $this<-pe_owner),
                                   |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                        array::map((SELECT VALUE in FROM $this<-pe_owner<-pe_owner),
                                   |$c| type::thing('inst_relate', record::id($c)).aabb.d)
                      ])) > array::len(array::filter(array::flatten([
                        array::map((SELECT VALUE in FROM $this<-pe_owner),
                                   |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                        array::map((SELECT VALUE in FROM $this<-pe_owner<-pe_owner),
                                   |$c| type::thing('inst_relate', record::id($c)).aabb.d)
                      ]), |$v| $v != NONE))
                LIMIT 1;
                "#,
            )
            .await?;
        let ids: Vec<surrealdb::sql::Thing> = r.take(0)?;
        if ids.is_empty() {
            eprintln!("库里没有「子件混 NONE」的设备，跳过");
            return Ok(());
        }
        let key = format!("{}", ids[0]);

        // 不过滤：必然失败。这是本条钉的立论基础——如果哪天不失败了，说明
        // Aabb 的 Deserialize 放宽了，本模块里的 array::filter 就可以撤。
        let raw = format!(
            r#"RETURN array::flatten([
                array::map((SELECT VALUE in FROM {key}<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                array::map((SELECT VALUE in FROM {key}<-pe_owner<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d)
            ]);"#
        );
        let mut resp = SUL_DB.query(raw).await?;
        assert!(
            resp.take::<Vec<Aabb>>(0).is_err(),
            "含 NONE 的数组本该反序列化失败；若这里通过了，说明 Aabb 的 \
             Deserialize 已放宽，equip_model 里的 array::filter 可以撤"
        );

        // 过滤后：必须成功且非空。
        let filtered = format!(
            r#"RETURN array::filter(array::flatten([
                array::map((SELECT VALUE in FROM {key}<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                array::map((SELECT VALUE in FROM {key}<-pe_owner<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d)
            ]), |$v| $v != NONE);"#
        );
        let mut resp = SUL_DB.query(filtered).await?;
        let aabbs: Vec<Aabb> = resp.take(0)?;
        assert!(
            !aabbs.is_empty(),
            "{key} 过滤掉 NONE 之后仍有子件几何，不该为空"
        );
        eprintln!("{key}: 过滤后 {} 个 aabb", aabbs.len());

        Ok(())
    }

    /// 跑一遍 `cal_equip_nearest_floor`，覆盖率不能退回修前水平。
    ///
    /// 修前基线：1165 台 EQUI 只有 367 台写出 `nearest_relate`（31.5%）。
    /// 两个缺陷都修掉后是 675 台（57.9%）。剩下的都是数据缺口
    /// （子件全无 aabb 的 SUBE、无子件、落在楼板覆盖区外的），不是查询缺陷。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009，会清空并重写 nearest_relate；必须单跑，见模块注释"]
    async fn coverage_does_not_regress() -> anyhow::Result<()> {
        attach_store_connected().await?;

        let mut r = SUL_DB
            .query("RETURN count(SELECT * FROM pe WHERE noun = 'EQUI');")
            .await?;
        let total: Option<i64> = r.take(0)?;
        let total = total.unwrap_or(0);
        if total == 0 {
            eprintln!("库里没有 EQUI，跳过");
            return Ok(());
        }

        // 清空后重算，避免旧的边让 `where array::len(->nearest_relate)=0` 提前跳过。
        SUL_DB.query("DELETE nearest_relate;").await?;
        // `cal_equip_nearest_floor` 靠 `GLOBAL_AABB_TREE` 做射线查询，不先加载
        // 树的话 `query_neareast_by_pos_dir` 一律返回 None，覆盖率会是 0。
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;
        cal_equip_nearest_floor().await?;

        let mut r = SUL_DB
            .query(
                "RETURN count(SELECT * FROM pe WHERE noun = 'EQUI' AND array::len(->nearest_relate) > 0);",
            )
            .await?;
        let covered: Option<i64> = r.take(0)?;
        let covered = covered.unwrap_or(0);
        let pct = covered as f64 * 100.0 / total as f64;
        eprintln!("EQUI 覆盖 {covered}/{total} = {pct:.1}%");

        // 修前 367（31.5%）。门槛设在 50%：既挡住回退，又不因数据小幅变化误红。
        assert!(
            pct >= 50.0,
            "覆盖率 {pct:.1}% 低于 50%，射线候选集或 NONE 过滤可能回退了（修前 31.5%）"
        );

        Ok(())
    }
}
