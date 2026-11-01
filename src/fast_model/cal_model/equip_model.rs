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

/// [`update_cal_equip_for_roots`] 的结果，给调用方记日志用。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EquipRefreshOutcome {
    /// 重写了 `cal_equi` 的设备台数。
    pub refreshed: usize,
    /// 重算后挂着 `nearest_relate` 的台数（树已就绪却打不中楼板的，旧边删掉、不计）。
    pub floors_linked: usize,
    /// 重算失败的台数（已逐台 `warn!`，旧值原样保留）。
    pub failed: usize,
    /// 空间树没就绪：`nearest_relate` 一条没动。
    pub floors_skipped: bool,
}

/// 根往下找设备的最大层数。真库 EQUI 最深在 SITE → ZONE → EQUI 这一级，再深是零件；
/// 上限防异常数据把展开拖成全树扫。
const EQUIP_EXPAND_DEPTH: usize = 4;

/// 增量版：只重算**指定生成根**影响到的设备派生量。
///
/// ## 为什么需要它
///
/// `update_cal_equip` 是全量版，只挂在启动序列上（`lib.rs` 的 `cal-equip` 段），而且
/// `nearest_relate` 那一半只给**还没有边**的设备补。增量更新路径此前完全不碰这两张表——
/// 模型改了以后 `cal_equi` 的 world transform 和 `nearest_relate` 的最近楼板都还是旧的。
///
/// ## 根 → 设备
///
/// 设备子件的生成根归到所在的 EQUI（`generation_root.rs` U-E2：EQUI 自成交付单元），所以根
/// 要么就是 EQUI，要么是 ZONE / SITE 这类容器。从根沿 `<-pe_owner`（`in` = 子、`out` = 父）
/// 逐层往下，`noun = 'EQUI'` 的收下且不再往里走（它的子件是零件），其余未删节点作下一层。
///
/// ## 写法（真库 python 探针逐条验过，SurrealDB 2.1.4）
///
/// - 一层一条查询：`SELECT id, noun, deleted FROM array::flatten((SELECT VALUE <-pe_owner.in FROM [..]))`。
/// - `cal_equi` 用 `UPSERT`：`CREATE` 撞上已有 id 报 `Database record already exists`、旧值原样
///   留着，而 `query()` 不检查语句级错误——不 `.check()` 就是静默不更新。
/// - 旧边用图写法 `DELETE pe:x->nearest_relate` 删，和新边放在同一个事务里换。
///
/// ## 空间树没就绪时不碰楼板边
///
/// 射线查询在树空 / 未装载时一律返回 `None`，与「脚下真的没有楼板」分不开。这时删旧边就是把
/// 正确数据抹掉（2026-11-01 实测：没装树跑了一遍，4 台设备的边被删光）。所以树不是
/// `Ready` / `ReadyEmpty` 时只重写 `cal_equi`，`nearest_relate` 原样保留并在结果里标出。
///
/// ## 已删设备
///
/// 软删的 EQUI 会断开 `pe_owner`（实测父边为空），从父根往下找不到它们；所以每次顺手把
/// `deleted = true` 的设备的两类派生行清掉（两张表各千行量级，实测各扫一遍 40～60ms）。
pub async fn update_cal_equip_for_roots(
    roots: &[RefnoEnum],
) -> anyhow::Result<EquipRefreshOutcome> {
    let mut outcome = EquipRefreshOutcome::default();
    if roots.is_empty() {
        return Ok(outcome);
    }

    SUL_DB
        .query(
            "DELETE nearest_relate WHERE in.deleted = true; \
             DELETE cal_equi WHERE type::thing('pe', record::id(id)).deleted = true;",
        )
        .await?
        .check()?;

    let equips = collect_live_equips(roots).await?;
    let link_floors = crate::fast_model::spatial_state::current_state().is_ready();
    outcome.floors_skipped = !link_floors && !equips.is_empty();
    for equip in equips {
        match refresh_one_equip(equip, link_floors).await {
            Ok(linked) => {
                outcome.refreshed += 1;
                outcome.floors_linked += usize::from(linked);
            }
            Err(error) => {
                tracing::warn!(equip = ?equip, error = %error, "设备派生量重算失败，旧值保留");
                outcome.failed += 1;
            }
        }
    }
    Ok(outcome)
}

