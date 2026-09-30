use super::*;

pub(super) fn state_path(path: &Path) -> std::path::PathBuf {
    path.with_file_name("connections-sync.json")
}
pub(super) fn journal_path(path: &Path) -> std::path::PathBuf {
    path.with_file_name("connections-transaction.json")
}
pub(crate) fn read_entries(path: &Path) -> Result<Vec<SavedConnection>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn read_state(path: &Path) -> Result<SyncState> {
    let state: SyncState = match std::fs::read(state_path(path)) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SyncState::default(),
        Err(e) => return Err(e.into()),
    };
    if state.version != 1 {
        return Err(failure("Unsupported local sync schema"));
    }
    Ok(state)
}
pub(crate) fn recover(path: &Path) -> Result<()> {
    let journal: Journal = match std::fs::read(journal_path(path)) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if journal.version != 1 || journal.sync.version != 1 {
        return Err(failure("Unsupported connection journal"));
    }
    atomic_write(
        &path.to_path_buf(),
        &serde_json::to_vec_pretty(&journal.entries)?,
    )?;
    atomic_write(&state_path(path), &serde_json::to_vec(&journal.sync)?)?;
    sync_directory(path)?;
    std::fs::remove_file(journal_path(path))?;
    sync_directory(path)
}
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
pub(crate) fn commit(path: &Path, entries: &[SavedConnection], sync: &SyncState) -> Result<()> {
    atomic_write(
        &journal_path(path),
        &serde_json::to_vec(&Journal {
            version: 1,
            entries: entries.to_vec(),
            sync: sync.clone(),
        })?,
    )?;
    // Once the journal exists, the logical commit is complete. Returning failure
    // would cause save_with_password to delete the newly committed credential.
    if let Err(error) = sync_directory(path).and_then(|_| recover(path)) {
        tracing::warn!(%error, "connection commit retained for recovery");
    }
    Ok(())
}
