use super::*;

/// A local Keychain snapshot. The scope cannot be changed by the form.
#[derive(Clone, Debug)]
pub struct NativeCredential {
    local: String,
    host: [u8; 32],
    pub mode: AuthenticationMode,
    pub answer: AuthenticationAnswer,
}

#[derive(Clone, Debug)]
pub struct NativeCredentialEdit {
    pub original: NativeCredential,
    pub answer: AuthenticationAnswer,
}

impl Engine {
    // Called on a background executor: DNS, file locks and Keychain can block.
    fn native_credential_host(&self, request: &ConnectionRequest) -> Result<Option<[u8; 32]>> {
        if request.protocol != ConnectionProtocol::Removent || request.pairing_code.is_some() {
            return Ok(None);
        }
        let (destination, supplied) = if let Some(route) = &request.relay {
            let route = route.validated(false).map_err(anyhow::Error::msg)?;
            (
                format!("relay:{}:{}", route.endpoint, request.address.host),
                route.optional_host_pin(),
            )
        } else {
            let address = self.rt.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    tokio::net::lookup_host((request.address.host.as_str(), request.address.port)),
                )
                .await?
                .map_err(anyhow::Error::from)?
                .next()
                .ok_or_else(|| anyhow::anyhow!("No addresses found for {}", request.address.host))
            })?;
            (address.to_string(), None)
        };
        let trust =
            removent_client::host_pins::for_connection(&self.paths, &destination, supplied, false)?;
        // Imported pins alone do not authorize editing another computer's credentials.
        Ok(trust.expected.filter(|_| !trust.needs_confirmation))
    }

    pub fn native_credentials(&self, request: &ConnectionRequest) -> Result<Vec<NativeCredential>> {
        if !can_remember(AuthenticationMode::Password) {
            return Ok(Vec::new());
        }
        let Some(host) = self.native_credential_host(request)? else {
            return Ok(Vec::new());
        };
        let local = self.identity()?.fingerprint_hex();
        let mut credentials = Vec::new();
        for mode in [
            AuthenticationMode::SystemAccount,
            AuthenticationMode::Password,
        ] {
            let key = account(&local, host, mode).expect("rememberable method");
            if let Some(value) = keychain::load(&key)? {
                credentials.push(NativeCredential {
                    local: local.clone(),
                    host,
                    mode,
                    answer: AuthenticationAnswer {
                        value,
                        remember: true,
                    },
                });
            }
        }
        Ok(credentials)
    }

    pub fn save_native_credentials(
        &self,
        request: &ConnectionRequest,
        edits: &[NativeCredentialEdit],
    ) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let local = self.identity()?.fingerprint_hex();
        let host = self.native_credential_host(request)?;
        // Validate every edit before writing anything. Secrets never follow an endpoint edit.
        for edit in edits {
            anyhow::ensure!(
                can_remember(edit.original.mode)
                    && edit.original.local == local
                    && Some(edit.original.host) == host,
                t!("auth.credentials_destination_changed").to_string()
            );
            anyhow::ensure!(
                !edit.answer.remember || edit.original.mode.valid_input(&edit.answer.value),
                t!("auth.saved_credentials_invalid").to_string()
            );
        }
        for edit in edits {
            PendingAuthentication {
                mode: edit.original.mode,
                answer: edit.answer.clone(),
                reused: false,
            }
            .commit_with(
                &local,
                edit.original.host,
                keychain::store,
                keychain::delete,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl NativeCredential {
    pub(crate) fn fixture(mode: AuthenticationMode, value: String) -> Self {
        Self {
            local: "fixture".into(),
            host: [1; 32],
            mode,
            answer: AuthenticationAnswer {
                value,
                remember: true,
            },
        }
    }
}
