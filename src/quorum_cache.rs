use crate::config::Config;
use crate::quorum_list::QuorumList;
use crate::quorum_loader;
use chrono::Local;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct QuorumCache {
    current_quorums: Arc<RwLock<Option<QuorumList>>>,
    previous_quorums: Arc<RwLock<Option<(u32, QuorumList)>>>,
    current_last_update: Arc<Mutex<Option<Instant>>>,
    previous_last_update: Arc<Mutex<Option<Instant>>>,
    config: Arc<Config>,
    update_interval: Duration,
}

impl QuorumCache {
    pub fn new(config: Config) -> Self {
        let ttl = config.quorum.cache_ttl_seconds;
        Self {
            current_quorums: Arc::new(RwLock::new(None)),
            previous_quorums: Arc::new(RwLock::new(None)),
            current_last_update: Arc::new(Mutex::new(None)),
            previous_last_update: Arc::new(Mutex::new(None)),
            config: Arc::new(config),
            update_interval: Duration::from_secs(ttl),
        }
    }

    /// Read the cached current quorums.
    ///
    /// This never contacts Dash Core: the background refresh task is the only
    /// writer. Serving a request must not depend on RPC latency.
    pub async fn get_quorums(
        &self,
    ) -> Result<QuorumList, Box<dyn std::error::Error + Send + Sync>> {
        let data = self
            .current_quorums
            .read()
            .map_err(|_| "Failed to read quorum cache")?;
        Ok(data
            .as_ref()
            .ok_or("Quorum cache is not populated yet")?
            .clone())
    }

    /// Read the cached previous quorums along with the height they were taken at.
    ///
    /// As with [`Self::get_quorums`], this is a pure cache read.
    pub async fn get_previous_quorums(
        &self,
    ) -> Result<(u32, QuorumList), Box<dyn std::error::Error + Send + Sync>> {
        let data = self
            .previous_quorums
            .read()
            .map_err(|_| "Failed to read previous quorum cache")?;
        Ok(data
            .as_ref()
            .ok_or("Previous quorum cache is not populated yet")?
            .clone())
    }

    /// Refresh both quorum caches from Dash Core.
    ///
    /// Called once at startup and then only by the background refresh task.
    pub async fn refresh(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update_current_cache().await?;
        self.update_previous_cache().await?;
        Ok(())
    }

    async fn update_current_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Updating quorum cache...");

        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(30),
            quorum_loader::load_initial_quorums(&self.config),
        )
        .await;

        match result {
            Ok(Ok(quorums)) => {
                {
                    let mut data = self
                        .current_quorums
                        .write()
                        .map_err(|_| "Failed to write to quorum cache")?;
                    *data = Some(quorums);
                }
                {
                    let mut last_update = self.current_last_update.lock().await;
                    *last_update = Some(Instant::now());
                }
                println!("Quorum cache updated successfully");
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                eprintln!("Quorum cache update timed out after 30 seconds");
                Err("Quorum cache update timed out after 30 seconds".into())
            }
        }
    }

    async fn update_previous_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Updating previous quorum cache...");

        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(30),
            self.fetch_previous_quorums(),
        )
        .await;

        match result {
            Ok(Ok((height, quorums))) => {
                {
                    let mut data = self
                        .previous_quorums
                        .write()
                        .map_err(|_| "Failed to write to previous quorum cache")?;
                    *data = Some((height, quorums));
                }
                {
                    let mut last_update = self.previous_last_update.lock().await;
                    *last_update = Some(Instant::now());
                }
                println!("Previous quorum cache updated successfully (height: {})", height);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                eprintln!("Previous quorum cache update timed out after 30 seconds");
                Err("Previous quorum cache update timed out after 30 seconds".into())
            }
        }
    }

    async fn fetch_previous_quorums(
        &self,
    ) -> Result<(u32, QuorumList), Box<dyn std::error::Error + Send + Sync>> {
        let current_height = quorum_loader::get_current_block_height(&self.config).await?;
        let previous_height = if current_height >= self.config.quorum.previous_blocks_offset {
            current_height - self.config.quorum.previous_blocks_offset
        } else {
            0
        };
        let quorums = quorum_loader::load_quorums_at_height(&self.config, previous_height).await?;
        Ok((previous_height, quorums))
    }

    pub async fn start_background_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(self.update_interval).await;
                let now = Local::now();
                println!(
                    "🔄 [{}] Background refresh: Starting quorum cache update...",
                    now.format("%Y-%m-%d %H:%M:%S")
                );

                let current_result = self.update_current_cache().await;
                let previous_result = self.update_previous_cache().await;

                match (current_result, previous_result) {
                    (Ok(_), Ok(_)) => println!(
                        "✅ [{}] Background refresh: Quorum caches updated successfully",
                        Local::now().format("%Y-%m-%d %H:%M:%S")
                    ),
                    (Err(e1), Err(e2)) => eprintln!(
                        "❌ [{}] Background refresh: Failed to update quorum caches: current={}, previous={}",
                        Local::now().format("%Y-%m-%d %H:%M:%S"),
                        e1,
                        e2
                    ),
                    (Err(e), Ok(_)) => eprintln!(
                        "⚠️  [{}] Background refresh: Failed to update current quorum cache: {}",
                        Local::now().format("%Y-%m-%d %H:%M:%S"),
                        e
                    ),
                    (Ok(_), Err(e)) => eprintln!(
                        "⚠️  [{}] Background refresh: Failed to update previous quorum cache: {}",
                        Local::now().format("%Y-%m-%d %H:%M:%S"),
                        e
                    ),
                }
            }
        });
    }
}
