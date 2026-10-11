use crate::config::{Config, Network};
use crate::quorum_cache::QuorumCache;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Quorum {
    pub id: u32,
    pub created: u32,
    pub retired: u32,
}

pub struct CoreState {
    pub tip: u32,
    pub quorums: Vec<Quorum>,
    pub calls: Vec<Value>,
    pub fail: bool,
    pub fail_current: bool,
    pub malformed: bool,
    pub empty: bool,
    pub fail_height: Option<u32>,
    pub fail_current_once: bool,
    pub delay: Duration,
    pub omitted_type_before: Option<u32>,
    pub commitments: std::collections::HashMap<u32, u32>,
    pub omit_at_height: Option<u32>,
    pub tip_script: std::collections::VecDeque<u32>,
    pub first_response_gate: Option<std::sync::mpsc::Receiver<()>>,
    pub omit_only: Option<u32>,
    pub historical_response_gate: Option<(u32, std::sync::mpsc::Receiver<()>)>,
    pub fail_block_count_at: Option<usize>,
    pub creation_heights: std::collections::HashMap<u32, u32>,
}

pub struct MockCore {
    pub config: Config,
    pub state: Arc<Mutex<CoreState>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MockCore {
    pub fn new(tip: u32, quorums: Vec<Quorum>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut config = Config {
            network: Network::Devnet,
            ..Config::default()
        };
        config.rpc.url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(CoreState {
            tip,
            quorums,
            calls: Vec::new(),
            fail: false,
            fail_current: false,
            malformed: false,
            empty: false,
            fail_height: None,
            fail_current_once: false,
            delay: Duration::ZERO,
            omitted_type_before: None,
            commitments: std::collections::HashMap::new(),
            omit_at_height: None,
            tip_script: std::collections::VecDeque::new(),
            first_response_gate: None,
            omit_only: None,
            historical_response_gate: None,
            fail_block_count_at: None,
            creation_heights: std::collections::HashMap::new(),
        }));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(e) => panic!("mock RPC accept failed: {e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                let (header_end, length) = loop {
                    let mut buf = [0; 4096];
                    let read = stream.read(&mut buf).unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buf[..read]);
                    if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..pos]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        break (pos + 4, length);
                    }
                };
                while bytes.len() < header_end + length {
                    let mut buf = [0; 4096];
                    let read = stream.read(&mut buf).unwrap();
                    assert!(read > 0);
                    bytes.extend_from_slice(&buf[..read]);
                }
                let request: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                let mut state = worker_state.lock().unwrap();
                state.calls.push(request.clone());
                if request["method"] == "getblockcount" {
                    if let Some(tip) = state.tip_script.pop_front() {
                        state.tip = tip;
                    }
                }
                let height = request["params"][1]
                    .as_u64()
                    .map(|h| h as u32)
                    .unwrap_or(state.tip);
                let is_list = request["params"][0] == "listextended";
                let fail_once = state.fail_current_once && is_list && height == state.tip;
                if fail_once {
                    state.fail_current_once = false;
                }
                let failed = state.fail
                    || fail_once
                    || (request["method"] == "getblockcount"
                        && state.fail_block_count_at
                            == Some(
                                state
                                    .calls
                                    .iter()
                                    .filter(|call| call["method"] == "getblockcount")
                                    .count(),
                            ))
                    || (is_list && state.fail_height == Some(height))
                    || (state.fail_current && is_list && height == state.tip);
                let result = if request["method"] == "getblockcount" {
                    json!(state.tip)
                } else if request["params"][0] == "listextended" {
                    if state
                        .omitted_type_before
                        .is_some_and(|activation| height < activation)
                    {
                        json!({"llmq_other": []})
                    } else if state.malformed {
                        json!({})
                    } else {
                        let entries: Vec<Value> = state.quorums.iter()
                            .filter(|q| !state.empty && (state.omit_at_height != Some(height) || state.omit_only.is_some_and(|id| id != q.id)) && q.created <= height && height < q.retired)
                            .map(|q| json!({hash(q.id): {"creationHeight":state.creation_heights.get(&q.id).copied().unwrap_or(q.created), "numValidMembers":12, "minedBlockHash":hash(q.id + 100000 + state.commitments.get(&q.id).copied().unwrap_or(0))}}))
                            .collect();
                        json!({"llmq_devnet_platform": entries})
                    }
                } else {
                    let id = u32::from_str_radix(&request["params"][2].as_str().unwrap()[56..], 16)
                        .unwrap();
                    let commitment = state.commitments.get(&id).copied().unwrap_or(0);
                    json!({"quorumPublicKey": format!("{:02x}", 17 + commitment).repeat(48)})
                };
                let body = if failed {
                    json!({"result":null, "error":{"code":-1,"message":"Core unavailable"}, "id":request["id"]})
                } else {
                    json!({"result":result, "error":null, "id":request["id"]})
                }.to_string();
                let delay = state.delay;
                let gate = if is_list
                    && state
                        .historical_response_gate
                        .as_ref()
                        .is_some_and(|(gated_height, _)| *gated_height == height)
                {
                    state.historical_response_gate.take().map(|(_, gate)| gate)
                } else {
                    state.first_response_gate.take()
                };
                drop(state);
                if let Some(gate) = gate {
                    let _ = gate.recv_timeout(Duration::from_secs(5));
                }
                std::thread::sleep(delay);
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            config,
            state,
            stop,
            worker: Some(worker),
        }
    }

    pub fn info_calls(&self) -> Vec<Value> {
        self.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|r| r["method"] == "quorum" && r["params"][0] == "info")
            .cloned()
            .collect()
    }
}

