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
        
        // Fetch new data
        let mut masternodes = masternode_loader::load_masternode_list(&self.config).await?;
        
        println!("Checking version for {} Evo masternodes...", masternodes.len());
        
        // Check version for each masternode
        let check_tasks: Vec<_> = masternodes.iter().enumerate().map(|(idx, node)| {
            let address = node.address.clone();
            let status = node.status.clone();
            
            async move {
                // Skip POSE_BANNED nodes
                if status == "POSE_BANNED" {
                    println!("⏭️  Node {} at {} - skipping (POSE_BANNED)", idx, address);
                    return (idx, "fail".to_string(), None, None);
                }
                
                // Parse address to get IP and port
                let parts: Vec<&str> = address.split(':').collect();
                if parts.len() != 2 {
                    println!("❌ Node {} at {} - invalid address format", idx, address);
                    return (idx, "fail".to_string(), None, None);
                }
                
                let ip = parts[0];
                let port = self.config.get_dapi_port();
                
                println!("🔍 Node {} at {} - checking version...", idx, address);
                
                // Check version with additional timeout wrapper (3 seconds total)
                match tokio::time::timeout(
                    tokio::time::Duration::from_secs(3),
                    grpc_client::check_node_version(ip, port)
                ).await {
                    Ok(Ok(result)) => {
                        if result.success {
                            println!("✓ Node {} at {} - version 2.0+ (DAPI: {:?}, Drive: {:?})", 
                                idx, address, result.dapi_version, result.drive_version);
                            (idx, "success".to_string(), result.dapi_version, result.drive_version)
                        } else {
                            println!("✗ Node {} at {} - version < 2.0 (DAPI: {:?}, Drive: {:?})", 
                                idx, address, result.dapi_version, result.drive_version);
                            (idx, "fail".to_string(), result.dapi_version, result.drive_version)
                        }
                    },
                    Ok(Err(e)) => {
                        println!("✗ Node {} at {} - error: {}", idx, address, e);
                        (idx, "fail".to_string(), None, None)
                    },
                    Err(_) => {
                        println!("✗ Node {} at {} - timeout after 3 seconds", idx, address);
                        (idx, "fail".to_string(), None, None)
                    },
                }
            }
        }).collect();
        
        // Execute all version checks concurrently
        let results = futures::future::join_all(check_tasks).await;
        
        // Update the version_check field and version info for each masternode
        for (idx, version_check, dapi_version, drive_version) in results {
            masternodes[idx].version_check = version_check;
            masternodes[idx].dapi_version = dapi_version;
            masternodes[idx].drive_version = drive_version;
        }
        
        let success_count = masternodes.iter().filter(|n| n.version_check == "success").count();
        let fail_count = masternodes.iter().filter(|n| n.version_check == "fail").count();
        println!("Version check complete: {} success, {} fail", success_count, fail_count);
        
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