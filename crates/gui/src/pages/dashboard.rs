//! Dashboard: network summary, live speeds, session totals, app counts.

use relm4::adw::{self, prelude::*};
use relm4::gtk;

use crate::fmt::{human_bytes, human_speed};
use crate::icons::{set_ui_icon, ui_image};
use crate::state::ClientState;

pub struct DashboardPage {
    container: gtk::Widget,
    network_row: adw::ActionRow,
    metered_badge: gtk::Label,
    gateway_row: adw::ActionRow,
    dns_row: adw::ActionRow,
    down_speed: gtk::Label,
    up_speed: gtk::Label,
    session_row: adw::ActionRow,
    active_row: adw::ActionRow,
    blocked_row: adw::ActionRow,
    /// Row icons, re-pointed at the right variant on theme change.
    icons: Vec<(gtk::Image, &'static str)>,
}

/// Build an ActionRow with exactly one prefix icon; the caller keeps
/// the image handle to refresh it on theme changes.
fn pref_row(
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

fn numeric_suffix(row: &adw::ActionRow) -> gtk::Label {
    let label = gtk::Label::new(Some("—"));
    label.add_css_class("numeric");
    label.set_valign(gtk::Align::Center);
    row.add_suffix(&label);
    label
}

impl DashboardPage {
    pub fn new() -> Self {
        let mut icons = Vec::new();

        let network = adw::PreferencesGroup::new();
        network.set_title("Network");
        let network_row = pref_row(&network, "connection", "Connection", &mut icons);
        let metered_badge = gtk::Label::new(Some("Metered"));
        metered_badge.add_css_class("accent");
        metered_badge.set_valign(gtk::Align::Center);
        metered_badge.set_visible(false);
        network_row.add_suffix(&metered_badge);
        let gateway_row = pref_row(&network, "gateway", "Gateway", &mut icons);
        let dns_row = pref_row(&network, "dns", "DNS servers", &mut icons);

        let traffic = adw::PreferencesGroup::new();
        traffic.set_title("Traffic");
        let down_row = pref_row(&traffic, "download", "Download", &mut icons);
        let down_speed = numeric_suffix(&down_row);
        let up_row = pref_row(&traffic, "upload", "Upload", &mut icons);
        let up_speed = numeric_suffix(&up_row);
        let session_row = pref_row(&traffic, "session", "This session", &mut icons);

        let apps = adw::PreferencesGroup::new();
        apps.set_title("Applications");
        let active_row = pref_row(&apps, "apps", "Active apps", &mut icons);
        let blocked_row = pref_row(&apps, "blocked", "Blocked apps", &mut icons);

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
            metered_badge,
            gateway_row,
            dns_row,
            down_speed,
            up_speed,
            session_row,
            active_row,
            blocked_row,
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
        match &state.network {
            Some(net) => {
                let name = net
                    .ssid
                    .clone()
                    .or_else(|| net.primary_interface.clone())
                    .unwrap_or_else(|| "Not connected".into());
                self.network_row.set_subtitle(&name);
                self.metered_badge.set_visible(net.metered.unwrap_or(false));
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
