use aios_core::accel_tree::acceleration_tree::RStarBoundingBox;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefU64, RefnoEnum, gen_bytes_hash, get_inst_relate_keys};
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::Point;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use surrealdb::sql::Thing;

use crate::fast_model::utils;
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: Thing,
    pub refno: RefnoEnum,
    pub noun: String,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    #[serde(deserialize_with = "deserialize_transform_flexible")]
    pub world_trans: Transform,
    /// 更新前已存在的包围盒。`rstar` 的 `remove` 按整值相等匹配，拿新值删不掉旧条目，
    /// 只有带上它才能把 R 树里的旧条目清干净（ADR-010 D3）。
    #[serde(default, deserialize_with = "deserialize_optional_aabb_flexible")]
    pub old_aabb: Option<Aabb>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeoAabbTrans {
    #[serde(deserialize_with = "deserialize_transform_flexible")]
    pub trans: Transform,
    #[serde(deserialize_with = "deserialize_aabb_flexible")]
    pub aabb: Aabb,
    /// `PrimLoft` 圆弧扫掠角（弧度）。直线扫掠 / 非 loft 为 none，走盒子 8 角变换。
    #[serde(default, deserialize_with = "deserialize_optional_f32_flexible")]
    pub revolve_sweep: Option<f32>,
    /// 直线扫掠时 Surreal 把缺失的 `SpineArc.clock_wise` 填成 null，不能当 `bool`。
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub revolve_cw: bool,
}

/// SurrealDB preserves the numeric kind of stored array members. Geometry
/// records written with an integral coordinate therefore come back as `i64`,
/// while Bevy/glam and parry derive strict `f32` deserializers. Accept every
/// finite JSON/Surreal numeric representation at this database boundary and
/// normalize it to the engine's `f32` scalar.
#[derive(Debug, Clone, Copy)]
struct FlexibleF32(f32);

impl<'de> serde::Deserialize<'de> for FlexibleF32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = FlexibleF32;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an integer or floating-point coordinate")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value as f32))
            }

            fn visit_f32<E>(self, value: f32) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(FlexibleF32(value))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

#[derive(serde::Deserialize)]
struct AabbWire {
    mins: [FlexibleF32; 3],
    maxs: [FlexibleF32; 3],
}

impl AabbWire {
    fn into_aabb(self) -> Aabb {
        let [min_x, min_y, min_z] = self.mins;
        let [max_x, max_y, max_z] = self.maxs;
        Aabb::new(
            Point::new(min_x.0, min_y.0, min_z.0),
            Point::new(max_x.0, max_y.0, max_z.0),
        )
    }
}

#[derive(serde::Deserialize)]
struct TransformWire {
    translation: [FlexibleF32; 3],
    rotation: [FlexibleF32; 4],
    scale: [FlexibleF32; 3],
}

impl TransformWire {
    fn into_transform(self) -> Transform {
        let [tx, ty, tz] = self.translation;
        let [rx, ry, rz, rw] = self.rotation;
        let [sx, sy, sz] = self.scale;
        Transform {
            translation: glam::Vec3::new(tx.0, ty.0, tz.0),
            rotation: glam::Quat::from_array([rx.0, ry.0, rz.0, rw.0]),
            scale: glam::Vec3::new(sx.0, sy.0, sz.0),
        }
    }
}

pub(crate) fn deserialize_aabb_flexible<'de, D>(deserializer: D) -> Result<Aabb, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <AabbWire as serde::Deserialize>::deserialize(deserializer).map(AabbWire::into_aabb)
}

fn deserialize_optional_aabb_flexible<'de, D>(deserializer: D) -> Result<Option<Aabb>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<AabbWire> as serde::Deserialize>::deserialize(deserializer)
        .map(|value| value.map(AabbWire::into_aabb))
}

fn deserialize_optional_f32_flexible<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<FlexibleF32> as serde::Deserialize>::deserialize(deserializer)
        .map(|value| value.map(|v| v.0))
}

fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(<Option<bool> as serde::Deserialize>::deserialize(deserializer)?.unwrap_or(false))
}

pub(crate) fn deserialize_transform_flexible<'de, D>(deserializer: D) -> Result<Transform, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <TransformWire as serde::Deserialize>::deserialize(deserializer)
        .map(TransformWire::into_transform)
}

/// 一个元素的包围盒确实变了。
///
/// `noun` 决定它进哪条房间分支（ADR-010 §2）：PANE 自己一动，整间房的成员全变，
/// 元素级表达不了，必须整块面板重算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AabbChange {
    pub refno: RefnoEnum,
    pub noun: String,
}

/// Return explicit targets that currently have a usable AABB.
///
/// A post-regeneration action cannot use `GLOBAL_AABB_TREE` as its old baseline:
/// the root generator may already have synchronized the new box into that tree. The
/// action itself proves a pose changed in this window, so geometry existence is the
/// final gate that excludes ANCI and other no-geometry nouns.
pub async fn existing_geometric_aabb_changes(
    refnos: &[RefnoEnum],
) -> anyhow::Result<Vec<AabbChange>> {
    #[derive(serde::Deserialize)]
    struct Row {
        refno: RefnoEnum,
        noun: String,
    }

    let mut changes = Vec::new();
    for chunk in refnos.chunks(100) {
        if chunk.is_empty() {
            continue;
        }
        let keys = get_inst_relate_keys(chunk);
        let mut response = crate::data_interface::staging::active_data_db()
            .query(format!(
                "SELECT in AS refno, in.noun AS noun FROM {keys} WHERE aabb.d != NONE;"
            ))
            .await?
            .check()?;
        changes.extend(
            response
                .take::<Vec<Row>>(0)?
                .into_iter()
                .map(|row| AabbChange {
                    refno: row.refno,
                    noun: row.noun,
                }),
        );
    }
    changes.sort_by_key(|change| change.refno);
    changes.dedup_by_key(|change| change.refno);
    Ok(changes)
}

///刷新inst_relate 的 aabb
/// 更新实例关联的包围盒数据
///
/// # 参数
///
/// * `refnos` - 参考号数组
/// * `replace_exist` - 是否替换已存在的包围盒数据
///
/// # 返回值
///
/// 包围盒**确实变了**的那些元素。房间归属的触发源就是它（ADR-010 §4）。
///
/// 变更基线取**空间树上的旧值**而不是行内的 `old_aabb`：定向重生成走的是「先删行再
/// 重插」（`save_instance_data(replace_exist)`），行内旧值在刷新时刻恒为 none 或者
/// 恒等于刚插入的新值，拿它作基线会退化成「根下每个元素每次重生成都算变」；树上的
/// 条目跨过删行重插存活，才是房间系统上一次真正看到的状态。树上没有条目（首次见到）
/// 同样算变——房间系统从没算过它，正需要一次回填。
///
/// 新值优先从 geo 侧重算；重算不出（`geo_aabbs` 为空或不可用）而行内有既有指针的，
/// 以指针值为准——隐含直管段（TUBI/BOXI）的 aabb 由生成层在插入时算好，geo 侧的
/// 共享单位几何没有 `aabb`/`pts`，此前这类行被整体跳过，从未进过空间树，也就从未
/// 参与过房间归属。
///
/// 普通入口仍只返回 AABB 变更集；定向增量入口则保守返回全部实际有几何的目标。
/// 两者不可混用：全量重刷按处理集入队会制造全库房间任务，而定向生成若仍只看 AABB，
/// 内部网格/布尔结果或对称位姿变化会静默漏掉（ADR-040）。
pub async fn update_inst_relate_aabbs_by_refnos(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    update_inst_relate_aabbs_by_refnos_mode(refnos, replace_exist, false).await
}

