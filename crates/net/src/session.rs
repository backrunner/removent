//! Session connection wrappers: control-stream codec, pairing-stream codec, RVP handshake.

use crate::error::{NetError, Result};
use crate::pairing::PairingMsg;
use bytes::{BufMut, BytesMut};
use quinn::Connection;
use removent_proto::{
    ControlDecodeOutcome, ControlMsg, ErrorCode, HandshakeClient, HandshakeServer, MAGIC,
    MAX_CONTROL_MSG_LEN, PROTO_VERSION, PROTOCOL_NAME, decode_control, encode_control,
    negotiate_proto_version,
};
use std::time::Duration;
use tokio_util::codec::{Decoder, Encoder, Framed};

/// Client-side handshake timeout: the HandshakeServer reply must arrive within this
/// window. A busy host never accepts the control stream, and without a deadline the
/// 2s QUIC keepalive keeps the connection idle timeout from ever firing, hanging the
/// client forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);

/// Server-side accept timeout: bounds "accept the control stream + read the Hello".
/// Without it, a client that completes the QUIC handshake but then stalls before
/// sending the Hello holds the host's single session slot forever — the 2s QUIC
/// keepalive (both ends) keeps the connection idle timeout from ever firing, so one
/// stuck client would permanently busy-reject every later connection. 10s is
/// independent of the client's 12s reply deadline (HANDSHAKE_TIMEOUT): it covers
/// only the receive phase, and the reply write afterwards is immediate, so a
/// slow-but-honest client still gets its HandshakeServer reply within its budget.
const SERVER_ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

pub type ControlSink = Framed<quinn::SendStream, ControlCodec>;
pub type ControlSource = Framed<quinn::RecvStream, ControlCodec>;
pub type PairSink = Framed<quinn::SendStream, PairCodec>;
pub type PairSource = Framed<quinn::RecvStream, PairCodec>;

/// A live transport can keep acknowledging keepalives while its application
/// never consumes control bytes. Bound writes separately from QUIC idle time.
/// On error the caller must end the stream: a timed-out send may be partial.
pub async fn send_control(sink: &mut ControlSink, msg: ControlMsg) -> Result<()> {
    use futures::SinkExt;
    tokio::time::timeout(Duration::from_secs(10), sink.send(msg))
        .await
        .map_err(|_| NetError::Timeout)?
}

/// Control-stream item: a normal message or a skipped unknown variant (protocol.md §8).
#[derive(Debug, PartialEq)]
pub enum ControlItem {
    Msg(Box<ControlMsg>),
    Skipped,
}

#[derive(Debug, Default)]
pub struct ControlCodec;

impl Encoder<ControlMsg> for ControlCodec {
    type Error = NetError;
    fn encode(&mut self, item: ControlMsg, dst: &mut BytesMut) -> Result<()> {
        let wire = encode_control(&item).map_err(|e| NetError::Framing(e.to_string()))?;
        dst.put_slice(&wire);
        Ok(())
    }
}

impl Decoder for ControlCodec {
    type Item = ControlItem;
    type Error = NetError;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<ControlItem>> {
        if src.len() < 4 {
            return Ok(None);
        }
        let body_len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if body_len as u32 > MAX_CONTROL_MSG_LEN {
            return Err(NetError::Framing(format!(
                "control msg too large: {body_len}"
            )));
        }
        let total = 4 + body_len;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }
        let frame = src.split_to(total).freeze();
        match decode_control(&frame) {
            Ok((ControlDecodeOutcome::Msg(m), _)) => Ok(Some(ControlItem::Msg(m))),
            Ok((ControlDecodeOutcome::Skipped(_), _)) => Ok(Some(ControlItem::Skipped)),
            Err(e) => Err(NetError::Framing(e.to_string())),
        }
    }
}

#[derive(Debug, Default)]
pub struct PairCodec;

impl Encoder<PairingMsg> for PairCodec {
    type Error = NetError;
    fn encode(&mut self, item: PairingMsg, dst: &mut BytesMut) -> Result<()> {
        let body = postcard::to_allocvec(&item).map_err(|e| NetError::Framing(e.to_string()))?;
        dst.put_u32(body.len() as u32);
        dst.put_slice(&body);
        Ok(())
    }
}

