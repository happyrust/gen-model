//! `StoreMode::SpawnedMem` 的实现：本进程拉起一个 `--memory` 后端的 SurrealDB 子进程，
//! 绑到配置里那对 `v_ip:v_port`。
//!
//! 换掉的是**谁来跑那台 surreal**，不是协议。`SUL_DB` 仍是 ws 连接、仍走
//! `init_surreal()`，plant-ui / `/sql` 探针 / `rvm_verify` / `Capture-*Evidence.ps1`
//! 连的还是同一个地址。这一句正是它与 `mem://`（[`crate::options::StoreMode::EmbeddedMem`]）
//! 的全部分别——那一档把库关在进程里，别的进程一个字都读不到，而 plant-ui 的启动序列
//! 第一步就是连 `v_ip:v_port`。
//!
//! 二进制的查找顺序与版本闸**逐条复刻 `scripts/Start-Surreal8009.ps1`**（计划 P5 要把
//! 「两份判据不许分叉」钉成护栏）。别在这里放宽：PATH 上通常是 `cargo install` 装的 3.x，
//! 而它的失败方式（要求不可逆迁移 / `Server sent no subprotocol`）会把人引向完全错误的方向。
//!
//! 端口上已经有东西时怎么办，由 [`judge_port`] 那张四格表决定（P2）：**空闲**就起一个、
//! **自家还活着**就附着、**自家的孤儿**就收掉重起、**别人的库**就拒绝启动。判据是写在库里的
//! 一枚[署名][`StoreSignature`]——从外面看，一台 memory 后端与一台 rocksdb 后端长得一模一样，
//! 只有库里那一行分得出「这是我起的」。任何一格分不清，一律**拒绝**，绝不往「杀掉它」那一边猜。
//!
//! 计划：`docs/plans/2026-09-04-spawned-memory-store-out-of-box.md`（P1 / P2）。
//! **本模块目前没有调用方**——接进 `run_app` 是 P3。

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};

/// 环境变量名：显式指定 SurrealDB 服务端二进制（`Start-Surreal8009.ps1` 同名同义）。
pub const SURREAL_EXE_ENV: &str = "AIOS_SURREAL_EXE";

/// 客户端 SDK 的大版本。服务端必须同线，理由见 `readme.md` 开头那两个坑。
pub const REQUIRED_MAJOR: u32 = 2;

/// 等端口起来的默认上限。冷起一个空的 memory 库是秒级的，30 s 是给「机器很忙」留的余量；
/// 真超了不是慢，是起不来，那时错误里必须带上它自己的日志尾巴（见 [`spawn_timeout_error`]）。
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// 轮询间隔。与 `bin/l3_suite` 那份 `wait_port` 同一档，没有调它的理由。
const PROBE_INTERVAL: Duration = Duration::from_millis(200);

/// 单次 TCP 探测的超时。比间隔略长，免得机器一忙就每次都判失败。
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(400);

/// 超时报错时从子进程日志尾巴上带多少行。
const LOG_TAIL_LINES: usize = 20;

/// 署名所在的命名空间 / 库 / 表 / 记录 id。**固定**，与工程无关——任何一个 gen-model
/// 都要能读到它，包括配错工程的那一个（那正是要拦下的局面之一）。
pub const SIGNATURE_NS: &str = "gen_model";
pub const SIGNATURE_DB: &str = "store";
pub const SIGNATURE_TABLE: &str = "owner";
pub const SIGNATURE_ID: &str = "current";

/// 心跳多久没动就算「停了」。
///
/// 属主每 [`HEARTBEAT_INTERVAL`] 敲一次，这里给出六倍余量：GC、换页、机器很忙都不该
/// 让一个活着的服务被判成孤儿。判成孤儿的代价是**杀掉别人正在用的库**，所以这个数宁大勿小。
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(60);

/// 属主敲心跳的间隔。
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// 写在库里的那枚署名：「这台 SurrealDB 是谁拉起来的」。
///
/// 它必须写在**库里**而不是磁盘上：磁盘上的文件与端口上那台服务没有任何绑定关系，
/// 一份陈旧的 owner 文件配上一个被复用的 pid，就足以让我们把别人的库当成自家孤儿杀掉。
/// 写进库里则是自证——能从这个端口上读出这一行，就说明听在这个端口上的正是我们建的那个库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreSignature {
    /// 属主（gen-model）的进程号。收孤儿时按它杀。
    pub owner_pid: u32,
    /// 属主拉起的 surreal 子进程号。收孤儿时真正要杀的是它。
    pub child_pid: u32,
    /// 属主本次运行的随机串。pid 会被复用，这个不会。
    pub nonce: String,
    /// 属主服务的工程与命名空间。对不上 = 同一个端口上跑着另一份部署，拒。
    pub project: String,
    pub namespace: String,
    #[serde(default)]
    pub data_face: String,
    /// 属主最后一次敲心跳的 unix 毫秒。
    pub heartbeat_ms: u64,
    /// 属主起来的 unix 毫秒。只用于说话，不参与判定。
    pub started_at_ms: u64,
}

/// 本进程的身份，用来跟端口上读到的署名比对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIdentity {
    pub project: String,
    pub namespace: String,
    pub data_face: String,
}

/// 端口上那东西是什么，以及该拿它怎么办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePortVerdict {
    /// 端口空着：正常拉起一个。
    Idle,
    /// 自家的存储，属主还活着：**附着**。探针进程（`direct_model_persist` /
    /// `db_window_repair` / python `full_init`）跑起来时这是正常局面——它们要读的
    /// 正是服务手上那个库，不是另起一个空库。
    Attach(StoreSignature),
    /// 自家的孤儿（属主的心跳停了、进程也确实不在了）：杀掉重起。
    ReclaimOrphan {
        signature: StoreSignature,
        reason: String,
    },
    /// 分不清，或明确是别人的：**拒绝启动**。
    Refuse(String),
}

/// 从端口上读署名这一步的结局。三种要分开——「读不出来」与「读出来没有」说的不是一件事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureProbe {
    /// 读到了。
    Found(StoreSignature),
    /// 连上了、也查了，但那张表是空的：一台没有署名的 SurrealDB。
    Absent,
    /// 连不上 / 登录不了 / 查不动。带上原话。
    Unreadable(String),
}

/// 四格表。**任何一格分不清都落 [`StorePortVerdict::Refuse`]**——这张表唯一会做的破坏性
/// 动作是「杀掉端口上那个进程」，而它的反面（多等一次、让人自己看一眼）代价小得多。
pub fn judge_port(
    port_open: bool,
    probe: SignatureProbe,
    ours: &StoreIdentity,
    now_ms: u64,
    owner_alive: Option<bool>,
) -> StorePortVerdict {
    if !port_open {
        return StorePortVerdict::Idle;
    }
    let signature = match probe {
        SignatureProbe::Unreadable(error) => {
            return StorePortVerdict::Refuse(format!(
                "端口上有服务在跑，但读不出自家署名（{error}）。这多半是别人的 SurrealDB——\
                 也可能是同端口上另一套凭据。本进程不会去动它：换一个 v_port，\
                 或改用 store_mode = \"external\" 连它。"
            ));
        }
        SignatureProbe::Absent => {
            return StorePortVerdict::Refuse(
                "端口上是一台**没有署名**的 SurrealDB，不是本进程拉起来的。\
                 从外面分不出它是 memory 还是 rocksdb，连上去就可能把内存形态的语义悄悄换成\
                 落盘形态，所以拒绝启动。手工起的那一台请配 store_mode = \"external\"，\
                 或者停掉它让本进程自己起。"
                    .to_owned(),
            );
        }
        SignatureProbe::Found(signature) => signature,
    };
    if signature.project != ours.project
        || signature.namespace != ours.namespace
        || signature.data_face != ours.data_face
    {
        return StorePortVerdict::Refuse(format!(
            "端口上是**另一份部署**的自带存储（project={} ns={} data_face={}，本进程是 project={} ns={} data_face={}）。\
             两套工程共用一个端口只会互相盖掉，拒绝启动。",
            signature.project,
            signature.namespace,
            signature.data_face,
            ours.project,
            ours.namespace,
            ours.data_face
        ));
    }
    let idle_ms = now_ms.saturating_sub(signature.heartbeat_ms);
    if idle_ms <= HEARTBEAT_STALE_AFTER.as_millis() as u64 {
        // **心跳新不等于属主还活着。** 属主被强杀之后的那 60 秒里，心跳看着还新、
        // 写它的人却已经没了——这一格是 2026-09-04 P6 实测撞出来的：那时附着上去，
        // 新进程不认领这台库、也不接着敲心跳，于是它变成一台**没有主人**的库，
        // 再过 60 秒就会被下一个启动的进程当孤儿杀掉——而那时它正被人用着。
        //
        // 心跳新 + 属主明确不在 = 没有歧义（写心跳的那个已经死了），照孤儿收。
        // 查不出属主在不在时仍然附着：新鲜心跳是够强的证据，而这一格的反面是杀进程。
        if owner_alive == Some(false) {
            let reason = format!(
                "自家存储的属主 pid={} 已经不在了（心跳 {} 秒前还在写——多半是被强杀，析构没跑）",
                signature.owner_pid,
                idle_ms / 1000
            );
            return StorePortVerdict::ReclaimOrphan { signature, reason };
        }
        return StorePortVerdict::Attach(signature);
    }
    match owner_alive {
        Some(false) => {
            let reason = format!(
                "自家存储的属主已经不在了（pid={}，心跳停了 {} 秒）",
                signature.owner_pid,
                idle_ms / 1000
            );
            StorePortVerdict::ReclaimOrphan { signature, reason }
        }
        Some(true) => StorePortVerdict::Refuse(format!(
            "自家存储的心跳停了 {} 秒，但属主进程 pid={} 还在。分不清它是卡住了还是正在退出，\
             本进程不猜——杀错了就是把一个正在跑的服务的库端掉。等它退干净，或手工确认后再起。",
            idle_ms / 1000,
            signature.owner_pid
        )),
        None => StorePortVerdict::Refuse(format!(
            "自家存储的心跳停了 {} 秒，但查不出属主进程 pid={} 还在不在（进程探测失败）。\
             同上，不猜。",
            idle_ms / 1000,
            signature.owner_pid
        )),
    }
}

