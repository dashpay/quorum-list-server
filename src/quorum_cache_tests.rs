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
                            .filter(|q| !state.empty && q.created <= height && height < q.retired)
                            .map(|q| json!({hash(q.id): {"creationHeight":q.created, "numValidMembers":12}}))
                            .collect();
                        json!({"llmq_devnet_platform": entries})
                    }
                } else {
                    json!({"quorumPublicKey": "11".repeat(48)})
                };
                let body = if failed {
                    json!({"result":null, "error":{"code":-1,"message":"Core unavailable"}, "id":request["id"]})
                } else {
                    json!({"result":result, "error":null, "id":request["id"]})
                }.to_string();
                let delay = state.delay;
                drop(state);
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

#[tokio::test]
async fn should_return_the_same_window_after_restart() {
    let core = jameson();
    let first = QuorumCache::new(core.config.clone());
    first.refresh().await.unwrap();
    let second = QuorumCache::new(core.config.clone());
    second.refresh().await.unwrap();
    assert_eq!(
        first.get_previous_quorums().await.unwrap(),
        second.get_previous_quorums().await.unwrap()
    );
}

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
    assert_eq!(response.status(), 200);
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

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
    tokio::time::sleep(Duration::from_millis(3100)).await;
    let health = endpoint(cache, core.config.clone(), "/health").await;
    assert_eq!(
        health["success"], false,
        "stale trusted keys must be observable to health monitors"
    );
    assert!(health["message"].as_str().unwrap().contains("degraded"));
}

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

#[tokio::test]
async fn should_drop_orphaned_quorums_after_a_reorg_without_clearing_the_memo() {
    let core = jameson();
    let cache = QuorumCache::new(core.config.clone());
    cache.refresh().await.unwrap();
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
    let (_, previous) = cache.get_previous_quorums().await.unwrap();
    assert!(!previous.contains_quorum(&hex::decode(hash(5)).unwrap()));
    assert!(previous.contains_quorum(&hex::decode(hash(6)).unwrap()));
}

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
        core.config.quorum_sample_step().is_err(),
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

#[tokio::test(flavor = "current_thread")]
async fn should_keep_the_async_runtime_responsive_during_blocking_core_calls() {
    let core = jameson();
    core.state.lock().unwrap().delay = Duration::from_millis(30);
    let cache = Arc::new(QuorumCache::new(core.config.clone()));
    let start = std::time::Instant::now();
    let refresh = tokio::spawn(async move { cache.refresh().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let elapsed = start.elapsed();
    refresh.await.unwrap().unwrap();
    assert!(
        elapsed < Duration::from_millis(150),
        "async timers stalled for {elapsed:?} during blocking RPC"
    );
}

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
