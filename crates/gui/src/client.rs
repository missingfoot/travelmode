//! Daemon client running on a background thread with its own tokio
//! runtime. The GTK main loop never blocks on networking: results and
//! events are shipped to the root component through a relm4::Sender
//! (flume channel, drained by the glib main context), and UI actions
//! come back through a tokio mpsc channel.
//!
//! One session = initial fetch (GetStatus, GetNetwork, GetTop,
//! GetConnections, ListRules) + a Subscribe stream. Commands
//! (block/unblock/pause) are one-shot requests on short-lived
//! connections, exactly like the CLI does. On any failure the loop
//! reports Disconnected and retries after a backoff.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use relm4::Sender;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use travelmode_core::ipc::{read_frame, write_frame, Request, Response};
use travelmode_core::types::*;

use crate::app::AppMsg;
use crate::tray;

/// How long to wait before reconnecting after a failure.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Full snapshot from the initial fetch. Boxed inside ClientMsg to
/// keep the message enum small.
#[derive(Debug)]
pub struct Snapshot {
    pub status: Status,
    pub network: NetworkInfo,
    pub top: Vec<AppUsage>,
    pub conns: Vec<ConnectionInfo>,
    pub rules: Vec<Rule>,
}

/// Messages from the client thread to the UI.
#[derive(Debug)]
pub enum ClientMsg {
    /// Connected; full snapshot from the initial fetch.
    Connected(Box<Snapshot>),
    /// Connection lost or daemon unreachable.
    Disconnected,
    /// Live event from the Subscribe stream.
    Event(Event),
    /// A UI-issued command was rejected by the daemon (or the daemon
    /// was unreachable).
    CommandFailed(String),
}

/// Commands from the UI to the daemon.
#[derive(Debug)]
pub enum GuiCmd {
    SetPaused(bool),
    Block { name: String, exe: PathBuf },
    Unblock { rule_id: u64 },
    /// Reconnect immediately instead of waiting out the backoff.
    Reconnect,
    /// Theme changed: re-push the tray icon pixmaps.
    RefreshTrayIcon,
}

/// Handle the UI uses to talk to the client thread.
pub struct ClientHandle {
    pub cmd_tx: mpsc::UnboundedSender<GuiCmd>,
}

/// Spawn the client thread. Returns immediately.
pub fn spawn(
    socket_path: PathBuf,
    app_sender: Sender<AppMsg>,
    paused_flag: Arc<AtomicBool>,
    dark_flag: Arc<AtomicBool>,
) -> ClientHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let handle = ClientHandle {
        cmd_tx: cmd_tx.clone(),
    };
    std::thread::Builder::new()
        .name("daemon-client".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(error = %e, "cannot build tokio runtime; daemon client disabled");
                    let _ = app_sender.send(AppMsg::Client(ClientMsg::CommandFailed(
                        format!("internal error: {e}"),
                    )));
                    return;
                }
            };
            rt.block_on(run(socket_path, app_sender, cmd_tx, cmd_rx, paused_flag, dark_flag));
        })
        .expect("failed to spawn daemon client thread");
    handle
}

async fn run(
    socket: PathBuf,
    sender: Sender<AppMsg>,
    cmd_tx: mpsc::UnboundedSender<GuiCmd>,
    mut cmd_rx: mpsc::UnboundedReceiver<GuiCmd>,
    paused_flag: Arc<AtomicBool>,
    dark_flag: Arc<AtomicBool>,
) {
    // System tray: best-effort, failures only cost the tray icon.
    let tray = tray::try_spawn(cmd_tx, sender.clone(), paused_flag, dark_flag).await;

    loop {
        if let Err(e) = session(&socket, &sender, &mut cmd_rx, &tray).await {
            debug!(error = %e, "daemon connection ended");
        }
        send(&sender, ClientMsg::Disconnected);

        // Backoff — but stay responsive to UI commands and manual retry.
        let mut backoff = std::pin::pin!(tokio::time::sleep(RECONNECT_DELAY));
        loop {
            tokio::select! {
                _ = &mut backoff => break,
                cmd = cmd_rx.recv() => match cmd {
                    Some(GuiCmd::Reconnect) => break,
                    Some(GuiCmd::RefreshTrayIcon) => refresh_tray(&tray).await,
                    Some(cmd) => try_command(&socket, &sender, cmd).await,
                    None => return, // UI gone
                },
            }
        }
    }
}

/// Ask ksni to re-query the tray's icon pixmaps (after a theme change).
async fn refresh_tray(tray: &Option<ksni::Handle<tray::TravelmodeTray>>) {
    if let Some(handle) = tray {
        handle.update(|_| ()).await;
    }
}

