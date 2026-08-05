use std::sync::atomic::{AtomicBool, Ordering};

use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefnoEnum, SUL_DB};
use dashmap::DashMap;
use itertools::Itertools;

use crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos;

/// 写回成功后应用窗口计算期间延迟的空间树变化，并返回房间增量触发集。
pub(crate) async fn apply_deferred_spatial_mutations(
    deferred: crate::data_interface::staging::write_context::DeferredSpatialMutations,
) -> anyhow::Result<Vec<crate::fast_model::occ_generate::AabbChange>> {
    if !deferred.remove.is_empty() {
        let removed = deferred
            .remove
            .iter()
            .map(RefnoEnum::refno)
            .collect::<std::collections::HashSet<_>>();
        if GLOBAL_AABB_TREE.write().await.remove_by_refnos(&removed) > 0 {
            mark_aabb_tree_dirty();
        }
    }
    if deferred.refresh.is_empty() {
        return Ok(Vec::new());
    }
    let refnos = deferred.refresh.into_iter().collect::<Vec<_>>();
    update_inst_relate_aabbs_by_refnos(&refnos, true).await
}

/// 空间树自上次写回 `accel_tree.bin` 以来是否有未持久化的增量变更。
///
/// 增量路径（AABB 刷新、删除清理）只更新内存树；不落盘的话，重启后
/// `load_aabb_tree` 读回旧文件、`sync_aabb_tree_with_db` 又只对账**数量**
/// （搬动不改变条数），启动时的全量房间重建就会拿旧位置把增量已收敛的
/// `room_relate` 边改写回搬家前的状态——不是「不再收敛」，是「主动回退」。
/// 落盘时机归 worker 空闲轮（ADR-010 落盘时机，2026-07-28 已决）。
static AABB_TREE_DIRTY: AtomicBool = AtomicBool::new(false);

/// 增量路径动过内存树之后调用：标记「有变更待落盘」。
pub fn mark_aabb_tree_dirty() {
    AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
}

/// rs-core 硬编码的裸文件名：`serialize_to_bin_file` / `deserialize_from_bin_file`
/// 都写死它（cwd 相对），且重建 refno 反向索引的方法是私有的，本仓无法绕开
/// rs-core 自己读写别的路径。
const BARE_TREE_FILE: &str = "accel_tree.bin";

/// 带项目名的落盘文件：`accel_tree_{project}.bin`（ADR-010 §6「路径带项目名」）。
fn project_tree_file() -> String {
    format!(
        "accel_tree_{}.bin",
        aios_core::get_db_option().project_name
    )
}

/// 启动加载空间树**之前**调用：把本项目的树文件放到 rs-core 硬编码的裸路径上。
///
/// 裸文件名的两个后果都有实证：换工作目录启动静默空树（2026-08-04 演练日志），
/// 多项目先后共用一个部署目录时读到**别的项目**的树——重启后
/// `sync_aabb_tree_with_db` 只对账数量，随后启动期的全量房间重建会拿错树的
/// 旧位置改写 `room_relate`。搬运语义：
/// - 项目专属文件存在 → 复制到裸名（覆盖别的项目残留）；
/// - 只有裸文件 → 首次迁移，沿用它，下次落盘起写回项目名；
/// - 都没有 → 空树告警由 rs-core 加载路径给出。
///
/// 已知限制：多项目**并发**共用同一个 cwd 时，裸文件仍是竞态窗口——rs-core
/// 硬编码之下无解，先后切换项目的场景（实际部署形态）已由本函数闭环。
pub fn stage_project_aabb_tree_file() {
    let project_file = project_tree_file();
    if std::path::Path::new(&project_file).is_file() {
        match std::fs::copy(&project_file, BARE_TREE_FILE) {
            Ok(_) => println!("空间树使用项目专属文件 {project_file}"),
            Err(error) => eprintln!(
                "放置项目空间树文件失败（{project_file} -> {BARE_TREE_FILE}），\
                 将按现有裸文件或空树启动: {error}"
            ),
        }
        return;
    }
    if std::path::Path::new(BARE_TREE_FILE).is_file() {
        println!(
            "未找到 {project_file}，沿用既有 {BARE_TREE_FILE}\
            （首次迁移：下次落盘起写入项目专属文件）"
        );
    }
}

/// 落盘成功后把裸文件归档为项目专属名。
///
/// 失败必须上抛：吞掉的话项目文件停在旧值，下次启动 `stage` 会拿旧树覆盖
/// 刚写好的裸文件——比不归档更糟。调用方靠保留脏位让下一轮连同序列化一起重试。
fn archive_project_aabb_tree_file() -> anyhow::Result<()> {
    let project_file = project_tree_file();
    std::fs::copy(BARE_TREE_FILE, &project_file)
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("归档空间树到 {project_file} 失败: {error}"))
}

