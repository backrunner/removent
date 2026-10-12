use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize)]
struct CheckHistory {
    last_checked_at: u64,
}

pub(super) fn load(path: &PathBuf) -> Option<u64> {
    let history: CheckHistory = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let timestamp = i64::try_from(history.last_checked_at).ok()?;
    (timestamp > 0 && chrono::DateTime::from_timestamp(timestamp, 0).is_some())
        .then_some(history.last_checked_at)
}

pub(super) fn save(path: &PathBuf, last_checked_at: u64) -> removent_core::Result<()> {
    removent_core::settings::atomic_write(
        path,
        &serde_json::to_vec(&CheckHistory { last_checked_at })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_corrupt_or_invalid_history_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("update-check.json");
        assert!(load(&path).is_none());
        std::fs::write(&path, "invalid JSON").unwrap();
        assert!(load(&path).is_none());
        for timestamp in [0, u64::MAX, i64::MAX as u64] {
            save(&path, timestamp).unwrap();
            assert!(load(&path).is_none());
        }
    }
}
