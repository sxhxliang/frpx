use anyhow::{anyhow, Result};
use clap::Parser;
use common::{read_command, write_command, join_streams, Command, Model};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::interval;
use tracing::{info, error, warn, Level};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Unique ID for this client instance.
    #[arg(short, long)]
    client_id: String,

    /// Address of the frps server.
    #[arg(short, long, default_value = "127.0.0.1")]
    server_addr: String,

    /// Port for the frps control connection.
    #[arg(long, default_value_t = 17000)]
    control_port: u16,

    /// Port for the frps proxy connection.
    #[arg(long, default_value_t = 17001)]
    proxy_port: u16,

    /// Address of the local service to expose.
    #[arg(long, default_value = "127.0.0.1")]
    local_addr: String,

    /// Port of the local service to expose.
    #[arg(long, default_value_t = 11434)]
    local_port: u16,

    /// Email for authentication (skip interactive input)
    #[arg(long)]
    email: Option<String>,

    /// Password for authentication (skip interactive input)
    #[arg(long)]
    password: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TokenData {
    token: String,
}

#[derive(Debug)]
struct SystemInfo {
    cpu_usage: f32,
    memory_usage: f32,
    disk_usage: f32,
}

// This struct is to deserialize the top-level JSON from Ollama API
#[derive(Deserialize, Debug)]
struct OllamaModelsResponse {
    data: Vec<Model>,
}

async fn get_ollama_models() -> Result<Vec<Model>> {
    let client = reqwest::Client::new();
    let res = client
        .get("http://localhost:11434/v1/models")
        .send()
        .await
        .map_err(|e| anyhow!("Failed to connect to Ollama: {}", e))?;

    if !res.status().is_success() {
        return Err(anyhow!(
            "Ollama API returned non-success status: {}",
            res.status()
        ));
    }

    let response: OllamaModelsResponse = res
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse JSON from Ollama: {}", e))?;

    Ok(response.data)
}


