//! `POST /api/v1/meshes/batch`（spec §4.11.4）：一次请求取一批网格，响应是一个键一帧的二进制流。
//!
//! 为什么要有它：一个节点常有上千种网格，逐个 `GET /api/v1/meshes/{键}.mesh` 在浏览器里就是上千次
//! 请求。单个路由（[`crate::web_service::handlers::mesh_glb`]）不变；这条路读的是同一份文件、过的是
//! 同一道字符集门（[`is_valid_geo_hash`]），只是把一批的字节拼在一个响应里。
//!
//! 线上格式是给 plant3d-web 的冻结契约，改动要先改 spec 再改这里：
//!
//! ```text
//! 流头   "PMB1"  u32 帧数
//! 帧     u8 状态  u8 键长  u16 保留(0)  u32 内容长  键  补零到 8  内容  补零到 8
//! 流尾   "PMBE"  u32 帧数
//! ```
//!
//! 整数小端。流头 8 字节、每帧都是 8 的倍数，所以每段内容相对响应体起点的偏移都是 8 的倍数——
//! 前端可以直接在整块缓冲上建 `Float32Array` / `Uint32Array` 视图。
//!
//! 只给原网格的 `.mesh` 形态：粗网格（`?lod=1`）不是内容寻址的，`.glb` 要现场转换，两者都留在单个路由上。

use std::collections::HashSet;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::rejection::JsonRejection;
use axum::http::{StatusCode, header};
use axum::response::Response;
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use tokio::sync::Semaphore;

use crate::web_service::ApiError;
use crate::web_service::mesh_glb::is_valid_geo_hash;

/// 一次请求最多带多少个键（spec §4.11.4；`/health.capabilities.mesh_batch_max_keys`）。
pub const MAX_MESH_BATCH_KEYS: usize = 1024;

/// 一次响应里给出的内容字节数之和的上限（帧头与补齐不计；`/health.capabilities.mesh_batch_max_bytes`）。
/// 到线之后的键不读、按「没读」回，客户端再发一次。
pub const MAX_MESH_BATCH_BYTES: u64 = 32 * 1024 * 1024;

/// 一次请求同时在读的文件数。响应按请求顺序流出，先读完的靠后的帧最多攒这么多份在内存里。
const READS_PER_REQUEST: usize = 16;

/// 全进程这条路由同时在读的文件数：多个请求并发时，读盘的总并发不随请求数涨。
const READS_IN_PROCESS: usize = 32;

static READ_PERMITS: Semaphore = Semaphore::const_new(READS_IN_PROCESS);

const STREAM_MAGIC: &[u8; 4] = b"PMB1";
const TRAILER_MAGIC: &[u8; 4] = b"PMBE";

/// 帧头的定长部分：状态、键长、保留、内容长。
const FRAME_HEAD: usize = 8;

/// 一帧的状态。数值是线上格式的一部分，只许加、不许改。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameStatus {
    /// 有：内容是 `.mesh` 的原样字节。
    Found = 0,
    /// 没有这个文件（等同单个路由的 404）。
    Missing = 1,
    /// 文件在，这一次没读出来。
    ReadFailed = 2,
    /// 没读：这一批的内容总量到了上限。这一帧起直到末尾都是它。
    Deferred = 3,
}

#[derive(Debug, Deserialize)]
pub struct MeshBatchReq {
    #[serde(default)]
    pub keys: Vec<String>,
    /// 只接受缺省 / 0。这条路不给粗网格；带了 1 就拒绝，不能悄悄当成原网格回。
    pub lod: Option<u8>,
}

/// 报错里回显客户端给的键时最多带多少个字符：键可以长到请求体上限，不能原样塞回去。
const ECHO_CHARS: usize = 64;

fn echo(key: &str) -> String {
    let mut shown: String = key.chars().take(ECHO_CHARS).collect();
    if key.chars().nth(ECHO_CHARS).is_some() {
        shown.push('…');
    }
    shown
}

