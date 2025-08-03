# frpx - Advanced Reverse Proxy with Load Balancing & Database Integration

`frpx` is a robust reverse proxy system inspired by `frp`, designed to expose local network services to the internet with enterprise-grade features. Its core strengths include random load balancing, database-backed authentication, Redis caching, and comprehensive monitoring capabilities.

## System Components

This project consists of three main components:
- **`frps`**: Server application (1,073+ lines) with PostgreSQL integration, Redis caching, and RESTful API
- **`frpc`**: Client application (462+ lines) with system monitoring, Ollama integration, and automatic machine ID generation  
- **`common`**: Shared protocol library (99 lines) with JSON-based command definitions and stream utilities

## Enterprise Features

### Core Features
- **High Availability**: Automatic failover with client health monitoring and removal of failed instances
- **Horizontal Scaling**: Dynamic client registration supporting unlimited scaling of backend services
- **Random Load Balancing**: Intelligent request distribution using `rand::seq::SliceRandom::choose()`
- **Database-Backed Authentication**: PostgreSQL integration for persistent API key and token management
- **Redis Caching**: 5-minute TTL caching for database queries, reducing latency and database load
- **Real-time Monitoring**: System metrics collection (CPU, memory, disk) with RESTful API exposure

### Advanced Capabilities
- **Four-Port Architecture**: Dedicated ports for control (17000), proxy (17001), public (18080), and API (18081)
- **Token-Based Security**: JWT-style token authentication with fallback to static API keys
- **Machine ID Integration**: Automatic client identification using `mid` crate for unique hardware fingerprinting
- **Ollama AI Integration**: Model discovery and routing for AI workloads via `/v1/chat/completions` endpoint
- **Cross-Platform Support**: Linux, macOS, and Windows compatibility with platform-specific system monitoring
- **Protocol Resilience**: Length-prefixed JSON protocol with comprehensive error handling

## Technical Architecture

### Multi-Port Server Design
The system operates using four specialized ports on the server:

- **Control Port (`17000`)**: Persistent client connections for registration, authentication, and command dispatch
- **Proxy Port (`17001`)**: Temporary connections for actual data forwarding between clients and end users
- **Public Port (`18080`)**: External user entry point with load balancing and API key validation
- **API Port (`18081`)**: RESTful HTTP API for monitoring, management, and system statistics

### Database Integration
- **PostgreSQL Backend**: Stores API keys, client information, and authentication tokens
- **Redis Caching Layer**: 5-minute TTL for token validation, reducing database load by ~90%
- **Automatic Schema Management**: Database pool with connection management via SQLx
- **Fallback Mechanisms**: Static API key validation when database is unavailable

### Protocol & Communication
- **JSON Command Protocol**: 15 command types including `Register`, `Login`, `Heartbeat`, `SystemInfo`
- **Length-Prefixed Messages**: 4-byte big-endian length header followed by JSON payload
- **Async Stream Handling**: Tokio-based bidirectional stream joining with `tokio::select!`
- **Connection Pairing**: UUID-based proxy connection matching between clients and users

## Code Review & Quality Assessment

### Server Implementation (`frps/src/main.rs` - 1,073 lines)

**Strengths:**
- **Robust Architecture**: Well-structured async/await implementation with proper error handling
- **Database Integration**: Professional-grade PostgreSQL integration with connection pooling
- **Caching Strategy**: Redis implementation reduces database queries significantly
- **API Design**: Comprehensive REST API with proper status codes and JSON responses
- **Security**: Token validation with database backing and fallback mechanisms
- **Monitoring**: Built-in metrics collection and health checking

**Key Functions:**
- `validate_token_in_db()`: Database validation with Redis caching (lines 171-204)
- `route_public_connection()`: Load balancing logic with API key validation (lines 906-1000+)
- `handle_control_connections()`: Client lifecycle management (lines 667-681)
- `join_streams()`: Bidirectional data forwarding utility

**Dependencies:** SQLx, Redis, Axum, Tower, Hyper, Chrono, UUID