#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    info!("Starting frpc with client_id: {}", args.client_id);
    info!("Server address: {}:{}", args.server_addr, args.control_port);
    info!("Local service: {}:{}", args.local_addr, args.local_port);

    let control_stream = TcpStream::connect(format!("{}:{}", args.server_addr, args.control_port)).await?;
    info!("Connected to control port.");

    let (mut reader, mut writer) = tokio::io::split(control_stream);

    let token_path = Path::new("token.json");
    if token_path.exists() {
        let token_data: TokenData = serde_json::from_str(&fs::read_to_string(token_path)?)?;
        let login_cmd = Command::LoginByToken { token: token_data.token };
        write_command(&mut writer, &login_cmd).await?;
    } else if let (Some(email), Some(password)) = (args.email.clone(), args.password.clone()) {
        // Use provided credentials
        let login_cmd = Command::Login {
            email,
            pass: password,
        };
        write_command(&mut writer, &login_cmd).await?;
    } else {
        print!("Enter email: ");
        io::stdout().flush()?;
        let mut email = String::new();
        io::stdin().read_line(&mut email)?;

        print!("Enter password: ");
        io::stdout().flush()?;
        let mut pass = String::new();
        io::stdin().read_line(&mut pass)?;

        let login_cmd = Command::Login {
            email: email.trim().to_string(),
            pass: pass.trim().to_string(),
        };
        write_command(&mut writer, &login_cmd).await?;
    }

    match read_command(&mut reader).await? {
        Command::LoginResult { success, error, token } => {
            if success {
                if let Some(token) = token {
                    fs::write("token.json", serde_json::to_string(&TokenData { token })?)?;
                }
                info!("Successfully logged in.");
            } else {
                error!("Login failed: {}", error.unwrap_or_default());
                return Err(anyhow!("Login failed"));
            }
        }
        _ => {
            return Err(anyhow!("Received unexpected command after login attempt."));
        }
    }

    // Register the client
    let register_cmd = Command::Register { client_id: args.client_id.clone() };
    write_command(&mut writer, &register_cmd).await?;

    // Wait for registration result
    match read_command(&mut reader).await? {
        Command::RegisterResult { success, error } => {
            if success {
                info!("Successfully registered with the server.");
            } else {
                error!("Registration failed: {}", error.unwrap_or_default());
                return Err(anyhow!("Registration failed"));
            }
        }
        _ => {
            return Err(anyhow!("Received unexpected command after registration attempt."));
        }
    }

    // Clone necessary variables for the heartbeat task
    let mut writer_clone = writer;
    
    // Spawn a task to send periodic heartbeats and system info
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(10)); // Send heartbeat every 10 seconds
        loop {
            interval.tick().await;

            // Get models from local Ollama instance
            let models = match get_ollama_models().await {
                Ok(models) => {
                    info!("Successfully fetched {} models from Ollama.", models.len());
                    Some(models)
                }
                Err(e) => {
                    warn!("Could not fetch models from Ollama: {}. This is okay if Ollama is not running.", e);
                    None
                }
            };

            // Send heartbeat with model info
            let heartbeat_cmd = Command::Heartbeat { models };
            if let Err(e) = write_command(&mut writer_clone, &heartbeat_cmd).await {
                error!("Failed to send heartbeat: {}", e);
                break;
            }

            // Collect and send system information
            if let Ok(sys_info) = collect_system_info().await {
                if let Err(e) = write_command(&mut writer_clone, &Command::SystemInfo {
                    cpu_usage: sys_info.cpu_usage,
                    memory_usage: sys_info.memory_usage,
                    disk_usage: sys_info.disk_usage,
                }).await {
                    error!("Failed to send system info: {}", e);
                    break;
                }
            }
        }
    });
    
    // Main loop to listen for commands from the server
    loop {
        match read_command(&mut reader).await {
            Ok(Command::RequestNewProxyConn { proxy_conn_id }) => {
                info!("Received request for new proxy connection: {}", proxy_conn_id);
                let args_clone = args.clone();
                tokio::spawn(async move {
                    if let Err(e) = create_proxy_connection(args_clone, proxy_conn_id).await {
                        error!("Failed to create proxy connection: {}", e);
                    }
                });
            }
            Ok(cmd) => {
                warn!("Received unexpected command: {:?}", cmd);
            }
            Err(ref e) if e.downcast_ref::<io::Error>().map_or(false, |io_err| io_err.kind() == io::ErrorKind::UnexpectedEof) => {
                error!("Control connection closed by server. Shutting down.");
                break;
            }
            Err(e) => {
                error!("Error reading from control connection: {}. Shutting down.", e);
                break;
            }
        }
    }

    Ok(())
}

async fn create_proxy_connection(args: Args, proxy_conn_id: String) -> Result<()> {
    let mut proxy_stream = TcpStream::connect(format!("{}:{}", args.server_addr, args.proxy_port)).await?;
    info!("('{}') Connected to proxy port.", proxy_conn_id);

    let notify_cmd = Command::NewProxyConn { proxy_conn_id: proxy_conn_id.clone() };
    write_command(&mut proxy_stream, &notify_cmd).await?;
    info!("('{}') Sent new proxy connection notification.", proxy_conn_id);

    let local_stream = TcpStream::connect(format!("{}:{}", args.local_addr, args.local_port)).await?;
    info!("('{}') Connected to local service at {}:{}", proxy_conn_id, args.local_addr, args.local_port);

    info!("('{}') Joining streams...", proxy_conn_id);
    join_streams(proxy_stream, local_stream).await?;
    info!("('{}') Streams joined and finished.", proxy_conn_id);

    Ok(())
}

#[cfg(target_os = "linux")]
async fn collect_system_info() -> Result<SystemInfo> {
    use std::process::Command;
    
    // Get CPU usage
    let cpu_output = Command::new("top")
        .args(["-bn1"])
        .output()?;
    let cpu_str = String::from_utf8(cpu_output.stdout)?;
    let mut cpu_usage = 0.0;
    for line in cpu_str.lines() {
        if line.contains("Cpu(s)") {
            if let Some(cpu_part) = line.split(',').next() {
                if let Some(usage_str) = cpu_part.split_whitespace().last() {
                    if let Ok(usage) = usage_str.trim_end_matches('%').parse::<f32>() {
                        cpu_usage = 100.0 - usage; // Idle to usage
                        break;
                    }
                }
            }
        }
    }
    
    // Get memory usage
    let mem_output = Command::new("free")
        .args(["-m"])
        .output()?;
    let mem_str = String::from_utf8(mem_output.stdout)?;
    let mut memory_usage = 0.0;
    for line in mem_str.lines() {
        if line.starts_with("Mem:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                if let (Ok(total), Ok(used)) = (parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                    memory_usage = (used / total) * 100.0;
                    break;
                }
            }
        }
    }
    
    // Get disk usage
    let disk_output = Command::new("df")
        .args(["/"])
        .output()?;
    let disk_str = String::from_utf8(disk_output.stdout)?;
    let mut disk_usage = 0.0;
    for line in disk_str.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            if let Ok(usage) = parts[4].trim_end_matches('%').parse::<f32>() {
                disk_usage = usage;
                break;
            }
        }
    }
    
    Ok(SystemInfo {
        cpu_usage,
        memory_usage,
        disk_usage,
    })
}

