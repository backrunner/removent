use removent_proto::automation::Command;
use rmcp::model::{Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Connect {
    pub saved_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub session_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cancel {
    pub session_id: String,
    pub operation_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Computer {
    pub session_id: String,
    /// Unique per operation in this session. Reuse the same ID only to recover the same operation's result.
    pub operation_id: String,
    /// Optional explicit host control revision; normally updated by resume_control.
    #[serde(default)]
    pub control_revision: Option<u64>,
    pub command: Command,
}
fn tool<T: JsonSchema>(name: &'static str, description: &'static str, read_only: bool) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(T))
        .unwrap()
        .as_object()
        .unwrap()
        .clone();
    let mut tool = Tool::new(name, description, schema);
    tool.annotations = Some(
        ToolAnnotations::new()
            .read_only(read_only)
            .destructive(!read_only)
            .idempotent(read_only)
            .open_world(true),
    );
    tool
}
pub fn all() -> Vec<Tool> {
    vec![
        tool::<Empty>(
            "removent_hosts",
            "List saved native Removent destinations and this controller's certificate fingerprint. Pair and save credentials in Removent before first MCP use.",
            true,
        ),
        tool::<Connect>(
            "removent_connect",
            "Initiate an authenticated connection to a saved destination. Returns immediately with a session ID. Inspect removent_status until connected or failed. Existing connections are reused.",
            false,
        ),
        tool::<Session>(
            "removent_status",
            "Inspect connection progress, failure and pending operation IDs. A disconnected session never reconnects automatically.",
            true,
        ),
        tool::<Session>(
            "removent_disconnect",
            "Cancel pending work and disconnect this MCP session.",
            false,
        ),
        tool::<Cancel>(
            "removent_cancel",
            "Cancel a pending computer operation. Earlier input may already have been applied; observe the remote state before continuing.",
            false,
        ),
        tool::<Computer>(
            "computer",
            "Operate the remote computer. Viewer input or local host input pauses mutations. Native actions require local_input_monitor.state=ready; if unavailable, resolve its reason on the host and reconnect. Query control_status even during an active action; after human input ends, use resume_control with its latest revision, then take new observations. Do not automatically resume while a person is using either computer. First call capabilities, then state to list app targets/displays. snapshot accepts frontmost, app:<pid> or bundle ID and returns native element references; screenshot returns an image and observation ID. Click/move/drag/scroll coordinates are pixels in that screenshot. Key chords use Meta+Shift+A or named keys. type_text inserts Unicode without clipboard. element_action names come from the snapshot. References and coordinates expire; refresh after changes. Successful mutations acknowledge native API completion; verify visible effects with a new snapshot or screenshot. Browser DOM tools are available only when a Removent extension bridge is connected; currently unavailable.",
            false,
        ),
    ]
}
