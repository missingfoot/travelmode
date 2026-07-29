//! System tray icon via StatusNotifierItem (ksni). Runs inside the
//! client thread's tokio runtime; activation callbacks just push
//! messages into the normal channels. Registration failure is
//! non-fatal: the app simply has no tray icon.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ksni::menu::{CheckmarkItem, StandardItem};
use relm4::Sender;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::app::AppMsg;
use crate::client::GuiCmd;

/// Tray state. Menu callbacks are plain `Fn(&mut Self)`, so everything
/// they need is cloned in here.
struct TravelmodeTray {
    cmd_tx: mpsc::UnboundedSender<GuiCmd>,
    app_sender: Sender<AppMsg>,
    paused: Arc<AtomicBool>,
}

impl ksni::Tray for TravelmodeTray {
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        "travelmode".into()
    }

    fn title(&self) -> String {
        "Travel Mode".into()
    }

    fn icon_name(&self) -> String {
        "network-transmit-receive-symbolic".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left click: same as the Open menu item.
        let _ = self.app_sender.send(AppMsg::TrayOpen);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let paused = self.paused.load(Ordering::Relaxed);
        vec![
            StandardItem {
                label: "Open Travel Mode".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.app_sender.send(AppMsg::TrayOpen);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Pause Filtering".into(),
                checked: paused,
                activate: Box::new(|tray: &mut Self| {
                    let new = !tray.paused.load(Ordering::Relaxed);
                    let _ = tray.cmd_tx.send(GuiCmd::SetPaused(new));
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| {
                    // Quits the GUI only; the daemon keeps running.
                    let _ = tray.app_sender.send(AppMsg::TrayQuit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Try to register the tray icon. Tells the UI whether it succeeded so
/// window-close can fall back to quitting when there is no tray.
pub async fn try_spawn(
    cmd_tx: mpsc::UnboundedSender<GuiCmd>,
    app_sender: Sender<AppMsg>,
    paused: Arc<AtomicBool>,
) {
    use ksni::TrayMethods;
    let tray = TravelmodeTray {
        cmd_tx,
        app_sender: app_sender.clone(),
        paused,
    };
    match tray.spawn().await {
        Ok(_handle) => {
            info!("system tray registered");
            let _ = app_sender.send(AppMsg::TrayReady(true));
        }
        Err(e) => {
            warn!(error = %e, "system tray unavailable; continuing without it");
            let _ = app_sender.send(AppMsg::TrayReady(false));
        }
    }
}
