//! `human_size!`：clone / compress 日志里的字节数人话（自 old-pdms-io `sync/utils.rs` 搬入）。

macro_rules! human_size {
    ($size:expr) => {{
        let size = ($size) as u64;
        if size > 1024 * 1024 {
            format!("{:.1} MiB ({} bytes)", size as f64 / 1048576.0, size)
        } else if size > 1024 {
            format!("{:.1} KiB ({} bytes)", size as f64 / 1024.0, size)
        } else {
            format!("{} bytes", size)
        }
    }};
}

#[cfg(test)]
mod tests {
    #[test]
    fn human_size_small() {
        assert_eq!(human_size!(100).as_str(), "100 bytes");
    }
    #[test]
    fn human_size_kib() {
        assert_eq!(human_size!(10_000).as_str(), "9.8 KiB (10000 bytes)");
    }
    #[test]
    fn human_size_mib() {
        assert_eq!(
            human_size!(100_000_000).as_str(),
            "95.4 MiB (100000000 bytes)"
        );
    }
}
