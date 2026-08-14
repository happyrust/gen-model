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
        // 无效指针值（NaN/Inf/反向范围）按「无可用指针」对待——与全量扫描同一
        // 口径（aabb_is_usable），否则重建后的树与增量维护的树对同一行给出两种
        // 裁决，条目集合永远对不拢。
        let (usable, unusable): (Vec<_>, Vec<_>) =
            rows.into_iter().partition(|row| aabb_is_usable(&row.aabb));
        if usable.len() < chunk.len() {
            // 树的口径是「镜像已提交指针」：指针已消失的 refresh 目标必须摘除，
            // 跳过会让旧盒一直留在树上当房间候选，直到下一次指针重建才自愈。
            let present = usable
                .iter()
                .map(|row| row.refno.refno())
                .collect::<std::collections::HashSet<_>>();
            let vanished = chunk
                .iter()
                .map(RefnoEnum::refno)
                .filter(|refno| !present.contains(refno))
                .collect::<std::collections::HashSet<_>>();
            println!(
                "提交后空间收敛：{} 个 refresh 目标中 {} 个在主库已无可用指针（含 {} 条无效值），摘除其树条目",
                chunk.len(),
                vanished.len(),
                unusable.len()
            );
            if GLOBAL_AABB_TREE.write().await.remove_by_refnos(&vanished) > 0 {
                mark_aabb_tree_dirty();
            }
        }
        let boxes = usable
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
/// 增量路径（AABB 刷新、删除清理）只更新内存树，这个标记决定的只是「这一轮要不要
/// 真的写文件」；落盘时机归 worker 空闲轮（ADR-010 落盘时机，2026-07-28 已决）。
/// 它跨不过重启，所以不承担正确性——崩溃后丢掉的内存树变更由库侧痕迹兜底：暂存
/// 路径靠 `spatial_reconcile` 意图行重放，直写路径靠同事务的 epoch bump 让启动
/// 判据落到指针重建。
static AABB_TREE_DIRTY: AtomicBool = AtomicBool::new(false);

/// 增量路径动过内存树之后调用：标记「有变更待落盘」。
pub fn mark_aabb_tree_dirty() {
    AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
}

/// 旧格式（V1）树文件：`accel_tree_{project}.bin`（ADR-010 §6「路径带项目名」）。
///
/// 自 V2 单文件快照（2026-08-12 方案 §3）起**只读不写**：仅作一次性迁移候选，
/// 首次 V2 发布成功后被 [`remove_legacy_tree_files`] 删除（D3——留着它会给
/// 「回退旧二进制 + 恰有 pending」开静默陈旧窗口；旧代码对 bin 缺失是无条件
/// 指针重建，删除让任何回退自动安全）。
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

/// sidecar 内容：树文件对应的库侧空间指纹与条目数。
///
/// `(epoch, db_epoch_updated_at)` 合成启动信任指纹（方案
/// `docs/2026-08-11_spatial-tree-startup-init-plan.md` §3）：单靠计数在
/// 「库快照回滚恰好回到同一计数」时会撞值，时间戳与计数同一事务写入、同源于
/// 库端时钟，双字段都相等才信文件。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TreeFileMeta {
    epoch: u64,
    /// 库侧该 epoch 的 `updated_at`（字符串化 datetime，与 epoch 同一事务落库）。
    /// 旧版 sidecar 没有此字段：serde 缺省空串，而空串永不等于库侧真实时刻，
    /// 于是老文件自动落入失配分支、一次自愈后补齐。
    #[serde(default)]
    db_epoch_updated_at: String,
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
    // 崩溃窗口 ③（一致性闭环方案 §8）：.tmp 已写完 sync、rename 之前——正式文件
    // 仍是旧版本，重启按旧快照走正常判据，残留 .tmp 被下次发布覆盖。
    crate::fast_model::spatial_state::failpoint("spatial_snapshot_tmp_written");
    std::fs::rename(&tmp, path).map_err(|e| anyhow::anyhow!("覆盖 {path} 失败: {e}"))?;
    Ok(())
}

fn read_project_tree_file() -> anyhow::Result<AccelerationTree> {
    let path = project_tree_file();
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("读取 {path} 失败: {e}"))?;
    bincode::deserialize(&bytes).map_err(|e| anyhow::anyhow!("反序列化 {path} 失败: {e}"))
}

