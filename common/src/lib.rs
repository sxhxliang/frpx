use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Model {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

/// Commands exchanged between client and server.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Command {
    /// Register a new client. Sent from frpc to frps.
    Register {
        client_id: String,
    },
    /// Result of the registration. Sent from frps to frpc.
    RegisterResult {
        success: bool,
        error: Option<String>,
    },
    /// Request a new proxy connection. Sent from frps to a chosen frpc.
    RequestNewProxyConn {
        proxy_conn_id: String,
    },
    /// Notify the proxy listener that a new client is ready. Sent from frpc to frps.
    NewProxyConn {
        proxy_conn_id: String,
    },
    // Login with email and password.
    Login {
        email: String,
        pass: String,
    },
    // Login with a token.
    LoginByToken {
        token: String,
    },
    // Login result.
    LoginResult {
        success: bool,
        error: Option<String>,
        token: Option<String>,
    },
    /// Heartbeat message from client to server
    Heartbeat {
        models: Option<Vec<Model>>,
    },
    /// System information from client to server
    SystemInfo {
        cpu_usage: f32,
        memory_usage: f32,
        disk_usage: f32,
        computer_name: String,
    },
}

/// Reads a command from an async reader.
/// The format is a 4-byte length prefix (u32) followed by the JSON-encoded command.
pub async fn read_command<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Command> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;

    serde_json::from_slice(&buf).map_err(|e| anyhow!("Failed to deserialize command: {}", e))
}

/// Writes a command to an async writer.
/// The format is a 4-byte length prefix (u32) followed by the JSON-encoded command.
pub async fn write_command<W: AsyncWrite + Unpin>(writer: &mut W, command: &Command) -> Result<()> {
    let buf = serde_json::to_vec(command)?;
    let len = buf.len() as u32;

    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&buf).await?;
    writer.flush().await?;
    Ok(())
}

/// Joins two streams, copying data in both directions.
pub async fn join_streams<A, B>(a: A, b: B) -> std::io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut a_reader, mut a_writer) = tokio::io::split(a);
    let (mut b_reader, mut b_writer) = tokio::io::split(b);
    tokio::select! {
        res = tokio::io::copy(&mut a_reader, &mut b_writer) => res?,
        res = tokio::io::copy(&mut b_reader, &mut a_writer) => res?,
    };
    Ok(())
}