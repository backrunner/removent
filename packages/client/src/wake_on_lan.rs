//! Manual UDP Wake-on-LAN. Sending a packet is not evidence that the host woke.
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeOnLan {
    pub mac_address: String,
    pub destination: Ipv4Addr,
    pub port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum WakeError {
    #[error("Enter a unicast MAC address such as 02:11:22:33:44:55")]
    MacAddress,
    #[error("Enter an IPv4 broadcast address or reachable IPv4 address")]
    Destination,
    #[error("Enter a UDP port from 1 to 65535")]
    Port,
    #[error("Could not send the wake request: {0}")]
    Send(#[from] std::io::Error),
}

fn parse_mac(value: &str) -> Result<[u8; 6], WakeError> {
    let value = value.trim();
    let compact = if value.len() == 12 && value.is_ascii() {
        value.to_owned()
    } else {
        let separator = if value.contains(':') { ':' } else { '-' };
        let parts: Vec<_> = value.split(separator).collect();
        if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
            return Err(WakeError::MacAddress);
        }
        parts.concat()
    };
    if !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(WakeError::MacAddress);
    }
    let mut mac = [0; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
            .map_err(|_| WakeError::MacAddress)?;
    }
    if mac == [0; 6] || mac[0] & 1 != 0 {
        return Err(WakeError::MacAddress);
    }
    Ok(mac)
}

impl WakeOnLan {
    pub fn parse(mac: &str, destination: &str, port: &str) -> Result<Self, WakeError> {
        let mac = parse_mac(mac)?;
        let destination: Ipv4Addr = destination
            .trim()
            .parse()
            .map_err(|_| WakeError::Destination)?;
        if destination.is_unspecified() || destination.is_multicast() {
            return Err(WakeError::Destination);
        }
        let port = port.trim().parse::<u16>().map_err(|_| WakeError::Port)?;
        if port == 0 {
            return Err(WakeError::Port);
        }
        Ok(Self {
            mac_address: mac
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":"),
            destination,
            port,
        })
    }

    pub fn validated(&self) -> Result<Self, WakeError> {
        Self::parse(
            &self.mac_address,
            &self.destination.to_string(),
            &self.port.to_string(),
        )
    }

    /// Call on a worker thread. No DNS, authentication, or RVP relay is involved.
    pub fn send(&self) -> Result<(), WakeError> {
        let config = self.validated()?;
        let mac = parse_mac(&config.mac_address)?;
        let mut packet = [0xFF; 102];
        for chunk in packet[6..].as_chunks_mut::<6>().0 {
            chunk.copy_from_slice(&mac);
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let sent = socket.send_to(&packet, SocketAddrV4::new(config.destination, config.port))?;
        if sent != packet.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "incomplete wake packet",
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_a_real_udp_magic_packet() {
        let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let config = WakeOnLan::parse(
            "02-11-22-33-44-aa",
            "127.0.0.1",
            &receiver.local_addr().unwrap().port().to_string(),
        )
        .unwrap();
        assert_eq!(config.mac_address, "02:11:22:33:44:AA");
        config.send().unwrap();
        let mut packet = [0; 103];
        let (len, _) = receiver.recv_from(&mut packet).unwrap();
        assert_eq!(len, 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        for chunk in packet[6..len].as_chunks::<6>().0 {
            assert_eq!(chunk, &[2, 0x11, 0x22, 0x33, 0x44, 0xAA]);
        }
    }

    #[test]
    fn rejects_invalid_or_non_device_addresses() {
        for mac in [
            "",
            "00:00:00:00:00:00",
            "FF:FF:FF:FF:FF:FF",
            "01:11:22:33:44:55",
            "02:11-22:33:44:55",
            "02:11:22:33:44:gg",
            "０２１１２２３３４４５５",
        ] {
            assert!(matches!(
                WakeOnLan::parse(mac, "255.255.255.255", "9"),
                Err(WakeError::MacAddress)
            ));
        }
        for destination in ["example.com", "::1", "0.0.0.0", "224.0.0.1"] {
            assert!(matches!(
                WakeOnLan::parse("021122334455", destination, "9"),
                Err(WakeError::Destination)
            ));
        }
        for port in ["0", "65536", "-1", "abc"] {
            assert!(matches!(
                WakeOnLan::parse("021122334455", "255.255.255.255", port),
                Err(WakeError::Port)
            ));
        }
        assert!(WakeOnLan::parse("021122334455", "192.168.1.255", "9").is_ok());
    }
}
