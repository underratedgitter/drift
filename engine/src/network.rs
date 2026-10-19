//! Select an actual local link, not a cellular/VPN default route, for invitations.
use std::net::{Ipv4Addr, UdpSocket};

pub fn lan_ipv4() -> String {
    let route = (|| {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        // UDP connect only chooses a route here; no datagram is sent.
        socket.connect("192.0.2.1:80").ok()?;
        match socket.local_addr().ok()?.ip() {
            std::net::IpAddr::V4(ip) => Some(ip),
            _ => None,
        }
    })();
    let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
    let candidates = interfaces
        .iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) => Some((interface.name.as_str(), ip)),
            _ => None,
        })
        .collect::<Vec<_>>();
    select(
        &candidates,
        route,
        cfg!(any(target_os = "android", target_os = "ios")),
    )
    .unwrap_or(Ipv4Addr::LOCALHOST)
    .to_string()
}

fn select(
    interfaces: &[(&str, Ipv4Addr)],
    route: Option<Ipv4Addr>,
    mobile: bool,
) -> Option<Ipv4Addr> {
    interfaces
        .iter()
        .filter_map(|(name, ip)| {
            let name = name.to_ascii_lowercase();
            if !(ip.is_private() || ip.is_link_local())
                || ip.is_loopback()
                || [
                    "rmnet", "ccmni", "pdp_ip", "utun", "tun", "tap", "awdl", "llw", "docker",
                    "veth", "vbox",
                ]
                .iter()
                .any(|prefix| name.starts_with(prefix))
            {
                return None;
            }
            let hotspot =
                name.starts_with("ap") || name.starts_with("swlan") || name.starts_with("bridge");
            let physical = name.starts_with("wlan")
                || name.starts_with("wl")
                || name.starts_with("en")
                || name.starts_with("eth")
                || name.contains("wi-fi")
                || name.contains("wireless");
            let rank = if mobile && hotspot {
                0
            } else if Some(*ip) == route {
                1
            } else if physical {
                2
            } else if hotspot {
                3
            } else {
                4
            };
            Some(((rank, u32::from(*ip)), *ip))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, ip)| ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ip(value: &str) -> Ipv4Addr {
        value.parse().unwrap()
    }
    #[test]
    fn android_hotspot_beats_cellular_default_route() {
        assert_eq!(
            select(
                &[
                    ("rmnet_data0", ip("10.51.2.3")),
                    ("ap0", ip("192.168.43.1"))
                ],
                Some(ip("10.51.2.3")),
                true
            ),
            Some(ip("192.168.43.1"))
        );
    }
    #[test]
    fn iphone_hotspot_beats_mobile_data_and_vpn() {
        assert_eq!(
            select(
                &[
                    ("pdp_ip0", ip("10.1.1.2")),
                    ("utun2", ip("10.8.0.2")),
                    ("bridge100", ip("172.20.10.1"))
                ],
                Some(ip("10.8.0.2")),
                true
            ),
            Some(ip("172.20.10.1"))
        );
    }
    #[test]
    fn laptop_prefers_routed_physical_link_and_ignores_vpn() {
        let addresses = [
            ("en0", ip("172.20.10.2")),
            ("utun3", ip("10.8.0.1")),
            ("bridge100", ip("192.168.64.1")),
        ];
        assert_eq!(
            select(&addresses, Some(ip("172.20.10.2")), false),
            Some(ip("172.20.10.2"))
        );
        assert_eq!(
            select(&addresses, Some(ip("10.8.0.1")), false),
            Some(ip("172.20.10.2"))
        );
    }
    #[test]
    fn never_advertises_cellular_public_or_loopback_as_lan() {
        assert_eq!(
            select(
                &[
                    ("rmnet0", ip("10.0.0.1")),
                    ("en0", ip("8.8.8.8")),
                    ("lo0", Ipv4Addr::LOCALHOST)
                ],
                Some(ip("10.0.0.1")),
                true
            ),
            None
        );
    }
}
