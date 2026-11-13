//! 客户端存储（REMOTE-ACCESS-DESIGN 5.5、第 6 节）：`%LOCALAPPDATA%\AiosDeployDash\` 下的
//! `servers.toml`（服务器列表）和 `panel.toml`（面板设置：列表折叠、上次选中的服务器）。
//!
//! - 只认一个很小的 TOML 子集：`key = value`、`[表]`、`[[表数组]]`，值是字符串、整数、布尔，`#` 注释。
//!   一行一行地读，看不懂的行跳过并记一条提示，文件坏了面板照样起得来；不认识的键和表悄悄忽略（以后只加字段不改版本）。
//! - 写：先写同目录的临时文件再改名；原文件读不懂时先备份成 `*.broken`；`schema` 比本面板新、或者读不出来时不改写。
//! - 令牌用 DPAPI（当前用户范围）加密，存成 `dpapi:<base64>`；换电脑或换用户解不开，那一台标成「要重新输入」。
//!   内存里是 [`Secret`]：没有 `Display`，`Debug` 只打 `••••`，丢掉时清零。提示文字里从不出现令牌。
#![allow(dead_code)]

use std::ffi::c_void;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::Duration;

/// 两个文件的格式版本：只加键不改版本，改了含义才加一。
pub const SCHEMA: i64 = 1;
/// 代理缺省端口（设计第 6 节）。
pub const DEFAULT_PORT: u16 = 8600;
pub const DIR_NAME: &str = "AiosDeployDash";
pub const SERVERS_FILE: &str = "servers.toml";
pub const PANEL_FILE: &str = "panel.toml";
const TOKEN_PREFIX: &str = "dpapi:";

const SERVERS_HEADER: &str = "# AIOS 部署面板的服务器列表。面板自己维护这个文件，一般不用手改。\n\
# 令牌用 Windows DPAPI 按当前用户加密（dpapi:…）：换电脑或换用户解不开，到时在面板里重新输入。\n";
const PANEL_HEADER: &str = "# AIOS 部署面板的界面设置（服务器列表折叠、上次选中的服务器）。\n";

// ───────────────────────── 令牌 ─────────────────────────

/// 令牌明文，只活在内存里：没有 `Display`，`Debug` 只打 `••••`，丢掉时把字节清零。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// 只给两处用：发请求时拼 `Authorization` 头、DPAPI 加密。
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(••••)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(unsafe { self.0.as_mut_vec() });
    }
}

fn wipe(bytes: &mut [u8]) {
    for b in bytes.iter_mut() {
        // volatile：别让「反正马上就释放」把清零优化掉
        unsafe { std::ptr::write_volatile(b, 0) };
    }
}

/// 一台服务器的令牌在文件里的样子。
#[derive(Clone, Default, PartialEq, Eq)]
pub enum StoredToken {
    #[default]
    None,
    Present(Secret),
    /// 解不开（换了电脑或用户）：原样留着那串 `dpapi:…`，保存时照写，直到重新输入。
    Unreadable(String),
}

impl fmt::Debug for StoredToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StoredToken::None => "None",
            StoredToken::Present(_) => "Present(••••)",
            StoredToken::Unreadable(_) => "Unreadable(••••)",
        })
    }
}

impl StoredToken {
    pub fn secret(&self) -> Option<&Secret> {
        match self {
            StoredToken::Present(s) => Some(s),
            _ => None,
        }
    }

    /// 文件里有令牌（不管解不解得开）：编辑时令牌框提示「已保存，重新输入可替换」。
    pub fn is_saved(&self) -> bool {
        !matches!(self, StoredToken::None)
    }
}

// ───────────────────────── 数据 ─────────────────────────

/// `servers.toml` 里的一台服务器。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEntry {
    pub name: String,
    /// 主机名或 IP；以后允许 `https://` 开头（第一版是 http）。
    pub address: String,
    pub port: u16,
    pub token: StoredToken,
    pub note: String,
    /// 排序（从 1 起）。读的时候按它排；写的时候按列表顺序重新编号。
    pub order: u32,
    /// 升级密钥（设计 7.3）：和令牌一样按当前用户加密存。没有就是这台没配升级。
    pub upgrade_key: StoredToken,
    /// 摆包变了就自动升这台（设计 7.6）。
    pub auto_upgrade: bool,
}

impl Default for ServerEntry {
    fn default() -> Self {
        ServerEntry {
            name: String::new(),
            address: String::new(),
            port: 0,
            token: StoredToken::None,
            note: String::new(),
            order: 0,
            upgrade_key: StoredToken::None,
            auto_upgrade: false,
        }
    }
}

impl ServerEntry {
    /// 连接和显示用的「主机:端口」；从不含令牌。
    pub fn endpoint(&self) -> String {
        endpoint(&self.address, self.port)
    }
}

/// 「主机:端口」：IPv6 加方括号，`https://` 前缀照留。
pub fn endpoint(address: &str, port: u16) -> String {
    let (scheme, host) = match address.strip_prefix("https://") {
        Some(h) => ("https://", h),
        None => ("", address),
    };
    if host.contains(':') && !host.starts_with('[') { format!("{scheme}[{host}]:{port}") } else { format!("{scheme}{host}:{port}") }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerList {
    pub servers: Vec<ServerEntry>,
}

/// `panel.toml`：面板设置。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelSettings {
    /// 用户按 `Ctrl+B` 收起（true）或展开（false）了服务器列表；`None` = 按窗口宽度自动。
    pub list_collapsed: Option<bool>,
    /// 上次选中的服务器（名字；名字在列表里唯一）。
    pub selected: Option<String>,
}

