use crate::config::Config;
use crate::grpc_client;
use crate::masternode::EvoMasternodeList;
use crate::masternode_loader;
use chrono::{Local, Utc};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The registry data and its successful fetch time are published atomically.
#[derive(Clone)]
pub struct MasternodeSnapshot {
    pub masternodes: EvoMasternodeList,
    /// Unix seconds, captured before loading the registry (not when serving it).
    pub last_updated: i64,
}

pub struct MasternodeCache {
    data: Arc<RwLock<Option<MasternodeSnapshot>>>,
    config: Arc<Config>,
    update_interval: Duration,
}

impl MasternodeCache {
    pub fn new(config: Config) -> Self {
        Self {
            data: Arc::new(RwLock::new(None)),
            config: Arc::new(config),
            update_interval: Duration::from_secs(600), // 10 minutes
        }
    }

    /// Read the cached masternode list.
    ///
    /// This never contacts Dash Core or probes any masternode: the background
    /// refresh task is the only writer.
    pub async fn get_masternodes(
        &self,
    ) -> Result<EvoMasternodeList, Box<dyn std::error::Error + Send + Sync>> {
        Ok(self.get_snapshot().await?.masternodes)
    }

    /// Read registry data and freshness from the same cached snapshot, without RPC.
    pub async fn get_snapshot(
        &self,
    ) -> Result<MasternodeSnapshot, Box<dyn std::error::Error + Send + Sync>> {
        let data = self.data.read().map_err(|_| "Failed to read cache")?;
        Ok(data
            .as_ref()
            .ok_or("Masternode cache is not populated yet")?
            .clone())
    }

    /// Refresh the masternode cache from Dash Core.
    ///
    /// Called once at startup and then only by the background refresh task.
    pub async fn refresh(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update_cache().await
    }

    async fn update_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Updating masternode cache...");

