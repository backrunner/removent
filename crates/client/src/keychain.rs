//! Password storage for saved connections.
//!
//! `connections.json` only records that a password exists (`password_hint`);
//! the secret itself lives in the macOS Keychain as a generic password keyed by
//! the bookmark's stable id. Items created by this app are readable by it
//! without an interactive prompt.

use removent_core::{CoreError, Result};

/// Generic-password service name; shows as the item name in Keychain Access.
/// Matches the app bundle identifier.
const SERVICE: &str = "com.alkinum.removent";

fn err(e: impl std::fmt::Display) -> CoreError {
    CoreError::SecureStore(e.to_string())
}

/// Create or update the stored password for a bookmark id.
pub fn store(account: &str, password: &str) -> Result<()> {
    store_imp(account, password).map_err(err)
}

/// Read the stored password; `Ok(None)` when no item exists.
pub fn load(account: &str) -> Result<Option<String>> {
    match load_imp(account) {
        Ok(bytes) => Ok(Some(
            String::from_utf8(bytes).map_err(|e| CoreError::SecureStore(e.to_string()))?,
        )),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(err(e)),
    }
}

/// Delete the stored password; a missing item is not an error.
pub fn delete(account: &str) -> Result<()> {
    match delete_imp(account) {
        Ok(()) => Ok(()),
        Err(e) if is_not_found(&e) => Ok(()),
        Err(e) => Err(err(e)),
    }
}

#[cfg(target_os = "macos")]
type StoreError = security_framework::base::Error;

#[cfg(target_os = "macos")]
fn is_not_found(e: &StoreError) -> bool {
    e.code() == security_framework_sys::base::errSecItemNotFound
}

#[cfg(target_os = "macos")]
fn store_imp(account: &str, password: &str) -> std::result::Result<(), StoreError> {
    security_framework::passwords::set_generic_password(SERVICE, account, password.as_bytes())
}

#[cfg(target_os = "macos")]
fn load_imp(account: &str) -> std::result::Result<Vec<u8>, StoreError> {
    use security_framework::passwords::{PasswordOptions, generic_password};
    generic_password(PasswordOptions::new_generic_password(SERVICE, account))
}

#[cfg(target_os = "macos")]
fn delete_imp(account: &str) -> std::result::Result<(), StoreError> {
    security_framework::passwords::delete_generic_password(SERVICE, account)
}

#[cfg(not(target_os = "macos"))]
type StoreError = String;

#[cfg(not(target_os = "macos"))]
fn is_not_found(_: &StoreError) -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn store_imp(_: &str, _: &str) -> std::result::Result<(), StoreError> {
    Err("keychain is only supported on macOS".into())
}

#[cfg(not(target_os = "macos"))]
fn load_imp(_: &str) -> std::result::Result<Vec<u8>, StoreError> {
    Err("keychain is only supported on macOS".into())
}

#[cfg(not(target_os = "macos"))]
fn delete_imp(_: &str) -> std::result::Result<(), StoreError> {
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // Touches the real login keychain — run manually with `cargo test -p
    // removent-client keychain -- --ignored`.
    #[test]
    #[ignore]
    fn store_load_delete_round_trip() {
        let account = format!("test-{:x}", rand::random::<u64>());
        store(&account, "hunter2").unwrap();
        assert_eq!(load(&account).unwrap().as_deref(), Some("hunter2"));
        store(&account, "hunter3").unwrap();
        assert_eq!(load(&account).unwrap().as_deref(), Some("hunter3"));
        delete(&account).unwrap();
        assert_eq!(load(&account).unwrap(), None);
    }
}
