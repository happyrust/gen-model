//! 安装类子命令（R7 / R8）：`open-firewall`、`install-autostart`、`uninstall-autostart`、`show-token`、`rotate-token`。
//!
//! - 前三个会改这台机器：**缺省只打印**要执行的命令（`--dry-run` 是同义词），加 `--apply` 才真执行。
//!   真执行那条路还没在任何机器上跑过（留到验收时用户在场）。
//! - `open-firewall`：只放行代理端口的入站 TCP，范围 Domain、Private（不含 Public），程序限定为代理的 exe，
//!   对方地址缺省本网段（`remoteip=localsubnet`；`--remote-ip <网段>` 可以换，没有「不限地址」）。
//!   要管理员权限：`--apply` 时经 PowerShell `Start-Process -Verb RunAs` 提权跑 `netsh`；提权失败或 netsh 报错，
//!   就把 netsh 原文交回，让人复制到管理员窗口里自己跑。
//! - `install-autostart`：当前账户登录时起代理的任务计划（`Register-ScheduledTask`，`-RunLevel Limited`，**不提权**）——
//!   和平时双击 `开始.cmd` 的是同一个账户、同一个完整性级别（设计 3.3：起停服务要同样权限）。
//! - 这些函数分三层：拼命令（纯函数，有单测）、只打印时的全文（`*_preview`，有单测）、真去执行（`execute_*`，单测不跑）。
//!   `open_firewall` / `install_autostart` / `uninstall_autostart` 把三层串起来，`deploy-agent-dev` 和 `deploy-dash --agent` 共用。

use std::net::IpAddr;
use std::path::Path;
use std::process::Command;

use crate::config::AgentConfig;
use crate::token::{self, Token};

/// 防火墙规则名、任务计划名（设计第 6 节）。
pub const RULE_NAME: &str = "AIOS 部署代理";
pub const TASK_NAME: &str = "AIOS 部署代理";

/// 子命令的执行结果：成功的说明，或者失败时交给人的原文。
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    /// 没做成：原因 + 让人自己在管理员窗口里跑的那条命令。
    Manual { reason: String, command: String },
}

// ───────────────────────── 对方地址 ─────────────────────────

/// 防火墙规则只放这些对方地址（netsh 的 `remoteip=`，防火墙界面里「作用域 → 远程 IP 地址」）。
/// 缺省本网段（`localsubnet`）；可以换成具体的地址、网段、范围（逗号隔开）。没有「不限地址」：
/// `any`、`*`、`/0`、比 IPv4 /8（IPv6 /7）还宽的都不收。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteIp(String);

/// IPv4 最宽写到 /8、IPv6 最宽 /7；再宽就差不多是不限地址了。
const WIDEST_V4: u32 = 8;
const WIDEST_V6: u32 = 7;

impl RemoteIp {
    pub const LOCAL_SUBNET: &'static str = "localsubnet";

    /// 解析 `--remote-ip` 的值，错了回给人看的一句话。
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(format!("--remote-ip 后面要写网段，例如 192.0.2.0/24；不写这个开关就是只放本网段（{}）", Self::LOCAL_SUBNET));
        }
        let items = text.split(',').map(|i| remote_item(i.trim())).collect::<Result<Vec<_>, _>>()?;
        Ok(Self(items.join(",")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_local_subnet(&self) -> bool {
        self.0 == Self::LOCAL_SUBNET
    }
}

impl Default for RemoteIp {
    fn default() -> Self {
        Self(Self::LOCAL_SUBNET.into())
    }
}

impl std::fmt::Display for RemoteIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for RemoteIp {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::parse(s)
    }
}