/// 请求体 → 要取的键。个数、字符集、重复都在读盘之前判，错一条整次 400。
pub(crate) fn checked_keys(request: MeshBatchReq) -> Result<Vec<String>, ApiError> {
    if let Some(lod) = request.lod
        && lod != 0
    {
        return Err(ApiError::bad_request(format!(
            "meshes/batch 只给原网格（lod=0）；粗网格请走 GET /api/v1/meshes/{{geo_hash}}.mesh?lod=1: lod={lod}"
        )));
    }
    if request.keys.is_empty() {
        return Err(ApiError::bad_request("keys 不能为空"));
    }
    if request.keys.len() > MAX_MESH_BATCH_KEYS {
        return Err(ApiError::bad_request(format!(
            "keys 有 {} 条；一次最多 {MAX_MESH_BATCH_KEYS} 条",
            request.keys.len()
        )));
    }
    let mut seen = HashSet::with_capacity(request.keys.len());
    for key in &request.keys {
        if !is_valid_geo_hash(key) {
            return Err(ApiError::bad_request(format!(
                "geo_hash 只允许字母、数字与下划线，长度 1..=128: {}",
                echo(key)
            )));
        }
        if !seen.insert(key.as_str()) {
            return Err(ApiError::bad_request(format!(
                "keys 里重复了 {key}；每个键只列一次"
            )));
        }
    }
    Ok(request.keys)
}

/// 不小于 `len` 的最小的 8 的倍数。
const fn ceil8(len: usize) -> usize {
    (len + 7) & !7
}

fn stream_edge(magic: &[u8; 4], frames: u32) -> Bytes {
    let mut edge = Vec::with_capacity(8);
    edge.extend_from_slice(magic);
    edge.extend_from_slice(&frames.to_le_bytes());
    Bytes::from(edge)
}

/// 编一帧。`key` 已经过了字符集门（ASCII、不超过 128 字节），`payload` 不超过 `u32::MAX` 字节。
pub(crate) fn encode_frame(status: FrameStatus, key: &str, payload: &[u8]) -> Bytes {
    let key_end = FRAME_HEAD + ceil8(key.len());
    let frame_end = key_end + ceil8(payload.len());
    let mut frame = Vec::with_capacity(frame_end);
    frame.push(status as u8);
    frame.push(key.len() as u8);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(key.as_bytes());
    frame.resize(key_end, 0);
    frame.extend_from_slice(payload);
    frame.resize(frame_end, 0);
    Bytes::from(frame)
}

/// 读一个键的结果，还没轮到按总量裁决。
enum MeshRead {
    Found(Vec<u8>),
    Missing,
    Failed,
    /// 总量已经到线，连盘都没碰。
    Skipped,
}

