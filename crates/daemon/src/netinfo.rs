//! Current network snapshot: interfaces, addresses, default gateway,
//! DNS servers, and (via NetworkManager over D-Bus) Wi-Fi SSID and the
//! metered flag.
//!
//! Everything degrades gracefully: netlink failures, missing
//! /etc/resolv.conf, and an absent NetworkManager all yield partial or
//! empty results, never an error.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;

use futures::TryStreamExt;
use netlink_packet_route::address::AddressAttribute;
use netlink_packet_route::link::{LinkAttribute, LinkFlags};
use netlink_packet_route::route::{RouteAddress, RouteAttribute, RouteHeader, RouteMessage, RouteType};
use tracing::{debug, warn};
use travelmode_core::types::{InterfaceInfo, InterfaceKind, NetworkInfo};

/// Produce a full network snapshot. Never fails hard.
pub async fn snapshot() -> NetworkInfo {
    let (interfaces, gateway, primary_interface) = match netlink_info().await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "netlink query failed; interface info unavailable");
            (Vec::new(), None, None)
        }
    };
    let dns_servers = read_dns_servers(Path::new("/etc/resolv.conf"));
    let (ssid, metered) = nm_info().await;
    NetworkInfo {
        interfaces,
        gateway,
        dns_servers,
        ssid,
        metered,
        primary_interface,
    }
}

// ---------------------------------------------------------------- netlink

type NetlinkInfo = (Vec<InterfaceInfo>, Option<IpAddr>, Option<String>);
type BoxError = Box<dyn std::error::Error + Send + Sync>;

async fn netlink_info() -> Result<NetlinkInfo, BoxError> {
    let (connection, handle, _) = rtnetlink::new_connection()?;
    tokio::spawn(connection);

    // Links: index → partial InterfaceInfo.
    let mut links: HashMap<u32, InterfaceInfo> = HashMap::new();
    let mut link_stream = handle.link().get().execute();
    while let Some(msg) = link_stream.try_next().await? {
        let index = msg.header.index;
        let mut name = String::new();
        let mut mac = None;
        for attr in msg.attributes {
            match attr {
                LinkAttribute::IfName(n) => name = n,
                LinkAttribute::Address(bytes) => mac = format_mac(&bytes),
                _ => {}
            }
        }
        if name.is_empty() {
            continue;
        }
        let flags = msg.header.flags;
        let kind = classify_interface(&name, flags);
        links.insert(
            index,
            InterfaceInfo {
                name: name.clone(),
                kind,
                mac,
                addrs: Vec::new(),
                is_up: flags.contains(LinkFlags::Up),
            },
        );
    }

    // Addresses, attached to their interface by index.
    let mut addr_stream = handle.address().get().execute();
    while let Some(msg) = addr_stream.try_next().await? {
        let Some(iface) = links.get_mut(&msg.header.index) else {
            continue;
        };
        let mut link_local: Option<IpAddr> = None;
        for attr in msg.attributes {
            match attr {
                AddressAttribute::Local(ip) => iface.addrs.push(ip),
                AddressAttribute::Address(ip) => link_local = Some(ip),
                _ => {}
            }
        }
        // Point-to-point links only carry Address; keep it if no Local.
        if let Some(ip) = link_local {
            if !iface.addrs.contains(&ip) {
                iface.addrs.push(ip);
            }
        }
    }

    // Default route from the main table → gateway + primary interface.
    let mut gateway = None;
    let mut primary_interface = None;
    let mut route_stream = handle.route().get(RouteMessage::default()).execute();
    while let Some(msg) = route_stream.try_next().await? {
        if msg.header.table != RouteHeader::RT_TABLE_MAIN
            || msg.header.kind != RouteType::Unicast
            || msg.header.destination_prefix_length != 0
        {
            continue;
        }
        let mut gw = None;
        let mut oif = None;
        for attr in msg.attributes {
            match attr {
                RouteAttribute::Gateway(RouteAddress::Inet(v4)) => {
                    gw = Some(IpAddr::V4(v4));
                }
                RouteAttribute::Gateway(RouteAddress::Inet6(v6)) => {
                    gw = Some(IpAddr::V6(v6));
                }
                RouteAttribute::Oif(idx) => oif = Some(idx),
                _ => {}
            }
        }
        // Prefer the IPv4 default route; take whatever appears first.
        if gateway.is_none() {
            gateway = gw;
            primary_interface = oif.and_then(|idx| links.get(&idx)).map(|i| i.name.clone());
        }
        if matches!(gw, Some(IpAddr::V4(_))) {
            gateway = gw;
            primary_interface = oif.and_then(|idx| links.get(&idx)).map(|i| i.name.clone());
            break;
        }
    }

    let interfaces: Vec<InterfaceInfo> = links.into_values().collect();
    Ok((interfaces, gateway, primary_interface))
}

