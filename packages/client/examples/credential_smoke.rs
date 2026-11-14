//! Exercise the actual credential store using only a unique test entry.
fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("REMOVENT_DESKTOP_TEST").as_deref() == Ok("1"),
        "Requires an isolated test desktop"
    );
    let account = format!("desktop-ci-{:032x}", rand::random::<u128>());
    if std::env::args().any(|arg| arg == "--expect-unavailable") {
        anyhow::ensure!(removent_client::keychain::load(&account).is_err());
        anyhow::ensure!(
            removent_client::keychain::store(&account, "must-not-reach-a-file").is_err()
        );
        anyhow::ensure!(removent_client::keychain::delete(&account).is_err());
        println!("PASS: unavailable native credential store fails closed");
        return Ok(());
    }
    let result = (|| -> anyhow::Result<()> {
        anyhow::ensure!(removent_client::keychain::load(&account)?.is_none());
        #[cfg(windows)]
        {
            keyring::Entry::new("com.alkinum.removent", &account)?
                .set_password("legacy test item")?;
            anyhow::ensure!(
                removent_client::keychain::load(&account)?.as_deref() == Some("legacy test item")
            );
        }
        removent_client::keychain::store(&account, "test password 一🔒")?;
        #[cfg(windows)]
        assert_local_persistence(&account)?;
        anyhow::ensure!(
            removent_client::keychain::load(&account)?.as_deref() == Some("test password 一🔒")
        );
        removent_client::keychain::store(&account, "updated password 二")?;
        #[cfg(windows)]
        assert_local_persistence(&account)?;
        anyhow::ensure!(
            removent_client::keychain::load(&account)?.as_deref() == Some("updated password 二")
        );
        #[cfg(windows)]
        {
            anyhow::ensure!(removent_client::keychain::store(&account, &"x".repeat(2561)).is_err());
            anyhow::ensure!(
                removent_client::keychain::load(&account)?.as_deref()
                    == Some("updated password 二")
            );
        }
        Ok(())
    })();
    let cleanup = removent_client::keychain::delete(&account);
    result?;
    cleanup?;
    anyhow::ensure!(removent_client::keychain::load(&account)?.is_none());
    println!("PASS: native credential store create, read, update, delete and missing entry");
    Ok(())
}

#[cfg(windows)]
fn assert_local_persistence(account: &str) -> anyhow::Result<()> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CredFree, CredReadW,
    };
    use zeroize::Zeroize;
    let target: Vec<u16> = format!("{account}.com.alkinum.removent")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut credential = std::ptr::null_mut();
    anyhow::ensure!(
        unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } != 0
    );
    let persistence = unsafe {
        let persistence = (*credential).Persist;
        if (*credential).CredentialBlobSize > 0 {
            std::slice::from_raw_parts_mut(
                (*credential).CredentialBlob,
                (*credential).CredentialBlobSize as usize,
            )
            .zeroize();
        }
        CredFree(credential.cast());
        persistence
    };
    anyhow::ensure!(
        persistence == CRED_PERSIST_LOCAL_MACHINE,
        "credentials must not roam"
    );
    println!("PASS: Windows credentials persist on this computer only");
    Ok(())
}
