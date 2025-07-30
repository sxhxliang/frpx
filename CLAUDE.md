# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Common Development Commands

### Build and Run
```bash
# Build all workspace members in release mode
cargo build --release

# Run the server (frps)
cargo run --release --bin frps

# Run a client (frpc) - requires unique client ID
cargo run --release --bin frpc -- --client-id client_A

# Test the complete system
./test_frpx.sh
```

### Server Configuration
Default ports can be overridden via command line arguments:
- Control port: `--control-port 17000` (client registration)
- Proxy port: `--proxy-port 17001` (data forwarding)
- Public port: `--public-port 18080` (external traffic)
- API port: `--api-port 18081` (RESTful monitoring)

### Client Configuration
```bash
# Basic client with custom server address
cargo run --release --bin frpc -- --client-id client_A --server-addr 192.168.1.100

# Client with custom local service
cargo run --release --bin frpc -- --client-id client_A --local-addr 127.0.0.1 --local-port 8080

# Client with auth credentials (skips interactive prompt)
cargo run --release --bin frpc -- --client-id client_A --email user@example.com --password secret123
```

## Architecture Overview

This is a reverse proxy system with load balancing, consisting of three main components:

### Workspace Structure
- `frps/` - Server binary that handles load balancing and request routing
- `frpc/` - Client binary that connects to server and forwards to local services
- `common/` - Shared protocol definitions and utilities

### Core Architecture

**Communication Protocol**: The system uses a JSON-based command protocol over TCP with length-prefixed messages. All protocol definitions are in `common/src/lib.rs`.

**Four-Port Server Design**:
1. **Control Port** (17000): Persistent client connections for registration and command dispatch
2. **Proxy Port** (17001): Temporary connections for actual data forwarding
3. **Public Port** (18080): External users connect here
4. **API Port** (18081): RESTful HTTP API for monitoring and management

**Load Balancing Strategy**: Random selection from active clients using `rand::seq::SliceRandom::choose()` in `frps/src/main.rs:route_public_connection()`.

### Key Data Structures

**Server State** (`frps/src/main.rs`):
```rust
type ActiveClients = Arc<Mutex<HashMap<String, ClientInfo>>>;
type PendingConnections = Arc<Mutex<HashMap<String, TcpStream>>>;

struct ClientInfo {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    authed: bool,
    system_info: Option<SystemInfo>,
    connected_at: DateTime<Utc>,
    models: Option<Vec<Model>>,
}
```

**Command Protocol** (`common/src/lib.rs`):
- `Register` / `RegisterResult` - Client registration
- `RequestNewProxyConn` / `NewProxyConn` - Connection establishment
- `Login` / `LoginByToken` / `LoginResult` - Authentication
- `Heartbeat` / `SystemInfo` - Health monitoring

### Request Flow
1. External user connects to public port (18080)
2. Server randomly selects active client via `route_public_connection()`
3. Server generates unique `proxy_conn_id` and stores user connection in `pending_connections`
4. Server sends `RequestNewProxyConn` command to chosen client
5. Client connects to proxy port (17001) with `NewProxyConn` command
6. Server matches `proxy_conn_id` to pair connections
7. `join_streams()` utility handles bidirectional data forwarding

### Authentication System
- Email/password authentication on first run
- Token-based authentication stored in `token.json`
- Clients must authenticate before registration
- Built-in test credentials: `test@example.com` / `123456`

### Monitoring Features
- RESTful API for client status, system metrics, and server statistics
- Heartbeat monitoring with automatic client removal on disconnect
- System information reporting (CPU, memory, disk usage)
- Ollama model integration for AI-based routing capabilities

### Integration with External Services
- **Ollama Integration**: Clients can report available AI models via heartbeat messages
- **Local Service Forwarding**: Default target is `localhost:11434` (Ollama's default port)
- **HTTP Server Testing**: `test_frpx.sh` uses Python's built-in HTTP server for validation

## Important Implementation Details

- **Connection Management**: Server automatically removes disconnected clients from the active pool
- **Unique Client IDs**: Required for proper load balancing and client identification
- **Async Runtime**: Built on Tokio with extensive use of async/await
- **Error Handling**: Uses `anyhow` for error propagation throughout the codebase
- **Logging**: `tracing` crate for structured logging across all components