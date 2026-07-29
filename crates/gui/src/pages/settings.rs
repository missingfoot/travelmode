//! Settings page: daemon status, global pause switch, about section.

use std::cell::Cell;
use std::rc::Rc;

use relm4::adw::{self, prelude::*};
use relm4::gtk;
use relm4::gtk::glib;
use tokio::sync::mpsc;

use crate::client::GuiCmd;
use crate::fmt::human_uptime;
use crate::icons::{set_ui_icon, ui_image};
use crate::state::ClientState;

pub struct SettingsPage {
    container: gtk::Widget,
    version_row: adw::ActionRow,
    uptime_row: adw::ActionRow,
    filtering_row: adw::ActionRow,
    pause_icon: gtk::Image,
    pause_switch: gtk::Switch,
    pause_updating: Rc<Cell<bool>>,
    /// Row icons, re-pointed at the right variant on theme change.
    icons: Vec<(gtk::Image, &'static str)>,
}

fn info_row(
    group: &adw::PreferencesGroup,
    icon: &'static str,
    title: &str,
    icons: &mut Vec<(gtk::Image, &'static str)>,
) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    let image = ui_image(icon, false);
    row.add_prefix(&image);
    icons.push((image, icon));
    group.add(&row);
    row
}

impl SettingsPage {
    pub fn new(socket_path: &str, cmd_tx: &mpsc::UnboundedSender<GuiCmd>) -> Self {
        let mut icons = Vec::new();

        let daemon = adw::PreferencesGroup::new();
        daemon.set_title("Daemon");
        let version_row = info_row(&daemon, "settings", "Version", &mut icons);
        let uptime_row = info_row(&daemon, "uptime", "Uptime", &mut icons);
        let filtering_row = info_row(&daemon, "filtering", "Filtering", &mut icons);
        let socket_row = info_row(&daemon, "socket", "Socket", &mut icons);
        socket_row.set_subtitle(socket_path);

        let filtering = adw::PreferencesGroup::new();
        filtering.set_title("Filtering");
        let pause_row = adw::ActionRow::new();
        pause_row.set_title("Pause filtering");
        pause_row.set_subtitle("Temporarily allow all traffic");
        let pause_icon = ui_image("pause", false);
        pause_row.add_prefix(&pause_icon);
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
        let about_row = info_row(&about, "plane", "Travel Mode", &mut icons);
        about_row.set_subtitle(&format!(
            "Per-application network control — v{}",
            env!("CARGO_PKG_VERSION")
        ));
        let license_row = info_row(&about, "license", "License", &mut icons);
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
            pause_icon,
            pause_switch,
            pause_updating,
            icons,
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    pub fn update(&self, state: &ClientState, dark: bool) {
        for (image, name) in &self.icons {
            set_ui_icon(image, name, dark);
        }
        // The pause row icon reflects the current state: paused shows
        // "play" (click to resume), running shows "pause".
        set_ui_icon(&self.pause_icon, if state.paused { "play" } else { "pause" }, dark);
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