/// 现在的 unix 毫秒。系统时钟被拨到 1970 之前时回 0——那种机器上心跳判定本来就没意义，
/// 而 `expect` 会让服务起不来。
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 盖署名 / 敲心跳的那条语句。先删后插，与 ADR-060 的写入口径一致；
/// 单行表不必 UPSERT，也就没有「撞上旧行保留旧值」那一格。
pub fn signature_upsert_sql(signature: &StoreSignature) -> String {
    format!(
        "DELETE {table}:{id}; \
         INSERT INTO {table} {{ id: {table}:{id}, owner_pid: {owner_pid}, child_pid: {child_pid}, \
         nonce: {nonce}, project: {project}, namespace: {namespace}, data_face: {data_face}, \
         heartbeat_ms: {heartbeat}, started_at_ms: {started} }};",
        table = SIGNATURE_TABLE,
        id = SIGNATURE_ID,
        owner_pid = signature.owner_pid,
        child_pid = signature.child_pid,
        nonce = sql_string(&signature.nonce),
        project = sql_string(&signature.project),
        namespace = sql_string(&signature.namespace),
        data_face = sql_string(&signature.data_face),
        heartbeat = signature.heartbeat_ms,
        started = signature.started_at_ms,
    )
}

/// 读署名那条语句。
pub fn signature_select_sql() -> String {
    format!("SELECT * FROM {SIGNATURE_TABLE}:{SIGNATURE_ID};")
}

/// 单引号字符串字面量。工程名与命名空间是配置里来的，不能直接拼进语句。
fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// 本次运行的随机串。不必是密码学随机——它只要在「pid 被复用」这个尺度上不重复。
fn fresh_nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", std::process::id(), now)
}

/// 进程还在不在。`None` = 查不出来（探测命令自己失败了），调用方必须把它当「不确定」，
/// 不能当「不在」——那个方向的误判是杀掉一个活着的服务的库。
pub fn process_exists(pid: u32) -> Option<bool> {
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // 没命中时 tasklist 回的是一句「没有运行的任务匹配指定标准」，不是空——
        // 所以判据是「输出里出现了带引号的那个 pid」，不是「输出非空」。
        Some(text.contains(&format!("\"{pid}\"")))
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        Some(status.success())
    }
}

fn binary_file_name() -> &'static str {
    if cfg!(windows) {
        "surreal.exe"
    } else {
        "surreal"
    }
}

/// 二进制的查找顺序：`AIOS_SURREAL_EXE` → 可执行文件旁边的 `bin/` → 仓根 `bin/`。
///
/// **刻意不含 PATH**（`Start-Surreal8009.ps1:73` 同一条纪律）。部署形态下第二项是有效的那个：
/// 发布包把 surreal 放在服务二进制旁边的 `bin/`；开发形态下命中的是第三项。
pub fn surreal_exe_candidates(
    env_override: Option<&OsStr>,
    exe_dir: Option<&Path>,
    repo_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(explicit) = env_override.filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(explicit));
    }
    for base in [exe_dir, repo_root].into_iter().flatten() {
        let candidate = base.join("bin").join(binary_file_name());
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// 一个候选都不存在时的说明。照抄 PS1 那段，因为人看到它的时候多半正准备去 PATH 上找一个。
pub fn missing_binary_error(candidates: &[PathBuf]) -> String {
    let listed = candidates
        .iter()
        .map(|path| format!("  - {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "找不到可用的 SurrealDB 服务端二进制（store_mode = spawned-mem 需要它）。\n\
         查找顺序：{SURREAL_EXE_ENV} → 可执行文件旁的 bin/ → 仓根 bin/\n{listed}\n\n\
         **不会回退到 PATH**：那上面通常是 cargo install 装的 3.x，用它会撞上存储版本不兼容与 \
         WS 子协议不兼容两个坑。请把 {} 的二进制放到 bin/，或用 {SURREAL_EXE_ENV} 指定；\n\
         也可以改用 store_mode = \"external\"，自己起一台 SurrealDB。",
        format_args!("{REQUIRED_MAJOR}.1.x"),
    )
}

/// 按 [`surreal_exe_candidates`] 找出第一个真实存在的文件。
pub fn resolve_surreal_exe() -> anyhow::Result<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let repo_root = std::env::current_dir().ok();
    let candidates = surreal_exe_candidates(
        std::env::var_os(SURREAL_EXE_ENV).as_deref(),
        exe_dir.as_deref(),
        repo_root.as_deref(),
    );
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| anyhow!(missing_binary_error(&candidates)))
}

/// 从 `surreal version` 的自报里取大版本号。它长这样：
/// `2.1.4+20250317.45013fc9 for windows on x86_64`。
pub fn parse_version_major(version_output: &str) -> Option<u32> {
    let digits = version_output
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find(|part| part.contains('.') && part.starts_with(|c: char| c.is_ascii_digit()))?;
    digits.split('.').next()?.parse().ok()
}

/// `Cargo.lock` 里锁定的 surrealdb fork rev。fork 的 2.1.4 与上游 2.1.4 不是一回事。
pub fn locked_surreal_rev(cargo_lock: &str) -> Option<&str> {
    let tail = cargo_lock.split("surrealdb.git#").nth(1)?;
    let rev = tail
        .split(|c: char| !c.is_ascii_hexdigit())
        .next()
        .unwrap_or_default();
    (rev.len() >= 7).then_some(rev)
}

/// 版本闸：`Err` = 拒绝启动；`Ok(Some(告警))` = 大版本对上但 rev 对不上，照跑但要出声；
/// `Ok(None)` = 干净。
///
/// 两档分开是有理由的：大版本不对是**确定**会炸（两个坑都复现过），rev 不对只是**可能**
/// 在解码或协议层出怪问题——把后者也做成硬拒，会让任何一次 fork 更新都先卡住所有人。
pub fn version_gate(
    exe_display: &str,
    version_output: &str,
    locked_rev: Option<&str>,
) -> Result<Option<String>, String> {
    let version = version_output.trim();
    let Some(major) = parse_version_major(version) else {
        return Err(format!(
            "无法从 '{exe_display}' 解析版本号，实际输出：{version}"
        ));
    };
    if major != REQUIRED_MAJOR {
        return Err(format!(
            "SurrealDB 服务端版本不匹配：{exe_display} 是 {version}，但本仓库的客户端 SDK 是 \
             {REQUIRED_MAJOR}.1.x（Cargo.lock: gitee.com/happydpc/surrealdb.git）。\n\
             用它会撞上两个坑：\n\
             \x20 * 打开既有 rocksdb 数据目录会要求不可逆迁移；\n\
             \x20 * 即便用 memory 后端，也会报 'Server sent no subprotocol'。\n\
             请改用 {REQUIRED_MAJOR}.1.x 的二进制，或用 {SURREAL_EXE_ENV} 指定。"
        ));
    }
    let Some(locked) = locked_rev else {
        return Ok(None);
    };
    let short = &locked[..locked.len().min(8)];
    if version.contains(short) {
        return Ok(None);
    }
    Ok(Some(format!(
        "⚠ SurrealDB 服务端自报 '{version}'，其中没有 Cargo.lock 锁定的 rev {short}。\
         大版本对得上所以继续启动；如果遇到解码或协议层的怪问题，先怀疑这里。"
    )))
}

