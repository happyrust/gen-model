use std::sync::atomic::{AtomicBool, Ordering};

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefnoEnum, SUL_DB};

use crate::fast_model::occ_generate::update_inst_relate_aabbs_by_refnos;

/// 写回成功后应用窗口计算期间延迟的空间树变化（提交后收敛专用）。
///
/// refresh 分支从**已提交的主库**按 `inst_relate.aabb` 指针值同步树条目
/// （[`sync_tree_from_committed_pointers`]），不重算几何、不写库：窗口 journal
/// 重放已经把刷新层算出的值落成主库真值，这里只需要让树追上库。此前这一步复跑
/// `update_inst_relate_aabbs_by_refnos(.., true)`，对着主库把几何 AABB 整个重算
/// 并重写一遍——跑在「收敛失败即停止出队」的关键路径上（I7），时长随窗口规模
/// 线性涨，还平白引入几何 join 这块失败面。
///
/// 房间触发不在这里产生：AABB 房间目标已在窗口内并入 finalize plan 随尾事务
/// 持久化，收敛只负责树本身。
pub(crate) async fn apply_deferred_spatial_mutations(
    deferred: crate::data_interface::staging::write_context::DeferredSpatialMutations,
) -> anyhow::Result<()> {
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
        return Ok(());
    }
    let mut refnos = deferred.refresh.into_iter().collect::<Vec<_>>();
    refnos.sort();
    sync_tree_from_committed_pointers(&refnos).await?;
    Ok(())
}

/// `(refno, noun, aabb 指针值)` 的查询行——树条目所需的全部信息。
#[derive(serde::Deserialize)]
struct PointerRow {
    refno: RefnoEnum,
    noun: Option<String>,
    aabb: parry3d::bounding_volume::Aabb,
}

impl PointerRow {
    fn into_box(self) -> RStarBoundingBox {
        RStarBoundingBox::new(
            self.aabb,
            self.refno,
            self.noun.unwrap_or_else(|| "UNSET".to_string()),
        )
    }
}

/// 从已提交主库按指针值同步这些 refno 的树条目，返回实际进树的条数。
///
/// 进树口径与刷新层一致（`world_trans.d != none and aabb.d != none`）；库里已经
/// 没有可用指针的 refno 不进树——对应「从未刷新过 / 几何不可用」的行，它们本来
/// 也不在树上。查询失败上抛：提交后收敛失败必须阻断出队（I7），不能静默放行。
async fn sync_tree_from_committed_pointers(refnos: &[RefnoEnum]) -> anyhow::Result<usize> {
    const CHUNK: usize = 500;
    let mut synced = 0usize;
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        let inst_keys = aios_core::get_inst_relate_keys(chunk);
        let sql = format!(
            "select in as refno, in.noun as noun, aabb.d as aabb from {inst_keys} \
             where world_trans.d != none and aabb.d != none"
        );
        let mut response = SUL_DB
            .query(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取已提交包围盒指针失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取已提交包围盒指针语句失败: {e}"))?;
        let rows: Vec<PointerRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析已提交包围盒指针失败: {e}"))?;
        if rows.len() < chunk.len() {
            // 树的口径是「镜像已提交指针」：指针已消失的 refresh 目标必须摘除，
            // 跳过会让旧盒一直留在树上当房间候选，直到下一次指针重建才自愈。
            let present = rows
                .iter()
                .map(|row| row.refno.refno())
                .collect::<std::collections::HashSet<_>>();
            let vanished = chunk
                .iter()
                .map(RefnoEnum::refno)
                .filter(|refno| !present.contains(refno))
                .collect::<std::collections::HashSet<_>>();
            println!(
                "提交后空间收敛：{} 个 refresh 目标中 {} 个在主库已无可用指针，摘除其树条目",
                chunk.len(),
                vanished.len()
            );
            if GLOBAL_AABB_TREE.write().await.remove_by_refnos(&vanished) > 0 {
                mark_aabb_tree_dirty();
            }
        }
        let boxes = rows
            .into_iter()
            .map(PointerRow::into_box)
            .collect::<Vec<_>>();
        let count = boxes.len();
        let stale = GLOBAL_AABB_TREE.write().await.sync_refnos(boxes);
        if count > 0 || !stale.is_empty() {
            mark_aabb_tree_dirty();
        }
        synced += count;
    }
    Ok(synced)
}

