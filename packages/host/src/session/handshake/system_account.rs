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

#[cfg(target_os = "linux")]
pub(super) fn verify(credentials: &SystemCredentials) -> bool {
    if !credentials.valid() {
        return false;
    }
    // Require the UID of the running desktop, not an arbitrary valid account.
    let Ok(name) = std::ffi::CString::new(credentials.username.as_str()) else {
        return false;
    };
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    let mut storage = vec![0; 65536];
    if unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            &mut entry,
            storage.as_mut_ptr(),
            storage.len(),
            &mut result,
        )
    } != 0
        || result.is_null()
        || entry.pw_uid != unsafe { libc::geteuid() }
        || entry.pw_uid == 0
    {
        return false;
    }
    let conversation = pam_client::conv_mock::Conversation::with_credentials(
        &credentials.username,
        &credentials.password,
    );
    let Ok(mut client) =
        pam_client::Context::new("login", Some(&credentials.username), conversation)
    else {
        return false;
    };
    client
        .authenticate(pam_client::Flag::DISALLOW_NULL_AUTHTOK)
        .is_ok()
        && client.acct_mgmt(pam_client::Flag::NONE).is_ok()
        && client.user().is_ok_and(|name| name == credentials.username)
}
#[cfg(any(windows, test))]
fn windows_logon_identity(username: &str) -> (Option<&str>, &str) {
    if let Some((domain, name)) = username.split_once('\\') {
        (Some(domain), name)
    } else if username.contains('@') {
        (None, username)
    } else {
        (Some("."), username)
    }
}

#[cfg(windows)]
pub(super) fn verify(credentials: &SystemCredentials) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::*,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    if !credentials.valid() {
        return false;
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn user(token: HANDLE) -> Option<Vec<usize>> {
        let mut needed = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        (unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } != 0)
            .then_some(buffer)
    }
    let (domain, name) = windows_logon_identity(&credentials.username);
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let name = wide(name);
    let domain = domain.map(wide);
    let mut password = wide(&credentials.password);
    let mut authenticated = std::ptr::null_mut();
    let ok = unsafe {
        LogonUserW(
            name.as_ptr(),
            domain.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            password.as_ptr(),
            LOGON32_LOGON_NETWORK,
            LOGON32_PROVIDER_DEFAULT,
            &mut authenticated,
        )
    };
    password.fill(0);
    if ok == 0 {
        return false;
    }
    let authenticated = Token(authenticated);
    let mut current = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut current) } == 0 {
        return false;
    }
    let current = Token(current);
    let (Some(a), Some(b)) = (user(authenticated.0), user(current.0)) else {
        return false;
    };
    unsafe {
        EqualSid(
            (*(a.as_ptr().cast::<TOKEN_USER>())).User.Sid,
            (*(b.as_ptr().cast::<TOKEN_USER>())).User.Sid,
        ) != 0
    }
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
    #[test]
    fn windows_account_names_preserve_explicit_domains() {
        for (input, domain, name) in [
            ("alice", Some("."), "alice"),
            ("WORKGROUP\\alice", Some("WORKGROUP"), "alice"),
            ("alice@example.com", None, "alice@example.com"),
            (
                "MicrosoftAccount\\alice@example.com",
                Some("MicrosoftAccount"),
                "alice@example.com",
            ),
        ] {
            assert_eq!(windows_logon_identity(input), (domain, name));
        }
    }
    #[test]
    #[ignore = "requires an isolated OS account with REMOVENT_AUTH_TEST_USERNAME/PASSWORD"]
    fn native_desktop_account_authentication() {
        let username = std::env::var("REMOVENT_AUTH_TEST_USERNAME").unwrap();
        let password = std::env::var("REMOVENT_AUTH_TEST_PASSWORD").unwrap();
        if std::env::var("REMOVENT_AUTH_EXPECT_REJECT").as_deref() == Ok("1") {
            assert!(!verify(&SystemCredentials { username, password }));
            return;
        }
        assert!(verify(&SystemCredentials {
            username: username.clone(),
            password,
        }));
        assert!(!verify(&SystemCredentials {
            username,
            password: "intentionally-wrong-native-test-password".into(),
        }));
        assert!(!verify(&SystemCredentials {
            username: "removent-nonexistent-test-account".into(),
            password: "irrelevant".into(),
        }));
    }
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
