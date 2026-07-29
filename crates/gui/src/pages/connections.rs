//! Connections page: live list of tracked flows.

use std::collections::HashMap;

use relm4::adw::{self, prelude::*};
use relm4::gtk;

use crate::fmt::human_bytes;
use crate::state::ClientState;

struct ConnRow {
    row: adw::ActionRow,
    stats: gtk::Label,
}

pub struct ConnsPage {
    container: gtk::Widget,
    list: gtk::ListBox,
    rows: HashMap<String, ConnRow>,
}

impl ConnsPage {
    pub fn new() -> Self {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let placeholder = gtk::Label::new(Some("No connections yet"));
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

    pub fn update(&mut self, state: &ClientState) {
        // Drop rows for closed flows.
        let stale: Vec<String> = self
            .rows
            .keys()
            .filter(|k| !state.conns.contains_key(*k))
            .cloned()
            .collect();
        for key in stale {
            if let Some(row) = self.rows.remove(&key) {
                self.list.remove(&row.row);
            }
        }
        for (key, conn) in &state.conns {
            let stats_text = format!(
                "↑ {} ↓ {}",
                human_bytes(conn.bytes_sent),
                human_bytes(conn.bytes_recv)
            );
            match self.rows.get(key) {
                Some(row) => row.stats.set_text(&stats_text),
                None => {
                    let process =
                        conn.process_name.clone().unwrap_or_else(|| "unknown".into());
                    let row = adw::ActionRow::new();
                    row.set_title(&format!(
                        "{process} · {}",
                        format!("{:?}", conn.protocol).to_lowercase()
                    ));
                    row.set_subtitle(&format!("{}:{}", conn.remote_addr, conn.remote_port));
                    row.add_prefix(&gtk::Image::from_icon_name(
                        "network-transmit-receive-symbolic",
                    ));
                    let stats = gtk::Label::new(Some(&stats_text));
                    stats.add_css_class("numeric");
                    stats.set_valign(gtk::Align::Center);
                    row.add_suffix(&stats);
                    self.list.append(&row);
                    self.rows.insert(key.clone(), ConnRow { row, stats });
                }
            }
        }
    }
}