impl Decoder for PairCodec {
    type Item = PairingMsg;
    type Error = NetError;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<PairingMsg>> {
        if src.len() < 4 {
            return Ok(None);
        }
        let body_len = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
        if body_len > 1 << 20 {
            return Err(NetError::Framing("pairing msg too large".into()));
        }
        let total = 4 + body_len;
        if src.len() < total {
            src.reserve(total - src.len());
            return Ok(None);
        }
        let frame = src.split_to(total).freeze();
        postcard::from_bytes(&frame[4..])
            .map(Some)
            .map_err(|e| NetError::Framing(e.to_string()))
    }
}

/// RVP connection: a thin wrapper around quinn::Connection providing stream opening and the handshake.
#[derive(Debug, Clone)]
pub struct RvpConnection {
    conn: Connection,
}

impl RvpConnection {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn inner(&self) -> &Connection {
        &self.conn
    }

    /// Peer certificate fingerprint of this connection (SHA-256 of end-entity DER).
    ///
    /// Taken directly from the QUIC handshake result, scoped per connection and
    /// unaffected by concurrent connections.
    pub fn peer_fingerprint(&self) -> Option<[u8; 32]> {
        use sha2::Digest;
        let chain = self
            .conn
            .peer_identity()?
            .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
            .ok()?;
        let end_entity = chain.first()?;
        Some(sha2::Sha256::digest(end_entity.as_ref()).into())
    }

    pub fn rtt_estimate(&self) -> f32 {
        self.conn.rtt().as_secs_f32() * 1000.0
    }

    /// Client side: open the control bidi stream (stream id 0) and complete the handshake.
    ///
    /// The handshake phase uses raw reads/writes throughout (to avoid mixing Framed
    /// buffering with read_exact and losing bytes); it is wrapped in Framed only afterwards.
    pub async fn connect_handshake(
        &self,
        hello: HandshakeClient,
    ) -> Result<(HandshakeServer, ControlSink, ControlSource)> {
        let (mut send, mut recv) = self.conn.open_bi().await?;

        let body = postcard::to_allocvec(&hello).map_err(|e| NetError::Framing(e.to_string()))?;
        let mut wire = Vec::with_capacity(8 + body.len());
        wire.extend_from_slice(&MAGIC);
        wire.extend_from_slice(&(body.len() as u32).to_be_bytes());
        wire.extend_from_slice(&body);
        send.write_all(&wire).await.map_err(write_err)?;
        flush_if_needed(&mut send).await?;

        let ack = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            read_postcard_prefixed::<HandshakeServer>(&mut recv),
        )
        .await
        .map_err(|_| NetError::Timeout)??;
        negotiate_proto_version(PROTO_VERSION, ack.proto_version).map_err(|_| {
            NetError::VersionMismatch {
                local: PROTO_VERSION,
                remote: ack.proto_version,
            }
        })?;
        Ok((
            ack,
            Framed::new(send, ControlCodec),
            Framed::new(recv, ControlCodec),
        ))
    }

    /// Server side: accept the control stream, read the Hello, send the reply.
    ///
    /// The reply is built by `ack` from the received Hello, so the host can answer
    /// fields like `resume_accepted`/`peer_known` honestly instead of guessing
    /// before the Hello is read.
    pub async fn accept_handshake(
        &self,
        ack: impl FnOnce(&HandshakeClient) -> HandshakeServer,
    ) -> Result<(HandshakeClient, ControlSink, ControlSource)> {
        // Bounded by SERVER_ACCEPT_TIMEOUT (see above): a stalled peer must not
        // occupy the connection slot indefinitely.
        let (mut send, recv, hello) =
            tokio::time::timeout(SERVER_ACCEPT_TIMEOUT, self.accept_control_and_hello())
                .await
                .map_err(|_| NetError::Timeout)??;
        if negotiate_proto_version(PROTO_VERSION, hello.proto_version).is_err() {
            // best-effort: try to tell the peer about the version incompatibility before disconnecting (if it can't be sent, just disconnect).
            if let Ok(wire) = encode_control(&ControlMsg::Error {
                code: ErrorCode::VersionMismatch,
            }) {
                let _ = send.write_all(&wire).await;
            }
            return Err(NetError::VersionMismatch {
                local: PROTO_VERSION,
                remote: hello.proto_version,
            });
        }

        let mut ack = ack(&hello);
        ack.proto_version = PROTO_VERSION;
        let body = postcard::to_allocvec(&ack).map_err(|e| NetError::Framing(e.to_string()))?;
        let mut wire = Vec::with_capacity(4 + body.len());
        wire.extend_from_slice(&(body.len() as u32).to_be_bytes());
        wire.extend_from_slice(&body);
        send.write_all(&wire).await.map_err(write_err)?;
        flush_if_needed(&mut send).await?;

        Ok((
            hello,
            Framed::new(send, ControlCodec),
            Framed::new(recv, ControlCodec),
        ))
    }

    /// Accept the control bidi stream and read the Hello off it (the receive phase
    /// of [`accept_handshake`], separated so the caller can bound it with a timeout).
    async fn accept_control_and_hello(
        &self,
    ) -> Result<(quinn::SendStream, quinn::RecvStream, HandshakeClient)> {
        let (send, mut recv) = self.conn.accept_bi().await?;

        let mut head = [0u8; 8];
        recv.read_exact(&mut head).await.map_err(read_err)?;
        if head[..4] != MAGIC {
            return Err(NetError::Handshake(format!(
                "bad magic, expected {PROTOCOL_NAME}"
            )));
        }
        let body_len = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
        if body_len > (1 << 20) {
            return Err(NetError::Handshake("hello too large".into()));
        }
        let mut buf = vec![0u8; body_len];
        recv.read_exact(&mut buf).await.map_err(read_err)?;
        let hello: HandshakeClient = match postcard::from_bytes(&buf) {
            Ok(h) => h,
            Err(e) => {
                return Err(NetError::Handshake(e.to_string()));
            }
        };
        Ok((send, recv, hello))
    }

    /// Pairing-specific bidi stream.
    pub async fn open_pairing(&self) -> Result<(PairSink, PairSource)> {
        let (send, recv) = self.conn.open_bi().await?;
        Ok((Framed::new(send, PairCodec), Framed::new(recv, PairCodec)))
    }

    pub async fn accept_pairing(&self) -> Result<(PairSink, PairSource)> {
        let (send, recv) = self.conn.accept_bi().await?;
        Ok((Framed::new(send, PairCodec), Framed::new(recv, PairCodec)))
    }

    /// Open a media uni-stream (sending).
    pub async fn open_media_stream(&self) -> Result<quinn::SendStream> {
        Ok(self.conn.open_uni().await?)
    }

    /// Accept a media uni-stream (receiving).
    pub async fn accept_media_stream(&self) -> Result<quinn::RecvStream> {
        Ok(self.conn.accept_uni().await?)
    }
}

