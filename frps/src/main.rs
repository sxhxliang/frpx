use anyhow::{anyhow, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get},
    Router,
};
use chrono::{DateTime, Utc};
use clap::Parser;
use common::{read_command, write_command, join_streams, Command, Model};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres, Row};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::net::tcp::{OwnedWriteHalf, OwnedReadHalf};
use tokio::io::{AsyncWriteExt};
use tower_http::cors::CorsLayer;
use tracing::{info, warn, error, Level};
use uuid::Uuid;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 17000)]
    control_port: u16,

    #[arg(long, default_value_t = 17001)]
    proxy_port: u16,

    #[arg(long, default_value_t = 18080)]
    public_port: u16,
    
    #[arg(long, default_value_t = 18081)]
    api_port: u16,
    
    /// Print client monitoring data
    #[arg(long)]
    monitor: bool,
    
    /// API key for authentication
    #[arg(long, default_value = "abc123")]
    api_key: String,
    
    /// Database URL for PostgreSQL connection
    #[arg(long, default_value = "postgres://username:password@localhost/database")]
    database_url: String,
}

#[derive(Debug, Clone, Serialize)]
struct SystemInfo {
    cpu_usage: f32,
    memory_usage: f32,
    disk_usage: f32,
    last_heartbeat: std::time::SystemTime,
}

// API Response structures
#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    data: Option<T>,
    message: String,
    timestamp: DateTime<Utc>,
}

impl<T> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            message: "操作成功".to_string(),
            timestamp: Utc::now(),
        }
    }
    
    fn error(message: String) -> ApiResponse<()> {
        ApiResponse {
            success: false,
            data: None,
            message,
            timestamp: Utc::now(),
        }
    }
}

#[derive(Serialize)]
struct ClientInfoResponse {
    client_id: String,
    authed: bool,
    system_info: Option<SystemInfoResponse>,
    connected_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct SystemInfoResponse {
    cpu_usage: f32,
    memory_usage: f32,
    disk_usage: f32,
    last_heartbeat: DateTime<Utc>,
    heartbeat_seconds_ago: u64,
}

#[derive(Serialize)]
struct ServerStats {
    active_clients: usize,
    pending_connections: usize,
    total_connections: u64,
    uptime_seconds: u64,
}

#[derive(Serialize, Clone)]
struct ServerConfig {
    control_port: u16,
    proxy_port: u16,
    public_port: u16,
    api_port: u16,
}

#[derive(Serialize)]
struct HealthStatus {
    status: String,
    timestamp: DateTime<Utc>,
    uptime_seconds: u64,
}

#[derive(Deserialize)]
struct ChatCompletionRequest {
    model: String,
}

struct ClientInfo {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    authed: bool,
    system_info: Option<SystemInfo>,
    connected_at: DateTime<Utc>,
    models: Option<Vec<Model>>,
}

struct User {
    pass: String,
}

// Application State for API
#[derive(Clone)]
struct AppState {
    active_clients: ActiveClients,
    pending_connections: PendingConnections,
    user_db: UserDb,
    token_db: TokenDb,
    server_start_time: DateTime<Utc>,
    total_connections: Arc<Mutex<u64>>,
    config: ServerConfig,
    db_pool: Arc<Pool<Postgres>>,
}

type UserDb = Arc<Mutex<HashMap<String, User>>>;
type TokenDb = Arc<Mutex<HashMap<String, String>>>;
type ActiveClients = Arc<Mutex<HashMap<String, ClientInfo>>>;
type PendingConnections = Arc<Mutex<HashMap<String, TcpStream>>>;

// Database functions
async fn validate_token_in_db(pool: &Pool<Postgres>, token: &str) -> Result<bool> {
    let row = sqlx::query(
        "SELECT key FROM \"public\".\"api_keys\" WHERE key = $1 AND status = 'active' AND (\"expiresAt\" IS NULL OR \"expiresAt\" > NOW())"
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    
    Ok(row.is_some())
}

async fn upsert_client_info(pool: &Pool<Postgres>, user_id: &str, machine_id: &str, name: &str, status: &str) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO "public"."gpu_assets" ("userId", "machineId", "name", "createdAt", "updatedAt")
        VALUES ($1, $2, $3, NOW(), NOW())
        ON CONFLICT ("machineId")
        DO UPDATE SET
            "name" = EXCLUDED."name",
            "status" = 'online'::gpu_asset_status,
            "updatedAt" = NOW();
        "#
    )
    .bind(user_id)
    .bind(machine_id)
    .bind(name)
    // .bind(status)
    .execute(pool)
    .await?;
    
