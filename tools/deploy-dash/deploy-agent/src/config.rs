//! 代理配置：`<根>\run\gui\agent.toml`（设计第 6 节、R1）。缺省只听 `127.0.0.1:8600`，要远程必须显式改。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use deploy_dash_proto as proto;
use serde::Deserialize;

/// 代理运行时的全部配置。路径都从 `root` 推出来。
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// 安装根（`stack.ps1`、`stack.toml` 所在的目录）。
    pub root: PathBuf,
    /// 监听地址；缺省 `127.0.0.1:8600`。
    pub listen: SocketAddr,
    /// 对外地址（可空）。配了就用它的主机名拼快照里的 `urls`，不看请求的 `Host` 头。
    pub public_url: String,
    /// 每个日志来源的环形缓冲行数。
    pub log_ring_lines: usize,
    /// 审计文件满多少字节改名 `.1`。
    pub audit_rotate_bytes: u64,
    /// 每个 401 的基本延迟。
    pub unauthorized_delay: Duration,
    /// 同一来源一分钟里 401 超过这么多次，就对它延迟 `unauthorized_slow_delay`。
    pub unauthorized_burst: u32,
    pub unauthorized_slow_delay: Duration,
    /// 长轮询最多挂多久。
    pub long_poll_max: Duration,
    /// 远程「浏览文件夹」的白名单根（第二版的 `/v1/fs/dirs` 用；这一版只写进 `/v1/config`）。
    pub browse_roots: Vec<String>,
    /// 日志行里含这些字样（不分大小写）就整行隐去再发出去。
    pub redact_keywords: Vec<String>,
    /// 按安装根取名的单实例互斥量；正式身份和开发 bin 是 true，单测是 false。
    pub single_instance: bool,
    /// 起来时给令牌文件设 ACL（只给当前账户）；单测可以关掉省时间。
    pub protect_token_file: bool,
    /// 代理自己的日志行同时打到标准输出（开发 bin 用）。
    pub echo_log: bool,
    /// 代理自己写 `<根>\logs\gui\agent.log`（满 10 MB 改名 `.1`）。嵌进 deploy-dash、由它的 tracing 写这个文件时关掉，
    /// 免得两边同时写、一边改名另一边还拿着句柄。关掉后日志行照样进 `agent` 日志来源。
    pub write_agent_log: bool,
    /// 只在模拟数据时有效：`/v1/ping` 报这个协议版本（例如 0 冒充老代理），给面板自检「版本对不上」那句用；别的端点照旧。
    /// `agent.toml` 的 `mock_ping_schema`，或 `deploy-agent-dev --ping-schema <n>`。真实后端不理它。
    pub mock_ping_schema: Option<u32>,
    /// 自动升级（设计 5.9）：渠道白名单、收不收 dirty 的包、留几次备份、检查等多久。
    pub upgrade: UpgradeConfig,
    /// 这个进程是不是「不带界面库的代理」（`deploy-dash-agent.exe`）。决定 `agent_exe.kind`：
    /// `headless` 能单独升，`panel`（面板 exe 的 `--agent` 身份）跟面板一起换、这一版不单独升。
    pub headless: bool,
    /// 免令牌模式（仅限受控内网或安全组已隔离来源 IP 时启用）；缺省 false。
    /// 只放行只读接口（GET），写接口照旧要令牌；也不给跨源（CORS）应答头。
    pub no_auth: bool,
    /// 跨源（CORS）放行的来源，只在令牌模式下给；空 = 任何来源（请求照样要带令牌）。
    pub cors_origins: Vec<String>,
}

/// `agent.toml` 的 `[upgrade]` 一节（设计 5.9），都可以不写。
#[derive(Clone, Debug)]
pub struct UpgradeConfig {
    /// 收哪些渠道的包；空 = 什么都收。正式服务器写 `["release"]`。
    pub accept_channels: Vec<String>,
    /// 收不收带 `dirty`（没提交的代码编的）的包。
    pub accept_dirty: bool,
    /// 留最近几次成功的升级的备份。
    pub keep_backups: u32,
    /// 升级后检查服务最多等多久（秒）。
    pub check_secs: u32,
}

impl Default for UpgradeConfig {
    fn default() -> Self {
        UpgradeConfig { accept_channels: Vec::new(), accept_dirty: true, keep_backups: 2, check_secs: 90 }
    }
}

/// 缺省要隐去的字样：口令、令牌一类。
pub const DEFAULT_REDACT_KEYWORDS: [&str; 10] =
    ["password", "passwd", "pwd=", "secret", "token", "authorization", "bearer ", "v_password", "口令", "密码"];

/// 本机已知别人在用的端口（设计第 6 节、派工单）：代理不许听在这些端口上。
pub const RESERVED_PORTS: [u16; 20] =
    [8009, 8022, 8023, 8029, 8199, 9199, 9098, 3080, 3100, 3101, 3102, 4877, 5181, 5182, 5190, 5390, 18700, 18709, 18722, 18798];

