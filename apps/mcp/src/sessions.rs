use removent_client::automation::Connection;
use removent_core::{DataPaths, DeviceIdentity, Settings};
use removent_proto::automation::{Command, Failure, Request};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct Sessions {
    pub paths: DataPaths,
    settings: Settings,
    identity: DeviceIdentity,
    entries: Mutex<HashMap<String, Entry>>,
}
struct Entry {
    destination: String,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    connection: Option<Arc<Connection>>,
    error: Option<String>,
    active: HashMap<String, CancellationToken>,
}
impl Sessions {
    pub fn new(paths: DataPaths, settings: Settings, identity: DeviceIdentity) -> Self {
        Self {
            paths,
            settings,
            identity,
            entries: Default::default(),
        }
    }
    pub fn hosts(&self) -> Result<Value, Failure> {
        let saved = removent_client::saved::SavedConnections::load(&self.paths)
            .map_err(|_| Failure::new("storage_error", "Cannot read saved connections"))?;
        Ok(
            json!({"client_fingerprint":self.identity.fingerprint_hex(),"hosts":saved.all().iter().filter(|s|s.protocol==removent_client::connection::ConnectionProtocol::Removent).map(|s|json!({"saved_id":s.id,"name":s.display_name(),"address":s.address().to_string(),"relay":s.relay.is_some(),"credentials_review_required":s.credentials_review_required})).collect::<Vec<_>>()}),
        )
    }
    pub fn connect(self: &Arc<Self>, saved_id: String) -> Result<Value, Failure> {
        let mut entries = self.entries.lock().unwrap();
        if let Some((id, _)) = entries.iter().find(|(_, e)| e.destination == saved_id) {
            return Ok(json!({"session_id":id,"status":"existing","next":"removent_status"}));
        }
        if entries.len() >= 8 {
            return Err(Failure::new(
                "session_limit",
                "Disconnect an existing session first",
            ));
        }
        let id = hex::encode(rand::random::<[u8; 16]>());
        let cancel = CancellationToken::new();
        entries.insert(
            id.clone(),
            Entry {
                destination: saved_id.clone(),
                cancel: cancel.clone(),
                task: None,
                connection: None,
                error: None,
                active: Default::default(),
            },
        );
        let owner = self.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let connected = tokio::select! {biased;_=cancel.cancelled()=>return,
            result=Connection::connect_saved(&owner.paths,&owner.identity,&owner.settings,&saved_id)=>result};
            let mut entries = owner.entries.lock().unwrap();
            if let Some(entry) = entries.get_mut(&task_id) {
                match connected {
                    Ok(connection) => entry.connection = Some(Arc::new(connection)),
                    Err(error) => entry.error = Some(error.to_string()),
                }
            }
        });
        entries.get_mut(&id).unwrap().task = Some(task);
        Ok(json!({"session_id":id,"status":"connecting","next":"removent_status"}))
    }
    pub async fn status(&self, id: &str) -> Result<Value, Failure> {
        let (connection, error, active) = {
            let entries = self.entries.lock().unwrap();
            let entry = entries.get(id).ok_or_else(missing)?;
            (
                entry.connection.clone(),
                entry.error.clone(),
                entry.active.keys().cloned().collect::<Vec<_>>(),
            )
        };
        let status = if error.is_some() {
            "failed"
        } else if let Some(connection) = &connection {
            if connection.alive().await {
                "connected"
            } else {
                "disconnected"
            }
        } else {
            "connecting"
        };
        let control = if let Some(connection) = &connection {
            if status == "connected"
                && connection.features() & removent_proto::feature_bits::AUTOMATION_HANDOVER != 0
            {
                let request = Request {
                    version: 1,
                    operation_id: format!("status-{:x}", rand::random::<u64>()),
                    control_revision: None,
                    command: Command::ControlStatus {},
                };
                match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    connection.call(&request),
                )
                .await
                {
                    Ok(Ok(response)) => match response.result {
                        Ok(state) => state,
                        Err(error) => json!({"error":error}),
                    },
                    _ => {
                        json!({"error":{"code":"unavailable","message":"Control status is temporarily unavailable"}})
                    }
                }
            } else {
                Value::Null
            }
        } else {
            Value::Null
        };
        Ok(
            json!({"session_id":id,"status":status,"error":error,"control":control,
            "automation_protocol":connection.as_ref().map(|c|c.features()&removent_proto::feature_bits::AUTOMATION!=0),
            "shared_connection":connection.as_ref().map(|c|c.shared()),"active_operations":active,
            "next":if status=="connected"{"computer: capabilities"}else{"removent_status"}}),
        )
    }
    pub fn disconnect(&self, id: &str) -> Result<Value, Failure> {
        let entry = self
            .entries
            .lock()
            .unwrap()
            .remove(id)
            .ok_or_else(missing)?;
        close(entry);
        Ok(json!({"session_id":id,"status":"disconnected"}))
    }
    pub fn cancel(&self, id: &str, operation: &str) -> Result<Value, Failure> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(id).ok_or_else(missing)?;
        let token = entry
            .active
            .get(operation)
            .ok_or_else(|| Failure::new("operation_missing", "Operation is no longer pending"))?;
        token.cancel();
        Ok(
            json!({"status":"cancellation_requested","operation_id":operation,"outcome":"earlier_input_may_have_been_applied"}),
        )
    }
    pub async fn execute(
        self: &Arc<Self>,
        id: String,
        operation_id: String,
        command: Command,
        control_revision: Option<u64>,
        request_cancel: CancellationToken,
    ) -> Result<Value, Failure> {
        let request = Request {
            control_revision,
            version: 1,
            operation_id: operation_id.clone(),
            command,
        };
        request.validate()?;
        let status_only = matches!(request.command, Command::ControlStatus {});
        let (connection, cancel) = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.get_mut(&id).ok_or_else(missing)?;
            let connection = entry
                .connection
                .clone()
                .ok_or_else(|| Failure::new("not_connected", "Wait for a connected session"))?;
            if connection.closed() {
                return Err(Failure::new(
                    "session_closed",
                    "Reconnect explicitly; old operation and element IDs cannot be reused",
                ));
            }
            if connection.features() & removent_proto::feature_bits::AUTOMATION == 0 {
                return Err(Failure::new(
                    "unsupported",
                    "Update the remote Removent host to support Computer Use",
                ));
            }
            if !status_only && entry.active.contains_key(&operation_id) {
                return Err(Failure::new(
                    "operation_pending",
                    "This operation is already pending",
                ));
            }
            if !status_only && !entry.active.is_empty() {
                return Err(Failure::new(
                    "busy",
                    "Wait for the current computer operation to finish",
                ));
            }
            let cancel = entry.cancel.child_token();
            if !status_only {
                entry.active.insert(operation_id.clone(), cancel.clone());
            }
            (connection, cancel)
        };
        let _guard = (!status_only).then(|| OperationGuard {
            owner: self.clone(),
            session: id,
            operation: operation_id,
            cancel: cancel.clone(),
        });
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(Failure::new("cancelled","Operation cancelled; observe state before deciding what to do next")),
            _=request_cancel.cancelled()=>Err(Failure::new("cancelled","MCP request cancelled; earlier input may have been applied")),
            response=connection.call(&request)=>response.map_err(|_|Failure::new("outcome_unknown","Transport interrupted; do not repeat a mutation with a new operation ID"))?.result,
        }
    }
    pub fn shutdown(&self) {
        for (_, entry) in self.entries.lock().unwrap().drain() {
            close(entry);
        }
    }
}
fn missing() -> Failure {
    Failure::new("session_missing", "Unknown session ID")
}
fn close(entry: Entry) {
    entry.cancel.cancel();
    if let Some(task) = entry.task {
        task.abort();
    }
    if let Some(connection) = entry.connection {
        connection.disconnect();
    }
}
struct OperationGuard {
    owner: Arc<Sessions>,
    session: String,
    operation: String,
    cancel: CancellationToken,
}
impl Drop for OperationGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(entry) = self.owner.entries.lock().unwrap().get_mut(&self.session) {
            entry.active.remove(&self.operation);
        }
    }
}