/// 脏则写回 `accel_tree.bin` 并归档项目专属文件（worker 空闲轮收尾调用），
/// 返回是否真的写了。
///
/// 落盘失败时**保留**脏标记，下一轮重试——清掉的话一次磁盘抖动就把变更永远留在内存里。
pub async fn persist_aabb_tree_if_dirty() -> anyhow::Result<bool> {
    if !AABB_TREE_DIRTY.swap(false, Ordering::SeqCst) {
        return Ok(false);
    }
    let written = match GLOBAL_AABB_TREE.read().await.serialize_to_bin_file() {
        Ok(_) => archive_project_aabb_tree_file(),
        Err(error) => Err(error),
    };
    if let Err(error) = written {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        return Err(error);
    }
    Ok(true)
}

/// 无条件写回并清脏标记（全量生成收尾、对账重建后走这里）。
///
/// 全量序列化覆盖了此前一切增量变更，所以顺手清标记，免得空闲轮紧接着再白写一遍。
pub async fn persist_aabb_tree() -> anyhow::Result<()> {
    GLOBAL_AABB_TREE.read().await.serialize_to_bin_file()?;
    archive_project_aabb_tree_file()?;
    AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
    Ok(())
}

#[derive(serde::Deserialize)]
struct CountRow {
    count: i64,
}

/// 取一条 `SELECT count() … GROUP ALL` 的结果。
/// 注意本仓的 SurrealDB fork 不会对 `SELECT VALUE count()` 解包，仍返回 `{count: n}`。
async fn count_rows(sql: &str) -> anyhow::Result<usize> {
    let mut response = SUL_DB.query(sql).await?.check()?;
    Ok(response
        .take::<Vec<CountRow>>(0)?
        .first()
        .map(|row| row.count)
        .unwrap_or(0)
        .max(0) as usize)
}

/// 用库里的包围盒数量与内存空间树对账，少了就重建。
///
/// 树只有 `manual_update_aabbs` 一个填充入口——`load_aabb_tree` 从库分页 bulk-load
/// 的那段是注释掉的，它只反序列化 `accel_tree.bin`。而旧的重建条件是「树为空」，
/// 于是文件里只要残留几条，`is_empty()` 就为假，重建永远不触发，树就永久停在残留
/// 状态，库里其余的包围盒一个都进不来（ADR-010 D8）。
pub async fn sync_aabb_tree_with_db() -> anyhow::Result<()> {
    let tree_count = GLOBAL_AABB_TREE.read().await.tree.size();

    // 快路径：存量包围盒数是上界，一条简单计数就能放行健康的树。
    let stored_count =
        count_rows("SELECT count() FROM inst_relate WHERE aabb != none GROUP ALL").await?;
    if stored_count == 0 || tree_count >= stored_count {
        return Ok(());
    }

    // 慢路径：上界不满足不代表树是坏的。实测本库 906 个存量包围盒里只有 403 个还能从
    // geo 侧重算出来（另外 503 个的几何源已经取不到 aabb 了），拿上界当判据会导致每次
    // 启动都重建一遍。所以再算一次「重建真正能产出多少」，只有确实更少才动手。
    //
    // 「能产出」有两类：geo 侧可重算的，以及行内已带 aabb 指针的（隐含直管段这类
    // 插入时写死 aabb 的行——刷新层现在以指针值为真值同样能把它送进树，见 ADR-010
    // D13）。漏掉后者，管段缺席空间树时这里永远不会触发重建补账。
    let buildable_count = count_rows(
        "SELECT count() FROM inst_relate \
         WHERE world_trans.d != none \
           AND (aabb.d != none \
             OR count((SELECT id FROM out->geo_relate WHERE out.aabb.d != none AND trans.d != none)) > 0) \
         GROUP ALL",
    )
    .await?;
    if tree_count >= buildable_count {
        return Ok(());
    }

    println!(
        "空间树只有 {tree_count} 条，可重建 {buildable_count} 条（库中存量 {stored_count}），正在重建空间树..."
    );
    manual_update_aabbs(true).await?;
    let rebuilt = GLOBAL_AABB_TREE.read().await.tree.size();
    println!("空间树重建完成: {rebuilt} 条");
    persist_aabb_tree().await?;
    Ok(())
}

