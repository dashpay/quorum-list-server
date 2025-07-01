use crate::config::Config;
use crate::masternode::{MasternodeList, EvoMasternodeList};
use dashcore_rpc::{Client, RpcApi, Auth};
use std::error::Error;

pub async fn load_masternode_list(
    config: &Config,
) -> Result<EvoMasternodeList, Box<dyn Error + Send + Sync>> {
    let auth = Auth::UserPass(config.rpc.username.clone(), config.rpc.password.clone());
    let client = Client::new(&config.rpc.url, auth)?;
    
    // Call masternode list command
    let result: serde_json::Value = client.call("masternode", &[serde_json::json!("list")])?;
    
    // Parse the result as a HashMap of masternodes
    let masternode_list: MasternodeList = serde_json::from_value(result)?;
    
    // Filter and convert to Evo masternodes only
    let evo_masternodes: EvoMasternodeList = masternode_list
        .into_iter()
        .filter_map(|(_, info)| info.into())
        .collect();
    
    println!("Loaded {} Evo masternodes from Dash Core", evo_masternodes.len());
    Ok(evo_masternodes)
}