//! 空间树进程态：状态机、全局空间串行锁与消费者门禁。
//!
//! 方案 `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`。
//!
//! 状态只回答一个问题：**空间消费者现在能不能安全地用 `GLOBAL_AABB_TREE`**。
//! `Ready` 的定义刻意放宽为「无未知漂移」（方案 D5）：已提交而未重放的空间意图
//! （`spatial_reconcile` pending 行）是**已知**差异，由消费者门前的重放（worker
//! 派发门 / 空闲轮）收口——staged 尾事务提交不翻状态，否则常态流量下状态高频
//! 抖动，health 的 `state` 字段成了噪音。
//!
//! 锁序（源码钉在 `batch_worker` / `mesh_generate` / 本模块测试）：
//!
//! ```text
//! STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL → GLOBAL_AABB_TREE
//! ```

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 空间树进程状态（方案 §2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpatialTreeState {
    /// 进程刚起，还没跑过启动装载。
    #[default]
    Uninitialized,
    /// 启动装载进行中。
    Loading,
    /// 树 = 某次校验通过的快照/重建产物 + 其后串行锁内的增量同步。
    Ready,
    /// 校验成功且库里可用指针为 0（全新库/清库）：消费者可运行，语义上无候选。
    ReadyEmpty,
    /// 快照可用但有已提交待重放的空间意图：重放完成前空间消费者不可运行。
    ReplayRequired,
    /// 指针重建进行中。
    Rebuilding,
    /// 库侧指纹暂时读不到，降级复用快照；等 revalidator 复检。
    DegradedReuse,
    /// 指针重建连续失败/漂移；等 revalidator 退避重试。
    DegradedBlocked,
}

impl SpatialTreeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::ReadyEmpty => "ready_empty",
            Self::ReplayRequired => "replay_required",
            Self::Rebuilding => "rebuilding",
            Self::DegradedReuse => "degraded_reuse",
            Self::DegradedBlocked => "degraded_blocked",
        }
    }

    /// 空间消费者（房间重建/重算、直接遍历树的业务查询）是否放行。
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::ReadyEmpty)
    }

    /// 当前内存树是否「经校验的产物 + 串行锁内增量」，允许**公共入口**覆盖快照文件。
    ///
    /// `ReplayRequired` 放行：启动立即重放走脏位落盘，重放刚追平的内容正是该落的。
    /// `Rebuilding` **拒绝**——重建路径的发布走内部函数不过此门；公共入口（空闲轮/
    /// 全量收尾）若在重建扫描窗口内抢发，会用新鲜指纹盖一份「重建正要替换的旧内容」
    /// 快照，这轮重建若随后漂移耗尽失败，下次启动指纹相等按 Reuse 复用，本该暴露的
    /// 漂移被洗白。`DegradedBlocked` 下树可能是空的（启动重建失败），
    /// `Loading`/`Uninitialized` 下树内容未定——覆盖快照会把残缺内容写过好文件
    /// （历史缺陷 `reconcile_persists_only_a_mutated_tree` 防的那条路，现在由
    /// 状态门统一挡）。
    pub(crate) fn allows_snapshot_publish(self) -> bool {
        matches!(self, Self::Ready | Self::ReadyEmpty | Self::ReplayRequired)
    }
}

/// 空间写入全局串行段（方案 D6）。
///
/// 持锁方（获取点见各调用处注释）：
/// - staged 提交后的空间收敛与发布（`reconcile_spatial_pending`，经
///   `STAGED_COMMIT_SERIAL` → 本锁）；
/// - direct/non-staged 指针事务到内存树同步（`mesh_generate` 直写、
///   `helper::delete_room_membership` 直写删除）；
/// - 全量指针重建的换树/发布段（分页读在锁外，D4）；
/// - 快照落盘（`persist_aabb_tree*`）与启动装载。
///
/// journal 写回与窗口尾事务**不**持本锁：尾事务不动树，崩溃安全靠 pending 行。
pub(crate) static SPATIAL_STATE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) type SpatialSerialGuard = tokio::sync::MutexGuard<'static, ()>;

/// 取空间串行锁。锁序纪律：若同时需要 `STAGED_COMMIT_SERIAL`，必须先取它；
/// 若随后要 `GLOBAL_AABB_TREE` 写锁，必须在本锁**之后**取。
pub(crate) async fn lock_spatial_serial() -> SpatialSerialGuard {
    SPATIAL_STATE_SERIAL.lock().await
}