#[cfg(target_os = "macos")]
async fn collect_system_info() -> Result<SystemInfo> {
    use std::process::Command;
    
    // Get CPU usage
    let cpu_output = Command::new("top")
        .args(["-l", "1", "-n", "0"])
        .output()?;
    let cpu_str = String::from_utf8(cpu_output.stdout)?;
    let mut cpu_usage = 0.0;
    for line in cpu_str.lines() {
        if line.contains("CPU usage:") {
            if let Some(usage_str) = line.split(',').next() {
                if let Some(usage_part) = usage_str.split_whitespace().nth(2) {
                    if let Ok(usage) = usage_part.trim_end_matches('%').parse::<f32>() {
                        cpu_usage = usage;
                        break;
                    }
                }
            }
        }
    }
    
    // Get memory usage
    let mem_output = Command::new("vm_stat")
        .output()?;
    let mem_str = String::from_utf8(mem_output.stdout)?;
    let mut memory_usage = 0.0;
    let (mut pages_active, mut pages_wired, mut pages_compressed) = (0u64, 0u64, 0u64);
    let mut pages_total = 0u64;
    
    for line in mem_str.lines() {
        if line.contains("Pages active:") {
            if let Some(pages_str) = line.split_whitespace().nth(2) {
                if let Ok(pages) = pages_str.trim_end_matches('.').parse::<u64>() {
                    pages_active = pages;
                }
            }
        } else if line.contains("Pages wired down:") {
            if let Some(pages_str) = line.split_whitespace().nth(3) {
                if let Ok(pages) = pages_str.trim_end_matches('.').parse::<u64>() {
                    pages_wired = pages;
                }
            }
        } else if line.contains("Pages occupied by compressor:") {
            if let Some(pages_str) = line.split_whitespace().nth(4) {
                if let Ok(pages) = pages_str.trim_end_matches('.').parse::<u64>() {
                    pages_compressed = pages;
                }
            }
        } else if line.contains("Mach Virtual Memory Statistics") {
            if let Some(pages_str) = line.split_whitespace().nth(5) {
                if let Ok(pages) = pages_str.parse::<u64>() {
                    pages_total = pages;
                }
            }
        }
    }
    
    if pages_total > 0 {
        let used_pages = pages_active + pages_wired + pages_compressed;
        memory_usage = (used_pages as f32 / pages_total as f32) * 100.0;
    }
    
    // Get disk usage
    let disk_output = Command::new("df")
        .args(["-P", "/"])
        .output()?;
    let disk_str = String::from_utf8(disk_output.stdout)?;
    let mut disk_usage = 0.0;
    for line in disk_str.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5 {
            if let Ok(usage) = parts[4].trim_end_matches('%').parse::<f32>() {
                disk_usage = usage;
                break;
            }
        }
    }
    
    Ok(SystemInfo {
        cpu_usage,
        memory_usage,
        disk_usage,
    })
}

#[cfg(target_os = "windows")]
async fn collect_system_info() -> Result<SystemInfo> {
    // For Windows, we'll return default values as implementing this properly
    // would require additional dependencies
    Ok(SystemInfo {
        cpu_usage: 0.0,
        memory_usage: 0.0,
        disk_usage: 0.0,
    })
}

// Fallback for other platforms
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
async fn collect_system_info() -> Result<SystemInfo> {
    Ok(SystemInfo {
        cpu_usage: 0.0,
        memory_usage: 0.0,
        disk_usage: 0.0,
    })
}