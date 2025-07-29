use anyhow::{anyhow, Result};
use clap::Parser;
use common::{read_command, write_command, join_streams, Command};
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::net::tcp::{OwnedWriteHalf, OwnedReadHalf};
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
    
    /// Print client monitoring data
    #[arg(long)]
    monitor: bool,
}

#[derive(Debug, Clone)]
struct SystemInfo {
    cpu_usage: f32,
    memory_usage: f32,
    disk_usage: f32,
    last_heartbeat: std::time::SystemTime,
}

struct ClientInfo {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    authed: bool,
    system_info: Option<SystemInfo>,
}

struct User {
    pass: String,
}

type UserDb = Arc<Mutex<HashMap<String, User>>>;
type TokenDb = Arc<Mutex<HashMap<String, String>>>;
type ActiveClients = Arc<Mutex<HashMap<String, ClientInfo>>>;
type PendingConnections = Arc<Mutex<HashMap<String, TcpStream>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    let active_clients: ActiveClients = Arc::new(Mutex::new(HashMap::new()));
    let pending_connections: PendingConnections = Arc::new(Mutex::new(HashMap::new()));
    let user_db: UserDb = Arc::new(Mutex::new(HashMap::from([
        ("test@example.com".to_string(), User { pass: "123456".to_string() }),
    ])));
    let token_db: TokenDb = Arc::new(Mutex::new(HashMap::new()));

    let control_listener = TcpListener::bind(format!("0.0.0.0:{}", args.control_port)).await?;
    let proxy_listener = TcpListener::bind(format!("0.0.0.0:{}", args.proxy_port)).await?;
    let public_listener = TcpListener::bind(format!("0.0.0.0:{}", args.public_port)).await?;

    info!("FRPS listening on ports: Control={}, Proxy={}, Public={}", args.control_port, args.proxy_port, args.public_port);

    // If monitor flag is set, just print monitoring data and exit
    if args.monitor {
        print_monitoring_data(active_clients.clone()).await;
        return Ok(());
    }
    
    let server_logic = tokio::select! {
        res = handle_control_connections(control_listener, active_clients.clone(), user_db, token_db) => res,
        res = handle_proxy_connections(proxy_listener, pending_connections.clone()) => res,
        res = handle_public_connections(public_listener, active_clients.clone(), pending_connections.clone()) => res,
    };

    if let Err(e) = server_logic {
        error!("Server error: {}", e);
    }

    Ok(())
}

async fn handle_control_connections(listener: TcpListener, active_clients: ActiveClients, user_db: UserDb, token_db: TokenDb) -> Result<()> {
    loop {
        let (stream, addr) = listener.accept().await?;
        info!("New control connection from: {}", addr);
        let active_clients_clone = active_clients.clone();
        let user_db_clone = user_db.clone();
        let token_db_clone = token_db.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_single_client(stream, active_clients_clone, user_db_clone, token_db_clone).await {
                error!("Error handling client {}: {}", addr, e);
            }
        });
    }
}

async fn handle_single_client(stream: TcpStream, active_clients: ActiveClients, user_db: UserDb, token_db: TokenDb) -> Result<()> {
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
            let tokens = token_db.lock().await;
            if tokens.contains_key(&token) {
                let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: true, error: None, token: None }).await;
                authed = true;
            } else {
                let _ = write_command(&mut *writer.lock().await, &Command::LoginResult { success: false, error: Some("Invalid token".to_string()), token: None }).await;
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
        });
        let _ = write_command(&mut *writer.lock().await, &Command::RegisterResult { success: true, error: None }).await;
        info!("Client {} registered successfully.", id);
        id
    } else {
        return Err(anyhow!("Second command was not Register"));
    };

    client_loop(&mut reader, client_id, active_clients).await
}

async fn client_loop(reader: &mut OwnedReadHalf, client_id: String, active_clients: ActiveClients) -> Result<()> {
    loop {
        match read_command(reader).await {
            Ok(Command::Heartbeat) => {
                info!("Received heartbeat from client {}", client_id);
                // Update last heartbeat time
                let mut clients = active_clients.lock().await;
                if let Some(client_info) = clients.get_mut(&client_id) {
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
            Ok(Command::SystemInfo { cpu_usage, memory_usage, disk_usage }) => {
                info!("Received system info from client {}: CPU: {:.2}%, Memory: {:.2}%, Disk: {:.2}%", 
                      client_id, cpu_usage, memory_usage, disk_usage);
                // Update system info
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

async fn handle_public_connections(listener: TcpListener, active_clients: ActiveClients, pending_connections: PendingConnections) -> Result<()> {
    loop {
        let (user_stream, addr) = listener.accept().await?;
        info!("New public connection from: {}", addr);
        let active_clients_clone = active_clients.clone();
        let pending_connections_clone = pending_connections.clone();

        tokio::spawn(async move {
            if let Err(e) = route_public_connection(user_stream, active_clients_clone, pending_connections_clone).await {
                error!("Failed to route public connection from {}: {}", addr, e);
            }
        });
    }
}

async fn route_public_connection(user_stream: TcpStream, active_clients: ActiveClients, pending_connections: PendingConnections) -> Result<()> {
    let mut clients = active_clients.lock().await;
    let client_ids: Vec<String> = clients.keys().cloned().collect();

    if client_ids.is_empty() {
        warn!("No active clients available to handle new public connection.");
        return Err(anyhow!("No active clients"));
    }

    let chosen_client_id = client_ids.choose(&mut rand::thread_rng()).ok_or_else(|| anyhow!("Failed to choose a client"))?;
    info!("Chose client '{}' for the new connection.", chosen_client_id);

    if let Some(client_info) = clients.get(chosen_client_id) {
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
            clients.remove(chosen_client_id);
            pending_connections.lock().await.remove(&proxy_conn_id);
            return Err(e.into());
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