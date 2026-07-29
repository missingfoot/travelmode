//! Length-prefixed JSON framing over the daemon's Unix socket.
//!
//! Wire format: 4-byte big-endian payload length, then a UTF-8 JSON body.
//! One `Request` per frame from client to daemon; the daemon replies with
//! `Response` frames (exactly one per request, or a stream of
//! `Response::Event` frames after `Request::Subscribe`).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::types::*;

pub const MAX_FRAME: u32 = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large ({0} bytes)")]
    FrameTooLarge(u32),
    #[error("connection closed")]
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Ping,
    GetStatus,
    GetNetwork,
    GetProcesses,
    GetConnections,
    GetTop,
    ListRules,
    AddRule { rule: RuleInput },
    RemoveRule { id: u64 },
    SetPaused { paused: bool },
    /// Switch this connection into event-streaming mode.
    Subscribe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Pong,
    Status(Status),
    Network(NetworkInfo),
    Processes(Vec<ProcessInfo>),
    Connections(Vec<ConnectionInfo>),
    Top(Vec<AppUsage>),
    Rules(Vec<Rule>),
    Ok,
    Error { message: String },
    Event(Event),
}

/// Read one length-prefixed frame and decode it as `T`.
pub async fn read_frame<T, R>(reader: &mut R) -> Result<T, IpcError>
where
    T: for<'de> Deserialize<'de>,
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(IpcError::Closed)
        }
        Err(e) => return Err(IpcError::Io(e)),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(IpcError::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

/// Encode `value` as JSON and write it as one length-prefixed frame.
pub async fn write_frame<T, W>(writer: &mut W, value: &T) -> Result<(), IpcError>
where
    T: Serialize,
    W: AsyncWriteExt + Unpin,
{
    let body = serde_json::to_vec(value)?;
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}
