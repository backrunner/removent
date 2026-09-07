//! RDP over TLS/CredSSP, decoded into the same frame and input channels as VNC.

use anyhow::{Context, Result, bail, ensure};
use ironrdp::connector::connection_activation::ConnectionActivationState;
use ironrdp::connector::{self, ConnectionResult, Credentials, Sequence};
use ironrdp::core::WriteBuf;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{CompressionType, PerformanceFlags, TimezoneInfo};
use ironrdp::session::{
    ActiveStageBuilder, ActiveStageOutput, GracefulDisconnectReason, image::DecodedImage,
};
use ironrdp_tokio::{FramedWrite, TokioFramed};
use removent_proto::ControlMsg;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_native_tls::{TlsConnector, TlsStream, native_tls};
use x509_cert::der::Decode;

use crate::DecodedFrame;
use crate::connection::ConnectionRequest;

mod input;

pub struct RdpSession {
    pub cmd_tx: mpsc::Sender<ControlMsg>,
    pub decoded_bgra_rx: removent_core::latest::Receiver<DecodedFrame>,
    pub completion: oneshot::Receiver<Result<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RdpSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type RdpStream = TokioFramed<TlsStream<TcpStream>>;

pub async fn connect_rdp(request: ConnectionRequest) -> Result<RdpSession> {
    let (result, stream) = tokio::time::timeout(Duration::from_secs(30), handshake(request))
        .await
        .context("RDP connection timed out after 30 seconds")??;
    let image = new_image(result.desktop_size)?;
    let (cmd_tx, cmd_rx) = mpsc::channel(128);
    let (frame_tx, decoded_bgra_rx) = removent_core::latest::channel();
    let (done_tx, completion) = oneshot::channel();
    let task = tokio::spawn(async move {
        let result = run(result, stream, image, cmd_rx, frame_tx).await;
        let _ = done_tx.send(result);
    });
    Ok(RdpSession {
        cmd_tx,
        decoded_bgra_rx,
        completion,
        task,
    })
}

async fn handshake(request: ConnectionRequest) -> Result<(ConnectionResult, RdpStream)> {
    let host = request.address.host.clone();
    let tcp = TcpStream::connect((host.as_str(), request.address.port))
        .await
        .context("RDP TCP connection")?;
    tcp.set_nodelay(true)?;
    let local_addr = tcp.local_addr()?;
    let tls = native_tls::TlsConnector::builder()
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .danger_accept_invalid_certs(request.accept_invalid_certificate)
        .danger_accept_invalid_hostnames(request.accept_invalid_certificate)
        .build()
        .context("RDP TLS configuration")?;
    let mut connector = connector::ClientConnector::new(connector_config(request), local_addr);
    let mut framed = TokioFramed::new(tcp);
    let upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .context("RDP negotiation")?;
    let (tcp, leftover) = framed.into_inner();
    ensure!(
        leftover.is_empty(),
        "Unexpected data before RDP TLS handshake"
    );
    let tls_stream = TlsConnector::from(tls)
        .connect(&host, tcp)
        .await
        .context("RDP TLS handshake or server certificate validation failed")?;
    let cert = tls_stream
        .get_ref()
        .peer_certificate()?
        .context("RDP server certificate missing")?;
    let cert = x509_cert::Certificate::from_der(&cert.to_der()?)?;
    let public_key = cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .context("RDP server public key is not byte-aligned")?
        .to_vec();
    let upgraded = ironrdp_tokio::mark_as_upgraded(upgrade, &mut connector);
    let mut framed = TokioFramed::new(tls_stream);
    let result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut framed,
        &mut ironrdp_tokio::reqwest::ReqwestNetworkClient::new(),
        host.into(),
        public_key,
        None,
    )
    .await
    .context("RDP authentication or session negotiation failed")?;
    Ok((result, framed))
}