/// 进程内状态与诊断簿记（health 曝光用）。std Mutex，绝不跨 await 持有。
#[derive(Debug, Clone)]
pub(crate) struct SpatialStateSnapshot {
    pub state: SpatialTreeState,
    pub last_error: Option<String>,
    pub last_rebuild_attempts: u32,
    pub last_verified_at_unix: Option<u64>,
    pub usable_pointer_rows: Option<u64>,
    pub invalid_pointer_rows: Option<u64>,
}

static STATE: Mutex<SpatialStateSnapshot> = Mutex::new(SpatialStateSnapshot {
    state: SpatialTreeState::Uninitialized,
    last_error: None,
    last_rebuild_attempts: 0,
    last_verified_at_unix: None,
    usable_pointer_rows: None,
    invalid_pointer_rows: None,
});

fn with_state<T>(f: impl FnOnce(&mut SpatialStateSnapshot) -> T) -> T {
    let mut cell = STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut cell)
}

pub fn current_state() -> SpatialTreeState {
    with_state(|cell| cell.state)
}

pub(crate) fn snapshot() -> SpatialStateSnapshot {
    with_state(|cell| cell.clone())
}

pub(crate) fn set_state(state: SpatialTreeState) {
    with_state(|cell| cell.state = state);
}

/// 校验成功后的收口：按树条目数落到 Ready / ReadyEmpty，盖上验证时刻并清错。
pub(crate) fn set_ready_by_entries(entries: usize) {
    with_state(|cell| {
        cell.state = if entries == 0 {
            SpatialTreeState::ReadyEmpty
        } else {
            SpatialTreeState::Ready
        };
        cell.last_error = None;
        cell.last_verified_at_unix = Some(now_unix());
    });
}

pub(crate) fn record_error(error: &str) {
    with_state(|cell| cell.last_error = Some(error.to_string()));
}

pub(crate) fn record_rebuild_attempts(attempts: u32) {
    with_state(|cell| cell.last_rebuild_attempts = attempts);
}

/// 记录最近一次全量扫描的口径统计（usable / invalid，方案 §4）。
pub(crate) fn record_scan_stats(usable: u64, invalid: u64) {
    with_state(|cell| {
        cell.usable_pointer_rows = Some(usable);
        cell.invalid_pointer_rows = Some(invalid);
    });
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 空间消费者被门禁时错误信息携带的内部错误码（方案 §6）。
pub const SPATIAL_TREE_NOT_READY: &str = "SPATIAL_TREE_NOT_READY";

/// 空间消费者入口闸：启动全量房间重建、RoomRecalc 面板/元素消费、空闲房间轮、
/// 直接遍历 `GLOBAL_AABB_TREE` 的业务查询。被拒的 durable 房间任务保留待重试。
///
/// 不拦：文件扫描/解析/入队、模型生成与指针更新、durable 空间重放、指针重建、
/// 直查数据库 `inst_relate` 的 `model.spatial.bounds`。
pub fn ensure_spatial_ready() -> anyhow::Result<()> {
    let state = current_state();
    anyhow::ensure!(
        state.is_ready(),
        "{SPATIAL_TREE_NOT_READY}: 空间树状态为 {}，空间消费者暂不可运行（等重放/重建/复检收敛）",
        state.as_str()
    );
    Ok(())
}

/// 显式测试装载模式（方案 §2 步骤 0）：live 夹具自己灌树、不走启动校验时调用。
///
/// 生产入口**永远不**调它——「内存树非空就当 preloaded」的旧短路已删除，
/// 空树校验/重建对生产是无条件的。
static FIXTURE_PRELOAD: AtomicBool = AtomicBool::new(false);

pub fn mark_spatial_tree_fixture_preloaded() {
    FIXTURE_PRELOAD.store(true, Ordering::SeqCst);
    // 夹具流程不跑装载器，状态直接置 Ready，消费者门对夹具敞开。
    set_ready_by_entries(1);
}

pub(crate) fn fixture_preload_requested() -> bool {
    FIXTURE_PRELOAD.load(Ordering::SeqCst)
}

/// env 门控崩溃注入点（方案 §8 崩溃窗口）。
///
/// `AIOS_FAILPOINT=<name>` 时命中即 abort，模拟「进程恰好死在这一点」。五个注入点：
/// `spatial_direct_after_db_commit`（直写事务提交后、树同步/摘除前）、
/// `spatial_after_tree_sync`（收敛树更新后、快照发布前）、
/// `spatial_snapshot_tmp_written`（.tmp 写完 sync 后、rename 前）、
/// `spatial_after_publish_before_ack`（快照发布后、pending 销账前）、
/// `spatial_rebuild_mid_scan`（重建分页中途）。
/// 未设 env 时只是一次环境变量查找；注入点全部位于低频路径（提交尾、落盘、
/// 重建换页），不值得为它建缓存。
pub fn failpoint(name: &str) {
    if failpoint_armed(std::env::var("AIOS_FAILPOINT").ok().as_deref(), name) {
        eprintln!("[failpoint] {name} 命中，按注入要求终止进程");
        std::process::abort();
    }
}

/// 纯判定半边：configured 必须与注入点名**精确相等**才触发。
fn failpoint_armed(configured: Option<&str>, name: &str) -> bool {
    configured == Some(name)
}

// ── 降级复检 revalidator（方案 §6）───────────────────────────────────────────

const REVALIDATE_INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);
const REVALIDATE_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(300);

