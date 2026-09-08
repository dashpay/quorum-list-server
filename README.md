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

### Masternode bootstrap metadata

`GET /masternodes` preserves the existing `data` array and adds `lastUpdated` to
successful responses. It is the Unix timestamp in seconds when the last
successfully cached DML fetch began. Repeated reads and failed refreshes do not
advance it. The list and timestamp are published together; the endpoint performs
no RPC calls or node probes. Clients should check both `success` and freshness
before using the data (the refresh interval is ten minutes).

Each evonode now includes `platformNodeID` and `platformP2PPort` when Core supplies
them, alongside the existing `platformHTTPPort`. On Core versions that supply
separate service endpoints, `addresses` preserves the `core_p2p`, `platform_p2p`,
and `platform_https` arrays, including non-default ports and IPv6 addresses.
Prefer `addresses.platform_p2p` when present; otherwise use the host in `address`
with `platformP2PPort`. Missing fields are omitted rather than guessed. Configured
address host overrides apply to these endpoint arrays as well as `address`.

```json
{
  "success": true,
  "data": [{
    "proTxHash": "...",
    "address": "192.0.2.1:9999",
    "addresses": {"platform_p2p": ["192.0.2.2:27656"]},
    "status": "ENABLED",
    "platformNodeID": "...",
    "platformP2PPort": 27656,
    "platformHTTPPort": 443,
    "versionCheck": "success"
  }],
  "message": null,
  "lastUpdated": 1788828000
}
```

The timestamp describes cache freshness, not Core synchronization or a successful
Tenderdash P2P handshake. Consumers still need to exclude banned entries and
validate the node ID and endpoint fields. Existing clients can continue reading
`data` unchanged.

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

## Docker

Build and run the containerized server (default port 3000 unless you mount a `config.toml`):

```bash
docker build -t quorum-list-server .

docker run --rm -p 3000:3000 \
  -e DASH_RPC_URL="http://192.168.1.100:19998" \
  -e DASH_RPC_USER="dashrpc" \
  -e DASH_RPC_PASSWORD="password" \
  -e DASH_NETWORK="testnet" \
  quorum-list-server
```

To use a custom configuration file instead of environment variables, mount it at `/app/config.toml`:

```bash
docker run --rm -p 8080:8080 \
  -v $(pwd)/config.toml:/app/config.toml:ro \
  quorum-list-server
```

The server reads `/app/config.toml` first; environment variables are only used when that file is absent.

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
