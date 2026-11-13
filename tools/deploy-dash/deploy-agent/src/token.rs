//! 令牌（设计第 6 节、R1 / R8）：每台服务器一个共享令牌。
//!
//! - 第一次起来时生成：32 字节随机数（操作系统的随机源），base64url 不带 `=`，写 `<根>\run\gui\agent.token`。
//! - 文件 ACL 只给当前账户：先建空文件、`icacls /inheritance:r /grant:r *<SID>:(F)`，再写内容、改名就位——
//!   令牌落盘的那一刻文件已经只有本账户能读。
//! - 代理内存里只留令牌的 SHA-256，比较用常量时间（`subtle`）。令牌文件换了（`rotate-token`）就重读。
//! - 令牌不进日志、不进审计、`Debug` 里打成 `••••`。

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// 令牌明文（只在生成、写文件、`show-token` 时出现）。
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn generate() -> anyhow::Result<Token> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("取不到随机数：{e}"))?;
        Ok(Token(base64url(&bytes)))
    }

    /// 只给单测、工具用：拿一段现成的文字当令牌。
    pub fn from_text(text: &str) -> Token {
        Token(text.trim().to_string())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn hash(&self) -> TokenHash {
        TokenHash::of(&self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(••••)")
    }
}

/// 令牌的 SHA-256。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    pub fn of(text: &str) -> TokenHash {
        TokenHash(Sha256::digest(text.trim().as_bytes()).into())
    }

    /// 常量时间比较：先把对方给的也做一次 SHA-256，再逐字节比，耗时和对不对、对到第几位都无关。
    pub fn matches(&self, presented: &str) -> bool {
        let other = TokenHash::of(presented);
        bool::from(self.0.ct_eq(&other.0))
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenHash(••••)")
    }
}

/// base64url，不带 `=`。
pub fn base64url(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

/// 读令牌文件（去掉首尾空白）。
pub fn read(path: &Path) -> std::io::Result<Token> {
    let text = std::fs::read_to_string(path)?;
    let t = text.trim();
    if t.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "令牌文件是空的"));
    }
    Ok(Token(t.to_string()))
}

/// 有就读，没有就生成一个写进去。返回（令牌，是不是新生成的，ACL 的说明）。
pub fn ensure(path: &Path, protect: bool) -> anyhow::Result<(Token, bool, String)> {
    match read(path) {
        Ok(t) => Ok((t, false, String::new())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let t = Token::generate()?;
            let note = write(path, &t, protect)?;
            Ok((t, true, note))
        }
        Err(e) => Err(anyhow::anyhow!("读不了令牌文件 {}：{e}", path.display())),
    }
}

/// 写令牌文件：建空的临时文件 → 设 ACL → 写内容 → 改名盖过旧的。返回 ACL 的说明（设不上时是原因）。
pub fn write(path: &Path, token: &Token, protect: bool) -> anyhow::Result<String> {
    let dir = path.parent().ok_or_else(|| anyhow::anyhow!("令牌文件路径没有上级目录"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = tmp_path(path);
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, b"")?;
    let note = if protect {
        match protect_file(&tmp) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(anyhow::anyhow!("没能把令牌文件设成只有当前账户能读：{e:#}"));
            }
        }
    } else {
        "没有设 ACL（单测）".to_string()
    };
    std::fs::write(&tmp, format!("{}\r\n", token.expose()))?;
    std::fs::rename(&tmp, path)?;
    Ok(note)
}

/// 换一个新令牌（`rotate-token`）。在跑的代理看到文件变了会重读，旧令牌立刻失效。
pub fn rotate(path: &Path, protect: bool) -> anyhow::Result<Token> {
    let t = Token::generate()?;
    write(path, &t, protect)?;
    Ok(t)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".new");
    path.with_file_name(name)
}

/// 当前账户的 SID（`whoami /user /fo csv /nh` 的第二列），拿不到就是 None。
pub fn current_user_sid() -> Option<String> {
    let out = Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let sid = line.rsplit(',').next()?.trim().trim_matches('"').to_string();
    sid.starts_with("S-1-").then_some(sid)
}

