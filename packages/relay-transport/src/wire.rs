use bytes::{BufMut, Bytes, BytesMut};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub const HEADER: usize = 16;
pub const MAX_PACKET: usize = 2048;
const MAX_PENDING: usize = 256;

pub fn route(bytes: &[u8]) -> Option<u64> {
    if bytes.len() <= HEADER {
        return None;
    }
    Some(u64::from_be_bytes(bytes[..8].try_into().ok()?))
}

/// Fragment only when path MTU requires it. No reliable outer stream: lost
/// fragments are recovered by the original end-to-end QUIC connection.
pub async fn send(
    conn: &quinn::Connection,
    route: u64,
    sequence: &mut u32,
    packet: &[u8],
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !packet.is_empty() && packet.len() <= MAX_PACKET,
        "Tunnel packet exceeds MTU"
    );
    // Both outer paths can have different MTUs. Stay below QUIC's guaranteed
    // 1200-byte minimum so the relay can forward without re-fragmenting.
    let chunk = conn
        .max_datagram_size()
        .unwrap_or(0)
        .min(1100)
        .saturating_sub(HEADER);
    anyhow::ensure!(chunk >= 256, "Relay DATAGRAM unavailable");
    let seq = *sequence;
    *sequence = sequence.wrapping_add(1);
    for (i, part) in packet.chunks(chunk).enumerate() {
        let mut wire = BytesMut::with_capacity(HEADER + part.len());
        wire.put_u64(route);
        wire.put_u32(seq);
        wire.put_u16((i * chunk) as u16);
        wire.put_u16(packet.len() as u16);
        wire.extend_from_slice(part);
        forward(conn, wire.freeze()).await?;
    }
    Ok(())
}

pub async fn forward(conn: &quinn::Connection, packet: Bytes) -> anyhow::Result<()> {
    // Propagate brief congestion rather than dropping entire bursts of inner
    // QUIC packets and forcing both congestion controllers into loss recovery.
    // A slow destination still has a fixed deadline and bounded queue.
    if let Ok(result) =
        tokio::time::timeout(Duration::from_millis(50), conn.send_datagram_wait(packet)).await
    {
        result?;
    }
    Ok(())
}

struct Partial {
    created: Instant,
    data: Vec<u8>,
    present: Vec<bool>,
    received: usize,
}

#[derive(Default)]
pub struct Assembler {
    pending: HashMap<(u64, u32), Partial>,
}

impl Assembler {
    pub fn receive(&mut self, packet: Bytes) -> Option<(u64, Bytes)> {
        let route = route(&packet)?;
        let seq = u32::from_be_bytes(packet[8..12].try_into().ok()?);
        let offset = u16::from_be_bytes(packet[12..14].try_into().ok()?) as usize;
        let total = u16::from_be_bytes(packet[14..16].try_into().ok()?) as usize;
        let payload = packet.slice(HEADER..);
        if total == 0 || total > MAX_PACKET || offset + payload.len() > total {
            return None;
        }
        if offset == 0 && payload.len() == total {
            return Some((route, payload));
        }
        let key = (route, seq);
        if !self.pending.contains_key(&key) {
            self.pending
                .retain(|_, p| p.created.elapsed() < Duration::from_secs(2));
            if self.pending.len() >= MAX_PENDING {
                let oldest = *self
                    .pending
                    .iter()
                    .min_by_key(|(_, p)| p.created)
                    .unwrap()
                    .0;
                self.pending.remove(&oldest);
            }
        }
        let part = self.pending.entry(key).or_insert_with(|| Partial {
            created: Instant::now(),
            data: vec![0; total],
            present: vec![false; total],
            received: 0,
        });
        if part.data.len() != total {
            self.pending.remove(&key);
            return None;
        }
        for (i, byte) in payload.iter().enumerate() {
            let index = offset + i;
            if part.present[index] && part.data[index] != *byte {
                self.pending.remove(&key);
                return None;
            }
            if !part.present[index] {
                part.received += 1;
            }
            part.present[index] = true;
            part.data[index] = *byte;
        }
        if part.received == total {
            return Some((route, Bytes::from(self.pending.remove(&key)?.data)));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fragment(seq: u32, offset: u16, total: u16, data: &[u8]) -> Bytes {
        let mut b = BytesMut::new();
        b.put_u64(1);
        b.put_u32(seq);
        b.put_u16(offset);
        b.put_u16(total);
        b.extend_from_slice(data);
        b.freeze()
    }
    #[test]
    fn reorder_duplicate_invalid_and_bounded_fragments() {
        let mut a = Assembler::default();
        assert!(a.receive(fragment(1, 2, 4, b"cd")).is_none());
        assert!(a.receive(fragment(1, 2, 4, b"cd")).is_none());
        assert_eq!(a.receive(fragment(1, 0, 4, b"ab")).unwrap().1, b"abcd"[..]);
        assert!(a.receive(fragment(2, 3, 4, b"xx")).is_none());
        assert!(a.receive(fragment(2, 0, 65535, b"x")).is_none());
        for seq in 0..1000 {
            a.receive(fragment(seq, 0, 2048, b"x"));
        }
        assert!(a.pending.len() <= MAX_PENDING);
    }
}