/// 拉起后台降级复检任务（幂等，多次调用只 spawn 一次）。
///
/// 只管 `DegradedReuse` / `DegradedBlocked` 两态：前者重跑启动装载（库侧指纹恢复
/// 可读后按启动同款裁决收敛——降级期间的直写增量已 bump epoch，指纹失配会正确
/// 落入重建，不会静默复用旧快照）；后者重试指针重建。30s 起指数退避至 5min，
/// 恢复 Ready 后唤醒调度器。Ready↔pending 的常态往返归 worker 派发门，本任务
/// 不掺和——避免派发门、空闲轮、revalidator 三处重试抢同一件事。
pub fn spawn_spatial_revalidator() {
    static SPAWNED: std::sync::Once = std::sync::Once::new();
    SPAWNED.call_once(|| {
        tokio::spawn(revalidator_loop());
    });
}

async fn revalidator_loop() {
    let mut backoff = REVALIDATE_INITIAL_BACKOFF;
    loop {
        tokio::time::sleep(backoff).await;
        let state = current_state();
        if !matches!(
            state,
            SpatialTreeState::DegradedReuse | SpatialTreeState::DegradedBlocked
        ) {
            backoff = REVALIDATE_INITIAL_BACKOFF;
            continue;
        }
        println!("空间树降级复检开始（当前状态 {}）", state.as_str());
        let attempt = match state {
            SpatialTreeState::DegradedReuse => {
                crate::fast_model::aabb_tree::load_project_tree_verified().await
            }
            SpatialTreeState::DegradedBlocked => {
                crate::fast_model::aabb_tree::rebuild_tree_from_pointers().await
            }
            _ => unreachable!("上面刚筛过"),
        };
        match attempt {
            Ok(()) if current_state().is_ready() => {
                println!(
                    "空间树降级复检通过（{}），唤醒调度器",
                    current_state().as_str()
                );
                crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
                backoff = REVALIDATE_INITIAL_BACKOFF;
            }
            Ok(()) => {
                // 复检本身没报错但仍未 Ready（例如库侧指纹仍不可读、重新落回
                // Degraded*，或落入 ReplayRequired 交给派发门）：继续退避。
                backoff = (backoff * 2).min(REVALIDATE_MAX_BACKOFF);
            }
            Err(error) => {
                println!("空间树降级复检未通过（退避重试）: {error:#}");
                backoff = (backoff * 2).min(REVALIDATE_MAX_BACKOFF);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态字符串是 health 契约的一部分：改名 = 破坏性修改。
    #[test]
    fn state_strings_are_stable() {
        let expect = [
            (SpatialTreeState::Uninitialized, "uninitialized"),
            (SpatialTreeState::Loading, "loading"),
            (SpatialTreeState::Ready, "ready"),
            (SpatialTreeState::ReadyEmpty, "ready_empty"),
            (SpatialTreeState::ReplayRequired, "replay_required"),
            (SpatialTreeState::Rebuilding, "rebuilding"),
            (SpatialTreeState::DegradedReuse, "degraded_reuse"),
            (SpatialTreeState::DegradedBlocked, "degraded_blocked"),
        ];
        for (state, name) in expect {
            assert_eq!(state.as_str(), name);
        }
    }

    /// 消费者门：只有 Ready / ReadyEmpty 放行。
    #[test]
    fn only_ready_states_admit_consumers() {
        assert!(SpatialTreeState::Ready.is_ready());
        assert!(SpatialTreeState::ReadyEmpty.is_ready());
        for blocked in [
            SpatialTreeState::Uninitialized,
            SpatialTreeState::Loading,
            SpatialTreeState::ReplayRequired,
            SpatialTreeState::Rebuilding,
            SpatialTreeState::DegradedReuse,
            SpatialTreeState::DegradedBlocked,
        ] {
            assert!(!blocked.is_ready(), "{blocked:?} 不该放行空间消费者");
        }
    }

    /// 发布门：树内容不可信的状态不许覆盖快照文件（B3 防线的状态机等价物）。
    /// `Rebuilding` 也在拒绝之列：重建自己的发布走内部函数不过此门，公共入口在
    /// 重建扫描窗口内抢发会用新鲜指纹洗白「重建正要替换的旧内容」。
    #[test]
    fn snapshot_publish_gate_blocks_untrusted_tree_content() {
        for allowed in [
            SpatialTreeState::Ready,
            SpatialTreeState::ReadyEmpty,
            SpatialTreeState::ReplayRequired,
        ] {
            assert!(allowed.allows_snapshot_publish(), "{allowed:?} 应允许发布");
        }
        for denied in [
            SpatialTreeState::Uninitialized,
            SpatialTreeState::Loading,
            SpatialTreeState::Rebuilding,
            SpatialTreeState::DegradedReuse,
            SpatialTreeState::DegradedBlocked,
        ] {
            assert!(
                !denied.allows_snapshot_publish(),
                "{denied:?} 覆盖快照会把不可信内容写过好文件"
            );
        }
    }

    /// failpoint 只认精确同名：别名/前缀/未配置都必须安然返回。
    #[test]
    fn failpoint_arms_only_on_exact_name() {
        assert!(failpoint_armed(
            Some("spatial_after_tree_sync"),
            "spatial_after_tree_sync"
        ));
        assert!(!failpoint_armed(
            Some("spatial_after_tree_sync"),
            "spatial_snapshot_tmp_written"
        ));
        assert!(!failpoint_armed(Some("spatial"), "spatial_after_tree_sync"));
        assert!(!failpoint_armed(None, "spatial_after_tree_sync"));
    }

    /// revalidator 的边界（方案 §6）：只接管 Degraded 两态、退避有界、
    /// 恢复 Ready 才唤醒调度器。回退即红。
    #[test]
    fn revalidator_only_handles_degraded_states_with_bounded_backoff() {
        let source = include_str!("spatial_state.rs");
        let body = source
            .split_once(concat!("async fn ", "revalidator_loop("))
            .expect("revalidator_loop must exist")
            .1
            .split_once("#[cfg(test)]")
            .expect("tests follow")
            .0;
        assert!(
            body.contains("SpatialTreeState::DegradedReuse | SpatialTreeState::DegradedBlocked"),
            "只接管两个降级态: {body}"
        );
        assert!(
            body.contains("REVALIDATE_MAX_BACKOFF"),
            "退避必须有上界: {body}"
        );
        let ready_check_at = body
            .find("current_state().is_ready()")
            .expect("必须复核 Ready 才算恢复");
        let wake_at = body
            .find("BatchScheduler::global().wake()")
            .expect("恢复后唤醒调度器");
        assert!(
            ready_check_at < wake_at,
            "唤醒必须在 Ready 复核之后: {body}"
        );
        assert!(
            !body.contains("reconcile_spatial_pending"),
            "常态 pending 重放归派发门，revalidator 不掺和: {body}"
        );
    }

    /// 门禁错误必须携带稳定错误码，durable 消费方按它识别「保留待重试」。
    #[test]
    fn gate_error_carries_the_stable_error_code() {
        with_state(|cell| cell.state = SpatialTreeState::Rebuilding);
        let error = ensure_spatial_ready().expect_err("Rebuilding 必须被门禁");
        assert!(error.to_string().contains(SPATIAL_TREE_NOT_READY));
        with_state(|cell| cell.state = SpatialTreeState::Ready);
        ensure_spatial_ready().expect("Ready 必须放行");
        // 复位，避免同进程其他测试受串扰。
        with_state(|cell| cell.state = SpatialTreeState::Uninitialized);
    }
}