/// `agent.toml` 的形状（都可以不写）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct AgentToml {
    listen: Option<String>,
    public_url: Option<String>,
    browse_roots: Option<Vec<String>>,
    redact_keywords: Option<Vec<String>>,
    audit_rotate_mb: Option<u64>,
    mock_ping_schema: Option<u32>,
    no_auth: Option<bool>,
    cors_origins: Option<Vec<String>>,
    upgrade: UpgradeToml,
}

/// `[upgrade]` 一节。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct UpgradeToml {
    accept_channels: Option<Vec<String>>,
    accept_dirty: Option<bool>,
    keep_backups: Option<u32>,
    check_secs: Option<u32>,
}

const DEFAULT_TOML: &str = "# 部署面板 stack 代理的配置（第一次起来时自动写的，可以改，改完重启代理）。\r\n\
# 缺省只听本机：面板装在这台服务器上时够用。要让局域网里别的电脑连，改成 \"0.0.0.0:8600\" 或某块网卡的地址，\r\n\
# 再用 open-firewall 子命令打出放行这个端口的命令（只放 Domain、Private 和本网段，不放 Public；加 --apply 才真建）。\r\n\
listen = \"127.0.0.1:8600\"\r\n\
\r\n\
# 对外地址（可空）。经过端口映射、或者有域名时填，例如 \"http://<服务器地址>\"；\r\n\
# 配了就用它的主机名拼面板上「打开网页」的地址，不看请求里的 Host。\r\n\
public_url = \"\"\r\n\
\r\n\
# 远程「浏览文件夹」只许在这些目录下走（第二版用）。\r\n\
browse_roots = []\r\n\
\r\n\
# 别的网页（跨源）用浏览器连这台时放不放行：只在令牌模式下放行，请求照样要带令牌；免令牌模式下一律不放行。\r\n\
# 空 = 任何来源；要收紧就写全来源，例如 [\"http://<面板所在机器>:8600\"]。\r\n\
cors_origins = []\r\n\
\r\n\
# 自动升级（设计 5.9）。开没开只看 run\\gui\\upgrade.key 在不在（upgrade-key 子命令管它）。\r\n\
[upgrade]\r\n\
# 收哪些渠道的包；空 = 什么都收。正式服务器写 [\"release\"]，挡住测试包和没提交的代码编出来的包。\r\n\
accept_channels = []\r\n\
# false = 清单里有部件标着 dirty（没提交的代码编的）就不收。\r\n\
accept_dirty = true\r\n\
# 留最近几次成功升级的备份。\r\n\
keep_backups = 2\r\n\
# 升级后检查服务最多等多少秒。\r\n\
check_secs = 90\r\n";

impl AgentConfig {
    /// 全部取缺省值。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            listen: SocketAddr::from(([127, 0, 0, 1], proto::DEFAULT_PORT)),
            public_url: String::new(),
            log_ring_lines: proto::LOG_RING_LINES,
            audit_rotate_bytes: 10 * 1024 * 1024,
            unauthorized_delay: Duration::from_millis(300),
            unauthorized_burst: 20,
            unauthorized_slow_delay: Duration::from_secs(2),
            long_poll_max: Duration::from_secs(proto::LONG_POLL_MAX_SECS),
            browse_roots: Vec::new(),
            redact_keywords: DEFAULT_REDACT_KEYWORDS.iter().map(|s| s.to_string()).collect(),
            single_instance: true,
            protect_token_file: true,
            echo_log: false,
            write_agent_log: true,
            mock_ping_schema: None,
            upgrade: UpgradeConfig::default(),
            headless: false,
            no_auth: false,
            cors_origins: Vec::new(),
        }
    }

    /// 读 `<根>\run\gui\agent.toml`（没有就写一份带注释的缺省文件）。`listen` 由命令行给时以命令行为准。
    pub fn load(root: impl Into<PathBuf>, listen_override: Option<SocketAddr>) -> anyhow::Result<Self> {
        let mut cfg = Self::new(root);
        let path = cfg.toml_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let t: AgentToml = toml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| anyhow::anyhow!("{} 写得不对：{e}", path.display()))?;
                if let Some(listen) = t.listen.filter(|s| !s.trim().is_empty()) {
                    cfg.listen = listen.trim().parse().map_err(|_| anyhow::anyhow!("{} 里 listen = \"{listen}\" 不是「地址:端口」", path.display()))?;
                }
                cfg.public_url = t.public_url.unwrap_or_default().trim().to_string();
                cfg.browse_roots = t.browse_roots.unwrap_or_default();
                if let Some(k) = t.redact_keywords {
                    cfg.redact_keywords = k;
                }
                if let Some(mb) = t.audit_rotate_mb.filter(|m| *m > 0) {
                    cfg.audit_rotate_bytes = mb * 1024 * 1024;
                }
                cfg.mock_ping_schema = t.mock_ping_schema;
                cfg.no_auth = t.no_auth.unwrap_or(false);
                cfg.cors_origins = t.cors_origins.unwrap_or_default().into_iter().map(|o| o.trim().trim_end_matches('/').to_string()).filter(|o| !o.is_empty()).collect();
                let u = t.upgrade;
                if let Some(ch) = u.accept_channels {
                    cfg.upgrade.accept_channels = ch.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
                }
                if let Some(b) = u.accept_dirty {
                    cfg.upgrade.accept_dirty = b;
                }
                if let Some(n) = u.keep_backups.filter(|n| *n > 0) {
                    cfg.upgrade.keep_backups = n;
                }
                if let Some(n) = u.check_secs.filter(|n| *n > 0) {
                    cfg.upgrade.check_secs = n;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&path, DEFAULT_TOML)?;
            }
            Err(e) => return Err(anyhow::anyhow!("读不了 {}：{e}", path.display())),
        }
        if let Some(l) = listen_override {
            cfg.listen = l;
        }
        Ok(cfg)
    }

    /// 端口能不能用：不许是 0、不许是别人的端口。
    pub fn check_listen(&self, extra_protected: &[u16]) -> anyhow::Result<()> {
        let port = self.listen.port();
        if RESERVED_PORTS.contains(&port) || extra_protected.contains(&port) {
            anyhow::bail!("端口 {port} 是这台机器上别的服务的（在保护名单里），代理不能听它；换一个，缺省是 {}", proto::DEFAULT_PORT);
        }
        Ok(())
    }

    /// 代理自己的东西都放这里：`<根>\run\gui\`。
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run").join("gui")
    }

    pub fn toml_path(&self) -> PathBuf {
        self.run_dir().join("agent.toml")
    }

    pub fn token_path(&self) -> PathBuf {
        self.run_dir().join("agent.token")
    }

    /// 任务输出 `<编号>.log` 和结果 `<编号>.result.json`。
    pub fn jobs_dir(&self) -> PathBuf {
        self.run_dir().join("jobs")
    }

    /// `<根>\logs\gui\`：审计和代理自己的日志。
    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs").join("gui")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.log_dir().join("agent-audit.jsonl")
    }

    pub fn agent_log_path(&self) -> PathBuf {
        self.log_dir().join("agent.log")
    }

    pub fn stack_toml_path(&self) -> PathBuf {
        self.root.join("stack.toml")
    }
}

