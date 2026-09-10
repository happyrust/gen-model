use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefnoEnum, SUL_DB};

use crate::fast_model::aabb_refresh::update_inst_relate_aabbs_by_refnos;
use crate::fast_model::model_memory_store::ProjectedGeometry;

/// 一条投影记录在空间树里的条目：世界盒 + 来源元素 + noun（ADR-072 D4）。
///
/// 盒**直接取** [`ProjectedGeometry::world_bounds`]——落库那一侧写进 `aabb.d` 的就是同一份值
/// （`model_db_adapter::collect_record`），树不必等行提交再回读一遍。隐式管身不进树（与此前
/// 只读 `inst_relate` 指针的口径一致，`tubi_relate` 从来不在树上）；NaN / Inf / 反向范围按
/// 「无可用盒」对待（[`aabb_is_usable`]）返回 `None`——与全量指针扫描同一裁决，否则重建出来的
/// 树与增量维护的树会对同一条记录给出两种答案，条目集合永远对不拢。
pub(crate) fn projected_tree_entry(record: &ProjectedGeometry) -> Option<RStarBoundingBox> {
    if record.tube.is_some() {
        return None;
    }
    let aabb = parry3d::bounding_volume::Aabb::new(
        record.world_bounds[0].into(),
        record.world_bounds[1].into(),
    );
    aabb_is_usable(&aabb).then(|| {
        RStarBoundingBox::new(
            aabb,
            crate::data_interface::generation_root::refno_from_e3d(record.source_refno),
            record.noun.clone(),
        )
    })
}

/// 模型落库提交之后把树追上**内存投影**（ADR-072 D4，取代按已提交指针回读库的旧收敛）。
///
/// `refreshed` 是这一轮写了新几何的来源元素（隐式管身不算），`boxes` 是它们的投影世界盒
/// （[`projected_tree_entry`]），`removed` 是被对账掉、房间边也一并清掉的来源。三步：
///
/// 1. `removed` 摘树；
/// 2. `refreshed` 里一条可用盒都没有的来源也摘树——树的口径是「镜像投影里可用的盒」，旧盒
///    留着会一直当房间候选，直到下一次指针重建才自愈；
/// 3. 其余按 `sync_refnos` 整批换新（一次遍历摘掉这些 refno 的全部旧条目再插新值）。
///
/// 不读库、不写库、不重算几何。动了树就置脏（落盘归空闲轮）并把 Ready / ReadyEmpty 那一格
/// 对齐条目数。调用点在事务**之后**：块没提交成功的组走不到这里（`persist_group` 出错即返回），
/// 所以树上只会出现库里已经有的盒。返回实际进树的条数。
pub(crate) async fn converge_tree_after_persist(
    refreshed: &[RefnoEnum],
    boxes: Vec<RStarBoundingBox>,
    removed: &[RefnoEnum],
) -> usize {
    let present = boxes.iter().map(|entry| entry.refno).collect::<HashSet<_>>();
    let mut evict = removed.iter().map(RefnoEnum::refno).collect::<HashSet<_>>();
    let vanished = refreshed
        .iter()
        .map(RefnoEnum::refno)
        .filter(|refno| !present.contains(refno))
        .collect::<Vec<_>>();
    if !vanished.is_empty() {
        println!(
            "落库后空间收敛：{} 个刷新来源中 {} 个在投影里没有可用盒，摘除其树条目",
            refreshed.len(),
            vanished.len()
        );
        evict.extend(vanished);
    }
    let synced = boxes.len();
    if evict.is_empty() && synced == 0 {
        return 0;
    }
    let (evicted, stale, entries) = {
        let mut tree = GLOBAL_AABB_TREE.write().await;
        let evicted = tree.remove_by_refnos(&evict);
        let stale = tree.sync_refnos(boxes).len();
        (evicted, stale, tree.size())
    };
    if evicted > 0 || stale > 0 || synced > 0 {
        mark_aabb_tree_dirty();
    }
    crate::fast_model::spatial_state::refresh_ready_kind(entries);
    synced
}

/// 账本重放的过渡壳：`side_effect_pending::publish_one_spatial_round` 还在调它，
/// 随第二条异步账整体退役（ADR-072 D5 / 计划 R4）。盒同样来自内存投影——按来源在
/// `ModelMemoryStore::global()` 里查记录——不再回读库。
pub(crate) async fn apply_deferred_spatial_mutations(
    deferred: crate::data_interface::staging::write_context::DeferredSpatialMutations,
) -> anyhow::Result<()> {
    let refreshed = deferred.refresh.into_iter().collect::<Vec<_>>();
    let removed = deferred.remove.into_iter().collect::<Vec<_>>();
    let members = refreshed
        .iter()
        .map(|refno| refno.refno().0 as i64)
        .collect::<std::collections::BTreeSet<_>>();
    let boxes = crate::fast_model::model_memory_store::ModelMemoryStore::global()
        .room_records_for_members(&members)
        .iter()
        .filter_map(projected_tree_entry)
        .collect::<Vec<_>>();
    converge_tree_after_persist(&refreshed, boxes, &removed).await;
    Ok(())
}

/// 测试专用：把这些来源的条目从全局树上摘掉（用例的准备 / 收尾）。树的写点白名单
/// （`tree_write_sites_stay_on_the_audited_whitelist`）只认本文件，别的模块的用例经这里动树。
#[cfg(test)]
pub(crate) async fn evict_tree_entries_for_test(sources: &[RefnoEnum]) {
    let refnos = sources.iter().map(RefnoEnum::refno).collect::<HashSet<_>>();
    GLOBAL_AABB_TREE.write().await.remove_by_refnos(&refnos);
}

/// 空间树自上次写回快照以来是否有未持久化的增量变更。
///
/// 增量路径（落库后收敛、AABB 刷新、删除清理）只更新内存树，这个标记决定的只是「这一轮要不要
/// 真的写文件」；落盘时机归 worker 空闲轮（ADR-010 落盘时机，2026-07-28 已决）——任务收尾
/// 置脏、空闲轮落盘。它跨不过重启，所以不承担正确性：崩溃后丢掉的内存树变更由启动判据兜底
/// （快照头的 epoch 指纹与各库水位数对任一与库不等即指针重建，见 [`load_project_tree_verified`]）。
static AABB_TREE_DIRTY: AtomicBool = AtomicBool::new(false);

/// 增量路径动过内存树之后调用：标记「有变更待落盘」。
pub fn mark_aabb_tree_dirty() {
    AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
}

/// 旧格式（V1）树文件：`accel_tree_{project}.bin`（ADR-010 §6「路径带项目名」）。
///
/// 自单文件快照（2026-08-12 方案 §3）起**不读不写**，只在快照发布成功后被
/// [`remove_legacy_tree_files`] 清掉。它的 sidecar 只带 epoch 指纹、没有各库水位数对，
/// 在 ADR-072 的启动判据下永远对不上——读它就是白读，一律从库指针重建。
pub fn project_tree_file() -> String {
    format!("accel_tree_{}.bin", aios_core::get_db_option().project_name)
}

