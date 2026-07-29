//! Unix socket IPC server.
//!
//! Each client sends one Request frame and gets one Response frame,
//! except Request::Subscribe, which switches the connection into
//! streaming mode: the client then receives Response::Event frames from
//! the daemon-wide broadcast channel until it disconnects.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, warn};
use travelmode_core::ipc::{read_frame, write_frame, Request, Response};

use crate::state::DaemonState;

/// Dispatch function supplied by main; keeps this module free of
/// request-handling logic.
pub type Dispatcher = fn(Arc<DaemonState>, Request) -> Response;

/// Bind the socket and serve clients forever. Returns the bound path so
/// the caller can unlink it on shutdown.
pub async fn serve(
    state: Arc<DaemonState>,
    dispatch: Dispatcher,
) -> std::io::Result<PathBuf> {
    let path = state.config.socket_path.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Remove a stale socket file from a previous run.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    // Owner/group read-write; world no access.
    set_permissions(&path);
    let bound = path.clone();
    tokio::spawn(async move {
        accept_loop(listener, state, dispatch).await;
    });
    Ok(bound)
}

fn set_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660)) {
        warn!(path = %path.display(), error = %e, "cannot chmod socket");
    }
}

async fn accept_loop(listener: UnixListener, state: Arc<DaemonState>, dispatch: Dispatcher) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    let reason = handle_client(stream, state, dispatch).await;
                    debug!(reason = %reason, "client connection ended");
                });
            }
            Err(e) => {
                warn!(error = %e, "accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_client(
    stream: UnixStream,
    state: Arc<DaemonState>,
    dispatch: Dispatcher,
) -> travelmode_core::ipc::IpcError {
    let (mut reader, mut writer) = stream.into_split();
    let request: Request = match read_frame(&mut reader).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    if matches!(request, Request::Subscribe) {
        stream_events(&mut writer, &state).await
    } else {
        let response = dispatch(state, request);
        match write_frame(&mut writer, &response).await {
            Ok(()) => travelmode_core::ipc::IpcError::Closed,
            Err(e) => e,
        }
    }
}

async fn stream_events(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    state: &Arc<DaemonState>,
) -> travelmode_core::ipc::IpcError {
    let mut rx = state.events.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let response = Response::Event(event);
                if let Err(e) = write_frame(writer, &response).await {
                    return e;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                debug!(skipped = n, "subscriber lagged; dropping events");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return travelmode_core::ipc::IpcError::Closed;
            }
        }
    }
}