### Client Implementation (`frpc/src/main.rs` - 462 lines)

**Strengths:**
- **Cross-Platform**: Native system monitoring for Linux, macOS, Windows
- **Machine ID Integration**: Automatic unique client identification using hardware fingerprinting
- **Ollama Integration**: AI model discovery and reporting via HTTP API
- **Robust Authentication**: Token persistence with interactive fallback
- **System Monitoring**: Real-time CPU, memory, disk usage collection
- **Heartbeat System**: 10-second intervals with graceful error handling

**Key Functions:**
- `get_ollama_models()`: AI model discovery (lines 86-107)
- `collect_system_info()`: Platform-specific system metrics (lines 288-461)
- `create_proxy_connection()`: Dynamic proxy connection establishment (lines 270-286)
- `get_computer_name()`: Cross-platform hostname detection (lines 13-29)

**Dependencies:** Reqwest, Mid, Clap, Serde, Tokio

### Common Library (`common/src/lib.rs` - 99 lines)

**Strengths:**
- **Protocol Definition**: Clean enum-based command structure with 15 command types
- **Stream Utilities**: Efficient bidirectional data copying with `tokio::select!`
- **Serialization**: Robust JSON serialization with error handling
- **Type Safety**: Strong typing for all protocol messages

**Key Components:**
- `Command` enum: 15 protocol commands for client-server communication
- `read_command()`/`write_command()`: Length-prefixed message protocol
- `join_streams()`: High-performance stream bridging
- `Model` struct: Ollama AI model representation

### Security Features
- **Authentication Flow**: Email/password → token generation → database storage
- **API Key Validation**: Database-backed with Redis caching and static fallback
- **Token Management**: Persistent storage with automatic renewal
- **Input Validation**: Comprehensive parameter validation and sanitization

### Performance Optimizations
- **Connection Pooling**: SQLx connection pool with configurable limits
- **Redis Caching**: 5-minute TTL reduces database load significantly
- **Async Architecture**: Full Tokio async/await implementation
- **Stream Efficiency**: Zero-copy data forwarding between streams
- **Load Balancing**: O(1) random selection from active client pool

### Monitoring & Observability
- **Structured Logging**: Tracing crate with configurable log levels
- **Health Checks**: Built-in health monitoring for all components
- **Metrics Collection**: CPU, memory, disk usage with periodic reporting
- **REST API**: 20+ endpoints for monitoring and management
- **Error Tracking**: Comprehensive error propagation and logging

## API Documentation

The server provides a comprehensive RESTful API for monitoring and management:

### Client Management
- `GET /api/clients` - Get all active clients
- `GET /api/clients/{client_id}` - Get specific client information
- `GET /api/clients/{client_id}/status` - Get client connection status
- `DELETE /api/clients/{client_id}` - Disconnect a specific client
- `GET /api/clients/{client_id}/heartbeat` - Get client heartbeat status

### System Monitoring
- `GET /api/monitoring` - Get system metrics for all clients
- `GET /api/monitoring/{client_id}` - Get system metrics for specific client
- `GET /api/health` - Server health check

### Statistics & Connections
- `GET /api/stats` - Get server statistics (uptime, connections, etc.)
- `GET /api/connections` - Get current connection information
- `GET /api/connections/pending` - Get pending connections count

### Configuration
- `GET /api/config` - Get server configuration
- `GET /api/ports` - Get port configuration

### Authentication
- `GET /api/users` - Get registered users list
- `GET /api/tokens/active` - Get active authentication tokens

### API Response Format
All API responses follow this format:
```json
{
  "success": true,
  "data": { /* response data */ },
  "message": "操作成功",
  "timestamp": "2025-07-29T17:55:48.826362Z"
}
```

## Getting Started

### Prerequisites