async fn read_mesh(dir: &Path, key: &str) -> MeshRead {
    let path = dir.join(format!("{key}.mesh"));
    // 信号量从不关闭，拿不到许可只会是等。
    let _permit = READ_PERMITS.acquire().await;
    match tokio::fs::read(&path).await {
        Ok(bytes) if u32::try_from(bytes.len()).is_ok() => MeshRead::Found(bytes),
        Ok(bytes) => {
            log::warn!(
                "[mesh-batch] 网格 {} 有 {} 字节，超出一帧能表示的长度",
                path.display(),
                bytes.len()
            );
            MeshRead::Failed
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MeshRead::Missing,
        Err(error) => {
            log::warn!("[mesh-batch] 读取网格 {} 失败: {error}", path.display());
            MeshRead::Failed
        }
    }
}

/// 响应体：流头、与 `keys` 逐条对应且同序的帧、流尾。
///
/// 读盘在 [`READS_PER_REQUEST`] 的窗口里并发，帧按请求顺序流出。总量的裁决只在出帧这一步做
/// （顺序的、单线的）：某个键的内容加上去会超过 `byte_limit`、而此前已经给出过内容，这一帧和之后
/// 的所有帧都回 [`FrameStatus::Deferred`]。所以第 0 帧永远不是「没读」，「没读」也不会夹在中间。
/// `exhausted` 只是给还没开始的读一个提示：到线之后不必再碰盘。
pub(crate) fn batch_stream(
    dir: PathBuf,
    keys: Vec<String>,
    byte_limit: u64,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let frames = keys.len() as u32;
    let dir = Arc::new(dir);
    let exhausted = Arc::new(AtomicBool::new(false));
    let reads = stream::iter(keys)
        .map({
            let exhausted = exhausted.clone();
            move |key| {
                let dir = dir.clone();
                let exhausted = exhausted.clone();
                async move {
                    let read = if exhausted.load(Ordering::Relaxed) {
                        MeshRead::Skipped
                    } else {
                        read_mesh(&dir, &key).await
                    };
                    (key, read)
                }
            }
        })
        .buffered(READS_PER_REQUEST);
    let body = reads.scan(
        (0u64, false),
        move |(delivered, deferring), (key, read)| {
            let frame = match read {
                _ if *deferring => encode_frame(FrameStatus::Deferred, &key, &[]),
                MeshRead::Found(bytes)
                    if *delivered == 0 || *delivered + bytes.len() as u64 <= byte_limit =>
                {
                    *delivered += bytes.len() as u64;
                    encode_frame(FrameStatus::Found, &key, &bytes)
                }
                MeshRead::Found(_) | MeshRead::Skipped => {
                    *deferring = true;
                    exhausted.store(true, Ordering::Relaxed);
                    encode_frame(FrameStatus::Deferred, &key, &[])
                }
                MeshRead::Missing => encode_frame(FrameStatus::Missing, &key, &[]),
                MeshRead::Failed => encode_frame(FrameStatus::ReadFailed, &key, &[]),
            };
            std::future::ready(Some(Ok(frame)))
        },
    );
    stream::once(std::future::ready(Ok(stream_edge(STREAM_MAGIC, frames))))
        .chain(body)
        .chain(stream::once(std::future::ready(Ok(stream_edge(
            TRAILER_MAGIC,
            frames,
        )))))
}

/// 请求体解不出来时的回法：太大照实回 413，其余（不是 JSON、形状不对、类型头不对）都是 400。
/// 不用框架缺省的纯文本拒绝，是为了让这条路的错误都是 spec §3 的形状。
fn rejected(rejection: JsonRejection) -> ApiError {
    let message = rejection.body_text();
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message,
            detail: None,
        }
    } else {
        ApiError::bad_request(message)
    }
}

async fn respond(
    dir: impl FnOnce() -> PathBuf + Send,
    request: Result<Json<MeshBatchReq>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(rejected)?;
    let keys = checked_keys(request)?;
    // 网格目录在所有门之后才解析：请求不对的时候不该碰配置和磁盘。
    let body = Body::from_stream(batch_stream(dir(), keys, MAX_MESH_BATCH_BYTES));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("构造网格批量响应失败: {error}")))
}

