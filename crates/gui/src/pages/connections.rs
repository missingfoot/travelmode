//! Connections page: live list of tracked flows, grouped into TCP /
//! UDP / Other sections that hide when empty, each live-sorted by
//! total bytes (desc).

use std::collections::HashMap;
use std::rc::Rc;

use relm4::adw::{self, prelude::*};
use relm4::gtk;
use travelmode_core::types::Protocol;

use crate::fmt::human_bytes;
use crate::icons::{set_ui_icon, ui_image};
use crate::pages::{SectionList, SortKey};
use crate::state::ClientState;

struct ConnRow {
    row: adw::ActionRow,
    icon: gtk::Image,
    stats: gtk::Label,
    proto: Protocol,
    sort_key: Rc<SortKey>,
}

pub struct ConnsPage {
    container: gtk::Widget,
    tcp: SectionList,
    udp: SectionList,
    other: SectionList,
    empty_label: gtk::Label,
    rows: HashMap<String, ConnRow>,
}

impl ConnsPage {
    pub fn new() -> Self {
        let tcp = SectionList::new("TCP");
        let udp = SectionList::new("UDP");
        let other = SectionList::new("Other");
        udp.root.set_visible(false);
        other.root.set_visible(false);

        let empty_label = gtk::Label::new(Some("No connections yet"));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_top(24);
        empty_label.set_margin_bottom(24);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&tcp.root);
        content.append(&udp.root);
        content.append(&other.root);
        content.append(&empty_label);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(700);
        clamp.set_child(Some(&content));

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&clamp));

        Self {
            container: scroll.upcast(),
            tcp,
            udp,
            other,
            empty_label,
            rows: HashMap::new(),
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    fn section(&self, proto: Protocol) -> &SectionList {
        match proto {
            Protocol::Tcp => &self.tcp,
            Protocol::Udp => &self.udp,
            Protocol::Other => &self.other,
        }
    }

    pub fn update(&mut self, state: &ClientState, dark: bool) {
        // Drop rows for closed flows.
        let stale: Vec<String> = self
            .rows
            .keys()
            .filter(|k| !state.conns.contains_key(*k))
            .cloned()
            .collect();
        for key in stale {
            if let Some(row) = self.rows.remove(&key) {
                // A flow's protocol never changes for a given key.
                self.section(row.proto).remove(row.row.upcast_ref());
            }
        }

        for (key, conn) in &state.conns {
            let total = conn.bytes_sent + conn.bytes_recv;
            let stats_text = format!(
                "↑ {} ↓ {}",
                human_bytes(conn.bytes_sent),
                human_bytes(conn.bytes_recv)
            );
            match self.rows.get(key) {
                Some(row) => {
                    row.sort_key.primary.set(total);
                    row.stats.set_text(&stats_text);
                    set_ui_icon(&row.icon, "connections", dark);
                }
                None => {
                    let process =
                        conn.process_name.clone().unwrap_or_else(|| "unknown".into());
                    let row = adw::ActionRow::new();
                    row.set_title(&format!(
                        "{process} · {}",
                        format!("{:?}", conn.protocol).to_lowercase()
                    ));
                    row.set_subtitle(&format!("{}:{}", conn.remote_addr, conn.remote_port));
                    let icon = ui_image("connections", dark);
                    row.add_prefix(&icon);
                    let stats = gtk::Label::new(Some(&stats_text));
                    stats.add_css_class("numeric");
                    stats.set_valign(gtk::Align::Center);
                    row.add_suffix(&stats);
                    let conn_row = ConnRow {
                        row: row.clone(),
                        icon,
                        stats,
                        proto: conn.protocol,
                        sort_key: Rc::new(SortKey {
                            primary: std::cell::Cell::new(total),
                            secondary: std::cell::Cell::new(0),
                            name: format!("{process}{}", conn.remote_addr),
                        }),
                    };
                    self.section(conn.protocol)
                        .insert(row.upcast_ref(), conn_row.sort_key.clone());
                    self.rows.insert(key.clone(), conn_row);
                }
            }
        }

        // Re-sort live and hide empty sections.
        for section in [&self.tcp, &self.udp, &self.other] {
            section.invalidate();
            section.root.set_visible(!section.is_empty());
        }
        self.empty_label.set_visible(
            self.tcp.is_empty() && self.udp.is_empty() && self.other.is_empty(),
        );
    }
}