/// 尽力找一份 `Cargo.lock` 读 rev。部署形态下多半找不到——那不是错误，PS1 里也是
/// `if ($lockedRev)`：rev 比对是锦上添花，大版本闸才是那道门。
fn locked_surreal_rev_from_disk() -> Option<String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("Cargo.lock"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("Cargo.lock"));
        if let Some(parent) = dir.parent() {
            candidates.push(parent.join("Cargo.lock"));
        }
    }
    candidates
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|text| locked_surreal_rev(&text).map(str::to_owned))
}

/// 配置里那对 `v_ip:v_port` 解出来的三个地址口径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreBind {
    /// 交给 `--bind` 的那个（必须是 socket 地址，主机名不行）。
    pub bind: SocketAddr,
    /// 起来之后往哪儿探活。`--bind` 是 `0.0.0.0` 时探回环。
    pub probe: SocketAddr,
    /// 回执与 `/health` 上报的地址，**保持配置里的字面形态**——跨部署互踩探测按它比对
    /// （`handlers.rs` 那一栏），改写成解析后的 IP 会让同一台库在两处长得不一样。
    pub endpoint: String,
}

/// 把 `v_ip` / `v_port` 解成可以喂给 `--bind` 的地址。
///
/// `--bind` 只吃 socket 地址，而配置里写的是 `localhost`——直接透传会让 surreal 在参数解析
/// 阶段就退出，日志里只有一行看不出所以然的用法提示。
///
/// 主机名（`localhost` 之外的）一律拒绝：解析出来的地址可能压根不在本机上，
/// 而「在本机拉起一个绑到别人机器上的进程」是做不到的事，早说比等 bind 失败强。
pub fn resolve_bind(v_ip: &str, v_port: u16) -> anyhow::Result<StoreBind> {
    let endpoint = format!("{v_ip}:{v_port}");
    let host = v_ip.trim();
    let ip: IpAddr = match host {
        "" | "localhost" => IpAddr::from([127, 0, 0, 1]),
        other => other.parse().map_err(|_| {
            anyhow!(
                "store_mode = spawned-mem 需要把 SurrealDB 绑到本机的一个地址，而 v_ip = {other:?} \
                 是主机名不是 IP，没法交给 --bind。请把 v_ip 写成 IP（本机用 127.0.0.1），\
                 或改用 store_mode = \"external\" 连一台已经在跑的库。"
            )
        })?,
    };
    let bind = SocketAddr::new(ip, v_port);
    let probe = if ip.is_unspecified() {
        // `0.0.0.0` / `::` 上探不了活，回环是它一定覆盖到的那一个。
        SocketAddr::new(
            if ip.is_ipv6() {
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1])
            } else {
                IpAddr::from([127, 0, 0, 1])
            },
            v_port,
        )
    } else {
        bind
    };
    Ok(StoreBind {
        bind,
        probe,
        endpoint,
    })
}

/// 自带子进程的数据源：`spawned-mem` 给 memory，`spawned-rocksdb` 给一个 rocksdb 目录。
/// 从外面看两台库长得一模一样（署名 / 四格裁决 / 收尸全同构），差别只有这一个位置参数
/// 与「行活不活得过重启」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnDatastore {
    /// `--memory` 后端：进程一退整库即消失。
    Memory,
    /// rocksdb 目录：数据落盘，活得过重启（ADR-070）。
    RocksDb(PathBuf),
}

impl SpawnDatastore {
    /// `surreal start` 的位置参数形态。
    pub fn as_positional(&self) -> String {
        match self {
            Self::Memory => "memory".into(),
            Self::RocksDb(dir) => format!("rocksdb:{}", dir.display()),
        }
    }

    /// 启动横幅里「介质」那半句。
    pub fn medium_banner(&self) -> String {
        match self {
            Self::Memory => "介质 memory，进程一退整库即消失".into(),
            Self::RocksDb(dir) => {
                format!("介质 rocksdb，数据落 {}，活得过重启", dir.display())
            }
        }
    }
}

/// `spawned-rocksdb` 的数据目录：`<workspace>\.surreal\<project_code>`（workspace =
/// 本进程实际在读的配置文件所在目录）。**按项目代号分家**：换代号 = 新目录新库，旧目录
/// 留在盘上人工清（ADR-070——这不是丢数据）。代号空 / "0"（出厂哨兵）时回落项目名，
/// 再回落 `ams`；目录名只保留路径安全字符。
pub fn default_rocksdb_dir(project_code: &str, project_name: &str) -> PathBuf {
    let sanitize = |raw: &str| -> String {
        raw.trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect()
    };
    let code = sanitize(project_code);
    let name = sanitize(project_name);
    let leaf = if !code.is_empty() && code != "0" {
        code
    } else if !name.is_empty() {
        name
    } else {
        "ams".to_string()
    };
    let workspace = crate::options::active_config_path()
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    workspace.join(".surreal").join(leaf)
}

/// `surreal start` 的参数表。数据源是**位置参数**，不是选项——写成
/// `--memory` 在 2.1.x 上是「未知参数」。
pub fn spawn_args(
    user: &str,
    password: &str,
    bind: SocketAddr,
    datastore: &SpawnDatastore,
) -> Vec<String> {
    vec![
        "start".into(),
        "--user".into(),
        user.to_owned(),
        "--pass".into(),
        password.to_owned(),
        "--bind".into(),
        bind.to_string(),
        datastore.as_positional(),
    ]
}

