//! travelmoded: per-application network control daemon for Linux.
//!
//! Runs as root; tracks flows via conntrack, attributes them to
//! processes, enforces Block rules via nftables + NFQUEUE, and serves
//! the CLI over a Unix socket.

mod attrib;
mod config;
mod conntrack;
mod firewall;
mod ipc;
mod netinfo;
mod procs;
mod rules;
mod state;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use travelmode_core::ipc::{Request, Response};
use travelmode_core::types::{AppUsage, Event, NetworkInfo, Status};

use crate::config::Config;
use crate::state::DaemonState;

#[derive(Parser)]
#[command(name = "travelmoded", version, about = "travelmode daemon")]
struct Args {
    /// Path to the TOML config file.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let config = Config::load(args.config.as_deref());

    // RUST_LOG wins over the config's log_level. The netlink-packet-route
    // crate warns on every dump about newer kernels' larger NLA payloads;
    // clamp it so real messages stay visible.
    let base_filter =
        std::env::var("RUST_LOG").unwrap_or_else(|_| config.log_level.clone());
    let filter = EnvFilter::try_new(format!(
        "{base_filter},netlink_packet_route=error,netlink_proto=error"
    ))
    .unwrap_or_else(|_| {
        EnvFilter::new("info,netlink_packet_route=error,netlink_proto=error")
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!(version = env!("CARGO_PKG_VERSION"), "travelmoded starting");
    let state = Arc::new(DaemonState::new(config));

    // Firewall: nftables table + NFQUEUE worker. Failure (e.g. not
    // root) is not fatal: the daemon still tracks and reports.
    match firewall::Firewall::setup(state.config.queue_num, state.is_paused()).await {
        Ok(fw) => {
            *state.firewall.lock().await = Some(fw);
            firewall::spawn_queue_worker(state.clone());
        }
        Err(e) => {
            warn!(error = %e,
                "nftables setup failed; filtering disabled (daemon keeps running)");
        }
    }

    // Initial network snapshot, then keep it fresh in the background.
    refresh_network(&state).await;
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                refresh_network(&state).await;
            }
        });
    }

    // Background tasks.
    tokio::spawn(conntrack::run(state.clone()));
    tokio::spawn(procs::run(state.clone()));
    tokio::spawn(reaper_loop(state.clone()));

    // IPC server.
    let socket_path = match ipc::serve(state.clone(), dispatch).await {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "cannot bind IPC socket; exiting");
            shutdown(state, None).await;
            std::process::exit(1);
        }
    };
    info!(socket = %socket_path.display(), "IPC server listening");

    wait_for_shutdown_signal().await;
    info!("shutting down");
    shutdown(state, Some(socket_path)).await;
    info!("shutdown complete");
    // The NFQUEUE worker thread is parked in a blocking recv() that
    // cannot be cancelled, and tokio's runtime drop would wait for it
    // forever — leaving the process (and its queue-0 binding) alive
    // past SIGTERM. All cleanup is done, so exit explicitly; the
    // kernel then closes the NFQUEUE socket and releases the queue.
    std::process::exit(0);
}

async fn shutdown(state: Arc<DaemonState>, socket_path: Option<PathBuf>) {
    let fw = state.firewall.lock().await.take();
    if let Some(fw) = fw {
        fw.teardown().await;
    }
    if let Some(path) = socket_path {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %path.display(), error = %e, "cannot remove socket file");
            }
        }
    }
    state.rules.save();
}

async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => {
            // No SIGTERM handler; just wait for Ctrl-C.
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }
}

async fn refresh_network(state: &Arc<DaemonState>) {
    let snapshot = netinfo::snapshot().await;
    *state.network.write().unwrap() = Some(snapshot);
}

/// Reap expired temporary rules every few seconds.
async fn reaper_loop(state: Arc<DaemonState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        for id in state.rules.reap_expired() {
            info!(rule_id = id, "temporary rule expired");
            state.publish(Event::RuleRemoved { id });
        }
    }
}

// ------------------------------------------------------------- dispatch

fn dispatch(state: Arc<DaemonState>, request: Request) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::GetStatus => Response::Status(Status {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: state.uptime_secs(),
            paused: state.is_paused(),
            filtering_active: state.filtering_active.load(Ordering::Relaxed),
            rules_count: state.rules.count(),
            tracked_connections: state.connections.read().unwrap().len(),
            tracked_processes: state.processes.read().unwrap().len(),
        }),
        Request::GetNetwork => {
            let empty = NetworkInfo {
                interfaces: Vec::new(),
                gateway: None,
                dns_servers: Vec::new(),
                ssid: None,
                metered: None,
                primary_interface: None,
            };
            Response::Network(state.network.read().unwrap().clone().unwrap_or(empty))
        }
        Request::GetProcesses => {
            Response::Processes(state.processes.read().unwrap().values().cloned().collect())
        }
        Request::GetConnections => {
            Response::Connections(state.connections.read().unwrap().values().cloned().collect())
        }
        Request::GetTop => Response::Top(top_apps(&state)),
        Request::ListRules => Response::Rules(state.rules.list()),
        Request::AddRule { rule } => {
            let stored = state.rules.add(rule);
            info!(id = stored.id, exe = %stored.exe_path.display(), "rule added");
            state.publish(Event::RuleAdded(stored));
            Response::Ok
        }
        Request::RemoveRule { id } => match state.rules.remove(id) {
            Some(rule) => {
                info!(id, exe = %rule.exe_path.display(), "rule removed");
                state.publish(Event::RuleRemoved { id });
                Response::Ok
            }
            None => Response::Error {
                message: format!("no rule with id {id}"),
            },
        },
        Request::SetPaused { paused } => {
            state.paused.store(paused, Ordering::Relaxed);
            state.publish(Event::PausedChanged { paused });
            // Toggle the nftables queue rule asynchronously.
            let state = state.clone();
            tokio::spawn(async move {
                let guard = state.firewall.lock().await;
                if let Some(fw) = guard.as_ref() {
                    let result = if paused { fw.pause().await } else { fw.resume().await };
                    if let Err(e) = result {
                        warn!(error = %e, "failed to toggle nftables queue rule");
                    }
                }
            });
            Response::Ok
        }
        Request::Subscribe => Response::Error {
            message: "Subscribe is handled by the IPC layer".to_string(),
        },
    }
}

/// Aggregate per-application usage from tracked connections.
fn top_apps(state: &Arc<DaemonState>) -> Vec<AppUsage> {
    let conns = state.connections.read().unwrap();
    let mut apps: HashMap<String, AppUsage> = HashMap::new();
    for conn in conns.values() {
        let key = conn
            .exe
            .as_ref()
            .map(|p| p.display().to_string())
            .or_else(|| conn.process_name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let entry = apps.entry(key).or_insert_with(|| AppUsage {
            name: conn
                .process_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            exe: conn.exe.clone(),
            bytes_sent: 0,
            bytes_recv: 0,
            connections: 0,
            blocked: false,
        });
        entry.bytes_sent += conn.bytes_sent;
        entry.bytes_recv += conn.bytes_recv;
        entry.connections += 1;
    }
    let mut apps: Vec<AppUsage> = apps.into_values().collect();
    for app in &mut apps {
        app.blocked = app
            .exe
            .as_ref()
            .is_some_and(|exe| state.rules.is_blocked(exe));
    }
    apps.sort_by_key(|a| std::cmp::Reverse(a.bytes_sent + a.bytes_recv));
    apps
}
