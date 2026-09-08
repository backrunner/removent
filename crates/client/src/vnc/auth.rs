use super::{
    ARD_SERVER_FLAG_SESSION_SELECT, ARD_SESSION_CMD_CONNECT_CONSOLE,
    ARD_SESSION_CMD_CONNECT_VIRTUAL, ARD_SESSION_CMD_REQUEST_CONSOLE, ARD_SESSION_STATUS_GRANTED,
    ARD_SESSION_STATUS_GRANTED_AFTER_PENDING, ARD_SESSION_STATUS_PENDING,
    ARD_SESSION_STATUS_PENDING_ALT, ARD_VERSION, MAX_NAME, MAX_PIXELS, PixelFormat, RFB_VERSION,
    SEC_ARD, SEC_ARD_MACOS, SEC_NONE, SEC_VNC_AUTH, VncError,
};
use crate::connection::{ConnectionProgress, ConnectionStage, report_progress};
use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use rand::RngCore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) async fn handshake(
    stream: &mut TcpStream,
    username: &str,
    password: &[u8],
    progress: Option<&ConnectionProgress>,
) -> Result<(u32, u32, PixelFormat, bool), VncError> {
    report_progress(progress, ConnectionStage::Negotiating);
    let mut server_version = [0u8; 12];
    super::with_timeout("server greeting", 10, async {
        Ok(stream.read_exact(&mut server_version).await?)
    })
    .await?;
    if !server_version.starts_with(b"RFB ") {
        return Err(VncError::Protocol("invalid server version".into()));
    }
    let apple_ard = &server_version == ARD_VERSION;
    // RFB 003.889 is Apple's vendor banner, but the wire protocol is RFB 3.8.
    // Reply with the highest standard version understood by both peers. This
    // matters for old 3.3/3.7 servers, whose security handshake differs from
    // 3.8 (and is also what noVNC and gtk-vnc do for Apple servers).
    let client_version = negotiated_version(&server_version)
        .ok_or_else(|| VncError::Protocol("unsupported server version".into()))?;
    stream.write_all(client_version).await?;
    let legacy_33 = client_version.get(8..11) == Some(b"003");
    let security = super::with_timeout("security negotiation", 10, async {
        Ok(if legacy_33 {
            let mut kind = [0u8; 4];
            stream.read_exact(&mut kind).await?;
            u32::from_be_bytes(kind) as u8
        } else {
            let mut count = [0u8; 1];
            stream.read_exact(&mut count).await?;
            if count[0] == 0 {
                return Err(VncError::Protocol(
                    "server offered no security types".into(),
                ));
            }
            let mut types = vec![0u8; count[0] as usize];
            stream.read_exact(&mut types).await?;
            choose_security_type(&types, username, password)?
        })
    })
    .await?;
    if !legacy_33 {
        stream.write_all(&[security]).await?;
    }
    tracing::info!(security, apple_ard, "VNC security method selected");
    report_progress(progress, ConnectionStage::Authenticating);
    super::with_timeout("authentication challenge", 30, async {
        if matches!(security, SEC_ARD | SEC_ARD_MACOS) {
            ard_auth(stream, username, password).await?;
        } else if security == SEC_VNC_AUTH {
            let mut challenge = [0u8; 16];
            stream.read_exact(&mut challenge).await?;
            stream
                .write_all(&vnc_response(password, &challenge))
                .await?;
        } else if security != SEC_NONE {
            return Err(VncError::Protocol("unsupported security type".into()));
        }
        Ok(())
    })
    .await?;
    // RFB 3.3 omits SecurityResult for the None type; newer versions send it
    // for every selected security method.
    if !legacy_33 || security != SEC_NONE {
        let mut result = [0u8; 4];
        super::with_timeout("authentication result", 30, async {
            Ok(stream.read_exact(&mut result).await?)
        })
        .await?;
        if u32::from_be_bytes(result) != 0 {
            return Err(VncError::Authentication);
        }
    }
    // Apple Screen Sharing interoperates with the ordinary RFB ClientInit
    // byte. The vendor banner is used only to select Apple auth/input quirks;
    // sending private flag bits here makes strict servers reject the session.
    let ard_session = apple_ard;
    stream.write_all(&[1]).await?;

    report_progress(progress, ConnectionStage::PreparingDesktop);
    super::with_timeout("desktop initialization", 30, async {
        let mut init = [0u8; 24];
        stream.read_exact(&mut init).await?;
        let width = u16::from_be_bytes([init[0], init[1]]) as u32;
        let height = u16::from_be_bytes([init[2], init[3]]) as u32;
        if (!apple_ard && (width == 0 || height == 0))
            || (width != 0 && height != 0 && width as usize * height as usize > MAX_PIXELS)
        {
            return Err(VncError::Protocol("invalid framebuffer dimensions".into()));
        }
        let format = PixelFormat {
            bits_per_pixel: init[4],
            depth: init[5],
            big_endian: init[6] != 0,
            red_max: u16::from_be_bytes([init[8], init[9]]),
            green_max: u16::from_be_bytes([init[10], init[11]]),
            blue_max: u16::from_be_bytes([init[12], init[13]]),
            red_shift: init[14],
            green_shift: init[15],
            blue_shift: init[16],
        };
        let name_len = u32::from_be_bytes([init[20], init[21], init[22], init[23]]) as usize;
        if name_len > MAX_NAME {
            return Err(VncError::Protocol("server name is too large".into()));
        }
        let mut name = vec![0u8; name_len];
        stream.read_exact(&mut name).await?;

        // Apple extends the ServerInit name field with a binary capability header
        // when ClientInit includes the Select/Enhanced flags. The first byte is a
        // NUL marker, followed by a reserved byte, flags, and a 16-byte bitmap;
        // the human-readable server name follows the final NUL. Parse the flags
        // here so sessions that initially report a 0x0 framebuffer can complete
        // the required Session Select exchange before normal setup messages.
        let server_flags = if ard_session && name.len() >= 22 && name[0] == 0 {
            u32::from_be_bytes([name[2], name[3], name[4], name[5]])
        } else {
            0
        };
        if server_flags & ARD_SERVER_FLAG_SESSION_SELECT != 0 {
            ard_session_select(stream).await?;
        }
        Ok((width, height, format, ard_session))
    })
    .await
}