/// 定向增量刷新入口。
///
/// 与普通刷新的差别只剩两点。其一，直写路径把 `model_update_pending` 房间任务也放进
/// 那个事务——全量生成本来就以 `build_room_relations` 的整体重建收尾，逐元素排房间
/// 任务等于给每个元素排一次重算。其二，写锁从**读输入之前**就取：本入口的调用方
/// （定向 regen 与 TransformOnly）会对同一个 refno 反复刷新，锁只跨事务的话，两次
/// 刷新可以先后算出 A、B 再按 B、A 的顺序落树，把陈旧的 A 发布在最后。
///
/// 两条路径共有的部分（指针写与 spatial epoch bump 同事务、事务成功后才推进
/// `GLOBAL_AABB_TREE`、锁跨 [判定 → 事务 → 同步]）不因入口而异。暂存路径一律不提前
/// 发布控制面任务，仍由窗口尾事务统一收口。
pub async fn update_inst_relate_aabbs_by_refnos_incremental(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    update_inst_relate_aabbs_by_refnos_mode(refnos, replace_exist, true).await
}

/// 「按 refno 取树上旧条目」那一步的累计观测（specs/026 T02，ADR-045 的前置量化）。
///
/// 这一步今天遍历整棵 `GLOBAL_AABB_TREE`，而一个生成根要问两遍（布尔前刷一次、布尔后
/// 按最终关系再刷一次）。初始化时树是边生成边长大的，于是它的总代价对库规模是平方级。
/// 动手改之前先把它单独量出来：AABB落库 那一段里还有记录落库、带房间 upsert 与 epoch
/// bump 的事务、以及 `sync_refnos`，大头未必在这儿。没有这组数，改完无从归因。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct StaleLookupStats {
    pub micros: u64,
    pub calls: u64,
    /// 观测窗口内见过的最大树尺寸。平方项成不成立，就看它随批次怎么长。
    pub max_tree_entries: u64,
}

static STALE_LOOKUP_MICROS: AtomicU64 = AtomicU64::new(0);
static STALE_LOOKUP_CALLS: AtomicU64 = AtomicU64::new(0);
static STALE_LOOKUP_MAX_TREE: AtomicU64 = AtomicU64::new(0);

fn note_stale_lookup(elapsed: std::time::Duration, tree_entries: usize) {
    STALE_LOOKUP_MICROS.fetch_add(elapsed.as_micros() as u64, Ordering::Relaxed);
    STALE_LOOKUP_CALLS.fetch_add(1, Ordering::Relaxed);
    STALE_LOOKUP_MAX_TREE.fetch_max(tree_entries as u64, Ordering::Relaxed);
}

/// 取走并清零。调用方按自己的观测窗口结算——一次
/// `process_meshes_update_db_deep_with_policy` 就是一个窗口。
pub(crate) fn take_stale_lookup_stats() -> StaleLookupStats {
    StaleLookupStats {
        micros: STALE_LOOKUP_MICROS.swap(0, Ordering::Relaxed),
        calls: STALE_LOOKUP_CALLS.swap(0, Ordering::Relaxed),
        max_tree_entries: STALE_LOOKUP_MAX_TREE.swap(0, Ordering::Relaxed),
    }
}