/// 空间树自上次写回项目树文件以来是否有未持久化的增量变更。
///
/// 增量路径（AABB 刷新、删除清理）只更新内存树；已提交的空间意图
/// （`spatial_reconcile` 行）保证崩溃后能从库里重放，这个标记决定的只是
/// 「这一轮要不要真的写文件」。落盘时机归 worker 空闲轮（ADR-010 落盘时机，
/// 2026-07-28 已决）。
static AABB_TREE_DIRTY: AtomicBool = AtomicBool::new(false);

/// 增量路径动过内存树之后调用：标记「有变更待落盘」。
pub fn mark_aabb_tree_dirty() {
    AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
}

/// 本项目空间树的落盘文件：`accel_tree_{project}.bin`（ADR-010 §6「路径带项目名」）。
///
/// 读写都由本仓自己做：bincode 编码与 rs-core 的 `serialize_to_bin_file` 同构
/// （`AccelerationTree` 的反向索引等派生字段是 `#[serde(skip)]`，不进文件），
/// 反序列化后的索引由 `ensure_refno_index` 在首次按 refno 操作时自愈重建，
/// 有单测钉着这条假设。rs-core 硬编码的裸 `accel_tree.bin` 不再参与——此前的
/// 「搬运语义」（加载前复制到裸名、落盘后归档回项目名）在多项目并发共用 cwd
/// 时是竞态窗口，现在整个消失。
pub fn project_tree_file() -> String {
    format!("accel_tree_{}.bin", aios_core::get_db_option().project_name)
}

/// 树文件的 sidecar 元数据：`accel_tree_{project}.meta.json`。
fn project_tree_meta_file() -> String {
    format!(
        "accel_tree_{}.meta.json",
        aios_core::get_db_option().project_name
    )
}

/// sidecar 内容：树文件对应的库侧空间版本号与条目数。
///
/// 启动默认直接复用树文件，不再因 epoch 不一致自动重建；这些字段只作诊断。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TreeFileMeta {
    epoch: u64,
    entries: u64,
    saved_at_unix: u64,
}

