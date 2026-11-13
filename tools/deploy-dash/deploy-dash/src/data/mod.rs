//! 数据层接口（DASHBOARD-DESIGN 14.4）。界面只认事件和命令。
//! 两个实现：`mock.rs` 模拟数据（自检、截图用）；`stack.rs` 真实服务——起停、生成都跑离线包里的 stack.ps1，
//! 状态由面板自己看端口、健康接口、进程和日志文件。
pub mod mock;
pub mod stack;
#[cfg(feature = "remote")] pub mod remote;
/// 自动升级的数据层（设计 7.2）：预览、传、逐台应用。和 `remote` 一样只在 `remote` 特性下编。
#[cfg(feature = "remote")] pub mod upgrade;

use crate::events::UiEvent;
use crate::model::{ProjectForm, Svc};

pub trait StackDriver: Send {
    /// 起后台线程，按节奏推事件：快照、指标、后端任务、日志、任务进度。
    fn start(&mut self, events: async_channel::Sender<UiEvent>);
    /// 发一个命令，立刻返回；结果以事件回来（JobStarted / JobFinished / Notice …）。
    fn send(&self, cmd: Command);
    /// 面板退出时调用。`keep_services` 为真时服务留着（DESIGN-REVIEW 乙15 的缺省）。
    fn shutdown(&mut self, keep_services: bool);
    /// 显示在顶栏和状态栏的数据来源说明，例如「模拟数据」。
    fn source_label(&self) -> &'static str;
    /// 真实服务（stack）还是模拟数据。界面据此藏起只有模拟才有的菜单项。
    fn is_real(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub enum Command {
    Up,
    Down { force: bool },
    StartService(Svc),
    StopService { svc: Svc, force: bool },
    RestartService(Svc),
    /// 生成模型：只给点名的库（= `stack init <库号…>`：解析进数据库并生成模型，后端里分不开）。
    Init(Vec<u32>),
    Doctor,
    Configure(ProjectForm),
    ListDbnums,
    PreviewUpdate(Vec<u32>),
    IncrementalUpdate(Vec<u32>),
    /// 增量同步开（恢复出队）/ 关（暂停出队）：后端 `POST /api/v1/queue/resume|pause`。
    SetIncrements(bool),
    RefreshNow,
    /// 只有模拟实现认。
    InjectFault(Fault),
    /// 只有模拟实现认：切到一个固定场景（自检截图用，数值可重现）。
    Fixture(Fixture),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// rs-server 进程退出（现在）。
    RsServerExit,
    /// 「现场故障」：rs-server 90 秒前退出、磁盘 15 分钟里从 15 GB 掉到 12 GB、后端有 2 条失败。
    FieldIncident,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixture {
    Unconfigured,
    Stopped,
    /// 已经稳定运行 2 小时 14 分，带 15 分钟健康历史。
    RunningHealthy,
}