#[derive(Debug, serde::Deserialize)]
struct PeNode {
    id: surrealdb::sql::Thing,
    noun: Option<String>,
    deleted: Option<bool>,
}

/// 「根 → 设备」逐层分拣的状态，抽成纯逻辑好离线钉住。
#[derive(Default)]
struct EquipScan {
    live: Vec<RefnoEnum>,
    // 根不能预先放进 `seen`：先放的话「根本身就是 EQUI」再 insert 返回 false，
    // 它自己就收不进 `live`（2026-11-01 live 测试实测：以设备为根展开出 0 台）。
    seen: std::collections::HashSet<RefnoEnum>,
}

impl EquipScan {
    /// 分拣一层节点，返回下一层要展开的前沿。
    fn absorb(&mut self, nodes: Vec<(RefnoEnum, Option<String>, bool)>) -> Vec<RefnoEnum> {
        let mut frontier = Vec::new();
        for (refno, noun, deleted) in nodes {
            if deleted || !self.seen.insert(refno) {
                continue;
            }
            if noun.as_deref() == Some("EQUI") {
                self.live.push(refno);
            } else {
                frontier.push(refno);
            }
        }
        frontier
    }
}

fn pe_keys(refnos: &[RefnoEnum]) -> String {
    refnos
        .iter()
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>()
        .join(",")
}

async fn fetch_nodes(sql: String) -> anyhow::Result<Vec<(RefnoEnum, Option<String>, bool)>> {
    let mut response = SUL_DB.query(sql).await?.check()?;
    let rows: Vec<PeNode> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let refno = crate::data_interface::helper::pe_thing_to_refno(row.id).ok()?;
            Some((refno, row.noun, row.deleted.unwrap_or(false)))
        })
        .collect())
}

async fn collect_live_equips(roots: &[RefnoEnum]) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut scan = EquipScan::default();
    let mut nodes = fetch_nodes(format!(
        "SELECT id, noun, deleted FROM [{}];",
        pe_keys(roots)
    ))
    .await?;
    for depth in 0..=EQUIP_EXPAND_DEPTH {
        let frontier = scan.absorb(std::mem::take(&mut nodes));
        if frontier.is_empty() || depth == EQUIP_EXPAND_DEPTH {
            break;
        }
        nodes = fetch_nodes(format!(
            "SELECT id, noun, deleted FROM array::flatten((SELECT VALUE <-pe_owner.in FROM [{}]));",
            pe_keys(&frontier)
        ))
        .await?;
    }
    Ok(scan.live)
}