/// POST /api/v1/meshes/batch — 见模块注释与 spec §4.11.4。
///
/// 网格目录与单个路由、生成侧走同一个解析口（`options::meshes_dir`，ADR-068 D7）。
pub async fn meshes_batch(
    request: Result<Json<MeshBatchReq>, JsonRejection>,
) -> Result<Response, ApiError> {
    respond(crate::options::meshes_dir, request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::Request;
    use axum::routing::post;
    use tower::ServiceExt;

    /// 照 spec §4.11.4 写的参考解码器：前端要实现的就是这一段。
    /// 回 `(流头帧数, 帧, 流尾是否完好)`；帧里带内容相对响应体起点的偏移，用来钉对齐。
    struct Frame {
        status: u8,
        key: String,
        payload: Vec<u8>,
        payload_offset: usize,
    }

    fn decode(body: &[u8]) -> (u32, Vec<Frame>, bool) {
        assert_eq!(&body[..4], b"PMB1", "流头魔数");
        let declared = u32::from_le_bytes(body[4..8].try_into().unwrap());
        let mut at = 8;
        let mut frames = Vec::new();
        for _ in 0..declared {
            if at + 8 > body.len() {
                return (declared, frames, false);
            }
            assert_eq!(at % 8, 0, "每帧从 8 的倍数开始");
            let status = body[at];
            let key_len = body[at + 1] as usize;
            assert_eq!(&body[at + 2..at + 4], &[0, 0], "保留位恒为 0");
            let payload_len = u32::from_le_bytes(body[at + 4..at + 8].try_into().unwrap()) as usize;
            let key_at = at + 8;
            let payload_at = key_at + ceil8(key_len);
            let next = payload_at + ceil8(payload_len);
            if next > body.len() {
                return (declared, frames, false);
            }
            assert!(
                body[key_at + key_len..payload_at].iter().all(|byte| *byte == 0),
                "键后面补的是 0"
            );
            assert!(
                body[payload_at + payload_len..next]
                    .iter()
                    .all(|byte| *byte == 0),
                "内容后面补的是 0"
            );
            frames.push(Frame {
                status,
                key: String::from_utf8(body[key_at..key_at + key_len].to_vec()).unwrap(),
                payload: body[payload_at..payload_at + payload_len].to_vec(),
                payload_offset: payload_at,
            });
            at = next;
        }
        let complete = body.len() == at + 8
            && &body[at..at + 4] == b"PMBE"
            && u32::from_le_bytes(body[at + 4..at + 8].try_into().unwrap()) == declared;
        (declared, frames, complete)
    }

    async fn collect(stream: impl Stream<Item = Result<Bytes, Infallible>>) -> Vec<u8> {
        let mut body = Vec::new();
        let mut stream = std::pin::pin!(stream);
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.unwrap());
        }
        body
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn request(body: &str) -> MeshBatchReq {
        serde_json::from_str(body).expect("请求体")
    }

    /// 帧的字节布局是给前端的冻结契约：逐字节钉住一帧。
    #[test]
    fn a_frame_is_laid_out_exactly_as_the_spec_says() {
        let frame = encode_frame(FrameStatus::Found, "abc", &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(
            frame.as_ref(),
            &[
                0, 3, 0, 0, 9, 0, 0, 0, // 状态 0、键长 3、保留、内容长 9（小端）
                b'a', b'b', b'c', 0, 0, 0, 0, 0, // 键，补到 8
                1, 2, 3, 4, 5, 6, 7, 8, // 内容
                9, 0, 0, 0, 0, 0, 0, 0, // 内容的尾巴，补到 8
            ][..]
        );
        // 键长、内容长恰好是 8 的倍数时不多补一格；没有内容的帧只有帧头和键。
        assert_eq!(encode_frame(FrameStatus::Found, "12345678", &[7; 16]).len(), 32);
        let missing = encode_frame(FrameStatus::Missing, "k", &[]);
        assert_eq!(missing.as_ref(), &[1, 1, 0, 0, 0, 0, 0, 0, b'k', 0, 0, 0, 0, 0, 0, 0][..]);
        assert_eq!(FrameStatus::ReadFailed as u8, 2);
        assert_eq!(FrameStatus::Deferred as u8, 3);
        // 最长的键（128 字节）也装得进一个字节的键长。
        let longest = "k".repeat(128);
        let frame = encode_frame(FrameStatus::Missing, &longest, &[]);
        assert_eq!(frame[1], 128);
        assert_eq!(frame.len(), 8 + 128);
    }

    #[test]
    fn keys_are_checked_before_anything_is_read() {
        assert_eq!(
            checked_keys(request(r#"{"keys":["1","e3d_baked_v2_ab"],"lod":0}"#)).unwrap(),
            keys(&["1", "e3d_baked_v2_ab"])
        );
        // 多余的字段不碍事：前端的通用封装可能带上 project / mdb。
        assert_eq!(
            checked_keys(request(r#"{"keys":["1"],"project":"AMS","mdb":"/ALL"}"#)).unwrap(),
            keys(&["1"])
        );
        for (body, why) in [
            (r#"{}"#, "不能为空"),
            (r#"{"keys":[]}"#, "不能为空"),
            (r#"{"keys":["1","1"]}"#, "重复"),
            (r#"{"keys":["a/b"]}"#, "只允许字母"),
            (r#"{"keys":["..\\x"]}"#, "只允许字母"),
            (r#"{"keys":[""]}"#, "只允许字母"),
            (r#"{"keys":["1"],"lod":1}"#, "只给原网格"),
        ] {
            let error = checked_keys(request(body)).unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error.code, "bad_request", "{body}");
            assert!(error.message.contains(why), "{body}: {}", error.message);
        }
        let exactly = (0..MAX_MESH_BATCH_KEYS).map(|i| format!("k{i}")).collect::<Vec<_>>();
        assert_eq!(
            checked_keys(MeshBatchReq { keys: exactly.clone(), lod: None }).unwrap().len(),
            MAX_MESH_BATCH_KEYS
        );
        let mut one_more = exactly;
        one_more.push("extra".into());
        let error = checked_keys(MeshBatchReq { keys: one_more, lod: None }).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("1025"), "{}", error.message);
        // 超长的坏键不原样塞回报错里。
        let error = checked_keys(MeshBatchReq { keys: vec!["x".repeat(5000)], lod: None }).unwrap_err();
        assert!(error.message.chars().count() < 200, "{}", error.message.len());
    }

    /// 一批里有的、没有的、读不出来的各回各的状态；帧与请求的键逐条对应、同序；
    /// 有内容的帧与磁盘上的文件逐字节相同；每段内容都落在 8 的倍数上。
    #[tokio::test]
    async fn every_key_gets_one_frame_in_request_order_with_the_file_bytes() {
        let dir = tempfile::tempdir().expect("临时网格目录");
        let files: Vec<(String, Vec<u8>)> = (0..40u32)
            .map(|i| {
                let bytes = (0..(i * 37 + 1)).map(|b| (b % 251) as u8).collect::<Vec<u8>>();
                (format!("e3d_baked_v2_{i:04}"), bytes)
            })
            .collect();
        for (key, bytes) in &files {
            std::fs::write(dir.path().join(format!("{key}.mesh")), bytes).unwrap();
        }
        // 读不出来的那一种：同名的是个目录。
        std::fs::create_dir(dir.path().join("a_directory.mesh")).unwrap();

        let mut asked = Vec::new();
        for (index, (key, _)) in files.iter().enumerate() {
            asked.push(key.clone());
            if index == 3 {
                asked.push("not_there".into());
            }
            if index == 20 {
                asked.push("a_directory".into());
            }
        }
        let body = collect(batch_stream(
            dir.path().to_path_buf(),
            asked.clone(),
            MAX_MESH_BATCH_BYTES,
        ))
        .await;
        let (declared, frames, complete) = decode(&body);
        assert!(complete, "流尾完好");
        assert_eq!(declared as usize, asked.len());
        assert_eq!(
            frames.iter().map(|frame| frame.key.clone()).collect::<Vec<_>>(),
            asked,
            "帧与请求的键逐条对应、同序"
        );
        let on_disk: std::collections::HashMap<_, _> = files.iter().cloned().collect();
        for frame in &frames {
            assert_eq!(frame.payload_offset % 8, 0, "{}", frame.key);
            match frame.key.as_str() {
                "not_there" => {
                    assert_eq!(frame.status, FrameStatus::Missing as u8);
                    assert!(frame.payload.is_empty());
                }
                "a_directory" => {
                    assert_eq!(frame.status, FrameStatus::ReadFailed as u8);
                    assert!(frame.payload.is_empty());
                }
                key => {
                    assert_eq!(frame.status, FrameStatus::Found as u8, "{key}");
                    assert_eq!(&frame.payload, &on_disk[key], "{key}");
                }
            }
        }
    }

    /// 总量到线：按请求顺序累计，超的那一帧和之后的都回「没读」，哪怕后面的文件很小、
    /// 或者根本不存在；到线之前的照常。
    #[tokio::test]
    async fn frames_past_the_byte_limit_are_deferred_to_the_end() {
        let dir = tempfile::tempdir().unwrap();
        for (key, len) in [("a", 100usize), ("b", 100), ("c", 100), ("d", 1), ("e", 100)] {
            std::fs::write(dir.path().join(format!("{key}.mesh")), vec![9u8; len]).unwrap();
        }
        let asked = keys(&["a", "gone", "b", "c", "d", "also_gone", "e"]);
        let body = collect(batch_stream(dir.path().to_path_buf(), asked.clone(), 250)).await;
        let (_, frames, complete) = decode(&body);
        assert!(complete);
        assert_eq!(
            frames.iter().map(|frame| frame.status).collect::<Vec<_>>(),
            vec![0, 1, 0, 3, 3, 3, 3],
            "a、b 共 200 字节在线内；c 加上去是 300，超了，从它起全部没读"
        );
        assert_eq!(frames.iter().map(|frame| frame.payload.len()).sum::<usize>(), 200);
        assert_eq!(
            frames.iter().map(|frame| frame.key.clone()).collect::<Vec<_>>(),
            asked
        );

        // 恰好到线不算超。
        let body = collect(batch_stream(dir.path().to_path_buf(), keys(&["a", "b", "d"]), 201)).await;
        let (_, frames, _) = decode(&body);
        assert_eq!(frames.iter().map(|frame| frame.status).collect::<Vec<_>>(), vec![0, 0, 0]);
    }

    /// 单个网格比上限还大也取得到：还没给出过内容时不裁。否则这个键永远拿不到。
    #[tokio::test]
    async fn the_first_delivered_mesh_is_never_deferred_however_large() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.mesh"), vec![1u8; 500]).unwrap();
        std::fs::write(dir.path().join("small.mesh"), vec![2u8; 10]).unwrap();

        let body = collect(batch_stream(dir.path().to_path_buf(), keys(&["big", "small"]), 100)).await;
        let (_, frames, complete) = decode(&body);
        assert!(complete);
        assert_eq!(frames.iter().map(|frame| frame.status).collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!(frames[0].payload.len(), 500);

        // 前面的键不存在、没给出过内容，大的那个仍然给。
        let body = collect(batch_stream(dir.path().to_path_buf(), keys(&["gone", "big", "small"]), 100)).await;
        let (_, frames, _) = decode(&body);
        assert_eq!(frames.iter().map(|frame| frame.status).collect::<Vec<_>>(), vec![1, 0, 3]);
    }

    /// 键多于读盘窗口时顺序仍然不乱（窗口里靠后的先读完也要等前面的）。
    #[tokio::test]
    async fn order_survives_more_keys_than_the_read_window() {
        let dir = tempfile::tempdir().unwrap();
        let asked: Vec<String> = (0..(READS_PER_REQUEST * 9 + 5)).map(|i| format!("m{i}")).collect();
        for (index, key) in asked.iter().enumerate() {
            // 大小交错：让靠后的小文件比靠前的大文件先读完。
            let len = if index % 2 == 0 { 200_000 } else { 3 };
            std::fs::write(dir.path().join(format!("{key}.mesh")), vec![(index % 250) as u8; len]).unwrap();
        }
        let body = collect(batch_stream(dir.path().to_path_buf(), asked.clone(), MAX_MESH_BATCH_BYTES)).await;
        let (_, frames, complete) = decode(&body);
        assert!(complete);
        assert_eq!(frames.len(), asked.len());
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.key, asked[index]);
            assert_eq!(frame.status, 0);
            assert!(frame.payload.iter().all(|byte| *byte == (index % 250) as u8));
        }
    }

    /// 响应体被截断时，参考解码器能认出来：没有流尾就不算完整，已经收全的帧仍然可用。
    #[tokio::test]
    async fn a_truncated_body_is_recognised_by_the_missing_trailer() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mesh"), [1u8; 20]).unwrap();
        std::fs::write(dir.path().join("b.mesh"), [2u8; 20]).unwrap();
        let body = collect(batch_stream(dir.path().to_path_buf(), keys(&["a", "b"]), MAX_MESH_BATCH_BYTES)).await;
        assert!(decode(&body).2);
        let (declared, frames, complete) = decode(&body[..body.len() - 8]);
        assert_eq!((declared, frames.len(), complete), (2, 2, false), "只少了流尾");
        let (declared, frames, complete) = decode(&body[..body.len() - 12]);
        assert_eq!((declared, frames.len(), complete), (2, 1, false), "第二帧没收全");
        assert_eq!(frames[0].payload, vec![1u8; 20]);
    }

    type Extracted = Result<Json<MeshBatchReq>, JsonRejection>;

    fn app(dir: PathBuf) -> Router {
        Router::new().route(
            "/api/v1/meshes/batch",
            post(move |request: Extracted| respond(move || dir, request)),
        )
    }

    async fn post_json(app: Router, body: &'static str) -> Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/meshes/batch")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("service answers")
    }

    /// 走一遍 HTTP：200 的头、流式的体；请求不对时是 spec §3 形状的 400，而不是框架的纯文本。
    #[tokio::test]
    async fn the_route_streams_frames_and_rejects_bad_requests_in_the_api_error_shape() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("1.mesh"), b"unit box").unwrap();

        let response = post_json(app(dir.path().to_path_buf()), r#"{"keys":["1","nope"]}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
        assert!(
            response.headers().get(header::CONTENT_LENGTH).is_none(),
            "流式响应不预先给长度"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let (declared, frames, complete) = decode(&body);
        assert!(complete);
        assert_eq!(declared, 2);
        assert_eq!((frames[0].status, frames[0].payload.as_slice()), (0, &b"unit box"[..]));
        assert_eq!((frames[1].status, frames[1].key.as_str()), (1, "nope"));

        for body in [
            r#"{"keys":[]}"#,
            r#"{"keys":["../1"]}"#,
            r#"{"keys":"1"}"#,
            r#"not json"#,
            r#"{"keys":["1"],"lod":1}"#,
        ] {
            let response = post_json(app(dir.path().to_path_buf()), body).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let error: serde_json::Value = serde_json::from_slice(&bytes).expect("错误体是 JSON");
            assert_eq!(error["code"], "bad_request", "{body}");
            assert!(error["message"].is_string(), "{body}");
        }

        // 请求体的类型头不对也按 400 回（框架缺省是 415 纯文本）。
        let response = app(dir.path().to_path_buf())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/meshes/batch")
                    .body(Body::from(r#"{"keys":["1"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// 请求不对的时候不解析网格目录：那一步会读配置、建目录。
    #[tokio::test]
    async fn the_mesh_directory_is_not_resolved_for_a_rejected_request() {
        let app = Router::new().route(
            "/api/v1/meshes/batch",
            post(|request: Extracted| {
                respond(
                    || -> PathBuf { panic!("请求被拒时不该解析网格目录") },
                    request,
                )
            }),
        );
        let response = post_json(app, r#"{"keys":["a b"]}"#).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// 请求体超过框架上限：照实回 413，形状仍是 spec §3 的。
    #[tokio::test]
    async fn an_oversized_request_body_is_a_413_in_the_api_error_shape() {
        let dir = tempfile::tempdir().unwrap();
        let huge = format!(r#"{{"keys":["{}"]}}"#, "x".repeat(3 * 1024 * 1024));
        let response = app(dir.path().to_path_buf())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/meshes/batch")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(huge))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let error: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["code"], "payload_too_large");
    }
}
