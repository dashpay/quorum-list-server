//! Binary Core proof relay. No quorum keys or roots from this service are trusted.
use crate::config::Config;
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use futures::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Arguments,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MAX_PROOF: usize = 1_048_576;
/// Core returns `proof_hex` and `bootstrap_hex`, each at most twice the decoded
/// limit, plus a small `target` object and the JSON-RPC envelope. Anything
/// larger is cut off at the socket before it is buffered or parsed.
const MAX_UPSTREAM: usize = 4 * MAX_PROOF + 65_536;
const CACHE_BYTES: usize = 16 * MAX_PROOF;
const CACHE_ENTRIES: usize = 64;
const TTL: Duration = Duration::from_secs(15);
/// Single deadline for the whole Core round trip. Cancelling the future drops
/// the connection, so a slow, hung, or truncating upstream cannot pin a worker.
const RPC_TIMEOUT: Duration = Duration::from_secs(60);
/// Core runs a dispatched `getquorumproofchain` to completion even after its
/// HTTP client is gone, so a worker that hit the deadline stays reserved this
/// much longer before the relay submits more work to Core.
const COOLDOWN: Duration = Duration::from_secs(60);

type Inflight = Shared<BoxFuture<'static, Result<Bytes, StatusCode>>>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
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
    bytes: Bytes,
}
#[derive(Clone)]
struct ProofRelay {
    config: Arc<Config>,
    client: Client<HttpConnector, Full<Bytes>>,
    deadline: Duration,
    cooldown: Duration,
    workers: Arc<Semaphore>,
    cache: Arc<Mutex<VecDeque<CachedProof>>>,
    inflight: Arc<Mutex<HashMap<ProofRequest, Inflight>>>,
}
pub fn router(config: Config) -> Router {
    ProofRelay::new(config, RPC_TIMEOUT, COOLDOWN).router()
}
/// Records the internal cause (never credentials or response bodies) before it
/// is collapsed into the public status code.
fn fail(status: StatusCode, cause: Arguments<'_>) -> StatusCode {
    eprintln!("proofs: {status} <- {cause}");
    status
}
/// hyper's transport errors display only a category ("client error (Connect)");
/// the operator-relevant cause such as "connection refused" sits in the chain.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut next = error.source();
    while let Some(cause) = next {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        next = cause.source();
    }
    text
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
async fn serve(
    State(relay): State<ProofRelay>,
    request: Result<Json<ProofRequest>, JsonRejection>,
) -> Response {
    // Public errors are status codes only; extractor diagnostics stay internal.
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return rejection.status().into_response(),
    };
    match relay.proof(request).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(status) => status.into_response(),
    }
}
impl ProofRelay {
    fn new(mut config: Config, deadline: Duration, cooldown: Duration) -> Self {
        // The other RPC clients accept a bare host:port; hyper needs a scheme.
        if !config.rpc.url.contains("://") {
            config.rpc.url = format!("http://{}", config.rpc.url);
        }
        Self {
            config: Arc::new(config),
            client: Client::builder(TokioExecutor::new()).build_http(),
            deadline,
            cooldown,
            workers: Arc::new(Semaphore::new(2)),
            cache: Arc::new(Mutex::new(VecDeque::new())),
            inflight: Arc::new(Mutex::new(HashMap::new())),
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
        let response = self.client.request(request).await.map_err(|e| {
            fail(
                StatusCode::BAD_GATEWAY,
                format_args!("RPC transport: {}", chain(&e)),
            )
        })?;
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
            let code = error.get("code").and_then(serde_json::Value::as_i64);
            let message: String = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            return Err(fail(
                StatusCode::BAD_GATEWAY,
                format_args!("RPC {status}: error {code:?} {message}"),
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
    fn cached(&self, request: &ProofRequest) -> Result<Option<Bytes>, StatusCode> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        cache.retain(|entry| entry.inserted.elapsed() < TTL);
        Ok(cache
            .iter()
            .find(|entry| &entry.request == request)
            .map(|entry| entry.bytes.clone()))
    }
    fn store(&self, request: ProofRequest, bytes: Bytes) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        let mut size: usize = cache.iter().map(|entry| entry.bytes.len()).sum();
        while size + bytes.len() > CACHE_BYTES || cache.len() >= CACHE_ENTRIES {
            match cache.pop_front() {
                Some(entry) => size -= entry.bytes.len(),
                None => break,
            }
        }
        cache.push_back(CachedProof {
            request,
            inserted: Instant::now(),
            bytes,
        });
    }
    /// Runs detached from any public connection: the worker permit is released
    /// only when Core has answered or, after the deadline, once the cooldown
    /// has passed. Waiters get the result as soon as it exists.
    async fn build(
        self,
        request: ProofRequest,
        permit: OwnedSemaphorePermit,
    ) -> Result<Bytes, StatusCode> {
        let result = match tokio::time::timeout(self.deadline, self.fetch(request.params())).await {
            Ok(value) => value
                .and_then(|value| decode_response(&value))
                .map(Bytes::from),
            Err(_) => {
                let cooldown = self.cooldown;
                tokio::spawn(async move {
                    let _permit = permit;
                    tokio::time::sleep(cooldown).await;
                });
                Err(fail(
                    StatusCode::GATEWAY_TIMEOUT,
                    format_args!("RPC exceeded {:?}", self.deadline),
                ))
            }
        };
        if let Ok(bytes) = &result {
            self.store(request.clone(), bytes.clone());
        }
        if let Ok(mut inflight) = self.inflight.lock() {
            inflight.remove(&request);
        }
        result
    }
    async fn proof(&self, request: ProofRequest) -> Result<Bytes, StatusCode> {
        if !request.valid() {
            return Err(StatusCode::BAD_REQUEST);
        }
        if let Some(bytes) = self.cached(&request)? {
            return Ok(bytes);
        }
        let inflight = {
            let mut inflight = self
                .inflight
                .lock()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            match inflight.get(&request) {
                // Identical requests share one Core call and one worker.
                Some(existing) => existing.clone(),
                None => {
                    let permit = self
                        .workers
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    let task = tokio::spawn(self.clone().build(request.clone(), permit));
                    let shared = async move { task.await.unwrap_or(Err(StatusCode::BAD_GATEWAY)) }
                        .boxed()
                        .shared();
                    inflight.insert(request, shared.clone());
                    shared
                }
            }
        };
        inflight.await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    const FIXTURE: &[u8] = include_bytes!("../tests/data/bootstrap.bin");

    fn request() -> ProofRequest {
        ProofRequest {
            checkpoint: "ab".repeat(32),
            height: 1,
            quorum_hash: "cd".repeat(32),
            llmq_type: 6,
            node_count: 4,
        }
    }
    fn relay_for(addr: std::net::SocketAddr, deadline: Duration, cooldown: Duration) -> ProofRelay {
        let mut config = Config::default();
        config.rpc.url = format!("http://{addr}");
        ProofRelay::new(config, deadline, cooldown)
    }
    fn rpc_ok() -> String {
        let body = serde_json::json!({"result":{"bootstrap_hex":hex::encode(FIXTURE)},"error":null,"id":"proofs"})
            .to_string();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
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
    async fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..2000 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("condition not met within 2s");
    }

    #[tokio::test]
    async fn serves_core_bytes_and_caches_without_refetching() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::Service;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
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
                        Json(serde_json::json!({"result":{"bootstrap_hex":hex::encode(FIXTURE)}, "error":null, "id":value["id"]}))
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
        let post_json = |body: String| {
            Request::post("/proofs")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap()
        };
        let payload = serde_json::to_string(&request()).unwrap();
        for _ in 0..2 {
            let response = app.call(post_json(payload.clone())).await.unwrap();
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
                FIXTURE
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Extractor rejections follow the same status-only contract.
        for (body, status) in [
            (" ".repeat(1025), StatusCode::PAYLOAD_TOO_LARGE),
            ("{".to_string(), StatusCode::BAD_REQUEST),
            ("{}".to_string(), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let response = app.call(post_json(body)).await.unwrap();
            assert_eq!(response.status(), status);
            assert!(to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test]
    async fn truncated_headers_fail_fast_and_release_the_worker() {
        let addr = raw_server(|mut stream| async move {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n").await;
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(5), Duration::ZERO);
        let started = Instant::now();
        assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(relay.workers.available_permits(), 2);
    }

    #[tokio::test]
    async fn oversized_upstream_bodies_are_cut_off_at_the_socket() {
        let written = Arc::new(AtomicUsize::new(0));
        let counter = written.clone();
        let addr = raw_server(move |mut stream| {
            let counter = counter.clone();
            async move {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    MAX_UPSTREAM * 4
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let chunk = vec![b' '; 65_536];
                // Stops once the relay drops the connection at the limit.
                while stream.write_all(&chunk).await.is_ok() {
                    counter.fetch_add(chunk.len(), Ordering::SeqCst);
                }
            }
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(10), Duration::ZERO);
        assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        assert_eq!(relay.workers.available_permits(), 2);
        // Kernel socket buffers absorb a little past the limit, never the 16 MiB.
        assert!(written.load(Ordering::SeqCst) < 3 * MAX_UPSTREAM);
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
        let relay = relay_for(addr, Duration::from_secs(5), Duration::ZERO);
        for _ in 0..2 {
            assert_eq!(relay.proof(request()).await, Err(StatusCode::BAD_GATEWAY));
        }
        assert_eq!(relay.workers.available_permits(), 2);
        assert!(relay.cache.lock().unwrap().is_empty());
        assert!(relay.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deadline_closes_upstream_and_reserves_workers_through_the_cooldown() {
        let (eof_tx, mut eof_rx) = tokio::sync::mpsc::unbounded_channel();
        let addr = raw_server(move |mut stream| {
            let eof_tx = eof_tx.clone();
            async move {
                // Never answer; report when the relay closes the connection.
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
                let _ = eof_tx.send(Instant::now());
            }
        })
        .await;
        let deadline = Duration::from_millis(300);
        let cooldown = Duration::from_millis(600);
        let relay = relay_for(addr, deadline, cooldown);
        let started = Instant::now();
        let slow = [1, 2].map(|height| {
            let relay = relay.clone();
            let request = ProofRequest {
                height,
                ..request()
            };
            tokio::spawn(async move { relay.proof(request).await })
        });
        wait_until(|| relay.workers.available_permits() == 0).await;
        assert_eq!(
            relay
                .proof(ProofRequest {
                    height: 3,
                    ..request()
                })
                .await,
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
        for task in slow {
            assert_eq!(task.await.unwrap(), Err(StatusCode::GATEWAY_TIMEOUT));
        }
        // Both upstream sockets were closed at the deadline, not at the cooldown.
        for _ in 0..2 {
            let closed = tokio::time::timeout(Duration::from_secs(1), eof_rx.recv())
                .await
                .expect("upstream never saw EOF")
                .unwrap();
            assert!(closed - started < deadline + Duration::from_millis(200));
        }
        assert_eq!(relay.workers.available_permits(), 0);
        assert!(relay.inflight.lock().unwrap().is_empty());
        wait_until(|| relay.workers.available_permits() == 2).await;
        assert!(started.elapsed() >= deadline + cooldown);
    }

    #[tokio::test]
    async fn client_disconnect_keeps_the_worker_until_core_answers() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let addr = raw_server(move |mut stream| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = stream.write_all(rpc_ok().as_bytes()).await;
            }
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(5), Duration::ZERO);
        let handler = {
            let relay = relay.clone();
            tokio::spawn(async move { relay.proof(request()).await })
        };
        wait_until(|| relay.workers.available_permits() == 1).await;
        // The public connection goes away; the Core call and its permit do not.
        handler.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(relay.workers.available_permits(), 1);
        assert_eq!(relay.inflight.lock().unwrap().len(), 1);
        wait_until(|| relay.workers.available_permits() == 2).await;
        assert!(relay.inflight.lock().unwrap().is_empty());
        // The abandoned result was still cached, so nobody pays for it twice.
        assert_eq!(relay.proof(request()).await.unwrap().as_ref(), FIXTURE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identical_concurrent_requests_share_one_core_call() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let addr = raw_server(move |mut stream| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = stream.write_all(rpc_ok().as_bytes()).await;
            }
        })
        .await;
        let relay = relay_for(addr, Duration::from_secs(5), Duration::ZERO);
        let waiters = (0..3)
            .map(|_| {
                let relay = relay.clone();
                tokio::spawn(async move { relay.proof(request()).await })
            })
            .collect::<Vec<_>>();
        for waiter in waiters {
            assert_eq!(waiter.await.unwrap().unwrap().as_ref(), FIXTURE);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(relay.workers.available_permits(), 2);
        assert!(relay.inflight.lock().unwrap().is_empty());
    }

    #[test]
    fn cache_expires_isolates_keys_and_bounds_entries_and_bytes() {
        let relay = ProofRelay::new(Config::default(), RPC_TIMEOUT, COOLDOWN);
        let other = ProofRequest {
            height: 2,
            ..request()
        };
        relay.store(request(), Bytes::from_static(b"aa"));
        assert_eq!(relay.cached(&request()).unwrap().unwrap().as_ref(), b"aa");
        assert!(relay.cached(&other).unwrap().is_none());
        relay.cache.lock().unwrap()[0].inserted = Instant::now().checked_sub(TTL).unwrap();
        assert!(relay.cached(&request()).unwrap().is_none());
        for height in 0..CACHE_ENTRIES as u32 + 1 {
            relay.store(
                ProofRequest {
                    height,
                    ..request()
                },
                Bytes::from_static(b"x"),
            );
        }
        let cache = relay.cache.lock().unwrap();
        assert_eq!(cache.len(), CACHE_ENTRIES);
        assert_eq!(cache.front().unwrap().request.height, 1);
        drop(cache);
        relay.store(other.clone(), Bytes::from(vec![0; MAX_PROOF]));
        relay.store(request(), Bytes::from(vec![0; CACHE_BYTES - MAX_PROOF]));
        let cache = relay.cache.lock().unwrap();
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.front().unwrap().request, other);
        drop(cache);
        relay.store(request(), Bytes::from(vec![0; CACHE_BYTES]));
        assert_eq!(relay.cache.lock().unwrap().len(), 1);
    }

    #[test]
    fn bare_host_port_rpc_urls_get_a_scheme() {
        let mut config = Config::default();
        config.rpc.url = "127.0.0.1:19998".into();
        let relay = ProofRelay::new(config, RPC_TIMEOUT, COOLDOWN);
        assert_eq!(relay.config.rpc.url, "http://127.0.0.1:19998");
        config = Config::default();
        config.rpc.url = "https://core.example:9998".into();
        let relay = ProofRelay::new(config, RPC_TIMEOUT, COOLDOWN);
        assert_eq!(relay.config.rpc.url, "https://core.example:9998");
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