fn read_tree_meta() -> anyhow::Result<TreeFileMeta> {
    let path = project_tree_meta_file();
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("读取 {path} 失败: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("解析 {path} 失败: {e}"))
}

// ── V2 单文件快照（一致性闭环方案 §3）────────────────────────────────────────
//
// 旧「bin + sidecar」两个文件之间没有原子性：崩溃可能留下「新树旧章」或
// 「旧树新章」的组合，指纹校验对后者无能为力。V2 把树载荷、指纹与自校验哈希
// 封进**一个**文件，rename 原子替换，读侧全套校验通过才接受。

/// 快照格式版本。改结构必须递增并写迁移分支——V2 校验失败一律按不可用重建，
/// 不做跨版本猜测。
const SNAPSHOT_FORMAT_VERSION: u32 = 2;

/// 本项目空间树的 V2 快照文件：`accel_tree_{project}.snapshot`。
pub fn project_snapshot_file() -> String {
    format!(
        "accel_tree_{}.snapshot",
        aios_core::get_db_option().project_name
    )
}

/// V2 快照：单文件自足——树载荷（`tree_bytes`，独立 bincode 段，哈希对字节算，
/// 避免「对结构再序列化一次」的双重序列化歧义）+ 指纹 + 口径统计 + 身份。
/// `project`/`namespace` 挡「拷别的项目的快照 / 换库同项目名」（R4）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SpatialTreeSnapshotV2 {
    format_version: u32,
    project: String,
    namespace: String,
    epoch: u64,
    db_epoch_updated_at: String,
    entries: u64,
    usable_pointer_rows: u64,
    invalid_pointer_rows: u64,
    tree_sha256: String,
    saved_at_unix: u64,
    tree_bytes: Vec<u8>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn encode_snapshot_v2(snapshot: &SpatialTreeSnapshotV2) -> anyhow::Result<Vec<u8>> {
    bincode::serialize(snapshot).map_err(|e| anyhow::anyhow!("序列化 V2 快照失败: {e}"))
}

/// 校验并解码 V2 快照：完整反序列化 + 版本 + 项目/namespace + 载荷哈希 + 条目数
/// **全部**通过才接受；任何一环差错都按不可用处理（调用方走指针重建）。
fn decode_snapshot_v2(
    bytes: &[u8],
    project: &str,
    namespace: &str,
) -> anyhow::Result<(SpatialTreeSnapshotV2, AccelerationTree)> {
    let snapshot: SpatialTreeSnapshotV2 =
        bincode::deserialize(bytes).map_err(|e| anyhow::anyhow!("反序列化 V2 快照失败: {e}"))?;
    anyhow::ensure!(
        snapshot.format_version == SNAPSHOT_FORMAT_VERSION,
        "快照格式版本 {} 不是 {SNAPSHOT_FORMAT_VERSION}",
        snapshot.format_version
    );
    anyhow::ensure!(
        snapshot.project == project,
        "快照属于项目 {}，当前项目 {project}",
        snapshot.project
    );
    anyhow::ensure!(
        snapshot.namespace == namespace,
        "快照属于 namespace {}，当前 {namespace}",
        snapshot.namespace
    );
    let digest = sha256_hex(&snapshot.tree_bytes);
    anyhow::ensure!(
        digest == snapshot.tree_sha256,
        "快照载荷哈希失配（文件 {} / 实算 {digest}）",
        snapshot.tree_sha256
    );
    let tree: AccelerationTree = bincode::deserialize(&snapshot.tree_bytes)
        .map_err(|e| anyhow::anyhow!("反序列化快照树载荷失败: {e}"))?;
    anyhow::ensure!(
        tree.size() as u64 == snapshot.entries,
        "快照条目数失配（头 {} / 树 {}）",
        snapshot.entries,
        tree.size()
    );
    Ok((snapshot, tree))
}

/// 最近一次发布/装载的快照头（/health 用）。V2 是 17MB 量级的单文件，health 每次
/// 全量读盘解码不可取；进程内缓存在发布与装载两个点更新。外部换文件在运行中不可
/// 见，由下次启动的全套校验接住。std Mutex，不跨 await 持有。
#[derive(Debug, Clone)]
pub(crate) struct SnapshotHeaderInfo {
    pub format_version: u32,
    pub epoch: u64,
    pub db_epoch_updated_at: String,
    pub entries: u64,
    pub tree_sha256: Option<String>,
    pub saved_at_unix: u64,
}

static SNAPSHOT_HEADER: std::sync::Mutex<Option<SnapshotHeaderInfo>> = std::sync::Mutex::new(None);

fn record_snapshot_header(header: SnapshotHeaderInfo) {
    *SNAPSHOT_HEADER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(header);
}

fn snapshot_header() -> Option<SnapshotHeaderInfo> {
    SNAPSHOT_HEADER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

impl SpatialTreeSnapshotV2 {
    fn header(&self) -> SnapshotHeaderInfo {
        SnapshotHeaderInfo {
            format_version: self.format_version,
            epoch: self.epoch,
            db_epoch_updated_at: self.db_epoch_updated_at.clone(),
            entries: self.entries,
            tree_sha256: Some(self.tree_sha256.clone()),
            saved_at_unix: self.saved_at_unix,
        }
    }
}

impl TreeFileMeta {
    fn header(&self) -> SnapshotHeaderInfo {
        SnapshotHeaderInfo {
            format_version: 1,
            epoch: self.epoch,
            db_epoch_updated_at: self.db_epoch_updated_at.clone(),
            entries: self.entries,
            tree_sha256: None,
            saved_at_unix: self.saved_at_unix,
        }
    }
}

/// V2 发布成功后删除旧格式文件（方案 D3）。
///
/// 旧二进制对 bin 缺失是**无条件指针重建**，任何回退场景自动安全；留着旧文件
/// 反而开一扇静默陈旧窗：回退旧版本时若恰有 pending，旧代码会按 HealByReplay
/// 复用一个冻结在迁移时刻的文件——迁移后已销账的意图不会再重放，完备集论证
/// 已破。删除失败只告警：V2 才是权威，残留旧文件最坏让回退多一分风险，
/// 不影响本版本。
fn remove_legacy_tree_files() {
    for path in [project_tree_file(), project_tree_meta_file()] {
        match std::fs::remove_file(&path) {
            Ok(()) => println!("已移除旧格式空间树文件 {path}（V2 快照已接管）"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("移除旧格式空间树文件 {path} 失败（不影响 V2 权威性）: {error}")
            }
        }
    }
}

/// 启动读快照的裁决：V2 优先；V2 缺失才看旧格式（迁移候选）；V2 在场但校验失败
/// **不**回落旧格式——旧文件自迁移起冻结，回落它就是引入陈旧。
enum SnapshotReadOutcome {
    /// V2 通过全套校验。
    V2(Box<(SpatialTreeSnapshotV2, AccelerationTree)>),
    /// V2 不存在，旧 bin 可读（meta 可缺）——一次性迁移候选。
    Legacy {
        tree: AccelerationTree,
        meta: Option<TreeFileMeta>,
    },
    /// 两代都不可用（缺失/损坏/校验失败），带原因。
    Unusable(String),
}

fn read_snapshot_for_startup(project: &str, namespace: &str) -> SnapshotReadOutcome {
    let v2_path = project_snapshot_file();
    match std::fs::read(&v2_path) {
        Ok(bytes) => match decode_snapshot_v2(&bytes, project, namespace) {
            Ok(decoded) => SnapshotReadOutcome::V2(Box::new(decoded)),
            Err(error) => {
                SnapshotReadOutcome::Unusable(format!("V2 快照 {v2_path} 校验失败: {error:#}"))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match read_project_tree_file() {
                Ok(tree) => SnapshotReadOutcome::Legacy {
                    tree,
                    meta: read_tree_meta().ok(),
                },
                Err(error) => {
                    SnapshotReadOutcome::Unusable(format!("V2 快照缺失且旧格式不可用: {error:#}"))
                }
            }
        }
        Err(error) => {
            SnapshotReadOutcome::Unusable(format!("读取 V2 快照 {v2_path} 失败: {error}"))
        }
    }
}

/// 库侧空间版本号所在的固定记录。
///
/// 不变量：**凡是改变了「树应有内容」的已提交变更，都在同一事务内把它 +1**
/// （[`render_spatial_epoch_bump`]）。暂存路径由携带空间意图（refresh / remove）的
/// 尾事务顺带 bump——水位、意图、版本号同一个事务里同生同死；直写路径（包围盒刷新、
/// 删除清理）没有意图行，epoch 就是它留在库侧的唯一痕迹。启动时 sidecar 的 epoch
/// 与它不相等，就说明「树文件之后还有过没被镜像进文件的空间提交」，文件不可信。
const SPATIAL_EPOCH_ID: &str = "spatial_epoch:current";

/// 渲染空间版本号的递增语句，与产生该变更的写入放进同一个事务。
///
/// 三处调用：窗口尾事务（与 `spatial_reconcile` 意图同事务）、直写包围盒刷新
/// （与指针 UPDATE 同事务）、直写删除清理（与房间边 DELETE 同事务）。
///
/// 重试或分块导致的多次递增无害：版本号只与 sidecar 比相等、不表达次数，多 bump
/// 一次至多让下次启动多做一次指针重建。
pub(crate) fn render_spatial_epoch_bump() -> String {
    format!("UPSERT {SPATIAL_EPOCH_ID} SET value = (value?:0) + 1, updated_at = time::now();")
}

#[derive(serde::Deserialize)]
struct EpochRow {
    #[serde(default)]
    value: u64,
    #[serde(default)]
    updated_at: Option<String>,
}

/// 读库侧当前空间指纹 `(epoch 值, 该 epoch 的 updated_at)`。
///
/// 记录不存在按 `(0, "")`——全新库 / 从未有过空间提交。`updated_at` 铸成字符串
/// 取回，跳过 datetime 在 serde 两侧的形状差异；它与计数由
/// [`render_spatial_epoch_bump`] 同一事务写入，合成的指纹见 [`TreeFileMeta`]。
pub(crate) async fn read_db_spatial_epoch_stamp() -> anyhow::Result<(u64, String)> {
    let mut response = SUL_DB
        // `value` 是 SurrealQL 的保留字（`SELECT VALUE …` 的投影修饰符）：不加反引号
        // 整条语句在 parse 阶段就失败，而启动路径上的 `?` 会把整个进程带下去。
        .query(format!(
            "SELECT `value`, <string> updated_at AS updated_at FROM {SPATIAL_EPOCH_ID};"
        ))
        .await
        .map_err(|e| anyhow::anyhow!("读取空间版本号失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取空间版本号语句失败: {e}"))?;
    Ok(response
        .take::<Vec<EpochRow>>(0)?
        .into_iter()
        .next()
        .map(|row| (row.value, row.updated_at.unwrap_or_default()))
        .unwrap_or((0, String::new())))
}

/// 只要数值的旧口径（`room_build:main` 对账凭据等消费方）。
pub(crate) async fn read_db_spatial_epoch() -> anyhow::Result<u64> {
    Ok(read_db_spatial_epoch_stamp().await?.0)
}

/// 发布当前内存树为 V2 单文件快照。
///
/// 指纹（epoch 值 + 库侧 updated_at）在写文件**之前**读：并发的写入方若在读章与
/// 写盘之间又推高了版本号，快照只会偏旧 → 下次启动宁可多做一次指针重建，方向
/// 安全；反过来先写后读会把新章盖在旧内容上。发布成功后顺手清掉旧格式文件
/// （方案 D3，见 [`remove_legacy_tree_files`]）并更新 /health 用的头缓存。
async fn persist_project_tree_now() -> anyhow::Result<()> {
    let (epoch, db_epoch_updated_at) = read_db_spatial_epoch_stamp().await?;
    let (tree_bytes, entries) = {
        let tree = GLOBAL_AABB_TREE.read().await;
        let bytes =
            bincode::serialize(&*tree).map_err(|e| anyhow::anyhow!("序列化空间树失败: {e}"))?;
        (bytes, tree.size() as u64)
    };
    let stats = crate::fast_model::spatial_state::snapshot();
    let db_option = aios_core::get_db_option();
    let snapshot = SpatialTreeSnapshotV2 {
        format_version: SNAPSHOT_FORMAT_VERSION,
        project: db_option.project_name.clone(),
        namespace: db_option.surreal_ns.clone(),
        epoch,
        db_epoch_updated_at,
        // 树按协议镜像 usable 指针（重建换树、增量同步都在串行锁内成对推进），
        // 发布时刻的 usable 口径就是条目数本身；invalid 是最近一次全量扫描的观测值。
        entries,
        usable_pointer_rows: entries,
        invalid_pointer_rows: stats.invalid_pointer_rows.unwrap_or(0),
        tree_sha256: sha256_hex(&tree_bytes),
        saved_at_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        tree_bytes,
    };
    write_file_atomic(&project_snapshot_file(), &encode_snapshot_v2(&snapshot)?)?;
    record_snapshot_header(snapshot.header());
    remove_legacy_tree_files();
    Ok(())
}

/// 脏则写回项目树文件（worker 空闲轮收尾调用），返回是否真的写了。
pub async fn persist_aabb_tree_if_dirty() -> anyhow::Result<bool> {
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    persist_aabb_tree_if_dirty_locked().await
}

/// 已持空间串行锁的版本（重放收敛等持锁调用方用，锁不可重入）。
///
/// 两道保留脏标记的分支，方向相同——变更不能静默丢在内存里：
/// - 发布门（方案 §3 / D5）：树内容不可信的状态（加载中 / 重建扫描窗口 /
///   重建失败后的 DegradedBlocked 等）不许覆盖快照文件，脏标记保留等状态收敛
///   后补写。这取代了旧的「空树进程靠脏位门控躲过覆盖」的单薄防线。
/// - 落盘失败时**保留**脏标记，下一轮重试——清掉的话一次磁盘抖动就把变更永远留在内存里。
pub(crate) async fn persist_aabb_tree_if_dirty_locked() -> anyhow::Result<bool> {
    if !AABB_TREE_DIRTY.swap(false, Ordering::SeqCst) {
        return Ok(false);
    }
    let state = crate::fast_model::spatial_state::current_state();
    if !state.allows_snapshot_publish() {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        println!(
            "空间树状态 {} 不允许发布快照，保留脏标记等状态收敛",
            state.as_str()
        );
        return Ok(false);
    }
    if let Err(error) = persist_project_tree_now().await {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        return Err(error);
    }
    Ok(true)
}

/// 无条件写回并清脏标记（全量生成收尾走这里；Python `spatial.persist(force)`
/// 在入口另有 Ready 严门）。
///
/// 全量序列化覆盖了此前一切增量变更，所以顺手清标记，免得空闲轮紧接着再白写一遍。
/// 发布门与 [`persist_aabb_tree_if_dirty_locked`] 同一套：不可信状态直接拒绝。
pub async fn persist_aabb_tree() -> anyhow::Result<()> {
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    let state = crate::fast_model::spatial_state::current_state();
    anyhow::ensure!(
        state.allows_snapshot_publish(),
        "空间树状态 {} 不允许无条件发布快照（等重建/复检收敛后重试）",
        state.as_str()
    );
    persist_project_tree_now().await?;
    AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
    Ok(())
}

/// `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值（与 `GEN_MODEL_DIRECT_INCREMENT`
/// 的 P2-1 同款纪律）。旧实现判 `std::env::var(..).is_ok()`：部署模板写 `=0`
/// 想关闭，实际**每次启动都强制全量指针重建**——方向与当年直写开关那只脚枪
/// 相反，根子相同。
fn force_spatial_rebuild_enabled() -> bool {
    force_spatial_rebuild_flag(std::env::var_os("AIOS_FORCE_SPATIAL_REBUILD").as_deref())
}

fn force_spatial_rebuild_flag(value: Option<&std::ffi::OsStr>) -> bool {
    use crate::data_interface::batch_worker::ExplicitFlag;
    match crate::data_interface::batch_worker::parse_explicit_flag(value) {
        ExplicitFlag::On => true,
        ExplicitFlag::Off => false,
        ExplicitFlag::Unrecognized(text) => {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                let message = format!(
                    "AIOS_FORCE_SPATIAL_REBUILD={text:?} 不是可识别的开关值（真值只认 1/true/yes/on），按关闭处理，走常规启动判据"
                );
                log::warn!("{message}");
                eprintln!("{message}");
            });
            false
        }
    }
}

/// 启动装载动作（方案 2026-08-12 一致性闭环 §2；纯判据，IO 全在调用方）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupAction {
    /// 快照可用、指纹一致且无待重放意图：直接复用（快路径）。
    Reuse,
    /// 快照可用且有待重放意图：复用快照并**立即**重放收敛（D2）。
    /// 意图行只在树落盘之后才销账（`reconcile_spatial_pending` 的顺序），
    /// 「可读快照 + 待重放意图」对暂存路径是完备集；指纹相等 + pending 则是
    /// 「发布成功、销账前崩溃」，重放幂等追认。
    Replay,
    /// 快照缺失/损坏（完备集论证不成立，无论有没有 pending），或指纹失配且
    /// 无意图可解释（直写崩溃 / 换文件 / 回滚库）：只读指针重建。
    Rebuild,
}