fn write_err(e: quinn::WriteError) -> NetError {
    match e {
        quinn::WriteError::ConnectionLost(ce) => NetError::Quinn(ce),
        quinn::WriteError::Stopped(_) => NetError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "stream stopped",
        )),
        quinn::WriteError::ClosedStream | quinn::WriteError::ZeroRttRejected => NetError::Io(
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream unavailable"),
        ),
    }
}

fn read_err(e: quinn::ReadExactError) -> NetError {
    match e {
        quinn::ReadExactError::FinishedEarly(_) => NetError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "stream finished",
        )),
        quinn::ReadExactError::ReadError(re) => match re {
            quinn::ReadError::ConnectionLost(ce) => NetError::Quinn(ce),
            quinn::ReadError::Reset(code) => NetError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                format!("stream reset ({code})"),
            )),
            other => NetError::Io(std::io::Error::other(other.to_string())),
        },
    }
}

async fn flush_if_needed(_send: &mut quinn::SendStream) -> Result<()> {
    // quinn SendStream writes go straight into the send queue; there is no explicit flush semantics.
    Ok(())
}

async fn read_postcard_prefixed<T: serde::de::DeserializeOwned>(
    recv: &mut quinn::RecvStream,
) -> Result<T> {
    let mut head = [0u8; 4];
    recv.read_exact(&mut head).await.map_err(read_err)?;
    let body_len = u32::from_be_bytes(head) as usize;
    if body_len > (1 << 20) {
        return Err(NetError::Handshake("handshake frame too large".into()));
    }
    let mut buf = vec![0u8; body_len];
    recv.read_exact(&mut buf).await.map_err(read_err)?;
    postcard::from_bytes(&buf).map_err(|e| NetError::Handshake(e.to_string()))
}
