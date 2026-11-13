//! 部署面板的 stack 代理（服务端半边，设计 REMOTE-ACCESS-DESIGN.md 第 3、4、6 节）。
//!
//! - [`AgentBackend`]：数据来源接口，和面板的 `StackDriver` 一一对上（见 `backend.rs` 的对照表）。
//! - [`run_agent`] / [`start_agent`]：HTTP/1.1 服务层（`httparse` + tokio 手写，不开 hyper 的 server 特性）：
//!   Bearer 鉴权（常量时间比较）、快照长轮询、每来源 5,000 行日志环、任务表（落盘、重启读回）、审计。
//! - [`mock::MockBackend`]：模拟数据（照 deploy-dash `data\mock.rs` 的场景和故障注入抄的逻辑）。
//! - [`install`]：`open-firewall`、`install-autostart`、`show-token`、`rotate-token`。
//!
//! 协议类型在 `deploy-dash-proto`（`schema = 1`），从这里 re-export 成 [`proto`]。

pub mod audit;
pub mod backend;
pub mod config;
pub mod fsx;
pub mod http;
pub mod install;
pub mod jobs;
pub mod mock;
pub mod ring;
pub mod server;
pub mod stack_cli;
pub mod stackcfg;
pub mod state;
pub mod svc_upgrade;
pub mod sys;
pub mod tail;
pub mod timefmt;
pub mod token;
pub mod upgrade_apply;
pub mod upgrade_http;
pub mod upgrade_key;
pub mod upgrade_run;
pub mod upgrade_store;

pub use backend::*;
pub use config::AgentConfig;
pub use deploy_dash_proto as proto;
pub use server::{AgentHandle, AlreadyRunning, start_agent};

/// 代理版本（`/v1/ping` 的 `agent_version`）。
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 起代理，一直跑到进程被结束。阻塞当前线程。
///
/// 依次：端口不在保护名单里（含 stack.toml `[guard] protect_ports`）→ 单实例互斥量（已有一个在跑 → 返回
/// [`AlreadyRunning`]，调用方以退出码 2 退出）→ 起 tokio（2 个工作线程）→ [`start_agent`]。
/// 状态随时落盘（任务结果、审计），所以被直接结束（Ctrl+C、任务管理器）也不丢东西；服务归 stack 管，照常跑。
pub fn run_agent(config: AgentConfig, backend: Box<dyn AgentBackend>) -> anyhow::Result<()> {
    let protect = config::stack_protect_ports(&config.stack_toml_path());
    config.check_listen(&protect)?;
    let _guard = if config.single_instance {
        match sys::acquire_single_instance(&sys::instance_mutex_name(&config.root))? {
            Some(g) => Some(g),
            None => return Err(AlreadyRunning.into()),
        }
    } else {
        None
    };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("deploy-agent").build()?;
    rt.block_on(async move {
        let handle = start_agent(config, backend).await?;
        handle.wait().await;
        Ok(())
    })
}