impl Drop for MockCore {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

pub fn hash(id: u32) -> String {
    format!("{id:064x}")
}

fn jameson() -> MockCore {
    MockCore::new(
        13271,
        vec![
            Quorum {
                id: 1,
                created: 13166,
                retired: 13262,
            },
            Quorum {
                id: 2,
                created: 13190,
                retired: u32::MAX,
            },
            Quorum {
                id: 3,
                created: 13214,
                retired: u32::MAX,
            },
            Quorum {
                id: 4,
                created: 13238,
                retired: u32::MAX,
            },
            Quorum {
                id: 5,
                created: 13262,
                retired: u32::MAX,
            },
        ],
    )
}

/// A cold client must retain a signer retired beyond the legacy snapshot.
#[tokio::test]
async fn should_serve_a_quorum_retired_after_the_previous_snapshot_height() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    assert!(!cache
        .get_quorums()
        .await
        .unwrap()
        .contains_quorum(&hex::decode(hash(1)).unwrap()));
    let (height, previous) = cache.get_previous_quorums().await.unwrap();
    assert!(
        previous.contains_quorum(&hex::decode(hash(1)).unwrap()),
        "signer active at tip-12 must survive a cold cache refresh even though absent at tip-8"
    );
    assert_eq!(height, 13223);
}

/// Successful and failed DKG schedules must leave no signer missing from the window.
#[tokio::test]
async fn should_cover_every_quorum_active_in_the_window() {
    for failed_dkg in [None, Some(8), Some(9)] {
        let starts: Vec<u32> = (0..20)
            .filter(|i| Some(*i) != failed_dkg)
            .map(|i| i * 24 + 10)
            .collect();
        let quorums: Vec<Quorum> = starts
            .iter()
            .enumerate()
            .map(|(i, &created)| Quorum {
                id: i as u32 + 1,
                created,
                retired: starts.get(i + 4).copied().unwrap_or(u32::MAX),
            })
            .collect();
        for (tip, retention, step) in [
            (250, 48, 48),
            (265, 48, 48),
            (299, 48, 48),
            (330, 48, 48),
            (250, 167, 73),
            (265, 167, 73),
            (299, 167, 73),
            (330, 167, 73),
        ] {
            let expected: Vec<String> = quorums
                .iter()
                .filter(|q| q.created <= tip && q.retired > tip - retention)
                .map(|q| hash(q.id))
                .collect();
            let mut core = MockCore::new(tip, quorums.clone());
            core.config.quorum.retention_blocks = retention;
            core.config.quorum.sample_step_blocks = Some(step);
            let cache = QuorumCache::new(core.config.clone());
            cache.refresh().await.unwrap();
            let (_, previous) = cache.get_previous_quorums().await.unwrap();
            let actual: Vec<String> = previous
                .iter()
                .map(|q| hex::encode(&q.quorum_hash))
                .collect();
            for quorum in expected {
                assert!(
                    actual.contains(&quorum),
                    "missing signer {quorum} at tip {tip}, failed DKG {failed_dkg:?}"
                );
            }
        }
    }
}