/// `--remote-ip` 的一项：`localsubnet`、一个地址、`地址/前缀`、`起-止`。回规范写法。
fn remote_item(item: &str) -> Result<String, String> {
    let unlimited = || format!("--remote-ip 不给「不限地址」（{item}）：不写这个开关就是只放本网段，要放别的就写具体网段，例如 192.0.2.0/24");
    let unknown = || format!("--remote-ip 认不出「{item}」：写成 192.0.2.0/24（网段）、192.0.2.10（一个地址）、192.0.2.10-192.0.2.20（范围）或 localsubnet，多个用逗号隔开");
    let too_wide = || format!("--remote-ip {item} 太宽了，差不多等于不限地址：IPv4 最宽写到 /{WIDEST_V4}，IPv6 最宽 /{WIDEST_V6}");
    let ip = |s: &str| s.trim().parse::<IpAddr>().map_err(|_| unknown());
    let lower = item.to_ascii_lowercase();
    match lower.as_str() {
        "" => return Err("--remote-ip 里逗号之间是空的".into()),
        RemoteIp::LOCAL_SUBNET => return Ok(lower),
        "any" | "*" => return Err(unlimited()),
        _ => {}
    }
    if let Some((addr, prefix)) = item.split_once('/') {
        let addr = ip(addr)?;
        let prefix: u32 = prefix.trim().parse().map_err(|_| unknown())?;
        let (max, widest) = if addr.is_ipv4() { (32, WIDEST_V4) } else { (128, WIDEST_V6) };
        return match prefix {
            0 => Err(unlimited()),
            p if p > max => Err(unknown()),
            p if p < widest => Err(too_wide()),
            p => Ok(format!("{addr}/{p}")),
        };
    }
    if let Some((from, to)) = item.split_once('-') {
        let (from, to) = (ip(from)?, ip(to)?);
        let narrow_enough = match (from, to) {
            (IpAddr::V4(a), IpAddr::V4(b)) if a <= b => u32::from(b) - u32::from(a) < 1 << (32 - WIDEST_V4),
            (IpAddr::V6(a), IpAddr::V6(b)) if a <= b => u128::from(b) - u128::from(a) < 1 << (128 - WIDEST_V6),
            _ => return Err(unknown()),
        };
        return if narrow_enough { Ok(format!("{from}-{to}")) } else { Err(too_wide()) };
    }
    Ok(ip(item)?.to_string())
}

// ───────────────────────── 防火墙 ─────────────────────────

/// `netsh advfirewall firewall add rule …` 的参数（不含 `netsh` 本身），对方地址只放本网段。
pub fn netsh_add_rule_args(port: u16, program: &Path) -> Vec<String> {
    netsh_add_rule_args_scoped(port, program, &RemoteIp::default())
}

/// 同上，对方地址由 `remote` 定（`--remote-ip`）。
pub fn netsh_add_rule_args_scoped(port: u16, program: &Path, remote: &RemoteIp) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={RULE_NAME}"),
        "dir=in".into(),
        "action=allow".into(),
        "protocol=TCP".into(),
        format!("localport={port}"),
        format!("program={}", program.display()),
        "profile=domain,private".into(),
        format!("remoteip={remote}"),
        "enable=yes".into(),
    ]
}

/// 给人复制的那一行（`name=`、`program=` 的值加双引号），对方地址只放本网段。
pub fn netsh_command_line(port: u16, program: &Path) -> String {
    netsh_command_line_scoped(port, program, &RemoteIp::default())
}

/// 同上，对方地址由 `remote` 定。
pub fn netsh_command_line_scoped(port: u16, program: &Path, remote: &RemoteIp) -> String {
    let args: Vec<String> = netsh_add_rule_args_scoped(port, program, remote)
        .into_iter()
        .map(|a| match a.split_once('=') {
            Some((k, v)) if v.contains(' ') || k == "name" || k == "program" => format!("{k}=\"{v}\""),
            _ => a,
        })
        .collect();
    format!("netsh {}", args.join(" "))
}

/// 查规则用的那一行（验收时看「只有代理端口」）。
pub fn netsh_show_rule_line() -> String {
    format!("netsh advfirewall firewall show rule name=\"{RULE_NAME}\"")
}

/// 提权跑 netsh 的 PowerShell 脚本：UAC 被拒（1223）或 netsh 失败时以非 0 退出。对方地址只放本网段。
pub fn elevated_netsh_script(port: u16, program: &Path) -> String {
    elevated_netsh_script_scoped(port, program, &RemoteIp::default())
}

/// 同上，对方地址由 `remote` 定。
pub fn elevated_netsh_script_scoped(port: u16, program: &Path, remote: &RemoteIp) -> String {
    let line = netsh_command_line_scoped(port, program, remote);
    let arg_list = line.trim_start_matches("netsh ").to_string();
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         try {{\r\n\
         \x20   $p = Start-Process -FilePath 'netsh.exe' -ArgumentList {} -Verb RunAs -Wait -PassThru -WindowStyle Hidden\r\n\
         \x20   exit $p.ExitCode\r\n\
         }} catch {{\r\n\
         \x20   Write-Output $_.Exception.Message\r\n\
         \x20   exit 1223\r\n\
         }}\r\n",
        ps_quote(&arg_list)
    )
}

