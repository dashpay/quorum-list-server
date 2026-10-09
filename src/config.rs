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
    Devnet,
    Regtest,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Mainnet => write!(f, "mainnet"),
            Network::Testnet => write!(f, "testnet"),
            Network::Devnet => write!(f, "devnet"),
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
            "devnet" => Ok(Network::Devnet),
            "regtest" => Ok(Network::Regtest),
            _ => Err(format!(
                "Invalid network '{}'. Must be one of: mainnet, testnet, devnet, regtest",
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
            Network::Devnet => "llmq_devnet_platform",
            Network::Regtest => "llmq_test_platform",
        }
    }

    pub fn llmq_type_id(&self) -> u32 {
        match self {
            Network::Mainnet => 4,   // llmq_100_67 = type 4
            Network::Testnet => 6,   // llmq_25_67 = type 6
            Network::Devnet => 107,  // llmq_devnet_platform = type 107
            Network::Regtest => 106, // llmq_test_platform = type 106
        }
    }

    pub fn dapi_port(&self) -> u16 {
        match self {
            Network::Mainnet => 443,
            Network::Testnet => 1443,
            Network::Devnet => 1443,
            Network::Regtest => 2443,
        }
    }

    /// Platform LLMQ defaults from Dash Core's llmq/params.h.
    pub fn platform_quorum_parameters(&self) -> (u32, u32) {
        match self {
            Network::Mainnet | Network::Testnet => (24, 24),
            Network::Devnet => (4, 24),
            Network::Regtest => (2, 24),
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
#[serde(default)]
pub struct QuorumConfig {
    pub previous_blocks_offset: u32,
    pub cache_ttl_seconds: u64,
    pub retention_blocks: u32,
    pub sample_step_blocks: Option<u32>,
    pub active_quorum_count: Option<u32>,
    pub dkg_interval: Option<u32>,
    pub max_entries: usize,
    pub key_memo_max_entries: usize,
    pub poll_interval_seconds: u64,
    pub health_stale_intervals: u32,
}

impl Default for QuorumConfig {
    fn default() -> Self {
        Self {
            previous_blocks_offset: 8,
            cache_ttl_seconds: 60,
            retention_blocks: 48,
            sample_step_blocks: None,
            active_quorum_count: None,
            dkg_interval: None,
            max_entries: 90,
            key_memo_max_entries: 180,
            poll_interval_seconds: 1,
            health_stale_intervals: 3,
        }
    }
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
            quorum: QuorumConfig::default(),
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

        if let Ok(ttl) = std::env::var("QUORUM_CACHE_TTL_SECONDS") {
            if let Ok(ttl_num) = ttl.parse::<u64>() {
                config.quorum.cache_ttl_seconds = ttl_num;
            }
        }

        macro_rules! quorum_env {
            ($name:literal, $field:ident) => {
                if let Ok(value) = std::env::var($name) {
                    config.quorum.$field = value
                        .parse()
                        .unwrap_or_else(|_| panic!("Invalid {}", $name));
                }
            };
            ($name:literal, $field:ident, optional) => {
                if let Ok(value) = std::env::var($name) {
                    config.quorum.$field = Some(
                        value
                            .parse()
                            .unwrap_or_else(|_| panic!("Invalid {}", $name)),
                    );
                }
            };
        }
        quorum_env!("QUORUM_RETENTION_BLOCKS", retention_blocks);
        quorum_env!("QUORUM_SAMPLE_STEP_BLOCKS", sample_step_blocks, optional);
        quorum_env!("QUORUM_ACTIVE_QUORUM_COUNT", active_quorum_count, optional);
        quorum_env!("QUORUM_DKG_INTERVAL", dkg_interval, optional);
        quorum_env!("QUORUM_MAX_ENTRIES", max_entries);
        quorum_env!("QUORUM_KEY_MEMO_MAX_ENTRIES", key_memo_max_entries);
        quorum_env!("QUORUM_POLL_INTERVAL_SECONDS", poll_interval_seconds);
        quorum_env!("QUORUM_HEALTH_STALE_INTERVALS", health_stale_intervals);

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

    pub fn quorum_sample_step(&self) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        let (active, interval) = self.network.platform_quorum_parameters();
        let active = self.quorum.active_quorum_count.unwrap_or(active);
        let interval = self.quorum.dkg_interval.unwrap_or(interval);
        let lifetime = active
            .checked_mul(interval)
            .filter(|value| *value > 0)
            .ok_or(
                "Quorum active count and DKG interval must be positive and their product fit u32",
            )?;
        // Commitment mining offsets can differ within a cycle, shortening an active run.
        let max_step = (active - 1) * interval + 1;
        let step = self
            .quorum
            .sample_step_blocks
            .unwrap_or((lifetime / 2).min(max_step).max(1));
        if step == 0 || step > max_step {
            return Err(format!("Quorum sample step {step} must be between 1 and {max_step}: commitment mining offsets can shorten active runs").into());
        }
        if self.quorum.cache_ttl_seconds == 0
            || self.quorum.poll_interval_seconds == 0
            || self.quorum.health_stale_intervals == 0
            || self.quorum.max_entries == 0
            || self.quorum.key_memo_max_entries == 0
        {
            return Err("Quorum intervals and cache limits must be positive".into());
        }
        Ok(step)
    }

    pub fn quorum_retention(&self) -> u32 {
        self.quorum
            .retention_blocks
            .max(self.quorum.previous_blocks_offset)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_platform_devnet() {
        let network = Network::try_from("devnet").expect("devnet should parse");

        assert_eq!(network, Network::Devnet);
        assert_eq!(network.to_string(), "devnet");
        assert_eq!(network.llmq_type(), "llmq_devnet_platform");
        assert_eq!(network.llmq_type_id(), 107);
        assert_eq!(network.dapi_port(), 1443);
    }

    #[test]
    fn deserializes_platform_devnet() {
        let toml = r#"
network = "devnet"

[server]
port = 8080
host = "0.0.0.0"

[rpc]
url = "http://127.0.0.1:20002"
username = "dashrpc"
password = "password"

[quorum]
previous_blocks_offset = 8
"#;
        let config: Config = toml::from_str(toml).expect("devnet config should deserialize");

        assert_eq!(config.network, Network::Devnet);
        assert_eq!(config.get_llmq_type(), "llmq_devnet_platform");
        assert_eq!(config.get_llmq_type_id(), 107);
    }

    #[test]
    fn should_default_legacy_configs_to_a_safe_rolling_window() {
        let quorum: QuorumConfig = toml::from_str("previous_blocks_offset = 8").unwrap();
        assert_eq!(quorum.retention_blocks, 48);
        assert_eq!(quorum.cache_ttl_seconds, 60);
        assert_eq!(quorum.max_entries, 90);
        for (network, expected) in [
            (Network::Mainnet, 288),
            (Network::Testnet, 288),
            (Network::Devnet, 48),
            (Network::Regtest, 24),
        ] {
            let mut config = Config {
                network,
                ..Config::default()
            };
            assert_eq!(config.quorum_sample_step().unwrap(), expected);
            config.quorum.active_quorum_count = Some(1);
            assert_eq!(config.quorum_sample_step().unwrap(), 1);
        }
    }

    #[test]
    fn should_reject_invalid_quorum_intervals_and_limits() {
        let mut config = Config::default();
        config.quorum.sample_step_blocks = Some(0);
        assert!(config.quorum_sample_step().is_err());
        config.quorum.sample_step_blocks = None;
        config.quorum.active_quorum_count = Some(0);
        assert!(config.quorum_sample_step().is_err());
        config.quorum.active_quorum_count = Some(u32::MAX);
        assert!(config.quorum_sample_step().is_err());
        config.quorum.active_quorum_count = None;
        config.quorum.cache_ttl_seconds = 0;
        assert!(config.quorum_sample_step().is_err());
    }

    #[test]
    fn should_preserve_quorum_env_fallbacks() {
        let variables = [
            ("QUORUM_PREVIOUS_BLOCKS_OFFSET", "96"),
            ("QUORUM_CACHE_TTL_SECONDS", "1"),
            ("QUORUM_RETENTION_BLOCKS", "720"),
            ("QUORUM_SAMPLE_STEP_BLOCKS", "12"),
            ("QUORUM_ACTIVE_QUORUM_COUNT", "4"),
            ("QUORUM_DKG_INTERVAL", "24"),
            ("QUORUM_MAX_ENTRIES", "80"),
            ("QUORUM_KEY_MEMO_MAX_ENTRIES", "160"),
            ("QUORUM_POLL_INTERVAL_SECONDS", "2"),
            ("QUORUM_HEALTH_STALE_INTERVALS", "5"),
        ];
        let original: Vec<_> = variables
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in variables {
            std::env::set_var(name, value);
        }
        let config = Config::load_from_env_or_file("/nonexistent/qls16-config.toml");
        for (name, value) in original {
            if let Some(value) = value {
                std::env::set_var(name, value);
            } else {
                std::env::remove_var(name);
            }
        }
        assert_eq!(config.quorum.previous_blocks_offset, 96);
        assert_eq!(config.quorum.cache_ttl_seconds, 1);
        assert_eq!(config.quorum_retention(), 720);
        assert_eq!(config.quorum_sample_step().unwrap(), 12);
        assert_eq!(config.quorum.max_entries, 80);
        assert_eq!(config.quorum.key_memo_max_entries, 160);
        assert_eq!(config.quorum.poll_interval_seconds, 2);
        assert_eq!(config.quorum.health_stale_intervals, 5);
    }

    #[test]
    fn should_load_the_shipped_config_file() {
        let config: Config = toml::from_str(include_str!("../config.toml")).unwrap();
        assert_eq!(config.network, Network::Testnet);
        assert_eq!(config.quorum_sample_step().unwrap(), 288);
    }
}
