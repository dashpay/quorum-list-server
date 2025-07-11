mod api;
mod config;
mod quorum_list;
mod quorum_loader;
mod masternode;
mod masternode_loader;
mod masternode_cache;
mod grpc_client;

use api::SharedQuorumList;
use config::Config;
use quorum_list::QuorumList;
use masternode_cache::MasternodeCache;
use std::sync::{Arc, RwLock};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting Quorum List Server...");

    // Load configuration
    let config = Config::load_from_env_or_file("config.toml");
    println!("Configuration loaded:");
    println!("  Server: {}:{}", config.server.host, config.server.port);
    println!("  RPC: {} (user: {})", config.rpc.url, config.rpc.username);
    println!("  Network: {}", config.network.network);
    println!("  LLMQ Type: {} (ID: {})", config.get_llmq_type(), config.get_llmq_type_id());
    println!("  DAPI Port: {}", config.get_dapi_port());
    println!("  Previous blocks offset: {}", config.quorum.previous_blocks_offset);
    
    // Load initial quorums from Dash Core
    println!("Loading initial quorums from Dash Core...");
    let initial_quorums = match quorum_loader::load_initial_quorums(&config).await {
        Ok(quorums) => {
            println!("Successfully loaded {} quorums", quorums.len());
            quorums
        }
        Err(e) => {
            println!("Warning: Failed to load initial quorums: {}. Starting with empty list.", e);
            QuorumList::new()
        }
    };
    
    let shared_quorum_list: SharedQuorumList = Arc::new(RwLock::new(initial_quorums));
    
    // Create masternode cache
    let masternode_cache = Arc::new(MasternodeCache::new(config.clone()));
    
    // Populate masternode cache on startup
    println!("Loading initial masternode list...");
    match masternode_cache.get_masternodes().await {
        Ok(masternodes) => {
            println!("Successfully loaded {} masternodes into cache", masternodes.len());
        }
        Err(e) => {
            eprintln!("Warning: Failed to load initial masternodes: {}. Cache will populate on first request.", e);
        }
    }
    
    // Start background refresh for masternode cache
    masternode_cache.clone().start_background_refresh().await;
    
    // Start the API server
    let app = api::create_router(shared_quorum_list.clone(), config.clone(), masternode_cache.clone());
    let listener = TcpListener::bind(format!("{}:{}", config.server.host, config.server.port)).await?;
    
    println!("API Server starting on {}:{}", config.server.host, config.server.port);
    
    let _server_handle = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("Failed to start API server");
    });
    
    // Set up graceful shutdown
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        println!("\nShutdown signal received...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    // Keep the server running until shutdown signal
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    
    println!("Quorum List Server shutting down...");
    Ok(())
}
