//! Binary Core proof relay. No quorum keys or roots from this service are trusted.
use crate::config::Config;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fmt::Arguments,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const MAX_PROOF: usize = 1_048_576;
/// Core returns `proof_hex` and `bootstrap_hex`, each at most twice the decoded
/// limit, plus a small `target` object and the JSON-RPC envelope. Anything
/// larger is cut off at the socket before it is buffered or parsed.
const MAX_UPSTREAM: usize = 4 * MAX_PROOF + 65_536;
const CACHE_BYTES: usize = 16 * MAX_PROOF;
const TTL: Duration = Duration::from_secs(15);
/// Single deadline for the whole Core round trip. Cancelling the future drops
/// the connection, so a slow, hung, or truncating upstream cannot pin a worker.
const RPC_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProofRequest {
    checkpoint: String,
    #[serde(default)]
    height: u32,
    quorum_hash: String,
    llmq_type: u8,
    node_count: u8,
}
impl ProofRequest {
    fn valid(&self) -> bool {
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        hash(&self.checkpoint)
            && hash(&self.quorum_hash)
            && matches!(self.llmq_type, 4 | 6)
            && self.node_count <= 15
            && self.height <= i32::MAX as u32
    }
    fn params(&self) -> Vec<serde_json::Value> {
        vec![
            self.checkpoint.clone().into(),
            self.height.into(),
            self.quorum_hash.clone().into(),
            self.llmq_type.into(),
            self.node_count.into(),
        ]
    }
}
struct CachedProof {
    request: ProofRequest,
    inserted: Instant,
    bytes: Vec<u8>,
}
#[derive(Clone)]
struct ProofRelay {
    config: Arc<Config>,
    client: Client<HttpConnector, Full<Bytes>>,
    deadline: Duration,
    workers: Arc<Semaphore>,
    cache: Arc<Mutex<VecDeque<CachedProof>>>,
}
pub fn router(config: Config) -> Router {
    ProofRelay::new(config, RPC_TIMEOUT).router()
}
/// Records the internal cause (never credentials or response bodies) before it
/// is collapsed into the public status code.
fn fail(status: StatusCode, cause: Arguments<'_>) -> StatusCode {
    eprintln!("proofs: {status} <- {cause}");
    status
}
fn decode_response(value: &serde_json::Value) -> Result<Vec<u8>, StatusCode> {
    let hex = value
        .get("bootstrap_hex")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            fail(
                StatusCode::BAD_GATEWAY,
                format_args!("missing bootstrap_hex"),
            )
        })?;
    if hex.is_empty() || hex.len() > MAX_PROOF * 2 {
        return Err(fail(
            StatusCode::BAD_GATEWAY,
            format_args!("bootstrap_hex length {}", hex.len()),
        ));
    }
    hex::decode(hex).map_err(|e| fail(StatusCode::BAD_GATEWAY, format_args!("bootstrap_hex: {e}")))
}
async fn serve(State(relay): State<ProofRelay>, Json(request): Json<ProofRequest>) -> Response {
    match relay.proof(request).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(status) => status.into_response(),
    }
}
impl ProofRelay {
    fn new(config: Config, deadline: Duration) -> Self {
        Self {
            config: Arc::new(config),
            client: Client::builder(TokioExecutor::new()).build_http(),
            deadline,
            workers: Arc::new(Semaphore::new(2)),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
    fn router(self) -> Router {
        Router::new()
            .route("/proofs", post(serve))
            .layer(DefaultBodyLimit::max(1024))
            .with_state(self)
    }
    /// One `getquorumproofchain` call. The response is bounded at the socket
    /// before it is buffered, and Core's error object is logged, not exposed.
    async fn fetch(&self, params: Vec<serde_json::Value>) -> Result<serde_json::Value, StatusCode> {
        let rpc = &self.config.rpc;
        let auth = base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", rpc.username, rpc.password));
        let body = serde_json::json!({
            "jsonrpc": "1.0", "id": "proofs", "method": "getquorumproofchain", "params": params,
        });
        let request = hyper::Request::post(rpc.url.as_str())
            .header(header::AUTHORIZATION, format!("Basic {auth}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .map_err(|e| fail(StatusCode::BAD_GATEWAY, format_args!("RPC request: {e}")))?;
        let response = self
            .client
            .request(request)
            .await
            .map_err(|e| fail(StatusCode::BAD_GATEWAY, format_args!("RPC transport: {e}")))?;
        let status = response.status();
        let body = Limited::new(response.into_body(), MAX_UPSTREAM)
            .collect()
            .await
            .map_err(|e| {
                fail(
                    StatusCode::BAD_GATEWAY,
                    format_args!("RPC {status} body: {e}"),
                )
            })?
            .to_bytes();
        let mut envelope: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| fail(StatusCode::BAD_GATEWAY, format_args!("RPC {status}: {e}")))?;
        if let Some(error) = envelope.get("error").filter(|error| !error.is_null()) {
            let error: String = error.to_string().chars().take(200).collect();
            return Err(fail(
                StatusCode::BAD_GATEWAY,
                format_args!("RPC {status}: {error}"),
            ));
        }
        envelope
            .get_mut("result")
            .map(serde_json::Value::take)
            .ok_or_else(|| {
                fail(
                    StatusCode::BAD_GATEWAY,
                    format_args!("RPC {status}: no result"),
                )
            })
    }
    async fn proof(&self, request: ProofRequest) -> Result<Vec<u8>, StatusCode> {
        if !request.valid() {
            return Err(StatusCode::BAD_REQUEST);
        }
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            cache.retain(|entry| entry.inserted.elapsed() < TTL);
            if let Some(entry) = cache.iter().find(|entry| entry.request == request) {
                return Ok(entry.bytes.clone());
            }
        }
        // The permit is released on every exit path, including the deadline,
        // because the cancelled fetch drops its connection with it.
        let _permit = self
            .workers
            .try_acquire()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let value = tokio::time::timeout(self.deadline, self.fetch(request.params()))
            .await
            .map_err(|_| {
                fail(
                    StatusCode::GATEWAY_TIMEOUT,
                    format_args!("RPC exceeded {:?}", self.deadline),
                )
            })??;
        let bytes = decode_response(&value)?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut size: usize = cache.iter().map(|entry| entry.bytes.len()).sum();
        while size + bytes.len() > CACHE_BYTES || cache.len() >= 64 {
            if let Some(entry) = cache.pop_front() {
                size -= entry.bytes.len();
            } else {
                break;
            }
        }
        cache.push_back(CachedProof {
            request,
            inserted: Instant::now(),
            bytes: bytes.clone(),
        });
        Ok(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn request() -> ProofRequest {
        ProofRequest {
            checkpoint: "ab".repeat(32),
            height: 1,
            quorum_hash: "cd".repeat(32),
            llmq_type: 6,
            node_count: 4,
        }
    }
    fn relay_for(addr: std::net::SocketAddr, deadline: Duration) -> ProofRelay {
        let mut config = Config::default();
        config.rpc.url = format!("http://{addr}");
        ProofRelay::new(config, deadline)
    }
    /// Raw HTTP server that runs `respond` on every connection once the request
    /// headers arrive, so malformed upstream behaviour can be scripted exactly.
    async fn raw_server<F, Fut>(respond: F) -> std::net::SocketAddr
    where
        F: Fn(tokio::net::TcpStream) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let respond = Arc::new(respond);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let respond = respond.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let mut seen = Vec::new();
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    respond(stream).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn serves_core_bytes_and_caches_without_refetching() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tower::Service;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let fixture = include_bytes!("../tests/data/bootstrap.bin");
        let rpc = Router::new().route(
            "/",
            post(
                move |headers: HeaderMap, Json(value): Json<serde_json::Value>| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let auth = headers[header::AUTHORIZATION].to_str().unwrap();
                        assert_eq!(auth, "Basic ZGFzaHJwYzpwYXNzd29yZA==");
                        assert_eq!(value["method"], "getquorumproofchain");
                        assert_eq!(value["params"][3], 6);
                        Json(serde_json::json!({"result":{"bootstrap_hex":hex::encode(fixture)}, "error":null, "id":value["id"]}))
                    }
                },
            ),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config::default();
        config.rpc.url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, rpc).await.unwrap();
        });
        let mut app = router(config);
        let payload = serde_json::to_string(&request()).unwrap();
        for _ in 0..2 {
            let response = app
                .call(
                    Request::post("/proofs")
                        .header("content-type", "application/json")
                        .body(Body::from(payload.clone()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/octet-stream"
            );
            assert_eq!(
                to_bytes(response.into_body(), MAX_PROOF)
                    .await
                    .unwrap()
                    .as_ref(),
                fixture
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let response = app
            .call(
                Request::post("/proofs")
                    .header("content-type", "application/json")
                    .body(Body::from(" ".repeat(1025)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test]
    async fn truncated_headers_fail_fast_and_release_the_worker() {
        let addr = raw_server(|mut stream| async move {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n").await;
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(5));
        let started = Instant::now();
        assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(relay.workers.available_permits(), 2);
    }

    #[tokio::test]
    async fn oversized_upstream_bodies_are_cut_off_at_the_socket() {
        let addr = raw_server(|mut stream| async move {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                MAX_UPSTREAM * 4
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let chunk = vec![b' '; 65_536];
            // Stops once the relay drops the connection at the limit.
            while stream.write_all(&chunk).await.is_ok() {}
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(10));
        assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        assert_eq!(relay.workers.available_permits(), 2);
    }

    #[tokio::test]
    async fn rpc_errors_are_not_cached_and_do_not_leak_workers() {
        let addr = raw_server(|mut stream| async move {
            let body = r#"{"result":null,"error":{"code":-32601,"message":"Method not found"},"id":"proofs"}"#;
            let head = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(body.as_bytes()).await;
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(5));
        for _ in 0..2 {
            assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        }
        assert_eq!(relay.workers.available_permits(), 2);
        assert!(relay.cache.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deadline_releases_workers_and_saturation_returns_503() {
        let addr = raw_server(|stream| async move {
            // Hold the connection open without ever answering.
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(stream);
        })
        .await;
        let relay = relay_for(addr, Duration::from_millis(300));
        let slow = (0..2)
            .map(|_| {
                let relay = relay.clone();
                tokio::spawn(async move { relay.proof(request()).await })
            })
            .collect::<Vec<_>>();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(relay.workers.available_permits(), 0);
        assert_eq!(
            relay.proof(request()).await,
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
        for task in slow {
            assert_eq!(task.await.unwrap(), Err(StatusCode::GATEWAY_TIMEOUT));
        }
        assert_eq!(relay.workers.available_permits(), 2);
    }

    #[test]
    fn rejects_bad_requests_and_unbounded_responses() {
        let mut request = request();
        assert!(request.valid());
        request.node_count = 16;
        assert!(!request.valid());
        request.node_count = 4;
        request.checkpoint.push('a');
        assert!(!request.valid());
        for value in [
            serde_json::json!({}),
            serde_json::json!({"bootstrap_hex":""}),
            serde_json::json!({"bootstrap_hex":"xx"}),
            serde_json::json!({"bootstrap_hex":"aa".repeat(MAX_PROOF + 1)}),
        ] {
            assert!(decode_response(&value).is_err());
        }
        assert_eq!(
            decode_response(&serde_json::json!({"bootstrap_hex":"aabb"})).unwrap(),
            [0xaa, 0xbb]
        );
    }
}
