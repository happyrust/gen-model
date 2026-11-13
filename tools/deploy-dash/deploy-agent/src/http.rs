//! 手写的 HTTP/1.1 小服务端（设计第 7 节甲路）：请求行和头用 `httparse`，按 `Content-Length` 读体，
//! 每个连接只处理一个请求，应答写完就 `Connection: close`（面板的 `http.rs` 本来就是 close）。
//! 不支持请求体分块（`Transfer-Encoding`）——客户端都是自己人，体都很小。

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// 请求头最多这么大。
pub const MAX_HEAD: usize = 16 * 1024;
/// 请求体最多这么大（除下面两条传内容的路由）。
pub const MAX_BODY: usize = 1024 * 1024;
/// 升级传内容那条路由的请求体上限（设计 4.5）。
pub const MAX_BLOB_BODY: usize = 8 * 1024 * 1024;
/// 网页面板老的推程序接口 `PUT /v1/services/upload` 的上限：网页按 2 MiB 一块传，留一倍余量。
pub const MAX_SERVICE_UPLOAD_BODY: usize = 4 * 1024 * 1024;
const MAX_HEADERS: usize = 48;

/// 按路径决定请求体上限。
///
/// 设计 4.5 只给 `PUT /v1/upgrade/sessions/<id>/blobs/<sha256>` 这一条路由放宽到 8 MiB；
/// `PUT /v1/services/upload` 在下线之前单给 4 MiB；别的路由仍是 1 MiB（不然谁都能拿 8 MiB 打过来）。
/// 判定必须在**读完头之后、读体之前**做，所以这里是独立函数而不是常量。
pub fn body_limit_for(path: &str) -> usize {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        ["v1", "upgrade", "sessions", _, "blobs", _] => MAX_BLOB_BODY,
        ["v1", "services", "upload"] => MAX_SERVICE_UPLOAD_BODY,
        _ => MAX_BODY,
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// 不带查询串的路径（原样，不解码）。
    pub path: String,
    /// 查询串，已经按 `%XX` 和 `+` 解码。
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub peer: SocketAddr,
    pub local: SocketAddr,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn query(&self, key: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, body: String) -> Self {
        Self { status, headers: vec![("Content-Type".into(), "application/json; charset=utf-8".into())], body: body.into_bytes() }
    }

    pub fn html(status: u16, body: &'static str) -> Self {
        Self { status, headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())], body: body.as_bytes().to_vec() }
    }

    pub fn empty(status: u16) -> Self {
        Self { status, headers: Vec::new(), body: Vec::new() }
    }

    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