/// 纯判据。顺序即优先级：快照不可用一律重建（D2——重放需要可读快照才完备）；
/// 快照可用时 pending 优先于指纹；两者都没有才看指纹。
fn startup_action(
    snapshot_usable: bool,
    fingerprint_matches: bool,
    has_pending_spatial_work: bool,
) -> StartupAction {
    if !snapshot_usable {
        StartupAction::Rebuild
    } else if has_pending_spatial_work {
        StartupAction::Replay
    } else if fingerprint_matches {
        StartupAction::Reuse
    } else {
        StartupAction::Rebuild
    }
}

/// 本进程启动加载的最终裁决，/health 的 `spatial_tree.startup_verdict` 曝光用。
static STARTUP_VERDICT: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

fn record_startup_verdict(verdict: &'static str) {
    let _ = STARTUP_VERDICT.set(verdict);
}

/// 启动加载（方案 2026-08-12 一致性闭环 §2，取代 2026-08-11 分层判据）：
///
/// 0. 显式夹具标记 + 内存树非空 → preloaded（仅测试装载模式；生产入口一律校验，
///    旧的「内存树非空即 preloaded」盲信短路已删除）；
/// 1. `AIOS_FORCE_SPATIAL_REBUILD` 为真值 → 指针重建；
/// 2. 树文件缺失/损坏 → 指针重建（无论有没有 pending：完备集论证只对可读文件成立）；
/// 3. 文件可读：
///    3a. 读库指纹失败 → DegradedReuse 降级复用 + 门禁消费者，revalidator 复检；
///    3b. 有待重放意图（读失败按有算）→ ReplayRequired：复用文件并**立即**重放，
///        成功晋升 Ready，失败留态给 worker 出队门重试（D2）；
///    3c. 指纹双字段一致 → Ready 复用（快路径）；
///    3d. 失配且无意图 → 指针重建。
///
/// 整个装载持空间串行锁，与写路径/收敛/落盘串行。
pub async fn load_project_tree_verified() -> anyhow::Result<()> {
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    load_project_tree_verified_locked().await
}

async fn load_project_tree_verified_locked() -> anyhow::Result<()> {
    use crate::fast_model::spatial_state::{self, SpatialTreeState};

    spatial_state::set_state(SpatialTreeState::Loading);
    if spatial_state::fixture_preload_requested() && !GLOBAL_AABB_TREE.read().await.is_empty() {
        record_startup_verdict("preloaded");
        let entries = GLOBAL_AABB_TREE.read().await.size();
        spatial_state::set_ready_by_entries(entries);
        return Ok(());
    }
    if force_spatial_rebuild_enabled() {
        println!("按环境变量要求跳过空间树快照，从库指针重建");
        return rebuild_at_startup_locked().await;
    }

    let db_option = aios_core::get_db_option();
    let (tree, header, migrating_from_legacy) =
        match read_snapshot_for_startup(&db_option.project_name, &db_option.surreal_ns) {
            SnapshotReadOutcome::V2(decoded) => {
                let (snapshot, tree) = *decoded;
                let header = snapshot.header();
                (tree, Some(header), false)
            }
            SnapshotReadOutcome::Legacy { tree, meta } => {
                let header = meta.as_ref().map(TreeFileMeta::header);
                (tree, header, true)
            }
            SnapshotReadOutcome::Unusable(reason) => {
                eprintln!("空间树快照不可用（{reason}），从库指针自动重建");
                return rebuild_at_startup_locked().await;
            }
        };
    let entries = tree.size();

    let (db_epoch, db_epoch_updated_at) = match read_db_spatial_epoch_stamp().await {
        Ok(stamp) => stamp,
        Err(error) => {
            *GLOBAL_AABB_TREE.write().await = tree;
            if let Some(header) = header {
                record_snapshot_header(header);
            }
            record_startup_verdict("degraded");
            spatial_state::set_state(SpatialTreeState::DegradedReuse);
            spatial_state::record_error(&format!("读取库侧空间指纹失败: {error:#}"));
            eprintln!(
                "读取库侧空间指纹失败（{error:#}），降级复用快照（{entries} 条）；\
                 空间消费者被门禁，revalidator 后台复检"
            );
            return Ok(());
        }
    };
    let has_pending = match crate::data_interface::side_effect_pending::SideEffectCompensator::
        has_pending_spatial_work()
    .await
    {
        Ok(pending) => pending,
        Err(error) => {
            // 判不了就按「可能有意图」处理：复用快照 + 立即重放的方向，比把一次
            // 诊断抖动放大成全量重建更稳（与 D2 同向）。
            eprintln!("读取待重放空间意图失败（{error:#}），按存在意图的方向处理");
            true
        }
    };
    let fingerprint_matches = header.as_ref().is_some_and(|header| {
        header.epoch == db_epoch && header.db_epoch_updated_at == db_epoch_updated_at
    });

    // 旧格式一次性迁移（方案 §3 / D3）：只有「双指纹匹配且无 pending」才封装发布
    // V2 并删除旧文件；其余情况一律重建——迁移矩阵保持最小，不跨格式版本信任
    // 重放完备性。
    if migrating_from_legacy {
        if fingerprint_matches && !has_pending {
            *GLOBAL_AABB_TREE.write().await = tree;
            record_startup_verdict("migrated");
            spatial_state::set_ready_by_entries(entries);
            match persist_project_tree_now().await {
                Ok(()) => println!(
                    "旧格式空间树文件校验通过，已迁移为 V2 快照 {}（{entries} 条）",
                    project_snapshot_file()
                ),
                Err(error) => {
                    // 树内容已由指纹校验背书，照常 Ready；发布留给空闲轮补
                    // （脏标记），旧文件保留到 V2 发布成功那一刻才删。
                    mark_aabb_tree_dirty();
                    spatial_state::record_error(&format!("V2 迁移发布失败: {error:#}"));
                    eprintln!("V2 迁移发布失败（{error:#}）：先按旧文件内容运行，空闲轮重试发布");
                }
            }
            return Ok(());
        }
        println!(
            "旧格式空间树文件不满足迁移条件（指纹匹配 {fingerprint_matches} / 待重放意图 {has_pending}），\
             从库指针重建"
        );
        return rebuild_at_startup_locked().await;
    }

    match startup_action(true, fingerprint_matches, has_pending) {
        StartupAction::Reuse => {
            *GLOBAL_AABB_TREE.write().await = tree;
            if let Some(header) = header {
                record_snapshot_header(header);
            }
            record_startup_verdict("reused");
            spatial_state::set_ready_by_entries(entries);
            println!(
                "空间树复用 V2 快照 {}（{entries} 条，指纹 epoch {db_epoch} @ {db_epoch_updated_at} 与库一致）",
                project_snapshot_file()
            );
        }
        StartupAction::Replay => {
            *GLOBAL_AABB_TREE.write().await = tree;
            if let Some(header) = header.clone() {
                record_snapshot_header(header);
            }
            record_startup_verdict("replayed");
            spatial_state::set_state(SpatialTreeState::ReplayRequired);
            println!(
                "存在已提交待重放的空间意图（快照 epoch {}，库 epoch {db_epoch} @ {db_epoch_updated_at}）：\
                 复用快照（{entries} 条），立即重放收敛",
                header
                    .as_ref()
                    .map(|header| header.epoch.to_string())
                    .unwrap_or_else(|| "缺失".to_string())
            );
            // D2：启动立即重放，不等 worker 派发门——queue_paused / autorun 关闭的
            // 部署下派发门可能永远不来，Ready 也就永远不来。失败保持 ReplayRequired
            // （空间消费者持续被门禁），派发门与空闲轮带着重试，启动本身不因此失败。
            match crate::data_interface::side_effect_pending::SideEffectCompensator::
                reconcile_spatial_pending_locked()
            .await
            {
                Ok(done) => println!("启动空间意图重放完成（{done} 个任务）"),
                Err(error) => {
                    spatial_state::record_error(&format!("启动空间意图重放失败: {error:#}"));
                    eprintln!(
                        "启动空间意图重放失败（{error:#}）：空间消费者保持门禁，worker 出队门继续重试"
                    );
                }
            }
        }
        StartupAction::Rebuild => {
            println!(
                "空间树快照指纹与库不一致且无待重放意图（快照 {}，库 epoch {db_epoch} @ {db_epoch_updated_at}）：\
                 无法解释的漂移（直写崩溃 / 换文件 / 回滚库），从库指针重建",
                header
                    .as_ref()
                    .map(|header| format!(
                        "epoch {} @ {}",
                        header.epoch, header.db_epoch_updated_at
                    ))
                    .unwrap_or_else(|| "头缺失".to_string())
            );
            return rebuild_at_startup_locked().await;
        }
    }
    Ok(())
}

