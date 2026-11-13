//! 读目录里最新一个 `.log` 的文件尾（`stack` 日志来源：`<根>\logs\stack\`）。做法照 dash2 `stack.rs::tail_logs`：
//! 第一次从末尾 64 KiB 读起；换了文件从头读；文件变短也从头读；一次最多读 512 KiB。

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct FileTail {
    dir: PathBuf,
    path: Option<PathBuf>,
    offset: u64,
    partial: Vec<u8>,
}

impl FileTail {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, path: None, offset: 0, partial: Vec::new() }
    }

    /// 正在读的文件。
    pub fn file(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 读新增的整行（去掉颜色控制符和回车，空行不要）。
    pub fn poll(&mut self) -> Vec<String> {
        let Some(path) = newest_log(&self.dir) else { return Vec::new() };
        let Ok(meta) = std::fs::metadata(&path) else { return Vec::new() };
        let len = meta.len();
        let same = self.path.as_ref() == Some(&path) && self.offset <= len;
        if !same {
            let first = self.path.is_none();
            self.offset = if first { len.saturating_sub(64 * 1024) } else { 0 };
            self.partial.clear();
            self.path = Some(path.clone());
        }
        if len <= self.offset {
            return Vec::new();
        }
        let Ok(mut f) = std::fs::File::open(&path) else { return Vec::new() };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let want = (len - self.offset).min(512 * 1024) as usize;
        let mut buf = vec![0u8; want];
        let Ok(n) = f.read(&mut buf) else { return Vec::new() };
        buf.truncate(n);
        self.offset += n as u64;
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(&buf);
        let cut = data.iter().rposition(|b| *b == b'\n').map(|p| p + 1).unwrap_or(0);
        self.partial = data.split_off(cut);
        data.split(|b| *b == b'\n').map(|l| clean_line(&String::from_utf8_lossy(l))).filter(|l| !l.trim().is_empty()).collect()
    }
}

fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("log")))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH))
        .map(|e| e.path())
}

/// 去掉颜色控制符、回车和开头的 BOM。
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn reads_only_new_complete_lines() {
        let dir = std::env::temp_dir().join(format!("deploy-agent-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stack-20261112.log");
        std::fs::write(&p, "\u{feff}第一行\r\n\u{1b}[32m第二行\u{1b}[0m\r\n半").unwrap();
        let mut t = FileTail::new(dir.clone());
        assert_eq!(t.poll(), vec!["第一行".to_string(), "第二行".to_string()]);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all("行\r\n".as_bytes()).unwrap();
        drop(f);
        assert_eq!(t.poll(), vec!["半行".to_string()]);
        assert!(t.poll().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
