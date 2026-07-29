//! Dashboard: network summary, live speeds, session totals, app counts.

use relm4::adw::{self, prelude::*};
use relm4::gtk;

use crate::fmt::{human_bytes, human_speed};
use crate::state::ClientState;

pub struct DashboardPage {
    container: gtk::Widget,
    network_row: adw::ActionRow,
    network_icon: gtk::Image,
    metered_badge: gtk::Label,
    gateway_row: adw::ActionRow,
    dns_row: adw::ActionRow,
    down_speed: gtk::Label,
    up_speed: gtk::Label,
    session_row: adw::ActionRow,
    active_row: adw::ActionRow,
    blocked_row: adw::ActionRow,
}

fn pref_row(group: &adw::PreferencesGroup, icon: &str, title: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    group.add(&row);
    row
}

fn numeric_suffix(row: &adw::ActionRow) -> gtk::Label {
    let label = gtk::Label::new(Some("—"));
    label.add_css_class("numeric");
    label.set_valign(gtk::Align::Center);
    row.add_suffix(&label);
    label
}

impl DashboardPage {
    pub fn new() -> Self {
        let network = adw::PreferencesGroup::new();
        network.set_title("Network");
        let network_row = pref_row(&network, "network-wired-symbolic", "Connection");
        let network_icon = gtk::Image::from_icon_name("network-wired-symbolic");
        network_row.add_prefix(&network_icon);
        let metered_badge = gtk::Label::new(Some("Metered"));
        metered_badge.add_css_class("accent");
        metered_badge.set_valign(gtk::Align::Center);
        metered_badge.set_visible(false);
        network_row.add_suffix(&metered_badge);
        let gateway_row = pref_row(&network, "network-server-symbolic", "Gateway");
        let dns_row = pref_row(&network, "preferences-system-network-symbolic", "DNS servers");

        let traffic = adw::PreferencesGroup::new();
        traffic.set_title("Traffic");
        let down_row = pref_row(&traffic, "go-bottom-symbolic", "Download");
        let down_speed = numeric_suffix(&down_row);
        let up_row = pref_row(&traffic, "go-top-symbolic", "Upload");
        let up_speed = numeric_suffix(&up_row);
        let session_row = pref_row(&traffic, "hourglass-symbolic", "This session");

        let apps = adw::PreferencesGroup::new();
        apps.set_title("Applications");
        let active_row = pref_row(&apps, "application-x-executable-symbolic", "Active apps");
        let blocked_row = pref_row(&apps, "action-unavailable-symbolic", "Blocked apps");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.append(&network);
        content.append(&traffic);
        content.append(&apps);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(700);
        clamp.set_child(Some(&content));

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&clamp));

        Self {
            container: scroll.upcast(),
            network_row,
            network_icon,
            metered_badge,
            gateway_row,
            dns_row,
            down_speed,
            up_speed,
            session_row,
            active_row,
            blocked_row,
        }
    }

    pub fn container(&self) -> &gtk::Widget {
        &self.container
    }

    pub fn update(&self, state: &ClientState) {
        match &state.network {
            Some(net) => {
                let name = net
                    .ssid
                    .clone()
                    .or_else(|| net.primary_interface.clone())
                    .unwrap_or_else(|| "Not connected".into());
                self.network_row.set_subtitle(&name);
                self.network_icon.set_icon_name(Some(if net.ssid.is_some() {
                    "network-wireless-symbolic"
                } else {
                    "network-wired-symbolic"
                }));
                self.metered_badge
                    .set_visible(net.metered.unwrap_or(false));
                self.gateway_row.set_subtitle(
                    &net.gateway
                        .map(|g| g.to_string())
                        .unwrap_or_else(|| "—".into()),
                );
                self.dns_row.set_subtitle(&if net.dns_servers.is_empty() {
                    "—".into()
                } else {
                    net.dns_servers
                        .iter()
                        .map(|d| d.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                });
            }
            None => {
                self.network_row.set_subtitle("—");
                self.gateway_row.set_subtitle("—");
                self.dns_row.set_subtitle("—");
                self.metered_badge.set_visible(false);
            }
        }
        self.down_speed.set_text(&human_speed(state.speed_down));
        self.up_speed.set_text(&human_speed(state.speed_up));
        self.session_row.set_subtitle(&format!(
            "↓ {} · ↑ {}",
            human_bytes(state.total_down),
            human_bytes(state.total_up)
        ));
        self.active_row
            .set_subtitle(&state.active_app_count().to_string());
        self.blocked_row
            .set_subtitle(&state.blocked_app_count().to_string());
    }
}
