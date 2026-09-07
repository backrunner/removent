//! Exercise the production client against a TLS/NLA RDP server over loopback TCP.

use anyhow::Result;
use ironrdp_server::{
    BitmapUpdate, Credentials, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat,
    RdpServer, RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler, TlsIdentityCtx,
};
use removent_client::connection::{ConnectionAddress, ConnectionProtocol, ConnectionRequest};
use removent_client::rdp::connect_rdp;
use removent_proto::{ControlMsg, KeyKind, KeyModifiers, MouseKind};
use std::net::SocketAddr;
use std::num::{NonZeroU16, NonZeroUsize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Debug)]
enum Input {
    Key(KeyboardEvent),
    Mouse(MouseEvent),
}

struct Handler(mpsc::UnboundedSender<Input>);
impl RdpServerInputHandler for Handler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        let _ = self.0.send(Input::Key(event));
    }
    fn mouse(&mut self, event: MouseEvent) {
        let _ = self.0.send(Input::Mouse(event));
    }
}

#[derive(Clone)]
struct Display {
    updates: Arc<tokio::sync::Mutex<mpsc::Receiver<DisplayUpdate>>>,
    size: Arc<Mutex<DesktopSize>>,
}

#[async_trait::async_trait]
impl RdpServerDisplay for Display {
    async fn size(&mut self) -> DesktopSize {
        *self.size.lock().unwrap()
    }
    async fn updates(&mut self) -> Result<Box<dyn RdpServerDisplayUpdates>> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Display {
    async fn next_update(&mut self) -> Result<Option<DisplayUpdate>> {
        let update = self.updates.lock().await.recv().await;
        if let Some(DisplayUpdate::Resize(size)) = &update {
            *self.size.lock().unwrap() = *size;
        }
        Ok(update)
    }
}

struct TestServer {
    addr: SocketAddr,
    display: mpsc::Sender<DisplayUpdate>,
    inputs: mpsc::UnboundedReceiver<Input>,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    async fn start(nla: bool) -> Self {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, cert.cert.pem()).unwrap();
        std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();
        let identity = TlsIdentityCtx::init_from_paths(&cert_path, &key_path).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let builder = RdpServer::builder().with_addr(addr);
        let builder = if nla {
            builder.with_hybrid(identity.make_acceptor().unwrap(), identity.pub_key)
        } else {
            builder.with_tls(identity.make_acceptor().unwrap())
        };
        let (input_tx, inputs) = mpsc::unbounded_channel();
        let (display, updates) = mpsc::channel(4);
        let builder = builder
            .with_input_handler(Handler(input_tx))
            .with_display_handler(Display {
                updates: Arc::new(tokio::sync::Mutex::new(updates)),
                size: Arc::new(Mutex::new(DesktopSize {
                    width: 32,
                    height: 16,
                })),
            });
        // Exercise both lossless bitmap updates and the default RemoteFX codec.
        let builder = if nla {
            builder
        } else {
            builder.with_bitmap_codecs(ironrdp::pdu::rdp::capability_sets::BitmapCodecs(Vec::new()))
        };
        let mut server = builder.build();
        server.set_credentials(Some(Credentials {
            username: "testuser".into(),
            password: "test-password".into(),
            domain: None,
        }));
        let task = tokio::task::spawn_local(async move {
            let (stream, _) = listener.accept().await?;
            server.run_connection(stream).await
        });
        Self {
            addr,
            display,
            inputs,
            task,
        }
    }

    fn request(&self, allow_untrusted: bool, password: &str) -> ConnectionRequest {
        ConnectionRequest {
            protocol: ConnectionProtocol::Rdp,
            address: ConnectionAddress::from_socket(self.addr),
            username: "testuser".into(),
            password: password.into(),
            domain: String::new(),
            accept_invalid_certificate: allow_untrusted,
        }
    }
}