/// 真去建防火墙规则（会弹 UAC），对方地址只放本网段。失败时回 netsh 原文。
pub fn execute_open_firewall(port: u16, program: &Path) -> Outcome {
    execute_open_firewall_scoped(port, program, &RemoteIp::default())
}

/// 同上，对方地址由 `remote` 定。
pub fn execute_open_firewall_scoped(port: u16, program: &Path, remote: &RemoteIp) -> Outcome {
    let script = elevated_netsh_script_scoped(port, program, remote);
    let out = windows_powershell_command(&script).output();
    match out {
        Ok(o) if o.status.success() => Outcome::Done(format!(
            "已放行 TCP {port} 入站（Domain、Private，对方地址 {remote}），程序 {}。核对：{}",
            program.display(),
            netsh_show_rule_line()
        )),
        Ok(o) => Outcome::Manual {
            reason: format!("没做成（退出码 {:?}：{}）。请在「以管理员身份运行」的命令行窗口里跑下面这一行", o.status.code(), token::decode_console(&o.stdout).trim()),
            command: netsh_command_line_scoped(port, program, remote),
        },
        Err(e) => Outcome::Manual { reason: format!("起不了 PowerShell：{e}"), command: netsh_command_line_scoped(port, program, remote) },
    }
}

// ───────────────────────── 登录自启 ─────────────────────────

/// 登录自启的 PowerShell 脚本：当前账户、登录时、不提权、只起一个、不限运行时长。
pub fn autostart_script(exe: &Path, args: &[String], workdir: &Path) -> String {
    let arg_line = args.iter().map(|a| win_quote(a)).collect::<Vec<_>>().join(" ");
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         $user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\r\n\
         $a = New-ScheduledTaskAction -Execute {exe} -Argument {args} -WorkingDirectory {dir}\r\n\
         $t = New-ScheduledTaskTrigger -AtLogOn -User $user\r\n\
         $p = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited\r\n\
         $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -StartWhenAvailable\r\n\
         Register-ScheduledTask -TaskName {name} -Action $a -Trigger $t -Principal $p -Settings $s -Description {desc} -Force | Out-Null\r\n\
         Write-Output \"已建任务计划：$user 登录时起代理\"\r\n",
        exe = ps_quote(&exe.display().to_string()),
        args = ps_quote(&arg_line),
        dir = ps_quote(&workdir.display().to_string()),
        name = ps_quote(TASK_NAME),
        desc = ps_quote("部署面板的 stack 代理：当前账户登录时在后台起（不提权）"),
    )
}

/// 删掉登录自启。
pub fn uninstall_autostart_script() -> String {
    format!("$ErrorActionPreference = 'Stop'\r\nUnregister-ScheduledTask -TaskName {} -Confirm:$false\r\nWrite-Output '已删掉登录自启'\r\n", ps_quote(TASK_NAME))
}

/// 代理登录自启时的命令行参数：`--agent --root <根> [--listen <地址>]`（给 deploy-dash.exe 用）。
pub fn agent_args(cfg: &AgentConfig, listen_explicit: bool) -> Vec<String> {
    let mut v = vec!["--agent".to_string(), "--root".to_string(), cfg.root.display().to_string()];
    if listen_explicit {
        v.push("--listen".into());
        v.push(cfg.listen.to_string());
    }
    v
}

/// 真去建任务计划（不提权）。
pub fn execute_powershell(script: &str) -> Outcome {
    let out = windows_powershell_command(script).output();
    match out {
        Ok(o) if o.status.success() => Outcome::Done(token::decode_console(&o.stdout).trim().to_string()),
        Ok(o) => Outcome::Manual {
            reason: format!("没做成（退出码 {:?}）：{}{}", o.status.code(), token::decode_console(&o.stdout).trim(), token::decode_console(&o.stderr).trim()),
            command: script.to_string(),
        },
        Err(e) => Outcome::Manual { reason: format!("起不了 PowerShell：{e}"), command: script.to_string() },
    }
}

