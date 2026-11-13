//! 升级密钥与签名（设计 4.3、5.9）：只用来给「交清单」和「下升级任务」签名，从不过网。
//!
//! - 密钥：32 字节随机数，文字形式是 64 位小写十六进制，落 `<根>\run\gui\upgrade.key`（ACL 同令牌文件）。
//! - `key_id`：密钥 32 字节的 sha256 的前 8 位十六进制——只够核对，泄露了也推不了程序。
//! - 签名：`HMAC-SHA256(密钥, "aios-upgrade-v1\n<用途>\n<随机数>\n<请求体 sha256>")` 的小写十六进制。
//! - 一次性随机数：两分钟内有效、用一次就作废，同时最多留 [`MAX_NONCES`] 个。
//!
//! 开没开升级**只看密钥文件在不在**（设计 5.9）：删掉文件就是关掉，不需要重启代理——
//! 每次验签都重新看文件，换密钥（`upgrade-key --rotate`）立刻生效。

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use deploy_dash_proto::{self as proto, sign_text};

type HmacSha256 = Hmac<Sha256>;

/// 同时最多留多少个没用过的随机数。设计写 16；并行传两台、每台 manifest + job + 重试很容易挤掉自己，改 32。
pub const MAX_NONCES: usize = 32;
/// 随机数有效期。
pub const NONCE_TTL: Duration = Duration::from_secs(120);
/// 密钥文件名（相对 `<根>\run\gui\`）。
pub const KEY_FILE: &str = "upgrade.key";

/// 32 字节密钥。`Debug` 打成 `••••`，不打印、不进日志。
#[derive(Clone, PartialEq, Eq)]
pub struct UpgradeKey([u8; 32]);

impl UpgradeKey {
    pub fn generate() -> anyhow::Result<UpgradeKey> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("取不到随机数：{e}"))?;
        Ok(UpgradeKey(bytes))
    }

    /// 从 64 位小写十六进制解析（读文件、单测用）。
    pub fn from_hex(text: &str) -> Result<UpgradeKey, String> {
        let t = text.trim();
        if t.len() != 64 {
            return Err(format!("升级密钥要是 64 位十六进制，这里是 {} 位", t.len()));
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in t.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).map_err(|_| "升级密钥里有非 ASCII 字符".to_string())?;
            bytes[i] = u8::from_str_radix(s, 16).map_err(|_| format!("升级密钥里有不是十六进制的字符：{s}"))?;
        }
        Ok(UpgradeKey(bytes))
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 密钥指纹：32 字节的 sha256 前 8 位十六进制。审计、界面只用它。
    pub fn key_id(&self) -> String {
        let h = Sha256::digest(self.0);
        h.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }

    /// 算签名（小写十六进制）。
    pub fn sign(&self, use_: &str, nonce: &str, body_sha256: &str) -> String {
        let text = sign_text(use_, nonce, body_sha256);
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC 接受任意长度密钥");
        mac.update(text.as_bytes());
        mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 常量时间比较（先比长度，再逐字节；长度不同直接假）。
    pub fn verify(&self, use_: &str, nonce: &str, body_sha256: &str, presented: &str) -> bool {
        let want = self.sign(use_, nonce, body_sha256);
        let a = want.as_bytes();
        let b = presented.trim().to_ascii_lowercase();
        let b = b.as_bytes();
        a.len() == b.len() && bool::from(a.ct_eq(b))
    }
}

impl fmt::Debug for UpgradeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UpgradeKey(••••)")
    }
}

/// `<根>\run\gui\upgrade.key`
pub fn key_path(root: &Path) -> PathBuf {
    root.join("run").join("gui").join(KEY_FILE)
}

