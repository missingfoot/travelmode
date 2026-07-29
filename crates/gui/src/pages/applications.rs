//! Applications page: one row per app with live traffic and an
//! internet switch. "Light switch" model: switch ON = the app may use
//! the network, switch OFF = blocked. Toggling OFF sends a Block rule,
//! toggling ON removes it. The daemon's rule events flowing back are
//! the source of truth for the switch position.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use relm4::adw::{self, prelude::*};
use relm4::gtk;
use relm4::gtk::glib;
use tokio::sync::mpsc;

use crate::client::GuiCmd;
use crate::fmt::{human_bytes, human_speed};
use crate::icons::{set_ui_icon, ui_image};
use crate::state::{AppEntry, ClientState};

struct AppRow {
    row: adw::ActionRow,
    icon: gtk::Image,
    blocked_badge: gtk::Image,
    status: gtk::Label,
    switch: gtk::Switch,
    /// Guards against reacting to our own set_active calls.
    updating: Rc<Cell<bool>>,
    /// Current Block rule id, shared with the switch handler.
    rule_id: Rc<Cell<Option<u64>>>,
}

pub struct AppsPage {
    container: gtk::Widget,
    list: gtk::ListBox,
    rows: HashMap<String, AppRow>,
}

impl AppsPage {
    pub fn new() -> Self {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let placeholder = gtk::Label::new(Some("No applications seen yet"));
        placeholder.add_css_class("dim-label");
        placeholder.set_margin_top(24);
        placeholder.set_margin_bottom(24);
        list.set_placeholder(Some(&placeholder));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&list);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(700);
        clamp.set_child(Some(&content));

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&clamp));

        Self {
            container: scroll.upcast(),
            list,
            rows: HashMap::new(),
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    pub fn update(
        &mut self,
        state: &ClientState,
        cmd_tx: &mpsc::UnboundedSender<GuiCmd>,
        dark: bool,
    ) {
        // Drop rows for apps that left the state.
        let stale: Vec<String> = self
            .rows
            .keys()
            .filter(|k| !state.apps.contains_key(*k))
            .cloned()
            .collect();
        for key in stale {
            if let Some(row) = self.rows.remove(&key) {
                self.list.remove(&row.row);
            }
        }
        for (key, app) in &state.apps {
            match self.rows.get(key) {
                Some(row) => Self::refresh_row(row, app, dark),
                None => {
                    let row = self.create_row(app, cmd_tx);
                    Self::refresh_row(&row, app, dark);
                    self.rows.insert(key.clone(), row);
                }
            }
        }
    }

    fn refresh_row(row: &AppRow, app: &AppEntry, dark: bool) {
        set_ui_icon(&row.icon, "app-generic", dark);
        row.blocked_badge.set_visible(app.blocked);
        set_ui_icon(&row.blocked_badge, "blocked", dark);
        row.row.set_subtitle(&format!(
            "↑ {} · ↓ {}  —  total ↑ {} ↓ {}",
            human_speed(app.speed_up),
            human_speed(app.speed_down),
            human_bytes(app.bytes_sent),
            human_bytes(app.bytes_recv),
        ));
        row.status.set_text(if app.blocked { "Blocked" } else { "Allowed" });
        if app.blocked {
            row.status.add_css_class("error");
            row.status.remove_css_class("dim-label");
        } else {
            row.status.remove_css_class("error");
            row.status.add_css_class("dim-label");
        }
        row.row.set_opacity(if app.blocked { 0.6 } else { 1.0 });
        row.rule_id.set(app.block_rule_id);
        row.updating.set(true);
        // Light-switch semantics: ON = allowed, OFF = blocked.
        row.switch.set_active(app.is_allowed());
        // Apps without a known exe cannot be blocked via rules.
        row.switch.set_sensitive(app.exe.is_some());
        row.updating.set(false);
    }

    fn create_row(&self, app: &AppEntry, cmd_tx: &mpsc::UnboundedSender<GuiCmd>) -> AppRow {
        let row = adw::ActionRow::new();
        row.set_title(&app.name);
        let icon = ui_image("app-generic", false);
        row.add_prefix(&icon);

        let blocked_badge = ui_image("blocked", false);
        blocked_badge.set_pixel_size(16);
        blocked_badge.set_visible(false);
        blocked_badge.set_valign(gtk::Align::Center);

        let status = gtk::Label::new(Some("Allowed"));
        status.set_valign(gtk::Align::Center);
        let switch = gtk::Switch::new();
        switch.set_valign(gtk::Align::Center);

        let suffix = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        suffix.append(&blocked_badge);
        suffix.append(&status);
        suffix.append(&switch);
        row.add_suffix(&suffix);

        let updating = Rc::new(Cell::new(false));
        let rule_id = Rc::new(Cell::new(app.block_rule_id));
        {
            let updating = updating.clone();
            let rule_id = rule_id.clone();
            let cmd_tx = cmd_tx.clone();
            let name = app.name.clone();
            let exe = app.exe.clone();
            switch.connect_state_set(move |_sw, active| {
                if updating.get() {
                    return glib::Propagation::Proceed;
                }
                if active {
                    // Switched ON: allow the app again.
                    if let Some(id) = rule_id.get() {
                        let _ = cmd_tx.send(GuiCmd::Unblock { rule_id: id });
                    }
                } else if let Some(exe) = &exe {
                    // Switched OFF: block the app.
                    let _ = cmd_tx.send(GuiCmd::Block {
                        name: name.clone(),
                        exe: exe.clone(),
                    });
                }
                glib::Propagation::Proceed
            });
        }

        self.list.append(&row);
        AppRow {
            row,
            icon,
            blocked_badge,
            status,
            switch,
            updating,
            rule_id,
        }
    }
}
