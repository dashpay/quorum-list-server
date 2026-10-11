use crate::config::Config;
use crate::quorum_list::QuorumList;
use crate::quorum_loader::{self, KeyMemo, LoadResult, REFRESH_TIMEOUT};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone)]
pub struct QuorumSnapshot {
    pub quorums: QuorumList,
    pub height: u32,
    pub last_updated: i64,
    refreshed: Instant,
}

#[derive(Default)]
struct CacheState {
    current: Option<QuorumSnapshot>,
    previous: Option<QuorumSnapshot>,
}

pub struct QuorumCache {
    state: RwLock<CacheState>,
    memo: Arc<Mutex<KeyMemo>>,
    rpc_worker: Arc<tokio::sync::Mutex<()>>,
    config: Arc<Config>,
    update_interval: Duration,
    #[cfg(test)]
    polls_completed: std::sync::atomic::AtomicUsize,
}

impl QuorumCache {
    pub fn new(config: Config) -> Self {
        let ttl = config.quorum.cache_ttl_seconds;
        Self {
            state: RwLock::new(CacheState::default()),
            memo: Arc::new(Mutex::new(KeyMemo::default())),
            rpc_worker: Arc::new(tokio::sync::Mutex::new(())),
            config: Arc::new(config),
            update_interval: Duration::from_secs(ttl),
            #[cfg(test)]
            polls_completed: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Reads the list and its metadata together without contacting Core.
    pub async fn get_snapshot(&self) -> LoadResult<QuorumSnapshot> {
        self.state
            .read()
            .map_err(|_| "Failed to read quorum cache")?
            .current
            .clone()
            .ok_or_else(|| "Quorum cache is not populated yet".into())
    }

    pub async fn get_quorums(&self) -> LoadResult<QuorumList> {
        Ok(self.get_snapshot().await?.quorums)
    }

    /// Returns the rolling union and the lowest sampled Core height.
    pub async fn get_previous_quorums(&self) -> LoadResult<(u32, QuorumList)> {
        let state = self
            .state
            .read()
            .map_err(|_| "Failed to read quorum cache")?;
        let previous = state
            .previous
            .as_ref()
            .ok_or("Previous quorum cache is not populated yet")?;
        Ok((previous.height, previous.quorums.clone()))
    }

    /// The permit remains with the blocking worker even if its async waiter times out.
    async fn rpc_work<T, F>(&self, work: F) -> LoadResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> LoadResult<T> + Send + 'static,
    {
        let permit = self.rpc_worker.clone().lock_owned().await;
        tokio::time::timeout(REFRESH_TIMEOUT, async {
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                work()
            })
            .await?
        })
        .await
        .map_err(|_| "Quorum RPC worker timed out")?
    }

