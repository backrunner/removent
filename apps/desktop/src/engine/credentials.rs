//! Native host credentials stay in Keychain, scoped to both device identities
//! and the host's authentication method. Never use a relay token or an IP as a key.
use super::*;
use removent_core::AuthenticationMode;

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticationAnswer {
    pub value: String,
    pub remember: bool,
}

impl std::fmt::Debug for AuthenticationAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticationAnswer")
            .field("value", &"[redacted]")
            .field("remember", &self.remember)
            .finish()
    }
}

pub fn can_remember(mode: AuthenticationMode) -> bool {
    cfg!(target_os = "macos")
        && matches!(
            mode,
            AuthenticationMode::SystemAccount | AuthenticationMode::Password
        )
}

fn account(local: &str, host: [u8; 32], mode: AuthenticationMode) -> Option<String> {
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

pub(super) struct PendingAuthentication {
    mode: AuthenticationMode,
    answer: AuthenticationAnswer,
    reused: bool,
}

impl PendingAuthentication {
    fn commit_with(
        self,
        local: &str,
        host: [u8; 32],
        store: impl FnOnce(&str, &str) -> removent_core::Result<()>,
        delete: impl FnOnce(&str) -> removent_core::Result<()>,
    ) -> removent_core::Result<()> {
        // Reusing a credential must not overwrite an edit made during the connection.
        if self.reused {
            return Ok(());
        }
        let Some(account) = account(local, host, self.mode) else {
            return Ok(());
        };
        if self.answer.remember && self.mode.valid_input(&self.answer.value) {
            store(&account, &self.answer.value)
        } else {
            delete(&account)
        }
    }

    pub(super) async fn commit(self, local: String, host: [u8; 32]) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            self.commit_with(&local, host, keychain::store, keychain::delete)
        })
        .await??;
        Ok(())
    }
}

pub(super) type Pending = Arc<Mutex<Option<PendingAuthentication>>>;

pub(super) fn prompt(
    attempt: ClientAttempt,
    local: String,
    trusted_host: Option<[u8; 32]>,
    pending: Pending,
) -> removent_client::PinRequest {
    prompt_with_loader(attempt, local, trusted_host, pending, keychain::load)
}

fn prompt_with_loader(
    attempt: ClientAttempt,
    local: String,
    trusted_host: Option<[u8; 32]>,
    pending: Pending,
    load: impl FnOnce(&str) -> removent_core::Result<Option<String>> + Send + 'static,
) -> removent_client::PinRequest {
    Box::new(move |mode, mut pin_tx| {
        tokio::spawn(async move {
            let saved = if can_remember(mode) {
                if let Some(account) = trusted_host.and_then(|host| account(&local, host, mode)) {
                    match tokio::task::spawn_blocking(move || load(&account)).await {
                        Ok(Ok(Some(value))) if mode.valid_input(&value) => {
                            Some(AuthenticationAnswer {
                                value,
                                remember: true,
                            })
                        }
                        Ok(Ok(_)) => None,
                        _ => {
                            let _ = attempt
                                .events
                                .send(UiEvent::Notice(t!("auth.keychain_read_failed").to_string()));
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            // If the connection was cancelled during a Keychain read, do not open a dialog.
            if pin_tx.is_closed() {
                return;
            }
            attempt.progress(if mode == AuthenticationMode::PairingCode {
                ConnectionStage::Pairing
            } else {
                ConnectionStage::Authenticating
            });
            if let Some(answer) = saved {
                let value = answer.value.clone();
                *pending.lock().unwrap() = Some(PendingAuthentication {
                    mode,
                    answer,
                    reused: true,
                });
                let _ = pin_tx.send(value);
                return;
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            if attempt
                .events
                .send(UiEvent::ClientNeedsPin {
                    generation: attempt.generation,
                    mode,
                    tx,
                })
                .is_err()
            {
                return;
            }
            tokio::select! {
                _ = pin_tx.closed() => {},
                answer = rx => {
                    if let Ok(mut answer) = answer {
                        answer.remember &= can_remember(mode);
                        let value = answer.value.clone();
                        // Record before waking the handshake. Failed/cancelled handshakes
                        // drop this in-memory value without changing Keychain.
                        if can_remember(mode) {
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
        removent_client::ConnectError::Pairing(t!("auth.saved_credentials_failed").to_string())
    } else {
        error
    }
}

mod saved;
pub use saved::{NativeCredential, NativeCredentialEdit};

#[cfg(test)]
mod tests;