/// 日志尾巴。超时那条错误里不带它，等于让人自己去猜子进程为什么没起来。
pub fn log_tail(text: &str, lines: usize) -> String {
    let collected: Vec<&str> = text.lines().rev().take(lines).collect();
    collected
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

/// 等端口超时的那条错误。
pub fn spawn_timeout_error(
    endpoint: &str,
    timeout: Duration,
    log_path: &Path,
    tail: &str,
) -> String {
    let tail = if tail.trim().is_empty() {
        "（子进程一个字都没输出）".to_owned()
    } else {
        tail.to_owned()
    };
    format!(
        "拉起的 SurrealDB 在 {:?} 内没有开始监听 {endpoint}。它自己的日志在 {}，最后几行：\n{tail}",
        timeout,
        log_path.display()
    )
}

/// 自带的 memory 后端 SurrealDB 子进程。
///
/// **它不许活过父进程**：正常退出与 panic 走 [`Drop`]，`Ctrl+C` 由调用方在信号处理里调
/// [`shutdown_spawned_store`]。父进程被强杀那一条兜不住（`Drop` 不跑），留给 P2 的
/// 端口署名与孤儿回收——那也是本形态最难看的失败模式：孤儿占着端口，下一次启动会
/// 「成功」地连上它，里头装着上一轮的半截数据。
#[derive(Debug)]
pub struct SpawnedStore {
    child: Option<Child>,
    pid: u32,
    exe: PathBuf,
    version: String,
    bind: StoreBind,
    log_path: PathBuf,
}

impl SpawnedStore {
    /// 起一个，等到端口能连上为止。
    ///
    /// 失败一律响亮返回，**不回落到别的介质**：静默回落等于「用户以为自己在内存形态，
    /// 实际连上了昨天那台落盘库」，而那正是 `store_mode` 这个键要消灭的沉默。
    pub fn start(
        v_ip: &str,
        v_port: u16,
        v_user: &str,
        v_password: &str,
        datastore: &SpawnDatastore,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        Self::start_on(
            resolve_bind(v_ip, v_port)?,
            v_user,
            v_password,
            datastore,
            timeout,
        )
    }

    /// [`Self::start`] 的后半段，地址已经解好。[`ensure_spawned_store`] 走这一条——
    /// 它在判端口的时候就把地址解出来了，不必再解一次（也不该，那等于把 §3.3 的裁决
    /// 建立在两次可能不同的解析上）。
    pub fn start_on(
        bind: StoreBind,
        v_user: &str,
        v_password: &str,
        datastore: &SpawnDatastore,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        let exe = resolve_surreal_exe()?;
        let exe_display = exe.display().to_string();

        let output = Command::new(&exe)
            .arg("version")
            .output()
            .with_context(|| format!("执行 {exe_display} version 失败"))?;
        let version = String::from_utf8_lossy(&output.stdout)
            .trim()
            .replace(['\r', '\n'], " ");
        match version_gate(
            &exe_display,
            &version,
            locked_surreal_rev_from_disk().as_deref(),
        ) {
            Err(refusal) => bail!(refusal),
            Ok(Some(warning)) => eprintln!("{warning}"),
            Ok(None) => {}
        }

        if let SpawnDatastore::RocksDb(dir) = datastore {
            // rocksdb 往既有目录里初始化没问题，父目录不存在才会让 surreal 起不来。
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建持久库数据目录 {} 失败", dir.display()))?;
        }
        let log_path = spawn_log_path(bind.bind.port());
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建日志目录 {} 失败", parent.display()))?;
        }
        // 子进程的输出必须落文件而不是继承控制台：surreal 自己的启动横幅会插进 gen-model
        // 的启动日志中间，而那一段正是出问题时第一批要读的东西。
        let log = File::create(&log_path)
            .with_context(|| format!("创建子进程日志 {} 失败", log_path.display()))?;
        let log_err = log
            .try_clone()
            .with_context(|| format!("复制日志句柄 {} 失败", log_path.display()))?;
        let child = Command::new(&exe)
            // The fork's default buffered response timer adds ~15 ms to each
            // sequential RPC on Windows. Flush responses immediately; this
            // changes no transaction/WAL durability setting. Explicit overrides win.
            .env(
                "SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE",
                std::env::var_os("SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE")
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "0".into()),
            )
            .args(spawn_args(v_user, v_password, bind.bind, datastore))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .with_context(|| format!("拉起 {exe_display} 失败"))?;
        let pid = child.id();

        let mut store = Self {
            child: Some(child),
            pid,
            exe,
            version,
            bind,
            log_path,
        };
        if !wait_port(store.bind.probe, timeout) {
            let tail = log_tail(&read_to_string_lossy(&store.log_path), LOG_TAIL_LINES);
            let message =
                spawn_timeout_error(&store.bind.endpoint, timeout, &store.log_path, &tail);
            // 起不来的那个也要收掉，否则它会一边占着端口一边没人认领。
            store.shutdown();
            bail!(message);
        }
        println!(
            "*** 存储子进程已就绪：pid={} 监听 {}（{}）",
            store.pid,
            store.bind.endpoint,
            datastore.medium_banner()
        );
        println!("    二进制 {} / {}", store.exe.display(), store.version);
        println!("    子进程日志：{}", store.log_path.display());
        Ok(store)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 回执与 `/health` 用的地址，保持配置里的字面形态。
    pub fn endpoint(&self) -> &str {
        &self.bind.endpoint
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    /// 收尸。**幂等**——`Drop` 与信号处理会各调一次，第二次是空操作。
    pub fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        kill_tree(self.pid);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for SpawnedStore {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 端口上的 ws 地址。探针与心跳都连它。
pub fn ws_url(probe: SocketAddr) -> String {
    format!("ws://{probe}")
}

/// 连上端口上那台库，只为读 / 写署名。**与生产连接分开**：这一步发生在「要不要用这个库」
/// 还没定之前，不能污染 `SUL_DB` 那个全局句柄。
async fn connect_signature_db(
    probe: SocketAddr,
    user: &str,
    password: &str,
) -> anyhow::Result<surrealdb::Surreal<surrealdb::engine::any::Any>> {
    use surrealdb::engine::any::connect;
    use surrealdb::opt::Config;
    use surrealdb::opt::auth::Root;

    let handle = connect((ws_url(probe), Config::default().ast_payload()))
        .await
        .with_context(|| format!("连 {} 失败", ws_url(probe)))?;
    handle
        .signin(Root {
            username: user,
            password,
        })
        .await
        .context("以 root 登录失败")?;
    handle
        .use_ns(SIGNATURE_NS)
        .use_db(SIGNATURE_DB)
        .await
        .context("切到署名库失败")?;
    Ok(handle)
}

/// 读端口上那枚署名。三种结局分开返回，[`judge_port`] 靠它们分格。
pub async fn probe_signature(probe: SocketAddr, user: &str, password: &str) -> SignatureProbe {
    let handle = match connect_signature_db(probe, user, password).await {
        Ok(handle) => handle,
        Err(error) => return SignatureProbe::Unreadable(format!("{error:#}")),
    };
    match handle.query(signature_select_sql()).await {
        Err(error) => SignatureProbe::Unreadable(format!("查询署名失败：{error}")),
        Ok(mut response) => match response.take::<Option<StoreSignature>>(0) {
            Err(error) => SignatureProbe::Unreadable(format!("署名这一行解不动：{error}")),
            Ok(None) => SignatureProbe::Absent,
            Ok(Some(signature)) => SignatureProbe::Found(signature),
        },
    }
}

/// 盖一次署名（也用于敲心跳）。
pub async fn stamp_signature(
    handle: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    signature: &StoreSignature,
) -> anyhow::Result<()> {
    handle
        .query(signature_upsert_sql(signature))
        .await
        .context("写署名失败")?
        .check()
        .context("写署名的语句被拒")?;
    Ok(())
}

/// 本进程拿到的那个库：自己起的，还是附着到别人（自家服务）已经起好的那一个。
#[derive(Debug)]
pub enum StoreHandle {
    /// 自己拉起来的。要收尸。
    Spawned(SpawnedStore),
    /// 附着到本部署已经在跑的那一个。**不收尸**——它不是我们的。
    Attached(StoreSignature),
}

impl StoreHandle {
    pub fn is_attached(&self) -> bool {
        matches!(self, Self::Attached(_))
    }
}

/// 这一次调用有没有资格把库拉起来。
///
/// **只有服务主进程是 [`StoreStartup::Spawn`]。** 探针、一次性工具、python 调试入口一律
/// [`StoreStartup::AttachOnly`]：它们要读的是服务手上那个库，端口空着时正确的反应是
/// **响亮地说「服务没起」**，而不是自己拉一台空库然后在上面作业——那种失败一声不响，
/// 现象是「查了半天什么都没有」，最难查的那一类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreStartup {
    /// 端口空着就拉一台；撞上自家孤儿就收掉重起。服务主进程用。
    Spawn,
    /// 只附着，绝不拉起、也绝不杀任何东西。
    AttachOnly,
}

/// 「让这个端口上有一个可用的自带存储」的完整一步：判端口 → 按四格表行事 → 盖署名 → 起心跳。
///
/// 失败一律响亮返回，**不回落到别的介质**。
pub async fn ensure_spawned_store(
    v_ip: &str,
    v_port: u16,
    v_user: &str,
    v_password: &str,
    identity: &StoreIdentity,
    startup: StoreStartup,
    timeout: Duration,
    datastore: &SpawnDatastore,
) -> anyhow::Result<StoreHandle> {
    let bind = resolve_bind(v_ip, v_port)?;
    let port_open = TcpStream::connect_timeout(&bind.probe, PROBE_CONNECT_TIMEOUT).is_ok();
    let probe = if port_open {
        probe_signature(bind.probe, v_user, v_password).await
    } else {
        SignatureProbe::Absent
    };
    let owner_alive = match &probe {
        SignatureProbe::Found(signature) => process_exists(signature.owner_pid),
        _ => None,
    };
    match judge_port(port_open, probe, identity, now_unix_ms(), owner_alive) {
        StorePortVerdict::Refuse(reason) => bail!(reason),
        StorePortVerdict::Attach(signature) => {
            println!(
                "*** 附着到本部署已经在跑的自带存储：{} （属主 pid={}，本进程不负责收尸）",
                bind.endpoint, signature.owner_pid
            );
            Ok(StoreHandle::Attached(signature))
        }
        StorePortVerdict::ReclaimOrphan { signature, reason } => {
            if startup == StoreStartup::AttachOnly {
                bail!(
                    "{reason}——本进程只附着、不收尸（探针与一次性工具没有资格杀别人留下的进程）。\
                     先起一次 gen-model 让它回收，或手工清掉 pid={}。",
                    signature.child_pid
                );
            }
            println!(
                "*** 回收孤儿存储进程：{reason}，杀掉 pid={}",
                signature.child_pid
            );
            reclaim_orphan(signature.child_pid, bind.probe, timeout)?;
            spawn_and_stamp(&bind, v_user, v_password, identity, datastore, timeout).await
        }
        StorePortVerdict::Idle => {
            if startup == StoreStartup::AttachOnly {
                bail!(
                    "{} 上没有自带存储在跑（store_mode = spawned-mem / spawned-rocksdb）。\
                     本进程只附着、不拉起——自己拉一台空库、再在上面作业，失败会一声不响。\
                     先起 gen-model，或改 store_mode = \"external\" 指一台已有的库。",
                    bind.endpoint
                );
            }
            spawn_and_stamp(&bind, v_user, v_password, identity, datastore, timeout).await
        }
    }
}

/// 杀掉孤儿并**等端口真的放开**。不等的话紧接着的那次 `--bind` 会撞 address already in use，
/// 而那条错误只会落在子进程自己的日志里。
fn reclaim_orphan(child_pid: u32, probe: SocketAddr, timeout: Duration) -> anyhow::Result<()> {
    kill_tree(child_pid);
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect_timeout(&probe, PROBE_CONNECT_TIMEOUT).is_err() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "杀掉孤儿存储进程 pid={child_pid} 之后，{probe} 在 {timeout:?} 内仍然被占着。\
                 请手工确认是谁在占这个端口。"
            );
        }
        std::thread::sleep(PROBE_INTERVAL);
    }
}