        // Wrap the entire operation in a timeout (30 seconds)
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(30),
            self.update_cache_internal(),
        )
        .await;

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
        // Only publish this time if loading and probing the new list succeed.
        let last_updated = Utc::now().timestamp();

        // Fetch new data
        let mut masternodes = masternode_loader::load_masternode_list(&self.config).await?;

        println!(
            "Checking version for {} Evo masternodes...",
            masternodes.len()
        );

        // Check version for each masternode
        let check_tasks: Vec<_> = masternodes.iter().enumerate().map(|(idx, node)| {
            let address = node.address.clone();
            let status = node.status.clone();
            let platform_http_port = node.platform_http_port;
            let config = self.config.clone();

            async move {
                let start = std::time::Instant::now();

                // Skip POSE_BANNED nodes
                if status == "POSE_BANNED" {
                    println!("⏭️  Node {} at {} - skipping (POSE_BANNED)", idx, address);
                    return (idx, "fail".to_string(), None, None, start.elapsed());
                }

                // Get the host to use for version check (may be replaced by version_check_host)
                let ip = config.get_version_check_host(&address);
                // Use platformHTTPPort from masternode info, fallback to config default
                let port = platform_http_port.unwrap_or_else(|| config.get_dapi_port());

                println!("🔍 Node {} at {} (resolved: {}:{}) - checking version...", idx, address, ip, port);

                // Check version with additional timeout wrapper (2 seconds total)
                let result = match tokio::time::timeout(
                    tokio::time::Duration::from_secs(2),
                    grpc_client::check_node_version(&ip, port, config.network)
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

        let success_count = masternodes
            .iter()
            .filter(|n| n.version_check == "success")
            .count();
        let fail_count = masternodes
            .iter()
            .filter(|n| n.version_check == "fail")
            .count();
        println!(
            "Version check complete: {} success, {} fail (total time: {:?})",
            success_count, fail_count, total_elapsed
        );

        // Report slow nodes
        if !slow_nodes.is_empty() {
            println!(
                "\n🐌 SLOW NODES DETECTED ({} nodes took >2s):",
                slow_nodes.len()
            );
            slow_nodes.sort_by(|a, b| b.2.cmp(&a.2)); // Sort by duration, slowest first
            for (idx, address, duration) in slow_nodes.iter().take(10) {
                println!("   Node {} at {} took {:?}", idx, address, duration);
            }
            println!();
        }

        // Update the cache
        {
            let mut data = self.data.write().map_err(|_| "Failed to write to cache")?;
            *data = Some(MasternodeSnapshot {
                masternodes,
                last_updated,
            });
        }

        println!("Masternode cache updated successfully");
        Ok(())
    }

    pub async fn start_background_refresh(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(self.update_interval).await;
                let now = Local::now();
                println!(
                    "🔄 [{}] Background refresh: Starting masternode cache update...",
                    now.format("%Y-%m-%d %H:%M:%S")
                );
                match self.update_cache().await {
                    Ok(_) => println!(
                        "✅ [{}] Background refresh: Masternode cache updated successfully",
                        Local::now().format("%Y-%m-%d %H:%M:%S")
                    ),
                    Err(e) => eprintln!(
                        "❌ [{}] Background refresh: Failed to update masternode cache: {}",
                        Local::now().format("%Y-%m-%d %H:%M:%S"),
                        e
                    ),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api, masternode::EvoMasternodeInfo, quorum_cache::QuorumCache};
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use serde_json::{json, Value};
    use tower::Service;

    fn snapshot() -> MasternodeSnapshot {
        MasternodeSnapshot {
            last_updated: 1234567890,
            masternodes: vec![EvoMasternodeInfo {
                pro_tx_hash: "11".repeat(32),
                address: "192.0.2.1:9999".into(),
                addresses: Some(std::collections::HashMap::from([
                    ("platform_p2p".into(), vec!["192.0.2.2:27656".into()]),
                    ("platform_https".into(), vec!["192.0.2.3:1443".into()]),
                ])),
                status: "ENABLED".into(),
                platform_node_id: Some("22".repeat(20)),
                platform_p2p_port: Some(27656),
                platform_http_port: Some(1443),
                version_check: "success".into(),
                dapi_version: None,
                drive_version: None,
            }],
        }
    }

    async fn response(config: Config, cache: Arc<MasternodeCache>, uri: &str) -> Value {
        let mut router =
            api::create_router(Arc::new(QuorumCache::new(config.clone())), config, cache);
        let response = router
            .call(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn should_serve_fields_and_original_refresh_time_without_rpc() {
        let mut config = Config::default();
        // Even an accidentally attempted RPC cannot reach the network.
        config.rpc.url = "not a URL".into();
        let cache = Arc::new(MasternodeCache::new(config.clone()));
        *cache.data.write().unwrap() = Some(snapshot());
        let first = response(config.clone(), cache.clone(), "/masternodes").await;
        assert_eq!(first["success"], true);
        assert_eq!(first["lastUpdated"], 1234567890);
        assert!(first["data"].is_array());
        assert_eq!(first["data"][0]["platformNodeID"], "22".repeat(20));
        assert_eq!(first["data"][0]["platformP2PPort"], 27656);
        assert_eq!(
            first["data"][0]["addresses"]["platform_p2p"],
            json!(["192.0.2.2:27656"])
        );
        // A failed refresh must not stamp the old list with a new timestamp.
        assert!(cache.refresh().await.is_err());
        assert_eq!(response(config, cache, "/masternodes").await, first);
    }

    #[tokio::test]
    async fn should_apply_host_overrides_to_separate_endpoints_without_changing_cache() {
        let mut config = Config::default();
        config.docker.address_host_override = Some("127.0.0.1".into());
        let cache = Arc::new(MasternodeCache::new(config.clone()));
        *cache.data.write().unwrap() = Some(snapshot());
        let result = response(config, cache.clone(), "/masternodes").await;
        assert_eq!(result["data"][0]["address"], "127.0.0.1:9999");
        assert_eq!(
            result["data"][0]["addresses"]["platform_p2p"],
            json!(["127.0.0.1:27656"])
        );
        assert_eq!(
            result["data"][0]["addresses"]["platform_https"],
            json!(["127.0.0.1:1443"])
        );
        assert_eq!(
            cache.get_snapshot().await.unwrap().masternodes[0].address,
            "192.0.2.1:9999"
        );
    }

    #[tokio::test]
    async fn should_omit_freshness_when_cache_is_empty_and_on_other_endpoints() {
        let config = Config::default();
        let cache = Arc::new(MasternodeCache::new(config.clone()));
        let result = response(config.clone(), cache.clone(), "/masternodes").await;
        assert_eq!(result["success"], false);
        assert!(result.get("lastUpdated").is_none());
        let health = response(config, cache, "/health").await;
        assert_eq!(health["success"], true);
        assert!(health.get("lastUpdated").is_none());
    }
}
