//! Darwin's NET_RT_IFLIST2 snapshot includes 64-bit totals, including utun links.
//! No root privileges or external commands are required.

use std::io;
use std::mem::size_of;
use std::ptr;

use super::{InterfaceFilter, InterfaceStats};

pub(super) fn read_interfaces(filter: &InterfaceFilter) -> io::Result<Vec<InterfaceStats>> {
    // Interfaces can appear between the size query and the read. Retry ENOMEM
    // with a freshly queried size rather than publishing a partial snapshot.
    for _ in 0..3 {
        let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
        let mut len = 0;
        // SAFETY: mib and len are valid writable objects; null oldp queries size.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                ptr::null_mut(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0u8; len];
        // SAFETY: bytes has len initialized bytes and sysctl receives its capacity.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                bytes.as_mut_ptr().cast(),
                &mut len,
                ptr::null_mut(),
                0,
            )
        } == 0
        {
            bytes.truncate(len);
            return parse_snapshot(&bytes, filter);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOMEM) {
            return Err(error);
        }
    }
    Err(io::Error::other(
        "interface list kept growing during sysctl",
    ))
}

fn invalid_snapshot() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid routing interface snapshot",
    )
}

fn parse_snapshot(mut bytes: &[u8], filter: &InterfaceFilter) -> io::Result<Vec<InterfaceStats>> {
    let mut interfaces = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(invalid_snapshot());
        }
        let len = usize::from(u16::from_ne_bytes([bytes[0], bytes[1]]));
        if len < 4 || len > bytes.len() {
            return Err(invalid_snapshot());
        }
        let (message, rest) = bytes.split_at(len);
        bytes = rest;
        if message[3] != libc::RTM_IFINFO2 as u8 {
            continue;
        }
        if message.len() < size_of::<libc::if_msghdr2>() {
            return Err(invalid_snapshot());
        }
        // SAFETY: length checked above; this C struct contains only integer
        // fields. read_unaligned handles the byte buffer's alignment.
        let header = unsafe { ptr::read_unaligned(message.as_ptr().cast::<libc::if_msghdr2>()) };
        let mut addresses = &message[size_of::<libc::if_msghdr2>()..];
        for slot in 0..libc::RTAX_MAX {
            if header.ifm_addrs & (1 << slot) == 0 {
                continue;
            }
            let len = usize::from(*addresses.first().ok_or_else(invalid_snapshot)?);
            // Darwin routing sockaddrs are rounded to a 32-bit boundary.
            let padded = (len.max(1) + 3) & !3;
            if padded > addresses.len() {
                return Err(invalid_snapshot());
            }
            let address = &addresses[..len];
            addresses = &addresses[padded..];
            if slot != libc::RTAX_IFP {
                continue;
            }
            // sockaddr_dl: len, family, index(u16), type, nlen, alen, slen,
            // followed by the interface name and link-layer address.
            if address.len() < 8 || address[1] != libc::AF_LINK as u8 {
                return Err(invalid_snapshot());
            }
            let name_end = 8 + usize::from(address[5]);
            let mac_end = name_end + usize::from(address[6]);
            if mac_end > address.len() {
                return Err(invalid_snapshot());
            }
            let name =
                std::str::from_utf8(&address[8..name_end]).map_err(|_| invalid_snapshot())?;
            if filter.keep(name) {
                let mac = address[name_end..mac_end]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":");
                interfaces.push(interface_stats(name, mac, &header));
            }
        }
    }
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(interfaces)
}