async fn spawn_and_stamp(
    bind: &StoreBind,
    v_user: &str,
    v_password: &str,
    identity: &StoreIdentity,
    datastore: &SpawnDatastore,
    timeout: Duration,
) -> anyhow::Result<StoreHandle> {
    let store = SpawnedStore::start_on(bind.clone(), v_user, v_password, datastore, timeout)?;
    let now = now_unix_ms();
    let signature = StoreSignature {
        owner_pid: std::process::id(),
        child_pid: store.pid(),
        nonce: fresh_nonce(),
        project: identity.project.clone(),
        namespace: identity.namespace.clone(),
        data_face: identity.data_face.clone(),
        heartbeat_ms: now,
        started_at_ms: now,
    };
    let handle = connect_signature_db(bind.probe, v_user, v_password).await?;
    stamp_signature(&handle, &signature).await?;
    spawn_heartbeat(handle, signature);
    Ok(StoreHandle::Spawned(store))
}

/// 心跳。停了就说明属主没了，下一个起来的进程据此把这台库当孤儿收掉。
///
/// 连不上就退出并出声——**不静默重试到天荒地老**：心跳停了而进程还在，会让下一个启动的
/// 进程落进 [`judge_port`] 那一格「分不清，拒绝启动」，那时日志里得有话可查。
fn spawn_heartbeat(
    handle: surrealdb::Surreal<surrealdb::engine::any::Any>,
    mut signature: StoreSignature,
) {
    tokio::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            signature.heartbeat_ms = now_unix_ms();
            match stamp_signature(&handle, &signature).await {
                Ok(()) => consecutive_failures = 0,
                Err(error) => {
                    consecutive_failures += 1;
                    eprintln!("自带存储心跳写入失败（第 {consecutive_failures} 次）：{error:#}");
                    if consecutive_failures >= 3 {
                        eprintln!(
                            "自带存储心跳连续 3 次失败，停止心跳。下一次启动会把这台库判成\
                             「心跳停了但属主还在」并拒绝启动——那时请看本进程的日志。"
                        );
                        return;
                    }
                }
            }
        }
    });
}

/// 进程内那一个。P3 起完之后放进来，信号处理与正常退出都从这里收尸。
static SPAWNED: OnceLock<Mutex<Option<SpawnedStore>>> = OnceLock::new();

fn spawned_slot() -> &'static Mutex<Option<SpawnedStore>> {
    SPAWNED.get_or_init(|| Mutex::new(None))
}

/// 交管。同一个进程只该有一个自带存储；重复放进来时把旧的先收掉，
/// 免得静默漏一个占着端口的孤儿。
pub fn install_spawned_store(store: SpawnedStore) {
    let mut slot = spawned_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut previous) = slot.replace(store) {
        previous.shutdown();
    }
}

/// 收掉自带存储（幂等）。`Ctrl+C`、正常退出、`run_app` 出错返回都该调它一次。
pub fn shutdown_spawned_store() {
    let taken = spawned_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(mut store) = taken {
        println!("正在收掉存储子进程 pid={}…", store.pid());
        store.shutdown();
    }
}

/// 本进程此刻有没有自带存储，有的话它的地址是什么。`/health` 那一栏用得上（P4）。
pub fn spawned_endpoint() -> Option<String> {
    spawned_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|store| store.endpoint().to_owned())
}

fn spawn_log_path(port: u16) -> PathBuf {
    PathBuf::from("logs").join(format!("surreal-{port}.log"))
}

fn read_to_string_lossy(path: &Path) -> String {
    let mut buffer = Vec::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_end(&mut buffer);
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

fn wait_port(addr: SocketAddr, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect_timeout(&addr, PROBE_CONNECT_TIMEOUT).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(PROBE_INTERVAL);
    }
}