fn connector_config(request: ConnectionRequest) -> connector::Config {
    connector::Config {
        credentials: Credentials::UsernamePassword {
            username: request.username,
            password: request.password,
        },
        domain: (!request.domain.is_empty()).then_some(request.domain),
        enable_tls: true,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0x0409,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: connector::DesktopSize {
            width: 1600,
            height: 900,
        },
        bitmap: None,
        client_build: 0,
        client_name: "Removent".into(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
        platform: MajorPlatformType::MACINTOSH,
        enable_server_pointer: true,
        request_data: None,
        autologon: true,
        enable_audio_playback: false,
        compression_type: Some(CompressionType::Rdp61),
        pointer_software_rendering: true,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 100,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

fn new_image(size: connector::DesktopSize) -> Result<DecodedImage> {
    ensure!(
        size.width > 0
            && size.height > 0
            && usize::from(size.width) * usize::from(size.height) <= 16 * 1024 * 1024,
        "Unsupported RDP desktop size: {}x{}",
        size.width,
        size.height
    );
    Ok(DecodedImage::new(
        PixelFormat::BgrA32,
        size.width,
        size.height,
    ))
}

fn snapshot(image: &DecodedImage, started: Instant) -> DecodedFrame {
    let mut data = image.data().to_vec();
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel[3] = 255;
    }
    DecodedFrame {
        data,
        width: u32::from(image.width()),
        height: u32::from(image.height()),
        pts_us: started.elapsed().as_micros() as i64,
    }
}

async fn run(
    result: ConnectionResult,
    mut stream: RdpStream,
    mut image: DecodedImage,
    mut commands: mpsc::Receiver<ControlMsg>,
    frames: removent_core::latest::Sender<DecodedFrame>,
) -> Result<()> {
    let activation_factory = result.activation_factory;
    let compression_type = result.compression_type;
    let mut stage = ActiveStageBuilder {
        static_channels: result.static_channels,
        user_channel_id: result.user_channel_id,
        io_channel_id: result.io_channel_id,
        message_channel_id: result.message_channel_id,
        share_id: result.share_id,
        compression_type: result.compression_type,
        enable_server_pointer: result.enable_server_pointer,
        pointer_software_rendering: result.pointer_software_rendering,
    }
    .build();
    let mut input = input::InputState::default();
    let started = Instant::now();
    loop {
        // Framed keeps partial reads in its own buffer, so this select is cancel-safe.
        let outputs = tokio::select! {
            pdu = stream.read_pdu() => {
                let (action, payload) = pdu.context("RDP receive")?;
                stage.process(&mut image, action, &payload).context("RDP decode")?
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                let events = input.translate(command, image.width(), image.height());
                stage.process_fastpath_input(&mut image, &events).context("RDP input")?
            }
        };
        let mut changed = false;
        for output in outputs {
            match output {
                ActiveStageOutput::ResponseFrame(bytes) => {
                    if !bytes.is_empty() {
                        stream.write_all(&bytes).await.context("RDP send")?;
                    }
                }
                ActiveStageOutput::GraphicsUpdate(_) => changed = true,
                ActiveStageOutput::Terminate(reason) => {
                    tracing::info!(reason = %reason.description(), "RDP session ended");
                    if let GracefulDisconnectReason::Other(description) = reason {
                        bail!("RDP server ended the session: {description}");
                    }
                    return Ok(());
                }
                ActiveStageOutput::DeactivateAll => {
                    let mut sequence = activation_factory.create();
                    let mut buf = WriteBuf::new();
                    tokio::time::timeout(Duration::from_secs(30), async {
                        while !sequence.state().is_terminal() {
                            ironrdp_tokio::single_sequence_step(
                                &mut stream,
                                &mut sequence,
                                &mut buf,
                            )
                            .await?;
                        }
                        Ok::<_, anyhow::Error>(())
                    })
                    .await
                    .context("RDP reactivation timed out")??;
                    let ConnectionActivationState::Finalized {
                        desktop_size,
                        share_id,
                        enable_server_pointer,
                        pointer_software_rendering,
                    } = sequence.connection_activation_state()
                    else {
                        bail!("RDP reactivation did not finish");
                    };
                    stage.set_share_id(share_id);
                    // A new activation also needs a new graphics context: frame ACKs
                    // carry the share ID, and pointer/RemoteFX state belongs to the
                    // previous desktop. Updating only x224 leaves stale fast-path state.
                    let bulk_decompressor = compression_type
                        .map(|compression| {
                            use ironrdp_bulk::{BulkCompressor, CompressionType as BulkType};
                            BulkCompressor::new(match compression {
                                CompressionType::K8 => BulkType::Rdp4,
                                CompressionType::K64 => BulkType::Rdp5,
                                CompressionType::Rdp6 => BulkType::Rdp6,
                                CompressionType::Rdp61 => BulkType::Rdp61,
                            })
                        })
                        .transpose()
                        .map_err(|e| anyhow::anyhow!("RDP decompressor: {e}"))?;
                    stage.set_fastpath_processor(
                        ironrdp::session::fast_path::ProcessorBuilder {
                            io_channel_id: activation_factory.io_channel_id(),
                            user_channel_id: activation_factory.user_channel_id(),
                            share_id,
                            enable_server_pointer,
                            pointer_software_rendering,
                            bulk_decompressor,
                        }
                        .build(),
                    );
                    stage.set_enable_server_pointer(enable_server_pointer);
                    image = new_image(desktop_size)?;
                    changed = true;
                }
                _ => {}
            }
        }
        if changed && frames.send(snapshot(&image, started)).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_desktops_and_publishes_opaque_bgra() {
        assert!(
            new_image(connector::DesktopSize {
                width: 65535,
                height: 65535
            })
            .is_err()
        );
        assert!(
            new_image(connector::DesktopSize {
                width: 0,
                height: 900
            })
            .is_err()
        );
        let image = new_image(connector::DesktopSize {
            width: 2,
            height: 1,
        })
        .unwrap();
        let frame = snapshot(&image, Instant::now());
        assert_eq!(frame.data, vec![0, 0, 0, 255, 0, 0, 0, 255]);
    }
}
