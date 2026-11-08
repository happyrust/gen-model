//! 分组写侧的任务体：把 HTTP 层交来的一批 [`GroupWriteOp`] 走完
//! [`validate_ops`](super::group_write::validate_ops) →
//! [`plan_stages`](super::group_write::plan_stages) →
//! [`group_exec::execute`](super::group_exec::execute)，交回一份可序列化的报告。
//!
//! 分工与三件套一致：这里只负责「把库和模板交到执行器手上」，校验与执行的口径都在
//! `group_write` / `group_exec` 里。执行器是同步的、`Transaction` 不是 `Send`，所以整条
//! 链路（含读库校验）都在 [`tokio::task::spawn_blocking`] 里跑一次。
//!
//! 报告里的 `index` 一律是**原批次下标**（`plan_stages` 保留了下标），不是阶段内序号——
//! 调用方拿它回指自己提交的第几条 op。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::direct_store::DirectSchema;
use super::direct_tree::{DirectTreeService, TreeStore};
use super::group_exec;
use super::group_write::{GroupWriteOp, Skipped, ValidatedBatch, plan_stages, validate_ops};

/// 一批分组写的结果：已提交的库、建出来的容器、写进去的成员、跳过的条目与错误。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GroupWriteReport {
    /// 已提交的库号，按提交顺序。跨库没有两阶段提交，失败即停，这里就是「真落了」的那几个库。
    pub committed: Vec<u32>,
    /// `(原批次下标, 建出来的 refno 的 pdms 串)`。
    pub created: Vec<(usize, String)>,
    /// `(原批次下标, GPITEM 的 refno 的 pdms 串, 是否新建)`；`inserted = false` 表示目标早已在组里。
    pub added: Vec<(usize, String, bool)>,
    /// 放行但执行时判定可跳过的条目：`(原批次下标, 原因)`，原因与读侧坏槽同一套口径。
    pub skipped: Vec<(usize, &'static str)>,
    /// `(出错的那条 op，库号，错误文本)`；不属于某一条 op 的失败（打开库 / 提交）下标为 `None`。
    pub errors: Vec<(Option<usize>, u32, String)>,
}

/// 跑完一批分组写。
///
/// `tree` 是 HTTP 层从 `AppState::direct_tree` 拿到的句柄：它的 `metadata()` 给出库号 → 库文件
/// 映射（`db_file` 闭包的来源），它的 `tree_store()` 给出校验要读的那只视图。`template_dir`
/// 里要有设计库模板 `desvir.dat`。
///
/// 校验不过整批不写，返回带逐条原因的 `Err`；校验通过之后才开写，写侧的失败进
/// [`GroupWriteReport::errors`]（调用方据此判 `partial`）。
pub async fn run(
    tree: std::sync::Arc<DirectTreeService>,
    template_dir: &Path,
    ops: Vec<GroupWriteOp>,
) -> anyhow::Result<GroupWriteReport> {
    let template_dir = template_dir.to_path_buf();
    tokio::task::spawn_blocking(move || run_blocking(&tree, &template_dir, ops))
        .await
        .map_err(|error| anyhow::anyhow!("分组写任务异常结束: {error}"))?
}

/// [`run`] 的同步主体：读库校验、拆阶段、执行，全在阻塞线程里做完。
fn run_blocking(
    tree: &DirectTreeService,
    template_dir: &Path,
    ops: Vec<GroupWriteOp>,
) -> anyhow::Result<GroupWriteReport> {
    // 校验必须读**树服务钉住的那只**视图，不能另取一次 `MdbReadProvider::shared(..).view()`：
    // `view()` 不是纯读——它每次都会对已打开的源比 `is_current()`，文件变了就整只换掉
    // `current`（`mdb_read_store.rs:308-322`）。树服务是在 `open` 时取视图、之后一直钉着
    // （`direct_tree.rs:130`），两次 `view()` 之间只要 E3D 存过一次盘，校验看到的就是另一个
    // sesno，而写侧仍按树服务记下的库文件写——校验结论与实际要写的库对不上。
    let view = tree.tree_store();
    let (ops, skipped) = match validate_ops(view.as_ref() as &dyn TreeStore, &ops)? {
        ValidatedBatch::Ready { ops, skipped } => (ops, skipped),
        ValidatedBatch::Rejected(rejections) => {
            let detail = rejections
                .iter()
                .map(|rejection| format!("ops[{}]={}", rejection.index, rejection.reason.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "写前校验拒绝整批（{} 条不通过，一条都没写）：{detail}",
                rejections.len()
            );
        }
    };
    let stages = match plan_stages(&ops) {
        Ok(stages) => stages,
        Err(rejection) => anyhow::bail!(
            "拆执行阶段失败：ops[{}]={}",
            rejection.index,
            rejection.reason.as_str()
        ),
    };
    let db_file: HashMap<u32, PathBuf> = tree
        .metadata()?
        .session_fingerprint()
        .into_iter()
        .map(|(dbnum, file, _sesno)| (dbnum, file))
        .collect();
    let report = group_exec::execute(
        template_dir,
        move |dbnum| db_file.get(&dbnum).cloned(),
        &stages,
        &skipped,
    )?;
    Ok(GroupWriteReport {
        committed: report.committed,
        created: report
            .created
            .into_iter()
            .map(|created| (created.index, created.refno.to_pdms_str()))
            .collect(),
        added: report
            .added
            .into_iter()
            .map(|added| (added.index, added.item.to_pdms_str(), added.inserted))
            .collect(),
        skipped: report
            .skipped
            .iter()
            .map(|skipped: &Skipped| (skipped.index, skipped.reason.as_str()))
            .collect(),
        errors: report
            .errors
            .into_iter()
            .map(|error| (error.index, error.stage_dbnum, error.error))
            .collect(),
    })
}

/// 模板目录：与读侧同一份（`DirectSchema::template_dir_from_env`，进程内缓存一次）。
pub fn template_dir() -> anyhow::Result<&'static Path> {
    DirectSchema::template_dir_from_env()
}
