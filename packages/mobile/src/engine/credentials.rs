use super::*;
use removent_client::keychain;
use removent_core::AuthenticationMode;

// Same account namespace as desktop. Neither endpoint strings nor relay tokens
// authorize native credential reuse; both certificate identities are required.
pub(crate) fn account(local: &str, host: [u8; 32], mode: AuthenticationMode) -> Option<String> {
    let method = match mode {
        AuthenticationMode::SystemAccount => "system",
        AuthenticationMode::Password => "password",
        _ => return None,
    };
    Some(format!(
        "native-auth:v1:{local}:{}:{method}",
        hex::encode(host)
    ))
}

pub(super) struct Answer {
    pub value: String,
    pub remember: bool,
}

pub(super) struct PendingAuthentication {
    mode: AuthenticationMode,
    answer: Answer,
    reused: bool,
}
pub(super) type Pending = Arc<Mutex<Option<PendingAuthentication>>>;

impl PendingAuthentication {
    pub(super) async fn commit(self, local: String, host: [u8; 32]) -> Result<()> {
        if self.reused {
            return Ok(());
        }
        let Some(key) = account(&local, host, self.mode) else {
            return Ok(());
        };
        tokio::task::spawn_blocking(move || {
            if self.answer.remember && self.mode.valid_input(&self.answer.value) {
                keychain::store(&key, &self.answer.value)
            } else {
                keychain::delete(&key)
            }
        })
        .await??;
        Ok(())
    }
}

pub(super) fn prompt(
    attempt: Attempt,
    trusted_host: Option<[u8; 32]>,
    pending: Pending,
) -> removent_client::PinRequest {
    prompt_with_loader(attempt, trusted_host, pending, keychain::load)
}

fn prompt_with_loader(
    attempt: Attempt,
    trusted_host: Option<[u8; 32]>,
    pending: Pending,
    load: impl FnOnce(&str) -> removent_core::Result<Option<String>> + Send + 'static,
) -> removent_client::PinRequest {
    Box::new(move |mode, mut pin_tx| {
        tokio::spawn(async move {
            let local = attempt.shared.identity.fingerprint_hex();
            let saved = if let Some(key) = trusted_host.and_then(|host| account(&local, host, mode))
            {
                match tokio::task::spawn_blocking(move || load(&key)).await {
                    Ok(Ok(Some(value))) if mode.valid_input(&value) => Some(value),
                    Ok(Ok(_)) => None,
                    _ => {
                        attempt.event(json!({"type":"notice", "code":"keychain_read_failed"}));
                        None
                    }
                }
            } else {
                None
            };
            if pin_tx.is_closed() {
                return;
            }
            attempt.progress(if mode == AuthenticationMode::PairingCode {
                "Pairing"
            } else {
                "Authenticating"
            });
            if let Some(value) = saved {
                *pending.lock().unwrap() = Some(PendingAuthentication {
                    mode,
                    answer: Answer {
                        value: value.clone(),
                        remember: true,
                    },
                    reused: true,
                });
                let _ = pin_tx.send(value);
                return;
            }
            let (tx, rx) = oneshot::channel();
            attempt.update(|s| {
                s.pin = Some(tx);
                s.auth_mode = mode;
                s.event(json!({"type":"pin", "mode":mode, "generation":attempt.generation}));
            });
            tokio::select! {
                _ = pin_tx.closed() => {},
                answer = rx => {
                    if let Ok(answer) = answer {
                        let value = answer.value.clone();
                        if account(&local, [0; 32], mode).is_some() {
                            *pending.lock().unwrap() = Some(PendingAuthentication { mode, answer, reused: false });
                        }
                        let _ = pin_tx.send(value);
                    }
                }
            }
        });
    })
}

pub(super) fn authentication_failure(
    error: removent_client::ConnectError,
    pending: &Pending,
) -> removent_client::ConnectError {
    if matches!(error, removent_client::ConnectError::Pairing(_))
        && pending
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|auth| auth.reused)
    {
        removent_client::ConnectError::Pairing("saved_native_credentials_failed".into())
    } else {
        error
    }
}

#[cfg(test)]
mod tests;
