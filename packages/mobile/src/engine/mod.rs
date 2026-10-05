use crate::request::{Input, Request};
use anyhow::{Context, Result, ensure};
use removent_client::{
    DecodedFrame,
    connection::{ConnectionProgress, ConnectionProtocol, ConnectionRequest},
    saved::{SavedConnection, SavedConnections},
};
use removent_core::{DataPaths, DeviceIdentity, MemoryClipboard, TextClipboard};
use removent_proto::{Caps, ControlMsg};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Validate {
        request: Request,
        credential_id: Option<String>,
    },
    Connect {
        request: Request,
        credential_id: Option<String>,
    },
    ConnectSaved {
        id: String,
        audio: bool,
        clipboard: bool,
    },
    Disconnect,
    PairingAddress {
        generation: u64,
        host: String,
        port: u16,
        error: Option<String>,
    },
    ConfirmCertificate {
        generation: u64,
        accept: bool,
    },
    Pin {
        generation: u64,
        pin: String,
    },
    Input {
        generation: u64,
        event: Input,
    },
    Clipboard {
        generation: u64,
        text: String,
    },
    ReadClipboard {
        generation: u64,
    },
    Keyframe {
        generation: u64,
    },
    Sync {
        command: removent_client::cloud_sync::Command,
    },
    List,
    Save {
        request: Request,
        name: String,
        id: Option<String>,
        credential_id: Option<String>,
    },
    Delete {
        id: String,
    },
    Scan {
        vnc: bool,
        rdp: bool,
    },
}

pub struct Engine {
    runtime: Option<tokio::runtime::Runtime>,
    pub shared: Arc<Shared>,
    task: Option<tokio::task::JoinHandle<()>>,
    scans: Vec<tokio::task::JoinHandle<()>>,
}

pub struct Shared {
    paths: DataPaths,
    identity: DeviceIdentity,
    pub state: Mutex<State>,
}

#[derive(Default)]
pub struct State {
    pub generation: u64,
    pub events: VecDeque<Value>,
    pub frame: Option<DecodedFrame>,
    pub audio: VecDeque<Vec<i16>>,
    pub audio_rate: u32,
    pub audio_channels: u8,
    input: Option<InputChannel>,
    control: Option<mpsc::Sender<ControlMsg>>,
    pin: Option<oneshot::Sender<String>>,
    certificate: Option<oneshot::Sender<bool>>,
    auth_mode: removent_core::AuthenticationMode,
    #[cfg(target_os = "ios")]
    pairing_address: Option<
        oneshot::Sender<
            std::result::Result<removent_client::connection::ConnectionAddress, String>,
        >,
    >,
    clipboard: Option<Arc<MemoryClipboard>>,
}

