# Quorum List Server

A Rust-based HTTP API server that provides RESTful endpoints for managing Dash LLMQ_25_67 quorum lists via RPC calls to Dash Core.

## Features

- RPC-based quorum loading from Dash Core (LLMQ_25_67 quorums for testnet)
- RESTful API for quorum list management
- TOML-based configuration with environment variable fallbacks
- Previous quorum state lookup (configurable block offset)
- Thread-safe shared state
- Cross-platform support (Linux/Windows)
- Graceful shutdown handling

## API Endpoints

### Health Check
- `GET /health` - Server health status

### Quorum Management
- `GET /quorums` - Get all current quorums
- `GET /quorums/stats` - Get quorum statistics 
- `GET /quorums/{hash}` - Get specific quorum by hash
- `GET /previous` - Get quorums from previous blocks (configurable offset)
- `POST /quorums/clear` - Clear all quorums

## Configuration

### config.toml
```toml
[server]
port = 3000
host = "0.0.0.0"

[rpc]
url = "http://127.0.0.1:19998"
username = "dashrpc"
password = "password"

[quorum]
previous_blocks_offset = 8
```

### Environment Variables (fallbacks)
- `API_HOST` - Server host (default: 0.0.0.0)
- `API_PORT` - HTTP server port (default: 3000)
- `DASH_RPC_URL` - RPC endpoint (default: http://127.0.0.1:19998)
- `DASH_RPC_USER` - RPC username (default: dashrpc)
- `DASH_RPC_PASSWORD` - RPC password (default: password)
- `QUORUM_PREVIOUS_BLOCKS_OFFSET` - Previous blocks offset (default: 8)

## Usage

```bash
# Start the server (uses config.toml)
cargo run

# With environment variables (overrides config.toml)
DASH_RPC_URL="http://192.168.1.100:19998" DASH_RPC_USER="myuser" cargo run

# Different port
API_PORT=8080 cargo run
```

## API Examples

```bash
# Check health
curl http://localhost:3000/health

# Get all current quorums
curl http://localhost:3000/quorums

# Get quorum stats
curl http://localhost:3000/quorums/stats

# Get previous quorums (8 blocks ago by default)
curl http://localhost:3000/previous

# Get specific quorum by hash
curl http://localhost:3000/quorums/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef

# Clear all quorums
curl -X POST http://localhost:3000/quorums/clear
```

## Response Format

All API responses follow this format:
```json
{
  "success": true,
  "data": { ... },
  "message": null
}
```

Example quorum response:
```json
{
  "success": true,
  "data": {
    "height": 1277520,
    "quorums": [
      {
        "quorum_hash": "00000226897e9f185152567c3ea4a529a2f2214d493d6a12627ddd5a13bf4443",
        "key": "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "height": 1277520,
        "members": [],
        "threshold_signature": "",
        "mining_members_count": 0,
        "valid_members_count": 25
      }
    ]
  }
}
```

## Architecture

- **RPC Integration**: Uses `dashcore-rpc` to communicate with Dash Core
- **LLMQ Type**: Only processes LLMQ_25_67 (type 6) quorums for testnet
- **Configuration**: TOML-first with environment variable fallbacks
- **State Management**: Thread-safe Arc<RwLock<QuorumList>> for shared state
- **API Framework**: Built with Axum for async HTTP handling

## Development

```bash
# Check compilation
cargo check

# Run with debug logging
RUST_LOG=debug cargo run

# Build release
cargo build --release

# Format code
cargo fmt

# Lint code
cargo clippy
```

## Requirements

- Rust 1.70+
- Access to a running Dash Core node with RPC enabled
- Dash Core configured for testnet (for LLMQ_25_67 quorums)

## RPC Configuration

Your Dash Core `dash.conf` should include:
```ini
server=1
rpcuser=dashrpc
rpcpassword=password
rpcallowip=127.0.0.1
testnet=1
```