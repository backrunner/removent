use super::*;

impl Engine {
    pub fn device_name(&self) -> String {
        self.settings.lock().unwrap().device_name.clone()
    }

    pub fn identity(&self) -> Result<DeviceIdentity> {
        let mut cached = self.identity.lock().unwrap();
        if let Some(identity) = cached.as_ref() {
            return Ok(identity.clone());
        }
        let identity = removent_core::identity::load_or_create(&self.paths, &self.device_name())?;
        *cached = Some(identity.clone());
        Ok(identity)
    }

    pub fn fingerprint_short(&self) -> String {
        self.identity()
            .map(|id| id.short_fingerprint_hex())
            .unwrap_or_default()
    }

    // ---- saved connections (bookmarks; passwords are never persisted) ----

    /// Snapshot of the saved-connection list, read fresh so edits by another
    /// window or instance are picked up.
    pub fn saved_connections(&self) -> Vec<SavedConnection> {
        SavedConnections::load(&self.paths)
            .map(|s| s.all().to_vec())
            .unwrap_or_default()
    }

    /// Persist a submitted connection form as a reusable bookmark. Same-endpoint
    /// submissions update the existing entry instead of duplicating it. The
    /// password goes to a versioned Keychain item referenced by the bookmark;
    /// empty means the reference is cleared and the old item is retired.
    pub fn save_connection(
        &self,
        request: &ConnectionRequest,
        name: String,
        id: Option<&str>,
    ) -> Result<SavedConnection> {
        let mut store = SavedConnections::load(&self.paths)?;
        let mut entry = SavedConnection::from_request(request, name);
        entry.id = id.unwrap_or_default().to_owned();
        Ok(store.save_with_password(entry, &request.password)?)
    }

    /// Only read the item referenced by the current bookmark. Cleared passwords
    /// must not reappear even when deleting an old Keychain item failed.
    pub fn saved_password(&self, entry: &SavedConnection) -> Option<String> {
        let account = entry.credential_account()?;
        match keychain::load(account) {
            Ok(password) => password,
            Err(e) => {
                tracing::warn!(err = %e, "failed to read connection password from keychain");
                None
            }
        }
    }

    pub fn touch_saved_connection(&self, id: &str) {
        if let Ok(mut store) = SavedConnections::load(&self.paths)
            && let Err(error) = store.touch(id)
        {
            tracing::warn!(%error, "could not refresh bookmark timestamp");
        }
    }

    pub fn remove_saved_connection(&self, id: &str) -> Result<()> {
        let mut store = SavedConnections::load(&self.paths)?;
        Ok(store.remove_with_password(id)?)
    }

    /// Short-fingerprint set of trusted devices (the "Paired" marker in the device list).
    pub fn trusted_short_fps(&self) -> std::collections::HashSet<String> {
        PeersStore::load(&self.paths)
            .map(|p| {
                p.all()
                    .iter()
                    .filter(|r| r.trusted)
                    .map(|r| r.short_fp.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---- daemon control ----
}