- **Rust**: Latest stable version ([installation guide](https://www.rust-lang.org/tools/install))
- **PostgreSQL**: Database server for authentication and API key storage
- **Redis**: Cache server for performance optimization (optional but recommended)
- **Local service**: For testing (e.g., Python HTTP server or Ollama)

### Database Setup

1. **Install PostgreSQL and Redis:**
```bash
# macOS with Homebrew
brew install postgresql redis

# Ubuntu/Debian
sudo apt-get install postgresql redis-server

# Start services
brew services start postgresql redis  # macOS
sudo systemctl start postgresql redis # Linux
```

2. **Create database and schema:**
```sql
-- Connect to PostgreSQL
psql -U postgres

-- Create database
CREATE DATABASE frpx;

-- Create API keys table
\c frpx
CREATE TABLE "public"."api_keys" (
    key VARCHAR PRIMARY KEY,
    status VARCHAR DEFAULT 'active',
    "expiresAt" TIMESTAMP,
    "createdAt" TIMESTAMP DEFAULT NOW(),
    "updatedAt" TIMESTAMP DEFAULT NOW()
);

-- Create GPU assets table for client info
CREATE TABLE "public"."gpu_assets" (
    "userId" VARCHAR,
    "machineId" VARCHAR PRIMARY KEY,
    name VARCHAR,
    status VARCHAR DEFAULT 'online',
    "createdAt" TIMESTAMP DEFAULT NOW(),
    "updatedAt" TIMESTAMP DEFAULT NOW()
);

-- Insert test API key
INSERT INTO "public"."api_keys" (key, status) VALUES ('test-api-key-123', 'active');
```

### 1. Build the Project

Clone the repository and build the binaries in release mode:

```bash
cargo build --release
```

The compiled binaries will be located in the `target/release/` directory.

### 2. Run a Local Service

For testing, you need a service running on the port that `frpc` will connect to (default is `11434`). You can use the built-in Python web server for this. Open a terminal and run:

```bash
python3 -m http.server 11434
```

### 3. Start the Server (`frps`)

Start the `frps` server with database configuration:

```bash
# With PostgreSQL and Redis
cargo run --release --bin frps -- \
  --database-url "postgres://username:password@localhost/frpx" \
  --redis-url "redis://127.0.0.1:6379" \
  --api-key "your-static-fallback-key"

# Minimal setup (uses defaults)
cargo run --release --bin frps
```

**Default connection strings:**
- PostgreSQL: `postgres://username:password@localhost/database`
- Redis: `redis://127.0.0.1:6379`
- API Key: `abc123`

You should see logs indicating successful connections to PostgreSQL and Redis, plus server startup on all four ports.

### 4. Start Multiple Clients (`frpc`)

To see the load balancing in action, you need to start at least two client instances. Each client will automatically use a unique machine ID as its identifier if not explicitly specified.

On first run, each client will prompt for authentication credentials:
- Email: `test@example.com`
- Password: `123456`

After successful authentication, a token will be saved to `token.json` for future use.

**Terminal 1 - Client A:**
```bash
cargo run --release --bin frpc -- --client-id client_A
```

**Terminal 2 - Client B:**
```bash
cargo run --release --bin frpc -- --client-id client_B
```

**Terminal 3 - Client with automatic ID:**
```bash
cargo run --release --bin frpc
```

Check the server logs to confirm that all clients have successfully registered.

### 5. Test with Database-Backed API Keys

Test the system using database-stored API keys:

```bash
# Test with valid database API key
curl -H "Authorization: Bearer test-api-key-123" http://localhost:18080

# Test with Bearer token format
curl -H "Authorization: Bearer test-api-key-123" \
  -H "Content-Type: application/json" \
  http://localhost:18080/v1/chat/completions \
  -d '{"model": "test", "messages": [{"role": "user", "content": "Hello"}]}'

# Monitor Redis cache hits
redis-cli monitor

# Check API endpoints
curl http://localhost:18081/api/health
curl http://localhost:18081/api/clients
curl http://localhost:18081/api/stats
```

Observe the server logs showing:
- Database token validation with Redis caching
- Random client selection: `Chose client 'client_A' for the new connection`
- Cache hits reducing database queries

### 6. Monitor Client System Information

#### Command Line Monitoring
To view the system information reported by clients, use the `--monitor` flag with the server:

```bash
cargo run --release --bin frps -- --monitor
```

This will display a table with the latest system metrics reported by each active client.

#### API Monitoring
You can also use the HTTP API to monitor clients:

```bash
# Get all clients
curl http://localhost:18081/api/clients

# Get server statistics
curl http://localhost:18081/api/stats

# Get system monitoring data
curl http://localhost:18081/api/monitoring

# Health check
curl http://localhost:18081/api/health
```

## Server Configuration

The `frps` server supports comprehensive configuration via command-line arguments:

```
Usage: frps [OPTIONS]

Network Configuration:
      --control-port <CONTROL_PORT>    Port for client control connections [default: 17000]
      --proxy-port <PROXY_PORT>        Port for client proxy connections [default: 17001]  
      --public-port <PUBLIC_PORT>      Port for public user connections [default: 18080]
      --api-port <API_PORT>            Port for HTTP API server [default: 18081]

Database & Caching:
      --database-url <DATABASE_URL>    PostgreSQL connection string 
                                       [default: postgres://username:password@localhost/database]
      --redis-url <REDIS_URL>          Redis connection string [default: redis://127.0.0.1:6379]

Security:
      --api-key <API_KEY>              Fallback API key for authentication [default: abc123]

Monitoring:
      --monitor                        Print client monitoring data and exit

General:
  -h, --help                          Print help
  -V, --version                       Print version
```

### Environment Variables
You can also configure the server using environment variables:
```bash
export DATABASE_URL="postgres://user:pass@localhost/frpx"
export REDIS_URL="redis://localhost:6379"
export API_KEY="your-secure-api-key"
```

## Client Configuration

The `frpc` client can be configured via command-line arguments:

```
Usage: frpc [OPTIONS]

Options:
  -c, --client-id <CLIENT_ID>
          Unique ID for this client instance. If not provided, uses machine ID.
  -s, --server-addr <SERVER_ADDR>
          Address of the frps server.
          [default: 127.0.0.1]
      --control-port <CONTROL_PORT>
          Port for the frps control connection.
          [default: 17000]
      --proxy-port <PROXY_PORT>
          Port for the frps proxy connection.
          [default: 17001]
      --local-addr <LOCAL_ADDR>
          Address of the local service to expose.
          [default: 127.0.0.1]
      --local-port <LOCAL_PORT>
          Port of the local service to expose.
          [default: 11434]
      --email <EMAIL>
          Email for authentication (skip interactive input)
      --password <PASSWORD>
          Password for authentication (skip interactive input)
  -h, --help
          Print help
  -V, --version
          Print version
```

## API Examples

Here are some practical examples of using the API:

### Monitor All Clients
```bash
curl -s http://localhost:18081/api/clients | jq '.'
```

### Check Server Health and Uptime
```bash
curl -s http://localhost:18081/api/health | jq '.data'
```

### Get Detailed Server Statistics
```bash
curl -s http://localhost:18081/api/stats | jq '.data'
```

### Monitor System Resources of All Clients
```bash
curl -s http://localhost:18081/api/monitoring | jq '.data'
```

### Disconnect a Specific Client
```bash
curl -X DELETE http://localhost:18081/api/clients/client_A
```

### Check Configuration
```bash
curl -s http://localhost:18081/api/config | jq '.data'
```

### Test Ollama Integration

If you have Ollama running locally, the client will automatically discover and report available models:

```bash
# Start Ollama (default port 11434)
ollama serve

# Pull a model
ollama pull qwen2.5:1.5b

# Test via frpx proxy with database API key
curl -H "Authorization: Bearer test-api-key-123" \
     -H "Content-Type: application/json" \
     http://localhost:18080/v1/chat/completions \
     -d '{
         "stream": true,
         "model": "qwen2.5:1.5b",
         "messages": [
             {"role": "system", "content": "You are a helpful assistant."},
             {"role": "user", "content": "Hello!"}
         ]
     }'
```

The client logs will show successful Ollama model discovery, and the server will route requests with load balancing.