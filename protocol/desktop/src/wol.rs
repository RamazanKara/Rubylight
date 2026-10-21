use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

use crate::config::HostRecord;

fn magic_packet(mac: &str) -> Result<[u8; 102], String> {
    let bytes = mac.as_bytes();
    if bytes.len() != 17 || !matches!(bytes[2], b':' | b'-') {
        return Err("The PC has no valid Wake-on-LAN MAC address.".into());
    }
    let mut address = [0; 6];
    for (i, octet) in address.iter_mut().enumerate() {
        let offset = i * 3;
        if !bytes[offset..offset + 2].iter().all(u8::is_ascii_hexdigit)
            || (i < 5 && bytes[offset + 2] != bytes[2])
        {
            return Err("The PC has no valid Wake-on-LAN MAC address.".into());
        }
        *octet = u8::from_str_radix(&mac[offset..offset + 2], 16).unwrap();
    }
    let mut packet = [0xff; 102];
    for chunk in packet[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(&address);
    }
    Ok(packet)
}

fn ports(http_port: u16) -> Vec<u16> {
    let http_port = if http_port == 0 { 47989 } else { http_port };
    let mut ports = vec![9, 47009];
    ports.extend(
        [9, 10, 11, 13, 21]
            .into_iter()
            .filter_map(|offset| http_port.checked_add(offset)),
    );
    ports.sort_unstable();
    ports.dedup();
    ports
}

pub fn wake(host: &HostRecord) -> Result<(), String> {
    let packet = magic_packet(host.mac.as_deref().unwrap_or_default())?;
    let mut addresses = BTreeSet::from([IpAddr::V4(Ipv4Addr::BROADCAST)]);
    for address in std::iter::once(&host.address).chain(&host.other_addresses) {
        if let Ok(resolved) = (address.as_str(), 9).to_socket_addrs() {
            addresses.extend(resolved.map(|address| address.ip()));
        }
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| e.to_string())?;
    socket.set_broadcast(true).map_err(|e| e.to_string())?;
    let ipv6 = UdpSocket::bind("[::]:0").ok();
    let ports = ports(host.http_port);
    let mut sent = false;
    let mut error = String::from("No Wake-on-LAN destination could be reached.");
    for _ in 0..3 {
        for address in &addresses {
            let socket = if address.is_ipv4() {
                Some(&socket)
            } else {
                ipv6.as_ref()
            };
            let Some(socket) = socket else { continue };
            for port in &ports {
                match socket.send_to(&packet, SocketAddr::new(*address, *port)) {
                    Ok(_) => sent = true,
                    Err(e) => error = e.to_string(),
                }
            }
        }
    }
    if sent {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_repeats_the_mac_sixteen_times() {
        let packet = magic_packet("00:1a:2B:3c:4D:5e").unwrap();
        assert_eq!(&packet[..6], &[255; 6]);
        for chunk in packet[6..].chunks_exact(6) {
            assert_eq!(chunk, &[0, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        }
        assert_eq!(magic_packet("00-1a-2b-3c-4d-5e").unwrap(), packet);
        for mac in [
            "",
            "00:1a:2b:3c:4d",
            "00:1a-2b:3c:4d:5e",
            "00:1a:2b:3c:4d:zz",
            "é0:1a:2b:3c:4d:5",
        ] {
            assert!(magic_packet(mac).is_err());
        }
    }

    #[test]
    fn custom_ports_keep_fixed_wake_ports_and_do_not_wrap() {
        assert_eq!(ports(0), [9, 47009, 47998, 47999, 48000, 48002, 48010]);
        assert_eq!(ports(48989), [9, 47009, 48998, 48999, 49000, 49002, 49010]);
        assert_eq!(ports(65535), [9, 47009]);
    }
}
