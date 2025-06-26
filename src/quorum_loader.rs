use crate::config::Config;
use crate::quorum_list::{QuorumList, QuorumListEntry, QuorumMember};
use dashcore_rpc::{Client, RpcApi, Auth};
use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Deserialize)]
pub struct QuorumListResult {
    pub quorums: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct QuorumInfo {
    pub quorum_hash: String,
    pub quorum_public_key: String,
}

pub async fn load_initial_quorums(
    config: &Config,
) -> Result<QuorumList, Box<dyn Error + Send + Sync>> {
    // Create RPC client
    let auth = Auth::UserPass(config.rpc.username.clone(), config.rpc.password.clone());
    let client = Client::new(&config.rpc.url, auth)?;
    
    // Get the extended quorum list (returns detailed info for all quorums)
    let result: serde_json::Value = client.call("quorum", &[serde_json::json!("listextended")])?;
    
    let mut quorum_list = QuorumList::new();
    
    // Parse the extended quorum list - extract llmq_25_67 quorums for testnet
    if let Some(quorum_obj) = result.as_object() {
        if let Some(llmq_25_67) = quorum_obj.get("llmq_25_67") {
            if let Some(quorums_arr) = llmq_25_67.as_array() {
                for quorum_item in quorums_arr {
                    if let Some(quorum_obj) = quorum_item.as_object() {
                        for (quorum_hash_str, quorum_info) in quorum_obj {
                            if let Some(info_obj) = quorum_info.as_object() {
                                let quorum_hash_bytes = hex::decode(quorum_hash_str)?;
                                if quorum_hash_bytes.len() == 32 {
                                    // Get the actual quorum public key via quorum info
                                    let rpc_params = [
                                        serde_json::json!("info"), 
                                        serde_json::json!(6), // llmq_25_67 = type 6
                                        serde_json::json!(quorum_hash_str)
                                    ];
                                    println!("DEBUG: Calling quorum with params: {:?}", rpc_params);
                                    let info_result: serde_json::Value = client.call("quorum", &rpc_params)?;
                                    
                                    if let Some(detailed_info) = info_result.as_object() {
                                        if let Some(pubkey_str) = detailed_info.get("quorumPublicKey").and_then(|v| v.as_str()) {
                                            let public_key = hex::decode(pubkey_str)?;
                                            
                                            if public_key.len() == 48 {
                                                let creation_height = info_obj.get("creationHeight")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0) as u32;
                                                let valid_members_count = info_obj.get("numValidMembers")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0) as u32;
                                                
                                                let entry = QuorumListEntry::new_extended(
                                                    quorum_hash_bytes,
                                                    public_key,
                                                    creation_height,
                                                    Vec::new(), // No members data needed
                                                    String::new(), // No threshold signature needed
                                                    0, // No mining members count needed
                                                    valid_members_count,
                                                );
                                                quorum_list.add_entry(entry);
                                                println!("Loaded quorum: {} at height {} with {} members", 
                                                    quorum_hash_str, creation_height, valid_members_count);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    println!("Loaded {} quorums from Dash Core", quorum_list.len());
    Ok(quorum_list)
}

pub async fn get_current_block_height(
    config: &Config,
) -> Result<u32, Box<dyn Error + Send + Sync>> {
    let auth = Auth::UserPass(config.rpc.username.clone(), config.rpc.password.clone());
    let client = Client::new(&config.rpc.url, auth)?;
    
    let result: serde_json::Value = client.call("getblockcount", &[])?;
    let height = result.as_u64().ok_or("Invalid block count response")? as u32;
    
    Ok(height)
}

pub async fn load_quorums_at_height(
    config: &Config,
    height: u32,
) -> Result<QuorumList, Box<dyn Error + Send + Sync>> {
    let auth = Auth::UserPass(config.rpc.username.clone(), config.rpc.password.clone());
    let client = Client::new(&config.rpc.url, auth)?;
    
    // Get the extended quorum list at specific height
    let result: serde_json::Value = client.call("quorum", &[
        serde_json::json!("listextended"),
        serde_json::json!(height)
    ])?;
    
    let mut quorum_list = QuorumList::new();
    
    // Parse the extended quorum list - extract llmq_25_67 quorums for testnet
    if let Some(quorum_obj) = result.as_object() {
        if let Some(llmq_25_67) = quorum_obj.get("llmq_25_67") {
            if let Some(quorums_arr) = llmq_25_67.as_array() {
                for quorum_item in quorums_arr {
                    if let Some(quorum_obj) = quorum_item.as_object() {
                        for (quorum_hash_str, quorum_info) in quorum_obj {
                            if let Some(info_obj) = quorum_info.as_object() {
                                let quorum_hash_bytes = hex::decode(quorum_hash_str)?;
                                if quorum_hash_bytes.len() == 32 {
                                    // Get the actual quorum public key via quorum info
                                    let rpc_params = [
                                        serde_json::json!("info"), 
                                        serde_json::json!(6), // llmq_25_67 = type 6
                                        serde_json::json!(quorum_hash_str),
                                        serde_json::json!(true) // includeSkShare
                                    ];
                                    println!("DEBUG: Calling quorum with params: {:?}", rpc_params);
                                    let info_result: serde_json::Value = client.call("quorum", &rpc_params)?;
                                    
                                    if let Some(detailed_info) = info_result.as_object() {
                                        if let Some(pubkey_str) = detailed_info.get("quorumPublicKey").and_then(|v| v.as_str()) {
                                            let public_key = hex::decode(pubkey_str)?;
                                            
                                            if public_key.len() == 48 {
                                                let creation_height = info_obj.get("creationHeight")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0) as u32;
                                                let valid_members_count = info_obj.get("numValidMembers")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0) as u32;
                                                
                                                let entry = QuorumListEntry::new_extended(
                                                    quorum_hash_bytes,
                                                    public_key,
                                                    creation_height,
                                                    Vec::new(), // No members data needed
                                                    String::new(), // No threshold signature needed
                                                    0, // No mining members count needed
                                                    valid_members_count,
                                                );
                                                quorum_list.add_entry(entry);
                                                println!("Loaded quorum at height {}: {} with {} members", 
                                                    height, quorum_hash_str, valid_members_count);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    println!("Loaded {} quorums from Dash Core at height {}", quorum_list.len(), height);
    Ok(quorum_list)
}