/// One daemon session: subscribe, initial fetch, then stream events
/// while servicing UI commands.
async fn session(
    socket: &PathBuf,
    sender: &Sender<AppMsg>,
    cmd_rx: &mut mpsc::UnboundedReceiver<GuiCmd>,
    tray: &Option<ksni::Handle<tray::TravelmodeTray>>,
) -> Result<(), String> {
    // Open the event stream first so nothing is lost between the fetch
    // and the subscription (events arriving during the fetch are
    // applied idempotently by the state reducer).
    let mut sub = connect(socket).await?;
    write_frame(&mut sub, &Request::Subscribe)
        .await
        .map_err(|e| format!("subscribe failed: {e}"))?;

    // Initial fetch: one request per connection, like the CLI.
    let status = fetch_status(socket).await?;
    let network = fetch_network(socket).await?;
    let top = fetch_top(socket).await?;
    let conns = fetch_connections(socket).await?;
    let rules = fetch_rules(socket).await?;
    send(
        sender,
        ClientMsg::Connected(Box::new(Snapshot {
            status,
            network,
            top,
            conns,
            rules,
        })),
    );
    info!("connected to travelmoded");

    loop {
        tokio::select! {
            frame = read_frame::<Response, _>(&mut sub) => {
                match frame {
                    Ok(Response::Event(event)) => send(sender, ClientMsg::Event(event)),
                    Ok(_) => {}
                    Err(e) => return Err(format!("event stream lost: {e}")),
                }
            }
            cmd = cmd_rx.recv() => match cmd {
                // Already connected; nothing to reconnect to.
                Some(GuiCmd::Reconnect) => {}
                Some(GuiCmd::RefreshTrayIcon) => refresh_tray(tray).await,
                Some(cmd) => try_command(socket, sender, cmd).await,
                None => return Ok(()),
            },
        }
    }
}

/// Run a command and report failures to the UI as a toast.
async fn try_command(socket: &PathBuf, sender: &Sender<AppMsg>, cmd: GuiCmd) {
    if let Err(e) = send_command(socket, cmd).await {
        send(sender, ClientMsg::CommandFailed(e));
    }
}

async fn send_command(socket: &PathBuf, cmd: GuiCmd) -> Result<(), String> {
    let request = match cmd {
        GuiCmd::SetPaused(paused) => Request::SetPaused { paused },
        GuiCmd::Block { name, exe } => Request::AddRule {
            rule: RuleInput {
                name,
                exe_path: exe,
                action: RuleAction::Block,
                persistent: true,
                ttl_secs: None,
            },
        },
        GuiCmd::Unblock { rule_id } => Request::RemoveRule { id: rule_id },
        GuiCmd::Reconnect | GuiCmd::RefreshTrayIcon => return Ok(()),
    };
    match one_shot(socket, &request).await? {
        Response::Ok => Ok(()),
        Response::Error { message } => Err(format!("daemon error: {message}")),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

async fn connect(socket: &PathBuf) -> Result<UnixStream, String> {
    UnixStream::connect(socket)
        .await
        .map_err(|e| format!("cannot connect to {}: {e}", socket.display()))
}

/// Send one request on a fresh connection, read one response.
async fn one_shot(socket: &PathBuf, request: &Request) -> Result<Response, String> {
    let mut stream = connect(socket).await?;
    write_frame(&mut stream, request)
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    read_frame(&mut stream)
        .await
        .map_err(|e| format!("response failed: {e}"))
}

async fn fetch_status(socket: &PathBuf) -> Result<Status, String> {
    match one_shot(socket, &Request::GetStatus).await? {
        Response::Status(v) => Ok(v),
        other => Err(format!("unexpected response to GetStatus: {other:?}")),
    }
}

async fn fetch_network(socket: &PathBuf) -> Result<NetworkInfo, String> {
    match one_shot(socket, &Request::GetNetwork).await? {
        Response::Network(v) => Ok(v),
        other => Err(format!("unexpected response to GetNetwork: {other:?}")),
    }
}

async fn fetch_top(socket: &PathBuf) -> Result<Vec<AppUsage>, String> {
    match one_shot(socket, &Request::GetTop).await? {
        Response::Top(v) => Ok(v),
        other => Err(format!("unexpected response to GetTop: {other:?}")),
    }
}

async fn fetch_connections(socket: &PathBuf) -> Result<Vec<ConnectionInfo>, String> {
    match one_shot(socket, &Request::GetConnections).await? {
        Response::Connections(v) => Ok(v),
        other => Err(format!("unexpected response to GetConnections: {other:?}")),
    }
}

async fn fetch_rules(socket: &PathBuf) -> Result<Vec<Rule>, String> {
    match one_shot(socket, &Request::ListRules).await? {
        Response::Rules(v) => Ok(v),
        other => Err(format!("unexpected response to ListRules: {other:?}")),
    }
}

fn send(sender: &Sender<AppMsg>, msg: ClientMsg) {
    // A dead receiver only means the window is gone; nothing to do.
    let _ = sender.send(AppMsg::Client(msg));
}
