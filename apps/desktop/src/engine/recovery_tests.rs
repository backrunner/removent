use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Real RFB sockets through the production desktop bridge: the original viewer
/// receiver survives EOF and receives the second server's pixels.
#[test]
fn vnc_transport_drop_reconnects_in_the_same_viewer_and_cancel_stops_retries() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (drop_tx, mut drop_rx) = tokio::sync::mpsc::channel::<()>(1);
            let server = tokio::spawn(async move {
                for color in [40, 80] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    socket.write_all(b"RFB 003.008\n").await.unwrap();
                    let mut greeting = [0; 12];
                    socket.read_exact(&mut greeting).await.unwrap();
                    socket.write_all(&[1, 1]).await.unwrap();
                    socket.read_u8().await.unwrap();
                    socket.write_u32(0).await.unwrap();
                    socket.read_u8().await.unwrap();
                    socket
                        .write_all(&[
                            0, 2, 0, 2, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0, 0,
                            0, 0, 0,
                        ])
                        .await
                        .unwrap();
                    loop {
                        match socket.read_u8().await.unwrap() {
                            0 => {
                                let mut format = [0; 19];
                                socket.read_exact(&mut format).await.unwrap();
                            }
                            2 => {
                                socket.read_u8().await.unwrap();
                                let count = socket.read_u16().await.unwrap();
                                let mut encodings = vec![0; count as usize * 4];
                                socket.read_exact(&mut encodings).await.unwrap();
                            }
                            3 => {
                                let mut request = [0; 9];
                                socket.read_exact(&mut request).await.unwrap();
                                socket
                                    .write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0, 0, 0, 0])
                                    .await
                                    .unwrap();
                                socket.write_all(&[color; 16]).await.unwrap();
                                drop_rx.recv().await.unwrap();
                                // FIN (not a TCP reset) is still an interrupted RFB
                                // session: VNC has no intentional peer-end message.
                                socket.shutdown().await.unwrap();
                                break;
                            }
                            other => panic!("unexpected RFB message {other}"),
                        }
                    }
                }
                assert!(
                    tokio::time::timeout(Duration::from_millis(500), listener.accept())
                        .await
                        .is_err(),
                    "cancel must prevent a third connection"
                );
            });
            let channels = Arc::new(Mutex::new(ClientChannels::default()));
            let (events, _events_rx) = std::sync::mpsc::channel();
            let recovery = channels.lock().unwrap().recovery.clone();
            let attempt = ClientAttempt {
                generation: 0,
                started: std::time::Instant::now(),
                recovery: recovery.clone(),
                channels: channels.clone(),
                events,
            };
            let mut request = ConnectionRequest::native(address);
            request.protocol = ConnectionProtocol::Vnc;
            let task = tokio::spawn(async move {
                let result = recovery
                    .run(|| compatibility::run_vnc_client(request.clone(), attempt.clone()))
                    .await;
                attempt.finish(result.err().map(|e| e.to_string()));
            });
            let mut frames = loop {
                if let Some(frames) = channels.lock().unwrap().frames.take() {
                    break frames;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            assert_eq!(frames.recv().await.unwrap().data[0], 40);
            drop_tx.send(()).await.unwrap();
            assert_eq!(frames.recv().await.unwrap().data[0], 80);
            assert!(channels.lock().unwrap().frames.is_none());
            task.abort();
            let _ = task.await;
            drop_tx.send(()).await.unwrap();
            server.await.unwrap();
        })
        .await
        .unwrap();
    });
}