/// 读密钥：文件不在就是 `Ok(None)`（= 没开升级）；内容不对是 `Err`（要人看一眼，不能当没开）。
pub fn read_key(root: &Path) -> anyhow::Result<Option<UpgradeKey>> {
    let path = key_path(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => UpgradeKey::from_hex(&text)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("升级密钥文件 {} 读不懂：{e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("读不了升级密钥文件 {}：{e}", path.display())),
    }
}

/// 生成并写盘（`upgrade-key`）。已经有一把时覆盖（`--rotate` 才允许，见调用方）。
pub fn write_key(root: &Path, key: &UpgradeKey, protect: bool) -> anyhow::Result<String> {
    let path = key_path(root);
    let dir = path.parent().ok_or_else(|| anyhow::anyhow!("升级密钥路径没有上级目录"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_file_name(format!("{KEY_FILE}.new"));
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, b"")?;
    let note = if protect {
        match crate::token::protect_file(&tmp) {
            Ok(n) => n,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(anyhow::anyhow!("没能把升级密钥文件设成只有当前账户能读：{e:#}"));
            }
        }
    } else {
        "没有设 ACL（单测）".to_string()
    };
    std::fs::write(&tmp, format!("{}\r\n", key.to_hex()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(note)
}

/// 删掉密钥 = 关掉升级（`upgrade-key --disable`）。返回原来在不在。
pub fn disable(root: &Path) -> std::io::Result<bool> {
    let path = key_path(root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// 一次性随机数的池子。
#[derive(Debug, Default)]
pub struct Nonces {
    live: VecDeque<(String, Instant)>,
}

impl Nonces {
    pub fn new() -> Nonces {
        Nonces::default()
    }

    /// 发一个新的。池子满了丢最老的（设计 4.3）。
    pub fn issue(&mut self) -> String {
        let mut bytes = [0u8; 32];
        // 取不到随机源时退化成时间 + 计数器：这不是密码学随机，只用于单测与极端环境；
        // 真正的防护是签名（要密钥），随机数只是防重放。
        if getrandom::getrandom(&mut bytes).is_err() {
            let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            bytes[..16].copy_from_slice(&n.to_le_bytes());
            bytes[16..].copy_from_slice(&(self.live.len() as u128).to_le_bytes());
        }
        let nonce: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        while self.live.len() >= MAX_NONCES {
            self.live.pop_front();
        }
        self.live.push_back((nonce.clone(), Instant::now()));
        nonce
    }

    /// 认不认这个随机数；认就当场作废（用一次就废）。
    pub fn consume(&mut self, nonce: &str) -> bool {
        self.purge();
        match self.live.iter().position(|(n, _)| n == nonce) {
            Some(i) => {
                self.live.remove(i);
                true
            }
            None => false,
        }
    }

    /// 丢过期的。
    pub fn purge(&mut self) {
        let now = Instant::now();
        self.live.retain(|(_, at)| now.duration_since(*at) < NONCE_TTL);
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

/// 验签的三种结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignError {
    /// 这台没开升级。
    Disabled,
    /// `X-Upgrade-Key-Id` 不是这台的。
    KeyMismatch,
    /// 随机数不认识、用过了、过期了。
    Nonce,
    /// 签名不对，或三个头没带全。
    Signature,
}

impl SignError {
    pub fn code(&self) -> &'static str {
        match self {
            SignError::Disabled => proto::upgrade_error_code::UPGRADE_DISABLED,
            SignError::KeyMismatch => proto::upgrade_error_code::UPGRADE_KEY_MISMATCH,
            SignError::Nonce => proto::upgrade_error_code::UPGRADE_NONCE,
            SignError::Signature => proto::upgrade_error_code::UPGRADE_SIGNATURE,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            SignError::Disabled => "这台没开升级：在服务器上跑 `upgrade-key --root <根>` 生成一把升级密钥",
            SignError::KeyMismatch => "升级密钥对不上（X-Upgrade-Key-Id 不是这台的）",
            SignError::Nonce => "随机数不认识、用过了或者过期了：重新领一个",
            SignError::Signature => "签名不对，或者三个 X-Upgrade-* 头没带全",
        }
    }
}

/// 三个头。缺一个都算签名不对（设计 4.3）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignHeaders {
    pub key_id: String,
    pub nonce: String,
    pub signature: String,
}

impl SignHeaders {
    /// 从头里取；三个都必须有且非空。
    pub fn from_headers(get: impl Fn(&str) -> Option<String>) -> Option<SignHeaders> {
        let h = SignHeaders {
            key_id: get(proto::header::KEY_ID)?.trim().to_string(),
            nonce: get(proto::header::NONCE)?.trim().to_string(),
            signature: get(proto::header::SIGNATURE)?.trim().to_string(),
        };
        (!h.key_id.is_empty() && !h.nonce.is_empty() && !h.signature.is_empty()).then_some(h)
    }
}

/// 验一次请求。顺序照设计 4.3：没开 → key_id 不对 → 随机数不认 → 签名不对。
/// 签名不对时那个随机数也作废（`nonces` 里已经被 `consume` 掉了）。
pub fn verify_request(
    key: Option<&UpgradeKey>,
    nonces: &mut Nonces,
    headers: Option<&SignHeaders>,
    use_: &str,
    body_sha256: &str,
) -> Result<(), SignError> {
    let Some(key) = key else { return Err(SignError::Disabled) };
    let Some(h) = headers else { return Err(SignError::Signature) };
    if h.key_id != key.key_id() {
        return Err(SignError::KeyMismatch);
    }
    if !nonces.consume(&h.nonce) {
        return Err(SignError::Nonce);
    }
    if !key.verify(use_, &h.nonce, body_sha256, &h.signature) {
        return Err(SignError::Signature);
    }
    Ok(())
}

/// 请求体字节的 sha256（小写十六进制）。签的是原始字节，所以两边不用约定 JSON 排版。
pub fn body_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> UpgradeKey {
        UpgradeKey([seed; 32])
    }

    #[test]
    fn hex_round_trip_and_key_id() {
        let k = key(0xab);
        let hex = k.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(UpgradeKey::from_hex(&hex).unwrap(), k);
        assert_eq!(UpgradeKey::from_hex(&format!("{hex}\r\n")).unwrap(), k, "文件里有换行也能读");
        let id = k.key_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(key(1).key_id(), key(2).key_id(), "不同密钥的指纹不同");
    }

    #[test]
    fn bad_hex_is_refused() {
        assert!(UpgradeKey::from_hex("abcd").unwrap_err().contains("64 位"));
        assert!(UpgradeKey::from_hex(&"z".repeat(64)).is_err());
        assert!(UpgradeKey::from_hex(&"g".repeat(64)).unwrap_err().contains("十六进制"));
    }

    #[test]
    fn signature_is_hex_and_verifies() {
        let k = key(7);
        let sig = k.sign("job", "n1", "h1");
        assert_eq!(sig.len(), 64);
        assert!(k.verify("job", "n1", "h1", &sig));
        assert!(!k.verify("job", "n1", "h2", &sig), "体变了签名就不对");
        assert!(!k.verify("job", "n2", "h1", &sig), "随机数变了签名就不对");
        assert!(!k.verify("manifest:s1", "n1", "h1", &sig), "用途变了签名就不对");
        assert!(!key(8).verify("job", "n1", "h1", &sig), "换一把密钥就不认");
    }

    #[test]
    fn signature_comparison_is_case_insensitive_but_exact() {
        let k = key(9);
        let sig = k.sign("job", "n", "h");
        assert!(k.verify("job", "n", "h", &sig.to_uppercase()), "大小写不影响（签名本身只发小写）");
        assert!(!k.verify("job", "n", "h", &sig[..62]), "截短的不认");
        assert!(!k.verify("job", "n", "h", ""), "空的不认");
    }

    #[test]
    fn known_hmac_vector() {
        // 用 32 个 0x0b 当密钥、文本 "Hi There" 的 HMAC-SHA256 是 RFC 4231 的第 1 组；
        // 这里验我们自己的文本拼法：固定输入算出来的值必须稳定（改动拼法会让它变）。
        let k = UpgradeKey([0x0b; 32]);
        let text = sign_text("job", "nonce", "hash");
        let mut mac = HmacSha256::new_from_slice(&k.0).unwrap();
        mac.update(text.as_bytes());
        let want: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(k.sign("job", "nonce", "hash"), want);
        assert_eq!(text, "aios-upgrade-v1\njob\nnonce\nhash");
    }

    #[test]
    fn nonce_is_single_use() {
        let mut n = Nonces::new();
        let a = n.issue();
        assert_eq!(a.len(), 64);
        assert!(n.consume(&a));
        assert!(!n.consume(&a), "用过的随机数不认");
        assert!(!n.consume("deadbeef"));
    }

    #[test]
    fn nonce_pool_caps_at_max() {
        let mut n = Nonces::new();
        let first = n.issue();
        for _ in 0..MAX_NONCES {
            n.issue();
        }
        assert_eq!(n.len(), MAX_NONCES);
        assert!(!n.consume(&first), "池子满了丢最老的");
    }

    #[test]
    fn verify_order_matches_the_design() {
        let k = key(3);
        let mut n = Nonces::new();
        // 没开升级
        assert_eq!(verify_request(None, &mut n, None, "job", "h"), Err(SignError::Disabled));
        // 没带三个头
        assert_eq!(verify_request(Some(&k), &mut n, None, "job", "h"), Err(SignError::Signature));
        // key_id 不对
        let bad = SignHeaders { key_id: "00000000".into(), nonce: n.issue(), signature: "x".into() };
        assert_eq!(verify_request(Some(&k), &mut n, Some(&bad), "job", "h"), Err(SignError::KeyMismatch));
        // 随机数不认
        let unknown = SignHeaders { key_id: k.key_id(), nonce: "nope".into(), signature: "x".into() };
        assert_eq!(verify_request(Some(&k), &mut n, Some(&unknown), "job", "h"), Err(SignError::Nonce));
        // 签名不对：随机数同时作废
        let nonce = n.issue();
        let wrong = SignHeaders { key_id: k.key_id(), nonce: nonce.clone(), signature: "0".repeat(64) };
        assert_eq!(verify_request(Some(&k), &mut n, Some(&wrong), "job", "h"), Err(SignError::Signature));
        assert!(!n.consume(&nonce), "签名不对也要作废那个随机数");
        // 全对
        let nonce = n.issue();
        let good = SignHeaders { key_id: k.key_id(), nonce: nonce.clone(), signature: k.sign("job", &nonce, "h") };
        assert_eq!(verify_request(Some(&k), &mut n, Some(&good), "job", "h"), Ok(()));
    }

    #[test]
    fn body_hash_is_lowercase_hex() {
        let h = body_sha256(b"{}");
        assert_eq!(h.len(), 64);
        assert_eq!(h, "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a", "空对象的 sha256");
    }

    #[test]
    fn key_file_lifecycle() {
        let dir = std::env::temp_dir().join(format!("upgrade-key-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_key(&dir).unwrap().is_none(), "没有文件 = 没开升级");
        let k = key(0x42);
        write_key(&dir, &k, false).unwrap();
        let back = read_key(&dir).unwrap().unwrap();
        assert_eq!(back, k);
        assert_eq!(back.key_id(), k.key_id());
        assert!(disable(&dir).unwrap());
        assert!(read_key(&dir).unwrap().is_none());
        assert!(!disable(&dir).unwrap(), "本来就不在");
        // 内容坏了要说出来，不能当没开
        std::fs::create_dir_all(dir.join("run").join("gui")).unwrap();
        std::fs::write(key_path(&dir), "not-a-key").unwrap();
        assert!(read_key(&dir).unwrap_err().to_string().contains("读不懂"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn headers_must_all_be_present() {
        let get = |name: &str| match name {
            proto::header::KEY_ID => Some("3fa9c2e1".to_string()),
            proto::header::NONCE => Some("  ".to_string()),
            _ => Some("sig".to_string()),
        };
        assert!(SignHeaders::from_headers(get).is_none(), "空的随机数算没带");
        let full = |name: &str| Some(format!("v-{name}"));
        let h = SignHeaders::from_headers(full).unwrap();
        assert_eq!(h.key_id, format!("v-{}", proto::header::KEY_ID));
    }
}
