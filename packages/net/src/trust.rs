//! Explicit first-use certificate confirmation, before application credentials.
use crate::{NetError, RvpConnection};
use std::{net::SocketAddr, time::Duration};
use tokio::sync::oneshot;

pub type CertificateConfirmation = Box<dyn FnOnce([u8; 32], oneshot::Sender<bool>) + Send>;

/// Close the probe before waiting for a human, then reconnect to the exact approved
/// certificate. This does not hold the server's application-handshake slot open.
pub async fn confirm_quic_peer(
    endpoint: &quinn::Endpoint,
    address: SocketAddr,
    server_name: &str,
    confirmation: CertificateConfirmation,
) -> Result<quinn::Connection, NetError> {
    async fn dial(
        endpoint: &quinn::Endpoint,
        address: SocketAddr,
        name: &str,
    ) -> Result<quinn::Connection, NetError> {
        let connecting = endpoint
            .connect(address, name)
            .map_err(|e| NetError::Tls(e.to_string()))?;
        tokio::time::timeout(Duration::from_secs(12), connecting)
            .await
            .map_err(|_| NetError::Tls("Certificate connection timed out".into()))?
            .map_err(|e| NetError::Tls(e.to_string()))
    }
    let probe = dial(endpoint, address, server_name).await?;
    let fingerprint = RvpConnection::new(probe.clone())
        .peer_fingerprint()
        .ok_or_else(|| NetError::Tls("Peer certificate missing".into()))?;
    probe.close(0u32.into(), b"certificate probe complete");
    let (tx, rx) = oneshot::channel();
    confirmation(fingerprint, tx);
    let accepted = tokio::time::timeout(Duration::from_secs(120), rx)
        .await
        .map_err(|_| NetError::Tls("Certificate confirmation expired".into()))?
        .map_err(|_| NetError::Tls("Certificate confirmation cancelled".into()))?;
    if !accepted {
        return Err(NetError::Tls("Certificate was not trusted".into()));
    }
    let connection = dial(endpoint, address, server_name).await?;
    if RvpConnection::new(connection.clone()).peer_fingerprint() != Some(fingerprint) {
        connection.close(0u32.into(), b"certificate changed");
        return Err(NetError::Tls(
            "Certificate changed after confirmation".into(),
        ));
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PinState, make_client_endpoint, make_server_endpoint};
    use removent_core::{DataPaths, identity};

    #[tokio::test]
    async fn confirmation_closes_probe_and_denial_never_reconnects() {
        for accepted in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let id = identity::load_or_create(
                &DataPaths {
                    root: dir.path().into(),
                },
                "peer",
            )
            .unwrap();
            let (server, _) =
                make_server_endpoint("127.0.0.1:0".parse().unwrap(), &id, PinState::new([], true))
                    .unwrap();
            let (client, _) =
                make_client_endpoint("127.0.0.1:0".parse().unwrap(), &id, PinState::new([], true))
                    .unwrap();
            let addr = server.local_addr().unwrap();
            let (closed_tx, closed_rx) = oneshot::channel();
            let server_task = tokio::spawn(async move {
                let probe = server.accept().await.unwrap().await.unwrap();
                tokio::select! {
                    _ = probe.closed() => {},
                    stream = probe.accept_bi() => assert!(stream.is_err(), "Application stream opened before confirmation"),
                }
                let _ = closed_tx.send(());
                if accepted {
                    let connection = server.accept().await.unwrap().await.unwrap();
                    connection.closed().await;
                } else {
                    assert!(
                        tokio::time::timeout(Duration::from_millis(100), server.accept())
                            .await
                            .is_err()
                    );
                }
            });
            let (asked_tx, asked_rx) = oneshot::channel();
            let task = tokio::spawn(async move {
                let result = confirm_quic_peer(
                    &client,
                    addr,
                    "removent",
                    Box::new(move |fp, answer| {
                        let _ = asked_tx.send((fp, answer));
                    }),
                )
                .await;
                if let Ok(connection) = &result {
                    connection.close(0u32.into(), b"test done");
                }
                result.is_ok()
            });
            let (fp, answer) = asked_rx.await.unwrap();
            assert_eq!(hex::encode(fp), id.fingerprint_hex());
            tokio::time::timeout(Duration::from_secs(2), closed_rx)
                .await
                .unwrap()
                .unwrap();
            answer.send(accepted).unwrap();
            assert_eq!(task.await.unwrap(), accepted);
            server_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn certificate_change_between_confirmation_and_reconnect_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let first = identity::load_or_create(
            &DataPaths {
                root: dir.path().join("first"),
            },
            "first",
        )
        .unwrap();
        let changed = identity::load_or_create(
            &DataPaths {
                root: dir.path().join("changed"),
            },
            "changed",
        )
        .unwrap();
        let (server, _) = make_server_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &first,
            PinState::new([], true),
        )
        .unwrap();
        let (client, _) = make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &first,
            PinState::new([], true),
        )
        .unwrap();
        let addr = server.local_addr().unwrap();
        let observing = server.clone();
        let server_task = tokio::spawn(async move {
            let probe = observing.accept().await.unwrap().await.unwrap();
            probe.closed().await;
            let connection = observing.accept().await.unwrap().await.unwrap();
            tokio::select! { _ = connection.closed() => {}, stream = connection.accept_bi() => assert!(stream.is_err(), "Changed certificate received an application stream") }
        });
        let tls = crate::tls::server_config(&changed, PinState::new([], true)).unwrap();
        let changed_cfg = quinn::ServerConfig::with_crypto(std::sync::Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap(),
        ));
        let result = confirm_quic_peer(
            &client,
            addr,
            "removent",
            Box::new(move |_, answer| {
                server.set_server_config(Some(changed_cfg));
                answer.send(true).unwrap();
            }),
        )
        .await;
        assert!(matches!(result, Err(NetError::Tls(message)) if message.contains("changed")));
        server_task.await.unwrap();
    }
}