/// 原子写文件：先写临时文件再 rename 覆盖。
///
/// 这个文件由空闲轮反复重写（17 MB 量级），原地重写意味着每次落盘都有一个
/// 「写半截崩溃 → 文件损坏」的窗口。std 的 `rename` 在 Windows 上带
/// REPLACE_EXISTING 语义，读者要么看到旧文件要么看到新文件（与 rs-core 旧
/// 路径同一纪律）。
fn write_file_atomic(path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let tmp = format!("{path}.tmp");
    {
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| anyhow::anyhow!("创建临时文件 {tmp} 失败: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| anyhow::anyhow!("写入临时文件 {tmp} 失败: {e}"))?;
        file.sync_all()
            .map_err(|e| anyhow::anyhow!("同步临时文件 {tmp} 失败: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| anyhow::anyhow!("覆盖 {path} 失败: {e}"))?;
    Ok(())
}

fn write_project_tree_file(tree: &AccelerationTree) -> anyhow::Result<()> {
    let bytes = bincode::serialize(tree).map_err(|e| anyhow::anyhow!("序列化空间树失败: {e}"))?;
    write_file_atomic(&project_tree_file(), &bytes)
}

fn read_project_tree_file() -> anyhow::Result<AccelerationTree> {
    let path = project_tree_file();
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("读取 {path} 失败: {e}"))?;
    bincode::deserialize(&bytes).map_err(|e| anyhow::anyhow!("反序列化 {path} 失败: {e}"))
}

fn write_tree_meta(meta: &TreeFileMeta) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(meta)?;
    write_file_atomic(&project_tree_meta_file(), &bytes)
}

fn read_tree_meta() -> anyhow::Result<TreeFileMeta> {
    let path = project_tree_meta_file();
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("读取 {path} 失败: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("解析 {path} 失败: {e}"))
}

/// 库侧空间版本号所在的固定记录。
///
/// 每条携带空间意图（refresh / remove）的尾事务顺带把它 +1
/// （[`render_spatial_epoch_bump`]）——水位、意图、版本号同一个事务里同生同死。
/// 启动时 sidecar 的 epoch 与它不相等，就说明「树文件之后还有过没被镜像进文件
/// 的空间提交」，文件不可信。
const SPATIAL_EPOCH_ID: &str = "spatial_epoch:current";

/// 渲染尾事务里的空间版本号递增语句（与 `spatial_reconcile` 意图同一事务使用）。
///
/// 窗口重试导致的多次递增无害：版本号只与 sidecar 比相等、不表达次数，多 bump
/// 一次至多让下次启动多做一次指针重建。
pub(crate) fn render_spatial_epoch_bump() -> String {
    format!("UPSERT {SPATIAL_EPOCH_ID} SET value = (value?:0) + 1, updated_at = time::now();")
}

#[derive(serde::Deserialize)]
struct EpochRow {
    #[serde(default)]
    value: u64,
}

/// 读库侧当前空间版本号（记录不存在按 0）。
pub(crate) async fn read_db_spatial_epoch() -> anyhow::Result<u64> {
    let mut response = SUL_DB
        // `value` 是 SurrealQL 的保留字（`SELECT VALUE …` 的投影修饰符）：不加反引号
        // 整条语句在 parse 阶段就失败，而启动路径上的 `?` 会把整个进程带下去。
        .query(format!("SELECT `value` FROM {SPATIAL_EPOCH_ID};"))
        .await
        .map_err(|e| anyhow::anyhow!("读取空间版本号失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取空间版本号语句失败: {e}"))?;
    Ok(response
        .take::<Vec<EpochRow>>(0)?
        .first()
        .map(|row| row.value)
        .unwrap_or(0))
}

/// 序列化当前内存树到项目文件并盖 sidecar 章。
///
/// epoch 在写文件**之前**读：并发的尾事务若在读章与写盘之间又推高了版本号，
/// sidecar 只会偏旧 → 下次启动宁可多做一次指针重建，方向安全；反过来先写后读
/// 会把新章盖在旧内容上。直写 / 全量路径不递增 epoch，但它们改完树都会走到
/// 这里落盘，章一样盖得上。
async fn persist_project_tree_now() -> anyhow::Result<()> {
    let epoch = read_db_spatial_epoch().await?;
    let entries = {
        let tree = GLOBAL_AABB_TREE.read().await;
        write_project_tree_file(&tree)?;
        tree.size() as u64
    };
    write_tree_meta(&TreeFileMeta {
        epoch,
        entries,
        saved_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}

/// 脏则写回项目树文件（worker 空闲轮收尾调用），返回是否真的写了。
///
/// 落盘失败时**保留**脏标记，下一轮重试——清掉的话一次磁盘抖动就把变更永远留在内存里。
pub async fn persist_aabb_tree_if_dirty() -> anyhow::Result<bool> {
    if !AABB_TREE_DIRTY.swap(false, Ordering::SeqCst) {
        return Ok(false);
    }
    if let Err(error) = persist_project_tree_now().await {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        return Err(error);
    }
    Ok(true)
}

/// 无条件写回并清脏标记（全量生成收尾、对账重建后走这里）。
///
/// 全量序列化覆盖了此前一切增量变更，所以顺手清标记，免得空闲轮紧接着再白写一遍。
pub async fn persist_aabb_tree() -> anyhow::Result<()> {
    persist_project_tree_now().await?;
    AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
    Ok(())
}

/// 启动加载：默认直接复用本项目已经落盘的空间树，不做 epoch 对账，也不自动重建。
///
/// 只有显式设置 `AIOS_FORCE_SPATIAL_REBUILD` 时才从库指针分页重建。树文件缺失或
/// 损坏时保持空树启动并打印操作提示，避免一次启动隐式触发耗时的全库读取。
/// 已在内存里的树保持不动（live 夹具先填树再启动的场景）。
pub async fn load_project_tree_verified() -> anyhow::Result<()> {
    if !GLOBAL_AABB_TREE.read().await.is_empty() {
        return Ok(());
    }
    if std::env::var("AIOS_FORCE_SPATIAL_REBUILD").is_ok() {
        println!("按环境变量要求跳过空间树文件，从库指针重建");
        return rebuild_tree_from_pointers().await;
    }

    match read_project_tree_file() {
        Ok(tree) => {
            let entries = tree.size();
            *GLOBAL_AABB_TREE.write().await = tree;
            let epoch = read_tree_meta()
                .map(|meta| meta.epoch.to_string())
                .unwrap_or_else(|_| "未知".to_string());
            println!(
                "空间树直接复用项目文件 {}（{entries} 条，记录 epoch {epoch}；未触发重建）",
                project_tree_file()
            );
        }
        Err(error) => {
            eprintln!(
                "空间树文件不可用（{error:#}），默认保持空树且不重建；如需重建请设置 AIOS_FORCE_SPATIAL_REBUILD=1"
            );
        }
    }
    Ok(())
}

/// 从库指针整树重建：分页读 `inst_relate` 的 `(refno, noun, aabb.d)`，bulk-load
/// 进全局树后立即落盘盖章。
///
/// 只读不写库；进树口径与刷新层一致（`world_trans.d != none and aabb.d != none`）。
/// 没赶上刷新的行（指针缺失）不进树——它们此前也从不在树上；真要把这类行补进
/// 来（重算几何并回写指针），用显式修复工具 [`manual_update_aabbs`]。
pub async fn rebuild_tree_from_pointers() -> anyhow::Result<()> {
    const PAGE: usize = 5000;
    let mut boxes: Vec<RStarBoundingBox> = Vec::new();
    let mut offset = 0usize;
    loop {
        let sql = format!(
            "select in as refno, in.noun as noun, aabb.d as aabb from inst_relate \
             where world_trans.d != none and aabb.d != none limit {PAGE} start {offset}"
        );
        let mut response = SUL_DB
            .query(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("分页读取包围盒指针失败（start {offset}）: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("分页读取包围盒指针语句失败（start {offset}）: {e}"))?;
        let rows: Vec<PointerRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析包围盒指针失败（start {offset}）: {e}"))?;
        let fetched = rows.len();
        boxes.extend(rows.into_iter().map(PointerRow::into_box));
        if fetched < PAGE {
            break;
        }
        offset += PAGE;
    }
    let entries = boxes.len();
    *GLOBAL_AABB_TREE.write().await = AccelerationTree::load(boxes);
    // 重建产物立即落盘盖章，不落的话下次启动还得再重建一遍。
    persist_aabb_tree().await?;
    println!("空间树已从库指针重建并落盘: {entries} 条");
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

/// 用库里的包围盒数量与内存空间树对账，少了就重建（**手工诊断 / 修复入口**）。
///
/// 启动路径已不再调用它——epoch 校验（[`load_project_tree_verified`]）取代了
/// 条数对账：条数辨认不出同数漂移，而这里的兜底重建 `manual_update_aabbs(true)`
/// 会全库重算几何并回写 `inst_relate.aabb`，又慢又重。保留它是因为指针重建
/// 覆盖不了「指针本身缺失 / 陈旧」的修复场景（几何在而 aabb 从没算过）——
/// 那正是这条重算路径的正当用途。
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

/// 手动更新所有 inst_relate 的 AABB 包围盒（**显式修复工具**，启动路径不再依赖）。
///
/// 此函数会分批遍历数据库中 inst_relate 表中的条目，
/// 获取它们的引用号（refnos），然后调用 update_inst_relate_aabbs_by_refnos
/// 函数更新这些条目的 AABB 包围盒数据——重算几何并**回写库**，与只读的
/// [`rebuild_tree_from_pointers`] 是两种工具：指针陈旧或缺失时用它补账。
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
    /// `persist_*` 会写真实的项目树文件，单测不能实跑，只能钉源码。
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

    /// 提交后收敛不得重算几何、不得写库：窗口 journal 重放已经把刷新层的计算结果
    /// 落成主库真值，收敛只许「按指针把树追上库」。此前这里复跑
    /// `update_inst_relate_aabbs_by_refnos(.., true)`，把几何 AABB 整个重算并重写
    /// 一遍——跑在阻断出队的关键路径上。回退即红。
    #[test]
    fn post_commit_reconcile_syncs_pointers_instead_of_recomputing() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!(
                "pub(crate) async fn ",
                "apply_deferred_spatial_mutations("
            ))
            .expect("apply_deferred_spatial_mutations must exist")
            .1
            .split_once(concat!("\n", "/// `(refno, noun, aabb 指针值)`"))
            .expect("PointerRow doc must follow")
            .0;
        assert!(
            body.contains("sync_tree_from_committed_pointers"),
            "refresh 分支必须走指针同步: {body}"
        );
        assert!(
            !body.contains("update_inst_relate_aabbs_by_refnos"),
            "提交后收敛不得重算几何/写库: {body}"
        );
    }

    /// 启动重建只许读库：`rebuild_tree_from_pointers` 是 epoch 不匹配时的兜底，
    /// 若它回到 `manual_update_aabbs` 那条重算路径，每次文件失配的启动都会把
    /// 整个 `inst_relate.aabb` 列重写一遍。回退即红。
    #[test]
    fn pointer_rebuild_reads_only() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("pub async fn ", "rebuild_tree_from_pointers("))
            .expect("rebuild_tree_from_pointers must exist")
            .1
            .split_once(concat!("\n", "#[derive(serde::Deserialize)]"))
            .expect("CountRow must follow")
            .0;
        assert!(
            !body.contains("manual_update_aabbs") && !body.contains("update_inst_relate_aabbs"),
            "指针重建不得重算几何/回写库: {body}"
        );
        assert!(
            body.contains("AccelerationTree::load"),
            "重建必须 bulk-load 整树: {body}"
        );
    }

    /// 默认启动必须直接复用项目树；只有显式环境变量才能进入指针重建。
    #[test]
    fn startup_reuses_project_tree_unless_rebuild_is_explicitly_requested() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("pub async fn ", "load_project_tree_verified("))
            .expect("load_project_tree_verified must exist")
            .1
            .split_once(concat!("pub async fn ", "rebuild_tree_from_pointers("))
            .expect("rebuild must follow")
            .0;
        assert!(
            body.contains("read_project_tree_file()"),
            "默认启动必须读取并复用项目树文件: {body}"
        );
        assert!(
            body.contains("std::env::var(\"AIOS_FORCE_SPATIAL_REBUILD\")")
                && body.contains("return rebuild_tree_from_pointers().await"),
            "指针重建必须只由显式环境变量触发: {body}"
        );
        assert!(
            !body.contains("read_db_spatial_epoch")
                && !body.contains("direct_increment_enabled")
                && !body.contains("sync_aabb_tree_with_db"),
            "默认启动不得因 epoch、直写模式或条数对账触发重建: {body}"
        );
    }

    /// 树的口径是「镜像已提交指针」：指针已消失的 refresh 目标必须摘除树条目，
    /// 而不是跳过——跳过会让旧盒一直留在树上当房间候选，直到下一次指针重建
    /// 才自愈。回退即红。
    #[test]
    fn pointer_sync_evicts_targets_whose_pointer_vanished() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("async fn ", "sync_tree_from_committed_pointers("))
            .expect("sync_tree_from_committed_pointers must exist")
            .1
            .split_once(concat!("\n", "/// 空间树自上次写回"))
            .expect("dirty-flag doc must follow")
            .0;
        assert!(
            body.contains("remove_by_refnos"),
            "指针消失的 refresh 目标必须摘除树条目: {body}"
        );
    }

    /// R3 的安全前提：绕开 rs-core 的 `deserialize_from_bin_file`（它私有地重建
    /// 反向索引）直接 bincode 反序列化后，首次按 refno 操作必须自愈索引——
    /// 否则 `sync_refnos` 删不中旧盒，同一 refno 会在树里堆叠历史包围盒。
    #[test]
    fn deserialized_tree_self_heals_its_refno_index() {
        fn bbox(seq: u64, min: f32, max: f32) -> RStarBoundingBox {
            RStarBoundingBox::new(
                parry3d::bounding_volume::Aabb::new(
                    parry3d::math::Point::new(min, min, min),
                    parry3d::math::Point::new(max, max, max),
                ),
                RefnoEnum::from(format!("4000000001/{seq}").as_str()),
                "BOX".to_string(),
            )
        }
        let tree = AccelerationTree::load(vec![bbox(1, 0.0, 10.0), bbox(2, 20.0, 30.0)]);
        let bytes = bincode::serialize(&tree).expect("serialize");
        let mut restored: AccelerationTree = bincode::deserialize(&bytes).expect("deserialize");

        let stale = restored.sync_refnos(vec![bbox(1, 100.0, 110.0)]);
        assert_eq!(stale.len(), 1, "反向索引必须自愈，否则旧盒删不中");
        assert_eq!(restored.size(), 2, "同一 refno 不允许堆叠历史包围盒");
    }

    /// sidecar 元数据的编解码往返。
    #[test]
    fn tree_meta_roundtrip() {
        let meta = TreeFileMeta {
            epoch: 42,
            entries: 906,
            saved_at_unix: 1_754_000_000,
        };
        let bytes = serde_json::to_vec(&meta).expect("encode");
        let back: TreeFileMeta = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(back.epoch, 42);
        assert_eq!(back.entries, 906);
    }

    /// 版本号递增语句：固定记录、自增一、缺省从 0 起。
    #[test]
    fn epoch_bump_targets_the_singleton_record() {
        let sql = render_spatial_epoch_bump();
        assert!(sql.contains(SPATIAL_EPOCH_ID), "必须写固定记录: {sql}");
        assert!(sql.contains("(value?:0) + 1"), "必须缺省 0 自增一: {sql}");
    }

    /// D8（ADR-010）：`accel_tree.bin` 里只要残留几条，旧的 `is_empty()` 判断就不会触发
    /// 重建，树永久停在残留状态——实测历史日志里它最多只到 45 条，而库里有 906 个包围盒。
    /// 本用例连真库，对比重建前后的条目数。
    ///
    /// 会写库（重算并回写 `inst_relate.aabb`）与 cwd 下的项目树文件，故默认 ignore。
    /// 用法：
    /// `AIOS_LIVE_WS=ws://localhost:8009 cargo test live_sync_aabb_tree -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: rewrites inst_relate.aabb and the project tree file"]
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