async fn verify_session(nla: bool) {
    let mut server = TestServer::start(nla).await;
    let mut session = timeout(
        Duration::from_secs(15),
        connect_rdp(server.request(true, "test-password")),
    )
    .await
    .unwrap()
    .unwrap();
    let pixels: Vec<u8> = (0..16)
        .flat_map(|y| (0..32).flat_map(move |x| [x as u8, y as u8, 200, 255]))
        .collect();
    server
        .display
        .send(DisplayUpdate::Bitmap(BitmapUpdate {
            x: 0,
            y: 0,
            width: NonZeroU16::new(32).unwrap(),
            height: NonZeroU16::new(16).unwrap(),
            format: PixelFormat::BgrA32,
            data: pixels.clone().into(),
            stride: NonZeroUsize::new(128).unwrap(),
        }))
        .await
        .unwrap();
    let frame = timeout(Duration::from_secs(5), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((frame.width, frame.height), (32, 16));
    if nla {
        // RemoteFX is lossy; verify channel order and image content with a bounded error.
        let total_error: u64 = frame
            .data
            .iter()
            .zip(&pixels)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        assert!(
            total_error < pixels.len() as u64 * 4,
            "RemoteFX pixel error: {total_error}"
        );
        assert!(frame.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    } else {
        assert_eq!(frame.data, pixels);
    }

    for kind in [KeyKind::Down, KeyKind::Up] {
        session
            .cmd_tx
            .send(ControlMsg::KeyEvent {
                vk_code: 0,
                modifiers: KeyModifiers::empty(),
                kind,
                unicode: Some('a'),
            })
            .await
            .unwrap();
    }
    for buttons in [2, 0] {
        session
            .cmd_tx
            .send(ControlMsg::MouseEvent {
                display_id: 0,
                x_px: 10.,
                y_px: 5.,
                buttons,
                kind: MouseKind::Moved,
            })
            .await
            .unwrap();
    }
    session
        .cmd_tx
        .send(ControlMsg::ScrollEvent {
            display_id: 0,
            dx_mm: 0.,
            dy_mm: -3.,
            phase: removent_proto::ScrollPhase::Changed,
        })
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        let mut seen = [false; 6];
        while !seen.into_iter().all(|v| v) {
            match server.inputs.recv().await.unwrap() {
                Input::Key(KeyboardEvent::Pressed {
                    code: 0x1e,
                    extended: false,
                }) => seen[0] = true,
                Input::Key(KeyboardEvent::Released {
                    code: 0x1e,
                    extended: false,
                }) => seen[1] = true,
                Input::Mouse(MouseEvent::Move { x: 10, y: 5 }) => seen[2] = true,
                Input::Mouse(MouseEvent::RightPressed) => seen[3] = true,
                Input::Mouse(MouseEvent::RightReleased) => seen[4] = true,
                Input::Mouse(MouseEvent::VerticalScroll { value: 120 }) => seen[5] = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    // Windows may reactivate the desktop after logon or a display change.
    server
        .display
        .send(DisplayUpdate::Resize(DesktopSize {
            width: 64,
            height: 32,
        }))
        .await
        .unwrap();
    server
        .display
        .send(DisplayUpdate::Bitmap(BitmapUpdate {
            x: 0,
            y: 0,
            width: NonZeroU16::new(64).unwrap(),
            height: NonZeroU16::new(32).unwrap(),
            format: PixelFormat::BgrA32,
            data: [20, 100, 200, 255].repeat(64 * 32).into(),
            stride: NonZeroUsize::new(256).unwrap(),
        }))
        .await
        .unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            let frame = session
                .decoded_bgra_rx
                .recv()
                .await
                .expect("RDP reactivation closed the stream");
            if (frame.width, frame.height) == (64, 32) && frame.data[2] > 180 {
                break;
            }
        }
    })
    .await
    .unwrap();
    drop(session);
    // Dropping a viewer must close the socket, including the session's background task.
    let _ = timeout(Duration::from_secs(5), &mut server.task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn tls_frames_input_and_disconnect() {
    tokio::task::LocalSet::new()
        .run_until(verify_session(false))
        .await;
}

#[tokio::test]
async fn nla_frames_input_and_disconnect() {
    tokio::task::LocalSet::new()
        .run_until(verify_session(true))
        .await;
}

#[tokio::test]
async fn untrusted_certificate_is_rejected_by_default() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = TestServer::start(true).await;
            let result = connect_rdp(server.request(false, "test-password")).await;
            let error = result
                .err()
                .expect("self-signed certificate must not be accepted");
            assert!(format!("{error:#}").contains("certificate"));
        })
        .await;
}

#[tokio::test]
async fn wrong_nla_password_is_rejected() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = TestServer::start(true).await;
            assert!(
                connect_rdp(server.request(true, "wrong-password"))
                    .await
                    .is_err()
            );
        })
        .await;
}

#[tokio::test]
async fn server_loss_closes_frames_and_reports_failure() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let server = TestServer::start(false).await;
            let mut session = connect_rdp(server.request(true, "test-password"))
                .await
                .unwrap();
            drop(server);
            assert!(
                timeout(Duration::from_secs(5), session.decoded_bgra_rx.recv())
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                timeout(Duration::from_secs(5), &mut session.completion)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
        })
        .await;
}