fn interface_stats(name: &str, mac: String, header: &libc::if_msghdr2) -> InterfaceStats {
    let data = header.ifm_data;
    let kind = if name == "plower0" || name.starts_with("inferos") || name.starts_with("utun") {
        "tunnel".into()
    } else {
        match data.ifi_type {
            6 => "ether".into(),
            24 => "loopback".into(),
            131 => "tunnel".into(),
            n => n.to_string(),
        }
    };
    InterfaceStats {
        name: name.into(),
        kind,
        mac,
        rx_bytes: Some(data.ifi_ibytes),
        tx_bytes: Some(data.ifi_obytes),
        rx_packets: Some(data.ifi_ipackets),
        tx_packets: Some(data.ifi_opackets),
        rx_errors: Some(data.ifi_ierrors),
        tx_errors: Some(data.ifi_oerrors),
        rx_drops: Some(data.ifi_iqdrops),
        tx_drops: u64::try_from(header.ifm_snd_drops).ok(),
        up: header.ifm_flags & (libc::IFF_UP | libc::IFF_RUNNING)
            == (libc::IFF_UP | libc::IFF_RUNNING),
        // IFF_RUNNING is driver readiness, not physical carrier detection.
        carrier: None,
        mtu: Some(u64::from(data.ifi_mtu)),
        speed_bytes: (data.ifi_baudrate > 0).then_some(data.ifi_baudrate / 8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(name: &str) -> Vec<u8> {
        use std::mem::offset_of;
        let addr_len = 8 + name.len();
        let padded = (addr_len + 3) & !3;
        let len = size_of::<libc::if_msghdr2>() + padded;
        let mut bytes = vec![0; len];
        let mut put = |offset: usize, value: &[u8]| {
            bytes[offset..offset + value.len()].copy_from_slice(value);
        };
        put(0, &(len as u16).to_ne_bytes());
        put(3, &[libc::RTM_IFINFO2 as u8]);
        put(
            offset_of!(libc::if_msghdr2, ifm_addrs),
            &libc::RTA_IFP.to_ne_bytes(),
        );
        put(
            offset_of!(libc::if_msghdr2, ifm_flags),
            &(libc::IFF_UP | libc::IFF_RUNNING).to_ne_bytes(),
        );
        put(
            offset_of!(libc::if_msghdr2, ifm_snd_drops),
            &3i32.to_ne_bytes(),
        );
        let data = offset_of!(libc::if_msghdr2, ifm_data);
        put(
            data + offset_of!(libc::if_data64, ifi_ibytes),
            &((1u64 << 40) + 123).to_ne_bytes(),
        );
        put(
            data + offset_of!(libc::if_data64, ifi_obytes),
            &((1u64 << 41) + 456).to_ne_bytes(),
        );
        put(
            data + offset_of!(libc::if_data64, ifi_iqdrops),
            &7u64.to_ne_bytes(),
        );
        put(
            data + offset_of!(libc::if_data64, ifi_mtu),
            &1380u32.to_ne_bytes(),
        );
        let addr = &mut bytes[size_of::<libc::if_msghdr2>()..];
        addr[0] = addr_len as u8;
        addr[1] = libc::AF_LINK as u8;
        addr[5] = name.len() as u8;
        addr[8..addr_len].copy_from_slice(name.as_bytes());
        bytes
    }

    #[test]
    fn tunnel_snapshot_preserves_64_bit_totals_and_filters_loopback() {
        let mut bytes = Vec::new();
        for name in ["utun9", "plower0", "lo0", "inferos42"] {
            bytes.extend(message(name));
        }
        let interfaces = parse_snapshot(&bytes, &InterfaceFilter::default()).unwrap();
        assert_eq!(
            interfaces
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>(),
            ["inferos42", "plower0", "utun9"]
        );
        for i in interfaces {
            assert_eq!(i.rx_bytes, Some((1u64 << 40) + 123));
            assert_eq!(i.tx_bytes, Some((1u64 << 41) + 456));
            assert_eq!((i.rx_drops, i.tx_drops), (Some(7), Some(3)));
            assert_eq!(i.kind, "tunnel");
            assert!(i.up);
            assert_eq!(i.carrier, None);
            assert_eq!(i.speed_bytes, None);
        }
    }

    #[test]
    fn malformed_snapshots_fail_instead_of_looping_or_reading_past_end() {
        let bytes = message("utun0");
        for end in 1..bytes.len() {
            assert!(parse_snapshot(&bytes[..end], &InterfaceFilter::default()).is_err());
        }
        assert!(parse_snapshot(&[0; 4], &InterfaceFilter::default()).is_err());
    }

    #[test]
    fn administratively_down_tunnel_is_not_up() {
        let mut bytes = message("plower0");
        let offset = std::mem::offset_of!(libc::if_msghdr2, ifm_flags);
        bytes[offset..offset + 4].copy_from_slice(&libc::IFF_RUNNING.to_ne_bytes());
        let interfaces = parse_snapshot(&bytes, &InterfaceFilter::default()).unwrap();
        assert!(!interfaces[0].up);
    }

    #[test]
    fn live_macos_snapshot_contains_loopback_when_included() {
        let all = read_interfaces(&InterfaceFilter::new(vec!["lo0".into()])).unwrap();
        let lo = all
            .iter()
            .find(|i| i.name == "lo0")
            .expect("macOS loopback");
        assert!(lo.up);
        assert!(lo.rx_bytes.is_some() && lo.tx_bytes.is_some());
        assert_eq!(lo.kind, "loopback");
    }
}