pub(super) fn negotiated_version(server_version: &[u8; 12]) -> Option<&'static [u8]> {
    let major = parse_version_component(server_version.get(4..7)?)?;
    let minor = parse_version_component(server_version.get(8..11)?)?;
    match (major, minor) {
        (3, 0..=6) => Some(b"RFB 003.003\n"),
        (3, 7) => Some(b"RFB 003.007\n"),
        (3, 8..=999) | (4..=999, _) => Some(RFB_VERSION),
        _ => None,
    }
}

fn parse_version_component(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 3 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        u16::from(bytes[0] - b'0') * 100
            + u16::from(bytes[1] - b'0') * 10
            + u16::from(bytes[2] - b'0'),
    )
}

/// Complete Apple's optional Session Select state machine. Apple sends this
/// immediately after the extended ServerInit when no console/virtual display
/// was selected by ClientInit. We select the console whenever available,
/// falling back to requesting one or connecting a virtual display according to
/// the server's advertised command bitmap.
async fn ard_session_select(stream: &mut TcpStream) -> Result<(), VncError> {
    let body_size = stream.read_u16().await? as usize;
    if !(10..=MAX_NAME).contains(&body_size) {
        return Err(VncError::Protocol("invalid Apple SessionInfo size".into()));
    }
    let mut body = vec![0u8; body_size];
    stream.read_exact(&mut body).await?;
    let allowed = u32::from_be_bytes(body[2..6].try_into().unwrap());
    let console_user = body[10..].split(|&byte| byte == 0).next().unwrap_or(&[]);

    let command = if allowed & (1 << ARD_SESSION_CMD_CONNECT_CONSOLE) != 0 {
        ARD_SESSION_CMD_CONNECT_CONSOLE
    } else if allowed & (1 << ARD_SESSION_CMD_REQUEST_CONSOLE) != 0 {
        ARD_SESSION_CMD_REQUEST_CONSOLE
    } else if allowed & (1 << ARD_SESSION_CMD_CONNECT_VIRTUAL) != 0 {
        ARD_SESSION_CMD_CONNECT_VIRTUAL
    } else {
        return Err(VncError::Protocol(
            "Apple server offered no usable session command".into(),
        ));
    };

    write_ard_session_command(stream, command, console_user).await?;
    // Pending is normal while macOS attaches the selected display. Bound the
    // number of status messages so a malicious peer cannot keep the handshake
    // alive forever, while still allowing a generous multi-second transition.
    for _ in 0..120 {
        let result_size = stream.read_u16().await? as usize;
        if !(6..=MAX_NAME).contains(&result_size) {
            return Err(VncError::Protocol(
                "invalid Apple SessionResult size".into(),
            ));
        }
        let mut result = vec![0u8; result_size];
        stream.read_exact(&mut result).await?;
        let status = u32::from_be_bytes(result[2..6].try_into().unwrap());
        match status {
            ARD_SESSION_STATUS_GRANTED | ARD_SESSION_STATUS_GRANTED_AFTER_PENDING => return Ok(()),
            ARD_SESSION_STATUS_PENDING | ARD_SESSION_STATUS_PENDING_ALT => continue,
            _ => {
                return Err(VncError::Protocol(format!(
                    "Apple session selection denied (status {status})"
                )));
            }
        }
    }
    Err(VncError::Protocol(
        "Apple session selection remained pending".into(),
    ))
}

async fn write_ard_session_command(
    stream: &mut TcpStream,
    command: u8,
    username: &[u8],
) -> Result<(), VncError> {
    if command > ARD_SESSION_CMD_CONNECT_VIRTUAL {
        return Err(VncError::Protocol("invalid Apple session command".into()));
    }
    let mut msg = [0u8; 74];
    msg[0..2].copy_from_slice(&72u16.to_be_bytes());
    msg[2..4].copy_from_slice(&1u16.to_be_bytes());
    msg[8] = command;
    let copy_len = username.len().min(63);
    msg[10..10 + copy_len].copy_from_slice(&username[..copy_len]);
    stream.write_all(&msg).await?;
    Ok(())
}

