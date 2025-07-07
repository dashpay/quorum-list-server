use semver::Version;
use std::time::Duration;
use tonic::transport::{Channel, ClientTlsConfig};

#[derive(Debug, Clone)]
pub struct VersionCheckResult {
    pub success: bool,
    pub dapi_version: Option<String>,
    pub drive_version: Option<String>,
}

pub mod platform {
    tonic::include_proto!("org.dash.platform.dapi.v0");
}

use platform::{platform_client::PlatformClient, get_status_request::GetStatusRequestV0};
use platform::GetStatusRequest;

pub async fn check_node_version(address: &str, port: u16) -> Result<VersionCheckResult, Box<dyn std::error::Error + Send + Sync>> {
    // Parse the address and create the endpoint
    let endpoint = format!("https://{}:{}", address, port);
    
    // Create a channel with TLS configuration
    let tls = ClientTlsConfig::new()
        .with_native_roots()
        .assume_http2(true);
    
    let channel = Channel::from_shared(endpoint)?
        .tls_config(tls)?
        .timeout(Duration::from_secs(2))
        .connect_timeout(Duration::from_secs(2))
        .connect()
        .await?;
    
    // Create the gRPC client
    let mut client = PlatformClient::new(channel);
    
    // Create the request
    let request = GetStatusRequest {
        version: Some(platform::get_status_request::Version::V0(GetStatusRequestV0 {})),
    };
    
    // Make the request with a timeout
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client.get_status(request)
    ).await??;
    
    // Extract the version information
    let response_inner = response.into_inner();
    println!("📋 Raw response from {}:{}: {:?}", address, port, response_inner);
    
    if let Some(response_version) = response_inner.version {
        if let platform::get_status_response::Version::V0(v0) = response_version {
            println!("📦 V0 response structure: {:?}", v0);
            
            if let Some(version_info) = v0.version {
                println!("🔧 Version info: {:?}", version_info);
                
                if let Some(software) = version_info.software {
                    println!("💾 Software versions - dapi: {}, drive: {:?}, tenderdash: {:?}", 
                        software.dapi, software.drive, software.tenderdash);
                    
                    let dapi_version = Some(software.dapi.clone());
                    let drive_version = software.drive.clone();
                    
                    // Check if any of the software versions are >= 2.0
                    let mut success = false;
                    
                    if let Some(ref drive_ver) = drive_version {
                        println!("🔍 Checking drive version: {}", drive_ver);
                        if is_version_2_or_higher(drive_ver) {
                            println!("✅ Drive version {} is >= 2.0", drive_ver);
                            success = true;
                        }
                    }
                    
                    if let Some(tenderdash_version) = software.tenderdash {
                        println!("🔍 Checking tenderdash version: {}", tenderdash_version);
                        if is_version_2_or_higher(&tenderdash_version) {
                            println!("✅ Tenderdash version {} is >= 2.0", tenderdash_version);
                            success = true;
                        }
                    }
                    
                    // Check dapi version
                    println!("🔍 Checking dapi version: {}", software.dapi);
                    if is_version_2_or_higher(&software.dapi) {
                        println!("✅ DAPI version {} is >= 2.0", software.dapi);
                        success = true;
                    }
                    
                    if !success {
                        println!("❌ No version >= 2.0 found");
                    }
                    
                    return Ok(VersionCheckResult {
                        success,
                        dapi_version,
                        drive_version,
                    });
                } else {
                    println!("⚠️ No software version info found");
                }
            } else {
                println!("⚠️ No version field in V0 response");
            }
        }
    } else {
        println!("⚠️ No version field in response");
    }
    
    Ok(VersionCheckResult {
        success: false,
        dapi_version: None,
        drive_version: None,
    })
}

fn is_version_2_or_higher(version_str: &str) -> bool {
    // Try to parse the version string
    // Handle versions like "2.0.0", "v2.0.0", "2.0.0-dev.1", etc.
    let cleaned_version = version_str.trim_start_matches('v');
    
    // Try to parse as semver
    if let Ok(version) = Version::parse(cleaned_version) {
        return version.major >= 2;
    }
    
    // If semver parsing fails, try a simple check
    cleaned_version.starts_with("2.") || cleaned_version.starts_with("3.") || cleaned_version.starts_with("4.")
}