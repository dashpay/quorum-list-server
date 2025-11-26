use crate::config::Config;
use crate::masternode::EvoMasternodeList;
use crate::masternode_loader;
use crate::grpc_client;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use chrono::Local;

pub struct MasternodeCache {
    data: Arc<RwLock<Option<EvoMasternodeList>>>,
    last_update: Arc<Mutex<Option<Instant>>>,
    config: Arc<Config>,
    update_interval: Duration,
}

impl MasternodeCache {
    pub fn new(config: Config) -> Self {
        Self {
            data: Arc::new(RwLock::new(None)),
            last_update: Arc::new(Mutex::new(None)),
            config: Arc::new(config),
            update_interval: Duration::from_secs(600), // 10 minutes
        }
    }

    pub async fn get_masternodes(&self) -> Result<EvoMasternodeList, Box<dyn std::error::Error + Send + Sync>> {
        // Check if we need to update the cache
        let should_update = {
            let last_update_guard = self.last_update.lock().await;
            match *last_update_guard {
                None => true,
                Some(last) => last.elapsed() >= self.update_interval,
            }
        };

        if should_update {
            self.update_cache().await?;
        }

        // Return the cached data
        let cached_data = {
            let data = self.data.read()
                .map_err(|_| "Failed to read cache")?;
            data.clone()
        };
        
        match cached_data {
            Some(masternodes) => Ok(masternodes),
            None => {
                // This shouldn't happen as we just updated, but handle it gracefully
                self.update_cache().await?;
                let data = self.data.read()
                    .map_err(|_| "Failed to read cache")?;
                Ok(data.as_ref().ok_or("No masternode data available")?.clone())
            }
        }
    }

    async fn update_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Updating masternode cache...");

        // Wrap the entire operation in a timeout (30 seconds)
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(30),
            self.update_cache_internal()
        ).await;

        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                eprintln!("⚠️  CACHE UPDATE TIMED OUT after 30 seconds - this indicates network issues or too many slow nodes");
                Err("Cache update timed out after 30 seconds".into())
            }
        }
    }

    async fn update_cache_internal(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Fetch new data
        let mut masternodes = masternode_loader::load_masternode_list(&self.config).await?;

        println!("Checking version for {} Evo masternodes...", masternodes.len());
        
        // Check version for each masternode
        let check_tasks: Vec<_> = masternodes.iter().enumerate().map(|(idx, node)| {
            let address = node.address.clone();
            let status = node.status.clone();
            let config = self.config.clone();

            async move {
                let start = std::time::Instant::now();

                // Skip POSE_BANNED nodes
                if status == "POSE_BANNED" {
                    println!("⏭️  Node {} at {} - skipping (POSE_BANNED)", idx, address);
                    return (idx, "fail".to_string(), None, None, start.elapsed());
                }

                // Parse address to get IP and port
                let parts: Vec<&str> = address.split(':').collect();
                if parts.len() != 2 {
                    println!("❌ Node {} at {} - invalid address format", idx, address);
                    return (idx, "fail".to_string(), None, None, start.elapsed());
                }

                let ip = parts[0];
                let port = config.get_dapi_port();

                println!("🔍 Node {} at {} - checking version...", idx, address);

                // Check version with additional timeout wrapper (2 seconds total)
                let result = match tokio::time::timeout(
                    tokio::time::Duration::from_secs(2),
                    grpc_client::check_node_version(ip, port)
                ).await {
                    Ok(Ok(result)) => {
                        let elapsed = start.elapsed();
                        if result.success {
                            println!("✓ Node {} at {} - version 2.0+ (DAPI: {:?}, Drive: {:?}) [took {:?}]",
                                idx, address, result.dapi_version, result.drive_version, elapsed);
                            (idx, "success".to_string(), result.dapi_version, result.drive_version, elapsed)
                        } else {
                            println!("✗ Node {} at {} - version < 2.0 (DAPI: {:?}, Drive: {:?}) [took {:?}]",
                                idx, address, result.dapi_version, result.drive_version, elapsed);
                            (idx, "fail".to_string(), result.dapi_version, result.drive_version, elapsed)
                        }
                    },
                    Ok(Err(e)) => {
                        let elapsed = start.elapsed();
                        println!("✗ Node {} at {} - error: {} [took {:?}]", idx, address, e, elapsed);
                        (idx, "fail".to_string(), None, None, elapsed)
                    },
                    Err(_) => {
                        let elapsed = start.elapsed();
                        println!("⏱️  Node {} at {} - TIMEOUT after {:?} ⚠️  THIS NODE IS SLOW!", idx, address, elapsed);
                        (idx, "fail".to_string(), None, None, elapsed)
                    },
                };
                result
            }
        }).collect();
        
        // Execute all version checks concurrently
        let overall_start = std::time::Instant::now();
        let results = futures::future::join_all(check_tasks).await;
        let total_elapsed = overall_start.elapsed();

        // Track slow nodes
        let mut slow_nodes: Vec<(usize, String, std::time::Duration)> = vec![];

        // Update the version_check field and version info for each masternode
        for (idx, version_check, dapi_version, drive_version, elapsed) in results {
            masternodes[idx].version_check = version_check;
            masternodes[idx].dapi_version = dapi_version;
            masternodes[idx].drive_version = drive_version;

            // Track nodes that took more than 2 seconds
            if elapsed.as_secs() >= 2 {
                slow_nodes.push((idx, masternodes[idx].address.clone(), elapsed));
            }
        }

        let success_count = masternodes.iter().filter(|n| n.version_check == "success").count();
        let fail_count = masternodes.iter().filter(|n| n.version_check == "fail").count();
        println!("Version check complete: {} success, {} fail (total time: {:?})", success_count, fail_count, total_elapsed);

        // Report slow nodes
        if !slow_nodes.is_empty() {
            println!("\n🐌 SLOW NODES DETECTED ({} nodes took >2s):", slow_nodes.len());
            slow_nodes.sort_by(|a, b| b.2.cmp(&a.2)); // Sort by duration, slowest first
            for (idx, address, duration) in slow_nodes.iter().take(10) {
                println!("   Node {} at {} took {:?}", idx, address, duration);
            }
            println!();
        }
        
        // Update the cache
        {
            let mut data = self.data.write()
                .map_err(|_| "Failed to write to cache")?;
            *data = Some(masternodes);
        }
        
        // Update the timestamp
        {
            let mut last_update = self.last_update.lock().await;
            *last_update = Some(Instant::now());
        }
        
        println!("Masternode cache updated successfully");
        Ok(())
    }

    pub async fn start_background_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(self.update_interval).await;
                let now = Local::now();
                println!("🔄 [{}] Background refresh: Starting masternode cache update...", now.format("%Y-%m-%d %H:%M:%S"));
                match self.update_cache().await {
                    Ok(_) => println!("✅ [{}] Background refresh: Masternode cache updated successfully", Local::now().format("%Y-%m-%d %H:%M:%S")),
                    Err(e) => eprintln!("❌ [{}] Background refresh: Failed to update masternode cache: {}", Local::now().format("%Y-%m-%d %H:%M:%S"), e),
                }
            }
        });
    }
}