use crate::config::Config;
use crate::quorum_list::{QuorumList, QuorumListEntry};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use tower_http::cors::CorsLayer;

pub type SharedQuorumList = Arc<RwLock<QuorumList>>;
pub type SharedConfig = Arc<Config>;

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


pub fn create_router(shared_list: SharedQuorumList, config: Config) -> Router {
    let shared_config = Arc::new(config);
    Router::new()
        .route("/health", get(health_check))
        .route("/quorums", get(get_all_quorums))
        .route("/quorums/stats", get(get_quorum_stats))
        .route("/quorums/clear", post(clear_quorums))
        .route("/previous", get(get_previous_quorums))
        .route("/quorums/:hash", get(get_quorum_by_hash))
        .with_state((shared_list, shared_config))
        .layer(CorsLayer::permissive())
}

async fn health_check() -> Json<ApiResponse<String>> {
    Json(ApiResponse::success("Quorum List Server is running".to_string()))
}

async fn get_all_quorums(
    State((shared_list, config)): State<(SharedQuorumList, SharedConfig)>,
) -> Result<Json<ApiResponse<Vec<QuorumEntryResponse>>>, StatusCode> {
    // Fetch fresh quorums from Dash Core
    match crate::quorum_loader::load_initial_quorums(&config).await {
        Ok(new_quorums) => {
            // Update the shared list with fresh data
            match shared_list.write() {
                Ok(mut list) => {
                    *list = new_quorums;
                }
                Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
            
            // Now read and return the updated list
            match shared_list.read() {
                Ok(list) => {
                    let quorums: Vec<QuorumEntryResponse> = list.iter().map(|entry| entry.into()).collect();
                    Ok(Json(ApiResponse::success(quorums)))
                }
                Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to load quorums: {}", e))))
    }
}

async fn get_quorum_stats(
    State((shared_list, _)): State<(SharedQuorumList, SharedConfig)>,
) -> Result<Json<ApiResponse<QuorumStats>>, StatusCode> {
    match shared_list.read() {
        Ok(list) => {
            let stats = QuorumStats {
                total_count: list.len(),
                is_empty: list.is_empty(),
            };
            Ok(Json(ApiResponse::success(stats)))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn get_quorum_by_hash(
    Path(hash): Path<String>,
    State((shared_list, _)): State<(SharedQuorumList, SharedConfig)>,
) -> Result<Json<ApiResponse<QuorumEntryResponse>>, StatusCode> {
    let hash_bytes = match hex::decode(&hash) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => return Ok(Json(ApiResponse::error("Invalid hash format. Must be 32 bytes hex encoded.".to_string()))),
    };

    match shared_list.read() {
        Ok(list) => {
            if let Some(entry) = list.get_entry(&hash_bytes) {
                Ok(Json(ApiResponse::success(entry.into())))
            } else {
                Ok(Json(ApiResponse::error("Quorum not found".to_string())))
            }
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}


async fn clear_quorums(
    State((shared_list, _)): State<(SharedQuorumList, SharedConfig)>,
) -> Result<Json<ApiResponse<String>>, StatusCode> {
    match shared_list.write() {
        Ok(mut list) => {
            list.clear();
            Ok(Json(ApiResponse::success("All quorums cleared successfully".to_string())))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[axum::debug_handler]
async fn get_previous_quorums(
    State((_, config)): State<(SharedQuorumList, SharedConfig)>,
) -> Result<Json<ApiResponse<QuorumsAtHeightResponse>>, StatusCode> {
    match crate::quorum_loader::get_current_block_height(&config).await {
        Ok(current_height) => {
            let previous_height = if current_height >= config.quorum.previous_blocks_offset { 
                current_height - config.quorum.previous_blocks_offset 
            } else { 
                0 
            };
            
            match crate::quorum_loader::load_quorums_at_height(&config, previous_height).await {
                Ok(quorum_list) => {
                    let quorums: Vec<QuorumEntryResponse> = quorum_list.iter().map(|entry| entry.into()).collect();
                    let response = QuorumsAtHeightResponse { height: previous_height, quorums };
                    Ok(Json(ApiResponse::success(response)))
                }
                Err(e) => Ok(Json(ApiResponse::error(format!("Failed to load quorums: {}", e))))
            }
        }
        Err(e) => Ok(Json(ApiResponse::error(format!("Failed to get block height: {}", e))))
    }
}