/// `icacls` 的参数：去掉继承来的权限，只给 `account` 完全控制。`account` 是 SID 时前面加 `*`。
pub fn icacls_args(path: &Path, account: &str) -> Vec<String> {
    let who = if account.starts_with("S-1-") { format!("*{account}") } else { account.to_string() };
    vec![path.display().to_string(), "/inheritance:r".into(), "/grant:r".into(), format!("{who}:(F)")]
}

/// 把文件的 ACL 设成只有当前账户（设计写的是 `:R`；这里给 `(F)`，代理要能改写、换令牌）。
pub fn protect_file(path: &Path) -> anyhow::Result<String> {
    let account = current_user_sid()
        .or_else(|| {
            let user = std::env::var("USERNAME").ok()?;
            let domain = std::env::var("USERDOMAIN").ok();
            Some(match domain {
                Some(d) if !d.is_empty() => format!("{d}\\{user}"),
                _ => user,
            })
        })
        .ok_or_else(|| anyhow::anyhow!("认不出当前账户"))?;
    let out = Command::new("icacls").args(icacls_args(path, &account)).output()?;
    if !out.status.success() {
        anyhow::bail!("icacls 退出码 {:?}：{}", out.status.code(), decode_console(&out.stdout).trim());
    }
    Ok(format!("ACL 只给当前账户（{account}）"))
}

/// 控制台程序的输出：先当 UTF-8，不是就按系统代码页的常见情形（GBK）粗略保留可读部分。
pub fn decode_console(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_known_values() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn generated_tokens_are_long_and_different() {
        let a = Token::generate().unwrap();
        let b = Token::generate().unwrap();
        assert_eq!(a.expose().len(), 43);
        assert_ne!(a, b);
        assert_eq!(format!("{a:?}"), "Token(••••)");
    }

    #[test]
    fn hash_matches_only_the_same_token() {
        let h = Token::from_text("fake-token-for-tests").hash();
        assert!(h.matches("fake-token-for-tests"));
        assert!(h.matches("  fake-token-for-tests\r\n"));
        assert!(!h.matches("fake-token-for-tests2"));
        assert!(!h.matches(""));
    }

    #[test]
    fn icacls_args_use_sid_star_syntax() {
        let a = icacls_args(Path::new("D:\\r\\run\\gui\\agent.token"), "S-1-5-21-1-2-3-1001");
        assert_eq!(a, vec!["D:\\r\\run\\gui\\agent.token", "/inheritance:r", "/grant:r", "*S-1-5-21-1-2-3-1001:(F)"]);
        let b = icacls_args(Path::new("x"), "PC\\user");
        assert_eq!(b[3], "PC\\user:(F)");
    }

    #[test]
    fn ensure_creates_once_then_reads_back_and_rotate_changes_it() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("run").join("gui").join("agent.token");
        let (t1, created, _) = ensure(&path, false).unwrap();
        assert!(created);
        let (t2, created, _) = ensure(&path, false).unwrap();
        assert!(!created);
        assert_eq!(t1, t2);
        let t3 = rotate(&path, false).unwrap();
        assert_ne!(t1, t3);
        assert_eq!(read(&path).unwrap(), t3);
        assert!(!tmp_path(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真设一次 ACL（只动临时目录里自己建的文件），再用 PowerShell 的 Get-Acl 核对：不继承、只剩一条。
    #[cfg(windows)]
    #[test]
    fn token_file_acl_is_current_account_only() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("agent.token");
        let t = Token::from_text("fake-token-for-acl-test");
        write(&path, &t, true).unwrap();
        // 脚本里不用双引号：Rust 给参数加的 \" 转义经 powershell.exe 的命令行解析后不可靠
        let script = format!(
            "$a = Get-Acl -LiteralPath '{}'; Write-Output $a.AreAccessRulesProtected; Write-Output $a.Access.Count",
            path.display().to_string().replace('\'', "''")
        );
        // 单测常在 pwsh 7 底下跑：它的 PSModulePath 会让 Windows PowerShell 去加载 7 的模块而失败
        let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).env_remove("PSModulePath").output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(text, "True 1", "ACL 应当不继承、只有一条：{text}；{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(read(&path).unwrap(), t);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
