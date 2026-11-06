use super::*;

/// Audio send loop: Opus encode → audio packet stream. Exits when `cancel` fires.
pub fn spawn_audio_loop(
    mut stream: removent_net::quinn::SendStream,
    mut audio_rx: mpsc::Receiver<removent_media_capture::AudioFrame>,
    bitrate_kbps: u32,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Input/control is higher priority; audio must not sit behind a video IDR.
        let _ = stream.set_priority(0);
        // Audio is optional. Losing its capture source or receiver must not
        // cancel the shared session token and tear down healthy video/control.
        // Session cancellation still stops this task, including blocked writes.
        let work = async {
            let mut encoder = match removent_media_codec::AudioEncoder::new(
                bitrate_kbps,
                removent_media_codec::Application::Audio,
            ) {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!(err=%e, "audio encoder init failed");
                    return;
                }
            };
            loop {
                let frame = tokio::select! {
                    _ = cancel.cancelled() => break,
                    item = audio_rx.recv() => {
                        let Some(frame) = item else { break };
                        // A capture callback can contain several consecutive 10 ms
                        // blocks. Preserve all of them: coalescing audio like video
                        // removes samples without a corresponding sequence gap.
                        frame
                    }
                };
                let (seq, packet) = match encoder.encode_frame(&frame.samples) {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!(err=%e, "opus encode failed");
                        continue;
                    }
                };
                let hdr = AudioPacketHeader {
                    seq,
                    // Source presentation timestamp (mach timebase, same epoch as
                    // the video pts) so the client can lip-sync A/V.
                    pts_us: frame.pts_micros,
                    flags: 0,
                    payload_len: packet.len() as u16,
                };
                let wire = build_audio_packet(&hdr, &packet);
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(10), stream.write_all(&wire)).await,
                    Ok(Ok(()))
                ) {
                    return;
                }
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = work => {},
        }
    })
}

#[cfg(test)]
mod tests;