    Ok(())
}

// API Handlers

// Client Query APIs
async fn get_all_clients(State(app_state): State<AppState>) -> Result<Json<ApiResponse<Vec<ClientInfoResponse>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    let mut client_responses = Vec::new();
    
    for (client_id, client_info) in clients.iter() {
        let system_info_response = client_info.system_info.as_ref().map(|sys_info| {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            SystemInfoResponse {
                cpu_usage: sys_info.cpu_usage,
                memory_usage: sys_info.memory_usage,
                disk_usage: sys_info.disk_usage,
                last_heartbeat: DateTime::from_timestamp(
                    sys_info.last_heartbeat.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::from_secs(0)).as_secs() as i64, 0
                ).unwrap_or(Utc::now()),
                heartbeat_seconds_ago: heartbeat_duration.as_secs(),
            }
        });
        
        client_responses.push(ClientInfoResponse {
            client_id: client_id.clone(),
            authed: client_info.authed,
            system_info: system_info_response,
            connected_at: client_info.connected_at,
        });
    }
    
    Ok(Json(ApiResponse::success(client_responses)))
}

async fn get_client_by_id(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<ClientInfoResponse>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    
    if let Some(client_info) = clients.get(&client_id) {
        let system_info_response = client_info.system_info.as_ref().map(|sys_info| {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            SystemInfoResponse {
                cpu_usage: sys_info.cpu_usage,
                memory_usage: sys_info.memory_usage,
                disk_usage: sys_info.disk_usage,
                last_heartbeat: DateTime::from_timestamp(
                    sys_info.last_heartbeat.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::from_secs(0)).as_secs() as i64, 0
                ).unwrap_or(Utc::now()),
                heartbeat_seconds_ago: heartbeat_duration.as_secs(),
            }
        });
        
        let response = ClientInfoResponse {
            client_id: client_id.clone(),
            authed: client_info.authed,
            system_info: system_info_response,
            connected_at: client_info.connected_at,
        };
        
        Ok(Json(ApiResponse::success(response)))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_client_status(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<HashMap<String, serde_json::Value>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    
    if let Some(client_info) = clients.get(&client_id) {
        let mut status = HashMap::new();
        status.insert("client_id".to_string(), serde_json::Value::String(client_id));
        status.insert("connected".to_string(), serde_json::Value::Bool(true));
        status.insert("authenticated".to_string(), serde_json::Value::Bool(client_info.authed));
        status.insert("connected_at".to_string(), serde_json::Value::String(client_info.connected_at.to_rfc3339()));
        
        if let Some(sys_info) = &client_info.system_info {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            status.insert("last_heartbeat_seconds_ago".to_string(), serde_json::Value::Number(heartbeat_duration.as_secs().into()));
        }
        
        Ok(Json(ApiResponse::success(status)))
    } else {
        let mut status = HashMap::new();
        status.insert("client_id".to_string(), serde_json::Value::String(client_id));
        status.insert("connected".to_string(), serde_json::Value::Bool(false));
        Ok(Json(ApiResponse::success(status)))
    }
}

// System Monitoring APIs
async fn get_monitoring_data(State(app_state): State<AppState>) -> Result<Json<ApiResponse<Vec<SystemInfoResponse>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    let mut monitoring_data = Vec::new();
    
    for (_client_id, client_info) in clients.iter() {
        if let Some(sys_info) = &client_info.system_info {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            monitoring_data.push(SystemInfoResponse {
                cpu_usage: sys_info.cpu_usage,
                memory_usage: sys_info.memory_usage,
                disk_usage: sys_info.disk_usage,
                last_heartbeat: DateTime::from_timestamp(
                    sys_info.last_heartbeat.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::from_secs(0)).as_secs() as i64, 0
                ).unwrap_or(Utc::now()),
                heartbeat_seconds_ago: heartbeat_duration.as_secs(),
            });
        }
    }
    
    Ok(Json(ApiResponse::success(monitoring_data)))
}