/// 读的结果：读不懂的地方变成提示，不会失败。
#[derive(Clone, Debug, Default)]
pub struct Loaded<T> {
    pub value: T,
    /// 给人看的提示（哪一行看不懂、哪台的令牌解不开）；从不含令牌。
    pub notes: Vec<String>,
    /// 文件在，但有读不懂的地方。保存前会先把原文件备份成 `*.broken`。
    pub damaged: bool,
    /// 不能改写：文件是更新版本的面板写的（`schema` 比本面板新），或者文件在但读不出来。
    pub read_only: bool,
    /// 保存一次就能修好的小毛病（明文令牌要加密、名字重了改过、没写 schema）。
    pub rewrite: bool,
}

// ───────────────────────── 读写 ─────────────────────────

/// 两个文件所在的目录。单测和自检用自己的临时目录（[`ClientStore::at`]），不碰用户目录。
#[derive(Clone, Debug)]
pub struct ClientStore {
    dir: PathBuf,
}

impl ClientStore {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `%LOCALAPPDATA%\AiosDeployDash`（Windows 已把它限成只有本用户可读）。取不到时是 None。
    pub fn user_default() -> Option<Self> {
        std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(|v| Self::at(PathBuf::from(v).join(DIR_NAME)))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn servers_path(&self) -> PathBuf {
        self.dir.join(SERVERS_FILE)
    }

    pub fn panel_path(&self) -> PathBuf {
        self.dir.join(PANEL_FILE)
    }

    pub fn load_servers(&self) -> Loaded<ServerList> {
        match read_text(&self.servers_path()) {
            Ok(None) => Loaded::default(),
            Ok(Some((text, lossy))) => {
                let mut out = parse_servers(&text);
                if lossy {
                    out.damaged = true;
                    out.notes.insert(0, format!("{SERVERS_FILE} 里有不是 UTF-8 的字节，按能认出的部分读"));
                }
                out
            }
            Err(e) => Loaded {
                notes: vec![format!("{SERVERS_FILE} 读不出来（{e}）：服务器列表先按空的打开，也不会改写它")],
                read_only: true,
                ..Default::default()
            },
        }
    }

    pub fn save_servers(&self, list: &ServerList) -> Result<(), String> {
        let path = self.servers_path();
        guard_existing(&path, SERVERS_FILE, |text| {
            let l = parse_servers(text);
            (l.damaged, l.read_only)
        })?;
        write_atomic(&path, &render_servers(list)?)
    }

    pub fn load_panel(&self) -> Loaded<PanelSettings> {
        match read_text(&self.panel_path()) {
            Ok(None) => Loaded::default(),
            Ok(Some((text, lossy))) => {
                let mut out = parse_panel(&text);
                out.damaged |= lossy;
                out
            }
            Err(e) => Loaded { notes: vec![format!("{PANEL_FILE} 读不出来（{e}）：面板设置按缺省")], read_only: true, ..Default::default() },
        }
    }

    pub fn save_panel(&self, settings: &PanelSettings) -> Result<(), String> {
        let path = self.panel_path();
        guard_existing(&path, PANEL_FILE, |text| {
            let l = parse_panel(text);
            (l.damaged, l.read_only)
        })?;
        write_atomic(&path, &render_panel(settings))
    }
}

/// 读文件：不在是 `Ok(None)`；其余读错是 `Err`。返回（内容，是否有不是 UTF-8 的字节）。
fn read_text(path: &Path) -> Result<Option<(String, bool)>, String> {
    match std::fs::read(path) {
        Ok(mut bytes) => {
            if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                bytes.drain(..3);
            }
            Ok(Some(match String::from_utf8(bytes) {
                Ok(text) => (text, false),
                Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
            }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// 改写前看一眼原文件：更新版本的面板写的、或读不出来 → 不改写；读不懂 → 先备份成 `*.broken`。
fn guard_existing(path: &Path, file: &str, inspect: impl Fn(&str) -> (bool, bool)) -> Result<(), String> {
    let (text, lossy) = match read_text(path) {
        Ok(None) => return Ok(()),
        Ok(Some(found)) => found,
        Err(e) => return Err(format!("{file} 读不出来（{e}），不改写它")),
    };
    let (damaged, read_only) = inspect(&text);
    if read_only {
        return Err(format!("{file} 是新版面板写的，本面板不改写它"));
    }
    if damaged || lossy {
        let backup = path.with_file_name(format!("{file}.broken"));
        std::fs::copy(path, &backup).map_err(|e| format!("{file} 有读不懂的地方，想先备份成 {file}.broken 但没备份成（{e}），这次不保存"))?;
    }
    Ok(())
}

/// 先写同目录的临时文件再改名：写到一半断电或崩溃，原文件还是完整的。
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("路径没有上一级目录")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了目录 {}：{e}", dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写不了 {name}：{e}"));
    }
    // 杀毒软件、索引服务偶尔会短暂占着目标文件：改名失败稍等再试
    let mut last = String::new();
    for attempt in 0..8u64 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(40 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(format!("{name} 换不上新内容：{last}"))
}

// ───────────────────────── servers.toml ─────────────────────────

fn render_servers(list: &ServerList) -> Result<String, String> {
    let mut out = String::from(SERVERS_HEADER);
    out += &format!("schema = {SCHEMA}\n");
    for (k, s) in list.servers.iter().enumerate() {
        out += "\n[[server]]\n";
        out += &format!("name = {}\n", quote(&s.name));
        out += &format!("address = {}\n", quote(&s.address));
        out += &format!("port = {}\n", s.port);
        match &s.token {
            StoredToken::None => {}
            StoredToken::Present(secret) => out += &format!("token = {}\n", quote(&protect_token(secret)?)),
            StoredToken::Unreadable(raw) => out += &format!("token = {}\n", quote(raw)),
        }
        // 升级密钥：和令牌同一套加密，但用另一个 entropy（两把不能互换）
        match &s.upgrade_key {
            StoredToken::None => {}
            StoredToken::Present(secret) => out += &format!("upgrade_key = {}\n", quote(&protect_secret(secret, ENTROPY_UPGRADE_KEY)?)),
            StoredToken::Unreadable(raw) => out += &format!("upgrade_key = {}\n", quote(raw)),
        }
        if s.auto_upgrade {
            out += "auto_upgrade = true\n";
        }
        out += &format!("note = {}\n", quote(&s.note));
        out += &format!("order = {}\n", k + 1);
    }
    Ok(out)
}

fn parse_servers(text: &str) -> Loaded<ServerList> {
    let doc = scan(text);
    let mut out: Loaded<ServerList> = Loaded::default();
    for p in &doc.problems {
        out.notes.push(format!("{SERVERS_FILE} {p}"));
    }
    out.damaged = !doc.problems.is_empty();
    check_schema(&doc, SERVERS_FILE, &mut out);
    let mut picked: Vec<(u32, usize, ServerEntry)> = Vec::new();
    for (idx, sec) in doc.sections.iter().filter(|s| s.head == Head::Array("server".into())).enumerate() {
        if let Some(entry) = server_from(sec, &mut out) {
            picked.push((entry.order, idx, entry));
        }
    }
    picked.sort_by_key(|(order, idx, _)| (*order, *idx));
    let mut seen: Vec<String> = Vec::new();
    for (k, (_, _, mut entry)) in picked.into_iter().enumerate() {
        let base = entry.name.clone();
        let mut n = 2;
        while seen.contains(&entry.name.to_lowercase()) {
            entry.name = format!("{base} ({n})");
            n += 1;
        }
        if entry.name != base {
            out.notes.push(format!("{SERVERS_FILE} 里有两台都叫「{base}」，后面那台改叫「{}」", entry.name));
            out.rewrite = true;
        }
        seen.push(entry.name.to_lowercase());
        if entry.order != k as u32 + 1 {
            entry.order = k as u32 + 1;
        }
        out.value.servers.push(entry);
    }
    out
}

fn server_from(sec: &Section, out: &mut Loaded<ServerList>) -> Option<ServerEntry> {
    let at = sec.line;
    let text_of = |key: &str, out: &mut Loaded<ServerList>| -> Option<String> {
        match sec.get(key) {
            Some((Val::Str(s), _)) => Some(s.trim().to_string()),
            Some((_, line)) => {
                out.notes.push(format!("{SERVERS_FILE} 第 {line} 行：{key} 要用引号写成文字"));
                out.damaged = true;
                None
            }
            None => None,
        }
    };
    let Some(name) = text_of("name", out).filter(|s| !s.is_empty()) else {
        out.notes.push(format!("{SERVERS_FILE} 第 {at} 行开始的那台服务器没有名字（name），跳过了"));
        out.damaged = true;
        return None;
    };
    let Some(raw_address) = text_of("address", out).filter(|s| !s.is_empty()) else {
        out.notes.push(format!("{SERVERS_FILE} 第 {at} 行开始的「{name}」没有地址（address），跳过了"));
        out.damaged = true;
        return None;
    };
    // 设计稿第一版的写法是 address = "主机:端口"：拆出来，有 port 键时以它为准
    let (address, port_in_address) = split_legacy_port(&raw_address);
    let port = match sec.get("port") {
        Some((Val::Int(p), _)) if (1..=65535).contains(p) => *p as u16,
        Some((_, line)) => {
            out.notes.push(format!("{SERVERS_FILE} 第 {line} 行：「{name}」的 port 要是 1–65535 的整数，先按 {}", port_in_address.unwrap_or(DEFAULT_PORT)));
            out.damaged = true;
            port_in_address.unwrap_or(DEFAULT_PORT)
        }
        None => port_in_address.unwrap_or(DEFAULT_PORT),
    };
    let token = match text_of("token", out) {
        None => StoredToken::None,
        Some(t) if t.is_empty() => StoredToken::None,
        Some(t) if t.starts_with(TOKEN_PREFIX) => match unprotect_token(&t) {
            Ok(secret) => StoredToken::Present(secret),
            Err(_) => {
                out.notes.push(format!("「{name}」的令牌解不开（换了电脑或用户？）：编辑这台服务器，重新输入令牌"));
                StoredToken::Unreadable(t)
            }
        },
        Some(t) => {
            out.notes.push(format!("「{name}」的令牌是明文写的，下次保存时会加密"));
            out.rewrite = true;
            StoredToken::Present(Secret::new(t))
        }
    };
    let note = text_of("note", out).unwrap_or_default();
    // 升级密钥（设计 7.3）：老存档没有这一项，照样读得了
    let upgrade_key = match text_of("upgrade_key", out) {
        None => StoredToken::None,
        Some(t) if t.is_empty() => StoredToken::None,
        Some(t) if t.starts_with(TOKEN_PREFIX) => match unprotect_secret(&t, ENTROPY_UPGRADE_KEY) {
            Ok(secret) => StoredToken::Present(secret),
            Err(_) => {
                out.notes.push(format!("「{name}」的升级密钥解不开（换了电脑或用户？）：在服务器抽屉里重新填"));
                StoredToken::Unreadable(t)
            }
        },
        Some(t) => {
            out.notes.push(format!("「{name}」的升级密钥是明文写的，下次保存时会加密"));
            out.rewrite = true;
            StoredToken::Present(Secret::new(t))
        }
    };
    let auto_upgrade = match sec.get("auto_upgrade") {
        Some((Val::Bool(b), _)) => *b,
        Some((_, line)) => {
            out.notes.push(format!("{SERVERS_FILE} 第 {line} 行：auto_upgrade 要写 true 或 false，先按关处理"));
            out.damaged = true;
            false
        }
        None => false,
    };
    let order = match sec.get("order") {
        Some((Val::Int(o), _)) if (0..=u32::MAX as i64).contains(o) => *o as u32,
        Some((_, line)) => {
            out.notes.push(format!("{SERVERS_FILE} 第 {line} 行：order 要是不小于 0 的整数，按文件里的先后排"));
            out.damaged = true;
            u32::MAX
        }
        None => u32::MAX,
    };
    Some(ServerEntry { name, address, port, token, note, order, upgrade_key, auto_upgrade })
}

/// 「主机:端口」拆成主机和端口；IPv6（多个冒号）和带方括号的照规矩来；`http://` 去掉（第一版就是 http）。
fn split_legacy_port(address: &str) -> (String, Option<u16>) {
    let (scheme, rest) = match address.strip_prefix("https://") {
        Some(r) => ("https://", r),
        None => ("", address.strip_prefix("http://").unwrap_or(address)),
    };
    if let Some(inner) = rest.strip_prefix('[')
        && let Some((host, tail)) = inner.split_once(']')
    {
        let port = tail.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()).filter(|p| *p > 0);
        return (format!("{scheme}{host}"), port);
    }
    if rest.matches(':').count() == 1
        && let Some((host, p)) = rest.split_once(':')
        && let Ok(port) = p.trim().parse::<u16>()
        && port > 0
    {
        return (format!("{scheme}{host}"), Some(port));
    }
    (format!("{scheme}{rest}"), None)
}

// ───────────────────────── panel.toml ─────────────────────────

fn render_panel(s: &PanelSettings) -> String {
    let mut out = String::from(PANEL_HEADER);
    out += &format!("schema = {SCHEMA}\n");
    if let Some(c) = s.list_collapsed {
        out += &format!("list_collapsed = {c}\n");
    }
    if let Some(name) = &s.selected {
        out += &format!("selected = {}\n", quote(name));
    }
    out
}

fn parse_panel(text: &str) -> Loaded<PanelSettings> {
    let doc = scan(text);
    let mut out: Loaded<PanelSettings> = Loaded::default();
    for p in &doc.problems {
        out.notes.push(format!("{PANEL_FILE} {p}"));
    }
    out.damaged = !doc.problems.is_empty();
    check_schema(&doc, PANEL_FILE, &mut out);
    let root = &doc.sections[0];
    match root.get("list_collapsed") {
        Some((Val::Bool(b), _)) => out.value.list_collapsed = Some(*b),
        Some((_, line)) => {
            out.notes.push(format!("{PANEL_FILE} 第 {line} 行：list_collapsed 要是 true 或 false，按窗口宽度自动"));
            out.damaged = true;
        }
        None => {}
    }
    match root.get("selected") {
        Some((Val::Str(s), _)) if !s.trim().is_empty() => out.value.selected = Some(s.trim().to_string()),
        Some((Val::Str(_), _)) | None => {}
        Some((_, line)) => {
            out.notes.push(format!("{PANEL_FILE} 第 {line} 行：selected 要用引号写成服务器名字"));
            out.damaged = true;
        }
    }
    out
}

fn check_schema<T>(doc: &Doc, file: &str, out: &mut Loaded<T>) {
    let root = &doc.sections[0];
    match root.get("schema") {
        Some((Val::Int(v), _)) if *v == SCHEMA => {}
        Some((Val::Int(v), _)) if *v > SCHEMA => {
            out.read_only = true;
            out.notes.push(format!("{file} 是新版面板写的（schema {v}），本面板只认 {SCHEMA}：照着读，但不改写它"));
        }
        Some((_, line)) => {
            out.damaged = true;
            out.notes.push(format!("{file} 第 {line} 行：schema 应该是 {SCHEMA}，按 {SCHEMA} 读"));
        }
        None if doc.is_blank() => {}
        None => {
            out.rewrite = true;
            out.notes.push(format!("{file} 没写 schema，按 {SCHEMA} 读"));
        }
    }
}

// ───────────────────────── TOML 子集 ─────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Head {
    Root,
    Table(String),
    Array(String),
    /// 表头那一行坏了：后面的键都归它，一起丢掉，免得混进上一台服务器。
    Broken,
}

