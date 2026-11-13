//! 本机 HTTP/1.1 小客户端（阻塞，放在后台线程上用）：问后端、数据库、rs-server、前端。
//! `request` / `request_with` 只连 127.0.0.1；`request_to` 连任意主机（远程的 stack 代理）。
//! `Connection: close`，读到对方关连接为止；认 `Transfer-Encoding: chunked`。
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};

use crate::json::Json;

const MAX_BODY: usize = 16 * 1024 * 1024;

pub struct Resp {
    pub status: u16,
    pub body: String,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Option<Json> {
        Json::parse(&self.body).ok()
    }
}

pub fn request(method: &str, port: u16, path: &str, body: Option<&str>, timeout: Duration) -> anyhow::Result<Resp> {
    request_with(method, port, path, body, &[], timeout)
}

/// 带额外请求头（头里可能有账号，调用方不要把它们写进日志）。只连 127.0.0.1。
pub fn request_with(method: &str, port: u16, path: &str, body: Option<&str>, headers: &[(&str, String)], timeout: Duration) -> anyhow::Result<Resp> {
    request_to(method, "127.0.0.1", port, path, body, headers, timeout)
}

/// 连任意主机（远程的 stack 代理）：`host` 是 IP 或计算机名，解析出几个地址就挨个试；`Host` 头照它写。其余同 `request_with`。
pub fn request_to(method: &str, host: &str, port: u16, path: &str, body: Option<&str>, headers: &[(&str, String)], timeout: Duration) -> anyhow::Result<Resp> {
    request_bytes_to(method, host, port, path, body.map(str::as_bytes), headers, timeout)
}

/// 同 [`request_to`]，但请求体是原始字节——升级传内容（最大 8 MiB 一块）走这条。
///
/// `Content-Type` 由调用方给（传内容时是 `application/octet-stream`）；没给就按 JSON。
pub fn request_bytes_to(method: &str, host: &str, port: u16, path: &str, body: Option<&[u8]>, headers: &[(&str, String)], timeout: Duration) -> anyhow::Result<Resp> {
    let deadline = Instant::now() + timeout;
    let host_port = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().with_context(|| format!("认不出主机 {host}"))?.collect();
    let mut last = None;
    let mut stream = None;
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, timeout.min(Duration::from_secs(3))) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let Some(mut s) = stream else {
        let e = last.map(anyhow::Error::from).unwrap_or_else(|| anyhow::anyhow!("{host} 没有可用的地址"));
        return Err(e.context(format!("连不上 {host_port}")));
    };
    s.set_write_timeout(Some(timeout))?;
    let payload = body.unwrap_or(&[]);
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nAccept: application/json\r\nUser-Agent: deploy-dash\r\n");
    for (k, v) in headers {
        head += &format!("{k}: {v}\r\n");
    }
    if body.is_some() {
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
            head += "Content-Type: application/json\r\n";
        }
        head += &format!("Content-Length: {}\r\n", payload.len());
    }
    head += "\r\n";
    s.write_all(head.as_bytes())?;
    s.write_all(payload)?;
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("{} 秒内没有读完应答", timeout.as_secs());
        }
        s.set_read_timeout(Some(left))?;
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_BODY {
                    bail!("应答超过 {} MB", MAX_BODY / 1024 / 1024);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted) => continue,
            Err(e) => return Err(e).context("读应答"),
        }
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").context("应答没有头部结束标记")?;
    let header = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut content = raw[split + 4..].to_vec();
    let status_line = header.lines().next().unwrap_or_default();
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).context("认不出状态行")?;
    let chunked = header.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if chunked {
        content = dechunk(&content);
    }
    Ok(Resp { status, body: String::from_utf8_lossy(&content).into_owned() })
}

fn dechunk(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let Some(line_end) = data[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_text = String::from_utf8_lossy(&data[i..i + line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16).unwrap_or(0);
        i += line_end + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(data.len());
        out.extend_from_slice(&data[i..end]);
        i = end + 2;
    }
    out
}

pub fn get(port: u16, path: &str, timeout: Duration) -> anyhow::Result<Resp> {
    request("GET", port, path, None, timeout)
}

pub fn post(port: u16, path: &str, body: &str, timeout: Duration) -> anyhow::Result<Resp> {
    request("POST", port, path, Some(body), timeout)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn dechunks() {
        let body = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(super::dechunk(body), b"Wikipedia");
    }

    /// 二进制体（升级传内容用）：一个 1 MiB 的随机字节，服务端按 Content-Length 收齐、原样回长度。
    #[test]
    fn binary_body_round_trips() {
        let listener = TcpListener::bind(("127.0.0.2", 0)).expect("绑 127.0.0.2 上的空闲端口");
        let port = listener.local_addr().unwrap().port();
        let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 256) as u8).collect();
        let expect = payload.clone();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut len = 0usize;
            let mut ctype = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-type:") {
                    ctype = v.trim().to_string();
                }
            }
            let mut body = vec![0u8; len];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            assert_eq!(body, expect, "字节一个不差");
            let mut s = s;
            let out = format!(r#"{{"got":{len},"type":"{ctype}"}}"#);
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).unwrap();
        });
        let headers = [("Content-Type", "application/octet-stream".to_string())];
        let r = request_bytes_to("PUT", "127.0.0.2", port, "/v1/upgrade/sessions/s/blobs/abc?offset=0", Some(&payload), &headers, Duration::from_secs(20)).unwrap();
        server.join().unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json().unwrap().get("got").u64(), Some(payload.len() as u64));
        assert_eq!(r.json().unwrap().get("type").str(), Some("application/octet-stream"));
    }

    /// 非 127.0.0.1 的主机走得通（Windows 上 127.0.0.0/8 都是本机回环，不用开网卡）；`Host` 头照传进来的主机写。
    #[test]
    fn request_to_reaches_another_host() {
        let listener = TcpListener::bind(("127.0.0.2", 0)).expect("绑 127.0.0.2 上的空闲端口");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut host = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.strip_prefix("Host:") {
                    host = v.trim().to_string();
                }
            }
            let mut s = s;
            let body = format!("{{\"host\":\"{host}\"}}");
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let r = request_to("GET", "127.0.0.2", port, "/v1/ping", None, &[], Duration::from_secs(5)).unwrap();
        server.join().unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.json().unwrap().get("host").str(), Some(format!("127.0.0.2:{port}").as_str()));
        let Err(e) = request_to("GET", "127.0.0.2", port, "/", None, &[], Duration::from_secs(5)) else { panic!("监听已关，应当连不上") };
        assert!(format!("{e:#}").starts_with(&format!("连不上 127.0.0.2:{port}")), "{e:#}");
    }
}
