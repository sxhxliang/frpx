use anyhow::{anyhow, Result};
use clap::Parser;
use common::{read_command, write_command, join_streams, Command};
use tokio::net::TcpStream;
use tokio::io::{self};
use tracing::{info, error, warn, Level};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Unique ID for this client instance.
    #[arg(short, long)]
    client_id: String,

    /// Address of the frps server.
    #[arg(short, long, default_value = "127.0.0.1")]
    server_addr: String,

    /// Port for the frps control connection.
    #[arg(long, default_value_t = 7000)]
    control_port: u16,

    /// Port for the frps proxy connection.
    #[arg(long, default_value_t = 7001)]
    proxy_port: u16,

    /// Address of the local service to expose.
    #[arg(long, default_value = "127.0.0.1")]
    local_addr: String,

    /// Port of the local service to expose.
    #[arg(long, default_value_t = 3000)]
    local_port: u16,
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
    info!("('{}') Connected to local service at {}:{}.", proxy_conn_id, args.local_addr, args.local_port);

    info!("('{}') Joining streams...", proxy_conn_id);
    join_streams(proxy_stream, local_stream).await?;
    info!("('{}') Streams joined and finished.", proxy_conn_id);

    Ok(())
}

// We need to implement Clone for Args to use it in the spawned task.
impl Clone for Args {
    fn clone(&self) -> Self {
        Args {
            client_id: self.client_id.clone(),
            server_addr: self.server_addr.clone(),
            control_port: self.control_port,
            proxy_port: self.proxy_port,
            local_addr: self.local_addr.clone(),
            local_port: self.local_port,
        }
    }
}