#[derive(Clone, Debug)]
struct Section {
    head: Head,
    line: usize,
    items: Vec<(String, Val, usize)>,
}

impl Section {
    fn get(&self, key: &str) -> Option<(&Val, usize)> {
        self.items.iter().find(|(k, _, _)| k == key).map(|(_, v, line)| (v, *line))
    }
}

struct Doc {
    sections: Vec<Section>,
    problems: Vec<String>,
}

impl Doc {
    fn is_blank(&self) -> bool {
        self.sections.len() == 1 && self.sections[0].items.is_empty()
    }
}

enum Line {
    Head(Head),
    Pair(String, Val),
}

fn scan(text: &str) -> Doc {
    let mut sections = vec![Section { head: Head::Root, line: 0, items: Vec::new() }];
    let mut problems = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match scan_line(line) {
            Ok(Line::Head(head)) => sections.push(Section { head, line: n, items: Vec::new() }),
            Ok(Line::Pair(key, val)) => {
                let sec = sections.last_mut().expect("至少有根");
                if sec.items.iter().any(|(k, _, _)| *k == key) {
                    problems.push(format!("第 {n} 行：{key} 重复了，用后面这个"));
                    sec.items.retain(|(k, _, _)| *k != key);
                }
                sec.items.push((key, val, n));
            }
            Err(why) => {
                problems.push(format!("第 {n} 行看不懂（{why}），这一行跳过了"));
                if line.starts_with('[') {
                    sections.push(Section { head: Head::Broken, line: n, items: Vec::new() });
                }
            }
        }
    }
    Doc { sections, problems }
}

