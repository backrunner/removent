//! Keep saved credentials on this computer. keyring's Windows writer defaults
//! to enterprise persistence, which allows Windows profile roaming.
use super::{SERVICE, StoreError};
use windows_sys::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
    CredWriteW,
};
use zeroize::Zeroizing;

pub(super) fn store(account: &str, password: &str) -> Result<(), StoreError> {
    if account.contains('\0') {
        return Err(StoreError::Invalid(
            "account".into(),
            "contains a NUL".into(),
        ));
    }
    // Retain keyring's target naming and UTF-16 blob encoding so existing items
    // remain readable, editable and removable through its native backend.
    let entry = keyring::windows::WinCredential::new_with_target(None, SERVICE, account)?;
    let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut target = wide(&entry.target_name);
    let mut username = wide(&entry.username);
    let mut secret = Zeroizing::new(password.encode_utf16().collect::<Vec<_>>());
    let bytes = secret.len() * std::mem::size_of::<u16>();
    if bytes > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(StoreError::TooLong(
            "password encoded as UTF-16".into(),
            CRED_MAX_CREDENTIAL_BLOB_SIZE,
        ));
    }
    // Zero initializes optional pointers, flags, attributes and the ignored
    // timestamp. All referenced buffers remain live for this synchronous call.
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = target.as_mut_ptr();
    credential.UserName = username.as_mut_ptr();
    credential.CredentialBlobSize = bytes as u32;
    credential.CredentialBlob = secret.as_mut_ptr().cast();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        Err(keyring::windows::decode_error())
    } else {
        Ok(())
    }
}
