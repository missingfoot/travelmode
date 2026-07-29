//! travelmode-gui: GTK4/libadwaita desktop client for travelmoded.
//!
//! The GUI is a stateless client of the daemon over the same IPC
//! protocol the CLI uses; all networking lives in the background
//! client thread (client.rs), never on the GTK main loop.

mod app;
mod client;
mod fmt;
mod icons;
mod pages;
mod state;
mod tray;

use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::app::{App, AppInit};

#[derive(Parser)]
#[command(name = "travelmode-gui", version, about = "Travel Mode desktop app")]
struct Args {
    /// Path to the travelmoded socket.
    #[arg(long, default_value = "/run/travelmode/daemon.sock")]
    socket: PathBuf,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let app = relm4::RelmApp::new("com.github.missingfoot.travelmode");
    // Hand GTK an empty argv: our own flags (e.g. --socket) are already
    // consumed by clap and GTK would reject them as unknown options.
    app.with_args(Vec::new()).run::<App>(AppInit {
        socket_path: args.socket,
    });
}
