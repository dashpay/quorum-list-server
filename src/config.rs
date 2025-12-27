use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    #[default]
    Testnet,
    Regtest,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Mainnet => write!(f, "mainnet"),
            Network::Testnet => write!(f, "testnet"),
            Network::Regtest => write!(f, "regtest"),
        }
    }
}

impl TryFrom<&str> for Network {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s.to_lowercase().as_str() {
            "mainnet" => Ok(Network::Mainnet),
            "testnet" => Ok(Network::Testnet),
            "regtest" => Ok(Network::Regtest),
            _ => Err(format!(
                "Invalid network '{}'. Must be one of: mainnet, testnet, regtest",
                s
            )),
        }
    }
}

impl Network {
    pub fn llmq_type(&self) -> &'static str {
        match self {
            Network::Mainnet => "llmq_100_67",
            Network::Testnet => "llmq_25_67",
            Network::Regtest => "llmq_test_platform",
        }
    }

    pub fn llmq_type_id(&self) -> u32 {
        match self {
            Network::Mainnet => 4,   // llmq_100_67 = type 4
            Network::Testnet => 6,   // llmq_25_67 = type 6
            Network::Regtest => 106, // llmq_test_platform = type 106
        }
    }

    pub fn dapi_port(&self) -> u16 {
        match self {
            Network::Mainnet => 443,
            Network::Testnet => 1443,
            Network::Regtest => 2443,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub rpc: RpcConfig,
    pub quorum: QuorumConfig,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub docker: DockerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DockerConfig {
    /// Use this host for version checks instead of the masternode's reported address.
    /// Useful when running in Docker where the reported IPs are not reachable.
    /// Example: "host.docker.internal" for Docker Desktop
    #[serde(default)]
    pub version_check_host: Option<String>,

    /// Replace the host in masternode addresses returned by the /masternodes endpoint.
    /// Useful when clients need to connect via a different host than what's reported.
    /// Example: "127.0.0.1" for local testing
    #[serde(default)]
    pub address_host_override: Option<String>,
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
            network: Network::default(),
            docker: DockerConfig::default(),
        }
    }
}

impl Config {
    pub fn load_from_file<P: AsRef<Path>>(
        path: P,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
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

        if let Ok(network_str) = std::env::var("DASH_NETWORK") {
            config.network =
                Network::try_from(network_str.as_str()).unwrap_or_else(|e| panic!("{}", e));
        }

        if let Ok(version_check_host) = std::env::var("VERSION_CHECK_HOST") {
            config.docker.version_check_host = Some(version_check_host);
        }

        if let Ok(address_host_override) = std::env::var("ADDRESS_HOST_OVERRIDE") {
            config.docker.address_host_override = Some(address_host_override);
        }

        config
    }

    pub fn save_to_file<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    pub fn get_llmq_type(&self) -> &'static str {
        self.network.llmq_type()
    }

    pub fn get_llmq_type_id(&self) -> u32 {
        self.network.llmq_type_id()
    }

    pub fn get_dapi_port(&self) -> u16 {
        self.network.dapi_port()
    }

    /// Get the host to use for version checks.
    /// If version_check_host is configured, use that host instead of the original address host.
    /// Returns the original host if no replacement is configured.
    pub fn get_version_check_host(&self, address: &str) -> String {
        if let Some(ref replacement) = self.docker.version_check_host {
            replacement.clone()
        } else {
            // Extract host from address (format: "ip:port")
            address
                .rsplit_once(':')
                .map(|(h, _)| h.to_string())
                .unwrap_or_else(|| address.to_string())
        }
    }

    /// Apply address host override to an address string.
    /// If address_host_override is configured, replace the host portion while keeping the port.
    /// Returns the original address if no override is configured.
    pub fn apply_address_host_override(&self, address: &str) -> String {
        if let Some(ref override_host) = self.docker.address_host_override {
            // Extract port from address (format: "ip:port")
            if let Some((_, port)) = address.rsplit_once(':') {
                format!("{}:{}", override_host, port)
            } else {
                address.to_string()
            }
        } else {
            address.to_string()
        }
    }
}
