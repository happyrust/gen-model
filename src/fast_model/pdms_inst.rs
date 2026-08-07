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

            // 为TUBI创建inst_relate记录
            let tubi_relate_sql = format!(
                "{{id: {0},  in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, aabb: aabb:⟨{4}⟩, generic: '{5}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), anc: fn::anc_u64({1}), dbnum: {1}.dbnum, has_cata_neg: {6}, solid: {7}}}",
                k.to_inst_relate_key(),
                k.to_pe_key(),
                v.id_str(),
                transform_hash,
                aabb_hash,
                v.generic_type.to_string(),
                v.has_cata_neg,
                v.is_solid,
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

        let relate_sql = format!(
            "{{id: {0},  in: {1}, out: inst_info:⟨{2}⟩, world_trans: trans:⟨{3}⟩, generic: '{4}', zone_refno: fn::find_ancestor_type({1}, 'ZONE'), anc: fn::anc_u64({1}), dbnum: {1}.dbnum, dt: fn::ses_date({1}), has_cata_neg: {5}, solid: {6}}}",
            k.to_inst_relate_key(),
            k.to_pe_key(),
            v.id_str(),
            transform_hash,
            v.generic_type.to_string(),
            v.has_cata_neg,
            v.is_solid,
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

        // 从权威脚本原文抠出两个函数定义（refno_u64 起、到 anc_u64 的收尾 `};`），
        // 避免测试里再抄一份出现两个事实来源。
        let common = std::fs::read_to_string("resource/surreal/common.surql")
            .expect("read resource/surreal/common.surql");
        let start = common
            .find("DEFINE FUNCTION OVERWRITE fn::refno_u64")
            .expect("common.surql 里应有 fn::refno_u64 定义");
        let anc_at = common[start..]
            .find("DEFINE FUNCTION OVERWRITE fn::anc_u64")
            .expect("common.surql 里应有 fn::anc_u64 定义");
        let end = common[start + anc_at..]
            .find("\n};")
            .expect("anc_u64 定义应以 `};` 收尾");
        let defines = &common[start..start + anc_at + end + 3];
        SUL_DB
            .query(defines)
            .await
            .expect("define fn::refno_u64 / fn::anc_u64")
            .check()
            .expect("define check");
        println!("[live] fn::refno_u64 / fn::anc_u64 已灌入");

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
