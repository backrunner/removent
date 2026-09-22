//! Cloudflare management without a shell, Python, or credentials in argv.
use crate::{cli::RemoteAction, management::read_private};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Deserialize, Serialize)]
struct Status {
    state: String,
    enabled: bool,
    running: bool,
}

fn endpoint(origin: &str, action: RemoteAction) -> Result<Url> {
    let mut url =
        Url::parse(origin).map_err(|_| anyhow::anyhow!("Invalid relay management origin"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        (url.scheme() == "https" || (url.scheme() == "http" && loopback))
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Use an HTTPS origin without a path, query or credentials"
    );
    url.set_path(&format!("/admin/{}", action.name()));
    Ok(url)
}

pub async fn control(action: RemoteAction, origin: &str, file: &Path) -> Result<()> {
    let url = endpoint(origin, action)?;
    let token = read_private(file, 128)?;
    removent_relay::config::decode_secret(token.trim())?;
    let client = Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(15))
        .build()?;
    let builder = if matches!(action, RemoteAction::Status) {
        client.get(url)
    } else {
        client.post(url)
    };
    let mut response = builder
        .bearer_auth(token.trim())
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Relay management connection failed"))?;
    ensure!(
        response.status().is_success(),
        "Relay management failed (HTTP {})",
        response.status().as_u16()
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Cannot read management response")?
    {
        if body.len() + chunk.len() > 4096 {
            bail!("Relay management response is too large");
        }
        body.extend_from_slice(&chunk);
    }
    let status: Status = serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("Invalid relay management status (contents redacted)"))?;
    ensure!(
        status.state.len() <= 64
            && status
                .state
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-'),
        "Invalid relay state"
    );
    println!("{}", serde_json::to_string(&status)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
    };

    async fn request(action: RemoteAction, response: String) -> (Result<()>, String) {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("admin.token");
        fs::write(&file, "12".repeat(32)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                connection.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 4096);
            }
            let _ = connection.write_all(response.as_bytes());
            String::from_utf8(request).unwrap()
        });
        let result = control(action, &origin, &file).await;
        (result, server.join().unwrap())
    }

    fn response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn control_uses_the_expected_method_route_and_private_bearer() {
        for (action, method) in [
            (RemoteAction::Start, "POST"),
            (RemoteAction::Stop, "POST"),
            (RemoteAction::Status, "GET"),
        ] {
            let (result, request) = request(
                action,
                response(
                    "200 OK",
                    r#"{"state":"running","enabled":true,"running":true}"#,
                ),
            )
            .await;
            result.unwrap();
            assert!(
                request.starts_with(&format!("{method} /admin/{} HTTP/1.1\r\n", action.name()))
            );
            assert!(
                request
                    .to_lowercase()
                    .contains(&format!("authorization: bearer {}\r\n", "12".repeat(32)))
            );
            assert!(!request.lines().next().unwrap().contains(&"12".repeat(32)));
        }
    }

    #[tokio::test]
    async fn control_rejects_redirects_oversized_and_secret_error_bodies() {
        for reply in [
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/credential-trap\r\nContent-Length: 0\r\n\r\n".to_string(),
            response("401 Unauthorized", "SECRET_FROM_SERVER"),
            response("200 OK", "SECRET_FROM_SERVER"),
            response("200 OK", &"x".repeat(4097)),
        ] {
            let (result, _) = request(RemoteAction::Status, reply).await;
            let error = format!("{:#}", result.unwrap_err());
            assert!(!error.contains("SECRET_FROM_SERVER"));
            assert!(!error.contains(&"12".repeat(32)));
            assert!(!error.contains("credential-trap"));
        }
    }
    #[test]
    fn management_origin_cannot_smuggle_tokens_or_redirect_destinations() {
        for bad in [
            "http://example.com",
            "https://user:password@example.com",
            "https://example.com/path",
            "https://example.com?token=x",
            "https://example.com#x",
            "file:///tmp/token",
        ] {
            assert!(endpoint(bad, RemoteAction::Start).is_err(), "{bad}");
        }
        assert_eq!(
            endpoint("https://relay.example:443", RemoteAction::Stop)
                .unwrap()
                .as_str(),
            "https://relay.example/admin/stop"
        );
    }
}