/// A cold restart must reconstruct the same window as a cache warmed through rotations.
#[tokio::test]
async fn should_return_the_same_window_after_restart() {
    let core = jameson();
    let first = QuorumCache::new(core.config.clone());
    for tip in [13223, 13247, 13271] {
        core.state.lock().unwrap().tip = tip;
        first.refresh().await.unwrap();
    }
    let second = QuorumCache::new(core.config.clone());
    second.refresh().await.unwrap();
    assert_eq!(
        first.get_previous_quorums().await.unwrap(),
        second.get_previous_quorums().await.unwrap()
    );
}

/// Unchanged commitments must not repeatedly fetch their public keys.
#[tokio::test]
async fn should_not_call_quorum_info_again_for_memoized_hashes() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let first = core.info_calls().len();
    cache.refresh().await.unwrap();
    assert_eq!(
        core.info_calls().len(),
        first,
        "known immutable public keys should not cost another info RPC"
    );
}

/// Every key lookup must explicitly exclude secret shares.
#[tokio::test]
async fn should_never_request_secret_key_shares() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let calls = core.info_calls();
    assert!(!calls.is_empty());
    for call in calls {
        assert_eq!(call["params"].as_array().unwrap().len(), 4);
        assert_eq!(
            call["params"][3], false,
            "quorum info must explicitly request public information only"
        );
    }
}