async fn update_inst_relate_aabbs_by_refnos_mode(
    refnos: &[RefnoEnum],
    replace_exist: bool,
    durable_room_trigger: bool,
) -> anyhow::Result<Vec<AabbChange>> {
    const CHUNK: usize = 100;
    let mut changes = Vec::new();
    for chunk in refnos.chunks(CHUNK) {
        if chunk.is_empty() {
            continue;
        }
        // The durable direct path must serialize before it reads the geometry /
        // transform inputs. Taking the lock only after `new_boxes` was computed
        // allowed two refreshes of the same refno to calculate A then B, acquire
        // the lock in reverse order, and publish stale A last. The plain direct
        // path takes the same locks further down, once the expensive input read
        // is behind it.
        //
        // 锁序（一致性闭环方案 D6）：SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE。
        // 空间串行锁把直写路径与 staged 提交后收敛、指针重建换树段、快照发布
        // 串成一条线；声明顺序保证释放顺序相反（先还树锁再还串行锁）。
        let mut _direct_serial = None;
        let mut direct_tree = None;
        if durable_room_trigger {
            _direct_serial = Some(crate::fast_model::spatial_state::lock_spatial_serial().await);
            direct_tree = Some(GLOBAL_AABB_TREE.write().await);
        }
        let mut rstar_objs = Vec::new();
        let inst_keys = get_inst_relate_keys(chunk);
        let mut sql = format!(
            r#"select id, in as refno, world_trans.d as world_trans, in.noun as noun, aabb.d as old_aabb,
            (select out.aabb.d as aabb, trans.d as trans,
             out.param.PrimLoft.path.SpineArc.angle as revolve_sweep,
             out.param.PrimLoft.path.SpineArc.clock_wise as revolve_cw
             from out->geo_relate where out.aabb.d != none and trans.d != none)
            as geo_aabbs from {inst_keys} where world_trans.d != none"#,
        );
        //替换所有的aabb
        if !replace_exist {
            sql.push_str(" and aabb=none");
        }
        // 失败即中止，整批上抛（与 persist_latest_main_data 同一纪律）：本函数所有写入
        // 幂等，调用方把整批当一个任务结算、重放收敛。此前这里是 `.unwrap()` + 反序列化
        // 失败静默 continue——传输抖动直接 panic（同款 panic 在生产日志有实证，os error
        // 10054），坏一块就无声丢掉 100 个元素的包围盒与房间触发。
        let db = crate::data_interface::staging::active_data_db();
        let mut response = db
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("查询 inst_relate 包围盒输入失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("查询 inst_relate 包围盒输入语句失败: {e}"))?;
        let result: Vec<QueryAabbParam> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析 inst_relate 包围盒输入失败: {e}"))?;
        let chunk_aabbs: DashMap<String, Aabb> = DashMap::new();
        let mut update_sql = String::new();
        // 本块每行的「当前真值」，树同步与变更判定共用。
        let mut new_boxes: Vec<(RefnoEnum, String, Aabb)> = Vec::new();
        for r in result {
            let mut computed = Aabb::new_invalid();
            for g in r.geo_aabbs {
                let t = r.world_trans * g.trans;
                let tmp_aabb = if let Some(sweep) = g.revolve_sweep.filter(|s| s.abs() > 1e-6) {
                    crate::fast_model::shared::aabb_z_revolve_apply_transform(
                        &g.aabb,
                        &t,
                        sweep,
                        g.revolve_cw,
                    )
                } else {
                    crate::fast_model::shared::aabb_apply_transform(&g.aabb, &t)
                };
                computed.merge(&tmp_aabb);
            }
            let magnitude = computed.extents().magnitude();
            let new_box = if magnitude.is_nan() || magnitude.is_infinite() {
                // geo 侧重算不出。有既有指针的（隐含直管段这类插入时写死 aabb 的行）
                // 以指针值为当前真值；两头都没有的才是真的无几何可用，跳过。
                match r.old_aabb {
                    Some(existing) => existing,
                    None => {
                        #[cfg(feature = "debug_model")]
                        dbg!("Found nan aabb");
                        continue;
                    }
                }
            } else {
                // 只有重算出来的值需要写库；指针回退的那条本来就是库里现值
                // （TUBI 这类建行写死 aabb 的行，aabb_d 也在建行时一并写过）。
                // aabb_d 与指针同语句原子写（P4 写时物化）：值在内存，渲染
                // 纯字面量，journal 维持纯数据。
                let aabb_hash = gen_bytes_hash::<_, 64>(&computed).to_string();
                let aabb_json = serde_json::to_string(&computed)
                    .map_err(|e| anyhow::anyhow!("序列化 Aabb 失败: {e}"))?;
                chunk_aabbs.entry(aabb_hash.clone()).or_insert(computed);
                update_sql.push_str(&format!(
                    "update {} set aabb = aabb:⟨{}⟩, aabb_d = {};",
                    r.refno.to_inst_relate_key(),
                    aabb_hash,
                    aabb_json,
                ));
                computed
            };
            rstar_objs.push(RStarBoundingBox::new(new_box, r.refno, r.noun.clone()));
            new_boxes.push((r.refno, r.noun, new_box));
        }
        // 变更必须在任何指针写入、内存树推进之前按旧树判定。直写事务若随后失败，
        // 指针和树都还留在旧基线，原模型任务重试时仍能再次得到同一批房间目标。
        let target_refnos = new_boxes
            .iter()
            .map(|(refno, _, _)| refno.refno())
            .collect::<HashSet<_>>();
        // 普通直写分支在这里补上写锁，跨度是 [变更判定 → 事务 → 树同步]。空闲轮否则
        // 可能在「DB epoch 已递增、树尚未同步」的极窄窗口把旧树盖上新 epoch sidecar；
        // 并发的删除清理也会挤进事务与同步之间，让刚摘掉的条目又被同步回树上。刻意
        // 不把它提前到读输入段——那一段含几何 join，是全量生成里最贵的部分，而镜像
        // 一致性只要求「要不要 bump」与「树到底动没动」由同一个加锁快照裁决。
        // durable 增量的锁更早（读输入之前就取，见上），这里只接管普通直写分支。
        // 锁序同上：先空间串行锁、后树写锁。
        if direct_tree.is_none() {
            _direct_serial = Some(crate::fast_model::spatial_state::lock_spatial_serial().await);
            direct_tree = Some(GLOBAL_AABB_TREE.write().await);
        }
        let t_stale = std::time::Instant::now();
        let (stale_by_refno, stale_tree_entries) = if let Some(tree) = direct_tree.as_ref() {
            let mut stale = HashMap::<RefU64, Vec<Aabb>>::new();
            for old in tree.iter().filter(|old| target_refnos.contains(&old.refno)) {
                stale.entry(old.refno).or_default().push(old.aabb);
            }
            (stale, tree.size())
        } else {
            // 暂存窗口这一轮不动树，读一次持久主库的旧基线就够：窗口内的变化寄存进
            // 上下文，提交后由 `sync_tree_from_committed_pointers` 按已提交指针收敛。
            let tree = GLOBAL_AABB_TREE.read().await;
            let mut stale = HashMap::<RefU64, Vec<Aabb>>::new();
            for old in tree.iter().filter(|old| target_refnos.contains(&old.refno)) {
                stale.entry(old.refno).or_default().push(old.aabb);
            }
            (stale, tree.size())
        };
        note_stale_lookup(t_stale.elapsed(), stale_tree_entries);
        let mut aabb_change_count = 0usize;
        let chunk_changes = new_boxes
            .iter()
            .filter_map(|(refno, noun, new_box)| {
                let olds = stale_by_refno
                    .get(&refno.refno())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let aabb_changed = tree_box_changed(olds, new_box);
                aabb_change_count += usize::from(aabb_changed);
                room_target_required(aabb_changed, durable_room_trigger).then(|| AabbChange {
                    refno: *refno,
                    noun: noun.clone(),
                })
            })
            .collect::<Vec<_>>();
        // 定向增量的输入已经是本次真正重写/变换的目标，不是全库候选。房间判定消费
        // 最终 mesh 与变换而不只消费 AABB；因此即使盒子逐位相等，也必须留下 durable
        // 房间目标和 epoch 痕迹。重试仍会无条件得到同一集合，关闭了“几何已写、任务未写”
        // 崩溃后因前后看起来相同而漏触发的窗口。
        let same_aabb_geometry = chunk_changes.len().saturating_sub(aabb_change_count);
        if same_aabb_geometry > 0 {
            println!(
                "[房间增量] 本块 {} 个定向几何目标 AABB 未变，仍保守排入房间重算并推进空间 epoch",
                same_aabb_geometry
            );
        }

        // aabb 记录先落库、指针后落库（与 trans 记录同一条 D9 教训，方向不能反）：
        // 反过来的话，两条语句之间的并发读者与中途崩溃都会看到指向缺位记录的指针，
        // `aabb.d` 为 none，元素从 `where aabb.d != none` 的所有读者里整条消失。
        utils::save_aabb_to_surreal(&chunk_aabbs).await?;

        if !chunk_changes.is_empty() {
            // 本块确有包围盒变化或定向几何变化 → 指针写与 epoch bump 必须同事务。
            // 直写路径不产生 `spatial_reconcile` 意图行，epoch 是它在库侧留下的**唯一**
            // 痕迹：少 bump 一次，落盘前崩溃的重启就会看到 sidecar 与库指纹相等、按
            // Reuse 复用一棵陈旧的树，而 /health 的 drift 恒为 false，没有人看得见。
            // 关掉房间增量、或走非定向的全量生成，都只摘掉 room_recalc 这一条语句。
            let room_upserts =
                (durable_room_trigger && crate::options::room_incremental()).then(|| {
                    crate::data_interface::model_update_pending::render_room_recalc_upserts(
                        &chunk_changes,
                    )
                });
            let mut statements = Vec::with_capacity(3);
            if !update_sql.is_empty() {
                statements.push(update_sql.clone());
            }
            if let Some(room_upserts) = room_upserts {
                statements.push(room_upserts);
            }
            statements.push(crate::fast_model::aabb_tree::render_spatial_epoch_bump());
            let transaction =
                crate::data_interface::increment_pipeline::wrap_in_transaction(&statements)
                    .expect("直写 AABB 事务至少包含 epoch bump");
            crate::surreal_retry::execute_surreal_checked(
                &transaction,
                "update inst_relate aabb pointers with spatial epoch bump",
            )
            .await?;
        } else if !update_sql.is_empty() {
            // 普通（非定向）重刷且盒子逐位相等：库侧「树应有内容」没变，不 bump。
            crate::surreal_retry::execute_model_write(
                &update_sql,
                "update inst_relate aabb pointers",
            )
            .await?;
        }

        // 崩溃窗口 ①（一致性闭环方案 §8）：DB 事务已提交、内存树未同步。epoch 已
        // 随事务 bump，重启判据必然认出指纹失配并走指针重建。
        crate::fast_model::spatial_state::failpoint("spatial_direct_after_db_commit");

        // 内存树只在本块 DB 写入全部成功后才动：失败块不留「树新库旧」的半掺状态。
        // sync_refnos 一次遍历摘掉这些 refno 的全部旧条目（含历史堆叠的重复）并插入新值。
        let tree = direct_tree
            .as_mut()
            .expect("直写分支必须持有写锁直到树同步结束");
        tree.sync_refnos(rstar_objs.clone());
        if !rstar_objs.is_empty() || !stale_by_refno.is_empty() {
            crate::fast_model::aabb_tree::mark_aabb_tree_dirty();
        }
        drop(direct_tree);
        changes.extend(chunk_changes);
    }

    // aabb 一到位，行才够格进 insts_flat 清扫（谓词含 `aabb.d != none`）：置脏，
    // 空闲轮收口（P4 写时物化）。
    crate::fast_model::pdms_inst::mark_insts_flat_dirty();

    Ok(changes)
}

/// 「这个元素的包围盒相对房间系统上一次看到的状态变了吗」的唯一判据（纯函数）。
///
/// 不变的唯一形态是：树上恰有一条旧条目且与新值逐位相等。没有旧条目是「首次见到」
/// ——房间从没算过它，必须回填；多于一条是历史堆叠的残留——状态本身已经坏了，
/// 重算一次才能收敛。
pub(crate) fn tree_box_changed(old_entries: &[Aabb], new_box: &Aabb) -> bool {
    !(old_entries.len() == 1 && old_entries[0] == *new_box)
}

/// 定向增量已经由工作计划把范围缩到真实重写目标，因此它的几何变化不能再由 AABB
/// 是否变化代替；普通全量/维护刷新仍只认 AABB，避免制造全库房间任务。
pub(crate) fn room_target_required(aabb_changed: bool, durable_incremental: bool) -> bool {
    aabb_changed || durable_incremental
}
