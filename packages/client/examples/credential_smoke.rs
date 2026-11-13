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
        removent_client::keychain::store(&account, "test password 一")?;
        anyhow::ensure!(
            removent_client::keychain::load(&account)?.as_deref() == Some("test password 一")
        );
        removent_client::keychain::store(&account, "updated password 二")?;
        anyhow::ensure!(
            removent_client::keychain::load(&account)?.as_deref() == Some("updated password 二")
        );
        Ok(())
    })();
    let cleanup = removent_client::keychain::delete(&account);
    result?;
    cleanup?;
    anyhow::ensure!(removent_client::keychain::load(&account)?.is_none());
    println!("PASS: native credential store create, read, update, delete and missing entry");
    Ok(())
}