impl State {
    fn clear_session(&mut self) {
        self.frame = None;
        self.audio.clear();
        self.input = None;
        self.control = None;
        self.pin = None;
        self.certificate = None;
        #[cfg(target_os = "ios")]
        {
            self.pairing_address = None;
        }
        self.clipboard = None;
    }
    fn event(&mut self, event: Value) {
        // UI suspension must not accumulate unbounded events. Media has separate slots.
        if self.events.len() >= 128 {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

enum InputChannel {
    Ordered(removent_client::InputSender),
    Rdp(mpsc::Sender<ControlMsg>),
}
impl InputChannel {
    fn send(&self, message: ControlMsg) -> Result<()> {
        match self {
            Self::Ordered(tx) => tx
                .try_send(message)
                .map_err(|e| anyhow::anyhow!(e.to_string())),
            Self::Rdp(tx) => tx
                .try_send(message)
                .map_err(|e| anyhow::anyhow!(e.to_string())),
        }
    }
}

#[derive(Clone)]
struct Attempt {
    shared: Arc<Shared>,
    generation: u64,
}
impl Attempt {
    fn update(&self, f: impl FnOnce(&mut State)) {
        let mut state = self.shared.state.lock().unwrap();
        if state.generation == self.generation {
            f(&mut state);
        }
    }
    fn event(&self, mut event: Value) {
        event["generation"] = self.generation.into();
        self.update(|s| s.event(event));
    }
    fn certificate_confirmation(
        &self,
        destination: String,
        relay: bool,
    ) -> removent_net::CertificateConfirmation {
        let attempt = self.clone();
        Box::new(move |_, tx| {
            attempt.update(|state| {
            state.certificate = Some(tx);
            state.event(json!({"type":"certificate", "generation":attempt.generation, "destination":destination, "relay":relay}));
        })
        })
    }
    fn progress(&self, stage: &str) {
        self.event(json!({"type":"progress", "stage":stage}));
    }
    fn progress_sink(&self) -> ConnectionProgress {
        let attempt = self.clone();
        Arc::new(move |stage| attempt.progress(&format!("{stage:?}")))
    }
    fn frame(&self, frame: DecodedFrame) {
        self.update(|s| s.frame = Some(frame));
    }
    fn finish(&self, result: Result<()>) {
        self.update(|s| {
            s.clear_session();
            s.event(json!({"generation":self.generation, "type":"closed",
                "error":result.err().map(|e| format!("{e:#}"))}));
        });
    }
}

impl Engine {
    fn request_with_credentials(
        &self,
        request: &Request,
        credential_id: Option<&str>,
    ) -> Result<ConnectionRequest> {
        let mut connection = request.connection()?;
        if let Some(id) = credential_id {
            ensure!(
                connection.password.is_empty(),
                "Choose either a saved or a new password"
            );
            let saved = SavedConnections::load(&self.shared.paths)?;
            let entry = saved
                .all()
                .iter()
                .find(|entry| entry.id == id)
                .context("Saved connection was removed")?;
            ensure!(
                same_credential_scope(entry, &connection),
                "The computer or account changed. Enter its password again."
            );
            connection.password = removent_client::keychain::load(
                entry
                    .credential_account()
                    .context("Saved password unavailable; enter it again")?,
            )?
            .context("Saved password unavailable; enter it again")?;
        }
        Ok(connection)
    }

    pub fn new(path: &str, name: &str) -> Result<Self> {
        ensure!(
            !path.is_empty() && std::path::Path::new(path).is_absolute(),
            "An absolute data directory is required"
        );
        let paths = DataPaths { root: path.into() };
        let identity = removent_core::identity::load_or_create(&paths, name)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        Ok(Self {
            runtime: Some(runtime),
            shared: Arc::new(Shared {
                paths,
                identity,
                state: Mutex::new(State::default()),
            }),
            task: None,
            scans: vec![],
        })
    }

    fn stop(&mut self) -> u64 {
        // Invalidate publication before abort: an in-flight task poll may still run.
        let mut state = self.shared.state.lock().unwrap();
        state.generation += 1;
        state.clear_session();
        state.events.clear();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        state.generation
    }

    fn connect(
        &mut self,
        request: ConnectionRequest,
        audio: bool,
        clipboard: bool,
    ) -> Result<Value> {
        let generation = self.stop();
        let attempt = Attempt {
            shared: self.shared.clone(),
            generation,
        };
        self.task = Some(self.runtime.as_ref().unwrap().spawn(async move {
            let result = run(request, audio, clipboard, attempt.clone()).await;
            attempt.finish(result);
        }));
        Ok(json!({"generation":generation}))
    }

    pub fn command(&mut self, command: Command) -> Result<Value> {
        match command {
            Command::Validate {
                request,
                credential_id,
            } => {
                self.request_with_credentials(&request, credential_id.as_deref())?;
                Ok(Value::Null)
            }
            Command::Connect {
                request,
                credential_id,
            } => self.connect(
                self.request_with_credentials(&request, credential_id.as_deref())?,
                request.audio,
                request.clipboard,
            ),
            Command::ConnectSaved {
                id,
                audio,
                clipboard,
            } => {
                let mut saved = SavedConnections::load(&self.shared.paths)?;
                let entry = saved
                    .all()
                    .iter()
                    .find(|e| e.id == id)
                    .context("Saved connection was removed")?;
                let mut request = entry.to_request();
                if entry.needs_credentials() {
                    request.password = removent_client::keychain::load(
                        entry
                            .credential_account()
                            .context("Saved password unavailable; edit this connection")?,
                    )?
                    .context("Saved password unavailable; edit this connection")?;
                }
                // Validate persisted public routing fields at the same boundary as new forms.
                if let Some(route) = &request.relay {
                    request.relay = Some(route.validated(false).map_err(anyhow::Error::msg)?);
                }
                saved.touch(&id)?;
                self.connect(request, audio, clipboard)
            }
            Command::Disconnect => Ok(json!({"generation":self.stop()})),
            Command::Sync { command } => Ok(removent_client::cloud_sync::dispatch(
                &self.shared.paths,
                command,
            )?),
            Command::List => Ok(
                json!({"connections": SavedConnections::load(&self.shared.paths)?.all(),
                "fingerprint":self.shared.identity.fingerprint_hex()}),
            ),
            Command::Save {
                request,
                name,
                id,
                credential_id,
            } => {
                ensure!(name.len() <= 256, "Name is too long");
                ensure!(
                    credential_id.is_none() || credential_id == id,
                    "Saved credentials must belong to this bookmark"
                );
                let request = self.request_with_credentials(&request, credential_id.as_deref())?;
                ensure!(
                    request.pairing_code.is_none(),
                    "Connection codes are temporary and cannot be saved"
                );
                let mut entry = SavedConnection::from_request(&request, name);
                entry.id = id.unwrap_or_default();
                let entry = SavedConnections::load(&self.shared.paths)?
                    .save_with_password(entry, &request.password)?;
                Ok(serde_json::to_value(entry)?)
            }
            Command::Delete { id } => {
                SavedConnections::load(&self.shared.paths)?.remove_with_password(&id)?;
                Ok(Value::Null)
            }
            Command::Scan { vnc, rdp } => {
                for task in self.scans.drain(..) {
                    task.abort();
                }
                for (enabled, protocol, label) in [
                    (vnc, removent_net::DiscoveryProtocol::Vnc, "vnc"),
                    (rdp, removent_net::DiscoveryProtocol::Rdp, "rdp"),
                ] {
                    if !enabled {
                        continue;
                    }
                    let shared = self.shared.clone();
                    self.scans.push(self.runtime.as_ref().unwrap().spawn(async move {
                        let Some(scanner) = removent_net::lan_scan::LanScanner::start(protocol) else { return };
                        let mut table = scanner.subscribe_table();
                        while table.changed().await.is_ok() {
                            let devices: Vec<_> = table.borrow_and_update().values().filter_map(|entry| {
                                entry.addr.map(|addr| {
                                    let address = removent_client::connection::ConnectionAddress::from_socket(addr);
                                    json!({"id":format!("{label}:{addr}"), "name":entry.name,
                                        "host":address.host, "port":address.port, "protocol":label})
                                })
                            }).collect();
                            shared.state.lock().unwrap().event(json!({"type":"scan", "protocol":label, "devices":devices}));
                        }
                    }));
                }
                Ok(Value::Null)
            }
            Command::PairingAddress {
                generation,
                host,
                port,
                error,
            } => {
                #[cfg(target_os = "ios")]
                {
                    let mut state = self.shared.state.lock().unwrap();
                    ensure!(state.generation == generation, "Connection was cancelled");
                    let address = if let Some(error) = error {
                        Err(error)
                    } else {
                        removent_client::connection::ConnectionAddress::parse(
                            &host,
                            &port.to_string(),
                        )
                        .map_err(|e| e.to_string())
                    };
                    state
                        .pairing_address
                        .take()
                        .context("No pairing lookup pending")?
                        .send(address)
                        .map_err(|_| anyhow::anyhow!("Lookup cancelled"))?;
                    Ok(Value::Null)
                }
                #[cfg(not(target_os = "ios"))]
                {
                    let _ = (generation, host, port, error);
                    anyhow::bail!("System Bonjour is only used on iOS")
                }
            }
            Command::ConfirmCertificate { generation, accept } => {
                let mut state = self.shared.state.lock().unwrap();
                ensure!(state.generation == generation, "Connection was cancelled");
                state
                    .certificate
                    .take()
                    .context("No certificate confirmation is pending")?
                    .send(accept)
                    .map_err(|_| anyhow::anyhow!("Certificate confirmation expired"))?;
                Ok(Value::Null)
            }
            Command::Pin { generation, pin } => {
                let mut s = self.shared.state.lock().unwrap();
                ensure!(
                    s.auth_mode.valid_input(&pin),
                    "Enter a valid password or six-digit code"
                );
                ensure!(s.generation == generation, "Connection was cancelled");
                s.pin
                    .take()
                    .context("No pairing request is pending")?
                    .send(pin)
                    .map_err(|_| anyhow::anyhow!("Pairing expired"))?;
                Ok(Value::Null)
            }
            Command::Input { generation, event } => {
                let messages = event.messages()?;
                let result = {
                    let s = self.shared.state.lock().unwrap();
                    ensure!(s.generation == generation, "Connection was cancelled");
                    let input = s.input.as_ref().context("Session is not ready")?;
                    messages.into_iter().try_for_each(|m| input.send(m))
                };
                if result.is_err() {
                    self.stop();
                } // Never continue with a lost key-up/button-up.
                result?;
                Ok(Value::Null)
            }
            Command::Clipboard { generation, text } => {
                ensure!(
                    text.len() <= removent_core::clip::MAX_CLIPBOARD_BYTES,
                    "Clipboard is too large"
                );
                let s = self.shared.state.lock().unwrap();
                ensure!(s.generation == generation, "Connection was cancelled");
                s.clipboard
                    .as_ref()
                    .context("Clipboard requires a Removent session")?
                    .write(&text)
                    .map_err(anyhow::Error::msg)?;
                Ok(Value::Null)
            }
            Command::ReadClipboard { generation } => {
                let s = self.shared.state.lock().unwrap();
                ensure!(s.generation == generation, "Connection was cancelled");
                Ok(
                    json!({"text":s.clipboard.as_ref().context("Clipboard is unavailable")?
                    .read().map_err(anyhow::Error::msg)?}),
                )
            }
            Command::Keyframe { generation } => {
                let s = self.shared.state.lock().unwrap();
                ensure!(s.generation == generation, "Connection was cancelled");
                s.control
                    .as_ref()
                    .context("Refresh requires a Removent session")?
                    .try_send(ControlMsg::KeyframeRequest)?;
                Ok(Value::Null)
            }
        }
    }
}

fn same_credential_scope(entry: &SavedConnection, request: &ConnectionRequest) -> bool {
    entry.protocol == request.protocol
        && entry.address() == request.address
        && entry.username == request.username
        && entry.domain == request.domain
        && entry.relay == request.relay
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
        for task in self.scans.drain(..) {
            task.abort();
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests;

mod compatibility;
mod native;
use compatibility::{addresses, run};
use native::run_native;