fn scan_line(line: &str) -> Result<Line, String> {
    if let Some(rest) = line.strip_prefix("[[") {
        let (inner, tail) = rest.split_once("]]").ok_or("表头少了 ]]")?;
        expect_end(tail)?;
        return Ok(Line::Head(Head::Array(table_name(inner)?)));
    }
    if let Some(rest) = line.strip_prefix('[') {
        let (inner, tail) = rest.split_once(']').ok_or("表头少了 ]")?;
        expect_end(tail)?;
        return Ok(Line::Head(Head::Table(table_name(inner)?)));
    }
    let (key, rest) = scan_key(line)?;
    let rest = rest.trim_start().strip_prefix('=').ok_or("少了 =")?;
    let (val, tail) = scan_value(rest.trim_start())?;
    expect_end(tail)?;
    Ok(Line::Pair(key, val))
}

fn is_bare(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn table_name(inner: &str) -> Result<String, String> {
    let name = inner.trim();
    if name.is_empty() || !name.chars().all(is_bare) {
        return Err("表名只认字母、数字、_ 和 -".into());
    }
    Ok(name.to_string())
}

fn scan_key(line: &str) -> Result<(String, &str), String> {
    let (key, rest) = if let Some(rest) = line.strip_prefix('"') {
        scan_basic(rest)?
    } else if let Some(rest) = line.strip_prefix('\'') {
        let (k, tail) = rest.split_once('\'').ok_or("键名少了收尾的 '")?;
        (k.to_string(), tail)
    } else {
        let end = line.find(|c: char| !is_bare(c)).unwrap_or(line.len());
        (line[..end].to_string(), &line[end..])
    };
    if key.is_empty() {
        return Err("没有键名".into());
    }
    if rest.trim_start().starts_with('.') {
        return Err("不认带点的键名".into());
    }
    Ok((key, rest))
}

fn scan_value(s: &str) -> Result<(Val, &str), String> {
    if s.starts_with("\"\"\"") || s.starts_with("'''") {
        return Err("不认多行字符串".into());
    }
    if let Some(rest) = s.strip_prefix('"') {
        let (text, tail) = scan_basic(rest)?;
        return Ok((Val::Str(text), tail));
    }
    if let Some(rest) = s.strip_prefix('\'') {
        let (text, tail) = rest.split_once('\'').ok_or("字符串少了收尾的 '")?;
        return Ok((Val::Str(text.to_string()), tail));
    }
    let end = s.find(|c: char| c.is_whitespace() || c == '#').unwrap_or(s.len());
    let word = &s[..end];
    let tail = &s[end..];
    match word {
        "true" => return Ok((Val::Bool(true), tail)),
        "false" => return Ok((Val::Bool(false), tail)),
        _ => {}
    }
    let digits = word.strip_prefix(['+', '-']).unwrap_or(word);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == '_') && !digits.starts_with('_') && !digits.ends_with('_') {
        let clean: String = word.chars().filter(|c| *c != '_').collect();
        let v = clean.parse::<i64>().map_err(|_| "整数太大".to_string())?;
        return Ok((Val::Int(v), tail));
    }
    Err("值只认带引号的文字、整数、true / false".into())
}