    async fn quorum_work<T, F>(&self, work: F) -> LoadResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Config, &mut KeyMemo) -> LoadResult<T> + Send + 'static,
    {
        let config = self.config.clone();
        let memo = self.memo.clone();
        self.rpc_work(move || {
            let mut memo = memo.lock().unwrap_or_else(|poisoned| {
                memo.clear_poison();
                let mut memo = poisoned.into_inner();
                *memo = KeyMemo::default();
                memo
            });
            work(&config, &mut memo)
        })
        .await
    }

    /// Publish active keys before spending the window's separate RPC budget on history.
    pub async fn refresh(&self) -> LoadResult<()> {
        let current = self.quorum_work(quorum_loader::refresh_current).await;
        let current_error = match &current {
            Ok(result) => match &result.current {
                Ok(quorums) => {
                    let mut state = self
                        .state
                        .write()
                        .map_err(|_| "Failed to write quorum cache")?;
                    state.current = Some(QuorumSnapshot {
                        quorums: quorums.clone(),
                        height: result.tip,
                        last_updated: chrono::Utc::now().timestamp(),
                        refreshed: Instant::now(),
                    });
                    None
                }
                Err(error) => Some(error.to_string()),
            },
            Err(error) => Some(error.to_string()),
        };
        let previous = self
            .quorum_work(move |config, memo| quorum_loader::refresh_previous(config, memo, current))
            .await;
        let mut state = self
            .state
            .write()
            .map_err(|_| "Failed to write quorum cache")?;
        let previous_error = match previous {
            Ok((height, quorums)) => {
                state.previous = Some(QuorumSnapshot {
                    quorums,
                    height,
                    last_updated: chrono::Utc::now().timestamp(),
                    refreshed: Instant::now(),
                });
                None
            }
            Err(error) => Some(error),
        };
        match (current_error, previous_error) {
            (None, None) => Ok(()),
            (Some(error), None) => Err(error.into()),
            (None, Some(error)) => Err(error),
            (Some(current), Some(previous)) => {
                Err(format!("Current quorums: {current}; rolling window: {previous}").into())
            }
        }
    }

    pub async fn is_healthy(&self) -> bool {
        let stale_after = self
            .update_interval
            .saturating_mul(self.config.quorum.health_stale_intervals);
        let Ok(state) = self.state.read() else {
            return false;
        };
        match (&state.current, &state.previous) {
            (Some(current), Some(previous)) => {
                current.refreshed.elapsed() < stale_after
                    && previous.refreshed.elapsed() < stale_after
            }
            _ => false,
        }
    }

    fn reload_due(&self, tip: u32) -> LoadResult<bool> {
        let state = self
            .state
            .read()
            .map_err(|_| "Failed to read quorum cache")?;
        Ok(match (&state.current, &state.previous) {
            (Some(current), Some(previous)) => {
                let changed = current.height != tip;
                changed
                    || current.refreshed.elapsed() >= self.update_interval
                    || previous.refreshed.elapsed() >= self.update_interval
                    || previous.height != tip.saturating_sub(self.config.quorum_retention())
            }
            _ => true,
        })
    }

    pub async fn start_background_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            let poll_interval = Duration::from_secs(self.config.quorum.poll_interval_seconds)
                .min(self.update_interval)
                .max(Duration::from_millis(1));
            let mut poll = tokio::time::interval(poll_interval);
            poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut failures = 0u32;
            let mut retry_after = tokio::time::Instant::now();
            let mut last_polled_tip = None;
            loop {
                poll.tick().await;
                let config = self.config.clone();
                match self
                    .rpc_work(move || quorum_loader::get_current_block_height(&config))
                    .await
                {
                    Ok(tip) => {
                        let new_tip = last_polled_tip.replace(tip) != Some(tip);
                        match self.reload_due(tip) {
                            Ok(true) if new_tip || tokio::time::Instant::now() >= retry_after => {
                                match self.refresh().await {
                                    Ok(()) => {
                                        failures = 0;
                                    }
                                    Err(error) => {
                                        eprintln!("Quorum refresh failed: {error}");
                                        failures = failures.saturating_add(1);
                                        let delay = poll_interval
                                            .saturating_mul(1 << failures.saturating_sub(1).min(5))
                                            .min(self.update_interval)
                                            .min(Duration::from_secs(30));
                                        retry_after = tokio::time::Instant::now() + delay;
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(error) => eprintln!("Cannot read quorum refresh state: {error}"),
                        }
                    }
                    Err(error) => eprintln!("Core tip poll failed: {error}"),
                }
                #[cfg(test)]
                self.polls_completed
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quorum_cache_tests::{MockCore, Quorum};

    /// A timed-out window waiter cannot release its worker permit or publish detached history.
    #[tokio::test]
    async fn should_hold_the_rpc_permit_after_the_window_waiter_times_out() {
        let core = MockCore::new(
            100,
            vec![Quorum {
                id: 1,
                created: 10,
                retired: u32::MAX,
            }],
        );
        let (release_first, first_gate) = std::sync::mpsc::channel();
        core.state.lock().unwrap().historical_response_gate = Some((52, first_gate));
        let cache = Arc::new(QuorumCache::new(core.config.clone()));
        let first_cache = cache.clone();
        let first = tokio::spawn(async move { first_cache.refresh().await });
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !core
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|call| call["params"][0] == "listextended" && call["params"][1] == 52)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "first window worker did not start"
            );
            tokio::task::yield_now().await;
        }
        let published_current = cache.get_snapshot().await.unwrap();
        let gated_call_count = core.state.lock().unwrap().calls.len();
        tokio::time::pause();
        tokio::time::advance(REFRESH_TIMEOUT + Duration::from_secs(1)).await;
        let error = first.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(
            cache.rpc_worker.try_lock().is_err(),
            "the detached blocking closure must still own the permit"
        );

        let (release_second, second_gate) = std::sync::mpsc::channel();
        {
            let mut state = core.state.lock().unwrap();
            state.historical_response_gate = Some((100, second_gate));
        }
        let second_cache = cache.clone();
        let (started, starting) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            started.send(()).unwrap();
            second_cache.refresh().await
        });
        starting.await.unwrap();
        assert!(!second.is_finished());
        assert_eq!(core.state.lock().unwrap().calls.len(), gated_call_count);
        assert_eq!(
            cache.get_snapshot().await.unwrap().refreshed,
            published_current.refreshed
        );
        assert!(cache.get_previous_quorums().await.is_err());

        release_first
            .send(())
            .expect("the first window RPC must still be gated after its waiter timed out");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while core
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|call| call["params"][0] == "listextended" && call["params"][1] == 100)
            .count()
            < 2
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the second worker did not start after the first was released"
            );
            tokio::task::yield_now().await;
        }
        // The first closure completed successfully; the second result is still gated.
        assert_eq!(core.info_calls().len(), 1);
        let current = cache.get_snapshot().await.unwrap();
        assert_eq!(current.refreshed, published_current.refreshed);
        assert_eq!(current.quorums, published_current.quorums);
        assert!(
            cache.get_previous_quorums().await.is_err(),
            "the detached first window must never publish a previous snapshot"
        );
        release_second
            .send(())
            .expect("the second active response must still be gated");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !second.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "the second refresh did not finish"
            );
            tokio::task::yield_now().await;
        }
        second.await.unwrap().unwrap();
        assert!(cache.get_snapshot().await.unwrap().refreshed > published_current.refreshed);
        assert_eq!(cache.get_previous_quorums().await.unwrap().0, 52);
    }

    async fn finish_poll(cache: &QuorumCache, expected_polls: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while cache
            .polls_completed
            .load(std::sync::atomic::Ordering::Acquire)
            < expected_polls
        {
            assert!(
                std::time::Instant::now() < deadline,
                "Core poll did not finish"
            );
            // Keep the paused runtime busy until blocking RPC and retry scheduling finish.
            tokio::task::yield_now().await;
        }
    }

    /// Repeated failures at one tip must back off, even without a successful active list.
    #[tokio::test]
    async fn should_back_off_active_list_failures_at_an_unchanged_tip() {
        check_active_list_backoff(false).await;
    }

    /// A failed refresh at a new tip must not keep bypassing backoff using the old snapshot.
    #[tokio::test]
    async fn should_back_off_active_list_failures_after_a_successful_refresh() {
        check_active_list_backoff(true).await;
    }

    async fn check_active_list_backoff(populated: bool) {
        let core = MockCore::new(
            100,
            vec![Quorum {
                id: 1,
                created: 10,
                retired: u32::MAX,
            }],
        );
        let cache = Arc::new(QuorumCache::new(core.config.clone()));
        if populated {
            cache.refresh().await.unwrap();
        }
        {
            let mut state = core.state.lock().unwrap();
            state.tip = 101;
            state.fail_current = true;
            state.calls.clear();
        }
        tokio::time::pause();
        cache.clone().start_background_refresh().await;
        tokio::task::yield_now().await;
        // Wake the first interval tick; later polls retry exactly when now == retry_after.
        tokio::time::advance(Duration::from_millis(1)).await;
        let active_attempts = || {
            core.state
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| call["params"][0] == "listextended" && call["params"][1] == 101)
                .count()
        };
        finish_poll(&cache, 1).await;
        assert_eq!(active_attempts(), 2);
        tokio::time::advance(Duration::from_secs(1)).await;
        finish_poll(&cache, 2).await;
        assert_eq!(active_attempts(), 4);
        tokio::time::advance(Duration::from_secs(1)).await;
        finish_poll(&cache, 3).await;
        assert_eq!(
            active_attempts(),
            4,
            "the second failure must delay the next refresh for two poll intervals"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        finish_poll(&cache, 4).await;
        assert_eq!(active_attempts(), 6);
        core.state.lock().unwrap().tip = 102;
        tokio::time::advance(Duration::from_secs(1)).await;
        finish_poll(&cache, 5).await;
        assert_eq!(
            core.state
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|call| call["params"][0] == "listextended" && call["params"][1] == 102)
                .count(),
            2,
            "a newly polled tip must bypass the four-second retry delay"
        );
    }
}
