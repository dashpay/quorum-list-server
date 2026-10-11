use crate::config::Config;
use crate::quorum_list::{QuorumList, QuorumListEntry};
use dashcore_rpc::{jsonrpc, Client, RpcApi};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::time::{Duration, Instant};

pub type LoadResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
// Platform quorum commitments are mined at offsets 10..18 after the DKG base height.
pub const LATEST_MINING_OFFSET: u32 = 18;

/// Only immutable public keys are memoized; membership always comes from Core.
#[derive(Default)]
pub struct KeyMemo {
    keys: HashMap<(String, String), Vec<u8>>,
    order: VecDeque<(String, String)>,
}

impl KeyMemo {
    fn insert(&mut self, hash: (String, String), key: Vec<u8>, capacity: usize) {
        if self.keys.contains_key(&hash) {
            return;
        }
        while self.keys.len() >= capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.keys.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(hash.clone());
        self.keys.insert(hash, key);
    }
}

pub struct CurrentResult {
    pub tip: u32,
    pub current: LoadResult<QuorumList>,
}

fn client(config: &Config) -> LoadResult<Client> {
    let url = if config.rpc.url.contains("://") {
        config.rpc.url.clone()
    } else {
        format!("http://{}", config.rpc.url)
    };
    let transport = jsonrpc::simple_http::SimpleHttpTransport::builder()
        .url(&url)?
        .auth(&config.rpc.username, Some(&config.rpc.password))
        .timeout(Duration::from_secs(5))
        .build();
    Ok(Client::from_jsonrpc(jsonrpc::Client::with_transport(
        transport,
    )))
}

fn block_height(client: &Client) -> LoadResult<u32> {
    let result: u64 = client.call("getblockcount", &[])?;
    Ok(u32::try_from(result)?)
}

pub fn get_current_block_height(config: &Config) -> LoadResult<u32> {
    block_height(&client(config)?)
}

fn load_at_height(
    client: &Client,
    config: &Config,
    height: u32,
    memo: &mut KeyMemo,
    deadline: Instant,
    allow_inactive_type: bool,
) -> LoadResult<QuorumList> {
    check_deadline(deadline)?;
    let result: Value = client.call("quorum", &[json!("listextended"), json!(height)])?;
    let result = result.as_object().ok_or("Invalid extended quorum list")?;
    let quorums = match result.get(config.get_llmq_type()) {
        Some(value) => value.as_array().ok_or("Invalid Platform quorum list")?,
        // Core omits types that were not enabled at a historical height.
        None if allow_inactive_type => return Ok(QuorumList::new()),
        None => return Err("Missing Platform quorum list at the tip".into()),
    };
    let mut list = QuorumList::new();
    for item in quorums {
        let entries = item.as_object().ok_or("Invalid quorum entry")?;
        if entries.is_empty() {
            return Err("Empty quorum entry".into());
        }
        for (hash, details) in entries {
            let hash_bytes = hex::decode(hash)?;
            if hash_bytes.len() != 32 {
                return Err("Invalid quorum hash length".into());
            }
            let mined_hash = details
                .get("minedBlockHash")
                .and_then(Value::as_str)
                .ok_or("Missing quorum mined block hash")?;
            if hex::decode(mined_hash)?.len() != 32 {
                return Err("Invalid mined block hash length".into());
            }
            let memo_key = (hash.clone(), mined_hash.to_string());
            let creation_height = details
                .get("creationHeight")
                .and_then(Value::as_u64)
                .ok_or("Missing quorum creation height")?;
            let valid_members = details
                .get("numValidMembers")
                .and_then(Value::as_u64)
                .ok_or("Missing quorum valid member count")?;
            let creation_height = u32::try_from(creation_height)?;
            let valid_members = u32::try_from(valid_members)?;
            let public_key = match memo.keys.get(&memo_key) {
                Some(key) => key.clone(),
                None => {
                    check_deadline(deadline)?;
                    let info: Value = client.call(
                        "quorum",
                        &[
                            json!("info"),
                            json!(config.get_llmq_type_id()),
                            json!(hash),
                            json!(false),
                        ],
                    )?;
                    let key = hex::decode(
                        info.get("quorumPublicKey")
                            .and_then(Value::as_str)
                            .ok_or("Missing quorum public key")?,
                    )?;
                    if key.len() != 48 {
                        return Err("Invalid quorum public key length".into());
                    }
                    let info_mined_hash = info
                        .get("minedBlock")
                        .and_then(Value::as_str)
                        .ok_or("Missing quorum minedBlock")?;
                    if info_mined_hash != mined_hash {
                        return Err("Quorum commitment changed during key lookup".into());
                    }
                    memo.insert(memo_key, key.clone(), config.quorum.key_memo_max_entries);
                    key
                }
            };
            list.add_entry(QuorumListEntry::new_extended(
                hash_bytes,
                public_key,
                creation_height,
                Vec::new(),
                String::new(),
                0,
                valid_members,
            ));
        }
    }
    sort_oldest_first(&mut list);
    Ok(list)
}