async fn get_client_monitoring(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<SystemInfoResponse>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    
    if let Some(client_info) = clients.get(&client_id) {
        if let Some(sys_info) = &client_info.system_info {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            let response = SystemInfoResponse {
                cpu_usage: sys_info.cpu_usage,
                memory_usage: sys_info.memory_usage,
                disk_usage: sys_info.disk_usage,
                last_heartbeat: DateTime::from_timestamp(
                    sys_info.last_heartbeat.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or(std::time::Duration::from_secs(0)).as_secs() as i64, 0
                ).unwrap_or(Utc::now()),
                heartbeat_seconds_ago: heartbeat_duration.as_secs(),
            };
            
            Ok(Json(ApiResponse::success(response)))
        } else {
            Err(StatusCode::NOT_FOUND)
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_health() -> Json<ApiResponse<HealthStatus>> {
    let health = HealthStatus {
        status: "healthy".to_string(),
        timestamp: Utc::now(),
        uptime_seconds: 0, // Will be calculated in main
    };
    
    Json(ApiResponse::success(health))
}

// Client Management APIs
async fn disconnect_client(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<HashMap<String, String>>>, StatusCode> {
    let mut clients = app_state.active_clients.lock().await;
    
    if clients.remove(&client_id).is_some() {
        let mut response = HashMap::new();
        response.insert("client_id".to_string(), client_id);
        response.insert("action".to_string(), "disconnected".to_string());
        Ok(Json(ApiResponse::success(response)))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_client_heartbeat(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<HashMap<String, serde_json::Value>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    
    if let Some(client_info) = clients.get(&client_id) {
        let mut heartbeat_info = HashMap::new();
        heartbeat_info.insert("client_id".to_string(), serde_json::Value::String(client_id));
        
        if let Some(sys_info) = &client_info.system_info {
            let heartbeat_duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            heartbeat_info.insert("last_heartbeat_seconds_ago".to_string(), serde_json::Value::Number(heartbeat_duration.as_secs().into()));
            heartbeat_info.insert("status".to_string(), serde_json::Value::String(
                if heartbeat_duration.as_secs() < 60 { "healthy" } else { "stale" }.to_string()
            ));
        } else {
            heartbeat_info.insert("status".to_string(), serde_json::Value::String("no_data".to_string()));
        }
        
        Ok(Json(ApiResponse::success(heartbeat_info)))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_client_models(
    Path(client_id): Path<String>,
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<Vec<Model>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    if let Some(client_info) = clients.get(&client_id) {
        if let Some(models) = &client_info.models {
            Ok(Json(ApiResponse::success(models.clone())))
        } else {
            Ok(Json(ApiResponse::success(vec![])))
        }
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn get_all_models(
    State(app_state): State<AppState>
) -> Result<Json<ApiResponse<HashMap<String, Vec<Model>>>>, StatusCode> {
    let clients = app_state.active_clients.lock().await;
    let mut all_models = HashMap::new();
    for (client_id, client_info) in clients.iter() {
        if let Some(models) = &client_info.models {
            all_models.insert(client_id.clone(), models.clone());
        }
    }
    Ok(Json(ApiResponse::success(all_models)))
}


// Connection Statistics APIs
async fn get_stats(State(app_state): State<AppState>) -> Json<ApiResponse<ServerStats>> {
    let clients = app_state.active_clients.lock().await;
    let pending = app_state.pending_connections.lock().await;
    let total_connections = *app_state.total_connections.lock().await;
    
    let uptime_seconds = Utc::now().signed_duration_since(app_state.server_start_time).num_seconds() as u64;
    
    let stats = ServerStats {
        active_clients: clients.len(),
        pending_connections: pending.len(),
        total_connections,
        uptime_seconds,
    };
    
    Json(ApiResponse::success(stats))
}

async fn get_connections(State(app_state): State<AppState>) -> Json<ApiResponse<HashMap<String, serde_json::Value>>> {
    let clients = app_state.active_clients.lock().await;
    let pending = app_state.pending_connections.lock().await;
    
    let mut connections = HashMap::new();
    connections.insert("active_clients".to_string(), serde_json::Value::Number(clients.len().into()));
    connections.insert("pending_connections".to_string(), serde_json::Value::Number(pending.len().into()));
    
    let mut client_list = Vec::new();
    for client_id in clients.keys() {
        client_list.push(serde_json::Value::String(client_id.clone()));
    }
    connections.insert("client_ids".to_string(), serde_json::Value::Array(client_list));
    
    Json(ApiResponse::success(connections))
}

async fn get_pending_connections(State(app_state): State<AppState>) -> Json<ApiResponse<HashMap<String, serde_json::Value>>> {
    let pending = app_state.pending_connections.lock().await;
    
    let mut response = HashMap::new();
    response.insert("count".to_string(), serde_json::Value::Number(pending.len().into()));
    
    let mut pending_list = Vec::new();
    for conn_id in pending.keys() {
        pending_list.push(serde_json::Value::String(conn_id.clone()));
    }
    response.insert("connection_ids".to_string(), serde_json::Value::Array(pending_list));
    
    Json(ApiResponse::success(response))
}

// Configuration Management APIs
async fn get_config(State(app_state): State<AppState>) -> Json<ApiResponse<ServerConfig>> {
    Json(ApiResponse::success(app_state.config))
}

async fn get_ports(State(app_state): State<AppState>) -> Json<ApiResponse<HashMap<String, u16>>> {
    let mut ports = HashMap::new();
    ports.insert("control_port".to_string(), app_state.config.control_port);
    ports.insert("proxy_port".to_string(), app_state.config.proxy_port);
    ports.insert("public_port".to_string(), app_state.config.public_port);
    ports.insert("api_port".to_string(), app_state.config.api_port);
    
    Json(ApiResponse::success(ports))
}

// Authentication Management APIs
async fn get_users(State(app_state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let users = app_state.user_db.lock().await;
    let user_list: Vec<String> = users.keys().cloned().collect();
    
    Json(ApiResponse::success(user_list))
}

async fn get_active_tokens(State(app_state): State<AppState>) -> Json<ApiResponse<HashMap<String, serde_json::Value>>> {
    let tokens = app_state.token_db.lock().await;
    
    let mut response = HashMap::new();
    response.insert("active_token_count".to_string(), serde_json::Value::Number(tokens.len().into()));
    
    let mut token_info = Vec::new();
    for (token, email) in tokens.iter() {
        let mut info = HashMap::new();
        info.insert("token_prefix".to_string(), serde_json::Value::String(format!("{}...", &token[..8])));
        info.insert("email".to_string(), serde_json::Value::String(email.clone()));
        token_info.push(serde_json::Value::Object(info.into_iter().collect()));
    }
    response.insert("tokens".to_string(), serde_json::Value::Array(token_info));
    
    Json(ApiResponse::success(response))
}

// Create API Router
fn create_api_router(app_state: AppState) -> Router {
    Router::new()
        // Client Query APIs
        .route("/api/clients", get(get_all_clients))
        .route("/api/clients/:client_id", get(get_client_by_id))
        .route("/api/clients/:client_id/status", get(get_client_status))
        
        // System Monitoring APIs
        .route("/api/monitoring", get(get_monitoring_data))
        .route("/api/monitoring/:client_id", get(get_client_monitoring))
        .route("/api/health", get(get_health))
        
        // Client Management APIs
        .route("/api/clients/:client_id", delete(disconnect_client))
        .route("/api/clients/:client_id/heartbeat", get(get_client_heartbeat))
        .route("/api/clients/:client_id/models", get(get_client_models))
        .route("/api/models", get(get_all_models))
        
        // Connection Statistics APIs
        .route("/api/stats", get(get_stats))
        .route("/api/connections", get(get_connections))
        .route("/api/connections/pending", get(get_pending_connections))
        
        // Configuration Management APIs
        .route("/api/config", get(get_config))
        .route("/api/ports", get(get_ports))
        
        // Authentication Management APIs
        .route("/api/users", get(get_users))
        .route("/api/tokens/active", get(get_active_tokens))
        
        .layer(CorsLayer::permissive())
        .with_state(app_state)
}

async fn run_api_server(app_state: AppState, port: u16) -> Result<()> {
    let app = create_api_router(app_state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    
    info!("API server listening on port {}", port);
    
    axum::serve(listener, app).await.map_err(Into::into)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    // Initialize database pool
    let db_pool = Arc::new(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .connect(&args.database_url)
            .await?
    );
    
    info!("Connected to database successfully");

    let active_clients: ActiveClients = Arc::new(Mutex::new(HashMap::new()));
    let pending_connections: PendingConnections = Arc::new(Mutex::new(HashMap::new()));
    let user_db: UserDb = Arc::new(Mutex::new(HashMap::from([
        ("test@example.com".to_string(), User { pass: "123456".to_string() }),
    ])));
    let token_db: TokenDb = Arc::new(Mutex::new(HashMap::new()));
    let total_connections = Arc::new(Mutex::new(0u64));
    let server_start_time = Utc::now();

    // Create application state for API
    let app_state = AppState {
        active_clients: active_clients.clone(),
        pending_connections: pending_connections.clone(),
        user_db: user_db.clone(),
        token_db: token_db.clone(),
        server_start_time,
        total_connections: total_connections.clone(),
        config: ServerConfig {
            control_port: args.control_port,
            proxy_port: args.proxy_port,
            public_port: args.public_port,
            api_port: args.api_port,
        },
        db_pool: db_pool.clone(),
    };

    let control_listener = TcpListener::bind(format!("0.0.0.0:{}", args.control_port)).await?;
    let proxy_listener = TcpListener::bind(format!("0.0.0.0:{}", args.proxy_port)).await?;
    let public_listener = TcpListener::bind(format!("0.0.0.0:{}", args.public_port)).await?;

    info!("FRPS listening on ports: Control={}, Proxy={}, Public={}, API={}", 
          args.control_port, args.proxy_port, args.public_port, args.api_port);

    // If monitor flag is set, just print monitoring data and exit
    if args.monitor {
        print_monitoring_data(active_clients.clone()).await;
        return Ok(());
    }
    
    let server_logic = tokio::select! {
        res = handle_control_connections(control_listener, active_clients.clone(), user_db, token_db, db_pool.clone()) => res,
        res = handle_proxy_connections(proxy_listener, pending_connections.clone()) => res,
        res = handle_public_connections(public_listener, active_clients.clone(), pending_connections.clone(), total_connections.clone(), args.api_key.clone()) => res,
        res = run_api_server(app_state, args.api_port) => res,
    };

    if let Err(e) = server_logic {
        error!("Server error: {}", e);
    }

    Ok(())
}

async fn handle_control_connections(listener: TcpListener, active_clients: ActiveClients, user_db: UserDb, token_db: TokenDb, db_pool: Arc<Pool<Postgres>>) -> Result<()> {
    loop {
        let (stream, addr) = listener.accept().await?;
        info!("New control connection from: {}", addr);
        let active_clients_clone = active_clients.clone();
        let user_db_clone = user_db.clone();
        let token_db_clone = token_db.clone();
        let db_pool_clone = db_pool.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_single_client(stream, active_clients_clone, user_db_clone, token_db_clone, db_pool_clone).await {
                error!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

async fn handle_single_client(stream: TcpStream, active_clients: ActiveClients, user_db: UserDb, token_db: TokenDb, db_pool: Arc<Pool<Postgres>>) -> Result<()> {
    let (mut reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let mut authed = false;

    match read_command(&mut reader).await? {
        Command::Login { email, pass } => {
            let users = user_db.lock().await;
            if let Some(user) = users.get(&email) {
                if user.pass == pass {
                    let token = Uuid::new_v4().to_string();
                    let mut tokens = token_db.lock().await;
                    tokens.insert(token.clone(), email.clone());
                    let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: true, error: None, token: Some(token) }).await;
                    authed = true;
                } else {
                    let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: false, error: Some("Invalid password".to_string()), token: None }).await;
                }
            } else {
                let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: false, error: Some("User not found".to_string()), token: None }).await;
            }
        }
        Command::LoginByToken { token } => {
            match validate_token_in_db(&db_pool, &token).await {
                Ok(is_valid) => {
                    if is_valid {
                        let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: true, error: None, token: None }).await;
                        authed = true;
                    } else {
                        let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: false, error: Some("Invalid token".to_string()), token: None }).await;
                    }
                }
                Err(e) => {
                    error!("Database error during token validation: {}", e);
                    let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: false, error: Some("Database error".to_string()), token: None }).await;
                }
            }
        }
        _ => {
            return Err(anyhow!("First command was not a login command"));
        }
    }

    if !authed {
        return Ok(());
    }

    let client_id = if let Command::Register { client_id: id } = read_command(&mut reader).await? {
        info!("Registration attempt for client_id: {}", id);
        let mut clients = active_clients.lock().await;
        if clients.contains_key(&id) {
            warn!("Client ID {} already registered.", id);
            let _ = write_command(&mut *writer.lock().await, &Command::RegisterResult { success: false, error: Some("Client ID already in use".to_string()) }).await;
            return Err(anyhow!("Client ID already registered"));
        }

        clients.insert(id.clone(), ClientInfo {
            writer: writer.clone(),
            authed,
            system_info: None,
            connected_at: Utc::now(),
            models: None,
        });
        let _ = write_command(&mut *writer.lock().await, &Command::RegisterResult { success: true, error: None }).await;
        info!("Client {} registered successfully.", id);
        id
    } else {
        return Err(anyhow!("Second command was not Register"));
    };

    client_loop(&mut reader, client_id, active_clients, db_pool).await
}

async fn client_loop(reader: &mut OwnedReadHalf, client_id: String, active_clients: ActiveClients, db_pool: Arc<Pool<Postgres>>) -> Result<()> {
    loop {
        match read_command(reader).await {
            Ok(Command::Heartbeat { models }) => {
                let model_count = models.as_ref().map_or(0, |m| m.len());
                info!("Received heartbeat from client {} with {} models", client_id, model_count);
                let mut clients = active_clients.lock().await;
                if let Some(client_info) = clients.get_mut(&client_id) {
                    client_info.models = models;
                    if let Some(ref mut sys_info) = client_info.system_info {
                        sys_info.last_heartbeat = std::time::SystemTime::now();
                    } else {
                        client_info.system_info = Some(SystemInfo {
                            cpu_usage: 0.0,
                            memory_usage: 0.0,
                            disk_usage: 0.0,
                            last_heartbeat: std::time::SystemTime::now(),
                        });
                    }
                }
            }
            Ok(Command::SystemInfo { cpu_usage, memory_usage, disk_usage, computer_name }) => {
                info!("Received system info from client {}: CPU: {:.2}%, Memory: {:.2}%, Disk: {:.2}%, Computer: {}", 
                      client_id, cpu_usage, memory_usage, disk_usage, computer_name);
                
                // Store client info in database
                let user_id = "S70Nu1PGu1WYU4EbzePOJA9HsFsRspIQ";
                if let Err(e) = upsert_client_info(&db_pool, user_id, &client_id, &computer_name, "online").await {
                    error!("Failed to store client info in database: {}", e);
                }
                
                // Update system info in memory
                let mut clients = active_clients.lock().await;
                if let Some(client_info) = clients.get_mut(&client_id) {
                    client_info.system_info = Some(SystemInfo {
                        cpu_usage,
                        memory_usage,
                        disk_usage,
                        last_heartbeat: std::time::SystemTime::now(),
                    });
                }
            }
            Ok(cmd) => {
                warn!("Received unexpected command: {:?}", cmd);
            }
            Err(_) => {
                warn!("Client {} disconnected.", client_id);
                
                // Update client status in database to offline
                if let Err(e) = sqlx::query("UPDATE \"public\".\"gpu_assets\" SET status = 'offline', \"updatedAt\" = NOW() WHERE \"machineId\" = $1")
                    .bind(&client_id)
                    .execute(&*db_pool)
                    .await {
                    error!("Failed to update client status to offline in database: {}", e);
                }
                
                active_clients.lock().await.remove(&client_id);
                break;
            }
        }
    }
    Ok(())
}

async fn handle_proxy_connections(listener: TcpListener, pending_connections: PendingConnections) -> Result<()> {
    loop {
        let (mut proxy_stream, addr) = listener.accept().await?;
        info!("New proxy connection from: {}", addr);
        let pending_clone = pending_connections.clone();
        tokio::spawn(async move {
            if let Ok(Command::NewProxyConn { proxy_conn_id }) = read_command(&mut proxy_stream).await {
                info!("Received proxy conn notification for id: {}", proxy_conn_id);
                let mut pending = pending_clone.lock().await;
                if let Some(user_stream) = pending.remove(&proxy_conn_id) {
                    info!("Pairing user stream with proxy stream for id: {}", proxy_conn_id);
                    tokio::spawn(async move {
                        if let Err(e) = join_streams(user_stream, proxy_stream).await {
                            error!("Error joining streams: {}", e);
                        }
                        info!("Streams for {} joined and finished.", proxy_conn_id);
                    });
                } else {
                    warn!("No pending user connection found for proxy_conn_id: {}", proxy_conn_id);
                }
            } else {
                error!("Failed to read NewProxyConn command from {}", addr);
            }
        });
    }
}

async fn handle_public_connections(listener: TcpListener, active_clients: ActiveClients, pending_connections: PendingConnections, total_connections: Arc<Mutex<u64>>, api_key: String) -> Result<()> {
    loop {
        let (user_stream, addr) = listener.accept().await?;
        info!("New public connection from: {}", addr);
        let active_clients_clone = active_clients.clone();
        let pending_connections_clone = pending_connections.clone();
        let total_connections_clone = total_connections.clone();
        let api_key = api_key.clone();

        tokio::spawn(async move {
            // Increment total connections counter
            {
                let mut counter = total_connections_clone.lock().await;
                *counter += 1;
            }
            
            if let Err(e) = route_public_connection(user_stream, active_clients_clone, pending_connections_clone, api_key.clone()).await {
                error!("Failed to route public connection from {}: {}", addr, e);
            }
        });
    }
}

async fn send_http_error_response(mut stream: TcpStream, status_code: u16, error_message: &str) -> Result<()> {
    let error_response = ApiResponse::<()>::error(error_message.to_string());
    let json_body = serde_json::to_string(&error_response)?;
    
    let status_text = match status_code {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        500 => "Internal Server Error",
        _ => "Error",
    };
    
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_code, status_text, json_body.len(), json_body
    );
    
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn find_client_by_model(model_name: &str, clients: &mut HashMap<String, ClientInfo>) -> Option<String> {
    for (client_id, client_info) in clients.iter() {
        if let Some(models) = &client_info.models {
            if models.iter().any(|m| m.id == model_name) {
                return Some(client_id.clone());
            }
        }
    }
    None
}

async fn route_public_connection(user_stream: TcpStream, active_clients: ActiveClients, pending_connections: PendingConnections, api_key: String) -> Result<()> {
    let mut buffer = [0; 4096];
    let n = user_stream.peek(&mut buffer).await?;
    let initial_data = &buffer[..n];

    let mut headers = [httparse::EMPTY_HEADER; 100];
    let mut req = httparse::Request::new(&mut headers);

    let chosen_client_id = if let Ok(httparse::Status::Complete(parsed_len)) = req.parse(initial_data) {
        // Validate API key from Authorization header
        let auth_header = req.headers.iter()
            .find(|h| h.name.to_lowercase() == "authorization")
            .and_then(|h| std::str::from_utf8(h.value).ok());
        
        if let Some(auth_value) = auth_header {
            // Support both "Bearer <token>" and plain token formats
            let provided_key = if auth_value.to_lowercase().starts_with("bearer ") {
                &auth_value[7..] // Remove "Bearer " prefix
            } else {
                auth_value
            };
            
            if provided_key != api_key {
                warn!("Invalid API key provided in Authorization header");
                if let Err(e) = send_http_error_response(user_stream, 401, "Invalid API key").await {
                    error!("Failed to send error response: {}", e);
                }
                return Ok(());
            }
        } else {
            warn!("No Authorization header found");
            if let Err(e) = send_http_error_response(user_stream, 401, "Missing API key in Authorization header").await {
                error!("Failed to send error response: {}", e);
            }
            return Ok(());
        }
        
        let mut clients = active_clients.lock().await;
        if req.method == Some("POST") && req.path == Some("/v1/chat/completions") {
            let body_offset = parsed_len;
            let body_bytes = &initial_data[body_offset..];

            // It's tricky to get the full body here as it might not be in the first packet.
            // For now, we assume the relevant part of the JSON is in the first packet.
            // A more robust solution would involve a proper body reading loop.
            if let Ok(body_str) = std::str::from_utf8(body_bytes) {
                 if let Ok(chat_req) = serde_json::from_str::<ChatCompletionRequest>(body_str) {
                    if let Some(client_id) = find_client_by_model(&chat_req.model, &mut clients).await {
                        info!("Found client '{}' for model '{}'", client_id, chat_req.model);
                        Some(client_id)
                    } else {
                       warn!("No client found for model '{}'. Falling back to random.", chat_req.model);
                       None
                    }
                 } else {
                    warn!("Could not parse chat completion body. Falling back to random.");
                    None
                 }
            } else {
                warn!("Could not parse body as UTF-8. Falling back to random.");
                None
            }
        } else {
            // Not a chat completion request, proceed with random selection
            None
        }
    } else {
        // Not a complete HTTP request in the first packet, or not HTTP at all.
        // For security, we require proper HTTP requests with API key validation
        warn!("Received non-HTTP or incomplete HTTP request");
        if let Err(e) = send_http_error_response(user_stream, 400, "Invalid HTTP request format").await {
            error!("Failed to send error response: {}", e);
        }
        return Ok(());
    };

    let mut clients = active_clients.lock().await;
    let chosen_client_id = if let Some(id) = chosen_client_id {
        id
    } else {
        // This should only happen for non-chat completion requests that passed API key validation
        let client_ids: Vec<String> = clients.keys().cloned().collect();
        if client_ids.is_empty() {
            warn!("No active clients available to handle new public connection.");
            if let Err(e) = send_http_error_response(user_stream, 503, "No active clients available").await {
                error!("Failed to send error response: {}", e);
            }
            return Ok(());
        }
        client_ids.choose(&mut rand::thread_rng()).ok_or_else(|| anyhow!("Failed to choose a client"))?.clone()
    };

    info!("Chose client '{}' for the new connection.", chosen_client_id);

    if let Some(client_info) = clients.get(&chosen_client_id) {
        if !client_info.authed {
            return Err(anyhow!("Chosen client not authenticated"));
        }
        let proxy_conn_id = Uuid::new_v4().to_string();
        let command = Command::RequestNewProxyConn { proxy_conn_id: proxy_conn_id.clone() };

        info!("Requesting new proxy connection with id: {}", proxy_conn_id);
        pending_connections.lock().await.insert(proxy_conn_id.clone(), user_stream);

        let mut writer = client_info.writer.lock().await;
        if let Err(e) = write_command(&mut *writer, &command).await {
            error!("Failed to send RequestNewProxyConn to client {}: {}. Removing from active list.", chosen_client_id, e);
            drop(writer);
            clients.remove(&chosen_client_id);
            pending_connections.lock().await.remove(&proxy_conn_id);
            return Err(e);
        }
        info!("Successfully sent RequestNewProxyConn to client {}", chosen_client_id);
    } else {
        error!("Chosen client {} not found in active list.", chosen_client_id);
        return Err(anyhow!("Chosen client disappeared"));
    }

    Ok(())
}

async fn print_monitoring_data(active_clients: ActiveClients) {
    let clients = active_clients.lock().await;
    if clients.is_empty() {
        println!("No active clients.");
        return;
    }
    
    println!("Client Monitoring Data:");
    println!("{:<20} {:<10} {:<10} {:<10} {:<20}", "Client ID", "CPU (%)", "Memory (%)", "Disk (%)", "Last Heartbeat");
    println!("{}", "-".repeat(80));
    
    for (client_id, client_info) in clients.iter() {
        if let Some(sys_info) = &client_info.system_info {
            let duration = sys_info.last_heartbeat.elapsed().unwrap_or(std::time::Duration::from_secs(0));
            let seconds = duration.as_secs();
            println!("{:<20} {:<10.2} {:<10.2} {:<10.2} {:<20}", 
                     client_id, 
                     sys_info.cpu_usage, 
                     sys_info.memory_usage, 
                     sys_info.disk_usage,
                     format!("{}s ago", seconds));
        } else {
            println!("{:<20} {:<10} {:<10} {:<10} {:<20}", 
                     client_id, 
                     "N/A", 
                     "N/A", 
                     "N/A",
                     "No data");
        }
    }
}