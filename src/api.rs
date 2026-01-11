use crate::config::Config;
use crate::masternode::EvoMasternodeList;
use crate::masternode_cache::MasternodeCache;
use crate::quorum_cache::QuorumCache;
use crate::quorum_list::QuorumListEntry;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

pub type SharedQuorumCache = Arc<QuorumCache>;
pub type SharedConfig = Arc<Config>;
pub type SharedMasternodeCache = Arc<MasternodeCache>;

#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub message: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            message: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            message: Some(message),
        }
    }
}

#[derive(Serialize)]
pub struct QuorumStats {
    pub total_count: usize,
    pub is_empty: bool,
}

#[derive(Serialize)]
pub struct QuorumsAtHeightResponse {
    pub height: u32,
    pub quorums: Vec<QuorumEntryResponse>,
}

#[derive(Serialize)]
pub struct QuorumEntryResponse {
    pub quorum_hash: String,
    pub key: String,
    pub height: u32,
    pub members: Vec<QuorumMemberResponse>,
    pub threshold_signature: String,
    pub mining_members_count: u32,
    pub valid_members_count: u32,
}

#[derive(Serialize)]
pub struct QuorumMemberResponse {
    pub proTxHash: String,
    pub pubKeyOperator: String,
    pub valid: bool,
    pub isPublicKeyShare: bool,
}

impl From<&crate::quorum_list::QuorumMember> for QuorumMemberResponse {
    fn from(member: &crate::quorum_list::QuorumMember) -> Self {
        Self {
            proTxHash: member.proTxHash.clone(),
            pubKeyOperator: member.pubKeyOperator.clone(),
            valid: member.valid,
            isPublicKeyShare: member.isPublicKeyShare,
        }
    }
}

impl From<&QuorumListEntry> for QuorumEntryResponse {
    fn from(entry: &QuorumListEntry) -> Self {
        Self {
            quorum_hash: hex::encode(&entry.quorum_hash),
            key: hex::encode(&entry.key),
            height: entry.height,
            members: entry.members.iter().map(|m| m.into()).collect(),
            threshold_signature: entry.threshold_signature.clone(),
            mining_members_count: entry.mining_members_count,
            valid_members_count: entry.valid_members_count,
        }
    }
}


pub fn create_router(quorum_cache: SharedQuorumCache, config: Config, masternode_cache: SharedMasternodeCache) -> Router {
    let shared_config = Arc::new(config);
    Router::new()
        .route("/health", get(health_check))
        .route("/quorums", get(get_all_quorums))
        .route("/quorums/stats", get(get_quorum_stats))
        .route("/previous", get(get_previous_quorums))
        .route("/quorums/:hash", get(get_quorum_by_hash))
        .route("/masternodes", get(get_masternodes))
        .with_state((quorum_cache, shared_config, masternode_cache))
        .layer(CorsLayer::permissive())
}

async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("Quorum List Server is running".to_string()))
}

async fn get_all_quorums(
    State((quorum_cache, _, _)): State<(SharedQuorumCache, SharedConfig, SharedMasternodeCache)>,
) -> Result<Json<ApiResponse<Vec<QuorumEntryResponse>>>, StatusCode> {
    match quorum_cache.get_quorums().await {
        Ok(quorums) => {
            let quorums: Vec<QuorumEntryResponse> = quorums.iter().map(|entry| entry.into()).collect();
            Ok(Json(ApiResponse::success(quorums)))
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to load quorums: {}", e))))
    }
}

async fn get_quorum_stats(
    State((quorum_cache, _, _)): State<(SharedQuorumCache, SharedConfig, SharedMasternodeCache)>,
) -> Result<Json<ApiResponse<QuorumStats>>, StatusCode> {
    match quorum_cache.get_quorums().await {
        Ok(quorums) => {
            let stats = QuorumStats {
                total_count: quorums.len(),
                is_empty: quorums.is_empty(),
            };
            Ok(Json(ApiResponse::success(stats)))
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to get quorum stats: {}", e))))
    }
}

async fn get_quorum_by_hash(
    Path(hash): Path<String>,
    State((quorum_cache, _, _)): State<(SharedQuorumCache, SharedConfig, SharedMasternodeCache)>,
) -> Result<Json<ApiResponse<QuorumEntryResponse>>, StatusCode> {
    let hash_bytes = match hex::decode(&hash) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => return Ok(Json(ApiResponse::error("Invalid hash format. Must be 32 bytes hex encoded.".to_string()))),
    };

    match quorum_cache.get_quorums().await {
        Ok(quorums) => {
            if let Some(entry) = quorums.get_entry(&hash_bytes) {
                Ok(Json(ApiResponse::success(entry.into())))
            } else {
                Ok(Json(ApiResponse::error("Quorum not found".to_string())))
            }
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to get quorums: {}", e))))
    }
}

#[axum::debug_handler]
async fn get_previous_quorums(
    State((quorum_cache, _, _)): State<(SharedQuorumCache, SharedConfig, SharedMasternodeCache)>,
) -> Result<Json<ApiResponse<QuorumsAtHeightResponse>>, StatusCode> {
    match quorum_cache.get_previous_quorums().await {
        Ok((height, quorum_list)) => {
            let quorums: Vec<QuorumEntryResponse> = quorum_list.iter().map(|entry| entry.into()).collect();
            let response = QuorumsAtHeightResponse { height, quorums };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to load previous quorums: {}", e))))
    }
}

#[axum::debug_handler]
async fn get_masternodes(
    State((_, config, masternode_cache)): State<(SharedQuorumCache, SharedConfig, SharedMasternodeCache)>,
) -> Result<Json<ApiResponse<EvoMasternodeList>>, StatusCode> {
    match masternode_cache.get_masternodes().await {
        Ok(masternodes) => {
            // Apply address host override if configured
            let masternodes: EvoMasternodeList = masternodes
                .into_iter()
                .map(|mut m| {
                    m.address = config.apply_address_host_override(&m.address);
                    m
                })
                .collect();
            Ok(Json(ApiResponse::success(masternodes)))
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to load masternodes: {}", e))))
    }
}