/// 启动路径的指针重建外壳：成功才记 `rebuilt`，失败记 `degraded` 并原样上抛
/// （调用点告警降级空树继续启动；状态已由重建路径置 DegradedBlocked，
/// revalidator 后台退避重试）。
async fn rebuild_at_startup_locked() -> anyhow::Result<()> {
    match rebuild_tree_from_pointers_locked().await {
        Ok(()) => {
            record_startup_verdict("rebuilt");
            Ok(())
        }
        Err(error) => {
            record_startup_verdict("degraded");
            Err(error)
        }
    }
}

/// /health 的 `spatial_tree` 字段：状态机、快照头/库指纹、口径统计与启动裁决。
///
/// 快照侧取进程内头缓存（V2 是 17MB 单文件，health 每次全量读盘解码不可取；
/// 缓存在发布与装载两处更新，外部换文件由下次启动的全套校验接住）；库侧指纹
/// 现读现比，运行中出现的漂移看得见。任何一侧读不出来都如实报 null、`drift`
/// 置 true 并把原因并入 `last_error`——健康端点不许因诊断失败而挂。
pub async fn spatial_tree_status() -> serde_json::Value {
    let entries = GLOBAL_AABB_TREE.read().await.size();
    let header = snapshot_header();
    let db = read_db_spatial_epoch_stamp().await;
    let pending = crate::data_interface::side_effect_pending::SideEffectCompensator::
        count_pending_spatial_work()
    .await;
    render_spatial_tree_status(
        entries,
        header.as_ref(),
        &db,
        &pending,
        STARTUP_VERDICT.get().copied().unwrap_or("unknown"),
        &crate::fast_model::spatial_state::snapshot(),
    )
}

/// /health `spatial_tree` 的纯渲染半边。
///
/// 十五个键是对外承诺（台账 G-02 契约迁移：取代旧九键形状，2026-08-12 方案 §7）：
/// 指纹**双字段都相等**才算无漂移（防「快照回滚撞回同一计数」）；`pending` 是
/// `spatial_reconcile.pending` 的同源镜像（权威仍在那边）；任何一侧读不出来都
/// 如实报 null、`drift` 置 true、原因并入 `last_error`——降级分支不许缩键。
/// `format_version` / `snapshot_sha256`：旧格式迁移前报 1 / null，V2 报 2 / 哈希。
fn render_spatial_tree_status(
    entries: usize,
    header: Option<&SnapshotHeaderInfo>,
    db: &anyhow::Result<(u64, String)>,
    pending: &anyhow::Result<usize>,
    startup_verdict: &'static str,
    state: &crate::fast_model::spatial_state::SpatialStateSnapshot,
) -> serde_json::Value {
    let drift = !matches!(
        (header, db),
        (Some(header), Ok((db_epoch, db_updated_at)))
            if header.epoch == *db_epoch && header.db_epoch_updated_at == *db_updated_at
    );
    let last_error = state
        .last_error
        .clone()
        .or_else(|| db.as_ref().err().map(|error| format!("{error:#}")))
        .or_else(|| pending.as_ref().err().map(|error| format!("{error:#}")));
    serde_json::json!({
        "state": state.state.as_str(),
        "ready": state.state.is_ready(),
        "startup_verdict": startup_verdict,
        "format_version": header.map(|header| header.format_version),
        "entries": entries,
        "usable_pointer_rows": state.usable_pointer_rows,
        "invalid_pointer_rows": state.invalid_pointer_rows,
        "pending": pending.as_ref().ok().copied(),
        "file_epoch": header.map(|header| header.epoch),
        "db_epoch": db.as_ref().ok().map(|(epoch, _)| *epoch),
        "drift": drift,
        "snapshot_sha256": header.and_then(|header| header.tree_sha256.clone()),
        "last_verified_at": state.last_verified_at_unix,
        "last_rebuild_attempts": state.last_rebuild_attempts,
        "last_error": last_error,
    })
}

/// 从库指针整树重建：record-range 分页读 `inst_relate` 指针，bulk-load 进全局树
/// 后立即落盘盖章。
///
/// 只读不写库。进树口径（一致性闭环方案 D1，current-only）：
/// - 排除版本化历史行（数组 id，`fn::backup_data` 遗产——字段整行拷贝、能命中
///   指针谓词；树口径是「每 refno 一条 current 行」，混进来会在首次增量刷新时被
///   `sync_refnos` 折叠，条目数自此对不上 usable 口径）；
/// - 排除软删元素（`in.deleted == true`，DeleteCleanup 清走行之前就退出口径）；
/// - `world_trans.d != none AND aabb.d != none`（与刷新层一致）；
/// - Rust 侧排除 NaN/Inf/反向 AABB（计数 + ≤10 样本，/health 曝光）。
///
/// 执行协议（D4）：分页读在**锁外**（不阻塞 staged 提交尾与派发门），换树 + 终局
/// stamp 比对 + 发布快照在空间串行锁内；前后 stamp 不同说明锁外读期间有并发空间
/// 提交，丢弃整轮重扫，连续 [`REBUILD_MAX_ATTEMPTS`] 次漂移或查询失败进入
/// `DegradedBlocked`（revalidator 退避重试）。锁外读期间 staged journal 写回的
/// 半窗口指针可能被扫进来且不 bump——其空间意图必在尾事务留 pending，随后的重放
/// 按已提交指针把这些 refno 追平（D5「已知 pending 由消费前重放收口」）。
///
/// 没赶上刷新的行（指针缺失）不进树——它们此前也从不在树上；真要把这类行补进
/// 来（重算几何并回写指针），用显式修复工具 [`manual_update_aabbs`]。
pub async fn rebuild_tree_from_pointers() -> anyhow::Result<()> {
    rebuild_tree_from_pointers_driver(false).await
}

/// 空间串行锁已由调用方持有的变体（启动装载用）。此时进程尚无并发写者，分页读
/// 顺带在锁内完成，stamp 协议照走（防启动期外部进程写库）。
pub(crate) async fn rebuild_tree_from_pointers_locked() -> anyhow::Result<()> {
    rebuild_tree_from_pointers_driver(true).await
}

const REBUILD_MAX_ATTEMPTS: u32 = 3;