fn check_deadline(deadline: Instant) -> LoadResult<()> {
    if Instant::now() >= deadline {
        return Err("Quorum refresh deadline exceeded".into());
    }
    Ok(())
}

fn sort_oldest_first(list: &mut QuorumList) {
    list.list.sort_by(|a, b| {
        a.height
            .cmp(&b.height)
            .then_with(|| a.quorum_hash.cmp(&b.quorum_hash))
    });
}

fn load_window(
    client: &Client,
    config: &Config,
    tip: u32,
    current: &LoadResult<QuorumList>,
    memo: &mut KeyMemo,
    deadline: Instant,
) -> LoadResult<(u32, QuorumList)> {
    let step = config.quorum_sample_step();
    let floor = tip.saturating_sub(config.quorum_retention());
    // Retry the tip for the window even if the independent active-list fetch failed.
    let active = match current {
        Ok(list) => list.clone(),
        Err(_) => load_at_height(client, config, tip, memo, deadline, false)?,
    };
    if active.is_empty() {
        return Err("Core returned no active Platform quorums".into());
    }
    let mut window = QuorumList::new();
    let mut height = tip;
    let mut history_error = None;
    while height > floor {
        // Include the lower boundary when the regular grid would overshoot it.
        height = height.saturating_sub(step).max(floor);
        match load_at_height(client, config, height, memo, deadline, true) {
            Ok(list) => {
                // A quorum observed later was already active after its latest mining height.
                for entry in active.iter().chain(window.iter()) {
                    if u64::from(entry.height) + u64::from(LATEST_MINING_OFFSET)
                        <= u64::from(height)
                        && !list.contains_quorum(&entry.quorum_hash)
                    {
                        return Err(
                            "Historical sample is missing a quorum that must still be active"
                                .into(),
                        );
                    }
                }
                for entry in list.list {
                    window.add_entry(entry);
                }
            }
            Err(error) => {
                history_error = Some(error);
                break;
            }
        }
    }
    if let Some(error) = history_error {
        return Err(error);
    }
    let active_hashes: HashSet<Vec<u8>> = active.iter().map(|q| q.quorum_hash.clone()).collect();
    for entry in active.list {
        window.add_entry(entry);
    }
    sort_oldest_first(&mut window);
    let mut excess = window.len().saturating_sub(config.quorum.max_entries);
    window.list.retain(|entry| {
        if excess > 0 && !active_hashes.contains(&entry.quorum_hash) {
            excess -= 1;
            false
        } else {
            true
        }
    });
    Ok((floor, window))
}