/// 手动更新所有 inst_relate 的 AABB 包围盒
///
/// 此函数会分批遍历数据库中 inst_relate 表中的条目，
/// 获取它们的引用号（refnos），然后调用 update_inst_relate_aabbs_by_refnos
/// 函数更新这些条目的 AABB 包围盒数据。
///
/// # 参数
///
/// * `replace_exist` - 是否替换已存在的包围盒数据
///
/// # 返回值
///
/// 返回 `anyhow::Result<()>` 表示更新是否成功
pub async fn manual_update_aabbs(replace_exist: bool) -> anyhow::Result<()> {
    // 查询和处理的批次大小
    const QUERY_CHUNK_SIZE: usize = 1000;
    const PROCESS_CHUNK_SIZE: usize = 100;

    let mut total_processed = 0;
    let mut offset = 0;

    loop {
        // 分批查询 inst_relate 的键
        let sql = format!(
            "SELECT value in.id AS refno FROM inst_relate LIMIT {QUERY_CHUNK_SIZE} START {offset}"
        );
        let mut response = SUL_DB.query(&sql).await?;

        let refnos: Vec<RefnoEnum> = response.take(0).unwrap();
        if refnos.is_empty() {
            break;
        }

        // 处理这批 refnos
        if !refnos.is_empty() {
            println!(
                "Processing batch of {} inst_relate entries (offset: {})",
                refnos.len(),
                offset
            );

            // 进一步分批处理，每批最多PROCESS_CHUNK_SIZE个
            for (i, chunk) in refnos.chunks(PROCESS_CHUNK_SIZE).enumerate() {
                println!("  Sub-batch {}, size: {}", i + 1, chunk.len());
                update_inst_relate_aabbs_by_refnos(chunk, replace_exist).await?;
            }

            total_processed += refnos.len();
        }

        // 更新偏移量，准备查询下一批
        offset += QUERY_CHUNK_SIZE;
    }

    println!(
        "Successfully updated AABBs for all {} inst_relate entries",
        total_processed
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::room::room::load_aabb_tree;
    use surrealdb::opt::auth::Root;

    /// 落盘失败必须保留脏标记（清掉等于把变更永远留在内存里），成功路径才允许清。
    /// `persist_*` 会写真实的 `accel_tree.bin`，单测不能实跑，只能钉源码。
    #[test]
    fn persist_failure_keeps_the_dirty_flag() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("pub async fn ", "persist_aabb_tree_if_dirty("))
            .expect("persist_aabb_tree_if_dirty must exist")
            .1
            .split_once(concat!("pub async fn ", "persist_aabb_tree("))
            .expect("unconditional persist must follow")
            .0;
        let restore_at = body
            .find("AABB_TREE_DIRTY.store(true")
            .expect("failure branch must restore the dirty flag");
        let err_at = body
            .find("return Err")
            .expect("failure branch must propagate");
        assert!(
            restore_at < err_at,
            "脏标记必须在错误返回之前恢复，否则一次磁盘抖动就丢掉待落盘变更"
        );
    }

    #[test]
    fn marking_dirty_is_observable() {
        AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
        mark_aabb_tree_dirty();
        assert!(AABB_TREE_DIRTY.swap(false, Ordering::SeqCst));
    }

    /// D8（ADR-010）：`accel_tree.bin` 里只要残留几条，旧的 `is_empty()` 判断就不会触发
    /// 重建，树永久停在残留状态——实测历史日志里它最多只到 45 条，而库里有 906 个包围盒。
    /// 本用例连真库，对比重建前后的条目数。
    ///
    /// 会写库（重算并回写 `inst_relate.aabb`）与 cwd 下的 `accel_tree.bin`，故默认 ignore。
    /// 用法：
    /// `AIOS_LIVE_WS=ws://localhost:8009 cargo test live_sync_aabb_tree -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: rewrites inst_relate.aabb and accel_tree.bin"]
    async fn live_sync_aabb_tree_fills_tree_from_db() {
        let endpoint = std::env::var("AIOS_LIVE_WS").expect("set AIOS_LIVE_WS");
        let ns = std::env::var("AIOS_LIVE_NS").unwrap_or_else(|_| "1516".into());
        let db = std::env::var("AIOS_LIVE_DB").unwrap_or_else(|_| "AvevaMarineSample".into());

        SUL_DB
            .connect(endpoint)
            .with_capacity(1000)
            .await
            .expect("connect");
        SUL_DB.use_ns(&ns).use_db(&db).await.expect("use ns/db");
        SUL_DB
            .signin(Root {
                username: "root",
                password: "root",
            })
            .await
            .expect("signin");

        load_aabb_tree().await.expect("load accel_tree.bin");
        let before = GLOBAL_AABB_TREE.read().await.tree.size();
        sync_aabb_tree_with_db().await.expect("sync tree with db");
        let after = GLOBAL_AABB_TREE.read().await.tree.size();
        println!("GLOBAL_AABB_TREE: {before} -> {after}");
        assert!(after >= before, "空间树反而变小了: {before} -> {after}");
    }
}