pub(super) fn choose_security_type(
    types: &[u8],
    username: &str,
    password: &[u8],
) -> Result<u8, VncError> {
    // Current Screen Sharing servers advertise both 30 and 35. Type 30
    // immediately supplies the standard ARD DH challenge; selecting 35 first
    // can leave both peers waiting for data (observed on RFB 003.889).
    if !username.is_empty() {
        if types.contains(&SEC_ARD) {
            return Ok(SEC_ARD);
        }
        if types.contains(&SEC_ARD_MACOS) {
            return Ok(SEC_ARD_MACOS);
        }
    }
    // macOS can also expose the separate legacy "VNC viewers" password. This
    // path deliberately remains available when no account username is set.
    if !password.is_empty() && types.contains(&SEC_VNC_AUTH) {
        return Ok(SEC_VNC_AUTH);
    }
    if types.contains(&SEC_NONE) {
        return Ok(SEC_NONE);
    }
    if types.contains(&SEC_VNC_AUTH) {
        return Ok(SEC_VNC_AUTH);
    }
    if username.is_empty()
        && types
            .iter()
            .any(|kind| matches!(*kind, SEC_ARD | SEC_ARD_MACOS))
    {
        return Err(VncError::Protocol(
            "Apple Remote Desktop requires a macOS username".into(),
        ));
    }
    Err(VncError::Protocol("unsupported security type".into()))
}

async fn ard_auth(stream: &mut TcpStream, username: &str, password: &[u8]) -> Result<(), VncError> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await?;
    let generator = u16::from_be_bytes([header[0], header[1]]);
    let key_len = u16::from_be_bytes([header[2], header[3]]) as usize;
    // Apple ARD uses a fixed-width DH group. Keep the bounds conservative so
    // a peer cannot force an unbounded allocation or an unusably small group.
    if !(16..=512).contains(&key_len) {
        return Err(VncError::Protocol("invalid Apple DH key length".into()));
    }
    let mut prime_bytes = vec![0u8; key_len];
    let mut server_public_bytes = vec![0u8; key_len];
    stream.read_exact(&mut prime_bytes).await?;
    stream.read_exact(&mut server_public_bytes).await?;
    let prime = BigUint::from_bytes_be(&prime_bytes);
    let server_public = BigUint::from_bytes_be(&server_public_bytes);
    let generator = BigUint::from(generator);
    if prime.bits() < 128
        || prime <= BigUint::from(4u8)
        || generator <= BigUint::from(1u8)
        || generator >= prime
        || server_public <= BigUint::from(1u8)
        || server_public >= prime
    {
        return Err(VncError::Protocol("invalid Apple DH parameters".into()));
    }

    let mut private_bytes = vec![0u8; key_len.max(32)];
    rand::thread_rng().fill_bytes(&mut private_bytes);
    // Keep the exponent in the valid subgroup range [2, p-2].
    let private = (BigUint::from_bytes_be(&private_bytes) % (&prime - 3u8)) + 2u8;
    let client_public = generator.modpow(&private, &prime);
    let shared = server_public.modpow(&private, &prime);
    let shared_bytes = fixed_be_bytes(&shared, key_len)?;

    let mut digest = Md5::new();
    digest.update(shared_bytes);
    let key: [u8; 16] = digest.finalize().into();
    let cipher = Aes128::new_from_slice(&key)
        .map_err(|_| VncError::Protocol("invalid Apple AES key".into()))?;
    let mut credentials = [0u8; 128];
    copy_c_string(
        &mut credentials[..64],
        username.as_bytes(),
        "Apple username",
    )?;
    copy_c_string(&mut credentials[64..], password, "Apple password")?;
    for chunk in credentials.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(chunk));
    }
    stream.write_all(&credentials).await?;
    let public_bytes = fixed_be_bytes(&client_public, key_len)?;
    stream.write_all(&public_bytes).await?;
    Ok(())
}

pub(super) fn fixed_be_bytes(value: &BigUint, len: usize) -> Result<Vec<u8>, VncError> {
    let bytes = value.to_bytes_be();
    if bytes.len() > len {
        return Err(VncError::Protocol(
            "Apple DH value exceeds key length".into(),
        ));
    }
    let mut out = vec![0u8; len];
    out[len - bytes.len()..].copy_from_slice(&bytes);
    Ok(out)
}

pub(super) fn copy_c_string(dst: &mut [u8], src: &[u8], label: &str) -> Result<(), VncError> {
    if dst.is_empty() || src.len() >= dst.len() {
        return Err(VncError::Protocol(format!(
            "{label} is too long for Apple ARD (maximum {} bytes)",
            dst.len().saturating_sub(1)
        )));
    }
    dst.fill(0);
    dst[..src.len()].copy_from_slice(src);
    Ok(())
}

pub(super) fn vnc_response(password: &[u8], challenge: &[u8; 16]) -> [u8; 16] {
    use des::Des;
    use des::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
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
