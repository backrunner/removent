//! Opus audio codec wrapper (48kHz stereo, 10ms frames).

use opus::{Channels, Decoder, Encoder};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("opus: {0}")]
    Opus(#[from] opus::Error),
    #[error("pcm frame must be {expected} samples, got {got}")]
    InvalidFrameLength { expected: usize, got: usize },
}

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Samples per frame (10ms).
pub const FRAME_SAMPLES_PER_CHANNEL: usize = SAMPLE_RATE as usize / 100;
/// Maximum buffer needed to encode one frame.
pub const MAX_PACKET_BYTES: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Application {
    Voip,
    Audio,
    LowDelay,
}

impl Application {
    fn to_opus(self) -> opus::Application {
        match self {
            Application::Voip => opus::Application::Voip,
            Application::Audio => opus::Application::Audio,
            Application::LowDelay => opus::Application::LowDelay,
        }
    }
}

pub struct AudioEncoder {
    encoder: Encoder,
    /// Frame counter, used for the protocol seq field.
    next_seq: u16,
}

impl AudioEncoder {
    pub fn new(bitrate_kbps: u32, application: Application) -> Result<Self, AudioError> {
        let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Stereo, application.to_opus())?;
        encoder.set_bitrate(opus::Bitrate::Bits((bitrate_kbps * 1000) as i32))?;
        // No in-band FEC / packet-loss hint / DTX: transport is reliable
        // ordered QUIC, so loss and reorder cannot occur; they would be pure
        // overhead (protocol.md §6.2 / FR-21).
        Ok(Self {
            encoder,
            next_seq: 0,
        })
    }

    pub fn set_bitrate_kbps(&mut self, kbps: u32) -> Result<(), AudioError> {
        self.encoder
            .set_bitrate(opus::Bitrate::Bits((kbps * 1000) as i32))?;
        Ok(())
    }

    /// Encodes exactly one frame (10ms) of interleaved i16 PCM; returns (seq, opus packet).
    pub fn encode_frame(&mut self, pcm: &[i16]) -> Result<(u16, Vec<u8>), AudioError> {
        let expected = FRAME_SAMPLES_PER_CHANNEL * CHANNELS;
        if pcm.len() != expected {
            return Err(AudioError::InvalidFrameLength {
                expected,
                got: pcm.len(),
            });
        }
        let mut out = [0u8; MAX_PACKET_BYTES];
        let n = self.encoder.encode(pcm, &mut out)?;
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        Ok((seq, out[..n].to_vec()))
    }
}

pub struct AudioDecoder {
    decoder: Decoder,
}

impl AudioDecoder {
    pub fn new() -> Result<Self, AudioError> {
        let decoder = Decoder::new(SAMPLE_RATE, Channels::Stereo)?;
        Ok(Self { decoder })
    }

    /// Decodes one packet to interleaved i16 PCM; truncated to the actual
    /// number of samples opus decodes (DTX/short-packet scenarios).
    pub fn decode_frame(&mut self, packet: &[u8]) -> Result<Vec<i16>, AudioError> {
        let mut out = vec![0i16; FRAME_SAMPLES_PER_CHANNEL * CHANNELS];
        let n = self.decoder.decode(packet, &mut out, false)?;
        out.truncate(n * CHANNELS);
        Ok(out)
    }

    /// Packet-loss concealment: triggers PLC with empty input.
    pub fn conceal(&mut self) -> Result<Vec<i16>, AudioError> {
        let mut out = vec![0i16; FRAME_SAMPLES_PER_CHANNEL * CHANNELS];
        let n = self.decoder.decode(&[], &mut out, false)?;
        out.truncate(n * CHANNELS);
        Ok(out)
    }
}
