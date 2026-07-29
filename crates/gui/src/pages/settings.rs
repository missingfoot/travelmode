//! Settings page: daemon status, global pause switch, about section.

use std::cell::Cell;
use std::rc::Rc;

use relm4::adw::{self, prelude::*};
use relm4::gtk;
use relm4::gtk::glib;
use tokio::sync::mpsc;

use crate::client::GuiCmd;use crate::fmt::human_uptime;
use crate::state::ClientState;

pub struct SettingsPage {
    container: gtk::Widget,
    version_row: adw::ActionRow,
    uptime_row: adw::ActionRow,
    filtering_row: adw::ActionRow,
    pause_switch: gtk::Switch,
    pause_updating: Rc<Cell<bool>>,
}

fn info_row(group: &adw::PreferencesGroup, icon: &str, title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    group.add(&row);
    row
}

impl SettingsPage {
    pub fn new(socket_path: &str, cmd_tx: &mpsc::UnboundedSender<GuiCmd>) -> Self {
        let daemon = adw::PreferencesGroup::new();
        daemon.set_title("Daemon");
        let version_row = info_row(&daemon, "emblem-system-symbolic", "Version");
        let uptime_row = info_row(&daemon, "hourglass-symbolic", "Uptime");
        let filtering_row = info_row(&daemon, "security-high-symbolic", "Filtering");
        let socket_row = info_row(&daemon, "network-wired-symbolic", "Socket");
        socket_row.set_subtitle(socket_path);

        let filtering = adw::PreferencesGroup::new();
        filtering.set_title("Filtering");
        let pause_row = adw::ActionRow::new();
        pause_row.set_title("Pause filtering");
        pause_row.set_subtitle("Temporarily allow all traffic");
        pause_row.add_prefix(&gtk::Image::from_icon_name("media-playback-pause-symbolic"));
        let pause_switch = gtk::Switch::new();
        pause_switch.set_valign(gtk::Align::Center);
        pause_row.add_suffix(&pause_switch);
        pause_row.set_activatable_widget(Some(&pause_switch));
        filtering.add(&pause_row);

        let pause_updating = Rc::new(Cell::new(false));
        {
            let pause_updating = pause_updating.clone();
            let cmd_tx = cmd_tx.clone();
            pause_switch.connect_state_set(move |_sw, active| {
                if !pause_updating.get() {
                    let _ = cmd_tx.send(GuiCmd::SetPaused(active));
                }
                glib::Propagation::Proceed
            });
        }

        let about = adw::PreferencesGroup::new();
        about.set_title("About");
        let about_row = info_row(&about, "network-transmit-receive-symbolic", "Travel Mode");
        about_row.set_subtitle(&format!(
            "Per-application network control — v{}",
            env!("CARGO_PKG_VERSION")
        ));
        let license_row = info_row(&about, "emblem-documents-symbolic", "License");
        license_row.set_subtitle("GPL-3.0-or-later");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&daemon);
        content.append(&filtering);
        content.append(&about);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(700);
        clamp.set_child(Some(&content));

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&clamp));

        Self {
            container: scroll.upcast(),
            version_row,
            uptime_row,
            filtering_row,
            pause_switch,
            pause_updating,
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    pub fn update(&self, state: &ClientState) {
        match &state.status {
            Some(status) => {
                self.version_row.set_subtitle(&status.version);
                self.uptime_row
                    .set_subtitle(&human_uptime(status.uptime_secs));
                self.filtering_row.set_subtitle(if status.filtering_active {
                    "Active"
                } else {
                    "Inactive"
                });
            }
            None => {
                self.version_row.set_subtitle("—");
                self.uptime_row.set_subtitle("—");
                self.filtering_row.set_subtitle("Unreachable");
            }
        }
        self.pause_updating.set(true);
        self.pause_switch.set_active(state.paused);
        self.pause_updating.set(false);
    }
}