fn format_mac(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 6 {
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

fn classify_interface(name: &str, flags: LinkFlags) -> InterfaceKind {
    if flags.contains(LinkFlags::Loopback) {
        return InterfaceKind::Loopback;
    }
    if Path::new("/sys/class/net").join(name).join("wireless").exists() {
        return InterfaceKind::Wifi;
    }
    if name.starts_with("tun") || name.starts_with("wg") || name.starts_with("ppp") {
        return InterfaceKind::Vpn;
    }
    if name.starts_with('e') {
        // eth0, enp3s0, ...
        return InterfaceKind::Ethernet;
    }
    InterfaceKind::Other
}

// ------------------------------------------------------------------- DNS

fn read_dns_servers(path: &Path) -> Vec<IpAddr> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_resolv_conf(&text),
        Err(e) => {
            debug!(path = %path.display(), error = %e, "cannot read resolv.conf");
            Vec::new()
        }
    }
}

/// Parse `nameserver` lines from resolv.conf contents.
fn parse_resolv_conf(text: &str) -> Vec<IpAddr> {
    let mut servers = Vec::new();
    for line in text.lines() {
        let line = line.split(['#', ';']).next().unwrap_or("").trim();
        let mut fields = line.split_whitespace();
        if fields.next() == Some("nameserver") {
            if let Some(Ok(ip)) = fields.next().map(str::parse) {
                servers.push(ip);
            }
        }
    }
    servers
}

// --------------------------------------------------------- NetworkManager

/// SSID + metered flag from NetworkManager. (None, None) on any failure.
async fn nm_info() -> (Option<String>, Option<bool>) {
    match nm_info_inner().await {
        Ok(v) => v,
        Err(e) => {
            debug!(error = %e, "NetworkManager query failed");
            (None, None)
        }
    }
}

async fn nm_info_inner() -> zbus::Result<(Option<String>, Option<bool>)> {
    use zbus::zvariant::OwnedObjectPath;

    let conn = zbus::Connection::system().await?;
    let nm = zbus::Proxy::new(
        &conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await?;

    let active: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections").await?;
    let mut ssid = None;
    let mut metered = None;

    for path in active {
        let ac = zbus::Proxy::new(
            &conn,
            "org.freedesktop.NetworkManager",
            path.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .await?;
        let conn_type: String = match ac.get_property("Type").await {
            Ok(t) => t,
            Err(_) => continue,
        };
        // Metered: 0 unknown, 1 yes, 2 no, 3 guess-yes, 4 guess-no.
        if metered.is_none() {
            if let Ok(m) = ac.get_property::<u32>("Metered").await {
                metered = Some(matches!(m, 1 | 3));
            }
        }
        if ssid.is_none() && conn_type == "802-11-wireless" {
            let devices: Vec<OwnedObjectPath> = match ac.get_property("Devices").await {
                Ok(d) => d,
                Err(_) => continue,
            };
            for dev_path in devices {
                let dev = zbus::Proxy::new(
                    &conn,
                    "org.freedesktop.NetworkManager",
                    dev_path.as_str(),
                    "org.freedesktop.NetworkManager.Device",
                )
                .await?;
                let ap_path: OwnedObjectPath = match dev.get_property("ActiveAccessPoint").await {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if ap_path.as_str() == "/" {
                    continue; // no active AP
                }
                let ap = zbus::Proxy::new(
                    &conn,
                    "org.freedesktop.NetworkManager",
                    ap_path.as_str(),
                    "org.freedesktop.NetworkManager.AccessPoint",
                )
                .await?;
                if let Ok(raw) = ap.get_property::<Vec<u8>>("Ssid").await {
                    ssid = Some(String::from_utf8_lossy(&raw).into_owned());
                    break;
                }
            }
        }
    }
    Ok((ssid, metered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_resolv_conf() {
        let text = "# Generated by NetworkManager\n\
                    search lan example.com\n\
                    nameserver 192.168.1.1\n\
                    nameserver 1.1.1.1 # fallback\n\
                    ; nameserver 9.9.9.9\n\
                    nameserver fe80::1\n\
                    options edns0\n";
        let servers = parse_resolv_conf(text);
        assert_eq!(
            servers,
            vec![
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
                IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            ]
        );
    }

    #[test]
    fn empty_resolv_conf_yields_nothing() {
        assert!(parse_resolv_conf("").is_empty());
        assert!(parse_resolv_conf("search lan\noptions edns0\n").is_empty());
        assert!(parse_resolv_conf("nameserver not-an-ip\n").is_empty());
    }

    #[test]
    fn formats_mac_addresses() {
        assert_eq!(
            format_mac(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]),
            Some("de:ad:be:ef:00:01".to_string())
        );
        assert_eq!(format_mac(&[]), None);
        assert_eq!(format_mac(&[1, 2, 3]), None);
    }

    #[test]
    fn classifies_interfaces() {
        assert_eq!(
            classify_interface("lo", LinkFlags::Loopback),
            InterfaceKind::Loopback
        );
        assert_eq!(
            classify_interface("tun0", LinkFlags::empty()),
            InterfaceKind::Vpn
        );
        assert_eq!(
            classify_interface("wg-home", LinkFlags::empty()),
            InterfaceKind::Vpn
        );
        assert_eq!(
            classify_interface("ppp0", LinkFlags::empty()),
            InterfaceKind::Vpn
        );
        assert_eq!(
            classify_interface("enp3s0", LinkFlags::empty()),
            InterfaceKind::Ethernet
        );
        assert_eq!(
            classify_interface("docker0", LinkFlags::empty()),
            InterfaceKind::Other
        );
    }
}
