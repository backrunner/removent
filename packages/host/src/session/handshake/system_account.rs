//! Verify the account owning this desktop service, never another user's desktop.
//! Passwords go directly to Open Directory, without shell arguments or persistence.
use removent_proto::SystemCredentials;

use super::*;
use removent_net::{
    PairingMsg,
    session::{PairSink, PairSource},
};

pub(super) async fn exchange(
    begin: PairingMsg,
    peer_fp: &str,
    sink: &mut PairSink,
    source: &mut PairSource,
    verifier: fn(&SystemCredentials) -> bool,
) -> Result<(), HostError> {
    let PairingMsg::Begin { nonce_c, fp_c } = begin else {
        return Err(HostError::Rejected("Invalid authentication request".into()));
    };
    if fp_c != peer_fp {
        return Err(HostError::Rejected(
            "Authentication identity mismatch".into(),
        ));
    }
    let nonce_h = new_token();
    sink.send(PairingMsg::SystemChallenge { nonce_h }).await?;
    let verify = tokio::time::timeout(Duration::from_secs(300), source.next())
        .await
        .map_err(|_| HostError::Timeout("system account credentials"))?
        .transpose()?
        .ok_or(HostError::Timeout("system account credentials"))?;
    let PairingMsg::SystemVerify {
        nonce_c: client_nonce,
        nonce_h: host_nonce,
        credentials,
    } = verify
    else {
        return Err(HostError::Rejected(
            "Invalid authentication response".into(),
        ));
    };
    if client_nonce != nonce_c || host_nonce != nonce_h || !credentials.valid() {
        return Err(HostError::Rejected(
            "Invalid authentication response".into(),
        ));
    }
    let ok = tokio::task::spawn_blocking(move || verifier(&credentials))
        .await
        .unwrap_or(false);
    sink.send(PairingMsg::SystemConfirm {
        nonce_c,
        nonce_h,
        ok,
    })
    .await?;
    if ok {
        Ok(())
    } else {
        Err(HostError::Rejected("System account authentication failed; check the sharing user's username and login password".into()))
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn verify(_: &SystemCredentials) -> bool {
    false
}

#[cfg(target_os = "macos")]
pub(super) fn verify(credentials: &SystemCredentials) -> bool {
    use core_foundation::{
        base::{CFRelease, TCFType},
        string::{CFString, CFStringRef},
    };
    use std::{
        ffi::{CString, c_void},
        ptr,
    };
    type Ref = *const c_void;
    #[link(name = "OpenDirectory", kind = "framework")]
    unsafe extern "C" {
        static kODSessionDefault: Ref;
        static kODRecordTypeUsers: CFStringRef;
        fn ODNodeCreateWithNodeType(
            allocator: Ref,
            session: Ref,
            kind: u32,
            error: *mut Ref,
        ) -> Ref;
        fn ODNodeCopyRecord(
            node: Ref,
            kind: CFStringRef,
            name: CFStringRef,
            attributes: Ref,
            error: *mut Ref,
        ) -> Ref;
        fn ODRecordVerifyPassword(record: Ref, password: CFStringRef, error: *mut Ref) -> bool;
    }
    if !credentials.valid() {
        return false;
    }
    let Ok(username) = CString::new(credentials.username.as_str()) else {
        return false;
    };
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0u8; 65536];
    let mut result = ptr::null_mut();
    // getpwnam_r owns no global/static buffer and is safe on a worker thread.
    let owner = unsafe {
        libc::getpwnam_r(
            username.as_ptr(),
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) == 0
            && !result.is_null()
            && (*result).pw_uid == libc::geteuid()
            && (*result).pw_uid != 0
    };
    if !owner {
        return false;
    }
    let username = CFString::new(&credentials.username);
    let password = CFString::new(&credentials.password);
    // Create/Copy objects are released on every path. No error objects are requested,
    // so system diagnostics cannot accidentally expose account or credential details.
    unsafe {
        let node =
            ODNodeCreateWithNodeType(ptr::null(), kODSessionDefault, 0x2201, ptr::null_mut());
        if node.is_null() {
            return false;
        }
        let record = ODNodeCopyRecord(
            node,
            kODRecordTypeUsers,
            username.as_concrete_TypeRef(),
            ptr::null(),
            ptr::null_mut(),
        );
        let ok = if record.is_null() {
            false
        } else {
            let ok =
                ODRecordVerifyPassword(record, password.as_concrete_TypeRef(), ptr::null_mut());
            CFRelease(record);
            ok
        };
        CFRelease(node);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exchange_requires_matching_nonces_and_successful_verification() {
        for (password, replay, expected) in [
            ("correct", false, true),
            ("incorrect", false, false),
            ("correct", true, false),
        ] {
            let (_endpoints, client, host) = crate::session::tests::quic_pair().await;
            let peer = hex::encode(host.peer_fingerprint().unwrap());
            let server = host.clone();
            let task = tokio::spawn(async move {
                let (mut sink, mut source) = server.accept_pairing().await.unwrap();
                let begin = source.next().await.unwrap().unwrap();
                exchange(begin, &peer, &mut sink, &mut source, |credentials| {
                    credentials.username == "owner" && credentials.password == "correct"
                })
                .await
            });
            let (mut sink, mut source) = client.open_pairing().await.unwrap();
            let nonce_c = [9; 16];
            sink.send(PairingMsg::Begin {
                nonce_c,
                fp_c: hex::encode(host.peer_fingerprint().unwrap()),
            })
            .await
            .unwrap();
            let PairingMsg::SystemChallenge { nonce_h } = source.next().await.unwrap().unwrap()
            else {
                panic!("system challenge");
            };
            sink.send(PairingMsg::SystemVerify {
                nonce_c,
                nonce_h: if replay { [0; 16] } else { nonce_h },
                credentials: SystemCredentials {
                    username: "owner".into(),
                    password: password.into(),
                },
            })
            .await
            .unwrap();
            if !replay {
                assert_eq!(
                    source.next().await.unwrap().unwrap(),
                    PairingMsg::SystemConfirm {
                        nonce_c,
                        nonce_h,
                        ok: expected
                    }
                );
            }
            assert_eq!(task.await.unwrap().is_ok(), expected);
        }
    }

    #[test]
    fn other_accounts_and_invalid_input_are_rejected_without_password_verification() {
        for username in ["root", "removent-nonexistent-test-account", ""] {
            assert!(!verify(&SystemCredentials {
                username: username.into(),
                password: "not-a-real-password".into()
            }));
        }
    }
}