// ───────────────────────── 缺省只打印，--apply 才执行 ─────────────────────────

/// 只打印时的最后一句。
pub const APPLY_HINT: &str = "要真执行加 --apply";

/// `--apply`、`--dry-run` → 真不真执行。缺省只打印，`--dry-run` 是它的同义词；两个一起给是写错了。
pub fn wants_apply(apply: bool, dry_run: bool) -> Result<bool, String> {
    if apply && dry_run {
        return Err("--apply 和 --dry-run 只能给一个（缺省就是只打印）".into());
    }
    Ok(apply)
}

/// `open-firewall` 只打印时的全文。
pub fn open_firewall_preview(port: u16, program: &Path, remote: &RemoteIp) -> String {
    let scope = if remote.is_local_subnet() { format!("本网段（{remote}）") } else { format!(" {remote}") };
    format!(
        "（只打印，没执行）要在「以管理员身份运行」的窗口里跑的是：\n{}\n\n对方地址只放{scope}；要换网段用 --remote-ip <网段>。\n{APPLY_HINT}（会弹 UAC）。",
        netsh_command_line_scoped(port, program, remote)
    )
}

/// `install-autostart` 只打印时的全文。
pub fn install_autostart_preview(cfg: &AgentConfig, program: &Path, listen_explicit: bool) -> String {
    format!(
        "（只打印，没执行）要在普通（不提权）的 Windows PowerShell 窗口里、用平时起停服务的那个账户跑的是：\n{}\n\n{APPLY_HINT}。",
        autostart_script(program, &agent_args(cfg, listen_explicit), &cfg.root).trim_end()
    )
}

/// `uninstall-autostart` 只打印时的全文。
pub fn uninstall_autostart_preview() -> String {
    format!("（只打印，没执行）要在普通的 Windows PowerShell 窗口里跑的是：\n{}\n\n{APPLY_HINT}。", uninstall_autostart_script().trim_end())
}

/// `open-firewall` 子命令：缺省打印，`apply` 时提权执行。回进程退出码：0 = 打印了或做成了；1 = 没做成（原文已打出来）。
pub fn open_firewall(port: u16, program: &Path, remote: &RemoteIp, apply: bool) -> i32 {
    print_or_apply(apply, || open_firewall_preview(port, program, remote), || execute_open_firewall_scoped(port, program, remote))
}

/// `install-autostart` 子命令：缺省打印，`apply` 时执行（不提权）。退出码同上。
pub fn install_autostart(cfg: &AgentConfig, program: &Path, listen_explicit: bool, apply: bool) -> i32 {
    print_or_apply(
        apply,
        || install_autostart_preview(cfg, program, listen_explicit),
        || execute_powershell(&autostart_script(program, &agent_args(cfg, listen_explicit), &cfg.root)),
    )
}

/// `uninstall-autostart` 子命令：缺省打印，`apply` 时执行。退出码同上。
pub fn uninstall_autostart(apply: bool) -> i32 {
    print_or_apply(apply, uninstall_autostart_preview, || execute_powershell(&uninstall_autostart_script()))
}

fn print_or_apply(apply: bool, preview: impl FnOnce() -> String, execute: impl FnOnce() -> Outcome) -> i32 {
    if !apply {
        println!("{}", preview());
        return 0;
    }
    match execute() {
        Outcome::Done(msg) => {
            println!("{msg}");
            0
        }
        Outcome::Manual { reason, command } => {
            println!("{reason}\n\n{command}");
            1
        }
    }
}

// ───────────────────────── 令牌 ─────────────────────────

/// `show-token`：在服务器本机显示一次给管理员抄。没有令牌文件就生成一个。
pub fn show_token(cfg: &AgentConfig) -> anyhow::Result<Token> {
    let (t, _, _) = token::ensure(&cfg.token_path(), cfg.protect_token_file)?;
    Ok(t)
}

/// `rotate-token`：换一个；在跑的代理看到文件变了马上用新的，旧令牌的客户端会收到 401。
pub fn rotate_token(cfg: &AgentConfig) -> anyhow::Result<Token> {
    token::rotate(&cfg.token_path(), cfg.protect_token_file)
}

// ───────────────────────── 小工具 ─────────────────────────