/// Blocking RPC work; callers must run this on a blocking worker.
pub fn refresh_current(config: &Config, memo: &mut KeyMemo) -> LoadResult<CurrentResult> {
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    let client = client(config)?;
    let tip = block_height(&client)?;
    let current = load_at_height(&client, config, tip, memo, deadline, false).and_then(|list| {
        if list.is_empty() {
            Err("Core returned no active Platform quorums".into())
        } else {
            Ok(list)
        }
    });
    Ok(CurrentResult { tip, current })
}

/// Window failures, including tip verification, are independent of active publication.
pub fn refresh_previous(
    config: &Config,
    memo: &mut KeyMemo,
    current: LoadResult<CurrentResult>,
) -> LoadResult<(u32, QuorumList)> {
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    let client = client(config)?;
    let CurrentResult { tip, current } = match current {
        Ok(result) => result,
        Err(error) => CurrentResult {
            tip: block_height(&client)?,
            current: Err(error),
        },
    };
    let mut previous = load_window(&client, config, tip, &current, memo, deadline);
    let next_tip = block_height(&client)?;
    if next_tip != tip {
        let next_active = load_at_height(&client, config, next_tip, memo, deadline, false);
        previous = load_window(&client, config, next_tip, &next_active, memo, deadline);
        if block_height(&client)? != next_tip {
            previous = Err("Core tip changed again while retrying the rolling window".into());
        }
    }
    previous
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorum_cache_tests::{hash, MockCore, Quorum};

    /// Missing commitment identity must be reported separately from a detected fork switch.
    #[test]
    fn should_report_a_missing_info_mined_block_without_memoizing() {
        let core = MockCore::new(
            100,
            vec![Quorum {
                id: 1,
                created: 10,
                retired: u32::MAX,
            }],
        );
        core.state.lock().unwrap().omit_info_mined_block = true;
        let mut memo = KeyMemo::default();
        let error = refresh_current(&core.config, &mut memo)
            .unwrap()
            .current
            .unwrap_err();
        assert_eq!(error.to_string(), "Missing quorum minedBlock");
        assert!(memo.keys.is_empty());
        assert!(memo.order.is_empty());
    }

    /// A fork switch during key lookup must not memoize the replacement under the old commitment.
    #[test]
    fn should_reject_a_commitment_change_between_list_and_info_without_memoizing() {
        let core = MockCore::new(
            100,
            vec![Quorum {
                id: 1,
                created: 10,
                retired: u32::MAX,
            }],
        );
        core.state.lock().unwrap().commitment_change_on_info = Some((1, 1));
        let mut memo = KeyMemo::default();
        let result = refresh_current(&core.config, &mut memo).unwrap();
        let error = result.current.expect_err(
            "the info key belongs to a different mined commitment than the listed quorum",
        );
        assert!(error.to_string().contains("commitment changed"));
        assert!(
            memo.keys.is_empty(),
            "the mismatched commitment must not populate the memo"
        );
        assert!(memo.order.is_empty());
        core.state.lock().unwrap().commitments.clear();
        let restored = refresh_current(&core.config, &mut memo)
            .unwrap()
            .current
            .unwrap();
        assert_eq!(
            restored
                .get_entry(&hex::decode(hash(1)).unwrap())
                .unwrap()
                .key,
            vec![17; 48],
            "returning to the original fork must fetch its original key"
        );
        assert_eq!(core.info_calls().len(), 2);
        assert_eq!(memo.keys.len(), 1);
    }

    /// Key caching must remain bounded even when historical commitments accumulate.
    #[test]
    fn key_memo_is_bounded_and_evicts_oldest() {
        let mut memo = KeyMemo::default();
        for id in 0..3 {
            memo.insert((id.to_string(), "mined".into()), vec![id], 2);
        }
        assert_eq!(memo.keys.len(), 2);
        assert!(!memo.keys.contains_key(&("0".into(), "mined".into())));
        assert!(memo.keys.contains_key(&("2".into(), "mined".into())));
        assert_eq!(memo.order.len(), 2);
    }
}