/// 连子孙一起杀。Windows 上 `Child::kill` 只杀那一个进程，surreal 自己不开子进程，
/// 但 `taskkill /T` 是 `bin/l3_suite` 跑了几个月的那一份，没有理由在这里换一种写法。
#[cfg(windows)]
fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn kill_tree(_pid: u32) {
    // 非 Windows 上交给 `Child::kill`：surreal 不开子进程，进程组那一层不必自己造。
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rkyv` 也给 `SocketAddr` 实现了 `PartialEq`，裸 `.parse()` 在断言里推不出类型。
    fn sock(text: &str) -> SocketAddr {
        text.parse().expect("测试里的地址应当合法")
    }

    /// 查找顺序的**全部**内容：显式变量优先，其次是可执行文件旁的 `bin/`，最后是仓根
    /// `bin/`。名单里不许出现 PATH——那上面通常是 3.x，而它的失败方式会把人引向
    /// 「按提示迁移数据」这条不可逆的错路（`Start-Surreal8009.ps1:73` 同一条纪律）。
    #[test]
    fn the_binary_search_order_never_falls_back_to_path() {
        let candidates = surreal_exe_candidates(
            Some(OsStr::new("D:/tools/surreal-2.1.4.exe")),
            Some(Path::new("/opt/gen-model")),
            Some(Path::new("/repo")),
        );
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0], PathBuf::from("D:/tools/surreal-2.1.4.exe"));
        assert!(candidates[1].starts_with("/opt/gen-model"));
        assert!(candidates[2].starts_with("/repo"));
        assert!(candidates.iter().all(|c| c.parent().is_some()));
    }

    /// 空的环境变量当没设——`$env:AIOS_SURREAL_EXE = ""` 是清空它最自然的写法，
    /// 把空串当路径会让查找在第一项就撞死。
    #[test]
    fn an_empty_env_override_is_not_a_candidate() {
        let candidates =
            surreal_exe_candidates(Some(OsStr::new("")), Some(Path::new("/opt")), None);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].starts_with("/opt"));
    }

    /// 同一个目录被当成 exe_dir 与 repo_root 时只留一项：名单会原样进错误文案，
    /// 里面出现两行一模一样的路径只会让人怀疑自己看错了。
    #[test]
    fn duplicate_bases_collapse_to_one_candidate() {
        let here = Path::new("/repo");
        let candidates = surreal_exe_candidates(None, Some(here), Some(here));
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn the_version_line_yields_its_major() {
        assert_eq!(
            parse_version_major("2.1.4+20250317.45013fc9 for windows on x86_64"),
            Some(2)
        );
        assert_eq!(parse_version_major("3.0.1 for linux on x86_64"), Some(3));
        assert_eq!(parse_version_major("surreal 2.1.4"), Some(2));
        assert_eq!(parse_version_major("no version here"), None);
    }

    /// 大版本不对是**确定**会炸的那一档，所以硬拒；错误里必须点名那两个坑，
    /// 否则人看到 'Server sent no subprotocol' 只会去查网络。
    #[test]
    fn a_three_x_binary_is_refused_and_names_both_traps() {
        let refusal = version_gate("bin/surreal.exe", "3.0.1 for windows on x86_64", None)
            .expect_err("3.x 必须被拒");
        assert!(refusal.contains("3.0.1"), "{refusal}");
        assert!(refusal.contains("不可逆迁移"), "{refusal}");
        assert!(refusal.contains("subprotocol"), "{refusal}");
        assert!(refusal.contains(SURREAL_EXE_ENV), "{refusal}");
    }

    #[test]
    fn an_unparseable_version_is_refused_with_the_raw_output() {
        let refusal = version_gate("bin/surreal.exe", "command not found", None)
            .expect_err("解析不出版本号必须被拒");
        assert!(refusal.contains("command not found"), "{refusal}");
    }

    /// rev 对不上只警告不拒：那只是**可能**出怪问题，做成硬拒会让任何一次 fork 更新
    /// 先卡住所有人。
    #[test]
    fn a_rev_mismatch_only_warns_while_a_match_stays_silent() {
        let version = "2.1.4+20250317.45013fc9 for windows on x86_64";
        assert_eq!(
            version_gate("bin/surreal.exe", version, Some("45013fc9e74f6a11")),
            Ok(None)
        );
        let warning = version_gate("bin/surreal.exe", version, Some("deadbeefcafe"))
            .expect("rev 不符不该拒绝启动")
            .expect("rev 不符必须出声");
        assert!(warning.contains("deadbeef"), "{warning}");
        assert!(warning.contains("继续启动"), "{warning}");
        // 拿不到 Cargo.lock 时不编造警告（部署形态下这是常态）。
        assert_eq!(version_gate("bin/surreal.exe", version, None), Ok(None));
    }

    /// 锁文件里那一行的形状，用**真的那一份**验——手搓的样本证明不了正则跟得上
    /// `Cargo.lock` 的实际写法。
    #[test]
    fn the_locked_rev_comes_out_of_the_real_cargo_lock() {
        let lock = include_str!("../Cargo.lock");
        let rev = locked_surreal_rev(lock).expect("Cargo.lock 里应该有 surrealdb.git# 那一行");
        assert!(rev.len() >= 7, "{rev}");
        assert!(rev.chars().all(|c| c.is_ascii_hexdigit()), "{rev}");
        assert!(locked_surreal_rev("没有那一行").is_none());
    }

    /// `localhost` 必须被换成回环 IP：`--bind` 只吃 socket 地址，透传主机名会让 surreal
    /// 在参数解析阶段就退出，日志里只剩一行用法提示。
    #[test]
    fn localhost_becomes_a_loopback_socket_but_the_endpoint_keeps_its_wording() {
        let bind = resolve_bind("localhost", 8009).expect("localhost 必须能解");
        assert_eq!(bind.bind, sock("127.0.0.1:8009"));
        assert_eq!(bind.probe, bind.bind);
        // 互踩探测按 /health 上这一栏比对，改写它会让同一台库在两处长得不一样。
        assert_eq!(bind.endpoint, "localhost:8009");
    }

    /// 绑 `0.0.0.0` 时探回环——在未指定地址上探活是探不通的。
    #[test]
    fn an_unspecified_bind_is_probed_on_loopback() {
        let bind = resolve_bind("0.0.0.0", 8009).expect("0.0.0.0 必须能解");
        assert_eq!(bind.bind, sock("0.0.0.0:8009"));
        assert_eq!(bind.probe, sock("127.0.0.1:8009"));
    }

    /// 主机名（`localhost` 之外）早拒：它可能压根不在本机上，而「在本机拉起一个绑到
    /// 别人机器上的进程」做不到。错误里要给出路，否则人只会把 v_ip 反复改来改去。
    #[test]
    fn a_remote_hostname_is_refused_with_a_way_out() {
        let error = resolve_bind("db.internal", 8009).expect_err("主机名必须被拒");
        let text = error.to_string();
        assert!(text.contains("db.internal"), "{text}");
        assert!(text.contains("external"), "{text}");
    }

    /// 数据源是位置参数不是选项。写成 `--memory` 在 2.1.x 上是「未知参数」，
    /// 而那条错误只会出现在子进程自己的日志里。rocksdb 档同一个位置，形态 `rocksdb:<目录>`。
    #[test]
    fn the_datastore_is_a_positional_argument_not_a_flag() {
        let args = spawn_args(
            "root",
            "root",
            sock("127.0.0.1:8009"),
            &SpawnDatastore::Memory,
        );
        assert_eq!(args.last().map(String::as_str), Some("memory"));
        assert!(!args.iter().any(|a| a == "--memory"));
        assert_eq!(
            args,
            vec![
                "start",
                "--user",
                "root",
                "--pass",
                "root",
                "--bind",
                "127.0.0.1:8009",
                "memory"
            ]
        );
        let rocks = spawn_args(
            "root",
            "root",
            sock("127.0.0.1:8010"),
            &SpawnDatastore::RocksDb(PathBuf::from(r"D:\ws\.surreal\1516")),
        );
        assert_eq!(
            rocks.last().map(String::as_str),
            Some(r"rocksdb:D:\ws\.surreal\1516")
        );
        assert!(!rocks.iter().any(|a| a == "memory"));
    }

    /// 持久库目录按项目代号分家；代号空 / "0"（出厂哨兵）回落项目名，再回落 ams；
    /// 目录名滤掉路径不安全字符。换代号 = 新目录新库，旧目录留盘人工清（ADR-070）。
    #[test]
    fn the_rocksdb_dir_is_keyed_by_project_code_with_fallbacks() {
        let leaf = |code: &str, name: &str| -> String {
            default_rocksdb_dir(code, name)
                .file_name()
                .expect("目录必有叶子名")
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(leaf("1516", "AvevaMarineSample"), "1516");
        assert_eq!(leaf("0", "AvevaMarineSample"), "AvevaMarineSample");
        assert_eq!(leaf("", "AvevaMarineSample"), "AvevaMarineSample");
        assert_eq!(leaf("", ""), "ams");
        assert_eq!(leaf(" 15/16 ", ""), "1516", "路径不安全字符要滤掉");
        let dir = default_rocksdb_dir("1516", "x");
        assert!(
            dir.parent()
                .is_some_and(|parent| parent.ends_with(".surreal")),
            "必须落在 workspace 的 .surreal 下：{}",
            dir.display()
        );
    }

    #[test]
    fn the_log_tail_keeps_the_last_lines_in_order() {
        let text = (1..=30)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = log_tail(&text, 3);
        assert_eq!(tail, "line 28\nline 29\nline 30");
        assert_eq!(log_tail("", 3), "");
    }

    /// 超时那条错误不带日志尾巴，等于让人自己去猜子进程为什么没起来；
    /// 而「一个字都没输出」本身也是一条线索，不能显示成空白。
    #[test]
    fn the_timeout_error_carries_the_log_path_and_its_tail() {
        let message = spawn_timeout_error(
            "localhost:8009",
            Duration::from_secs(30),
            Path::new("logs/surreal-8009.log"),
            "Error: address already in use",
        );
        assert!(message.contains("localhost:8009"), "{message}");
        assert!(message.contains("logs"), "{message}");
        assert!(message.contains("address already in use"), "{message}");

        let silent = spawn_timeout_error(
            "localhost:8009",
            Duration::from_secs(30),
            Path::new("logs/surreal-8009.log"),
            "   ",
        );
        assert!(silent.contains("一个字都没输出"), "{silent}");
    }

    #[test]
    fn the_log_path_is_named_after_the_port() {
        assert!(
            spawn_log_path(8009)
                .to_string_lossy()
                .contains("surreal-8009")
        );
    }

    /// 一个候选都不存在时，错误要把查找顺序原样列出来，并明说不回退 PATH——
    /// 人看到它的时候多半正准备去 PATH 上找一个。
    #[test]
    fn the_missing_binary_error_lists_the_order_and_forbids_path() {
        let message = missing_binary_error(&[
            PathBuf::from("/opt/gen-model/bin/surreal"),
            PathBuf::from("/repo/bin/surreal"),
        ]);
        assert!(message.contains("/opt/gen-model/bin/surreal"), "{message}");
        assert!(message.contains("/repo/bin/surreal"), "{message}");
        assert!(message.contains("PATH"), "{message}");
        assert!(message.contains("external"), "{message}");
    }

    fn ours() -> StoreIdentity {
        StoreIdentity {
            project: "AvevaMarineSample".into(),
            namespace: "1516".into(),
            data_face: "read-through".into(),
        }
    }

    fn signature_of(owner_pid: u32, heartbeat_ms: u64) -> StoreSignature {
        StoreSignature {
            owner_pid,
            child_pid: owner_pid + 1,
            nonce: "cafe-1".into(),
            project: "AvevaMarineSample".into(),
            namespace: "1516".into(),
            data_face: "read-through".into(),
            heartbeat_ms,
            started_at_ms: heartbeat_ms,
        }
    }

    const NOW: u64 = 1_800_000_000_000;

    /// 端口空着就是空着，署名探测的结果一概不参与——那一格连查都不该查。
    #[test]
    fn a_closed_port_is_simply_idle() {
        assert_eq!(
            judge_port(
                false,
                SignatureProbe::Unreadable("连不上".into()),
                &ours(),
                NOW,
                None
            ),
            StorePortVerdict::Idle
        );
    }

    /// 心跳还新**且属主还在** = 本部署的服务正在用它 → **附着**。Q5 把默认翻成
    /// spawned-mem 之后，这是探针进程（`direct_model_persist` / `db_window_repair` /
    /// python `full_init`）跑起来时的**正常局面**：它们要读的正是服务手上那个库，
    /// 另起一个空库等于什么都查不到。
    #[test]
    fn a_live_sibling_store_is_attached_not_respawned() {
        let signature = signature_of(4242, NOW - 5_000);
        assert_eq!(
            judge_port(
                true,
                SignatureProbe::Found(signature.clone()),
                &ours(),
                NOW,
                Some(true)
            ),
            StorePortVerdict::Attach(signature.clone())
        );
        // 查不出属主在不在时仍然附着：新鲜心跳是够强的证据，而反面是杀进程。
        assert_eq!(
            judge_port(
                true,
                SignatureProbe::Found(signature.clone()),
                &ours(),
                NOW,
                None
            ),
            StorePortVerdict::Attach(signature)
        );
    }

    /// **心跳新不等于属主还活着**（2026-09-04 P6 实测撞出来的那一格）。
    ///
    /// 强杀服务之后的 60 秒里，心跳看着还新、写它的人却已经没了。此前这一格判「附着」，
    /// 而附着的进程不认领这台库、也不接着敲心跳——于是它变成一台**没有主人**的库，
    /// 再过 60 秒就会被下一个启动的进程当孤儿杀掉，**而那时它正被人用着**。
    /// 心跳新 + 属主明确不在 = 没有歧义（写心跳的那个已经死了），照孤儿收。
    #[test]
    fn a_fresh_heartbeat_from_a_dead_owner_is_still_an_orphan() {
        let signature = signature_of(4242, NOW - 5_000);
        let verdict = judge_port(
            true,
            SignatureProbe::Found(signature),
            &ours(),
            NOW,
            Some(false),
        );
        match verdict {
            StorePortVerdict::ReclaimOrphan { reason, .. } => {
                assert!(reason.contains("4242"), "{reason}");
                assert!(reason.contains("强杀"), "{reason}");
            }
            other => panic!("心跳新但属主已死，应当判成孤儿，实际 {other:?}"),
        }
    }

    /// 心跳停了、属主进程也确实不在 = 自家孤儿 → 收掉重起。理由里要带上停了多久与 pid，
    /// 那两样是人事后复查唯一能抓的东西。
    #[test]
    fn an_orphan_of_ours_is_reclaimed_and_the_reason_names_it() {
        let signature = signature_of(4242, NOW - 300_000);
        let verdict = judge_port(
            true,
            SignatureProbe::Found(signature.clone()),
            &ours(),
            NOW,
            Some(false),
        );
        match verdict {
            StorePortVerdict::ReclaimOrphan {
                signature: found,
                reason,
            } => {
                assert_eq!(found, signature);
                assert!(reason.contains("4242"), "{reason}");
                assert!(reason.contains("300"), "{reason}");
            }
            other => panic!("应当判成孤儿，实际 {other:?}"),
        }
    }

    /// **心跳停了但进程还在 = 不猜。** 杀错的代价是把一个正在跑的服务的库端掉；
    /// 多等一次的代价只是人来看一眼。查不出进程在不在时同理。
    #[test]
    fn an_ambiguous_owner_is_never_guessed_into_a_kill() {
        let signature = signature_of(4242, NOW - 300_000);
        for owner_alive in [Some(true), None] {
            let verdict = judge_port(
                true,
                SignatureProbe::Found(signature.clone()),
                &ours(),
                NOW,
                owner_alive,
            );
            match verdict {
                StorePortVerdict::Refuse(reason) => {
                    assert!(reason.contains("不猜"), "{reason}");
                    assert!(reason.contains("4242"), "{reason}");
                }
                other => panic!("{owner_alive:?} 应当拒绝，实际 {other:?}"),
            }
        }
    }

    /// 端口上有服务、但一枚署名都读不出来 = 别人的库 → 拒。**从外面分不出 memory 与
    /// rocksdb**，连上去就可能把内存形态的语义悄悄换成落盘形态。
    #[test]
    fn an_unsigned_store_is_refused_with_both_ways_out() {
        let verdict = judge_port(true, SignatureProbe::Absent, &ours(), NOW, None);
        match verdict {
            StorePortVerdict::Refuse(reason) => {
                assert!(reason.contains("没有署名"), "{reason}");
                assert!(reason.contains("external"), "{reason}");
            }
            other => panic!("应当拒绝，实际 {other:?}"),
        }
    }

    /// 连不上 / 登录不了 / 查不动，也一律拒——「读不出来」与「读出来没有」说的不是一件事，
    /// 但处置相同：都不许动它。
    #[test]
    fn an_unreadable_port_is_refused_and_quotes_the_error() {
        let verdict = judge_port(
            true,
            SignatureProbe::Unreadable("There was a problem with authentication".into()),
            &ours(),
            NOW,
            None,
        );
        match verdict {
            StorePortVerdict::Refuse(reason) => {
                assert!(reason.contains("authentication"), "{reason}");
                assert!(reason.contains("external"), "{reason}");
            }
            other => panic!("应当拒绝，实际 {other:?}"),
        }
    }

    /// 同一个端口上跑着另一份工程的自带存储：署名读得到、心跳也新，但**不是我们的库**。
    /// 附着上去等于两套工程互相盖。
    #[test]
    fn another_deployments_store_is_refused_even_when_it_is_healthy() {
        let mut signature = signature_of(4242, NOW - 1_000);
        signature.project = "ZDJ".into();
        let verdict = judge_port(
            true,
            SignatureProbe::Found(signature),
            &ours(),
            NOW,
            Some(true),
        );
        match verdict {
            StorePortVerdict::Refuse(reason) => {
                assert!(reason.contains("ZDJ"), "{reason}");
                assert!(reason.contains("AvevaMarineSample"), "{reason}");
            }
            other => panic!("应当拒绝，实际 {other:?}"),
        }
    }

    /// 心跳判据的边界：刚好卡在阈值上算活着。阈值本身宁大勿小——判成孤儿要杀进程。
    #[test]
    fn the_heartbeat_threshold_leans_towards_alive() {
        let stale = HEARTBEAT_STALE_AFTER.as_millis() as u64;
        assert!(matches!(
            judge_port(
                true,
                SignatureProbe::Found(signature_of(1, NOW - stale)),
                &ours(),
                NOW,
                Some(true)
            ),
            StorePortVerdict::Attach(_)
        ));
        assert!(matches!(
            judge_port(
                true,
                SignatureProbe::Found(signature_of(1, NOW - stale - 1)),
                &ours(),
                NOW,
                Some(false)
            ),
            StorePortVerdict::ReclaimOrphan { .. }
        ));
        // 心跳间隔要远小于判死阈值，否则一次 GC 就能让活着的服务被判成孤儿。
        assert!(HEARTBEAT_INTERVAL * 4 <= HEARTBEAT_STALE_AFTER);
    }

    /// 时钟往回跳时不许把「未来的心跳」算成停了很久（`saturating_sub`）。
    #[test]
    fn a_heartbeat_from_the_future_still_counts_as_alive() {
        assert!(matches!(
            judge_port(
                true,
                SignatureProbe::Found(signature_of(1, NOW + 10_000)),
                &ours(),
                NOW,
                Some(true)
            ),
            StorePortVerdict::Attach(_)
        ));
    }

    /// 工程名里的单引号必须被转义：它是配置里来的，直接拼进语句就是一条注入。
    #[test]
    fn the_signature_statement_escapes_its_strings_and_deletes_before_inserting() {
        let mut signature = signature_of(7, NOW);
        signature.project = "it's".into();
        let sql = signature_upsert_sql(&signature);
        assert!(sql.starts_with("DELETE "), "{sql}");
        assert!(sql.contains("INSERT INTO"), "{sql}");
        assert!(sql.contains("'it\\'s'"), "{sql}");
        assert!(!sql.contains("UPDATE"), "{sql}");
        assert!(sql.contains(&format!("heartbeat_ms: {NOW}")), "{sql}");
    }

    /// 署名住在**固定**的 ns/db：配错工程的那一个 gen-model 也要读得到它，
    /// 因为它正是要被拦下的局面之一。
    #[test]
    fn the_signature_lives_in_a_project_independent_place() {
        assert_eq!(SIGNATURE_NS, "gen_model");
        assert_eq!(SIGNATURE_DB, "store");
        assert!(signature_select_sql().contains(SIGNATURE_TABLE));
        assert!(signature_select_sql().contains(SIGNATURE_ID));
    }

    /// 署名要能原样过一遍 JSON——库里存的就是这些字段，字段名改了而读侧没跟上，
    /// 表现是「读不出署名 → 拒绝启动」，看上去像端口被别人占了。
    #[test]
    fn the_signature_round_trips_through_serde() {
        let signature = signature_of(11, NOW);
        let text = serde_json::to_string(&signature).expect("序列化");
        let back: StoreSignature = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(back, signature);
        assert!(text.contains("owner_pid"), "{text}");
        assert!(text.contains("child_pid"), "{text}");
        assert!(text.contains("heartbeat_ms"), "{text}");
    }

    /// 本进程一定在。它是 `process_exists` 唯一能不靠外部状态验证的输入；
    /// 回 `None`（探测命令自己失败）在开发机上就该发现，别等到判孤儿的时候。
    #[test]
    fn process_exists_finds_this_very_process() {
        assert_eq!(process_exists(std::process::id()), Some(true));
    }

    /// 真起真杀。要 `bin/surreal.exe`（**未纳入 git**，CI 上没有），所以默认不跑：
    /// `cargo test -- --ignored spawns_and_reaps` 手动验。
    #[test]
    #[ignore = "需要本地 bin/surreal.exe，且会占用一个端口"]
    fn spawns_and_reaps_a_real_memory_store() {
        let port = 18009;
        let mut store = SpawnedStore::start(
            "127.0.0.1",
            port,
            "root",
            "root",
            &SpawnDatastore::Memory,
            DEFAULT_READY_TIMEOUT,
        )
        .expect("拉起 memory 后端失败");
        let addr = sock(&format!("127.0.0.1:{port}"));
        assert!(TcpStream::connect_timeout(&addr, PROBE_CONNECT_TIMEOUT).is_ok());
        store.shutdown();
        // 收尸之后端口必须真的放开，否则下一次启动会撞上自己留下的孤儿。
        std::thread::sleep(Duration::from_millis(500));
        assert!(TcpStream::connect_timeout(&addr, PROBE_CONNECT_TIMEOUT).is_err());
        // 幂等：Drop 还会再调一次。
        store.shutdown();
    }

    /// 署名走一遍真库。**这条不能只靠纯函数**：`signature_upsert_sql` 的语句形状与
    /// `StoreSignature` 的字段名要真的被 SurrealDB 认下来，一旦对不上，表现是
    /// 「读不出署名 → 拒绝启动」，看上去像端口被别人占了——最难查的那一类。
    #[tokio::test]
    #[ignore = "需要本地 bin/surreal.exe，且会占用一个端口"]
    async fn a_real_store_answers_with_the_signature_we_stamped() {
        let port = 18010;
        let bind = resolve_bind("127.0.0.1", port).expect("地址应当能解");
        let mut store = SpawnedStore::start_on(
            bind.clone(),
            "root",
            "root",
            &SpawnDatastore::Memory,
            DEFAULT_READY_TIMEOUT,
        )
        .expect("拉起 memory 后端失败");

        // 盖之前：一台没有署名的库，判据必须落「拒绝」而不是「附着」。
        assert_eq!(
            probe_signature(bind.probe, "root", "root").await,
            SignatureProbe::Absent
        );

        let signature = StoreSignature {
            owner_pid: std::process::id(),
            child_pid: store.pid(),
            nonce: fresh_nonce(),
            project: "AvevaMarineSample".into(),
            namespace: "1516".into(),
            data_face: "read-through".into(),
            heartbeat_ms: now_unix_ms(),
            started_at_ms: now_unix_ms(),
        };
        let handle = connect_signature_db(bind.probe, "root", "root")
            .await
            .expect("连署名库失败");
        stamp_signature(&handle, &signature)
            .await
            .expect("写署名失败");

        let probe = probe_signature(bind.probe, "root", "root").await;
        assert_eq!(probe, SignatureProbe::Found(signature.clone()));

        // 盖完之后：同一份身份 + 新鲜心跳 = 附着。
        assert_eq!(
            judge_port(true, probe, &ours(), now_unix_ms(), Some(true)),
            StorePortVerdict::Attach(signature.clone())
        );

        // 重复盖一次（心跳就是这么敲的）不该留下两行。
        stamp_signature(&handle, &signature)
            .await
            .expect("第二次写署名失败");
        assert!(matches!(
            probe_signature(bind.probe, "root", "root").await,
            SignatureProbe::Found(_)
        ));

        drop(handle);
        store.shutdown();
    }

    /// 整条路径走一遍：第一次**起**，第二次**附着**。
    ///
    /// 这是 `ensure_spawned_store` 唯一的端到端证据。两次调用之间什么都不变，
    /// 第二次却必须走到完全不同的分支——那正是 Q5 翻默认之后探针进程的日常：
    /// 服务在跑、库在它手上，探针要读的是那个库，不是另起一个空库。
    #[tokio::test]
    #[ignore = "需要本地 bin/surreal.exe，且会占用一个端口"]
    async fn ensure_spawns_once_and_attaches_the_second_time() {
        let port = 18011;
        let identity = ours();

        // 探针那一档在端口空着时必须**响亮地说服务没起**，而不是自己拉一台空库。
        let idle = ensure_spawned_store(
            "127.0.0.1",
            port,
            "root",
            "root",
            &identity,
            StoreStartup::AttachOnly,
            DEFAULT_READY_TIMEOUT,
            &SpawnDatastore::Memory,
        )
        .await
        .expect_err("端口空着时 AttachOnly 必须失败");
        assert!(idle.to_string().contains("没有自带存储在跑"), "{idle:#}");

        let first = ensure_spawned_store(
            "127.0.0.1",
            port,
            "root",
            "root",
            &identity,
            StoreStartup::Spawn,
            DEFAULT_READY_TIMEOUT,
            &SpawnDatastore::Memory,
        )
        .await
        .expect("第一次应当把库起起来");
        let StoreHandle::Spawned(mut store) = first else {
            panic!("端口原本空着，第一次必须是自己起的");
        };

        // 两档都该附着：库还活着的时候，服务主进程与探针看到的是同一件事。
        for startup in [StoreStartup::Spawn, StoreStartup::AttachOnly] {
            let again = ensure_spawned_store(
                "127.0.0.1",
                port,
                "root",
                "root",
                &identity,
                startup,
                DEFAULT_READY_TIMEOUT,
                &SpawnDatastore::Memory,
            )
            .await
            .unwrap_or_else(|error| panic!("{startup:?} 应当附着上去：{error:#}"));
            assert!(
                again.is_attached(),
                "{startup:?}：自家还活着的库必须附着，不能另起一个"
            );
        }

        // 换一份工程身份再来一次：同一个端口上跑着别人的库，必须拒绝而不是附着。
        let stranger = StoreIdentity {
            project: "ZDJ".into(),
            namespace: "1600".into(),
            data_face: "read-through".into(),
        };
        let refused = ensure_spawned_store(
            "127.0.0.1",
            port,
            "root",
            "root",
            &stranger,
            StoreStartup::Spawn,
            DEFAULT_READY_TIMEOUT,
            &SpawnDatastore::Memory,
        )
        .await
        .expect_err("另一份工程必须被拒");
        assert!(refused.to_string().contains("另一份部署"), "{refused:#}");

        store.shutdown();
    }
}