async fn rebuild_tree_from_pointers_driver(serial_already_held: bool) -> anyhow::Result<()> {
    use crate::fast_model::spatial_state::{self, SpatialTreeState};

    spatial_state::set_state(SpatialTreeState::Rebuilding);
    let outcome = async {
        for attempt in 1..=REBUILD_MAX_ATTEMPTS {
            spatial_state::record_rebuild_attempts(attempt);
            let stamp_before = read_db_spatial_epoch_stamp().await?;
            let scan = scan_usable_pointers().await?;
            let _serial = if serial_already_held {
                None
            } else {
                Some(crate::fast_model::spatial_state::lock_spatial_serial().await)
            };
            let stamp_after = read_db_spatial_epoch_stamp().await?;
            if stamp_after != stamp_before {
                println!(
                    "指针重建第 {attempt}/{REBUILD_MAX_ATTEMPTS} 轮作废：扫描期间库侧空间指纹 \
                     {stamp_before:?} → {stamp_after:?}，重扫"
                );
                continue;
            }
            let entries = scan.boxes.len();
            let usable_rows = scan.usable_rows;
            let invalid_rows = scan.invalid_rows;
            *GLOBAL_AABB_TREE.write().await = AccelerationTree::load(scan.boxes);
            let tree_entries = GLOBAL_AABB_TREE.read().await.size();
            anyhow::ensure!(
                tree_entries as u64 == usable_rows,
                "重建自检失败：树条目 {tree_entries} ≠ usable 指针行 {usable_rows}"
            );
            spatial_state::record_scan_stats(usable_rows, invalid_rows);
            // 重建产物立即落盘盖章（直接走内部发布：状态此刻是 Rebuilding，公开
            // 入口的发布门不认它），不落的话下次启动还得再重建一遍。
            persist_project_tree_now().await?;
            AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
            println!("空间树已从库指针重建并落盘: {entries} 条（排除无效指针 {invalid_rows} 条）");
            return Ok(entries);
        }
        anyhow::bail!(
            "指针重建连续 {REBUILD_MAX_ATTEMPTS} 轮撞上并发空间提交（stamp 漂移），放弃本轮"
        )
    }
    .await;
    match outcome {
        Ok(entries) => {
            spatial_state::set_ready_by_entries(entries);
            Ok(())
        }
        Err(error) => {
            spatial_state::set_state(SpatialTreeState::DegradedBlocked);
            spatial_state::record_error(&format!("指针重建失败: {error:#}"));
            Err(error)
        }
    }
}

/// 一次全量指针扫描的产物。`usable_rows == boxes.len()` 恒成立（按行进树），
/// 单列出来是给锁内自检与 /health 的口径统计用。
struct PointerScan {
    boxes: Vec<RStarBoundingBox>,
    usable_rows: u64,
    invalid_rows: u64,
}

/// 渲染一页指针扫描（record-range 分页；两引擎语义由 fork 兼容套件双跑钉住，D8）。
///
/// `inst_relate:⟨cursor⟩..` 是从 cursor（**含**）到表尾的记录区间——表扫描天然按
/// id 序，区间起点免掉 `LIMIT/START` 每页从头数的 O(n²) 与页间写入的漏/重；
/// cursor 行本身由调用方剔重。谓词口径见 [`rebuild_tree_from_pointers`]。
pub(crate) fn render_pointer_scan_page(cursor: Option<&str>, page: usize) -> String {
    let source = match cursor {
        Some(cursor) => format!("inst_relate:⟨{cursor}⟩.."),
        None => "inst_relate".to_string(),
    };
    format!(
        "SELECT record::id(id) AS row_id, in AS refno, in.noun AS noun, aabb.d AS aabb \
         FROM {source} WHERE !type::is::array(record::id(id)) AND in.deleted != true \
         AND world_trans.d != none AND aabb.d != none LIMIT {page};"
    )
}

/// 扫描行：`row_id` 只用作分页游标（谓词已排除数组 id，恒为字符串）。
#[derive(serde::Deserialize)]
struct ScanRow {
    row_id: String,
    refno: RefnoEnum,
    noun: Option<String>,
    aabb: parry3d::bounding_volume::Aabb,
}

/// 指针值必须是有限、非反向的盒子才配进树：NaN/Inf 让 r 星树的包络比较全线失真，
/// 反向范围（mins > maxs）是空集语义、命中不了任何查询——都按无效计数并采样。
fn aabb_is_usable(aabb: &parry3d::bounding_volume::Aabb) -> bool {
    let finite = aabb
        .mins
        .coords
        .iter()
        .chain(aabb.maxs.coords.iter())
        .all(|v| v.is_finite());
    finite && aabb.mins.x <= aabb.maxs.x && aabb.mins.y <= aabb.maxs.y && aabb.mins.z <= aabb.maxs.z
}

const SCAN_PAGE: usize = 5000;

