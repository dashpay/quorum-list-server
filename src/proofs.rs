//! Binary Core proof relay. No quorum keys or roots from this service are trusted.
use crate::config::Config;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use dashcore_rpc::{Auth, Client, RpcApi};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const MAX_PROOF: usize = 1_048_576;
const CACHE_BYTES: usize = 16 * MAX_PROOF;
const TTL: Duration = Duration::from_secs(15);

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
    workers: Arc<Semaphore>,
    cache: Arc<Mutex<VecDeque<CachedProof>>>,
}
pub fn router(config: Config) -> Router {
    Router::new()
        .route("/proofs", post(serve))
        .layer(DefaultBodyLimit::max(1024))
        .with_state(ProofRelay {
            config: Arc::new(config),
            workers: Arc::new(Semaphore::new(2)),
            cache: Arc::new(Mutex::new(VecDeque::new())),
        })
}
fn decode_response(value: &serde_json::Value) -> Result<Vec<u8>, StatusCode> {
    let hex = value
        .get("bootstrap_hex")
        .and_then(serde_json::Value::as_str)
        .ok_or(StatusCode::BAD_GATEWAY)?;
    if hex.is_empty() || hex.len() > MAX_PROOF * 2 {
        return Err(StatusCode::BAD_GATEWAY);
    }
    hex::decode(hex).map_err(|_| StatusCode::BAD_GATEWAY)
}
async fn serve(State(relay): State<ProofRelay>, Json(request): Json<ProofRequest>) -> Response {
    match relay.proof(request).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(status) => status.into_response(),
    }
}
impl ProofRelay {
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
        let permit = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let config = self.config.clone();
        let params = request.params();
        let task = tokio::task::spawn_blocking(move || {
            // Keep capacity occupied until the blocking RPC ends, including after timeout.
            let _permit = permit;
            let client = Client::new(
                &config.rpc.url,
                Auth::UserPass(config.rpc.username.clone(), config.rpc.password.clone()),
            )?;
            client.call::<serde_json::Value>("getquorumproofchain", &params)
        });
        let value = tokio::time::timeout(Duration::from_secs(60), task)
            .await
            .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
            .map_err(|_| StatusCode::BAD_GATEWAY)?
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
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
        let rpc = Router::new().route("/", post(move |Json(value): Json<serde_json::Value>| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                assert_eq!(value["method"], "getquorumproofchain");
                assert_eq!(value["params"][3], 6);
                Json(serde_json::json!({"result":{"bootstrap_hex":hex::encode(fixture)}, "error":null, "id":value["id"]}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config::default();
        config.rpc.url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, rpc).await.unwrap();
        });
        let mut app = router(config);
        let payload = serde_json::json!({"checkpoint":"ab".repeat(32),"height":1,"quorumHash":"cd".repeat(32),"llmqType":6,"nodeCount":4}).to_string();
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

    #[test]
    fn rejects_bad_requests_and_unbounded_responses() {
        let mut request = ProofRequest {
            checkpoint: "ab".repeat(32),
            height: 0,
            quorum_hash: "cd".repeat(32),
            llmq_type: 6,
            node_count: 4,
        };
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