/// 双引号字符串（开头的引号已经去掉）：返回内容和收尾引号之后的部分。
fn scan_basic(s: &str) -> Result<(String, &str), String> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &s[i + 1..])),
            '\\' => {
                let (_, e) = chars.next().ok_or("转义没写完")?;
                match e {
                    'b' => out.push('\u{8}'),
                    't' => out.push('\t'),
                    'n' => out.push('\n'),
                    'f' => out.push('\u{c}'),
                    'r' => out.push('\r'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'u' | 'U' => {
                        let len = if e == 'u' { 4 } else { 8 };
                        let hex: String = (0..len).filter_map(|_| chars.next().map(|(_, h)| h)).collect();
                        let code = u32::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == len).ok_or("\\u 后面不是十六进制")?;
                        out.push(char::from_u32(code).ok_or("\\u 不是合法的字符")?);
                    }
                    other => return Err(format!("认不出的转义 \\{other}")),
                }
            }
            c if (c as u32) < 0x20 && c != '\t' => return Err("字符串里有控制字符".into()),
            c => out.push(c),
        }
    }
    Err("字符串少了收尾的引号".into())
}

fn expect_end(tail: &str) -> Result<(), String> {
    let t = tail.trim_start();
    if t.is_empty() || t.starts_with('#') { Ok(()) } else { Err("值后面还有别的东西".into()) }
}