async fn scan_usable_pointers() -> anyhow::Result<PointerScan> {
    let mut boxes: Vec<RStarBoundingBox> = Vec::new();
    let mut invalid_rows = 0u64;
    let mut invalid_samples: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sql = render_pointer_scan_page(cursor.as_deref(), SCAN_PAGE);
        let mut response = SUL_DB
            .query(&sql)
            .await
            .map_err(|e| anyhow::anyhow!("分页读取包围盒指针失败（cursor {cursor:?}）: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("分页读取包围盒指针语句失败（cursor {cursor:?}）: {e}"))?;
        let rows: Vec<ScanRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析包围盒指针失败（cursor {cursor:?}）: {e}"))?;
        let fetched = rows.len();
        let mut last_id: Option<String> = None;
        for row in rows {
            let is_cursor_repeat = cursor.as_deref() == Some(row.row_id.as_str());
            last_id = Some(row.row_id.clone());
            if is_cursor_repeat {
                // 区间含起点：上一页的最后一行会在本页重现一次，剔掉。
                continue;
            }
            if aabb_is_usable(&row.aabb) {
                boxes.push(RStarBoundingBox::new(
                    row.aabb,
                    row.refno,
                    row.noun.unwrap_or_else(|| "UNSET".to_string()),
                ));
            } else {
                invalid_rows += 1;
                if invalid_samples.len() < 10 {
                    invalid_samples.push(row.row_id.clone());
                }
            }
        }
        if fetched < SCAN_PAGE {
            break;
        }
        cursor = last_id;
        // 崩溃窗口 ⑤（方案 §8）：重建分页中途进程死亡——旧快照原样在场，重启走
        // 正常判据；配合并发 epoch 注入可测漂移重试。
        crate::fast_model::spatial_state::failpoint("spatial_rebuild_mid_scan");
    }
    if invalid_rows > 0 {
        println!(
            "指针扫描排除 {invalid_rows} 条无效 AABB（NaN/Inf/反向范围），样本: {}",
            invalid_samples.join(", ")
        );
    }
    let usable_rows = boxes.len() as u64;
    Ok(PointerScan {
        boxes,
        usable_rows,
        invalid_rows,
    })
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

    /// 落盘失败必须保留脏标记（清掉等于把变更永远留在内存里），成功路径才允许清；
    /// 发布门拒绝时同样保留脏标记（变更等状态收敛后补写）。
    /// `persist_*` 会写真实的项目树文件，单测不能实跑，只能钉源码。
    #[test]
    fn persist_failure_keeps_the_dirty_flag() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!(
                "pub(crate) async fn ",
                "persist_aabb_tree_if_dirty_locked("
            ))
            .expect("persist_aabb_tree_if_dirty_locked must exist")
            .1
            .split_once(concat!("pub async fn ", "persist_aabb_tree("))
            .expect("unconditional persist must follow")
            .0;
        // 发布门分支：状态不可信 → 恢复脏标记且不落盘。
        let gate_at = body
            .find("allows_snapshot_publish()")
            .expect("落盘前必须过发布门");
        let gate_restore_at = body
            .find("AABB_TREE_DIRTY.store(true")
            .expect("发布门拒绝必须恢复脏标记");
        assert!(
            gate_at < gate_restore_at,
            "发布门在前，恢复脏标记在后: {body}"
        );
        // 失败分支：恢复脏标记必须在错误返回之前。
        let restore_at = body
            .rfind("AABB_TREE_DIRTY.store(true")
            .expect("failure branch must restore the dirty flag");
        let err_at = body
            .find("return Err")
            .expect("failure branch must propagate");
        assert!(
            restore_at < err_at,
            "脏标记必须在错误返回之前恢复，否则一次磁盘抖动就丢掉待落盘变更"
        );
        // 公开入口必须持空间串行锁再进 _locked。
        let public = source
            .split_once(concat!("pub async fn ", "persist_aabb_tree_if_dirty("))
            .expect("public persist must exist")
            .1
            .split_once(concat!(
                "pub(crate) async fn ",
                "persist_aabb_tree_if_dirty_locked("
            ))
            .expect("locked variant must follow")
            .0;
        assert!(
            public.contains("lock_spatial_serial().await"),
            "公开落盘入口必须先取空间串行锁: {public}"
        );
    }

    #[test]
    fn marking_dirty_is_observable() {
        AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
        mark_aabb_tree_dirty();
        assert!(AABB_TREE_DIRTY.swap(false, Ordering::SeqCst));
    }

    /// /health `spatial_tree` 的十五键契约（台账 G-02 契约迁移，取代旧九键形状）。
    ///
    /// 指纹双字段（epoch + 库侧 bump 时刻）都相等才算无漂移——计数撞值的
    /// 快照回滚必须按漂移报；诊断失败的降级分支键一个不少、file_* 如实为
    /// null、`last_error` 说得出原因；`pending` 是 `spatial_reconcile.pending`
    /// 的同源镜像。
    #[test]
    fn spatial_tree_status_keeps_its_fifteen_key_shape_in_both_branches() {
        use crate::fast_model::spatial_state::{SpatialStateSnapshot, SpatialTreeState};

        let header = SnapshotHeaderInfo {
            format_version: 2,
            epoch: 7,
            db_epoch_updated_at: "d'2026-08-12T00:00:00Z'".into(),
            entries: 42,
            tree_sha256: Some("abc123".into()),
            saved_at_unix: 1_755_000_000,
        };
        let db = Ok((7u64, "d'2026-08-12T00:00:00Z'".to_string()));
        let pending = Ok(0usize);
        let state = SpatialStateSnapshot {
            state: SpatialTreeState::Ready,
            last_error: None,
            last_rebuild_attempts: 0,
            last_verified_at_unix: Some(1_755_000_000),
            usable_pointer_rows: Some(42),
            invalid_pointer_rows: Some(0),
        };

        let keys = [
            "state",
            "ready",
            "startup_verdict",
            "format_version",
            "entries",
            "usable_pointer_rows",
            "invalid_pointer_rows",
            "pending",
            "file_epoch",
            "db_epoch",
            "drift",
            "snapshot_sha256",
            "last_verified_at",
            "last_rebuild_attempts",
            "last_error",
        ];
        let healthy =
            render_spatial_tree_status(42, Some(&header), &db, &pending, "reused", &state);
        let object = healthy.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {healthy}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {healthy}");
        }
        assert_eq!(healthy["state"], "ready");
        assert_eq!(healthy["ready"], true);
        assert_eq!(healthy["drift"], false);
        assert_eq!(healthy["last_error"], serde_json::Value::Null);
        assert_eq!(healthy["startup_verdict"], "reused");
        assert_eq!(healthy["format_version"], 2);
        assert_eq!(healthy["snapshot_sha256"], "abc123");
        assert_eq!(healthy["file_epoch"], 7);
        assert_eq!(healthy["db_epoch"], 7);
        assert_eq!(healthy["pending"], 0);
        assert_eq!(healthy["usable_pointer_rows"], 42);

        // 计数相等而库侧时刻不同 = 快照回滚撞回同一计数，必须按漂移报。
        let rolled_back = Ok((7u64, "d'2026-08-11T00:00:00Z'".to_string()));
        let drifted =
            render_spatial_tree_status(42, Some(&header), &rolled_back, &pending, "reused", &state);
        assert_eq!(drifted["drift"], true, "指纹双字段必须都相等: {drifted}");

        // 旧格式迁移前的头（format_version 1，无哈希）：sha 如实 null。
        let legacy_header = SnapshotHeaderInfo {
            format_version: 1,
            tree_sha256: None,
            ..header.clone()
        };
        let legacy =
            render_spatial_tree_status(42, Some(&legacy_header), &db, &pending, "reused", &state);
        assert_eq!(legacy["format_version"], 1);
        assert_eq!(legacy["snapshot_sha256"], serde_json::Value::Null);

        // 诊断失败/头缺失：键一个不少，快照侧如实为 null，last_error 报得出原因，
        // ready 只由状态机决定。
        let degraded_state = SpatialStateSnapshot {
            state: SpatialTreeState::DegradedBlocked,
            last_error: None,
            last_rebuild_attempts: 3,
            last_verified_at_unix: None,
            usable_pointer_rows: None,
            invalid_pointer_rows: None,
        };
        let broken = render_spatial_tree_status(
            0,
            None,
            &db,
            &Err(anyhow::anyhow!("读取 pending 失败")),
            "unknown",
            &degraded_state,
        );
        let broken_object = broken.as_object().expect("降级形状必须是对象");
        assert_eq!(
            broken_object.len(),
            keys.len(),
            "降级分支不许缩键: {broken}"
        );
        assert_eq!(broken["state"], "degraded_blocked");
        assert_eq!(broken["ready"], false);
        assert_eq!(broken["format_version"], serde_json::Value::Null);
        assert_eq!(broken["file_epoch"], serde_json::Value::Null);
        assert_eq!(broken["snapshot_sha256"], serde_json::Value::Null);
        assert_eq!(broken["pending"], serde_json::Value::Null);
        assert_eq!(broken["last_rebuild_attempts"], 3);
        assert_eq!(broken["drift"], true, "读不出指纹必须按漂移报: {broken}");
        assert!(
            broken["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("读取 pending 失败")
        );

        // 状态簿记里的错误优先曝光（重建失败原因比诊断读失败更接近根因）。
        let with_state_error = SpatialStateSnapshot {
            last_error: Some("指针重建失败: boom".into()),
            ..degraded_state.clone()
        };
        let prioritized = render_spatial_tree_status(
            0,
            None,
            &db,
            &Err(anyhow::anyhow!("读取 pending 失败")),
            "degraded",
            &with_state_error,
        );
        assert!(
            prioritized["last_error"]
                .as_str()
                .expect("必须报错")
                .contains("指针重建失败"),
            "状态簿记的错误必须优先: {prioritized}"
        );
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

    /// 启动装载（方案 2026-08-12 一致性闭环 §2）：整个装载持空间串行锁；快路径必须
    /// 比双字段指纹；pending 优先且**立即**重放（D2）；文件缺失/损坏必须自动重建；
    /// 强制重建只认真值解析；生产入口不得再有「内存树非空即 preloaded」的盲信短路；
    /// 默认路径仍不得触发条数对账或几何重算重写。
    #[test]
    fn startup_layers_fingerprint_replay_then_rebuild() {
        let source = include_str!("aabb_tree.rs");
        let wrapper = source
            .split_once(concat!("pub async fn ", "load_project_tree_verified("))
            .expect("load_project_tree_verified must exist")
            .1
            .split_once(concat!("async fn ", "load_project_tree_verified_locked("))
            .expect("locked variant must follow")
            .0;
        assert!(
            wrapper.contains("lock_spatial_serial().await"),
            "启动装载必须持空间串行锁: {wrapper}"
        );
        let body = source
            .split_once(concat!("async fn ", "load_project_tree_verified_locked("))
            .expect("locked loader must exist")
            .1
            .split_once(concat!("async fn ", "rebuild_at_startup_locked("))
            .expect("startup rebuild shell must follow")
            .0;
        assert!(
            body.contains("fixture_preload_requested()"),
            "preloaded 短路必须由显式夹具标记门控，不得盲信「树非空」: {body}"
        );
        assert!(
            body.contains("read_snapshot_for_startup("),
            "默认启动必须先尝试快照（V2 优先、旧格式仅作迁移候选）: {body}"
        );
        assert!(
            body.contains("read_db_spatial_epoch_stamp()"),
            "启动必须读库侧指纹（epoch 值 + updated_at）: {body}"
        );
        assert!(
            body.contains("has_pending_spatial_work()"),
            "必须问待重放意图，pending 优先于指纹: {body}"
        );
        assert!(
            body.contains("force_spatial_rebuild_enabled()") && !body.contains(".is_ok()"),
            "强制重建必须走真值解析，不得回到 is_ok 判定: {body}"
        );
        let pending_at = body
            .find("has_pending_spatial_work()")
            .expect("checked above");
        let verdict_at = body
            .find("match startup_action(")
            .expect("裁决必须由纯判据函数给出");
        assert!(
            pending_at < verdict_at,
            "意图查询必须发生在裁决之前: {body}"
        );
        // D2：Replay 分支必须立即重放，不等 worker 派发门。
        assert!(
            body.contains("reconcile_spatial_pending_locked()"),
            "ReplayRequired 必须立即重放收敛: {body}"
        );
        assert!(
            !body.contains("sync_aabb_tree_with_db") && !body.contains("manual_update_aabbs"),
            "默认启动不得触发条数对账或几何重算重写: {body}"
        );

        // 快照缺失/损坏分支必须走自动重建（决策 D1/D2），不再空树等人工，
        // 且无论有没有 pending（完备集论证只对可读快照成立）。
        let unusable_branch = body
            .split_once("SnapshotReadOutcome::Unusable(")
            .expect("必须处理快照不可用分支")
            .1;
        assert!(
            unusable_branch.contains("return rebuild_at_startup_locked().await"),
            "快照不可用必须自动指针重建: {body}"
        );

        // 旧格式一次性迁移（D3）：仅「指纹匹配 + 无 pending」封装发布 V2，
        // 其余一律重建；发布动作必须在迁移分支内。
        let migration_branch = body
            .split_once("if migrating_from_legacy {")
            .expect("必须有旧格式迁移分支")
            .1
            .split_once("match startup_action(")
            .expect("迁移分支在常规裁决之前")
            .0;
        assert!(
            migration_branch.contains("fingerprint_matches && !has_pending"),
            "迁移条件必须是双指纹匹配且无 pending: {migration_branch}"
        );
        assert!(
            migration_branch.contains("persist_project_tree_now().await"),
            "迁移必须当场发布 V2: {migration_branch}"
        );
        assert!(
            migration_branch.contains("record_startup_verdict(\"migrated\")"),
            "迁移裁决必须记 migrated: {migration_branch}"
        );
        assert!(
            migration_branch.contains("return rebuild_at_startup_locked().await"),
            "不满足迁移条件必须重建: {migration_branch}"
        );
    }

    /// 启动动作真值表（纯函数，方案 §2，含 D2 交叉项）。
    #[test]
    fn startup_action_truth_table() {
        // 快照可用、指纹一致、无 pending → 复用（快路径）。
        assert_eq!(startup_action(true, true, false), StartupAction::Reuse);
        // D2 交叉项 1：指纹一致 + pending（发布成功、销账前崩溃）→ 重放优先。
        assert_eq!(startup_action(true, true, true), StartupAction::Replay);
        // 失配 + pending → 复用快照立即重放（旧 HealByReplay 的完备集场景）。
        assert_eq!(startup_action(true, false, true), StartupAction::Replay);
        // 失配 + 无 pending → 无法解释的漂移，重建。
        assert_eq!(startup_action(true, false, false), StartupAction::Rebuild);
        // D2 交叉项 2：快照不可用时 pending 不改变裁决——完备集论证只对可读
        // 快照成立，在空树上重放增量是缺陷，必须整树重建。
        assert_eq!(startup_action(false, false, true), StartupAction::Rebuild);
        assert_eq!(startup_action(false, false, false), StartupAction::Rebuild);
        assert_eq!(startup_action(false, true, true), StartupAction::Rebuild);
    }

    /// 扫描 SQL 的口径钉（D1 current-only + D8 record-range）。两引擎的执行语义
    /// 由 fork 兼容套件 `dual_pointer_scan_pagination_agrees` 双跑钉住。
    #[test]
    fn pointer_scan_page_sql_pins_the_current_only_scope() {
        let first = render_pointer_scan_page(None, 5000);
        assert!(
            first.contains("FROM inst_relate WHERE"),
            "首页从表头扫: {first}"
        );
        assert!(
            first.contains("!type::is::array(record::id(id))"),
            "必须排除版本化数组 id 行: {first}"
        );
        assert!(
            first.contains("in.deleted != true"),
            "必须排除软删元素: {first}"
        );
        assert!(
            first.contains("world_trans.d != none AND aabb.d != none"),
            "进树前提与刷新层一致: {first}"
        );
        assert!(first.contains("LIMIT 5000"), "{first}");
        assert!(
            !first.to_lowercase().contains(" start "),
            "不得回到 LIMIT/START 分页（每页从头数 = O(n²) + 页间写入漏/重）: {first}"
        );

        let paged = render_pointer_scan_page(Some("8000_123"), 100);
        assert!(
            paged.contains("FROM inst_relate:⟨8000_123⟩.."),
            "游标页必须走 record-range 区间: {paged}"
        );
        assert!(paged.contains("LIMIT 100"), "{paged}");
    }

    /// 无效 AABB 的判定口径：有限、非反向才配进树（NaN/Inf/反向范围计数排除）。
    #[test]
    fn aabb_usability_rejects_nan_inf_and_reversed_ranges() {
        let boxed = |mins: [f32; 3], maxs: [f32; 3]| {
            parry3d::bounding_volume::Aabb::new(mins.into(), maxs.into())
        };
        assert!(aabb_is_usable(&boxed([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])));
        assert!(
            aabb_is_usable(&boxed([1.0, 1.0, 1.0], [1.0, 1.0, 1.0])),
            "零体积盒（点）合法"
        );
        assert!(!aabb_is_usable(&boxed(
            [f32::NAN, 0.0, 0.0],
            [1.0, 1.0, 1.0]
        )));
        assert!(!aabb_is_usable(&boxed(
            [0.0, 0.0, 0.0],
            [f32::INFINITY, 1.0, 1.0]
        )));
        assert!(
            !aabb_is_usable(&boxed([2.0, 0.0, 0.0], [1.0, 1.0, 1.0])),
            "反向范围是空集语义"
        );
    }

    /// 重建执行协议（D4）：stamp_before 在扫描之前读、分页读在锁外、stamp 复核与
    /// 换树/发布在串行锁内按序、漂移重试有界、耗尽落 DegradedBlocked。回退即红。
    #[test]
    fn rebuild_protocol_reads_outside_and_swaps_inside_the_serial_lock() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("async fn ", "rebuild_tree_from_pointers_driver("))
            .expect("rebuild driver must exist")
            .1
            .split_once(concat!("\n", "/// 一次全量指针扫描的产物"))
            .expect("scan struct doc follows")
            .0;
        let before_at = body.find("let stamp_before").expect("扫描前必须读 stamp");
        let scan_at = body.find("scan_usable_pointers()").expect("必须走统一扫描");
        let lock_at = body
            .find("lock_spatial_serial().await")
            .expect("换树段必须持串行锁");
        let after_at = body.find("let stamp_after").expect("锁内必须复读 stamp");
        let swap_at = body
            .find("AccelerationTree::load")
            .expect("必须 bulk-load 换树");
        let publish_at = body
            .find("persist_project_tree_now()")
            .expect("必须发布快照");
        assert!(
            before_at < scan_at && scan_at < lock_at,
            "stamp_before 在扫描前、分页读在锁外: {body}"
        );
        assert!(
            lock_at < after_at && after_at < swap_at && swap_at < publish_at,
            "stamp 复核与换树/发布必须在锁内按序: {body}"
        );
        assert!(
            body.contains("REBUILD_MAX_ATTEMPTS"),
            "漂移重试必须有界: {body}"
        );
        assert!(
            body.contains("DegradedBlocked"),
            "耗尽必须落 DegradedBlocked: {body}"
        );
        // 发布门与重建的分工：公共落盘入口不认 Rebuilding（防「重建窗口内抢发新
        // 指纹旧内容、漂移耗尽后被 Reuse 洗白」），重建自己的发布只走内部函数。
        assert!(
            !body.contains("persist_aabb_tree"),
            "重建不得经公共落盘入口发布（发布门不认 Rebuilding）: {body}"
        );
    }

    /// 指纹判定保持双字段口径（方案 2026-08-11 遗留不变量，由装载器内联计算）。
    #[test]
    fn fingerprint_match_requires_both_fields() {
        let meta = |epoch: u64, at: &str| TreeFileMeta {
            epoch,
            db_epoch_updated_at: at.to_string(),
            entries: 1,
            saved_at_unix: 0,
        };
        let matches = |meta: &TreeFileMeta, db_epoch: u64, db_at: &str| {
            meta.epoch == db_epoch && meta.db_epoch_updated_at == db_at
        };
        // 双字段都相等才算一致。
        assert!(matches(&meta(3, "t3"), 3, "t3"));
        // 数值相等、时间戳不等（库快照回滚恰好撞回同一计数）→ 失配。
        assert!(!matches(&meta(3, "t-old"), 3, "t3"));
        // 旧版 sidecar（无时间戳字段，缺省空串）→ 一律失配。
        assert!(!matches(&meta(3, ""), 3, "t3"));
        // 全新库（无 epoch 记录 → (0, "")）+ 与之匹配的 sidecar → 一致。
        assert!(matches(&meta(0, ""), 0, ""));
    }

    /// `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值：旧的 `is_ok()` 判定下，部署
    /// 模板写 `=0` 想关闭，实际每次启动都强制全量重建。
    #[test]
    fn force_rebuild_only_accepts_truthy_values() {
        use std::ffi::OsStr;

        assert!(!force_spatial_rebuild_flag(None), "unset 必须关闭");
        for off in ["", "  ", "0", "false", "no", "off", "FALSE", " Off "] {
            assert!(
                !force_spatial_rebuild_flag(Some(OsStr::new(off))),
                "明确假值必须关闭: {off:?}"
            );
        }
        for on in ["1", "true", "yes", "on", "TRUE", " On "] {
            assert!(
                force_spatial_rebuild_flag(Some(OsStr::new(on))),
                "明确真值必须打开: {on:?}"
            );
        }
        for junk in ["2", "rebuild", "开"] {
            assert!(
                !force_spatial_rebuild_flag(Some(OsStr::new(junk))),
                "认不出的值必须按关闭处理: {junk:?}"
            );
        }
    }

    /// 盖章指纹必须在写文件之前读：并发 bump 只许把 sidecar 盖旧（下次多做一次
    /// 重建，方向保守），不许把新章盖在旧内容上。
    #[test]
    fn fingerprint_is_read_before_the_file_is_written() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("async fn ", "persist_project_tree_now("))
            .expect("persist_project_tree_now must exist")
            .1
            .split_once(concat!("pub async fn ", "persist_aabb_tree_if_dirty("))
            .expect("dirty persist must follow")
            .0;
        let stamp_at = body
            .find("read_db_spatial_epoch_stamp()")
            .expect("盖章前必须读库侧指纹");
        let write_at = body.find("write_file_atomic(").expect("必须原子写快照文件");
        assert!(stamp_at < write_at, "指纹必须在写文件之前读: {body}");
        // D3：发布成功后才清旧格式文件，顺序不能反。
        let legacy_cleanup_at = body
            .find("remove_legacy_tree_files()")
            .expect("V2 发布后必须清旧格式文件");
        assert!(
            write_at < legacy_cleanup_at,
            "旧文件只能在 V2 写成之后删: {body}"
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

    /// V2 快照编解码矩阵：round-trip 全过；截断、错项目、错 namespace、哈希/载荷
    /// 篡改、条目数篡改、格式版本不符——任何一环失败都必须整体拒绝（调用方走重建）。
    #[test]
    fn snapshot_v2_validation_matrix() {
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
        let tree_bytes = bincode::serialize(&tree).expect("serialize tree");
        let snapshot = SpatialTreeSnapshotV2 {
            format_version: SNAPSHOT_FORMAT_VERSION,
            project: "AMS".into(),
            namespace: "1516".into(),
            epoch: 42,
            db_epoch_updated_at: "d'2026-08-12T00:00:00Z'".into(),
            entries: 2,
            usable_pointer_rows: 2,
            invalid_pointer_rows: 0,
            tree_sha256: sha256_hex(&tree_bytes),
            saved_at_unix: 1_755_000_000,
            tree_bytes,
        };
        let bytes = encode_snapshot_v2(&snapshot).expect("encode");

        let (decoded, restored) = decode_snapshot_v2(&bytes, "AMS", "1516").expect("round-trip");
        assert_eq!(decoded.epoch, 42);
        assert_eq!(decoded.db_epoch_updated_at, "d'2026-08-12T00:00:00Z'");
        assert_eq!(restored.size(), 2, "树载荷必须完整还原");

        assert!(
            decode_snapshot_v2(&bytes[..bytes.len() / 2], "AMS", "1516").is_err(),
            "截断文件必须拒绝"
        );
        assert!(
            decode_snapshot_v2(&bytes, "OTHER", "1516")
                .err()
                .expect("错项目必须拒绝")
                .to_string()
                .contains("项目"),
        );
        assert!(
            decode_snapshot_v2(&bytes, "AMS", "9999")
                .err()
                .expect("错 namespace 必须拒绝")
                .to_string()
                .contains("namespace"),
        );

        let mut hash_tamper = snapshot.clone();
        hash_tamper.tree_sha256 = sha256_hex(b"not the payload");
        let tampered = encode_snapshot_v2(&hash_tamper).expect("encode tampered");
        assert!(
            decode_snapshot_v2(&tampered, "AMS", "1516")
                .err()
                .expect("哈希失配必须拒绝")
                .to_string()
                .contains("哈希失配"),
        );

        let mut payload_tamper = snapshot.clone();
        payload_tamper.tree_bytes[0] ^= 0xFF;
        let tampered = encode_snapshot_v2(&payload_tamper).expect("encode tampered");
        assert!(
            decode_snapshot_v2(&tampered, "AMS", "1516").is_err(),
            "载荷被翻位必须拒绝（哈希兜底）"
        );

        let mut entries_tamper = snapshot.clone();
        entries_tamper.entries = 3;
        let tampered = encode_snapshot_v2(&entries_tamper).expect("encode tampered");
        assert!(
            decode_snapshot_v2(&tampered, "AMS", "1516")
                .err()
                .expect("条目数失配必须拒绝")
                .to_string()
                .contains("条目数"),
        );

        let mut version_tamper = snapshot.clone();
        version_tamper.format_version = 1;
        let tampered = encode_snapshot_v2(&version_tamper).expect("encode tampered");
        assert!(
            decode_snapshot_v2(&tampered, "AMS", "1516")
                .err()
                .expect("格式版本不符必须拒绝")
                .to_string()
                .contains("版本"),
        );
    }

    /// V2 在场但校验失败：一律 Unusable（走重建），**不回落**旧格式——旧文件自
    /// 迁移起冻结，回落它就是引入陈旧。回退即红。
    #[test]
    fn corrupt_v2_snapshot_never_falls_back_to_legacy() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once("fn read_snapshot_for_startup(")
            .expect("read_snapshot_for_startup must exist")
            .1
            .split_once("库侧空间版本号所在的固定记录")
            .expect("epoch 段随后")
            .0;
        let ok_arm = body
            .split_once("Ok(bytes) =>")
            .expect("V2 读取成功分支")
            .1
            .split_once("ErrorKind::NotFound")
            .expect("NotFound 分支随后")
            .0;
        assert!(
            ok_arm.contains("SnapshotReadOutcome::Unusable"),
            "V2 校验失败必须判不可用: {ok_arm}"
        );
        assert!(
            !ok_arm.contains("read_project_tree_file"),
            "V2 损坏不得回落旧格式: {ok_arm}"
        );
        // 旧格式只在 V2 **缺失**时作为迁移候选。
        let legacy_arm = body
            .split_once("ErrorKind::NotFound")
            .expect("checked above")
            .1;
        assert!(
            legacy_arm.contains("read_project_tree_file()"),
            "V2 缺失才读旧格式: {legacy_arm}"
        );
    }

    /// sidecar 元数据的编解码往返（含指纹时间戳字段）。
    #[test]
    fn tree_meta_roundtrip() {
        let meta = TreeFileMeta {
            epoch: 42,
            db_epoch_updated_at: "2026-08-11T07:00:00Z".into(),
            entries: 906,
            saved_at_unix: 1_754_000_000,
        };
        let bytes = serde_json::to_vec(&meta).expect("encode");
        let back: TreeFileMeta = serde_json::from_slice(&bytes).expect("decode");
        assert_eq!(back.epoch, 42);
        assert_eq!(back.db_epoch_updated_at, "2026-08-11T07:00:00Z");
        assert_eq!(back.entries, 906);
    }

    /// 旧版 sidecar（无 `db_epoch_updated_at` 字段）必须能解析且缺省空串——
    /// 空串永不等于库侧真实时刻，于是老文件自动落入失配分支、一次自愈后补齐。
    #[test]
    fn legacy_tree_meta_without_timestamp_parses_as_mismatch() {
        let back: TreeFileMeta =
            serde_json::from_slice(br#"{"epoch":42,"entries":906,"saved_at_unix":1}"#)
                .expect("decode legacy sidecar");
        assert_eq!(back.epoch, 42);
        assert_eq!(back.db_epoch_updated_at, "");
        let fingerprint_matches =
            back.epoch == 42 && back.db_epoch_updated_at == "2026-08-11T00:00:00Z";
        assert_eq!(
            startup_action(true, fingerprint_matches, false),
            StartupAction::Rebuild,
            "旧版 sidecar 不得凭数值相等直接放行"
        );
    }

    /// 版本号递增语句：固定记录、自增一、缺省从 0 起。
    #[test]
    fn epoch_bump_targets_the_singleton_record() {
        let sql = render_spatial_epoch_bump();
        assert!(sql.contains(SPATIAL_EPOCH_ID), "必须写固定记录: {sql}");
        assert!(sql.contains("(value?:0) + 1"), "必须缺省 0 自增一: {sql}");
    }

    /// 仓级钉（2026-08-12 审查修复计划 P2）：`GLOBAL_AABB_TREE` 的写入点必须
    /// 恰好落在下面这份**已审计**的白名单里。每个写入点自己的源码钉只护得住
    /// 所在文件；新文件里冒出来的新写点没有任何测试会红——而「动树却不留库侧
    /// 痕迹」正是直写路径静默漂移（H1/H2，2026-08-12 修复）的根因。
    ///
    /// 这条红了怎么办：新写入点要么满足不变量「变更与 epoch bump / 意图行同
    /// 事务提交，事务成功后才动树、动完标脏」（ADR-010 2026-08-12 增补）并配上
    /// 自己的源码钉，然后把文件加进白名单；要么改走既有入口（暂存 defer /
    /// durable 事务 / 提交后收敛）。白名单文件不再写树时把它摘掉，保持名单诚实。
    #[test]
    fn tree_write_sites_stay_on_the_audited_whitelist() {
        fn collect(dir: &std::path::Path, root: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    collect(&path, root, hits);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && std::fs::read_to_string(&path)
                        .expect("read source file")
                        .contains("GLOBAL_AABB_TREE.write()")
                {
                    hits.push(
                        path.strip_prefix(root)
                            .expect("under src")
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        collect(&root, &root, &mut hits);
        hits.sort();

        let whitelist = [
            // 直写删除：锁下探测 → 边删除+bump 同事务 → 摘树 → 标脏。
            "data_interface/helper.rs",
            // 启动加载/指针重建与提交后收敛：本身即自愈动作，落盘自带盖章。
            "fast_model/aabb_tree.rs",
            // 直写/durable 刷新：指针+bump 同事务，锁跨 [判定 → 事务 → 同步]。
            "fast_model/occ_generate.rs",
            // 房间测试夹具（#[cfg(test)]，不在生产路径）。
            "fast_model/room_fixture.rs",
        ]
        .iter()
        .map(|path| path.to_string())
        .collect::<Vec<_>>();
        assert_eq!(
            hits, whitelist,
            "GLOBAL_AABB_TREE 的写入点集合变了。新增写点：先满足「变更与 epoch bump/\
             意图行同事务、成功后才动树、动完标脏」并配源码钉，再进白名单；移除写点：\
             把文件从白名单摘掉"
        );
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