pub async fn endpoint(cache: Arc<QuorumCache>, config: Config, path: &str) -> Value {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::Service;
    let masternodes = Arc::new(crate::masternode_cache::MasternodeCache::new(
        config.clone(),
    ));
    let mut router = crate::api::create_router(cache, config, masternodes);
    let response = router
        .call(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(
        status,
        if path == "/ready" && value["success"] == false {
            503
        } else {
            200
        }
    );
    value
}

/// Height and refresh metadata must describe the cached list while keeping its array shape.
#[tokio::test]
async fn should_report_the_core_height_on_quorums() {
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let first = endpoint(cache.clone(), core.config.clone(), "/quorums").await;
    assert_eq!(first["height"], 13271);
    assert!(first["lastUpdated"].as_i64().unwrap() > 0);
    assert!(first["data"].is_array(), "legacy data must remain an array");
    assert_eq!(
        first,
        endpoint(cache, core.config.clone(), "/quorums").await
    );
}

/// Core failures must preserve data and become visible through health and readiness.
#[tokio::test]
async fn should_keep_last_good_window_and_report_degraded_health_on_core_failure() {
    let mut core = jameson();
    core.config.quorum.cache_ttl_seconds = 1;
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let before = endpoint(cache.clone(), core.config.clone(), "/previous").await;
    core.state.lock().unwrap().fail = true;
    assert!(cache.refresh().await.is_err());
    assert_eq!(
        before,
        endpoint(cache.clone(), core.config.clone(), "/previous").await
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    let ready = endpoint(cache.clone(), core.config.clone(), "/ready").await;
    assert_eq!(ready["success"], false);
    let health = endpoint(cache, core.config.clone(), "/health").await;
    assert_eq!(
        health["success"], false,
        "stale trusted keys must be observable to health monitors"
    );
    assert!(health["message"].as_str().unwrap().contains("degraded"));
}

/// New signing keys must become available without waiting for the cache TTL.
#[tokio::test]
async fn should_reload_when_the_tip_changes_before_the_ttl() {
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    cache.clone().start_background_refresh().await;
    {
        let mut state = core.state.lock().unwrap();
        state.tip += 24;
        let created = state.tip;
        state.quorums.push(Quorum {
            id: 6,
            created,
            retired: u32::MAX,
        });
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if cache
            .get_quorums()
            .await
            .unwrap()
            .contains_quorum(&hex::decode(hash(6)).unwrap())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "tip changes must not wait for the 60-second TTL"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Trimming must preserve active keys and response order must protect newest client cache entries.
#[tokio::test]
async fn should_order_oldest_to_newest_and_trim_retired_first_at_the_cap() {
    let quorums = (0..12)
        .map(|id| Quorum {
            id,
            created: id * 24 + 10,
            retired: (id + 4) * 24 + 10,
        })
        .collect();
    let mut core = MockCore::new(275, quorums);
    core.config.quorum.retention_blocks = 192;
    for (cap, expected) in [(6, vec![6, 7, 8, 9, 10, 11]), (2, vec![8, 9, 10, 11])] {
        core.config.quorum.max_entries = cap;
        let cache = QuorumCache::new(core.config.clone());
        cache.refresh().await.unwrap();
        let (_, previous) = cache.get_previous_quorums().await.unwrap();
        let actual: Vec<String> = previous
            .iter()
            .map(|q| hex::encode(&q.quorum_hash))
            .collect();
        assert_eq!(actual, expected.into_iter().map(hash).collect::<Vec<_>>(),
            "oldest retired keys must go first and active keys must survive even when active count exceeds cap");
    }
}

/// The retention floor and legacy offset must leave no unsampled boundary gap.
#[tokio::test]
async fn should_sample_the_lower_boundary_and_preserve_legacy_offset_coverage() {
    let mut core = jameson();
    core.config.quorum.retention_blocks = 7;
    core.config.quorum.previous_blocks_offset = 101;
    core.config.quorum.sample_step_blocks = Some(47);
    core.state.lock().unwrap().quorums.push(Quorum {
        id: 0,
        created: 13076,
        retired: 13172,
    });
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let (floor, previous) = cache.get_previous_quorums().await.unwrap();
    assert_eq!(floor, 13170);
    assert!(
        previous.contains_quorum(&hex::decode(hash(0)).unwrap()),
        "quorum retired two blocks after the floor must be covered"
    );
    let sampled: Vec<u64> = core
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|r| r["params"][0] == "listextended")
        .map(|r| r["params"][1].as_u64().unwrap())
        .collect();
    assert_eq!(sampled, [13271, 13224, 13177, 13170]);
}

/// A failed historical query must never replace a complete window with a partial one.
#[tokio::test]
async fn should_keep_the_window_when_a_historical_sample_fails() {
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let before = endpoint(cache.clone(), core.config.clone(), "/previous").await;
    core.state.lock().unwrap().fail_height = Some(13223);
    assert!(cache.refresh().await.is_err());
    assert_eq!(
        before,
        endpoint(cache, core.config.clone(), "/previous").await
    );
}

/// New active signing keys must survive a timed-out historical refresh.
#[tokio::test]
async fn should_publish_current_when_the_window_times_out() {
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let previous = cache.get_previous_quorums().await.unwrap();
    let (release, gate) = std::sync::mpsc::channel();
    {
        let mut state = core.state.lock().unwrap();
        state.tip += 1;
        let created = state.tip;
        state.quorums.push(Quorum {
            id: 6,
            created,
            retired: u32::MAX,
        });
        state.historical_response_gate = Some((13224, gate));
        state.calls.clear();
    }
    let refreshing = cache.clone();
    let refresh = tokio::spawn(async move { refreshing.refresh().await });
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !core
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|call| call["params"][0] == "listextended" && call["params"][1] == 13224)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "historical request did not start"
        );
        tokio::task::yield_now().await;
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    let result = refresh.await.unwrap();
    release.send(()).unwrap();
    assert!(result.unwrap_err().to_string().contains("timed out"));
    let current = endpoint(cache.clone(), core.config.clone(), "/quorums").await;
    assert_eq!(current["success"], true);
    assert!(
        current["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["quorum_hash"] == hash(6)),
        "successful active keys must survive the window timeout"
    );
    assert_eq!(cache.get_snapshot().await.unwrap().height, 13272);
    assert_eq!(cache.get_previous_quorums().await.unwrap(), previous);
    assert!(!cache.is_healthy().await);
}

/// Late verification errors must preserve history without discarding new signing keys.
#[tokio::test]
async fn should_publish_current_when_the_final_tip_verification_fails() {
    for failing_call in [2, 3] {
        let core = jameson();
        let cache = Arc::new(QuorumCache::new(core.config.clone()));
        cache.refresh().await.unwrap();
        let previous = cache.get_previous_quorums().await.unwrap();
        {
            let mut state = core.state.lock().unwrap();
            state.tip += 1;
            let created = state.tip;
            state.quorums.push(Quorum {
                id: 6,
                created,
                retired: u32::MAX,
            });
            state.calls.clear();
            state.fail_block_count_at = Some(failing_call);
            if failing_call == 3 {
                state.tip_script = [13272, 13273, 13273].into();
            }
        }
        assert!(cache.refresh().await.is_err());
        let current = endpoint(cache.clone(), core.config.clone(), "/quorums").await;
        assert!(
            current["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["quorum_hash"] == hash(6)),
            "successful active keys must survive a late getblockcount failure"
        );
        assert_eq!(cache.get_snapshot().await.unwrap().height, 13272);
        assert_eq!(cache.get_previous_quorums().await.unwrap(), previous);
        assert!(!cache.is_healthy().await);
        core.state.lock().unwrap().fail_block_count_at = None;
        cache.refresh().await.unwrap();
        assert!(cache.is_healthy().await);
    }
}

/// An active-list failure must not skip the independently recoverable rolling window.
#[tokio::test]
async fn should_attempt_the_window_even_when_the_current_list_fails() {
    let core = jameson();
    core.state.lock().unwrap().fail_current_once = true;
    let cache = QuorumCache::new(core.config.clone());
    assert!(cache.refresh().await.is_err());
    assert!(cache.get_quorums().await.is_err());
    let (_, previous) = cache.get_previous_quorums().await.unwrap();
    assert!(previous.contains_quorum(&hex::decode(hash(1)).unwrap()));
}

/// An unavailable active list cannot produce a window and must not trigger historical RPCs.
#[tokio::test]
async fn should_skip_historical_queries_when_the_active_tip_retry_fails() {
    check_unavailable_active_list_skips_history(false).await;
}

/// An empty tip retry must stop before spending RPCs on an unusable historical window.
#[tokio::test]
async fn should_skip_historical_queries_when_the_active_tip_retry_is_empty() {
    check_unavailable_active_list_skips_history(true).await;
}

async fn check_unavailable_active_list_skips_history(empty: bool) {
    let core = jameson();
    {
        let mut state = core.state.lock().unwrap();
        state.fail_current = !empty;
        state.empty = empty;
    }
    let cache = QuorumCache::new(core.config.clone());
    assert!(cache.refresh().await.is_err());
    assert!(cache.get_quorums().await.is_err());
    assert!(cache.get_previous_quorums().await.is_err());
    let sampled: Vec<u64> = core
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|call| call["params"][0] == "listextended")
        .map(|call| call["params"][1].as_u64().unwrap())
        .collect();
    assert_eq!(
        sampled.iter().filter(|&&height| height != 13271).count(),
        0,
        "history cannot recover a window without a populated active list at the tip"
    );
    assert_eq!(
        sampled,
        [13271, 13271],
        "the active tip must still be retried"
    );
    assert!(core.info_calls().is_empty());
}

/// Malformed or empty upstream data must not become a successful trusted-key response.
#[tokio::test]
async fn should_never_publish_an_empty_or_malformed_success() {
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let before = endpoint(cache.clone(), core.config.clone(), "/quorums").await;
    for malformed in [false, true] {
        {
            let mut state = core.state.lock().unwrap();
            state.empty = !malformed;
            state.malformed = malformed;
        }
        assert!(cache.refresh().await.is_err());
        assert_eq!(
            before,
            endpoint(cache.clone(), core.config.clone(), "/quorums").await
        );
    }
    let fresh = Arc::new(QuorumCache::new(core.config.clone()));
    assert!(fresh.refresh().await.is_err());
    assert_eq!(
        endpoint(fresh, core.config.clone(), "/quorums").await["success"],
        false
    );
}

/// Reorgs must remove orphaned membership without refetching unchanged commitments.
#[tokio::test]
async fn should_drop_orphaned_quorums_after_a_reorg_without_clearing_the_memo() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let initial_info_calls = core.info_calls().len();
    {
        let mut state = core.state.lock().unwrap();
        state.tip -= 1;
        state.quorums.retain(|q| q.id != 5);
        state.quorums.push(Quorum {
            id: 6,
            created: 13262,
            retired: u32::MAX,
        });
    }
    cache.refresh().await.unwrap();
    assert_eq!(
        core.info_calls().len(),
        initial_info_calls + 1,
        "unchanged commitments must remain memoized across a reorg"
    );
    let (_, previous) = cache.get_previous_quorums().await.unwrap();
    assert!(!previous.contains_quorum(&hex::decode(hash(5)).unwrap()));
    assert!(previous.contains_quorum(&hex::decode(hash(6)).unwrap()));
}

/// The TTL must force reloads even when Core height does not advance.
#[tokio::test]
async fn should_reload_unchanged_height_at_the_ttl() {
    let mut core = jameson();
    core.config.quorum.cache_ttl_seconds = 1;
    core.config.quorum.poll_interval_seconds = 60;
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let count_lists = || {
        core.state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|r| r["params"][0] == "listextended")
            .count()
    };
    let initial = count_lists();
    cache.clone().start_background_refresh().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while count_lists() == initial {
        assert!(
            tokio::time::Instant::now() < deadline,
            "TTL must bound refreshes even if configured poll interval is longer"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Unsafe sampling must be rejected when commitment mining offsets shorten active runs.
#[tokio::test]
async fn should_reject_steps_that_miss_quorums_with_variable_mining_offsets() {
    let starts: Vec<u32> = (0..16)
        .map(|cycle| cycle * 24 + if cycle == 0 { 18 } else { 10 })
        .collect();
    let quorums: Vec<Quorum> = starts
        .iter()
        .enumerate()
        .map(|(i, &created)| Quorum {
            id: i as u32,
            created,
            retired: starts.get(i + 4).copied().unwrap_or(u32::MAX),
        })
        .collect();
    assert_eq!(quorums[0].retired - quorums[0].created, 88);
    let mut core = MockCore::new(299, quorums);
    core.config.quorum.retention_blocks = 288;
    core.config.quorum.sample_step_blocks = Some(96);
    // The naive grid 299, 203, 107, 11 skips a quorum active on [18, 106).
    assert!(
        core.config.validate_quorum().is_err(),
        "A*D is unsafe when commitment mining offsets vary"
    );
    core.config.quorum.sample_step_blocks = Some(73);
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    assert!(cache
        .get_previous_quorums()
        .await
        .unwrap()
        .1
        .contains_quorum(&hex::decode(hash(0)).unwrap()));
}

/// A blocked Core RPC must leave the async runtime able to run other tasks.
#[tokio::test(flavor = "current_thread")]
async fn should_keep_the_async_runtime_responsive_during_blocking_core_calls() {
    let core = jameson();
    let (release, gate) = std::sync::mpsc::channel();
    core.state.lock().unwrap().first_response_gate = Some(gate);
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    let refresh = tokio::spawn(async move { cache.refresh().await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while core.state.lock().unwrap().calls.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // This runtime task must be able to release a Core RPC that is still blocked.
    release.send(()).unwrap();
    refresh.await.unwrap().unwrap();
}

/// Pre-activation historical samples must not prevent serving the first active keys.
#[tokio::test]
async fn should_serve_the_window_when_the_floor_precedes_platform_quorum_activation() {
    let core = MockCore::new(
        30,
        vec![Quorum {
            id: 1,
            created: 10,
            retired: u32::MAX,
        }],
    );
    core.state.lock().unwrap().omitted_type_before = Some(10);
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let (floor, window) = cache.get_previous_quorums().await.unwrap();
    assert_eq!(floor, 0);
    assert_eq!(window.len(), 1);
    assert!(window.contains_quorum(&hex::decode(hash(1)).unwrap()));
}

/// A replacement final commitment for the same quorum hash must replace its key.
#[tokio::test]
async fn should_refetch_a_key_when_the_mined_commitment_changes() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let before = core.info_calls().len();
    core.state.lock().unwrap().commitments.insert(5, 1);
    cache.refresh().await.unwrap();
    assert_eq!(core.info_calls().len(), before + 1);
    let list = cache.get_quorums().await.unwrap();
    assert_eq!(
        list.get_entry(&hex::decode(hash(5)).unwrap()).unwrap().key,
        vec![18; 48]
    );
}

/// Core's empty historical result must not erase a quorum that must exist there.
#[tokio::test]
async fn should_reject_a_historical_sample_missing_an_older_active_quorum() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let before = cache.get_previous_quorums().await.unwrap();
    for omit_only in [None, Some(2)] {
        {
            let mut state = core.state.lock().unwrap();
            state.omit_at_height = Some(13223);
            state.omit_only = omit_only;
        }
        assert!(cache.refresh().await.is_err());
        assert_eq!(cache.get_previous_quorums().await.unwrap(), before);
    }
}

/// Newer history must prove completeness after tip-active quorums cease to constrain samples.
#[tokio::test]
async fn should_reject_a_missing_deep_sample_proven_by_newer_history() {
    let mining_heights = [858, 882, 898, 922, 946, 970, 994];
    let quorums = mining_heights
        .iter()
        .enumerate()
        .map(|(id, &created)| Quorum {
            id: id as u32,
            created,
            retired: mining_heights.get(id + 4).copied().unwrap_or(u32::MAX),
        })
        .collect();
    let mut core = MockCore::new(1000, quorums);
    core.config.quorum.retention_blocks = 144;
    core.config.quorum.sample_step_blocks = Some(48);
    core.state.lock().unwrap().creation_heights = (0..7).map(|id| (id, 840 + id * 24)).collect();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    let before = cache.get_previous_quorums().await.unwrap();
    assert!(
        before.1.contains_quorum(&hex::decode(hash(0)).unwrap()),
        "the quorum mined at 858 and retired at 946 appears only at the 904 sample"
    );
    core.state.lock().unwrap().omit_at_height = Some(904);
    let error = cache
        .refresh()
        .await
        .expect_err("a quorum created at 864 and observed at 952 must exist at 904");
    assert!(error.to_string().contains("must still be active"));
    assert_eq!(cache.get_previous_quorums().await.unwrap(), before);
}

/// Readiness gives load balancers a failing status without changing health's HTTP status.
#[tokio::test]
async fn should_return_service_unavailable_when_not_ready() {
    use axum::{body::Body, http::Request};
    use tower::Service;
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    let masternodes = Arc::new(crate::masternode_cache::MasternodeCache::new(
        core.config.clone(),
    ));
    let mut router = crate::api::create_router(cache.clone(), core.config.clone(), masternodes);
    let response = router
        .call(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    cache.refresh().await.unwrap();
    let response = router
        .call(
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

/// A moving chain must not discard an active list tied to an explicit sampled height.
#[tokio::test]
async fn should_publish_current_and_retry_the_window_when_the_tip_moves() {
    let core = jameson();
    {
        let mut state = core.state.lock().unwrap();
        state.tip_script = [13271, 13295, 13295].into();
        state.quorums.push(Quorum {
            id: 6,
            created: 13286,
            retired: u32::MAX,
        });
    }
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
    assert_eq!(cache.get_snapshot().await.unwrap().height, 13271);
    let (floor, previous) = cache.get_previous_quorums().await.unwrap();
    assert_eq!(floor, 13247);
    assert!(previous.contains_quorum(&hex::decode(hash(6)).unwrap()));
    assert!(cache.is_healthy().await);
}

/// Every legal mining-offset pair must leave no hole at the maximum allowed step.
#[tokio::test]
async fn should_cover_all_mining_offset_pairs_at_the_maximum_step() {
    for first in 10..=18 {
        for replacement in 10..=18 {
            let starts: Vec<u32> = (0..20)
                .map(|cycle| {
                    cycle * 24
                        + match cycle {
                            0 => first,
                            4 => replacement,
                            _ => 10,
                        }
                })
                .collect();
            let quorums: Vec<Quorum> = starts
                .iter()
                .enumerate()
                .map(|(i, &created)| Quorum {
                    id: i as u32,
                    created,
                    retired: starts.get(i + 4).copied().unwrap_or(u32::MAX),
                })
                .collect();
            let mut core = MockCore::new(250, quorums.clone());
            core.config.quorum.retention_blocks = 288;
            core.config.quorum.sample_step_blocks = Some(73);
            core.config.validate_quorum().unwrap();
            let cache = QuorumCache::new(core.config.clone());
            for tip in [250u32, 299, 345] {
                core.state.lock().unwrap().tip = tip;
                cache.refresh().await.unwrap();
                let (floor, window) = cache.get_previous_quorums().await.unwrap();
                let expected: Vec<String> = quorums
                    .iter()
                    .filter(|q| q.created <= tip && q.retired > floor)
                    .map(|q| hash(q.id))
                    .collect();
                let actual: Vec<String> =
                    window.iter().map(|q| hex::encode(&q.quorum_hash)).collect();
                assert_eq!(
                    actual, expected,
                    "mining offsets {first}/{replacement}, tip {tip}"
                );
            }
        }
    }
}

/// The legacy SDK JSON structs must accept actual endpoint responses unchanged.
#[tokio::test]
async fn should_deserialize_responses_into_legacy_sdk_types() {
    #[derive(serde::Deserialize)]
    struct QuorumData {
        quorum_hash: String,
        key: String,
        height: u64,
        valid_members_count: u32,
    }
    #[derive(serde::Deserialize)]
    struct QuorumsResponse {
        success: bool,
        data: Vec<QuorumData>,
    }
    #[derive(serde::Deserialize)]
    struct PreviousQuorumsData {
        height: u64,
        quorums: Vec<QuorumData>,
    }
    #[derive(serde::Deserialize)]
    struct PreviousQuorumsResponse {
        success: bool,
        data: PreviousQuorumsData,
    }
    let core = jameson();
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    cache.refresh().await.unwrap();
    let current: QuorumsResponse =
        serde_json::from_value(endpoint(cache.clone(), core.config.clone(), "/quorums").await)
            .unwrap();
    assert!(current.success);
    assert_eq!(current.data.len(), 4);
    assert_eq!(current.data[0].quorum_hash, hash(2));
    assert_eq!(current.data[0].key.len(), 96);
    assert_eq!(current.data[0].height, 13190);
    assert_eq!(current.data[0].valid_members_count, 12);
    let previous: PreviousQuorumsResponse =
        serde_json::from_value(endpoint(cache, core.config.clone(), "/previous").await).unwrap();
    assert!(previous.success);
    assert_eq!(previous.data.height, 13223);
    assert_eq!(previous.data.quorums.len(), 5);
}
