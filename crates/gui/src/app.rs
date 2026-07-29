//! Root relm4 component: window chrome, page wiring, and the update
//! loop that folds ClientMsg into ClientState and re-renders the pages.
//!
//! Widget references live in the model (they are cheap Rc'd handles in
//! GTK), so all rendering happens in `update` and no separate
//! view-sync pass is needed.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use relm4::adw::{self, prelude::*};
use relm4::gtk;
use relm4::gtk::glib;
use relm4::{ComponentParts, ComponentSender, SimpleComponent};
use tokio::sync::mpsc;
use travelmode_core::types::Event;

use crate::client::{self, ClientMsg, GuiCmd};
use crate::pages::applications::AppsPage;
use crate::pages::connections::ConnsPage;
use crate::pages::dashboard::DashboardPage;
use crate::pages::settings::SettingsPage;
use crate::state::ClientState;

/// Everything the root component reacts to.
#[derive(Debug)]
pub enum AppMsg {
    Client(ClientMsg),
    /// One-second UI tick (speeds, uptime).
    Tick,
    /// Banner's "Retry now" button.
    Retry,
    TrayOpen,
    TrayQuit,
    TrayReady(bool),
}

pub struct AppInit {
    pub socket_path: PathBuf,
}

pub struct App {
    state: ClientState,
    cmd_tx: mpsc::UnboundedSender<GuiCmd>,
    paused_flag: Arc<AtomicBool>,
    tray_available: Rc<Cell<bool>>,
    quitting: Rc<Cell<bool>>,
    dashboard: DashboardPage,
    apps_page: AppsPage,
    conns_page: ConnsPage,
    settings: SettingsPage,
    // Chrome widgets, updated directly from `update`.
    window: adw::ApplicationWindow,
    banner: adw::Banner,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
}

impl SimpleComponent for App {
    type Input = AppMsg;
    type Output = ();
    type Init = AppInit;
    type Root = adw::ApplicationWindow;
    type Widgets = ();

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .title("Travel Mode")
            .icon_name("com.github.missingfoot.travelmode")
            .default_width(900)
            .default_height(600)
            .build()
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        // Background daemon client (+ tray on its runtime).
        let paused_flag = Arc::new(AtomicBool::new(false));
        let dark_flag = Arc::new(AtomicBool::new(false));
        let handle = client::spawn(
            init.socket_path.clone(),
            sender.input_sender().clone(),
            paused_flag.clone(),
            dark_flag.clone(),
        );
        let cmd_tx = handle.cmd_tx;

        // Follow the system color scheme for the tray glyph variant.
        {
            let style = adw::StyleManager::default();
            dark_flag.store(style.is_dark(), Ordering::Relaxed);
            let dark_flag = dark_flag.clone();
            let cmd_tx = cmd_tx.clone();
            style.connect_notify(Some("dark"), move |style, _| {
                dark_flag.store(style.is_dark(), Ordering::Relaxed);
                let _ = cmd_tx.send(GuiCmd::RefreshTrayIcon);
            });
        }

        // Window chrome: header bar with live network subtitle, offline
        // banner, view stack + bottom switcher (adaptive), toasts.
        let title = adw::WindowTitle::new("Travel Mode", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));

        let banner = adw::Banner::new("travelmoded not reachable — retrying");
        banner.set_button_label(Some("Retry now"));
        banner.set_revealed(true);
        {
            let sender = sender.clone();
            banner.connect_button_clicked(move |_| sender.input(AppMsg::Retry));
        }

        let dashboard = DashboardPage::new();
        let apps_page = AppsPage::new();
        let conns_page = ConnsPage::new();
        let settings = SettingsPage::new(&init.socket_path.display().to_string(), &cmd_tx);

        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(
            dashboard.container(),
            Some("dashboard"),
            "Dashboard",
            "user-home-symbolic",
        );
        stack.add_titled_with_icon(
            apps_page.container(),
            Some("applications"),
            "Applications",
            "application-x-executable-symbolic",
        );
        stack.add_titled_with_icon(
            conns_page.container(),
            Some("connections"),
            "Connections",
            "network-transmit-receive-symbolic",
        );
        stack.add_titled_with_icon(
            settings.container(),
            Some("settings"),
            "Settings",
            "emblem-system-symbolic",
        );
        stack.set_vexpand(true);

        let switcher = adw::ViewSwitcherBar::new();
        switcher.set_stack(Some(&stack));
        switcher.set_reveal(true);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&banner);
        content.append(&stack);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        toolbar.add_bottom_bar(&switcher);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));
        root.set_content(Some(&toasts));

        // Close hides to the tray when available; otherwise quits.
        let tray_available = Rc::new(Cell::new(false));
        let quitting = Rc::new(Cell::new(false));
        {
            let tray_available = tray_available.clone();
            let quitting = quitting.clone();
            root.connect_close_request(move |window| {
                if tray_available.get() && !quitting.get() {
                    window.set_visible(false);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }

        // One-second tick for speeds/uptime.
        {
            let sender = sender.clone();
            glib::timeout_add_seconds_local(1, move || {
                sender.input(AppMsg::Tick);
                glib::ControlFlow::Continue
            });
        }

        let model = App {
            state: ClientState::default(),
            cmd_tx,
            paused_flag,
            tray_available,
            quitting,
            dashboard,
            apps_page,
            conns_page,
            settings,
            window: root,
            banner,
            title,
            toasts,
        };
        ComponentParts {
            model,
            widgets: (),
        }
    }

    fn update(&mut self, message: Self::Input, _sender: ComponentSender<Self>) {
        match message {
            AppMsg::Client(ClientMsg::Connected(snapshot)) => {
                self.paused_flag
                    .store(snapshot.status.paused, Ordering::Relaxed);
                self.state.apply_snapshot(
                    snapshot.status,
                    snapshot.network,
                    snapshot.top,
                    snapshot.conns,
                    snapshot.rules,
                );
                self.refresh();
            }
            AppMsg::Client(ClientMsg::Disconnected) => {
                self.state.connected = false;
                self.refresh();
            }
            AppMsg::Client(ClientMsg::Event(event)) => {
                if let Event::PausedChanged { paused } = &event {
                    self.paused_flag.store(*paused, Ordering::Relaxed);
                }
                self.state.apply_event(&event);
                self.refresh();
            }
            AppMsg::Client(ClientMsg::CommandFailed(message)) => {
                tracing::warn!(%message, "daemon command failed");
                self.toasts.add_toast(adw::Toast::new(&message));
            }
            AppMsg::Tick => {
                self.state.tick();
                self.refresh();
            }
            AppMsg::Retry => {
                let _ = self.cmd_tx.send(GuiCmd::Reconnect);
            }
            AppMsg::TrayOpen => {
                self.window.present();
            }
            AppMsg::TrayQuit => {
                self.quitting.set(true);
                relm4::main_application().quit();
            }
            AppMsg::TrayReady(ok) => self.tray_available.set(ok),
        }
    }
}

impl App {
    /// Re-render chrome + all pages from the current state.
    fn refresh(&mut self) {
        self.banner.set_revealed(!self.state.connected);
        let subtitle = if !self.state.connected {
            "Offline".to_string()
        } else {
            self.state
                .network
                .as_ref()
                .and_then(|n| n.ssid.clone().or_else(|| n.primary_interface.clone()))
                .unwrap_or_default()
        };
        self.title.set_subtitle(&subtitle);

        self.dashboard.update(&self.state);
        self.apps_page.update(&self.state, &self.cmd_tx);
        self.conns_page.update(&self.state);
        self.settings.update(&self.state);
    }
}