/// 重算单台设备：`cal_equi` 照写；`link_floors` 时再换 `nearest_relate`。
/// 返回重算后这台设备是否挂着楼板边。
async fn refresh_one_equip(equip: RefnoEnum, link_floors: bool) -> anyhow::Result<bool> {
    let world_trans = aios_core::get_world_transform(equip)
        .await?
        .unwrap_or_default();
    let transform_hash = gen_bytes_hash::<_, 64>(&world_trans);
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    transform_map.insert(transform_hash, serde_json::to_string(&world_trans)?);
    save_transforms_to_surreal(&transform_map).await?;
    SUL_DB
        .query(format!(
            "UPSERT {} SET world_trans = trans:⟨{transform_hash}⟩;",
            equip.to_table_key("cal_equi")
        ))
        .await?
        .check()?;

    if !link_floors {
        return Ok(false);
    }

    let e = equip.to_pe_key();
    let mut response = SUL_DB
        .query(format!(
            r#"
            SELECT VALUE array::filter(array::flatten([
                array::map((SELECT VALUE in FROM {e}<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d),
                array::map((SELECT VALUE in FROM {e}<-pe_owner<-pe_owner), |$c| type::thing('inst_relate', record::id($c)).aabb.d)
            ]), |$v| $v != NONE) FROM ONLY {e};
            "#
        ))
        .await?
        .check()?;
    // 读不出来是数据异常，不是「脚下没有楼板」：上抛，旧边原样留着。
    let aabbs: Vec<Aabb> = response.take(0)?;
    let nearest = nearest_floor_below(&aabbs).await;

    let sql = match nearest {
        Some((floor, dist)) => format!(
            "BEGIN TRANSACTION; DELETE {e}->nearest_relate; \
             RELATE {e}->nearest_relate->{} SET dist = {dist}; COMMIT TRANSACTION;",
            floor.to_table_key("FLOOR")
        ),
        // 树已就绪仍打不中：脚下确实没有楼板（或设备已无几何），旧边已过期。
        None => format!("DELETE {e}->nearest_relate;"),
    };
    SUL_DB.query(sql).await?.check()?;
    Ok(nearest.is_some())
}

/// 合并子件 aabb，从底面四角向 -Z 打射线，取第一个打中的楼板。
async fn nearest_floor_below(aabbs: &[Aabb]) -> Option<(RefnoEnum, f32)> {
    if aabbs.is_empty() {
        return None;
    }
    let mut merged = Aabb::new_invalid();
    for aabb in aabbs {
        merged.merge(aabb);
    }
    for corner in &merged.vertices()[..4] {
        let pt: Vec3 = (*corner).into();
        if let Ok(Some(hit)) = query_neareast_by_pos_dir(pt, Vec3::NEG_Z, "FLOOR").await {
            return Some(hit);
        }
    }
    None
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
        // `UPSERT` 不是 `CREATE`：`CREATE` 撞上已有 id 报错、旧值原样留着，第二次启动起
        // world transform 就再也不更新了（真库实测 SurrealDB 2.1.4）。
        sql.push_str(&format!(
            "UPSERT cal_equi:{refno} SET world_trans=trans:⟨{}⟩;",
            transform_hash
        ));
    }
    save_transforms_to_surreal(&transform_map).await?;
    // 2026-11-01 修：这里原来是 `.unwrap()`。1165 条 `create` 挤在一个请求里，
    // 一旦超时或服务端拒绝就 panic；而这个函数现在挂在启动序列上
    // （`lib.rs` 的 `cal-equip` 段），panic 会直接带走整个进程——
    // 派生的缓存数据没写成功不该是启动杀手。
    SUL_DB
        .query(sql)
        .await
        .and_then(|response| response.check())
        .map_err(|error| anyhow::anyhow!("写入 cal_equi 的 world_trans 失败: {error}"))?;
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
        // 同上：`unwrap()` 在启动序列里等于「一条边写不进去就杀进程」。
        SUL_DB
            .query(equip_sql)
            .await
            .and_then(|response| response.check())
            .map_err(|error| anyhow::anyhow!("写入 nearest_relate 失败: {error}"))?;
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
    /// 两个缺陷都修掉后是 675 台（57.9%）；射线起点后退修掉「底面贴楼板面」后
    /// 676/1173（57.6%，2026-11-01）。
    ///
    /// 剩下 497 台的分类（探针逐条射线实测，2026-11-01）：483 台的子件全是没有
    /// `inst_relate` 行的 SUBE，取不到一个 aabb，这是生成侧的数据缺口；其余 14 台
    /// 有 aabb，但四个底角正下方都没有 FLOOR 包围盒（楼板覆盖区外 / 开孔），再加 1 台
    /// 网格可用却打不中。射线本身（网格读取、坐标变换、solid 标志）没有查出问题。
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

#[cfg(test)]
mod equip_scan_tests {
    //! 「根 → 设备」分拣的离线钉，不需要真库。
    use super::*;

    fn r(n: u32) -> RefnoEnum {
        RefnoEnum::from(format!("24381/{n}").as_str())
    }

    fn node(n: u32, noun: &str, deleted: bool) -> (RefnoEnum, Option<String>, bool) {
        (r(n), Some(noun.to_string()), deleted)
    }

    /// 生成根本身就是设备（EQUI 自成交付单元，这是常态）时要把它自己收下，且不再往下展开。
    #[test]
    fn equip_root_collects_itself() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "EQUI", false)]);
        assert_eq!(scan.live, vec![r(1)]);
        assert!(frontier.is_empty(), "设备的子件是零件，不再往下展开");
    }

    #[test]
    fn container_root_expands_to_child_equips() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "ZONE", false)]);
        assert_eq!(frontier, vec![r(1)]);
        assert!(scan.live.is_empty());

        let frontier = scan.absorb(vec![
            node(2, "EQUI", false),
            node(3, "EQUI", false),
            node(4, "STRU", false),
        ]);
        assert_eq!(scan.live, vec![r(2), r(3)]);
        assert_eq!(frontier, vec![r(4)]);
    }

    #[test]
    fn deleted_nodes_are_neither_collected_nor_expanded() {
        let mut scan = EquipScan::default();
        let frontier = scan.absorb(vec![node(1, "EQUI", true), node(2, "ZONE", true)]);
        assert!(scan.live.is_empty());
        assert!(frontier.is_empty());
    }

    #[test]
    fn repeated_nodes_are_taken_once() {
        let mut scan = EquipScan::default();
        scan.absorb(vec![node(1, "ZONE", false), node(2, "EQUI", false)]);
        let frontier = scan.absorb(vec![node(1, "ZONE", false), node(2, "EQUI", false)]);
        assert_eq!(scan.live, vec![r(2)]);
        assert!(frontier.is_empty(), "已经见过的节点不再进前沿（防环）");
    }
}

