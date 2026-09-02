use super::{DisplayTarget, MAX_FRAME_BYTES, RFB_VERSION, SEC_NONE, SEC_VNC_AUTH};
use crate::session::fit_capture_dims;
use des::Des;
use des::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use rand::RngCore;
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) async fn perform_handshake(
    stream: &mut TcpStream,
    password: &[u8],
) -> io::Result<DisplayTarget> {
    stream.write_all(RFB_VERSION).await?;
    let mut client_version = [0u8; 12];
    stream.read_exact(&mut client_version).await?;
    if !client_version.starts_with(b"RFB ") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RFB version",
        ));
    }

    let security = if password.is_empty() {
        SEC_NONE
    } else {
        SEC_VNC_AUTH
    };
    // RFB 3.3 sends one u32 security type; 3.7+ sends a count/list followed
    // by the client's one-byte selection. Apple clients normally negotiate
    // 3.8, but accepting 3.3 costs little and helps older VNC viewers.
    let legacy_33 = client_version.get(8..11) == Some(b"003");
    if legacy_33 {
        stream.write_all(&(security as u32).to_be_bytes()).await?;
    } else {
        stream.write_all(&[1, security]).await?;
        let mut selected = [0u8; 1];
        stream.read_exact(&mut selected).await?;
        if selected[0] != security {
            send_security_failure(stream, "unsupported security type").await?;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unsupported security type",
            ));
        }
    }
    if security == SEC_VNC_AUTH && !check_vnc_password(stream, password).await? {
        send_security_failure(stream, "VNC authentication failed").await?;
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "VNC authentication failed",
        ));
    }
    // RFB 3.3 has no SecurityResult for the None security type; sending one
    // would leave four bytes in front of ServerInit and desynchronise legacy
    // clients. RFB 3.7+ always receives SecurityResult.
    if !legacy_33 || security != SEC_NONE {
        stream.write_all(&0u32.to_be_bytes()).await?;
    }

    let target = current_display();
    if target.capture_width as usize * target.capture_height as usize * 4 > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "framebuffer too large",
        ));
    }
    let mut shared = [0u8; 1];
    stream.read_exact(&mut shared).await?;
    write_server_init(stream, &target).await?;
    Ok(target)
}

async fn check_vnc_password(stream: &mut TcpStream, password: &[u8]) -> io::Result<bool> {
    let mut challenge = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut challenge);
    stream.write_all(&challenge).await?;
    let mut response = [0u8; 16];
    stream.read_exact(&mut response).await?;
    Ok(vnc_response(password, &challenge) == response)
}

pub(super) fn vnc_response(password: &[u8], challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = [0u8; 8];
    for (i, byte) in password.iter().take(8).enumerate() {
        key[i] = byte.reverse_bits();
    }
    let cipher = Des::new(GenericArray::from_slice(&key));
    let mut response = *challenge;
    for block in response.as_chunks_mut::<8>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    response
}

async fn send_security_failure(stream: &mut TcpStream, reason: &str) -> io::Result<()> {
    stream.write_all(&1u32.to_be_bytes()).await?;
    let bytes = reason.as_bytes();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    stream.write_all(bytes).await
}

fn current_display() -> DisplayTarget {
    let display = removent_input::display_list()
        .into_iter()
        .find(|d| d.is_main)
        .or_else(|| removent_input::display_list().into_iter().next())
        .unwrap_or(removent_proto::DisplayInfo {
            id: 1,
            w_px: 1280,
            h_px: 720,
            scale: 1.0,
            dpi: 96,
            is_main: true,
        });
    let (capture_width, capture_height) = fit_capture_dims(display.w_px, display.h_px);
    DisplayTarget {
        id: display.id,
        width: capture_width,
        height: capture_height,
        capture_width,
        capture_height,
    }
}

async fn write_server_init(stream: &mut TcpStream, target: &DisplayTarget) -> io::Result<()> {
    stream
        .write_all(&(target.width as u16).to_be_bytes())
        .await?;
    stream
        .write_all(&(target.height as u16).to_be_bytes())
        .await?;
    // 32bpp, 24-bit depth, little endian, true colour, BGR byte order.
    stream
        .write_all(&[
            32, 24, 0, 1, // bits-per-pixel, depth, endian, true-colour
        ])
        .await?;
    stream.write_all(&255u16.to_be_bytes()).await?;
    stream.write_all(&255u16.to_be_bytes()).await?;
    stream.write_all(&255u16.to_be_bytes()).await?;
    stream.write_all(&16u8.to_be_bytes()).await?;
    stream.write_all(&8u8.to_be_bytes()).await?;
    stream.write_all(&0u8.to_be_bytes()).await?;
    stream.write_all(&[0, 0, 0]).await?;
    let name = b"Removent VNC";
    stream.write_all(&(name.len() as u32).to_be_bytes()).await?;
    stream.write_all(name).await
}