/// 写成 TOML 双引号字符串。
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ───────────────────────── DPAPI ─────────────────────────

#[repr(C)]
struct Blob {
    len: u32,
    data: *mut u8,
}

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
/// 附加熵：同一个用户下别的程序调 DPAPI 也解不开这些令牌，除非知道这串字。
const ENTROPY: &[u8] = b"AiosDeployDash servers.toml token v1";
/// 升级密钥单独一份 entropy：和令牌的密文不能互换（设计 7.3）。
pub const ENTROPY_UPGRADE_KEY: &[u8] = b"AiosDeployDash servers.toml upgrade-key v1";

// windows-sys 没开 Win32_Security_Cryptography 特性（开了会让 tokio 等一串基础包连带重编），
// 照 winsys.rs 处理 DWM 的办法手写这两条。
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        data_in: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut Blob,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

/// DPAPI 加密（当前用户范围、不弹任何界面）。
pub fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    dpapi_protect_with(plain, ENTROPY)
}

/// DPAPI 加密，用指定的 entropy（令牌和升级密钥各用各的，密文不能互换）。
pub fn dpapi_protect_with(plain: &[u8], entropy_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let input = Blob { len: plain.len() as u32, data: plain.as_ptr() as *mut u8 };
    let entropy = Blob { len: entropy_bytes.len() as u32, data: entropy_bytes.as_ptr() as *mut u8 };
    let mut out = Blob { len: 0, data: null_mut() };
    let ok = unsafe { CryptProtectData(&input, null(), &entropy, null_mut(), null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
    if ok == 0 || out.data.is_null() {
        return Err(format!("DPAPI 加密失败：{}", std::io::Error::last_os_error()));
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.data, out.len as usize) }.to_vec();
    unsafe { LocalFree(out.data as *mut c_void) };
    Ok(bytes)
}

/// DPAPI 解密。换了电脑、换了用户、密文被改过都会失败。
pub fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
    dpapi_unprotect_with(cipher, ENTROPY)
}

/// DPAPI 解密，用指定的 entropy。
pub fn dpapi_unprotect_with(cipher: &[u8], entropy_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let input = Blob { len: cipher.len() as u32, data: cipher.as_ptr() as *mut u8 };
    let entropy = Blob { len: entropy_bytes.len() as u32, data: entropy_bytes.as_ptr() as *mut u8 };
    let mut out = Blob { len: 0, data: null_mut() };
    let ok = unsafe { CryptUnprotectData(&input, null_mut(), &entropy, null_mut(), null_mut(), CRYPTPROTECT_UI_FORBIDDEN, &mut out) };
    if ok == 0 || out.data.is_null() {
        return Err(format!("DPAPI 解不开：{}", std::io::Error::last_os_error()));
    }
    let plain = unsafe { std::slice::from_raw_parts_mut(out.data, out.len as usize) };
    let copy = plain.to_vec();
    wipe(plain);
    unsafe { LocalFree(out.data as *mut c_void) };
    Ok(copy)
}

/// 令牌 → `dpapi:<base64>`。
pub fn protect_token(secret: &Secret) -> Result<String, String> {
    protect_secret(secret, ENTROPY)
}

/// 任意机密（令牌、升级密钥）→ `dpapi:<base64>`，用指定的 entropy。
pub fn protect_secret(secret: &Secret, entropy_bytes: &[u8]) -> Result<String, String> {
    let cipher = dpapi_protect_with(secret.expose().as_bytes(), entropy_bytes)?;
    Ok(format!("{TOKEN_PREFIX}{}", base64_encode(&cipher)))
}

/// `dpapi:<base64>` → 令牌。错误文字里没有令牌。
pub fn unprotect_token(stored: &str) -> Result<Secret, String> {
    unprotect_secret(stored, ENTROPY)
}

/// `dpapi:<base64>` → 机密，用指定的 entropy。错误文字里没有内容。
pub fn unprotect_secret(stored: &str, entropy_bytes: &[u8]) -> Result<Secret, String> {
    let b64 = stored.strip_prefix(TOKEN_PREFIX).ok_or("不是 dpapi: 开头")?;
    let cipher = base64_decode(b64.trim()).ok_or("base64 坏了")?;
    let plain = dpapi_unprotect_with(&cipher, entropy_bytes)?;
    match String::from_utf8(plain) {
        Ok(text) => Ok(Secret(text)),
        Err(e) => {
            let mut bytes = e.into_bytes();
            wipe(&mut bytes);
            Err("解出来的不是文字".into())
        }
    }
}

