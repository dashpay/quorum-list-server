use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasternodeInfo {
    #[serde(rename = "proTxHash")]
    pub pro_tx_hash: String,
    pub address: String,
    pub payee: String,
    pub status: String,
    #[serde(rename = "type")]
    pub node_type: String,
    #[serde(rename = "platformNodeID", skip_serializing_if = "Option::is_none")]
    pub platform_node_id: Option<String>,
    #[serde(rename = "platformP2PPort", skip_serializing_if = "Option::is_none")]
    pub platform_p2p_port: Option<u16>,
    #[serde(rename = "platformHTTPPort", skip_serializing_if = "Option::is_none")]
    pub platform_http_port: Option<u16>,
    #[serde(rename = "pospenaltyscore")]
    pub pos_penalty_score: u32,
    #[serde(rename = "consecutivePayments")]
    pub consecutive_payments: u32,
    #[serde(rename = "lastpaidtime")]
    pub last_paid_time: u64,
    #[serde(rename = "lastpaidblock")]
    pub last_paid_block: u32,
    #[serde(rename = "owneraddress")]
    pub owner_address: String,
    #[serde(rename = "votingaddress")]
    pub voting_address: String,
    #[serde(rename = "collateraladdress")]
    pub collateral_address: String,
    #[serde(rename = "pubkeyoperator")]
    pub pubkey_operator: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvoMasternodeInfo {
    #[serde(rename = "proTxHash")]
    pub pro_tx_hash: String,
    pub address: String,
    pub status: String,
    #[serde(rename = "platformHTTPPort", skip_serializing_if = "Option::is_none")]
    pub platform_http_port: Option<u16>,
    #[serde(rename = "versionCheck")]
    pub version_check: String, // "success", "fail", or "pending"
    #[serde(rename = "dapiVersion", skip_serializing_if = "Option::is_none")]
    pub dapi_version: Option<String>,
    #[serde(rename = "driveVersion", skip_serializing_if = "Option::is_none")]
    pub drive_version: Option<String>,
}

impl From<MasternodeInfo> for Option<EvoMasternodeInfo> {
    fn from(info: MasternodeInfo) -> Self {
        if info.node_type == "Evo" {
            // Set initial version_check based on status
            let version_check = if info.status == "POSE_BANNED" {
                "fail".to_string()
            } else {
                "pending".to_string()
            };

            Some(EvoMasternodeInfo {
                pro_tx_hash: info.pro_tx_hash,
                address: info.address,
                status: info.status,
                platform_http_port: info.platform_http_port,
                version_check,
                dapi_version: None,
                drive_version: None,
            })
        } else {
            None
        }
    }
}

pub type MasternodeList = HashMap<String, MasternodeInfo>;
pub type EvoMasternodeList = Vec<EvoMasternodeInfo>;