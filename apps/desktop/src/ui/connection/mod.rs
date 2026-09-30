//! Two-level connection dialog: protocol, then session-specific connection details.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Render, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    input::{InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
};
use removent_client::connection::{
    AddressError, ConnectionAddress, ConnectionProtocol, ConnectionRequest, ConnectionStage,
    RelayRoute, RelayTransport,
};
use removent_client::saved::SavedConnection;
use rust_i18n::t;
use std::time::{Duration, Instant};

use super::widgets::{Button, form_input, icon_16, section_header};

gpui::actions!(connection, [ConnectionTab, ConnectionTabPrev]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionDialogEvent {
    Submit,
    Save,
    Cancel,
    Close,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Step {
    #[default]
    Protocol,
    Details(ConnectionProtocol),
}

pub struct ConnectionDialog {
    step: Step,
    saved_id: Option<String>,
    save_warning: Option<String>,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    domain: Entity<InputState>,
    accept_invalid_certificate: bool,
    via_relay: bool,
    relay_endpoint: Entity<InputState>,
    relay_transport: RelayTransport,
    relay_pin: Entity<InputState>,
    host_pin: Entity<InputState>,
    relay_choices: Vec<SavedConnection>,
    relay_secret_scope: Option<(String, String, String, RelayTransport)>,
    relay_secret_task: Option<gpui::Task<()>>,
    connecting: bool,
    stage: ConnectionStage,
    started: Option<Instant>,
    elapsed_task: Option<gpui::Task<()>>,
    retry: bool,
    cancelled: bool,
    obscured: bool,
    restore_focus: Option<FocusHandle>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<ConnectionDialogEvent> for ConnectionDialog {}

impl Focusable for ConnectionDialog {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn stage_label(stage: ConnectionStage) -> String {
    t!(match stage {
        ConnectionStage::Resolving => "connection.stage_resolving",
        ConnectionStage::Connecting => "connection.stage_connecting",
        ConnectionStage::Negotiating => "connection.stage_negotiating",
        ConnectionStage::Pairing => "connection.stage_pairing",
        ConnectionStage::Authenticating => "connection.stage_authenticating",
        ConnectionStage::PreparingDesktop => "connection.stage_desktop",
    })
    .to_string()
}

mod form;
mod render;
#[cfg(test)]
mod tests;