/// 读请求出错时该回什么。
#[derive(Debug)]
pub enum ReadError {
    /// 对方没发完就断了、超时了：什么都不回，直接关。
    Gone,
    /// 回这个状态码。
    Status(u16, &'static str),
}

pub async fn read_request(stream: &mut TcpStream, peer: SocketAddr, local: SocketAddr, timeout: Duration) -> Result<Request, ReadError> {
    match tokio::time::timeout(timeout, read_inner(stream, peer, local)).await {
        Ok(r) => r,
        Err(_) => Err(ReadError::Gone),
    }
}

async fn read_inner(stream: &mut TcpStream, peer: SocketAddr, local: SocketAddr) -> Result<Request, ReadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let (head_len, mut req) = loop {
        let n = stream.read(&mut chunk).await.map_err(|_| ReadError::Gone)?;
        if n == 0 {
            return Err(ReadError::Gone);
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut parsed = httparse::Request::new(&mut headers);
        match parsed.parse(&buf) {
            Ok(httparse::Status::Complete(len)) => {
                if len > MAX_HEAD {
                    return Err(ReadError::Status(431, "请求头太大"));
                }
                let method = parsed.method.unwrap_or("").to_string();
                let target = parsed.path.unwrap_or("/").to_string();
                let headers: Vec<(String, String)> =
                    parsed.headers.iter().map(|h| (h.name.to_string(), String::from_utf8_lossy(h.value).trim().to_string())).collect();
                let (path, query) = split_target(&target);
                break (len, Request { method, path, query, headers, body: Vec::new(), peer, local });
            }
            Ok(httparse::Status::Partial) => {
                if buf.len() > MAX_HEAD {
                    return Err(ReadError::Status(431, "请求头太大"));
                }
            }
            Err(httparse::Error::TooManyHeaders) => return Err(ReadError::Status(431, "请求头太多")),
            Err(_) => return Err(ReadError::Status(400, "认不出的 HTTP 请求")),
        }
    };
    if req.header("transfer-encoding").is_some() {
        return Err(ReadError::Status(411, "请求体不支持分块，请带 Content-Length"));
    }
    let len: usize = match req.header("content-length") {
        None => 0,
        Some(v) => v.parse().map_err(|_| ReadError::Status(400, "Content-Length 不是数字"))?,
    };
    let limit = body_limit_for(&req.path);
    if len > limit {
        return Err(ReadError::Status(413, "请求体太大"));
    }
    let mut body = buf.split_off(head_len);
    while body.len() < len {
        let n = stream.read(&mut chunk).await.map_err(|_| ReadError::Gone)?;
        if n == 0 {
            return Err(ReadError::Gone);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    req.body = body;
    Ok(req)
}

pub async fn write_response(stream: &mut TcpStream, resp: &Response) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, reason(resp.status));
    for (k, v) in &resp.headers {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    if resp.status != 304 {
        head.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    }
    head.push_str("Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    if resp.status != 304 && !resp.body.is_empty() {
        stream.write_all(&resp.body).await?;
    }
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

/// 出错应答写完后，把对方还在发的东西读掉（最多 1 秒、1 MB）再关：
/// 直接关一个还有没读的数据的连接，Windows 会发 RST，对方可能连应答都收不到。
pub async fn linger(stream: &mut TcpStream) {
    let mut sink = [0u8; 8192];
    let mut total = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while total < 1024 * 1024 {
        match tokio::time::timeout_at(deadline, stream.read(&mut sink)).await {
            Ok(Ok(n)) if n > 0 => total += n,
            _ => break,
        }
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

/// `/v1/status?since=3&summary=1` → (`/v1/status`, [("since","3"),("summary","1")])。
pub fn split_target(target: &str) -> (String, Vec<(String, String)>) {
    let (path, q) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let query = q
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(kv), String::new()),
        })
        .collect();
    (path.to_string(), query)
}

/// 解 `%XX` 和 `+`（查询串里 `+` 是空格）。坏的 `%` 原样留着；解出来不是 UTF-8 的字节换成 U+FFFD。
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 把文字编成查询串里的值（客户端、单测用）。
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_decodes_query() {
        let (p, q) = split_target("/v1/fs/probe?path=D%3A%5CAVEVA%5C%E5%B7%A5%E7%A8%8B&x=a+b&flag");
        assert_eq!(p, "/v1/fs/probe");
        assert_eq!(q[0], ("path".to_string(), "D:\\AVEVA\\工程".to_string()));
        assert_eq!(q[1], ("x".to_string(), "a b".to_string()));
        assert_eq!(q[2], ("flag".to_string(), String::new()));
    }

    #[test]
    fn bad_percent_is_kept() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz1"), "%zz1");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("%41"), "A");
    }

    #[test]
    fn encode_round_trips() {
        let s = "D:\\AVEVA\\Projects\\工程 1";
        assert_eq!(percent_decode(&percent_encode(s)), s);
    }

    #[test]
    fn only_the_two_upload_routes_get_a_larger_body_limit() {
        let sha = "a".repeat(64);
        let blob = format!("/v1/upgrade/sessions/u20261113-031200-a1b2/blobs/{sha}");
        assert_eq!(body_limit_for(&blob), MAX_BLOB_BODY);
        assert_eq!(body_limit_for("/v1/upgrade/inventory"), MAX_BODY);
        assert_eq!(body_limit_for("/v1/upgrade/sessions"), MAX_BODY);
        assert_eq!(body_limit_for("/v1/upgrade/sessions/x"), MAX_BODY);
        assert_eq!(body_limit_for("/v1/upgrade/sessions/x/manifest"), MAX_BODY);
        assert_eq!(body_limit_for("/v1/jobs"), MAX_BODY, "别的路由一律 1 MiB");
        assert_eq!(body_limit_for(&format!("/v1/upgrade/sessions/x/blobs/{sha}/extra")), MAX_BODY, "多一段就不是那条路由");
        assert_eq!(body_limit_for("/v1/upgrade/sessions//blobs/x"), MAX_BODY, "空段不算");
        assert_eq!(MAX_BLOB_BODY, 8 * 1024 * 1024);

        assert_eq!(body_limit_for("/v1/services/upload"), MAX_SERVICE_UPLOAD_BODY);
        assert_eq!(body_limit_for("/v1/services/upload/"), MAX_SERVICE_UPLOAD_BODY, "末尾斜杠一样");
        assert_eq!(body_limit_for("/v1/services/upgrade"), MAX_BODY);
        assert_eq!(body_limit_for("/v1/services/upload/x"), MAX_BODY);
        assert_eq!(MAX_SERVICE_UPLOAD_BODY, 4 * 1024 * 1024, "网页一块 2 MiB，留一倍余量");
    }
}