#[cfg(test)]
mod incremental_live_tests {
    //! `update_cal_equip_for_roots` 的 live 钉，需要真库 8009。
    //!
    //! **必须逐条单跑**：`SUL_DB` 是进程级全局连接，空间树状态也是进程级的；第二条要求
    //! 进程里**没有**装过空间树。
    //!
    //! ```text
    //! cargo test --lib container_root_refreshes_every_equip_and_relinks_floors -- --ignored --test-threads=1
    //! cargo test --lib equip_root_overwrites_stale_world_trans_and_keeps_edges_without_tree -- --ignored --test-threads=1
    //! ```

    use super::*;
    use crate::attach_store_connected;
    use crate::data_interface::helper::pe_thing_to_refno;
    use crate::fast_model::spatial_state;

    /// 一个下挂 >= `min_equips` 台未删 EQUI 的 ZONE，连同这些 EQUI。
    async fn zone_root_with_equips(
        min_equips: usize,
    ) -> anyhow::Result<(RefnoEnum, Vec<RefnoEnum>)> {
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM pe WHERE noun = 'ZONE' \
                 AND array::len(<-pe_owner[WHERE in.noun = 'EQUI']) >= {min_equips} LIMIT 1;"
            ))
            .await?
            .check()?;
        let found: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let root = pe_thing_to_refno(
            found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("库里没有下挂 >= {min_equips} 台 EQUI 的 ZONE"))?,
        )?;

        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE in FROM {}<-pe_owner WHERE in.noun = 'EQUI' AND in.deleted != true;",
                root.to_pe_key()
            ))
            .await?
            .check()?;
        let things: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let equips = things
            .into_iter()
            .map(pe_thing_to_refno)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((root, equips))
    }

    /// 这些设备的楼板边，`设备->楼板` 形式、排好序。
    async fn floor_edges(equips: &[RefnoEnum]) -> anyhow::Result<Vec<String>> {
        let mut r = SUL_DB
            .query(format!(
                "SELECT VALUE record::id(in) + '->' + record::id(out) FROM nearest_relate WHERE in IN [{}];",
                pe_keys(equips)
            ))
            .await?
            .check()?;
        let mut edges: Vec<String> = r.take(0)?;
        edges.sort();
        Ok(edges)
    }

    /// 容器根要重算它下面每一台设备；树已就绪时楼板边按「一台一条」换新，重跑不堆边。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会装载空间树并重写这些设备的 cal_equi / nearest_relate；必须单跑"]
    async fn container_root_refreshes_every_equip_and_relinks_floors() -> anyhow::Result<()> {
        attach_store_connected().await?;
        crate::fast_model::aabb_tree::load_project_tree_verified().await?;
        assert!(
            spatial_state::current_state().is_ready(),
            "装载后空间树应就绪，实际 {}",
            spatial_state::current_state().as_str()
        );

        let (root, equips) = zone_root_with_equips(2).await?;
        let started = std::time::Instant::now();
        let first = update_cal_equip_for_roots(&[root]).await?;
        eprintln!(
            "根 {}（{} 台 EQUI）→ {first:?}，{}ms",
            root.to_pe_key(),
            equips.len(),
            started.elapsed().as_millis()
        );
        assert_eq!(first.refreshed, equips.len(), "每台 EQUI 子件都要重算");
        assert_eq!(first.failed, 0);
        assert!(!first.floors_skipped, "树已就绪，不该跳过楼板边");
        assert!(first.floors_linked >= 1, "树已就绪，至少一台该打中楼板");
        let edges = floor_edges(&equips).await?;
        assert_eq!(
            edges.len(),
            first.floors_linked,
            "挂楼板的设备恰好一条边: {edges:?}"
        );

        let second = update_cal_equip_for_roots(&[root]).await?;
        assert_eq!(second, first, "同一个根重跑，结果应一致");
        assert_eq!(floor_edges(&equips).await?, edges, "重跑不能堆边或换边");
        Ok(())
    }

    /// 根本身是设备时要把它自己算进去；`cal_equi` 已有旧值也必须被覆盖；树没装时旧楼板边原样保留。
    #[tokio::test]
    #[ignore = "live: 需要真库 8009；会改写一台设备的 cal_equi；必须单跑（进程里不能装过空间树）"]
    async fn equip_root_overwrites_stale_world_trans_and_keeps_edges_without_tree()
    -> anyhow::Result<()> {
        attach_store_connected().await?;
        assert!(
            !spatial_state::current_state().is_ready(),
            "本测试要求进程里没装过空间树，实际 {}",
            spatial_state::current_state().as_str()
        );

        let mut r = SUL_DB
            .query(
                "SELECT VALUE in FROM nearest_relate WHERE in.noun = 'EQUI' AND in.deleted != true LIMIT 1;",
            )
            .await?
            .check()?;
        let found: Vec<surrealdb::sql::Thing> = r.take(0)?;
        let equip = pe_thing_to_refno(
            found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("库里没有挂着楼板边的 EQUI"))?,
        )?;
        let edges_before = floor_edges(&[equip]).await?;

        let cal_key = equip.to_table_key("cal_equi");
        SUL_DB
            .query(format!("UPSERT {cal_key} SET world_trans = trans:⟨stale_sentinel⟩;"))
            .await?
            .check()?;

        let outcome = update_cal_equip_for_roots(&[equip]).await?;
        eprintln!("根 {}（本身是设备）→ {outcome:?}", equip.to_pe_key());
        assert_eq!(outcome.refreshed, 1, "根本身是设备时要把它自己算进去");
        assert!(outcome.floors_skipped, "树没就绪必须跳过楼板边");

        let mut r = SUL_DB
            .query(format!("RETURN record::id({cal_key}.world_trans);"))
            .await?
            .check()?;
        let trans_id: Option<String> = r.take(0)?;
        let expected = gen_bytes_hash::<_, 64>(
            &aios_core::get_world_transform(equip)
                .await?
                .unwrap_or_default(),
        )
        .to_string();
        assert_eq!(
            trans_id.as_deref(),
            Some(expected.as_str()),
            "cal_equi 必须被覆盖成当前 world transform（CREATE 撞已有 id 会静默失败）"
        );
        assert_eq!(
            floor_edges(&[equip]).await?,
            edges_before,
            "树没就绪时旧楼板边必须原样保留"
        );
        Ok(())
    }
}