/// PowerShell 单引号字符串（里面的 `'` 写两遍）。
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Windows 命令行的一个参数（照 CommandLineToArgvW 的规矩：有空格、引号就加双引号，引号前的反斜杠加倍）。
pub fn win_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

fn windows_powershell() -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\Windows"))
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

/// Windows PowerShell 5.1 跑一段脚本。去掉继承来的 PSModulePath：代理若是从 pwsh 7 里起的，
/// 5.1 会去加载 7 的模块（ScheduledTasks、Microsoft.PowerShell.Security…）而失败。
pub fn windows_powershell_command(script: &str) -> Command {
    let mut c = Command::new(windows_powershell());
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]).env_remove("PSModulePath");
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn netsh_args_only_open_the_agent_port_for_domain_and_private() {
        let exe = PathBuf::from("D:\\<根>\\deploy dash\\deploy-dash.exe");
        let a = netsh_add_rule_args(8600, &exe);
        assert_eq!(a[..4], ["advfirewall", "firewall", "add", "rule"]);
        assert!(a.contains(&"dir=in".to_string()));
        assert!(a.contains(&"protocol=TCP".to_string()));
        assert!(a.contains(&"localport=8600".to_string()));
        assert!(a.contains(&"profile=domain,private".to_string()));
        assert!(a.contains(&"remoteip=localsubnet".to_string()), "缺省只放本网段");
        assert!(!a.iter().any(|x| x.contains("public") || x.contains("any")));
        let line = netsh_command_line(8600, &exe);
        assert_eq!(
            line,
            "netsh advfirewall firewall add rule name=\"AIOS 部署代理\" dir=in action=allow protocol=TCP localport=8600 program=\"D:\\<根>\\deploy dash\\deploy-dash.exe\" profile=domain,private remoteip=localsubnet enable=yes"
        );
        let lan = RemoteIp::parse("192.0.2.0/24, 198.51.100.7").unwrap();
        assert!(netsh_command_line_scoped(8600, &exe, &lan).ends_with(" profile=domain,private remoteip=192.0.2.0/24,198.51.100.7 enable=yes"));
    }

    #[test]
    fn elevation_script_quotes_for_powershell() {
        let s = elevated_netsh_script(8600, Path::new("D:\\it's\\deploy-dash.exe"));
        assert!(s.contains("-Verb RunAs"));
        assert!(s.contains("program=\"D:\\it''s\\deploy-dash.exe\""), "{s}");
        assert!(s.contains("remoteip=localsubnet"), "{s}");
        assert!(s.contains("exit 1223"));
    }

    #[test]
    fn remote_ip_is_the_local_subnet_by_default_and_never_unlimited() {
        assert_eq!(RemoteIp::default().as_str(), "localsubnet");
        assert!(RemoteIp::default().is_local_subnet());
        let ok = |s: &str| RemoteIp::parse(s).unwrap_or_else(|e| panic!("{s}：{e}")).to_string();
        assert_eq!(ok(" LocalSubnet "), "localsubnet");
        assert_eq!(ok("192.0.2.0/24"), "192.0.2.0/24");
        assert_eq!(ok("192.0.2.10"), "192.0.2.10");
        assert_eq!(ok("192.0.2.10-192.0.2.20"), "192.0.2.10-192.0.2.20");
        assert_eq!(ok("10.0.0.0/8,localsubnet"), "10.0.0.0/8,localsubnet");
        assert_eq!(ok("2001:db8::/32"), "2001:db8::/32");
        assert_eq!(ok("fe80::/10"), "fe80::/10");
        let err = |s: &str| RemoteIp::parse(s).expect_err(s);
        for unlimited in ["any", "ANY", "*", "0.0.0.0/0", "::/0", "localsubnet,any"] {
            assert!(err(unlimited).contains("不限地址"), "{unlimited}");
        }
        for too_wide in ["10.0.0.0/7", "0.0.0.0-255.255.255.255", "10.0.0.0-11.0.0.0", "2000::/3"] {
            assert!(err(too_wide).contains("太宽"), "{too_wide}");
        }
        assert!(RemoteIp::parse("10.0.0.0-10.255.255.255").is_ok(), "正好一个 /8 的范围可以");
        for bad in ["", " ", "dns", "defaultgateway", "192.0.2.0/33", "192.0.2.0/255.255.255.0", "192.0.2.20-192.0.2.10", "192.0.2.1-2001:db8::1", "a.b", "192.0.2.0/24;calc", "192.0.2.0/24,"] {
            assert!(RemoteIp::parse(bad).is_err(), "{bad:?} 应当不收");
        }
    }

    #[test]
    fn nothing_runs_without_apply() {
        assert_eq!(wants_apply(false, false), Ok(false), "缺省只打印");
        assert_eq!(wants_apply(false, true), Ok(false), "--dry-run 是同义词");
        assert_eq!(wants_apply(true, false), Ok(true));
        assert!(wants_apply(true, true).is_err());
        assert_eq!(print_or_apply(false, || "预览".into(), || panic!("不带 --apply 不该执行")), 0);
        assert_eq!(print_or_apply(true, || panic!("--apply 不打预览"), || Outcome::Done("做成了".into())), 0);
        assert_eq!(print_or_apply(true, String::new, || Outcome::Manual { reason: "没做成".into(), command: "netsh …".into() }), 1);
    }

    #[test]
    fn previews_print_the_command_and_the_apply_hint() {
        let exe = PathBuf::from("D:\\<面板目录>\\deploy-dash.exe");
        let fw = open_firewall_preview(8600, &exe, &RemoteIp::default());
        assert!(fw.contains(&netsh_command_line(8600, &exe)), "{fw}");
        assert!(fw.contains("本网段（localsubnet）") && fw.contains("--remote-ip") && fw.contains(APPLY_HINT), "{fw}");
        let lan = open_firewall_preview(8600, &exe, &RemoteIp::parse("192.0.2.0/24").unwrap());
        assert!(lan.contains("remoteip=192.0.2.0/24") && lan.contains("对方地址只放 192.0.2.0/24"), "{lan}");
        let cfg = AgentConfig::new("D:\\<根>");
        let auto = install_autostart_preview(&cfg, &exe, false);
        assert!(auto.contains("Register-ScheduledTask -TaskName 'AIOS 部署代理'") && auto.contains("-RunLevel Limited"), "{auto}");
        assert!(auto.ends_with(&format!("{APPLY_HINT}。")), "{auto}");
        let un = uninstall_autostart_preview();
        assert!(un.contains("Unregister-ScheduledTask -TaskName 'AIOS 部署代理'") && un.contains(APPLY_HINT), "{un}");
    }

    #[test]
    fn autostart_is_current_user_at_logon_without_elevation() {
        let cfg = AgentConfig::new("D:\\<根> 1");
        let args = agent_args(&cfg, false);
        assert_eq!(args, vec!["--agent", "--root", "D:\\<根> 1"]);
        let s = autostart_script(Path::new("D:\\<根> 1\\deploy-dash.exe"), &args, &cfg.root);
        assert!(s.contains("-AtLogOn -User $user"));
        assert!(s.contains("-RunLevel Limited"));
        assert!(!s.contains("Highest"));
        assert!(s.contains("-Argument '--agent --root \"D:\\<根> 1\"'"), "{s}");
        assert!(s.contains("-TaskName 'AIOS 部署代理'"));
        assert!(s.contains("-MultipleInstances IgnoreNew"));
        let u = uninstall_autostart_script();
        assert!(u.contains("Unregister-ScheduledTask -TaskName 'AIOS 部署代理' -Confirm:$false"));
        let mut cfg2 = AgentConfig::new("D:\\r");
        cfg2.listen = "0.0.0.0:8600".parse().unwrap();
        assert_eq!(agent_args(&cfg2, true)[3..], ["--listen".to_string(), "0.0.0.0:8600".to_string()]);
    }

    #[test]
    fn quoting_rules() {
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(win_quote("plain"), "plain");
        assert_eq!(win_quote("a b"), "\"a b\"");
        assert_eq!(win_quote("D:\\a b\\"), "\"D:\\a b\\\\\"");
        assert_eq!(win_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(win_quote(""), "\"\"");
    }

    #[test]
    fn show_and_rotate_token_use_the_token_file() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = AgentConfig::new(&dir);
        cfg.protect_token_file = false;
        let a = show_token(&cfg).unwrap();
        assert_eq!(show_token(&cfg).unwrap(), a, "show-token 不换令牌");
        let b = rotate_token(&cfg).unwrap();
        assert_ne!(a, b);
        assert_eq!(show_token(&cfg).unwrap(), b);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
