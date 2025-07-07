use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub rpc: RpcConfig,
    pub quorum: QuorumConfig,
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuorumConfig {
    pub previous_blocks_offset: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_network")]
    pub network: String, // "mainnet" or "testnet"
}

fn default_network() -> String {
    "testnet".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                port: 3000,
                host: "0.0.0.0".to_string(),
            },
            rpc: RpcConfig {
                url: "http://127.0.0.1:19998".to_string(),
                username: "dashrpc".to_string(),
                password: "password".to_string(),
            },
            quorum: QuorumConfig {
                previous_blocks_offset: 8,
            },
            network: NetworkConfig {
                network: default_network(),
            },
        }
    }
}

impl Config {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_from_env_or_file<P: AsRef<Path>>(path: P) -> Self {
        // Try to load from file first
        if let Ok(config) = Self::load_from_file(path) {
            return config;
        }

        // Fall back to environment variables or defaults
        let mut config = Config::default();
        
        if let Ok(port) = std::env::var("API_PORT") {
            if let Ok(port_num) = port.parse::<u16>() {
                config.server.port = port_num;
            }
        }

        if let Ok(host) = std::env::var("API_HOST") {
            config.server.host = host;
        }

        if let Ok(url) = std::env::var("DASH_RPC_URL") {
            config.rpc.url = url;
        }

        if let Ok(username) = std::env::var("DASH_RPC_USER") {
            config.rpc.username = username;
        }

        if let Ok(password) = std::env::var("DASH_RPC_PASSWORD") {
            config.rpc.password = password;
        }

        if let Ok(offset) = std::env::var("QUORUM_PREVIOUS_BLOCKS_OFFSET") {
            if let Ok(offset_num) = offset.parse::<u32>() {
                config.quorum.previous_blocks_offset = offset_num;
            }
        }

        if let Ok(network) = std::env::var("DASH_NETWORK") {
            if network == "mainnet" || network == "testnet" {
                config.network.network = network;
            }
        }

        config
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn get_llmq_type(&self) -> &str {
        match self.network.network.as_str() {
            "mainnet" => "llmq_100_67",
            _ => "llmq_25_67", // default to testnet
        }
    }

    pub fn get_llmq_type_id(&self) -> u32 {
        match self.network.network.as_str() {
            "mainnet" => 4, // llmq_100_67 = type 4
            _ => 6, // llmq_25_67 = type 6 (testnet)
        }
    }

    pub fn get_dapi_port(&self) -> u16 {
        match self.network.network.as_str() {
            "mainnet" => 443,
            _ => 1443, // default to testnet
        }
    }
}