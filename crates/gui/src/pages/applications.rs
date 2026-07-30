//! Applications page: one row per app with live traffic and an
//! internet switch, grouped into "Allowed" and "Blocked" sections that
//! hide when empty. Each section is live-sorted by download bytes
//! (desc). "Light switch" model: switch ON = the app may use the
//! network, switch OFF = blocked. Toggling OFF sends a Block rule,
//! toggling ON removes it. The daemon's rule events flowing back are
//! the source of truth for the switch position and the row's group.

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
use crate::pages::{SectionList, SortKey};
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
    /// Which section the row currently lives in.
    blocked: Cell<bool>,
    /// Live sort key (recv desc, sent desc, name asc).
    sort_key: Rc<SortKey>,
}

pub struct AppsPage {
    container: gtk::Widget,
    allowed: SectionList,
    blocked: SectionList,
    empty_label: gtk::Label,
    rows: HashMap<String, AppRow>,
}

impl AppsPage {
    pub fn new() -> Self {
        let allowed = SectionList::new("Allowed");
        let blocked = SectionList::new("Blocked");
        blocked.root.set_visible(false);

        let empty_label = gtk::Label::new(Some("No applications seen yet"));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_top(24);
        empty_label.set_margin_bottom(24);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&allowed.root);
        content.append(&blocked.root);
        content.append(&empty_label);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(700);
        clamp.set_child(Some(&content));

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&clamp));

        Self {
            container: scroll.upcast(),
            allowed,
            blocked,
            empty_label,
            rows: HashMap::new(),
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    fn section(&self, blocked: bool) -> &SectionList {
        if blocked {
            &self.blocked
        } else {
            &self.allowed
        }
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
                self.section(row.blocked.get())
                    .remove(row.row.upcast_ref());
            }
        }

        // Field-level borrows so the row map can be mutably borrowed
        // alongside the sections.
        let pick = |blocked: bool| -> &SectionList {
            if blocked {
                &self.blocked
            } else {
                &self.allowed
            }
        };

        for (key, app) in &state.apps {
            match self.rows.get_mut(key) {
                Some(row) => {
                    // Group changed (rule round-tripped): move the
                    // existing row widget to the other section.
                    if row.blocked.get() != app.blocked {
                        pick(row.blocked.get()).remove(row.row.upcast_ref());
                        row.blocked.set(app.blocked);
                        pick(app.blocked).insert(row.row.upcast_ref(), row.sort_key.clone());
                    }
                    Self::refresh_row(row, app, dark);
                }
                None => {
                    let row = self.create_row(app, cmd_tx);
                    Self::refresh_row(&row, app, dark);
                    self.section(app.blocked)
                        .insert(row.row.upcast_ref(), row.sort_key.clone());
                    self.rows.insert(key.clone(), row);
                }
            }
        }

        // Re-sort live and hide empty sections.
        self.allowed.invalidate();
        self.blocked.invalidate();
        self.allowed.root.set_visible(!self.allowed.is_empty());
        self.blocked.root.set_visible(!self.blocked.is_empty());
        self.empty_label
            .set_visible(self.allowed.is_empty() && self.blocked.is_empty());
    }

    fn refresh_row(row: &AppRow, app: &AppEntry, dark: bool) {
        row.sort_key.primary.set(app.bytes_recv);
        row.sort_key.secondary.set(app.bytes_sent);
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

        AppRow {
            row,
            icon,
            blocked_badge,
            status,
            switch,
            updating,
            rule_id,
            blocked: Cell::new(app.blocked),
            sort_key: Rc::new(SortKey {
                primary: Cell::new(app.bytes_recv),
                secondary: Cell::new(app.bytes_sent),
                name: app.name.clone(),
            }),
        }
    }
}
