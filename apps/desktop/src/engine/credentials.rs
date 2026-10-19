//! Native host credentials stay in Keychain, scoped to both device identities
//! and the host's authentication method. Never use a relay token or an IP as a key.
use super::*;
use removent_core::AuthenticationMode;

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
}

impl PendingAuthentication {
    fn commit_with(
        self,
        local: &str,
        host: [u8; 32],
        store: impl FnOnce(&str, &str) -> removent_core::Result<()>,
        delete: impl FnOnce(&str) -> removent_core::Result<()>,
    ) -> removent_core::Result<()> {
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
    Box::new(move |mode, mut pin_tx| {
        tokio::spawn(async move {
            let prefill = if can_remember(mode) {
                if let Some(account) = trusted_host.and_then(|host| account(&local, host, mode)) {
                    match tokio::task::spawn_blocking(move || keychain::load(&account)).await {
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
            let (tx, rx) = tokio::sync::oneshot::channel();
            if attempt
                .events
                .send(UiEvent::ClientNeedsPin {
                    generation: attempt.generation,
                    mode,
                    prefill,
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
                            *pending.lock().unwrap() = Some(PendingAuthentication { mode, answer });
                        }
                        let _ = pin_tx.send(value);
                    }
                }
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn prompting_keeps_credentials_in_memory_until_success_and_cancellation_discards_them() {
        for cancel in [false, true] {
            let channels = Arc::new(Mutex::new(ClientChannels::default()));
            let (events, rx) = std::sync::mpsc::channel();
            let pending: Pending = Default::default();
            let attempt = ClientAttempt {
                generation: 0,
                started: std::time::Instant::now(),
                recovery: Default::default(),
                channels,
                events,
                sessions: Default::default(),
            };
            let (pin_tx, mut pin_rx) = tokio::sync::oneshot::channel();
            prompt(attempt, "local".into(), None, pending.clone())(
                AuthenticationMode::Password,
                pin_tx,
            );
            let tx = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(UiEvent::ClientNeedsPin { tx, prefill, .. }) = rx.try_recv() {
                        assert!(prefill.is_none());
                        break tx;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(pending.lock().unwrap().is_none());
            if cancel {
                drop(tx);
                assert!(pin_rx.await.is_err());
                assert!(pending.lock().unwrap().is_none());
            } else {
                tx.send(AuthenticationAnswer {
                    value: "test-password".into(),
                    remember: true,
                })
                .unwrap();
                assert_eq!(
                    pin_rx.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                );
                assert_eq!(pin_rx.await.unwrap(), "test-password");
                let submitted = pending.lock().unwrap().take().unwrap();
                assert!(
                    submitted
                        .commit_with(
                            "local",
                            [1; 32],
                            |_, _| {
                                Err(removent_core::CoreError::SecureStore("test denial".into()))
                            },
                            |_| Ok(())
                        )
                        .is_err()
                );
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "uses an isolated synthetic item in the real login Keychain"]
    async fn native_credentials_round_trip_in_keychain() {
        let local = format!("qa-{}", std::process::id());
        let host = [71; 32];
        for mode in [
            AuthenticationMode::Password,
            AuthenticationMode::SystemAccount,
        ] {
            let key = account(&local, host, mode).unwrap();
            let value = if mode == AuthenticationMode::SystemAccount {
                removent_proto::SystemCredentials {
                    username: "qa-owner".into(),
                    password: "synthetic-password".into(),
                }
                .encode()
            } else {
                "synthetic-password".into()
            };
            PendingAuthentication {
                mode,
                answer: AuthenticationAnswer {
                    value: value.clone(),
                    remember: true,
                },
            }
            .commit(local.clone(), host)
            .await
            .unwrap();
            let loaded = keychain::load(&key).unwrap();
            PendingAuthentication {
                mode,
                answer: AuthenticationAnswer {
                    value: value.clone(),
                    remember: false,
                },
            }
            .commit(local.clone(), host)
            .await
            .unwrap();
            assert_eq!(loaded, Some(value));
            assert_eq!(keychain::load(&key).unwrap(), None);
        }
    }

    #[test]
    fn credentials_are_isolated_by_local_host_and_method_and_debug_is_redacted() {
        let key = account("local", [1; 32], AuthenticationMode::Password).unwrap();
        assert_ne!(
            Some(key.clone()),
            account("other", [1; 32], AuthenticationMode::Password)
        );
        assert_ne!(
            Some(key.clone()),
            account("local", [2; 32], AuthenticationMode::Password)
        );
        assert_ne!(
            Some(key),
            account("local", [1; 32], AuthenticationMode::SystemAccount)
        );
        for mode in [
            AuthenticationMode::PairingCode,
            AuthenticationMode::Otp,
            AuthenticationMode::None,
        ] {
            assert!(account("local", [1; 32], mode).is_none());
            assert!(!can_remember(mode));
        }
        let answer = AuthenticationAnswer {
            value: "secret".into(),
            remember: true,
        };
        assert!(!format!("{answer:?}").contains("secret"));
    }

    #[test]
    fn successful_authentication_stores_or_forgets_only_the_selected_method() {
        for mode in [
            AuthenticationMode::Password,
            AuthenticationMode::SystemAccount,
        ] {
            for remember in [false, true] {
                let value = if mode == AuthenticationMode::SystemAccount {
                    removent_proto::SystemCredentials {
                        username: "owner".into(),
                        password: "test-password".into(),
                    }
                    .encode()
                } else {
                    "test-password".into()
                };
                let expected = value.clone();
                PendingAuthentication {
                    mode,
                    answer: AuthenticationAnswer { value, remember },
                }
                .commit_with(
                    "local",
                    [1; 32],
                    |key, value| {
                        assert!(remember);
                        assert_eq!(Some(key.to_owned()), account("local", [1; 32], mode));
                        assert_eq!(value, expected);
                        Ok(())
                    },
                    |key| {
                        assert!(!remember);
                        assert_eq!(Some(key.to_owned()), account("local", [1; 32], mode));
                        Ok(())
                    },
                )
                .unwrap();
            }
        }
    }
}