/// 读 `stack.toml` 里 `[guard] protect_ports`（没有就是空）。
pub fn stack_protect_ports(stack_toml: &Path) -> Vec<u16> {
    let Ok(text) = std::fs::read_to_string(stack_toml) else { return Vec::new() };
    let Ok(v) = text.trim_start_matches('\u{feff}').parse::<toml::Table>() else { return Vec::new() };
    v.get("guard")
        .and_then(|g| g.get("protect_ports"))
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_integer()).filter_map(|x| u16::try_from(x).ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_writes_the_default_file_and_reads_mock_ping_schema() {
        let root = std::env::temp_dir().join(format!("deploy-agent-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = AgentConfig::load(&root, None).unwrap();
        assert_eq!((cfg.listen.to_string().as_str(), cfg.mock_ping_schema), ("127.0.0.1:8600", None));
        let default_text = std::fs::read_to_string(cfg.toml_path()).unwrap();
        assert!(default_text.contains("--apply"), "缺省文件写了说明");
        assert!(!default_text.contains("mock_ping_schema"), "自检用的开关不写进缺省文件");

        std::fs::write(cfg.toml_path(), "listen = \"0.0.0.0:8600\"\nmock_ping_schema = 0\n").unwrap();
        let cfg = AgentConfig::load(&root, Some("127.0.0.1:18650".parse().unwrap())).unwrap();
        assert_eq!((cfg.listen.port(), cfg.mock_ping_schema), (18650, Some(0)), "命令行的 listen 优先");
        std::fs::write(cfg.toml_path(), "mock_ping_schema = -1\n").unwrap();
        assert!(AgentConfig::load(&root, None).unwrap_err().to_string().contains("写得不对"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_auth_and_cors_origins_default_off_and_are_read_from_the_file() {
        let root = std::env::temp_dir().join(format!("deploy-agent-config-cors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cfg = AgentConfig::load(&root, None).unwrap();
        assert!(!cfg.no_auth && cfg.cors_origins.is_empty(), "缺省要令牌、不限来源");
        let default_text = std::fs::read_to_string(cfg.toml_path()).unwrap();
        assert!(default_text.contains("cors_origins = []") && !default_text.contains("no_auth"), "缺省文件写了来源名单，不提免令牌");

        std::fs::write(cfg.toml_path(), "no_auth = true\ncors_origins = [\" http://panel.example:8600/ \", \"\", \"https://ops.example\"]\n").unwrap();
        let cfg = AgentConfig::load(&root, None).unwrap();
        assert!(cfg.no_auth);
        assert_eq!(cfg.cors_origins, vec!["http://panel.example:8600".to_string(), "https://ops.example".to_string()], "去掉空白、末尾斜杠和空项");
        let _ = std::fs::remove_dir_all(&root);
    }
}
