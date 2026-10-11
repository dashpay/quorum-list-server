# Quorum List Server

A Rust-based HTTP API server providing the configured network's Platform quorum lists via RPC calls to Dash Core.

## Features

- RPC-based Platform quorum loading for mainnet, testnet, devnet and regtest
- RESTful API for quorum list management
- TOML-based configuration with environment variable overrides
- Stateless rolling window of active and recently retired quorums
- Thread-safe shared state
- Cross-platform support (Linux/Windows)
- Graceful shutdown handling

## API Endpoints

### Health Check
- `GET /health` - Quorum cache freshness status (HTTP 200, inspect the JSON body)
- `GET /ready` - Readiness status (HTTP 503 while degraded, 200 when fresh)

### Quorum Management
- `GET /quorums` - Get all current quorums
- `GET /quorums/stats` - Get quorum statistics 
- `GET /quorums/{hash}` - Get specific quorum by hash
- `GET /previous` - Get the union of Platform quorums active in the retention window

### Quorum retention and freshness

`/quorums` keeps its existing `data` array and adds top-level `height` and
`lastUpdated`. These describe the Core height sampled and the Unix timestamp in
seconds of the successful refresh. Repeated reads and failed active-list refreshes
do not advance them. Entries are ordered oldest to newest.

`/previous` keeps `{ "success": true, "data": { "height": ..., "quorums": [...] } }`.
Its list is rebuilt from Core on every refresh by taking the union of
`quorum listextended` at the tip, then descending by the configured sample step,
including the lower boundary `max(0, tip - R)`. Here
`R = max(retention_blocks, previous_blocks_offset)`: existing offset settings
retain at least their old coverage. `data.height` is now the lowest sampled
height. No membership history is persisted, so a restart reconstructs the same
window and a refresh removes orphaned quorums after a reorg.

The Core 24 rc.3 Platform parameters are:

| Network | LLMQ type | Active count A | DKG interval D | Default step |
| --- | --- | --- | --- | --- |
| mainnet | 4 (`llmq_100_67`) | 24 | 24 | 288 |
| testnet | 6 (`llmq_25_67`) | 24 | 24 | 288 |
| devnet | 107 (`llmq_devnet_platform`) | 4 | 24 | 48 |
| regtest | 106 (`llmq_test_platform`) | 2 | 24 | 24 |