// ───────────────────────── base64 ─────────────────────────

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (chunk[0] as u32) << 16 | (b1 as u32) << 8 | b2 as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// 标准字母表、带补齐的 base64；有一点不对就是 None。
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let groups = bytes.len() / 4;
    let mut out = Vec::with_capacity(groups * 3);
    for (gi, chunk) in bytes.chunks(4).enumerate() {
        let last = gi + 1 == groups;
        let mut n = 0u32;
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if last && i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return None,
            };
            if pad > 0 && c != b'=' {
                return None;
            }
            n = n << 6 | v as u32;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ───────────────────────── 单测 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 单测自己的临时目录，测完删掉（不碰 %LOCALAPPDATA%\AiosDeployDash）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fleet-ui-test-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn store(&self) -> ClientStore {
            ClientStore::at(&self.0)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // 测试用的假令牌：不是任何真令牌
    const FAKE_TOKEN: &str = "fake-token-for-unit-test-0123456789abcdef";

    fn entry(name: &str, address: &str, port: u16, token: StoredToken, note: &str, order: u32) -> ServerEntry {
        ServerEntry { name: name.into(), address: address.into(), port, token, note: note.into(), order, ..Default::default() }
    }

    #[test]
    fn servers_round_trip_and_token_never_in_plain_text() {
        let tmp = TempDir::new("round-trip");
        let store = tmp.store();
        let list = ServerList {
            servers: vec![
                entry("服务器甲", "server-a.example", 8600, StoredToken::Present(Secret::new(FAKE_TOKEN)), "二楼 \"机房\" \\ 第一排\n备用", 1),
                entry("服务器乙", "fe80::1", 8601, StoredToken::None, "", 2),
                entry("Server C", "https://server-c.example", 9443, StoredToken::Present(Secret::new("another-fake")), "tab\there", 3),
            ],
        };
        store.save_servers(&list).unwrap();
        let raw = std::fs::read_to_string(store.servers_path()).unwrap();
        assert!(raw.contains("schema = 1"));
        assert!(raw.contains("token = \"dpapi:"));
        assert!(!raw.contains(FAKE_TOKEN) && !raw.contains("another-fake"));
        let loaded = store.load_servers();
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
        assert!(!loaded.damaged && !loaded.read_only && !loaded.rewrite);
        assert_eq!(loaded.value, list);
        assert_eq!(loaded.value.servers[0].token.secret().unwrap().expose(), FAKE_TOKEN);
        assert_eq!(loaded.value.servers[1].endpoint(), "[fe80::1]:8601");
        assert_eq!(loaded.value.servers[2].endpoint(), "https://server-c.example:9443");
        // Debug 里看不到令牌
        let dbg = format!("{:?}", loaded.value);
        assert!(!dbg.contains(FAKE_TOKEN) && dbg.contains("Present(••••)"));
        // 没有留下临时文件
        let leftovers = std::fs::read_dir(&tmp.0).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn damaged_file_gives_notes_and_is_backed_up_before_save() {
        let tmp = TempDir::new("damaged");
        let store = tmp.store();
        let text = "schema = 1\n\
                    this line is not toml\n\
                    [[server]]\n\
                    name = \"甲\"\n\
                    address = \"a.example\"\n\
                    port = \"abc\"\n\
                    [[server\n\
                    name = \"乙\"\n\
                    address = \"b.example\"\n\
                    [[server]]\n\
                    address = \"no-name.example\"\n\
                    [[server]]\n\
                    name = \"丙\"\n\
                    address = \"c.example\"\n\
                    port = 70000\n\
                    token = \"dpapi:!!!\"\n\
                    note = \"没收尾\n";
        std::fs::write(store.servers_path(), text).unwrap();
        let loaded = store.load_servers();
        assert!(loaded.damaged && !loaded.read_only);
        assert!(loaded.notes.len() >= 5, "{:?}", loaded.notes);
        let names: Vec<&str> = loaded.value.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["甲", "丙"]);
        assert_eq!(loaded.value.servers[0].port, DEFAULT_PORT);
        assert_eq!(loaded.value.servers[1].port, DEFAULT_PORT);
        assert!(matches!(loaded.value.servers[1].token, StoredToken::Unreadable(ref raw) if raw == "dpapi:!!!"));
        assert!(loaded.notes.iter().any(|n| n.contains("第 2 行")));
        // 保存前把读不懂的原文件备份成 .broken；解不开的令牌原样写回
        store.save_servers(&loaded.value).unwrap();
        let backup = std::fs::read_to_string(tmp.0.join("servers.toml.broken")).unwrap();
        assert_eq!(backup, text);
        let again = store.load_servers();
        assert!(!again.damaged, "{:?}", again.notes);
        assert_eq!(again.value.servers.len(), 2);
        assert!(matches!(again.value.servers[1].token, StoredToken::Unreadable(_)));
    }

    #[test]
    fn binary_garbage_does_not_panic() {
        let tmp = TempDir::new("garbage");
        let store = tmp.store();
        let mut bytes: Vec<u8> = (0u8..=255).collect();
        bytes.extend_from_slice(b"\n[[server]]\nname = \"x\"\n\xff\xfe");
        std::fs::write(store.servers_path(), &bytes).unwrap();
        let loaded = store.load_servers();
        assert!(loaded.damaged);
        assert!(!loaded.notes.is_empty());
        std::fs::write(store.panel_path(), &bytes).unwrap();
        let panel = store.load_panel();
        assert!(panel.damaged);
        assert_eq!(panel.value, PanelSettings::default());
    }

    #[test]
    fn newer_schema_is_read_only_and_never_overwritten() {
        let tmp = TempDir::new("newer");
        let store = tmp.store();
        let text = "schema = 2\n[[server]]\nname = \"甲\"\naddress = \"a.example\"\nport = 8600\n[tls]\nfingerprint = \"sha256:x\"\n";
        std::fs::write(store.servers_path(), text).unwrap();
        let loaded = store.load_servers();
        assert!(loaded.read_only);
        assert_eq!(loaded.value.servers.len(), 1);
        assert!(store.save_servers(&ServerList::default()).is_err());
        assert_eq!(std::fs::read_to_string(store.servers_path()).unwrap(), text);
    }

    #[test]
    fn plaintext_token_is_kept_and_flagged_without_echo() {
        let tmp = TempDir::new("plain");
        let store = tmp.store();
        std::fs::write(store.servers_path(), format!("schema = 1\n[[server]]\nname = \"甲\"\naddress = \"a.example\"\ntoken = \"{FAKE_TOKEN}\"\n")).unwrap();
        let loaded = store.load_servers();
        assert!(loaded.rewrite && !loaded.damaged);
        assert_eq!(loaded.value.servers[0].token.secret().unwrap().expose(), FAKE_TOKEN);
        assert!(loaded.notes.iter().all(|n| !n.contains(FAKE_TOKEN)));
        store.save_servers(&loaded.value).unwrap();
        let raw = std::fs::read_to_string(store.servers_path()).unwrap();
        assert!(!raw.contains(FAKE_TOKEN) && raw.contains("dpapi:"));
    }

    #[test]
    fn order_legacy_port_and_duplicate_names() {
        let text = "schema = 1\n\
                    [[server]]\nname = \"后\"\naddress = \"b.example:9001\"\norder = 2\n\
                    [[server]]\nname = \"先\"\naddress = \"[fe80::2]:9002\"\norder = 1\n\
                    [[server]]\nname = \"先\"\naddress = 'c.example' # 字面量字符串\nport = 8_600\n";
        let loaded = parse_servers(text);
        let got: Vec<(String, String, u16, u32)> = loaded.value.servers.iter().map(|s| (s.name.clone(), s.address.clone(), s.port, s.order)).collect();
        assert_eq!(
            got,
            [
                ("先".to_string(), "fe80::2".to_string(), 9002, 1),
                ("后".to_string(), "b.example".to_string(), 9001, 2),
                ("先 (2)".to_string(), "c.example".to_string(), 8600, 3),
            ]
        );
        assert!(loaded.rewrite && !loaded.damaged);
    }

    #[test]
    fn panel_round_trip_and_defaults() {
        let tmp = TempDir::new("panel");
        let store = tmp.store();
        let missing = store.load_panel();
        assert!(missing.notes.is_empty() && !missing.damaged);
        assert_eq!(missing.value, PanelSettings::default());
        let s = PanelSettings { list_collapsed: Some(true), selected: Some("服务器 \"乙\"".into()) };
        store.save_panel(&s).unwrap();
        assert_eq!(store.load_panel().value, s);
        std::fs::write(store.panel_path(), "schema = 1\nlist_collapsed = \"yes\"\nselected = 3\n").unwrap();
        let bad = store.load_panel();
        assert!(bad.damaged && bad.notes.len() == 2);
        assert_eq!(bad.value, PanelSettings::default());
    }

    #[test]
    fn dpapi_round_trip_same_account() {
        let plain = "中文 token \u{1F512} 0123456789".as_bytes();
        let cipher = dpapi_protect(plain).unwrap();
        assert!(cipher.len() > plain.len());
        assert!(!cipher.windows(plain.len()).any(|w| w == plain));
        assert_eq!(dpapi_unprotect(&cipher).unwrap(), plain);
        let mut tampered = cipher.clone();
        let mid = tampered.len() / 2;
        tampered[mid] ^= 0x5a;
        assert!(dpapi_unprotect(&tampered).is_err());
        let stored = protect_token(&Secret::new(FAKE_TOKEN)).unwrap();
        assert!(stored.starts_with("dpapi:") && !stored.contains(FAKE_TOKEN));
        assert_eq!(unprotect_token(&stored).unwrap().expose(), FAKE_TOKEN);
        assert!(unprotect_token("dpapi:AAAA").is_err());
        assert!(unprotect_token("plain").is_err());
    }

    #[test]
    fn base64_vectors() {
        for (plain, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64_encode(plain.as_bytes()), enc);
            assert_eq!(base64_decode(enc).unwrap(), plain.as_bytes());
        }
        let all: Vec<u8> = (0u8..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        for bad in ["Zg=", "Z===", "Zg=a", "Zm9v!A==", "====", "Zg==Zg=="] {
            assert!(base64_decode(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn secret_debug_is_masked() {
        let s = Secret::new(FAKE_TOKEN);
        assert_eq!(format!("{s:?}"), "Secret(••••)");
        assert_eq!(format!("{:?}", StoredToken::Unreadable("dpapi:xyz".into())), "Unreadable(••••)");
    }

    #[test]
    fn toml_subset_scanner() {
        let doc = scan("a = 1 # 注释\n\"b c\" = \"x\\u4E2D\\\"\"\nd = -2_000\ne = true\n[t]\nf.g = 1\nh = 1.5\ni = \"\"\"x\"\"\"\nj = [1]\n");
        let root = &doc.sections[0];
        assert_eq!(root.get("a").unwrap().0, &Val::Int(1));
        assert_eq!(root.get("b c").unwrap().0, &Val::Str("x中\"".into()));
        assert_eq!(root.get("d").unwrap().0, &Val::Int(-2000));
        assert_eq!(root.get("e").unwrap().0, &Val::Bool(true));
        assert_eq!(doc.sections[1].head, Head::Table("t".into()));
        assert_eq!(doc.problems.len(), 4, "{:?}", doc.problems);
        assert_eq!(quote("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }
}