/// 旧格式树文件的 sidecar 元数据：`accel_tree_{project}.meta.json`（同上，只清不读）。
fn project_tree_meta_file() -> String {
    format!(
        "accel_tree_{}.meta.json",
        aios_core::get_db_option().project_name
    )
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

// ── 单文件快照（一致性闭环方案 §3；V4 头随 ADR-072 D4 改水位数对）────────────
//
// 旧「bin + sidecar」两个文件之间没有原子性：崩溃可能留下「新树旧章」或
// 「旧树新章」的组合，指纹校验对后者无能为力。单文件快照把树载荷、指纹与自校验哈希
// 封进**一个**文件，rename 原子替换，读侧全套校验通过才接受。V4 把头里的
// 「文件级已发布输入清单」（版本 / 文件数 / 摘要，随 publish 阶段退役）换成各库
// `(applied_sesno, model_sesno)` 数对：树镜像的是任务同步写进库的模型行，
// 而三水位正是「库写到了哪一版」的唯一表达。

/// 快照格式版本。改结构必须递增——旧版校验失败一律按不可用重建，不做跨版本猜测
/// （V3 → V4 的升级就是这样：第一次启动多做一次指针重建，之后落盘的就是 V4）。
const SNAPSHOT_FORMAT_VERSION: u32 = 4;

/// 本项目空间树的单文件快照：`accel_tree_{project}.snapshot`。
pub fn project_snapshot_file() -> String {
    format!(
        "accel_tree_{}.snapshot",
        aios_core::get_db_option().project_name
    )
}

/// 各库 `(applied_sesno, model_sesno)` 数对，键是 dbnum（ADR-072 D4）。
pub(crate) type WatermarkPairs = BTreeMap<u32, (i32, i32)>;

/// 单文件快照：树载荷（`tree_bytes`，独立 bincode 段，哈希对字节算，避免「对结构再序列化
/// 一次」的双重序列化歧义）+ 两半指纹（库侧 epoch 章、各库水位数对）+ 口径统计 + 身份。
/// `project`/`namespace` 挡「拷别的项目的快照 / 换库同项目名」（R4）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SpatialTreeSnapshot {
    format_version: u32,
    project: String,
    namespace: String,
    epoch: u64,
    db_epoch_updated_at: String,
    /// 封装时刻登记表里各库的 `(applied_sesno, model_sesno)`。启动与镜像同源的那张表
    /// 比对，任一库不等（含多了 / 少了一个库）即从 `inst_relate` 指针重建。
    watermarks: WatermarkPairs,
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

fn encode_snapshot(snapshot: &SpatialTreeSnapshot) -> anyhow::Result<Vec<u8>> {
    bincode::serialize(snapshot).map_err(|e| anyhow::anyhow!("序列化空间树快照失败: {e}"))
}

/// 校验并解码快照：完整反序列化 + 版本 + 项目/namespace + 载荷哈希 + 条目数
/// **全部**通过才接受；任何一环差错都按不可用处理（调用方走指针重建）。
fn decode_snapshot(
    bytes: &[u8],
    project: &str,
    namespace: &str,
) -> anyhow::Result<(SpatialTreeSnapshot, AccelerationTree)> {
    let snapshot: SpatialTreeSnapshot = bincode::deserialize(bytes)
        .map_err(|e| anyhow::anyhow!("反序列化空间树快照失败: {e}"))?;
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

/// 最近一次发布/装载的快照头（/health 用）。快照是 17MB 量级的单文件，health 每次
/// 全量读盘解码不可取；进程内缓存在发布与装载两个点更新。外部换文件在运行中不可
/// 见，由下次启动的全套校验接住。std Mutex，不跨 await 持有。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotHeaderInfo {
    pub format_version: u32,
    pub epoch: u64,
    pub db_epoch_updated_at: String,
    pub entries: u64,
    /// 封装时刻各库的 `(applied_sesno, model_sesno)`。
    pub watermarks: WatermarkPairs,
    pub tree_sha256: String,
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

/// 库侧当前的空间指纹：epoch 章 + 各库水位数对。快照头两半都与它相等才算「文件 = 库」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DbSpatialFingerprint {
    pub epoch: u64,
    pub db_epoch_updated_at: String,
    pub watermarks: WatermarkPairs,
}

impl SnapshotHeaderInfo {
    /// 纯判据：epoch 双字段相等（防「快照回滚撞回同一计数」）**且**各库水位数对逐库相等
    /// （多一个库、少一个库、任一库的 applied / model 不同，都算不等）。
    pub(crate) fn matches(&self, db: &DbSpatialFingerprint) -> bool {
        self.epoch == db.epoch
            && self.db_epoch_updated_at == db.db_epoch_updated_at
            && self.watermarks == db.watermarks
    }
}

impl SpatialTreeSnapshot {
    fn header(&self) -> SnapshotHeaderInfo {
        SnapshotHeaderInfo {
            format_version: self.format_version,
            epoch: self.epoch,
            db_epoch_updated_at: self.db_epoch_updated_at.clone(),
            entries: self.entries,
            watermarks: self.watermarks.clone(),
            tree_sha256: self.tree_sha256.clone(),
            saved_at_unix: self.saved_at_unix,
        }
    }
}

/// 快照发布成功后删除旧格式文件（方案 D3）。
///
/// 旧格式自单文件快照起就不再被读，留着只会让人误以为它还算数。删除失败只告警：
/// 单文件快照才是权威，残留旧文件不影响本版本。
fn remove_legacy_tree_files() {
    for path in [project_tree_file(), project_tree_meta_file()] {
        match std::fs::remove_file(&path) {
            Ok(()) => println!("已移除旧格式空间树文件 {path}（单文件快照已接管）"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("移除旧格式空间树文件 {path} 失败（不影响快照权威性）: {error}")
            }
        }
    }
}

/// 启动读快照的裁决：只认当前格式的单文件快照；缺失 / 损坏 / 校验失败 / 旧版本
/// 一律不可用（走指针重建）。旧「bin + sidecar」格式**不**回落——它的头没有水位数对，
/// 在 ADR-072 的判据下永远对不上，读它就是白读。
enum SnapshotReadOutcome {
    /// 通过文件内全套校验；两半指纹在装载器里再与库核对。
    Usable(Box<(SpatialTreeSnapshot, AccelerationTree)>),
    /// 不可用（缺失/损坏/校验失败/旧版本），带原因。
    Unusable(String),
}

fn read_snapshot_for_startup(project: &str, namespace: &str) -> SnapshotReadOutcome {
    let path = project_snapshot_file();
    match std::fs::read(&path) {
        Ok(bytes) => match decode_snapshot(&bytes, project, namespace) {
            Ok(decoded) => SnapshotReadOutcome::Usable(Box::new(decoded)),
            Err(error) => {
                SnapshotReadOutcome::Unusable(format!("快照 {path} 校验失败: {error:#}"))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            SnapshotReadOutcome::Unusable(format!("快照 {path} 不存在"))
        }
        Err(error) => SnapshotReadOutcome::Unusable(format!("读取快照 {path} 失败: {error}")),
    }
}

/// 库侧空间版本号所在的固定记录。
///
/// 不变量：**凡是改变了「树应有内容」的已提交变更，都在同一事务内把它 +1**
/// （[`render_spatial_epoch_bump`]）。模型落库的每一块、直写路径（包围盒刷新、删除清理）、
/// 回退到水位都 bump——它是「树应有内容」变化在库侧留下的痕迹，与各库水位数对一起组成
/// 启动指纹：水位数对说「库写到了哪一版」，epoch 章兜住那些不推水位却动了几何的写
/// （`/model/ensure` 逐根落库、运维删除、手工修复）。启动时快照头的 epoch 与它不相等，
/// 就说明「快照之后还有过没被镜像进文件的空间提交」，文件不可信。
const SPATIAL_EPOCH_ID: &str = "spatial_epoch:current";

/// 渲染空间版本号的递增语句，与产生该变更的写入放进同一个事务。
///
/// 调用点：模型落库动几何的块、直写包围盒刷新（与指针 UPDATE 同事务）、直写删除清理
/// （与房间边 DELETE 同事务）、数据尾事务与回退到水位。
///
/// 重试或分块导致的多次递增无害：版本号只与快照头比相等、不表达次数，多 bump
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
/// [`render_spatial_epoch_bump`] 同一事务写入，合成的指纹见 [`DbSpatialFingerprint`]。
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

/// 各库当前的 `(applied_sesno, model_sesno)` 数对——快照头与启动判据的另一半指纹（ADR-072 D4）。
///
/// 读的是 `dbnum_watermark` 登记表（计划 §4.5 白名单：三水位 / sesno 这类控制面信息），
/// 与水位镜像**同源**——镜像是这张表按格的缓存、没有整表枚举口，而这里要的是全表：
/// 一个在快照之后才初始化的库，靠头里已有的键是列举不出来的。只算已建立数据水位的库
/// （`initialized`）：只被扫到、还没初始化的登记行没有模型行，也就不在树上。
pub(crate) async fn current_watermark_pairs() -> anyhow::Result<WatermarkPairs> {
    Ok(crate::data_interface::dbnum_state::DbnumState::list_registered()
        .await
        .map_err(|e| anyhow::anyhow!("读取各库水位数对失败: {e}"))?
        .into_iter()
        .filter(|state| state.initialized)
        .map(|state| (state.dbnum, (state.applied_sesno, state.model_sesno)))
        .collect())
}

/// 库侧当前的完整空间指纹：epoch 章 + 各库水位数对，两次读同一处失败语义。
pub(crate) async fn read_db_spatial_fingerprint() -> anyhow::Result<DbSpatialFingerprint> {
    let (epoch, db_epoch_updated_at) = read_db_spatial_epoch_stamp().await?;
    let watermarks = current_watermark_pairs().await?;
    Ok(DbSpatialFingerprint {
        epoch,
        db_epoch_updated_at,
        watermarks,
    })
}

/// 发布当前内存树为单文件快照。
///
/// 两半指纹（epoch 章 + 各库水位数对）都在写文件**之前**读：并发的写入方若在读章与
/// 写盘之间又推高了版本号或盖了水位，快照只会偏旧 → 下次启动宁可多做一次指针重建，
/// 方向安全；反过来先写后读会把新章盖在旧内容上。发布成功后顺手清掉旧格式文件
/// （方案 D3，见 [`remove_legacy_tree_files`]）并更新 /health 用的头缓存。
async fn persist_project_tree_now() -> anyhow::Result<()> {
    let fingerprint = read_db_spatial_fingerprint().await?;
    let (tree_bytes, entries) = {
        let tree = GLOBAL_AABB_TREE.read().await;
        let bytes =
            bincode::serialize(&*tree).map_err(|e| anyhow::anyhow!("序列化空间树失败: {e}"))?;
        (bytes, tree.size() as u64)
    };
    let stats = crate::fast_model::spatial_state::snapshot();
    let db_option = aios_core::get_db_option();
    let snapshot = SpatialTreeSnapshot {
        format_version: SNAPSHOT_FORMAT_VERSION,
        project: db_option.project_name.clone(),
        namespace: db_option.surreal_ns.clone(),
        epoch: fingerprint.epoch,
        db_epoch_updated_at: fingerprint.db_epoch_updated_at,
        watermarks: fingerprint.watermarks,
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
    write_file_atomic(&project_snapshot_file(), &encode_snapshot(&snapshot)?)?;
    record_snapshot_header(snapshot.header());
    remove_legacy_tree_files();
    Ok(())
}

/// 脏则写回项目树文件（worker 空闲轮收尾调用），返回是否真的写了。
///
/// 落盘时机就是这里（ADR-072 计划 R3「任务收尾置脏 + 空闲轮落盘」）：任务内的落库后收敛
/// 只置脏，不在关键路径上写 17MB 的文件。
pub async fn persist_aabb_tree_if_dirty() -> anyhow::Result<bool> {
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    persist_aabb_tree_if_dirty_locked().await
}

/// 已持空间串行锁的版本（持锁调用方用，锁不可重入）。
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

/// 过渡壳：文件发布清单退役前（`side_effect_pending` / `room_resident::advance_manifest`
/// 仍在调）保留原语义——不看脏标记、过发布门就无条件写一次快照并清脏。快照头不再记
/// 发布清单，`_jobs` 没有用处；随第二条异步账一起删（ADR-072 D5 / 计划 R3–R4）。
pub(crate) async fn persist_aabb_tree_if_dirty_locked_for_publication(
    _jobs: &[crate::data_interface::side_effect_pending::PublishedRoundIdentity],
) -> anyhow::Result<bool> {
    let state = crate::fast_model::spatial_state::current_state();
    if !state.allows_snapshot_publish() {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        println!(
            "空间树状态 {} 不允许发布文件快照，保留脏标记等状态收敛",
            state.as_str()
        );
        return Ok(false);
    }
    if let Err(error) = persist_project_tree_now().await {
        AABB_TREE_DIRTY.store(true, Ordering::SeqCst);
        return Err(error);
    }
    AABB_TREE_DIRTY.store(false, Ordering::SeqCst);
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

/// 启动装载动作（方案 2026-08-12 一致性闭环 §2，ADR-072 D4 收成两档；纯判据，IO 全在调用方）。
///
/// 没有第三档：publish 阶段与 `spatial_reconcile` 意图行退役之后，「快照 + 待重放意图」
/// 这一形态不存在了；文件级已发布输入清单也随之退，清单不匹配曾经落 `DegradedBlocked`
/// 停掉空间消费者——现在任何不等都只是「文件比库旧」，答案是重建，不是停摆。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupAction {
    /// 快照可用且两半指纹（epoch 章 + 各库水位数对）都与库一致：直接复用（快路径）。
    Reuse,
    /// 快照缺失/损坏/旧版本，或任一半指纹与库不等（任务中途崩溃、直写崩溃、回退到水位、
    /// 换文件、回滚库）：只读指针重建。
    Rebuild,
}

/// 纯判据：快照可用**且**指纹一致才复用，其余一律重建。
fn startup_action(snapshot_usable: bool, fingerprint_matches: bool) -> StartupAction {
    if snapshot_usable && fingerprint_matches {
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

/// 启动加载（方案 2026-08-12 一致性闭环 §2，判据按 ADR-072 D4 改）：
///
/// 0. 显式夹具标记 + 内存树非空 → preloaded（仅测试装载模式；生产入口一律校验，
///    旧的「内存树非空即 preloaded」盲信短路已删除）；
/// 1. `AIOS_FORCE_SPATIAL_REBUILD` 为真值 → 指针重建；
/// 2. 快照缺失/损坏/旧版本 → 指针重建；
/// 3. 快照可读：
///    3a. 读库侧指纹（epoch 章 / 各库水位数对）失败 → DegradedReuse 降级复用 + 门禁消费者，
///        revalidator 复检；
///    3b. epoch 双字段一致**且**各库 `(applied_sesno, model_sesno)` 与登记表逐库相等 → Ready 复用；
///    3c. 任一半不等 → 指针重建（不再有 `DegradedBlocked`：文件比库旧不是故障，是重建的理由）。
///
/// 不再问 `spatial_reconcile` 意图行、文件轮次、已发布输入清单——三者随 publish 阶段退役
/// （计划 R3 / R4），定义本身由退役第二条异步账的那一步删。整个装载持空间串行锁，
/// 与写路径/收敛/落盘串行。
pub async fn load_project_tree_verified() -> anyhow::Result<()> {
    let _serial = crate::fast_model::spatial_state::lock_spatial_serial().await;
    load_project_tree_verified_locked().await
}

async fn load_project_tree_verified_locked() -> anyhow::Result<()> {
    use crate::fast_model::spatial_state::{self, SpatialTreeState};

    // 同进程重复装载短路（启动链 run_app 与 run_cli 各调一次装载，快照 3.5-21MB 的
    // 读盘+解码白付一遍）。判据是 [`startup_action`] 的 Reuse 那组原样：本进程已
    // 收敛到 Ready/ReadyEmpty、库侧两半指纹与进程内缓存的快照头一致——不是「内存树
    // 非空即信」的盲信短路；另要求状态与树条目自洽（崩溃模拟测试会清树不清状态，
    // 两者矛盾时走完整装载）。指纹读不到、头缺失都不短路；强制重建与夹具预载优先级更高。
    let short_circuit_state = spatial_state::current_state();
    if !force_spatial_rebuild_enabled()
        && !spatial_state::fixture_preload_requested()
        && short_circuit_state.is_ready()
    {
        let entries = GLOBAL_AABB_TREE.read().await.size();
        let state_matches_tree = (entries > 0) == (short_circuit_state == SpatialTreeState::Ready);
        if state_matches_tree
            && let Some(header) = snapshot_header()
            && let Ok(db) = read_db_spatial_fingerprint().await
            && header.matches(&db)
        {
            println!(
                "空间树本进程已装载且库侧指纹一致（{entries} 条，epoch {}，{} 个库的水位数对相等），跳过重复装载",
                db.epoch,
                db.watermarks.len()
            );
            return Ok(());
        }
    }

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
    let (snapshot, tree) =
        match read_snapshot_for_startup(&db_option.project_name, &db_option.surreal_ns) {
            SnapshotReadOutcome::Usable(decoded) => *decoded,
            SnapshotReadOutcome::Unusable(reason) => {
                eprintln!("空间树快照不可用（{reason}），从库指针自动重建");
                return rebuild_at_startup_locked().await;
            }
        };
    let header = snapshot.header();
    let entries = tree.size();

    let db = match read_db_spatial_fingerprint().await {
        Ok(db) => db,
        Err(error) => {
            *GLOBAL_AABB_TREE.write().await = tree;
            record_snapshot_header(header);
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

    match startup_action(true, header.matches(&db)) {
        StartupAction::Reuse => {
            *GLOBAL_AABB_TREE.write().await = tree;
            println!(
                "空间树复用快照 {}（{entries} 条，epoch {} @ {} 与库一致，{} 个库的水位数对相等）",
                project_snapshot_file(),
                db.epoch,
                db.db_epoch_updated_at,
                db.watermarks.len()
            );
            record_snapshot_header(header);
            record_startup_verdict("reused");
            spatial_state::set_ready_by_entries(entries);
        }
        StartupAction::Rebuild => {
            println!(
                "空间树快照与库不一致（快照 epoch {} @ {} / {}；库 epoch {} @ {} / {}）：\
                 文件比库旧（任务中途崩溃 / 回退到水位 / 直写崩溃 / 换文件 / 回滚库），从库指针重建",
                header.epoch,
                header.db_epoch_updated_at,
                describe_watermark_pairs(&header.watermarks),
                db.epoch,
                db.db_epoch_updated_at,
                describe_watermark_pairs(&db.watermarks)
            );
            return rebuild_at_startup_locked().await;
        }
    }
    Ok(())
}

/// 日志用：`{dbnum:(applied,model), …}`。
fn describe_watermark_pairs(pairs: &WatermarkPairs) -> String {
    let body = pairs
        .iter()
        .map(|(dbnum, (applied, model))| format!("{dbnum}:({applied},{model})"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("水位数对 {{{body}}}")
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
/// 快照侧取进程内头缓存（V3 是 17MB 单文件，health 每次全量读盘解码不可取；
/// 缓存在发布与装载两处更新，外部换文件由下次启动的全套校验接住）；库侧指纹
/// 现读现比，运行中出现的漂移看得见。任何一侧读不出来都如实报 null、`drift`
/// 置 true 并把原因并入 `last_error`——健康端点不许因诊断失败而挂。
pub async fn spatial_tree_status() -> serde_json::Value {
    let entries = GLOBAL_AABB_TREE.read().await.size();
    let header = snapshot_header();
    let db = read_db_spatial_fingerprint().await;
    render_spatial_tree_status(
        entries,
        header.as_ref(),
        &db,
        STARTUP_VERDICT.get().copied().unwrap_or("unknown"),
        &crate::fast_model::spatial_state::snapshot(),
    )
}

/// /health `spatial_tree` 的纯渲染半边。
///
/// 十六个键是对外承诺（台账 G-02 契约迁移的十八键形状按 ADR-072 收：`pending` 与
/// `published_*` 三格随 publish 阶段 / 意图行退役消失，`snapshot_watermarks` / `db_watermarks`
/// 两格进来，2026-09-10 计划 §7「文件轮次 / pending 格消失」）。
/// `drift`：epoch **双字段都相等**（防「快照回滚撞回同一计数」）**且**各库水位数对逐库相等
/// 才算无漂移；任何一侧读不出来都如实报 null、`drift` 置 true、原因并入 `last_error`
/// ——降级分支不许缩键。水位数对渲成 `{ "<dbnum>": [applied, model] }`。
fn render_spatial_tree_status(
    entries: usize,
    header: Option<&SnapshotHeaderInfo>,
    db: &anyhow::Result<DbSpatialFingerprint>,
    startup_verdict: &'static str,
    state: &crate::fast_model::spatial_state::SpatialStateSnapshot,
) -> serde_json::Value {
    let drift = !matches!((header, db), (Some(header), Ok(db)) if header.matches(db));
    let last_error = state
        .last_error
        .clone()
        .or_else(|| db.as_ref().err().map(|error| format!("{error:#}")));
    let render_pairs = |pairs: &WatermarkPairs| {
        pairs
            .iter()
            .map(|(dbnum, (applied, model))| {
                (dbnum.to_string(), serde_json::json!([applied, model]))
            })
            .collect::<serde_json::Map<_, _>>()
    };
    serde_json::json!({
        "state": state.state.as_str(),
        "ready": state.state.is_ready(),
        "startup_verdict": startup_verdict,
        "format_version": header.map(|header| header.format_version),
        "entries": entries,
        "usable_pointer_rows": state.usable_pointer_rows,
        "invalid_pointer_rows": state.invalid_pointer_rows,
        "file_epoch": header.map(|header| header.epoch),
        "db_epoch": db.as_ref().ok().map(|db| db.epoch),
        "drift": drift,
        "snapshot_watermarks": header.map(|header| render_pairs(&header.watermarks)),
        "db_watermarks": db.as_ref().ok().map(|db| render_pairs(&db.watermarks)),
        "snapshot_sha256": header.map(|header| header.tree_sha256.clone()),
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
             OR count((SELECT id FROM out->geo_relate WHERE out.aabb.d != none AND (transform ?? trans.d) != none)) > 0) \
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
            .split_once(concat!(
                "pub(crate) async fn ",
                "persist_aabb_tree_if_dirty_locked_for_publication("
            ))
            .expect("publication shim must follow")
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

    fn pairs(entries: &[(u32, i32, i32)]) -> WatermarkPairs {
        entries
            .iter()
            .map(|&(dbnum, applied, model)| (dbnum, (applied, model)))
            .collect()
    }

    fn header_with(epoch: u64, at: &str, watermarks: WatermarkPairs) -> SnapshotHeaderInfo {
        SnapshotHeaderInfo {
            format_version: SNAPSHOT_FORMAT_VERSION,
            epoch,
            db_epoch_updated_at: at.into(),
            entries: 42,
            watermarks,
            tree_sha256: "abc123".into(),
            saved_at_unix: 1_755_000_000,
        }
    }

    fn db_with(epoch: u64, at: &str, watermarks: WatermarkPairs) -> DbSpatialFingerprint {
        DbSpatialFingerprint {
            epoch,
            db_epoch_updated_at: at.into(),
            watermarks,
        }
    }

    /// /health `spatial_tree` 的十六键契约（台账 G-02 的十八键按 ADR-072 收：`pending` /
    /// `published_*` 三格退，`snapshot_watermarks` / `db_watermarks` 进）。
    ///
    /// 指纹双字段（epoch + 库侧 bump 时刻）都相等**且**各库水位数对相等才算无漂移——
    /// 计数撞值的快照回滚、某个库水位前进了而快照没跟上，都必须按漂移报；诊断失败的
    /// 降级分支键一个不少、file_* 如实为 null、`last_error` 说得出原因。
    #[test]
    fn spatial_tree_status_keeps_its_sixteen_key_shape_in_both_branches() {
        use crate::fast_model::spatial_state::{SpatialStateSnapshot, SpatialTreeState};

        let header = header_with(7, "d'2026-08-12T00:00:00Z'", pairs(&[(7997, 163, 163)]));
        let db = Ok(db_with(7, "d'2026-08-12T00:00:00Z'", pairs(&[(7997, 163, 163)])));
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
            "file_epoch",
            "db_epoch",
            "drift",
            "snapshot_watermarks",
            "db_watermarks",
            "snapshot_sha256",
            "last_verified_at",
            "last_rebuild_attempts",
            "last_error",
        ];
        let healthy = render_spatial_tree_status(42, Some(&header), &db, "reused", &state);
        let object = healthy.as_object().expect("形状必须是对象");
        assert_eq!(object.len(), keys.len(), "键数漂移: {healthy}");
        for key in keys {
            assert!(object.contains_key(key), "缺键 {key}: {healthy}");
        }
        for retired in ["pending", "published_version", "published_input_files", "published_input_sha256"] {
            assert!(!object.contains_key(retired), "退役格 {retired} 不许回来: {healthy}");
        }
        assert_eq!(healthy["state"], "ready");
        assert_eq!(healthy["ready"], true);
        assert_eq!(healthy["drift"], false);
        assert_eq!(healthy["last_error"], serde_json::Value::Null);
        assert_eq!(healthy["startup_verdict"], "reused");
        assert_eq!(healthy["format_version"], SNAPSHOT_FORMAT_VERSION);
        assert_eq!(healthy["snapshot_sha256"], "abc123");
        assert_eq!(healthy["file_epoch"], 7);
        assert_eq!(healthy["db_epoch"], 7);
        assert_eq!(healthy["usable_pointer_rows"], 42);
        assert_eq!(
            healthy["snapshot_watermarks"],
            serde_json::json!({ "7997": [163, 163] }),
            "水位数对渲成 dbnum → [applied, model]: {healthy}"
        );
        assert_eq!(healthy["db_watermarks"], serde_json::json!({ "7997": [163, 163] }));

        // 计数相等而库侧时刻不同 = 快照回滚撞回同一计数，必须按漂移报。
        let rolled_back = Ok(db_with(7, "d'2026-08-11T00:00:00Z'", pairs(&[(7997, 163, 163)])));
        let drifted = render_spatial_tree_status(42, Some(&header), &rolled_back, "reused", &state);
        assert_eq!(drifted["drift"], true, "指纹双字段必须都相等: {drifted}");

        // epoch 一致而某个库的水位前进了：快照没跟上，同样是漂移。
        let advanced = Ok(db_with(7, "d'2026-08-12T00:00:00Z'", pairs(&[(7997, 165, 163)])));
        let drifted = render_spatial_tree_status(42, Some(&header), &advanced, "reused", &state);
        assert_eq!(drifted["drift"], true, "水位数对不等必须按漂移报: {drifted}");

        // 诊断失败/头缺失：键一个不少，快照侧如实为 null，last_error 报得出原因，
        // ready 只由状态机决定。
        let degraded_state = SpatialStateSnapshot {
            state: SpatialTreeState::DegradedReuse,
            last_error: None,
            last_rebuild_attempts: 3,
            last_verified_at_unix: None,
            usable_pointer_rows: None,
            invalid_pointer_rows: None,
        };
        let broken = render_spatial_tree_status(
            0,
            None,
            &Err(anyhow::anyhow!("读取各库水位数对失败")),
            "unknown",
            &degraded_state,
        );
        let broken_object = broken.as_object().expect("降级形状必须是对象");
        assert_eq!(
            broken_object.len(),
            keys.len(),
            "降级分支不许缩键: {broken}"
        );
        assert_eq!(broken["state"], "degraded_reuse");
        assert_eq!(broken["ready"], false);
        assert_eq!(broken["format_version"], serde_json::Value::Null);
        assert_eq!(broken["file_epoch"], serde_json::Value::Null);
        assert_eq!(broken["db_epoch"], serde_json::Value::Null);
        assert_eq!(broken["snapshot_sha256"], serde_json::Value::Null);
        assert_eq!(broken["snapshot_watermarks"], serde_json::Value::Null);
        assert_eq!(broken["db_watermarks"], serde_json::Value::Null);
        assert_eq!(broken["last_rebuild_attempts"], 3);
        assert_eq!(broken["drift"], true, "读不出指纹必须按漂移报: {broken}");
        assert!(
            broken["last_error"]
                .as_str()
                .expect("降级必须报出错误原因")
                .contains("读取各库水位数对失败")
        );

        // 状态簿记里的错误优先曝光（重建失败原因比诊断读失败更接近根因）。
        let with_state_error = SpatialStateSnapshot {
            last_error: Some("指针重建失败: boom".into()),
            ..degraded_state.clone()
        };
        let prioritized = render_spatial_tree_status(
            0,
            None,
            &Err(anyhow::anyhow!("读取各库水位数对失败")),
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

    /// 落库后收敛只认内存投影（ADR-072 D4）：不读库、不写库、不重算几何。此前这里
    /// 按已提交指针 `select … aabb.d from inst_relate` 回读一遍，再往前则复跑
    /// `update_inst_relate_aabbs_by_refnos(.., true)` 把几何整个重算重写——两条路都
    /// 跑在任务的关键路径上，也都违反「任务内不回读库」。回退即红。
    #[test]
    fn post_persist_convergence_takes_boxes_from_the_projection_not_the_database() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once(concat!("pub(crate) fn ", "projected_tree_entry("))
            .expect("projected_tree_entry must exist")
            .1
            .split_once(concat!("\n", "/// 空间树自上次写回"))
            .expect("dirty-flag doc must follow")
            .0;
        assert!(
            body.contains("record.world_bounds[0].into()")
                && body.contains("record.world_bounds[1].into()"),
            "盒必须直接取投影记录的 world_bounds: {body}"
        );
        assert!(
            body.contains("tree.sync_refnos(boxes)") && body.contains("tree.remove_by_refnos(&evict)"),
            "收敛 = sync_refnos 换新 + remove_by_refnos 摘旧: {body}"
        );
        assert!(
            body.contains("refresh_ready_kind(entries)"),
            "动过树要把 Ready/ReadyEmpty 那一格对齐条目数: {body}"
        );
        for forbidden in [
            "SUL_DB",
            ".query(",
            "inst_relate",
            "get_inst_relate_keys",
            concat!("sync_tree_from_", "committed_pointers"),
            "update_inst_relate_aabbs_by_refnos",
        ] {
            assert!(
                !body.contains(forbidden),
                "落库后收敛不得回读库 / 重算几何（{forbidden}）: {body}"
            );
        }
        // 全文件不许再有那个按指针回读的函数——验收 ② 的源码钉。
        assert!(
            !source
                .split(concat!("#[cfg(test)]\n", "mod tests"))
                .next()
                .expect("production half")
                .contains(concat!("sync_tree_from_", "committed_pointers")),
            "按已提交指针回读库的树同步已退役，不得回来"
        );
    }

    /// 投影记录 → 树条目的口径：管身不进树；NaN / Inf / 反向盒按无可用盒；正常记录的
    /// 盒、refno、noun 逐字进树。
    #[test]
    fn projected_tree_entry_mirrors_world_bounds_and_skips_tubes_and_invalid_boxes() {
        use crate::fast_model::model_memory_store::{ProjectedShape, ProjectedTube};
        use e3d_io::refno::RefNo;
        use e3d_model::elmodl::GeometryId;

        let record = ProjectedGeometry {
            record_id: "24383_73948".into(),
            geometry_id: GeometryId::Element {
                refno: "24383/73948".into(),
            },
            source_refno: RefNo::new(24383, 73948),
            dbnum: 7997,
            source_sesno: 3,
            noun: "BOX".into(),
            mesh_id: "mesh".into(),
            world_transform: glam::DMat4::IDENTITY,
            world_bounds: [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            ancestors: vec![],
            shape: ProjectedShape::Baked,
            tube: None,
        };
        let entry = projected_tree_entry(&record).expect("正常记录进树");
        assert_eq!(
            entry.refno,
            crate::data_interface::generation_root::refno_from_e3d(record.source_refno).refno()
        );
        assert_eq!(entry.noun, "BOX");
        assert_eq!(entry.aabb.mins, parry3d::math::Point::new(1.0, 2.0, 3.0));
        assert_eq!(entry.aabb.maxs, parry3d::math::Point::new(4.0, 5.0, 6.0));

        let tube = ProjectedGeometry {
            tube: Some(ProjectedTube {
                route_ordinal: 0,
                from: RefNo::new(24383, 1),
                to: RefNo::new(24383, 2),
                invalid: false,
            }),
            ..record.clone()
        };
        assert!(projected_tree_entry(&tube).is_none(), "隐式管身不进树");

        let reversed = ProjectedGeometry {
            world_bounds: [[4.0, 2.0, 3.0], [1.0, 5.0, 6.0]],
            ..record.clone()
        };
        assert!(projected_tree_entry(&reversed).is_none(), "反向盒是空集语义");
        let nan = ProjectedGeometry {
            world_bounds: [[f32::NAN, 2.0, 3.0], [4.0, 5.0, 6.0]],
            ..record
        };
        assert!(projected_tree_entry(&nan).is_none(), "NaN 盒不进树");
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

    /// 启动装载（方案 2026-08-12 一致性闭环 §2，判据按 ADR-072 D4）：整个装载持空间串行锁；
    /// 必须读两半指纹（epoch 章 + 各库水位数对）并交给纯判据；快照缺失/损坏/旧版本与任一半
    /// 指纹不等都必须自动重建；**不许再问**意图行 / 文件轮次 / 已发布输入清单，也不许再落
    /// `DegradedBlocked`（清单不匹配曾经停掉空间消费者——现在不等只是重建的理由）；强制重建
    /// 只认真值解析；生产入口不得再有「内存树非空即 preloaded」的盲信短路；默认路径仍不得触发
    /// 条数对账或几何重算重写。验收 ④ 的源码钉。
    #[test]
    fn startup_compares_the_snapshot_header_with_the_watermark_pairs_then_rebuilds() {
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
            "默认启动必须先尝试快照: {body}"
        );
        assert!(
            body.contains("read_db_spatial_fingerprint().await"),
            "启动必须读库侧完整指纹（epoch 章 + 各库水位数对）: {body}"
        );
        assert!(
            body.contains("header.matches(&db)"),
            "快照头必须与库侧指纹整体比对: {body}"
        );
        assert!(
            body.contains("force_spatial_rebuild_enabled()") && !body.contains(".is_ok()"),
            "强制重建必须走真值解析，不得回到 is_ok 判定: {body}"
        );
        let fingerprint_at = body
            .find("read_db_spatial_fingerprint().await")
            .expect("checked above");
        let verdict_at = body
            .rfind("match startup_action(")
            .expect("裁决必须由纯判据函数给出");
        assert!(
            fingerprint_at < verdict_at,
            "指纹读取必须发生在裁决之前: {body}"
        );
        for retired in [
            "has_pending_spatial_work",
            "has_collecting_file_rounds",
            "acceptable_published_input_manifests",
            "published_file_input",
            "reconcile_spatial_pending",
            "side_effect_pending",
            "SpatialTreeState::DegradedBlocked",
            "SpatialTreeState::ReplayRequired",
            "published_input_mismatch",
            "file_round_private",
        ] {
            assert!(
                !body.contains(retired),
                "启动判据不得再问已退役的 {retired}（ADR-072 D4/D5）: {body}"
            );
        }
        assert!(
            !body.contains("sync_aabb_tree_with_db") && !body.contains("manual_update_aabbs"),
            "默认启动不得触发条数对账或几何重算重写: {body}"
        );

        // 快照缺失/损坏/旧版本分支必须走自动重建（决策 D1/D2），不再空树等人工。
        let unusable_branch = body
            .split_once("SnapshotReadOutcome::Unusable(")
            .expect("必须处理快照不可用分支")
            .1;
        assert!(
            unusable_branch.contains("return rebuild_at_startup_locked().await"),
            "快照不可用必须自动指针重建: {body}"
        );
        // 指纹不等的那一臂必须是重建，而不是任何一种降级停摆。
        let rebuild_arm = body
            .split_once("StartupAction::Rebuild =>")
            .expect("必须有重建臂")
            .1;
        assert!(
            rebuild_arm.contains("return rebuild_at_startup_locked().await"),
            "快照头与水位数对不等必须走指针重建: {rebuild_arm}"
        );
        // 读不到库侧指纹是唯一的降级分支，且只许 DegradedReuse（revalidator 复检）。
        assert_eq!(
            body.matches("SpatialTreeState::Degraded").count(),
            1,
            "启动只剩一处降级（读不到指纹 → DegradedReuse）: {body}"
        );
        assert!(body.contains("SpatialTreeState::DegradedReuse"));
    }

    /// 启动动作真值表（纯函数，ADR-072 D4 之后只剩两档）。
    #[test]
    fn startup_action_truth_table() {
        // 快照可用、两半指纹一致 → 复用（快路径）。
        assert_eq!(startup_action(true, true), StartupAction::Reuse);
        // 快照可用、任一半指纹不等 → 重建，不是降级停摆。
        assert_eq!(startup_action(true, false), StartupAction::Rebuild);
        // 快照不可用 → 无论指纹如何一律重建。
        assert_eq!(startup_action(false, false), StartupAction::Rebuild);
        assert_eq!(startup_action(false, true), StartupAction::Rebuild);
    }

    /// 快照头与库侧指纹的整体比对（验收 ④ 的纯函数半边）：epoch 双字段与各库水位数对
    /// **三者都相等**才算一致；某个库 applied 或 model 前进、多一个库、少一个库、epoch 计数
    /// 撞值而时刻不同——每一种都必须判不等，进而落到 [`StartupAction::Rebuild`]。
    #[test]
    fn a_header_matches_only_when_epoch_and_every_watermark_pair_agree() {
        let header = header_with(7, "t7", pairs(&[(7997, 163, 163), (8000, 40, 40)]));
        assert!(header.matches(&db_with(7, "t7", pairs(&[(7997, 163, 163), (8000, 40, 40)]))));
        assert_eq!(
            startup_action(
                true,
                header.matches(&db_with(7, "t7", pairs(&[(7997, 163, 163), (8000, 40, 40)])))
            ),
            StartupAction::Reuse
        );

        let mismatches: [(&str, DbSpatialFingerprint); 6] = [
            ("数据水位前进", db_with(7, "t7", pairs(&[(7997, 165, 163), (8000, 40, 40)]))),
            ("模型水位前进", db_with(7, "t7", pairs(&[(7997, 163, 165), (8000, 40, 40)]))),
            ("回退到水位", db_with(7, "t7", pairs(&[(7997, 160, 160), (8000, 40, 40)]))),
            ("多了一个库", db_with(7, "t7", pairs(&[(7997, 163, 163), (8000, 40, 40), (8001, 1, 1)]))),
            ("少了一个库", db_with(7, "t7", pairs(&[(7997, 163, 163)]))),
            ("epoch 撞值时刻不同", db_with(7, "t-old", pairs(&[(7997, 163, 163), (8000, 40, 40)]))),
        ];
        for (why, db) in mismatches {
            assert!(!header.matches(&db), "{why} 必须判不等");
            assert_eq!(
                startup_action(true, header.matches(&db)),
                StartupAction::Rebuild,
                "{why} 必须落到指针重建，而不是 DegradedBlocked"
            );
        }
        // 全新库（无 epoch 记录、无登记）+ 与之匹配的空头 → 一致。
        assert!(header_with(0, "", pairs(&[])).matches(&db_with(0, "", pairs(&[]))));
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

    /// 指纹判定保持 epoch 双字段口径（方案 2026-08-11 遗留不变量，现在是
    /// [`SnapshotHeaderInfo::matches`] 的一半）。
    #[test]
    fn fingerprint_match_requires_both_epoch_fields() {
        let header = |epoch: u64, at: &str| header_with(epoch, at, pairs(&[]));
        let db = |epoch: u64, at: &str| db_with(epoch, at, pairs(&[]));
        // 双字段都相等才算一致。
        assert!(header(3, "t3").matches(&db(3, "t3")));
        // 数值相等、时间戳不等（库快照回滚恰好撞回同一计数）→ 失配。
        assert!(!header(3, "t-old").matches(&db(3, "t3")));
        // 时刻缺失（空串）永不等于库侧真实时刻 → 失配。
        assert!(!header(3, "").matches(&db(3, "t3")));
        // 全新库（无 epoch 记录 → (0, "")）+ 与之匹配的头 → 一致。
        assert!(header(0, "").matches(&db(0, "")));
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

    /// 盖章指纹（epoch 章 + 各库水位数对）必须在序列化树、写文件之前读：并发 bump / 盖水位
    /// 只许把快照头盖旧（下次多做一次重建，方向保守），不许把新章盖在旧内容上。
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
            .find("read_db_spatial_fingerprint().await")
            .expect("盖章前必须读库侧完整指纹");
        let serialize_at = body
            .find("GLOBAL_AABB_TREE.read().await")
            .expect("必须序列化内存树");
        let write_at = body.find("write_file_atomic(").expect("必须原子写快照文件");
        assert!(
            stamp_at < serialize_at && serialize_at < write_at,
            "指纹必须在序列化树、写文件之前读: {body}"
        );
        assert!(
            body.contains("watermarks: fingerprint.watermarks"),
            "快照头必须记下各库水位数对: {body}"
        );
        // D3：发布成功后才清旧格式文件，顺序不能反。
        let legacy_cleanup_at = body
            .find("remove_legacy_tree_files()")
            .expect("发布后必须清旧格式文件");
        assert!(
            write_at < legacy_cleanup_at,
            "旧文件只能在快照写成之后删: {body}"
        );
    }

    /// 树的口径是「镜像投影里可用的盒」：刷新来源在投影里一条可用盒都没有时必须摘除树条目，
    /// 而不是跳过——跳过会让旧盒一直留在树上当房间候选，直到下一次指针重建才自愈。
    /// 用真树跑一遍：三个来源，一个换新盒、一个变成无效盒、一个被对账掉。
    #[tokio::test]
    async fn convergence_evicts_sources_without_a_usable_box_and_reconciled_sources() {
        fn bbox(seq: u32, min: f32, max: f32) -> RStarBoundingBox {
            RStarBoundingBox::new(
                parry3d::bounding_volume::Aabb::new(
                    parry3d::math::Point::new(min, min, min),
                    parry3d::math::Point::new(max, max, max),
                ),
                RefnoEnum::from(format!("4000000009/{seq}").as_str()),
                "BOX".to_string(),
            )
        }
        let refno = |seq: u32| RefnoEnum::from(format!("4000000009/{seq}").as_str());
        // 本用例独占这一组 refno（4000000009/*），与同进程其它用例不撞。
        {
            let mut tree = GLOBAL_AABB_TREE.write().await;
            tree.sync_refnos(vec![bbox(1, 0.0, 1.0), bbox(2, 0.0, 1.0), bbox(3, 0.0, 1.0)]);
        }
        AABB_TREE_DIRTY.store(false, Ordering::SeqCst);

        // 1 换了新盒；2 是刷新来源但投影里没有可用盒（盒无效被 projected_tree_entry 滤掉）；
        // 3 被对账掉。
        let synced = converge_tree_after_persist(
            &[refno(1), refno(2)],
            vec![bbox(1, 10.0, 11.0)],
            &[refno(3)],
        )
        .await;
        assert_eq!(synced, 1);
        assert!(AABB_TREE_DIRTY.load(Ordering::SeqCst), "动过树必须置脏");

        let tree = GLOBAL_AABB_TREE.read().await;
        let entries_of = |seq: u32| {
            tree.iter()
                .filter(|entry| entry.refno == refno(seq).refno())
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(entries_of(1), vec![bbox(1, 10.0, 11.0)], "换新盒，旧盒不堆叠");
        assert!(entries_of(2).is_empty(), "没有可用盒的刷新来源必须摘树");
        assert!(entries_of(3).is_empty(), "被对账掉的来源必须摘树");
        drop(tree);
        GLOBAL_AABB_TREE
            .write()
            .await
            .remove_by_refnos(&HashSet::from([refno(1).refno()]));
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

    /// 快照编解码矩阵：round-trip 全过（含各库水位数对）；截断、错项目、错 namespace、
    /// 哈希/载荷篡改、条目数篡改、格式版本不符（含上一代 V3）——任何一环失败都必须整体
    /// 拒绝（调用方走重建）。
    #[test]
    fn snapshot_validation_matrix() {
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
        let snapshot = SpatialTreeSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            project: "AMS".into(),
            namespace: "1516".into(),
            epoch: 42,
            db_epoch_updated_at: "d'2026-08-12T00:00:00Z'".into(),
            watermarks: pairs(&[(7997, 163, 163), (8000, 40, 38)]),
            entries: 2,
            usable_pointer_rows: 2,
            invalid_pointer_rows: 0,
            tree_sha256: sha256_hex(&tree_bytes),
            saved_at_unix: 1_755_000_000,
            tree_bytes,
        };
        let bytes = encode_snapshot(&snapshot).expect("encode");

        let (decoded, restored) = decode_snapshot(&bytes, "AMS", "1516").expect("round-trip");
        assert_eq!(decoded.epoch, 42);
        assert_eq!(decoded.db_epoch_updated_at, "d'2026-08-12T00:00:00Z'");
        assert_eq!(
            decoded.watermarks,
            pairs(&[(7997, 163, 163), (8000, 40, 38)]),
            "各库水位数对必须原样往返"
        );
        assert_eq!(restored.size(), 2, "树载荷必须完整还原");

        assert!(
            decode_snapshot(&bytes[..bytes.len() / 2], "AMS", "1516").is_err(),
            "截断文件必须拒绝"
        );
        assert!(
            decode_snapshot(&bytes, "OTHER", "1516")
                .err()
                .expect("错项目必须拒绝")
                .to_string()
                .contains("项目"),
        );
        assert!(
            decode_snapshot(&bytes, "AMS", "9999")
                .err()
                .expect("错 namespace 必须拒绝")
                .to_string()
                .contains("namespace"),
        );

        let mut hash_tamper = snapshot.clone();
        hash_tamper.tree_sha256 = sha256_hex(b"not the payload");
        let tampered = encode_snapshot(&hash_tamper).expect("encode tampered");
        assert!(
            decode_snapshot(&tampered, "AMS", "1516")
                .err()
                .expect("哈希失配必须拒绝")
                .to_string()
                .contains("哈希失配"),
        );

        let mut payload_tamper = snapshot.clone();
        payload_tamper.tree_bytes[0] ^= 0xFF;
        let tampered = encode_snapshot(&payload_tamper).expect("encode tampered");
        assert!(
            decode_snapshot(&tampered, "AMS", "1516").is_err(),
            "载荷被翻位必须拒绝（哈希兜底）"
        );

        let mut entries_tamper = snapshot.clone();
        entries_tamper.entries = 3;
        let tampered = encode_snapshot(&entries_tamper).expect("encode tampered");
        assert!(
            decode_snapshot(&tampered, "AMS", "1516")
                .err()
                .expect("条目数失配必须拒绝")
                .to_string()
                .contains("条目数"),
        );

        for stale_version in [1, 3] {
            let mut version_tamper = snapshot.clone();
            version_tamper.format_version = stale_version;
            let tampered = encode_snapshot(&version_tamper).expect("encode tampered");
            assert!(
                decode_snapshot(&tampered, "AMS", "1516")
                    .err()
                    .expect("格式版本不符必须拒绝")
                    .to_string()
                    .contains("版本"),
                "上一代快照（V{stale_version}）没有水位数对，必须按不可用重建"
            );
        }
    }

    /// 快照校验失败 / 缺失：一律 Unusable（走重建），**不回落**旧「bin + sidecar」格式——
    /// 它的头没有水位数对，在 ADR-072 的判据下永远对不上。回退即红。
    #[test]
    fn an_unusable_snapshot_never_falls_back_to_the_legacy_files() {
        let source = include_str!("aabb_tree.rs");
        let body = source
            .split_once("fn read_snapshot_for_startup(")
            .expect("read_snapshot_for_startup must exist")
            .1
            .split_once("库侧空间版本号所在的固定记录")
            .expect("epoch 段随后")
            .0;
        assert!(
            body.contains("SnapshotReadOutcome::Unusable"),
            "校验失败必须判不可用: {body}"
        );
        for legacy in ["read_project_tree_file", "read_tree_meta", "TreeFileMeta", "Legacy"] {
            assert!(
                !body.contains(legacy),
                "不得再读旧格式树文件（{legacy}）: {body}"
            );
        }
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        assert!(
            production.contains("fn remove_legacy_tree_files()"),
            "旧格式文件仍要在快照发布后清掉"
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
            // 直写/durable 刷新：指针+bump 同事务，锁跨 [判定 → 事务 → 同步]
            // （源码钉在同文件的 `aabb_write_order_tests`；2026-09 自 mesh_generate.rs
            // 搬出，旧生成器退到 legacy_model feature 后面后不再写树）。
            "fast_model/aabb_refresh.rs",
            // 启动加载/指针重建与提交后收敛：本身即自愈动作，落盘自带盖章。
            "fast_model/aabb_tree.rs",
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