Quorums become active when their commitments are mined. Core allows mining
offsets 10 through 18 within a DKG cycle, so a run can be as short as `A*D - 8`
blocks. Sampling every `A*D` blocks could miss a quorum. The conservative maximum
step is `(A-1)*D + 1`, allowing any mining offset within a cycle. The default is
`max(1, min(A*D/2, (A-1)*D+1))`. Failed DKGs lengthen active runs. Parameter
overrides must match the connected Core; unsafe steps, zero intervals and zero
cache limits are rejected at startup with an error. The output cap must exceed A,
the key memo must hold at least the output cap, and a window may use at most
256 samples (including both boundaries) to limit work within the refresh budget. See Core's
[parameter table](https://github.com/dashpay/dash/blob/v24.0.0-rc.3/src/llmq/params.h)
and [mined commitment scan](https://github.com/dashpay/dash/blob/v24.0.0-rc.3/src/llmq/quorumsman.cpp#L243-L244).

The rolling list is ordered oldest to newest, with a default cap of 90 to fit
clients' 100-entry caches. Overflow removes the oldest retired quorums first;
active quorums are always retained, even if their count alone exceeds the cap.
Trimming sacrifices coverage at the old end of the window. Size the retention
and cap together. The 48-block default is suitable for 150-second blocks;
accelerated 10-second devnets typically need 360–720 blocks to cover Core/Platform
lag, a Platform block interval, the client's 31-minute metadata tolerance and
clock skew.

A bounded public-key memo keyed by `(quorumHash, minedBlockHash)` avoids repeat
`quorum info` calls for unchanged commitments. A reorg can replace a commitment
and its public key for the same quorum hash; the new mined block forces a refetch.
All such calls explicitly use `includeSkShare=false`. Membership is recomputed,
not taken from the memo. All quorum RPC runs on a serialized blocking worker;
requests only read cached data. RPC sockets have a five-second timeout, refreshes
have a 30-second deadline, and a timed-out worker retains its permit until it
finishes, preventing accumulating background workers.

The service polls `getblockcount` once per second and reloads when the height
changes. The cache TTL bounds reloads even at an unchanged height (including
same-height reorgs), and a poll interval longer than the TTL is reduced to the
TTL. Both lists are attempted independently and keep their last good data on
failure. An empty active list or invalid quorum/key data is a failed refresh,
never an empty success. Historical samples must contain every tip-active quorum
whose creation height plus the mining-window end (18 blocks) precedes the sample;
a short or empty result missing such a quorum fails the window refresh. A moving
tip preserves the current list tied to its sampled height and retries the window
once at the new tip. Repeated failed window reloads back off up to the smaller of
30 seconds and the TTL; a newly observed tip bypasses this backoff.

`/health` still returns HTTP 200 and the same JSON shape;
it returns `success:false`, `data:null` and a degraded message when the latest
window refresh failed, either list has never loaded, or its last successful
refresh is older than
`cache_ttl_seconds * health_stale_intervals` (three intervals by default). `/ready`
returns the same body with HTTP 503 while degraded and HTTP 200 when fresh, so
load balancers can use it without changing existing `/health` matchers.

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
network = "testnet"

[server]
port = 3000
host = "0.0.0.0"

[rpc]
url = "http://127.0.0.1:19998"
username = "dashrpc"
password = "password"

[quorum]
previous_blocks_offset = 8
retention_blocks = 48
cache_ttl_seconds = 60
poll_interval_seconds = 1
max_entries = 90
key_memo_max_entries = 180
health_stale_intervals = 3
# sample_step_blocks = 288
# active_quorum_count = 24
# dkg_interval = 24
```

### Environment Variables (overrides)
- `API_HOST` - Server host (default: 0.0.0.0)
- `API_PORT` - HTTP server port (default: 3000)
- `DASH_RPC_URL` - RPC endpoint (default: http://127.0.0.1:19998)
- `DASH_RPC_USER` - RPC username (default: dashrpc)
- `DASH_RPC_PASSWORD` - RPC password (default: password)
- `DASH_NETWORK` - mainnet, testnet, devnet or regtest (default: testnet)

| TOML `[quorum]` key | Environment override | Default |
| --- | --- | --- |
| `previous_blocks_offset` | `QUORUM_PREVIOUS_BLOCKS_OFFSET` | 8; lower bound on retention |
| `retention_blocks` | `QUORUM_RETENTION_BLOCKS` | 48 |
| `cache_ttl_seconds` | `QUORUM_CACHE_TTL_SECONDS` | 60; local dashmate networks can use 1 |
| `sample_step_blocks` | `QUORUM_SAMPLE_STEP_BLOCKS` | Derived from A and D as above |
| `active_quorum_count` | `QUORUM_ACTIVE_QUORUM_COUNT` | Network table above |
| `dkg_interval` | `QUORUM_DKG_INTERVAL` | 24 blocks |
| `max_entries` | `QUORUM_MAX_ENTRIES` | 90; active entries are never trimmed |
| `key_memo_max_entries` | `QUORUM_KEY_MEMO_MAX_ENTRIES` | 180 public keys |
| `poll_interval_seconds` | `QUORUM_POLL_INTERVAL_SECONDS` | 1 |
| `health_stale_intervals` | `QUORUM_HEALTH_STALE_INTERVALS` | 3 |

Environment variables override TOML values. When the file cannot be loaded,
environment variables override the built-in defaults. Older TOML files
with only `previous_blocks_offset` continue to work with the new defaults.

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

The server reads `/app/config.toml` first, then applies environment overrides.
The image does not set `API_HOST` or `API_PORT`; the binary defaults to `0.0.0.0:3000`
when neither the file nor explicitly supplied environment variables configure them.

## API Examples

```bash
# Check health
curl http://localhost:3000/health

# Get all current quorums
curl http://localhost:3000/quorums

# Get quorum stats
curl http://localhost:3000/quorums/stats

# Get active and recently retired quorums (48-block window by default)
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
- **LLMQ Type**: Processes the configured network's Platform quorum type
- **Configuration**: TOML values with environment variable overrides
- **State Management**: Cached lists and freshness metadata published under one lock
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

### Verified snapshot proofs

`POST /proofs` forwards compact Core bootstrap evidence as binary
`application/octet-stream`, with the same API as DAPI. Configure the existing RPC
connection to a Core build supporting `getquorumproofchain` with the required
historical blocks retained. Core reads evidence from disk on demand; no extra
index or startup scan is required. An unpruned node can serve older checkpoints.

```json
{"checkpoint":"<release checkpoint block hash>","height":1549547,"quorumHash":"<Platform quorum hash>","llmqType":6,"nodeCount":4}
```

Hashes use RPC display order. `height` is a minimum target height; zero selects
Core's latest available ChainLock. Mainnet uses Platform quorum type 4, testnet 6.
`nodeCount` is 0–15. Success contains a `DASHNC02` proof and authenticated quorum
and EvoNode record openings, not JSON keys to trust. Clients verify it against
an independently pinned snapshot. Gzip is supported; clients must enforce the
1 MiB decoded response limit.

The relay limits request bodies to 1 KiB, concurrent Core workers to two, and
cached proof payloads to 16 MiB / 64 entries for 15 seconds. Invalid requests
return 400 (or 422 for invalid JSON schema), oversized bodies 413, saturated
workers 503, upstream failures 502, and timeouts 504. Core responses are cut off
at the socket above roughly 4 MiB before they are buffered or parsed. Each Core
call runs detached from the public connection: identical concurrent requests
share one call, a client that disconnects does not free the worker early, and
the result is cached for later callers. One 60-second deadline covers the whole
Core round trip; hitting it drops the Core connection. Core finishes an
already-dispatched proof anyway and replies only when it is done, so a worker
whose call got no reply after Core started work (deadline, or the connection
lost while waiting) stays reserved for a further 120 seconds. Any reply from
Core, including an error, releases the worker at once. This bounds how fast the relay re-dispatches abandoned work,
roughly one call per three minutes per worker; it is a rate limit, not a
guarantee that Core has finished, since very long historical spans can outlast
it. Fast failures release the worker immediately. Clients
should allow additional time for response delivery (the SDK uses 65 seconds).
Failure causes are logged to stderr without credentials or response bodies;
every public error response, including malformed JSON, returns its status code
with an empty response body.

Existing `/quorums`, `/previous`, and `/masternodes` endpoints remain available
for explicitly trusted clients. This branch does not deploy the service.
