use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasternodeInfo {
    #[serde(rename = "proTxHash")]
    pub pro_tx_hash: String,
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<HashMap<String, Vec<String>>>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addresses: Option<HashMap<String, Vec<String>>>,
    pub status: String,
    #[serde(rename = "platformNodeID", skip_serializing_if = "Option::is_none")]
    pub platform_node_id: Option<String>,
    #[serde(rename = "platformP2PPort", skip_serializing_if = "Option::is_none")]
    pub platform_p2p_port: Option<u16>,
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
                addresses: info.addresses,
                platform_node_id: info.platform_node_id,
                platform_p2p_port: info.platform_p2p_port,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dml_entry() -> serde_json::Value {
        json!({
            "proTxHash": "11".repeat(32),
            "address": "192.0.2.1:9999",
            "payee": "payee",
            "status": "ENABLED",
            "type": "Evo",
            "platformNodeID": "22".repeat(20),
            "platformP2PPort": 27656,
            "platformHTTPPort": 1443,
            "pospenaltyscore": 0,
            "consecutivePayments": 0,
            "lastpaidtime": 0,
            "lastpaidblock": 0,
            "owneraddress": "owner",
            "votingaddress": "voter",
            "collateraladdress": "collateral",
            "pubkeyoperator": "operator"
        })
    }

    #[test]
    fn should_preserve_platform_identity_ports_and_separate_addresses_from_dml() {
        let mut entry = dml_entry();
        entry["addresses"] = json!({
            "core_p2p": ["192.0.2.1:9999"],
            "platform_p2p": ["192.0.2.2:27656", "[2001:db8::1]:27656"],
            "platform_https": ["192.0.2.3:1443"]
        });
        let node = Option::<EvoMasternodeInfo>::from(
            serde_json::from_value::<MasternodeInfo>(entry.clone()).unwrap(),
        )
        .unwrap();
        let response = serde_json::to_value(node).unwrap();
        for field in [
            "platformNodeID",
            "platformP2PPort",
            "platformHTTPPort",
            "addresses",
            "address",
            "proTxHash",
        ] {
            assert_eq!(
                response[field], entry[field],
                "{field} was lost in conversion"
            );
        }
    }

    #[test]
    fn should_preserve_legacy_responses_without_inventing_platform_fields() {
        let mut entry = dml_entry();
        for field in ["platformNodeID", "platformP2PPort", "platformHTTPPort"] {
            entry.as_object_mut().unwrap().remove(field);
        }
        let node = Option::<EvoMasternodeInfo>::from(
            serde_json::from_value::<MasternodeInfo>(entry).unwrap(),
        )
        .unwrap();
        let response = serde_json::to_value(node).unwrap();
        for field in [
            "platformNodeID",
            "platformP2PPort",
            "platformHTTPPort",
            "addresses",
        ] {
            assert!(response.get(field).is_none());
        }
        assert_eq!(response["address"], "192.0.2.1:9999");
    }

    #[test]
    fn should_keep_ban_status_and_continue_excluding_regular_masternodes() {
        let mut entry = dml_entry();
        entry["status"] = json!("POSE_BANNED");
        let node = Option::<EvoMasternodeInfo>::from(
            serde_json::from_value::<MasternodeInfo>(entry.clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(node.status, "POSE_BANNED");
        assert_eq!(node.version_check, "fail");
        entry["type"] = json!("Regular");
        assert!(Option::<EvoMasternodeInfo>::from(
            serde_json::from_value::<MasternodeInfo>(entry).unwrap(),
        )
        .is_none());
    }
}
