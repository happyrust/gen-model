//! 内容摘要：SHA-256 十六进制小写。
//!
//! 原住 `model_publish_queue::digest`——那个目录随初始化发布 run 退役（ADR-071 S5 / S7），
//! 摘要这件公用小物 2026-09-10 搬到这里，**口径一字不改**：模型目标哈希、模型输入缓存的键
//! 都是它算的（房间输入版本 / 目录版本令牌 / 拓扑分片键随房间流程退役，ADR-073 D2 / D6），
//! 换算法或换编码会让存量库里这些键全部失配。

pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    /// 口径钉：SHA-256 / 小写十六进制 / 不加盐。存量键靠它不漂。
    #[test]
    fn digest_is_plain_lowercase_hex_sha256() {
        assert_eq!(
            super::digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            super::digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
